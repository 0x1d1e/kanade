use std::time::{Duration, SystemTime};

use super::super::rules::Effects;
use crate::sources::sound::Hints;

#[derive(Debug, Clone)]
pub struct Notification {
    pub(crate) id: u32,

    pub(crate) app_name: String,

    // the `desktop-entry` hint, like "org.gnome.Nautilus", and the `category` hint, like
    // "im.received"; empty when the sender gives none
    pub(crate) desktop_entry: String,

    pub(crate) category: String,

    pub(crate) summary: String,

    pub(crate) body: String,

    pub(crate) icon: String,

    pub(crate) image: String,

    pub(crate) urgency: Urgency,

    // the "default" action is kept apart, it has no button of its own
    pub(crate) actions: Vec<Action>,

    pub(crate) has_default: bool,

    // the placeholder of the reply field when the sender asked for an inline reply (the KDE
    // "inline-reply" action; ADR 0079), empty for the default one
    pub(crate) reply: Option<String>,

    // the sender's unique bus name, who hears the reply; empty when unknown
    pub(crate) sender: String,

    // stays open after an action, until it is dismissed or the sender closes it
    pub(crate) resident: bool,

    // what its sender asked for as sound
    pub(crate) sound: Hints,

    // the `value` hint: how far along it is, 0 to 100, for a progress Live Activity (ADR 0059)
    pub(crate) value: Option<u8>,

    /*
     * `expire_timeout` as the sender gave it: none when it left the time to the server (-1),
     * `Duration::ZERO` when it asked for the notification never to expire, and its time otherwise
     */
    pub(crate) timeout: Option<Duration>,

    pub(crate) received: SystemTime,

    // which version of the id this is: each replacement gets a new one, never reused
    pub(crate) version: u64,

    // what the rule that matched it as it arrived does, its urgency already applied
    pub(crate) effects: Effects,
}

// a button a notification asks for, like "reply" or "open"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub(crate) key: String,

    pub(crate) label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Urgency {
    Low,

    // what a notification without the hint gets
    #[default]
    Normal,

    Critical,
}

impl Notification {
    // pass this to Notifications::dismiss
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn app_name(&self) -> &str {
        &self.app_name
    }

    // the `desktop-entry` hint, empty when the sender gives none
    pub fn desktop_entry(&self) -> &str {
        &self.desktop_entry
    }

    // the `category` hint, empty when the sender gives none
    pub fn category(&self) -> &str {
        &self.category
    }

    // the one-line title
    pub fn summary(&self) -> &str {
        &self.summary
    }

    // may be empty, and may hold simple markup like <b>
    pub fn body(&self) -> &str {
        &self.body
    }

    // an icon name like "firefox", a file path, or empty
    pub fn icon(&self) -> &str {
        &self.icon
    }

    // a picture for this one notification, like a sender's avatar: a file path, a file url, or empty
    pub fn image(&self) -> &str {
        &self.image
    }

    pub fn urgency(&self) -> Urgency {
        self.urgency
    }

    // the buttons to show, in the order the sender gave them
    pub fn actions(&self) -> &[Action] {
        &self.actions
    }

    // whether the sender takes a typed reply, and the field's placeholder, see Notifications::reply
    pub fn reply(&self) -> Option<&str> {
        self.reply.as_deref()
    }

    // whether clicking the notification itself does something, see Notifications::click
    pub fn has_default_action(&self) -> bool {
        self.has_default
    }

    pub fn sound(&self) -> &Hints {
        &self.sound
    }

    pub fn effects(&self) -> &Effects {
        &self.effects
    }

    // how far along the sender says it is, in percent
    pub fn value(&self) -> Option<u8> {
        self.value
    }

    // how long the sender asked for it to show, `Duration::ZERO` when it asked never, and none
    // when it left that to the server
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    // when it arrived, or when the sender last replaced it
    pub fn received(&self) -> SystemTime {
        self.received
    }
}

impl Action {
    // pass this to Notifications::invoke
    pub fn key(&self) -> &str {
        &self.key
    }

    // the text to show on the button
    pub fn label(&self) -> &str {
        &self.label
    }
}

impl Urgency {
    // the spec sends urgency as a byte: 0 low, 1 normal, 2 critical
    pub(crate) fn from_level(level: f64) -> Self {
        match level as u8 {
            0 => Self::Low,
            2 => Self::Critical,
            _ => Self::Normal,
        }
    }
}
