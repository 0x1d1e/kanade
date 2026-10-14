use std::cell::RefCell;

// runs with the clipboard's text, if there is any, once the event loop has read it
pub type Paste = Box<dyn FnOnce(String)>;

thread_local! {
    // set by a key handler, which cannot reach the compositor; the event loop takes it right after
    static REQUEST: RefCell<Option<Paste>> = const { RefCell::new(None) };
}

pub fn request(done: Paste) {
    REQUEST.set(Some(done));
}

pub fn take() -> Option<Paste> {
    REQUEST.take()
}
