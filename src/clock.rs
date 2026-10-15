//! The local time Rest shows. A window that draws it subscribes to `WallClock` and arms one
//! timer for the next minute; when it fires, only the windows that drew the time draw again, and
//! they arm it for the minute after. A window that stops drawing it stops arming it, so with no
//! island at Rest nothing wakes.
//!
//! std has no local time, so this asks libc, which std links
//! anyway: `localtime_r` for the time zone and its daylight saving, and a `CLOCK_REALTIME` timerfd
//! for the minute. A monotonic deadline would fall behind across a suspend or a clock step; the
//! timerfd fires at the wall minute after a resume, and a step cancels it, which redraws at once.
//!
//! This is Kanade's platform boundary: the only `unsafe`, and the only hand-kept ABI, Linux-PAM's
//! and the evdev event's. Everything else asks `rustix` (timerfd, signals, the terminal's mode,
//! ioctl opcodes) or the `libc` crate (what rustix leaves out: local time and signal dispositions; only `tzset` is declared here).
//! It also holds `interrupt`, which std lacks, for a child that must end cleanly, the one place a
//! UTC moment becomes local time, which the calendar's events need too, `authenticate`, Linux-PAM's
//! password check, which the lock screen runs itself so it can drop a result sleep made stale, and
//! the kernel's input and rfkill ABI the OSD reads keys, lock lights and radios through (ADR 0021),
//! and `WallTimer`, a wall-clock moment for a thread to wait for, like a calendar reminder.

use std::ffi::{CString, c_char, c_int, c_void};
use std::fs::File;
use std::io::{self, Read};
use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use kanade_runtime::{MutexExt, service::Service};
use rustix::io::Errno;
use rustix::ioctl::{self, IntegerSetter, Setter, opcode};
use rustix::process::{Pid, Signal, kill_process};
use rustix::time::{
    Itimerspec, TimerfdClockId, TimerfdFlags, TimerfdTimerFlags, Timespec, timerfd_create,
    timerfd_settime,
};
use zeroize::{Zeroize, Zeroizing};

use crate::supervise;

unsafe extern "C" {
    // re-reads the time zone, which the libc crate does not declare
    fn tzset();
}

// USER_HZ, the clock ticks /proc counts a process's start time in
pub const USER_HZ: u64 = 100;

// <security/_pam_types.h>, as Linux-PAM has it on every target
const PAM_SUCCESS: c_int = 0;
const PAM_CONV_ERR: c_int = 19;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;

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

// EVIOCSMASK and EVIOCSCLOCKID, _IOW('E', 0x93, struct input_mask) and _IOW('E', 0xa0, int)
const EVIOCSMASK: ioctl::Opcode = opcode::write::<InputMask>(b'E', 0x93);
const EVIOCSCLOCKID: ioctl::Opcode = opcode::write::<c_int>(b'E', 0xa0);

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

        // SAFETY: EVIOCSMASK takes a `struct input_mask`, and the words it points at outlive the
        // call, which only reads them
        unsafe { ioctl::ioctl(device, Setter::<EVIOCSMASK, InputMask>::new(mask)) }
            .map_err(io::Error::from)
    };

    // the codes first: until the types are masked, every other type still comes through
    for &(kind, kind_codes) in codes {
        set(u32::from(kind), &bitmap(&mut kind_codes.iter().copied()))?;
    }

    set(EV_TYPES, &bitmap(&mut codes.iter().map(|&(kind, _)| kind)))?;

    // SAFETY: EVIOCSCLOCKID takes an `int`
    unsafe {
        ioctl::ioctl(
            device,
            Setter::<EVIOCSCLOCKID, c_int>::new(libc::CLOCK_MONOTONIC),
        )
    }
    .map_err(io::Error::from)
}

// I2C_SLAVE, <linux/i2c-dev.h>: the address the next reads and writes on the bus go to
const I2C_SLAVE: ioctl::Opcode = opcode::none(0x07, 0x03);

/*
 * an i2c-dev bus opened for reads and writes to the device at `address`, for DDC/CI (ADR 0082).
 * The caller names a bus its own discovery found; nothing here probes
 */
pub fn open_i2c(path: &std::path::Path, address: u16) -> io::Result<File> {
    let bus = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)?;

    // SAFETY: I2C_SLAVE takes the address as its integer argument
    unsafe {
        ioctl::ioctl(
            &bus,
            IntegerSetter::<I2C_SLAVE>::new_usize(usize::from(address)),
        )
    }
    .map_err(io::Error::from)?;

    Ok(bus)
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
static ARMED: Mutex<Option<i64>> = Mutex::new(None);

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
    let fd = match wall_timer() {
        Ok(fd) => fd,
        Err(error) => {
            unturned(&format!("no clock timer ({error})"));
            return;
        }
    };

    let timer = TIMER.get_or_init(|| fd);

    match timer.try_clone() {
        Ok(timer) => {
            let mut timer = File::from(timer);
            supervise::spawn("clock", move || follow(&mut timer));
        }
        Err(error) => unturned(&format!("no clock timer ({error})")),
    }
}

fn wall_timer() -> io::Result<OwnedFd> {
    Ok(timerfd_create(
        TimerfdClockId::Realtime,
        TimerfdFlags::CLOEXEC,
    )?)
}

/*
 * a wall-clock moment one thread waits for, like a calendar reminder's (ADR 0043): it fires at the
 * moment set, at once after a resume that passed it, and early when the clock is set, so the
 * waiter works out again what is due. `set` and `wait` may be on different threads
 */
pub struct WallTimer(File);

impl WallTimer {
    pub fn new() -> io::Result<WallTimer> {
        wall_timer().map(|fd| WallTimer(File::from(fd)))
    }

    // fires at `at` seconds since the epoch, at once for one past; replaces the moment set before
    pub fn set(&self, at: i64) -> io::Result<()> {
        // 0 would disarm it, and any moment before now fires at once
        arm(&self.0, at.max(1))
    }

    // asleep until the moment set passes or the clock is set
    pub fn wait(&self) -> io::Result<()> {
        let mut expirations = [0; 8];

        loop {
            match (&self.0).read_exact(&mut expirations) {
                Ok(()) => return Ok(()),
                Err(error) if canceled(&error) => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}

// the timer's wait was cancelled because the clock was set
fn canceled(error: &io::Error) -> bool {
    error.raw_os_error() == Some(Errno::CANCELED.raw_os_error())
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
                Err(error) if canceled(&error) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    unturned(&format!("lost the clock timer ({error})"));
                    return;
                }
            }

            *ARMED.locked() = None;
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

// the seconds since the epoch
pub fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
        })
}

// the local date now, asking for a redraw when the minute turns, so a new day shows
pub fn today() -> NaiveDate {
    minute().and_then(|local| date(&local)).unwrap_or_default()
}

// the local time of `seconds` since the epoch, in the time zone the clock last read
pub fn local_time(seconds: i64) -> Option<NaiveDateTime> {
    let local = local(seconds)?;
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
fn minute() -> Option<libc::tm> {
    drop(WallClock::read());

    let seconds = unix_seconds();

    let mut armed = ARMED.locked();

    // a new minute reads the time zone again, so a change to it shows by the next one
    if armed.is_none_or(|minute| minute <= seconds) {
        // SAFETY: plain call
        unsafe { tzset() };
    }

    let local = local(seconds)?;
    let next = seconds - i64::from(local.tm_sec) + 60;

    if *armed != Some(next)
        && let Some(timer) = TIMER.get()
        && arm(timer, next).is_ok()
    {
        *armed = Some(next);
    }

    Some(local)
}

fn date(local: &libc::tm) -> Option<NaiveDate> {
    NaiveDate::from_ymd_opt(
        local.tm_year + 1900,
        u32::try_from(local.tm_mon + 1).ok()?,
        u32::try_from(local.tm_mday).ok()?,
    )
}

// the local date and time now, like "2026-10-07 15-36-38", for a file name
pub fn stamp() -> String {
    let seconds = unix_seconds();

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
    signal_process(child.id(), Signal::INT)
}

// sends `pid` SIGUSR1, which a terminal like kitty takes as reloading its config
pub fn user_signal(pid: u32) -> io::Result<()> {
    signal_process(pid, Signal::USR1)
}

fn signal_process(pid: u32, signal: Signal) -> io::Result<()> {
    let pid = i32::try_from(pid)
        .ok()
        .and_then(Pid::from_raw)
        .ok_or_else(|| io::Error::other("not a process id"))?;

    Ok(kill_process(pid, signal)?)
}

/*
 * this process ignores SIGINT, so Ctrl+C ends only the child it waits on, which `default_sigint`
 * gives back the default, and it can still say how the child ended (`kanade activity run`)
 */
pub fn ignore_sigint() {
    // SAFETY: plain call, setting a disposition, no handler
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_IGN);
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
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            Ok(())
        })
    }
}

// what PAM says to the user meanwhile, as the lock screen shows it (ADR 0065)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    // PAM_TEXT_INFO, like "Place your finger on the reader"
    Info(String),

    // PAM_ERROR_MSG
    Error(String),
}

// the longest note kept, in characters: the lock screen draws one line
const NOTE: usize = 64;

// a note's text as one line of printable characters, none if nothing is left
fn clean(text: &str) -> Option<String> {
    let line: String = text
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !c.is_control())
        .collect();
    let line = line.trim();

    if line.is_empty() {
        return None;
    }

    if line.chars().count() <= NOTE {
        return Some(line.to_owned());
    }

    let cut: String = line.chars().take(NOTE - 1).collect();

    Some(format!("{}\u{2026}", cut.trim_end()))
}

// what one message of PAM's conversation gets
#[derive(Debug, PartialEq, Eq)]
enum Turn {
    // the password, the one hidden prompt of a check
    Answer,

    // shown to the user, answered with nothing
    Tell(Note),

    // nothing to show or answer
    Skip,
}

/*
 * what each of `messages`, a conversation's styles and texts, gets, or none when it cannot be
 * answered, which fails the check. The password answers the first hidden prompt of a check and
 * nothing else: not an echoed prompt, which is a name or a code, nor a second hidden one, which is
 * another secret (a one-time code, a PIN), nor a style this does not know. `answered` is whether
 * the password was given already, and is set when it is
 */
fn decide(messages: &[(c_int, String)], answered: &mut bool) -> Option<Vec<Turn>> {
    let mut given = *answered;
    let mut turns = Vec::with_capacity(messages.len());

    for (style, text) in messages {
        turns.push(match *style {
            PAM_PROMPT_ECHO_OFF if !given => {
                given = true;
                Turn::Answer
            }
            PAM_TEXT_INFO => clean(text).map_or(Turn::Skip, |text| Turn::Tell(Note::Info(text))),
            PAM_ERROR_MSG => clean(text).map_or(Turn::Skip, |text| Turn::Tell(Note::Error(text))),
            // an echoed prompt, or a second hidden one
            PAM_PROMPT_ECHO_OFF | PAM_PROMPT_ECHO_ON => return None,
            _ => return None,
        });
    }

    *answered = given;

    Some(turns)
}

// what a conversation has to answer with and tell to
struct Talk<'a> {
    password: &'a CString,
    answered: bool,
    tell: &'a mut dyn FnMut(Note),
}

/*
 * whether PAM's `service` accepts `password` for `user`; blocks for as long as PAM takes, seconds
 * for a wrong one. What PAM says meanwhile goes to `tell`, on this thread. The copy it makes is
 * zeroed before it returns; the caller zeroes its own
 */
pub fn authenticate(service: &str, user: &str, password: &str, tell: &mut dyn FnMut(Note)) -> bool {
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

    pam(&service, &user, &password, tell)
}

// one PAM transaction answering with `password`, as `authenticate` says
fn pam(service: &CString, user: &CString, password: &CString, tell: &mut dyn FnMut(Note)) -> bool {
    let mut talk = Talk {
        password,
        answered: false,
        tell,
    };
    let conversation = PamConv {
        converse,
        data: (&raw mut talk).cast(),
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

// answers the first hidden prompt of the check with the password, shows PAM's notes, fails the rest
extern "C" fn converse(
    count: c_int,
    messages: *mut *const PamMessage,
    responses: *mut *mut PamResponse,
    data: *mut c_void,
) -> c_int {
    let Ok(count) = usize::try_from(count) else {
        return PAM_CONV_ERR;
    };

    // SAFETY: `data` is the `Talk` `pam` made, which outlives the transaction and is used by this
    // thread alone
    let talk = unsafe { &mut *data.cast::<Talk<'_>>() };

    // SAFETY: Linux-PAM hands `count` messages, an array of pointers, each to a style and a text
    let texts: Vec<(c_int, String)> = (0..count)
        .map(|index| unsafe {
            let message = &**messages.add(index);
            let text = if message.text.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(message.text)
                    .to_string_lossy()
                    .into_owned()
            };

            (message.style, text)
        })
        .collect();

    // all decided before anything is allocated, so a refusal leaves nothing for PAM to free
    let Some(turns) = decide(&texts, &mut talk.answered) else {
        return PAM_CONV_ERR;
    };

    // SAFETY: Linux-PAM takes the answers, an array it frees, each text with it
    unsafe {
        let answers = libc::calloc(count, size_of::<PamResponse>()).cast::<PamResponse>();
        if answers.is_null() {
            return PAM_CONV_ERR;
        }

        for (index, turn) in turns.into_iter().enumerate() {
            match turn {
                Turn::Answer => {
                    let text = libc::strdup(talk.password.as_ptr());

                    if text.is_null() {
                        for done in 0..index {
                            libc::free((*answers.add(done)).text.cast());
                        }
                        libc::free(answers.cast());

                        return PAM_CONV_ERR;
                    }

                    (*answers.add(index)).text = text;
                }
                Turn::Tell(note) => (talk.tell)(note),
                Turn::Skip => {}
            }
        }

        *responses = answers;
    }

    PAM_SUCCESS
}

fn local(seconds: i64) -> Option<libc::tm> {
    // SAFETY: `tm` is plain integers and a zone pointer, which zero makes null
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };

    // SAFETY: both pointers are to live locals; tm_zone is never read
    let done = unsafe { libc::localtime_r(&raw const seconds, &raw mut tm) };

    (!done.is_null()).then_some(tm)
}

// once, at `at` seconds since the epoch; cancelled if the clock is set
fn arm(timer: impl AsFd, at: i64) -> io::Result<()> {
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

    timerfd_settime(
        timer,
        TimerfdTimerFlags::ABSTIME | TimerfdTimerFlags::CANCEL_ON_SET,
        &when,
    )?;

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

    fn said(styles: &[c_int]) -> Vec<(c_int, String)> {
        styles
            .iter()
            .map(|&style| (style, String::from("a message")))
            .collect()
    }

    // the password goes to one hidden prompt of a check, and never to any other
    #[test]
    fn the_password_answers_only_the_first_hidden_prompt() {
        let mut answered = false;
        let turns = decide(&said(&[PAM_TEXT_INFO, PAM_PROMPT_ECHO_OFF]), &mut answered).unwrap();

        assert_eq!(
            turns.iter().filter(|turn| **turn == Turn::Answer).count(),
            1
        );
        assert!(answered);

        // a second hidden prompt, in this conversation or a later one, is a code or a PIN
        assert_eq!(decide(&said(&[PAM_PROMPT_ECHO_OFF]), &mut answered), None);
        assert_eq!(
            decide(
                &said(&[PAM_PROMPT_ECHO_OFF, PAM_PROMPT_ECHO_OFF]),
                &mut false
            ),
            None
        );

        // an echoed prompt asks for a name or a code; styles unknown are not guessed at
        assert_eq!(decide(&said(&[PAM_PROMPT_ECHO_ON]), &mut false), None);
        assert_eq!(decide(&said(&[5]), &mut false), None);

        // notes alone give the password to nobody
        let mut answered = false;
        let turns = decide(&said(&[PAM_TEXT_INFO, PAM_ERROR_MSG]), &mut answered).unwrap();
        assert!(!answered);
        assert!(turns.iter().all(|turn| matches!(turn, Turn::Tell(_))));
    }

    #[test]
    fn a_note_is_one_short_printable_line() {
        assert_eq!(
            clean("  Place your\tfinger \n"),
            Some(String::from("Place your finger"))
        );
        assert_eq!(clean("a\u{1b}[31mb"), Some(String::from("a[31mb")));
        assert_eq!(clean(" \n\t "), None);

        let line = clean(&"x".repeat(200)).unwrap();
        assert_eq!(line.chars().count(), NOTE);
        assert!(line.ends_with('\u{2026}'));
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

        assert_eq!(child.wait().expect("reaped").signal(), Some(libc::SIGINT));
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

        assert_eq!(i64::from(tm.tm_sec), seconds % 60);
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
        let mut timer = File::from(wall_timer().unwrap());

        // a moment already past fires at once
        let past = unix_seconds() - 1;

        arm(&timer, past).unwrap();

        let mut expirations = [0; 8];
        timer.read_exact(&mut expirations).unwrap();
        assert_eq!(u64::from_ne_bytes(expirations), 1);
    }
}
