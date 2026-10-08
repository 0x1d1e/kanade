//! The Google account's credentials in the Secret Service (`org.freedesktop.secrets`: GNOME
//! Keyring, KWallet, KeePassXC), never in a file Kanade writes (ADR 0016). One item, found by its
//! attributes, holds the OAuth client and the refresh token as JSON. A locked keyring asks the
//! user through the keyring's own prompt, on the calling thread, which waits for it.
//!
//! The secret crosses the session bus unencrypted (the `plain` algorithm), as the bus is the
//! user's own; it is never logged or printed, and `Credentials`' `Debug` leaves it out.

use std::collections::HashMap;
use std::fmt;

use serde_json::{Value, json};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value as Variant};

const SERVICE: &str = "org.freedesktop.secrets";
const PATH: &str = "/org/freedesktop/secrets";
const SECRETS: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";

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

// the secret as the Secret Service passes it: session, parameters, value, content type
type Secret = (OwnedObjectPath, Vec<u8>, Vec<u8>, String);

struct Keyring {
    connection: Connection,
    session: OwnedObjectPath,
}

impl Keyring {
    fn open() -> Result<Keyring, String> {
        let connection = Connection::session().map_err(|error| error.to_string())?;
        let (_, session): (OwnedValue, OwnedObjectPath) = proxy(&connection, PATH, SECRETS)?
            .call("OpenSession", &("plain", Variant::from("")))
            .map_err(said)?;

        Ok(Keyring {
            connection,
            session,
        })
    }

    fn proxy(&self, path: &str, interface: &'static str) -> Result<Proxy<'static>, String> {
        proxy(&self.connection, path, interface)
    }

    // Kanade's items, unlocked; a locked one is unlocked first, which may prompt
    fn items(&self) -> Result<Vec<OwnedObjectPath>, String> {
        let attributes: HashMap<&str, &str> = ATTRIBUTES.into_iter().collect();
        let (mut unlocked, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = self
            .proxy(PATH, SECRETS)?
            .call("SearchItems", &(attributes,))
            .map_err(said)?;

        if !locked.is_empty() {
            unlocked.extend(self.unlock(locked)?);
        }

        Ok(unlocked)
    }

    // the objects unlocked, after the keyring's prompt if it shows one
    fn unlock(&self, objects: Vec<OwnedObjectPath>) -> Result<Vec<OwnedObjectPath>, String> {
        let (mut unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = self
            .proxy(PATH, SECRETS)?
            .call("Unlock", &(objects,))
            .map_err(said)?;

        if let Some(result) = self.prompt(&prompt)? {
            unlocked.extend(Vec::<OwnedObjectPath>::try_from(result).map_err(said)?);
        }

        Ok(unlocked)
    }

    /*
     * shows the keyring's prompt at `prompt` and waits for the user; none for no prompt ("/"),
     * else what it gives. Dismissing it is an error
     */
    fn prompt(&self, prompt: &ObjectPath<'_>) -> Result<Option<OwnedValue>, String> {
        if prompt.as_str() == "/" {
            return Ok(None);
        }

        let proxy = self.proxy(prompt.as_str(), PROMPT)?;
        let mut completed = proxy.receive_signal("Completed").map_err(said)?;

        // no window of Kanade's to put it over
        proxy.call_method("Prompt", &("",)).map_err(said)?;

        let message = completed
            .next()
            .ok_or_else(|| String::from("the keyring's prompt went away"))?;
        let (dismissed, result): (bool, OwnedValue) = message.body().deserialize().map_err(said)?;

        if dismissed {
            return Err(String::from("the keyring was not unlocked"));
        }

        Ok(Some(result))
    }
}

fn proxy(
    connection: &Connection,
    path: &str,
    interface: &'static str,
) -> Result<Proxy<'static>, String> {
    Proxy::new(connection, SERVICE, path.to_owned(), interface).map_err(said)
}

// the Secret Service's own words; they never hold the secret
fn said(error: impl fmt::Display) -> String {
    format!("the Secret Service: {error}")
}

// the credentials kept, none when there are none
pub fn load() -> Result<Option<Credentials>, String> {
    let keyring = Keyring::open()?;

    for item in keyring.items()? {
        let (_, _, value, _): Secret = keyring
            .proxy(item.as_str(), ITEM)?
            .call("GetSecret", &(&keyring.session,))
            .map_err(said)?;

        if let Some(credentials) = Credentials::decode(&value) {
            return Ok(Some(credentials));
        }
    }

    Ok(None)
}

// keeps `credentials` in the default keyring, over any kept before
pub fn store(credentials: &Credentials) -> Result<(), String> {
    let keyring = Keyring::open()?;

    let collection: OwnedObjectPath = keyring
        .proxy(PATH, SECRETS)?
        .call("ReadAlias", &("default",))
        .map_err(said)?;
    if collection.as_str() == "/" {
        return Err(String::from("the Secret Service has no default keyring"));
    }

    keyring.unlock(vec![collection.clone()])?;

    let attributes: HashMap<&str, &str> = ATTRIBUTES.into_iter().collect();
    let properties: HashMap<&str, Variant<'_>> = HashMap::from([
        ("org.freedesktop.Secret.Item.Label", Variant::from(LABEL)),
        (
            "org.freedesktop.Secret.Item.Attributes",
            Variant::from(attributes),
        ),
    ]);
    let secret: Secret = (
        keyring.session.clone(),
        Vec::new(),
        credentials.encode(),
        String::from("application/json"),
    );

    let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = keyring
        .proxy(collection.as_str(), COLLECTION)?
        .call("CreateItem", &(properties, secret, true))
        .map_err(said)?;
    keyring.prompt(&prompt)?;

    Ok(())
}

// forgets the credentials, none kept being no error
pub fn delete() -> Result<(), String> {
    let keyring = Keyring::open()?;

    for item in keyring.items()? {
        let prompt: OwnedObjectPath = keyring
            .proxy(item.as_str(), ITEM)?
            .call("Delete", &())
            .map_err(said)?;
        keyring.prompt(&prompt)?;
    }

    Ok(())
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
