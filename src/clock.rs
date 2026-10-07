//! The local time Rest shows (#89). A window that draws it subscribes to `WallClock` and arms one
//! timer for the next minute; when it fires, only the windows that drew the time draw again, and
//! they arm it for the minute after. A window that stops drawing it stops arming it, so with no
//! island at Rest nothing wakes.
//!
//! std has no local time, so this asks libc, which std links
//! anyway: `localtime_r` for the time zone and its daylight saving, and a `CLOCK_REALTIME` timerfd
//! for the minute. A monotonic deadline would fall behind across a suspend or a clock step; the
//! timerfd fires at the wall minute after a resume, and a step cancels it, which redraws at once.
//!
//! This is Kanade's platform boundary: the only `unsafe` and the only hand-kept libc ABI. It also
//! holds `interrupt`, which std lacks, for a child that must end cleanly.

use std::ffi::{c_char, c_int, c_long};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::process::Child;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use amane::Service;

use crate::supervise;

// the hand-kept libc ABI below holds only here; another target must check it before building
#[cfg(not(all(
    target_os = "linux",
    any(
        target_arch = "x86_64",
        target_arch = "aarch64",
        target_arch = "riscv64"
    )
)))]
compile_error!(
    "src/clock.rs declares libc's Linux ABI by hand for x86_64, aarch64 and riscv64 only"
);

// <time.h> and <sys/timerfd.h>, asm-generic values, as on x86_64, aarch64 and riscv64
const CLOCK_REALTIME: c_int = 0;
const TFD_CLOEXEC: c_int = 0o2_000_000;
const TFD_TIMER_ABSTIME: c_int = 1;
const TFD_TIMER_CANCEL_ON_SET: c_int = 2;
const ECANCELED: i32 = 125;

// <signal.h>
pub const SIGINT: c_int = 2;

#[repr(C)]
struct Tm {
    tm_sec: c_int,
    tm_min: c_int,
    tm_hour: c_int,
    tm_mday: c_int,
    tm_mon: c_int,
    tm_year: c_int,
    tm_wday: c_int,
    tm_yday: c_int,
    tm_isdst: c_int,
    tm_gmtoff: c_long,
    tm_zone: *const c_char,
}

#[repr(C)]
struct Timespec {
    tv_sec: c_long,
    tv_nsec: c_long,
}

#[repr(C)]
struct Itimerspec {
    it_interval: Timespec,
    it_value: Timespec,
}

unsafe extern "C" {
    fn tzset();
    fn localtime_r(time: *const c_long, tm: *mut Tm) -> *mut Tm;
    fn timerfd_create(clock: c_int, flags: c_int) -> c_int;
    fn timerfd_settime(
        fd: c_int,
        flags: c_int,
        new: *const Itimerspec,
        old: *mut Itimerspec,
    ) -> c_int;
    fn kill(pid: c_int, signal: c_int) -> c_int;
}

// the timer the thread waits on, made before any window draws
static TIMER: OnceLock<OwnedFd> = OnceLock::new();

// the minute the timer is armed for, so a window drawing every frame of a morph arms it once
static ARMED: Mutex<Option<c_long>> = Mutex::new(None);

// how the time reads, `clock` in the config
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Hours {
    // 14:05
    #[default]
    TwentyFour,

    // 2:05 PM
    Twelve,
}

/*
 * written when the minute turns, so only the windows that drew the time draw again; it holds
 * nothing, since the time follows from when a window draws
 */
pub struct WallClock;

impl Service for WallClock {
    fn new() -> Self {
        WallClock
    }

    fn listen() {}
}

// called before the windows draw; without a timer the time shows but never turns
pub fn spawn() {
    // SAFETY: plain call, the fd it returns is owned below
    let fd = unsafe { timerfd_create(CLOCK_REALTIME, TFD_CLOEXEC) };

    if fd < 0 {
        unturned(&format!("no clock timer ({})", io::Error::last_os_error()));
        return;
    }

    // SAFETY: a fresh fd nothing else owns
    let timer = TIMER.get_or_init(|| unsafe { OwnedFd::from_raw_fd(fd) });

    match timer.try_clone() {
        Ok(timer) => {
            let mut timer = File::from(timer);
            supervise::spawn("clock", move || follow(&mut timer));
        }
        Err(error) => unturned(&format!("no clock timer ({error})")),
    }
}

// the time shows but never turns again, which stderr and `kanade status` say
fn unturned(problem: &str) {
    let why = format!("{problem}, the time will not turn");

    eprintln!("kanade: {why}");
    supervise::stopped("clock", why);
}

// asleep until the armed minute, or until the clock is set
fn follow(timer: &mut File) {
    let mut expirations = [0; 8];

    supervise::run("clock", || {
        loop {
            match timer.read_exact(&mut expirations) {
                Ok(()) => {}
                Err(error) if error.raw_os_error() == Some(ECANCELED) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    unturned(&format!("lost the clock timer ({error})"));
                    return;
                }
            }

            *ARMED.lock().unwrap_or_else(PoisonError::into_inner) = None;
            drop(WallClock::write());
        }
    });
}

// the local time now, asking for a redraw when the minute turns
pub fn now(hours: Hours) -> String {
    drop(WallClock::read());

    let seconds = seconds();

    let mut armed = ARMED.lock().unwrap_or_else(PoisonError::into_inner);

    // a new minute reads the time zone again, so a change to it shows by the next one
    if armed.is_none_or(|minute| minute <= seconds) {
        // SAFETY: plain call
        unsafe { tzset() };
    }

    let Some(local) = local(seconds) else {
        return String::new();
    };

    let next = seconds - c_long::from(local.tm_sec) + 60;

    if *armed != Some(next)
        && let Some(timer) = TIMER.get()
        && arm(timer.as_raw_fd(), next).is_ok()
    {
        *armed = Some(next);
    }

    read(local.tm_hour, local.tm_min, hours)
}

// the local date and time now, like "2026-10-07 15-36-38", for a file name
pub fn stamp() -> String {
    let seconds = seconds();

    // SAFETY: plain call
    unsafe { tzset() };

    match local(seconds) {
        Some(local) => format!(
            "{:04}-{:02}-{:02} {:02}-{:02}-{:02}",
            local.tm_year + 1900,
            local.tm_mon + 1,
            local.tm_mday,
            local.tm_hour,
            local.tm_min,
            local.tm_sec
        ),
        None => seconds.to_string(),
    }
}

/*
 * sends `child` SIGINT, as Ctrl+C would, so it can finish what it writes; std only kills. A
 * reaped child's pid may be another process's by now, so the caller makes sure it is not reaped
 */
pub fn interrupt(child: &Child) -> io::Result<()> {
    let pid = c_int::try_from(child.id()).map_err(io::Error::other)?;

    // SAFETY: plain call
    match unsafe { kill(pid, SIGINT) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

fn seconds() -> c_long {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());

    c_long::try_from(seconds).unwrap_or(c_long::MAX)
}

fn local(seconds: c_long) -> Option<Tm> {
    let mut tm = Tm {
        tm_sec: 0,
        tm_min: 0,
        tm_hour: 0,
        tm_mday: 0,
        tm_mon: 0,
        tm_year: 0,
        tm_wday: 0,
        tm_yday: 0,
        tm_isdst: 0,
        tm_gmtoff: 0,
        tm_zone: std::ptr::null(),
    };

    // SAFETY: both pointers are to live locals; tm_zone is never read
    let done = unsafe { localtime_r(&raw const seconds, &raw mut tm) };

    (!done.is_null()).then_some(tm)
}

// once, at `at` seconds since the epoch; cancelled if the clock is set
fn arm(fd: RawFd, at: c_long) -> io::Result<()> {
    let when = Itimerspec {
        it_interval: Timespec {
            tv_sec: 0,
            tv_nsec: 0,
        },
        it_value: Timespec {
            tv_sec: at,
            tv_nsec: 0,
        },
    };

    // SAFETY: `when` is live, and no old value is asked for
    let set = unsafe {
        timerfd_settime(
            fd,
            TFD_TIMER_ABSTIME | TFD_TIMER_CANCEL_ON_SET,
            &raw const when,
            std::ptr::null_mut(),
        )
    };

    if set < 0 {
        return Err(io::Error::last_os_error());
    }

    Ok(())
}

fn read(hour: c_int, minute: c_int, hours: Hours) -> String {
    match hours {
        Hours::TwentyFour => format!("{hour:02}:{minute:02}"),
        Hours::Twelve => {
            let half = if hour < 12 { "AM" } else { "PM" };
            let hour = (hour + 11) % 12 + 1;

            format!("{hour}:{minute:02} {half}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_interrupted_child_ends_by_sigint() {
        use std::os::unix::process::ExitStatusExt;

        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep runs");

        interrupt(&child).expect("signalled");

        assert_eq!(child.wait().expect("reaped").signal(), Some(SIGINT));
    }

    #[test]
    fn twenty_four_hours_pad_both() {
        assert_eq!(read(0, 0, Hours::TwentyFour), "00:00");
        assert_eq!(read(9, 5, Hours::TwentyFour), "09:05");
        assert_eq!(read(23, 59, Hours::TwentyFour), "23:59");
    }

    #[test]
    fn twelve_hours_go_from_twelve_to_eleven() {
        assert_eq!(read(0, 0, Hours::Twelve), "12:00 AM");
        assert_eq!(read(9, 5, Hours::Twelve), "9:05 AM");
        assert_eq!(read(12, 30, Hours::Twelve), "12:30 PM");
        assert_eq!(read(23, 59, Hours::Twelve), "11:59 PM");
    }

    // the zone is the process's, so this checks only what holds in every zone
    #[test]
    fn local_time_is_a_whole_minute_offset_from_utc() {
        let seconds = 1_700_000_000;
        let tm = local(seconds).expect("libc reads local time");

        assert_eq!(c_long::from(tm.tm_sec), seconds % 60);
        assert_eq!(tm.tm_gmtoff % 60, 0);
        assert!((0..24).contains(&tm.tm_hour));
    }

    #[test]
    fn a_stamp_is_a_date_and_a_time() {
        let stamp = stamp();
        let shape: String = stamp
            .chars()
            .map(|char| if char.is_ascii_digit() { '0' } else { char })
            .collect();

        assert_eq!(shape, "0000-00-00 00-00-00", "{stamp}");
    }

    #[test]
    fn an_armed_timer_fires_at_its_minute() {
        // SAFETY: plain call, owned below
        let fd = unsafe { timerfd_create(CLOCK_REALTIME, TFD_CLOEXEC) };
        assert!(fd >= 0);
        // SAFETY: a fresh fd nothing else owns
        let mut timer = File::from(unsafe { OwnedFd::from_raw_fd(fd) });

        // a moment already past fires at once
        let past = c_long::try_from(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
        .unwrap()
            - 1;

        arm(timer.as_raw_fd(), past).unwrap();

        let mut expirations = [0; 8];
        timer.read_exact(&mut expirations).unwrap();
        assert_eq!(u64::from_ne_bytes(expirations), 1);
    }
}
