//! Notification toasts (ADR 0007): a notification that arrives, or is replaced,
//! shows as a Banner on the focused output, never on the island. The Banner goes away; the
//! notification stays in the daemon's list, the history, until it is dismissed or its sender closes
//! it, which also takes its Banner down.
//!
//! DND (`notifications dnd toggle`) silences Banners, never the history, for as long as it is on or
//! for a time it is given (`timed`). Banners never take the keyboard.
//!
//! A notification's rule (`rules`) may show it with no Banner or sound, under DND, for its own time,
//! or keep it out of the history: then it closes as expired once its Banner is gone, or at once with none.
//!
//! With `notifications.sound` on, a new notification plays a sound (`sound`), never under DND.
//!
//! A notification with a `value` hint under 100 is also a progress Live Activity (`live`, ADR 0059),
//! while its rule lets a Banner show and, under DND, only a loud one: it ends when the value reaches
//! 100 and goes when the notification closes or is replaced with no value.
//!
//! `Notifications` has no subscription, so this reads it again when the bus carries a
//! notification arriving or closing (`wake`), and polls while that settles or the bus cannot be
//! watched; a read that finds the same notifications shows nothing. Kanade is the daemon only if no
//! other one, like mako, got the bus name first. Then nothing arrives, and `Daemon` says who has it
//! for the Notifications Surface.

use std::collections::HashMap;
use std::process;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::bus::Bus;
use kanade_runtime::service::Service;

use super::apps::Apps;
use super::bus;
use super::icons;
use super::live;
use super::sound::{self, Hints};
use super::wake::{Announcer, Pace, Waker, Wakes};
use crate::banners::{Banner, Banners};
use crate::config;
use crate::hooks::{self, Event};
use crate::island::activity::Toast;
use crate::island::service::IslandService;
use crate::{banners, modules, supervise};

mod daemon;
pub mod rules;
pub mod timed;

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

// whether Kanade is the notification daemon, for the Notifications Surface's error state
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

// one version of a notification: a replacement keeps the id and gets a new version
type Version = (u32, u64);

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

// what a notification's `value` hint does to its progress Live Activity (ADR 0059)
#[derive(Debug, PartialEq, Eq)]
enum Step {
    // shows it, started when it is not live; `moved` is the live id it had under another app name,
    // to take away first
    Show {
        id: String,
        moved: Option<String>,
        title: String,
        detail: String,
        percent: u8,
        lasts: Option<Duration>,
    },

    // the value reached 100: ends as done
    Finish(String),

    // the notification no longer shows progress: takes it away
    Withdraw(String),
}

/*
 * the live id of a notification's progress: its app and its id, which is the sender's replaces_id
 * for every version of it. The app is cut to letters and digits, so the id fits `live`'s alphabet.
 * A replacement may name another app, so a live id in use is kept in `tracked`, never derived again
 */
fn live_id(notification: &Notification) -> String {
    let app: String = notification
        .app_name()
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(32)
        .collect::<String>()
        .to_ascii_lowercase();

    format!("{}{app}.{}", live::NOTIFICATION, notification.id())
}

/*
 * the step for a notification that arrived or was replaced: `allowed` says its rule, Do Not
 * Disturb and the `live` Module let it show, `tracked` the live id its progress has, if any. A value
 * under 100 shows while allowed; 100 ends it; none, or no longer allowed, takes it away. All three
 * act on the tracked id, which a replacement under another app name does not change
 */
fn progress(notification: &Notification, allowed: bool, tracked: Option<&str>) -> Option<Step> {
    let id = live_id(notification);

    match notification.value() {
        Some(percent @ 0..100) if allowed => {
            let (summary, body) = (plain(notification.summary()), plain(notification.body()));
            let app = notification.app_name();

            Some(Step::Show {
                moved: tracked.filter(|old| *old != id).map(str::to_owned),
                id,
                title: if summary.is_empty() {
                    app.to_owned()
                } else {
                    summary
                },
                detail: if body.is_empty() {
                    app.to_owned()
                } else {
                    body
                },
                percent,
                lasts: notification.timeout(),
            })
        }
        Some(100..) => tracked.map(|old| Step::Finish(old.to_owned())),
        _ => tracked.map(|old| Step::Withdraw(old.to_owned())),
    }
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
 * entries `apps` read; none while it is still reading them. Kanade's own notifications, which
 * have no desktop entry, take the `kanade` icon of the theme (`dist/kanade.png`)
 */
fn app_icon(notification: &Notification) -> Option<String> {
    let apps = Apps::read();

    let entry = apps
        .list()
        .iter()
        .find(|app| {
            !notification.icon().is_empty() && app.icon.as_deref() == Some(notification.icon())
        })
        .or_else(|| {
            apps.list()
                .iter()
                .find(|app| app.name.eq_ignore_ascii_case(notification.app_name()))
        })
        .and_then(|app| app.icon_file.clone());

    entry
        .or_else(|| {
            notification
                .app_name()
                .eq_ignore_ascii_case("kanade")
                .then(icons::own_icon)
                .flatten()
        })
        .map(|file| file.to_string_lossy().into_owned())
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

// wakes `follow` for a change of DND, which no announcement on the bus carries
static WAKER: OnceLock<Waker> = OnceLock::new();

// how many times DND has been set since Kanade started, whether it changed or not
static DND_REVISION: AtomicU64 = AtomicU64::new(0);

/*
 * which setting of DND is the last, for what turns it on and later wants to know whether it is still
 * as it left it: a user who sets it again is told from no one having touched it
 */
pub fn dnd_revision() -> u64 {
    DND_REVISION.load(Ordering::Relaxed)
}

/*
 * DND quiets the Banners, never the history, and the progress Live Activities it does not let show.
 * Any setting of it ends a timed DND's timer (`timed`): the timer ends only what it turned on
 */
pub fn set_dnd(dnd: bool, now: Instant) {
    let _timer = timed::cancel();

    apply(dnd, now);
}

// sets DND, with the end of a timed one already dealt with by the caller, which holds its lock
fn apply(dnd: bool, now: Instant) {
    // a hook (ADR 0055) tells of a change, not of a setting that left it as it was
    let changed = IslandService::read().dnd() != dnd;

    DND_REVISION.fetch_add(1, Ordering::Relaxed);
    IslandService::write().set_dnd(dnd, now);

    if dnd {
        banners::silence(now);
    }

    if let Some(waker) = WAKER.get() {
        waker.wake();
    }

    if changed {
        hooks::fire_with(Event::Dnd, if dnd { "on" } else { "off" });
    }
}

/*
 * the progress steps for one read: for each of `candidates` that the step is wanted of, by id. The
 * ones that arrived or were replaced are the candidates, and every notification in the list when DND
 * changed, as that alone moves a progress that did not
 */
fn steps(
    list: &[Notification],
    posted: &[Version],
    dnd_changed: bool,
    dnd: bool,
    tracked: &HashMap<u32, String>,
) -> Vec<(u32, Step)> {
    list.iter()
        .filter(|notification| {
            dnd_changed || posted.contains(&(notification.id(), notification.version()))
        })
        .filter_map(|notification| {
            // a progress shows as its rule lets a Banner show, and under DND only a loud one
            let loud =
                notification.urgency() == Urgency::Critical || notification.effects().bypass_dnd;
            let allowed = notification.effects().banner && (!dnd || loud);
            let tracked = tracked.get(&notification.id()).map(String::as_str);

            progress(notification, allowed, tracked).map(|step| (notification.id(), step))
        })
        .collect()
}

// runs on its own thread for good
pub fn follow() {
    let (mut wakes, waker) = Wakes::woken(PACE, vec![BUS]);
    let mut before: Vec<Version> = Vec::new();
    let mut daemon = Daemon::Starting;
    let shows_banners = modules::on("banners");
    let shows_progress = modules::on("live");

    // the progress Live Activities, by notification id, as the live ids they are
    let mut tracked: HashMap<u32, String> = HashMap::new();

    // DND as the last read had it, for the read that finds it changed
    let mut seen_dnd = IslandService::read().dnd();

    drop(WAKER.set(waker));

    supervise::run("notifications", || {
        loop {
            let dnd = IslandService::read().dnd();

            // the first read starts the daemon
            let notifications = Notifications::read();
            let running = notifications.running();

            let now: Vec<Version> = notifications
                .list()
                .iter()
                .map(|notification| (notification.id(), notification.version()))
                .collect();

            let (posted, gone) = changes(&before, &now);
            let changed = !posted.is_empty() || !gone.is_empty();

            // read here, so they are of the same list
            let arrived: Vec<&Notification> = notifications
                .list()
                .iter()
                .filter(|notification| {
                    posted.contains(&(notification.id(), notification.version()))
                })
                .collect();

            let shown: Vec<Banner> = if shows_banners {
                arrived
                    .iter()
                    .filter(|notification| notification.effects().banner)
                    .map(|notification| Banner::of(notification))
                    .collect()
            } else {
                Vec::new()
            };

            // a new version whose rule shows no Banner takes the old version's down
            let quiet: Vec<u32> = arrived
                .iter()
                .filter(|notification| !notification.effects().banner)
                .map(|notification| notification.id())
                .collect();

            // those whose rule keeps them out of the history, which close unless a Banner shows
            let unkept: Vec<(u32, u64)> = arrived
                .iter()
                .filter(|notification| !notification.effects().history)
                .map(|notification| (notification.id(), notification.version()))
                .collect();

            // only a notification new to the list rings; a replacement, like a progress, does not
            let rings: Vec<Hints> = if config::get().notification_sound {
                arrived
                    .iter()
                    .filter(|notification| notification.effects().sound)
                    .filter(|notification| !before.iter().any(|(id, _)| *id == notification.id()))
                    .map(|notification| notification.sound().clone())
                    .collect()
            } else {
                Vec::new()
            };

            let steps: Vec<(u32, Step)> = if shows_progress {
                steps(
                    notifications.list(),
                    &posted,
                    dnd != seen_dnd,
                    dnd,
                    &tracked,
                )
            } else {
                Vec::new()
            };

            seen_dnd = dnd;

            drop(notifications);

            for (id, step) in steps {
                match step {
                    Step::Show {
                        id: live_id,
                        moved,
                        title,
                        detail,
                        percent,
                        lasts,
                    } => {
                        if let Some(old) = moved {
                            tracked.remove(&id);
                            live::withdraw(&old);
                        }

                        // refused past the bound on what is live, which is no reason to stop
                        if live::show(&live_id, &title, &detail, percent, lasts).is_ok() {
                            tracked.insert(id, live_id);
                        }
                    }
                    Step::Finish(live_id) => {
                        tracked.remove(&id);
                        live::finish(&live_id);
                    }
                    Step::Withdraw(live_id) => {
                        tracked.remove(&id);
                        live::withdraw(&live_id);
                    }
                }
            }

            // closed by its sender or dismissed, or replaced and gone from the list
            for id in &gone {
                if let Some(live_id) = tracked.remove(id) {
                    live::withdraw(&live_id);
                }
            }

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

            // never under DND, which quiets what the Banners show
            if !dnd {
                for hints in &rings {
                    sound::play(hints);
                }
            }

            /*
             * a Banner DND drops that replaces none, or one already put away, changes nothing, so
             * neither writes for nothing
             */
            let (shown, closed): (Vec<Banner>, Vec<u32>) = if shows_banners {
                let banners = Banners::read();

                (
                    shown
                        .into_iter()
                        .filter(|banner| !dnd || banner.loud() || banners.contains(banner.id))
                        .collect(),
                    gone.iter()
                        .chain(&quiet)
                        .copied()
                        .filter(|id| banners.contains(*id))
                        .collect(),
                )
            } else {
                (Vec::new(), Vec::new())
            };

            // a Banner that arrives and goes is the Banners' to close, as it leaves
            let expired: Vec<(u32, u64)> = unkept
                .into_iter()
                .filter(|(id, _)| !shown.iter().any(|banner| banner.id == *id))
                .collect();

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

            for (id, version) in expired {
                Notifications::expire(id, version);
            }

            // until the daemon has the bus name, nothing it would announce can arrive
            let busy = changed || daemon == Daemon::Starting;
            before = now;
            wakes.wait(busy);
        }
    });
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use super::*;
    use crate::sources::notifications::rules::Effects;

    fn notification(app: &str, id: u32, value: Option<u8>) -> Notification {
        Notification {
            id,
            app_name: app.to_owned(),
            desktop_entry: String::new(),
            category: String::new(),
            summary: String::from("Copying"),
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
            value,
            timeout: None,
            received: SystemTime::UNIX_EPOCH,
            version: 1,
            effects: Effects::default(),
        }
    }

    // the id and the steps come from another program's words, so the id always fits `live`
    #[test]
    fn a_progress_lives_by_value_and_its_id_fits_live() {
        for app in ["", "Files", "my app/ünï \t<b>", &"x".repeat(500)] {
            let id = live_id(&notification(app, u32::MAX, None));

            assert!(live::valid_id(&id), "{id}");
        }

        let live = Some("notification.files.7");
        let step =
            |value, allowed, tracked| progress(&notification("Files", 7, value), allowed, tracked);
        let id = || String::from("notification.files.7");

        assert!(matches!(
            step(Some(40), true, None),
            Some(Step::Show {
                percent: 40,
                moved: None,
                ..
            })
        ));
        assert!(matches!(
            step(Some(0), true, live),
            Some(Step::Show {
                percent: 0,
                moved: None,
                ..
            })
        ));
        assert_eq!(step(Some(100), true, live), Some(Step::Finish(id())));
        assert_eq!(step(Some(100), true, None), None);
        assert_eq!(step(None, true, live), Some(Step::Withdraw(id())));
        assert_eq!(step(Some(40), false, live), Some(Step::Withdraw(id())));
        assert_eq!(step(Some(40), false, None), None);
        assert_eq!(step(None, true, None), None);
    }

    // DND changing moves a progress that did not change: under it takes the Activity away, and out
    // of it brings one back
    #[test]
    fn a_change_of_dnd_moves_every_progress_not_only_the_ones_that_arrived() {
        let list = [notification("Files", 7, Some(40))];
        let posted = [(7, 1)];
        let live = HashMap::from([(7, String::from("notification.files.7"))]);

        // nothing arrived, DND still as it was: no step
        assert!(steps(&list, &[], false, false, &live).is_empty());

        // DND on: the Activity is taken away
        assert_eq!(
            steps(&list, &[], true, true, &live),
            [(7, Step::Withdraw(String::from("notification.files.7")))]
        );

        // DND off again, the Activity gone: it comes back at its value
        assert!(matches!(
            steps(&list, &[], true, false, &HashMap::new()).as_slice(),
            [(7, Step::Show { percent: 40, .. })]
        ));

        // a notification that arrived is stepped, DND changed or not
        assert_eq!(
            steps(&list, &posted, false, false, &HashMap::new()).len(),
            1
        );

        // a loud one stays under DND
        let mut loud = notification("Files", 7, Some(40));
        loud.urgency = Urgency::Critical;

        assert!(matches!(
            steps(&[loud], &[], true, true, &HashMap::new()).as_slice(),
            [(7, Step::Show { .. })]
        ));
    }

    // a replacement may name another app: the Activity it had is the one that is moved or ended
    #[test]
    fn a_replacement_under_another_app_name_acts_on_the_live_id_it_had() {
        let live = Some("notification.files.7");
        let renamed = |value| progress(&notification("Other", 7, value), true, live);

        assert!(matches!(
            renamed(Some(40)),
            Some(Step::Show { id, moved: Some(old), .. })
                if id == "notification.other.7" && old == "notification.files.7"
        ));
        assert_eq!(
            renamed(Some(100)),
            Some(Step::Finish(String::from("notification.files.7")))
        );
        assert_eq!(
            renamed(None),
            Some(Step::Withdraw(String::from("notification.files.7")))
        );
    }
}
