//! The Google account's credentials in the Secret Service (`sources::secrets`), never in a file
//! Kanade writes (ADR 0016). One item, found by its attributes, holds the OAuth client and the
//! refresh token as JSON; `Credentials`' `Debug` leaves the secrets out.

use std::fmt;

use serde_json::{Value, json};

use crate::sources::secrets;

// what finds Kanade's one item, and what a keyring app shows of it
const ATTRIBUTES: [(&str, &str); 2] = [("application", "kanade"), ("account", "google-calendar")];
const LABEL: &str = "Kanade: Google Calendar";

// the OAuth client the user made in Google Cloud, and the account's grant to it
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    // this sign-in, random, so the synced events can say whose they are without a secret
    pub account: String,
    pub client_id: String,
    pub client_secret: String,
    pub refresh_token: String,
}

// the client id is no secret, the rest is
impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("account", &self.account)
            .field("client_id", &self.client_id)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    fn encode(&self) -> Vec<u8> {
        json!({
            "account": self.account,
            "client_id": self.client_id,
            "client_secret": self.client_secret,
            "refresh_token": self.refresh_token,
        })
        .to_string()
        .into_bytes()
    }

    fn decode(bytes: &[u8]) -> Option<Credentials> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        let field = |name: &str| Some(value.get(name)?.as_str()?.to_owned());

        Some(Credentials {
            account: field("account")?,
            client_id: field("client_id")?,
            client_secret: field("client_secret")?,
            refresh_token: field("refresh_token")?,
        })
    }
}

// the credentials kept, none when there are none
pub fn load() -> Result<Option<Credentials>, String> {
    secrets::load(&ATTRIBUTES, Credentials::decode)
}

// keeps `credentials` in the default keyring, over any kept before
pub fn store(credentials: &Credentials) -> Result<(), String> {
    let encoded = zeroize::Zeroizing::new(credentials.encode());

    secrets::store(&ATTRIBUTES, LABEL, &encoded, "application/json")
}

// forgets the credentials, none kept being no error
pub fn delete() -> Result<(), String> {
    secrets::delete(&ATTRIBUTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn credentials() -> Credentials {
        Credentials {
            account: String::from("a1"),
            client_id: String::from("id.apps.googleusercontent.com"),
            client_secret: String::from("GOCSPX-secret"),
            refresh_token: String::from("1//refresh"),
        }
    }

    #[test]
    fn credentials_round_trip_as_json() {
        let kept = credentials();

        assert_eq!(Credentials::decode(&kept.encode()), Some(kept));
        assert_eq!(Credentials::decode(b"{\"client_id\": \"a\"}"), None);
        assert_eq!(Credentials::decode(b"not json"), None);
    }

    #[test]
    fn debug_leaves_the_secrets_out() {
        let printed = format!("{:?}", credentials());

        assert!(printed.contains("id.apps.googleusercontent.com"));
        assert!(!printed.contains("GOCSPX"));
        assert!(!printed.contains("refresh"));
    }
}
