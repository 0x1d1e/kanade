//! Config hot reload (#102, docs/design.md Hot reload): a change to a layer's file, or
//! `kanade config reload`, reads every layer again. A config without a problem applies
//! whole, at once; one with any problem changes nothing, so a half-typed edit never reverts a key
//! to its default, and the problems go to stderr and the last reload error. A key that only
//! applies at start, which `[modules]` is, keeps its running value and is pending restart.
//!
//! The watch is inotify on the config directory and the settings file's directory, or the nearest
//! of their parents while they do not exist yet, and on each layer's file, so an edit through a
//! symlink is seen too. The thread blocks on it, so watching costs no idle wakeups.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use amane::Service;
use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};

use crate::config::{self, Config, Place};
use crate::island::service::IslandService;
use crate::sources::windows;
use crate::{dock, modules, supervise, theme};

// an editor's save is several events: a write, a rename over the old file, a backup removed
const DEBOUNCE: Duration = Duration::from_millis(150);

// what the reloads came to, for `status`
struct State {
    // the configs that applied, the one read at start the first
    generation: u64,

    // the last reload's problems, none once one applies
    error: Option<String>,

    // the keys whose new value waits for a restart
    pending: Vec<String>,
}

// held for a whole reload, so the watch and an IPC reload never interleave
static STATE: Mutex<State> = Mutex::new(State {
    generation: 1,
    error: None,
    pending: Vec::new(),
});

// what a reload or validate found
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    // applies, with the keys pending restart
    Valid(Vec<String>),

    // changes nothing, for these problems
    Invalid(Vec<String>),
}

// the watch's thread; one that cannot watch says so, and `config reload` still reloads
pub fn spawn() {
    supervise::spawn("config", || {
        let places = config::places();

        if let Err(error) = watch(&places, || drop(reload())) {
            let why = format!("config changes are not followed, reload by hand: {error}");

            eprintln!("kanade: {why}");
            supervise::stopped("config", why);
        }
    });
}

// reads every layer again and applies them if they hold no problem; says on stderr what came of it
pub fn reload() -> Outcome {
    let mut state = STATE.lock().unwrap_or_else(PoisonError::into_inner);
    let (read, problems) = config::read();
    let running = config::get();

    let outcome = judge(&running, &read, problems);

    match &outcome {
        Outcome::Valid(pending) => {
            let next = Config {
                off: running.off.clone(),
                ..read
            };

            if next != *running {
                // swapped first, so the windows the island's write wakes draw the new config
                config::install(next.clone());

                if next.palette != running.palette {
                    theme::follow(next.palette.as_deref());
                }

                IslandService::write().retime(next.island);
            }

            // even unchanged, so a reload also finds the `.desktop` files installed since
            if modules::on("windows") {
                windows::rematch();
            }
            if modules::on("dock") {
                dock::pin();
            }

            state.generation += 1;
            state.error = None;
            state.pending.clone_from(pending);

            eprintln!("kanade: config reloaded, generation {}", state.generation);
            for key in pending {
                eprintln!("kanade: {key} is pending restart");
            }
        }
        Outcome::Invalid(problems) => {
            state.error = Some(problems.join("\n"));

            eprintln!(
                "kanade: config not reloaded, generation {} stays:",
                state.generation
            );
            for problem in problems {
                eprintln!("kanade: {problem}");
            }
        }
    }

    outcome
}

// what a reload would come to, changing nothing
pub fn validate() -> Outcome {
    let (read, problems) = config::read();

    judge(&config::get(), &read, problems)
}

// the layout and generation, what the config in effect skipped at start, the last reload error
// and the keys pending restart
pub fn status() -> Vec<String> {
    let state = STATE.lock().unwrap_or_else(PoisonError::into_inner);

    lines(&state, &config::skipped())
}

fn lines(state: &State, skipped: &[String]) -> Vec<String> {
    let mut lines = vec![format!(
        "config schema_version {}, generation {}",
        config::SCHEMA_VERSION,
        state.generation
    )];

    // every reload that applied read a config with nothing to skip
    if state.generation == 1 && !skipped.is_empty() {
        lines.push(String::from("config skipped at start:"));
        lines.extend(skipped.iter().map(|problem| format!("  {problem}")));
    }

    if let Some(error) = &state.error {
        lines.push(String::from("config last reload error:"));
        lines.extend(error.lines().map(|line| format!("  {line}")));
    }

    lines.extend(
        state
            .pending
            .iter()
            .map(|key| format!("config {key} is pending restart")),
    );
    lines
}

// any problem refuses the config; else the keys that differ from the running one but need a restart
fn judge(running: &Config, read: &Config, problems: Vec<String>) -> Outcome {
    if !problems.is_empty() {
        return Outcome::Invalid(problems);
    }

    let pending = modules::ALL
        .iter()
        .filter(|module| running.off(module.name) != read.off(module.name))
        .map(|module| format!("modules.{}", module.name))
        .collect();

    Outcome::Valid(pending)
}

// what makes an event under one watch a change to the config
#[derive(Debug, Clone, PartialEq)]
enum Want {
    // a layer appearing, changing or going in the config directory
    Layers,

    // this entry: the settings file, or the next directory on the way to a missing one
    Named(OsString),

    // anything to the watched file itself
    Any,
}

const DIRECTORY: WatchMask = WatchMask::CREATE
    .union(WatchMask::CLOSE_WRITE)
    .union(WatchMask::MOVED_TO)
    .union(WatchMask::MOVED_FROM)
    .union(WatchMask::DELETE)
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF)
    .union(WatchMask::ONLYDIR);

const FILE: WatchMask = WatchMask::CLOSE_WRITE
    .union(WatchMask::DELETE_SELF)
    .union(WatchMask::MOVE_SELF);

/*
 * blocks until a change to a place's layers, waits for the events to settle, watches again, since
 * a save by rename or a directory made ends a watch, then calls `changed`; returns only on an error
 */
fn watch(places: &[Place], mut changed: impl FnMut()) -> io::Result<()> {
    let mut inotify = Inotify::init()?;
    let mut buffer = [0; 4096];
    let mut watches = arm(&mut inotify, places, HashMap::new());

    loop {
        loop {
            let events = inotify.read_events_blocking(&mut buffer)?;

            if events.into_iter().any(|event| relevant(&watches, &event)) {
                break;
            }
        }

        // until a quiet DEBOUNCE, whatever came in it
        let mut quiet = Instant::now() + DEBOUNCE;

        while let Some(wait) = quiet.checked_duration_since(Instant::now()) {
            thread::sleep(wait);

            match inotify.read_events(&mut buffer) {
                Ok(mut events) => {
                    if events.any(|event| relevant(&watches, &event)) {
                        quiet = Instant::now() + DEBOUNCE;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(error) => return Err(error),
            }
        }

        watches = arm(&mut inotify, places, watches);
        changed();
    }
}

// the watches for every place as it is now, the old ones removed
fn arm(
    inotify: &mut Inotify,
    places: &[Place],
    old: HashMap<WatchDescriptor, Vec<Want>>,
) -> HashMap<WatchDescriptor, Vec<Want>> {
    for (watch, _) in old {
        // fails for a watch the kernel already ended
        let _ = inotify.watches().remove(watch);
    }

    let mut watches: HashMap<WatchDescriptor, Vec<Want>> = HashMap::new();

    for (path, mask, want) in targets(places) {
        match inotify.watches().add(&path, mask) {
            Ok(watch) => watches.entry(watch).or_default().push(want),
            Err(error) => eprintln!("kanade: cannot watch {}: {error}", path.display()),
        }
    }

    watches
}

// what to watch for each place: its directory or the nearest that exists, and each layer's file
fn targets(places: &[Place]) -> Vec<(PathBuf, WatchMask, Want)> {
    let mut targets = Vec::new();

    for place in places {
        let (dir, want) = match place {
            Place::Directory(dir) => (dir.as_path(), Want::Layers),
            Place::File(file) => match (file.parent(), file.file_name()) {
                (Some(dir), Some(name)) => (dir, Want::Named(name.to_owned())),
                _ => continue,
            },
        };

        if let Some((dir, want)) = nearest(dir, want) {
            targets.push((dir.to_owned(), DIRECTORY, want));
        }

        let files: Vec<PathBuf> = match place {
            Place::Directory(dir) => layers(dir),
            Place::File(file) => vec![file.clone()],
        };

        targets.extend(
            files
                .into_iter()
                .filter(|file| file.is_file())
                .map(|file| (file, FILE, Want::Any)),
        );
    }

    targets
}

// a directory, or while it is missing the nearest parent that is not, waiting for the next step
fn nearest(dir: &Path, want: Want) -> Option<(&Path, Want)> {
    let mut dir = dir;
    let mut want = want;

    while !dir.is_dir() {
        want = Want::Named(dir.file_name()?.to_owned());
        dir = dir.parent()?;
    }

    Some((dir, want))
}

fn layers(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = dir.read_dir() else {
        return Vec::new();
    };

    entries
        .filter_map(Result::ok)
        .filter(|entry| config::layer_name(&entry.file_name()))
        .map(|entry| entry.path())
        .collect()
}

/*
 * an event the watch it came from wants; one from a watch already replaced wants nothing. An
 * overflow, which comes from no watch, may have dropped an edit, so every layer is read again
 */
fn relevant(watches: &HashMap<WatchDescriptor, Vec<Want>>, event: &inotify::Event<&OsStr>) -> bool {
    if event.mask.contains(EventMask::Q_OVERFLOW) {
        eprintln!("kanade: config watch missed events, reading the config again");
        return true;
    }

    let Some(wants) = watches.get(&event.wd) else {
        return false;
    };

    let ended = EventMask::DELETE_SELF | EventMask::MOVE_SELF | EventMask::IGNORED;

    event.mask.intersects(ended)
        || wants.iter().any(|want| match (want, event.name) {
            (Want::Any, _) => true,
            (Want::Layers, Some(name)) => config::layer_name(name),
            (Want::Named(wanted), Some(name)) => name == wanted,
            (_, None) => false,
        })
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::mpsc;

    use super::*;

    #[test]
    fn a_problem_refuses_the_whole_config() {
        let running = Config::default();
        let problem = String::from("a.toml:3: timings.hover: 0 is outside 1-60000 ms");

        assert_eq!(
            judge(&running, &running, vec![problem.clone()]),
            Outcome::Invalid(vec![problem])
        );
        assert_eq!(judge(&running, &running, vec![]), Outcome::Valid(vec![]));
    }

    // a Module turned on or off waits for a restart; turned back, nothing waits
    #[test]
    fn modules_are_pending_restart() {
        let running = Config {
            off: vec!["media"],
            ..Config::default()
        };
        let read = Config {
            off: vec!["battery"],
            ..Config::default()
        };

        assert_eq!(
            judge(&running, &read, vec![]),
            Outcome::Valid(vec![
                String::from("modules.battery"),
                String::from("modules.media"),
            ])
        );
        assert_eq!(
            judge(&running, &running.clone(), vec![]),
            Outcome::Valid(vec![])
        );
    }

    #[test]
    fn status_says_what_the_config_skipped_failed_and_waits_for() {
        let skipped = [String::from("a.toml:1: clok: unknown key, skipped")];
        let mut state = State {
            generation: 1,
            error: None,
            pending: Vec::new(),
        };

        assert_eq!(
            lines(&state, &[]),
            [format!(
                "config schema_version {}, generation 1",
                config::SCHEMA_VERSION
            )]
        );
        assert_eq!(
            lines(&state, &skipped)[1..],
            [
                "config skipped at start:",
                "  a.toml:1: clok: unknown key, skipped",
            ]
        );

        // once a reload applied, the config in effect skipped nothing
        state.generation = 2;
        state.error = Some(String::from("a.toml:1: x\nb.toml:2: y"));
        state.pending = vec![String::from("modules.media")];

        assert_eq!(
            lines(&state, &skipped)[1..],
            [
                "config last reload error:",
                "  a.toml:1: x",
                "  b.toml:2: y",
                "config modules.media is pending restart",
            ]
        );
    }

    #[test]
    fn a_missing_directory_waits_on_its_nearest_parent() {
        let root = env_dir("nearest");
        fs::create_dir_all(root.join("a")).unwrap();

        assert_eq!(
            nearest(&root.join("a/b/c"), Want::Layers),
            Some((root.join("a").as_path(), Want::Named(OsString::from("b"))))
        );
        assert_eq!(
            nearest(&root.join("a"), Want::Layers),
            Some((root.join("a").as_path(), Want::Layers))
        );

        fs::remove_dir_all(&root).unwrap();
    }

    /*
     * against the kernel: the config directory made after the watch began, a layer written in it,
     * then a save by rename and a symlinked layer's target changing; other files change nothing
     */
    #[test]
    fn the_watch_sees_layers_change() {
        let root = env_dir("watch");
        let config = root.join("config/kanade");
        let state = root.join("state/kanade");
        let elsewhere = root.join("dotfiles");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("linked.toml"), "").unwrap();

        let places = vec![
            Place::Directory(config.clone()),
            Place::File(state.join("settings.toml")),
        ];
        let (sender, changes) = mpsc::channel();
        thread::spawn(move || watch(&places, || sender.send(()).unwrap()));

        let changed = || changes.recv_timeout(Duration::from_secs(2)).is_ok();
        let unchanged = || changes.recv_timeout(DEBOUNCE * 4).is_err();

        // the watch takes a moment to arm
        thread::sleep(Duration::from_millis(100));

        fs::create_dir_all(&config).unwrap();
        assert!(changed(), "the directory made");

        fs::write(config.join("a.toml"), "clock = \"12h\"").unwrap();
        assert!(changed(), "a layer written");

        fs::write(config.join(".a.toml.swp"), "").unwrap();
        fs::write(config.join("notes.txt"), "").unwrap();
        assert!(unchanged(), "not layers");

        fs::write(config.join("b.toml.tmp"), "").unwrap();
        fs::rename(config.join("b.toml.tmp"), config.join("b.toml")).unwrap();
        assert!(changed(), "a save by rename");

        std::os::unix::fs::symlink(elsewhere.join("linked.toml"), config.join("linked.toml"))
            .unwrap();
        assert!(changed(), "a symlink made");

        fs::write(elsewhere.join("linked.toml"), "clock = \"24h\"").unwrap();
        assert!(changed(), "a symlinked layer's target written");

        fs::create_dir_all(&state).unwrap();
        assert!(changed(), "the state directory made");

        fs::write(state.join("other.toml"), "").unwrap();
        assert!(unchanged(), "only settings.toml is a layer there");

        fs::write(state.join("settings.toml"), "").unwrap();
        assert!(changed(), "the settings file written");

        fs::remove_dir_all(&root).unwrap();
    }

    /*
     * against the kernel: past its queue limit inotify drops events and says so once, with no
     * watch, so an edit may be among those lost and only a reread of every layer is sure
     */
    #[test]
    fn a_queue_overflow_rereads_every_layer() {
        let root = env_dir("overflow");
        let limit: usize = fs::read_to_string("/proc/sys/fs/inotify/max_queued_events")
            .unwrap()
            .trim()
            .parse()
            .unwrap();

        let mut inotify = Inotify::init().unwrap();
        let watches = arm(
            &mut inotify,
            &[Place::Directory(root.clone())],
            HashMap::new(),
        );

        // three events a file, none read: a create, a close after writing and a delete
        for at in 0..limit / 3 + 100 {
            let file = root.join(format!("{at}.txt"));
            fs::write(&file, "").unwrap();
            fs::remove_file(&file).unwrap();
        }

        let mut buffer = [0; 4096];
        let mut overflowed = false;

        while let Ok(events) = inotify.read_events(&mut buffer) {
            for event in events {
                if event.mask.contains(EventMask::Q_OVERFLOW) {
                    overflowed = true;
                    assert!(relevant(&watches, &event));
                }
            }
        }

        fs::remove_dir_all(&root).unwrap();

        assert!(overflowed, "the queue overflowed");
    }

    fn env_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kanade-reload-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        dir
    }
}
