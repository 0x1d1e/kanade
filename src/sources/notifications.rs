//! Notification toasts (plan 3, 5.1, 7): a notification that arrives, or is replaced, shows as a
//! Transient toast on the focused island. Actionable when it has actions, Critical, so preempting,
//! when its sender says critical. The toast expires; the notification stays in Amane's list, the
//! history, until it is dismissed or its sender closes it, which also takes its toast down.
//!
//! DND is the Arbiter's (`island dnd toggle`): it silences toasts, never the history. Toasts never
//! take the keyboard: the island takes it only for a pointer or a Surface (`IslandService::keyboard`).
//!
//! Amane's Notifications has no subscription, so this polls its read; a read that finds the same
//! notifications posts nothing. Amane is the daemon only if no other one, like mako, got the bus name
//! first. Then nothing arrives, and `Daemon` says who has it for the Notifications Surface.

use std::fs;
use std::process;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use amane::{Apps, Argument, Bus, Notification, Notifications, Service, Urgency};

use crate::island::activity::{Action, Activity, Detail, Id, Kind, Priority, Toast};
use crate::island::service::IslandService;

// a toast shows within this of arriving
const POLL: Duration = Duration::from_millis(100);

// plan 5.2: 4000-6000 ms
const TOAST: Duration = Duration::from_millis(5000);

// the bus name a notification daemon owns
const NAME: &str = "org.freedesktop.Notifications";

const DBUS: &str = "org.freedesktop.DBus";

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
    // `owner` is the process that has the bus name, asked only while Amane is not running
    fn of(running: bool, owner: impl FnOnce() -> Option<u32>) -> Daemon {
        if running {
            return Daemon::Running;
        }

        // Amane owns it and is about to say so, or no one does yet
        match owner() {
            Some(pid) if pid != process::id() => Daemon::Conflict(name(pid)),
            _ => Daemon::Starting,
        }
    }
}

// one version of a notification: a replacement keeps the id and gets a new time
type Version = (u32, SystemTime);

/*
 * the versions in `now` that were not in `before`, to post, and the ids gone from it, to withdraw.
 * A replaced notification is a new version of the same id, so its repost replaces its toast and
 * shows it for a full Lifetime again
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

// one per notification, so a sender replacing it replaces its toast
fn id(notification: u32) -> Id {
    Id::new(Kind::Notification, notification.to_string())
}

// Critical preempts even an open Surface, and DND never silences it (plan 5.1 rules 4, 5)
fn activity(notification: u32, urgency: Urgency, actions: Vec<Action>, toast: Toast) -> Activity {
    let priority = match urgency {
        Urgency::Critical => Priority::Critical,
        _ if !actions.is_empty() => Priority::Actionable,
        _ => Priority::Passive,
    };

    Activity::transient(id(notification), priority, TOAST)
        .with_actions(actions)
        .with_detail(Detail::Notification(toast))
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

// the process that has the notification bus name, none while no one has it
fn owner() -> Option<u32> {
    let bus = Bus::session();
    let path = "/org/freedesktop/DBus";

    let owner = bus.call(DBUS, path, DBUS, "GetNameOwner", &[Argument::from(NAME)]);

    if owner.text().is_empty() {
        return None;
    }

    let pid = bus.call(
        DBUS,
        path,
        DBUS,
        "GetConnectionUnixProcessID",
        &[Argument::from(owner.text())],
    );

    Some(pid.number() as u32).filter(|&pid| pid > 0)
}

// as the process names itself, like "mako"
fn name(pid: u32) -> String {
    fs::read_to_string(format!("/proc/{pid}/comm"))
        .map_or_else(|_| format!("process {pid}"), |name| name.trim().to_owned())
}

// runs on its own thread for good
pub fn follow() {
    let mut before: Vec<Version> = Vec::new();
    let mut daemon = Daemon::Starting;

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

        // read here, so they are of the same list
        let posted: Vec<Activity> = notifications
            .list()
            .iter()
            .filter(|notification| posted.contains(&(notification.id(), notification.received())))
            .map(|notification| {
                let actions = notification
                    .actions()
                    .iter()
                    .map(|action| Action {
                        key: action.key().to_owned(),
                        label: action.label().to_owned(),
                    })
                    .collect();

                activity(
                    notification.id(),
                    notification.urgency(),
                    actions,
                    toast(notification),
                )
            })
            .collect();

        drop(notifications);

        // Running and Conflict are for good, so only Starting asks again
        if daemon == Daemon::Starting {
            let next = Daemon::of(running, owner);

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

        // a toast that already expired is not registered, so withdrawing it would write for nothing
        let gone: Vec<Id> = {
            let island = IslandService::read();

            gone.into_iter()
                .map(id)
                .filter(|id| island.contains(id))
                .collect()
        };

        if !posted.is_empty() || !gone.is_empty() {
            let now = Instant::now();
            let mut island = IslandService::write();

            for activity in posted {
                island.post(activity, now);
            }

            for id in &gone {
                island.withdraw(id, now);
            }
        }

        before = now;
        thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::island::activity::{Interrupt, Lifetime};
    use crate::island::presentation::{Presentation, Surface};

    fn at(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    fn reply() -> Vec<Action> {
        vec![Action {
            key: String::from("reply"),
            label: String::from("Reply"),
        }]
    }

    fn toast(urgency: Urgency, actions: Vec<Action>) -> Activity {
        activity(7, urgency, actions, Toast::default())
    }

    #[test]
    fn a_new_notification_posts_and_a_closed_one_withdraws() {
        let one = [(1, at(1))];
        let two = [(1, at(1)), (2, at(2))];

        assert_eq!(changes(&[], &one), (vec![(1, at(1))], vec![]));
        assert_eq!(changes(&one, &two), (vec![(2, at(2))], vec![]));
        assert_eq!(changes(&two, &[(2, at(2))]), (vec![], vec![1]));
    }

    #[test]
    fn the_same_list_posts_nothing() {
        let list = [(1, at(1)), (2, at(2))];

        assert_eq!(changes(&list, &list), (vec![], vec![]));
    }

    // a replacement keeps its id, so it replaces the toast instead of withdrawing it
    #[test]
    fn a_replaced_notification_posts_again() {
        assert_eq!(
            changes(&[(1, at(1))], &[(1, at(5))]),
            (vec![(1, at(5))], vec![])
        );
    }

    #[test]
    fn a_toast_is_transient_for_the_focused_output() {
        let toast = toast(Urgency::Normal, vec![]);

        assert_eq!(toast.id(), &Id::new(Kind::Notification, "7"));
        assert_eq!(toast.lifetime(), Lifetime::Transient(TOAST));
        assert_eq!(toast.priority(), Priority::Passive);
        assert_eq!(toast.interrupt(), Interrupt::Transient);
    }

    #[test]
    fn actions_make_it_actionable_and_critical_preempts() {
        assert_eq!(
            toast(Urgency::Low, reply()).priority(),
            Priority::Actionable
        );
        assert_eq!(toast(Urgency::Normal, reply()).actions(), reply());

        for actions in [vec![], reply()] {
            let critical = toast(Urgency::Critical, actions);

            assert_eq!(critical.priority(), Priority::Critical);
            assert_eq!(critical.interrupt(), Interrupt::Preempt);
        }
    }

    #[test]
    fn a_toast_never_displaces_an_open_surface() {
        let now = Instant::now();
        let mut island = IslandService::new();
        let media = Activity::persistent(Id::new(Kind::Media, "mpv"), Priority::Media);

        island.post(media, now);
        island.open("eDP-1", Surface::Media, now);
        assert_eq!(
            island.presentation("eDP-1"),
            Presentation::Expanded(Surface::Media)
        );

        island.post(toast(Urgency::Normal, reply()), now);

        assert_eq!(
            island.presentation("eDP-1"),
            Presentation::Expanded(Surface::Media)
        );
        assert_eq!(island.frame("eDP-1", now).transient, None);
        assert_eq!(island.frame("eDP-1", now).queued.len(), 1);
    }

    #[test]
    fn dnd_silences_the_toast() {
        let now = Instant::now();
        let mut island = IslandService::new();

        island.set_dnd(true, now);
        island.post(toast(Urgency::Normal, vec![]), now);
        assert_eq!(island.frame("eDP-1", now).transient, None);
        assert_eq!(island.presentation("eDP-1"), Presentation::Rest);

        island.post(toast(Urgency::Critical, vec![]), now);
        assert!(island.frame("eDP-1", now).transient.is_some());
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
