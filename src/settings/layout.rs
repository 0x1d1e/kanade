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

    // an edge and a side, as a grid of the eight places they make: three on the top and bottom,
    // the middle of each side
    Position {
        edge: &'static str,
        align: &'static str,
    },

    // `kanade doctor`'s checks, run in the shell (ADR 0088); it has no key
    Health,

    // what `kanade sddm status` finds and the root commands still to run; it has no key
    Login,

    // an option of niri's own config (`niri`, ADR 0119), by its path in `config.kdl`; it has no key
    Niri(&'static str),

    // the connected monitors, a card each, tried live and then kept or reverted (`outputs`)
    Outputs,

    // a recommended layer rule from Installation.md
    NiriRule(&'static str),

    // finds the Weather location from a place name or the IP address, and sets `weather.location`
    // and `weather.place` (ADR 0136); it has no key of its own, those two keep their own rows
    Place,
}

impl Row {
    pub fn keys(&self) -> Vec<&'static str> {
        match self {
            Row::Key(key) => vec![key],
            Row::Position { edge, align } => vec![edge, align],
            Row::Health
            | Row::Login
            | Row::Niri(_)
            | Row::Outputs
            | Row::NiriRule(_)
            | Row::Place => Vec::new(),
        }
    }
}

// the Modules page's groups, each Module but the core once
pub const MODULE_GROUPS: &[(&str, &[&str])] = &[
    (
        "Desktop",
        &[
            "dock",
            "windows",
            "wallpaper",
            "wallpaper-surface",
            "workspace",
            "desk",
        ],
    ),
    (
        "Activities",
        &["media", "timer", "live", "launcher-entry", "battery"],
    ),
    (
        "Notifications & privacy",
        &[
            "notifications",
            "banners",
            "notification-surface",
            "privacy",
            "osd",
            "capture",
            "dictation",
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
            "night-light",
            "focus",
            "clipboard",
            "clipboard-surface",
            "keys-surface",
            "timer-surface",
            "capture-surface",
            "window-surface",
            "strip-surface",
            "emoji-surface",
            "gesture",
        ],
    ),
    (
        "Calendar & weather",
        &[
            "calendar",
            "calendar-surface",
            "calendar-reminders",
            "google-calendar",
            "calendar-subscriptions",
            "weather",
            "weather-surface",
        ],
    ),
    (
        "Session",
        &["lock", "sddm", "idle", "session", "settings", "ui", "hooks"],
    ),
];

pub const MODULES: &str = "modules";

// settings owned by another control surface, or lists of tables set in config files only, but
// still accepted in the config schema
pub const EXTERNAL_KEYS: &[&str] = &[
    "dock.pinned",
    "gestures",
    "notifications.sound_low",
    "notifications.sound_normal",
    "notifications.sound_critical",
    "notifications.rules",
    "focus.auto",
    // what an agent may read of the windows: set in a config file or `kanade config set`, not by a click
    "ui.describe",
    "hooks.lock",
    "hooks.unlock",
    "hooks.wallpaper",
    "hooks.tone",
    "hooks.charger",
    "hooks.suspend",
    "hooks.resume",
    "hooks.started",
    "hooks.dnd",
    "hooks.power_profile",
    "hooks.logout",
    "hooks.reboot",
    "hooks.poweroff",
    "hooks.timeout",
];

// old config keys that remain readable but no longer affect the shell
pub const LEGACY_KEYS: &[&str] = &["theme.palette"];

pub const PAGES: &[Page] = &[
    Page {
        name: "appearance",
        title: "Appearance",
        summary: "Choose the color tone of Settings and apps, the shell's finish, and type",
        group: "Shell",
        icon: Icon::Palette,
        sections: &[
            Section {
                title: "Tone",
                rows: &[Row::Key("appearance.tone"), Row::Key("appearance.sync")],
            },
            Section {
                title: "Finish",
                rows: &[Row::Key("appearance.finish")],
            },
            Section {
                title: "Type",
                rows: &[Row::Key("appearance.font")],
            },
            Section {
                title: "Size",
                rows: &[Row::Key("appearance.scale")],
            },
        ],
    },
    Page {
        name: "shell",
        title: "Shell",
        summary: "Choose whether the Island and Dock stand apart or share one window",
        group: "Shell",
        icon: Icon::Island,
        sections: &[
            Section {
                title: "Arrangement",
                rows: &[Row::Key("shell.mode")],
            },
            Section {
                title: "Combined position",
                rows: &[Row::Position {
                    edge: "shell.edge",
                    align: "shell.align",
                }],
            },
        ],
    },
    Page {
        name: "island",
        title: "Island",
        summary: "Where the Island sits in Separate mode, how large it grows and what it shows at Rest",
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
                title: "Pointer",
                rows: &[Row::Key("island.wheel"), Row::Key("island.device_events")],
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
                rows: &[Row::Key("media.visualizer"), Row::Key("media.lyrics")],
            },
            Section {
                title: "Launcher",
                rows: &[
                    Row::Key("launcher.style"),
                    Row::Key("launcher.actions"),
                    Row::Key("launcher.commands"),
                ],
            },
            Section {
                title: "Feedback",
                rows: &[Row::Key("osd.layout")],
            },
            Section {
                title: "Desk",
                rows: &[Row::Key("desk.clock"), Row::Key("desk.clock_corner")],
            },
        ],
    },
    Page {
        name: "dock",
        title: "Dock",
        summary: "Where the Dock sits in Separate mode, and how its icons move",
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
                title: "Advanced",
                rows: &[Row::Key("windows.apps")],
            },
        ],
    },
    Page {
        name: "motion",
        title: "Motion & timing",
        summary: "How quickly transitions move and how long the Island waits",
        group: "Experience",
        icon: Icon::Pulse,
        sections: &[
            Section {
                title: "Animation speed",
                rows: &[Row::Key("animation.speed")],
            },
            Section {
                title: "Response",
                rows: &[Row::Key("appearance.motion")],
            },
            Section {
                title: "Accessibility",
                rows: &[
                    Row::Key("reduced_motion"),
                    Row::Key("appearance.accessibility"),
                ],
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
        summary: "How Banners arrive, how capture is marked and how long live Activities wait",
        group: "Experience",
        icon: Icon::Bell,
        sections: &[
            Section {
                title: "Banners",
                rows: &[Row::Key("banners.entrance")],
            },
            Section {
                title: "Sound",
                rows: &[
                    Row::Key("notifications.sound"),
                    Row::Key("notifications.fullscreen_silence"),
                    Row::Key("notifications.sound_file"),
                    Row::Key("notifications.sound_volume"),
                ],
            },
            Section {
                title: "Privacy",
                rows: &[Row::Key("privacy.indicators")],
            },
            Section {
                title: "Dictation",
                rows: &[Row::Key("dictate.insert"), Row::Key("dictate.never_in")],
            },
            Section {
                title: "Screen recording",
                rows: &[
                    Row::Key("recording.quality"),
                    Row::Key("recording.encoder"),
                    Row::Key("recording.device"),
                ],
            },
            Section {
                title: "Live Activities",
                rows: &[Row::Key("live.stale")],
            },
        ],
    },
    Page {
        name: "timer",
        title: "Timer",
        summary: "How long a pomodoro's focus and breaks last",
        group: "Experience",
        icon: Icon::Stopwatch,
        sections: &[Section {
            title: "Pomodoro",
            rows: &[
                Row::Key("timer.pomodoro.work"),
                Row::Key("timer.pomodoro.break"),
                Row::Key("timer.pomodoro.long_break"),
                Row::Key("timer.pomodoro.rounds"),
            ],
        }],
    },
    Page {
        name: "modules",
        title: "Modules",
        summary: "What runs in the shell. A change takes a restart; the Island's core always runs",
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
        summary: "Where wallpapers, calendars and the weather come from, and how a wallpaper changes",
        group: "System",
        icon: Icon::Data,
        sections: &[
            Section {
                title: "Wallpaper",
                rows: &[
                    Row::Key("wallpaper.directory"),
                    Row::Key("wallpaper.per_monitor"),
                    Row::Key("wallpaper.transition"),
                    Row::Key("wallpaper.transition_duration"),
                ],
            },
            Section {
                title: "Calendar",
                rows: &[
                    Row::Key("calendar.paths"),
                    Row::Key("calendar.reminder"),
                    Row::Key("calendar.subscriptions"),
                    Row::Key("calendar.subscription_interval"),
                ],
            },
            Section {
                title: "Weather",
                rows: &[
                    Row::Place,
                    Row::Key("weather.location"),
                    Row::Key("weather.place"),
                    Row::Key("weather.units"),
                ],
            },
        ],
    },
    Page {
        name: "power",
        title: "Power & night light",
        summary: "When the battery warns and what it does when critical; what sets the brightness; how warm the screen turns at night; what Focus turns on; which tiles Controls shows",
        group: "System",
        icon: Icon::Sunset,
        sections: &[
            Section {
                title: "Battery",
                rows: &[
                    Row::Key("battery.low"),
                    Row::Key("battery.critical"),
                    Row::Key("battery.critical_action"),
                ],
            },
            Section {
                title: "Brightness",
                rows: &[Row::Key("brightness.backend")],
            },
            Section {
                title: "Night light",
                rows: &[
                    Row::Key("night-light.mode"),
                    Row::Key("night-light.temperature"),
                    Row::Key("night-light.sunset"),
                    Row::Key("night-light.sunrise"),
                ],
            },
            Section {
                title: "Controls",
                rows: &[Row::Key("controls.tiles")],
            },
            Section {
                title: "Focus",
                rows: &[
                    Row::Key("focus.dnd"),
                    Row::Key("focus.caffeine"),
                    Row::Key("focus.night_light"),
                ],
            },
        ],
    },
    Page {
        name: "lock",
        title: "Lock screen",
        summary: "What shows behind the lock screen, and when the session locks on its own",
        group: "System",
        icon: Icon::Lock,
        sections: &[
            Section {
                title: "",
                rows: &[Row::Key("lock.backdrop")],
            },
            Section {
                title: "Unlocking",
                rows: &[Row::Key("lock.pam_service"), Row::Key("lock.grace")],
            },
            Section {
                title: "When idle",
                rows: &[
                    Row::Key("idle.dim"),
                    Row::Key("idle.lock"),
                    Row::Key("idle.screen_off"),
                    Row::Key("idle.suspend"),
                ],
            },
        ],
    },
    Page {
        name: "login",
        title: "Login screen",
        summary: "The SDDM login screen's look, written to an overlay SDDM reads (kanade sddm)",
        group: "System",
        icon: Icon::Lock,
        sections: &[
            Section {
                title: "",
                rows: &[Row::Key("sddm.theme"), Row::Key("sddm.background")],
            },
            Section {
                title: "Picture",
                rows: &[
                    Row::Key("sddm.background_path"),
                    Row::Key("sddm.surface"),
                    Row::Key("sddm.blur"),
                    Row::Key("sddm.dim"),
                    Row::Key("sddm.sync"),
                ],
            },
            Section {
                title: "Status",
                rows: &[Row::Login],
            },
        ],
    },
    Page {
        name: "health",
        title: "Health",
        summary: "What Kanade and its Modules need from the session, as kanade doctor checks it",
        group: "System",
        icon: Icon::Check,
        sections: &[
            Section {
                title: "Checks",
                rows: &[Row::Health],
            },
            Section {
                title: "First start",
                rows: &[Row::Key("settings.health_notice")],
            },
        ],
    },
    Page {
        name: "niri",
        title: "niri",
        summary: "Edit niri's own config.kdl: its monitors, layout, focus, keyboard, touchpad, mouse and cursor. niri applies a change itself",
        group: "Compositor",
        icon: Icon::Columns,
        sections: &[
            Section {
                title: "Outputs",
                rows: &[Row::Outputs],
            },
            Section {
                title: "Layout",
                rows: &[
                    Row::Niri("layout.gaps"),
                    Row::Niri("layout.center-focused-column"),
                    Row::Niri("layout.always-center-single-column"),
                    Row::Niri("layout.empty-workspace-above-first"),
                    Row::Niri("layout.default-column-display"),
                    Row::Niri("layout.struts.left"),
                    Row::Niri("layout.struts.right"),
                    Row::Niri("layout.struts.top"),
                    Row::Niri("layout.struts.bottom"),
                ],
            },
            Section {
                title: "Focus ring and border",
                rows: &[
                    Row::Niri("layout.focus-ring"),
                    Row::Niri("layout.focus-ring.width"),
                    Row::Niri("layout.focus-ring.active-color"),
                    Row::Niri("layout.focus-ring.inactive-color"),
                    Row::Niri("layout.border"),
                    Row::Niri("layout.border.width"),
                    Row::Niri("layout.border.active-color"),
                    Row::Niri("layout.border.inactive-color"),
                    Row::Niri("layout.shadow"),
                ],
            },
            Section {
                title: "Input",
                rows: &[
                    Row::Niri("input.warp-mouse-to-focus"),
                    Row::Niri("input.focus-follows-mouse"),
                    Row::Niri("input.workspace-auto-back-and-forth"),
                    Row::Niri("input.disable-power-key-handling"),
                    Row::Niri("input.mod-key"),
                ],
            },
            Section {
                title: "Keyboard",
                rows: &[
                    Row::Niri("input.keyboard.repeat-delay"),
                    Row::Niri("input.keyboard.repeat-rate"),
                    Row::Niri("input.keyboard.numlock"),
                    Row::Niri("input.keyboard.track-layout"),
                    Row::Niri("input.keyboard.xkb.layout"),
                    Row::Niri("input.keyboard.xkb.variant"),
                    Row::Niri("input.keyboard.xkb.options"),
                ],
            },
            Section {
                title: "Touchpad",
                rows: &[
                    Row::Niri("input.touchpad.tap"),
                    Row::Niri("input.touchpad.natural-scroll"),
                    Row::Niri("input.touchpad.dwt"),
                    Row::Niri("input.touchpad.left-handed"),
                    Row::Niri("input.touchpad.middle-emulation"),
                    Row::Niri("input.touchpad.accel-speed"),
                    Row::Niri("input.touchpad.accel-profile"),
                    Row::Niri("input.touchpad.scroll-factor"),
                    Row::Niri("input.touchpad.scroll-method"),
                    Row::Niri("input.touchpad.click-method"),
                ],
            },
            Section {
                title: "Mouse",
                rows: &[
                    Row::Niri("input.mouse.natural-scroll"),
                    Row::Niri("input.mouse.left-handed"),
                    Row::Niri("input.mouse.middle-emulation"),
                    Row::Niri("input.mouse.accel-speed"),
                    Row::Niri("input.mouse.accel-profile"),
                    Row::Niri("input.mouse.scroll-factor"),
                    Row::Niri("input.mouse.scroll-method"),
                ],
            },
            Section {
                title: "Cursor",
                rows: &[
                    Row::Niri("cursor.xcursor-theme"),
                    Row::Niri("cursor.xcursor-size"),
                    Row::Niri("cursor.hide-when-typing"),
                    Row::Niri("cursor.hide-after-inactive-ms"),
                ],
            },
            Section {
                title: "Other",
                rows: &[
                    Row::Niri("overview.zoom"),
                    Row::Niri("animations.off"),
                    Row::Niri("animations.slowdown"),
                    Row::Niri("prefer-no-csd"),
                    Row::Niri("screenshot-path"),
                ],
            },
            Section {
                title: "Recommended rules",
                rows: &[
                    Row::NiriRule("blur-xray"),
                    Row::NiriRule("screencast-banners"),
                    Row::NiriRule("overview-backdrop"),
                ],
            },
        ],
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
    fn every_setting_is_shown_once_or_owned_by_another_control() {
        let shown: Vec<&str> = PAGES
            .iter()
            .flat_map(|page| page.sections)
            .flat_map(|section| section.rows)
            .flat_map(Row::keys)
            .collect();
        let settings: Vec<&str> = modules::ALL
            .iter()
            .flat_map(|module| module.settings)
            .map(|setting| setting.key)
            .collect();

        for key in EXTERNAL_KEYS {
            assert!(settings.contains(key), "{key} remains in the config schema");
            assert!(
                !shown.contains(key),
                "{key} belongs to its own control surface"
            );
        }
        for key in LEGACY_KEYS {
            assert!(settings.contains(key), "{key} remains in the config schema");
            assert!(!shown.contains(key), "{key} is no longer shown");
        }
        for key in &settings {
            let times = shown.iter().filter(|&&shown| shown == *key).count();
            assert_eq!(
                times,
                usize::from(!EXTERNAL_KEYS.contains(key) && !LEGACY_KEYS.contains(key)),
                "{key} shows {times} times"
            );
        }
        assert_eq!(
            shown.len(),
            settings.len() - EXTERNAL_KEYS.len() - LEGACY_KEYS.len()
        );
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
    fn every_niri_row_is_an_option_listed_once() {
        let rows: Vec<&str> = PAGES
            .iter()
            .flat_map(|page| page.sections)
            .flat_map(|section| section.rows)
            .filter_map(|row| match row {
                Row::Niri(path) => Some(*path),
                _ => None,
            })
            .collect();

        for path in &rows {
            assert!(
                super::super::niri::item(path).is_some(),
                "{path} is no option"
            );
            assert_eq!(rows.iter().filter(|&row| row == path).count(), 1, "{path}");
        }
        assert_eq!(rows.len(), super::super::niri::ITEMS.len());
    }

    #[test]
    fn every_niri_rule_is_a_preset_listed_once() {
        let rules: Vec<&str> = PAGES
            .iter()
            .flat_map(|page| page.sections)
            .flat_map(|section| section.rows)
            .filter_map(|row| match row {
                Row::NiriRule(id) => Some(*id),
                _ => None,
            })
            .collect();

        for id in &rules {
            assert!(
                crate::niri_config::preset(id).is_some(),
                "{id} is no preset"
            );
            assert_eq!(rules.iter().filter(|&rule| rule == id).count(), 1, "{id}");
        }
        assert_eq!(rules.len(), crate::niri_config::PRESETS.len());
    }

    #[test]
    fn page_names_are_unique() {
        for (index, page) in PAGES.iter().enumerate() {
            assert!(PAGES[..index].iter().all(|other| other.name != page.name));
        }
    }
}
