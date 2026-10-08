//! `kanade doctor` (#105, docs/design.md CLI): read-only diagnostics of what Kanade runs on, a line
//! each: versions, the shell, niri, the Wayland protocols, the buses and PipeWire, the config, the
//! Modules it turns on and what each needs outside Kanade. Runs without a shell, which may be what is
//! wrong, so it reads everything itself. A failure means Kanade, or a Module that is on, cannot work;
//! a warning, that it runs worse. There is no fix mode yet: Kanade owns no state a fix could touch.

mod wayland;

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use amane::{Argument, Bus};

use crate::cli::{self, Reply};
use crate::config::{self, Config};
use crate::lock;
use crate::modules::{self, Module, Provider};
use crate::sources::json::Json;
use crate::sources::{bus, clipboard, niri, pipewire};

// the oldest niri Kanade follows (docs/design.md Constraints)
const NIRI: (u32, u32) = (26, 4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Check {
    verdict: Verdict,
    text: String,

    // lines under it, like each config problem
    detail: Vec<String>,
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
    let (config, problems) = config::read();
    let (shell, pid) = shell();
    let running = pid.is_some();

    let mut checks = vec![
        Check::ok(format!(
            "kanade {}, protocol {}",
            env!("CARGO_PKG_VERSION"),
            cli::PROTOCOL
        )),
        amane(include_str!("../../Cargo.lock")),
        shell,
        unit(pid),
        niri(),
    ];

    checks.extend(wayland::check());
    checks.extend(sockets());
    checks.push(config_check(problems));
    checks.extend(modules(&config, running));

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

// the pinned Amane, as Cargo.lock has it
fn amane(lock: &str) -> Check {
    let package = lock
        .split("[[package]]")
        .find(|package| package.contains("\nname = \"amane\"\n"));

    let field = |name: &str| {
        package?.lines().find_map(|line| {
            line.strip_prefix(name)?
                .strip_prefix(" = \"")?
                .strip_suffix('"')
        })
    };

    let rev = field("source")
        .and_then(|source| source.rsplit_once('#'))
        .map(|(_, rev)| &rev[..rev.len().min(7)]);

    match (field("version"), rev) {
        (Some(version), Some(rev)) => Check::ok(format!("amane {version}, rev {rev}")),
        _ => Check::warn(String::from("amane: not found in Cargo.lock")),
    }
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
            "unit {unit}: {load}, see the README to install it: {unrestarted}"
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
// info`, which resolves it the same way Amane does, is asked
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
                Provider::Pam(service) => pam_found(service, lock::pam()),
                Provider::Niri => niri_found(niri_version()),
                Provider::Paste => match clipboard::paste_version() {
                    Ok(version) => Found::Present(version),
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

    checks
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
        None => Found::Absent(format!("no PAM service {service}")),
    }
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
    fn amane_is_read_from_the_lock() {
        assert_eq!(
            amane(
                "[[package]]
name = \"amane\"
version = \"0.1.1\"
source = \"git+https://github.com/MystiaFin/amane?rev=6ace43e5c69c#6ace43e5c69cd4b39bb06b28c20fc4b883422519\"
dependencies = [
 \"zbus\",
]

[[package]]
name = \"amane-cli\"
version = \"9.9.9\"
"
            ),
            Check::ok(String::from("amane 0.1.1, rev 6ace43e"))
        );
        assert_eq!(
            amane("[[package]]\nname = \"other\"\n").verdict,
            Verdict::Warn
        );

        // the one this build is pinned to
        assert_eq!(amane(include_str!("../../Cargo.lock")).verdict, Verdict::Ok);
    }

    #[test]
    fn a_shell_on_another_protocol_fails() {
        assert_eq!(
            shell_status("kanade 0.1.0, protocol 4, pid 1234"),
            (
                Check::ok(String::from(
                    "shell: running kanade 0.1.0, protocol 4, pid 1234"
                )),
                Some(1234)
            )
        );

        for first in [
            "kanade 0.1.0, protocol 3, pid 1234",
            "kanade 0.1.0, protocol 3",
            "something else",
            "",
        ] {
            let (check, pid) = shell_status(first);
            assert_eq!((check.verdict, pid), (Verdict::Fail, None), "{first}");
        }
    }

    #[test]
    fn niri_before_26_04_fails() {
        assert_eq!(
            niri_check("26.04 (8ed0da4)"),
            Check::ok(String::from("niri 26.04 (8ed0da4)"))
        );
        assert_eq!(niri_check("26.10").verdict, Verdict::Ok);
        assert_eq!(niri_check("27.01-1").verdict, Verdict::Ok);
        assert_eq!(
            niri_check("25.11 (b35bcae)"),
            Check::fail(String::from(
                "niri 25.11 (b35bcae), Kanade needs 26.04 or later"
            ))
        );
        assert_eq!(niri_check("unstable").verdict, Verdict::Warn);
    }

    #[test]
    fn a_niri_patch_release_is_judged_by_year_and_month() {
        assert_eq!(niri_check("26.04.1 (8ed0da4)").verdict, Verdict::Ok);
        assert_eq!(
            niri_check("25.05.1 (b35bcae)"),
            Check::fail(String::from(
                "niri 25.05.1 (b35bcae), Kanade needs 26.04 or later"
            ))
        );
    }

    #[test]
    fn another_notification_daemon_fails_its_module() {
        let name = "org.freedesktop.Notifications";

        assert_eq!(
            holder(name, Some(String::from("mako")), false)
                .check("notifications", "no notifications arrive"),
            Check::fail(String::from(
                "module notifications: mako holds org.freedesktop.Notifications: no notifications arrive; stop it and restart Kanade"
            ))
        );
        assert_eq!(
            holder(name, Some(String::from("kanade")), true),
            Found::Present(String::from("org.freedesktop.Notifications held by Kanade"))
        );
        assert_eq!(
            holder(name, None, false),
            Found::Present(String::from(
                "org.freedesktop.Notifications free, Kanade takes it at start"
            ))
        );
        assert!(matches!(holder(name, None, true), Found::Missing(_)));
    }

    #[test]
    fn capture_without_niri_fails_naming_its_backend() {
        let absent = niri_found(Err(String::from("NIRI_SOCKET is not set")));

        assert_eq!(
            absent.check("capture", "no screenshot backend"),
            Check::fail(String::from(
                "module capture: niri: NIRI_SOCKET is not set: no screenshot backend"
            ))
        );
        assert_eq!(
            niri_found(Ok(String::from("25.11 (b35bcae)"))),
            Found::Absent(String::from("niri 25.11 (b35bcae) is too old"))
        );
        assert_eq!(
            niri_found(Ok(String::from("26.04 (8ed0da4)"))),
            Found::Present(String::from("niri 26.04 (8ed0da4)"))
        );
    }

    #[test]
    fn a_program_missing_from_path_degrades_its_module() {
        let dir = env::temp_dir().join(format!("kanade-doctor-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();

        let program = dir.join("pw-dump");
        fs::write(&program, "").unwrap();

        // not executable, so not found
        assert_eq!(
            program_found("pw-dump", Some(&dir)),
            Found::Missing(String::from("pw-dump missing from PATH"))
        );

        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(
            program_found("pw-dump", Some(&dir)),
            Found::Present(format!("pw-dump at {}", program.display()))
        );
        assert_eq!(
            program_found("pw-dump", None::<&OsStr>).check("privacy", "no indicators"),
            Check::warn(String::from(
                "module privacy: pw-dump missing from PATH: no indicators"
            ))
        );

        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_shell_the_unit_does_not_run_warns() {
        let unit = |load: &str, active: &str, file: &str, pid| Unit {
            load: String::from(load),
            active: String::from(active),
            file: String::from(file),
            pid,
        };

        assert_eq!(
            unit_check(Some(7), &unit("loaded", "active", "enabled", 7)),
            Check::ok(String::from(
                "unit kanade.service: active, enabled, runs the shell, pid 7"
            ))
        );
        assert_eq!(
            unit_check(Some(8), &unit("loaded", "active", "enabled", 7)).text,
            "unit kanade.service: active, but its pid 7 is not the shell's 8, so the shell \
             running is not its: a crashed shell is not restarted, which leaves a locked session \
             on niri's red screen"
        );

        let disabled = unit_check(Some(7), &unit("loaded", "active", "disabled", 7));
        assert_eq!(
            disabled.text,
            "unit kanade.service: active but disabled, so the next session starts no shell"
        );
        assert_eq!(disabled.detail, ["systemctl --user enable kanade.service"]);

        assert_eq!(
            unit_check(Some(7), &unit("loaded", "inactive", "enabled", 0)).text,
            "unit kanade.service: inactive, so the shell running is not its: a crashed shell is \
             not restarted, which leaves a locked session on niri's red screen"
        );
        assert_eq!(
            unit_check(None, &unit("not-found", "inactive", "", 0)).text,
            "unit kanade.service: not-found, see the README to install it: a crashed shell is \
             not restarted, which leaves a locked session on niri's red screen"
        );

        let failed = unit_check(None, &unit("loaded", "failed", "enabled", 0));
        assert_eq!(failed.verdict, Verdict::Warn);
        assert_eq!(
            failed.detail[1..],
            [
                "systemctl --user reset-failed kanade.service",
                "systemctl --user restart kanade.service",
            ]
        );
    }

    #[test]
    fn the_lock_without_its_pam_service_fails() {
        assert_eq!(
            pam_found("login", None).check("lock", "no password unlocks"),
            Check::fail(String::from(
                "module lock: no PAM service login: no password unlocks"
            ))
        );
        assert_eq!(
            pam_found("login", Some(PathBuf::from("/etc/pam.d/login"))),
            Found::Present(String::from("PAM service login at /etc/pam.d/login"))
        );
    }

    #[test]
    fn an_invalid_config_fails_with_each_problem_under_it() {
        let check = config_check(vec![String::from("a.toml:3: unknown key colour")]);

        assert_eq!(
            check.render(),
            "fail config invalid; each problem is skipped alone
       a.toml:3: unknown key colour"
        );
        assert_eq!(config_check(Vec::new()).render(), "ok   config valid");
    }
}
