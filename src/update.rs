//! `kanade update` builds main with Cargo, then refreshes the user unit.
//! A failed build leaves the installed binary as it was. The unit always runs
//! `$HOME/.cargo/bin/kanade`, independent of `CARGO_HOME`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::atomic::{self, Mode};
use crate::cli;
use crate::config;

pub fn run() -> ExitCode {
    let dest = match binary() {
        Ok(path) => path,
        Err(why) => return fail(&why),
    };
    let unit = match unit_path() {
        Ok(path) => path,
        Err(why) => return fail(&why),
    };
    // Cargo builds in its own checkout and installs only after compilation succeeds.
    let root = dest
        .parent()
        .and_then(Path::parent)
        .expect("binary under .cargo/bin");
    let status = Command::new("cargo")
        .args([
            "install",
            "--locked",
            "--git",
            "https://github.com/0x1d1e/kanade",
            "--branch",
            "main",
            "--root",
        ])
        .arg(root)
        .args(["--force", "kanade"])
        .status();
    match status {
        Ok(status) if status.success() => {}
        Ok(_) => return fail("Cargo build failed"),
        Err(why) => return fail(&format!("could not start cargo: {why}")),
    }
    let unit_bytes = include_bytes!("../dist/kanade.service");
    let mut lines = vec![format!("built and installed {}", dest.display())];

    match install_unit(&unit, unit_bytes) {
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

    // the unit file only. A drop-in from `systemctl --user edit` lives in kanade.service.d and
    // is not part of this write
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_changed_unit_is_kept_and_a_drop_in_is_left_alone() {
        let dir =
            std::env::temp_dir().join(format!("kanade-unit-{}-{}", std::process::id(), stamp()));
        let _clean = DirClean(dir.clone());
        fs::create_dir_all(&dir).unwrap();

        let unit = dir.join("kanade.service");
        fs::write(&unit, b"old\n").unwrap();
        let drop_in = dir.join("kanade.service.d/override.conf");
        fs::create_dir_all(drop_in.parent().unwrap()).unwrap();
        fs::write(&drop_in, b"[Service]\n").unwrap();

        assert!(matches!(
            install_unit(&unit, b"old\n").unwrap(),
            Unit::Unchanged
        ));
        assert!(!dir_has_backup(&dir));

        let Unit::Replaced(backup) = install_unit(&unit, b"new\n").unwrap() else {
            panic!("a differing unit was not kept");
        };
        assert_eq!(fs::read(&backup).unwrap(), b"old\n");
        assert_eq!(fs::read(&unit).unwrap(), b"new\n");
        assert_eq!(fs::read(&drop_in).unwrap(), b"[Service]\n");
        assert!(backup.starts_with(&dir));
        assert!(!backup.starts_with(dir.join("kanade.service.d")));
    }

    fn dir_has_backup(dir: &Path) -> bool {
        fs::read_dir(dir).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".bak.")
        })
    }

    struct DirClean(PathBuf);

    impl Drop for DirClean {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}
