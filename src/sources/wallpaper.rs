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

use std::collections::{HashMap, VecDeque};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{self, BufRead, Read};
use std::panic;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime};

use kanade_runtime::service::Service;

use super::wake;
use crate::{config, modules, supervise};

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

            // the lock screen shows the new one from its first frame
            if shown.is_ok() && modules::on("lock") {
                look();
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

/*
 * the image awww shows on each output, by its name, as the lock screen draws it behind the clock;
 * looked up as a lock is asked (`look`), as a wallpaper may have been set by anything since
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

/*
 * asks awww what it shows, off the caller's thread; a daemon still starting, as at login, is asked
 * again a few times, and one never answering leaves what was known
 */
pub fn look() {
    /*
     * which look is newest, and which wrote last: one still asking again never overwrites what a
     * later one found, nor is what it finds thrown away for a later one that found nothing
     */
    static LOOKS: AtomicU64 = AtomicU64::new(0);
    static WROTE: Mutex<u64> = Mutex::new(0);

    let this = LOOKS.fetch_add(1, Ordering::Relaxed) + 1;

    // awww looked for off the caller's thread too, as it may be asked on the way to sleep
    let spawned = std::thread::Builder::new()
        .name(String::from("wallpaper look"))
        .spawn(move || {
            if !wake::found(AWWW) {
                return;
            }

            let mut said = wake::query_within(AWWW, &["query"], LIMIT);

            for _ in 0..TRIES {
                if said.is_ok() {
                    break;
                }

                std::thread::sleep(AGAIN);
                said = wake::query_within(AWWW, &["query"], LIMIT);
            }

            let Ok(said) = said else {
                return;
            };
            // a wallpaper that is no image the lock screen decodes is as none: it shows solid
            let by: HashMap<String, Showing> = shown(&said)
                .into_iter()
                .filter(|(_, path)| format(path).is_some())
                .map(|(output, path)| {
                    let light = lightness(&path);

                    (output, Showing { path, light })
                })
                .collect();

            // held while written, so looks write in turn
            let mut wrote = WROTE.lock().unwrap_or_else(PoisonError::into_inner);

            // a look older than one written is dropped, so a later one is never undone
            if *wrote >= this {
                return;
            }
            *wrote = this;

            // read first, as a write redraws the lock screens
            if Shown::read().by != by {
                Shown::write().by = by;
            }
        });

    if let Err(error) = spawned {
        eprintln!("kanade: cannot ask awww for the wallpaper: {error}");
    }
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
    let decoding = DECODING.lock().unwrap_or_else(PoisonError::into_inner);

    // under DECODING, so two looks at once decode it once
    if let Some(light) = known(&KNOWN.lock().unwrap_or_else(PoisonError::into_inner)) {
        return light;
    }

    // a decoder that panics on a bad file lets the look go on, with the wallpaper's lightness unknown
    let light = panic::catch_unwind(|| decoded_light(path)).ok().flatten();
    drop(decoding);

    let mut known = KNOWN.lock().unwrap_or_else(PoisonError::into_inner);

    // a few wallpapers are enough to know; a slideshow through hundreds keeps none of them for long
    if known.len() >= KNOWN_MOST {
        known.clear();
    }
    known.insert(path.to_owned(), (changed, light));

    light
}

// what the lock screen decodes: all but svg told by how they start, svg by name
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Webp,
    Gif,
    Svg,
}

pub fn format(path: &Path) -> Option<Format> {
    let mut start = [0; 12];
    let read = fs::File::open(path)
        .and_then(|mut file| file.read(&mut start))
        .ok()?;
    let start = &start[..read];

    if start.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if start.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(Format::Jpeg)
    } else if start.starts_with(b"RIFF")
        && start.get(8..).is_some_and(|rest| rest.starts_with(b"WEBP"))
    {
        Some(Format::Webp)
    } else if start.starts_with(b"GIF8") {
        Some(Format::Gif)
    } else {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
            .then_some(Format::Svg)
    }
}

// how many rows of a png are read for its lightness: one in so many
const ROWS: usize = 8;

// the most pixels an image read for its lightness may have, an 8K screen's
const MOST_PIXELS: usize = 7680 * 4320;

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
            let (_, _, rgba) = decoded(path)?;
            return light(&rgba, 4);
        }
    };

    light(&pixels, components)
}

// the longest side an svg is drawn at: sharp enough to cover a 4K output, whatever size it asks
const VECTOR_SIDE: f32 = 3840.0;

// the most bytes an svg is read from
const MOST_SVG: u64 = 8 << 20;

/*
 * an image whole, as 8-bit RGBA with plain alpha, for the lock screen to draw: the first frame of
 * a webp or gif, an svg drawn at `VECTOR_SIDE`; none past `MOST_PIXELS`, told by its header
 */
pub fn decoded(path: &Path) -> Option<(u32, u32, Vec<u8>)> {
    match format(path)? {
        Format::Png => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let mut decoder = png::Decoder::new(file);
            decoder.set_transformations(png::Transformations::normalize_to_color8());

            let mut reader = decoder.read_info().ok()?;
            let info = reader.info();
            let (width, height) = (info.width, info.height);
            if width as usize * height as usize > MOST_PIXELS {
                return None;
            }

            let mut pixels = vec![0; reader.output_buffer_size()?];
            let frame = reader.next_frame(&mut pixels).ok()?;
            pixels.truncate(frame.buffer_size());

            let rgba = match frame.color_type.samples() {
                4 => pixels,
                3 => rgba(&pixels, 3, |pixel| [pixel[0], pixel[1], pixel[2], 255]),
                2 => rgba(&pixels, 2, |pixel| [pixel[0], pixel[0], pixel[0], pixel[1]]),
                1 => rgba(&pixels, 1, |pixel| [pixel[0], pixel[0], pixel[0], 255]),
                _ => return None,
            };

            Some((width, height, rgba))
        }
        Format::Jpeg => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let options = zune_jpeg::zune_core::options::DecoderOptions::default()
                .jpeg_set_out_colorspace(zune_jpeg::zune_core::colorspace::ColorSpace::RGBA);
            let mut decoder = zune_jpeg::JpegDecoder::new_with_options(file, options);
            decoder.decode_headers().ok()?;

            let (width, height) = decoder.dimensions()?;
            if width * height > MOST_PIXELS {
                return None;
            }

            let pixels = decoder.decode().ok()?;

            Some((
                u32::try_from(width).ok()?,
                u32::try_from(height).ok()?,
                pixels,
            ))
        }
        Format::Webp => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let mut decoder = image_webp::WebPDecoder::new(file).ok()?;

            let (width, height) = decoder.dimensions();
            if width as usize * height as usize > MOST_PIXELS {
                return None;
            }

            let mut pixels = vec![0; decoder.output_buffer_size()?];
            decoder.read_image(&mut pixels).ok()?;

            let rgba = if decoder.has_alpha() {
                pixels
            } else {
                rgba(&pixels, 3, |pixel| [pixel[0], pixel[1], pixel[2], 255])
            };

            Some((width, height, rgba))
        }
        Format::Gif => {
            let file = io::BufReader::new(fs::File::open(path).ok()?);
            let mut options = gif::DecodeOptions::new();
            options.set_color_output(gif::ColorOutput::RGBA);

            // the crate's own 50 MB would refuse a screen past 12 MP, below `MOST_PIXELS`
            options.set_memory_limit(gif::MemoryLimit::Bytes(std::num::NonZeroU64::new(
                MOST_PIXELS as u64 * 4,
            )?));

            let mut decoder = options.read_info(file).ok()?;
            let (width, height) = (usize::from(decoder.width()), usize::from(decoder.height()));
            if width * height > MOST_PIXELS {
                return None;
            }

            // the first frame, which may cover only part of the screen, on a clear one
            let frame = decoder.read_next_frame().ok()??;
            let mut canvas = vec![0; width * height * 4];
            let (left, top) = (usize::from(frame.left), usize::from(frame.top));
            let across = usize::from(frame.width);

            // what lies past the screen's edge is clipped
            let visible = across.min(width.saturating_sub(left)) * 4;

            for (row, pixels) in frame.buffer.chunks_exact(across * 4).enumerate() {
                let start = ((top + row) * width + left) * 4;

                if top + row >= height {
                    break;
                }
                canvas[start..start + visible].copy_from_slice(&pixels[..visible]);
            }

            Some((
                u32::from(decoder.width()),
                u32::from(decoder.height()),
                canvas,
            ))
        }
        Format::Svg => {
            // read up to a byte past the cap, so a longer one is refused and a special file is no wait
            let mut bytes = Vec::new();
            fs::File::open(path)
                .ok()?
                .take(MOST_SVG + 1)
                .read_to_end(&mut bytes)
                .ok()?;
            if bytes.len() as u64 > MOST_SVG {
                return None;
            }

            let tree =
                resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default()).ok()?;
            let size = tree.size();
            let scale = VECTOR_SIDE / size.width().max(size.height());
            let (width, height) = (
                (size.width() * scale).ceil() as u32,
                (size.height() * scale).ceil() as u32,
            );

            // none for an svg with no size to draw at
            let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::from_scale(scale, scale),
                &mut pixmap.as_mut(),
            );

            // tiny-skia keeps premultiplied alpha, the lock screen takes plain
            let rgba = pixmap
                .pixels()
                .iter()
                .flat_map(|pixel| {
                    let plain = pixel.demultiply();
                    [plain.red(), plain.green(), plain.blue(), plain.alpha()]
                })
                .collect();

            Some((width, height, rgba))
        }
    }
}

// pixels of `components` each, made RGBA one by one
fn rgba(pixels: &[u8], components: usize, each: impl Fn(&[u8]) -> [u8; 4]) -> Vec<u8> {
    pixels.chunks_exact(components).flat_map(each).collect()
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
