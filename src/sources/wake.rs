//! What wakes a source that reads an Amane service (#40). Amane has no subscription between
//! services, so such a source reads the service again to learn what changed. Rather than read on a
//! timer for good, it waits for the system to announce a change on a command's output, like
//! `pactl subscribe`, then reads at its poll pace while the change settles: the service learns of
//! the change on its own thread, maybe after the announcement, so one read could come too early.
//! A command that is not running cannot announce anything, so while one is down the source polls.

use std::io::{self, BufRead, BufReader};
use std::panic::{self, AssertUnwindSafe};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use crate::supervise;

// how long until a command runs again after it ended, doubling up to `LAST_RETRY` while it keeps
// failing
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);

// how long a command must have run for its end to count as what it follows going away rather than
// failing
const HEALTHY: Duration = Duration::from_secs(60);

// ties each program's life to Kanade's (`spawn`)
pub const SETPRIV: &str = "setpriv";

// what setpriv exits with when it cannot find the program, like a shell
const NOT_FOUND: i32 = 127;

/*
 * runs `program` until `stop`, `follow` reading its output until it ends and saying why; ran again
 * after a backoff, or after `supervise::RESTART` at least when `follow` panicked. Returns why
 * `program` cannot be started at all, like when it is missing, or none once stopped. Call it from a
 * thread that lives as long as Kanade: the program dies with that thread
 */
pub fn run(
    program: &str,
    args: &[&str],
    stop: &Stop,
    mut follow: impl FnMut(BufReader<ChildStdout>) -> io::Error,
) -> Option<io::Error> {
    let command = [program, args.join(" ").as_str()].join(" ");
    let mut wait = FIRST_RETRY;

    loop {
        // started under the lock, so a stop either comes first or finds the program to kill
        let output = {
            let mut running = stop.lock();

            if running.stopped {
                return None;
            }

            let mut child = match spawn(program, args) {
                Ok(child) => child,
                Err(error) => return Some(error),
            };

            let output = child.stdout.take();
            running.child = Some(child);
            output
        };

        let started = Instant::now();
        let followed = match output {
            Some(output) => {
                panic::catch_unwind(AssertUnwindSafe(|| follow(BufReader::new(output))))
            }
            None => Ok(io::Error::other("no output")),
        };

        let child = stop.lock().child.take();

        if let Some(mut child) = child {
            drop(child.kill());

            if let Ok(status) = child.wait()
                && status.code() == Some(NOT_FOUND)
            {
                return Some(io::ErrorKind::NotFound.into());
            }
        }

        if stop.lock().stopped {
            return None;
        }

        wait = retry(wait, started.elapsed());

        // a panic, already printed by the panic hook, waits as long as any source's restart
        let (lost, pause) = match followed {
            Ok(lost) => (lost, wait),
            Err(payload) => {
                supervise::panicked(thread::current().name().unwrap_or(program), &*payload);
                (io::Error::other("panicked"), wait.max(supervise::RESTART))
            }
        };

        eprintln!("kanade: lost `{command}` ({lost}), running it again in {pause:?}");

        if stop.pause(pause) {
            return None;
        }

        wait = (wait * 2).min(LAST_RETRY);
    }
}

/*
 * through setpriv, so the kernel kills the program when Kanade dies, even killed, rather than
 * leave it running for nobody; without setpriv the program runs on its own, and may outlive Kanade
 */
fn spawn(program: &str, args: &[&str]) -> io::Result<Child> {
    guarded(program, args, |command| {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    })
}

// starts `program` through setpriv as `spawn` says, `start` setting up its pipes and starting it
fn guarded(
    program: &str,
    args: &[&str],
    start: impl Fn(&mut Command) -> io::Result<Child>,
) -> io::Result<Child> {
    let guarded = start(
        Command::new(SETPRIV)
            .args(["--pdeathsig", "KILL", program])
            .args(args),
    );

    match guarded {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            start(Command::new(program).args(args))
        }
        guarded => guarded,
    }
}

/*
 * runs `program` to do one thing and waits for it to exit (ADR 0011: an action), dying with Kanade
 * as `spawn` says; never runs it again. Says why it failed, with what it printed on stderr
 */
pub fn act(program: &str, args: &[&str]) -> Result<(), String> {
    query(program, args).map(drop)
}

// an action that reads one thing, as `act`, and hands back what it printed on stdout
pub fn query(program: &str, args: &[&str]) -> Result<String, String> {
    let command = [program, args.join(" ").as_str()].join(" ");

    let child = guarded(program, args, |command| {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
    });

    let output = child
        .and_then(Child::wait_with_output)
        .map_err(|error| format!("cannot run `{command}`: {error}"))?;

    if output.status.success() {
        return String::from_utf8(output.stdout)
            .map_err(|_| format!("`{command}` printed something that is not text"));
    }

    if output.status.code() == Some(NOT_FOUND) {
        return Err(format!("cannot run `{command}`: {program} not found"));
    }

    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();

    Err(match said.is_empty() {
        true => format!("`{command}` failed ({})", output.status),
        false => format!("`{command}` failed ({}): {said}", output.status),
    })
}

// ends what `run` runs from another thread: kills the program, or ends the wait to run it again,
// and it does not run again
#[derive(Clone, Default)]
pub struct Stop(Arc<(Mutex<Running>, Condvar)>);

#[derive(Default)]
struct Running {
    stopped: bool,
    child: Option<Child>,
}

impl Stop {
    pub fn stop(&self) {
        let mut running = self.lock();
        running.stopped = true;

        if let Some(child) = &mut running.child {
            drop(child.kill());
        }

        self.0.1.notify_all();
    }

    // waits `pause` unless stopped first; says whether it was
    fn pause(&self, pause: Duration) -> bool {
        let (running, _) = self
            .0
            .1
            .wait_timeout_while(self.lock(), pause, |running| !running.stopped)
            .unwrap_or_else(PoisonError::into_inner);

        running.stopped
    }

    fn lock(&self) -> MutexGuard<'_, Running> {
        self.0.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// how long to wait before running a command again: soon after a run long enough to have followed
// what it follows, else as long as the backoff has grown, so one that dies right after its first
// print still backs off
fn retry(wait: Duration, ran: Duration) -> Duration {
    if ran >= HEALTHY { FIRST_RETRY } else { wait }
}

// a command whose output announces changes, one line each
#[derive(Clone, Copy)]
pub struct Announcer {
    pub program: &'static str,
    pub args: &'static [&'static str],

    // whether a line announces a change the source reads; the rest are noise, like a header
    pub announces: fn(&str) -> bool,
}

// how often a source reads
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pace {
    // between reads while a change settles, or while an announcer is down
    pub poll: Duration,

    // how long reads go on at `poll` after the last announcement or changed reading
    pub settle: Duration,

    // between reads while nothing happens, for what nothing announces; none waits for good
    pub idle: Option<Duration>,
}

impl Pace {
    // how long to wait for an announcement before reading anyway
    fn timeout(self, blind: bool, settling: bool) -> Option<Duration> {
        if blind || settling {
            Some(self.poll)
        } else {
            self.idle
        }
    }
}

enum Message {
    Announced,

    // an announcer started, so it may have missed a change while it was down
    Up,

    Down,
}

// what wakes one source: its announcers, each on its own thread, stopped once it is dropped
pub struct Wakes {
    pace: Pace,
    messages: Receiver<Message>,

    // so a source that restarts does not leave its old announcers running for nobody
    stops: Vec<Stop>,

    // announcers not running; down until they first start
    down: usize,

    // reads go on at the poll pace until then
    settling: Instant,
}

impl Wakes {
    pub fn new(pace: Pace, announcers: Vec<Announcer>) -> Wakes {
        let (send, messages) = mpsc::channel();
        let down = announcers.len();
        let mut stops = Vec::new();

        for announcer in announcers {
            let send = send.clone();
            let stop = Stop::default();

            stops.push(stop.clone());
            supervise::spawn(announcer.program, move || {
                announce(announcer, &stop, send.clone());
            });
        }

        Wakes {
            pace,
            messages,
            stops,
            down,
            settling: Instant::now() + pace.settle,
        }
    }

    /*
     * after a read: waits until the next one is due. `busy` says the read found a change, or
     * something the source still waits for, so reads go on at the poll pace for a while
     */
    pub fn wait(&mut self, busy: bool) {
        if busy {
            self.settle();
        }

        let now = Instant::now();
        let timeout = self.pace.timeout(self.down > 0, now < self.settling);

        let message = match timeout {
            Some(timeout) => self.messages.recv_timeout(timeout),
            None => self
                .messages
                .recv()
                .map_err(|_| RecvTimeoutError::Disconnected),
        };

        match message {
            Ok(Message::Announced) => self.settle(),
            Ok(Message::Up) => {
                self.down -= 1;
                self.settle();
            }
            Ok(Message::Down) => self.down += 1,
            Err(RecvTimeoutError::Timeout) => {}

            // every announcer could not start, so none is left to announce; poll rather than spin
            Err(RecvTimeoutError::Disconnected) => thread::sleep(self.pace.poll),
        }
    }

    fn settle(&mut self) {
        self.settling = Instant::now() + self.pace.settle;
    }
}

impl Drop for Wakes {
    fn drop(&mut self) {
        self.stops.iter().for_each(Stop::stop);
    }
}

// runs on its own thread until stopped; an announcer that cannot start stays down, so its source
// polls
fn announce(announcer: Announcer, stop: &Stop, send: Sender<Message>) {
    let Announcer {
        program,
        args,
        announces,
    } = announcer;

    let error = run(program, args, stop, |output| {
        let _up = Up::new(&send);

        follow(output, announces, &send)
    });

    if let Some(error) = error {
        eprintln!("kanade: cannot run {program} ({error}), polling instead");
    }
}

// an announcer's output being read: says Up, and Down once it ends, even by a panic, so the count
// of announcers down stays true across a restart
struct Up<'a>(&'a Sender<Message>);

impl<'a> Up<'a> {
    fn new(send: &'a Sender<Message>) -> Self {
        drop(send.send(Message::Up));

        Up(send)
    }
}

impl Drop for Up<'_> {
    fn drop(&mut self) {
        drop(self.0.send(Message::Down));
    }
}

// sends each announcement until the output ends; returns why it ended
fn follow(output: impl BufRead, announces: fn(&str) -> bool, send: &Sender<Message>) -> io::Error {
    for line in output.lines() {
        match line {
            Ok(line) if announces(&line) => drop(send.send(Message::Announced)),
            Ok(_) => {}
            Err(error) => return error,
        }
    }

    io::ErrorKind::UnexpectedEof.into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PACE: Pace = Pace {
        poll: Duration::from_millis(80),
        settle: Duration::from_secs(1),
        idle: None,
    };

    #[test]
    fn retries_back_off_until_a_run_is_healthy() {
        let short = Duration::from_millis(50);

        assert_eq!(retry(Duration::from_secs(8), short), Duration::from_secs(8));
        assert_eq!(retry(Duration::from_secs(8), HEALTHY), FIRST_RETRY);
    }

    #[test]
    fn an_action_says_why_it_failed() {
        assert_eq!(act("true", &[]), Ok(()));

        let missing = act("kanade-no-such-program", &["x"]).unwrap_err();
        assert!(
            missing.starts_with("cannot run `kanade-no-such-program x`"),
            "{missing}"
        );

        let failed = act("sh", &["-c", "echo nope >&2; exit 3"]).unwrap_err();
        assert!(
            failed.starts_with("`sh -c echo nope >&2; exit 3` failed"),
            "{failed}"
        );
        assert!(failed.ends_with(": nope"), "{failed}");
    }

    #[test]
    fn a_query_hands_back_what_it_printed() {
        assert_eq!(query("echo", &["graph"]), Ok("graph\n".into()));
    }

    #[test]
    fn a_missing_program_is_not_retried() {
        let error = run("kanade-no-such-program", &[], &Stop::default(), |output| {
            output.lines().count();
            io::ErrorKind::UnexpectedEof.into()
        });

        assert_eq!(
            error.map(|error| error.kind()),
            Some(io::ErrorKind::NotFound)
        );
    }

    // a source restarting drops its Wakes, and the announcers it started must not outlive it
    #[test]
    fn a_stopped_program_is_killed_and_not_run_again() {
        let stop = Stop::default();
        let (started, runs) = mpsc::channel();

        let running = thread::spawn({
            let stop = stop.clone();

            move || {
                run("sleep", &["60"], &stop, |output| {
                    started.send(()).unwrap();
                    output.lines().count();
                    io::ErrorKind::UnexpectedEof.into()
                })
            }
        });

        runs.recv().unwrap();
        let stopping = Instant::now();
        stop.stop();

        assert!(running.join().unwrap().is_none());
        assert!(stopping.elapsed() < FIRST_RETRY);
        assert!(runs.try_recv().is_err());

        // stopped before it ran, it never does
        assert!(run("sleep", &["60"], &stop, |_| unreachable!()).is_none());
    }

    // a program that ended waits out its backoff, and a stop must end that wait too
    #[test]
    fn a_stop_between_runs_ends_the_backoff() {
        let stop = Stop::default();
        let (ended, runs) = mpsc::channel();

        let running = thread::spawn({
            let stop = stop.clone();

            move || {
                run("true", &[], &stop, |output| {
                    output.lines().count();
                    ended.send(()).unwrap();
                    io::ErrorKind::UnexpectedEof.into()
                })
            }
        });

        // well into the first backoff, of FIRST_RETRY
        runs.recv().unwrap();
        thread::sleep(Duration::from_millis(200));

        let stopping = Instant::now();
        stop.stop();

        assert!(running.join().unwrap().is_none());
        assert!(stopping.elapsed() < Duration::from_millis(300));
        assert!(runs.try_recv().is_err());
    }

    #[test]
    fn an_idle_source_waits_for_an_announcement() {
        assert_eq!(PACE.timeout(false, false), None);

        let slow = Pace {
            idle: Some(Duration::from_secs(5)),
            ..PACE
        };
        assert_eq!(slow.timeout(false, false), Some(Duration::from_secs(5)));
    }

    #[test]
    fn a_settling_or_blind_source_polls() {
        assert_eq!(PACE.timeout(false, true), Some(PACE.poll));
        assert_eq!(PACE.timeout(true, false), Some(PACE.poll));
    }

    fn announced(output: &str) -> usize {
        let (send, messages) = mpsc::channel();
        let lost = follow(output.as_bytes(), |line| line.starts_with('!'), &send);

        assert_eq!(lost.kind(), io::ErrorKind::UnexpectedEof);
        drop(send);

        messages.iter().count()
    }

    #[test]
    fn only_announcing_lines_wake() {
        assert_eq!(announced("header\n!one\nnoise\n!two\n"), 2);
        assert_eq!(announced(""), 0);
    }

    #[test]
    fn a_wait_ends_at_an_announcement() {
        let (send, messages) = mpsc::channel();
        let mut wakes = Wakes {
            pace: PACE,
            messages,
            stops: Vec::new(),
            down: 0,
            settling: Instant::now(),
        };

        send.send(Message::Announced).unwrap();
        wakes.wait(false);

        assert!(wakes.settling > Instant::now());
    }

    #[test]
    fn an_announcer_that_starts_ends_the_blind_polling() {
        let (send, messages) = mpsc::channel();
        let mut wakes = Wakes {
            pace: PACE,
            messages,
            stops: Vec::new(),
            down: 1,
            settling: Instant::now(),
        };

        send.send(Message::Up).unwrap();
        wakes.wait(false);
        assert_eq!(wakes.down, 0);

        send.send(Message::Down).unwrap();
        wakes.wait(false);
        assert_eq!(wakes.down, 1);
    }
}
