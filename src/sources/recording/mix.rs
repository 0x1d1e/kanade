//! A simultaneous system and microphone mix for recording (ADR 0116 update).
//!
//! PulseAudio / PipeWire is asked for a virtual null sink (`kanade-record-mix`) and two loopback
//! modules routing the default output monitor and default input source into it. While recording,
//! the modules are tracked in a runtime ledger `pulse-mix.<pid>`. On stop or drop, the modules
//! are unloaded and the ledger removed. On startup, stale ledgers and lingering mix modules
//! are reclaimed.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process;
use std::thread;
use std::time::Duration;

use super::super::wake;
use crate::atomic::{self, Mode};

const SINK_NAME: &str = "kanade-record-mix";
const SINK_DESC: &str = "device.description=KanadeRecordMix";
const PACTL: &str = "pactl";
const LIMIT: Duration = Duration::from_secs(2);
const LEDGER_PREFIX: &str = "pulse-mix";

// manages loaded virtual null-sink and loopbacks for a recording session
#[derive(Debug)]
pub struct MixSession {
    modules: Vec<String>,
    ledger: Option<PathBuf>,
}

impl MixSession {
    pub fn create() -> Result<MixSession, String> {
        let mut loaded = Vec::new();

        let steps: [&[&str]; 3] = [
            &[
                "module-null-sink",
                &format!("sink_name={SINK_NAME}"),
                &format!("sink_properties={SINK_DESC}"),
            ],
            &[
                "module-loopback",
                "source=@DEFAULT_MONITOR@",
                &format!("sink={SINK_NAME}"),
                "latency_msec=20",
            ],
            &[
                "module-loopback",
                "source=@DEFAULT_SOURCE@",
                &format!("sink={SINK_NAME}"),
                "latency_msec=20",
            ],
        ];

        for step in steps {
            match load_module(step) {
                Ok(id) => loaded.push(id),
                Err(why) => {
                    unload_all(&loaded);
                    return Err(why);
                }
            }
        }

        let ledger = write_ledger(&loaded);

        Ok(MixSession {
            modules: loaded,
            ledger,
        })
    }
}

impl Drop for MixSession {
    fn drop(&mut self) {
        let modules = std::mem::take(&mut self.modules);
        let ledger = self.ledger.take();

        if modules.is_empty() && ledger.is_none() {
            return;
        }

        let cleanup_modules = modules.clone();
        let cleanup_ledger = ledger.clone();

        let spawned = thread::Builder::new()
            .name(String::from("pulse-mix-drop"))
            .spawn(move || {
                unload_all(&cleanup_modules);
                if let Some(path) = cleanup_ledger {
                    let _ = fs::remove_file(path);
                }
            });

        if let Err(error) = spawned {
            eprintln!("capture: cannot spawn mix cleanup thread: {error}");
            unload_all(&modules);
            if let Some(path) = ledger {
                let _ = fs::remove_file(path);
            }
        }
    }
}

fn load_module(args: &[&str]) -> Result<String, String> {
    let mut command_args = Vec::with_capacity(args.len() + 1);
    command_args.push("load-module");
    command_args.extend_from_slice(args);

    let output =
        wake::query_within(PACTL, &command_args, LIMIT).map_err(|failed| match failed {
            wake::Failed::Overran(d) => format!("{PACTL} timed out after {d:?}"),
            wake::Failed::Said(why) => why,
        })?;

    let id = output.trim();
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_digit()) {
        return Err(format!("{PACTL} returned unexpected module id: {output:?}"));
    }

    Ok(id.to_owned())
}

fn unload_module(id: &str) {
    if let Err(error) = wake::act_within(PACTL, &["unload-module", id], LIMIT) {
        eprintln!("capture: cannot unload mix module {id}: {error:?}");
    }
}

fn unload_all(modules: &[String]) {
    for id in modules.iter().rev() {
        unload_module(id);
    }
}

fn ledger_dir() -> Option<PathBuf> {
    let runtime = env::var_os("XDG_RUNTIME_DIR").filter(|dir| !dir.is_empty())?;
    Some(PathBuf::from(runtime).join("kanade"))
}

fn ledger_file() -> Option<PathBuf> {
    Some(ledger_dir()?.join(format!("{LEDGER_PREFIX}.{}", process::id())))
}

fn write_ledger(modules: &[String]) -> Option<PathBuf> {
    let file = ledger_file()?;
    let text = serialize_ledger(modules);
    if let Err(error) = atomic::write(&file, text.as_bytes(), Mode::Private) {
        eprintln!(
            "capture: cannot write pulse mix ledger {}: {error}",
            file.display()
        );
        None
    } else {
        Some(file)
    }
}

// reclaims stale ledgers and any lingering mix sink modules
pub fn reclaim() {
    let spawned = thread::Builder::new()
        .name(String::from("pulse-mix-reclaim"))
        .spawn(|| {
            reclaim_stale();
            reclaim_lingering();
        });

    if let Err(error) = spawned {
        eprintln!("capture: cannot spawn mix reclaim thread: {error}");
    }
}

fn reclaim_stale() {
    let Some(dir) = ledger_dir() else {
        return;
    };
    let stales = stale_ledgers(&dir, process::id(), super::window::alive);
    for (path, pid) in stales {
        eprintln!("capture: reclaiming audio mix ledger for dead pid {pid}");
        if let Ok(text) = fs::read_to_string(&path) {
            let ids = parse_ledger(&text);
            unload_all(&ids);
        }
        let _ = fs::remove_file(&path);
    }
}

fn reclaim_lingering() {
    let Ok(output) = wake::query_within(PACTL, &["list", "short", "modules"], LIMIT) else {
        return;
    };
    let lingering = extract_lingering_modules(&output, SINK_NAME);
    if !lingering.is_empty() {
        eprintln!(
            "capture: reclaiming {} lingering mix modules",
            lingering.len()
        );
        unload_all(&lingering);
    }
}

pub fn parse_ledger_pid(name: &str) -> Option<u32> {
    name.strip_prefix(LEDGER_PREFIX)
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|pid| pid.parse::<u32>().ok())
}

pub fn parse_ledger(content: &str) -> Vec<String> {
    content
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(String::from)
        .collect()
}

pub fn serialize_ledger(ids: &[String]) -> String {
    let mut out = ids.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out
}

pub fn extract_lingering_modules(list_output: &str, sink_name: &str) -> Vec<String> {
    list_output
        .lines()
        .filter(|line| line.contains(sink_name))
        .filter_map(|line| {
            let first = line.split_whitespace().next()?;
            if !first.is_empty() && first.chars().all(|c| c.is_ascii_digit()) {
                Some(first.to_string())
            } else {
                None
            }
        })
        .collect()
}

pub fn stale_ledgers(dir: &Path, own: u32, alive: impl Fn(u32) -> bool) -> Vec<(PathBuf, u32)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if let Some(pid) = parse_ledger_pid(name).filter(|&pid| pid == own || !alive(pid)) {
            found.push((entry.path(), pid));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ledger_round_trip() {
        let empty: Vec<String> = Vec::new();
        assert_eq!(serialize_ledger(&empty), "");
        assert_eq!(parse_ledger(""), empty);

        let ids = vec![String::from("536870916")];
        assert_eq!(serialize_ledger(&ids), "536870916\n");
        assert_eq!(parse_ledger("536870916\n"), ids);

        let multi = vec![
            String::from("101"),
            String::from("102"),
            String::from("103"),
        ];
        let serialized = serialize_ledger(&multi);
        assert_eq!(serialized, "101\n102\n103\n");
        assert_eq!(parse_ledger(&serialized), multi);

        let messy = "  101  \n\n  102\n 103 \n";
        assert_eq!(parse_ledger(messy), multi);
    }

    #[test]
    fn parse_pid_from_filenames() {
        assert_eq!(parse_ledger_pid("pulse-mix.1234"), Some(1234));
        assert_eq!(parse_ledger_pid("pulse-mix.0"), Some(0));
        assert_eq!(parse_ledger_pid("pulse-mix.abc"), None);
        assert_eq!(parse_ledger_pid("pulse-mix."), None);
        assert_eq!(parse_ledger_pid("pulse-mix"), None);
        assert_eq!(parse_ledger_pid("window-casts.1234"), None);
        assert_eq!(parse_ledger_pid("other.1234"), None);
    }

    #[test]
    fn extract_lingering_module_ids() {
        let sample = "\
1\tlibpipewire-module-rt\t{}\n\
536870916\tmodule-null-sink\tsink_name=kanade-record-mix sink_properties=device.description=KanadeRecordMix\t\n\
536870917\tmodule-loopback\tsource=@DEFAULT_SOURCE@ sink=kanade-record-mix latency_msec=20\t\n\
536870918\tmodule-loopback\tsource=@DEFAULT_MONITOR@ sink=kanade-record-mix latency_msec=20\t\n\
536870919\tmodule-device-manager\t\n";

        let extracted = extract_lingering_modules(sample, "kanade-record-mix");
        assert_eq!(extracted, vec!["536870916", "536870917", "536870918"]);

        let clean = "1\tlibpipewire-module-rt\t{}\n2\tlibpipewire-module-protocol-native\t{}\n";
        assert!(extract_lingering_modules(clean, "kanade-record-mix").is_empty());
    }

    #[test]
    fn stale_ledgers_detects_dead_and_own_pids() {
        let temp = env::temp_dir().join(format!("kanade-mix-test-{}", process::id()));
        let _ = fs::create_dir_all(&temp);

        let f_own = temp.join("pulse-mix.100");
        let f_alive = temp.join("pulse-mix.200");
        let f_dead = temp.join("pulse-mix.300");
        let f_other = temp.join("window-casts.300");

        let _ = fs::write(&f_own, "1\n");
        let _ = fs::write(&f_alive, "2\n");
        let _ = fs::write(&f_dead, "3\n");
        let _ = fs::write(&f_other, "4\n");

        let stales = stale_ledgers(&temp, 100, |pid| pid == 200);

        let paths: Vec<PathBuf> = stales.into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec![f_own.clone(), f_dead.clone()]);

        let _ = fs::remove_file(f_own);
        let _ = fs::remove_file(f_alive);
        let _ = fs::remove_file(f_dead);
        let _ = fs::remove_file(f_other);
        let _ = fs::remove_dir(temp);
    }
}
