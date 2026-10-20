use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::thread;
use std::time::{Duration, Instant};

// the most a call may send, past the longest `kanade dmenu`, and how long it may take to
const MOST: u64 = 24 * 1024 * 1024;
const PATIENCE: Duration = Duration::from_secs(2);

pub struct IpcCall {
    pub(crate) name: String,

    pub(crate) arguments: Vec<String>,
}

/*
 * a handler answers through the `IpcReply` it is given, at once or later from anywhere, as a
 * call that waits on the user does; dropped unanswered, the caller reads an empty reply
 */
type Handler = fn(&[String], IpcReply);

// where one call's answer goes; the caller waits on it until it is sent or dropped
pub struct IpcReply {
    stream: UnixStream,
}

impl IpcReply {
    pub(crate) fn new(stream: UnixStream) -> Self {
        Self { stream }
    }

    /*
     * the answer is written off the caller's thread: a caller that stopped reading fills its
     * socket's buffer, and a write that waited on it where the handler runs would freeze the draw
     * thread with it. Each answer gets its own thread, so one stuck caller holds up no other, and
     * no lock is held while a write waits on a reader
     */
    pub fn send(self, reply: &str) {
        let stream = self.stream;
        let reply = reply.to_owned();

        thread::spawn(move || written(stream, &reply));
    }
}

// the answer, given up on once the caller has stopped reading it
fn written(mut stream: UnixStream, reply: &str) {
    let timeout = Some(Duration::from_secs(1));

    if stream.set_write_timeout(timeout).is_ok() {
        let _ = stream.write_all(reply.as_bytes());
    }
}

#[derive(Default)]
pub struct IpcHandlers {
    pub by_name: HashMap<String, Handler>,
}

impl IpcCall {
    pub fn new(name: &str, arguments: &[String]) -> Self {
        Self {
            name: String::from(name),
            arguments: arguments.to_vec(),
        }
    }

    // the name goes on the first line, then each argument on its own line
    pub fn write(&self, writer: &mut impl Write) -> io::Result<()> {
        writeln!(writer, "{}", self.name)?;

        for argument in &self.arguments {
            writeln!(writer, "{argument}")?;
        }

        Ok(())
    }

    /*
     * reads until the sender closes its side, which marks the end of the call; a sender that
     * sends too much, or drips it past `PATIENCE`, is cut off, since the shell waits on it
     */
    pub(crate) fn read(reader: impl Read) -> io::Result<Self> {
        let bounded = Bounded {
            reader: reader.take(MOST + 1),
            until: Instant::now() + PATIENCE,
        };
        let mut lines = BufReader::new(bounded).lines();

        let Some(name) = lines.next() else {
            return Err(io::ErrorKind::UnexpectedEof.into());
        };

        let name = name?;

        let arguments = lines.collect::<io::Result<Vec<String>>>()?;

        Ok(Self { name, arguments })
    }
}

// a reader that fails past `until`, and once its `Take` ran out with more to come
struct Bounded<R> {
    reader: io::Take<R>,
    until: Instant,
}

impl<R: Read> Read for Bounded<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if Instant::now() > self.until {
            return Err(io::ErrorKind::TimedOut.into());
        }
        if self.reader.limit() == 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "call too long"));
        }

        self.reader.read(buffer)
    }
}

impl IpcHandlers {
    pub fn insert(&mut self, name: &str, handler: Handler) {
        self.by_name.insert(String::from(name), handler);
    }

    // the reply is whatever the handler sends, so an unknown name answers in words too
    pub fn run(&self, call: &IpcCall, reply: IpcReply) {
        let Some(handler) = self.by_name.get(&call.name) else {
            reply.send(&format!("no handler named {}", call.name));
            return;
        };

        handler(&call.arguments, reply);
    }
}

pub fn ipc_socket() -> PathBuf {
    // every wayland session sets this, and it is private to the logged in user
    let runtime_directory =
        env::var_os("XDG_RUNTIME_DIR").expect("failed to find socket: XDG_RUNTIME_DIR is not set");

    PathBuf::from(runtime_directory).join("kanade.sock")
}

#[cfg(test)]
mod tests {
    use super::*;

    // the shell's own thread reads a call, so one past the bound is refused, not read on
    #[test]
    fn a_call_past_the_bound_is_refused() {
        let call = IpcCall::read("dmenu\n--\nline\n".as_bytes()).unwrap();
        assert_eq!((call.name.as_str(), call.arguments.len()), ("dmenu", 2));

        let long = vec![b'x'; MOST as usize + 1];
        let error = IpcCall::read(long.as_slice()).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[cfg(test)]
mod reply_tests {
    use super::*;

    use std::io::Read;
    use std::time::Instant;

    /*
     * a caller that never reads fills its socket's buffer; the answer waited on it where the
     * handler ran, freezing the draw thread for the whole write timeout. `send` now hands the
     * stream to a thread of its own and returns at once
     */
    #[test]
    fn sending_never_waits_for_a_caller_that_stopped_reading() {
        let (writing, reading) = UnixStream::pair().unwrap();

        // far past what the socket holds, so the write waits on a reader that never comes
        let answer = "x".repeat(8 << 20);

        let start = Instant::now();
        IpcReply::new(writing).send(&answer);

        assert!(
            start.elapsed() < Duration::from_millis(100),
            "send waited {:?} for a caller that reads nothing",
            start.elapsed()
        );

        /*
         * the caller's end stays open, so the write waits on it rather than failing at once; it is
         * left open for good, as the writing thread is still waiting on it
         */
        std::mem::forget(reading);
    }

    // an answer reaches a caller that reads it whole
    #[test]
    fn an_answer_reaches_the_caller() {
        let (writing, mut reading) = UnixStream::pair().unwrap();

        IpcReply::new(writing).send("done\n");

        let mut answer = String::new();
        reading.read_to_string(&mut answer).unwrap();

        assert_eq!(answer, "done\n");
    }
}
