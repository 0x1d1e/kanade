//! Native Kanade CLI transport, independent of Amane.
//!
//! Transport only: command parsing, module gating and effects remain with
//! Kanade's existing CLI/IPC policy. The main Wayland loop handles requests
//! via Server::drain on Event::SourcesChanged, then Incoming::respond.
//!
//! One dedicated thread blocks on a private Unix socket and hands requests to
//! the existing Wayland waker. No idle timer, polling or one-thread-per-client
//! fanout. The 4-byte length prefix accepts paths containing newlines/NUL.
//! The reply payload retains the existing "ok/refused/unknown\n..." format.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::wake::Waker;

// Match crate::cli::PROTOCOL until the native CLI replaces Amane's transport.
pub const PROTOCOL: u32 = 4;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ARGUMENTS: usize = 128;
const TIMEOUT: Duration = Duration::from_secs(5);
const MAX_QUEUED: usize = 32;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    version: u32,
    argv: Vec<String>,
}

/// Command to process synchronously on the native shell's policy thread.
pub struct Incoming {
    pub argv: Vec<String>,
    response: mpsc::Sender<String>,
}

impl Incoming {
    /// Exactly one reply; the shell must not report success until action
    /// policy has actually accepted/finished the operation.
    pub fn respond(self, encoded_reply: String) -> Result<(), String> {
        self.response
            .send(encoded_reply)
            .map_err(|_| "the CLI caller disconnected".into())
    }
}

pub struct Server {
    socket: PathBuf,
    closing: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
    requests: mpsc::Receiver<Incoming>,
}

impl Server {
    /// The socket lives at $XDG_RUNTIME_DIR/kanade/native.sock. The parent
    /// directory is 0700 and the socket is 0600; never trust a symlink or
    /// replace an existing listener. Old sockets require explicit cleanup.
    pub fn bind(runtime_dir: &Path, waker: Waker) -> io::Result<Self> {
        let private = runtime_dir.join("kanade");
        fs::create_dir_all(&private)?;
        let metadata = fs::symlink_metadata(&private)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "IPC directory is not private",
            ));
        }
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700))?;
        let socket = private.join("native.sock");
        if fs::symlink_metadata(&socket).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "native IPC socket already exists",
            ));
        }
        let listener = UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        let (sender, requests) = mpsc::sync_channel(MAX_QUEUED);
        let closing = Arc::new(AtomicBool::new(false));
        let thread_closing = closing.clone();
        let worker = match thread::Builder::new()
            .name("kanade-native-ipc".into())
            .spawn(move || serve(listener, sender, waker, thread_closing))
        {
            Ok(worker) => worker,
            Err(error) => {
                drop(fs::remove_file(&socket));
                return Err(error);
            }
        };

        Ok(Self {
            socket,
            closing,
            worker: Some(worker),
            requests,
        })
    }

    pub fn path(&self) -> &Path {
        &self.socket
    }

    /// Called after Event::SourcesChanged. The shell owns the actual
    /// command handler; a service thread cannot mutate UI state directly.
    pub fn drain(&self) -> Vec<Incoming> {
        self.requests.try_iter().collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.closing.store(true, Ordering::Release);
        // An accept call is interrupted by a local connection, not a timer.
        let unblocked = UnixStream::connect(&self.socket).is_ok();
        if unblocked && let Some(worker) = self.worker.take() {
            drop(worker.join());
        }
        drop(fs::remove_file(&self.socket));
    }
}

fn serve(
    listener: UnixListener,
    requests: mpsc::SyncSender<Incoming>,
    waker: Waker,
    closing: Arc<AtomicBool>,
) {
    for accepted in listener.incoming() {
        if closing.load(Ordering::Acquire) {
            break;
        }
        let Ok(mut stream) = accepted else {
            break;
        };
        drop(stream.set_read_timeout(Some(TIMEOUT)));
        drop(stream.set_write_timeout(Some(TIMEOUT)));
        let reply = match request_from(&mut stream) {
            Ok(argv) => {
                let (sender, response) = mpsc::channel();
                match requests.try_send(Incoming {
                    argv,
                    response: sender,
                }) {
                    Ok(()) => {
                        if waker.wake().is_err() {
                            String::from("unknown\nnative event loop is unavailable")
                        } else {
                            response.recv_timeout(TIMEOUT).unwrap_or_else(|_| {
                                String::from("unknown\nshell did not answer within five seconds")
                            })
                        }
                    }
                    Err(mpsc::TrySendError::Full(_)) => {
                        String::from("refused\nnative IPC queue is full")
                    }
                    Err(mpsc::TrySendError::Disconnected(_)) => {
                        String::from("unknown\nnative shell stopped")
                    }
                }
            }
            Err(reason) => format!("refused\n{reason}"),
        };
        drop(send_frame(&mut stream, reply.as_bytes()));
    }
}

fn request_from(stream: &mut UnixStream) -> Result<Vec<String>, String> {
    let payload = read_frame(stream).map_err(|e| format!("bad native IPC request: {e}"))?;
    let envelope: Envelope =
        serde_json::from_slice(&payload).map_err(|_| "invalid JSON request".to_owned())?;
    if envelope.version != PROTOCOL {
        return Err(format!(
            "unsupported CLI protocol version {}",
            envelope.version
        ));
    }
    if envelope.argv.len() > MAX_ARGUMENTS {
        return Err("too many command arguments".into());
    }
    Ok(envelope.argv)
}

fn send_frame(stream: &mut UnixStream, bytes: &[u8]) -> io::Result<()> {
    let len = u32::try_from(bytes.len()).map_err(|_| io::Error::other("IPC response too large"))?;
    if len as usize > MAX_BYTES {
        return Err(io::Error::other("IPC response too large"));
    }
    stream.write_all(&len.to_be_bytes())?;
    stream.write_all(bytes)
}

fn read_frame(stream: &mut UnixStream) -> io::Result<Vec<u8>> {
    let mut len = [0; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 || len > MAX_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "IPC frame exceeds limit",
        ));
    }
    let mut frame = vec![0u8; len];
    stream.read_exact(&mut frame)?;
    Ok(frame)
}

/// Client used by the future CLI cutover; no Amane IPC dependency.
pub fn call(path: &Path, argv: &[String]) -> Result<String, String> {
    let payload = serde_json::to_vec(&Envelope {
        version: PROTOCOL,
        argv: argv.to_vec(),
    })
    .map_err(|e| format!("serializing CLI request: {e}"))?;
    let mut stream = UnixStream::connect(path).map_err(|e| format!("connecting to shell: {e}"))?;
    stream
        .set_read_timeout(Some(TIMEOUT))
        .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
        .map_err(|e| format!("setting IPC deadline: {e}"))?;
    send_frame(&mut stream, &payload).map_err(|e| format!("sending CLI request: {e}"))?;
    let reply = read_frame(&mut stream).map_err(|e| format!("receiving CLI response: {e}"))?;
    String::from_utf8(reply).map_err(|_| "CLI response is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake;
    use std::time::Instant;

    #[test]
    fn requests_are_delivered_to_shell_thread_without_polling() {
        let temp = tempfile::tempdir().unwrap();
        let (wake, mut reader) = wake::pair().unwrap();
        let server = Server::bind(temp.path(), wake).unwrap();
        let path = server.path().to_owned();
        let argv = vec![
            "wallpaper".to_owned(),
            "set".to_owned(),
            "name with\nnewline and \0 NUL".to_owned(),
        ];
        let expected = argv.clone();
        let caller = thread::spawn(move || call(&path, &argv).unwrap());
        let start = Instant::now();
        let commands = loop {
            if wake::drain(&mut reader).unwrap() {
                break server.drain();
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "native IPC did not wake Wayland"
            );
            thread::sleep(Duration::from_millis(5));
        };
        let [command] = <[_; 1]>::try_from(commands).unwrap_or_else(|_| panic!("one command"));
        assert_eq!(command.argv, expected);
        command.respond("ok\naccepted".into()).unwrap();
        assert_eq!(caller.join().unwrap(), "ok\naccepted");
    }

    #[test]
    fn refuses_duplicate_socket_without_clobbering_active_server() {
        let temp = tempfile::tempdir().unwrap();
        let (wake, _) = wake::pair().unwrap();
        let first = Server::bind(temp.path(), wake.clone()).unwrap();
        assert!(Server::bind(temp.path(), wake).is_err());
        assert!(first.path().exists());
    }

    #[test]
    fn frame_limits_block_oversized_and_truncated_requests() {
        let temp = tempfile::tempdir().unwrap();
        let (wake, _) = wake::pair().unwrap();
        let server = Server::bind(temp.path(), wake).unwrap();
        let mut stream = UnixStream::connect(server.path()).unwrap();
        stream.write_all(&(u32::MAX).to_be_bytes()).unwrap();
        let response = read_frame(&mut stream).unwrap();
        assert!(
            String::from_utf8(response)
                .unwrap()
                .starts_with("refused\n")
        );
    }
}
