//! `kanade doctor` (docs/archived/design.md CLI): read-only diagnostics of what Kanade runs on, a line
//! each: versions, the shell, niri, the Wayland protocols, the buses and PipeWire, the config, the
//! Modules it turns on and what each needs outside Kanade. Runs without a shell, which may be what
//! is wrong, so it reads everything itself. A failure means Kanade, or a Module that is on, cannot work; a
//! warning, that it runs worse. There is no fix mode yet: Kanade owns no state a fix could touch.
//! Settings' Health page shows the same checks from inside the shell (`in_shell`), read-only too.

mod wayland;

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::bus::{Argument, Bus};

use crate::cli::{self, Reply};
use crate::config::{self, Config, Preference};
use crate::lock;
use crate::look::{Finish, Visualizer};
use crate::modules::{self, Module, Provider};
use crate::sddm;
use crate::sources::json::Json;
use crate::sources::{
    binds, bus, cava, clipboard, keys, niri, pipewire, radios, recording, sound, wake, wallpaper,
};

// the oldest niri Kanade follows (docs/archived/design.md Constraints)
const NIRI: (u32, u32) = (26, 4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    pub verdict: Verdict,
    pub text: String,

    // lines under it, like each config problem
    pub detail: Vec<String>,
}

impl Check {
    fn ok(text: String) -> Check {
        Check::new(Verdict::Ok, text)
    }

    fn warn(text: String) -> Check {
        Check::new(Verdict::Warn, text)
    }

    fn fail(text: String) -> Check {
        Check::new(Verdict::Fail, text)
    }

    fn new(verdict: Verdict, text: String) -> Check {
        Check {
            verdict,
            text,
            detail: Vec::new(),
        }
    }

    fn render(&self) -> String {
        let verdict = match self.verdict {
            Verdict::Ok => "ok",
            Verdict::Warn => "warn",
            Verdict::Fail => "fail",
        };

        std::iter::once(format!("{verdict:<4} {}", self.text))
            .chain(self.detail.iter().map(|line| format!("       {line}")))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// fails when any check does
pub fn run() -> ExitCode {
    let (shell, pid) = shell();
    let checks = gather(shell, pid);

    let report: String = checks
        .iter()
        .map(|check| format!("{}\n", check.render()))
        .collect();

    cli::print(
        &report,
        match checks.iter().any(|check| check.verdict == Verdict::Fail) {
            true => ExitCode::FAILURE,
            false => ExitCode::SUCCESS,
        },
    )
}

/*
 * the checks of the shell this runs in, for Settings' Health page (ADR 0088): the same as
 * `kanade doctor`, which would only ask this shell about itself. Slow and blocking, so off the draw
 * thread
 */
pub fn in_shell() -> Vec<Check> {
    let pid = std::process::id();
    let shell = Check::ok(format!(
        "shell: running kanade {}, protocol {}, pid {pid}",
        env!("CARGO_PKG_VERSION"),
        cli::PROTOCOL
    ));

    gather(shell, Some(pid))
}

// every check, with `shell` the check of the shell and `pid` its pid when it runs and is talked to
fn gather(shell: Check, pid: Option<u32>) -> Vec<Check> {
    let (config, problems) = config::read();
    let running = pid.is_some();

    let mut checks = vec![
        Check::ok(format!(
            "kanade {}, protocol {}",
            env!("CARGO_PKG_VERSION"),
            cli::PROTOCOL
        )),
        shell,
        unit(pid),
        niri(),
    ];

    let blur = config.appearance.finish == Finish::Blur;

    checks.extend(wayland::check(blur));
    checks.extend(sockets());
    checks.push(config_check(problems));
    checks.extend(modules(&config, running));
    checks.extend(backdrop(&config));
    checks.extend(login_screen(&config));
    checks.extend(blur.then(blur_check).flatten());
    checks.extend(screencast_check(&config));
    checks.extend(recording_check(&config));
    checks.extend(sounds(&config));

    checks
}

/*
 * a shell that is not running is no fault of doctor's to fix; one that cannot be talked to is.
 * With the pid of one that runs and is talked to
 */
fn shell() -> (Check, Option<u32>) {
    match cli::call(&[String::from("status")]) {
        Ok(Reply::Done(status)) => shell_status(status.lines().next().unwrap_or_default()),
        Ok(Reply::Refused(refused)) => (
            Check::fail(format!("shell: refused status: {refused}")),
            None,
        ),
        Ok(Reply::Unknown(unknown)) => (
            Check::fail(format!("shell: status unclear: {unknown}")),
            None,
        ),
        Err(problem) if problem.starts_with("no shell is running") => {
            (Check::warn(format!("shell: {problem}")), None)
        }
        Err(problem) => (Check::fail(format!("shell: {problem}")), None),
    }
}

// from the first line of its status, like "kanade 0.1.0, protocol 4, pid 1234"
fn shell_status(first: &str) -> (Check, Option<u32>) {
    let said = first
        .rsplit_once(", protocol ")
        .and_then(|(_, rest)| rest.split_once(", pid "))
        .and_then(|(protocol, pid)| {
            Some((protocol.parse::<u32>().ok()?, pid.parse::<u32>().ok()?))
        });

    match said {
        Some((cli::PROTOCOL, pid)) => (Check::ok(format!("shell: running {first}")), Some(pid)),
        _ => (
            Check::fail(format!(
                "shell: running {first}, not protocol {}; restart it",
                cli::PROTOCOL
            )),
            None,
        ),
    }
}

const SYSTEMD: &str = "org.freedesktop.systemd1";

// what systemd says of the unit
#[derive(Default)]
struct Unit {
    load: String,
    active: String,
    // UnitFileState: whether the next session starts it
    file: String,
    // MainPID, 0 when it runs nothing
    pid: u32,
}

/*
 * the user unit that restarts a crashed shell, which a locked session needs (ADR 0018), with
 * `shell` the pid of the shell doctor talked to
 */
fn unit(shell: Option<u32>) -> Check {
    let bus = Bus::session();

    if !bus::reachable(bus) {
        return Check::warn(format!(
            "unit {}: unknown, the session bus is unreachable",
            lock::UNIT
        ));
    }

    // GetUnit, not LoadUnit, so doctor loads nothing; it fails for a unit systemd has not
    // loaded, like one not enabled
    let path = bus.call(
        SYSTEMD,
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "GetUnit",
        &[Argument::from(lock::UNIT)],
    );
    if path.text().is_empty() {
        return unit_check(
            shell,
            &Unit {
                load: String::from("not loaded"),
                ..Unit::default()
            },
        );
    }

    let property = |interface, name| bus.property(SYSTEMD, path.text(), interface, name);
    let state = |name| {
        let value = property("org.freedesktop.systemd1.Unit", name);
        value.text().to_owned()
    };
    let pid = property("org.freedesktop.systemd1.Service", "MainPID").number();

    unit_check(
        shell,
        &Unit {
            load: state("LoadState"),
            active: state("ActiveState"),
            file: state("UnitFileState"),
            pid: pid as u32,
        },
    )
}

/*
 * whether the unit runs the shell doctor talked to, `shell` its pid, and the next session starts
 * it again
 */
fn unit_check(shell: Option<u32>, state: &Unit) -> Check {
    let unit = lock::UNIT;
    let unrestarted = "a crashed shell is not restarted, which leaves a locked session on \
                       niri's red screen";
    let (load, active, file) = (&*state.load, &*state.active, &*state.file);
    let enabled = matches!(file, "enabled" | "enabled-runtime");

    match (load, active) {
        ("loaded", "active" | "activating" | "reloading")
            if let Some(pid) = shell
                && pid != state.pid =>
        {
            Check::warn(format!(
                "unit {unit}: {active}, but its pid {} is not the shell's {pid}, so the shell \
                 running is not its: {unrestarted}",
                state.pid
            ))
        }
        ("loaded", "active" | "activating" | "reloading") if !enabled => Check {
            detail: vec![format!("systemctl --user enable {unit}")],
            ..Check::warn(format!(
                "unit {unit}: {active} but {file}, so the next session starts no shell"
            ))
        },
        ("loaded", "active" | "activating" | "reloading") => match shell {
            Some(pid) => Check::ok(format!(
                "unit {unit}: {active}, {file}, runs the shell, pid {pid}"
            )),
            None => Check::ok(format!("unit {unit}: {active}, {file}")),
        },
        ("loaded", "failed") => Check {
            detail: vec![
                String::from("once its cause is fixed:"),
                format!("systemctl --user reset-failed {unit}"),
                format!("systemctl --user restart {unit}"),
            ],
            ..Check::warn(format!(
                "unit {unit}: failed, maybe its start limit: {unrestarted}"
            ))
        },
        ("loaded", _) if shell.is_some() => Check::warn(format!(
            "unit {unit}: {active}, so the shell running is not its: {unrestarted}"
        )),
        ("loaded", _) => Check::warn(format!("unit {unit}: {active}")),
        ("", _) => Check::warn(format!("unit {unit}: unknown, systemd did not answer")),
        _ => Check::warn(format!(
            "unit {unit}: {load}, see docs/wiki/Installation.md to install it: {unrestarted}"
        )),
    }
}

fn niri() -> Check {
    match niri_version() {
        Ok(version) => niri_check(&version),
        Err(problem) => Check::fail(format!("niri: {problem}")),
    }
}

fn niri_version() -> Result<String, String> {
    let reply = niri::ask("\"Version\"").map_err(|error| error.to_string())?;

    reply
        .get("Version")
        .and_then(Json::as_str)
        .map(String::from)
        .ok_or_else(|| String::from("unexpected reply to Version"))
}

// like "26.04 (8ed0da4)", or "26.04.1 (...)" for a patch release, which leaves the minimum alone
fn niri_check(version: &str) -> Check {
    let mut parts = version
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .next()
        .unwrap_or_default()
        .split('.')
        .map(str::parse::<u32>);

    let release = match (parts.next(), parts.next()) {
        (Some(Ok(year)), Some(Ok(month))) => Some((year, month)),
        _ => None,
    };

    match release {
        Some(release) if release >= NIRI => Check::ok(format!("niri {version}")),
        Some(_) => Check::fail(format!(
            "niri {version}, Kanade needs {}.{:02} or later",
            NIRI.0, NIRI.1
        )),
        None => Check::warn(format!("niri {version}: cannot tell the release")),
    }
}

// the buses Modules read, and the PipeWire and PulseAudio servers the audio and privacy ones do
fn sockets() -> Vec<Check> {
    let bus = |name: &str, bus: Bus| match bus::reachable(bus) {
        true => Check::ok(format!("{name} bus reachable")),
        false => Check::warn(format!(
            "{name} bus unreachable: the Modules that read it see nothing"
        )),
    };

    vec![
        bus("session", Bus::session()),
        bus("system", Bus::system()),
        pipewire(),
        pulseaudio(),
    ]
}

const NO_INDICATORS: &str =
    "microphone and camera indicators, audio devices and app streams will not show";
const NO_VOLUME: &str = "volume level and OSD will not show";

// libpipewire finds its socket from PIPEWIRE_REMOTE (a name, a path, an abstract socket or a list of
// them), its runtime dirs and the system socket, so pw-dump, the client privacy and audio run, is asked
fn pipewire() -> Check {
    match ask(pipewire::DUMP, &[]) {
        Asked::Answered(_) => Check::ok(String::from("PipeWire reachable")),
        Asked::Refused(error) => Check::warn(format!(
            "PipeWire: {}: {error}: {NO_INDICATORS}",
            pipewire::DUMP
        )),
        Asked::Absent => default_socket(
            "PipeWire",
            pipewire::DUMP,
            env::var_os("PIPEWIRE_RUNTIME_DIR")
                .or_else(|| env::var_os("XDG_RUNTIME_DIR"))
                .map(|dir| Path::new(&dir).join("pipewire-0")),
            NO_INDICATORS,
        ),
    }
}

// libpulse finds its server from PULSE_SERVER, client.conf, X11 or the default socket, so `pactl
// info`, which resolves it the same way the runtime does, is asked
fn pulseaudio() -> Check {
    match ask("pactl", &["info"]) {
        Asked::Answered(info) => {
            let server = info
                .lines()
                .find_map(|line| line.strip_prefix("Server String: "))
                .unwrap_or("its server");
            Check::ok(format!("PulseAudio at {server}"))
        }
        Asked::Refused(error) => {
            Check::warn(format!("PulseAudio: pactl info: {error}: {NO_VOLUME}"))
        }
        Asked::Absent => default_socket(
            "PulseAudio",
            "pactl",
            env::var_os("PULSE_RUNTIME_PATH")
                .map(PathBuf::from)
                .or_else(|| env::var_os("XDG_RUNTIME_DIR").map(|dir| Path::new(&dir).join("pulse")))
                .map(|dir| dir.join("native")),
            NO_VOLUME,
        ),
    }
}

enum Asked {
    Answered(String),
    // the first line of what it said went wrong
    Refused(String),
    Absent,
}

// a client that resolves its server the way the Modules' do, so doctor need not
fn ask(program: &str, arguments: &[&str]) -> Asked {
    match Command::new(program).args(arguments).output() {
        Ok(output) if output.status.success() => {
            Asked::Answered(String::from_utf8_lossy(&output.stdout).into_owned())
        }
        Ok(output) => Asked::Refused(
            String::from_utf8_lossy(&output.stderr)
                .lines()
                .next()
                .unwrap_or("failed")
                .to_owned(),
        ),
        Err(_) => Asked::Absent,
    }
}

// without its client only the default local socket can be looked at, and its absence proves nothing
fn default_socket(name: &str, client: &str, socket: Option<PathBuf>, without: &str) -> Check {
    match socket {
        Some(path) if UnixStream::connect(&path).is_ok() => {
            Check::ok(format!("{name} at {}", path.display()))
        }
        _ => Check::warn(format!(
            "{name}: {client} is missing to ask, and the default local socket answers nothing: \
             maybe {without}"
        )),
    }
}

fn config_check(problems: Vec<String>) -> Check {
    if problems.is_empty() {
        return Check::ok(String::from("config valid"));
    }

    Check {
        detail: problems,
        ..Check::fail(String::from(
            "config invalid; each problem is skipped alone",
        ))
    }
}

// which Modules run with this config, why one is not as asked, and what each that runs needs
fn modules(config: &Config, running: bool) -> Vec<Check> {
    let resolved = modules::resolve(modules::ALL, |name| config.off(name));

    let mut checks: Vec<Check> = modules::ALL
        .iter()
        .filter_map(|module| {
            let state = resolved.state(module.name)?;

            match (state.problem(module.name), module.warns) {
                (Some(problem), _) => Some(Check::warn(problem)),
                (None, Some(warning)) if !resolved.on(module.name) => Some(Check::warn(format!(
                    "module {} is off: {warning}",
                    module.name
                ))),
                _ => None,
            }
        })
        .collect();

    let on: Vec<&Module> = modules::ALL
        .iter()
        .filter(|module| resolved.on(module.name))
        .collect();

    checks.insert(
        0,
        Check::ok(format!(
            "modules on: {}",
            on.iter()
                .map(|module| module.name)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    );

    let session = bus::reachable(Bus::session());
    let system = bus::reachable(Bus::system());

    for module in on {
        for need in module.needs {
            let found = match need.on {
                Provider::Program(program) => program_found(program, env::var_os("PATH")),
                Provider::SystemService(name) if system => service_found(Bus::system(), name),
                Provider::SessionService(name) if session => service_found(Bus::session(), name),
                Provider::SessionName(name) if session => holder(
                    name,
                    bus::owner(Bus::session(), name).map(bus::process),
                    running,
                ),
                Provider::Pam => {
                    let service = lock::service_of(config);
                    pam_found(service, lock::pam_file(service))
                }
                Provider::Niri => niri_found(niri_version()),
                Provider::Paste => match clipboard::paste_version() {
                    Ok(version) => Found::Present(version),
                    Err(why) => Found::Missing(why),
                },
                Provider::Keyboards => match keys::found() {
                    Ok(found) => Found::Present(found),
                    Err(why) => Found::Missing(why),
                },
                Provider::Rfkill => match radios::found() {
                    Ok(found) => Found::Present(found),
                    Err(why) => Found::Missing(why),
                },
                Provider::SystemService(name)
                | Provider::SessionName(name)
                | Provider::SessionService(name) => {
                    Found::Missing(format!("{name} unknown, its bus is unreachable"))
                }
            };

            checks.push(found.check(module.name, need.without));
        }
    }

    // cava only matters to the visualizer that asks for it; without it the synthetic bars draw
    if resolved.on("media") && config.visualizer == Visualizer::Audio {
        checks.push(
            program_found(cava::CAVA, env::var_os("PATH"))
                .check("media", "the audio visualizer draws the synthetic bars"),
        );
    }

    // two idle daemons would each lock, dim and suspend on their own timers
    if resolved.on("idle") {
        for daemon in IDLE_DAEMONS.iter().filter(|daemon| process_running(daemon)) {
            checks.push(Check::warn(format!(
                "{daemon} runs while module idle is on, so both act on idle; stop {daemon} or set `[modules] idle = false`"
            )));
        }
    }

    // a grace is a window with no password, so it is said where the lock is checked (ADR 0066)
    if resolved.on("lock")
        && let Some(grace) = lock::grace_of(config)
    {
        checks.push(Check::warn(format!(
            "lock.grace is {} s: any input unlocks without a password for that long after a lock the idle Module asked for{}",
            grace.as_secs(),
            if resolved.on("idle") {
                ""
            } else {
                " (module idle is off, so it never applies)"
            }
        )));
    }

    checks
}

// idle daemons that would act beside the idle Module
const IDLE_DAEMONS: &[&str] = &["hypridle", "swayidle"];

// notification sounds play through a helper, so say whether one is found; Kanade works without. A
// chosen sound file that is missing, or name no player can find, is named too: it is skipped for the next sound
fn sounds(config: &Config) -> Vec<Check> {
    if !modules::resolve(modules::ALL, |name| config.off(name)).on("notifications") {
        return Vec::new();
    }

    let mut checks = vec![match (sound::player(), config.notification_sound) {
        (Some(player), true) => Check::ok(format!("notifications.sound: {player} plays them")),
        (Some(player), false) => Check::ok(format!(
            "notifications.sound is off; {player} would play them"
        )),
        (None, true) => Check::warn(format!(
            "notifications.sound is on, but none of {} is on PATH: no sound plays",
            sound::PLAYERS.join(", ")
        )),
        (None, false) => Check::ok(format!(
            "notifications.sound is off; none of {} is on PATH",
            sound::PLAYERS.join(", ")
        )),
    }];

    if config.notification_sound {
        checks.extend(
            sound::unusable(&config.notification_sounds)
                .into_iter()
                .map(Check::warn),
        );
    }

    checks
}

// the login screen: Kanade writes an overlay SDDM reads, so say whether it is made, read and
// readable by the sddm user, and whether a Qt greeter exists to draw it. Silent when the Module is
// off; one line when the overlay is not made, which is how it starts
fn login_screen(config: &Config) -> Vec<Check> {
    if !modules::resolve(modules::ALL, |name| config.off(name)).on("sddm") {
        return Vec::new();
    }

    let mut checks: Vec<Check> = sddm::findings(config)
        .into_iter()
        .map(|finding| {
            let mut check = if finding.ok {
                Check::ok(finding.text)
            } else {
                Check::warn(finding.text)
            };
            check.detail = finding.detail;
            check
        })
        .collect();

    let path = env::var_os("PATH");
    let qt6_found = greeter_found("sddm-greeter-qt6", path.as_ref());
    let qt5_found = greeter_found("sddm-greeter", path.as_ref());
    let theme_qt = sddm::configured_qt_version(config);
    let made = checks.len() > 1;

    let greeter_check = match (qt6_found, qt5_found) {
        (Some(qt6), _) if theme_qt == 6 => Check::ok(format!("sddm: greeter {qt6}")),
        (_, Some(qt5)) if theme_qt == 5 => Check::ok(format!("sddm: greeter {qt5}")),
        (None, Some(qt5)) if theme_qt == 6 => Check::warn(format!(
            "sddm: theme requests QtVersion=6, but only {qt5} (Qt5) was found (install sddm-greeter-qt6)"
        )),
        (Some(qt6), None) if theme_qt == 5 => Check::warn(format!(
            "sddm: theme requests QtVersion=5, but only {qt6} (Qt6) was found (install sddm-greeter)"
        )),
        (Some(qt6), _) => Check::ok(format!("sddm: greeter {qt6}")),
        (_, Some(qt5)) => Check::ok(format!("sddm: greeter {qt5}")),
        (None, None) if made => Check::warn(format!(
            "sddm: none of {} was found, so there is no Qt greeter to draw the theme",
            SDDM_GREETERS.join(", ")
        )),
        (None, None) => Check::ok(format!(
            "sddm: none of {} was found (SDDM is not installed?)",
            SDDM_GREETERS.join(", ")
        )),
    };
    checks.push(greeter_check);

    checks
}

const SDDM_GREETERS: &[&str] = &["sddm-greeter-qt6", "sddm-greeter"];
const SDDM_GREETER_DIRS: &[&str] = &["/usr/libexec", "/usr/lib/sddm", "/usr/lib64/sddm"];

fn greeter_found(program: &str, path: Option<impl AsRef<OsStr>>) -> Option<String> {
    greeter_found_in(program, path, SDDM_GREETER_DIRS)
}

fn greeter_found_in(
    program: &str,
    path: Option<impl AsRef<OsStr>>,
    system_dirs: &[impl AsRef<Path>],
) -> Option<String> {
    if let Found::Present(found) = program_found(program, path) {
        return Some(found);
    }
    for dir in system_dirs {
        let candidate = dir.as_ref().join(program);
        if executable(&candidate) {
            return Some(format!("{program} at {}", candidate.display()));
        }
    }
    None
}

// whether a process of this name runs, as `/proc/<pid>/comm` names it
fn process_running(name: &str) -> bool {
    let Ok(entries) = fs::read_dir("/proc") else {
        return false;
    };

    entries.flatten().any(|entry| {
        fs::read_to_string(entry.path().join("comm")).is_ok_and(|comm| comm.trim_end() == name)
    })
}

// what doctor found of a Need
#[derive(Debug, PartialEq, Eq)]
enum Found {
    Present(String),

    // the Module runs without what it gives
    Missing(String),

    // another program has it, so the Module gets nothing until it is stopped
    Taken(String),

    // the Module cannot work without it
    Absent(String),
}

impl Found {
    fn check(self, module: &str, without: &str) -> Check {
        match self {
            Found::Present(found) => Check::ok(format!("module {module}: {found}")),
            Found::Missing(missing) => {
                Check::warn(format!("module {module}: {missing}: {without}"))
            }
            Found::Taken(taken) => Check::fail(format!(
                "module {module}: {taken}: {without}; stop it and restart Kanade"
            )),
            Found::Absent(absent) => Check::fail(format!("module {module}: {absent}: {without}")),
        }
    }
}

fn program_found(program: &str, path: Option<impl AsRef<OsStr>>) -> Found {
    match path.and_then(|path| find(program, path.as_ref())) {
        Some(found) => Found::Present(format!("{program} at {}", found.display())),
        None => Found::Missing(format!("{program} missing from PATH")),
    }
}

// without the service's file PAM falls back to `other`, which usually refuses every password
fn pam_found(service: &str, file: Option<PathBuf>) -> Found {
    match file {
        Some(file) => Found::Present(format!("PAM service {service} at {}", file.display())),
        None if !lock::valid(service) => Found::Absent(format!(
            "lock.pam_service {service:?} is no PAM service file name (letters, digits, dots, dashes and underscores)"
        )),
        None => Found::Absent(format!("no PAM service {service}")),
    }
}

/*
 * whether niri's overview shows the wallpaper: it draws a Background layer in the overview only
 * under a layer rule with `place-within-backdrop`, in niri's config (edited by Kanade only on request). Asked only
 * while the `wallpaper` Module runs awww, whose layers are named `awww-daemon`. Kanade's own
 * layers are Top, Bottom or Overlay, which the rule does not apply to
 */
fn backdrop(config: &Config) -> Option<Check> {
    if modules::resolve(modules::ALL, |name| config.off(name)).on("wallpaper") {
        let path = binds::config()?;
        let found = binds::places_within_backdrop(&path, wallpaper::DAEMON, &mut |file| {
            fs::read_to_string(file)
        });

        Some(backdrop_check(found))
    } else {
        None
    }
}

fn backdrop_check(found: Result<bool, binds::Unread>) -> Check {
    match found {
        Ok(true) => Check::ok(String::from(
            "niri: a layer rule places the wallpaper in the overview backdrop",
        )),
        Ok(false) => Check {
            detail: vec![
                String::from("add to niri's config.kdl, see docs/wiki/Installation.md:"),
                format!(
                    "layer-rule {{ match namespace=\"^{}\"; place-within-backdrop true; }}",
                    wallpaper::DAEMON
                ),
            ],
            ..Check::warn(String::from(
                "niri: no layer rule places the wallpaper in the overview backdrop, so the \
                 overview shows niri's background color behind the windows",
            ))
        },
        Err(unread) => Check::warn(format!(
            "niri: cannot tell whether the wallpaper is in the overview backdrop: {}: {}",
            unread.file.display(),
            unread.why
        )),
    }
}

// the layers whose panes blur behind them under the finish `blur`
const BLURRED: [&str; 3] = ["kanade", "kanade-dock", "kanade-banners"];

/*
 * whether niri blurs the windows behind Kanade's panes, not only the wallpaper: it blurs what a
 * layer asks for xray, the wallpaper alone, unless a layer rule in niri's config (edited by Kanade only on request)
 * sets `background-effect { xray false }` (ADR 0057). Asked only while the finish is `blur`
 */
fn blur_check() -> Option<Check> {
    let path = binds::config()?;
    let found = binds::xrayed(&path, &BLURRED, &mut |file| fs::read_to_string(file));

    Some(match found {
        Ok(xrayed) if xrayed.is_empty() => Check::ok(String::from(
            "niri: a layer rule blurs the windows behind the panes, not only the wallpaper",
        )),
        Ok(xrayed) => Check {
            detail: vec![
                String::from("add to niri's config.kdl, see docs/wiki/Installation.md:"),
                String::from(
                    "layer-rule { match namespace=\"^kanade(-dock|-banners)?$\"; \
                     background-effect { xray false; }; }",
                ),
            ],
            ..Check::warn(format!(
                "niri: no layer rule sets xray false on {}, so the finish blur blurs only the \
                 wallpaper behind those panes, and windows show through them sharp",
                xrayed.join(", ")
            ))
        },
        Err(unread) => Check::warn(format!(
            "niri: cannot tell whether the finish blur blurs the windows behind the panes: {}: {}",
            unread.file.display(),
            unread.why
        )),
    })
}

/*
 * whether Banners, which show a notification's text, are hidden from screencasts: only by an
 * opt-in layer rule in niri's config (edited by Kanade only on request), never a warning (ADR 0070). Asked only
 * while the `banners` Module runs
 */
fn screencast_check(config: &Config) -> Option<Check> {
    if !modules::resolve(modules::ALL, |name| config.off(name)).on("banners") {
        return None;
    }

    let path = binds::config()?;
    let found = binds::blocked_from_screencasts(&path, "kanade-banners", &mut |file| {
        fs::read_to_string(file)
    });

    Some(match found {
        Ok(true) => Check::ok(String::from(
            "niri: a layer rule blocks Banners out of screencasts",
        )),
        Ok(false) => Check {
            detail: vec![
                String::from(
                    "opt in with this rule in niri's config.kdl, see docs/wiki/Installation.md:",
                ),
                String::from(
                    "layer-rule { match namespace=\"^kanade-banners$\"; block-out-from \"screencast\"; }",
                ),
            ],
            ..Check::ok(String::from(
                "niri: Banners show on screencasts (opt-in layer rule not set)",
            ))
        },
        Err(unread) => Check::warn(format!(
            "niri: cannot tell whether Banners are blocked out of screencasts: {}: {}",
            unread.file.display(),
            unread.why
        )),
    })
}

/*
 * the encoder a recording would use, which encodes a frame to find out (ADR 0116, Update): ok on a
 * GPU's, a warning on the CPU's, which costs processor time, unless `encoder = software` asked
 * for it, and a failure with none. Always tried afresh, not as a recording remembers. Why, and
 * what to install for a GPU that is not used. Asked only while the `capture` Module runs and
 * `wf-recorder` is there
 */
fn recording_check(config: &Config) -> Option<Check> {
    if !modules::resolve(modules::ALL, |name| config.off(name)).on("capture")
        || !wake::found(recording::RECORDER)
    {
        return None;
    }

    let report = recording::report(&config.recording);
    let mut detail = report.notes;
    let window_failed = match &report.window {
        Some(Ok(line)) => {
            detail.push(line.clone());
            false
        }
        Some(Err(line)) => {
            detail.push(line.clone());
            true
        }
        None => false,
    };

    detail.extend(
        report
            .hints
            .iter()
            .map(|hint| format!("install {hint} to record on the GPU")),
    );

    let quality = config.recording.quality.name();

    let check = match (report.encoder, report.gpu) {
        (Some(encoder), true) => Check {
            detail,
            ..Check::ok(format!(
                "recording: {encoder}, quality {quality} (quantizer {})",
                report.quantizer.unwrap_or_default()
            ))
        },
        (Some(encoder), false) if config.recording.encoder == Preference::Software => Check {
            detail,
            ..Check::ok(format!(
                "recording: {encoder}, as set, quality {quality} (crf {})",
                report.quantizer.unwrap_or_default()
            ))
        },
        (Some(encoder), false) => Check {
            detail,
            ..Check::warn(format!(
                "recording: {encoder} on the CPU, quality {quality} (crf {})",
                report.quantizer.unwrap_or_default()
            ))
        },
        (None, _) => Check {
            detail,
            ..Check::fail(String::from(
                "recording: no encoder works; install ffmpeg with libx264, or a GPU driver",
            ))
        },
    };

    // recording a screen or an area does not need GStreamer: only a window's chain is in doubt
    Some(match check.verdict {
        Verdict::Ok if window_failed => Check {
            verdict: Verdict::Warn,
            ..check
        },
        _ => check,
    })
}

// niri as a Module needs it: reachable, and new enough for what Kanade asks of it
fn niri_found(version: Result<String, String>) -> Found {
    match version {
        Ok(version) if niri_check(&version).verdict != Verdict::Fail => {
            Found::Present(format!("niri {version}"))
        }
        Ok(version) => Found::Absent(format!("niri {version} is too old")),
        Err(problem) => Found::Absent(format!("niri: {problem}")),
    }
}

// the first executable `program` on `path`, as a shell finds it
fn find(program: &str, path: &OsStr) -> Option<PathBuf> {
    env::split_paths(path)
        .map(|dir| dir.join(program))
        .find(|candidate| executable(candidate))
}

fn executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn service_found(bus: Bus, name: &str) -> Found {
    if bus::owner(bus, name).is_some() {
        Found::Present(format!("{name} running"))
    } else if bus::activatable(bus, name) {
        Found::Present(format!("{name} starts on demand"))
    } else {
        Found::Missing(format!("{name} not running"))
    }
}

/*
 * who holds a session name Kanade takes: no one is fine before the shell takes it, Kanade is fine,
 * anyone else keeps it from Kanade for as long as it runs
 */
fn holder(name: &str, holder: Option<String>, running: bool) -> Found {
    match holder.as_deref() {
        Some("kanade") => Found::Present(format!("{name} held by Kanade")),
        Some(other) => Found::Taken(format!("{other} holds {name}")),
        None if running => Found::Missing(format!("{name} held by no one")),
        None => Found::Present(format!("{name} free, Kanade takes it at start")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_checks_sddm_greeter_qt_version_compatibility() {
        let temp = std::env::temp_dir().join(format!("kanade-doctor-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        fs::create_dir_all(&temp).unwrap();

        // Create only sddm-greeter (Qt5)
        let qt5 = temp.join("sddm-greeter");
        fs::write(&qt5, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&qt5, fs::Permissions::from_mode(0o755)).unwrap();

        let new_path = std::env::join_paths(std::iter::once(temp.clone())).unwrap();

        let _config = Config {
            off: vec![],
            ..Config::default()
        };

        // Kanade default theme requires QtVersion 6
        let qt6_found = match program_found("sddm-greeter-qt6", Some(&new_path)) {
            Found::Present(found) => Some(found),
            _ => None,
        };
        let qt5_found = match program_found("sddm-greeter", Some(&new_path)) {
            Found::Present(found) => Some(found),
            _ => None,
        };
        assert!(qt6_found.is_none());
        assert!(qt5_found.is_some());

        let theme_qt = 6;
        let mismatch = matches!(
            (qt6_found.as_ref(), qt5_found.as_ref()),
            (None, Some(_)) if theme_qt == 6
        );
        assert!(
            mismatch,
            "mismatch detected when only Qt5 greeter is present for Qt6 theme"
        );

        let _ = fs::remove_dir_all(&temp);
    }

    #[test]
    fn doctor_finds_greeter_in_candidate_system_dirs() {
        let temp =
            std::env::temp_dir().join(format!("kanade-doctor-sysdir-{}", std::process::id()));
        let _ = fs::remove_dir_all(&temp);
        let lib_dir = temp.join("usr/lib/sddm");
        fs::create_dir_all(&lib_dir).unwrap();

        let greeter = lib_dir.join("sddm-greeter-qt6");
        fs::write(&greeter, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&greeter, fs::Permissions::from_mode(0o755)).unwrap();

        let empty_path: Option<&std::ffi::OsStr> = None;
        let found = greeter_found_in("sddm-greeter-qt6", empty_path, &[&lib_dir]);
        assert!(found.is_some());
        assert!(found.unwrap().contains(&greeter.display().to_string()));

        let _ = fs::remove_dir_all(&temp);
    }
}
