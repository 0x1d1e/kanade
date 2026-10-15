//! What must be true before anything is recorded or sent: a key in the Secret Service and the
//! user's consent to the disclosure. The decision is pure; reading the key and the marker is the
//! caller's.

use std::fmt;
use std::io;
use std::path::PathBuf;

use zeroize::Zeroizing;

use crate::atomic::{self, Mode};
use crate::config;
use crate::sources::secrets;

// what finds Kanade's one item, and what a keyring app shows of it
const ATTRIBUTES: [(&str, &str); 2] = [("application", "kanade"), ("account", "dictation-gemini")];
const LABEL: &str = "Kanade: Gemini API key (dictation)";

// the consent marker, under `$XDG_STATE_HOME`
const MARKER: &str = "kanade/dictation-consent";

/*
 * what the marker says when the user accepted the disclosure. The number is the disclosure's
 * version: reword what the user agrees to materially and raise it, so the old consent no longer
 * counts
 */
pub const CONSENT: &str = "gemini-audio-disclosure v1";

/*
 * what `kanade dictate consent` shows before it asks. Reword what the user agrees to materially and
 * raise `CONSENT`'s number with it
 */
pub const DISCLOSURE: &str = "\
Dictation sends your voice to Google.

  - When you stop a dictation, the recording (up to 120 seconds) is sent to the Gemini API
    over HTTPS in one request, and the text comes back. Nothing is recorded or sent otherwise.
  - Kanade keeps the audio in memory only and writes none to disk. Your API key stays in the
    Secret Service. The microphone indicator lights while it records.
  - On the free tier, Google's terms allow it to use what you send to improve its products, and
    human reviewers may read it. Do not dictate passwords, private or confidential matter, or
    things about other people with a free-tier key. Paid usage is under other terms; Kanade cannot
    tell which kind of key you gave it, and cannot vouch for Google's terms. Read them.
  - `kanade dictate forget` deletes the key and takes this consent back.
";

// what the user types to accept it, exactly
pub const PHRASE: &str = "I agree";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    NoKey,
    NoConsent,
    Ready,
}

impl Gate {
    // for `kanade dictate status`
    pub fn said(self) -> &'static str {
        match self {
            Gate::NoKey => "no key (kanade dictate key)",
            Gate::NoConsent => "no consent (kanade dictate consent)",
            Gate::Ready => "ready",
        }
    }
}

// the key comes first: consent is asked for when there is something to send it with
pub fn decide(has_key: bool, marker: Option<&str>) -> Gate {
    if !has_key {
        Gate::NoKey
    } else if marker.map(str::trim) != Some(CONSENT) {
        Gate::NoConsent
    } else {
        Gate::Ready
    }
}

// whether the audio may still be sent, asked just before it is, with the key already in hand: a
// consent taken back (or a disclosure reworded) while it was recorded means it stays
pub fn may_send(marker: Option<&str>) -> bool {
    decide(true, marker) == Gate::Ready
}

// the Gemini API key; its `Debug` and `Display` leave it out, and it is zeroed when dropped
#[derive(Clone)]
pub struct Key(Zeroizing<String>);

impl Key {
    // the key in `text`, trimmed; none for what cannot be one (empty, or not printable ASCII, which
    // a header could not carry)
    pub fn new(text: &str) -> Option<Key> {
        let text = text.trim();

        (!text.is_empty() && text.bytes().all(|byte| byte.is_ascii_graphic()))
            .then(|| Key(Zeroizing::new(text.to_owned())))
    }

    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Key {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Key(..)")
    }
}

// the key kept, none when there is none
pub fn load_key() -> Result<Option<Key>, String> {
    secrets::load(&ATTRIBUTES, |bytes| {
        Key::new(std::str::from_utf8(bytes).ok()?)
    })
}

// keeps `key` in the default keyring, over any kept before
pub fn store_key(key: &Key) -> Result<(), String> {
    secrets::store(&ATTRIBUTES, LABEL, key.expose().as_bytes(), "text/plain")
}

// forgets the key, none kept being no error
pub fn delete_key() -> Result<(), String> {
    secrets::delete(&ATTRIBUTES)
}

fn marker() -> Option<PathBuf> {
    config::base(
        "XDG_STATE_HOME",
        ".local/state",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join(MARKER))
}

// the marker's text, none when there is no marker
pub fn read_consent() -> Option<String> {
    std::fs::read_to_string(marker()?).ok()
}

// records that the user accepted the disclosure
pub fn record_consent() -> io::Result<()> {
    let path =
        marker().ok_or_else(|| io::Error::other("neither XDG_STATE_HOME nor HOME is set"))?;

    atomic::write(&path, format!("{CONSENT}\n").as_bytes(), Mode::Private)
}

// takes the consent back; none recorded being no error
pub fn withdraw_consent() -> io::Result<()> {
    match marker().map(std::fs::remove_file) {
        Some(Err(error)) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_ready_without_both() {
        assert_eq!(decide(false, None), Gate::NoKey);
        assert_eq!(decide(false, Some(CONSENT)), Gate::NoKey);
        assert_eq!(decide(true, None), Gate::NoConsent);
        assert_eq!(decide(true, Some("")), Gate::NoConsent);
        assert_eq!(
            decide(true, Some("gemini-audio-disclosure v0")),
            Gate::NoConsent
        );
        assert_eq!(decide(true, Some(&format!("{CONSENT}\n"))), Gate::Ready);
    }

    #[test]
    fn audio_is_not_sent_once_the_consent_is_gone_or_stale() {
        assert!(may_send(Some(CONSENT)));
        assert!(!may_send(None));
        assert!(!may_send(Some("gemini-audio-disclosure v0")));
    }

    #[test]
    fn a_key_is_one_header_can_carry_and_is_never_shown() {
        assert!(Key::new("  \n").is_none());
        assert!(Key::new("AIza one").is_none());
        assert!(Key::new("AIza\r\nX-Evil: 1").is_none());
        assert!(Key::new("clé").is_none());

        let key = Key::new(" AIzaSyExample\n").unwrap();

        assert_eq!(key.expose(), "AIzaSyExample");
        assert!(!format!("{key:?}").contains("AIza"));
    }
}
