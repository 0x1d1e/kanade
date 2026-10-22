//! The desktop portals that draw GTK dialogs (`xdg-desktop-portal-gnome` and
//! `xdg-desktop-portal-gtk`) are long-running processes that read `settings.ini` and `gtk.css` once,
//! so they keep the old theme after a sync until restarted. Both implement the file chooser and the
//! app chooser; `portals.conf` picks which draws a dialog, so both are restarted. On a desktop it
//! prefers, `xdg-desktop-portal-gnome` also serves the screen cast and remote desktop, so restarting
//! it drops an in-flight cast or remote-desktop session; the GTK backend only closes open dialogs.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::Builder;
use std::time::{Duration, Instant};

use kanade_runtime::service::Service;

use super::config_home;

const DIRS: [&str; 2] = ["gtk-3.0", "gtk-4.0"];

pub(super) const GTK_UNIT: &str = "xdg-desktop-portal-gtk.service";
pub(super) const GNOME_UNIT: &str = "xdg-desktop-portal-gnome.service";

// rapid wallpaper changes restart them at most this often; the last one still lands
const MIN_INTERVAL: Duration = Duration::from_secs(30);

// systemctl's exit code for a unit that is not installed, which is no failure for us
const NOT_FOUND: i32 = 5;

// what the GTK side of a sync writes, per dir: the settings file and the generated and user css
pub(super) type Look = [[Option<String>; 3]; 2];

pub(super) fn look() -> Look {
    DIRS.map(|dir| {
        let root = config_home().join(dir);
        ["settings.ini", "kanade.css", "gtk.css"].map(|name| read(&root.join(name)))
    })
}

fn read(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

// when to restart the portal: at once for the first change, then no more often than `MIN_INTERVAL`,
// once for all the changes since
#[derive(Debug, Default)]
pub(super) struct Restart {
    last: Option<Instant>,
    due: Option<Instant>,
}

impl Restart {
    // the GTK look changed
    pub(super) fn changed(&mut self, now: Instant) {
        if self.due.is_none() {
            self.due = Some(match self.last {
                Some(last) => (last + MIN_INTERVAL).max(now),
                None => now,
            });
        }
    }

    // whether to restart now
    pub(super) fn take_due(&mut self, now: Instant) -> bool {
        if self.due.is_some_and(|due| due <= now) {
            self.due = None;
            self.last = Some(now);
            return true;
        }
        false
    }

    // how long until a restart is due
    pub(super) fn wait(&self, now: Instant) -> Option<Duration> {
        self.due.map(|due| due.saturating_duration_since(now))
    }

    // record that a restart occurred, ensuring no further restart runs within MIN_INTERVAL
    pub(super) fn record_restart(&mut self, now: Instant) {
        self.last = Some(now);
        if let Some(due) = self.due {
            self.due = Some(due.max(now + MIN_INTERVAL));
        }
    }
}

pub(super) fn can_restart_gnome() -> bool {
    let niri = crate::sources::niri::Niri::read();
    niri.is_connected() && niri.seen.casts.is_empty() && niri.seen.sessions.is_empty()
}

pub(super) fn has_active_casts() -> bool {
    !can_restart_gnome()
}

pub(super) fn restart_gtk() {
    restart_unit(GTK_UNIT);
}

pub(super) fn restart_gnome() -> bool {
    if !can_restart_gnome() {
        return false;
    }
    restart_unit(GNOME_UNIT);
    true
}

fn restart_unit(unit: &'static str) {
    let spawned = Command::new("systemctl")
        .args(["--user", "--no-block", "try-restart", unit])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            eprintln!("kanade: could not restart {unit}: {error}");
            return;
        }
    };
    let reaper = Builder::new()
        .name(String::from("kanade-portal-restart"))
        .spawn(move || match child.wait() {
            Ok(status) if status.success() || status.code() == Some(NOT_FOUND) => {}
            Ok(status) => eprintln!("kanade: restarting {unit} failed: {status}"),
            Err(error) => eprintln!("kanade: could not wait for systemctl: {error}"),
        });
    if let Err(error) = reaper {
        eprintln!("kanade: could not watch the {unit} restart: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restarting_gnome_is_suppressed_when_casts_are_active() {
        use crate::sources::niri::{Niri, Target};

        Niri::write().connected = false;
        assert!(has_active_casts());
        assert!(!can_restart_gnome());
        assert!(!restart_gnome());

        Niri::write().connected = true;
        Niri::write().seen.casts.clear();
        Niri::write().seen.sessions.clear();
        assert!(!has_active_casts());
        assert!(can_restart_gnome());

        Niri::write().seen.casts.insert(Target::Screen);
        assert!(has_active_casts());
        assert!(!can_restart_gnome());
        assert!(
            !restart_gnome(),
            "active cast suppresses gnome portal restart"
        );

        Niri::write().seen.casts.clear();
        Niri::write().seen.sessions.insert(42);
        assert!(has_active_casts());
        assert!(!can_restart_gnome());
        assert!(
            !restart_gnome(),
            "active session suppresses gnome portal restart"
        );

        Niri::write().seen.sessions.clear();
        assert!(!has_active_casts());
        assert!(can_restart_gnome());
    }

    #[test]
    fn restart_records_throttle_interval() {
        let mut restart = Restart::default();
        let now = Instant::now();
        restart.changed(now);
        assert!(restart.take_due(now));

        // immediately after, another change arrives
        let t1 = now + Duration::from_secs(1);
        restart.changed(t1);
        assert!(!restart.take_due(t1));

        // record_restart flushes deferred gnome restart at t = 5s
        let t5 = now + Duration::from_secs(5);
        restart.record_restart(t5);

        // at t = 31s (which would have been 30s after `now`), it is NOT due yet because record_restart postponed it
        let t31 = now + Duration::from_secs(31);
        assert!(!restart.take_due(t31));

        // at t = 36s (31s after record_restart), it is due
        let t36 = now + Duration::from_secs(36);
        assert!(restart.take_due(t36));
    }

    #[test]
    fn portal_watches_the_dirs_the_gtk_sync_writes() {
        // the sync writes these dirs and the portals are restarted when any of them changes
        assert_eq!(
            super::super::GtkConfig::ALL.map(super::super::GtkConfig::dir),
            DIRS
        );
    }

    // the base portal holds the session's screens, casts and dialogs: only the two frontends restart
    #[test]
    fn restarting_a_portal_never_restarts_the_base_one() {
        assert!(![GTK_UNIT, GNOME_UNIT].contains(&"xdg-desktop-portal.service"));
    }

    #[test]
    fn restarts_once_for_a_burst_and_only_after_a_change() {
        let start = Instant::now();
        let mut restart = Restart::default();
        assert!(!restart.take_due(start), "nothing changed");
        assert_eq!(restart.wait(start), None);

        restart.changed(start);
        assert!(restart.take_due(start), "the first change restarts at once");

        let burst = start + Duration::from_secs(2);
        restart.changed(burst);
        restart.changed(burst + Duration::from_secs(1));
        assert!(!restart.take_due(burst + Duration::from_secs(1)));
        assert_eq!(
            restart.wait(burst),
            Some(MIN_INTERVAL - Duration::from_secs(2))
        );

        let later = start + MIN_INTERVAL;
        assert!(restart.take_due(later), "the burst lands once, in time");
        assert!(!restart.take_due(later + MIN_INTERVAL));
    }
}
