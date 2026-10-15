//! The Gemini API as dictation speaks it: the request's body, the answer's transcript, and the
//! errors an answer or a failed call becomes. Pure, so the shapes below can be changed and tested
//! without a key. None of it holds the key or puts it in a message.
//!
//! UNVERIFIED (ADR 0111): no live key has been used against any of this. Every shape here is
//! written from the documented Interactions API and may need adjusting: that the model takes
//! inline audio, where `store` goes, and the field that holds the text of an answer.

use std::fmt;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

pub const ENDPOINT: &str = "https://generativelanguage.googleapis.com/v1beta/interactions";
pub const MODEL: &str = "gemini-3.5-transcribe";

// longest server message passed on, in characters
const MESSAGE: usize = 200;

/*
 * the body of the one request: `wav` inline as the only input, and `store` false so the
 * interaction is not kept on Google's side. This is the one place the request's shape lives.
 * UNVERIFIED: the model's acceptance of inline audio here, and `store` being a top-level field.
 */
pub fn request_body(wav: &[u8]) -> Value {
    json!({
        "model": MODEL,
        "input": [{
            "type": "audio",
            "data": STANDARD.encode(wav),
            "mime_type": "audio/wav",
        }],
        "store": false,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    // 401 or 403: the key is wrong, revoked, or restricted away from this API
    Key,
    // 429
    Limit,
    // 5xx
    Server(u16),
    // any other refusal, with the server's words (the key cut out of them)
    Refused(u16, String),
    // an answer that holds no text, or is not the shape expected
    Unreadable,
    // nothing was said, as far as the model could tell
    Silence,
    // no answer at all: DNS, TLS, timeout
    Offline(String),
    // the consent was withdrawn before the audio was sent, so none was
    Withdrawn,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Key => f.write_str("Google refused the API key"),
            Error::Limit => f.write_str("the Gemini API's rate limit is reached; try again later"),
            Error::Server(status) => write!(f, "the Gemini API failed ({status})"),
            Error::Refused(status, why) if why.is_empty() => {
                write!(f, "the Gemini API refused the request ({status})")
            }
            Error::Refused(status, why) => {
                write!(f, "the Gemini API refused the request ({status}): {why}")
            }
            Error::Unreadable => f.write_str("the Gemini API's answer held no transcript"),
            Error::Silence => f.write_str("no speech was heard"),
            Error::Offline(why) => write!(f, "cannot reach the Gemini API: {why}"),
            Error::Withdrawn => f.write_str("the consent was withdrawn; nothing was sent"),
        }
    }
}

/*
 * what a non-200 answer is. The fixed errors carry no server text; `Refused` carries the server's
 * `error.message` with `key` cut out, limited to a line
 */
pub fn failure(status: u16, body: &str, key: &str) -> Error {
    match status {
        401 | 403 => Error::Key,
        429 => Error::Limit,
        500..=599 => Error::Server(status),
        _ => Error::Refused(status, message(body, key)),
    }
}

// `text` with the key cut out of it
fn redact(text: &str, key: &str) -> String {
    if key.is_empty() {
        text.to_owned()
    } else {
        text.replace(key, "[key]")
    }
}

fn message(body: &str, key: &str) -> String {
    let said = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|body| body.pointer("/error/message")?.as_str().map(str::to_owned))
        .unwrap_or_default();

    redact(&said, key)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MESSAGE)
        .collect()
}

// the most characters of the answer's `error.status` kept
const CODE: usize = 32;

// the answer's `error.status` ("UNAVAILABLE", "INVALID_ARGUMENT"), with the key cut out and only
// plain characters kept; empty when it has none
fn code(body: &str, key: &str) -> String {
    let said = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|body| body.pointer("/error/status")?.as_str().map(str::to_owned))
        .unwrap_or_default();

    redact(&said, key)
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(CODE)
        .collect()
}

/*
 * the log line for a non-200 answer: its status and Google's own `error.status` and `error.message`,
 * the message with the key cut out, on one line and capped. Never a header, the request or the audio
 */
pub fn answered(status: u16, body: &str, key: &str) -> String {
    let (code, message) = (code(body, key), message(body, key));
    let said: Vec<&str> = [code.as_str(), message.as_str()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect();

    match said.is_empty() {
        true => format!("Gemini answered {status}"),
        false => format!("Gemini answered {status}: {}", said.join(" ")),
    }
}

/*
 * the transcript of a 200 answer, trimmed. UNVERIFIED shape: the text parts of `outputs`
 * (`{"type": "text", "text": ...}`), else of the `model_output` steps' `content` in `steps`, joined.
 * An answer that is JSON of the right kind but holds no such array, step or text is no speech
 * heard (`Silence`); one that is not JSON, not an object, or has a field of the wrong type is
 * `Unreadable`.
 */
pub fn transcript(body: &str) -> Result<String, Error> {
    let body: Value = serde_json::from_str(body).map_err(|_| Error::Unreadable)?;
    let body = body.as_object().ok_or(Error::Unreadable)?;
    let mut parts = Vec::new();

    if let Some(outputs) = body.get("outputs") {
        parts.extend(texts(outputs.as_array().ok_or(Error::Unreadable)?)?);
    } else if let Some(steps) = body.get("steps") {
        for step in steps.as_array().ok_or(Error::Unreadable)? {
            if step["type"] != "model_output" {
                continue;
            }

            if let Some(content) = step.get("content") {
                parts.extend(texts(content.as_array().ok_or(Error::Unreadable)?)?);
            }
        }
    }

    let text = parts.join(" ");
    let text = text.trim();

    if text.is_empty() {
        Err(Error::Silence)
    } else {
        Ok(text.to_owned())
    }
}

// the text of each text part; a text part whose text is not a string is not the shape
fn texts(parts: &[Value]) -> Result<Vec<&str>, Error> {
    parts
        .iter()
        .filter(|part| part["type"] == "text")
        .map(|part| part["text"].as_str().ok_or(Error::Unreadable))
        .collect()
}

// the most characters of a name from the server kept in the diagnostic
const NAME: usize = 32;

// a name the server sent, for the log: a few plain characters, so it cannot fill or fake a line
fn name(name: &str) -> String {
    name.chars()
        .take(NAME)
        .map(
            |c| match c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                true => c,
                false => '?',
            },
        )
        .collect()
}

/*
 * what the log says of an answer that was not read as a transcript: the status, the top-level keys
 * and the `type` of each step, all names of the format. Never a value, so no text of the user's,
 * the request or a header, and not the key
 */
pub fn shape(status: u16, body: &str) -> String {
    let Ok(Value::Object(body)) = serde_json::from_str::<Value>(body) else {
        return format!("status {status}, not a JSON object");
    };

    let keys: Vec<String> = body.keys().map(|key| name(key)).collect();
    let mut said = format!("status {status}, keys [{}]", keys.join(", "));

    if let Some(steps) = body.get("steps").and_then(Value::as_array) {
        let types: Vec<String> = steps
            .iter()
            .map(|step| step["type"].as_str().map_or(String::from("-"), name))
            .collect();

        said.push_str(&format!(", step types [{}]", types.join(", ")));
    }

    said
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_body_carries_the_audio_inline_and_stores_nothing() {
        let body = request_body(b"RIFFabc");

        assert_eq!(body["model"], "gemini-3.5-transcribe");
        assert_eq!(body["store"], false);
        assert_eq!(body["input"][0]["type"], "audio");
        assert_eq!(body["input"][0]["mime_type"], "audio/wav");
        assert_eq!(body["input"][0]["data"], "UklGRmFiYw==");
        assert_eq!(body["input"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn a_transcript_is_the_text_of_the_answer() {
        let answer = r#"{"id": "i1", "status": "completed",
            "outputs": [{"type": "text", "text": "  Hello there. "}]}"#;
        assert_eq!(transcript(answer).unwrap(), "Hello there.");

        let steps = r#"{"steps": [{"type": "user_input", "content": [{"type": "text", "text": "x"}]},
            {"type": "model_output", "content": [{"type": "text", "text": "Hi"},
            {"type": "text", "text": "again"}]}]}"#;
        assert_eq!(transcript(steps).unwrap(), "Hi again");
    }

    #[test]
    fn an_answer_without_text_is_silence() {
        for body in [
            r#"{"outputs": []}"#,
            r#"{"outputs": [{"type": "text", "text": " "}]}"#,
            // JSON of the right kind with no outputs or steps at all
            r#"{"id": "i1", "status": "completed"}"#,
            r#"{}"#,
            // steps with no model_output, one without content, or without a text part
            r#"{"steps": []}"#,
            r#"{"steps": [{"type": "user_input", "content": [{"type": "text", "text": "x"}]}]}"#,
            r#"{"steps": [{"type": "model_output"}]}"#,
            r#"{"steps": [{"type": "model_output", "content": [{"type": "thought", "summary": "s"}]}]}"#,
            r#"{"steps": [{"type": "model_output", "content": [{"type": "text", "text": "  "}]}]}"#,
        ] {
            assert_eq!(transcript(body), Err(Error::Silence), "{body}");
        }
    }

    #[test]
    fn an_answer_that_is_not_the_shape_is_unreadable() {
        for body in [
            "<html>",
            "",
            "[]",
            r#""text""#,
            r#"{"outputs": "text"}"#,
            r#"{"outputs": [{"type": "text", "text": 5}]}"#,
            r#"{"steps": {"type": "model_output"}}"#,
            r#"{"steps": [{"type": "model_output", "content": "hi"}]}"#,
        ] {
            assert_eq!(transcript(body), Err(Error::Unreadable), "{body}");
        }
    }

    #[test]
    fn the_diagnostic_names_the_shape_and_never_a_value() {
        let body = r#"{"id": "SECRET-ID", "usage": {"n": 1}, "steps": [
            {"type": "user_input", "content": [{"type": "text", "text": "private words"}]},
            {"type": "model_output", "content": []}, {"no": "type"}, {"type": "we ird\nname"}]}"#;
        let said = shape(200, body);

        assert_eq!(
            said,
            "status 200, keys [id, steps, usage], step types [user_input, model_output, -, we?ird?name]"
        );
        assert!(!said.contains("SECRET") && !said.contains("private"));
        assert_eq!(shape(200, "<html>secret"), "status 200, not a JSON object");
    }

    #[test]
    fn statuses_become_typed_errors() {
        assert_eq!(failure(401, "", "k"), Error::Key);
        assert_eq!(failure(403, "{}", "k"), Error::Key);
        assert_eq!(failure(429, "", "k"), Error::Limit);
        assert_eq!(failure(503, "", "k"), Error::Server(503));
        assert_eq!(
            failure(400, "not json", "k"),
            Error::Refused(400, String::new())
        );
    }

    #[test]
    fn the_log_line_has_the_status_and_googles_words_without_the_key() {
        let body = r#"{"error": {"code": 503, "status": "UNAVAI LABLE\nx",
            "message": "The model is  overloaded.\nAIzaSecret was tried"}}"#;

        assert_eq!(
            answered(503, body, "AIzaSecret"),
            "Gemini answered 503: UNAVAILABLEx The model is overloaded. [key] was tried"
        );

        // a key that is itself plain characters is cut out of the status too
        let body = r#"{"error": {"status": "AIzaSecret", "message": ""}}"#;

        assert!(!answered(400, body, "AIzaSecret").contains("AIzaSecret"));

        // no JSON, no words: the status alone
        assert_eq!(
            answered(502, "<html>AIzaSecret", "AIzaSecret"),
            "Gemini answered 502"
        );
    }

    #[test]
    fn the_log_line_is_capped() {
        let body = format!(
            r#"{{"error": {{"status": "{}", "message": "{}"}}}}"#,
            "S".repeat(100),
            "m ".repeat(500)
        );
        let line = answered(500, &body, "k");

        assert!(line.len() <= "Gemini answered 500: ".len() + CODE + 1 + MESSAGE);
    }

    #[test]
    fn a_refusal_never_echoes_the_key() {
        let body = r#"{"error": {"code": 400, "message": "bad value for AIzaSecret\nin input"}}"#;
        let error = failure(400, body, "AIzaSecret");

        assert_eq!(
            error,
            Error::Refused(400, String::from("bad value for [key] in input"))
        );
        assert!(!error.to_string().contains("AIzaSecret"));
        assert!(
            !failure(401, body, "AIzaSecret")
                .to_string()
                .contains("AIzaSecret")
        );
    }
}
