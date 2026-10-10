//! Settings' pages (ADR 0029): what the window shows, by task, not by the Module owning each key.
//! Every key of the schema shows on exactly one page, and every Module but the core in exactly one
//! group of the Modules page; the tests hold both.

use crate::icon::Icon;

// a page of the window
pub struct Page {
    // as `kanade settings open` takes it
    pub name: &'static str,
    pub title: &'static str,

    // a line under the title
    pub summary: &'static str,

    // the heading it is listed under down the side
    pub group: &'static str,
    pub icon: Icon,
    pub sections: &'static [Section],
}

// a heading on a page and its rows
pub struct Section {
    pub title: &'static str,
    pub rows: &'static [Row],
}

// a row of a page
pub enum Row {
    // a key, drawn by its Kind
    Key(&'static str),

    // `appearance.material` as tiles of each material
    Material,

    // an edge and a side, as a grid of the eight places they make: three on the top and bottom,
    // the middle of each side
    Position {
        edge: &'static str,
        align: &'static str,
    },
}

impl Row {
    pub fn keys(&self) -> Vec<&'static str> {
        match self {
            Row::Key(key) => vec![key],
            Row::Material => vec![MATERIAL],
            Row::Position { edge, align } => vec![edge, align],
        }
    }
}

pub const MATERIAL: &str = "appearance.material";

// the Modules page's groups, each Module but the core once
pub const MODULE_GROUPS: &[(&str, &[&str])] = &[
    ("Desktop", &["dock", "windows", "wallpaper", "workspace"]),
    ("Activities", &["media", "timer", "battery"]),
    (
        "Notifications & privacy",
        &[
            "notifications",
            "banners",
            "notification-surface",
            "privacy",
            "osd",
            "capture",
        ],
    ),
    (
        "Controls",
        &[
            "controls",
            "audio",
            "brightness",
            "network",
            "bluetooth",
            "power",
            "tray",
            "launcher",
            "caffeine",
            "clipboard",
            "clipboard-surface",
        ],
    ),
    (
        "Calendar & weather",
        &[
            "calendar",
            "calendar-surface",
            "google-calendar",
            "weather",
            "weather-surface",
        ],
    ),
    ("Session", &["lock", "session", "settings"]),
];

pub const MODULES: &str = "modules";

pub const PAGES: &[Page] = &[
    Page {
        name: "appearance",
        title: "Appearance",
        summary: "What the Island, Dock and lock screen are made of, and how they read",
        group: "Shell",
        icon: Icon::Palette,
        sections: &[
            Section {
                title: "",
                rows: &[Row::Material],
            },
            Section {
                title: "Tone & light",
                rows: &[
                    Row::Key("appearance.tone"),
                    Row::Key("appearance.highlight"),
                ],
            },
            Section {
                title: "Color & type",
                rows: &[Row::Key("theme.palette"), Row::Key("appearance.font")],
            },
        ],
    },
    Page {
        name: "island",
        title: "Island",
        summary: "Where the Island sits, how large it grows and what it shows at Rest",
        group: "Shell",
        icon: Icon::Island,
        sections: &[
            Section {
                title: "Placement",
                rows: &[Row::Position {
                    edge: "island.edge",
                    align: "island.align",
                }],
            },
            Section {
                title: "Size",
                rows: &[Row::Key("island.width"), Row::Key("island.height")],
            },
            Section {
                title: "Presence",
                rows: &[Row::Key("island.autohide"), Row::Key("island.reserve")],
            },
            Section {
                title: "At Rest",
                rows: &[
                    Row::Key("clock"),
                    Row::Key("rest.clock"),
                    Row::Key("rest.battery"),
                    Row::Key("rest.peek.battery"),
                    Row::Key("rest.peek.weather"),
                    Row::Key("rest.peek.agenda"),
                ],
            },
            Section {
                title: "Playing media",
                rows: &[Row::Key("media.visualizer")],
            },
        ],
    },
    Page {
        name: "dock",
        title: "Dock",
        summary: "Where the Dock sits, how it moves and what it keeps",
        group: "Shell",
        icon: Icon::Dock,
        sections: &[
            Section {
                title: "Placement",
                rows: &[Row::Position {
                    edge: "dock.edge",
                    align: "dock.align",
                }],
            },
            Section {
                title: "Presence",
                rows: &[Row::Key("dock.autohide"), Row::Key("dock.reserve")],
            },
            Section {
                title: "With the Island",
                rows: &[Row::Key("dock.merge")],
            },
            Section {
                title: "Icons",
                rows: &[Row::Key("dock.size"), Row::Key("dock.magnification")],
            },
            Section {
                title: "Your apps",
                rows: &[Row::Key("dock.pinned")],
            },
            Section {
                title: "Advanced",
                rows: &[Row::Key("windows.apps")],
            },
        ],
    },
    Page {
        name: "motion",
        title: "Motion & timing",
        summary: "How the Island and Dock move, and how long they wait",
        group: "Experience",
        icon: Icon::Pulse,
        sections: &[
            Section {
                title: "Response",
                rows: &[Row::Key("appearance.motion")],
            },
            Section {
                title: "Accessibility",
                rows: &[Row::Key("reduced_motion")],
            },
            Section {
                title: "Interaction timing",
                rows: &[
                    Row::Key("timings.hover"),
                    Row::Key("timings.expand"),
                    Row::Key("timings.surface_change"),
                    Row::Key("timings.collapse"),
                    Row::Key("timings.grace"),
                    Row::Key("timings.osd"),
                ],
            },
        ],
    },
    Page {
        name: "notifications",
        title: "Notifications & privacy",
        summary: "How Banners arrive and how capture is marked",
        group: "Experience",
        icon: Icon::Bell,
        sections: &[
            Section {
                title: "Banners",
                rows: &[Row::Key("banners.entrance")],
            },
            Section {
                title: "Privacy",
                rows: &[Row::Key("privacy.indicators")],
            },
        ],
    },
    Page {
        name: "modules",
        title: "Modules",
        summary: "What runs in the shell; the Island's core always does",
        group: "System",
        icon: Icon::Grid,
        sections: &[Section {
            title: "",
            rows: &[Row::Key(MODULES)],
        }],
    },
    Page {
        name: "data",
        title: "Data",
        summary: "Where wallpapers, calendars and the weather come from",
        group: "System",
        icon: Icon::Data,
        sections: &[
            Section {
                title: "Wallpaper",
                rows: &[Row::Key("wallpaper.directory")],
            },
            Section {
                title: "Calendar",
                rows: &[Row::Key("calendar.paths")],
            },
            Section {
                title: "Weather",
                rows: &[
                    Row::Key("weather.location"),
                    Row::Key("weather.place"),
                    Row::Key("weather.units"),
                ],
            },
        ],
    },
    Page {
        name: "lock",
        title: "Lock screen",
        summary: "What shows behind the lock screen",
        group: "System",
        icon: Icon::Lock,
        sections: &[Section {
            title: "",
            rows: &[Row::Key("lock.backdrop")],
        }],
    },
];

// the page showing `key`
pub fn page_of(key: &str) -> Option<&'static Page> {
    PAGES.iter().find(|page| {
        page.sections
            .iter()
            .flat_map(|section| section.rows)
            .any(|row| row.keys().contains(&key))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modules::{self, CORE};

    #[test]
    fn every_key_shows_on_one_page() {
        let shown: Vec<&str> = PAGES
            .iter()
            .flat_map(|page| page.sections)
            .flat_map(|section| section.rows)
            .flat_map(Row::keys)
            .collect();

        for module in modules::ALL {
            for setting in module.settings {
                let times = shown.iter().filter(|&&key| key == setting.key).count();
                assert_eq!(times, 1, "{} shows {times} times", setting.key);
            }
        }

        let keys = modules::ALL
            .iter()
            .flat_map(|module| module.settings)
            .count();
        assert_eq!(shown.len(), keys, "a page shows a key no Module owns");
    }

    #[test]
    fn every_module_but_the_core_is_in_one_group() {
        let grouped: Vec<&str> = MODULE_GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter().copied())
            .collect();

        for module in modules::ALL.iter().filter(|module| module.name != CORE) {
            let times = grouped.iter().filter(|&&name| name == module.name).count();
            assert_eq!(times, 1, "{} is in {times} groups", module.name);
        }

        assert_eq!(grouped.len(), modules::ALL.len() - 1);
    }

    #[test]
    fn page_names_are_unique() {
        for (index, page) in PAGES.iter().enumerate() {
            assert!(PAGES[..index].iter().all(|other| other.name != page.name));
        }
    }
}
