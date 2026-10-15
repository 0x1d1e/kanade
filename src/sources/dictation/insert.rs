//! Where the text of a dictation goes (ADR 0111): the clipboard, or into the focused window. Typing
//! into a window the user did not mean would put their words, maybe private, in front of someone
//! else's app or a chat, so the choice is a pure function that falls back to the clipboard, marked
//! sensitive, whenever it cannot be sure: the session is locked, the window in front is not the one
//! the dictation started in, is on the `never_in` list, is not known, or the tool is missing. It
//! reads no clock. It is asked again just before the keys are sent, but between that and the
//! compositor acting on them focus can still move: the check narrows the window, it cannot close it.

use crate::config::Insertion;

// the program that types and sends the paste chord
pub const TYPER: &str = "wtype";

// the window with the focus, as niri names it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Focus {
    pub window: u64,

    // none for a window that never set one
    pub app_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    // copying is what `dictate.insert` says
    Chosen,

    // the focused window is not known now, or was not at the start, so it cannot be the same
    FocusUnknown,

    // the session is locked, where nothing is typed: the lock screen is no place for it
    Locked,

    // another window has the focus than at the start
    FocusMoved,

    // the window in front is on `dictate.never_in`
    NeverIn,

    // `wtype` is not on the PATH
    NoTool,

    // `wtype` ran and failed; decided when it does, not by `decide`
    ToolFailed,
}

impl Reason {
    // whether the copy is marked sensitive: anything but a copy the user asked for
    pub fn sensitive(self) -> bool {
        self != Reason::Chosen
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Insert {
    Type,
    Paste,
    Copy(Reason),
}

impl Insert {
    // what the island says was done, never the text
    pub fn said(self) -> &'static str {
        match self {
            Insert::Type => "Typed",
            Insert::Paste => "Pasted",
            Insert::Copy(Reason::Chosen) => "Copied to the clipboard",
            Insert::Copy(Reason::Locked) => "Copied; the session is locked",
            Insert::Copy(Reason::FocusUnknown) => "Copied; the window in front is unknown",
            Insert::Copy(Reason::FocusMoved) => "Copied; the window changed",
            Insert::Copy(Reason::NeverIn) => "Copied; the app is in never_in",
            Insert::Copy(Reason::NoTool) => "Copied; wtype not found",
            Insert::Copy(Reason::ToolFailed) => "Copied; wtype failed",
        }
    }
}

/*
 * what to do with the text: `mode` is what the user asked for, `start` the focus when the
 * dictation started, `now` the focus now. `tool` is whether `wtype` can be run and `locked`
 * whether the session is locked
 */
pub fn decide(
    mode: Insertion,
    start: Option<&Focus>,
    now: Option<&Focus>,
    never_in: &[String],
    tool: bool,
    locked: bool,
) -> Insert {
    if mode == Insertion::Copy {
        return Insert::Copy(Reason::Chosen);
    }

    // a locked session gets the text only on the clipboard, marked sensitive, not lost: what is
    // typed there would go to the lock screen's password field or to nothing
    if locked {
        return Insert::Copy(Reason::Locked);
    }

    let (Some(start), Some(now)) = (start, now) else {
        return Insert::Copy(Reason::FocusUnknown);
    };

    if start.window != now.window {
        return Insert::Copy(Reason::FocusMoved);
    }

    if now
        .app_id
        .as_deref()
        .is_some_and(|app| never_in.iter().any(|never| same_app(never, app)))
    {
        return Insert::Copy(Reason::NeverIn);
    }

    if !tool {
        return Insert::Copy(Reason::NoTool);
    }

    match mode {
        Insertion::Type => Insert::Type,
        _ => Insert::Paste,
    }
}

// app ids compare without case and without a `.desktop` either may carry
fn same_app(listed: &str, app: &str) -> bool {
    let bare = |id: &str| id.trim().trim_end_matches(".desktop").to_lowercase();

    bare(listed) == bare(app)
}

/*
 * the text as it is typed: one line, as a line break would press Enter, which in a terminal runs
 * what was said, and a tab would move the focus in a form. Every other control character (Escape,
 * Ctrl-C, Ctrl-D, Backspace, Delete, NUL) becomes a space too, as a terminal or an editor would
 * act on it instead of showing it
 */
pub fn typed(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(window: u64, app: &str) -> Option<Focus> {
        Some(Focus {
            window,
            app_id: Some(app.to_owned()),
        })
    }

    fn pick(mode: Insertion, start: Option<Focus>, now: Option<Focus>, tool: bool) -> Insert {
        let never = [String::from("org.keepassxc.KeePassXC")];

        decide(mode, start.as_ref(), now.as_ref(), &never, tool, false)
    }

    #[test]
    fn copy_is_what_was_asked_and_needs_no_focus() {
        assert_eq!(
            pick(Insertion::Copy, None, None, false),
            Insert::Copy(Reason::Chosen)
        );
        assert!(!Reason::Chosen.sensitive());
    }

    #[test]
    fn typing_and_pasting_go_only_where_the_dictation_started() {
        let here = at(7, "kitty");

        assert_eq!(
            pick(Insertion::Type, here.clone(), here.clone(), true),
            Insert::Type
        );
        assert_eq!(
            pick(Insertion::Paste, here.clone(), here.clone(), true),
            Insert::Paste
        );
        assert_eq!(
            pick(Insertion::Type, here.clone(), at(8, "kitty"), true),
            Insert::Copy(Reason::FocusMoved)
        );
    }

    #[test]
    fn an_unknown_window_is_never_typed_into() {
        let here = at(7, "kitty");

        assert_eq!(
            pick(Insertion::Type, None, here.clone(), true),
            Insert::Copy(Reason::FocusUnknown)
        );
        assert_eq!(
            pick(Insertion::Type, here, None, true),
            Insert::Copy(Reason::FocusUnknown)
        );
    }

    #[test]
    fn a_listed_app_gets_the_clipboard_however_the_id_is_written() {
        for app in ["org.keepassxc.KeePassXC", "org.keepassxc.keepassxc.desktop"] {
            assert_eq!(
                pick(Insertion::Type, at(7, app), at(7, app), true),
                Insert::Copy(Reason::NeverIn)
            );
        }
    }

    #[test]
    fn a_missing_tool_falls_back_and_the_fallbacks_are_sensitive() {
        let here = at(7, "kitty");

        let missing = pick(Insertion::Paste, here.clone(), here, false);

        assert_eq!(missing, Insert::Copy(Reason::NoTool));

        for reason in [
            Reason::Locked,
            Reason::FocusUnknown,
            Reason::FocusMoved,
            Reason::NeverIn,
            Reason::NoTool,
            Reason::ToolFailed,
        ] {
            assert!(reason.sensitive());
            assert!(Insert::Copy(reason).said().starts_with("Copied"));
        }
    }

    #[test]
    fn typed_text_is_one_line() {
        assert_eq!(typed("one\ntwo\r\n\n  three\tfour "), "one two three four");
    }

    #[test]
    fn typed_text_carries_no_control_character() {
        // Escape, Ctrl-C, Ctrl-D, Backspace, Delete, NUL and a C1 control
        let wild = "a\u{1b}[2Jb\u{3}c\u{4}d\u{8}e\u{7f}f\u{0}g\u{85}h\u{9b}i";

        assert_eq!(typed(wild), "a [2Jb c d e f g h i");
        assert!(!typed(wild).chars().any(char::is_control));
    }

    #[test]
    fn a_locked_session_is_never_typed_into() {
        let here = at(7, "kitty");
        let locked = |mode| decide(mode, here.as_ref(), here.as_ref(), &[], true, true);

        assert_eq!(locked(Insertion::Type), Insert::Copy(Reason::Locked));
        assert_eq!(locked(Insertion::Paste), Insert::Copy(Reason::Locked));
        assert!(Reason::Locked.sensitive());

        // the copy the user asked for stays what they asked for
        assert_eq!(locked(Insertion::Copy), Insert::Copy(Reason::Chosen));
    }
}
