//! Just enough JSON for niri's event lines and pw-dump's prints. Numbers stay f64, which holds
//! their ids exactly.

use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(Vec<(String, Json)>),
}

impl Json {
    // none for anything but exactly one value, whitespace around it aside
    pub fn parse(text: &str) -> Option<Json> {
        let mut chars = text.chars().peekable();
        let value = value(&mut chars)?;

        skip_space(&mut chars);

        chars.peek().is_none().then_some(value)
    }

    // the field `key` of an object, none for a missing key or anything but an object
    pub fn get(&self, key: &str) -> Option<&Json> {
        let Json::Object(fields) = self else {
            return None;
        };

        fields
            .iter()
            .find_map(|(name, value)| (name == key).then_some(value))
    }

    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Json::Bool(value) => Some(value),
            _ => None,
        }
    }

    // only whole numbers that fit, so a mangled id is none rather than a different id
    pub fn as_u64(&self) -> Option<u64> {
        match *self {
            Json::Number(value)
                if value >= 0.0 && value.fract() == 0.0 && value < 2f64.powi(53) =>
            {
                Some(value as u64)
            }
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match *self {
            Json::Number(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Json::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Json]> {
        match self {
            Json::Array(values) => Some(values),
            _ => None,
        }
    }
}

type Input<'a> = Peekable<Chars<'a>>;

fn value(chars: &mut Input) -> Option<Json> {
    skip_space(chars);

    match *chars.peek()? {
        '{' => object(chars),
        '[' => array(chars),
        '"' => string(chars).map(Json::String),
        't' => word(chars, "true", Json::Bool(true)),
        'f' => word(chars, "false", Json::Bool(false)),
        'n' => word(chars, "null", Json::Null),
        _ => number(chars),
    }
}

fn object(chars: &mut Input) -> Option<Json> {
    chars.next();

    let mut fields = Vec::new();

    skip_space(chars);

    if chars.next_if_eq(&'}').is_some() {
        return Some(Json::Object(fields));
    }

    loop {
        skip_space(chars);

        let key = string(chars)?;

        skip_space(chars);
        chars.next_if_eq(&':')?;

        fields.push((key, value(chars)?));

        skip_space(chars);

        match chars.next()? {
            ',' => {}
            '}' => return Some(Json::Object(fields)),
            _ => return None,
        }
    }
}

fn array(chars: &mut Input) -> Option<Json> {
    chars.next();

    let mut values = Vec::new();

    skip_space(chars);

    if chars.next_if_eq(&']').is_some() {
        return Some(Json::Array(values));
    }

    loop {
        values.push(value(chars)?);

        skip_space(chars);

        match chars.next()? {
            ',' => {}
            ']' => return Some(Json::Array(values)),
            _ => return None,
        }
    }
}

// `text` as a JSON string, quoted, for a tool that takes JSON
pub fn quote(text: &str) -> String {
    let mut quoted = String::from('"');

    for char in text.chars() {
        match char {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            control if control < ' ' => quoted.push_str(&format!("\\u{:04x}", u32::from(control))),
            other => quoted.push(other),
        }
    }

    quoted.push('"');
    quoted
}

fn string(chars: &mut Input) -> Option<String> {
    chars.next_if_eq(&'"')?;

    let mut text = String::new();

    loop {
        match chars.next()? {
            '"' => return Some(text),
            '\\' => text.push(escape(chars)?),
            control if control < ' ' => return None,
            other => text.push(other),
        }
    }
}

fn escape(chars: &mut Input) -> Option<char> {
    Some(match chars.next()? {
        '"' => '"',
        '\\' => '\\',
        '/' => '/',
        'b' => '\u{8}',
        'f' => '\u{c}',
        'n' => '\n',
        'r' => '\r',
        't' => '\t',
        'u' => {
            let unit = hex(chars)?;

            // outside the basic plane a character is a surrogate pair, two escapes long
            if (0xD800..0xDC00).contains(&unit) {
                chars.next_if_eq(&'\\')?;
                chars.next_if_eq(&'u')?;

                let low = hex(chars)?;

                if !(0xDC00..0xE000).contains(&low) {
                    return None;
                }

                char::from_u32(0x10000 + ((unit - 0xD800) << 10) + (low - 0xDC00))?
            } else {
                char::from_u32(unit)?
            }
        }
        _ => return None,
    })
}

fn hex(chars: &mut Input) -> Option<u32> {
    (0..4).try_fold(0, |unit, _| Some(unit * 16 + chars.next()?.to_digit(16)?))
}

// JSON's grammar, narrower than Rust's float parsing: no leading zeros, no bare dots, no `+`
fn number(chars: &mut Input) -> Option<Json> {
    let mut text = String::new();

    text.extend(chars.next_if_eq(&'-'));

    if let Some(zero) = chars.next_if_eq(&'0') {
        text.push(zero);
    } else if !digits(chars, &mut text) {
        return None;
    }

    if let Some(dot) = chars.next_if_eq(&'.') {
        text.push(dot);

        if !digits(chars, &mut text) {
            return None;
        }
    }

    if let Some(e) = chars.next_if(|&c| c == 'e' || c == 'E') {
        text.push(e);
        text.extend(chars.next_if(|&c| c == '+' || c == '-'));

        if !digits(chars, &mut text) {
            return None;
        }
    }

    text.parse().ok().map(Json::Number)
}

// false when there was not even one
fn digits(chars: &mut Input, text: &mut String) -> bool {
    let before = text.len();

    while let Some(digit) = chars.next_if(char::is_ascii_digit) {
        text.push(digit);
    }

    text.len() > before
}

fn word(chars: &mut Input, word: &str, value: Json) -> Option<Json> {
    for expected in word.chars() {
        chars.next_if_eq(&expected)?;
    }

    Some(value)
}

fn skip_space(chars: &mut Input) {
    while chars
        .next_if(|c| matches!(c, ' ' | '\t' | '\n' | '\r'))
        .is_some()
    {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quoted_string_parses_back() {
        let text = "a \"node\" \\ with\ttabs";

        assert_eq!(Json::parse(&quote(text)), Some(Json::String(text.into())));
    }

    #[test]
    fn reads_every_kind_of_value() {
        let json =
            Json::parse(r#" {"a": [1, -2.5e3, true, false, null], "b": {}, "c": [], "d": "x"} "#);

        assert_eq!(
            json,
            Some(Json::Object(vec![
                (
                    "a".into(),
                    Json::Array(vec![
                        Json::Number(1.0),
                        Json::Number(-2500.0),
                        Json::Bool(true),
                        Json::Bool(false),
                        Json::Null,
                    ])
                ),
                ("b".into(), Json::Object(vec![])),
                ("c".into(), Json::Array(vec![])),
                ("d".into(), Json::String("x".into())),
            ]))
        );
    }

    #[test]
    fn decodes_escapes_and_non_ascii() {
        let json = Json::parse(r#""a\"\\\/\b\f\n\r\té🎵 — 日本""#);

        assert_eq!(
            json.as_ref().and_then(Json::as_str),
            Some("a\"\\/\u{8}\u{c}\n\r\té🎵 — 日本")
        );
    }

    #[test]
    fn refuses_what_is_not_json() {
        for text in [
            "",
            "{",
            r#"{"a" 1}"#,
            r#"{"a": 1,}"#,
            "[1 2]",
            "[1,]",
            "tru",
            "nul",
            "01",
            "1.",
            ".5",
            "+1",
            "1.e3",
            r#""\x""#,
            r#""\ud83c""#,
            r#""\ud83cx""#,
            r#""\ud83c\u0041""#,
            "\"a\nb\"",
            "{} {}",
            "inf",
        ] {
            assert_eq!(Json::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn reads_fields_and_whole_numbers() {
        let json = Json::parse(r#"{"id": 7, "f": 1.5, "n": -1, "on": true, "list": [0]}"#).unwrap();

        assert_eq!(json.get("id").and_then(Json::as_u64), Some(7));
        assert_eq!(json.get("f").and_then(Json::as_u64), None);
        assert_eq!(json.get("n").and_then(Json::as_u64), None);
        assert_eq!(Json::Number(1e20).as_u64(), None);
        assert_eq!(json.get("on").and_then(Json::as_bool), Some(true));
        assert_eq!(
            json.get("list").and_then(Json::as_array).map(<[_]>::len),
            Some(1)
        );
        assert_eq!(json.get("missing"), None);
        assert_eq!(Json::Null.get("id"), None);
    }
}
