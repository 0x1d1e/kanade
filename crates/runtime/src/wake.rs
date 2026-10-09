//! Cross-thread wake without polling or timer threads.
//!
//! A Source changes typed Kanade state then calls Waker::wake. The native
//! Wayland loop polls the read end beside the compositor fd. One wake drains
//! all accumulated signals, and no work is scheduled when all Sources idle.
use std::{
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, PoisonError},
};

#[derive(Clone)]
pub struct Waker {
    writer: Arc<Mutex<UnixStream>>,
}

pub fn pair() -> io::Result<(Waker, UnixStream)> {
    let (reader, writer) = UnixStream::pair()?;
    reader.set_nonblocking(true)?;
    writer.set_nonblocking(true)?;
    Ok((
        Waker {
            writer: Arc::new(Mutex::new(writer)),
        },
        reader,
    ))
}

impl Waker {
    pub fn wake(&self) -> io::Result<()> {
        let mut writer = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        match writer.write(&[1]) {
            Ok(1) => Ok(()),
            // A full socket already contains a wake, so it is sufficient.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Ok(_) => Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "wake write failed",
            )),
            Err(error) => Err(error),
        }
    }
}

/// Called on the native runtime thread after the poll indicates readiness.
pub fn drain(reader: &mut UnixStream) -> io::Result<bool> {
    let mut received = false;
    let mut bytes = [0; 1024];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "wake writer closed",
                ));
            }
            Ok(_) => received = true,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(received),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_source_signals_coalesce_into_one_event() {
        let (waker, mut reader) = pair().unwrap();
        assert!(!drain(&mut reader).unwrap());
        let other = waker.clone();
        for _ in 0..50 {
            waker.wake().unwrap();
            other.wake().unwrap();
        }
        assert!(drain(&mut reader).unwrap());
        assert!(!drain(&mut reader).unwrap());
    }

    #[test]
    fn disconnected_reader_reports_failure_without_panicking() {
        let (waker, reader) = pair().unwrap();
        drop(reader);
        assert!(waker.wake().is_err());
    }
}
