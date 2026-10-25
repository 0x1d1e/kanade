//! Voice dictation through the Gemini API (ADR 0111): the recording is kept in memory, wrapped as
//! WAV and sent in one request after it stops. Nothing records or sends without a key in the
//! Secret Service and the user's recorded consent to the disclosure.
//!
//! `api`, `gate`, `wav` and `machine` are pure; `run` carries the machine out (the recorder, the
//! request, the clipboard, the Activity); `cli/dictate.rs` is the setup done in the user's terminal.

pub mod api;
pub mod gate;
pub mod insert;
pub mod machine;
pub mod run;
pub mod wav;

pub use run::locked;

use std::time::Duration;

use ureq::Agent;

use crate::sources::body;

use api::Error;
use gate::Key;

// what `kanade dictate` asks. The first three go to the shell; the rest run in the user's terminal
// and never reach it, as they handle the key and the consent
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    Toggle,
    Cancel,
    Status,
    Key,
    Consent,
    Forget,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            ["toggle"] => Some(Request::Toggle),
            ["cancel"] => Some(Request::Cancel),
            ["status"] => Some(Request::Status),
            ["key"] => Some(Request::Key),
            ["consent"] => Some(Request::Consent),
            ["forget"] => Some(Request::Forget),
            _ => None,
        }
    }

    // whether the CLI does it itself
    pub fn local(self) -> bool {
        matches!(self, Request::Key | Request::Consent | Request::Forget)
    }
}

// how long to wait for the answer to the one request
const PATIENCE: Duration = Duration::from_secs(60);

// the most of an answer read
const ANSWER: u64 = 1 << 20;

/*
 * `pcm` (16 kHz, mono, s16le, as `wav` describes) as text, in one request. The key goes in a header
 * only. Blocks: the caller runs it off the draw thread.
 */
pub fn transcribe(key: &Key, pcm: &[u8]) -> Result<String, Error> {
    let agent: Agent = Agent::config_builder()
        .https_only(true)
        .max_redirects(0)
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .user_agent(concat!("Kanade/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let body = api::request_body(&wav::wrap(pcm)).to_string();

    let mut response = agent
        .post(api::ENDPOINT)
        .header("x-goog-api-key", key.expose())
        .header("content-type", "application/json")
        .send(body)
        .map_err(offline)?;
    let status = response.status().as_u16();
    let text = body::text(response.body_mut(), ANSWER).map_err(offline)?;

    if status == 200 {
        let heard = api::transcript(&text);

        // an answer not read as a transcript says what shape it had, never what it held, so the
        // next run with real speech shows what the request or the parser has wrong
        if matches!(heard, Err(Error::Unreadable | Error::Silence)) {
            eprintln!(
                "kanade: dictation: no transcript: {}",
                api::shape(status, &text)
            );
        }

        heard
    } else {
        // Google's own words for the failure, with the key cut out, for the log
        eprintln!(
            "kanade: dictation: {}",
            api::answered(status, &text, key.expose())
        );

        Err(api::failure(status, &text, key.expose()))
    }
}

// ureq's words name the host and the cause, never a header; the log gets the kind only
fn offline(error: ureq::Error) -> Error {
    eprintln!("kanade: dictation: cannot reach Gemini: {}", kind(&error));

    Error::Offline(error.to_string())
}

// what went wrong with the transport, in a word: no address, query or header
fn kind(error: &ureq::Error) -> String {
    match error {
        ureq::Error::Timeout(_) => String::from("timeout"),
        ureq::Error::HostNotFound => String::from("host not found"),
        ureq::Error::ConnectionFailed => String::from("connection failed"),
        ureq::Error::Io(error) => format!("I/O error ({:?})", error.kind()),
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => String::from("TLS error"),
        ureq::Error::BodyExceedsLimit(_) => String::from("answer too large"),
        _ => String::from("other transport error"),
    }
}
