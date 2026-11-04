use crate::input::ReadyHandler;

// Readiness needs both a drawn key handler and the compositor's keyboard enter. Either can arrive
// first; drawing a new visit while this window already owns the keyboard must notify it too.
#[derive(Default)]
pub(super) struct KeyboardReady {
    focused: bool,
    handler: Option<ReadyHandler>,
}

impl KeyboardReady {
    pub fn enter(&mut self) {
        self.focused = true;
        self.notify();
    }

    pub fn leave(&mut self) {
        self.focused = false;
    }

    pub fn drawn(&mut self, handler: Option<ReadyHandler>) {
        self.handler = handler;
        self.notify();
    }

    fn notify(&self) {
        if self.focused
            && let Some(handler) = &self.handler
        {
            handler();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use super::*;

    #[test]
    fn readiness_waits_for_keyboard_enter_and_the_current_drawn_handler() {
        let calls = Rc::new(Cell::new(0));
        let count = calls.clone();
        let handler: ReadyHandler = Rc::new(move || count.set(count.get() + 1));
        let mut ready = KeyboardReady::default();

        // The IPC open used to answer here, before the compositor gave the window any keys.
        ready.drawn(Some(handler.clone()));
        assert_eq!(calls.get(), 0);
        ready.enter();
        assert_eq!(calls.get(), 1);

        ready.leave();
        ready.drawn(Some(handler.clone()));
        assert_eq!(calls.get(), 1);

        // Hidden or not yet configured: enter alone cannot promise a usable handler.
        ready.drawn(None);
        ready.enter();
        assert_eq!(calls.get(), 1);
        ready.drawn(Some(handler));
        assert_eq!(calls.get(), 2);

        // Switching Surface while already focused has no second enter event.
        let current = Rc::new(Cell::new(false));
        let called = current.clone();
        ready.drawn(Some(Rc::new(move || called.set(true))));
        assert!(current.get());
        assert_eq!(calls.get(), 2);
    }
}
