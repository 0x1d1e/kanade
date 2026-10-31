use std::time::{Duration, SystemTime};

use kanade_runtime::service::Service;

use crate::bus::{Argument, Bus, Method, MethodEvent, Value};

use super::{Notification, Notifications, Urgency};

use super::{Action, Reason, image_hint};
use crate::config;
use crate::modules;
use crate::sources::notifications::rules::{self, Fields};
use crate::sources::sound::Hints;

const NAME: &str = "org.freedesktop.Notifications";

const PATH: &str = "/org/freedesktop/Notifications";

const INTERFACE: &str = "org.freedesktop.Notifications";

// the key of what clicking the notification itself does
pub const DEFAULT_ACTION: &str = "default";

// the KDE extension for a typed reply (ADR 0079): this action key is a field, not a button
const REPLY_ACTION: &str = "inline-reply";

// the hint with the field's placeholder
const REPLY_PLACEHOLDER: &str = "x-kde-reply-placeholder-text";

// longest reply sent, in characters
pub const REPLY_LIMIT: usize = 2000;

// the reply field lives on a Banner, so it is offered only while Banners are
fn replies() -> bool {
    modules::on("banners")
}

pub fn run() {
    let bus = Bus::session();

    // Watch before owning the name, so no early call is missed.
    let mut methods = bus.methods_with_reconnect(PATH, INTERFACE);
    let mut ever_owned = false;
    let mut attached = false;

    for event in &mut methods {
        match event {
            // the name is taken on the connection the calls arrive on; a reattach that fails is
            // retried on the next, and the Service says it is not running meanwhile
            MethodEvent::Attached(connection, _) => {
                attached = bus.own_on(&connection, NAME);
                if !attached && !ever_owned {
                    return;
                }
                ever_owned |= attached;
                Notifications::write().running = attached;
            }
            MethodEvent::Method(method) if attached => answer(&method),
            MethodEvent::Method(_) => {}
        }
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

    let (actions, has_default, wants_reply) = read_actions(actions, replies());

    let reply = wants_reply.then(|| String::from(hints.get(REPLY_PLACEHOLDER).text()));

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
        reply,
        sender: method.sender(),

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

    // the rules in effect as it arrives decide what it does, for good; app silence applies
    // only when none matched
    let rules = config::get().notification_rules.clone();
    let fields = Fields {
        app: &notification.app_name,
        desktop_entry: &notification.desktop_entry,
        category: &notification.category,
        summary: &notification.summary,
    };
    let effects = super::super::policy::quiet_app(
        rules::resolve(&rules, fields),
        rules::matched(&rules, fields),
        super::super::silence::covers(&notification.app_name, &notification.desktop_entry),
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

/*
 * `expire_timeout`: -1 leaves the time to the server and 0 is never, which a Banner tells from the
 * urgency's own time; a positive one is that time
 */
fn lasts(milliseconds: f64) -> Option<Duration> {
    if milliseconds == 0.0 {
        Some(Duration::ZERO)
    } else {
        (milliseconds > 0.0).then(|| Duration::from_millis(milliseconds as u64))
    }
}

/*
 * actions come as one flat list: key, label, key, label;
 * the "default" one is what clicking the notification itself does, and with `replies` the
 * "inline-reply" one is a field to type in, not a button
 */
fn read_actions(list: &Value, replies: bool) -> (Vec<Action>, bool, bool) {
    let mut actions = Vec::new();
    let mut has_default = false;
    let mut has_reply = false;

    for [key, label] in list.list().as_chunks::<2>().0 {
        let key = key.text();
        let label = label.text();

        if key == DEFAULT_ACTION {
            has_default = true;

            continue;
        }

        if replies && key == REPLY_ACTION {
            has_reply = true;

            continue;
        }

        actions.push(Action {
            key: String::from(key),
            label: String::from(label),
        });
    }

    (actions, has_default, has_reply)
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

    if replies() {
        capabilities.push(String::from(REPLY_ACTION));
    }

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

// the typed reply goes to the one program that sent the notification, never to everyone
pub fn announce_reply(sender: &str, id: u32, text: &str) {
    let arguments = [Argument::from(id), Argument::from(text)];

    Bus::session().emit_to(sender, PATH, INTERFACE, "NotificationReplied", &arguments);
}

pub fn announce_closed(id: u32, reason: Reason) {
    let arguments = [Argument::from(id), Argument::from(reason as u32)];

    Bus::session().emit(PATH, INTERFACE, "NotificationClosed", &arguments);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn list(keys: &[&str]) -> Value {
        Value::List(
            keys.iter()
                .flat_map(|key| [Value::Text((*key).into()), Value::Text(String::new())])
                .collect(),
        )
    }

    // `expire_timeout` keeps 0 apart from -1: 0 is never, which the Banner shows as no time at
    // all, and -1 leaves the time to the server, which its urgency gives
    #[test]
    fn a_sender_that_asks_never_is_not_given_a_time() {
        assert_eq!(lasts(0.0), Some(Duration::ZERO));
        assert_eq!(lasts(-1.0), None);
        assert_eq!(lasts(6000.0), Some(Duration::from_secs(6)));
        assert_eq!(lasts(-2000.0), None);
    }

    // the reply action is a field only while Banners can show one; otherwise a sender gets what it
    // always got, a button
    #[test]
    fn the_reply_action_is_a_field_only_when_replies_are_offered() {
        let actions = list(&["inline-reply", "default", "open"]);

        let (buttons, has_default, has_reply) = read_actions(&actions, true);
        assert_eq!(buttons.len(), 1);
        assert!(has_default && has_reply);

        let (buttons, _, has_reply) = read_actions(&actions, false);
        assert_eq!(buttons.len(), 2);
        assert!(!has_reply);
    }
}
