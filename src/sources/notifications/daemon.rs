//! The notification daemon (ADR 0027 step 1): Kanade owns `org.freedesktop.Notifications` and
//! keeps the list, the history, that `notifications` follows.

mod image_hint;
mod notification;
mod server;

pub use notification::{Action, Notification, Urgency};

use kanade_runtime::service::Service;

pub struct Notifications {
    list: Vec<Notification>,

    // the spec keeps 0 for "no notification", so ids start at 1
    next_id: u32,

    // the version the next notification or replacement gets
    next_version: u64,

    running: bool,
}

// the history's bounds (docs/design.md): this many of one app, and this many in all
const PER_APP: usize = 20;
const TOTAL: usize = 100;

/*
 * the ids to close for the bounds, the oldest received first, never `new`, which was just added: of
 * `app` past `PER_APP`, then of all past `TOTAL`
 */
fn oldest(list: &[Notification], app: &str, new: u32) -> Vec<u32> {
    let mut oldest: Vec<&Notification> = list
        .iter()
        .filter(|notification| notification.id != new)
        .collect();
    oldest.sort_by_key(|notification| notification.received);

    let of_app = oldest
        .iter()
        .filter(|notification| notification.app_name == app);
    let over_app = (of_app.clone().count() + 1).saturating_sub(PER_APP);
    let mut closing: Vec<u32> = of_app.take(over_app).map(|n| n.id).collect();

    let over_all = (list.len() - closing.len()).saturating_sub(TOTAL);

    let rest: Vec<u32> = oldest
        .iter()
        .filter(|notification| !closing.contains(&notification.id))
        .take(over_all)
        .map(|notification| notification.id)
        .collect();
    closing.extend(rest);

    closing
}

// the numbers the spec gives each way a notification can close
#[derive(Clone, Copy)]
pub enum Reason {
    // its rule keeps it out of the history, and its Banner is gone or never showed
    Expired = 1,

    Dismissed = 2,

    // the sender asked with CloseNotification
    Closed = 3,
}

// Kanade is the notification server here: programs send their notifications to it
impl Service for Notifications {
    fn new() -> Self {
        Self {
            list: Vec::new(),
            next_id: 1,
            next_version: 1,
            running: false,
        }
    }

    fn listen() {
        server::run();
    }
}

impl Notifications {
    // oldest first
    pub fn list(&self) -> &[Notification] {
        &self.list
    }

    // false when another daemon like mako or dunst already owns the name
    pub fn running(&self) -> bool {
        self.running
    }

    pub fn dismiss(id: u32) {
        Self::write().close(id, Reason::Dismissed);
    }

    // those of a stack, in one write
    pub fn dismiss_all(ids: &[u32]) {
        let mut notifications = Self::write();

        for &id in ids {
            notifications.close(id, Reason::Dismissed);
        }
    }

    pub fn clear() {
        let mut notifications = Self::write();

        let mut ids = Vec::new();

        for notification in &notifications.list {
            ids.push(notification.id);
        }

        for id in ids {
            notifications.close(id, Reason::Dismissed);
        }
    }

    /*
     * closes `version` of `id` when its rule keeps it out of the history; a newer version, or one
     * gone, stays as it is. Read first, as a write wakes every reader, then checked again under the
     * write that closes it, so a replacement in between is never closed
     */
    pub fn expire(id: u32, version: u64) {
        if !Self::read().expires(id, version) {
            return;
        }

        let mut notifications = Self::write();
        if notifications.expires(id, version) {
            notifications.close(id, Reason::Expired);
        }
    }

    fn expires(&self, id: u32, version: u64) -> bool {
        self.list.iter().any(|notification| {
            notification.id == id
                && notification.version == version
                && !notification.effects.history
        })
    }

    // a button was pressed, pass the key from notification.actions()
    pub fn invoke(id: u32, key: &str) {
        Self::write().run_action(id, key);
    }

    // the notification itself was clicked: its default action, or a dismiss when it has none
    pub fn click(id: u32) {
        let mut notifications = Self::write();

        let has_default = notifications
            .list
            .iter()
            .any(|notification| notification.id == id && notification.has_default);

        if has_default {
            notifications.run_action(id, server::DEFAULT_ACTION);
        } else {
            notifications.close(id, Reason::Dismissed);
        }
    }

    // the spec closes a notification once an action ran, unless it asked to stay
    fn run_action(&mut self, id: u32, key: &str) {
        let Some(notification) = self.list.iter().find(|notification| notification.id == id) else {
            return;
        };

        let resident = notification.resident;

        server::announce_action(id, key);

        if !resident {
            self.close(id, Reason::Dismissed);
        }
    }

    // a sender may replace its own earlier notification, which keeps its id and place
    fn add(&mut self, mut notification: Notification, replaces_id: u32) -> u32 {
        notification.version = self.next_version;
        self.next_version += 1;

        let replaced = self.list.iter_mut().find(|old| old.id == replaces_id);

        if let Some(old) = replaced {
            notification.id = replaces_id;

            image_hint::remove(&old.image);

            *old = notification;

            return replaces_id;
        }

        let id = self.next_id;

        self.next_id += 1;

        notification.id = id;

        let app = notification.app_name.clone();

        self.list.push(notification);

        // a notification past the bounds closes as expired, the oldest of its app first
        for old in oldest(&self.list, &app, id) {
            self.close(old, Reason::Expired);
        }

        id
    }

    fn close(&mut self, id: u32, reason: Reason) {
        let Some(index) = self
            .list
            .iter()
            .position(|notification| notification.id == id)
        else {
            return;
        };

        let closed = self.list.remove(index);

        image_hint::remove(&closed.image);

        server::announce_closed(id, reason);
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::*;
    use crate::sources::notifications::rules::Effects;
    use crate::sources::sound::Hints;

    fn notification(history: bool) -> Notification {
        Notification {
            id: 0,
            app_name: String::new(),
            desktop_entry: String::new(),
            category: String::new(),
            summary: String::new(),
            body: String::new(),
            icon: String::new(),
            image: String::new(),
            urgency: Urgency::Normal,
            actions: Vec::new(),
            has_default: false,
            resident: false,
            sound: Hints::default(),
            value: None,
            timeout: None,
            received: SystemTime::UNIX_EPOCH,
            version: 0,
            effects: Effects {
                history,
                ..Effects::default()
            },
        }
    }

    fn version(notifications: &Notifications, id: u32) -> u64 {
        notifications
            .list
            .iter()
            .find(|notification| notification.id == id)
            .unwrap()
            .version
    }

    // the history is bounded, so a burst from one app or many never grows it without end
    #[test]
    fn the_history_keeps_20_of_an_app_and_100_in_all() {
        let from = |id: u32, app: &str| Notification {
            id,
            app_name: app.to_owned(),
            received: SystemTime::UNIX_EPOCH + Duration::from_secs(u64::from(id)),
            ..notification(true)
        };

        let one_app: Vec<_> = (1..=21).map(|id| from(id, "a")).collect();
        assert_eq!(oldest(&one_app, "a", 21), vec![1]);
        assert!(oldest(&one_app[..20], "a", 20).is_empty());

        let many: Vec<_> = (1..=101).map(|id| from(id, &id.to_string())).collect();
        assert_eq!(oldest(&many, "101", 101), vec![1]);
        assert!(oldest(&many[..100], "100", 100).is_empty());
    }

    // an expiry of a version a replacement took over closes nothing, whatever the new one's rule
    #[test]
    fn a_stale_expiry_never_closes_a_replacement() {
        let mut notifications = Notifications::new();

        let id = notifications.add(notification(false), 0);
        let old = version(&notifications, id);
        assert!(notifications.expires(id, old));

        // replaced by one kept in the history
        notifications.add(notification(true), id);
        let kept = version(&notifications, id);
        assert!(!notifications.expires(id, old));
        assert!(!notifications.expires(id, kept));

        // and by one kept out of it: only its own expiry closes it
        notifications.add(notification(false), id);
        let unkept = version(&notifications, id);
        assert!(!notifications.expires(id, old));
        assert!(notifications.expires(id, unkept));
    }
}
