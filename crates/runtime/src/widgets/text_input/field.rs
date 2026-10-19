use crate::input::ime::{Batch, Context, MOST_BYTES};
use crate::{Key, Modifiers};

// what a key asked of the input beyond moving the cursor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effect {
    // the text is as it was, and the cursor may have moved
    Unchanged,

    Changed,

    // the clipboard's text goes in; the compositor holds it, so it comes later
    Paste,
}

// what a text input holds: its text, and where the cursor sits in it
#[derive(Clone, Default)]
pub struct Field {
    pub text: String,

    // counts letters, not bytes
    pub cursor: usize,

    // the other end of the selection, the cursor being the first; letters, like the cursor
    pub anchor: Option<usize>,

    // what an input method is composing, shown at the cursor and not yet in the text
    pub preedit: Option<Preedit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preedit {
    // one line, like the text
    pub text: String,

    // the bytes of `text` the method marks as its cursor, none when it hides the cursor
    pub cursor: Option<(usize, usize)>,
}

impl Field {
    pub fn apply(&mut self, key: Key, modifiers: Modifiers) -> Effect {
        // the input method owns the keys while it composes; one that gets here was not taken by it
        if self.preedit.is_some() {
            // an input method that is gone leaves its preedit behind; these two let the user out
            if matches!(key, Key::Escape | Key::Backspace) {
                self.preedit = None;

                return Effect::Changed;
            }

            return Effect::Unchanged;
        }

        let Modifiers { shift, ctrl, .. } = modifiers;

        match key {
            Key::SelectAll => self.shortcut('a'),
            Key::Character(letter) if ctrl => self.shortcut(letter),
            Key::Character(letter) => self.insert(letter.encode_utf8(&mut [0; 4])),
            Key::Space if ctrl => Effect::Unchanged,
            Key::Space => self.insert(" "),
            Key::Backspace => self.erase(ctrl, false),
            Key::Delete => self.erase(ctrl, true),
            Key::Left => self.step(ctrl, shift, false),
            Key::Right => self.step(ctrl, shift, true),
            Key::Home => self.travel(0, shift),
            Key::End => self.travel(self.length(), shift),
            _ => Effect::Unchanged,
        }
    }

    // what was pasted goes where the cursor is, over the selection
    pub fn paste(&mut self, text: &str) -> Effect {
        if self.preedit.is_some() {
            return Effect::Unchanged;
        }

        self.insert(&single_line(text))
    }

    // the selected letters' start and end, none when only the cursor sits there
    pub fn selection(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;

        (anchor != self.cursor).then(|| (anchor.min(self.cursor), anchor.max(self.cursor)))
    }

    /*
     * what an input method asked since its last `done`, in the order the protocol gives: the
     * old preedit goes, the text around the cursor is deleted, the commit goes in at the cursor,
     * then the new preedit is shown there. Every offset is the method's, in bytes, and is
     * clamped to the text and to whole letters
     */
    pub fn apply_done(&mut self, batch: &Batch) -> Effect {
        self.preedit = None;

        let mut changed = false;

        if batch.delete_before > 0 || batch.delete_after > 0 {
            changed |= self.delete_around(batch.delete_before, batch.delete_after);
        }

        if let Some(commit) = batch
            .commit
            .as_ref()
            .filter(|text| text.len() <= MOST_BYTES)
        {
            let line = single_line(commit);

            if !line.is_empty() {
                changed |= self.insert(&line) == Effect::Changed;
            }
        }

        if let Some((text, begin, end)) = &batch.preedit {
            changed |= self.compose(text, *begin, *end);
        }

        match changed {
            true => Effect::Changed,
            false => Effect::Unchanged,
        }
    }

    // removes `before` bytes before the cursor and `after` after it, less if that splits a letter
    fn delete_around(&mut self, before: u32, after: u32) -> bool {
        let cursor = self.byte_index(self.cursor);

        let start = cursor.saturating_sub(before as usize);
        let start = (start..=cursor)
            .find(|at| self.text.is_char_boundary(*at))
            .unwrap_or(cursor);

        let end = cursor.saturating_add(after as usize).min(self.text.len());
        let end = (cursor..=end)
            .rev()
            .find(|at| self.text.is_char_boundary(*at))
            .unwrap_or(cursor);

        // the deletion is around the cursor, so a selection no longer stands
        self.anchor = None;

        if start == end {
            return false;
        }

        let letters = self.text[..start].chars().count();

        self.text.replace_range(start..end, "");
        self.cursor = letters;

        true
    }

    // shows `text` at the cursor as the preedit; whether it took a selection's text away
    fn compose(&mut self, text: &str, begin: i32, end: i32) -> bool {
        if text.len() > MOST_BYTES {
            return false;
        }

        let line = single_line(text);

        if line.is_empty() {
            return false;
        }

        let marked = |at: i32| {
            let at = usize::try_from(at).ok()?;

            Some(floor_boundary(text, at))
        };

        // offsets are the method's, so they are carried over to the one-line text
        let cursor = match (marked(begin), marked(end)) {
            (Some(begin), Some(end)) => {
                let kept = |at: usize| floor_boundary(&line, single_line(&text[..at]).len());

                Some((kept(begin.min(end)), kept(begin.max(end))))
            }

            _ => None,
        };

        // typing over a selection replaces it, and so does composing
        let replaced = self.selection().is_some() && self.insert("") == Effect::Changed;

        self.preedit = Some(Preedit { text: line, cursor });

        replaced
    }

    // what an input method may be told: a bounded part of the text around the cursor
    pub fn context(&self, sensitive: bool) -> Context {
        if sensitive {
            return Context {
                sensitive,
                text: String::new(),
                cursor: 0,
                anchor: 0,
            };
        }

        let cursor = self.byte_index(self.cursor);
        let anchor = self.anchor.map_or(cursor, |anchor| self.byte_index(anchor));

        // the window of at most MOST_BYTES that has the cursor at its middle where it can
        let mut start = 0;
        let mut end = self.text.len();

        if end > MOST_BYTES {
            start = cursor.saturating_sub(MOST_BYTES / 2);
            end = (start + MOST_BYTES).min(self.text.len());
            start = end - MOST_BYTES;

            while !self.text.is_char_boundary(start) {
                start += 1;
            }

            while !self.text.is_char_boundary(end) {
                end -= 1;
            }
        }

        Context {
            sensitive,
            text: self.text[start..end].to_string(),
            cursor: cursor.clamp(start, end) - start,
            anchor: anchor.clamp(start, end) - start,
        }
    }

    // ctrl and a letter, which names the key by the letter it types without ctrl
    fn shortcut(&mut self, letter: char) -> Effect {
        match letter {
            'a' => {
                self.anchor = Some(0);
                self.cursor = self.length();

                Effect::Unchanged
            }

            'v' => Effect::Paste,

            // copy and cut would need the clipboard written, which the runtime does not do yet
            _ => Effect::Unchanged,
        }
    }

    fn insert(&mut self, text: &str) -> Effect {
        let (start, end) = self.selection().unwrap_or((self.cursor, self.cursor));

        if text.is_empty() && start == end {
            return Effect::Unchanged;
        }

        self.replace(start, end, text);

        Effect::Changed
    }

    // removes the selection, else the letter or word on the cursor's left or right
    fn erase(&mut self, word: bool, forward: bool) -> Effect {
        let cursor = self.cursor.min(self.length());

        let (start, end) = match (self.selection(), forward, word) {
            (Some(selection), _, _) => selection,
            (None, false, false) => (cursor.saturating_sub(1), cursor),
            (None, true, false) => (cursor, cursor + 1),
            (None, false, true) => (word_left(&self.chars(), cursor), cursor),
            (None, true, true) => (cursor, word_right(&self.chars(), cursor)),
        };

        let end = end.min(self.length());

        if start == end {
            return Effect::Unchanged;
        }

        self.replace(start, end, "");

        Effect::Changed
    }

    // a selection a plain arrow ends goes to its edge on that side, as in every text field
    fn step(&mut self, word: bool, extend: bool, forward: bool) -> Effect {
        if let (Some((start, end)), false, false) = (self.selection(), word, extend) {
            return self.travel(if forward { end } else { start }, false);
        }

        let to = match (forward, word) {
            (false, false) => self.cursor.saturating_sub(1),
            (true, false) => self.cursor + 1,
            (false, true) => word_left(&self.chars(), self.cursor),
            (true, true) => word_right(&self.chars(), self.cursor),
        };

        self.travel(to, extend)
    }

    // extending keeps the selection's other end where the cursor was
    fn travel(&mut self, cursor: usize, extend: bool) -> Effect {
        self.anchor = match extend {
            true => Some(self.anchor.unwrap_or(self.cursor)),
            false => None,
        };

        self.cursor = cursor.min(self.length());

        Effect::Unchanged
    }

    // puts `with` in the place of the letters from `start` to `end`, the cursor after it
    fn replace(&mut self, start: usize, end: usize, with: &str) {
        let from = self.byte_index(start);
        let to = self.byte_index(end);

        self.text.replace_range(from..to, with);

        self.cursor = start + with.chars().count();
        self.anchor = None;
    }

    fn length(&self) -> usize {
        self.text.chars().count()
    }

    fn chars(&self) -> Vec<char> {
        self.text.chars().collect()
    }

    // a String is indexed by bytes, and one letter can take several
    pub fn byte_index(&self, index: usize) -> usize {
        match self.text.char_indices().nth(index) {
            Some((byte, _)) => byte,
            None => self.text.len(),
        }
    }
}

// the largest index up to `at` that does not split a letter
fn floor_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());

    while !text.is_char_boundary(at) {
        at -= 1;
    }

    at
}

#[derive(PartialEq)]
enum Class {
    Space,
    Word,
    Other,
}

impl Class {
    fn of(letter: char) -> Self {
        if letter.is_whitespace() {
            Self::Space
        } else if letter.is_alphanumeric() || letter == '_' {
            Self::Word
        } else {
            Self::Other
        }
    }
}

// the start of the word before `from`: spaces are skipped, then a run of letters or of symbols
fn word_left(chars: &[char], from: usize) -> usize {
    let mut at = from.min(chars.len());

    while at > 0 && Class::of(chars[at - 1]) == Class::Space {
        at -= 1;
    }

    let Some(class) = at.checked_sub(1).map(|before| Class::of(chars[before])) else {
        return 0;
    };

    while at > 0 && Class::of(chars[at - 1]) == class {
        at -= 1;
    }

    at
}

// the end of the word after `from`, found the same way
fn word_right(chars: &[char], from: usize) -> usize {
    let mut at = from.min(chars.len());

    while at < chars.len() && Class::of(chars[at]) == Class::Space {
        at += 1;
    }

    let Some(class) = chars.get(at).map(|letter| Class::of(*letter)) else {
        return chars.len();
    };

    while at < chars.len() && Class::of(chars[at]) == class {
        at += 1;
    }

    at
}

// an input is one line: line breaks and tabs become a space, other control characters go
fn single_line(text: &str) -> String {
    let mut line = String::with_capacity(text.len());

    let mut letters = text.chars().peekable();

    while let Some(letter) = letters.next() {
        match letter {
            // a Windows line break is one space, with its \n
            '\r' if letters.peek() == Some(&'\n') => {}
            '\r' | '\n' | '\t' => line.push(' '),
            letter if letter.is_control() => {}
            letter => line.push(letter),
        }
    }

    line
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTRL: Modifiers = Modifiers {
        shift: false,
        ctrl: true,
        alt: false,
        logo: false,
    };

    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ctrl: false,
        alt: false,
        logo: false,
    };

    fn field(text: &str, cursor: usize) -> Field {
        Field {
            text: String::from(text),
            cursor,
            anchor: None,
            preedit: None,
        }
    }

    #[test]
    fn word_jumps_skip_spaces_then_one_run() {
        let chars: Vec<char> = "foo  bar_1, baz".chars().collect();

        assert_eq!(word_left(&chars, 15), 12);
        assert_eq!(word_left(&chars, 12), 10);
        assert_eq!(word_left(&chars, 10), 5);
        assert_eq!(word_left(&chars, 5), 0);
        assert_eq!(word_left(&chars, 0), 0);

        assert_eq!(word_right(&chars, 0), 3);
        assert_eq!(word_right(&chars, 3), 10);
        assert_eq!(word_right(&chars, 10), 11);
        assert_eq!(word_right(&chars, 11), 15);
        assert_eq!(word_right(&chars, 15), 15);
    }

    #[test]
    fn word_delete_takes_the_word_before_or_after() {
        let mut backward = field("one two three", 7);

        assert_eq!(backward.apply(Key::Backspace, CTRL), Effect::Changed);
        assert_eq!(backward.text, "one  three");
        assert_eq!(backward.cursor, 4);

        let mut forward = field("one two three", 3);

        assert_eq!(forward.apply(Key::Delete, CTRL), Effect::Changed);
        assert_eq!(forward.text, "one three");
        assert_eq!(forward.cursor, 3);

        let mut start = field("one", 0);

        assert_eq!(start.apply(Key::Backspace, CTRL), Effect::Unchanged);
        assert_eq!(
            start.apply(Key::Backspace, Modifiers::default()),
            Effect::Unchanged
        );
    }

    #[test]
    fn delete_removes_the_letter_after_the_cursor_even_when_it_is_wide() {
        let mut field = field("aéb", 1);

        assert_eq!(
            field.apply(Key::Delete, Modifiers::default()),
            Effect::Changed
        );
        assert_eq!(field.text, "ab");
        assert_eq!(field.cursor, 1);

        let mut end = self::field("ab", 2);

        assert_eq!(
            end.apply(Key::Delete, Modifiers::default()),
            Effect::Unchanged
        );
    }

    #[test]
    fn select_all_then_typing_replaces_the_text() {
        let mut field = field("hello", 2);

        assert_eq!(field.apply(Key::Character('a'), CTRL), Effect::Unchanged);
        assert_eq!(field.selection(), Some((0, 5)));

        assert_eq!(
            field.apply(Key::Character('x'), Modifiers::default()),
            Effect::Changed
        );
        assert_eq!(field.text, "x");
        assert_eq!(field.cursor, 1);
        assert_eq!(field.selection(), None);
    }

    #[test]
    fn select_all_then_backspace_clears_and_an_arrow_ends_it_at_an_edge() {
        let mut cleared = field("hello", 5);

        cleared.apply(Key::Character('a'), CTRL);

        assert_eq!(
            cleared.apply(Key::Backspace, Modifiers::default()),
            Effect::Changed
        );
        assert_eq!(cleared.text, "");

        let mut left = field("hello", 5);

        left.apply(Key::Character('a'), CTRL);
        left.apply(Key::Left, Modifiers::default());

        assert_eq!((left.cursor, left.selection()), (0, None));

        let mut right = field("hello", 2);

        right.apply(Key::Character('a'), CTRL);
        right.apply(Key::Right, Modifiers::default());

        assert_eq!((right.cursor, right.selection()), (5, None));
    }

    #[test]
    fn shift_extends_the_selection_from_where_it_began() {
        let mut field = field("one two", 3);

        field.apply(Key::Right, SHIFT);
        field.apply(Key::Right, SHIFT);

        assert_eq!(field.selection(), Some((3, 5)));

        field.apply(Key::Left, SHIFT);
        field.apply(Key::Left, SHIFT);
        field.apply(Key::Left, SHIFT);

        assert_eq!(field.selection(), Some((2, 3)));

        field.apply(Key::End, SHIFT);

        assert_eq!(field.selection(), Some((3, 7)));
    }

    #[test]
    fn ctrl_arrows_jump_a_word() {
        let mut field = field("one two", 0);

        field.apply(Key::Right, CTRL);

        assert_eq!(field.cursor, 3);

        field.apply(Key::Right, CTRL);

        assert_eq!(field.cursor, 7);

        field.apply(Key::Left, CTRL);

        assert_eq!(field.cursor, 4);
    }

    #[test]
    fn only_ctrl_v_asks_for_the_clipboard_and_other_shortcuts_type_nothing() {
        let mut field = field("a", 1);

        assert_eq!(field.apply(Key::Character('v'), CTRL), Effect::Paste);
        assert_eq!(field.apply(Key::Character('c'), CTRL), Effect::Unchanged);
        assert_eq!(field.apply(Key::Character('x'), CTRL), Effect::Unchanged);
        assert_eq!(field.apply(Key::Space, CTRL), Effect::Unchanged);
        assert_eq!(field.text, "a");
    }

    #[test]
    fn a_paste_is_one_line_and_replaces_the_selection() {
        assert_eq!(single_line("a\r\nb\tc\rd\u{7}e\u{1b}[0m"), "a b c de[0m");

        let mut field = field("hello", 5);

        field.apply(Key::Character('a'), CTRL);

        assert_eq!(field.paste("x\ny"), Effect::Changed);
        assert_eq!(field.text, "x y");
        assert_eq!(field.cursor, 3);

        assert_eq!(field.paste("\u{7}"), Effect::Unchanged);
    }

    fn done(
        preedit: Option<(&str, i32, i32)>,
        commit: Option<&str>,
        before: u32,
        after: u32,
    ) -> Batch {
        Batch {
            preedit: preedit.map(|(text, begin, end)| (String::from(text), begin, end)),
            commit: commit.map(String::from),
            delete_before: before,
            delete_after: after,
        }
    }

    #[test]
    fn a_done_applies_delete_then_commit_then_preedit_over_the_old_preedit() {
        let mut field = field("ab cd", 2);

        field.preedit = Some(Preedit {
            text: String::from("old"),
            cursor: None,
        });

        // "b" goes, "XY" is typed where it was, and "z" is composed after it
        let batch = done(Some(("z", 1, 1)), Some("XY"), 1, 0);

        assert_eq!(field.apply_done(&batch), Effect::Changed);
        assert_eq!(field.text, "aXY cd");
        assert_eq!(field.cursor, 3);

        let preedit = field.preedit.as_ref().expect("a preedit");

        assert_eq!(preedit.text, "z");
        assert_eq!(preedit.cursor, Some((1, 1)));

        // a batch without a preedit ends the composition, the text stays
        assert_eq!(field.apply_done(&Batch::default()), Effect::Unchanged);
        assert_eq!(field.preedit, None);
        assert_eq!(field.text, "aXY cd");
    }

    #[test]
    fn composing_alone_changes_no_text_and_a_selection_is_replaced() {
        let mut composing = field("ab", 1);

        assert_eq!(
            composing.apply_done(&done(Some(("ni", 0, 2)), None, 0, 0)),
            Effect::Unchanged
        );
        assert_eq!(composing.text, "ab");

        let mut selected = field("hello", 5);

        selected.apply(Key::Character('a'), CTRL);

        assert_eq!(
            selected.apply_done(&done(Some(("x", -1, -1)), None, 0, 0)),
            Effect::Changed
        );
        assert_eq!(selected.text, "");
        assert_eq!(selected.selection(), None);
        assert_eq!(selected.preedit.expect("a preedit").cursor, None);
    }

    #[test]
    fn offsets_from_the_method_are_clamped_to_the_text_and_to_whole_letters() {
        // "é" is two bytes: a deletion that would split it takes less, never a half
        let mut field = field("aébc", 2);

        assert_eq!(field.apply_done(&done(None, None, 1, 0)), Effect::Unchanged);
        assert_eq!(field.text, "aébc");

        assert_eq!(field.apply_done(&done(None, None, 99, 99)), Effect::Changed);
        assert_eq!(field.text, "");
        assert_eq!(field.cursor, 0);

        // preedit offsets past its end or inside a letter land on letter edges
        let mut field = self::field("", 0);

        field.apply_done(&done(Some(("éa", 1, 99)), None, 0, 0));

        let preedit = field.preedit.expect("a preedit");

        assert_eq!(preedit.cursor, Some((0, 3)));
    }

    #[test]
    fn a_commit_is_one_line_and_an_oversized_one_is_dropped() {
        let mut field = field("", 0);

        assert_eq!(
            field.apply_done(&done(None, Some("a\nb"), 0, 0)),
            Effect::Changed
        );
        assert_eq!(field.text, "a b");

        let long = "x".repeat(MOST_BYTES + 1);

        assert_eq!(
            field.apply_done(&done(None, Some(&long), 0, 0)),
            Effect::Unchanged
        );
        assert_eq!(field.text, "a b");
    }

    #[test]
    fn keys_wait_while_the_method_composes() {
        let mut field = field("ab", 2);

        field.apply_done(&done(Some(("k", 1, 1)), None, 0, 0));

        assert_eq!(
            field.apply(Key::Character('x'), Modifiers::default()),
            Effect::Unchanged
        );
        assert_eq!(field.paste("y"), Effect::Unchanged);
        assert_eq!(field.text, "ab");
    }

    #[test]
    fn the_context_is_bounded_on_letter_edges_and_empty_for_a_password() {
        let text = "é".repeat(MOST_BYTES);

        let mut long = field(&text, MOST_BYTES / 2);

        long.anchor = Some(0);

        let context = long.context(false);

        assert!(context.text.len() <= MOST_BYTES);
        assert!(context.cursor <= context.text.len());
        assert!(context.text.is_char_boundary(context.cursor));
        assert!(context.text.is_char_boundary(context.anchor));
        assert!(context.anchor <= context.cursor);

        let secret = field("hunter2", 7).context(true);

        assert!(secret.sensitive);
        assert_eq!(secret.text, "");
    }
}
