//! Notification toasts (plan 3, 5.1, 7, ADR 0007): a notification that arrives, or is replaced,
//! shows as a Banner on the focused output, never on the island. The Banner goes away; the
//! notification stays in the daemon's list, the history, until it is dismissed or its sender closes
//! it, which also takes its Banner down.
//!
//! DND (`notifications dnd toggle`) silences Banners, never the history. Banners never take the
//! keyboard.
//!
//! `Notifications` has no subscription, so this reads it again when the bus carries a
//! notification arriving or closing (`wake`), and polls while that settles or the bus cannot be
//! watched; a read that finds the same notifications shows nothing. Kanade is the daemon only if no
//! other one, like mako, got the bus name first. Then nothing arrives, and `Daemon` says who has it
//! for the Notifications Surface.

use std::process;
use std::time::{Duration, Instant, SystemTime};

use crate::bus::Bus;
use kanade_runtime::service::Service;

use super::apps::Apps;
use super::bus;
use super::wake::{Announcer, Pace, Wakes};
use crate::banners::{Banner, Banners};
use crate::island::activity::Toast;
use crate::island::service::IslandService;
use crate::{banners, modules, supervise};

mod daemon;

pub use daemon::{Notification, Notifications, Urgency};

const PACE: Pace = Pace {
    // a Banner shows within this of arriving
    poll: Duration::from_millis(100),

    // the daemon takes in a notification on its own thread, maybe after the bus carried it
    settle: Duration::from_secs(1),

    idle: None,
};

// the bus carries each notification arriving, closed by its sender, or closed by Kanade, which
// is every change to the daemon's list
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
    // the daemon has not got the bus name yet
    #[default]
    Starting,

    Running,

    // another daemon has the bus name, named by its process like "mako"; Kanade never asks again
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

    // `owner` is the process that has the bus name, asked only while the daemon is not running
    fn of(running: bool, owner: impl FnOnce() -> Option<u32>) -> Daemon {
        if running {
            return Daemon::Running;
        }

        // the daemon owns it and is about to say so, or no one does yet
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

// a file path or file url Kanade can draw; an icon theme name is none
fn local(path: &str) -> Option<String> {
    let path = path.strip_prefix("file://").unwrap_or(path);

    path.starts_with('/').then(|| path.to_owned())
}

/*
 * the icon of the app that sent it, by the icon name it gave or else its name, from the desktop
 * entries `apps` read; none while it is still reading them
 */
fn app_icon(notification: &Notification) -> Option<String> {
    let apps = Apps::read();

    let app = apps
        .list()
        .iter()
        .find(|app| {
            !notification.icon().is_empty() && app.icon.as_deref() == Some(notification.icon())
        })
        .or_else(|| {
            apps.list()
                .iter()
                .find(|app| app.name.eq_ignore_ascii_case(notification.app_name()))
        })?;

    Some(app.icon_file.as_ref()?.to_string_lossy().into_owned())
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
            // the first read starts the daemon
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

            // until the daemon has the bus name, nothing it would announce can arrive
            let busy = changed || daemon == Daemon::Starting;
            before = now;
            wakes.wait(busy);
        }
    });
}
