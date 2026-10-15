/*
 * Kanade's own verbs, as a Launcher provider (ADR 0112): a closed table of the `kanade` verbs a
 * person types the name of ("caffeine" finds Toggle Caffeine), each a static argv. Pressing one
 * runs it through the shell's own socket, as the CLI does, so the Module gate and the parse apply
 * (`Action::Run`). A row shows only while its Module is on, is found by name alone and never takes
 * typed text; this is not the gesture allow-list (ADR 0107)
 */

use std::time::Duration;

use crate::cli::{self, Call, Reply};
use crate::icon::Icon;
use crate::island::activity::Leave;
use crate::lock;
use crate::sources::live;

use super::destinations::WALLPAPER;
use super::provider::{self, Action, Answer, Fit, LauncherProvider, Mark};
use super::{emoji, history};

// the best a command fits: a name that is the query's is an app's or a destination's, which a
// generic alias like `record` must not outrank
const BEST: Fit = Fit::Prefix;

// the fewest letters a command takes, as a destination does: one would list rows for most words
const FROM: usize = 2;

// how long a verb of the table may take to answer; past `later::LATER`, so a slow source's word arrives
const PATIENCE: Duration = Duration::from_secs(6);

// what a row or an Activity says first when the verb was not run, and when it may have been
const NOT_RUN: &str = "Not run: ";
const NOT_CONFIRMED: &str = "Not confirmed, may have run: ";

// a failure's words are at most this long, as an Activity's detail is (`live::gesture`)
const WORDS: usize = 60;

// which verbs exist now: the Module that owns each is on and, for a radio, the machine has one
#[derive(Debug, Clone, Copy, Default)]
pub struct Commands {
    pub caffeine: bool,
    pub night_light: bool,
    pub focus: bool,
    pub notifications: bool,
    pub wifi: bool,
    pub bluetooth: bool,
    pub power: bool,
    pub capture: bool,
    pub lock: bool,
    pub wallpaper: bool,
    pub dictation: bool,
}

// the flag of `Commands` a row needs
#[derive(Debug, Clone, Copy)]
enum Gate {
    Caffeine,
    NightLight,
    Focus,
    Notifications,
    Wifi,
    Bluetooth,
    Power,
    Capture,
    Lock,
    Wallpaper,
    Dictation,
}

struct Command {
    gate: Gate,
    icon: Icon,
    title: &'static str,
    detail: &'static str,

    // lowercase names it is found by besides its title; none is another command's
    aliases: &'static [&'static str],

    // what pressing it runs, parsed by `cli::parse` as any verb is
    verb: &'static [&'static str],
}

const TABLE: &[Command] = &[
    Command {
        gate: Gate::Caffeine,
        icon: Icon::Cup,
        title: "Toggle Caffeine",
        detail: "Keep the session awake",
        aliases: &["caffeine", "awake", "keep awake", "coffee"],
        verb: &["caffeine", "toggle"],
    },
    Command {
        gate: Gate::NightLight,
        icon: Icon::Sunset,
        title: "Toggle Night Light",
        detail: "Warm the screen",
        aliases: &["night light", "night-light", "nightlight", "blue light"],
        verb: &["night-light", "toggle"],
    },
    Command {
        gate: Gate::Focus,
        icon: Icon::Focus,
        title: "Toggle Focus",
        detail: "Do Not Disturb, caffeine and night light together",
        aliases: &["focus", "focus mode"],
        verb: &["focus", "toggle"],
    },
    Command {
        gate: Gate::Notifications,
        icon: Icon::Moon,
        title: "Toggle Do Not Disturb",
        detail: "Quiet notifications",
        aliases: &["do not disturb", "dnd", "silence notifications"],
        verb: &["notifications", "dnd", "toggle"],
    },
    Command {
        gate: Gate::Wifi,
        icon: Icon::Wifi,
        title: "Toggle Wi-Fi",
        detail: "Turn the Wi-Fi radio on or off",
        aliases: &["wifi", "wi-fi", "wireless"],
        verb: &["wifi", "toggle"],
    },
    Command {
        gate: Gate::Bluetooth,
        icon: Icon::Bluetooth,
        title: "Toggle Bluetooth",
        detail: "Turn the Bluetooth radio on or off",
        aliases: &["bluetooth", "bt"],
        verb: &["bluetooth", "toggle"],
    },
    Command {
        gate: Gate::Power,
        icon: Icon::Bolt,
        title: "Switch Power Profile",
        detail: "The next profile the machine has",
        aliases: &[
            "power profile",
            "power-profile",
            "performance",
            "power saver",
        ],
        verb: &["power-profile", "cycle"],
    },
    Command {
        gate: Gate::Capture,
        icon: Icon::Capture,
        title: "Screenshot Area",
        detail: "Pick an area to save",
        aliases: &["screenshot area", "area screenshot", "region"],
        verb: &["capture", "screenshot", "area"],
    },
    Command {
        gate: Gate::Capture,
        icon: Icon::Capture,
        title: "Screenshot Window",
        detail: "Save a window",
        aliases: &["screenshot window", "window screenshot"],
        verb: &["capture", "screenshot", "window"],
    },
    Command {
        gate: Gate::Capture,
        icon: Icon::Capture,
        title: "Screenshot Screen",
        detail: "Save the whole screen",
        aliases: &["screenshot screen", "screenshot output", "fullscreen"],
        verb: &["capture", "screenshot", "output"],
    },
    Command {
        gate: Gate::Capture,
        icon: Icon::Camera,
        title: "Start Recording",
        detail: "Record the screen",
        aliases: &["record", "record screen", "screen recording"],
        verb: &["capture", "record", "start"],
    },
    Command {
        gate: Gate::Capture,
        icon: Icon::Camera,
        title: "Stop Recording",
        detail: "End the screen recording",
        aliases: &["stop recording", "stop record"],
        verb: &["capture", "record", "stop"],
    },
    Command {
        gate: Gate::Lock,
        icon: Icon::Lock,
        title: "Lock Screen",
        detail: "Lock the session",
        aliases: &["lock", "lock session"],
        verb: &["lock"],
    },
    Command {
        gate: Gate::Wallpaper,
        icon: Icon::Picture,
        title: "Shuffle Wallpaper",
        detail: "Set a random wallpaper",
        aliases: &["shuffle", "random wallpaper", "new wallpaper"],
        verb: &["wallpaper", "shuffle"],
    },
    /*
     * a row because the user types its name; the key and the consent it needs are still asked for
     * by `kanade dictate key|consent` in a terminal, so a press without them is refused (ADR 0112)
     */
    Command {
        gate: Gate::Dictation,
        icon: Icon::Microphone,
        title: "Toggle Dictation",
        detail: "Speech to text through the Gemini API",
        aliases: &["dictate", "dictation", "voice typing", "speech to text"],
        verb: &["dictate", "toggle"],
    },
];

impl Commands {
    fn offers(&self, gate: Gate) -> bool {
        match gate {
            Gate::Caffeine => self.caffeine,
            Gate::NightLight => self.night_light,
            Gate::Focus => self.focus,
            Gate::Notifications => self.notifications,
            Gate::Wifi => self.wifi,
            Gate::Bluetooth => self.bluetooth,
            Gate::Power => self.power,
            Gate::Capture => self.capture,
            Gate::Lock => self.lock,
            Gate::Wallpaper => self.wallpaper,
            Gate::Dictation => self.dictation,
        }
    }
}

impl LauncherProvider for Commands {
    // by name within a fit; nothing for a query of one letter or none, nor for `:`, `>` or `@`
    fn find(&self, query: &str) -> Vec<Answer> {
        if query.chars().count() < FROM
            || query.starts_with(emoji::PREFIX)
            || query.starts_with(history::PREFIX)
            || query.starts_with(WALLPAPER)
        {
            return Vec::new();
        }

        let query = query.to_lowercase();

        TABLE
            .iter()
            .filter(|command| self.offers(command.gate))
            .filter_map(|command| {
                let title = command.title.to_lowercase();
                let fit = command
                    .aliases
                    .iter()
                    .copied()
                    .chain([title.as_str()])
                    .filter_map(|name| provider::fit(&query, name))
                    .min()?;

                Some(Answer {
                    // never over an app or a destination of the name
                    fit: fit.max(BEST),
                    title: command.title.to_owned(),
                    detail: command.detail.to_owned(),
                    mark: Mark::Icon(command.icon),
                    action: Action::Run(command.verb),
                })
            })
            .collect()
    }
}

// the title of the row running `verb`, for an Activity that has to name it
pub fn title(verb: &[&str]) -> &'static str {
    TABLE
        .iter()
        .find(|command| command.verb == verb)
        .map_or("Command", |command| command.title)
}

/*
 * whether `verb` would catch the island in what it takes: a screenshot or a recording started. It
 * runs once the island is gone (`surfaces::capture::CLEAR`), when no row is left to say it failed
 */
pub fn clears_island(verb: &[&str]) -> bool {
    matches!(
        verb,
        ["capture", "screenshot", _] | ["capture", "record", "start"]
    )
}

// the first line of `why` after `prefix`, the whole kept to `WORDS` with the prefix intact
fn worded(prefix: &str, why: &str) -> String {
    let why = why.lines().next().unwrap_or_default();

    let room = WORDS.saturating_sub(prefix.chars().count());

    format!("{prefix}{}", why.chars().take(room).collect::<String>())
}

// the words for a verb that was not run
pub fn not_run(why: &str) -> String {
    worded(NOT_RUN, why)
}

/*
 * how a call went, in the words a row or an Activity shows: nothing for done; "Not run" for a
 * refusal or no shell, which did nothing; "Not confirmed" for an unclear answer or one that did not
 * come in time, as the verb may have run, so no one is invited to press it again blindly
 */
pub fn outcome(called: Result<Reply, String>) -> Result<(), String> {
    match called {
        Ok(Reply::Done(_)) => Ok(()),
        Ok(Reply::Refused(why)) => Err(not_run(&why)),
        Ok(Reply::Unknown(why)) => Err(worded(NOT_CONFIRMED, &why)),
        Err(why) if why == cli::NO_SHELL => Err(not_run(&why)),
        Err(why) => Err(worded(NOT_CONFIRMED, &why)),
    }
}

/*
 * runs the verb of a row through the shell's own socket and waits for its answer (call it off the
 * view thread). Not while the session is locked, as a gesture does not (ADR 0107)
 */
pub fn dispatch(verb: &'static [&'static str]) -> Result<(), String> {
    if denied(verb) {
        return Err(not_run("not a verb the Launcher runs"));
    }

    if lock::holds() {
        return Err(not_run("the session is locked"));
    }

    let words: Vec<String> = verb.iter().map(|word| (*word).to_owned()).collect();
    let result = outcome(cli::call_within(&words, PATIENCE));

    if let Err(why) = &result {
        eprintln!("launcher: {verb:?}: {why}");
    }

    result
}

/*
 * whether a verb is never run from the Launcher: it types or presses into windows (`ui`), runs a
 * program (`activity run|hook`), ends the session (`session logout|reboot|poweroff`), or waits on
 * the user (`dmenu`, `gesture`, `capture color`); or it is no verb. The table holds none, and
 * `press` checks again
 */
pub fn denied(words: &[&str]) -> bool {
    let Ok((_, call)) = cli::parse(words) else {
        return true;
    };

    matches!(
        call,
        Call::Ui(_)
            | Call::Live(live::Request::Run { .. } | live::Request::Hook(_))
            | Call::Session(Leave::Ending(_))
            | Call::Dmenu(_)
            | Call::Gesture(_)
            | Call::PickColor
    )
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::surfaces::launcher::provider::Fit;

    fn all() -> Commands {
        Commands {
            caffeine: true,
            night_light: true,
            focus: true,
            notifications: true,
            wifi: true,
            bluetooth: true,
            power: true,
            capture: true,
            lock: true,
            wallpaper: true,
            dictation: true,
        }
    }

    #[test]
    fn every_verb_parses_and_none_is_denied() {
        for command in TABLE {
            assert!(
                cli::parse(command.verb).is_ok(),
                "{:?} does not parse",
                command.verb
            );
            assert!(!denied(command.verb), "{:?} is denied", command.verb);
        }
    }

    // what the CLI has that a Launcher never runs
    #[test]
    fn the_deny_set_holds() {
        for words in [
            &["ui", "describe"][..],
            &["activity", "run", "b", "Build", "--", "make"],
            &["activity", "hook", "claude"],
            &["session", "logout"],
            &["session", "reboot"],
            &["session", "poweroff"],
            &["dmenu"],
            &["gesture"],
            &["capture", "color"],
            &["not-a-verb"],
        ] {
            assert!(denied(words), "{words:?}");
        }
    }

    #[test]
    fn no_alias_is_two_commands() {
        let mut seen = HashSet::new();

        for command in TABLE {
            for alias in command.aliases {
                assert_eq!(*alias, alias.to_lowercase(), "{alias}");
                assert!(seen.insert(*alias), "{alias} names two commands");
            }
        }
    }

    #[test]
    fn a_row_needs_its_module() {
        assert!(Commands::default().find("caffeine").is_empty());

        let only = Commands {
            caffeine: true,
            ..Commands::default()
        };

        assert_eq!(only.find("caffeine").len(), 1);
        assert!(only.find("wifi").is_empty());
        assert!(!all().find("caffeine").is_empty());
    }

    #[test]
    fn no_row_for_a_prefix_a_short_or_no_query() {
        for query in ["", "c", ":caffeine", ">caffeine", "@caffeine", "@"] {
            assert!(all().find(query).is_empty(), "{query:?}");
        }
    }

    // a destination of the same name comes first
    #[test]
    fn a_tie_ranks_after_the_destinations() {
        use crate::surfaces::launcher::destinations::Destinations;

        let destinations = Destinations {
            wifi: true,
            ..Destinations::default()
        };
        let wifi = Commands {
            wifi: true,
            ..Commands::default()
        };

        let ranked = provider::ranked(&[&destinations, &wifi], "wifi");

        assert_eq!(ranked.len(), 2);
        assert!(matches!(ranked[0].action, Action::Open(_)));
        assert!(matches!(ranked[1].action, Action::Run(_)));
        assert_eq!(ranked[0].fit, Fit::Exact);
        assert_eq!(ranked[1].fit, Fit::Prefix);
    }

    // an exact app or destination name leads, however generic a command's alias
    #[test]
    fn no_command_fits_better_than_a_prefix() {
        for command in TABLE {
            for name in command.aliases.iter().copied().chain([command.title]) {
                let found = all().find(name);
                let row = found
                    .iter()
                    .find(|answer| answer.title == command.title)
                    .expect(name);

                assert!(row.fit >= Fit::Prefix, "{name}");
            }
        }
    }

    // a verb that may have run never reads as not run, whatever the reason's length
    #[test]
    fn an_unclear_answer_does_not_say_not_run() {
        let long = "x".repeat(200);

        for called in [
            Ok(Reply::Unknown(long.clone())),
            Err(long.clone()),
            Err(String::from("did not answer within 6s")),
        ] {
            let said = outcome(called).unwrap_err();

            assert!(said.starts_with(NOT_CONFIRMED), "{said}");
            assert!(said.chars().count() <= WORDS);
        }

        for called in [
            Ok(Reply::Refused(format!("module caffeine is off\n{long}"))),
            Err(String::from(cli::NO_SHELL)),
        ] {
            let said = outcome(called).unwrap_err();

            assert!(said.starts_with(NOT_RUN), "{said}");
            assert!(!said.contains('\n'));
            assert!(said.chars().count() <= WORDS);
        }

        assert_eq!(outcome(Ok(Reply::Done(String::new()))), Ok(()));
    }
}
