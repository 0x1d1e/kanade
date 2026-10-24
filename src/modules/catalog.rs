//! Every Module `modules` knows, with what each requires, its settings, verbs and start.

use std::time::Instant;

use kanade_runtime::Monitors;
use kanade_runtime::service::{self, Service};

use crate::cli::{Call, Verb};
use crate::clock::WallClock;
use crate::island::activity::{Ending, Leave};
use crate::island::command::{Command, Unparsed};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::sources::audio::Mixer;
use crate::sources::battery::BatteryLevel;
use crate::sources::bluetooth::Adapter;
use crate::sources::calendar::Calendars;
use crate::sources::clipboard::Clipboard;
use crate::sources::network::Connectivity;
use crate::sources::notifications::{Daemon, timed};
use crate::sources::system::Switch;
use crate::sources::tray::Tray;
use crate::sources::windows::Windows;
use crate::sources::{
    apps, audio, battery, bluetooth, brightness, caffeine, calendar, capture, charge_limit,
    clipboard, devices, dictation, focus, fullscreen, google, keys, launch, launcher_entry, live,
    lyrics, media, network, night_light,
    niri::{self, Niri},
    notifications, osd, pipewire, power, privacy, radios, recording, reminders, seat, session,
    sleep, subscriptions, system, timer, tray, wake, wallpaper, weather, windows, workspace,
};
use crate::{
    appearance, autohide, banners, cli, clock, cluster, config, dock, gesture, hooks, idle, ipc,
    lock, reload, settings, shadow, supervise, surfaces, ui, view,
};

use super::{CORE, Hooks, Module, Need, Provider, named, on, osd_reads, withheld};

// what a source that waits on an announcer does without it (`sources::wake`)
const POLLS: &str = "it polls instead, waking Kanade more often";

// every Module, each after the Modules it requires
pub const ALL: &[Module] = &[
    Module {
        name: CORE,
        about: "The Island, its Activities and Surfaces; always on",
        requires: &[],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::Program(wake::SETPRIV),
            without: "a helper program may outlive Kanade when it is killed",
        }],
        settings: config::ISLAND,
        hooks: Hooks::NONE,
        verbs: &[
            Verb {
                name: "island",
                about: "Collapse the open Surface",
                usage: || String::from("island collapse"),
                parse: |arguments| match arguments {
                    ["collapse"] => Ok(Call::Island(Command::Collapse)),
                    _ => Err(Unparsed::Usage),
                },
            },
            Verb {
                name: "config",
                about: "Reload, validate, print or change the config",
                usage: || String::from("config reload|validate"),
                parse: |arguments| match arguments {
                    ["reload"] => Ok(Call::Reload),
                    ["validate"] => Ok(Call::Validate),
                    _ => Err(Unparsed::Usage),
                },
            },
            Verb {
                name: "status",
                about: "Show config generation, reload error, pending restarts",
                usage: || String::from("status"),
                parse: |arguments| match arguments {
                    [] => Ok(Call::Status),
                    _ => Err(Unparsed::Usage),
                },
            },
            Verb {
                name: "module",
                about: "List Modules, turn one on or off",
                usage: || {
                    let names: Vec<&str> =
                        ALL.iter().filter_map(|module| named(module.name)).collect();

                    format!(
                        "module list|enable <name>|disable <name>\n  <name>: {}",
                        names.join("|")
                    )
                },
                parse: |arguments| {
                    match arguments {
                        ["list"] => Some(Call::Modules),
                        ["enable", name] => named(name).map(|name| Call::Turn(name, true)),
                        ["disable", name] => named(name).map(|name| Call::Turn(name, false)),
                        _ => None,
                    }
                    .ok_or(Unparsed::Usage)
                },
            },
            // fake Activities, to see what the island does with one
            Verb {
                name: "debug",
                about: "Post fake Activities to the island",
                usage: Command::debug_usage,
                parse: |arguments| Command::debug(arguments).map(Call::Island),
            },
        ],
        start: |app| {
            let config = config::get();

            IslandService::write().retime(config.animation_timings());
            /*
             * an output that comes, goes or changes may leave the largest body more or less room;
             * watched before the first read, so none comes between, and read under the write, so a
             * reload meanwhile cannot land first
             */
            service::watch::<Monitors>(|| view::resize(&mut IslandService::write()));
            service::watch::<Calendars>(|| view::resize(&mut IslandService::write()));
            service::watch::<WallClock>(|| view::resize(&mut IslandService::write()));
            service::watch::<privacy::Privacy>(|| view::resize(&mut IslandService::write()));
            view::resize(&mut IslandService::write());
            service::watch::<Monitors>(autohide::forget_gone);
            service::watch::<Monitors>(cluster::forget_gone);
            IslandService::write().withhold(&withheld());
            crate::surfaces::welcome::start_after_focus();
            reload::spawn();
            clock::spawn();
            shadow::prepare();

            // what Kanade's own niri stream says; the Modules that read it watch it in their own
            // start, and `modules::start` runs the stream once they have
            service::watch::<Niri>(|| {
                let seen = Niri::read().seen.clone();
                let mut island = IslandService::write();

                if !island.set_niri(seen.focused_output, seen.overview, Instant::now()) {
                    island.quiet();
                }
            });
            service::watch::<Niri>(fullscreen::follow_shown);

            // the keyboards, for the OSD's keys and lights; typing and key repeat are the runtime's
            if on("osd") {
                let reads = osd_reads();
                supervise::spawn("keys", move || keys::follow(reads));
            }

            app.window_per_monitor(view::reserve)
                .window_per_monitor(view::island)
                .ipc(cli::HANDLER, ipc::answer, ipc::draws)
        },
    },
    // a switch the island's niri stream tells of
    Module {
        name: "workspace",
        about: "A workspace switch, shown on the Island",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[],
        start: |app| {
            service::watch::<Niri>(workspace::follow);
            app
        },
    },
    // the running apps for the Dock (ADR 0014), from the island's niri stream
    Module {
        name: "windows",
        about: "Matches niri's windows to their apps, for the Dock",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::Niri,
            without: "no running windows",
        }],
        settings: config::WINDOWS,
        hooks: Hooks {
            status: Some(|| vec![Windows::read().status()]),
            reload: Some(windows::rematch),
        },
        verbs: &[],
        start: |app| {
            niri::on_windows(windows::hear);
            app
        },
    },
    // the pinned and running apps, bottom centre on every monitor
    Module {
        name: "dock",
        about: "Pinned and running apps",
        requires: &[CORE, "windows"],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Niri,
                without: "no running apps, and a click neither launches nor focuses",
            },
            Need {
                on: Provider::Program(launch::TERMINAL),
                without: "an app with `Terminal=true` does not launch",
            },
        ],
        settings: config::DOCK,
        hooks: Hooks {
            status: Some(|| vec![dock::status()]),
            reload: Some(dock::pin),
        },
        verbs: &[],
        start: |app| {
            dock::pin();
            service::watch::<Monitors>(dock::forget_gone);

            app.window_per_monitor(dock::reserve)
                .window_per_monitor(dock::window)
                .window_per_monitor(dock::context_window)
        },
    },
    // the privacy cluster: the microphone and camera from PipeWire, and screen casts from the
    // island's niri stream
    Module {
        name: "privacy",
        about: "Marks a microphone, camera or screen capture in use",
        requires: &[CORE],
        optional: &[],
        warns: Some("microphone, camera and screen cast indicators will not show"),
        needs: &[Need {
            on: Provider::Program(pipewire::DUMP),
            without: "microphone and camera indicators will not show",
        }],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[],
        start: |app| {
            service::watch::<Niri>(privacy::follow_casts);
            supervise::spawn("privacy", privacy::follow);
            app
        },
    },
    Module {
        name: "battery",
        about: "Battery state, the low-battery Activity, its critical action and charge limit",
        requires: &[CORE],
        optional: &["session"],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(battery::POWER.program),
                without: POLLS,
            },
            Need {
                on: Provider::SystemService(osd::UPOWER),
                without: "no charge limit",
            },
        ],
        settings: config::BATTERY,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "battery",
            about: "Limit the battery's charge, through UPower",
            usage: || String::from("battery limit on|off|status"),
            parse: |arguments| {
                charge_limit::Request::parse(arguments)
                    .map(Call::Battery)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            service::watch::<BatteryLevel>(devices::follow_charger);
            service::watch::<BatteryLevel>(battery::follow_charger_hook);
            supervise::spawn("battery", battery::follow);
            supervise::spawn("charge-limit", charge_limit::follow);
            app
        },
    },
    Module {
        name: "media",
        about: "What is playing, its track and transport",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::MEDIA,
        hooks: Hooks {
            status: Some(lyrics::status),
            reload: None,
        },
        verbs: &[
            Verb {
                name: "media",
                about: "Play, pause or skip the player, open or close the media Surface",
                usage: || String::from("media open|close|toggle"),
                parse: |arguments| {
                    Command::surface(Surface::Media, arguments)
                        .map(Call::Island)
                        .ok_or(Unparsed::Usage)
                },
            },
            Verb {
                name: "media",
                about: "Play, pause or skip the player, open or close the media Surface",
                usage: || {
                    String::from(
                        "media play-pause|next|previous
  play-pause, next, previous: ask the player playing, or else the one shown last",
                    )
                },
                parse: |arguments| {
                    media::Transport::parse(arguments)
                        .map(Call::Media)
                        .ok_or(Unparsed::Usage)
                },
            },
        ],
        start: |app| {
            supervise::spawn("media", media::follow);
            app
        },
    },
    Module {
        name: "timer",
        about: "A countdown on the Island",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::TIMER,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "timer",
            about: "Start a timer or a pomodoro; pause, resume, skip or cancel it",
            usage: || {
                format!(
                    "timer start <duration>|pomodoro [<focus> <break> <long-break> <rounds>]|pause|resume|skip|cancel
  <duration>, <focus>, <break>, <long-break>: like 90s, 25m or 1h30m, up to 24h
  <rounds>: focus rounds before the long break, 1-{}
  pomodoro: takes the lengths in order; those left off come from timer.pomodoro.*
  skip: moves the pomodoro to its next phase at once",
                    timer::ROUNDS
                )
            },
            parse: |arguments| {
                let request = match arguments {
                    ["start", length] => timer::duration(length).map(timer::Request::Start),
                    ["pomodoro", lengths @ ..] => {
                        timer::lengths(lengths).map(timer::Request::Pomodoro)
                    }
                    ["skip"] => Some(timer::Request::Skip(None)),
                    ["pause"] => Some(timer::Request::Pause),
                    ["resume"] => Some(timer::Request::Resume),
                    ["cancel"] => Some(timer::Request::Cancel),
                    _ => None,
                };
                request.map(Call::Timer).ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            timer::spawn();
            app
        },
    },
    // live Activities a script, a build or a download posts with `kanade activity` (ADR 0042)
    Module {
        name: "live",
        about: "Activities other programs show with `kanade activity`",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::LIVE,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "activity",
            about: "Show what a script or a command does as a live Activity",
            usage: live::usage,
            parse: |arguments| {
                live::Request::parse(arguments)
                    .map(Call::Live)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            supervise::spawn("live", live::follow);

            // a failed reload of niri's config shows as a failed one, heard from niri
            niri::on_config_failed(live::niri_config_failed);
            app
        },
    },
    // an app's progress through the Unity launcher entry signal, shown as a live Activity (ADR 0083)
    Module {
        name: "launcher-entry",
        about: "Progress apps report through their launcher entry, as live Activities",
        requires: &[CORE, "live"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[],
        start: |app| {
            supervise::spawn("launcher-entry", launcher_entry::follow);
            app
        },
    },
    // Audio: the default speaker and microphone in Controls and Media, and their OSD.
    // Kanade's own adapter adds the devices and app streams (ADR 0011); while it is off nothing
    // reads Audio
    Module {
        name: "audio",
        about: "Volume, devices and per-app levels",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(pipewire::DUMP),
                without: "no audio devices or app streams, only the default speaker and microphone",
            },
            Need {
                on: Provider::Program(audio::WPCTL),
                without: "no device selection or app volumes, only the default speaker and microphone",
            },
            Need {
                on: Provider::Program(pipewire::SET_METADATA),
                without: "no choosing a device that both plays and records as the default",
            },
        ],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[],
        start: |app| {
            service::watch::<Mixer>(devices::follow_output);
            supervise::spawn("audio", audio::follow);
            app
        },
    },
    // Brightness, the backlight, as `audio`
    Module {
        name: "brightness",
        about: "Display brightness",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::BRIGHTNESS,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "brightness",
            about: "Set the brightness of the backlight or of the focused monitor",
            usage: || {
                String::from(
                    "brightness set <percent>|up|down|status
  <percent>: 1-100
  up, down: 5 points
  With DDC/CI (brightness.backend), the monitor on the focused output",
                )
            },
            parse: |arguments| {
                brightness::Request::parse(arguments)
                    .map(Call::Brightness)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // with both of those off it shows only the lock lights, airplane mode and keyboard layouts
    Module {
        name: "osd",
        about: "Volume, brightness, keyboard backlight, mode and keyboard layout feedback",
        requires: &[CORE],
        optional: &["audio", "brightness"],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(osd::PULSE.program),
                without: POLLS,
            },
            Need {
                on: Provider::Program(osd::BACKLIGHT.program),
                without: POLLS,
            },
            Need {
                on: Provider::Keyboards,
                without: "a level key at its limit and Caps Lock and Num Lock show nothing",
            },
            Need {
                on: Provider::Rfkill,
                without: "airplane mode shows nothing",
            },
            Need {
                on: Provider::SystemService(osd::UPOWER),
                without: "the keyboard backlight shows nothing",
            },
        ],
        settings: config::OSD,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "osd",
            about: "Show the volume or brightness OSD",
            usage: || String::from("osd volume|brightness"),
            parse: |arguments| {
                osd::Asked::parse(arguments)
                    .map(Call::Osd)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            let reads = osd_reads();

            if reads.any() {
                supervise::spawn("osd", move || osd::follow(reads));
            }

            // the lock lights and airplane mode show whatever else is on; its keys and lights
            // are read with the island's keyboards
            supervise::spawn("radios", radios::follow);

            // the keyboard's backlight shows whatever else is on, heard from UPower; a machine
            // without one ends it at once, which is no fault
            supervise::spawn_once("kbd-backlight", osd::follow_keyboard);

            // a VT switch hands the keyboards and radios to another session, which shows nothing
            supervise::spawn("seat", seat::follow);

            // a keyboard layout switch shows whatever else is on, heard from niri
            niri::on_layout(osd::layout);

            // reads the volume fresh for `kanade osd volume`
            if reads.audio {
                let asks = osd::volume_asks();
                supervise::spawn("osd-volume", move || osd::answer_volume(&asks));
            }

            app
        },
    },
    Module {
        name: "notifications",
        about: "The notification daemon, its history and Do Not Disturb",
        requires: &[CORE],
        optional: &["live"],
        warns: None,
        needs: &[
            Need {
                on: Provider::SessionName(notifications::NAME),
                without: "no notifications arrive",
            },
            Need {
                on: Provider::Program(notifications::BUS.program),
                without: POLLS,
            },
        ],
        settings: config::NOTIFICATIONS,
        // Do Not Disturb only quiets notifications, so it goes with them; opening their Surface is
        // `notification-surface`'s
        hooks: Hooks {
            status: Some(|| {
                let mut lines = vec![Daemon::read().status()];

                if timed::ends().is_some() {
                    lines.push(format!("do not disturb {}", timed::status()));
                }

                lines
            }),
            ..Hooks::NONE
        },
        verbs: &[Verb {
            name: "notifications",
            about: "Clear, set do not disturb, open the list",
            usage: || {
                String::from(
                    "notifications clear
notifications dnd on|off|toggle|status
notifications dnd on <duration>
notifications dnd on until <HH:MM>
  <duration>: like 90s, 25m or 1h30m, up to 24h
  <HH:MM>: the next time the clock reads it",
                )
            },
            parse: |arguments| {
                let call = match arguments {
                    ["clear"] => Some(Call::ClearNotifications),
                    ["dnd", dnd @ ..] => Command::dnd(dnd)
                        .map(Call::Island)
                        .or_else(|| timed::Request::parse(dnd).map(Call::Dnd)),
                    _ => None,
                };
                call.ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            supervise::spawn("notifications", notifications::follow);

            // the app scan takes seconds, so it starts here for the first notifications to have
            // their app's icon (`notifications::app_icon`)
            apps::refresh();
            app
        },
    },
    Module {
        name: "banners",
        about: "Notifications coming out of the Island",
        requires: &["notifications"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks {
            reload: Some(banners::follow_cover),
            ..Hooks::NONE
        },
        verbs: &[],
        start: |app| {
            service::watch::<Monitors>(banners::forget_gone);
            service::watch::<Niri>(banners::follow_focus);
            service::watch::<Niri>(banners::follow_cover);
            service::watch::<kanade_runtime::Toplevels>(banners::follow_cover);
            service::watch::<crate::sources::fullscreen::Fullscreen>(banners::follow_cover);

            app.window_per_monitor(banners::window)
        },
    },
    // the three share one system bus watcher, which follows only the daemons of those that are on
    Module {
        name: "network",
        about: "Wi-Fi and connections, from NetworkManager",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::SystemService(network::NAME),
            without: "no network state or Wi-Fi controls",
        }],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "wifi",
            about: "Turn the Wi-Fi radio on or off",
            usage: || String::from("wifi on|off|toggle|status"),
            parse: |arguments| {
                Switch::parse(arguments)
                    .map(Call::Wifi)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            service::watch::<Connectivity>(devices::follow_wifi);
            system::spawn();
            app
        },
    },
    Module {
        name: "bluetooth",
        about: "Bluetooth devices and pairing",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::SystemService(bluetooth::BLUEZ),
            without: "no Bluetooth state or controls",
        }],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "bluetooth",
            about: "Turn the Bluetooth adapter on or off",
            usage: || String::from("bluetooth on|off|toggle|status"),
            parse: |arguments| {
                Switch::parse(arguments)
                    .map(Call::Bluetooth)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            service::watch::<Adapter>(devices::follow_bluetooth);
            system::spawn();
            app
        },
    },
    // the StatusNotifierItems apps show: the strip at Rest and the Tray Surface with their menus
    Module {
        name: "tray",
        about: "Apps' tray items and their menus",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks {
            status: Some(|| vec![Tray::read().status()]),
            ..Hooks::NONE
        },
        verbs: &[Verb {
            name: "tray",
            about: "Open or close the tray Surface",
            usage: || String::from("tray open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Tray, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            tray::follow();
            surfaces::tray::start();
            app
        },
    },
    // the clipboard history, kept only in memory; `clipboard-surface` shows it
    Module {
        name: "clipboard",
        about: "A history of what was copied, kept in memory",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Paste,
                without: "no clipboard history",
            },
            Need {
                on: Provider::Program(clipboard::COPY),
                without: "no restoring a clipboard entry",
            },
        ],
        settings: &[],
        // clearing is the history's own; opening its Surface is `clipboard-surface`'s
        hooks: Hooks {
            status: Some(|| vec![Clipboard::read().status()]),
            ..Hooks::NONE
        },
        verbs: &[Verb {
            name: "clipboard",
            about: "Clear the history, open its Surface",
            usage: || String::from("clipboard clear"),
            parse: |arguments| match arguments {
                ["clear"] => Ok(Call::ClearClipboard),
                _ => Err(Unparsed::Usage),
            },
        }],
        start: |app| {
            clipboard::follow();
            app
        },
    },
    // power profiles, for Controls
    Module {
        name: "power",
        about: "Power profiles",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::SystemService(power::NAME),
            without: "no power profile in Controls",
        }],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "power-profile",
            about: "Switch the power profile",
            usage: || {
                String::from(
                    "power-profile set <profile>|cycle|status
  <profile>: power-saver|balanced|performance, as the machine has them
  cycle: the next profile the machine has, then the first again",
                )
            },
            parse: |arguments| {
                power::Request::parse(arguments)
                    .map(Call::PowerProfile)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            system::spawn();
            app
        },
    },
    // the island's Surfaces: each off never opens, so what only it reads stays cold
    Module {
        name: "controls",
        about: "Quick controls for sound, network, Bluetooth and more",
        requires: &[CORE],
        optional: &[
            "notifications",
            "audio",
            "brightness",
            "network",
            "bluetooth",
            "power",
            "privacy",
        ],
        warns: None,
        needs: &[],
        settings: config::CONTROLS,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "controls",
            about: "Open or close the controls Surface",
            usage: || String::from("controls open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Controls, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // the wallpaper: Kanade selects, awww renders
    Module {
        name: "wallpaper",
        about: "Wallpapers to choose from, shown by awww",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(wallpaper::AWWW),
                without: "no setting the wallpaper",
            },
            Need {
                on: Provider::Program(wallpaper::DAEMON),
                without: "no wallpaper unless one runs already",
            },
            Need {
                on: Provider::Program(wake::SETPRIV),
                without: "no wallpaper unless awww-daemon runs already, as Kanade's could outlive it",
            },
            Need {
                on: Provider::Program(appearance::MATUGEN),
                without: "no wallpaper-derived shell colors or configured application templates",
            },
        ],
        settings: config::WALLPAPER,
        hooks: Hooks {
            status: Some(|| vec![wallpaper::status()]),
            ..Hooks::NONE
        },
        verbs: &[Verb {
            name: "wallpaper",
            about: "Set the wallpaper or one at random, show status, open its Surface",
            usage: || String::from("wallpaper set <path>|shuffle [--output <name>|all]|status"),
            parse: |arguments| {
                wallpaper::Request::parse(arguments)
                    .map(Call::Wallpaper)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            // holds awww's daemon only when none answers: with one running, or without setpriv,
            // there is nothing to hold, so its return is its end
            supervise::spawn_once("wallpaper", wallpaper::follow);
            supervise::spawn("wallpaper-set", wallpaper::serve);
            supervise::spawn("wallpaper-shown", wallpaper::watch);
            // fills an empty folder once, from the Favorites list: done, not stopped
            supervise::spawn_once("wallpaper-seed", wallpaper::seed);
            app
        },
    },
    Module {
        name: "launcher",
        about: "Apps, calculations, emoji and wallpapers",
        requires: &[CORE],
        optional: &["wallpaper", "windows"],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(clipboard::COPY),
                without: "no copying a calculator value or an emoji",
            },
            Need {
                on: Provider::Program(surfaces::launcher::web::OPEN),
                without: "no web search, and no address opened in the browser",
            },
        ],
        settings: config::LAUNCHER,
        hooks: Hooks::NONE,
        verbs: &[
            Verb {
                name: "launcher",
                about: "Open or close the launcher",
                usage: || String::from("launcher open|close|toggle"),
                parse: |arguments| {
                    Command::surface(Surface::Launcher, arguments)
                        .map(Call::Island)
                        .ok_or(Unparsed::Usage)
                },
            },
            Verb {
                name: "dmenu",
                about: "Choose one of the lines on stdin in the launcher",
                usage: surfaces::launcher::choose::usage,
                parse: |arguments| {
                    surfaces::launcher::choose::Asked::parse(arguments)
                        .map(Call::Dmenu)
                        .ok_or(Unparsed::Usage)
                },
            },
        ],
        start: |app| {
            // read now, so the Launcher opens on a list
            apps::refresh();
            surfaces::launcher::start();
            app.window_per_monitor(surfaces::launcher::detached::window)
        },
    },
    // the notification list on the island; Banners show them without it
    Module {
        name: "notification-surface",
        about: "Notifications to browse and act on",
        requires: &[CORE, "notifications"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "notifications",
            about: "Clear, set do not disturb, open the list",
            usage: || String::from("notifications open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Notifications, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            surfaces::notifications::start();
            app
        },
    },
    // the local calendars' events, read from iCalendar files; `calendar-surface` shows them
    Module {
        name: "calendar",
        about: "Events from local iCalendar files",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::CALENDAR,
        hooks: Hooks {
            reload: Some(calendar::reread),
            ..Hooks::NONE
        },
        verbs: &[],
        start: |app| {
            supervise::spawn("calendar", calendar::follow);
            app
        },
    },
    // a month and the agenda of a day on the island
    Module {
        name: "calendar-surface",
        about: "The month and its agenda",
        requires: &[CORE, "calendar"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "calendar",
            about: "Open or close the calendar",
            usage: || String::from("calendar open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Calendar, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // the events' reminders, on the island (ADR 0043)
    Module {
        name: "calendar-reminders",
        about: "Reminders of calendar events on the island",
        requires: &[CORE, "calendar"],
        optional: &["calendar-surface"],
        warns: None,
        needs: &[],
        settings: config::REMINDERS,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "reminders",
            about: "Dismiss the reminders shown, or say when the next is",
            usage: || String::from("reminders dismiss|status"),
            parse: |arguments| {
                reminders::Request::parse(arguments)
                    .map(Call::Reminders)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            reminders::spawn();
            service::watch::<Calendars>(reminders::changed);
            app
        },
    },
    /*
     * a Google account's calendars, synced into files the calendar reads (ADR 0016); idle until
     * signed in
     */
    Module {
        name: "google-calendar",
        about: "A Google account synced into local calendar files",
        requires: &[CORE, "calendar"],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::SessionService(google::SECRETS),
                without: "no signing in, as nothing keeps the credentials",
            },
            Need {
                on: Provider::Program(google::OPEN),
                without: "no browser opened to sign in; open the address sign-in prints",
            },
        ],
        settings: &[],
        hooks: Hooks {
            status: Some(|| vec![google::status()]),
            ..Hooks::NONE
        },
        verbs: &[Verb {
            name: "google-calendar",
            about: "Sign in to Google Calendar, sync, sign out",
            usage: || {
                String::from(
                    "google-calendar sign-in <client.json>|sign-out|sync|status
  <client.json>: a Desktop app OAuth client, as Google Cloud downloads it",
                )
            },
            parse: |arguments| {
                google::Request::parse(arguments)
                    .map(Call::Google)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            supervise::spawn("google-calendar", google::follow);
            app
        },
    },
    /*
     * public iCalendar feeds, fetched into read-only files `calendar` reads (ADR 0062); fetches
     * nothing without a `calendar.subscriptions`
     */
    Module {
        name: "calendar-subscriptions",
        about: "Public iCalendar feeds fetched into read-only calendars",
        requires: &[CORE, "calendar"],
        optional: &["calendar-surface"],
        warns: None,
        needs: &[],
        settings: config::SUBSCRIPTIONS,
        hooks: Hooks {
            status: Some(subscriptions::status),
            reload: Some(subscriptions::reread),
        },
        verbs: &[Verb {
            name: "calendar-subscriptions",
            about: "Fetch the calendar subscriptions now, or show how they stand",
            usage: || String::from("calendar-subscriptions sync|status"),
            parse: |arguments| {
                subscriptions::Request::parse(arguments)
                    .map(Call::Subscriptions)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            supervise::spawn("calendar-subscriptions", subscriptions::follow);
            app
        },
    },
    // the weather for `weather.location`, from Open-Meteo (ADR 0017); fetches nothing without one
    Module {
        name: "weather",
        about: "Open-Meteo forecasts for a set location",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::WEATHER,
        hooks: Hooks {
            status: Some(|| vec![weather::status()]),
            reload: Some(weather::reread),
        },
        verbs: &[Verb {
            name: "weather",
            about: "Refresh the weather, show status, open its Surface",
            usage: || String::from("weather refresh|status"),
            parse: |arguments| {
                weather::Request::parse(arguments)
                    .map(Call::Weather)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            supervise::spawn("weather", weather::follow);
            app
        },
    },
    // the weather now and the days ahead on the island
    Module {
        name: "weather-surface",
        about: "Current conditions and the forecast",
        requires: &[CORE, "weather"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "weather",
            about: "Refresh the weather, show status, open its Surface",
            usage: || String::from("weather open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Weather, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // the clipboard history on the island: search, copy, delete and clear
    Module {
        name: "clipboard-surface",
        about: "The clipboard history to browse and copy from",
        requires: &[CORE, "clipboard"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "clipboard",
            about: "Clear the history, open its Surface",
            usage: || String::from("clipboard open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Clipboard, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            surfaces::clipboard::start();
            app
        },
    },
    // the wallpaper folder on the island as a grid of pictures: search, choose, shuffle
    Module {
        name: "wallpaper-surface",
        about: "The wallpaper folder as a grid of pictures to choose from",
        requires: &[CORE, "wallpaper"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "wallpaper",
            about: "Set the wallpaper or one at random, show status, open its Surface",
            usage: || String::from("wallpaper open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Wallpapers, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // a countdown or a pomodoro to start from the island, and the one running
    Module {
        name: "timer-surface",
        about: "Start, pause and cancel a timer or a pomodoro",
        requires: &[CORE, "timer"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "timer",
            about: "Start a timer or a pomodoro; pause, resume, skip or cancel it, open its Surface",
            usage: || String::from("timer open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Timer, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // niri's keybinds on the island, read from its config, and the keys Kanade's Surfaces take
    Module {
        name: "keys-surface",
        about: "niri's keybinds and Kanade's own keys, to look up",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "keys",
            about: "Open the keybinds' Surface",
            usage: || String::from("keys open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Keys, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    /*
     * screenshots and the color picker, by niri, whose own notification says when each screenshot
     * is saved. Screen recording, by wf-recorder, which niri counts as a cast, so `privacy` shows it on its own;
     * a window's, by niri's screen cast through GStreamer and ffmpeg (ADR 0117)
     */
    Module {
        name: "capture",
        about: "Screenshots and screen recording",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Niri,
                without: "no screenshot backend",
            },
            Need {
                on: Provider::Program(clipboard::COPY),
                without: "no copying a screenshot's path or a picked color",
            },
            Need {
                on: Provider::Program(capture::OPEN),
                without: "no opening a screenshot or a recording",
            },
            Need {
                on: Provider::Program(recording::RECORDER),
                without: "no screen recording",
            },
            Need {
                on: Provider::Program(recording::PICKER),
                without: "no choosing an area to record (give it as x,y WxH)",
            },
            Need {
                on: Provider::Program(recording::JOINER),
                without: "no pausing a screen recording, and no recording a window",
            },
            Need {
                on: Provider::Program(recording::ENCODER),
                without: "no recording a window",
            },
        ],
        settings: config::RECORDING,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "capture",
            about: "Take a screenshot, pick a color or record the screen",
            usage: || {
                String::from(
                    "capture screenshot area|window|output\ncapture color\ncapture record start|pause|resume|stop|status\n  start [screen|area [<x,y WxH>]|window [<id>]] [none|system|mic]: none is the default; mic is recorded only when asked for; window is the focused one without an id\ncapture cast window [<id>]|monitor [<output>]|clear",
                )
            },
            parse: |arguments| match arguments {
                ["screenshot", mode] => capture::Mode::parse(mode)
                    .map(Call::Screenshot)
                    .ok_or(Unparsed::Usage),
                ["color"] => Ok(Call::PickColor),
                ["record", words @ ..] => recording::Request::parse(words)
                    .map(Call::Record)
                    .ok_or(Unparsed::Usage),
                ["cast", target @ ..] => capture::parse_cast(target)
                    .map(Call::Cast)
                    .ok_or(Unparsed::Usage),
                _ => Err(Unparsed::Usage),
            },
        }],
        // a paused window recording hears its window close; a session an earlier run left is stopped;
        // prewarm window pipelines as well as encoder choices in the background thread spawned at startup
        start: |app| {
            niri::on_windows(recording::heard);
            recording::reclaim();
            recording::prewarm();
            app
        },
    },
    // screenshots and the screen recording from the island
    Module {
        name: "capture-surface",
        about: "Screenshots and screen recording to choose from",
        requires: &[CORE, "capture"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "capture",
            about: "Take a screenshot or record the screen, open its Surface",
            usage: || String::from("capture open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Capture, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // column and window controls tied to the focused window
    Module {
        name: "window-surface",
        about: "Column and window controls from the Island",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "window",
            about: "Open the column and window controls Surface",
            usage: || String::from("window open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Window, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    /*
     * a stroke drawn with the pointer runs a verb of Kanade (ADR 0105 to 0109): the overlay opens
     * only on `kanade gesture`, which the user binds, so Kanade grabs no key. Inert until a
     * gesture is recorded
     */
    Module {
        name: "gesture",
        about: "Draw a pointer gesture to run a Kanade verb",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::GESTURES,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "gesture",
            about: "Draw a pointer gesture to run a verb, record or forget one",
            usage: || {
                format!(
                    "gesture\ngesture record <name>\ngesture forget <name>\ngesture list\n  <name>: {}",
                    gesture::NAME_RULE
                )
            },
            parse: |arguments| {
                gesture::Request::parse(arguments)
                    .map(Call::Gesture)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            gesture::spawn();
            app.window_per_monitor(gesture::window)
        },
    },
    // keeps the session from going idle while on, by systemd-inhibit, which only it starts
    Module {
        name: "caffeine",
        about: "Keeps the session awake on request",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(caffeine::INHIBIT),
                without: "no caffeine",
            },
            Need {
                on: Provider::Program(wake::SETPRIV),
                without: "no caffeine, which could outlive Kanade without it",
            },
        ],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "caffeine",
            about: "Keep the session from going idle",
            usage: || {
                String::from(
                    "caffeine on|off|toggle [<duration>]|status
  <duration>: like 90s, 25m or 1h30m, up to 24h; none keeps it on until turned off",
                )
            },
            parse: |arguments| {
                caffeine::Request::parse(arguments)
                    .map(Call::Caffeine)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    /*
     * speech to text through the Gemini API (ADR 0111): off by default, as the audio goes to Google.
     * Nothing records or sends without a key in the Secret Service and the user's consent, both
     * given with `kanade dictate`, which runs in the user's terminal
     */
    Module {
        name: "dictation",
        about: "Turns speech into text with the Gemini API",
        requires: &[CORE],
        optional: &["privacy"],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(dictation::run::RECORDER),
                without: "no dictation, which records with it",
            },
            Need {
                on: Provider::Program(wake::SETPRIV),
                without: "the recorder could outlive Kanade, keeping the microphone open",
            },
            Need {
                on: Provider::SessionService(google::SECRETS),
                without: "no dictation, which keeps the API key there",
            },
            Need {
                on: Provider::Program(clipboard::COPY),
                without: "the text cannot be put on the clipboard",
            },
        ],
        settings: config::DICTATE,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "dictate",
            about: "Dictate: speech to text through the Gemini API",
            usage: || {
                String::from(
                    "dictate toggle|cancel|status|key|consent|forget
  toggle: starts recording, or stops it and transcribes; needs a key and your consent; refused while the session is locked
  cancel: before the stop, drops the recording; after it the upload is under way and cannot be recalled, so this only discards what comes back
  status: how it stands, and whether the key and the consent are in place
  key: reads the Gemini API key from stdin (typed hidden at a terminal) into the Secret Service
  consent: shows what dictation sends to Google, and records that you accept it; needs a terminal
  forget: deletes the key and withdraws the consent; a recording not yet sent is then not sent",
                )
            },
            parse: |arguments| {
                dictation::Request::parse(arguments)
                    .map(Call::Dictate)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    /*
     * warms the screen by `wlsunset` (ADR 0046), all day or from sunset to sunrise, as
     * `night-light.mode` says; held by its own thread, so it dies with Kanade
     */
    Module {
        name: "night-light",
        about: "Warms the screen at night",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(night_light::WLSUNSET),
                without: "no night light",
            },
            Need {
                on: Provider::Program(wake::SETPRIV),
                without: "no night light, which could outlive Kanade without it",
            },
        ],
        settings: config::NIGHT_LIGHT,
        hooks: Hooks {
            reload: Some(night_light::reread),
            ..Hooks::NONE
        },
        verbs: &[Verb {
            name: "night-light",
            about: "Warm the screen at night",
            usage: || String::from("night-light on|off|toggle|status"),
            parse: |arguments| {
                night_light::Request::parse(arguments)
                    .map(Call::NightLight)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            supervise::spawn("night-light", night_light::follow);
            app
        },
    },
    /*
     * one switch for do not disturb, caffeine and the night light (ADR 0060), through the Modules
     * that own them; each is optional, so Focus runs with whichever are on
     */
    Module {
        name: "focus",
        about: "Quiets the session with one switch",
        requires: &[CORE],
        optional: &["notifications", "caffeine", "night-light", "privacy"],
        warns: None,
        needs: &[],
        settings: config::FOCUS,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "focus",
            about: "Quiet the session",
            usage: || String::from("focus on|off|toggle|status"),
            parse: |arguments| {
                focus::Request::parse(arguments)
                    .map(Call::Focus)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            service::watch::<privacy::Privacy>(focus::follow_cast);
            app
        },
    },
    /*
     * the lock screen (ADR 0018), drawn by `kanade-lock` (ADR 0025), in the shell process; at
     * start it locks again a session logind still counts as locked, which a crash left on niri's
     * red screen
     */
    Module {
        name: "lock",
        about: "The lock screen and its password check",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Pam,
                without: "no password unlocks, so `kanade lock` refuses",
            },
            Need {
                on: Provider::SystemService("org.freedesktop.login1"),
                without: "no locking again after a crash while locked, nor before sleep",
            },
            Need {
                on: Provider::Program(caffeine::INHIBIT),
                without: "sleep may come before the lock",
            },
            Need {
                on: Provider::Program(wake::SETPRIV),
                without: "sleep may come before the lock",
            },
            Need {
                on: Provider::Program(wallpaper::AWWW),
                without: "the lock screen shows a solid color, as when another tool sets the wallpaper",
            },
        ],
        settings: config::LOCK,
        hooks: Hooks {
            status: Some(lock::lines),
            ..Hooks::NONE
        },
        verbs: &[Verb {
            name: "lock",
            about: "Lock the session",
            usage: || String::from("lock [status]"),
            parse: |arguments| match arguments {
                [] => Ok(Call::Lock),
                ["status"] => Ok(Call::LockStatus),
                _ => Err(Unparsed::Usage),
            },
        }],
        start: |app| {
            lock::relock();
            sleep::spawn();

            // the layout in use shows in the password field; a hint, which the lock never needs
            niri::on_keyboard(lock::keyboard);
            app.window_per_monitor(lock::scene)
        },
    },
    /*
     * the login screen's look (`kanade sddm`): keys only, read by a command that runs without a
     * shell; the shell only follows the wallpaper (`sddm.sync`) and says once at start if the
     * overlay SDDM reads is out of date
     */
    Module {
        name: "sddm",
        about: "The login screen's look, written by `kanade sddm sync`",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::SDDM,
        hooks: Hooks::NONE,
        verbs: &[],
        start: |app| {
            crate::sddm::check_at_start();
            app
        },
    },
    /*
     * the Session Surface and `kanade session`: lock, by the lock Module, then sleep, restart,
     * power off and log out by logind, the last three after a countdown that cancels
     */
    Module {
        name: "session",
        about: "Sleep, restart, power off and log out",
        requires: &[CORE],
        optional: &["lock"],
        warns: None,
        needs: &[Need {
            on: Provider::SystemService("org.freedesktop.login1"),
            without: "no sleep, restart, power off or log out",
        }],
        settings: &[],
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "session",
            about: "Open the session menu, suspend, reboot, power off, log out",
            usage: || String::from("session menu|suspend|reboot|poweroff|logout"),
            parse: |arguments| match arguments {
                ["menu"] => Ok(Call::Island(Command::Open(Surface::Session))),
                ["suspend"] => Ok(Call::Session(Leave::Sleep)),
                ["reboot"] => Ok(Call::Session(Leave::Ending(Ending::Restart))),
                ["poweroff"] => Ok(Call::Session(Leave::Ending(Ending::PowerOff))),
                ["logout"] => Ok(Call::Session(Leave::Ending(Ending::LogOut))),
                _ => Err(Unparsed::Usage),
            },
        }],
        start: |app| {
            session::spawn();
            app
        },
    },
    /*
     * idle in the shell (ADR 0047): a dim grace, then the lock, screens off and suspend; off by
     * default, as hypridle may already do it
     */
    Module {
        name: "idle",
        about: "Dims, locks, turns the screens off and suspends when idle",
        requires: &[CORE],
        optional: &["lock", "session"],
        warns: None,
        needs: &[Need {
            on: Provider::SystemService("org.freedesktop.login1"),
            without: "logind's idle inhibitors are not honored, caffeine's only directly, and nothing suspends",
        }],
        settings: config::IDLE,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "idle",
            about: "Show the idle stages and what inhibits them",
            usage: || String::from("idle status"),
            parse: |arguments| match arguments {
                ["status"] => Ok(Call::IdleStatus),
                _ => Err(Unparsed::Usage),
            },
        }],
        start: |app| {
            idle::spawn();
            app.window_per_monitor(idle::overlay::overlay)
        },
    },
    // the Settings window, which writes only the settings file
    Module {
        name: "settings",
        about: "This window",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::SETTINGS,
        hooks: Hooks::NONE,
        verbs: &[Verb {
            name: "settings",
            about: "Open or close the Settings window",
            usage: || {
                let pages: Vec<&str> = settings::pages().collect();

                format!(
                    "settings open [<page>]|close\n  <page>: {}",
                    pages.join("|")
                )
            },
            parse: |arguments| match arguments {
                ["open"] => Ok(Call::Settings(None)),
                ["open", page] => settings::page(page)
                    .map(|page| Call::Settings(Some(page)))
                    .ok_or(Unparsed::Usage),
                ["close"] => Ok(Call::CloseSettings),
                _ => Err(Unparsed::Usage),
            },
        }],
        start: |app| {
            settings::spawn_notice();
            app
        },
    },
    // the windows' semantic tree and acts by name, for scripts, tests and agents (ADR 0095)
    Module {
        name: ui::MODULE,
        about: "The windows as a named tree to read and press by name, with kanade ui",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::UI,
        hooks: Hooks {
            status: Some(ui::status),
            reload: Some(ui::reread),
        },
        verbs: &[Verb {
            name: "ui",
            about: "Read the windows as a tree, press, type and wait by name",
            usage: ui::usage,
            parse: ui::parse,
        }],
        start: |app| {
            ui::reread();
            app
        },
    },
    /*
     * commands the user lists for events (ADR 0055); the Module that owns an event fires it, so
     * this one has no thread or window of its own, and off, it makes every fire do nothing
     */
    Module {
        name: hooks::MODULE,
        about: "Runs your commands on lock, unlock, wallpaper, tone, charger, suspend, resume, start, Do Not Disturb, power profile, log out, restart and power off",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: hooks::SETTINGS,
        hooks: Hooks {
            status: Some(hooks::status),
            ..Hooks::NONE
        },
        verbs: &[],
        // last of the Modules, so every one that is on has started
        start: |app| {
            hooks::fire(hooks::Event::Started);
            app
        },
    },
];
