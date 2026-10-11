//! Mailboxes that outlive the thread reading them. A thread that panics is run again (#95), and
//! what was sent while it was down is still there for the next one, since both ends sit in a static.

use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Mutex, MutexGuard};

use crate::unpoisoned::MutexExt;

// every message sent, in order, for the one thread that reads them
pub struct Queue<T> {
    send: Sender<T>,
    receive: Mutex<Receiver<T>>,
}

impl<T> Queue<T> {
    pub fn new() -> Self {
        let (send, receive) = mpsc::channel();

        Self {
            send,
            receive: Mutex::new(receive),
        }
    }

    pub fn send(&self, message: T) {
        // the receiver lives as long as the queue, so it is never gone
        let _ = self.send.send(message);
    }

    // the reading end, held by the one thread that reads; a run again takes it after the last
    pub fn receiver(&self) -> MutexGuard<'_, Receiver<T>> {
        self.receive.locked()
    }
}

impl<T> Default for Queue<T> {
    fn default() -> Self {
        Self::new()
    }
}

// a wake for a thread that otherwise sleeps until a deadline; one pending is enough, since it
// reads the deadline again when it wakes
pub struct Nudge {
    send: SyncSender<()>,
    receive: Mutex<Receiver<()>>,
}

impl Nudge {
    pub fn new() -> Self {
        let (send, receive) = mpsc::sync_channel(1);

        Self {
            send,
            receive: Mutex::new(receive),
        }
    }

    pub fn nudge(&self) {
        // full means one is already pending
        let _ = self.send.try_send(());
    }

    // the reading end, held by the one thread that sleeps on it
    pub fn receiver(&self) -> MutexGuard<'_, Receiver<()>> {
        self.receive.locked()
    }
}

impl Default for Nudge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::thread;

    use super::*;

    // a thread that panics holding the reading end, as a listener does before it is run again (#95)
    #[test]
    fn a_message_sent_after_a_panic_reaches_the_next_reader() {
        let queue = Arc::new(Queue::new());
        queue.send(1);

        let reader = Arc::clone(&queue);
        let died = thread::spawn(move || {
            let receiver = reader.receiver();
            assert_eq!(receiver.recv(), Ok(1));
            panic!("listener died");
        })
        .join();
        assert!(died.is_err());

        queue.send(2);

        assert_eq!(queue.receiver().recv(), Ok(2));
    }

    #[test]
    fn nudges_coalesce_into_one_wake() {
        let nudge = Nudge::new();
        nudge.nudge();
        nudge.nudge();
        nudge.nudge();

        let receiver = nudge.receiver();
        assert_eq!(receiver.try_recv(), Ok(()));
        assert!(receiver.try_recv().is_err());
    }
}
