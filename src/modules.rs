//! Modules (#94, docs/design.md Modules): every feature the user can turn off. A Module names the
//! Modules it requires and those it can do without; `resolve` decides once at start which run, and
//! `start` starts only those. A Module that is off starts no thread, opens no window, reads no
//! Service and refuses its verbs, so the Amane Services only it reads stay cold. Turning one on or
//! off takes a restart, since Amane registers windows only at start.

use std::sync::OnceLock;
use std::thread;

use amane::{App, Apps, Service};

use crate::cli::{Call, Verb};
use crate::island::command::{Command, Unparsed};
use crate::island::presentation::Surface;
use crate::island::service::IslandService;
use crate::sources::{
    audio, battery, bluetooth, caffeine, capture, clipboard, media, network, niri, notifications,
    osd, pipewire, power, privacy, recording, system, timer, tray, wake,
};
use crate::{
    banners, cli, clock, cluster, config, dock, ipc, reload, shadow, supervise, theme, view,
};

pub struct Module {
    pub name: &'static str,

    // hard: one that is not on turns this one off, saying which
    pub requires: &'static [&'static str],

    // soft: one that is not on leaves this one running without what it gives
    pub optional: &'static [&'static str],

    // what the user loses with it off, said at start whenever it is, since that loss is easy to miss
    pub warns: Option<&'static str>,

    // what outside Kanade it runs worse without, for `kanade doctor`
    pub needs: &'static [Need],

    // the config keys it owns, which others may read too; a key no Module owns is unknown
    pub settings: &'static [config::Setting],

    // its CLI schema: the verbs of `kanade <verb>` it owns, which the shell refuses while it is off
    pub verbs: &'static [Verb],

    /*
     * starts its threads and adds its windows to the shell; runs once at start, after
     * every Module it requires
     */
    pub start: fn(App) -> App,
}

// something outside Kanade a Module runs worse without, never refusing to start
pub struct Need {
    pub on: Provider,

    // what the user loses while it is missing
    pub without: &'static str,
}

pub enum Provider {
    // a helper program on PATH
    Program(&'static str),

    // a system bus service, running or started on demand
    SystemService(&'static str),

    // a session bus name Kanade takes, which no other program may hold
    SessionName(&'static str),

    // niri's IPC, which the Module cannot work without
    Niri,
}

// what a source that waits on an announcer does without it (`sources::wake`)
const POLLS: &str = "it polls instead, waking Kanade more often";

// the one Module that cannot be turned off: without it no window opens, and Amane needs one
pub const CORE: &str = "island";

// every Module, each after the Modules it requires
pub const ALL: &[Module] = &[
    Module {
        name: CORE,
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
                usage: || String::from("island collapse"),
                parse: |arguments| match arguments {
                    ["collapse"] => Ok(Call::Island(Command::Collapse)),
                    _ => Err(Unparsed::Usage),
                },
            },
            Verb {
                name: "config",
                usage: || String::from("config reload|validate"),
                parse: |arguments| match arguments {
                    ["reload"] => Ok(Call::Reload),
                    ["validate"] => Ok(Call::Validate),
                    _ => Err(Unparsed::Usage),
                },
            },
            Verb {
                name: "status",
                usage: || String::from("status"),
                parse: |arguments| match arguments {
                    [] => Ok(Call::Status),
                    _ => Err(Unparsed::Usage),
                },
            },
            // fake Activities, to see what the island does with one
            Verb {
                name: "debug",
                usage: Command::debug_usage,
                parse: |arguments| Command::debug(arguments).map(Call::Island),
            },
        ],
        start: |app| {
            let config = config::get();

            IslandService::write().retime(config.island);
            IslandService::write().withhold(&withheld());
            theme::follow(config.palette.as_deref());
            reload::spawn();
            clock::spawn();
            shadow::prepare(shadow::ShadowStyle::island());

            // Kanade's own niri stream, which `workspace`, `windows`, `privacy`, `banners`, `osd`
            // and `capture` also read when on
            let posts = niri::Posts {
                workspace: on("workspace"),
                privacy: on("privacy"),
                banners: on("banners"),
                osd: on("osd"),
                capture: on("capture"),
                windows: on("windows"),
            };
            supervise::spawn("niri", move || niri::follow(posts));

            app.window_per_monitor(view::island)
                .ipc(cli::HANDLER, ipc::answer)
        },
    },
    // read from the island's niri stream, so it starts nothing of its own
    Module {
        name: "workspace",
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
        requires: &[CORE, "windows"],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::Niri,
            without: "no running apps, and a click neither launches nor focuses",
        }],
        settings: config::DOCK,
        verbs: &[],
        start: |app| {
            dock::pin();
            app.window_per_monitor(dock::window)
        },
    },
    // the privacy cluster: the microphone and camera from PipeWire, and screen casts from the
    // island's niri stream
    Module {
        name: "privacy",
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
            app.window_per_monitor(cluster::window)
        },
    },
    Module {
        name: "battery",
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
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[Verb {
            name: "media",
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
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[Verb {
            name: "timer",
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
    // Amane's Audio: the default speaker and microphone in Controls and Media, and their OSD.
    // Kanade's own adapter adds the devices and app streams (ADR 0011); while it is off nothing
    // reads Audio
    Module {
        name: "audio",
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
    // Amane's Brightness, the backlight, as `audio`
    Module {
        name: "brightness",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[],
        start: |app| app,
    },
    // with both of those off it has nothing to show, so it starts no thread
    Module {
        name: "osd",
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
        ],
        settings: &[],
        verbs: &[Verb {
            name: "osd",
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

            // reads the volume fresh for `kanade osd volume`
            if reads.audio {
                let asks = osd::volume_asks();
                supervise::spawn("osd-volume", move || osd::answer_volume(&asks));
            }

            app.window_per_monitor(crate::osd::window)
        },
    },
    Module {
        name: "notifications",
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
            app
        },
    },
    Module {
        name: "banners",
        requires: &["notifications"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[],
        start: |app| app.window_per_monitor(banners::window),
    },
    // the three share one system bus watcher, which follows only the daemons of those that are on
    Module {
        name: "network",
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
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[Verb {
            name: "tray",
            usage: || String::from("tray open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Tray, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            tray::follow();
            app
        },
    },
    // the clipboard history, kept only in memory; `clipboard-surface` shows it
    Module {
        name: "clipboard",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[
            Need {
                on: Provider::Program(clipboard::PASTE),
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
            usage: || String::from("controls open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Controls, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    Module {
        name: "launcher",
        requires: &[CORE],
        optional: &[],
        warns: None,
        needs: &[Need {
            on: Provider::Program(clipboard::COPY),
            without: "no copying a calculator value or an emoji",
        }],
        settings: &[],
        verbs: &[Verb {
            name: "launcher",
            usage: || String::from("launcher open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Launcher, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| {
            // the first read starts Amane's app scan, which takes seconds, so the Launcher opens on a list
            thread::spawn(|| drop(Apps::read()));
            app
        },
    },
    // the notification list on the island; Banners show them without it
    Module {
        name: "notification-surface",
        requires: &[CORE, "notifications"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[Verb {
            name: "notifications",
            usage: || String::from("notifications open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Notifications, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    // the clipboard history on the island: search, copy, delete and clear
    Module {
        name: "clipboard-surface",
        requires: &[CORE, "clipboard"],
        optional: &[],
        warns: None,
        needs: &[],
        settings: &[],
        verbs: &[Verb {
            name: "clipboard",
            usage: || String::from("clipboard open|close|toggle"),
            parse: |arguments| {
                Command::surface(Surface::Clipboard, arguments)
                    .map(Call::Island)
                    .ok_or(Unparsed::Usage)
            },
        }],
        start: |app| app,
    },
    /*
     * screenshots, by niri; its stream, which the core follows, says when each is saved. Screen
     * recording, by wf-recorder, which niri counts as a cast, so `privacy` shows it on its own
     */
    Module {
        name: "capture",
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
];

// each Surface the island opens, with the Module that draws it
const SURFACES: [(Surface, &str); 6] = [
    (Surface::Media, "media"),
    (Surface::Notifications, "notification-surface"),
    (Surface::Controls, "controls"),
    (Surface::Launcher, "launcher"),
    (Surface::Tray, "tray"),
    (Surface::Clipboard, "clipboard-surface"),
];

// the Surfaces whose Module is off, which the island never opens
fn withheld() -> Vec<Surface> {
    SURFACES
        .iter()
        .filter(|(_, module)| !on(module))
        .map(|&(surface, _)| surface)
        .collect()
}

// what became of a Module at start
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    // running, without the optional Modules named
    On { without: Vec<&'static str> },

    // turned off in the config
    Off,

    // off, since this Module it requires is not on
    Missing(&'static str),

    // off, since it requires itself through these, in order
    Cycle(Vec<&'static str>),
}

impl State {
    // why a Module is not as asked, for stderr; none for one that runs whole or was turned off
    pub fn problem(&self, name: &str) -> Option<String> {
        match self {
            State::On { without } if without.is_empty() => None,
            State::On { without } => {
                Some(format!("module {name} runs without {}", without.join(", ")))
            }
            State::Off => None,
            State::Missing(required) => Some(format!(
                "module {name} is off: it requires {required}, which is not on"
            )),
            State::Cycle(cycle) => Some(format!(
                "module {name} is off: its requirements loop: {} -> {}",
                cycle.join(" -> "),
                cycle[0]
            )),
        }
    }
}

// every Module's State, in the order given
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Modules {
    states: Vec<(&'static str, State)>,
}

impl Modules {
    pub fn state(&self, name: &str) -> Option<&State> {
        self.states
            .iter()
            .find(|(each, _)| *each == name)
            .map(|(_, state)| state)
    }

    pub fn on(&self, name: &str) -> bool {
        matches!(self.state(name), Some(State::On { .. }))
    }

    fn lines(&self) -> Vec<String> {
        self.states
            .iter()
            .map(|(name, state)| {
                state.problem(name).unwrap_or_else(|| match state {
                    State::On { .. } => format!("module {name} is on"),
                    _ => format!("module {name} is off"),
                })
            })
            .collect()
    }
}

// pure: the Modules, and which the config turns off, to what runs
pub fn resolve(all: &[Module], off: impl Fn(&str) -> bool) -> Modules {
    let cycles = cycles(all);

    // hard requirements only, which have no cycle left to follow
    let mut hard: Vec<Option<State>> = vec![None; all.len()];

    for index in 0..all.len() {
        settle(all, &cycles, &off, &mut hard, index);
    }

    let running = |name: &str| {
        all.iter()
            .position(|module| module.name == name)
            .is_some_and(|index| matches!(hard[index], Some(State::On { .. })))
    };

    let states = all
        .iter()
        .zip(&hard)
        .map(|(module, state)| {
            let state = match state.clone().expect("every module settled") {
                State::On { .. } => State::On {
                    without: module
                        .optional
                        .iter()
                        .copied()
                        .filter(|name| !running(name))
                        .collect(),
                },
                other => other,
            };

            (module.name, state)
        })
        .collect();

    Modules { states }
}

// a Module's State from its own setting and its requirements, each settled first
fn settle(
    all: &[Module],
    cycles: &[Option<Vec<&'static str>>],
    off: &impl Fn(&str) -> bool,
    hard: &mut [Option<State>],
    index: usize,
) -> State {
    if let Some(state) = &hard[index] {
        return state.clone();
    }

    let module = &all[index];

    let state = if let Some(cycle) = &cycles[index] {
        State::Cycle(cycle.clone())
    } else if off(module.name) {
        State::Off
    } else {
        module
            .requires
            .iter()
            .find(|required| {
                all.iter()
                    .position(|other| other.name == **required)
                    .is_none_or(|other| {
                        !matches!(settle(all, cycles, off, hard, other), State::On { .. })
                    })
            })
            .map_or(
                State::On {
                    without: Vec::new(),
                },
                |required| State::Missing(required),
            )
    };

    hard[index] = Some(state.clone());
    state
}

// for each Module on a cycle of requirements, that cycle starting at it
fn cycles(all: &[Module]) -> Vec<Option<Vec<&'static str>>> {
    (0..all.len()).map(|start| cycle(all, start)).collect()
}

// depth first from `start`, looking for a way back to it
fn cycle(all: &[Module], start: usize) -> Option<Vec<&'static str>> {
    let index = |name: &str| all.iter().position(|module| module.name == name);

    // each step's Module, and how many of its requirements were tried
    let mut path = vec![(start, 0)];

    while let Some((at, tried)) = path.last_mut() {
        let Some(required) = all[*at].requires.get(*tried) else {
            path.pop();
            continue;
        };

        *tried += 1;

        let Some(required) = index(required) else {
            continue;
        };

        if required == start {
            return Some(path.iter().map(|&(at, _)| all[at].name).collect());
        }

        if path.iter().all(|&(at, _)| at != required) {
            path.push((required, 0));
        }
    }

    None
}

static MODULES: OnceLock<Modules> = OnceLock::new();

/*
 * resolves the Modules against the config, says on stderr what is not as asked, and starts every
 * Module that is on, in order; called once, before the shell runs
 */
pub fn start(mut app: App) -> App {
    let modules = MODULES.get_or_init(|| resolve(ALL, |name| config::get().off(name)));

    for module in ALL {
        let state = modules.state(module.name).expect("every module resolved");

        if let Some(problem) = state.problem(module.name) {
            eprintln!("kanade: {problem}");
        }

        match (state, module.warns) {
            (State::On { .. }, _) => app = (module.start)(app),
            (_, Some(warning)) => eprintln!("kanade: module {} is off: {warning}", module.name),
            (_, None) => {}
        }
    }

    app
}

// a line for each Module, whether it runs and why it is not as asked
pub fn status() -> Vec<String> {
    MODULES.get().map_or_else(Vec::new, Modules::lines)
}

// whether a Module runs; none do before `start`
pub fn on(name: &str) -> bool {
    MODULES.get().is_some_and(|modules| modules.on(name))
}

// the levels the OSD reads: those of the `audio` and `brightness` Modules that are on
pub fn osd_reads() -> osd::Reads {
    osd::Reads {
        audio: on("audio"),
        brightness: on("brightness"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn module(name: &'static str, requires: &'static [&'static str]) -> Module {
        Module {
            name,
            requires,
            optional: &[],
            warns: None,
            needs: &[],
            settings: &[],
            verbs: &[],
            start: |app| app,
        }
    }

    fn on(without: &[&'static str]) -> State {
        State::On {
            without: without.to_vec(),
        }
    }

    fn states(modules: &Modules) -> Vec<(&'static str, State)> {
        modules.states.clone()
    }

    #[test]
    fn everything_runs_when_nothing_is_off() {
        let all = [module("island", &[]), module("media", &["island"])];

        assert_eq!(
            states(&resolve(&all, |_| false)),
            [("island", on(&[])), ("media", on(&[]))]
        );
    }

    #[test]
    fn a_missing_requirement_turns_off_and_names_it() {
        let all = [
            module("island", &[]),
            module("notifications", &["island"]),
            module("banners", &["notifications"]),
            module("weather", &["network"]),
        ];

        let modules = resolve(&all, |name| name == "notifications");

        assert_eq!(
            states(&modules),
            [
                ("island", on(&[])),
                ("notifications", State::Off),
                ("banners", State::Missing("notifications")),
                ("weather", State::Missing("network")),
            ]
        );
        assert!(!modules.on("banners"));
        assert_eq!(
            modules.lines(),
            [
                "module island is on",
                "module notifications is off",
                "module banners is off: it requires notifications, which is not on",
                "module weather is off: it requires network, which is not on",
            ]
        );
        assert_eq!(
            State::Missing("notifications")
                .problem("banners")
                .as_deref(),
            Some("module banners is off: it requires notifications, which is not on")
        );
        assert_eq!(State::Off.problem("notifications"), None);
    }

    // order does not matter: a requirement listed later still settles first
    #[test]
    fn requirements_settle_in_any_order() {
        let all = [
            module("banners", &["notifications"]),
            module("notifications", &[]),
        ];

        assert_eq!(
            states(&resolve(&all, |name| name == "notifications")),
            [
                ("banners", State::Missing("notifications")),
                ("notifications", State::Off),
            ]
        );
    }

    #[test]
    fn a_missing_optional_module_degrades() {
        let all = [
            module("audio", &[]),
            module("brightness", &[]),
            Module {
                optional: &["audio", "brightness", "nothing"],
                ..module("osd", &[])
            },
        ];

        let modules = resolve(&all, |name| name == "brightness");

        assert_eq!(modules.state("osd"), Some(&on(&["brightness", "nothing"])));
        assert!(modules.on("osd"));
        assert_eq!(
            on(&["brightness", "nothing"]).problem("osd").as_deref(),
            Some("module osd runs without brightness, nothing")
        );
        assert_eq!(on(&[]).problem("osd"), None);
    }

    // an optional loop is no loop: neither needs the other to run
    #[test]
    fn cycles_are_rejected_and_their_dependents_turn_off() {
        let all = [
            module("a", &["b"]),
            module("b", &["c"]),
            module("c", &["a"]),
            module("self", &["self"]),
            module("d", &["a"]),
            Module {
                optional: &["f"],
                ..module("e", &[])
            },
            Module {
                optional: &["e"],
                ..module("f", &[])
            },
        ];

        assert_eq!(
            states(&resolve(&all, |_| false)),
            [
                ("a", State::Cycle(vec!["a", "b", "c"])),
                ("b", State::Cycle(vec!["b", "c", "a"])),
                ("c", State::Cycle(vec!["c", "a", "b"])),
                ("self", State::Cycle(vec!["self"])),
                ("d", State::Missing("a")),
                ("e", on(&[])),
                ("f", on(&[])),
            ]
        );
        assert_eq!(
            State::Cycle(vec!["a", "b", "c"]).problem("a").as_deref(),
            Some("module a is off: its requirements loop: a -> b -> c -> a")
        );
    }

    // a Module starts after what it requires, so the island is configured before any source reads it
    #[test]
    fn every_module_comes_after_what_it_requires() {
        for (at, module) in ALL.iter().enumerate() {
            for required in module.requires.iter().chain(module.optional) {
                let before = ALL[..at].iter().any(|other| other.name == *required);
                let named = ALL.iter().any(|other| other.name == *required);

                assert!(named, "{} names an unknown module {required}", module.name);
                assert!(
                    before || module.optional.contains(required),
                    "{} comes before {required}",
                    module.name
                );
            }
        }
    }

    #[test]
    fn every_surface_has_a_module() {
        for surface in Surface::ALL {
            let (_, module) = SURFACES
                .iter()
                .find(|(each, _)| *each == surface)
                .unwrap_or_else(|| panic!("{surface:?} has no module"));
            let module = ALL.iter().find(|each| each.name == *module).unwrap();

            assert!(module.requires.contains(&CORE), "{}", module.name);
        }
    }

    // the Modules of the issue's dependencies, as the registry declares them
    #[test]
    fn surfaces_turn_off_with_what_they_require() {
        let off = |turned: &'static str| resolve(ALL, move |name| name == turned);

        let modules = off("notifications");
        assert_eq!(
            modules.state("notification-surface"),
            Some(&State::Missing("notifications"))
        );
        assert_eq!(
            modules.state("controls"),
            Some(&State::On {
                without: vec!["notifications"]
            })
        );
        assert!(modules.on("launcher"));

        let modules = off("clipboard");
        assert_eq!(
            modules.state("clipboard-surface"),
            Some(&State::Missing("clipboard"))
        );

        for surface in [
            "controls",
            "launcher",
            "notification-surface",
            "clipboard-surface",
        ] {
            let modules = off(surface);

            assert_eq!(modules.state(surface), Some(&State::Off));
            assert!(
                ALL.iter()
                    .filter(|module| module.name != surface)
                    .all(|module| modules.on(module.name)),
                "{surface} turns off another"
            );
        }
    }

    #[test]
    fn names_are_unique_and_only_the_core_has_no_requirements() {
        for (at, module) in ALL.iter().enumerate() {
            assert!(
                ALL[..at].iter().all(|other| other.name != module.name),
                "{} twice",
                module.name
            );
            assert_eq!(module.requires.is_empty(), module.name == CORE);
        }

        assert_eq!(ALL[0].name, CORE);
    }
}
