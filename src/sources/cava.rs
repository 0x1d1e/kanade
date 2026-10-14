//! The audio-reactive visualizer (`media.visualizer = "audio"`, ADR 0056): the bars of what the
//! default sink plays, read from `cava`'s raw output. It is drawn, not heard, so Kanade never
//! records: cava reads the default sink, its stream linked from an `Audio/Sink` node, so the privacy
//! indicator, which counts a stream linked from a microphone or camera, never lights for it
//! (`privacy.rs`).
//!
//! Cava runs only on demand. The visualizer asks for it (`want`) each time it draws; a frame that
//! finds the last ask older than `LAPSE` ends it, so it stops once the media is paused, the
//! visualizer is not on screen or the setting changed, and a still island draws no frame. Its thread
//! starts on the first ask, so Kanade holds none while the setting is not `audio`. Without cava the
//! view falls back to the synthetic bars, and a missing cava is looked for again after `RETRY`.
//! It runs through setpriv, so it dies with Kanade.

use std::cell::Cell;
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::process::ChildStdout;
use std::sync::{Condvar, Mutex, Once};
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};

use super::wake::{self, Stop};
use crate::supervise;

pub const CAVA: &str = "cava";

// how many bars it gives, so how many the visualizer draws
pub const BARS: usize = 4;

// each bar at most this, as the config below says
pub const PEAK: u8 = 100;

// how long without an ask before cava ends: a frame draws every 33 ms while it runs
const LAPSE: Duration = Duration::from_millis(1500);

// how long cava that could not run is left alone before the next ask tries again
const RETRY: Duration = Duration::from_secs(30);

// the monitor of the default sink through PulseAudio's protocol, which PipeWire serves, and the
// bars as text lines on stdout, `;` between them. A file of its own, so the user's cava config,
// which draws in a terminal, never applies
const CONFIG: &str = "[general]
bars = 4
framerate = 30

[input]
method = pulse
source = auto

[output]
method = raw
raw_target = /dev/stdout
data_format = ascii
ascii_max_range = 100
bar_delimiter = 59
frame_delimiter = 10
";

// the latest bars, as far as cava gave them
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Spectrum {
    // none while cava gives nothing, so the view draws the synthetic bars
    pub bars: Option<[u8; BARS]>,
}

impl Service for Spectrum {
    fn new() -> Self {
        Spectrum::default()
    }

    fn listen() {}
}

#[derive(Default)]
struct Demand {
    // when the visualizer last drew and asked for cava
    asked: Option<Instant>,

    // the worker has cava running, so an ask need not wake it
    running: bool,

    // cava could not run, until then asks are not tried
    unavailable: Option<Instant>,
}

static DEMAND: Mutex<Demand> = Mutex::new(Demand {
    asked: None,
    running: false,
    unavailable: None,
});
static ASKED: Condvar = Condvar::new();
static STARTED: Once = Once::new();

/*
 * the visualizer draws and wants cava running: starts its thread the first time, then wakes it if
 * cava is not running. Call it from the view; it never waits on cava
 */
pub fn want() {
    STARTED.call_once(|| supervise::spawn("cava", follow));

    let now = Instant::now();
    let mut demand = DEMAND.locked();

    demand.asked = Some(now);

    if !demand.running && !holds_off(&demand, now) {
        ASKED.notify_one();
    }
}

// cava could not run lately, so asks leave it be until `RETRY` has passed
fn holds_off(demand: &Demand, now: Instant) -> bool {
    demand.unavailable.is_some_and(|until| now < until)
}

// the bars to draw, none while cava gives none
pub fn bars() -> Option<[u8; BARS]> {
    Spectrum::read().bars
}

// whether the last ask is recent
fn wanted(demand: &Demand) -> bool {
    demand.asked.is_some_and(|asked| asked.elapsed() < LAPSE)
}

// whether cava should run now and does not
fn startable(demand: &Demand) -> bool {
    wanted(demand) && !demand.running && !holds_off(demand, Instant::now())
}

/*
 * the cava thread's body, for good: waits for an ask, runs cava while they keep coming, then waits
 * again. It lives as long as Kanade, as the program dies with its thread (`wake::run`)
 */
fn follow() {
    loop {
        {
            let mut demand = ASKED
                .wait_while(DEMAND.locked(), |demand| !startable(demand))
                .unwrap_or_else(std::sync::PoisonError::into_inner);

            demand.running = true;
        }

        let outcome = run();

        {
            let mut demand = DEMAND.locked();

            demand.running = false;

            if let Err(why) = &outcome {
                // said once per try, which is once per `RETRY` at most
                eprintln!("kanade: cava: {why}, drawing the synthetic bars");
                demand.unavailable = Some(Instant::now() + RETRY);
            }
        }

        let mut spectrum = Spectrum::write();

        spectrum.bars = None;
        spectrum.quiet();
    }
}

// runs cava until the asks lapse, or says why it cannot run
fn run() -> Result<(), String> {
    let path = config_file().map_err(|error| format!("cannot write its config ({error})"))?;
    let path = path.to_string_lossy().into_owned();

    let stop = Stop::default();
    let silent = Cell::new(false);
    let failed = wake::run(CAVA, &["-p", &path], &stop, |output| {
        read(output, &stop, &silent)
    });

    if silent.get() {
        return Err(format!("{CAVA} ran but gave no bars"));
    }

    match failed {
        Some(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(format!("{CAVA} not found on PATH"))
        }
        Some(error) => Err(format!("cannot run {CAVA} ({error})")),
        None => Ok(()),
    }
}

/*
 * reads bars until cava ends or the asks lapse, which stops it. Each line is posted, which draws
 * the windows that show them
 */
fn read(output: BufReader<ChildStdout>, stop: &Stop, silent: &Cell<bool>) -> io::Error {
    let mut framed = false;

    for line in output.lines() {
        if !wanted(&DEMAND.locked()) {
            stop.stop();

            return io::Error::other("not wanted");
        }

        match line {
            Ok(line) => {
                if let Some(bars) = parse(&line) {
                    framed = true;
                    Spectrum::write().bars = Some(bars);
                }
            }
            Err(error) => return error,
        }
    }

    // the bars of a cava that ended freeze if kept, and one that never gave any would be run again
    // for good, as no line arrives to find the asks lapsed
    Spectrum::write().bars = None;

    if !framed {
        silent.set(true);
        stop.stop();
    }

    io::Error::other("ended")
}

// one frame: the bars' heights, `;` between them and one at the end, each up to `PEAK`
fn parse(line: &str) -> Option<[u8; BARS]> {
    let mut bars = [0; BARS];
    let mut values = line.trim_end().split(';').filter(|value| !value.is_empty());

    for bar in &mut bars {
        *bar = values.next()?.trim().parse::<u8>().ok()?.min(PEAK);
    }

    values.next().is_none().then_some(bars)
}

// the config cava reads, under the runtime directory only the user reads. Without one, no shared
// directory is trusted, and the synthetic bars draw
fn config_file() -> io::Result<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|directory| !directory.is_empty())
        .ok_or_else(|| io::Error::other("no XDG_RUNTIME_DIR"))?;
    let directory = PathBuf::from(runtime).join("kanade");

    fs::create_dir_all(&directory)?;

    let path = directory.join("cava.conf");

    fs::write(&path, CONFIG)?;

    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_is_one_height_per_bar() {
        assert_eq!(parse("0;12;100;7;\n"), Some([0, 12, 100, 7]));
        assert_eq!(parse("0;12;100;7"), Some([0, 12, 100, 7]));
    }

    #[test]
    fn a_height_over_the_peak_is_cut() {
        assert_eq!(parse("0;0;0;250;"), Some([0, 0, 0, PEAK]));
    }

    #[test]
    fn a_frame_of_the_wrong_length_or_not_numbers_is_skipped() {
        assert_eq!(parse("1;2;3;"), None);
        assert_eq!(parse("1;2;3;4;5;"), None);
        assert_eq!(parse("1;2;x;4;"), None);
        assert_eq!(parse("1;2;-3;4;"), None);
        assert_eq!(parse(""), None);
    }

    // the config's bars, and the heights it caps at, are what the parser reads
    #[test]
    fn the_config_matches_what_is_parsed() {
        assert!(CONFIG.contains(&format!("bars = {BARS}\n")));
        assert!(CONFIG.contains(&format!("ascii_max_range = {PEAK}\n")));
    }
}
