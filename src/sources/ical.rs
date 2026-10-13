//! The iCalendar text Kanade writes, for the files it keeps for the `calendar` Module: Google's
//! mirror (ADR 0016) and the events added in the Calendar Surface (ADR 0045).

// a content line's longest, in octets, before it folds (RFC 5545 3.1)
pub const LINE: usize = 75;

// a TEXT value, its backslashes, separators and line breaks escaped
pub fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());

    for char in text.chars() {
        match char {
            '\\' => escaped.push_str("\\\\"),
            ';' => escaped.push_str("\\;"),
            ',' => escaped.push_str("\\,"),
            '\n' => escaped.push_str("\\n"),
            '\r' => {}
            char if char.is_control() => {}
            char => escaped.push(char),
        }
    }

    escaped
}

// one content line, folded into lines of at most `LINE` octets, never inside a character
pub fn line(text: &mut String, content: &str) {
    let mut room = LINE;
    let mut used = 0;

    for char in content.chars() {
        if used + char.len_utf8() > room {
            text.push_str("\r\n ");
            used = 0;
            // the space that starts a folded line is one of its octets
            room = LINE - 1;
        }

        text.push(char);
        used += char.len_utf8();
    }

    text.push_str("\r\n");
}
