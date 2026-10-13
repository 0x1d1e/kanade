//! Wallpaper (docs/design.md Wallpaper): Kanade selects, awww renders. `kanade wallpaper set
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

use std::collections::{HashMap, VecDeque};
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use inotify::{Inotify, WatchDescriptor, WatchMask};
use kanade_runtime::{MOST_PIXELS, MutexExt, queue::Queue, service::Service};

// what the lock screen decodes, svg told by name and the rest by how they start
pub use kanade_runtime::{ImageFormat as Format, image_format as format};

use super::wake;
use crate::{appearance, config, modules, supervise};

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

// how often, and how far apart, a daemon not yet answering what it shows is asked again
const TRIES: u32 = 5;
const AGAIN: Duration = Duration::from_secs(1);

// for awww to write the cache of every output, so one external set is one look
const SETTLE: Duration = Duration::from_millis(150);

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
    from: Option<From>,
    done: Option<Done>,
}

/*
 * where on the screen a transition that grows or shrinks a circle centers it, as awww takes it: 0
 * to 1 across from the left and up from the bottom
 */
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct From {
    pub x: f32,
    pub y: f32,
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
static QUEUE: LazyLock<Queue<Job>> = LazyLock::new(Queue::new);

// what `kanade wallpaper` asks
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Set(PathBuf),

    // a wallpaper of the directory picked at random, none of those showing
    Shuffle,
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["set", path] if !path.is_empty() => Some(Request::Set(PathBuf::from(path))),
            ["shuffle"] => Some(Request::Shuffle),
            ["status"] => Some(Request::Status),
            _ => None,
        }
    }
}

/*
 * asks awww to show the image at `path`, answering at once with its serial; `done` is told how it
 * went. A circle transition centers on `from`, else the middle. Refused at once when there is no
 * file there or no awww to ask
 */
pub fn set(path: PathBuf, from: Option<From>, done: Option<Done>) -> Result<u64, Unset> {
    if !path.is_file() {
        return Err(Unset::NoImage(path));
    }

    if !wake::found(AWWW) {
        return Err(Unset::NoAwww);
    }

    let serial = {
        let mut wallpaper = WALLPAPER.locked();

        wallpaper.issued += 1;
        let serial = wallpaper.issued;
        wallpaper.pending.push(serial);

        serial
    };

    // the receiver lives in the static, so sending never fails
    QUEUE.send(Job {
        serial,
        path,
        from,
        done,
    });

    Ok(serial)
}

/*
 * does what `request` asks: a set answered as setting, its serial to wait on through `status`, a
 * shuffle the same and then the image it picked
 */
pub fn request(request: Request) -> Result<String, String> {
    match request {
        Request::Set(path) if !path.is_absolute() => Err(format!(
            "{} is not an absolute path, which the shell needs",
            path.display()
        )),
        Request::Set(path) => set(path, None, None)
            .map(|serial| format!("setting #{serial}"))
            .map_err(|unset| unset.to_string()),
        Request::Shuffle => shuffle(None, None)
            .map(|(serial, path)| format!("setting #{serial} {}", path.display())),
        Request::Status => Ok(WALLPAPER.locked().status().to_string()),
    }
}

/*
 * sets a wallpaper of the directory picked at random, none of those showing, as `set` does: its
 * serial and the image, or why there is none to set
 */
pub fn shuffle(from: Option<From>, done: Option<Done>) -> Result<(u64, PathBuf), String> {
    let images = images();
    let path =
        pick(&images, &showing(), RandomState::new().hash_one(())).ok_or_else(
            || match directory() {
                Some(directory) if images.is_empty() => {
                    format!("no wallpapers in {}", directory.display())
                }
                Some(directory) => format!("no other wallpaper in {}", directory.display()),
                None => String::from("no wallpaper folder, as HOME is not set"),
            },
        )?;

    let serial = set(path.clone(), from, done).map_err(|unset| unset.to_string())?;

    Ok((serial, path))
}

// one of `images` none of `showing`, the `random`th of those left
fn pick(images: &[PathBuf], showing: &[PathBuf], random: u64) -> Option<PathBuf> {
    let left: Vec<&PathBuf> = images
        .iter()
        .filter(|image| !showing.contains(image))
        .collect();

    let at = usize::try_from(random % u64::try_from(left.len()).ok()?.max(1)).ok()?;

    left.get(at).map(|&image| image.clone())
}

// what the shell answers a set: its serial, to wait on through `status`
pub fn setting(reply: &str) -> Option<u64> {
    reply.strip_prefix("setting #")?.parse().ok()
}

// what the shell answers a shuffle: its serial, and the image it picked
pub fn shuffled(reply: &str) -> Option<(u64, PathBuf)> {
    let (serial, path) = reply.strip_prefix("setting #")?.split_once(' ')?;

    Some((serial.parse().ok()?, PathBuf::from(path)))
}

// the image Kanade last set, none until it sets one
pub fn current() -> Option<PathBuf> {
    WALLPAPER.locked().current.clone()
}

/*
 * the images showing now as best known: what awww said it shows on each output when last asked,
 * which follows every set through its cache (`watch`), else the one Kanade last set
 */
pub fn showing() -> Vec<PathBuf> {
    let mut showing: Vec<PathBuf> = Shown::read()
        .by
        .values()
        .map(|shown| shown.path.clone())
        .collect();

    if showing.is_empty() {
        showing.extend(current());
    }

    showing
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
        let job = QUEUE.receiver().recv();

        // the sender lives in the static, so the queue never closes
        let Ok(Job {
            serial,
            path,
            from,
            done,
        }) = job
        else {
            return;
        };

        let shown = show(&[AWWW], &path, config::get().transition, from, LIMIT);

        if let Err(why) = &shown {
            eprintln!(
                "kanade: cannot set the wallpaper to {} ({why})",
                path.display()
            );
        }

        {
            let mut wallpaper = WALLPAPER.locked();

            wallpaper.pending.retain(|pending| *pending != serial);

            if shown.is_ok() {
                wallpaper.current = Some(path.clone());
            }

            // the lock screen shows the new one from its first frame
            if shown.is_ok() && modules::on("lock") {
                look();
            }

            if wallpaper.done.len() == RECENT {
                wallpaper.done.pop_front();
            }
            wallpaper.done.push_back((serial, shown.clone()));
        }

        if shown.is_ok() {
            appearance::wallpaper(path);
        }

        if let Some(done) = done {
            done(shown);
        }
    }
}

/*
 * `awww img` through `awww`, the program and any arguments before its own, with `transition`,
 * killed after `limit`; says that the daemon is not running rather than awww's own words for it
 */
fn show(
    awww: &[&str],
    path: &Path,
    transition: config::Transition,
    from: Option<From>,
    limit: Duration,
) -> Result<(), Unset> {
    let text = path.to_str().ok_or_else(|| {
        Unset::Refused(format!("{} is not UTF-8, which awww needs", path.display()))
    })?;

    let (program, before) = awww.split_first().expect("a program");
    let act = |args: &[&str]| {
        let args: Vec<&str> = before.iter().chain(args).copied().collect();
        wake::act_within(program, &args, limit)
    };

    let img = img(text, transition, from);
    let img: Vec<&str> = img.iter().map(String::as_str).collect();

    act(&img).map_err(|failed| match failed {
        wake::Failed::Overran(limit) => Unset::Stuck(limit),
        wake::Failed::Said(why) => match act(&["query"]) {
            Ok(()) => Unset::Refused(why),
            Err(_) if !wake::found(program) => Unset::NoAwww,
            Err(_) => Unset::NoDaemon,
        },
    })
}

/*
 * awww's arguments to show `path` with `transition`: a circle centered on `from`, which awww reads
 * as fractions of the screen for having a point; awww's own center without one
 */
fn img(path: &str, transition: config::Transition, from: Option<From>) -> Vec<String> {
    let mut img = vec![
        String::from("img"),
        String::from("--transition-type"),
        String::from(transition.kind),
        String::from("--transition-duration"),
        format!("{:.3}", transition.duration.as_secs_f32()),
    ];

    if let Some(From { x, y }) = from.filter(|_| transition.circle()) {
        img.push(String::from("--transition-pos"));
        img.push(format!("{:.3},{:.3}", x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
    }

    img.push(String::from(path));

    img
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

    let mut listed = LISTED.locked();

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

/*
 * the image awww shows on each output, by its name, as the lock screen draws it behind the clock;
 * looked up at startup and when a lock is asked, since anything may have set it
 */
#[derive(Debug, Default)]
pub struct Shown {
    by: HashMap<String, Showing>,
}

// an image awww shows, and how light it is, 0 to 1, none for one Kanade does not decode
#[derive(Debug, Clone, PartialEq)]
pub struct Showing {
    pub path: PathBuf,
    pub light: Option<f32>,
}

impl Service for Shown {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

impl Shown {
    pub fn on(&self, output: &str) -> Option<&Showing> {
        self.by.get(output)
    }
}

// One shell palette follows the focused output, or a stable first output when focus is unknown.
fn appearance_path(by: &HashMap<String, PathBuf>, focused_output: Option<&str>) -> Option<PathBuf> {
    let showing = focused_output
        .and_then(|output| by.get(output))
        .filter(|path| path.is_file())
        .or_else(|| {
            by.iter()
                .filter(|(_, path)| path.is_file())
                .min_by(|(first, _), (next, _)| first.as_str().cmp(next.as_str()))
                .map(|(_, path)| path)
        })?;

    Some(showing.clone())
}

fn has_wallpaper(said: &str) -> bool {
    appearance_path(&shown(said), None).is_some()
}

fn query_look<E>(
    retry_for_image: bool,
    mut query: impl FnMut() -> Result<String, E>,
) -> Result<String, E> {
    let mut retries = 0;

    loop {
        match query() {
            Ok(said) if !retry_for_image || has_wallpaper(&said) => return Ok(said),
            Ok(said) if retries == TRIES => return Ok(said),
            Err(error) if retries == TRIES => return Err(error),
            Ok(_) | Err(_) => {}
        }

        retries += 1;
        std::thread::sleep(AGAIN);
    }
}

// what a look does with the appearance palette
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Palette {
    // nothing
    Keep,

    // seeds it at start, unless a wallpaper set since gave one
    Seed,

    // replaces it with what awww now shows, after something else set the wallpaper
    Replace,
}

#[derive(Debug, Default)]
struct LookOrder {
    issued: u64,
    written: u64,
    appearance_pending: bool,

    // the first look that may replace the palette, and the last that did
    replace_from: u64,
    replaced: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct LookCompletion {
    publish: bool,
    appearance: Option<PathBuf>,
    replace: bool,
}

impl LookOrder {
    fn start(&mut self, palette: Palette) -> (u64, bool) {
        self.issued += 1;
        self.appearance_pending |= palette == Palette::Seed;
        if palette == Palette::Replace {
            self.replace_from = self.issued;
        }
        (self.issued, self.appearance_pending)
    }

    fn finish(&mut self, serial: u64, appearance: Option<PathBuf>) -> LookCompletion {
        let publish = serial > self.written;
        if publish {
            self.written = serial;
        }

        /*
         * only a look at least as new as the latest replace request, and none older after it; one
         * that found no image, as a lock screen's may, leaves the request to an older look still out
         */
        let replace = self.replace_from > self.replaced
            && serial >= self.replace_from
            && appearance.is_some();
        if replace {
            self.replaced = serial;
        }

        let appearance = if self.appearance_pending || replace {
            appearance
        } else {
            None
        };
        if appearance.is_some() {
            self.appearance_pending = false;
        }

        LookCompletion {
            publish,
            appearance,
            replace,
        }
    }
}

static LOOKS: Mutex<LookOrder> = Mutex::new(LookOrder {
    issued: 0,
    written: 0,
    appearance_pending: false,
    replace_from: 0,
    replaced: 0,
});

/*
 * asks awww what it shows, off the caller's thread; a daemon still starting, as at login, is asked
 * again a few times until an image appears or the bounded retry window ends
 */
pub fn look() {
    look_with(Palette::Keep);
}

pub fn look_for_appearance() {
    look_with(Palette::Seed);
}

fn look_with(palette: Palette) {
    let (this, retry_for_image) = {
        let mut looks = LOOKS.locked();
        looks.start(palette)
    };

    // awww looked for off the caller's thread too, as it may be asked on the way to sleep
    let spawned = std::thread::Builder::new()
        .name(String::from("wallpaper look"))
        .spawn(move || {
            if !wake::found(AWWW) {
                return;
            }

            let said = query_look(retry_for_image, || {
                wake::query_within(AWWW, &["query"], LIMIT)
            });
            let Ok(said) = said else {
                return;
            };
            let raw = shown(&said);
            let focused = super::niri::Niri::read().seen.focused_output.clone();
            let palette = appearance_path(&raw, focused.as_deref());

            // A palette may use an image the lock screen cannot decode, so keep its candidates apart.
            let by: HashMap<String, Showing> = raw
                .into_iter()
                .filter(|(_, path)| format(path).is_some())
                .map(|(output, path)| {
                    let light = lightness(&path);

                    (output, Showing { path, light })
                })
                .collect();

            // held while written, so looks write in turn; a newer lock lookup can also claim a
            // pending startup seed before an older startup query finishes
            let palette = {
                let mut looks = LOOKS.locked();
                let completion = looks.finish(this, palette);

                if completion.publish && Shown::read().by != by {
                    Shown::write().by = by;
                }

                completion.appearance.map(|path| (path, completion.replace))
            };

            match palette {
                Some((path, true)) => appearance::shown_wallpaper(path),
                Some((path, false)) => appearance::seed_wallpaper(path),
                None => {}
            }
        });

    if let Err(error) = spawned {
        eprintln!("kanade: cannot ask awww for the wallpaper: {error}");
    }
}

// awww's cache: a directory per awww version, in it a file per output that each `awww img` rewrites
fn cache() -> Option<PathBuf> {
    env::var_os("XDG_CACHE_HOME")
        .filter(|cache| !cache.is_empty())
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
        .map(|cache| cache.join("awww"))
}

/*
 * follows a wallpaper set outside Kanade, as by `awww img` in a script: a write to awww's cache
 * looks again, for the lock screen, and replaces the appearance palette. It blocks on inotify, so
 * it costs no idle wakeups
 */
pub fn watch() {
    let Some(cache) = cache() else { return };

    if let Err(error) = watch_cache(&cache) {
        eprintln!("kanade: cannot follow wallpapers set outside Kanade: {error}");
    }
}

fn watch_cache(cache: &Path) -> io::Result<()> {
    let mut inotify = Inotify::init()?;
    let mut buffer = [0; 4096];
    let mut watches = arm_cache(&mut inotify, cache, Vec::new());

    loop {
        let mut shown = false;
        let mut moved = false;
        for event in inotify.read_events_blocking(&mut buffer)? {
            match watches.iter().position(|watch| *watch == event.wd) {
                Some(0) => moved = true,
                Some(_) => shown = true,
                // a watch already replaced, saying it ended
                None => {}
            }
        }

        // awww writes one output's file after the other: until a quiet SETTLE, one look for all
        while shown {
            std::thread::sleep(SETTLE);
            let events = match inotify.read_events(&mut buffer) {
                Ok(events) => events,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
            };
            let mut quiet = true;
            for event in events {
                quiet = false;
                match watches.iter().position(|watch| *watch == event.wd) {
                    Some(0) => moved = true,
                    Some(_) => shown = true,
                    None => {}
                }
            }
            if quiet {
                break;
            }
        }

        if moved {
            watches = arm_cache(&mut inotify, cache, watches);
        }
        if shown {
            look_with(Palette::Replace);
        }
    }
}

// the watch on the cache, or its nearest parent while awww has made none, first; then one on
// each version's directory
fn arm_cache(
    inotify: &mut Inotify,
    cache: &Path,
    old: Vec<WatchDescriptor>,
) -> Vec<WatchDescriptor> {
    for watch in old {
        // fails for a watch the kernel already ended
        let _ = inotify.watches().remove(watch);
    }

    let mut dir = cache;
    while !dir.is_dir() {
        let Some(parent) = dir.parent() else {
            return Vec::new();
        };
        dir = parent;
    }

    let parent =
        WatchMask::CREATE | WatchMask::MOVED_TO | WatchMask::DELETE_SELF | WatchMask::ONLYDIR;
    let mut watches = match inotify.watches().add(dir, parent) {
        Ok(watch) => vec![watch],
        Err(error) => {
            eprintln!("kanade: cannot watch {}: {error}", dir.display());
            return Vec::new();
        }
    };
    if dir != cache {
        return watches;
    }

    let versions = cache.read_dir().into_iter().flatten().flatten();
    for version in versions.filter(|entry| entry.path().is_dir()) {
        let outputs = WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO | WatchMask::ONLYDIR;
        match inotify.watches().add(version.path(), outputs) {
            Ok(watch) => watches.push(watch),
            Err(error) => eprintln!("kanade: cannot watch {}: {error}", version.path().display()),
        }
    }

    watches
}

// how many wallpapers' lightness is kept
const KNOWN_MOST: usize = 16;

/*
 * how light the image at `path` is, its mean luma, 0 to 1: any `Format`, none
 * for one too large. Kept by the file's modification time, as decoding a large one takes a tenth
 * of a second, and decoded one at a time, as each may take a few hundred megabytes
 */
fn lightness(path: &Path) -> Option<f32> {
    type Known = HashMap<PathBuf, (SystemTime, Option<f32>)>;
    static KNOWN: LazyLock<Mutex<Known>> = LazyLock::new(Mutex::default);
    static DECODING: Mutex<()> = Mutex::new(());

    let changed = fs::metadata(path).and_then(|file| file.modified()).ok()?;
    let known = |known: &Known| {
        known
            .get(path)
            .filter(|(when, _)| *when == changed)
            .map(|(_, light)| *light)
    };

    // KNOWN is left unlocked while decoding, so reading it waits on nothing; a look waits for one
    let decoding = DECODING.locked();

    // under DECODING, so two looks at once decode it once
    if let Some(light) = known(&KNOWN.locked()) {
        return light;
    }

    // a decoder that panics on a bad file lets the look go on, with the wallpaper's lightness unknown
    let light = panic::catch_unwind(|| decoded_light(path)).ok().flatten();
    drop(decoding);

    let mut known = KNOWN.locked();

    // a few wallpapers are enough to know; a slideshow through hundreds keeps none of them for long
    if known.len() >= KNOWN_MOST {
        known.clear();
    }
    known.insert(path.to_owned(), (changed, light));

    light
}

// how many rows of a png are read for its lightness: one in so many
const ROWS: usize = 8;

fn decoded_light(path: &Path) -> Option<f32> {
    let (pixels, components) = match format(path)? {
        // row by row, keeping a few, so even a huge one takes little memory
        Format::Png => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let mut decoder = png::Decoder::new(file);
            decoder.set_transformations(png::Transformations::normalize_to_color8());

            let mut reader = decoder.read_info().ok()?;
            let info = reader.info();
            if info.width as usize * info.height as usize > MOST_PIXELS {
                return None;
            }

            let components = reader.output_color_type().0.samples();
            let mut pixels = Vec::new();
            let mut row = 0;

            // an interlaced one's passes are each spread over the whole image, so every few rows still are
            while let Some(read) = reader.next_row().ok()? {
                if row % ROWS == 0 {
                    pixels.extend_from_slice(read.data());
                }
                row += 1;
            }

            (pixels, components)
        }
        Format::Jpeg => {
            // read as it decodes, so one too large is let go on its header
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let mut decoder = zune_jpeg::JpegDecoder::new(file);
            decoder.decode_headers().ok()?;

            let (width, height) = decoder.dimensions()?;
            if width * height > MOST_PIXELS {
                return None;
            }

            let pixels = decoder.decode().ok()?;

            (pixels, decoder.output_colorspace()?.num_components())
        }
        // whole, as there is no cheaper way to a mean; a still, a first frame or a drawing is small
        Format::Webp | Format::Gif | Format::Svg => {
            let (_, _, rgba) = decoded(path, MEAN_SIDE)?;
            return light(&rgba, 4);
        }
    };

    light(&pixels, components)
}

// the longest side an svg is drawn at to take its mean: all its pixels are read
const MEAN_SIDE: f32 = 256.0;

/*
 * an image whole, as 8-bit RGBA with plain alpha, for the lock screen to draw: the first frame of
 * a webp or gif, an svg drawn with its longest side `vector_side` pixels across; none past
 * `MOST_PIXELS`, told by its header
 */
pub fn decoded(path: &Path, vector_side: f32) -> Option<(u32, u32, Vec<u8>)> {
    kanade_runtime::decode_image(path, vector_side)
}

// the mean luma of 8-bit pixels of `components` each, gray or red, green and blue first
fn light(pixels: &[u8], components: usize) -> Option<f32> {
    if components == 0 {
        return None;
    }

    // every few pixels is as good a mean, at a fraction of the time
    let (sum, count) = pixels
        .chunks_exact(components)
        .step_by(7)
        .map(|pixel| match pixel {
            [red, green, blue, ..] => {
                0.2126 * f32::from(*red) + 0.7152 * f32::from(*green) + 0.0722 * f32::from(*blue)
            }
            [gray, ..] => f32::from(*gray),
            [] => 0.0,
        })
        .fold((0.0, 0u32), |(sum, count), luma| (sum + luma, count + 1));

    (count > 0).then(|| sum / count as f32 / 255.0)
}

/*
 * the images in what `awww query` says, a line per output like
 * `: eDP-1: 1920x1080, scale: 1, currently displaying: image: /path`, its namespace first if any;
 * an output showing a color has none
 */
fn shown(said: &str) -> HashMap<String, PathBuf> {
    let mut shown = HashMap::new();

    // an output under several namespaces keeps the first awww lists
    let said = said.lines().filter_map(|line| {
        let (head, path) = line.split_once("currently displaying: image: ")?;
        let pieces: Vec<&str> = head.split([':', ',']).map(str::trim).collect();

        // the output's name is the piece before its size
        let name = pieces.windows(2).find_map(|pair| {
            let size = pair[1].split_once('x')?;
            let number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());

            (number(size.0) && number(size.1) && !pair[0].is_empty()).then_some(pair[0])
        })?;

        // a path may end in spaces; only the line's end is cut
        Some((name.to_owned(), PathBuf::from(path.trim_end_matches('\r'))))
    });

    for (name, path) in said {
        shown.entry(name).or_insert(path);
    }

    shown
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct TestFile(PathBuf);

    impl TestFile {
        fn new(extension: &str, bytes: &[u8]) -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "kanade-wallpaper-test-{}-{}.{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
                extension
            ));
            fs::write(&path, bytes).unwrap();
            Self(path)
        }
    }

    impl Drop for TestFile {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }

    #[test]
    fn appearance_uses_focused_output_then_stable_first_available_image() {
        let first = TestFile::new("png", b"first");
        let second = TestFile::new("png", b"second");
        let missing = PathBuf::from("/wallpapers/missing.png");
        let by = HashMap::from([
            (String::from("B-output"), second.0.clone()),
            (String::from("A-output"), first.0.clone()),
            (String::from("0-output"), missing),
        ]);

        assert_eq!(
            appearance_path(&by, Some("B-output")),
            Some(second.0.clone())
        );
        assert_eq!(
            appearance_path(&by, Some("0-output")),
            Some(first.0.clone())
        );
        assert_eq!(appearance_path(&HashMap::new(), None), None);
    }

    #[test]
    fn palette_candidates_include_images_the_lock_screen_cannot_decode() {
        let bmp = TestFile::new("bmp", b"BM");
        let by = HashMap::from([(String::from("eDP-1"), bmp.0.clone())]);

        assert_eq!(format(&bmp.0), None);
        assert_eq!(appearance_path(&by, Some("eDP-1")), Some(bmp.0.clone()));
    }

    #[test]
    fn startup_lookup_retries_successful_solid_queries_until_an_image_appears() {
        let wallpaper = TestFile::new("bmp", b"BM");
        let replies = std::cell::RefCell::new(VecDeque::from([
            Ok::<_, ()>(String::from(
                ": eDP-1: 1920x1080, scale: 1, currently displaying: color: #111111",
            )),
            Ok(format!(
                ": eDP-1: 1920x1080, scale: 1, currently displaying: image: {}",
                wallpaper.0.display()
            )),
        ]));
        let mut calls = 0;

        let said = query_look(true, || {
            calls += 1;
            replies.borrow_mut().pop_front().unwrap()
        })
        .unwrap();

        assert_eq!(calls, 2);
        assert!(said.contains(&wallpaper.0.display().to_string()));
    }

    #[test]
    fn newer_lock_lookup_can_finish_the_pending_startup_seed() {
        let wallpaper = TestFile::new("png", b"wallpaper");
        let mut looks = LookOrder::default();
        let (startup, _) = looks.start(Palette::Seed);
        let (lock, retry) = looks.start(Palette::Keep);
        assert!(retry);

        let lock = looks.finish(lock, Some(wallpaper.0.clone()));
        assert!(lock.publish);
        assert_eq!(lock.appearance, Some(wallpaper.0.clone()));

        let startup = looks.finish(startup, Some(wallpaper.0.clone()));
        assert!(!startup.publish);
        assert_eq!(startup.appearance, None);
        assert!(!looks.appearance_pending);
    }

    #[test]
    fn newer_solid_lookup_does_not_discard_the_pending_startup_seed() {
        let wallpaper = TestFile::new("png", b"wallpaper");
        let mut looks = LookOrder::default();
        let (startup, _) = looks.start(Palette::Seed);
        let (lock, retry) = looks.start(Palette::Keep);
        assert!(retry);

        let lock = looks.finish(lock, None);
        assert!(lock.publish);
        assert_eq!(lock.appearance, None);
        assert!(looks.appearance_pending);

        let startup = looks.finish(startup, Some(wallpaper.0.clone()));
        assert!(!startup.publish);
        assert_eq!(startup.appearance, Some(wallpaper.0.clone()));
    }

    #[test]
    fn newer_imageless_lookup_does_not_consume_a_pending_replacement() {
        let wallpaper = TestFile::new("png", b"wallpaper");
        let mut looks = LookOrder::default();
        let (external, _) = looks.start(Palette::Replace);
        let (lock, _) = looks.start(Palette::Keep);

        let lock = looks.finish(lock, None);
        assert_eq!(lock.appearance, None);

        let external = looks.finish(external, Some(wallpaper.0.clone()));
        assert!(external.replace);
        assert_eq!(external.appearance, Some(wallpaper.0.clone()));
    }

    #[test]
    fn shuffle_never_picks_an_image_showing() {
        let images: Vec<PathBuf> = ["a.png", "b.png", "c.png"].map(PathBuf::from).into();
        let showing = [PathBuf::from("b.png")];

        let picked: Vec<PathBuf> = (0..6)
            .filter_map(|random| pick(&images, &showing, random))
            .collect();

        assert_eq!(picked.len(), 6);
        assert!(!picked.contains(&showing[0]));
        assert!(picked.contains(&images[0]) && picked.contains(&images[2]));

        assert_eq!(pick(&images[1..2], &showing, 7), None);
        assert_eq!(pick(&[], &[], 7), None);
    }

    #[test]
    fn shuffle_reply_keeps_a_path_with_spaces() {
        assert_eq!(
            shuffled("setting #12 /home/k/Pictures/Wallpapers/a lake.png"),
            Some((12, PathBuf::from("/home/k/Pictures/Wallpapers/a lake.png")))
        );
        assert_eq!(shuffled("no other wallpaper in /tmp"), None);
    }
}
