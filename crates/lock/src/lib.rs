//! The lock screen from outside the runtime (ADR 0025). niri waits a second for a lock surface on every
//! output and shows red where none came. So this crate holds `ext-session-lock` over its own
//! Wayland connection, and draws each lock surface on the CPU the moment niri sizes it: the
//! backdrop, worked out ahead from the scene the shell hands it, the clock, who is signed in and a
//! pill of liquid glass for the password.
//!
//! The shell keeps what the lock means: it hands a scene per output, gets the password typed, and
//! asks to unlock once PAM accepts it.

#![forbid(unsafe_code)]

mod client;
mod paint;
mod picture;
mod refract;
pub mod stage;
mod text;

use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::SystemTime;

use smithay_client_toolkit::reexports::calloop::channel::{self, Sender};

pub use picture::Picture;
pub use text::FontFile;

// a color with plain alpha
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
    pub alpha: u8,
}

impl Color {
    pub const fn rgba(red: u8, green: u8, blue: u8, alpha: u8) -> Self {
        Self {
            red,
            green,
            blue,
            alpha,
        }
    }

    // its alpha times `opacity`
    pub(crate) fn faded(self, opacity: f32) -> Self {
        Self {
            alpha: (f32::from(self.alpha) * opacity.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }

    pub(crate) fn skia(self) -> tiny_skia::Color {
        tiny_skia::Color::from_rgba8(self.red, self.green, self.blue, self.alpha)
    }
}

// a picture's file as last written, so one written over at the same path decodes again
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Image {
    pub path: PathBuf,
    pub modified: Option<SystemTime>,
}

impl Image {
    pub fn new(path: PathBuf) -> Image {
        let modified = std::fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .ok();
        Image { path, modified }
    }
}

// what the lock screen shows behind everything
#[derive(Debug, Clone, PartialEq)]
pub enum Backdrop {
    // a wallpaper covering the output, blurred or not, under a shade; `otherwise` until it decodes
    Wallpaper {
        image: Image,
        blurred: bool,
        shade: Color,
        otherwise: Color,
    },
    Solid(Color),
}

// the faces the lock screen's text is set in, the time in its own
#[derive(Debug, Clone, PartialEq)]
pub struct Fonts {
    pub regular: FontFile,
    pub medium: FontFile,
    pub semibold: FontFile,
    pub display: FontFile,

    // a face with the characters the others lack, as a name in another script, set to the weight
    // of each it stands in for; none needed
    pub fallback: Option<FontFile>,
}

// what the text and the glass are drawn in
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    pub text: Color,
    pub muted: Color,
    pub critical: Color,

    // dark glass under light text, else light glass under dark text
    pub dark: bool,

    // how much the glass's rim catches the light, 0 to 1
    pub highlight: f32,

    // the tint of glass that only lays a tint over the backdrop, as the shell's materials but
    // liquid glass; none frosts the backdrop and bends it into the rim, tinted by its light
    pub tint: Option<Color>,
}

// what one output's lock screen shows
#[derive(Debug, Clone, PartialEq)]
pub struct Scene {
    pub backdrop: Backdrop,
    pub date: String,
    pub time: String,

    // who is signed in, and their picture, else their initial on glass
    pub name: String,
    pub face: Option<Image>,

    // under the field, in `palette.critical` if `critical`
    pub status: String,
    pub critical: bool,

    // passwords refused so far: each one more shakes the field
    pub refused: u64,

    pub palette: Palette,
    pub fonts: Fonts,
}

/*
 * what the lock did, as niri said; a lock is told by the newest `Locker::lock` it serves, so the
 * shell knows news of a lock from before a request it made
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum News {
    // every output shows a lock surface, and niri holds the lock; not said of one an unlock asked
    // while niri locked ends at once, only that it `Unlocked`. Said again of a lock held when
    // asked for again
    Locked(u64),

    // niri ended a lock it held, or never held it, as while another locker holds the session
    Finished(u64),

    // the lock asked to end did
    Unlocked(u64),

    // the connection to the compositor is gone, and with it the lock surfaces: niri stays locked
    Gone,
}

// a decoder of the pictures shown: the wallpaper and the face
pub type Decode = fn(&Path) -> Option<Picture>;

// a password typed, when Enter is pressed; whether the shell took it, which empties the field
pub type Submit = Box<dyn FnMut(String) -> bool + Send>;

pub type Report = Box<dyn FnMut(News) + Send>;

// what the shell asks of the lock's thread
enum Ask {
    Lock(u64),
    Unlock,
    Scene(String, Box<Scene>),
}

// the lock screen's own connection and thread
pub struct Locker {
    asks: Sender<Ask>,
}

impl Locker {
    /*
     * connects to the compositor and starts the thread that holds the lock; none without a
     * compositor, `ext-session-lock`, shared memory or a thread
     */
    pub fn start(decode: Decode, submit: Submit, report: Report) -> Result<Self, String> {
        let (asks, asked) = channel::channel();
        let (started, starting) = std::sync::mpsc::channel();

        thread::Builder::new()
            .name(String::from("kanade-lock"))
            .spawn(move || {
                // the report, kept here too, so a panic on this thread still says the lock is gone
                let report = Arc::new(Mutex::new(report));
                let reporting = Arc::clone(&report);
                let reported: Report = Box::new(move |news| {
                    (reporting.lock().unwrap_or_else(PoisonError::into_inner))(news);
                });
                let up = AtomicBool::new(false);

                let ran = panic::catch_unwind(AssertUnwindSafe(|| {
                    client::run(asked, decode, submit, reported, &started, &up);
                }));

                // one that never started said so to `start`, which fails
                if ran.is_err() && up.load(Ordering::Relaxed) {
                    eprintln!("kanade: the lock's thread panicked");
                    (report.lock().unwrap_or_else(PoisonError::into_inner))(News::Gone);
                }
            })
            .map_err(|error| format!("cannot start the lock's thread: {error}"))?;

        starting
            .recv()
            .unwrap_or_else(|_| Err(String::from("the lock's thread ended as it started")))?;

        Ok(Self { asks })
    }

    /*
     * locks the session for `request`, unless it is locked or being locked, which then serves
     * it; an unlock still owed is dropped
     */
    pub fn lock(&self, request: u64) {
        let _ = self.asks.send(Ask::Lock(request));
    }

    // ends the lock, once niri holds it if it does not yet
    pub fn unlock(&self) {
        let _ = self.asks.send(Ask::Unlock);
    }

    /*
     * what the lock screen of `output`, by its name, shows; handed while unlocked too, so its
     * backdrop is worked out before a lock asks for it
     */
    pub fn scene(&self, output: &str, scene: Scene) {
        let _ = self
            .asks
            .send(Ask::Scene(output.to_owned(), Box::new(scene)));
    }
}
