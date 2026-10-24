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

// the history's bounds (docs/archived/design.md): this many of one app, and this many in all
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

    /*
     * a reply typed on a notification that asked for one (ADR 0079): told to its sender alone,
     * then closed unless it is resident. The text is cut to `REPLY_LIMIT` characters, kept
     * nowhere and never logged. False when there is nothing to reply to, text empty or no field
     */
    pub fn reply(id: u32, text: &str) -> bool {
        Self::write().send_reply(id, text)
    }

    fn send_reply(&mut self, id: u32, text: &str) -> bool {
        let text: String = text.trim().chars().take(server::REPLY_LIMIT).collect();

        if text.is_empty() {
            return false;
        }

        // the bus name that created the notification, which a replacement by another one never
        // takes over: a reply goes to it alone
        let Some((sender, resident)) = self
            .list
            .iter()
            .find(|notification| notification.id == id)
            .filter(|notification| notification.reply.is_some() && !notification.sender.is_empty())
            .map(|notification| (notification.sender.clone(), notification.resident))
        else {
            return false;
        };

        server::announce_reply(&sender, id, &text);

        if !resident {
            self.close(id, Reason::Dismissed);
        }

        true
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

    /*
     * a sender may replace its own earlier notification, which keeps its id and place. Anyone on
     * the bus can name an id, so a replacement by another sender never becomes the one a reply is
     * sent to: the creator's bus name stays, and with it the field, unless the replacement itself
     * asks for one
     */
    fn add(&mut self, mut notification: Notification, replaces_id: u32) -> u32 {
        notification.version = self.next_version;
        self.next_version += 1;

        if let Some(old) = self.list.iter_mut().find(|old| old.id == replaces_id) {
            notification.id = replaces_id;

            if notification.sender != old.sender {
                notification.sender = old.sender.clone();
                notification.reply = None;
            }

            image_hint::remove(&old.image);

            let app = notification.app_name.clone();

            *old = notification;

            // a replacement may name another app, so the bounds are of the app it names now
            self.trim(&app, replaces_id);

            return replaces_id;
        }

        let id = self.next_id;

        self.next_id += 1;

        notification.id = id;

        let app = notification.app_name.clone();

        self.list.push(notification);

        self.trim(&app, id);

        id
    }

    // a notification past the bounds closes as expired, the oldest of its app first
    fn trim(&mut self, app: &str, id: u32) {
        for old in oldest(&self.list, app, id) {
            self.close(old, Reason::Expired);
        }
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
            reply: None,
            sender: String::new(),
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

    // a reply reaches only a notification that asked for one and whose sender is known, is never
    // empty, and closes it unless it is resident
    #[test]
    fn a_reply_needs_a_field_a_sender_and_text() {
        let mut notifications = Notifications::new();
        let replying = |resident| Notification {
            reply: Some(String::new()),
            sender: String::from(":1.5"),
            resident,
            ..notification(true)
        };

        let plain = notifications.add(notification(true), 0);
        let anonymous = notifications.add(
            Notification {
                sender: String::new(),
                ..replying(false)
            },
            0,
        );
        let asking = notifications.add(replying(false), 0);
        let staying = notifications.add(replying(true), 0);

        assert!(!notifications.send_reply(plain, "hi"));
        assert!(!notifications.send_reply(anonymous, "hi"));
        assert!(!notifications.send_reply(asking, " \n "));
        assert!(!notifications.send_reply(99, "hi"));

        assert!(notifications.send_reply(asking, "hi"));
        assert!(notifications.send_reply(staying, "hi"));

        let ids: Vec<u32> = notifications.list.iter().map(|n| n.id).collect();
        assert_eq!(ids, [plain, anonymous, staying]);
    }

    /*
     * a reply goes to the one bus name that created the notification. Ids are small and guessable,
     * so any client may replace one by naming its id; the text the user typed is never sent to it
     */
    #[test]
    fn a_reply_is_never_sent_to_a_sender_that_replaced_the_notification() {
        let mut notifications = Notifications::new();
        let asking = |sender: &str| Notification {
            app_name: String::from("Chat"),
            reply: Some(String::new()),
            sender: String::from(sender),
            ..notification(true)
        };

        let id = notifications.add(asking(":1.5"), 0);
        notifications.add(asking(":1.9"), id);

        // the creator's bus name stands, so that is who hears it
        assert_eq!(
            notifications
                .list
                .iter()
                .find(|notification| notification.id == id)
                .map(|notification| notification.sender.as_str()),
            Some(":1.5")
        );

        // and a replacement that takes no reply takes the field with it, so there is nothing to
        // send the draft to at all
        notifications.add(notification(true), id);
        assert!(!notifications.send_reply(id, "hi"));
    }

    // a replacement may name another app, and the bound is of the app it names now
    #[test]
    fn a_replacement_by_another_app_keeps_the_per_app_bound() {
        let mut notifications = Notifications::new();

        let mut ids = Vec::new();

        for app in ["a", "b"] {
            for _ in 0..PER_APP {
                ids.push(notifications.add(
                    Notification {
                        app_name: String::from(app),
                        ..notification(true)
                    },
                    0,
                ));
            }
        }

        assert_eq!(notifications.list.len(), 2 * PER_APP);

        // each of `a`'s is replaced by one naming `b`: `b` never grows past its bound
        for id in &ids[..PER_APP] {
            notifications.add(
                Notification {
                    app_name: String::from("b"),
                    ..notification(true)
                },
                *id,
            );

            let of_b = notifications
                .list
                .iter()
                .filter(|notification| notification.app_name == "b")
                .count();

            assert!(of_b <= PER_APP, "{of_b} of b");
        }

        assert!(notifications.list.len() <= 2 * PER_APP);
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
