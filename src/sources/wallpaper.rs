//! Wallpaper (docs/archived/design.md Wallpaper): Kanade selects, awww renders. `kanade wallpaper set
//! <path>` and the Wallpapers Surface both ask `set`, which hands the image to `awww img`.
//!
//! awww draws through its daemon, which Kanade runs as a holder (ADR 0011) while the Module is on,
//! only through setpriv, so it dies with Kanade even killed; on its next start awww shows the last
//! image again from its own cache. When a daemon already answers, like one the compositor started,
//! Kanade sets on that one instead, setpriv or not.
//!
//! `awww img` decodes and scales the image itself, which takes up to a tenth of a second, so it runs
//! on a thread of its own, one image at a time in the order asked, never on the draw thread, and is
//! killed after `LIMIT` so one that hangs holds up no set after it. `set` answers at once; `kanade`
//! is answered once awww showed the image (`Later`).

use std::collections::HashMap;
use std::env;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::hash::{BuildHasher, RandomState};
use std::io::{self, BufRead};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant, SystemTime};

use inotify::{Inotify, WatchDescriptor, WatchMask};
use kanade_runtime::{MOST_PIXELS, MutexExt, queue::Queue, service::Service};

// what the lock screen decodes, svg told by name and the rest by how they start
pub use kanade_runtime::{ImageFormat as Format, image_format as format};

use super::wake;
use crate::backoff::Backoff;
use crate::hooks::{self, Event};
use crate::later::{LATER, Later};
use crate::{appearance, config, modules, supervise};

#[path = "wallpaper_seed.rs"]
mod seed;

// fills an empty wallpaper folder once, from the Favorites list; a later emptying is left empty
pub fn seed() {
    seed::run();
    choose_initial();
}

// Seed first so downloaded images are eligible, then wait for awww to answer. Existing images
// are preserved; a successful query with no image allows the initial pick.
fn choose_initial() {
    let Some(said) = ready_query(
        || wake::query_within(AWWW, &["query"], LIMIT),
        std::thread::sleep,
    ) else {
        return;
    };
    if appearance_path(&shown(&said), None).is_some() {
        return;
    }

    let images = images();
    let Some(path) = pick(&images, &[], RandomState::new().hash_one(())) else {
        return;
    };
    let _ = set(path, Output::All, None, None);
}

fn ready_query<E>(
    mut query: impl FnMut() -> Result<String, E>,
    mut sleep: impl FnMut(Duration),
) -> Option<String> {
    for attempt in 0..=TRIES {
        match query() {
            Ok(said) => return Some(said),
            Err(_) if attempt < TRIES => sleep(AGAIN),
            Err(_) => return None,
        }
    }
    None
}

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

// how long a watch that ended waits before it is made again, and how long it must have lasted to
// start over: a cache that keeps disappearing is retried slowly, not in a loop
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);
const LASTED: Duration = Duration::from_secs(60);

// for awww to write the cache of every output, so one external set is one look
const SETTLE: Duration = Duration::from_millis(150);

// what is told how a set went, like the Launcher, which closes or says it was not set
pub type Done = Box<dyn FnOnce(Result<(), Unset>) + Send>;

// why a wallpaper was not set
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unset {
    NoImage(PathBuf),
    NoAwww,
    NoDaemon,

    // awww lists no output of this name
    NoOutput(String),

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
            Unset::NoOutput(_) => "no such output",
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
            Unset::NoOutput(name) => write!(f, "{AWWW} shows nothing on an output named {name}"),
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
    path: PathBuf,
    output: Output,

    // the output focused when it was asked, which the palette may follow (ADR 0064)
    driver: Option<String>,
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
}

static WALLPAPER: Mutex<Wallpaper> = Mutex::new(Wallpaper { current: None });

// the wallpaper last told to the hook, so one seen twice, as awww's cache reports one Kanade set,
// fires once; the one shown at start is only noted, as it is no change
static HOOKED: Mutex<Option<PathBuf>> = Mutex::new(None);

fn hook(path: &Path, fire: bool) {
    let new = HOOKED.locked().replace(path.to_owned()).as_deref() != Some(path);

    if new && fire {
        hooks::fire_with(Event::Wallpaper, path);
        // the login screen's picture, when `sddm.sync` asks for it; returns at once
        crate::sddm::wallpaper_changed(path);
    }
}

// the sets waiting for `serve`, which outlive a restart of it
static QUEUE: LazyLock<Queue<Job>> = LazyLock::new(Queue::new);

// the outputs of a pick (ADR 0064)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Output {
    All,

    // one, by the name awww and niri give it, like `eDP-1`
    Named(String),
}

// the longest output name taken, well over any connector's
const NAME: usize = 64;

impl Output {
    // `all`, or an output's name: no space, comma or control character, which awww's list takes
    // apart, and no leading dash, which awww would read as a flag
    fn parse(word: &str) -> Option<Output> {
        let name = !word.is_empty()
            && word.len() <= NAME
            && !word.starts_with('-')
            && !word
                .chars()
                .any(|c| c == ',' || c.is_whitespace() || c.is_control());

        match word {
            "all" => Some(Output::All),
            _ if name => Some(Output::Named(word.to_owned())),
            _ => None,
        }
    }

    fn word(&self) -> &str {
        match self {
            Output::All => "all",
            Output::Named(name) => name,
        }
    }
}

// what `kanade wallpaper` asks
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    // the image on the outputs asked, else all of them
    Set(PathBuf, Option<Output>),

    // a wallpaper of the directory picked at random, none of those showing on the outputs asked,
    // else the focused one
    Shuffle(Option<Output>),

    // what each output shows
    Status,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        let (words, output) = without_output(arguments)?;

        match (words.as_slice(), output) {
            (["set", path], output) if !path.is_empty() => {
                Some(Request::Set(PathBuf::from(path), output))
            }
            (["shuffle"], output) => Some(Request::Shuffle(output)),
            (["status"], None) => Some(Request::Status),
            _ => None,
        }
    }

    // the words of `parse`'s arguments, with the path as given
    pub fn arguments(&self) -> Vec<String> {
        let (mut words, output) = match self {
            Request::Set(path, output) => (
                vec![
                    String::from("wallpaper"),
                    String::from("set"),
                    path.to_string_lossy().into_owned(),
                ],
                output,
            ),
            Request::Shuffle(output) => (
                vec![String::from("wallpaper"), String::from("shuffle")],
                output,
            ),
            Request::Status => {
                return vec![String::from("wallpaper"), String::from("status")];
            }
        };

        if let Some(output) = output {
            words.push(String::from("--output"));
            words.push(output.word().to_owned());
        }

        words
    }
}

// the arguments without `--output NAME` or `--output=NAME`, and its output; none if it is odd
fn without_output<'a>(arguments: &[&'a str]) -> Option<(Vec<&'a str>, Option<Output>)> {
    let mut words = Vec::new();
    let mut output = None;
    let mut rest = arguments.iter().copied();

    while let Some(word) = rest.next() {
        let name = match word {
            "--output" => rest.next()?,
            _ => match word.strip_prefix("--output=") {
                Some(name) => name,
                None => {
                    words.push(word);
                    continue;
                }
            },
        };

        // once
        if output.replace(Output::parse(name)?).is_some() {
            return None;
        }
    }

    Some((words, output))
}

// the output with the focus, none while niri does not say
fn focused() -> Option<String> {
    super::niri::Niri::read().seen.focused_output.clone()
}

// where a wallpaper picked on `monitor` lands: that monitor alone, or every one by
// `wallpaper.per_monitor`
pub fn picked_on(monitor: &str) -> Output {
    if config::get().wallpaper_per_monitor {
        Output::Named(monitor.to_owned())
    } else {
        Output::All
    }
}

/*
 * asks awww to show the image at `path` on `output`, the others as they were, answering at once;
 * `done` is told how it went. A circle transition centers on `from`, else the middle. Refused at
 * once when there is no file there or no awww to ask
 */
pub fn set(
    path: PathBuf,
    output: Output,
    from: Option<From>,
    done: Option<Done>,
) -> Result<(), Unset> {
    if !path.is_file() {
        return Err(Unset::NoImage(path));
    }

    if !wake::found(AWWW) {
        return Err(Unset::NoAwww);
    }

    // the receiver lives in the static, so sending never fails
    QUEUE.send(Job {
        path,
        output,
        driver: focused(),
        from,
        done,
    });

    Ok(())
}

/*
 * does what `request` asks: a set or a shuffle answers `later` with the image once awww showed it,
 * or why it did not
 */
pub fn request(request: Request, later: Later) {
    later.unsettled(format!(
        "{AWWW} did not show the wallpaper within {}s; it may still",
        LATER.as_secs()
    ));

    let set = |path: PathBuf, output: Output| {
        let answer = later.clone();
        let shown = path.display().to_string();
        let done: Done = Box::new(move |result| match result {
            Ok(()) => answer.done(shown),
            Err(unset) => answer.refused(unset.to_string()),
        });

        set(path, output, None, Some(done)).map_err(|unset| unset.to_string())
    };

    let asked = match request {
        Request::Set(path, _) if !path.is_absolute() => Err(format!(
            "{} is not an absolute path, which the shell needs",
            path.display()
        )),
        Request::Set(path, output) => set(path, output.unwrap_or(Output::All)),
        Request::Shuffle(output) => {
            let output = output.unwrap_or_else(|| {
                focused()
                    .filter(|_| config::get().wallpaper_per_monitor)
                    .map_or(Output::All, Output::Named)
            });

            pick_other(&output).and_then(|path| set(path, output))
        }
        Request::Status => {
            say_status(later);
            return;
        }
    };

    if let Err(why) = asked {
        later.refused(why);
    }
}

/*
 * sets a wallpaper of the directory picked at random, none of those showing on `output`, as `set`
 * does: the image, or why there is none to set
 */
pub fn shuffle(output: Output, from: Option<From>, done: Option<Done>) -> Result<PathBuf, String> {
    let path = pick_other(&output)?;

    set(path.clone(), output, from, done).map_err(|unset| unset.to_string())?;

    Ok(path)
}

// an image of the directory none of those showing on `output`, picked at random
fn pick_other(output: &Output) -> Result<PathBuf, String> {
    let images = images();

    pick(
        &images,
        &showing_on(output),
        RandomState::new().hash_one(()),
    )
    .ok_or_else(|| match directory() {
        Some(directory) if images.is_empty() => {
            format!("no wallpapers in {}", directory.display())
        }
        Some(directory) => format!("no other wallpaper in {}", directory.display()),
        None => String::from("no wallpaper folder, as HOME is not set"),
    })
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

// the image Kanade last set, none until it sets one
pub fn current() -> Option<PathBuf> {
    WALLPAPER.locked().current.clone()
}

/*
 * the images showing on `output` as best known: what awww said it shows on each output when last
 * asked, which follows every set through its cache (`watch`). An output awww did not report has
 * none known, not the one Kanade last set, which may well be shown elsewhere; for every output
 * together, awww having said nothing at all still falls back to that, as it is the one image
 * Kanade knows is on the screens
 */
pub fn showing_on(output: &Output) -> Vec<PathBuf> {
    known(output, &Shown::read().by, current())
}

// what `showing_on` answers, from awww's reading of each output and the image Kanade last set
fn known(output: &Output, by: &HashMap<String, Showing>, current: Option<PathBuf>) -> Vec<PathBuf> {
    let showing: Vec<PathBuf> = match output {
        Output::All => by.values().map(|shown| shown.path.clone()).collect(),
        Output::Named(name) => by
            .get(name)
            .map(|shown| shown.path.clone())
            .into_iter()
            .collect(),
    };

    if !showing.is_empty() {
        showing
    } else if *output == Output::All {
        current.into_iter().collect()
    } else {
        Vec::new()
    }
}

/*
 * the image awww shows now, asked here and now: the focused output's, else the first by name. For
 * a command that runs without a shell (`kanade sddm sync`); none with no awww, no answer in time or
 * no image shown
 */
pub fn showing_now() -> Option<PathBuf> {
    if !wake::found(AWWW) {
        return None;
    }

    let said = wake::query_within(AWWW, &["query"], LIMIT).ok()?;

    appearance_path(&shown(&said), focused().as_deref())
}

// the output the palette follows: the one focused at the last pick of Kanade's, none before
static DRIVER: Mutex<Option<String>> = Mutex::new(None);

// whether a pick on `output` changes the palette, as `driver` (ADR 0064) follows it
fn drives(output: &Output, driver: Option<&str>) -> bool {
    match (output, driver) {
        (Output::Named(name), Some(driver)) => name == driver,
        _ => true,
    }
}

// for `kanade status`
pub fn status() -> String {
    match current() {
        Some(path) => format!("wallpaper: {}", path.display()),
        None => String::from("wallpaper: none set since start"),
    }
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
            path,
            output,
            driver,
            from,
            done,
        }) = job
        else {
            return;
        };

        // before awww writes its cache, which `watch` looks at to choose the palette's image
        let drives = {
            let mut remembered = DRIVER.locked();

            if driver.is_some() {
                *remembered = driver;
            }

            drives(&output, remembered.as_deref())
        };

        let shown = show(
            &[AWWW],
            &path,
            &output,
            config::get().transition,
            from,
            LIMIT,
        );

        if let Err(why) = &shown {
            eprintln!(
                "kanade: cannot set the wallpaper to {} ({why})",
                path.display()
            );
        }

        {
            let mut wallpaper = WALLPAPER.locked();

            if shown.is_ok() {
                wallpaper.current = Some(path.clone());
            }

            // the lock screen shows the new one from its first frame
            if shown.is_ok() && modules::on("lock") {
                look();
            }
        }

        // on another output than the palette's, it is the look of awww's cache that says it unchanged
        if shown.is_ok() && drives {
            hook(&path, true);
            appearance::wallpaper(path);
        }

        if let Some(done) = done {
            done(shown);
        }
    }
}

/*
 * `awww img` through `awww`, the program and any arguments before its own, on `output` only, with
 * `transition`, killed after `limit`; says that the daemon is not running rather than awww's own
 * words for it, and that an output awww does not list is not there, asking it nothing
 */
fn show(
    awww: &[&str],
    path: &Path,
    output: &Output,
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

    // a query that fails is left to the set, which then says why
    if let Output::Named(name) = output {
        let args: Vec<&str> = before.iter().copied().chain(["query"]).collect();

        if let Ok(said) = wake::query_within(program, &args, limit)
            && !listed(&said).any(|(listed, _)| listed == *name)
        {
            return Err(Unset::NoOutput(name.clone()));
        }
    }

    let img = img(text, output, transition, from);
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
 * awww's arguments to show `path` on `output` with `transition`: a circle centered on `from`, which
 * awww reads as fractions of the screen for having a point; awww's own center without one
 */
fn img(
    path: &str,
    output: &Output,
    transition: config::Transition,
    from: Option<From>,
) -> Vec<String> {
    let mut img = vec![
        String::from("img"),
        String::from("--transition-type"),
        String::from(transition.kind),
        String::from("--transition-duration"),
        format!("{:.3}", transition.duration.as_secs_f32()),
    ];

    // awww sets every output unless told which
    if let Output::Named(name) = output {
        img.push(format!("--outputs={name}"));
    }

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
            // the output of the last pick, else the focused one (ADR 0064)
            let driver = DRIVER
                .locked()
                .clone()
                .filter(|driver| raw.contains_key(driver));
            let preferred = driver.or_else(focused);
            let palette = appearance_path(&raw, preferred.as_deref());

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
                Some((path, true)) => {
                    hook(&path, true);
                    appearance::shown_wallpaper(path);
                }
                Some((path, false)) => {
                    hook(&path, false);
                    appearance::seed_wallpaper(path);
                }
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
 * it costs no idle wakeups. A watch that ends, as when awww's cache is deleted or inotify runs out
 * of watches, is made again after a backoff, so following never stops for the rest of the run
 */
pub fn watch() {
    let Some(cache) = cache() else { return };

    let mut backoff = Backoff::new(FIRST_RETRY, LAST_RETRY);
    let mut said = None;

    loop {
        let started = Instant::now();
        let error = watch_cache(&cache).err();

        let Some(error) = error else { return };

        // said once, not at every try
        let why = format!("cannot follow wallpapers set outside Kanade: {error}");

        if said.as_ref() != Some(&why) {
            eprintln!("kanade: {why}");
            said = Some(why);
        }

        std::thread::sleep(backoff.after(started.elapsed(), LASTED));
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
    let said = listed(said).filter_map(|(name, what)| {
        let path = what.strip_prefix("image: ")?;

        // a path may end in spaces; only the line's end is cut
        Some((name, PathBuf::from(path)))
    });

    for (name, path) in said {
        shown.entry(name).or_insert(path);
    }

    shown
}

/*
 * what `awww query` says each output shows, by its name, like `image: /path` or `color: #111111`;
 * an output under several namespaces is listed for each
 */
fn listed(said: &str) -> impl Iterator<Item = (String, &str)> {
    said.lines().filter_map(|line| {
        let (head, what) = line.split_once("currently displaying: ")?;
        let pieces: Vec<&str> = head.split([':', ',']).map(str::trim).collect();

        // the output's name is the piece before its size
        let name = pieces.windows(2).find_map(|pair| {
            let size = pair[1].split_once('x')?;
            let number = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());

            (number(size.0) && number(size.1) && !pair[0].is_empty()).then_some(pair[0])
        })?;

        Some((name.to_owned(), what.trim_end_matches('\r')))
    })
}

/*
 * answers `later` with what each output shows, and which the palette follows, asking awww off the
 * draw thread
 */
fn say_status(later: Later) {
    let spawned = std::thread::Builder::new()
        .name(String::from("wallpaper status"))
        .spawn({
            let later = later.clone();

            move || {
                let mut text = status();

                if wake::found(AWWW) {
                    match wake::query_within(AWWW, &["query"], LIMIT) {
                        Ok(said) => text.push_str(&outputs(&said, DRIVER.locked().as_deref())),
                        Err(_) => text.push_str(&format!("\n{DAEMON} is not answering")),
                    }
                }

                later.done(text);
            }
        });

    if let Err(error) = spawned {
        later.refused(format!("cannot ask awww: {error}"));
    }
}

// a line per output of what `awww query` said, the one the palette follows marked
fn outputs(said: &str, driver: Option<&str>) -> String {
    let mut seen: Vec<String> = Vec::new();
    let mut text = String::new();

    for (name, what) in listed(said) {
        if seen.contains(&name) {
            continue;
        }

        let mark = if driver == Some(name.as_str()) {
            " (palette)"
        } else {
            ""
        };

        text.push_str(&format!("\n{name}: {what}{mark}"));
        seen.push(name);
    }

    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
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

    // an output awww never reported shows nothing, not the image Kanade last set somewhere else
    #[test]
    fn an_output_awww_did_not_report_shows_no_wallpaper() {
        let image = PathBuf::from("/wallpapers/b.png");
        let other = PathBuf::from("/wallpapers/other.png");
        let by = HashMap::from([(
            String::from("eDP-1"),
            Showing {
                path: image.clone(),
                light: None,
            },
        )]);

        assert_eq!(
            known(&Output::Named(String::from("eDP-1")), &by, None),
            std::slice::from_ref(&image)
        );

        // awww said nothing of this output, though Kanade last set one
        assert!(
            known(
                &Output::Named(String::from("HDMI-A-1")),
                &by,
                Some(other.clone())
            )
            .is_empty()
        );

        // every output together still falls back to it, as the one Kanade knows is showing
        assert_eq!(
            known(&Output::All, &HashMap::new(), Some(other.clone())),
            std::slice::from_ref(&other)
        );
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
    fn startup_query_retries_until_awww_answers() {
        let mut answers = [
            Err("not ready".to_owned()),
            Err("not ready".to_owned()),
            Ok("ready".to_owned()),
        ]
        .into_iter();
        let mut retries = 0;
        let said = ready_query(
            || answers.next().expect("query should be retried"),
            |_| retries += 1,
        );
        assert_eq!(said.as_deref(), Some("ready"));
        assert_eq!(retries, 2);
    }

    #[test]
    fn startup_query_gives_up_when_awww_is_missing() {
        let mut attempts = 0;
        let mut retries = 0;
        let said: Option<String> = ready_query(
            || {
                attempts += 1;
                Err("missing")
            },
            |_| retries += 1,
        );
        assert_eq!(said, None);
        assert_eq!(attempts, TRIES as usize + 1);
        assert_eq!(retries, TRIES as usize);
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
    fn output_words_are_taken_whole_or_not_at_all() {
        let named = |name: &str| Some(Output::Named(name.to_owned()));
        let set = |path: &str, output| Some(Request::Set(PathBuf::from(path), output));

        assert_eq!(Request::parse(&["set", "/a.png"]), set("/a.png", None));
        assert_eq!(
            Request::parse(&["set", "/a.png", "--output", "eDP-1"]),
            set("/a.png", named("eDP-1"))
        );
        assert_eq!(
            Request::parse(&["set", "--output=all", "/a.png"]),
            set("/a.png", Some(Output::All))
        );
        assert_eq!(
            Request::parse(&["shuffle", "--output", "HDMI-A-5"]),
            Some(Request::Shuffle(named("HDMI-A-5")))
        );
        assert_eq!(Request::parse(&["shuffle"]), Some(Request::Shuffle(None)));

        // no name, two names, a list, a flag for awww, and an output for a status
        for odd in [
            &["set", "/a.png", "--output"][..],
            &["set", "/a.png", "--output", "A", "--output", "B"],
            &["set", "/a.png", "--output", "A,B"],
            &["set", "/a.png", "--output", "-x"],
            &["set", "/a.png", "--output", ""],
            &["set", "/a.png", "--output", "A B"],
            &["status", "--output", "A"],
        ] {
            assert_eq!(Request::parse(odd), None, "{odd:?}");
        }

        // what the client sends on is what the shell parses
        let request = Request::Set(PathBuf::from("/a.png"), named("eDP-1"));
        let words = request.arguments();
        let words: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();

        assert_eq!(Request::parse(&words), Some(request));
    }

    #[test]
    fn a_pick_on_another_output_leaves_the_palette_to_the_driver() {
        let named = |name: &str| Output::Named(name.to_owned());

        assert!(drives(&Output::All, Some("eDP-1")));
        assert!(drives(&named("eDP-1"), Some("eDP-1")));
        assert!(!drives(&named("HDMI-A-5"), Some("eDP-1")));

        // no driver known yet: the pick is the palette, as before outputs were told apart
        assert!(drives(&named("HDMI-A-5"), None));
    }

    #[test]
    fn query_lines_name_each_output_and_what_it_shows() {
        let said = ": eDP-1: 1920x1080, scale: 1, currently displaying: image: /a b.png\n\
                    : HDMI-A-5: 1280x1024, scale: 1, currently displaying: color: 111111\n\
                    ns: DP-2: 640x480, scale: 2, currently displaying: image: /c.png";

        let listed: Vec<(String, &str)> = listed(said).collect();
        assert_eq!(listed[1], (String::from("HDMI-A-5"), "color: 111111"));
        assert_eq!(listed.len(), 3);

        let shown = shown(said);
        assert_eq!(shown.len(), 2);
        assert_eq!(shown["eDP-1"], PathBuf::from("/a b.png"));

        let text = outputs(said, Some("DP-2"));
        assert!(text.contains("\nHDMI-A-5: color: 111111\n"));
        assert!(text.ends_with("DP-2: image: /c.png (palette)"));
    }

    // a stand-in for awww: lists two outputs and records the arguments of `img`
    fn awww(log: &Path) -> Vec<String> {
        let script = format!(
            "if [ \"$1\" = query ]; then \
               echo ': eDP-1: 1920x1080, scale: 1, currently displaying: image: /a.png'; \
               echo ': HDMI-A-5: 1280x1024, scale: 1, currently displaying: image: /a.png'; \
             else echo \"$@\" > {}; fi",
            log.display()
        );

        ["sh", "-c", &script, "awww"].map(String::from).into()
    }

    #[test]
    fn a_named_output_is_the_only_one_awww_is_told_to_change() {
        let image = TestFile::new("png", b"image");
        let log = TestFile::new("log", b"");
        let awww = awww(&log.0);
        let awww: Vec<&str> = awww.iter().map(String::as_str).collect();
        let transition = config::get().transition;
        let sent = || fs::read_to_string(&log.0).unwrap();

        let show = |output| show(&awww, &image.0, &output, transition, None, LIMIT);

        show(Output::Named(String::from("HDMI-A-5"))).unwrap();
        assert!(sent().split(' ').any(|word| word == "--outputs=HDMI-A-5"));

        // all of them: awww's own default, no list
        show(Output::All).unwrap();
        assert!(!sent().contains("--outputs"));

        // an output awww does not list is not asked for, so nothing is set on any
        fs::write(&log.0, "").unwrap();
        assert_eq!(
            show(Output::Named(String::from("DP-9"))),
            Err(Unset::NoOutput(String::from("DP-9")))
        );
        assert_eq!(sent(), "");
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
}
