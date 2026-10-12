use std::collections::HashMap;
use std::env;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

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

    // a caller that stopped reading must not freeze the thread answering it
    pub fn send(mut self, reply: &str) {
        let timeout = Some(Duration::from_secs(1));

        if self.stream.set_write_timeout(timeout).is_ok() {
            let _ = self.stream.write_all(reply.as_bytes());
        }
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

    // reads until the sender closes its side, which marks the end of the call
    pub(crate) fn read(reader: impl Read) -> io::Result<Self> {
        let mut lines = BufReader::new(reader).lines();

        let Some(name) = lines.next() else {
            return Err(io::ErrorKind::UnexpectedEof.into());
        };

        let name = name?;

        let arguments = lines.collect::<io::Result<Vec<String>>>()?;

        Ok(Self { name, arguments })
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
