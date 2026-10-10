/*
 * where Kanade itself goes, as a Launcher provider: Wi-Fi and Bluetooth open Controls on their
 * networks and devices, Clipboard opens the history, and each page of Settings and each setting
 * opens Settings on the page showing it. They only open: switching a radio or joining a network
 * is done in the Surface they open
 */

use crate::icon::Icon;
use crate::settings;

use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{emoji, wallpaper};

#[derive(Debug, Clone, PartialEq)]
pub enum Destination {
    Wifi,
    Bluetooth,
    Clipboard,

    // Settings, on the page of this name
    Settings(&'static str),
}

// which destinations exist now: their Module is on and, for a radio, the machine has one
#[derive(Debug, Clone, Copy, Default)]
pub struct Destinations {
    pub wifi: bool,
    pub bluetooth: bool,
    pub clipboard: bool,
    pub settings: bool,
}

// the fewest letters any destination takes: one is a prefix of an alias or in half the settings, and
// Enter on it would open one rather than the app typed
const FROM: usize = 2;

impl LauncherProvider for Destinations {
    // by name within a fit; nothing for a query of one letter or none, which lists the apps
    fn find(&self, query: &str) -> Vec<Answer> {
        if query.chars().count() < FROM
            || query.starts_with(emoji::PREFIX)
            || query.starts_with(wallpaper::PREFIX)
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
                self.clipboard,
                Icon::Clipboard,
                "Clipboard",
                "History",
                &["clipboard", "clip", "paste", "history"][..],
                Destination::Clipboard,
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
                    .all(|term| anywhere.contains(term))
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
