/*
 * where Kanade itself goes, as a Launcher provider: Wi-Fi and Bluetooth open Controls on their
 * networks and devices, Clipboard opens the history, Calendar the month and agenda, Wallpapers the wallpaper grid (also for
 * anything typed after `@`), Keys the keybinds, Notifications, Controls, Weather, Tray, Session, Timer and Capture
 * their Surfaces, and each page of Settings and each setting opens Settings on the page showing it. They only open: switching a radio or joining a network
 * is done in the Surface they open
 */

use crate::clock;
use crate::icon::Icon;
use crate::settings;
use crate::sources::recents;

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{emoji, history};

// what opens the Wallpapers Surface when typed first, whatever follows
pub const WALLPAPER: &str = "@";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Destination {
    Wifi,
    Bluetooth,
    Notifications,
    Controls,
    Weather,
    Tray,
    Session,
    Timer,
    Capture,
    Clipboard,
    Calendar,
    Wallpapers,
    Keys,
    Window,

    // Settings, on the page of this name
    Settings(&'static str),
}

impl Destination {
    // the name its launches are kept under (`sources::recents`), which is the destination's
    pub fn name(&self) -> &'static str {
        match self {
            Destination::Wifi => "wifi",
            Destination::Bluetooth => "bluetooth",
            Destination::Notifications => "notifications",
            Destination::Controls => "controls",
            Destination::Weather => "weather",
            Destination::Tray => "tray",
            Destination::Session => "session",
            Destination::Timer => "timer",
            Destination::Capture => "capture",
            Destination::Clipboard => "clipboard",
            Destination::Calendar => "calendar",
            Destination::Wallpapers => "wallpapers",
            Destination::Keys => "keys",
            Destination::Window => "window",
            Destination::Settings(page) => page,
        }
    }
}

// which destinations exist now: their Module is on and, for a radio, the machine has one
#[derive(Debug, Clone, Copy, Default)]
pub struct Destinations {
    pub wifi: bool,
    pub bluetooth: bool,
    pub notifications: bool,
    pub controls: bool,
    pub weather: bool,
    pub tray: bool,
    pub session: bool,
    pub timer: bool,
    pub capture: bool,
    pub clipboard: bool,
    pub calendar: bool,
    pub wallpapers: bool,
    pub keys: bool,
    pub window: bool,
    pub settings: bool,
}

// the fewest letters any destination takes: one is a prefix of an alias or in half the settings, and
// Enter on it would open one rather than the app typed
const FROM: usize = 2;

// the fewest letters of each word that may match anywhere in a setting's help: fewer match nearly all
const ANYWHERE: usize = 3;

impl LauncherProvider for Destinations {
    /*
     * by name within a fit; nothing for a query of one letter or none, which lists the apps.
     * An empty query lists every destination and every Settings page as a Prefix, which the
     * Launcher's Recents resolve against; a single setting still takes a query
     */
    fn find(&self, query: &str) -> Vec<Answer> {
        // `@` and whatever follows it opens the grid of wallpapers
        if query.starts_with(WALLPAPER) {
            return self.wallpapers.then(wallpapers).into_iter().collect();
        }

        if query.starts_with(emoji::PREFIX) || query.starts_with(history::PREFIX) {
            return Vec::new();
        }

        let empty = query.is_empty();

        if !empty && query.chars().count() < FROM {
            return Vec::new();
        }

        let query = query.to_lowercase();
        let now = clock::unix_seconds();
        let mut answers = Vec::new();

        let tiles = [
            (
                self.wifi,
                Icon::Wifi,
                "Wi-Fi",
                "Networks",
                &["wifi", "wi-fi", "wireless", "network"][..],
                Destination::Wifi,
            ),
            (
                self.bluetooth,
                Icon::Bluetooth,
                "Bluetooth",
                "Devices",
                &["bluetooth", "bt"][..],
                Destination::Bluetooth,
            ),
            (
                self.notifications,
                Icon::Bell,
                "Notifications",
                "Recent and Do Not Disturb",
                &["notifications", "notification", "notifs", "dnd"][..],
                Destination::Notifications,
            ),
            (
                self.controls,
                Icon::Sliders,
                "Controls",
                "Volume, brightness and toggles",
                &["controls", "control", "volume", "brightness"][..],
                Destination::Controls,
            ),
            (
                self.weather,
                Icon::Cloud,
                "Weather",
                "Forecast",
                &["weather", "forecast", "temperature"][..],
                Destination::Weather,
            ),
            (
                self.tray,
                Icon::Grid,
                "Tray",
                "Apps' tray items and menus",
                &["tray", "systray", "status icons", "indicators"][..],
                Destination::Tray,
            ),
            (
                self.session,
                Icon::Power,
                "Session",
                "Lock, log out, restart and power off",
                &["session", "power", "lock", "logout", "shutdown", "restart"][..],
                Destination::Session,
            ),
            (
                self.timer,
                Icon::Stopwatch,
                "Timer",
                "Countdown and pomodoro",
                &["timer", "countdown", "pomodoro", "stopwatch"][..],
                Destination::Timer,
            ),
            (
                self.capture,
                Icon::Capture,
                "Capture",
                "Screenshot and screen recording",
                &["capture", "screenshot", "record", "recording"][..],
                Destination::Capture,
            ),
            (
                self.clipboard,
                Icon::Clipboard,
                "Clipboard",
                "History",
                &["clipboard", "clip", "paste", "history"][..],
                Destination::Clipboard,
            ),
            (
                self.calendar,
                Icon::Calendar,
                "Calendar",
                "Events and agenda",
                &["calendar", "events", "agenda", "schedule"][..],
                Destination::Calendar,
            ),
            (
                self.wallpapers,
                Icon::Picture,
                "Wallpapers",
                "Choose a wallpaper",
                &["wallpapers", "wallpaper", "background"][..],
                Destination::Wallpapers,
            ),
            (
                self.keys,
                Icon::Keyboard,
                "Keys",
                "Keybinds and shortcuts",
                &["keys", "keybinds", "shortcuts", "hotkeys", "bindings"][..],
                Destination::Keys,
            ),
            (
                self.window,
                Icon::Columns,
                "Window",
                "Column and window controls",
                &["window", "column", "tile", "expel", "consume"][..],
                Destination::Window,
            ),
        ];

        let mut found: Vec<(u8, f64, Answer)> = Vec::new();

        recents::with(|usage| {
            for (offered, icon, title, detail, names, destination) in tiles {
                let fit = names
                    .iter()
                    .filter_map(|name| provider::fit(&query, name))
                    .min();

                let Some(fit) = fit.filter(|_| offered) else {
                    continue;
                };

                let key = recents::destination(destination.name());

                found.push((
                    usage.learned(&key, &query, now),
                    usage.score(&key, now),
                    Answer {
                        fit,
                        title: title.to_owned(),
                        detail: detail.to_owned(),
                        mark: Mark::Icon(icon),
                        action: Action::Open(destination),
                    },
                ));
            }
        });

        // stable, so the rest stay in the order of the tiles
        found.sort_by(|left, right| right.0.cmp(&left.0).then(right.1.total_cmp(&left.1)));
        answers.extend(found.into_iter().map(|(.., answer)| answer));

        if self.settings {
            // a recent Settings page lists in the Recents; a setting alone takes a query
            answers.extend(pages(&query));
            if !empty {
                answers.extend(placed(&query));
            }
        }

        answers
    }
}

// the one answer of an `@` search
fn wallpapers() -> Answer {
    Answer {
        fit: Fit::Exact,
        title: String::from("Wallpapers"),
        detail: String::from("Choose a wallpaper"),
        mark: Mark::Icon(Icon::Picture),
        action: Action::Open(Destination::Wallpapers),
    }
}

// each page of Settings, as "<page> settings"
fn pages(query: &str) -> impl Iterator<Item = Answer> {
    settings::listed().filter_map(move |page| {
        let title = format!("{} settings", page.title);
        let fit = provider::fit(query, &title.to_lowercase())?;

        Some(Answer {
            fit,
            title,
            detail: page.summary.to_owned(),
            mark: Mark::Icon(page.icon),
            action: Action::Open(Destination::Settings(page.name)),
        })
    })
}

/*
 * each setting by its label or key, then by every word of it anywhere in what it says of itself,
 * opening the page it is on
 */
fn placed(query: &str) -> impl Iterator<Item = Answer> {
    settings::placed().iter().filter_map(move |placed| {
        let setting = placed.setting;

        let fit = provider::fit(query, &setting.label.to_lowercase())
            .into_iter()
            .chain(provider::fit(query, setting.key))
            .min()
            .or_else(|| {
                let anywhere = format!(
                    "{} {} {} {}",
                    setting.label, setting.key, setting.help, placed.title
                )
                .to_lowercase();

                query
                    .split_whitespace()
                    .all(|term| term.chars().count() >= ANYWHERE && anywhere.contains(term))
                    .then_some(Fit::Anywhere)
            })?;

        Some(Answer {
            fit,
            title: setting.label.to_owned(),
            detail: format!("Settings - {}", placed.title),
            mark: Mark::Icon(placed.icon),
            action: Action::Open(Destination::Settings(placed.page)),
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // two letters find a setting by its name, never by a word somewhere in its help
    #[test]
    fn a_short_query_matches_no_help_text() {
        let destinations = Destinations {
            settings: true,
            ..Destinations::default()
        };

        for query in ["on", "to", "an", "se"] {
            let answers = destinations.find(query);

            assert!(
                answers.iter().all(|answer| answer.fit != Fit::Anywhere),
                "{query}"
            );
        }
    }
}
