/*
 * where Kanade itself goes, as a Launcher provider: Wi-Fi and Bluetooth open Controls on their
 * networks and devices, Clipboard opens the history, Calendar the month and agenda, Wallpapers the wallpaper grid (also for
 * anything typed after `@`), Keys the keybinds, Notifications, Controls, Weather, Tray, Session, Timer and Capture
 * their Surfaces, and each page of Settings and each setting opens Settings on the page showing it. They only open: switching a radio or joining a network
 * is done in the Surface they open
 */

use crate::icon::Icon;
use crate::settings;

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

    // Settings, on the page of this name
    Settings(&'static str),
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
    pub settings: bool,
}

// the fewest letters any destination takes: one is a prefix of an alias or in half the settings, and
// Enter on it would open one rather than the app typed
const FROM: usize = 2;

// the fewest letters of each word that may match anywhere in a setting's help: fewer match nearly all
const ANYWHERE: usize = 3;

impl LauncherProvider for Destinations {
    // by name within a fit; nothing for a query of one letter or none, which lists the apps
    fn find(&self, query: &str) -> Vec<Answer> {
        // `@` and whatever follows it opens the grid of wallpapers
        if query.starts_with(WALLPAPER) {
            return self.wallpapers.then(wallpapers).into_iter().collect();
        }

        if query.chars().count() < FROM
            || query.starts_with(emoji::PREFIX)
            || query.starts_with(history::PREFIX)
        {
            return Vec::new();
        }

        let query = query.to_lowercase();
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
        ];

        for (offered, icon, title, detail, names, destination) in tiles {
            let fit = names
                .iter()
                .filter_map(|name| provider::fit(&query, name))
                .min();

            if let Some(fit) = fit.filter(|_| offered) {
                answers.push(Answer {
                    fit,
                    title: title.to_owned(),
                    detail: detail.to_owned(),
                    mark: Mark::Icon(icon),
                    action: Action::Open(destination),
                });
            }
        }

        if self.settings {
            answers.extend(pages(&query));
            answers.extend(placed(&query));
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
