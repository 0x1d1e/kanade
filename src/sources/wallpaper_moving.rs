//! A moving wallpaper (ADR 0144): an animated GIF or APNG the shell draws itself, on a window of its
//! own on each output. awww stays the renderer of every still: it is asked for the same file as
//! before, Kanade paints over it, and a file that is not drawn leaves awww's picture where it was.
//! So the palette, the hooks and the lock screen's backdrop (ADR 0024) all read the one file awww
//! was given, whichever draws it.
//!
//! What a file is comes from its bytes, never its name, and only with `wallpaper.moving` on. A file
//! past the caps, one that is not moving, or one that fails to decode shows as awww drew it; the
//! last says so once, and never names the path.
//!
//! The file is read once, bounded, to tell its size and how many shots it has. Its shots are then
//! decoded one at a time by a thread of its own, into a ring of at most `RING`, and paced by the
//! thread that hands each to the runtime's image path as plain RGBA: the decode is never on a draw
//! thread, nothing is encoded, and a long animation costs no more memory than a short one.
//!
//! Pacing pauses, without losing the shot on screen, while the session is locked, while niri's
//! overview is open, and over an output a fullscreen window covers. Each of those is a Service the
//! shell already follows, so nothing here polls, and a paused wallpaper draws no frame at all.

use std::collections::{HashMap, VecDeque};
use std::f64;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;
use kanade_runtime::{
    Bitmap, Image, Keyboard, Layer, LayerWindow, Monitor, Monitors, Rectangle, Size, WindowSize,
    Zone,
};
use kanade_runtime::{MOST_PIXELS, MutexExt};

use super::wallpaper::Output;
use super::{fullscreen, niri::Niri};
use crate::lock;

// the most a moving wallpaper's file may be
const MOST_BYTES: u64 = 64 << 20;

// the most shots a file may hold; a longer animation is left to awww
const MOST_SHOTS: usize = 3000;

// the most shots decoded ahead of the one drawn, so a slow one holds nothing up and a long
// animation costs no more memory than a short one
const RING: usize = 3;

// the shortest a shot is shown; one whose own delay is 0 or under this shows for UNSET, as a
// browser shows such a GIF
const LEAST: Duration = Duration::from_millis(20);
const UNSET: Duration = Duration::from_millis(100);

/*
 * what each output's window draws now, by the output's name, none while only awww's still shows:
 * one part per output, so a shot advances that output's window alone
 */
#[derive(Default)]
pub struct Moving {
    shots: HashMap<String, Arc<Bitmap>>,
}

impl Service for Moving {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

impl Moving {
    // the shot `output` draws now, none while awww's still is all there is
    pub fn shot(&self, output: &str) -> Option<Arc<Bitmap>> {
        self.shots.get(output).cloned()
    }
}

/*
 * whether a moving wallpaper advances, as the Services the shell already follows say: paused while
 * niri's overview is open, and over each output a fullscreen window covers. A watcher keeps it, so a
 * change reaches the pacing at once and a still one wakes nothing; the session's lock is read where
 * it is decided, as no Service carries it (ADR 0144)
 */
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resting {
    pub overview: bool,

    // the outputs a fullscreen window covers, by name
    pub covered: Vec<String>,
}

impl Service for Resting {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {}
}

// reads what holds a moving wallpaper back; the same again changes nothing, so wakes nothing
pub fn follow() {
    let overview = Niri::read().seen.overview;
    let covered: Vec<String> = Monitors::read()
        .all()
        .iter()
        .filter(|monitor| fullscreen::covers(monitor))
        .map(|monitor| monitor.name.clone())
        .collect();

    let next = Resting { overview, covered };

    let mut resting = Resting::write();

    if *resting == next {
        resting.quiet();
    } else {
        *resting = next;
    }
}

/*
 * the window of one output: the shot its part holds, filling the output, on the Background layer over
 * the still awww drew, which stays there for a pause, a failure or an output with nothing to draw;
 * hidden while no shot is set there, so the wallpaper is awww's alone again
 */
pub fn window(monitor: &Monitor) -> LayerWindow {
    let shot = Moving::read_part(&monitor.name).shot(&monitor.name);

    let Some(shot) = shot else {
        return shell()
            .visible(false)
            .child(Rectangle::new().width(0.0).height(0.0));
    };

    shell().visible(true).child(
        Rectangle::new()
            .width(Size::Parent)
            .height(Size::Parent)
            .fill(Image::bitmap(shot)),
    )
}

fn shell() -> LayerWindow {
    LayerWindow::new()
        .width(WindowSize::Full)
        .height(WindowSize::Full)
        .layer(Layer::Background)
        .keyboard(Keyboard::None)
        .space(Zone::Ignore)
        .namespace("kanade-wallpaper")
        .click_through()
}

/*
 * what the shell is being asked to draw, the newest request winning: a file that is not drawn hides
 * every window and leaves awww's picture where it was
 */
#[derive(Default)]
struct Asked {
    newest: Option<(PathBuf, Output)>,
}

static ASKED: Mutex<Asked> = Mutex::new(Asked { newest: None });

// every waker takes the lock first, so a wake is never lost between a look and a wait
static WAKE: Condvar = Condvar::new();

// the file last told it cannot be drawn, so a file says so once and never twice
static SAID: Mutex<Option<PathBuf>> = Mutex::new(None);

// asks for `path` to be drawn on `output` in place of what was drawn before
pub fn play(path: PathBuf, output: Output) {
    ASKED.locked().newest = Some((path, output));

    WAKE.notify_all();
}

// what holds a moving wallpaper back changed: the lock, the overview, a fullscreen window
pub fn paused() {
    let _asked = ASKED.locked();

    WAKE.notify_all();
}

/*
 * draws what was last asked, one shot at a time at the file's own pace, for as long as Kanade runs;
 * a request replaces what plays, and while nothing plays or everything is paused it waits on the
 * wake a change gives it, waking for no shot and drawing no frame
 */
pub fn run() {
    let mut playing: Option<Playing> = None;
    let mut due: Option<Instant> = None;

    loop {
        if let Some((path, output)) = ASKED.locked().newest.take() {
            if let Some(old) = &playing {
                old.ring.stop();
            }

            due = None;

            // every output goes back to awww's own still until this file's first shot comes
            hide();

            playing = Playing::start(path, output);
        }

        let Some(current) = &playing else {
            wait(None);

            continue;
        };

        // the outputs this file was set on that are advancing now
        let live = advancing(&current.output);

        if live.is_empty() {
            wait(None);

            continue;
        }

        if let Some(at) = due && at > Instant::now() {
            wait(Some(at));

            continue;
        }

        let Some(shot) = current.ring.next() else {
            hide();
            failed(&current.path);

            playing = None;

            continue;
        };

        let now = Instant::now();

        due = Some(due_after(due.unwrap_or(now), shot.delay, now));

        show(&live, &shot);
    }
}

// waits on the wake until `until`, or for any waker when there is none; every waker takes the lock first
fn wait(until: Option<Instant>) {
    let asked = ASKED.locked();

    // the guard each wait hands back is dropped by the map, with the lock it holds, at once
    let _woken: Result<(), ()> = match until {
        Some(at) => {
            let left = at.saturating_duration_since(Instant::now());

            WAKE.wait_timeout(asked, left).map(drop).map_err(drop)
        }
        None => WAKE.wait(asked).map(drop).map_err(drop),
    };
}

// what plays: its file, the outputs it was set on, and its ring of decoded shots
struct Playing {
    path: PathBuf,
    output: Output,
    ring: Arc<Ring>,
}

impl Playing {
    // none for a file that is a still, is past the caps or cannot be decoded, and for one that is
    // asked for with `wallpaper.moving` off: awww keeps showing it
    fn start(path: PathBuf, output: Output) -> Option<Playing> {
        if !crate::config::get().wallpaper_moving {
            return None;
        }

        match inspect(&path) {
            Inspection::Still => None,
            Inspection::Refused(why) => {
                failed_because(&path, why);

                None
            }
            Inspection::Moving {
                kind,
                width,
                height,
                bytes,
            } => {
                let Some(canvas) = Canvas::new(width, height) else {
                    failed(&path);

                    return None;
                };

                let ring = Arc::new(Ring::default());
                let decoding = Arc::clone(&ring);

                let spawned = thread::Builder::new()
                    .name(String::from("wallpaper-frames"))
                    .spawn(move || read(decoding, bytes, kind, canvas));

                if let Err(error) = spawned {
                    eprintln!("kanade: cannot start decoding the moving wallpaper: {error}");
                    failed(&path);

                    return None;
                }

                Some(Playing { path, output, ring })
            }
        }
    }
}

// the outputs `output` names that advance now
fn advancing(output: &Output) -> Vec<String> {
    let resting = Resting::read();
    let locked = lock::holds();

    let monitors: Vec<Monitor> = Monitors::read().all().to_vec();

    let covered = |name: &str| resting.covered.iter().any(|covered| covered == name);

    let mut live = Vec::new();

    match output {
        Output::All => {
            for monitor in &monitors {
                if advancing_now(locked, resting.overview, covered(&monitor.name)) {
                    live.push(monitor.name.clone());
                }
            }
        }
        Output::Named(name) => {
            if advancing_now(locked, resting.overview, covered(name)) {
                live.push(name.clone());
            }
        }
    }

    live
}

/*
 * whether a moving wallpaper advances on an output: not while the session is locked, niri's
 * overview is open, or a fullscreen window covers that output, which leaves the shot on screen
 */
fn advancing_now(locked: bool, overview: bool, covered: bool) -> bool {
    !(locked || overview || covered)
}

// when the shot after `delay` is due, counted from the last due time so a late one does not push
// every shot after it later; one already past is due now
fn due_after(due: Instant, delay: Duration, now: Instant) -> Instant {
    (due + pace(delay)).max(now)
}

// how long a shot with `delay` shows: one of 0, or one under LEAST, shows for UNSET
fn pace(delay: Duration) -> Duration {
    if delay < LEAST { UNSET } else { delay }
}

// hands `shot` to the window of each output, one part each, so only those windows draw again
fn show(outputs: &[String], shot: &Shot) {
    let pixels = Arc::new(shot.pixels.clone());

    let mut moving = Moving::write();

    for output in outputs {
        let bitmap = Arc::new(Bitmap::shared(shot.width, shot.height, Arc::clone(&pixels)));

        moving.shots.insert(output.clone(), bitmap);
        moving.part(output);
    }
}

// no output draws a shot of its own any more, so awww's still shows everywhere again
fn hide() {
    let mut moving = Moving::write();

    if moving.shots.is_empty() {
        moving.quiet();

        return;
    }

    let outputs: Vec<String> = moving.shots.keys().cloned().collect();

    for output in outputs {
        moving.shots.remove(&output);
        moving.part(&output);
    }
}

// one notice per file, and never the path: it stays as awww showed it
fn failed(path: &Path) {
    failed_because(path, "it cannot be decoded");
}

fn failed_because(path: &Path, why: &str) {
    let mut said = SAID.locked();

    if said.as_deref() == Some(path) {
        return;
    }

    *said = Some(path.to_owned());

    eprintln!("kanade: the moving wallpaper stays as awww drew it: {why}");
}

/*
 * what a file turns out to be: a moving picture to draw, a still awww shows, or why it is not drawn.
 * Its bytes say which, never its name
 */
enum Inspection {
    Still,
    Refused(&'static str),
    Moving {
        kind: Kind,
        width: u32,
        height: u32,
        bytes: Arc<Vec<u8>>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Gif,
    Apng,
}

fn inspect(path: &Path) -> Inspection {
    let Ok(meta) = fs::metadata(path) else {
        return Inspection::Refused("it cannot be read");
    };

    if let Some(why) = size_refused(meta.len()) {
        return Inspection::Refused(why);
    }

    let mut bytes = Vec::new();

    // a byte past the cap, so a file grown since the stat is refused too
    let read =
        fs::File::open(path).and_then(|file| file.take(MOST_BYTES + 1).read_to_end(&mut bytes));

    if read.is_err() {
        return Inspection::Refused("it cannot be read");
    }

    if let Some(why) = size_refused(bytes.len() as u64) {
        return Inspection::Refused(why);
    }

    looked(Arc::new(bytes))
}

// why a file of `len` bytes is too large to draw, none while it fits
fn size_refused(len: u64) -> Option<&'static str> {
    (len > MOST_BYTES).then_some("it is larger than 64 MiB")
}

// what a file's own bytes say: a GIF or APNG of two shots or more, else a still
fn looked(bytes: Arc<Vec<u8>>) -> Inspection {
    let (kind, width, height, shots) = if bytes.starts_with(b"GIF8") {
        let Some((width, height)) = gif_size(&bytes) else {
            return Inspection::Refused("it is a GIF that cannot be read");
        };

        let shots = gif_shots(&bytes);

        (Kind::Gif, width, height, shots)
    } else if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        let Some((width, height, shots)) = png_size(&bytes) else {
            return Inspection::Refused("it is a PNG that cannot be read");
        };

        (Kind::Apng, width, height, shots)
    } else {
        return Inspection::Still;
    };

    let Some(shots) = shots else {
        return Inspection::Still;
    };

    if shots < 2 {
        return Inspection::Still;
    }

    if shots > MOST_SHOTS {
        return Inspection::Refused("it has more than 3000 shots");
    }

    Inspection::Moving {
        kind,
        width,
        height,
        bytes,
    }
}

// a GIF's picture size, from its screen descriptor
fn gif_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let screen = bytes.get(6..13)?;
    let width = u32::from(u16::from_le_bytes([screen[0], screen[1]]));
    let height = u32::from(u16::from_le_bytes([screen[2], screen[3]]));

    (width > 0 && height > 0).then_some((width, height))
}

// how many shots a GIF holds, counted from its blocks without decoding any of them
fn gif_shots(bytes: &[u8]) -> Option<usize> {
    let screen = bytes.get(6..13)?;
    let packed = screen[4];

    let mut at = 13;

    if packed & 0x80 != 0 {
        at += 3 * (1 << ((packed & 0x07) + 1));
    }

    let mut shots = 0;

    loop {
        match *bytes.get(at)? {
            // the trailer
            0x3b => return Some(shots),

            // an extension: its label, then its blocks
            0x21 => at = skip_blocks(bytes, at + 2)?,

            // a shot: its descriptor, its own color table if it has one, then its pixels
            0x2c => {
                let descriptor = bytes.get(at + 1..at + 10)?;
                let packed = descriptor[8];

                let mut next = at + 10;

                if packed & 0x80 != 0 {
                    next += 3 * (1 << ((packed & 0x07) + 1));
                }

                // past the LZW minimum code size, then the pixel blocks
                at = skip_blocks(bytes, next + 1)?;
                shots += 1;
            }
            _ => return None,
        }
    }
}

// the byte after a run of blocks, each a length byte and its data, ended by a block of none
fn skip_blocks(bytes: &[u8], mut at: usize) -> Option<usize> {
    loop {
        let length = usize::from(*bytes.get(at)?);

        at += 1 + length;

        if length == 0 {
            return Some(at);
        }
    }
}

// a PNG's picture size, and how many shots its animation holds, none for a still or unreadable one
fn png_size(bytes: &[u8]) -> Option<(u32, u32, Option<usize>)> {
    let header = bytes.get(16..24)?;

    let width = u32::from_be_bytes(header[0..4].try_into().ok()?);
    let height = u32::from_be_bytes(header[4..8].try_into().ok()?);

    if width == 0 || height == 0 {
        return None;
    }

    let mut at = 8;

    while let Some(chunk) = bytes.get(at..at + 8) {
        let length = u32::from_be_bytes(chunk[0..4].try_into().ok()?) as usize;

        let shots = match &chunk[4..8] {
            // the animation's control, which the spec puts before the picture
            b"acTL" => {
                let data = bytes.get(at + 8..at + 12)?;
                let shots = u32::from_be_bytes(data.try_into().ok()?);

                Some(shots as usize)
            }

            // a picture that ends before any animation control is a still one
            b"IDAT" | b"IEND" => None,
            _ => {
                at = at.checked_add(12 + length)?;

                continue;
            }
        };

        return Some((width, height, shots));
    }

    Some((width, height, None))
}

/*
 * the shots of `bytes`, decoded by a thread of its own into `ring`: the file's end draws it again
 * from its first shot, as a wallpaper loops
 */
fn read(ring: Arc<Ring>, bytes: Arc<Vec<u8>>, kind: Kind, mut canvas: Canvas) {
    loop {
        let ended = match kind {
            Kind::Gif => gif_pass(&bytes, &mut canvas, &mut |shot| ring.push(shot)),
            Kind::Apng => apng_pass(&bytes, &mut canvas, &mut |shot| ring.push(shot)),
        };

        if !ended {
            break;
        }
    }

    ring.end();
}

/*
 * the shots of a GIF, drawn one after the other on `canvas` as its disposal says, each handed to
 * `push`; false once the ring took no more, and true when the file ended and draws again
 */
fn gif_pass(bytes: &[u8], canvas: &mut Canvas, push: &mut dyn FnMut(Shot) -> bool) -> bool {
    let mut options = gif::DecodeOptions::new();

    options.set_color_output(gif::ColorOutput::RGBA);

    if let Some(limit) = std::num::NonZeroU64::new(canvas.pixels.len() as u64) {
        options.set_memory_limit(gif::MemoryLimit::Bytes(limit));
    }

    let Ok(mut decoder) = options.read_info(Cursor::new(bytes)) else {
        return false;
    };

    loop {
        let frame = match decoder.read_next_frame() {
            Ok(Some(frame)) => frame,
            Ok(None) => return true,
            Err(_) => return false,
        };

        let (left, top) = (usize::from(frame.left), usize::from(frame.top));
        let (width, height) = (usize::from(frame.width), usize::from(frame.height));

        let dispose = frame.dispose;
        let delay = pace(Duration::from_millis(10 * u64::from(frame.delay)));

        // what showed before this shot, for a disposal that puts it back
        let kept = (dispose == gif::DisposalMethod::Previous).then(|| canvas.pixels.clone());

        canvas.draw((left, top), (width, height), &frame.buffer, true);

        if !push(canvas.shot(delay)) {
            return false;
        }

        match dispose {
            gif::DisposalMethod::Background => canvas.clear((left, top), (width, height)),
            gif::DisposalMethod::Previous => {
                if let Some(kept) = kept {
                    canvas.pixels = kept;
                }
            }
            gif::DisposalMethod::Any | gif::DisposalMethod::Keep => {}
        }
    }
}

/*
 * the shots of an APNG, composed as its frame controls say (blend and dispose), each handed to
 * `push`; false once the ring took no more, and true when the file ended and draws again
 */
fn apng_pass(bytes: &[u8], canvas: &mut Canvas, push: &mut dyn FnMut(Shot) -> bool) -> bool {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));

    decoder.set_transformations(png::Transformations::normalize_to_color8());

    let Ok(mut reader) = decoder.read_info() else {
        return false;
    };

    // as large as the whole picture, which no shot of it is larger than
    let mut buffer = vec![0; canvas.pixels.len()];

    let mut shots = 0;

    loop {
        let Ok(read) = reader.next_frame(&mut buffer) else {
            return shots > 0;
        };

        let Some(&control) = reader.info().frame_control() else {
            // the default picture of one whose animation leaves it out: not a shot of it
            continue;
        };

        let (left, top) = (control.x_offset as usize, control.y_offset as usize);
        let (width, height) = (read.width as usize, read.height as usize);

        let pixels = rgba(&read, &buffer[..read.buffer_size()]);

        let delay = pace(apng_delay(control.delay_num, control.delay_den));

        let kept = (control.dispose_op == png::DisposeOp::Previous).then(|| canvas.pixels.clone());

        canvas.draw(
            (left, top),
            (width, height),
            &pixels,
            control.blend_op == png::BlendOp::Over,
        );

        if !push(canvas.shot(delay)) {
            return false;
        }

        shots += 1;

        match control.dispose_op {
            png::DisposeOp::Background => canvas.clear((left, top), (width, height)),
            png::DisposeOp::Previous => {
                if let Some(kept) = kept {
                    canvas.pixels = kept;
                }
            }
            png::DisposeOp::None => {}
        }
    }
}

// how long an APNG's shot stays: a fraction of a second, a hundredth when the denominator says none
fn apng_delay(numerator: u16, denominator: u16) -> Duration {
    let denominator = if denominator == 0 { 100 } else { denominator };

    Duration::from_secs_f64(f64::from(numerator) / f64::from(denominator))
}

// a shot's pixels as plain RGBA, whatever color type the decoder settled on
fn rgba(info: &png::OutputInfo, pixels: &[u8]) -> Vec<u8> {
    let samples = info.color_type.samples();

    if samples == 4 {
        return pixels.to_vec();
    }

    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);

    for pixel in pixels.chunks_exact(samples.max(1)) {
        match samples {
            3 => rgba.extend_from_slice(&[pixel[0], pixel[1], pixel[2], 255]),
            2 => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], pixel[1]]),
            _ => rgba.extend_from_slice(&[pixel[0], pixel[0], pixel[0], 255]),
        }
    }

    rgba
}

// one shot as the runtime draws it: plain RGBA of the picture's own size, and how long it stays
struct Shot {
    width: u32,
    height: u32,
    pixels: Vec<u8>,
    delay: Duration,
}

// the picture a file's shots are drawn on, plain RGBA and clear at first
struct Canvas {
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Canvas {
    fn new(width: u32, height: u32) -> Option<Canvas> {
        let width = usize::try_from(width).ok()?;
        let height = usize::try_from(height).ok()?;
        let pixels = width.checked_mul(height)?.checked_mul(4)?;

        (pixels > 0 && pixels <= MOST_PIXELS * 4).then(|| Canvas {
            width,
            height,
            pixels: vec![0; pixels],
        })
    }

    // the picture as it stands, for the delay this shot stays
    fn shot(&self, delay: Duration) -> Shot {
        Shot {
            width: self.width as u32,
            height: self.height as u32,
            pixels: self.pixels.clone(),
            delay,
        }
    }

    // draws a shot's own pixels at `at`, `over` blending them by their alpha or replacing what is there
    fn draw(&mut self, at: (usize, usize), size: (usize, usize), pixels: &[u8], over: bool) {
        let (left, top) = at;
        let (width, height) = size;

        let across = width.min(self.width.saturating_sub(left));

        if across == 0 || pixels.len() < width * height * 4 {
            return;
        }

        let rows = height.min(self.height.saturating_sub(top));

        for row in 0..rows {
            let from = &pixels[row * width * 4..][..across * 4];

            let to = (top + row) * self.width * 4 + left * 4;
            let into = &mut self.pixels[to..to + across * 4];

            if over {
                blend(into, from);
            } else {
                into.copy_from_slice(from);
            }
        }
    }

    // clears a shot's place, as a disposal to the background does
    fn clear(&mut self, at: (usize, usize), size: (usize, usize)) {
        let (left, top) = at;

        let across = size.0.min(self.width.saturating_sub(left));
        let rows = size.1.min(self.height.saturating_sub(top));

        for row in 0..rows {
            let to = (top + row) * self.width * 4 + left * 4;

            self.pixels[to..to + across * 4].fill(0);
        }
    }
}

// draws `over` onto `under`, both plain RGBA, by each pixel's alpha
fn blend(under: &mut [u8], over: &[u8]) {
    let under = under.as_chunks_mut::<4>().0;
    let over = over.as_chunks::<4>().0;

    for (to, from) in under.iter_mut().zip(over) {
        let source = u32::from(from[3]);

        if source == 0 {
            continue;
        }

        if source == 255 {
            to.copy_from_slice(from);

            continue;
        }

        let target = u32::from(to[3]);
        let out = source + target * (255 - source) / 255;

        if out == 0 {
            continue;
        }

        for channel in 0..3 {
            let above = u32::from(from[channel]) * source;
            let below = u32::from(to[channel]) * target * (255 - source) / 255;

            to[channel] = u8::try_from((above + below) / out).unwrap_or(u8::MAX);
        }

        to[3] = u8::try_from(out).unwrap_or(u8::MAX);
    }
}

/*
 * the shots decoded ahead of the one drawn: at most RING of them wait, and the decode waits while
 * they do, so a long animation costs no more memory than a short one
 */
#[derive(Default)]
struct Ring {
    state: Mutex<Ringed>,
    ready: Condvar,
    space: Condvar,
}

#[derive(Default)]
struct Ringed {
    shots: VecDeque<Shot>,

    // no shot comes after the ones waiting: the file cannot be decoded further
    over: bool,

    // the pacing moved on to another file
    stop: bool,
}

impl Ring {
    // the next shot, waiting for it while the decode runs; none once no more come
    fn next(&self) -> Option<Shot> {
        let mut state = self.state.locked();

        loop {
            if let Some(shot) = state.shots.pop_front() {
                drop(state);
                self.space.notify_one();

                return Some(shot);
            }

            if state.over {
                return None;
            }

            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    // waits while the ring is full; false once the pacing moved on
    fn push(&self, shot: Shot) -> bool {
        let mut state = self.state.locked();

        while state.shots.len() >= RING && !state.stop {
            state = self
                .space
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }

        if state.stop {
            return false;
        }

        state.shots.push_back(shot);

        drop(state);
        self.ready.notify_one();

        true
    }

    fn end(&self) {
        let mut state = self.state.locked();

        state.over = true;

        drop(state);
        self.ready.notify_all();
    }

    fn stop(&self) {
        let mut state = self.state.locked();

        state.stop = true;

        drop(state);
        self.space.notify_all();
        self.ready.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicU64, Ordering};

    // the key is off unless the config says otherwise: awww keeps every file, moving or not
    #[test]
    fn a_moving_wallpaper_is_off_by_default() {
        assert!(!crate::config::Config::default().wallpaper_moving);
    }

    // a file past the caps is refused rather than read
    #[test]
    fn a_file_past_the_caps_is_refused() {
        assert_eq!(size_refused(MOST_BYTES), None);
        assert!(size_refused(MOST_BYTES + 1).is_some());
    }

    // a GIF is told by its bytes, however its file is named, and a single shot of one is a still
    #[test]
    fn a_gif_is_told_by_its_bytes() {
        assert!(matches!(
            looked(Arc::new(gif(2))),
            Inspection::Moving {
                kind: Kind::Gif,
                width: 4,
                height: 4,
                ..
            }
        ));

        assert!(matches!(looked(Arc::new(gif(1))), Inspection::Still));
        assert!(matches!(
            looked(Arc::new(b"not a picture".to_vec())),
            Inspection::Still
        ));
    }

    // a png is a moving one only with an animation, which its own control chunk says
    #[test]
    fn a_png_is_moving_only_with_an_animation() {
        assert!(matches!(
            looked(Arc::new(apng(2))),
            Inspection::Moving {
                kind: Kind::Apng,
                ..
            }
        ));

        assert!(matches!(looked(Arc::new(apng(1))), Inspection::Still));
    }

    // a file of more shots than the cap allows is refused, so a long animation costs nothing
    #[test]
    fn too_many_shots_are_refused() {
        assert!(matches!(
            looked(Arc::new(gif(MOST_SHOTS + 1))),
            Inspection::Refused(why) if why.contains("3000")
        ));
    }

    // a shot shows for its own delay, and one too short for the eye for UNSET
    #[test]
    fn a_shot_shows_for_its_delay() {
        assert_eq!(pace(Duration::from_millis(80)), Duration::from_millis(80));
        assert_eq!(pace(Duration::ZERO), UNSET);
        assert_eq!(pace(Duration::from_millis(10)), UNSET);
        assert_eq!(pace(LEAST), LEAST);
    }

    // the delays count from the last due time, so a slow shot does not push every one after it
    #[test]
    fn delays_count_from_the_last_due_time() {
        let start = Instant::now();
        let mut due = start;

        for _ in 0..3 {
            due = due_after(due, Duration::from_millis(100), start);
        }

        assert_eq!(due, start + Duration::from_millis(300));

        // one already past is due now, and the ones after count from there
        let late = start + Duration::from_secs(5);
        let now = due_after(due, Duration::from_millis(100), late);

        assert_eq!(now, late);
        assert_eq!(
            due_after(now, Duration::from_millis(100), late),
            late + Duration::from_millis(100)
        );
    }

    // a moving wallpaper pauses while the session is locked, in the overview or under a fullscreen
    // window, any of them alone, and the shot on screen stays
    #[test]
    fn a_moving_wallpaper_pauses_while_it_is_not_looked_at() {
        assert!(advancing_now(false, false, false));
        assert!(!advancing_now(true, false, false));
        assert!(!advancing_now(false, true, false));
        assert!(!advancing_now(false, false, true));
        assert!(!advancing_now(true, true, true));
    }

    // a GIF's shots follow each other on the one picture, and each hands its own delay
    #[test]
    fn a_gifs_shots_follow_each_other() {
        let bytes = gif(2);

        let (width, height) = gif_size(&bytes).expect("a picture size");
        let mut canvas = Canvas::new(width, height).expect("a canvas");

        let mut shots = Vec::new();

        assert!(gif_pass(&bytes, &mut canvas, &mut |shot| {
            shots.push(shot);

            true
        }));

        assert_eq!(shots.len(), 2);
        assert_eq!(shots[0].pixels.len(), 4 * 4 * 4);
        assert_eq!(shots[0].delay, Duration::from_millis(100));

        // white, as the file's own table says
        assert_eq!(&shots[1].pixels[..4], &[255, 255, 255, 255]);
    }

    // an APNG's shots are composed as its frame controls say, and hand their own delays
    #[test]
    fn an_apngs_shots_follow_each_other() {
        let bytes = apng(2);

        let mut canvas = Canvas::new(4, 4).expect("a canvas");
        let mut shots = Vec::new();

        assert!(apng_pass(&bytes, &mut canvas, &mut |shot| {
            shots.push(shot);

            true
        }));

        assert_eq!(shots.len(), 2);

        // the first shot's own control is the encoder's, 1/30 of a second; the second is the delay
        // the test asked for
        assert_eq!(shots[0].delay, Duration::from_secs(1) / 30);
        assert_eq!(shots[1].delay, Duration::from_millis(150));

        // white, then black drawn over it
        assert_eq!(&shots[0].pixels[..4], &[255, 255, 255, 255]);
        assert_eq!(&shots[1].pixels[..4], &[0, 0, 0, 255]);
    }

    // a shot that is clear leaves what is under it, and one that is solid replaces it
    #[test]
    fn a_clear_shot_leaves_what_is_under_it() {
        let mut under = vec![10, 20, 30, 255, 40, 50, 60, 255];
        let over = vec![200, 100, 0, 0, 0, 0, 0, 255];

        blend(&mut under, &over);

        assert_eq!(under, [10, 20, 30, 255, 0, 0, 0, 255]);
    }

    // a shot cleared by its disposal shows the picture behind it, not what it drew before
    #[test]
    fn a_disposal_to_the_background_clears_a_shots_place() {
        let mut canvas = Canvas::new(2, 2).expect("a canvas");

        canvas.draw((0, 0), (2, 2), &[255; 16], false);
        canvas.clear((0, 0), (1, 1));

        assert_eq!(
            canvas.pixels,
            [
                0, 0, 0, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
            ]
        );
    }

    // the file is told from its own bytes on disk, and a name that says nothing of what it holds
    #[test]
    fn a_file_is_told_from_what_it_holds() {
        let file = temp(".txt", &gif(2));

        assert!(matches!(
            inspect(&file.path),
            Inspection::Moving {
                kind: Kind::Gif,
                ..
            }
        ));
    }

    // a file written for a test, removed when it is dropped
    struct Temp {
        path: PathBuf,
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    fn temp(extension: &str, bytes: &[u8]) -> Temp {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        let path = std::env::temp_dir().join(format!(
            "kanade-moving-test-{}-{}{extension}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));

        fs::write(&path, bytes).expect("a file to write");

        Temp { path }
    }

    // a GIF of one white 4 by 4 shot per `shots`, each showing for 100 ms
    fn gif(shots: usize) -> Vec<u8> {
        let mut bytes = Vec::new();

        {
            let mut encoder =
                gif::Encoder::new(&mut bytes, 4, 4, &[0, 0, 0, 255, 255, 255]).expect("an encoder");

            for _ in 0..shots {
                let mut frame = gif::Frame::from_indexed_pixels(4, 4, vec![1; 16], None);

                frame.delay = 10;

                encoder.write_frame(&frame).expect("a shot");
            }
        }

        bytes
    }

    // a PNG of `shots` white 4 by 4 pictures, the second black over the first, 150 ms after it
    fn apng(shots: u32) -> Vec<u8> {
        let mut bytes = Vec::new();

        {
            let mut encoder = png::Encoder::new(&mut bytes, 4, 4);

            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);

            if shots > 1 {
                encoder.set_animated(shots, 0).expect("an animation");
            }

            let mut writer = encoder.write_header().expect("a header");

            writer.write_image_data(&[255; 64]).expect("a picture");

            if shots > 1 {
                writer.set_frame_delay(15, 100).expect("a delay");
                writer
                    .write_image_data(&[0, 0, 0, 255].repeat(16))
                    .expect("a shot");
            }

            writer.finish().expect("the end");
        }

        bytes
    }
}
