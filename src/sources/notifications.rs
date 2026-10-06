//! Notification toasts (plan 3, 5.1, 7, ADR 0007): a notification that arrives, or is replaced,
//! shows as a Banner on the focused output, never on the island. The Banner goes away; the
//! notification stays in Amane's list, the history, until it is dismissed or its sender closes it,
//! which also takes its Banner down.
//!
//! DND (`notifications dnd toggle`) silences Banners, never the history. Banners never take the
//! keyboard.
//!
//! Amane's Notifications has no subscription, so this reads it again when the bus carries a
//! notification arriving or closing (`wake`), and polls while that settles or the bus cannot be
//! watched; a read that finds the same notifications shows nothing. Amane is the daemon only if no
//! other one, like mako, got the bus name first. Then nothing arrives, and `Daemon` says who has it
//! for the Notifications Surface.

use std::process;
use std::time::{Duration, Instant, SystemTime};

use amane::{Apps, Bus, Notification, Notifications, Service};

use super::bus;
use super::wake::{Announcer, Pace, Wakes};
use crate::banners::{Banner, Banners};
use crate::island::activity::Toast;
use crate::island::service::IslandService;
use crate::{banners, modules, supervise};

const PACE: Pace = Pace {
    // a Banner shows within this of arriving
    poll: Duration::from_millis(100),

    // Amane takes in a notification on its own thread, maybe after the bus carried it
    settle: Duration::from_secs(1),

    idle: None,
};

// the bus carries each notification arriving, closed by its sender, or closed by Amane, which
// is every change to Amane's list
pub const BUS: Announcer = Announcer {
    program: "dbus-monitor",
    args: &[
        "--session",
        "--profile",
        "interface='org.freedesktop.Notifications'",
    ],
    announces,
};

// the bus name a notification daemon owns
pub const NAME: &str = "org.freedesktop.Notifications";

// whether Kanade is the notification daemon, for the Notifications Surface's error state (plan 7)
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Daemon {
    // Amane has not got the bus name yet
    #[default]
    Starting,

    Running,

    // another daemon has the bus name, named by its process like "mako"; Amane never asks again
    Conflict(String),
}

// written only by `follow`, read by views
impl Service for Daemon {
    fn new() -> Self {
        Daemon::Starting
    }

    fn listen() {}
}

impl Daemon {
    // for `kanade status`
    pub fn status(&self) -> String {
        match self {
            Daemon::Starting => String::from("notification daemon: starting"),
            Daemon::Running => String::from("notification daemon: Kanade"),
            Daemon::Conflict(other) => {
                format!("notification daemon: {other}, not Kanade; stop it and restart Kanade")
            }
        }
    }

    // `owner` is the process that has the bus name, asked only while Amane is not running
    fn of(running: bool, owner: impl FnOnce() -> Option<u32>) -> Daemon {
        if running {
            return Daemon::Running;
        }

        // Amane owns it and is about to say so, or no one does yet
        match owner() {
            Some(pid) if pid != process::id() => Daemon::Conflict(bus::process(pid)),
            _ => Daemon::Starting,
        }
    }
}

// one version of a notification: a replacement keeps the id and gets a new time
type Version = (u32, SystemTime);

/*
 * the versions in `now` that were not in `before`, to show, and the ids gone from it, to close. A
 * replaced notification is a new version of the same id, so it replaces its Banner and shows it
 * for a full time again
 */
fn changes(before: &[Version], now: &[Version]) -> (Vec<Version>, Vec<u32>) {
    let posted = now
        .iter()
        .filter(|version| !before.contains(version))
        .copied()
        .collect();

    let gone = before
        .iter()
        .map(|&(id, _)| id)
        .filter(|id| !now.iter().any(|(other, _)| other == id))
        .collect();

    (posted, gone)
}

pub(crate) fn toast(notification: &Notification) -> Toast {
    let summary = match plain(notification.summary()) {
        summary if summary.is_empty() => notification.app_name().to_owned(),
        summary => summary,
    };

    Toast {
        app: notification.app_name().to_owned(),
        summary,
        body: plain(notification.body()),
        image: local(notification.image())
            .or_else(|| local(notification.icon()))
            .or_else(|| app_icon(notification)),
    }
}

/*
 * the body's simple markup as one line of text: tags dropped, the five escapes the spec allows read
 * back, line breaks and runs of spaces made one space
 */
fn plain(markup: &str) -> String {
    let mut text = String::with_capacity(markup.len());
    let mut tag = false;

    for character in markup.chars() {
        match character {
            '<' => tag = true,
            '>' if tag => tag = false,
            _ if tag => {}
            character => text.push(character),
        }
    }

    let text = text
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&");

    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

// a file path or file url Amane can draw; an icon theme name is none
fn local(path: &str) -> Option<String> {
    let path = path.strip_prefix("file://").unwrap_or(path);

    path.starts_with('/').then(|| path.to_owned())
}

/*
 * the icon of the app that sent it, by the icon name it gave or else its name, from the desktop
 * entries Amane read; none while it is still reading them
 */
fn app_icon(notification: &Notification) -> Option<String> {
    let apps = Apps::read();

    let app = apps
        .list()
        .iter()
        .find(|app| !notification.icon().is_empty() && app.icon() == Some(notification.icon()))
        .or_else(|| {
            apps.list()
                .iter()
                .find(|app| app.name().eq_ignore_ascii_case(notification.app_name()))
        })?;

    Some(app.icon_path()?.to_string_lossy().into_owned())
}

/*
 * whether a line of `dbus-monitor --profile` announces a change to the notifications: tab separated
 * type, time, serial, sender, destination, path, interface and member, after a header and the
 * monitor's own NameAcquired and NameLost
 */
fn announces(line: &str) -> bool {
    let columns: Vec<&str> = line.split('\t').collect();

    matches!(
        columns.as_slice(),
        ["mc" | "sig", _, _, _, _, _, NAME, member]
            if ["Notify", "CloseNotification", "NotificationClosed"].contains(member)
    )
}

// DND quiets the Banners, never the history
pub fn set_dnd(dnd: bool, now: Instant) {
    IslandService::write().set_dnd(dnd, now);

    if dnd {
        banners::silence(now);
    }
}

// runs on its own thread for good
pub fn follow() {
    let mut wakes = Wakes::new(PACE, vec![BUS]);
    let mut before: Vec<Version> = Vec::new();
    let mut daemon = Daemon::Starting;
    let shows_banners = modules::on("banners");

    supervise::run("notifications", || {
        loop {
            // the first read starts Amane's daemon
            let notifications = Notifications::read();
            let running = notifications.running();

            let now: Vec<Version> = notifications
                .list()
                .iter()
                .map(|notification| (notification.id(), notification.received()))
                .collect();

            let (posted, gone) = changes(&before, &now);
            let changed = !posted.is_empty() || !gone.is_empty();

            // read here, so they are of the same list
            let arrived: Vec<&Notification> = notifications
                .list()
                .iter()
                .filter(|notification| {
                    posted.contains(&(notification.id(), notification.received()))
                })
                .collect();

            let shown: Vec<Banner> = if shows_banners {
                arrived
                    .iter()
                    .map(|notification| Banner::of(notification))
                    .collect()
            } else {
                Vec::new()
            };

            drop(notifications);

            // Running and Conflict are for good, so only Starting asks again
            if daemon == Daemon::Starting {
                let next = Daemon::of(running, || bus::owner(Bus::session(), NAME));

                if let Daemon::Conflict(other) = &next {
                    eprintln!(
                        "kanade: {other} is the notification daemon, stop it and restart Kanade to get notifications"
                    );
                }

                if next != daemon {
                    *Daemon::write() = next.clone();
                    daemon = next;
                }
            }

            let dnd = IslandService::read().dnd();

            /*
             * a Banner DND drops that replaces none, or one already put away, changes nothing, so
             * neither writes for nothing
             */
            let (shown, closed): (Vec<Banner>, Vec<u32>) = if shows_banners {
                let banners = Banners::read();

                (
                    shown
                        .into_iter()
                        .filter(|banner| !dnd || banner.critical() || banners.contains(banner.id))
                        .collect(),
                    gone.iter()
                        .copied()
                        .filter(|id| banners.contains(*id))
                        .collect(),
                )
            } else {
                (Vec::new(), Vec::new())
            };

            if !shown.is_empty() || !closed.is_empty() {
                let now = Instant::now();
                let mut banners = Banners::write();

                for banner in shown {
                    banners.arrive(banner, dnd, now);
                }

                for id in closed {
                    banners.close(id, now);
                }
            }

            // until Amane has the bus name, nothing it would announce can arrive
            let busy = changed || daemon == Daemon::Starting;
            before = now;
            wakes.wait(busy);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names_the_daemon() {
        assert_eq!(Daemon::Running.status(), "notification daemon: Kanade");
        assert_eq!(
            Daemon::Conflict(String::from("mako")).status(),
            "notification daemon: mako, not Kanade; stop it and restart Kanade"
        );
    }

    // as `dbus-monitor --profile` prints them
    #[test]
    fn only_notifications_arriving_or_closing_announce() {
        let line = |kind: &str, interface: &str, member: &str| {
            format!(
                "{kind}\t1791255388.49\t9\t:1.165\t:1.162\t/org/freedesktop/Notifications\t{interface}\t{member}"
            )
        };

        assert!(announces(&line("mc", NAME, "Notify")));
        assert!(announces(&line("mc", NAME, "CloseNotification")));
        assert!(announces(&line("sig", NAME, "NotificationClosed")));

        assert!(!announces(&line("mc", NAME, "GetServerInformation")));
        assert!(!announces(&line("mr", NAME, "Notify")));
        assert!(!announces(&line(
            "sig",
            "org.freedesktop.DBus",
            "NameAcquired"
        )));
        assert!(!announces(
            "#type\ttimestamp\tserial\tsender\tdestination\tpath\tinterface\tmember"
        ));
    }

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn a_new_notification_shows_and_a_closed_one_closes() {
        let one = [(1, at(1))];
        let two = [(1, at(1)), (2, at(2))];

        assert_eq!(changes(&[], &one), (vec![(1, at(1))], vec![]));
        assert_eq!(changes(&one, &two), (vec![(2, at(2))], vec![]));
        assert_eq!(changes(&two, &[(2, at(2))]), (vec![], vec![1]));
    }

    #[test]
    fn the_same_list_shows_nothing() {
        let list = [(1, at(1)), (2, at(2))];

        assert_eq!(changes(&list, &list), (vec![], vec![]));
    }

    // a replacement keeps its id, so it replaces the Banner instead of closing it
    #[test]
    fn a_replaced_notification_shows_again() {
        assert_eq!(
            changes(&[(1, at(1))], &[(1, at(5))]),
            (vec![(1, at(5))], vec![])
        );
    }

    #[test]
    fn markup_becomes_one_line_of_text() {
        assert_eq!(plain("<b>Hello</b>\n  <i>world</i>"), "Hello world");
        assert_eq!(
            plain("a &lt;tag&gt; &amp; &quot;q&quot; &apos;s"),
            "a <tag> & \"q\" 's"
        );
        assert_eq!(plain(r#"<a href="x">link</a>"#), "link");
        assert_eq!(plain(""), "");
    }

    #[test]
    fn only_local_files_are_images() {
        assert_eq!(local("/tmp/a.png"), Some(String::from("/tmp/a.png")));
        assert_eq!(local("file:///tmp/a.png"), Some(String::from("/tmp/a.png")));
        assert_eq!(local("firefox"), None);
        assert_eq!(local(""), None);
    }

    #[test]
    fn another_process_with_the_name_is_a_conflict() {
        assert_eq!(Daemon::of(true, || panic!("not asked")), Daemon::Running);
        assert_eq!(Daemon::of(false, || None), Daemon::Starting);
        assert_eq!(Daemon::of(false, || Some(process::id())), Daemon::Starting);
        assert!(matches!(
            Daemon::of(false, || Some(1)),
            Daemon::Conflict(name) if !name.is_empty()
        ));
    }
}
