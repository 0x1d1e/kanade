//! Selects the compositor whose IPC is reachable, preferring Umbriel when both are present.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compositor {
    Niri,
    Umbriel,
    None,
}

impl Compositor {
    pub fn current() -> Self {
        active()
    }
}

pub fn active() -> Compositor {
    if super::umbriel::is_active() {
        Compositor::Umbriel
    } else if super::niri::is_active() {
        Compositor::Niri
    } else {
        Compositor::None
    }
}

/// The selected compositor's overview snapshot, without polling its IPC.
pub fn overview() -> bool {
    use kanade_runtime::service::Service;

    match active() {
        Compositor::Umbriel => super::umbriel::Umbriel::read().seen.overview,
        Compositor::Niri => super::niri::Niri::read().seen.overview,
        Compositor::None => false,
    }
}

/// The selected compositor's focused output snapshot, without polling its IPC or reading the
/// inactive backend.
pub fn focused_output() -> Option<String> {
    use kanade_runtime::service::Service;

    match active() {
        Compositor::Umbriel => super::umbriel::Umbriel::read().seen.focused_output.clone(),
        Compositor::Niri => super::niri::Niri::read().seen.focused_output.clone(),
        Compositor::None => None,
    }
}

/// Follow overview events only from the selected compositor, then read its initial snapshot.
/// Called from the owning Module's start so a disabled Module subscribes to nothing.
pub fn watch_overview(then: fn()) {
    watch(then);
}

/// Follow focus/output events only from the selected compositor, then read its initial snapshot.
/// Called from the owning Module's start so a disabled Module subscribes to nothing.
pub fn watch_focus(then: fn()) {
    watch(then);
}

fn watch(then: fn()) {
    use kanade_runtime::service;

    match active() {
        Compositor::Umbriel => service::watch::<super::umbriel::Umbriel>(then),
        Compositor::Niri => service::watch::<super::niri::Niri>(then),
        Compositor::None => {}
    }
    then();
}

/// Logical `(x, y)` origin of `name`, preferring the runtime's cached output coordinates and
/// falling back to the selected compositor backend (never calling `niri::ask` under Umbriel).
pub fn output_logical_origin(name: &str) -> (i32, i32) {
    use kanade_runtime::service::Service;

    if let Some(origin) = kanade_runtime::Monitors::read().origin(name) {
        return origin;
    }

    match active() {
        Compositor::Umbriel => super::umbriel::output_logical_origin(name).unwrap_or((0, 0)),
        Compositor::Niri => {
            let Ok(reply) = super::niri::ask("\"Outputs\"") else {
                return (0, 0);
            };
            let outputs = crate::settings::outputs::parse_outputs(&reply);
            outputs
                .iter()
                .find(|o| o.name == name)
                .and_then(|o| o.logical.as_ref())
                .map_or((0, 0), |logical| (logical.x, logical.y))
        }
        Compositor::None => (0, 0),
    }
}

#[cfg(test)]
fn detected(umbriel: bool, niri: bool) -> Compositor {
    match (umbriel, niri) {
        (true, _) => Compositor::Umbriel,
        (false, true) => Compositor::Niri,
        (false, false) => Compositor::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_detection_ignores_missing_and_stale_sockets() {
        use std::os::unix::net::UnixListener;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("kanade-detection-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let niri = dir.join("niri.sock");
        let umbriel = dir.join("umbriel.sock");
        let check = |expected: &str| {
            assert!(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "sources::compositor::tests::socket_detection_child"
                    ])
                    .env("NIRI_SOCKET", &niri)
                    .env("UMBRIEL_SOCKET", &umbriel)
                    .env("XDG_RUNTIME_DIR", &dir)
                    .env("KANADE_TEST_COMPOSITOR", expected)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        check("none");
        let niri_listener = UnixListener::bind(&niri).unwrap();
        niri_listener.set_nonblocking(true).unwrap();
        check("niri");
        let mut niri_conns = 0;
        while niri_listener.accept().is_ok() {
            niri_conns += 1;
        }
        assert!(niri_conns > 0);
        let umbriel_listener = UnixListener::bind(&umbriel).unwrap();
        check("umbriel");
        assert!(
            niri_listener.accept().is_err(),
            "niri socket was contacted even though umbriel was active"
        );
        drop(niri_listener);
        check("umbriel");
        drop(umbriel_listener);
        check("none");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn socket_detection_child() {
        let Ok(expected) = std::env::var("KANADE_TEST_COMPOSITOR") else {
            return;
        };
        let expected = match expected.as_str() {
            "niri" => Compositor::Niri,
            "umbriel" => Compositor::Umbriel,
            "none" => Compositor::None,
            _ => panic!("unknown test compositor"),
        };
        assert_eq!(active(), expected);
        assert_eq!(Compositor::current(), expected);
    }

    #[test]
    fn overview_events_reach_desk_and_wallpaper_from_only_the_selected_compositor() {
        use std::os::unix::net::UnixListener;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("kanade-overview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let niri = dir.join("niri.sock");
        let umbriel = dir.join("umbriel.sock");
        let check = |expected: &str| {
            assert!(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "sources::compositor::tests::overview_event_child"
                    ])
                    .env("NIRI_SOCKET", &niri)
                    .env("UMBRIEL_SOCKET", &umbriel)
                    .env("XDG_RUNTIME_DIR", &dir)
                    .env("KANADE_TEST_OVERVIEW", expected)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        check("none");
        let niri_listener = UnixListener::bind(&niri).unwrap();
        check("niri");
        let umbriel_listener = UnixListener::bind(&umbriel).unwrap();
        check("umbriel");
        drop((niri_listener, umbriel_listener));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn overview_event_child() {
        use super::super::{niri::Niri, umbriel::Umbriel, wallpaper_moving};
        use crate::desk;
        use kanade_runtime::service::{self, Service};

        let Ok(expected) = std::env::var("KANADE_TEST_OVERVIEW") else {
            return;
        };
        let selected = match expected.as_str() {
            "niri" => Compositor::Niri,
            "umbriel" => Compositor::Umbriel,
            _ => Compositor::None,
        };
        assert_eq!(active(), selected);
        watch_overview(desk::follow_overview);
        watch_overview(wallpaper_moving::follow);
        let check = |open| {
            assert_eq!(overview(), open);
            assert_eq!(*desk::Overview::read() != desk::Overview::default(), open);
            assert_eq!(wallpaper_moving::Resting::read().overview, open);
        };
        check(false);
        // An inactive backend's events cannot hide the Desk or pause its wallpaper.
        if selected != Compositor::Niri {
            Niri::write().seen.overview = true;
        }
        if selected != Compositor::Umbriel {
            Umbriel::write().seen.overview = true;
        }
        service::derive();
        check(false);
        match selected {
            Compositor::Niri => Niri::write().seen.overview = true,
            Compositor::Umbriel => Umbriel::write().seen.overview = true,
            Compositor::None => return,
        }
        service::derive();
        check(true);
        match selected {
            Compositor::Niri => Niri::write().seen.overview = false,
            Compositor::Umbriel => Umbriel::write().seen.overview = false,
            Compositor::None => unreachable!(),
        }
        service::derive();
        check(false);
    }

    #[test]
    fn detection_selects_one_reachable_backend() {
        assert_eq!(detected(false, false), Compositor::None);
        assert_eq!(detected(false, true), Compositor::Niri);
        assert_eq!(detected(true, false), Compositor::Umbriel);
        assert_eq!(detected(true, true), Compositor::Umbriel);
    }

    #[test]
    fn focus_and_fullscreen_events_reach_banners_from_only_the_selected_compositor() {
        use std::os::unix::net::UnixListener;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("kanade-focus-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let niri = dir.join("niri.sock");
        let umbriel = dir.join("umbriel.sock");
        let check = |expected: &str| {
            assert!(
                Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "sources::compositor::tests::focus_event_child",
                        "--nocapture",
                    ])
                    .env("NIRI_SOCKET", &niri)
                    .env("UMBRIEL_SOCKET", &umbriel)
                    .env("XDG_RUNTIME_DIR", &dir)
                    .env("KANADE_TEST_FOCUS", expected)
                    .status()
                    .unwrap()
                    .success()
            );
        };
        check("none");
        let niri_listener = UnixListener::bind(&niri).unwrap();
        niri_listener.set_nonblocking(true).unwrap();
        check("niri");
        while niri_listener.accept().is_ok() {}
        let umbriel_listener = UnixListener::bind(&umbriel).unwrap();
        check("umbriel");
        assert!(
            niri_listener.accept().is_err(),
            "niri socket was contacted while umbriel was active"
        );
        drop((niri_listener, umbriel_listener));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn focus_event_child() {
        use super::super::{fullscreen, niri::Niri, notifications, umbriel::Umbriel};
        use crate::banners::{self, Banners};
        use kanade_runtime::service::{self, Service};
        use kanade_runtime::{Monitor, Monitors};

        let Ok(expected) = std::env::var("KANADE_TEST_FOCUS") else {
            return;
        };
        let selected = match expected.as_str() {
            "niri" => Compositor::Niri,
            "umbriel" => Compositor::Umbriel,
            _ => Compositor::None,
        };
        assert_eq!(active(), selected);

        Monitors::publish(vec![
            Monitor {
                name: String::from("eDP-1"),
                width: 1920,
                height: 1080,
            },
            Monitor {
                name: String::from("HDMI-A-1"),
                width: 1920,
                height: 1080,
            },
        ]);

        // Match catalog.rs startup order: island updates Fullscreen, then banners follow focus/cover.
        service::watch::<Niri>(fullscreen::follow_shown);
        service::watch::<Umbriel>(fullscreen::follow_shown);
        watch_focus(banners::follow_focus);
        watch_focus(banners::follow_cover);
        service::watch::<fullscreen::Fullscreen>(banners::follow_cover);

        let check = |output: Option<&str>, quiet: bool| {
            assert_eq!(focused_output().as_deref(), output);
            assert_eq!(Banners::read().focused_output(), output);
            assert_eq!(notifications::fullscreen_quiet(), quiet);
            assert_eq!(Banners::read().is_held(), quiet);
        };
        check(None, false);

        let showing = super::super::niri::Showing {
            app_id: Some(String::from("game")),
            tile: (1920.0, 1080.0),
            fullscreen: true,
        };

        // Inactive backend updates must not affect focused_output, Banners, or fullscreen_silence.
        if selected != Compositor::Niri {
            let mut niri = Niri::write();
            niri.seen.focused_output = Some(String::from("eDP-1"));
            niri.seen
                .shown
                .insert(String::from("eDP-1"), showing.clone());
        }
        if selected != Compositor::Umbriel {
            let mut umbriel = Umbriel::write();
            umbriel.seen.focused_output = Some(String::from("eDP-1"));
            umbriel
                .seen
                .shown
                .insert(String::from("eDP-1"), showing.clone());
        }
        service::derive();
        check(None, false);

        match selected {
            Compositor::Niri => {
                let mut niri = Niri::write();
                niri.seen.focused_output = Some(String::from("eDP-1"));
                niri.seen.shown.insert(String::from("eDP-1"), showing);
            }
            Compositor::Umbriel => {
                let mut umbriel = Umbriel::write();
                umbriel.seen.focused_output = Some(String::from("eDP-1"));
                umbriel.seen.shown.insert(String::from("eDP-1"), showing);
            }
            Compositor::None => return,
        }
        service::derive();
        check(Some("eDP-1"), true);

        // Moving focus to an uncovered output releases fullscreen_silence and updates Banners.
        match selected {
            Compositor::Niri => Niri::write().seen.focused_output = Some(String::from("HDMI-A-1")),
            Compositor::Umbriel => {
                Umbriel::write().seen.focused_output = Some(String::from("HDMI-A-1"))
            }
            Compositor::None => unreachable!(),
        }
        service::derive();
        check(Some("HDMI-A-1"), false);
    }

    #[test]
    fn umbriel_outputs_and_recording_never_contact_niri() {
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixListener;
        use std::process::Command;

        let dir = std::env::temp_dir().join(format!("kanade-umbriel-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let niri = dir.join("niri.sock");
        let umbriel = dir.join("umbriel.sock");

        let niri_listener = UnixListener::bind(&niri).unwrap();
        niri_listener.set_nonblocking(true).unwrap();
        let umbriel_listener = UnixListener::bind(&umbriel).unwrap();

        let server = std::thread::spawn(move || {
            for stream in umbriel_listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    continue;
                }
                let mut writer = stream;
                if line.contains("\"outputs\"") {
                    let reply = r#"{"ok":[{"name":"DP-1","enabled":true,"focused":true,"logical_size":{"width":1920,"height":1080},"position":{"x":-1920,"y":120},"scale":1.0,"transform":"Normal","modes":[]}]}"#;
                    let _ = writeln!(writer, "{reply}");
                }
            }
        });

        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "sources::compositor::tests::umbriel_outputs_and_recording_child",
                "--nocapture",
            ])
            .env("NIRI_SOCKET", &niri)
            .env("UMBRIEL_SOCKET", &umbriel)
            .env("XDG_RUNTIME_DIR", &dir)
            .env("KANADE_TEST_UMBRIEL_OUT", "1")
            .status()
            .unwrap();

        assert!(
            niri_listener.accept().is_err(),
            "niri socket was contacted while umbriel was active"
        );
        drop(niri_listener);
        drop(server);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(status.success());
    }

    #[test]
    fn umbriel_outputs_and_recording_child() {
        use super::super::recording::{self, Source, Target};

        if std::env::var_os("KANADE_TEST_UMBRIEL_OUT").is_none() {
            return;
        }

        assert_eq!(active(), Compositor::Umbriel);
        assert_eq!(output_logical_origin("DP-1"), (-1920, 120));
        assert_eq!(
            recording::resolve_source(Target::Screen),
            Ok(Some(Source::Output(String::from("DP-1"))))
        );
        let err = recording::resolve_source(Target::Window(None)).unwrap_err();
        assert!(err.contains("umbriel"), "{err}");
    }
}
