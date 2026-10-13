//! A helper program run to its end, within a timeout, that a newer request can cut short. Its output
//! is read as it is written and capped, so a chatty helper never blocks on a full pipe.

use std::io::{self, Read};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::thread::{self, Builder, JoinHandle};
use std::time::{Duration, Instant};

const POLL_INTERVAL: Duration = Duration::from_millis(20);
const OUTPUT_LIMIT: usize = 1024 * 1024;

// how a helper ended: its stdout, or why not, unless a newer request `N` cut it short
pub enum Ran<N, T> {
    Done(Result<T, String>),
    Superseded(N),
}

// what stops a helper before it ends, as the caller's queue says
pub enum Interrupt<N> {
    Superseded(N),
    Abort(String),
}

type Captured = (Vec<u8>, bool);
type Reader = JoinHandle<io::Result<Captured>>;

// `name` is what the messages call the helper; `interrupt` is asked between polls
pub fn run<N>(
    mut command: Command,
    name: &str,
    timeout: Duration,
    mut interrupt: impl FnMut() -> Option<Interrupt<N>>,
) -> Ran<N, Vec<u8>> {
    let mut child = match command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return Ran::Done(Err(format!("could not run {name}: {error}"))),
    };

    let readers = child
        .stdout
        .take()
        .and_then(|output| capture(output, name, "stdout").ok())
        .zip(
            child
                .stderr
                .take()
                .and_then(|output| capture(output, name, "stderr").ok()),
        );
    let Some((stdout, stderr)) = readers else {
        abandon(child, None);
        return Ran::Done(Err(format!("could not read {name} output")));
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
                return Ran::Done(Err(why));
            }
            None => {}
        }

        match child.try_wait() {
            Ok(Some(status)) => return Ran::Done(finish(name, status, stdout, stderr)),
            Ok(None) => {}
            Err(error) => {
                abandon(child, Some((stdout, stderr)));
                return Ran::Done(Err(format!("could not wait for {name}: {error}")));
            }
        }

        if Instant::now() >= deadline {
            abandon(child, Some((stdout, stderr)));
            return Ran::Done(Err(format!(
                "{name} timed out after {} ms",
                timeout.as_millis()
            )));
        }

        thread::sleep(POLL_INTERVAL);
    }
}

fn capture<R: Read + Send + 'static>(mut stream: R, name: &str, which: &str) -> io::Result<Reader> {
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

                let remaining = OUTPUT_LIMIT.saturating_sub(captured.len());
                captured.extend_from_slice(&buffer[..count.min(remaining)]);
                over_limit |= count > remaining;
            }
        })
}

// kills the child, which ends the readers
fn abandon(mut child: Child, readers: Option<(Reader, Reader)>) {
    let _ = child.kill();
    let _ = child.wait();

    if let Some((stdout, stderr)) = readers {
        let _ = stdout.join();
        let _ = stderr.join();
    }
}

fn finish(
    name: &str,
    status: ExitStatus,
    stdout: Reader,
    stderr: Reader,
) -> Result<Vec<u8>, String> {
    let (stdout, too_much_stdout) = reader(name, stdout, "stdout")?;
    let (stderr, _) = reader(name, stderr, "stderr")?;

    if !status.success() {
        let error = String::from_utf8_lossy(&stderr).trim().to_owned();
        return Err(if error.is_empty() {
            format!("{name} exited with {status}")
        } else {
            error
        });
    }

    if too_much_stdout {
        return Err(format!("{name} output exceeded {OUTPUT_LIMIT} bytes"));
    }

    Ok(stdout)
}

fn reader(name: &str, handle: Reader, which: &str) -> Result<Captured, String> {
    handle
        .join()
        .map_err(|_| format!("{name} {which} reader stopped unexpectedly"))?
        .map_err(|error| format!("could not read {name} {which}: {error}"))
}
