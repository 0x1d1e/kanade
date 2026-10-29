//! Config hot reload (docs/archived/design.md Hot reload): a change to a layer's file, or
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
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use inotify::{EventMask, Inotify, WatchDescriptor, WatchMask};
use kanade_runtime::{MutexExt, service::Service};

use crate::atomic::{self, Mode};
use crate::config::{self, Config, Place};
use crate::hooks::{self, Event};
use crate::island::service::IslandService;
use crate::{appearance, modules, settings, sources, supervise, view};

// an editor's save is several events: a write, a rename over the old file, a backup removed
const DEBOUNCE: Duration = Duration::from_millis(150);

// the paths of the config that last applied, in the state directory beside the other state: one
// line each, and never the config itself, which may hold a token
const LAST_GOOD: &str = "last-good-config";

// whether a write of it was already reported
static SAID_LAST_GOOD: AtomicBool = AtomicBool::new(false);

// what the reloads came to, for `status`
struct State {
    // the configs that applied, the one read at start the first
    generation: u64,

    // the last reload's problems, none once one applies
    error: Option<String>,

    // the keys whose new value waits for a restart
    pending: Vec<String>,

    // the first problem of the refusal last shown on the island, so the same refused read is not
    // shown at each reload of an unchanged file
    noticed: Option<String>,

    // the paths whose contents the config in effect applied, which is what `status` says: only a
    // file that contributed is among them, and the file beside the other state mirrors them
    // (`keep_last_good`)
    last_good: Vec<PathBuf>,
}

// held for a whole reload, so the watch and an IPC reload never interleave
static STATE: Mutex<State> = Mutex::new(State {
    generation: 1,
    error: None,
    pending: Vec::new(),
    noticed: None,
    last_good: Vec::new(),
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
    // the config read at start is the last good one until a reload applies another
    let applied = config::applied();
    STATE.locked().last_good = applied;

    // at start, handed to the worker as every other write of it is
    keep_last_good();

    supervise::spawn("config", || {
        let places = config::places();

        if let Err(error) = watch(&places, || drop(reload())) {
            let why = format!("config changes are not followed, reload by hand: {error}");

            eprintln!("kanade: {why}");
            supervise::stopped("config", why);
        }
    });
}

/*
 * whether the Modules read again what lives outside the config: one turned on or off, or the weather
 * moved to another place, which its thread learns only from this (ADR 0017)
 */
fn rereads(running: &Config, next: &Config) -> bool {
    next.off != running.off || next.location != running.location
}

/*
 * reads every layer again and applies them if they hold no problem; says on stderr what came of it.
 * The paths whose contents applied are kept for `status`, and handed to the worker to write, so no
 * file is written on the thread that draws a frame
 */
pub fn reload() -> Outcome {
    let mut state = STATE.locked();
    let (read, problems, applied) = config::read_applied();
    let running = config::get();

    let outcome = judge(&running, &read, problems);
    let mut kept = false;

    match &outcome {
        Outcome::Valid(pending) => {
            let next = config::restarted(&running, read);

            if next != *running {
                // swapped first, so the windows the island's write wakes draw the new config
                config::install(next.clone());

                if next.appearance.tone != running.appearance.tone {
                    appearance::tone_changed(next.appearance.tone);
                    hooks::fire_with(Event::Tone, next.appearance.tone.name());
                }

                // the runtime draws every window again at it, and republishes the monitors the
                // views lay out in, which resizes the Island and the Dock
                if next.appearance.scale != running.appearance.scale {
                    kanade_runtime::set_ui_scale(next.appearance.ui_scale());
                }

                if next.appearance.sync != running.appearance.sync
                    || next.appearance.sync && next.appearance.tone != running.appearance.tone
                {
                    appearance::sync(&next);
                }

                // only the timings the island draws by changed, so only then retime and resize
                if next.animation_timings() != running.animation_timings() {
                    {
                        // read under the write, so an output changing meanwhile cannot land first
                        let mut island = IslandService::write();
                        island.retime(next.animation_timings());
                        view::resize(&mut island);
                    }
                }

                // last, so every other window that reads the config draws all of it at once
                config::announce();
            }

            // only when a module was turned on or off; a reload that changed nothing else is a no-op
            if rereads(&running, &next) {
                modules::reread();
            }

            state.generation += 1;
            state.error = None;
            state.noticed = None;
            state.pending.clone_from(pending);

            // the files this read applied, of which the file beside the other state is told off
            // this thread
            state.last_good = applied;
            kept = true;

            eprintln!("kanade: config reloaded, generation {}", state.generation);
            for key in pending {
                eprintln!("kanade: {key} is pending restart");
            }
        }
        Outcome::Invalid(problems) => {
            state.error = Some(problems.join("\n"));

            // one Transient on the focused output, through the Service, naming the first problem
            // only; the rest is in `kanade status`. The same refused read is not shown again
            let first = problems.first().cloned().unwrap_or_default();
            let show = newly_refused(&mut state, &first);

            eprintln!(
                "kanade: config not reloaded, generation {} stays:",
                state.generation
            );
            for problem in problems {
                eprintln!("kanade: {problem}");
            }

            if show {
                sources::live::config_refused(&first);
            }
        }
    }

    // after the lock, as the window's snapshot reads the last error
    drop(state);
    if modules::on("settings") {
        settings::refresh();
    }

    if kept {
        keep_last_good();
    }

    outcome
}

// the last reload's problems, none once one applies
pub fn error() -> Option<String> {
    STATE.locked().error.clone()
}

// whether a refusal is one to show: the first problem of the last shown refusal, so a file read
// again unchanged says nothing new, while a different one does
fn newly_refused(state: &mut State, first: &str) -> bool {
    if state.noticed.as_deref() == Some(first) {
        return false;
    }

    state.noticed = Some(first.to_owned());
    true
}

// where the last good config's paths are kept, beside the other state
fn last_good_file() -> Option<PathBuf> {
    config::base(
        "XDG_STATE_HOME",
        ".local/state",
        std::env::var("HOME").ok().as_deref(),
    )
    .map(|dir| dir.join("kanade").join(LAST_GOOD))
}

/*
 * the paths of the config that last applied, written to the state directory by the worker, never on
 * the thread that draws a frame: `status` reads the record from memory meanwhile, and a failure to
 * write is no reason to fail a reload, said once (`keep`)
 */
fn keep_last_good() {
    kanade_runtime::worker::run(|| {
        let files = STATE.locked().last_good.clone();

        keep(&files);
    });
}

/*
 * the paths written only when they changed: one a line, the file 0600 and replaced whole, so a crash
 * leaves the old list or the new one. A failure is no reason to fail a reload, and is said once
 */
fn keep(files: &[PathBuf]) {
    let Some(path) = last_good_file() else {
        return;
    };

    if let Err(error) = keep_at(&path, files)
        && !SAID_LAST_GOOD.swap(true, Ordering::Relaxed)
    {
        eprintln!(
            "kanade: cannot keep the last good config at {}: {error}",
            path.display()
        );
    }
}

// `keep`, at a file named
fn keep_at(path: &Path, files: &[PathBuf]) -> io::Result<()> {
    let mut text = String::new();
    for file in files {
        text.push_str(&file.to_string_lossy());
        text.push('\n');
    }

    if fs::read_to_string(path).is_ok_and(|kept| kept == text) {
        return Ok(());
    }

    atomic::write(path, text.as_bytes(), Mode::Private)
}

// what a reload would come to, changing nothing
pub fn validate() -> Outcome {
    let (read, problems) = config::read();

    judge(&config::get(), &read, problems)
}

// the layout and generation, the paths of the last good config, what the config in effect skipped
// at start, the last reload error and the keys pending restart
pub fn status() -> Vec<String> {
    let state = STATE.locked();

    lines(&state, &config::skipped())
}

fn lines(state: &State, skipped: &[String]) -> Vec<String> {
    let mut lines = vec![format!(
        "config schema_version {}, generation {}",
        config::SCHEMA_VERSION,
        state.generation
    )];

    // the file or files that last applied, which a reload that refuses its files keeps running
    lines.extend(
        state
            .last_good
            .iter()
            .map(|path| format!("config: last good {}", path.display())),
    );

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

    Outcome::Valid(config::pending(running, read))
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
    // the weather thread learns of a new place only from this: it read the old one back otherwise
    #[test]
    fn a_new_weather_location_makes_the_modules_read_again() {
        let running = Config::default();
        let mut next = running.clone();
        assert!(!rereads(&running, &next));

        next.location = Some(config::Location::new(11.56, 104.92).unwrap());
        assert!(rereads(&running, &next));
    }

    use std::fs;
    use std::os::unix::fs::PermissionsExt;
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
            noticed: None,
            last_good: vec![PathBuf::from("/home/a/.config/kanade/config.toml")],
        };

        assert_eq!(
            lines(&state, &[]),
            [
                format!(
                    "config schema_version {}, generation 1",
                    config::SCHEMA_VERSION
                ),
                String::from("config: last good /home/a/.config/kanade/config.toml"),
            ]
        );
        assert_eq!(
            lines(&state, &skipped)[2..],
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
            lines(&state, &skipped)[2..],
            [
                "config last reload error:",
                "  a.toml:1: x",
                "  b.toml:2: y",
                "config modules.media is pending restart",
            ]
        );
    }

    // an unchanged file read again refuses nothing new; another problem is shown, and one read
    // again after a reload applied is shown again
    #[test]
    fn a_refusal_is_shown_once() {
        let mut state = State {
            generation: 1,
            error: None,
            pending: Vec::new(),
            noticed: None,
            last_good: Vec::new(),
        };

        assert!(newly_refused(&mut state, "a.toml:3: x"));
        assert!(!newly_refused(&mut state, "a.toml:3: x"));
        assert!(newly_refused(&mut state, "a.toml:3: y"));

        state.noticed = None;
        assert!(newly_refused(&mut state, "a.toml:3: y"));
    }

    #[test]
    fn the_last_good_paths_are_private_and_one_a_line() {
        let dir = env_dir("last-good");
        let path = dir.join("last-good-config");
        let files = [
            PathBuf::from("/home/a/.config/kanade/config.toml"),
            PathBuf::from("/home/a/.local/state/kanade/settings.toml"),
        ];
        let kept = |path: &Path| -> Vec<String> {
            fs::read_to_string(path)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        };

        keep_at(&path, &files).unwrap();

        assert_eq!(
            kept(&path),
            [
                "/home/a/.config/kanade/config.toml",
                "/home/a/.local/state/kanade/settings.toml",
            ]
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        // what applies with no file of its own is nothing to keep
        keep_at(&path, &[]).unwrap();
        assert!(kept(&path).is_empty());

        fs::remove_dir_all(&dir).unwrap();
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
