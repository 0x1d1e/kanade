//! `kanade update`: replace the installed binary with the published Arch x86_64 one, then refresh
//! the user unit. It does not need the shell. A failed download leaves the installed binary as it
//! was. The path is the one `scripts/install` writes, `$HOME/.cargo/bin/kanade`, because the unit's
//! `ExecStart` is `%h/.cargo/bin/kanade`. `CARGO_HOME` is not consulted.

use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

use ring::digest;
use ureq::Agent;

use crate::atomic::{self, Mode};
use crate::cli;
use crate::config;

const RELEASE: &str = "https://github.com/0x1d1e/kanade/releases/download/install-0.1";
const RAW: &str = "https://raw.githubusercontent.com/0x1d1e/kanade";

// a verified binary is at least this long; a short file is a failed download
const LEAST: u64 = 1024;

pub fn run() -> ExitCode {
    if std::env::consts::ARCH != "x86_64" {
        return fail(&format!(
            "failed: Kanade's install binary is x86_64 only, this machine is {}",
            std::env::consts::ARCH
        ));
    }

    let dest = match binary() {
        Ok(path) => path,
        Err(why) => return fail(&why),
    };
    let unit = match unit_path() {
        Ok(path) => path,
        Err(why) => return fail(&why),
    };

    let staging = dest.with_file_name(".kanade.update");
    let sum_path = dest.with_file_name(".kanade.update.sha256");
    let unit_staging = unit.with_file_name(".kanade.service.update");
    let _clean = Clean(vec![
        staging.clone(),
        sum_path.clone(),
        unit_staging.clone(),
    ]);

    if let Err(why) = fetch(&format!("{RELEASE}/kanade"), &staging) {
        return fail(&format!("{why}, the installed binary was not replaced"));
    }
    if let Err(why) = fetch(&format!("{RELEASE}/kanade.sha256"), &sum_path) {
        return fail(&format!("{why}, the installed binary was not replaced"));
    }
    if let Err(why) = verified(&staging, &sum_path) {
        return fail(&format!("{why}, the installed binary was not replaced"));
    }

    let rev = match fetch(&format!("{RAW}/install-0.1/kanade.rev"), &sum_path) {
        Ok(()) => fs::read_to_string(&sum_path).unwrap_or_default(),
        Err(_) => String::new(),
    };
    let tree = tree_of(&rev);

    if let Err(why) = fetch(&format!("{RAW}/{tree}/dist/kanade.service"), &unit_staging) {
        return fail(&format!("{why}, the installed binary was not replaced"));
    }
    let unit_bytes = match fs::read(&unit_staging) {
        Ok(bytes) if !bytes.is_empty() => bytes,
        _ => {
            return fail("failed: the unit file is short, the installed binary was not replaced");
        }
    };

    if let Err(why) = install_binary(&staging, &dest) {
        return fail(&format!(
            "failed: {why}, the installed binary was not replaced"
        ));
    }

    let mut lines = vec![format!("replaced {}", dest.display())];
    let rev = rev.trim();
    if tree != "release/0.1" {
        lines.push(format!("kanade {rev}"));
    }

    match install_unit(&unit, &unit_bytes) {
        Ok(Unit::Replaced(backup)) => {
            lines.push(format!(
                "replaced {}, the old one is {}",
                unit.display(),
                backup.display()
            ));
        }
        Ok(Unit::Unchanged) => lines.push("unit unchanged".to_owned()),
        Ok(Unit::Installed) => lines.push(format!("installed {}", unit.display())),
        Err(why) => {
            lines.push(format!("failed: {why}"));
            return cli::print(&format!("{}\n", lines.join("\n")), ExitCode::FAILURE);
        }
    }

    if let Err(why) = reload_unit() {
        lines.push(format!("failed: {why}"));
        return cli::print(&format!("{}\n", lines.join("\n")), ExitCode::FAILURE);
    }

    match restart_if_active() {
        Ok(true) => lines.push("restarted kanade.service".to_owned()),
        Ok(false) => lines.push("kanade.service is not running".to_owned()),
        Err(why) => {
            lines.push(format!("failed: {why}"));
            return cli::print(&format!("{}\n", lines.join("\n")), ExitCode::FAILURE);
        }
    }

    cli::print(&format!("{}\n", lines.join("\n")), ExitCode::SUCCESS)
}

fn fail(text: &str) -> ExitCode {
    eprintln!("kanade: {text}");
    ExitCode::FAILURE
}

fn binary() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "failed: HOME is not set".to_string())?;
    Ok(PathBuf::from(home).join(".cargo/bin/kanade"))
}

fn unit_path() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "failed: HOME is not set".to_string())?;
    config::base("XDG_CONFIG_HOME", ".config", Some(&home))
        .map(|dir| dir.join("systemd/user/kanade.service"))
        .ok_or_else(|| "failed: HOME is not set".to_string())
}

const PATIENCE: Duration = Duration::from_secs(60);
// a published binary, not a page; past this the download is not the asset
const MOST: u64 = 128 * 1024 * 1024;

fn agent() -> Agent {
    Agent::config_builder()
        .https_only(true)
        .max_redirects(3)
        .timeout_global(Some(PATIENCE))
        .http_status_as_error(false)
        .user_agent(concat!("Kanade/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

// the last HTTP status, or why the download failed; a 404 is a missing asset
fn fetch(url: &str, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("failed: {error}"))?;
    }

    let response = agent()
        .get(url)
        .call()
        .map_err(|error| format!("failed: {error}"))?;
    let status = response.status().as_u16();
    if status == 404 {
        let _ = fs::remove_file(dest);
        return Err("failed: the download is missing".to_owned());
    }
    if status != 200 {
        let _ = fs::remove_file(dest);
        return Err(format!("failed: the download failed (HTTP {status})"));
    }

    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(MOST.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed: {error}"))?;
    if bytes.is_empty() || bytes.len() as u64 > MOST {
        let _ = fs::remove_file(dest);
        return Err("failed: the download is short".to_owned());
    }

    fs::write(dest, &bytes).map_err(|error| format!("failed: {error}"))
}

fn verified(binary: &Path, sum: &Path) -> Result<(), String> {
    let len = fs::metadata(binary).map(|meta| meta.len()).unwrap_or(0);
    if len < LEAST {
        return Err("failed: the download is short".to_owned());
    }

    let text = fs::read_to_string(sum).map_err(|error| format!("failed: {error}"))?;
    let expected = checksum_of(&text).ok_or("failed: the checksum file is short")?;
    let got = digest(binary)?;
    if got != expected {
        return Err("failed: the checksum does not match".to_owned());
    }
    Ok(())
}

// the first word of a sha256 file, when it is a hash
fn checksum_of(text: &str) -> Option<String> {
    let hash = text.split_whitespace().next()?;
    if hash.len() == 64 && hash.chars().all(|char| char.is_ascii_hexdigit()) {
        Some(hash.to_ascii_lowercase())
    } else {
        None
    }
}

fn digest(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|error| format!("failed: {error}"))?;
    let hash = digest::digest(&digest::SHA256, &bytes);
    Ok(hash
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

// a hex rev names its tree; anything else, including a missing file, is the release branch
fn tree_of(rev: &str) -> &str {
    let rev = rev.trim();
    if rev.len() >= 2 && rev.chars().all(|char| char.is_ascii_hexdigit()) {
        rev
    } else {
        "release/0.1"
    }
}

fn install_binary(staging: &Path, dest: &Path) -> Result<(), String> {
    fs::set_permissions(staging, fs::Permissions::from_mode(0o755))
        .map_err(|error| error.to_string())?;
    fs::rename(staging, dest).map_err(|error| error.to_string())
}

enum Unit {
    Replaced(PathBuf),
    Unchanged,
    Installed,
}

fn install_unit(path: &Path, bytes: &[u8]) -> Result<Unit, String> {
    if fs::read(path).is_ok_and(|have| have == bytes) {
        return Ok(Unit::Unchanged);
    }

    let backup = if path.is_file() {
        let stamp = stamp();
        let backup = path.with_file_name(format!("kanade.service.bak.{stamp}"));
        fs::copy(path, &backup).map_err(|error| error.to_string())?;
        Some(backup)
    } else {
        None
    };

    atomic::write(path, bytes, Mode::Exactly(0o644)).map_err(|error| error.to_string())?;

    Ok(match backup {
        Some(backup) => Unit::Replaced(backup),
        None => Unit::Installed,
    })
}

fn stamp() -> String {
    let output = Command::new("date").arg("+%Y%m%d-%H%M%S").output();
    match output {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            let text = text.trim();
            if text.is_empty() {
                "now".to_owned()
            } else {
                text.to_owned()
            }
        }
        _ => "now".to_owned(),
    }
}

fn reload_unit() -> Result<(), String> {
    systemctl(&["--user", "daemon-reload"])
}

// true when the unit was running and was restarted; other `kanade` processes are left alone
fn restart_if_active() -> Result<bool, String> {
    let active = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", "kanade.service"])
        .status()
        .map_err(|error| format!("could not ask systemctl ({error})"))?;

    if !active.success() {
        return Ok(false);
    }

    systemctl(&["--user", "try-restart", "kanade.service"])?;
    Ok(true)
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let output = Command::new("systemctl")
        .args(args)
        .output()
        .map_err(|error| format!("could not ask systemctl ({error})"))?;
    if output.status.success() {
        return Ok(());
    }
    let why = String::from_utf8_lossy(&output.stderr);
    let why = why.trim();
    Err(if why.is_empty() {
        "systemctl failed".to_owned()
    } else {
        why.to_owned()
    })
}

// staging files removed however `run` returns, so a failed download leaves nothing in the bin
struct Clean(Vec<PathBuf>);

impl Drop for Clean {
    fn drop(&mut self) {
        for path in &self.0 {
            if path.exists() {
                let _ = fs::remove_file(path);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checksum_file_is_its_first_hash() {
        assert_eq!(
            checksum_of(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  kanade\n"
            )
            .as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(checksum_of("abc\n"), None);
        assert_eq!(checksum_of(""), None);
        assert_eq!(checksum_of("short\n"), None);
    }

    #[test]
    fn a_rev_names_its_tree_or_the_release() {
        assert_eq!(tree_of("abc123\n"), "abc123");
        assert_eq!(tree_of(""), "release/0.1");
        assert_eq!(tree_of("not a rev"), "release/0.1");
        assert_eq!(tree_of("g"), "release/0.1");
    }
}
