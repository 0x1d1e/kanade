use std::collections::{HashMap, VecDeque};
use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// the most a call may send, past the longest `kanade dmenu`, and how long it may take to
const MOST: u64 = 24 * 1024 * 1024;
const PATIENCE: Duration = Duration::from_secs(2);

// Fixed writers and a short queue keep non-reading clients from growing threads or memory.
const REPLY_WRITERS: usize = 4;
const PENDING_REPLIES: usize = 16;

pub struct IpcCall {
    pub(crate) name: String,

    pub(crate) arguments: Vec<String>,
}

/*
 * a handler answers through the `IpcReply` it is given, at once or later from anywhere, as a
 * call that waits on the user does; dropped unanswered, the caller reads an empty reply
 */
type Handler = fn(&[String], IpcReply);

/*
 * whether a call changes what a window draws itself, on top of whatever Services it writes: a
 * handler that leaves every change to a Service does not, since the write already asked the event
 * loop for the frames of the windows that read it. Marking every window for a call that drew
 * nothing only drew frames the change never needed
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Draws {
    Yes,
    No,
}

impl Draws {
    pub fn of(draws: bool) -> Self {
        if draws { Draws::Yes } else { Draws::No }
    }

    pub fn any(self) -> bool {
        self == Draws::Yes
    }
}

// where one call's answer goes; the caller waits on it until it is sent or dropped
pub struct IpcReply {
    stream: UnixStream,
}

impl IpcReply {
    pub(crate) fn new(stream: UnixStream) -> Self {
        Self { stream }
    }

    /*
     * Replies run away from the event loop because a caller may stop reading. A full queue drops
     * the answer and closes the stream; the caller sees EOF instead of blocking the shell.
     */
    pub fn send(self, reply: &str) {
        let _ = REPLIES.enqueue((self.stream, reply.to_owned()));
    }
}

struct ReplyQueue {
    pending: Mutex<VecDeque<(UnixStream, String)>>,
    ready: Condvar,
}

impl ReplyQueue {
    fn enqueue(&self, reply: (UnixStream, String)) -> Result<(), (UnixStream, String)> {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if pending.len() == PENDING_REPLIES {
            return Err(reply);
        }
        pending.push_back(reply);
        self.ready.notify_one();
        Ok(())
    }

    fn take(&self) -> (UnixStream, String) {
        let mut pending = self
            .pending
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        loop {
            if let Some(reply) = pending.pop_front() {
                return reply;
            }
            pending = self
                .ready
                .wait(pending)
                .unwrap_or_else(|error| error.into_inner());
        }
    }
}

static REPLIES: LazyLock<Arc<ReplyQueue>> = LazyLock::new(|| {
    let queue = Arc::new(ReplyQueue {
        pending: Mutex::new(VecDeque::with_capacity(PENDING_REPLIES)),
        ready: Condvar::new(),
    });

    for _ in 0..REPLY_WRITERS {
        let queue = Arc::clone(&queue);
        thread::Builder::new()
            .name(String::from("kanade-ipc-reply"))
            .spawn(move || {
                loop {
                    let (stream, reply) = queue.take();
                    let _ = catch_unwind(AssertUnwindSafe(|| written(stream, &reply)));
                }
            })
            .expect("failed to start ipc reply worker");
    }

    queue
});

// the answer, given up on once the caller has stopped reading it
fn written(mut stream: UnixStream, reply: &str) {
    let timeout = Some(Duration::from_secs(1));

    if stream.set_write_timeout(timeout).is_ok() {
        let _ = stream.write_all(reply.as_bytes());
    }
}

#[derive(Clone, Copy)]
pub struct Registered {
    pub handler: Handler,

    // read before the handler runs, from the words of the call alone
    pub draws: fn(&[String]) -> Draws,
}

#[derive(Default)]
pub struct IpcHandlers {
    pub by_name: HashMap<String, Registered>,
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
    pub fn insert(&mut self, name: &str, handler: Handler, draws: fn(&[String]) -> Draws) {
        self.by_name
            .insert(String::from(name), Registered { handler, draws });
    }

    /*
     * runs one call, answering what it drew: the reply is whatever the handler sends, so an
     * unknown name answers in words too, and draws nothing
     */
    pub fn run(&self, call: &IpcCall, reply: IpcReply) -> Draws {
        let Some(registered) = self.by_name.get(&call.name) else {
            reply.send(&format!("no handler named {}", call.name));
            return Draws::No;
        };

        let draws = (registered.draws)(&call.arguments);

        (registered.handler)(&call.arguments, reply);

        draws
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

    fn answering(_: &[String], reply: IpcReply) {
        reply.send("answered");
    }

    fn drawn(_: &[String]) -> Draws {
        Draws::Yes
    }

    fn undrawn(_: &[String]) -> Draws {
        Draws::No
    }

    /*
     * what a call drew is its handler's answer, which the event loop reads to know whether the
     * windows must be drawn again: a handler that leaves its change to a Service write is not,
     * and a name with no handler is answered in words and draws nothing either
     */
    #[test]
    fn a_call_answers_what_its_handler_drew() {
        let mut handlers = IpcHandlers::default();

        handlers.insert("drawn", answering, drawn);
        handlers.insert("undrawn", answering, undrawn);

        for (name, expected) in [("drawn", Draws::Yes), ("undrawn", Draws::No)] {
            let call = IpcCall::read(format!("{name}\n").as_bytes()).unwrap();
            let (writing, mut reading) = UnixStream::pair().unwrap();

            assert_eq!(handlers.run(&call, IpcReply::new(writing)), expected);

            let mut answer = String::new();
            reading.read_to_string(&mut answer).unwrap();

            assert_eq!(answer, "answered");
        }

        let call = IpcCall::read("unknown\n".as_bytes()).unwrap();
        let (writing, mut reading) = UnixStream::pair().unwrap();

        assert_eq!(handlers.run(&call, IpcReply::new(writing)), Draws::No);

        let mut answer = String::new();
        reading.read_to_string(&mut answer).unwrap();

        assert_eq!(answer, "no handler named unknown");
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

        // Closing the peer releases the writer; its 1 s timeout bounds a write already in progress.
        drop(reading);
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
