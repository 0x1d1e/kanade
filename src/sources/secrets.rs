//! Kanade's secrets in the Secret Service (`org.freedesktop.secrets`: GNOME Keyring, KWallet,
//! KeePassXC), never in a file Kanade writes: the Google account's credentials (ADR 0016). An item is
//! found by its attributes. A locked keyring asks the user through the keyring's own prompt, on
//! the calling thread, which waits for it.
//!
//! A secret crosses the session bus unencrypted (the `plain` algorithm), as the bus is the user's
//! own; it is never logged or printed, and none of the errors here hold it.

use std::collections::HashMap;
use std::fmt;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value as Variant};
use zeroize::{Zeroize, Zeroizing};

// the Secret Service's bus name, which a Module that keeps a secret needs
pub const SERVICE: &str = "org.freedesktop.secrets";

const PATH: &str = "/org/freedesktop/secrets";
const SECRETS: &str = "org.freedesktop.Secret.Service";
const COLLECTION: &str = "org.freedesktop.Secret.Collection";
const ITEM: &str = "org.freedesktop.Secret.Item";
const PROMPT: &str = "org.freedesktop.Secret.Prompt";

// what the user dismissing the keyring's prompt fails with
pub const NOT_UNLOCKED: &str = "the keyring was not unlocked";

// an item's attributes, which find it; each of Kanade's has `application` = `kanade`
pub type Attributes<'a> = &'a [(&'a str, &'a str)];

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

    // the items with `attributes`, unlocked; a locked one is unlocked first, which may prompt
    fn items(&self, attributes: Attributes<'_>) -> Result<Vec<OwnedObjectPath>, String> {
        let attributes: HashMap<&str, &str> = attributes.iter().copied().collect();
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
            return Err(String::from(NOT_UNLOCKED));
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

/*
 * the first secret kept with `attributes` that `read` takes, none when there is none. The bytes
 * are zeroed once `read` is done with them
 */
pub fn load<T>(
    attributes: Attributes<'_>,
    read: impl Fn(&[u8]) -> Option<T>,
) -> Result<Option<T>, String> {
    let keyring = Keyring::open()?;

    for item in keyring.items(attributes)? {
        let (_, _, value, _): Secret = keyring
            .proxy(item.as_str(), ITEM)?
            .call("GetSecret", &(&keyring.session,))
            .map_err(said)?;
        let value = Zeroizing::new(value);

        if let Some(read) = read(&value) {
            return Ok(Some(read));
        }
    }

    Ok(None)
}

/*
 * keeps `secret` in the default keyring with `attributes`, over any kept with them before, under
 * `label`, which a keyring app shows
 */
pub fn store(
    attributes: Attributes<'_>,
    label: &str,
    secret: &[u8],
    content_type: &str,
) -> Result<(), String> {
    let keyring = Keyring::open()?;

    let collection: OwnedObjectPath = keyring
        .proxy(PATH, SECRETS)?
        .call("ReadAlias", &("default",))
        .map_err(said)?;
    if collection.as_str() == "/" {
        return Err(String::from("the Secret Service has no default keyring"));
    }

    keyring.unlock(vec![collection.clone()])?;

    let attributes: HashMap<&str, &str> = attributes.iter().copied().collect();
    let properties: HashMap<&str, Variant<'_>> = HashMap::from([
        ("org.freedesktop.Secret.Item.Label", Variant::from(label)),
        (
            "org.freedesktop.Secret.Item.Attributes",
            Variant::from(attributes),
        ),
    ]);
    let mut sent: Secret = (
        keyring.session.clone(),
        Vec::new(),
        secret.to_vec(),
        String::from(content_type),
    );

    let created = keyring
        .proxy(collection.as_str(), COLLECTION)
        .and_then(|collection| {
            collection
                .call::<_, _, (OwnedObjectPath, OwnedObjectPath)>(
                    "CreateItem",
                    &(properties, &sent, true),
                )
                .map_err(said)
        });
    sent.2.zeroize();

    let (_, prompt) = created?;
    keyring.prompt(&prompt)?;

    Ok(())
}

// forgets the secrets kept with `attributes`, none kept being no error
pub fn delete(attributes: Attributes<'_>) -> Result<(), String> {
    let keyring = Keyring::open()?;

    for item in keyring.items(attributes)? {
        let prompt: OwnedObjectPath = keyring
            .proxy(item.as_str(), ITEM)?
            .call("Delete", &())
            .map_err(said)?;
        keyring.prompt(&prompt)?;
    }

    Ok(())
}
