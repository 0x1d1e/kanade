//! Modules (#94, docs/design.md Modules): every feature the user can turn off. A Module names the
//! Modules it requires and those it can do without; `resolve` decides once at start which run, and
//! `start` starts only those. A Module that is off starts no thread, opens no window, reads no
//! Service and takes no IPC verb, so the Amane Services only it reads stay cold. Turning one on or
//! off takes a restart, since Amane registers windows only at start.

use std::sync::OnceLock;
use std::thread;

use amane::{App, Apps, Service};

use crate::sources::{battery, media, niri, notifications, osd, privacy, system, timer};
use crate::{clock, config, ipc, island, shadow, theme, view};

pub struct Module {
    pub name: &'static str,

    // hard: one that is not on turns this one off, saying which
    pub requires: &'static [&'static str],

    // soft: one that is not on leaves this one running without what it gives
    pub optional: &'static [&'static str],

    // what the user loses with it off, said at start whenever it is, since that loss is easy to miss
    pub warns: Option<&'static str>,

    /*
     * starts its threads and adds its windows and IPC verbs to the shell; runs once at start, after
     * every Module it requires, so a source finds the island configured
     */
    pub start: fn(App) -> App,
}

// the one Module that cannot be turned off: without it no window opens, and Amane needs one
pub const CORE: &str = "island";

// every Module, each after the Modules it requires
pub const ALL: &[Module] = &[
    Module {
        name: CORE,
        requires: &[],
        optional: &[],
        warns: None,
        start: |app| {
            let config = config::get();

            // before anything reads the island, which takes its timings once
            island::service::configure(config.island);
            theme::follow(config.palette.as_deref());
            clock::spawn();
            shadow::prepare(shadow::ShadowStyle::island());

            // Kanade's own niri stream, which `workspace` and `privacy` also read when on
            thread::spawn(|| {
                niri::follow(niri::Posts {
                    workspace: on("workspace"),
                    cast: on("privacy"),
                });
            });

            // the first read starts Amane's app scan, which takes seconds, so the Launcher opens on a list
            thread::spawn(|| drop(Apps::read()));

            app.window_per_monitor(view::island)
                .ipc("island", ipc::island)
        },
    },
    // read from the island's niri stream, so it starts nothing of its own
    Module {
        name: "workspace",
        requires: &[CORE],
        optional: &[],
        warns: None,
        start: |app| app,
    },
    // the microphone and camera from PipeWire, and screen casts from the island's niri stream
    Module {
        name: "privacy",
        requires: &[CORE],
        optional: &[],
        warns: Some("microphone, camera and screen cast indicators will not show"),
        start: |app| {
            spawn("privacy", privacy::follow);
            app
        },
    },
    Module {
        name: "battery",
        requires: &[CORE],
        optional: &[],
        warns: None,
        start: |app| {
            spawn("battery", battery::follow);
            app
        },
    },
    Module {
        name: "media",
        requires: &[CORE],
        optional: &[],
        warns: None,
        start: |app| {
            spawn("media", media::follow);
            app
        },
    },
    Module {
        name: "timer",
        requires: &[CORE],
        optional: &[],
        warns: None,
        start: |app| {
            timer::spawn();
            app
        },
    },
    Module {
        name: "osd",
        requires: &[CORE],
        optional: &[],
        warns: None,
        start: |app| {
            spawn("osd", osd::follow);
            app
        },
    },
    Module {
        name: "notifications",
        requires: &[CORE],
        optional: &[],
        warns: None,
        start: |app| {
            spawn("notifications", notifications::follow);
            app
        },
    },
    // the three share one system bus watcher, which follows only the daemons of those that are on
    Module {
        name: "network",
        requires: &[CORE],
        optional: &[],
        warns: None,
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
        start: |app| {
            system::spawn();
            app
        },
    },
    // power profiles, for Controls
    Module {
        name: "power",
        requires: &[CORE],
        optional: &[],
        warns: None,
        start: |app| {
            system::spawn();
            app
        },
    },
];

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

// a Module's own thread, named after it, so `/proc/<pid>/task/*/comm` says which Modules run
fn spawn(name: &str, run: fn()) {
    let spawned = thread::Builder::new().name(name.to_owned()).spawn(run);

    if let Err(error) = spawned {
        eprintln!("kanade: module {name} cannot start a thread: {error}");
    }
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

// whether a Module runs; none do before `start`
pub fn on(name: &str) -> bool {
    MODULES.get().is_some_and(|modules| modules.on(name))
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
