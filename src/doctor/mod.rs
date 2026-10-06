//! `kanade doctor` (#105, docs/design.md CLI): read-only diagnostics of what Kanade runs on, a line
//! each: versions, the shell, niri, the Wayland protocols, the buses and PipeWire, the config, the
//! Modules it turns on and what each needs outside Kanade. Runs without a shell, which may be what is
//! wrong, so it reads everything itself. A failure means Kanade, or a Module that is on, cannot work;
//! a warning, that it runs worse. There is no fix mode yet: Kanade owns no state a fix could touch.

mod wayland;

use std::env;
use std::ffi::OsStr;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use amane::Bus;

use crate::cli::{self, Reply};
use crate::config::{self, Config};
use crate::modules::{self, Module, Provider};
use crate::sources::json::Json;
use crate::sources::{bus, privacy};

// the oldest niri Kanade follows (docs/design.md Constraints)
const NIRI: (u32, u32) = (26, 4);

// how long niri may take to say its version
const PATIENCE: Duration = Duration::from_secs(2);

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
    let shell = shell();
    let running = shell.verdict == Verdict::Ok;

    let mut checks = vec![
        Check::ok(format!(
            "kanade {}, protocol {}",
            env!("CARGO_PKG_VERSION"),
            cli::PROTOCOL
        )),
        amane(include_str!("../../Cargo.lock")),
        shell,
        niri(),
    ];

    checks.extend(wayland::check());
    checks.extend(sockets());
    checks.push(config_check(problems));
    checks.extend(modules(&config, running));

    for check in &checks {
        println!("{}", check.render());
    }

    match checks.iter().any(|check| check.verdict == Verdict::Fail) {
        true => ExitCode::FAILURE,
        false => ExitCode::SUCCESS,
    }
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

// a shell that is not running is no fault of doctor's to fix; one that cannot be talked to is
fn shell() -> Check {
    match cli::call(&[String::from("status")]) {
        Ok(Reply::Done(status)) => shell_status(status.lines().next().unwrap_or_default()),
        Ok(Reply::Refused(refused)) => Check::fail(format!("shell: refused status: {refused}")),
        Err(problem) if problem.starts_with("no shell is running") => {
            Check::warn(format!("shell: {problem}"))
        }
        Err(problem) => Check::fail(format!("shell: {problem}")),
    }
}

// from the first line of its status, like "kanade 0.1.0, protocol 1"
fn shell_status(first: &str) -> Check {
    let protocol = first
        .rsplit_once("protocol ")
        .and_then(|(_, protocol)| protocol.parse::<u32>().ok());

    match protocol {
        Some(cli::PROTOCOL) => Check::ok(format!("shell: running {first}")),
        _ => Check::fail(format!(
            "shell: running {first}, not protocol {}; restart it",
            cli::PROTOCOL
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
    let path = env::var("NIRI_SOCKET").map_err(|_| String::from("NIRI_SOCKET is not set"))?;
    let failed = |error: std::io::Error| format!("{path}: {error}");

    let mut stream = UnixStream::connect(&path).map_err(failed)?;
    stream.set_read_timeout(Some(PATIENCE)).map_err(failed)?;
    stream.write_all(b"\"Version\"\n").map_err(failed)?;

    let mut reply = String::new();
    BufReader::new(stream)
        .read_line(&mut reply)
        .map_err(failed)?;

    Json::parse(&reply)
        .as_ref()
        .and_then(|reply| reply.get("Ok")?.get("Version")?.as_str().map(String::from))
        .ok_or_else(|| format!("unexpected reply to Version: {}", reply.trim_end()))
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

const NO_INDICATORS: &str = "microphone and camera indicators will not show";
const NO_VOLUME: &str = "volume level and OSD will not show";

// libpipewire finds its socket from PIPEWIRE_REMOTE (a name, a path, an abstract socket or a list of
// them), its runtime dirs and the system socket, so pw-dump, the client privacy runs, is asked
fn pipewire() -> Check {
    match ask(privacy::DUMP, &[]) {
        Asked::Answered(_) => Check::ok(String::from("PipeWire reachable")),
        Asked::Refused(error) => Check::warn(format!(
            "PipeWire: {}: {error}: {NO_INDICATORS}",
            privacy::DUMP
        )),
        Asked::Absent => default_socket(
            "PipeWire",
            privacy::DUMP,
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
                Provider::SystemService(name) if system => service_found(name),
                Provider::SessionName(name) if session => holder(
                    name,
                    bus::owner(Bus::session(), name).map(bus::process),
                    running,
                ),
                Provider::SystemService(name) | Provider::SessionName(name) => {
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
        }
    }
}

fn program_found(program: &str, path: Option<impl AsRef<OsStr>>) -> Found {
    match path.and_then(|path| find(program, path.as_ref())) {
        Some(found) => Found::Present(format!("{program} at {}", found.display())),
        None => Found::Missing(format!("{program} missing from PATH")),
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

fn service_found(name: &str) -> Found {
    let system = Bus::system();

    if bus::owner(system, name).is_some() {
        Found::Present(format!("{name} running"))
    } else if bus::activatable(system, name) {
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
            shell_status("kanade 0.1.0, protocol 1"),
            Check::ok(String::from("shell: running kanade 0.1.0, protocol 1"))
        );

        for first in ["kanade 0.2.0, protocol 2", "something else", ""] {
            assert_eq!(shell_status(first).verdict, Verdict::Fail, "{first}");
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
