//! Progress an app reports through its launcher entry (ADR 0083): the Unity `LauncherEntry`
//! signal, `com.canonical.Unity.LauncherEntry.Update(app_uri, properties)`, which Chromium and
//! Firefox downloads, Nautilus and others send on the session bus. An entry whose `progress-visible`
//! is true is a live Activity (ADR 0042) titled with the app's name; it ends as done at full, and goes
//! when the entry hides it. `count` and `urgent` are not shown.
//!
//! Any process on the session bus can send the signal, so what comes in is bounded as a client's
//! `kanade activity` is: the app is named by a `.desktop` file id of a small alphabet, at most two
//! entries are live (`live::LAUNCHED`), and the name shown is the desktop entry's, never the
//! signal's. A signal is taken only from the app it speaks for: its desktop file id, minus
//! `.desktop`, is also the bus name a D-Bus activatable app owns (the launcher entry spec), so when
//! the bus says that name has an owner, the signal must come from it. A name nobody owns, or one
//! D-Bus could not name (an id with a hyphen), says nothing and is taken as before. The thread
//! blocks on the signal, so it adds no idle wakeup.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::desktop;
use super::live;
use crate::bus::{Argument, Bus, Value};

const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
const SCHEME: &str = "application://";
const DBUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";

// the longest desktop file id taken; the live id is `launcher.` and this, within `live::ID`
const LONGEST: usize = 48;

// how many apps' progress are tracked at once; the live ones are fewer still
const TRACKED: usize = 16;

// the desktop entries are read again for an unknown app no oftener than this
const RESCAN: Duration = Duration::from_secs(30);

/// What an app says of its progress; the signal may carry only the properties that changed.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Progress {
    pub visible: bool,

    // 0 to 1
    pub amount: f64,
}

impl Progress {
    // takes `progress-visible` and `progress` from the signal's properties, if present and of the
    // right kind; says whether either was there
    fn apply(&mut self, properties: &Value) -> bool {
        let mut told = false;

        if let Value::Bool(visible) = properties.get("progress-visible") {
            self.visible = *visible;
            told = true;
        }

        if let Value::Number(amount) = properties.get("progress") {
            // a NaN or an out-of-range amount is no more than the ends of the bar
            self.amount = if amount.is_nan() {
                0.0
            } else {
                amount.clamp(0.0, 1.0)
            };
            told = true;
        }

        told
    }

    /// The bar in whole percent.
    pub fn percent(self) -> u8 {
        (self.amount * 100.0).round() as u8
    }
}

/// The desktop file id an `application://` URI names, if it is one of a small alphabet and length.
pub fn app_of(uri: &str) -> Option<&str> {
    let id = uri.strip_prefix(SCHEME)?;
    let name = id.strip_suffix(".desktop")?;

    let fits = !name.is_empty()
        && id.len() <= LONGEST
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));

    fits.then_some(id)
}

fn live_id(app: &str) -> String {
    format!("{}{app}", live::LAUNCHER)
}

/// Runs on its own thread for good, or ends at once without a session bus.
pub fn follow() {
    let mut tracked: HashMap<String, Progress> = HashMap::new();
    let mut names = Names::default();
    let mut senders = Senders::default();

    for signal in Bus::session().signals(INTERFACE, "Update") {
        let [uri, properties] = signal.arguments() else {
            continue;
        };

        let Some(app) = app_of(uri.text()) else {
            continue;
        };

        if !senders.trusts(app, signal.sender(), guard_of) {
            continue;
        }

        if !tracked.contains_key(app) && tracked.len() >= TRACKED {
            continue;
        }

        let progress = tracked.entry(app.to_owned()).or_default();

        if !progress.apply(properties) {
            continue;
        }

        let progress = *progress;
        let id = live_id(app);

        if !progress.visible {
            tracked.remove(app);
            live::withdraw(&id);
        } else if progress.amount >= 1.0 {
            tracked.remove(app);
            live::finish(&id);
        } else {
            let name = names.of(app);

            // refused past the Activities allowed; the next update asks again
            let _ = live::show(&id, &name, "", progress.percent(), None);
        }
    }
}

/*
 * whose signal each app's progress is taken from: the owner of the app's own bus name, which is its
 * desktop file id without `.desktop` where that is a valid D-Bus name. Asked at most once per app
 * per RESCAN, never at each signal, as an app reports its progress at every step and a stranger
 * must not cost a round trip each; a restarted app is followed within RESCAN
 */
#[derive(Default)]
struct Senders {
    owners: HashMap<String, (Option<String>, Instant)>,
}

impl Senders {
    /*
     * whether a signal from `sender` speaks for `app`: a name nobody owns, or an id D-Bus could not
     * name, says nothing, and the signal is taken as it was before this check
     */
    fn trusts(&mut self, app: &str, sender: &str, ask: impl Fn(&str) -> Option<String>) -> bool {
        if sender.is_empty() {
            return false;
        }

        let Some(name) = app.strip_suffix(".desktop").filter(|name| bus_name(name)) else {
            return true;
        };

        let known = self.owners.get(app).filter(|(_, at)| at.elapsed() < RESCAN);
        let owner = match known {
            Some((owner, _)) => owner.clone(),
            None => {
                let owner = ask(name);

                self.owners
                    .insert(app.to_owned(), (owner.clone(), Instant::now()));

                owner
            }
        };

        owner.as_deref().is_none_or(|owner| owner == sender)
    }
}

// the unique name that owns `name` on the session bus, none when nobody does
fn guard_of(name: &str) -> Option<String> {
    let owner = Bus::session().call(
        DBUS,
        BUS_PATH,
        DBUS,
        "GetNameOwner",
        &[Argument::from(name)],
    );
    let owner = owner.text();

    (!owner.is_empty()).then(|| owner.to_owned())
}

// a well-known bus name, as D-Bus's spec has it (as `launch.rs` reads an activatable entry's id)
fn bus_name(name: &str) -> bool {
    let elements: Vec<&str> = name.split('.').collect();

    name.len() <= 255
        && elements.len() >= 2
        && elements.iter().all(|element| {
            element
                .chars()
                .next()
                .is_some_and(|first| !first.is_ascii_digit())
                && element
                    .chars()
                    .all(|char| char.is_ascii_alphanumeric() || char == '_' || char == '-')
        })
}

// the display names of desktop entries, read when an app is first met
#[derive(Default)]
struct Names {
    by_id: HashMap<String, String>,
    read: Option<Instant>,
}

impl Names {
    // the entry's name, else the file id without `.desktop`
    fn of(&mut self, app: &str) -> String {
        if !self.by_id.contains_key(app) && self.read.is_none_or(|at| at.elapsed() >= RESCAN) {
            self.by_id = desktop::scan(&desktop::dirs())
                .into_iter()
                .map(|entry| (entry.id, entry.name))
                .collect();
            self.read = Some(Instant::now());
        }

        self.by_id
            .get(app)
            .cloned()
            .unwrap_or_else(|| app.trim_end_matches(".desktop").to_owned())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn properties(entries: &[(&str, Value)]) -> Value {
        Value::Map(
            entries
                .iter()
                .map(|(key, value)| ((*key).to_owned(), value.clone()))
                .collect::<BTreeMap<_, _>>(),
        )
    }

    // a stranger on the session bus must not drive an app's progress, though an app that owns no
    // name, or an id D-Bus cannot name, keeps working as before
    #[test]
    fn only_the_apps_own_sender_speaks_for_it() {
        let mut senders = Senders::default();
        let owned = |_: &str| Some(String::from(":1.7"));

        // the app that owns its own name, and not another
        assert!(senders.trusts("org.mozilla.firefox.desktop", ":1.7", owned));
        assert!(!senders.trusts("org.mozilla.firefox.desktop", ":1.9", |_| {
            Some(String::from(":1.7"))
        }));

        // an app whose name nobody owns: as before this check, and no round trip for the next
        assert!(senders.trusts("org.example.app.desktop", ":1.4", |_| None));
        assert!(senders.trusts("org.example.app.desktop", ":1.5", |_| {
            panic!("asked again before RESCAN")
        }));

        // an id D-Bus cannot name (a hyphen, or one element): never checked
        for app in ["libreoffice-writer.desktop", "firefox.desktop"] {
            assert!(senders.trusts(app, ":1.9", |_| panic!("asked for {app}")));
        }

        // nothing at all is no sender
        assert!(!senders.trusts("org.a.desktop", "", |_| panic!("asked")));
    }

    #[test]
    fn only_a_plain_desktop_file_id_names_an_app() {
        assert_eq!(
            app_of("application://org.mozilla.firefox.desktop"),
            Some("org.mozilla.firefox.desktop")
        );
        assert_eq!(app_of("application://a-b_c.desktop"), Some("a-b_c.desktop"));

        for uri in [
            "",
            "application://",
            "application://.desktop",
            "file:///a.desktop",
            "application://a.service",
            "application://a b.desktop",
            "application://a/b.desktop",
            "application://a\u{202e}.desktop",
            "application://a\n.desktop",
            &format!("application://{}.desktop", "a".repeat(LONGEST)),
        ] {
            assert_eq!(app_of(uri), None, "{uri:?}");
        }
    }

    #[test]
    fn the_signal_is_read_for_the_two_properties_and_nothing_else() {
        let mut progress = Progress::default();

        // a count alone says nothing of progress
        assert!(!progress.apply(&properties(&[("count", Value::Number(3.0))])));
        assert_eq!(progress, Progress::default());

        assert!(progress.apply(&properties(&[
            ("progress-visible", Value::Bool(true)),
            ("progress", Value::Number(0.426)),
        ])));
        assert_eq!((progress.visible, progress.percent()), (true, 43));

        // a later signal that carries only the amount keeps the rest
        assert!(progress.apply(&properties(&[("progress", Value::Number(7.0))])));
        assert_eq!((progress.visible, progress.percent()), (true, 100));

        assert!(progress.apply(&properties(&[("progress", Value::Number(f64::NAN))])));
        assert_eq!(progress.percent(), 0);

        // a value of another kind is no value
        assert!(!progress.apply(&properties(&[
            ("progress-visible", Value::Text(String::from("true"))),
            ("progress", Value::Text(String::from("0.5"))),
        ])));
        assert!(progress.visible);

        assert!(!progress.apply(&Value::Nothing));
    }
}
