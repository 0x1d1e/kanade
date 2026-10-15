use std::time::{Duration, SystemTime};

use kanade_runtime::service::Service;

use crate::bus::{Argument, Bus, Method, Value};

use super::{Notification, Notifications, Urgency};

use super::{Action, Reason, image_hint};
use crate::config;
use crate::sources::notifications::rules::{self, Fields};
use crate::sources::sound::Hints;

const NAME: &str = "org.freedesktop.Notifications";

const PATH: &str = "/org/freedesktop/Notifications";

const INTERFACE: &str = "org.freedesktop.Notifications";

// the key of what clicking the notification itself does
pub const DEFAULT_ACTION: &str = "default";

pub fn run() {
    let bus = Bus::session();

    // watched before owning the name, so no early call is missed
    let methods = bus.methods(PATH, INTERFACE);

    if !bus.own(NAME) {
        return;
    }

    Notifications::write().running = true;

    for method in methods {
        answer(&method);
    }
}

fn answer(method: &Method) {
    match method.name() {
        "Notify" => notify(method),
        "CloseNotification" => close(method),
        "GetCapabilities" => send_capabilities(method),
        "GetServerInformation" => send_information(method),

        // every call waits for an answer, even one Kanade does not know
        _ => method.reply(&[]),
    }
}

fn notify(method: &Method) {
    let [
        app_name,
        replaces_id,
        icon,
        summary,
        body,
        actions,
        hints,
        timeout,
    ] = method.arguments()
    else {
        method.reply(&[]);

        return;
    };

    let (actions, has_default) = read_actions(actions);

    let mut notification = Notification {
        // add() picks the real id
        id: 0,

        app_name: String::from(app_name.text()),
        desktop_entry: String::from(hints.get("desktop-entry").text()),
        category: String::from(hints.get("category").text()),
        summary: String::from(summary.text()),
        body: String::from(body.text()),
        icon: String::from(icon.text()),
        image: image_hint::read(hints),

        urgency: Urgency::from_level(hints.get("urgency").number()),

        actions,
        has_default,

        resident: hints.get("resident").bool(),

        sound: Hints {
            suppress: hints.get("suppress-sound").bool(),
            file: String::from(hints.get("sound-file").text()),
            name: String::from(hints.get("sound-name").text()),
        },

        value: progress(hints.get("value")),
        timeout: lasts(timeout.number()),

        received: SystemTime::now(),

        // add() picks it too
        version: 0,

        effects: rules::Effects::default(),
    };

    // the rules in effect as it arrives decide what it does, for good
    let effects = rules::resolve(
        &config::get().notification_rules,
        Fields {
            app: &notification.app_name,
            desktop_entry: &notification.desktop_entry,
            category: &notification.category,
            summary: &notification.summary,
        },
    );

    if let Some(urgency) = effects.urgency {
        notification.urgency = urgency;
    }

    notification.effects = effects;

    let replaces_id = replaces_id.number() as u32;

    let id = Notifications::write().add(notification, replaces_id);

    method.reply(&[Argument::from(id)]);
}

/*
 * the `value` hint, a progress from 0 to 100 that the spec does not list but senders like
 * `notify-send -h int:value:40` give (ADR 0059); a number of any kind, out of range kept in it,
 * anything else none
 */
fn progress(hint: &Value) -> Option<u8> {
    match hint {
        Value::Number(value) if value.is_finite() => Some(value.round().clamp(0.0, 100.0) as u8),
        _ => None,
    }
}

// `expire_timeout`: -1 leaves it to the server and 0 is never, so only a positive one is a time
fn lasts(milliseconds: f64) -> Option<Duration> {
    (milliseconds >= 1.0).then(|| Duration::from_millis(milliseconds as u64))
}

/*
 * actions come as one flat list: key, label, key, label;
 * the "default" one is what clicking the notification itself does
 */
fn read_actions(list: &Value) -> (Vec<Action>, bool) {
    let mut actions = Vec::new();
    let mut has_default = false;

    for [key, label] in list.list().as_chunks::<2>().0 {
        let key = key.text();
        let label = label.text();

        if key == DEFAULT_ACTION {
            has_default = true;

            continue;
        }

        actions.push(Action {
            key: String::from(key),
            label: String::from(label),
        });
    }

    (actions, has_default)
}

fn close(method: &Method) {
    let id = method.arguments().first().map(|id| id.number() as u32);

    if let Some(id) = id {
        Notifications::write().close(id, Reason::Closed);
    }

    method.reply(&[]);
}

fn send_capabilities(method: &Method) {
    let mut capabilities = vec![
        String::from("actions"),
        String::from("body"),
        String::from("icon-static"),
    ];

    // so a sender that can play its own sound knows Kanade does
    if config::get().notification_sound {
        capabilities.push(String::from("sound"));
    }

    method.reply(&[Argument::from(capabilities)]);
}

fn send_information(method: &Method) {
    let information = [
        Argument::from("Kanade"),
        Argument::from("0x1d1e"),
        Argument::from(env!("CARGO_PKG_VERSION")),
        Argument::from("1.2"),
    ];

    method.reply(&information);
}

// the sender hears which button was pressed, and does the rest itself
pub fn announce_action(id: u32, key: &str) {
    let arguments = [Argument::from(id), Argument::from(key)];

    Bus::session().emit(PATH, INTERFACE, "ActionInvoked", &arguments);
}

pub fn announce_closed(id: u32, reason: Reason) {
    let arguments = [Argument::from(id), Argument::from(reason as u32)];

    Bus::session().emit(PATH, INTERFACE, "NotificationClosed", &arguments);
}
