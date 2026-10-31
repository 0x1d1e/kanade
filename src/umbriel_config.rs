//! The Umbriel config editor's engine and CLI: `kanade umbriel-config`
//! reads, sets and removes single settings in Umbriel's `config.toml`, undoes the last change and
//! restores a backup. The file is the user's: it is changed only on request, by mutating TOML
//! preserving comments, formatting, and ordering, and only after validation accepted the result.

pub mod backup;
pub mod edit;
pub mod rules;

use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rustix::fs::Access;

use crate::atomic::{self, Mode};
use crate::config;
use backup::{Last, Ring};
pub use rules::{RuleInfo, RuleStatus};

/// What an option holds in the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    Own(String),
    Unset,
}

/// Default Umbriel configuration template.
pub const DEFAULT_CONFIG: &str = include_str!("../dist/umbriel/config.toml");

/// Installs the default Umbriel config to `dest` if missing.
pub fn create_default(dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create directory {}: {e}", parent.display()))?;
    }
    fs::write(dest, DEFAULT_CONFIG)
        .map_err(|e| format!("cannot write default config {}: {e}", dest.display()))?;
    Ok(())
}

/// Whether the `umbriel` binary is installed on PATH.
pub fn is_installed() -> bool {
    Command::new("umbriel")
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok()
}

/// How long `umbriel config validate` may take.
const VALIDATE: Duration = Duration::from_secs(10);

/// Checks a config file the way Umbriel will read it: why not, in one message.
pub type Validate<'a> = &'a dyn Fn(&Path) -> Result<(), String>;

/// Umbriel's config for this run: `--config` if given, else `$UMBRIEL_CONFIG`,
/// else `$XDG_CONFIG_HOME/umbriel/config.toml`, else `~/.config/umbriel/config.toml`.
pub fn target(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(file) = explicit {
        return Ok(file.to_owned());
    }

    if let Some(path) = env::var_os("UMBRIEL_CONFIG").filter(|p| !p.is_empty()) {
        return Ok(PathBuf::from(path));
    }

    let home = env::var("HOME").ok();
    config::base("XDG_CONFIG_HOME", ".config", home.as_deref())
        .map(|dir| dir.join("umbriel/config.toml"))
        .ok_or_else(|| String::from("HOME is not set"))
}

/// Where the backups are kept.
pub fn state() -> Result<PathBuf, String> {
    let home = env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade/umbriel-config"))
        .ok_or_else(|| String::from("neither XDG_STATE_HOME nor HOME is set"))
}

pub struct Editor<'a> {
    pub file: PathBuf,
    pub state: PathBuf,
    pub validate: Validate<'a>,
    pub now_ms: fn() -> u64,
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

struct Snapshot {
    modified: (i64, i64),
    len: u64,
    hash: String,
}

impl Snapshot {
    fn of(file: &Path) -> Result<(Snapshot, Vec<u8>), String> {
        let bytes = fs::read(file).map_err(|error| format!("{}: {error}", file.display()))?;
        let meta = fs::metadata(file).map_err(|error| format!("{}: {error}", file.display()))?;

        Ok((
            Snapshot {
                modified: (meta.mtime(), meta.mtime_nsec()),
                len: meta.len(),
                hash: hash(&bytes),
            },
            bytes,
        ))
    }
}

pub enum Plan {
    Same(String),
    Write(Vec<u8>, String),
}

fn hash(bytes: &[u8]) -> String {
    use std::fmt::Write;

    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);

    digest.as_ref().iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(ca), Ok(cb)) => ca == cb,
        _ => false,
    }
}

impl Editor<'_> {
    fn ring(&self) -> Ring {
        Ring::new(&self.state, &self.file)
    }

    /// Checks the config as it is, as `kanade umbriel-config validate`.
    pub fn check(&self) -> Result<String, String> {
        (self.validate)(&self.file).map(|()| String::from("valid"))
    }

    /// Retrieves the value at `path`.
    pub fn get(&self, path: &str) -> Result<String, String> {
        let text = fs::read_to_string(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        edit::get(&text, path)
    }

    /// As `get`, in a form to match on.
    pub fn read(&self, path: &str) -> Reading {
        match self.get(path) {
            Ok(val) => Reading::Own(val),
            Err(_) => Reading::Unset,
        }
    }

    /// Sets `path` to `value`.
    pub fn set(&self, path: &str, value: &str) -> Result<String, String> {
        self.change(true, |bytes| {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; Umbriel would not read it"))?;

            let was = edit::get(text, path).ok();

            match edit::set(text, path, value)? {
                None => Ok(Plan::Same(format!("{path} is {value} already"))),
                Some(edited) => {
                    let said = format!(
                        "{path} = {value} (was {})",
                        was.as_deref().unwrap_or("unset")
                    );
                    Ok(Plan::Write(edited.into_bytes(), said))
                }
            }
        })
    }

    /// Removes `path`.
    pub fn remove(&self, path: &str) -> Result<String, String> {
        self.change(true, |bytes| {
            let text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; Umbriel would not read it"))?;

            let was = edit::get(text, path).ok();

            match edit::remove(text, path)? {
                None => Ok(Plan::Same(format!("{path} is not set"))),
                Some(edited) => {
                    let said =
                        format!("{path} removed (was {})", was.as_deref().unwrap_or("unset"));
                    Ok(Plan::Write(edited.into_bytes(), said))
                }
            }
        })
    }

    /// Restores what the last change replaced.
    pub fn undo(&self) -> Result<String, String> {
        self.change(false, |bytes| {
            let ring = self.ring();
            let last = ring
                .last()
                .ok_or("nothing to undo: Kanade has not changed this config")?;

            if !same_file(&last.config, &self.file) {
                return Err(format!(
                    "the last change was to {}, not this config",
                    last.config.display()
                ));
            }
            if hash(bytes) != last.after {
                return Err(String::from(
                    "the config changed since Kanade's last change; undo would lose that. Pick a backup with `backups` and `restore`",
                ));
            }
            let (name, old) = ring.named(&last.before)?;
            Ok(Plan::Write(old, format!("undone: restored {name}")))
        })
    }

    /// Lists backups newest first.
    pub fn backups(&self) -> String {
        let ring = self.ring();
        let mut lines = Vec::new();

        for backup in ring.list() {
            let when =
                chrono::DateTime::<chrono::Utc>::from_timestamp_millis(backup.unix_ms as i64)
                    .map_or_else(
                        || backup.unix_ms.to_string(),
                        |when| when.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                    );

            lines.push(format!("{}\t{when}\t{} bytes", backup.number, backup.bytes));
        }

        if let Ok((_, bytes)) = ring.read("orig") {
            lines.push(format!(
                "orig\tthe first bytes Kanade saw\t{} bytes",
                bytes.len()
            ));
        }

        if lines.is_empty() {
            return String::from("no backups: Kanade has not changed this config");
        }

        lines.join("\n")
    }

    /// Restores a backup.
    pub fn restore(&self, which: &str) -> Result<String, String> {
        self.change(false, |bytes| {
            let (name, old) = self.ring().read(which)?;
            if bytes == old {
                return Ok(Plan::Same(format!("the config is {name} already")));
            }
            Ok(Plan::Write(old, format!("restored {name}")))
        })
    }

    /// Creates the default configuration if the file does not exist yet.
    pub fn create_default(&self) -> Result<(), String> {
        if self.file.exists() {
            return Err(format!("{}: file already exists", self.file.display()));
        }
        create_default(&self.file)
    }

    /// Several settings in one change, one backup and one `undo`: each path set to its value,
    /// or removed when none.
    pub fn apply(&self, changes: &[(&str, Option<&str>)]) -> Result<String, String> {
        self.change(true, |bytes| {
            let mut text = std::str::from_utf8(bytes)
                .map_err(|_| String::from("the config is not UTF-8; Umbriel would not read it"))?
                .to_owned();
            let mut said = Vec::new();
            let mut written = false;

            for &(path, value) in changes {
                let was = edit::get(&text, path).ok();
                match value {
                    Some(val) => match edit::set(&text, path, val)? {
                        None => {
                            if changes.len() == 1 {
                                said.push(format!("{path} is {val} already"));
                            }
                        }
                        Some(edited) => {
                            text = edited;
                            written = true;
                            said.push(format!(
                                "{path} = {val} (was {})",
                                was.as_deref().unwrap_or("unset")
                            ));
                        }
                    },
                    None => match edit::remove(&text, path)? {
                        None => {
                            if changes.len() == 1 {
                                said.push(format!("{path} is not set"));
                            }
                        }
                        Some(edited) => {
                            text = edited;
                            written = true;
                            said.push(format!(
                                "{path} removed (was {})",
                                was.as_deref().unwrap_or("unset")
                            ));
                        }
                    },
                }
            }

            let said = if said.is_empty() {
                String::from("nothing changed")
            } else {
                said.join("; ")
            };

            Ok(if written {
                Plan::Write(text.into_bytes(), said)
            } else {
                Plan::Same(said)
            })
        })
    }

    /// Undoes only if the current file hash matches `after`.
    pub fn undo_only(&self, after: Option<&str>) -> Result<String, String> {
        self.change(false, |bytes| {
            let ring = self.ring();
            let last = ring
                .last()
                .ok_or("nothing to undo: Kanade has not changed this config")?;

            if !same_file(&last.config, &self.file) {
                return Err(format!(
                    "the last change was to {}, not this config",
                    last.config.display()
                ));
            }
            if let Some(expected) = after {
                if hash(bytes) != expected || last.after != expected {
                    return Err(String::from(
                        "the config changed since Kanade's last change; undo would lose that. Pick a backup with `backups` and `restore`",
                    ));
                }
            } else if hash(bytes) != last.after {
                return Err(String::from(
                    "the config changed since Kanade's last change; undo would lose that. Pick a backup with `backups` and `restore`",
                ));
            }
            let (name, old) = ring.named(&last.before)?;
            Ok(Plan::Write(old, format!("undone: restored {name}")))
        })
    }

    /// Hash of the file after the last change.
    pub fn last_change_hash(&self) -> Result<String, String> {
        let ring = self.ring();
        let last = ring.last().ok_or("no last change recorded")?;
        Ok(last.after)
    }

    #[allow(dead_code)]
    pub fn editable(&self) -> Result<(), String> {
        self.writable().map(|_| ())
    }

    pub(crate) fn change(
        &self,
        validate_original: bool,
        plan: impl FnOnce(&[u8]) -> Result<Plan, String>,
    ) -> Result<String, String> {
        self.writable()?;
        let _lock = self.lock()?;
        let symlink_meta = fs::symlink_metadata(&self.file)
            .map_err(|error| format!("{}: {error}", self.file.display()))?;
        let is_symlink = symlink_meta.file_type().is_symlink();
        let target = if is_symlink {
            self.file
                .canonicalize()
                .map_err(|error| format!("{}: {error}", self.file.display()))?
        } else {
            self.file.clone()
        };
        let meta =
            fs::metadata(&target).map_err(|error| format!("{}: {error}", target.display()))?;
        let (snapshot, old) = Snapshot::of(&target)?;

        if validate_original {
            (self.validate)(&self.file).map_err(|why| {
                format!("Umbriel rejects the config as it is, so nothing was changed: {why}")
            })?;
        }

        let (new, said) = match plan(&old)? {
            Plan::Same(said) => return Ok(said),
            Plan::Write(new, said) => (new, said),
        };

        let staged = atomic::stage(
            &target,
            &new,
            Mode::Exactly(meta.permissions().mode() & 0o7777),
        )
        .map_err(|error| {
            format!(
                "cannot stage the change beside {}: {error}",
                target.display()
            )
        })?;

        (self.validate)(staged.path()).map_err(|why| {
            let part = staged
                .path()
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let file = self
                .file
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            format!(
                "Umbriel rejects the change, so nothing was changed: {}",
                why.replace(&part, &file)
            )
        })?;

        let ring = self.ring();
        let before = ring
            .stage(&old, (self.now_ms)())
            .map_err(|error| format!("cannot keep a backup, so nothing was changed: {error}"))?;

        // Read again right before committing to detect disk changes
        let (now, _) = Snapshot::of(&target)?;

        if now.hash != snapshot.hash || now.len != snapshot.len || now.modified != snapshot.modified
        {
            ring.discard(&before);
            return Err(String::from(
                "config changed on disk since it was read; nothing was written, try again",
            ));
        }

        if is_symlink && self.file.canonicalize().ok().as_ref() != Some(&target) {
            ring.discard(&before);
            return Err(String::from(
                "config symlink changed on disk since it was read; nothing was written, try again",
            ));
        }

        let after = hash(&new);

        if let Err(error) = staged.commit(&target) {
            ring.discard(&before);
            return Err(format!("cannot replace {}: {error}", target.display()));
        }

        ring.prune();

        let recorded = ring.record(&Last {
            config: target,
            after,
            before,
        });

        Ok(match recorded {
            Ok(()) => format!("{said}; backup 1, `kanade umbriel-config undo` reverts"),
            Err(error) => format!(
                "{said}; the change was written but could not be recorded for undo: {error}"
            ),
        })
    }

    fn lock(&self) -> Result<File, String> {
        let canonical = self
            .file
            .canonicalize()
            .or_else(|_| {
                let absolute = std::path::absolute(&self.file)?;
                let parent = absolute
                    .parent()
                    .ok_or_else(|| std::io::Error::other("no parent directory"))?;
                Ok::<PathBuf, std::io::Error>(
                    parent
                        .canonicalize()?
                        .join(absolute.file_name().unwrap_or_default()),
                )
            })
            .map_err(|error| {
                format!(
                    "{}: cannot resolve config path for its lock: {error}",
                    self.file.display()
                )
            })?;
        let digest = ring::digest::digest(
            &ring::digest::SHA256,
            canonical.as_os_str().as_encoded_bytes(),
        );
        let key = digest.as_ref()[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        fs::create_dir_all(&self.state)
            .map_err(|error| format!("{}: {error}", self.state.display()))?;
        fs::set_permissions(&self.state, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("{}: {error}", self.state.display()))?;
        let path = self.state.join(format!("config.{key}.lock"));
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(&path)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        file.lock()
            .map_err(|error| format!("{}: {error}", path.display()))?;
        Ok(file)
    }

    fn writable(&self) -> Result<fs::Metadata, String> {
        let shown = self.file.display();

        if self.file.starts_with("/etc") {
            return Err(format!(
                "{shown} is the system config, which Kanade does not write. Make your own: \
                 mkdir -p ~/.config/umbriel && cp {shown} ~/.config/umbriel/config.toml"
            ));
        }

        let meta = fs::symlink_metadata(&self.file).map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!(
                "{shown}: no such file; Umbriel runs on its default config, which Kanade does not create"
            ),
            _ => format!("{shown}: {error}"),
        })?;

        if meta.file_type().is_symlink() {
            if meta.uid() != rustix::process::geteuid().as_raw() {
                return Err(format!("{shown} is not owned by you"));
            }

            let canonical = self.file.canonicalize().map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    format!("{shown}: dangling symbolic link")
                } else if error.raw_os_error() == Some(rustix::io::Errno::LOOP.raw_os_error()) {
                    format!("{shown}: symbolic link loop")
                } else {
                    format!("{shown}: {error}")
                }
            })?;

            if canonical.starts_with("/etc") {
                return Err(format!(
                    "{shown} points to {}, which is under /etc and not edited by Kanade",
                    canonical.display()
                ));
            }

            let canonical_meta = fs::metadata(&canonical)
                .map_err(|error| format!("{}: {error}", canonical.display()))?;

            if !canonical_meta.is_file() {
                return Err(format!("{} is not a regular file", canonical.display()));
            }

            if canonical_meta.nlink() > 1 {
                return Err(format!(
                    "{} is a hard link (another name for the same file); Kanade edits only a file with one name in this version",
                    canonical.display()
                ));
            }

            if canonical_meta.uid() != rustix::process::geteuid().as_raw() {
                return Err(format!("{} is not owned by you", canonical.display()));
            }

            let dir = canonical
                .parent()
                .filter(|dir| !dir.as_os_str().is_empty())
                .unwrap_or(Path::new("."));

            if rustix::fs::access(&canonical, Access::WRITE_OK).is_err()
                || rustix::fs::access(dir, Access::WRITE_OK).is_err()
            {
                return Err(format!(
                    "{} or its directory is not writable",
                    canonical.display()
                ));
            }

            return Ok(canonical_meta);
        }

        if !meta.is_file() {
            return Err(format!("{shown} is not a regular file"));
        }

        if meta.nlink() > 1 {
            return Err(format!(
                "{shown} is a hard link (another name for the same file); Kanade edits only a file with one name in this version"
            ));
        }

        if meta.uid() != rustix::process::geteuid().as_raw() {
            return Err(format!("{shown} is not owned by you"));
        }

        let dir = self
            .file
            .parent()
            .filter(|dir| !dir.as_os_str().is_empty())
            .unwrap_or(Path::new("."));

        if rustix::fs::access(&self.file, Access::WRITE_OK).is_err()
            || rustix::fs::access(dir, Access::WRITE_OK).is_err()
        {
            return Err(format!("{shown} or its directory is not writable"));
        }

        Ok(meta)
    }
}

const MOST_OUTPUT: u64 = 1 << 20;

fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(pipe) = pipe {
            let _ = pipe.take(MOST_OUTPUT).read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Fallback TOML syntax validator when umbriel CLI is not available.
pub fn fallback_syntax_validate(file: &Path) -> Result<(), String> {
    let content = fs::read_to_string(file).map_err(|e| format!("{}: {e}", file.display()))?;
    content
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| format!("syntax error: {e}"))?;
    Ok(())
}

/// Validates an Umbriel config file using `umbriel config validate -c <staged>`
/// when available, or fallback syntax validation otherwise.
pub fn validate_umbriel(file: &Path) -> Result<(), String> {
    let mut cmd = Command::new("umbriel");
    cmd.args(["config", "validate", "-c"])
        .arg(file.as_os_str())
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return fallback_syntax_validate(file);
        }
        Err(err) => {
            return Err(format!("cannot run umbriel config validate ({err})"));
        }
    };

    let out = drain(child.stdout.take());
    let err = drain(child.stderr.take());

    let until = Instant::now() + VALIDATE;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < until => thread::sleep(Duration::from_millis(5)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(String::from(
                    "umbriel config validate did not finish in 10 s",
                ));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("umbriel config validate: {error}"));
            }
        }
    };

    let output = format!(
        "{}\n{}",
        err.join().unwrap_or_default(),
        out.join().unwrap_or_default()
    );

    if status.success() && !output.contains("Error:") && !output.contains("error:") {
        Ok(())
    } else {
        let first_err = output
            .lines()
            .map(str::trim)
            .find(|l| {
                !l.is_empty()
                    && (l.starts_with("warning:")
                        || l.starts_with("error:")
                        || l.contains("invalid")
                        || l.contains("Error")
                        || l.contains("error"))
            })
            .or_else(|| output.lines().map(str::trim).find(|l| !l.is_empty()))
            .unwrap_or("validation failed");
        Err(first_err.to_string())
    }
}

#[cfg(test)]
mod tests {
    use std::process;

    use super::*;

    fn test_dir(name: &str) -> (PathBuf, PathBuf) {
        let base = env::temp_dir().join(format!("kanade-umbriel-test-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&base);
        let config_dir = base.join("config");
        let state_dir = base.join("state");
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&state_dir).unwrap();
        (config_dir, state_dir)
    }

    #[test]
    fn editor_get_set_remove_undo_flow() {
        let (config_dir, state_dir) = test_dir("flow");
        let config_file = config_dir.join("config.toml");
        let initial = r#"
[layout]
mode = "scrolling"
gap = 8
"#;
        fs::write(&config_file, initial).unwrap();

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        // Validate
        assert!(editor.editable().is_ok());
        assert_eq!(editor.check().unwrap(), "valid");

        // Get
        assert_eq!(editor.get("layout.gap").unwrap(), "8");

        // Set
        let set_res = editor.set("layout.gap", "12").unwrap();
        assert!(set_res.contains("layout.gap = 12"));
        assert_eq!(editor.get("layout.gap").unwrap(), "12");

        // Check backups
        let backups = editor.backups();
        assert!(backups.contains("orig"));
        assert!(backups.contains("1\t"));

        // Remove
        let rem_res = editor.remove("layout.gap").unwrap();
        assert!(rem_res.contains("layout.gap removed"));
        assert!(editor.get("layout.gap").is_err());

        // Undo
        let undo_res = editor.undo().unwrap();
        assert!(undo_res.contains("undone"));
        assert_eq!(editor.get("layout.gap").unwrap(), "12");
    }

    #[test]
    fn editor_apply_batch_and_undo_only() {
        let (config_dir, state_dir) = test_dir("apply_batch");
        let config_file = config_dir.join("config.toml");
        let initial = r#"
[layout]
mode = "scrolling"
gap = 8
"#;
        fs::write(&config_file, initial).unwrap();

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        let changes = [
            ("layout.gap", Some("16")),
            ("appearance.border_width", Some("3")),
        ];
        let res = editor.apply(&changes).unwrap();
        assert!(res.contains("layout.gap = 16"));
        assert!(res.contains("appearance.border_width = 3"));
        assert_eq!(editor.read("layout.gap"), Reading::Own(String::from("16")));
        assert_eq!(
            editor.read("appearance.border_width"),
            Reading::Own(String::from("3"))
        );

        let after_hash = editor.last_change_hash().unwrap();
        let undo_res = editor.undo_only(Some(&after_hash)).unwrap();
        assert!(undo_res.contains("undone"));
        assert_eq!(editor.read("layout.gap"), Reading::Own(String::from("8")));
        assert_eq!(editor.read("appearance.border_width"), Reading::Unset);
    }

    #[test]
    fn editor_create_default_when_missing() {
        let (config_dir, state_dir) = test_dir("create_default");
        let config_file = config_dir.join("config.toml");
        assert!(!config_file.exists());

        let editor = Editor {
            file: config_file.clone(),
            state: state_dir,
            validate: &fallback_syntax_validate,
            now_ms,
        };

        assert!(editor.create_default().is_ok());
        assert!(config_file.exists());
        assert_eq!(
            editor.read("layout.mode"),
            Reading::Own(String::from("\"scrolling\""))
        );

        // Cannot recreate if already present
        assert!(editor.create_default().is_err());
    }

    #[test]
    fn fallback_validation_rejects_malformed_toml() {
        let (config_dir, _) = test_dir("malformed");
        let config_file = config_dir.join("config.toml");
        fs::write(&config_file, "this is not valid toml = [[[").unwrap();
        assert!(fallback_syntax_validate(&config_file).is_err());
    }
}
