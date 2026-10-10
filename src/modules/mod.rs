//! Modules (#94, docs/design.md Modules): every feature the user can turn off. A Module names the
//! Modules it requires and those it can do without; `resolve` decides once at start which run, and
//! `start` starts only those. A Module that is off starts no thread, opens no window, reads no
//! Service and refuses its verbs, so the Services only it reads stay cold. Turning one on or
//! off takes a restart, since the runtime registers windows only at start. `kanade module` (#149) lists
//! them and turns one on or off in the settings file, saying what the restart will change.

use std::sync::OnceLock;

use kanade_runtime::App;

use crate::cli::Verb;
use crate::island::presentation::Surface;
use crate::sources::osd;
use crate::{config, reload, settings};

mod catalog;

pub use self::catalog::ALL;

pub struct Module {
    pub name: &'static str,

    // what it does, a line for Settings
    pub about: &'static str,

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

    // a session bus service, running or started on demand, like the Secret Service
    SessionService(&'static str),

    // a PAM service file, which checks a password
    Pam(&'static str),

    // niri's IPC, which the Module cannot work without
    Niri,

    // wl-paste, new enough to say which copies are sensitive (`clipboard::SENSITIVE_SINCE`)
    Paste,

    // the keyboards' evdev devices, readable in the input group (`keys::found`)
    Keyboards,

    // /dev/rfkill, readable in the rfkill group (`radios::found`)
    Rfkill,
}

// the one Module that cannot be turned off: without it no window opens, and the runtime needs one
pub const CORE: &str = "island";

// each Surface the island opens, with the Module that draws it
const SURFACES: [(Surface, &str); 9] = [
    (Surface::Media, "media"),
    (Surface::Notifications, "notification-surface"),
    (Surface::Controls, "controls"),
    (Surface::Launcher, "launcher"),
    (Surface::Tray, "tray"),
    (Surface::Clipboard, "clipboard-surface"),
    (Surface::Calendar, "calendar-surface"),
    (Surface::Weather, "weather-surface"),
    (Surface::Session, "session"),
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
            State::Off => None,
            _ => Some(format!("module {name} {}", self.said())),
        }
    }

    // what became of it, after its name
    fn said(&self) -> String {
        match self {
            State::On { without } if without.is_empty() => String::from("is on"),
            State::On { without } => format!("runs without {}", without.join(", ")),
            State::Off => String::from("is off"),
            State::Missing(required) => {
                format!("is off: it requires {required}, which is not on")
            }
            State::Cycle(cycle) => format!(
                "is off: its requirements loop: {} -> {}",
                cycle.join(" -> "),
                cycle[0]
            ),
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
            .map(|(name, state)| format!("module {name} {}", state.said()))
            .collect()
    }

    /*
     * a line for each Module, with what a restart would make of it where that differs: `next`,
     * resolved against the config the files give now. Each with whether it differs
     */
    fn against(&self, next: &Modules) -> Vec<(&'static str, String, bool)> {
        self.states
            .iter()
            .map(|(name, state)| {
                let line = format!("module {name} {}", state.said());

                match next.state(name).filter(|after| *after != state) {
                    Some(after) => (
                        *name,
                        format!("{line}; after a restart it {}", after.said()),
                        true,
                    ),
                    None => (*name, line, false),
                }
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

// what the files would start, which a restart applies
fn next() -> Modules {
    let (read, _) = config::layers(true);

    resolve(ALL, |name| read.off(name))
}

// `module list`: a line for each Module, whether it runs, why it is not as asked, and what a restart changes
pub fn list() -> Vec<String> {
    let Some(modules) = MODULES.get() else {
        return Vec::new();
    };

    modules
        .against(&next())
        .into_iter()
        .map(|(_, line, _)| line)
        .collect()
}

/*
 * `module enable|disable`: turns a Module on or off in the settings file, which a restart applies;
 * says what becomes of it and of every other Module a restart changes, and what is lost with it off
 */
pub fn turn(name: &'static str, on: bool) -> Result<String, String> {
    settings::set(&["modules", name], Some(toml::Value::Boolean(on)))?;

    // at once, not on the watch's debounce, so `status` right after shows the change pending
    let refused = match reload::reload() {
        reload::Outcome::Valid(_) => None,
        reload::Outcome::Invalid(_) => Some(String::from(
            "the config did not reload, see `kanade status`; a restart still applies this",
        )),
    };

    let Some(modules) = MODULES.get() else {
        return Ok(String::new());
    };

    let mut lines: Vec<String> = modules
        .against(&next())
        .into_iter()
        .filter(|&(each, _, changes)| each == name || changes)
        .map(|(_, line, _)| line)
        .collect();

    if let Some(warning) = ALL
        .iter()
        .find(|module| module.name == name)
        .and_then(|module| module.warns)
        .filter(|_| !on)
    {
        lines.push(format!("with module {name} off, {warning}"));
    }

    lines.extend(refused);

    Ok(lines.join("\n"))
}

// the Module of this name that can be turned on or off: any but the core
pub fn named(name: &str) -> Option<&'static str> {
    ALL.iter()
        .map(|module| module.name)
        .find(|each| *each == name && *each != CORE)
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
            about: "",
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
    fn a_missing_requirement_turns_off_and_names_it() {
        let all = [
            module("island", &[]),
            module("notifications", &["island"]),
            module("banners", &["notifications"]),
            module("vpn", &["network"]),
        ];

        let modules = resolve(&all, |name| name == "notifications");

        assert_eq!(
            states(&modules),
            [
                ("island", on(&[])),
                ("notifications", State::Off),
                ("banners", State::Missing("notifications")),
                ("vpn", State::Missing("network")),
            ]
        );
        assert!(!modules.on("banners"));
        assert_eq!(
            modules.lines(),
            [
                "module island is on",
                "module notifications is off",
                "module banners is off: it requires notifications, which is not on",
                "module vpn is off: it requires network, which is not on",
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
