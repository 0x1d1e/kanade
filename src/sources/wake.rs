//! What wakes a source that reads an Amane service (#40). Amane has no subscription between
//! services, so such a source reads the service again to learn what changed. Rather than read on a
//! timer for good, it waits for the system to announce a change on a command's output, like
//! `pactl subscribe`, then reads at its poll pace while the change settles: the service learns of
//! the change on its own thread, maybe after the announcement, so one read could come too early.
//! A command that is not running cannot announce anything, so while one is down the source polls.

use std::io::{self, BufRead, BufReader};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

// how long until a command runs again after it ended, doubling up to `LAST_RETRY` while it keeps
// failing
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LAST_RETRY: Duration = Duration::from_secs(60);

// how long a command must have run for its end to count as what it follows going away rather than
// failing
const HEALTHY: Duration = Duration::from_secs(60);

// what setpriv exits with when it cannot find the program, like a shell
const NOT_FOUND: i32 = 127;

/*
 * runs `program` for good, `follow` reading its output until it ends and saying why; ran again
 * after a backoff. Returns only when `program` cannot be started at all, like when it is missing.
 * Call it from a thread that lives as long as Kanade: the program dies with that thread
 */
pub fn run(
    program: &str,
    args: &[&str],
    mut follow: impl FnMut(BufReader<ChildStdout>) -> io::Error,
) -> io::Error {
    let command = [program, args.join(" ").as_str()].join(" ");
    let mut wait = FIRST_RETRY;

    loop {
        let mut child = match spawn(program, args) {
            Ok(child) => child,
            Err(error) => return error,
        };

        let started = Instant::now();
        let lost = match child.stdout.take() {
            Some(output) => follow(BufReader::new(output)),
            None => io::Error::other("no output"),
        };

        drop(child.kill());

        if let Ok(status) = child.wait()
            && status.code() == Some(NOT_FOUND)
        {
            return io::ErrorKind::NotFound.into();
        }

        wait = retry(wait, started.elapsed());
        eprintln!("kanade: lost `{command}` ({lost}), running it again in {wait:?}");

        thread::sleep(wait);
        wait = (wait * 2).min(LAST_RETRY);
    }
}

/*
 * through setpriv, so the kernel kills the program when Kanade dies, even killed, rather than
 * leave it running for nobody; without setpriv the program runs on its own, and may outlive Kanade
 */
fn spawn(program: &str, args: &[&str]) -> io::Result<Child> {
    let spawn = |command: &mut Command| {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    };

    let guarded = spawn(
        Command::new("setpriv")
            .args(["--pdeathsig", "KILL", program])
            .args(args),
    );

    match guarded {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            spawn(Command::new(program).args(args))
        }
        guarded => guarded,
    }
}

// how long to wait before running a command again: soon after a run long enough to have followed
// what it follows, else as long as the backoff has grown, so one that dies right after its first
// print still backs off
fn retry(wait: Duration, ran: Duration) -> Duration {
    if ran >= HEALTHY { FIRST_RETRY } else { wait }
}

// a command whose output announces changes, one line each
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

// what wakes one source: its announcers, each on its own thread
pub struct Wakes {
    pace: Pace,
    messages: Receiver<Message>,

    // announcers not running; down until they first start
    down: usize,

    // reads go on at the poll pace until then
    settling: Instant,
}

impl Wakes {
    pub fn new(pace: Pace, announcers: Vec<Announcer>) -> Wakes {
        let (send, messages) = mpsc::channel();
        let down = announcers.len();

        for announcer in announcers {
            let send = send.clone();
            thread::spawn(move || announce(announcer, send));
        }

        Wakes {
            pace,
            messages,
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

            // announcers run for good, so this is only a panicked one; poll rather than spin
            Err(RecvTimeoutError::Disconnected) => thread::sleep(self.pace.poll),
        }
    }

    fn settle(&mut self) {
        self.settling = Instant::now() + self.pace.settle;
    }
}

// runs on its own thread for good; an announcer that cannot start stays down, so its source polls
fn announce(announcer: Announcer, send: Sender<Message>) {
    let Announcer {
        program,
        args,
        announces,
    } = announcer;

    let error = run(program, args, |output| {
        drop(send.send(Message::Up));
        let lost = follow(output, announces, &send);
        drop(send.send(Message::Down));

        lost
    });

    eprintln!("kanade: cannot run {program} ({error}), polling instead");
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
    fn a_missing_program_is_not_retried() {
        let error = run("kanade-no-such-program", &[], |output| {
            output.lines().count();
            io::ErrorKind::UnexpectedEof.into()
        });

        assert_eq!(error.kind(), io::ErrorKind::NotFound);
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
