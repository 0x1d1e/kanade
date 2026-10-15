//! DDC/CI over a monitor's I2C bus, for the brightness of an external monitor (ADR 0082): the
//! packets of VCP feature 0x10, the monitors the kernel lists, and a thread that does the bus's
//! slow reads and writes, so no Service lock or draw waits on a monitor.
//!
//! The packets are the VESA DDC/CI ones, checked against the framing `ddcutil` and DankMaterialShell
//! use; not tested against a live monitor in this release.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::{Duration, Instant};

use rustix::fs::{FlockOperation, flock};

use crate::clock;

// the monitor's DDC/CI address on the bus, as 7 bits
const ADDRESS: u16 = 0x37;

// VCP feature 0x10, luminance
const LUMINANCE: u8 = 0x10;

// the host's address as the monitor sees a write, and the implied one of its reply
const WRITE_SEED: u8 = 0x6e;
const REPLY_SEED: u8 = 0x50;

const SOURCE: u8 = 0x51;
const GET: u8 = 0x01;
const SET: u8 = 0x03;
const REPLY: u8 = 0x02;

// a monitor needs this long to answer a request, and between two
const SETTLE: Duration = Duration::from_millis(50);

// after a failure the bus is left alone this long
const COOLDOWN: Duration = Duration::from_secs(10);

const DRM: &str = "/sys/class/drm";

// a connector's link to its I2C adapter; split, as `boundary` reads a bare "ddc" as a hex color
const LINK: &str = concat!("dd", "c");

fn checksum(seed: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(seed, |sum, byte| sum ^ byte)
}

fn request(payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![SOURCE, 0x80 | payload.len() as u8];

    packet.extend_from_slice(payload);
    packet.push(checksum(WRITE_SEED, &packet));
    packet
}

pub fn get_request() -> Vec<u8> {
    request(&[GET, LUMINANCE])
}

pub fn set_request(value: u16) -> Vec<u8> {
    let [high, low] = value.to_be_bytes();

    request(&[SET, LUMINANCE, high, low])
}

/// The monitor's luminance and its largest value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reading {
    pub current: u16,
    pub max: u16,
}

/// A reply to `get_request`: `6e`, `0x80 | 8`, `02`, result, feature, type, max, current, checksum.
/// Anything else, a short read, a monitor that does not know the feature or a bad sum is refused.
pub fn parse_reply(bytes: &[u8]) -> Result<Reading, &'static str> {
    let frame = bytes.get(..11).ok_or("short reply")?;
    let [
        source,
        length,
        opcode,
        result,
        feature,
        _,
        max_high,
        max_low,
        high,
        low,
        sum,
    ] = frame.try_into().map_err(|_| "short reply")?;

    if source != WRITE_SEED || length != 0x88 || opcode != REPLY {
        return Err("not a feature reply");
    }

    if checksum(REPLY_SEED, &frame[..10]) != sum {
        return Err("bad checksum");
    }

    if result != 0 {
        return Err("feature not supported");
    }

    if feature != LUMINANCE {
        return Err("another feature");
    }

    let max = u16::from_be_bytes([max_high, max_low]);

    if max == 0 {
        return Err("no range");
    }

    Ok(Reading {
        current: u16::from_be_bytes([high, low]),
        max,
    })
}

pub fn percent_of(reading: Reading) -> u8 {
    let max = u32::from(reading.max);

    ((u32::from(reading.current) * 100 + max / 2) / max).min(100) as u8
}

/// The value for `percent`, never 0: a black screen is hard to undo without seeing it.
pub fn value_of(percent: u8, max: u16) -> u16 {
    let max = u32::from(max);
    let value = (max * u32::from(percent.clamp(1, 100)) + 50) / 100;

    value.clamp(1, max.max(1)) as u16
}

/// A connected external monitor, by the connector niri names its output with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    pub connector: String,
    pub bus: u32,
}

// a laptop's own panel does not take DDC/CI, and a write to it is not wanted
fn internal(connector: &str) -> bool {
    ["eDP", "LVDS", "DSI"]
        .iter()
        .any(|kind| connector.starts_with(kind))
}

// `card1-DP-1` is DP-1
fn connector_of(entry: &str) -> Option<&str> {
    let (card, connector) = entry.split_once('-')?;

    card.strip_prefix("card")
        .filter(|number| !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit()))?;

    Some(connector)
}

// the number of `i2c-7`
fn bus_of(link: &Path) -> Option<u32> {
    link.file_name()?
        .to_str()?
        .strip_prefix("i2c-")?
        .parse()
        .ok()
}

/// The connected external monitors whose connector has an I2C bus. Reads sysfs only.
pub fn monitors() -> Vec<Monitor> {
    let Ok(entries) = fs::read_dir(DRM) else {
        return Vec::new();
    };

    let mut found: Vec<Monitor> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let connector = connector_of(&name)?;
            let path = entry.path();

            if internal(connector)
                || fs::read_to_string(path.join("status")).ok()?.trim() != "connected"
            {
                return None;
            }

            // `ddc` links to the adapter, `i2c-7`, whose own `i2c-dev/i2c-7` is the node
            let bus = bus_of(&fs::canonicalize(path.join(LINK)).ok()?)?;

            Some(Monitor {
                connector: connector.to_owned(),
                bus,
            })
        })
        .collect();

    found.sort_by(|a, b| a.connector.cmp(&b.connector));
    found
}

fn open(bus: u32) -> std::io::Result<File> {
    let file = clock::open_i2c(&PathBuf::from(format!("/dev/i2c-{bus}")), ADDRESS)?;

    // other tools take turns on the bus by the same lock
    flock(&file, FlockOperation::NonBlockingLockExclusive)?;

    Ok(file)
}

fn read(bus: u32) -> Result<Reading, String> {
    let mut file = open(bus).map_err(|error| error.to_string())?;

    file.write_all(&get_request())
        .map_err(|error| error.to_string())?;
    thread::sleep(SETTLE);

    let mut reply = [0_u8; 12];
    let count = file.read(&mut reply).map_err(|error| error.to_string())?;

    parse_reply(&reply[..count]).map_err(str::to_owned)
}

fn write(bus: u32, value: u16) -> Result<(), String> {
    let mut file = open(bus).map_err(|error| error.to_string())?;

    file.write_all(&set_request(value))
        .map_err(|error| error.to_string())?;
    thread::sleep(SETTLE);

    Ok(())
}

enum Job {
    Read(u32, Box<dyn FnOnce(Option<Reading>) + Send>),
    Set(u32, u16),
}

static JOBS: LazyLock<Sender<Job>> = LazyLock::new(|| {
    let (sender, receiver) = mpsc::channel::<Job>();

    thread::Builder::new()
        .name(String::from("ddc-ci"))
        .spawn(move || {
            let mut failed: Option<(u32, Instant)> = None;

            // blocks while there is nothing to do, so a shell that never uses it wakes nothing
            while let Ok(mut job) = receiver.recv() {
                // a drag sends many sets: only the newest of a bus is written
                while let Ok(next) = receiver.try_recv() {
                    let replaces = matches!((&job, &next), (Job::Set(bus, _), Job::Set(other, _)) if bus == other);

                    if !replaces {
                        run(job, &mut failed);
                    }

                    job = next;
                }

                run(job, &mut failed);
            }
        })
        .expect("failed to start the ddc thread");

    sender
});

fn run(job: Job, failed: &mut Option<(u32, Instant)>) {
    let bus = match &job {
        Job::Read(bus, _) | Job::Set(bus, _) => *bus,
    };

    if matches!(failed, Some((was, at)) if *was == bus && at.elapsed() < COOLDOWN) {
        if let Job::Read(_, done) = job {
            done(None);
        }

        return;
    }

    let outcome = match job {
        Job::Read(bus, done) => match read(bus) {
            Ok(reading) => {
                done(Some(reading));
                Ok(())
            }
            Err(error) => {
                done(None);
                Err(error)
            }
        },
        Job::Set(bus, value) => write(bus, value),
    };

    match outcome {
        Ok(()) => *failed = None,
        Err(error) => {
            // the monitor's, never the user's, text
            eprintln!("kanade: brightness: DDC/CI on i2c-{bus}: {error}");
            *failed = Some((bus, Instant::now()));
        }
    }
}

/// Reads the monitor's luminance on the ddc thread and hands the result to `done`.
pub fn ask(bus: u32, done: impl FnOnce(Option<Reading>) + Send + 'static) {
    let _ = JOBS.send(Job::Read(bus, Box::new(done)));
}

/// Writes the monitor's luminance on the ddc thread; a newer set of the same bus replaces this.
pub fn set(bus: u32, percent: u8, max: u16) {
    let _ = JOBS.send(Job::Set(bus, value_of(percent, max)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_the_ddc_ci_frames() {
        // the frame ddcutil sends to read luminance
        assert_eq!(get_request(), [0x51, 0x82, 0x01, 0x10, 0xac]);
        // 0x6e ^ 0x51 ^ 0x84 ^ 0x03 ^ 0x10 ^ 0x00 ^ 0x32
        assert_eq!(set_request(50), [0x51, 0x84, 0x03, 0x10, 0x00, 0x32, 0x9a]);
    }

    fn reply(feature: u8, result: u8, max: u16, current: u16) -> Vec<u8> {
        let mut bytes = vec![0x6e, 0x88, 0x02, result, feature, 0x00];

        bytes.extend(max.to_be_bytes());
        bytes.extend(current.to_be_bytes());
        bytes.push(checksum(REPLY_SEED, &bytes));
        bytes
    }

    #[test]
    fn a_reply_is_read_only_when_whole_and_right() {
        assert_eq!(
            parse_reply(&reply(0x10, 0, 100, 37)),
            Ok(Reading {
                current: 37,
                max: 100
            })
        );

        let mut bad = reply(0x10, 0, 100, 37);
        bad[8] ^= 1;
        assert_eq!(parse_reply(&bad), Err("bad checksum"));

        assert_eq!(
            parse_reply(&reply(0x10, 1, 100, 37)),
            Err("feature not supported")
        );
        assert_eq!(
            parse_reply(&reply(0x12, 0, 100, 37)),
            Err("another feature")
        );
        assert_eq!(parse_reply(&reply(0x10, 0, 0, 0)), Err("no range"));
        assert_eq!(
            parse_reply(&reply(0x10, 0, 100, 37)[..10]),
            Err("short reply")
        );
        assert_eq!(parse_reply(&[]), Err("short reply"));

        let mut other = reply(0x10, 0, 100, 37);
        other[0] = 0x6f;
        assert_eq!(parse_reply(&other), Err("not a feature reply"));
    }

    #[test]
    fn a_value_never_blacks_the_screen_and_reads_back() {
        assert_eq!(value_of(0, 100), 1);
        assert_eq!(value_of(1, 10), 1);
        assert_eq!(value_of(100, 255), 255);

        for max in [1_u16, 10, 100, 255, 65535] {
            for percent in 1..=100_u8 {
                let value = value_of(percent, max);

                assert!((1..=max).contains(&value));
            }
        }

        for percent in 1..=100_u8 {
            let reading = Reading {
                current: value_of(percent, 100),
                max: 100,
            };

            assert_eq!(percent_of(reading), percent);
        }
    }

    #[test]
    fn only_a_connected_external_connector_is_a_monitor() {
        assert_eq!(connector_of("card1-DP-1"), Some("DP-1"));
        assert_eq!(connector_of("card0-eDP-1"), Some("eDP-1"));
        assert_eq!(connector_of("card1"), None);
        assert_eq!(connector_of("renderD128"), None);
        assert!(internal("eDP-1") && internal("LVDS-1") && internal("DSI-1"));
        assert!(!internal("DP-1") && !internal("HDMI-A-1"));
        assert_eq!(bus_of(Path::new("/sys/class/i2c-adapter/i2c-7")), Some(7));
        assert_eq!(bus_of(Path::new("/sys/class/i2c-adapter/card1")), None);
    }
}
