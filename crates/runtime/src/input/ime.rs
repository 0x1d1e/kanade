use crate::graphics::Area;

/*
 * the compositor's input method (text-input-v3) and a text input, which cannot reach each other:
 * the event loop applies what the method sent, and asks what the focused input holds to tell it
 */

// what a text input may be told is not a letter more than this, which is the protocol's limit
pub const MOST_BYTES: usize = 4000;

// one `done` of the input method: what it asked since the last, to be applied together
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Batch {
    // text being composed, with where the cursor sits in it (bytes; negative hides the cursor)
    pub preedit: Option<(String, i32, i32)>,

    pub commit: Option<String>,

    // bytes around the cursor to delete
    pub delete_before: u32,
    pub delete_after: u32,
}

// what the input method may know of the focused input
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    // a password or the like: it is told its purpose and nothing of what is typed
    pub sensitive: bool,

    // a bounded part of the text around the cursor, empty when sensitive
    pub text: String,

    // bytes of `text`
    pub cursor: usize,
    pub anchor: usize,
}

// the focused text input, as the event loop reaches it
pub trait Editor {
    fn context(&self) -> Context;

    // applies a batch; whether the text changed
    fn apply(&self, batch: &Batch) -> bool;
}

// where the caret is, for the input method's popup to sit under: in the input's own space
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Caret {
    pub id: &'static str,
    pub area: Area,
}
