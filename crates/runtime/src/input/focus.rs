use std::cell::RefCell;

use std::rc::Rc;

use crate::{Key, Modifiers};

use super::ime::Editor;

// a window's on_key is not given modifiers, a text input needs them for its shortcuts
pub type FocusHandler = Rc<dyn Fn(Key, Modifiers)>;

// the text input that typed keys go to, and what it does with them
struct Focus {
    id: &'static str,
    on_key: FocusHandler,

    // how the input method reaches it
    editor: Rc<dyn Editor>,
}

thread_local! {
    // only one text input takes keys at a time
    static FOCUS: RefCell<Option<Focus>> = const { RefCell::new(None) };

    // the input that takes the keyboard when it is next drawn, for a key that moves to it
    static WANTED: RefCell<Option<&'static str>> = const { RefCell::new(None) };
}

pub fn want(id: &'static str) {
    WANTED.set(Some(id));
}

// whether `id` was wanted, which it then no longer is
pub fn wanted(id: &'static str) -> bool {
    WANTED.with_borrow_mut(|wanted| wanted.take_if(|wanted| *wanted == id).is_some())
}

pub fn set(id: &'static str, on_key: FocusHandler, editor: Rc<dyn Editor>) {
    FOCUS.set(Some(Focus { id, on_key, editor }));
}

// the focused text input and how to edit it for the input method
pub fn editor() -> Option<(&'static str, Rc<dyn Editor>)> {
    FOCUS.with_borrow(|focus| {
        focus
            .as_ref()
            .map(|focus| (focus.id, Rc::clone(&focus.editor)))
    })
}

pub fn clear() {
    FOCUS.set(None);
}

pub fn has(id: &'static str) -> bool {
    FOCUS.with_borrow(|focus| matches!(focus, Some(focus) if focus.id == id))
}

// returns whether a text input took the key
pub fn send(key: Key, modifiers: Modifiers) -> bool {
    // a text input has no use for these, so they go on to the window's on_key
    if matches!(
        key,
        Key::Up | Key::Down | Key::PageUp | Key::PageDown | Key::Tab | Key::BackTab | Key::Other
    ) {
        return false;
    }

    // copied out first, so the handler is free to change the focus itself
    let on_key = FOCUS.with_borrow(|focus| focus.as_ref().map(|focus| focus.on_key.clone()));

    let Some(on_key) = on_key else {
        return false;
    };

    on_key(key, modifiers);

    // escape leaves the input and still reaches the window, which may want to close
    key != Key::Escape
}
