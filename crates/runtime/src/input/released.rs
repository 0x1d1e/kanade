use std::sync::OnceLock;

static HANDLER: OnceLock<fn()> = OnceLock::new();

pub fn set(handler: fn()) {
    // set once, before the loop runs; a second app in the process keeps the first
    let _ = HANDLER.set(handler);
}

pub fn send() {
    if let Some(handler) = HANDLER.get() {
        handler();
    }
}
