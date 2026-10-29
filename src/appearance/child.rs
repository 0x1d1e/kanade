//! A helper program run to its end, within a timeout, that a newer request can cut short. Its output
//! is read as it is written and capped, so a chatty helper never blocks on a full pipe.

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, Builder, JoinHandle};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const OUTPUT_LIMIT: usize = 1024 * 1024;

// how much of each stream a caller keeps: what a helper says as its answer, and only why it failed
#[derive(Clone, Copy)]
pub struct Limits {
    pub stdout: usize,
    pub stderr: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            stdout: OUTPUT_LIMIT,
            stderr: OUTPUT_LIMIT,
        }
    }
}

// how long a reader may outlast its child: a grandchild that kept the pipe open holds it past that
const READER_GRACE: Duration = Duration::from_millis(250);

// how a helper ended: its stdout, or why not, unless a newer request `N` cut it short. `E` is what
// a caller reads its failure as: the message (`run`), or the typed `Error` (`run_typed`)
pub enum Ran<N, T, E = String> {
    Done(Result<T, E>),
    Superseded(N),
}

/*
 * Why a helper gave no output. What tells a refusal from a helper that may have done its work:
 * only `Exited` is the program itself saying no; every other one is Kanade's side of the run
 * failing, so the helper may have acted before it was lost
 */
#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    // it could not be started, as a missing program
    Spawn(String),

    // it ended with a non-zero status: its own words, or the status when it said nothing
    Exited(String),

    // its output could not be read, or the reader died before it ended
    Reader(String),

    // it wrote more than `OUTPUT_LIMIT` bytes
    TooLong(String),

    // it did not end in time and was killed
    TimedOut(String),

    // the caller cut it short (`Interrupt::Abort`)
    Aborted(String),
}

impl Error {
    /// Whether the helper may have acted before it was lost: everything but its own refusal.
    pub fn unanswered(&self) -> bool {
        !matches!(self, Error::Exited(_))
    }

    /// What to say of it, as the messages and the status lines have said it.
    pub fn message(&self) -> String {
        match self {
            Error::Spawn(why)
            | Error::Exited(why)
            | Error::Reader(why)
            | Error::TooLong(why)
            | Error::TimedOut(why)
            | Error::Aborted(why) => why.clone(),
        }
    }
}

// what stops a helper before it ends, as the caller's queue says
pub enum Interrupt<N> {
    Superseded(N),
    Abort(String),
}

type Captured = (Vec<u8>, bool);
type Reader = JoinHandle<io::Result<Captured>>;

// `name` is what the messages call the helper; `interrupt` is asked between polls. Why it ended as
// its message alone, for a caller that only prints it
pub fn run<N>(
    command: Command,
    name: &str,
    timeout: Duration,
    interrupt: impl FnMut() -> Option<Interrupt<N>>,
) -> Ran<N, Vec<u8>> {
    run_capped(command, name, timeout, Limits::default(), interrupt)
}

// `run`, keeping no more than `limits` of each stream: a helper whose stderr is only why it failed
// need not be allowed megabytes of it
pub fn run_capped<N>(
    command: Command,
    name: &str,
    timeout: Duration,
    limits: Limits,
    interrupt: impl FnMut() -> Option<Interrupt<N>>,
) -> Ran<N, Vec<u8>> {
    match run_typed_capped(command, name, timeout, limits, interrupt) {
        Ran::Done(Ok(output)) => Ran::Done(Ok(output)),
        Ran::Done(Err(error)) => Ran::Done(Err(error.message())),
        Ran::Superseded(newer) => Ran::Superseded(newer),
    }
}

// `run`, keeping why it ended apart: a helper that refused is not one that may have acted
pub fn run_typed<N>(
    command: Command,
    name: &str,
    timeout: Duration,
    interrupt: impl FnMut() -> Option<Interrupt<N>>,
) -> Ran<N, Vec<u8>, Error> {
    run_typed_capped(command, name, timeout, Limits::default(), interrupt)
}

// both, with what of each stream a caller keeps
pub fn run_typed_capped<N>(
    mut command: Command,
    name: &str,
    timeout: Duration,
    limits: Limits,
    mut interrupt: impl FnMut() -> Option<Interrupt<N>>,
) -> Ran<N, Vec<u8>, Error> {
    let mut child = match command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return Ran::Done(Err(Error::Spawn(format!("could not run {name}: {error}"))));
        }
    };

    let readers = child
        .stdout
        .take()
        .and_then(|output| capture(output, name, "stdout", limits.stdout).ok())
        .zip(
            child
                .stderr
                .take()
                .and_then(|output| capture(output, name, "stderr", limits.stderr).ok()),
        );
    let Some((stdout, stderr)) = readers else {
        abandon(child, None);
        return Ran::Done(Err(Error::Reader(format!("could not read {name} output"))));
    };
    let deadline = Instant::now() + timeout;

    loop {
        match interrupt() {
            Some(Interrupt::Superseded(newer)) => {
                abandon(child, Some((stdout, stderr)));
                return Ran::Superseded(newer);
            }
            Some(Interrupt::Abort(why)) => {
                abandon(child, Some((stdout, stderr)));
                return Ran::Done(Err(Error::Aborted(why)));
            }
            None => {}
        }

        match child.try_wait() {
            Ok(Some(status)) => return Ran::Done(finish(name, status, stdout, stderr, limits)),
            Ok(None) => {}
            Err(error) => {
                abandon(child, Some((stdout, stderr)));
                return Ran::Done(Err(Error::Reader(format!(
                    "could not wait for {name}: {error}"
                ))));
            }
        }

        if Instant::now() >= deadline {
            abandon(child, Some((stdout, stderr)));
            return Ran::Done(Err(Error::TimedOut(format!(
                "{name} timed out after {} ms",
                timeout.as_millis()
            ))));
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn capture<R: Read + Send + 'static>(
    mut stream: R,
    name: &str,
    which: &str,
    limit: usize,
) -> io::Result<Reader> {
    Builder::new()
        .name(format!("kanade-{name}-{which}"))
        .spawn(move || {
            let mut captured = Vec::new();
            let mut over_limit = false;
            let mut buffer = [0; 8192];

            loop {
                let count = stream.read(&mut buffer)?;
                if count == 0 {
                    return Ok((captured, over_limit));
                }

                let remaining = limit.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
                over_limit |= count > remaining;
            }
        })
}

// kills the child, which ends the readers unless a grandchild holds the pipes, which are left behind
fn abandon(mut child: Child, readers: Option<(Reader, Reader)>) {
    let _ = child.kill();
    let _ = child.wait();

    if let Some((stdout, stderr)) = readers {
        let _ = settle(stdout);
        let _ = settle(stderr);
    }
}

// a reader's result, or nothing when a grandchild still holds the pipe after `READER_GRACE`
fn settle(handle: Reader) -> Option<thread::Result<io::Result<Captured>>> {
    let end = Instant::now() + READER_GRACE;

    while !handle.is_finished() {
        if Instant::now() >= end {
            return None;
        }

        thread::sleep(Duration::from_millis(5));
    }

    Some(handle.join())
}

fn finish(
    name: &str,
    status: ExitStatus,
    stdout: Reader,
    stderr: Reader,
    limits: Limits,
) -> Result<Vec<u8>, Error> {
    let (stdout, too_much_stdout) = reader(name, stdout, "stdout")?;
    let (stderr, _) = reader(name, stderr, "stderr")?;

    if !status.success() {
        let error = String::from_utf8_lossy(&stderr).trim().to_owned();

        return Err(Error::Exited(if error.is_empty() {
            format!("{name} exited with {status}")
        } else {
            error
        }));
    }

    if too_much_stdout {
        return Err(Error::TooLong(format!(
            "{name} output exceeded {} bytes",
            limits.stdout
        )));
    }

    Ok(stdout)
}

fn reader(name: &str, handle: Reader, which: &str) -> Result<Captured, Error> {
    settle(handle)
        .unwrap_or_else(|| Ok(Ok((Vec::new(), false))))
        .map_err(|_| Error::Reader(format!("{name} {which} reader stopped unexpectedly")))?
        .map_err(|error| Error::Reader(format!("could not read {name} {which}: {error}")))
}
