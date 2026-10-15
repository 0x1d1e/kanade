//! Settings' Health page and the first-start notice (ADR 0088): `kanade doctor`'s checks, run
//! inside the shell off the draw thread and shown read-only. Nothing here fixes anything; each
//! check says what is missing and doctor's detail lines say what to run.

use std::thread;
use std::time::Duration;

use kanade_runtime::service::Service;

use crate::atomic::{self, Mode};
use crate::config;
use crate::doctor::{self, Check, Verdict};
use crate::sources::live;

use super::Settings;

// the page's name
pub const PAGE: &str = "health";

// how long after the start the notice waits, so the shell has taken its names and the session its
// services, and the checks do not compete with the start for the disk and the buses
const SETTLE: Duration = Duration::from_secs(30);

// the checks that warn or fail
pub fn needing(checks: &[Check]) -> usize {
    checks
        .iter()
        .filter(|check| check.verdict != Verdict::Ok)
        .count()
}

/*
 * runs the checks on a worker, if none runs, and puts them in the Service. A failure to start one
 * is shown as a failed check rather than an empty page
 */
pub fn load() {
    {
        let mut settings = Settings::write();
        if settings.health_loading {
            return;
        }
        settings.health_loading = true;
    }

    let worker = thread::Builder::new()
        .name(String::from("kanade-health"))
        .spawn(|| finish(doctor::in_shell()));

    if let Err(error) = worker {
        finish(vec![Check {
            verdict: Verdict::Fail,
            text: format!("could not start the checks: {error}"),
            detail: Vec::new(),
        }]);
    }
}

fn finish(checks: Vec<Check>) {
    let mut settings = Settings::write();
    settings.health = checks;
    settings.health_loading = false;
    settings.health_loaded = true;
}

/*
 * the first start: once the session settles, runs the checks and, if any needs attention, tells so
 * on the island. Recorded before the checks run, so it is told once however they end, and a state
 * directory that cannot be written tells nothing rather than at every start
 */
pub fn spawn_notice() {
    if !config::get().health_notice {
        return;
    }

    let _ = thread::Builder::new()
        .name(String::from("kanade-health-notice"))
        .spawn(|| {
            thread::sleep(SETTLE);
            notice();
        });
}

fn notice() {
    let Some(marker) = marker() else {
        return;
    };
    if marker.exists() {
        return;
    }

    let recorded = atomic::write(&marker, env!("CARGO_PKG_VERSION").as_bytes(), Mode::Private);
    if recorded.is_err() {
        return;
    }

    let waiting = needing(&doctor::in_shell());
    if waiting > 0 {
        live::health_attention(waiting);
    }
}

fn marker() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade").join("health-notice"))
}
