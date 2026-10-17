//! Finding out which encoder works (ADR 0116, Update): the render nodes under `/sys/class/drm`, and
//! a real encode of one frame by `ffmpeg` (for `wf-recorder`) or `gst-launch-1.0` (for a window),
//! killed when it takes too long. `encode::choose` decides from what these find. That an encoder
//! works is kept for the life of the process, so a recording starts without waiting for the probe
//! again; that one failed, for a minute; that it overran or could not be tried (no `ffmpeg`), not at
//! all. `kanade doctor` always asks again.

use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;

use super::encode::{self, Choice, Download, Encoder, Encoding, Kind, Node, Probe, Probed, Vendor};
use super::plan::{ENCODER, JOINER};
use crate::config::{Preference, Recording};
use crate::sources::wake::{self, Failed};

// how long one trial may take: a GPU that sleeps wakes in about a second
const TRIAL: Duration = Duration::from_millis(1500);

// where the kernel lists the GPUs, and the nodes themselves
const SYS: &str = "/sys/class/drm";
const DEV: &str = "/dev/dri";

// the most the software encoder is given, so a recording leaves the machine usable
const MOST_THREADS: usize = 4;

// how long a failed trial is believed
const FAILURE_KEPT: Duration = Duration::from_secs(60);

// every trial answered so far, by the program and arguments it was, and when
static TRIED: Mutex<Vec<(String, Probed, Instant)>> = Mutex::new(Vec::new());

// the render nodes of the machine, `renderD128` first
pub fn nodes() -> Vec<Node> {
    nodes_in(Path::new(SYS), Path::new(DEV))
}

fn nodes_in(sys: &Path, dev: &Path) -> Vec<Node> {
    let mut found: Vec<Node> = fs::read_dir(sys)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let number = name.strip_prefix("renderD")?;

            if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }

            let device = entry.path().join("device");
            let vendor = fs::read_to_string(device.join("vendor")).unwrap_or_default();
            let driver = fs::read_link(device.join("driver"))
                .ok()
                .and_then(|link| {
                    link.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                })
                .unwrap_or_default();
            let primary =
                fs::read_to_string(device.join("boot_vga")).is_ok_and(|text| text.trim() == "1");

            Some(Node {
                path: dev.join(&name).to_string_lossy().into_owned(),
                vendor: Vendor::read(&vendor),
                driver,
                primary,
            })
        })
        .collect();

    found.sort_by(|a, b| (a.path.len(), &a.path).cmp(&(b.path.len(), &b.path)));
    found
}

// how many threads the software encoder may use: half the cores, 2 to `MOST_THREADS`
pub fn threads() -> usize {
    thread::available_parallelism()
        .map_or(2, |cores| cores.get() / 2)
        .clamp(2, MOST_THREADS)
}

// the probes that run the programs, asking again whatever is kept if `fresh`
struct Real {
    fresh: bool,
}

impl Probe for Real {
    fn vaapi(&mut self, node: &Node, first: bool) -> Probed {
        let encoding = trial(Kind::Vaapi, Some(node), first);

        ffmpeg(&encoding, self.fresh)
    }

    fn nvenc(&mut self) -> Probed {
        ffmpeg(&trial(Kind::Nvenc, None, true), self.fresh)
    }

    fn software(&mut self) -> Probed {
        ffmpeg(&trial(Kind::Software, None, true), self.fresh)
    }
}

// an encoding to try, at the default quality: a trial is about whether it runs
fn trial(kind: Kind, node: Option<&Node>, first: bool) -> Encoding {
    Encoding {
        encoder: Encoder {
            kind,
            node: node.map(|node| node.path.clone()),
            first,
        },
        quality: Default::default(),
        threads: threads(),
        download: Download::Gl,
    }
}

/*
 * the answer to a program run with `args`, from `run` when it was not asked before. That it works
 * is kept for good; that it failed for `FAILURE_KEPT`, since a driver gets installed and a GPU
 * wakes; that it overran or could not be run not at all. `fresh` asks again whatever is kept (and
 * keeps the new answer), which `kanade doctor` does so that it never reports a stale one
 */
fn tried(
    program: &str,
    args: &[String],
    fresh: bool,
    run: impl FnOnce() -> (Probed, bool),
) -> Probed {
    let key = format!("{program} {}", args.join(" "));
    let now = Instant::now();

    if !fresh {
        let known = TRIED
            .locked()
            .iter()
            .find(|(asked, _, _)| *asked == key)
            .filter(|(_, found, at)| !expired(found, *at, now))
            .map(|(_, found, _)| found.clone());

        if let Some(found) = known {
            return found;
        }
    }

    let (found, keep) = run();
    let mut tried = TRIED.locked();

    tried.retain(|(asked, found, at)| *asked != key && !expired(found, *at, now));

    if keep {
        tried.push((key, found.clone(), now));
    }

    found
}

// whether a kept answer is too old to be trusted
fn expired(found: &Probed, at: Instant, now: Instant) -> bool {
    matches!(found, Probed::Fails(_)) && now.duration_since(at) > FAILURE_KEPT
}

// whether `ffmpeg` encodes a frame the way `wf-recorder` is told to
fn ffmpeg(encoding: &Encoding, fresh: bool) -> Probed {
    let arguments = encoding.trial();

    tried(JOINER, &arguments, fresh, || {
        run(JOINER, &arguments, encoding.encoder.kind.codec())
    })
}

// whether GStreamer encodes a frame the way a window's pipeline does
fn gstreamer(encoding: &Encoding, fresh: bool) -> Probed {
    let arguments = encoding.gst_trial();

    tried(ENCODER, &arguments, fresh, || {
        run(ENCODER, &arguments, &encoding.encoder.element())
    })
}

// the answer of one trial, and whether it is worth keeping: not when nothing was decided
fn run(program: &str, arguments: &[String], what: &str) -> (Probed, bool) {
    if !wake::found(program) {
        return (
            Probed::Unknown(format!("{program} is not installed")),
            false,
        );
    }

    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();

    match wake::act_within(program, &arguments, TRIAL) {
        Ok(()) => (Probed::Works, true),
        Err(Failed::Overran(limit)) => (
            Probed::Fails(format!(
                "{what} did not finish in {}",
                format_timeout(limit)
            )),
            false,
        ),
        Err(Failed::Said(said)) => (Probed::Fails(brief(&said, program, what)), true),
    }
}

fn format_timeout(limit: Duration) -> String {
    let secs = limit.as_secs_f32();
    if secs.fract() == 0.0 {
        format!("{secs:.0} s")
    } else {
        format!("{secs:.1} s")
    }
}

/*
 * what a failed trial said, in a line: the last line of its stderr, or that the program has no such
 * encoder, which both `ffmpeg` and `gst-launch-1.0` say in their own words
 */
fn brief(said: &str, program: &str, what: &str) -> String {
    if said.contains("Unknown encoder") || said.contains("no element") {
        return format!("{program} has no {what}");
    }

    let said = said.rsplit_once("): ").map_or(said, |(_, rest)| rest);
    let line = said
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it failed");

    line.chars().take(140).collect()
}

/*
 * the encoder to record with under `recording`, and why. `software` is the fallback of a recording
 * whose chosen encoder failed to start: software alone is tried
 */
pub fn choice(recording: &Recording, software: bool, fresh: bool) -> Choice {
    let preference = match software {
        true => Preference::Software,
        false => recording.encoder,
    };

    encode::choose(
        preference,
        recording.device.as_deref(),
        &nodes(),
        &mut Real { fresh },
    )
}

// the encoding of `recording`, or why nothing encodes
pub fn encoding(recording: &Recording, software: bool) -> Result<Encoding, String> {
    let choice = choice(recording, software, false);

    match choice.encoder {
        Some(encoder) => Ok(Encoding {
            encoder,
            quality: recording.quality,
            threads: threads(),
            download: Download::Gl,
        }),
        None => Err(format!(
            "no encoder works ({}); `kanade doctor` says what is missing",
            choice.notes.join("; ")
        )),
    }
}

/*
 * the encoding for a window, which GStreamer runs: `encoding` if its pipeline encodes a frame, else
 * software's, else why neither does. The notes say what gave way. `no_va` when the VA-API download
 * already failed on a live cast, which no trial with a test frame can see: only EGL is tried for
 * `encoding` then. Software's trial is free to use VA-API again, as the encoder may have failed
 * for its own reasons
 */
pub fn window(
    encoding: Encoding,
    recording: &Recording,
    no_va: bool,
) -> Result<(Encoding, Option<String>), String> {
    let nodes = nodes();
    let device = recording.device.as_deref();
    let failed = match downloading(&encoding, &nodes, device, false, no_va) {
        Ok(found) => return Ok((found, None)),
        Err(why) => why,
    };

    if encoding.encoder.kind == Kind::Software {
        return Err(format!("{ENCODER} cannot encode: {failed}"));
    }

    let software = Encoding {
        encoder: Encoder::software(),
        quality: recording.quality,
        threads: threads(),
        download: Download::Gl,
    };

    match downloading(&software, &nodes, device, false, false) {
        Ok(found) => Ok((
            found,
            Some(format!(
                "{} cannot encode a window ({failed})",
                encoding.encoder
            )),
        )),
        Err(also) => Err(format!(
            "{ENCODER} cannot encode: {failed}; software too: {also}"
        )),
    }
}

/*
 * `encoding` with the way to get a window's frames to system memory that works, found by a trial of
 * the whole chain: NVIDIA's encoder and x264 try the VA-API post-processor of each node VA-API may
 * use, which imports what niri casts (Intel's compressed tiled DMA-BUF among it), then EGL, which
 * does not import all of it but needs no VA-API driver. VA-API's own chain needs none. Err says
 * what each said
 */
fn downloading(
    encoding: &Encoding,
    nodes: &[Node],
    device: Option<&str>,
    fresh: bool,
    no_va: bool,
) -> Result<Encoding, String> {
    let works = |tried: &Encoding| match gstreamer(tried, fresh) {
        Probed::Works | Probed::Unknown(_) => Ok(()),
        Probed::Fails(why) => Err(why),
    };

    if encoding.encoder.kind == Kind::Vaapi {
        return works(encoding).map(|()| encoding.clone());
    }

    let mut said = Vec::new();

    for download in encode::downloads(nodes, device, no_va) {
        let tried = Encoding {
            download,
            ..encoding.clone()
        };

        match works(&tried) {
            Ok(()) => return Ok(tried),
            Err(why) => said.push(format!("{}: {why}", tried.download)),
        }
    }

    Err(said.join("; "))
}

/*
 * what `kanade doctor` says of recording a window with `encoding`: asked afresh, whether GStreamer
 * encodes a test frame through the chain a window's pipeline has after the cast, and by which
 * download of the cast's frames. That is the chain and not the cast: no frame of a live cast (niri's
 * DMA-BUF) is tried, and a GPU's own tiling can fail where a test frame does not. None when
 * `gst-launch-1.0` is not there to ask, which a start names
 */
pub fn window_line(encoding: &Encoding, device: Option<&str>) -> Option<Result<String, String>> {
    if !wake::found(ENCODER) {
        return None;
    }

    let element = encoding.encoder.element();

    Some(match downloading(encoding, &nodes(), device, true, false) {
        Ok(found) if found.encoder.kind == Kind::Vaapi => Ok(format!(
            "window: {element} encodes a test frame; a live cast is not tried"
        )),
        Ok(found) => Ok(format!(
            "window: {element} encodes a test frame, frames downloaded through {}; a live cast is not tried",
            found.download
        )),
        Err(why) => Err(match encoding.encoder.kind {
            Kind::Software => format!("window: {element} cannot encode ({why})"),
            _ => format!(
                "window: {element} cannot encode ({why}); a window records with software x264 instead"
            ),
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asked(key: &str, fresh: bool, answer: Probed, keep: bool, runs: &mut u32) -> Probed {
        tried("probe-test", &[key.into()], fresh, || {
            *runs += 1;
            (answer, keep)
        })
    }

    // a trial that overran says nothing of the encoder, and a failure is believed for a while only
    #[test]
    fn an_overrun_is_asked_again_and_a_failure_expires() {
        let mut runs = 0;
        let fails = || Probed::Fails(String::from("no"));

        asked("overran", false, fails(), false, &mut runs);
        asked("overran", false, fails(), false, &mut runs);
        assert_eq!(runs, 2);

        asked("works", false, Probed::Works, true, &mut runs);
        asked("works", false, Probed::Works, true, &mut runs);
        assert_eq!(runs, 3);

        // doctor asks again whatever is kept
        asked("works", true, Probed::Works, true, &mut runs);
        assert_eq!(runs, 4);

        let now = Instant::now();

        assert!(!expired(&fails(), now, now));
        assert!(expired(
            &fails(),
            now,
            now + FAILURE_KEPT + Duration::from_secs(1)
        ));
        assert!(!expired(&Probed::Works, now, now + FAILURE_KEPT * 100));
    }

    #[test]
    fn format_timeout_preserves_subsecond_accuracy() {
        assert_eq!(format_timeout(Duration::from_millis(1500)), "1.5 s");
        assert_eq!(format_timeout(Duration::from_secs(2)), "2 s");
    }
}
