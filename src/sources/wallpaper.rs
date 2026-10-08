//! Wallpaper (#145, docs/design.md Wallpaper): Kanade selects, awww renders. `kanade wallpaper set
//! <path>` and the Launcher's wallpapers (`@`) both ask `set`, which hands the image to `awww img`.
//!
//! awww draws through its daemon, which Kanade runs as a holder (ADR 0011) while the Module is on,
//! only through setpriv, so it dies with Kanade even killed; on its next start awww shows the last
//! image again from its own cache. When a daemon already answers, like one the compositor started,
//! Kanade sets on that one instead, setpriv or not.
//!
//! `awww img` decodes and scales the image itself, which takes up to a tenth of a second, so it runs
//! on a thread of its own, one image at a time in the order asked, never on the draw thread, and is
//! killed after `LIMIT` so one that hangs holds up no set after it. `set` answers at once with a
//! serial; `kanade` waits on `status` for how it went.

use std::collections::VecDeque;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use super::wake;
use crate::{config, supervise};

pub const AWWW: &str = "awww";
pub const DAEMON: &str = "awww-daemon";

// where the wallpapers are without `wallpaper.directory`, under the home directory
const DIRECTORY: &str = "Pictures/Wallpapers";

// the files listed as wallpapers, by extension: what awww decodes
const IMAGES: &[&str] = &[
    "bmp", "ff", "gif", "jpeg", "jpg", "pbm", "pgm", "png", "pnm", "ppm", "svg", "tga", "tif",
    "tiff", "webp",
];

// how long awww may take to show an image, well over the tenth of a second it takes a large one
const LIMIT: Duration = Duration::from_secs(10);

// how many sets `status` says how they went, so a `kanade` waiting on one finds it after others
const RECENT: usize = 16;

// what is told how a set went, like the Launcher, which closes or says it was not set
pub type Done = Box<dyn FnOnce(Result<(), Unset>) + Send>;

// why a wallpaper was not set
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unset {
    NoImage(PathBuf),
    NoAwww,
    NoDaemon,

    // awww took longer than this to show it, so it was stopped
    Stuck(Duration),

    // awww would not show it, in its words
    Refused(String),
}

impl Unset {
    // in a few words, for a row
    pub fn brief(&self) -> &'static str {
        match self {
            Unset::NoImage(_) => "no such image",
            Unset::NoAwww => "awww not found",
            Unset::NoDaemon => "awww-daemon is not running",
            Unset::Stuck(_) => "awww did not answer",
            Unset::Refused(_) => "awww could not show it",
        }
    }
}

impl fmt::Display for Unset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Unset::NoImage(path) => write!(f, "no image at {}", path.display()),
            Unset::NoAwww => write!(f, "{AWWW} not found; install awww to set the wallpaper"),
            Unset::NoDaemon => write!(f, "{DAEMON} is not running"),
            Unset::Stuck(limit) => write!(
                f,
                "{AWWW} did not show it within {}s, so it was stopped",
                limit.as_secs()
            ),
            Unset::Refused(why) => f.write_str(why),
        }
    }
}

struct Job {
    serial: u64,
    path: PathBuf,
    done: Option<Done>,
}

#[derive(Default)]
struct Wallpaper {
    // the image Kanade last set, none until one is
    current: Option<PathBuf>,

    // the serials given out since Kanade started, never given again
    issued: u64,

    // asked and not yet done, oldest first
    pending: Vec<u64>,

    // how the newest sets went, newest last
    done: VecDeque<(u64, Result<(), Unset>)>,
}

static WALLPAPER: Mutex<Wallpaper> = Mutex::new(Wallpaper {
    current: None,
    issued: 0,
    pending: Vec::new(),
    done: VecDeque::new(),
});

// the sets waiting for `serve`, which outlive a restart of it
static QUEUE: LazyLock<(Sender<Job>, Mutex<Receiver<Job>>)> = LazyLock::new(|| {
    let (send, receive) = mpsc::channel();
    (send, Mutex::new(receive))
});

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

// what `kanade wallpaper` asks
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Set(PathBuf),
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["set", path] if !path.is_empty() => Some(Request::Set(PathBuf::from(path))),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

/*
 * asks awww to show the image at `path`, answering at once with its serial; `done` is told how it
 * went. Refused at once when there is no file there or no awww to ask
 */
pub fn set(path: PathBuf, done: Option<Done>) -> Result<u64, Unset> {
    if !path.is_file() {
        return Err(Unset::NoImage(path));
    }

    if !wake::found(AWWW) {
        return Err(Unset::NoAwww);
    }

    let serial = {
        let mut wallpaper = lock(&WALLPAPER);

        wallpaper.issued += 1;
        let serial = wallpaper.issued;
        wallpaper.pending.push(serial);

        serial
    };

    // the receiver lives in the static, so sending never fails
    drop(QUEUE.0.send(Job { serial, path, done }));

    Ok(serial)
}

// does what `request` asks: a set answered as setting, its serial to wait on through `status`
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Set(path) if !path.is_absolute() => Err(format!(
            "{} is not an absolute path, which the shell needs",
            path.display()
        )),
        Request::Set(path) => set(path, None)
            .map(|serial| format!("setting #{serial}"))
            .map_err(|unset| unset.to_string()),
        Request::Status => Ok(lock(&WALLPAPER).status().to_string()),
    }
}

// what the shell answers a set: its serial, to wait on through `status`
pub fn setting(reply: &str) -> Option<u64> {
    reply.strip_prefix("setting #")?.parse().ok()
}

// the image Kanade last set, none until it sets one
pub fn current() -> Option<PathBuf> {
    lock(&WALLPAPER).current.clone()
}

// for `kanade status`
pub fn status() -> String {
    match current() {
        Some(path) => format!("wallpaper: {}", path.display()),
        None => String::from("wallpaper: none set since start"),
    }
}

impl Wallpaper {
    fn status(&self) -> Status {
        Status {
            current: self.current.clone(),
            pending: self.pending.clone(),
            done: self
                .done
                .iter()
                .map(|(serial, done)| (*serial, done.as_ref().err().map(Unset::to_string)))
                .collect(),
        }
    }
}

/*
 * how the sets stand, as `status` says it: the image Kanade last set, then a line for each set
 * still pending and for each of the newest done; `kanade` parses it back to wait on a set
 */
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub current: Option<PathBuf>,
    pub pending: Vec<u64>,

    // each with why it failed, none when it was set
    pub done: Vec<(u64, Option<String>)>,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.current {
            Some(path) => write!(f, "current {}", path.display())?,
            None => write!(f, "none")?,
        }

        for serial in &self.pending {
            write!(f, "\nsetting #{serial}")?;
        }

        for (serial, failed) in &self.done {
            match failed {
                Some(why) => write!(f, "\n#{serial} failed: {}", why.replace('\n', "; "))?,
                None => write!(f, "\n#{serial} set")?,
            }
        }

        Ok(())
    }
}

impl Status {
    pub fn parse(text: &str) -> Option<Status> {
        let serial = |text: &str| text.strip_prefix('#')?.parse::<u64>().ok();
        let mut lines = text.lines();

        let current = match lines.next()? {
            "none" => None,
            line => Some(PathBuf::from(line.strip_prefix("current ")?)),
        };

        let mut status = Status {
            current,
            pending: Vec::new(),
            done: Vec::new(),
        };

        for line in lines {
            if let Some(number) = line.strip_prefix("setting ") {
                status.pending.push(serial(number)?);
            } else if let Some(number) = line.strip_suffix(" set") {
                status.done.push((serial(number)?, None));
            } else {
                let (number, why) = line.split_once(" failed: ")?;
                status.done.push((serial(number)?, Some(why.to_owned())));
            }
        }

        Some(status)
    }

    // where the set that `serial` names stands
    pub fn settled(&self, serial: u64) -> Settled {
        if self.pending.contains(&serial) {
            return Settled::Waiting;
        }

        match self.done.iter().find(|(done, _)| *done == serial) {
            Some((_, None)) => Settled::Set,
            Some((_, Some(why))) => Settled::Failed(why.clone()),
            None => Settled::Lost(format!(
                "the shell no longer knows how wallpaper #{serial} went; it says \"{}\"",
                self.to_string().replace('\n', "; ")
            )),
        }
    }
}

// where a set stands, as `status` says
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    Waiting,
    Set,
    Failed(String),

    // so many sets came after it that how it went is forgotten
    Lost(String),
}

/*
 * shows each image asked of `set`, in order, through `awww img`; runs as long as Kanade, so each
 * awww dies with it
 */
pub fn serve() {
    loop {
        let job = lock(&QUEUE.1).recv();

        // the sender lives in the static, so the queue never closes
        let Ok(Job { serial, path, done }) = job else {
            return;
        };

        let shown = show(&[AWWW], &path, LIMIT);

        if let Err(why) = &shown {
            eprintln!(
                "kanade: cannot set the wallpaper to {} ({why})",
                path.display()
            );
        }

        {
            let mut wallpaper = lock(&WALLPAPER);

            wallpaper.pending.retain(|pending| *pending != serial);

            if shown.is_ok() {
                wallpaper.current = Some(path);
            }

            if wallpaper.done.len() == RECENT {
                wallpaper.done.pop_front();
            }
            wallpaper.done.push_back((serial, shown.clone()));
        }

        if let Some(done) = done {
            done(shown);
        }
    }
}

/*
 * `awww img` through `awww`, the program and any arguments before its own, killed after `limit`;
 * says that the daemon is not running rather than awww's own words for it
 */
fn show(awww: &[&str], path: &Path, limit: Duration) -> Result<(), Unset> {
    let text = path.to_str().ok_or_else(|| {
        Unset::Refused(format!("{} is not UTF-8, which awww needs", path.display()))
    })?;

    let (program, before) = awww.split_first().expect("a program");
    let act = |args: &[&str]| {
        let args: Vec<&str> = before.iter().chain(args).copied().collect();
        wake::act_within(program, &args, limit)
    };

    act(&["img", text]).map_err(|failed| match failed {
        wake::Failed::Overran(limit) => Unset::Stuck(limit),
        wake::Failed::Said(why) => match act(&["query"]) {
            Ok(()) => Unset::Refused(why),
            Err(_) if !wake::found(program) => Unset::NoAwww,
            Err(_) => Unset::NoDaemon,
        },
    })
}

// who runs the daemon the wallpaper is set on
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Daemon {
    // one that already answers, like one the compositor started
    Theirs,

    // Kanade, through setpriv
    Kanade,

    // nobody: without setpriv one Kanade ran could outlive it
    Nobody,
}

fn daemon(answers: bool, guards: bool) -> Daemon {
    match (answers, guards) {
        (true, _) => Daemon::Theirs,
        (false, true) => Daemon::Kanade,
        (false, false) => Daemon::Nobody,
    }
}

/*
 * runs awww's daemon while Kanade does, again after a backoff when it ends; none when one already
 * answers, which Kanade then sets on, nor without setpriv
 */
pub fn follow() {
    let answers = wake::act_within(AWWW, &["query"], LIMIT).is_ok();

    match daemon(answers, wake::guards()) {
        Daemon::Theirs => {
            eprintln!("kanade: {DAEMON} already runs, so the wallpaper is set on that one");
            return;
        }
        Daemon::Nobody => {
            let why = format!(
                "{} not found, so {DAEMON} could outlive Kanade; no wallpaper unless one runs \
                 already",
                wake::SETPRIV
            );

            eprintln!("kanade: {why}");
            supervise::stopped("wallpaper", why);
            return;
        }
        Daemon::Kanade => {}
    }

    // what it prints only goes to stdout, read until it ends
    let error = wake::run_guarded(DAEMON, &["--quiet"], &wake::Stop::default(), |output| {
        for line in output.lines() {
            if let Err(error) = line {
                return error;
            }
        }

        io::Error::other("it exited")
    });

    // nothing stops it
    let Some(error) = error else { return };

    let why = format!("cannot run {DAEMON} ({error}), no wallpaper");

    eprintln!("kanade: {why}");
    supervise::stopped("wallpaper", why);
}

// the directory the Launcher lists wallpapers from: `wallpaper.directory`, else ~/Pictures/Wallpapers
pub fn directory() -> Option<PathBuf> {
    config::get()
        .wallpapers
        .as_ref()
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(DIRECTORY)))
}

// the directory's listing, kept until the directory changes
#[allow(clippy::type_complexity)]
static LISTED: Mutex<Option<(PathBuf, SystemTime, Arc<Vec<PathBuf>>)>> = Mutex::new(None);

/*
 * the images in `directory()`, by name; listed again only once the directory changed, so each key
 * typed in the Launcher costs one stat
 */
pub fn images() -> Arc<Vec<PathBuf>> {
    let Some(directory) = directory() else {
        return Arc::default();
    };

    let Ok(modified) = fs::metadata(&directory).and_then(|meta| meta.modified()) else {
        return Arc::default();
    };

    let mut listed = lock(&LISTED);

    if let Some((at, when, images)) = &*listed
        && *at == directory
        && *when == modified
    {
        return images.clone();
    }

    let images = Arc::new(list(&directory));
    *listed = Some((directory, modified, images.clone()));

    images
}

// the images in `directory`, not below it, by name whatever the case; hidden ones left out
fn list(directory: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(directory) else {
        return Vec::new();
    };

    let mut images: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| image(path) && path.is_file())
        .collect();

    images.sort_by_cached_key(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
    });

    images
}

// whether the file's name says it is an image awww decodes: not hidden, UTF-8 and a known extension
fn image(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };

    !name.starts_with('.')
        && path
            .extension()
            .and_then(OsStr::to_str)
            .is_some_and(|extension| IMAGES.contains(&extension.to_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_takes_one_path_and_status_nothing() {
        assert_eq!(
            Request::parse(&["set", "a.png"]),
            Some(Request::Set(PathBuf::from("a.png")))
        );
        assert_eq!(Request::parse(&["status"]), Some(Request::Status));

        for words in [
            &[][..],
            &["set"],
            &["set", ""],
            &["set", "a.png", "b.png"],
            &["status", "now"],
            &["get"],
        ] {
            assert_eq!(Request::parse(words), None, "{words:?}");
        }
    }

    #[test]
    fn a_relative_path_or_no_file_is_refused_at_once() {
        assert!(request(Request::Set(PathBuf::from("a.png"))).is_err());

        let missing = request(Request::Set(PathBuf::from("/kanade/no/such/image.png")));
        assert_eq!(
            missing,
            Err(String::from("no image at /kanade/no/such/image.png"))
        );
    }

    #[test]
    fn status_round_trips() {
        for status in [
            Status {
                current: None,
                pending: Vec::new(),
                done: Vec::new(),
            },
            Status {
                current: Some(PathBuf::from("/w/sea shore.png")),
                pending: vec![4, 5],
                done: vec![
                    (2, None),
                    (3, Some(String::from("awww-daemon is not running"))),
                ],
            },
        ] {
            assert_eq!(Status::parse(&status.to_string()), Some(status));
        }

        assert_eq!(Status::parse(""), None);
        assert_eq!(Status::parse("on"), None);
        assert_eq!(Status::parse("none\n#x set"), None);
    }

    #[test]
    fn a_set_settles_once_done_or_is_lost_once_forgotten() {
        let status = Status {
            current: Some(PathBuf::from("/w/a.png")),
            pending: vec![5],
            done: vec![(3, None), (4, Some(String::from("bad image")))],
        };

        assert_eq!(status.settled(5), Settled::Waiting);
        assert_eq!(status.settled(3), Settled::Set);
        assert_eq!(
            status.settled(4),
            Settled::Failed(String::from("bad image"))
        );
        assert!(matches!(status.settled(1), Settled::Lost(_)));
    }

    #[test]
    fn kanade_runs_the_daemon_only_through_setpriv_and_when_none_answers() {
        assert_eq!(daemon(true, true), Daemon::Theirs);
        assert_eq!(daemon(true, false), Daemon::Theirs);
        assert_eq!(daemon(false, true), Daemon::Kanade);
        assert_eq!(daemon(false, false), Daemon::Nobody);
    }

    // a stand-in for awww: `img` of an image named hang never exits, any other one is shown
    const AWWW_HANGS: &[&str] = &[
        "sh",
        "-c",
        r#"case "$1 $2" in "img "*hang*) exec sleep 60 ;; "img "*|query) exit 0 ;; esac; exit 1"#,
        "awww",
    ];

    #[test]
    fn an_awww_that_hangs_is_stopped_and_the_next_image_is_shown() {
        let limit = Duration::from_millis(300);
        let started = std::time::Instant::now();

        assert_eq!(
            show(AWWW_HANGS, Path::new("/w/hang.png"), limit),
            Err(Unset::Stuck(limit))
        );
        assert!(started.elapsed() < Duration::from_secs(5));

        assert_eq!(show(AWWW_HANGS, Path::new("/w/sea.png"), limit), Ok(()));
    }

    #[test]
    fn the_reply_to_a_set_names_its_serial() {
        assert_eq!(setting("setting #7"), Some(7));
        assert_eq!(setting("setting #"), None);
        assert_eq!(setting("none"), None);
    }

    #[test]
    fn only_images_awww_decodes_are_listed_by_name() {
        let directory =
            std::env::temp_dir().join(format!("kanade-wallpapers-{}", std::process::id()));
        drop(fs::remove_dir_all(&directory));
        fs::create_dir_all(directory.join("nested.png")).unwrap();

        for name in [
            "b.JPG",
            "a.png",
            "C.webp",
            ".hidden.png",
            "clip.mp4",
            "notes.txt",
            "noext",
        ] {
            fs::write(directory.join(name), b"").unwrap();
        }

        let names: Vec<String> = list(&directory)
            .iter()
            .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
            .collect();

        assert_eq!(names, ["a.png", "b.JPG", "C.webp"]);
        assert!(list(&directory.join("missing")).is_empty());

        fs::remove_dir_all(&directory).unwrap();
    }
}
