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
    fn detection_selects_one_reachable_backend() {
        assert_eq!(detected(false, false), Compositor::None);
        assert_eq!(detected(false, true), Compositor::Niri);
        assert_eq!(detected(true, false), Compositor::Umbriel);
        assert_eq!(detected(true, true), Compositor::Umbriel);
    }
}
