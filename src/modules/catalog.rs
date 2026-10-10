//! Every Module `modules` knows, with what each requires, its settings, verbs and start.

use std::time::Instant;

use kanade_runtime::Monitors;
use kanade_runtime::service::{self, Service};

use crate::cli::{Call, Verb};
use crate::island::activity::{Ending, Leave};
use crate::island::command::{Command, Unparsed};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::sources::{
    apps, audio, battery, bluetooth, caffeine, calendar, capture, clipboard, google, keys, launch,
    media, network, niri, notifications, osd, pipewire, power, privacy, radios, recording, seat,
    session, sleep, system, timer, tray, wake, wallpaper, weather,
};
use crate::{
    autohide, banners, cli, clock, config, dock, ipc, lock, reload, settings, shadow, supervise,
    surfaces, theme, view,
};

use super::{CORE, Module, Need, Provider, named, on, osd_reads, withheld};

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
                about: "Reload, validate or print the config",
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

            IslandService::write().retime(config.island);
            /*
             * an output that comes, goes or changes may leave the largest body more or less room;
             * watched before the first read, so none comes between, and read under the write, so a
             * reload meanwhile cannot land first
             */
            service::watch::<Monitors>(|| {
                IslandService::write().resize(
                    view::largest(),
                    view::hang(),
                    view::peek(),
                    Instant::now(),
                );
            });
            IslandService::write().resize(
                view::largest(),
                view::hang(),
                view::peek(),
                Instant::now(),
            );
            service::watch::<Monitors>(autohide::forget_gone);
            IslandService::write().withhold(&withheld());
            theme::follow(config.palette.as_deref());
            reload::spawn();
            clock::spawn();
            shadow::prepare();

            // Kanade's own niri stream, which `workspace`, `windows`, `privacy`, `banners` and
            // `capture` also read when on
            let posts = niri::Posts {
                workspace: on("workspace"),
                privacy: on("privacy"),
                banners: on("banners"),
                capture: on("capture"),
                windows: on("windows"),
            };
            supervise::spawn("niri", move || niri::follow(posts));

            // the keyboards, for the OSD's keys and lights; typing and key repeat are the runtime's
            if on("osd") {
                let reads = osd_reads();
                supervise::spawn("keys", move || keys::follow(reads));
            }

            app.window_per_monitor(view::reserve)
                .window_per_monitor(view::island)
                .ipc(cli::HANDLER, ipc::answer)
        },
    },
    // read from the island's niri stream, so it starts nothing of its own
    Module {
        name: "workspace",
        about: "A workspace switch, shown on the Island",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[],
        start: |app| app,
    },
    // the running apps for the Dock (ADR 0014), also read from the island's niri stream
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
        verbs: &[],
        start: |app| app,
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
        verbs: &[],
        start: |app| {
            dock::pin();
            service::watch::<Monitors>(dock::forget_gone);

            app.window_per_monitor(dock::reserve)
                .window_per_monitor(dock::window)
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
        verbs: &[],
        start: |app| {
            supervise::spawn("privacy", privacy::follow);
            app
        },
    },
    Module {
        name: "battery",
        about: "Battery state and the low-battery Activity",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::Program(battery::POWER.program),
            without: POLLS,
        }],
        settings: &[],
        verbs: &[],
        start: |app| {
            supervise::spawn("battery", battery::follow);
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
        verbs: &[Verb {
            name: "media",
            about: "Open or close the media Surface",
            usage: || String::from("media open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Media, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
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
        settings: &[],
        verbs: &[Verb {
            name: "timer",
            about: "Start, pause, resume or cancel the timer",
            usage: || {
                String::from(
                    "timer start <duration>|pause|resume|cancel
  <duration>: like 90s, 25m or 1h30m, up to 24h",
                )
            },
            parse: |arguments| {
                let request = match arguments {
                    ["start", length] => timer::duration(length).map(timer::Request::Start),
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
        verbs: &[],
        start: |app| {
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
        settings: &[],
        verbs: &[],
        start: |app| app,
    },
    // with both of those off it shows only the lock lights and airplane mode
    Module {
        name: "osd",
        about: "Volume, brightness, keyboard backlight and mode feedback",
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
        settings: &[],
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

            // the keyboard's backlight shows whatever else is on, heard from UPower
            supervise::spawn("kbd-backlight", osd::follow_keyboard);

            // a VT switch hands the keyboards and radios to another session, which shows nothing
            supervise::spawn("seat", seat::follow);

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
        optional: &[],
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
        settings: &[],
        // Do Not Disturb only quiets notifications, so it goes with them; opening their Surface is
        // `notification-surface`'s
        verbs: &[Verb {
            name: "notifications",
            about: "Clear, set do not disturb, open the list",
            usage: || String::from("notifications clear\nnotifications dnd on|off|toggle"),
            parse: |arguments| {
                let call = match arguments {
                    ["clear"] => Some(Call::ClearNotifications),
                    ["dnd", dnd @ ..] => Command::dnd(dnd).map(Call::Island),
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
        verbs: &[],
        start: |app| {
            service::watch::<Monitors>(banners::forget_gone);

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
        verbs: &[],
        start: |app| {
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
        verbs: &[],
        start: |app| {
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
        verbs: &[],
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
        settings: &[],
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
        ],
        settings: config::WALLPAPER,
        verbs: &[Verb {
            name: "wallpaper",
            about: "Set the wallpaper or show the current one",
            usage: || String::from("wallpaper set <path>|status"),
            parse: |arguments| {
                wallpaper::Request::parse(arguments)
                    .map(Call::Wallpaper)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            supervise::spawn("wallpaper", wallpaper::follow);
            supervise::spawn("wallpaper-set", wallpaper::serve);
            app
        },
    },
    Module {
        name: "launcher",
        about: "Apps, calculations, emoji and wallpapers",
        requires: &[CORE],
        optional: &["wallpaper"],
        warns: None,
        needs: &[Need {
            on: Provider::Program(clipboard::COPY),
            without: "no copying a calculator value or an emoji",
        }],
        settings: &[],
        verbs: &[Verb {
            name: "launcher",
            about: "Open or close the launcher",
            usage: || String::from("launcher open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Launcher, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            // read now, so the Launcher opens on a list
            apps::refresh();
            surfaces::launcher::start();
            app
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
    // the weather for `weather.location`, from Open-Meteo (ADR 0017); fetches nothing without one
    Module {
        name: "weather",
        about: "Open-Meteo forecasts for a set location",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: config::WEATHER,
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
    /*
     * screenshots, by niri; its stream, which the core follows, says when each is saved. Screen
     * recording, by wf-recorder, which niri counts as a cast, so `privacy` shows it on its own
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
                without: "no copying a screenshot's path",
            },
            Need {
                on: Provider::Program(capture::OPEN),
                without: "no opening a screenshot or a recording",
            },
            Need {
                on: Provider::Program(recording::RECORDER),
                without: "no screen recording",
            },
        ],
        settings: &[],
        verbs: &[Verb {
            name: "capture",
            about: "Take a screenshot or record the screen",
            usage: || {
                String::from(
                    "capture screenshot area|window|output\ncapture record start|stop|status",
                )
            },
            parse: |arguments| match arguments {
                ["screenshot", mode] => capture::Mode::parse(mode)
                    .map(Call::Screenshot)
                    .ok_or(Unparsed::Usage),
                ["record", request] => recording::Request::parse(request)
                    .map(Call::Record)
                    .ok_or(Unparsed::Usage),
                _ => Err(Unparsed::Usage),
            },
        }],
        start: |app| app,
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
                on: Provider::Pam(lock::PAM),
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
            app.window_per_monitor(lock::scene)
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
    // the Settings window, which writes only the settings file
    Module {
        name: "settings",
        about: "This window",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
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
        start: |app| app,
    },
];
