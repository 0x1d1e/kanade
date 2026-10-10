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
//! holds `interrupt`, which std lacks, for a child that must end cleanly, the one place a UTC
//! moment becomes local time, which the calendar's events need too, `authenticate`, Linux-PAM's
//! password check, which the lock screen runs itself so it can drop a result sleep made stale, and
//! the kernel's input and rfkill ABI the OSD reads keys, lock lights and radios through (ADR 0021).

use std::ffi::{CString, c_char, c_int, c_long, c_ulong, c_void};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use kanade_runtime::service::Service;
use zeroize::{Zeroize, Zeroizing};

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
const SIG_DFL: usize = 0;

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
    fn signal(signal: c_int, handler: usize) -> usize;
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;

    // PAM frees a conversation's answers with free(), so they come from the C allocator
    fn calloc(count: usize, size: usize) -> *mut c_void;
    fn strdup(text: *const c_char) -> *mut c_char;
}

// <security/_pam_types.h>, as Linux-PAM has it on every target
const PAM_SUCCESS: c_int = 0;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;

#[repr(C)]
struct PamMessage {
    style: c_int,
    text: *const c_char,
}

#[repr(C)]
struct PamResponse {
    text: *mut c_char,
    code: c_int,
}

type Converse =
    extern "C" fn(c_int, *mut *const PamMessage, *mut *mut PamResponse, *mut c_void) -> c_int;

#[repr(C)]
struct PamConv {
    converse: Converse,
    data: *mut c_void,
}

// libpam, for the lock screen's password check
#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conversation: *const PamConv,
        handle: *mut *mut c_void,
    ) -> c_int;
    fn pam_authenticate(handle: *mut c_void, flags: c_int) -> c_int;

    // refuses an expired or locked account too, which the password alone does not
    fn pam_acct_mgmt(handle: *mut c_void, flags: c_int) -> c_int;
    fn pam_end(handle: *mut c_void, status: c_int) -> c_int;
}

// <linux/input.h> and <linux/input-event-codes.h>, as on every 64-bit target
pub const EV_KEY: u16 = 0x01;
pub const EV_LED: u16 = 0x11;
pub const KEY_VOLUMEDOWN: u16 = 114;
pub const KEY_VOLUMEUP: u16 = 115;
pub const KEY_BRIGHTNESSDOWN: u16 = 224;
pub const KEY_BRIGHTNESSUP: u16 = 225;
pub const LED_NUML: u16 = 0;
pub const LED_CAPSL: u16 = 1;

// EVIOCSMASK, _IOW('E', 0x93, struct input_mask)
const EVIOCSMASK: c_ulong = 0x4010_4593;

// EVIOCSCLOCKID, _IOW('E', 0xa0, int)
const EVIOCSCLOCKID: c_ulong = 0x4004_45a0;

// <time.h>
const CLOCK_MONOTONIC: c_int = 1;

// the mask of event types is the one EV_SYN's slot holds; EV_SYN itself is never masked
const EV_TYPES: u32 = 0;

// a struct input_event: a timeval of two longs, then type, code, value
pub const INPUT_EVENT: usize = 24;

#[repr(C)]
struct InputMask {
    kind: u32,

    // in bytes
    codes_size: u32,
    codes_ptr: u64,
}

/*
 * the layouts here are 64-bit Linux's: a 32-bit target's timeval, and so its input_event, is
 * smaller
 */
#[cfg(not(target_pointer_width = "64"))]
compile_error!("clock.rs keeps the evdev ABI of 64-bit Linux");

const _: () = assert!(size_of::<InputMask>() == 16);

// an event read off an evdev device, what follows its time
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputEvent {
    pub kind: u16,
    pub code: u16,
    pub value: i32,
}

impl InputEvent {
    pub fn of(bytes: &[u8; INPUT_EVENT]) -> InputEvent {
        let [.., k0, k1, c0, c1, v0, v1, v2, v3] = *bytes;

        InputEvent {
            kind: u16::from_ne_bytes([k0, k1]),
            code: u16::from_ne_bytes([c0, c1]),
            value: i32::from_ne_bytes([v0, v1, v2, v3]),
        }
    }
}

/*
 * has the kernel give `device` only these events: of `codes`' types, each only its codes, every
 * other type none. What the device queued before, unmasked, is thrown away unread: the kernel
 * flushes a reader's queue as its events' clock changes, from the realtime one it opens with to the
 * monotonic one
 */
pub fn mask_input(device: &File, codes: &[(u16, &[u16])]) -> io::Result<()> {
    let bitmap = |set: &mut dyn Iterator<Item = u16>| {
        let mut words = Vec::new();

        for code in set {
            let word = usize::from(code / 64);

            if words.len() <= word {
                words.resize(word + 1, 0_u64);
            }

            words[word] |= 1 << (code % 64);
        }

        words
    };

    let set = |kind: u32, words: &[u64]| {
        let mask = InputMask {
            kind,
            codes_size: u32::try_from(words.len() * 8).map_err(io::Error::other)?,
            codes_ptr: words.as_ptr() as u64,
        };

        // SAFETY: `mask` and the words it points at outlive the call, which only reads them
        match unsafe { ioctl(device.as_raw_fd(), EVIOCSMASK, &raw const mask) } {
            0 => Ok(()),
            _ => Err(io::Error::last_os_error()),
        }
    };

    // the codes first: until the types are masked, every other type still comes through
    for &(kind, kind_codes) in codes {
        set(u32::from(kind), &bitmap(&mut kind_codes.iter().copied()))?;
    }

    set(EV_TYPES, &bitmap(&mut codes.iter().map(|&(kind, _)| kind)))?;

    let clock = CLOCK_MONOTONIC;

    // SAFETY: `clock` outlives the call, which only reads it
    match unsafe { ioctl(device.as_raw_fd(), EVIOCSCLOCKID, &raw const clock) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

// <linux/rfkill.h>, a struct rfkill_event as read in its first, fixed size
pub const RFKILL_EVENT: usize = 8;
pub const RFKILL_OP_ADD: u8 = 0;
pub const RFKILL_OP_DEL: u8 = 1;
pub const RFKILL_OP_CHANGE: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RfkillEvent {
    pub index: u32,

    // the radio's type
    pub kind: u8,
    pub op: u8,

    // blocked by software, like airplane mode, or by a hardware switch
    pub soft: bool,
    pub hard: bool,
}

impl RfkillEvent {
    pub fn of(bytes: &[u8; RFKILL_EVENT]) -> RfkillEvent {
        let [i0, i1, i2, i3, kind, op, soft, hard] = *bytes;

        RfkillEvent {
            index: u32::from_ne_bytes([i0, i1, i2, i3]),
            kind,
            op,
            soft: soft != 0,
            hard: hard != 0,
        }
    }
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
    match minute() {
        Some(local) => read(local.tm_hour, local.tm_min, hours),
        None => String::new(),
    }
}

// the local date now, asking for a redraw when the minute turns, so a new day shows
pub fn today() -> NaiveDate {
    minute().and_then(|local| date(&local)).unwrap_or_default()
}

// the local time of `seconds` since the epoch, in the time zone the clock last read
pub fn local_time(seconds: i64) -> Option<NaiveDateTime> {
    let local = local(c_long::try_from(seconds).ok()?)?;
    let time = NaiveTime::from_hms_opt(
        u32::try_from(local.tm_hour).ok()?,
        u32::try_from(local.tm_min).ok()?,
        // a leap second reads as the one before it
        u32::try_from(local.tm_sec.min(59)).ok()?,
    )?;

    Some(date(&local)?.and_time(time))
}

// a time of day as `clock` reads it
pub fn time(at: NaiveTime, hours: Hours) -> String {
    use chrono::Timelike;

    // both under 60 and 24, so they fit
    read(at.hour() as c_int, at.minute() as c_int, hours)
}

// the local time now, the timer armed for the next minute and a redraw asked for then
fn minute() -> Option<Tm> {
    drop(WallClock::read());

    let seconds = seconds();

    let mut armed = ARMED.lock().unwrap_or_else(PoisonError::into_inner);

    // a new minute reads the time zone again, so a change to it shows by the next one
    if armed.is_none_or(|minute| minute <= seconds) {
        // SAFETY: plain call
        unsafe { tzset() };
    }

    let local = local(seconds)?;
    let next = seconds - c_long::from(local.tm_sec) + 60;

    if *armed != Some(next)
        && let Some(timer) = TIMER.get()
        && arm(timer.as_raw_fd(), next).is_ok()
    {
        *armed = Some(next);
    }

    Some(local)
}

fn date(local: &Tm) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        local.tm_year + 1900,
        u32::try_from(local.tm_mon + 1).ok()?,
        u32::try_from(local.tm_mday).ok()?,
    )
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

/*
 * `command` starts with SIGINT at its default, so `interrupt` ends it even when Kanade was started
 * with SIGINT ignored, as a job run in the background of a script is, which children inherit
 */
pub fn default_sigint(command: &mut Command) -> &mut Command {
    // SAFETY: `signal` is async-signal-safe, and the only call between fork and exec
    unsafe {
        command.pre_exec(|| {
            signal(SIGINT, SIG_DFL);
            Ok(())
        })
    }
}

/*
 * whether PAM's `service` accepts `password` for `user`; blocks for as long as PAM takes, seconds
 * for a wrong one. The copy it makes is zeroed before it returns; the caller zeroes its own
 */
pub fn authenticate(service: &str, user: &str, password: &str) -> bool {
    let (Ok(service), Ok(user)) = (CString::new(service), CString::new(user)) else {
        return false;
    };
    // zeroed as it drops, a panic in PAM too
    let password = match CString::new(password) {
        Ok(password) => Zeroizing::new(password),
        Err(error) => {
            error.into_vec().zeroize();
            return false;
        }
    };

    pam(&service, &user, &password)
}

// one PAM transaction answering with `password`, as `authenticate` says
fn pam(service: &CString, user: &CString, password: &CString) -> bool {
    let conversation = PamConv {
        converse,
        data: password.as_ptr().cast_mut().cast(),
    };
    let mut handle = std::ptr::null_mut();

    // SAFETY: every pointer outlives the handle, which `pam_end` lets go of before this returns
    unsafe {
        if pam_start(
            service.as_ptr(),
            user.as_ptr(),
            &raw const conversation,
            &raw mut handle,
        ) != PAM_SUCCESS
        {
            return false;
        }

        let mut status = pam_authenticate(handle, 0);
        if status == PAM_SUCCESS {
            status = pam_acct_mgmt(handle, 0);
        }

        pam_end(handle, status);
        status == PAM_SUCCESS
    }
}

// answers every prompt PAM gives with the password in `data`, and every message with nothing
extern "C" fn converse(
    count: c_int,
    messages: *mut *const PamMessage,
    responses: *mut *mut PamResponse,
    data: *mut c_void,
) -> c_int {
    let Ok(count) = usize::try_from(count) else {
        return PAM_CONV_ERR;
    };
    let password = data.cast::<c_char>().cast_const();

    // SAFETY: Linux-PAM hands `count` messages, an array of pointers, and takes the answers,
    // which it frees
    unsafe {
        let answers = calloc(count, size_of::<PamResponse>()).cast::<PamResponse>();
        if answers.is_null() {
            return PAM_CONV_ERR;
        }

        for index in 0..count {
            let style = (**messages.add(index)).style;

            if style == PAM_PROMPT_ECHO_OFF || style == PAM_PROMPT_ECHO_ON {
                (*answers.add(index)).text = strdup(password);
            }
        }

        *responses = answers;
    }

    PAM_SUCCESS
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

    // a Caps Lock press as a 64-bit kernel writes it: a timeval of two longs, type, code, value
    #[test]
    fn an_input_event_reads_past_its_time() {
        let mut bytes = [0xaa; INPUT_EVENT];
        bytes[16..18].copy_from_slice(&EV_KEY.to_ne_bytes());
        bytes[18..20].copy_from_slice(&58_u16.to_ne_bytes());
        bytes[20..].copy_from_slice(&1_i32.to_ne_bytes());

        assert_eq!(
            InputEvent::of(&bytes),
            InputEvent {
                kind: EV_KEY,
                code: 58,
                value: 1
            }
        );
    }

    // radio 3, Bluetooth, soft-blocked
    #[test]
    fn an_rfkill_event_reads_as_the_kernel_writes_it() {
        let mut bytes = [0; RFKILL_EVENT];
        bytes[..4].copy_from_slice(&3_u32.to_ne_bytes());
        bytes[4..].copy_from_slice(&[2, RFKILL_OP_CHANGE, 1, 0]);

        assert_eq!(
            RfkillEvent::of(&bytes),
            RfkillEvent {
                index: 3,
                kind: 2,
                op: RFKILL_OP_CHANGE,
                soft: true,
                hard: false
            }
        );
    }

    #[test]
    fn an_interrupted_child_ends_by_sigint() {
        use std::os::unix::process::ExitStatusExt;

        let mut child = default_sigint(Command::new("sleep").arg("30"))
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

    // the zone is the process's, so this checks the date and time against libc's own fields
    #[test]
    fn a_moment_reads_as_its_local_date_and_time() {
        use chrono::Timelike;

        let seconds = 1_700_000_000;
        let tm = local(seconds).expect("libc reads local time");
        let at = local_time(seconds).expect("a local time");

        assert_eq!(Some(at.date()), date(&tm));
        assert_eq!(
            (at.hour(), at.minute(), at.second()),
            (tm.tm_hour as u32, tm.tm_min as u32, 20)
        );
        assert_eq!(
            time(NaiveTime::from_hms_opt(14, 5, 0).unwrap(), Hours::Twelve),
            "2:05 PM"
        );
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
