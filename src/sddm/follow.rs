//! `sddm.sync`: the login screen's picture follows the wallpaper. The wallpaper source announces a
//! change (`sources::wallpaper::hook`); this makes the picture again, off the caller's thread, with
//! no polling. Changes that come close together are one write of the last (`Pending`), and a
//! flock on a file in the user's state directory makes a write here and one of `kanade sddm sync`
//! take turns.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use kanade_runtime::MutexExt;

use super::class;
use super::{
    Config, OWN, SddmBackground, Wanted, Where, backdrop, largest_output, locked, put_picture,
    ready,
};
use crate::modules;

// a quiet this long after a change, so a slideshow or the outputs of one set make one picture
const SETTLE: Duration = Duration::from_millis(600);

/*
 * the wallpaper waiting to be written and whether a worker is on it. The state is one lock, so a
 * change that arrives as the worker finds nothing left either reaches it or starts the next worker
 */
#[derive(Default)]
struct Pending {
    path: Option<PathBuf>,
    running: bool,

    // which worker owns the two above: one that dies of a panic must not clear a newer worker's
    worker: u64,
}

impl Pending {
    // notes the latest wallpaper; the worker to start for it, or none when one already runs
    fn submit(&mut self, path: PathBuf) -> Option<u64> {
        self.path = Some(path);

        if std::mem::replace(&mut self.running, true) {
            return None;
        }

        self.worker += 1;
        Some(self.worker)
    }

    // the wallpaper to write, the latest; none ends the worker
    fn take(&mut self) -> Option<PathBuf> {
        let path = self.path.take();

        if path.is_none() {
            self.running = false;
        }

        path
    }
}

static PENDING: Mutex<Pending> = Mutex::new(Pending {
    path: None,
    running: false,
    worker: 0,
});

// what was last said, so a failure that repeats with every wallpaper is said once
static SAID: Mutex<Option<String>> = Mutex::new(None);

/*
 * the wallpaper now shows `path`: makes the login screen's picture from it, if `sddm.sync` is on
 * and the Module runs. Returns at once
 */
pub fn wallpaper_changed(path: &Path) {
    if !modules::on("sddm") {
        return;
    }

    let config = crate::config::get();
    if !config.sddm_sync || config.sddm_background != SddmBackground::Wallpaper {
        return;
    }

    let worker = PENDING.locked().submit(path.to_owned());
    let Some(worker) = worker else {
        return;
    };

    let spawned = std::thread::Builder::new()
        .name(String::from("sddm sync"))
        .spawn(move || follow(worker));

    if let Err(error) = spawned {
        PENDING.locked().running = false;
        super::on_stderr(&format!("cannot follow the wallpaper: {error}"));
    }
}

/*
 * a worker that dies of a panic leaves `running` set, which would stop every later change from
 * starting one, and the path it was given pending, which no worker is left to write. Dropped by a
 * panic, it clears both; a worker that ends by itself already has, and a newer worker may be
 * running by then, which it must not clear
 */
struct Unwound<'a> {
    pending: &'a Mutex<Pending>,
    worker: u64,
}

impl Drop for Unwound<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }

        let mut pending = self.pending.locked();
        if pending.worker == self.worker {
            pending.path = None;
            pending.running = false;
        }
    }
}

fn follow(worker: u64) {
    let _unwound = Unwound {
        pending: &PENDING,
        worker,
    };

    loop {
        std::thread::sleep(SETTLE);

        let Some(path) = PENDING.locked().take() else {
            return;
        };
        let done = refresh(&Where::here(), fresh, &path, largest_output);

        let mut said = SAID.locked();
        match done {
            Ok(_) => *said = None,
            Err(error) if said.as_deref() == Some(error.as_str()) => {}
            Err(error) => {
                super::on_stderr(&format!("not following the wallpaper: {error}"));
                *said = Some(error);
            }
        }
    }
}

// the config as the files give it now, the settings file too: what a `disable` just wrote, which
// the shell's own copy may not have reloaded yet
fn fresh() -> Config {
    crate::config::layers(true).0
}

/*
 * writes the login screen picture under flock, rechecking config so a concurrent disable is not
 * undone. Writes only the picture if the overlay already names it; otherwise writes the full look
 * or reports when the overlay is missing
 */
pub fn refresh(
    w: &Where,
    config: impl Fn() -> Config,
    source: &Path,
    size: impl Fn() -> (u32, u32),
) -> Result<String, String> {
    let prefix = w.text()?;

    // the size is asked of niri before the lock, so a `kanade sddm sync` waiting on it is not kept
    // waiting for an IPC round trip; when the config as it reads now says no picture is written,
    // it is asked under the lock only if the config changed while this waited for it
    let early = config();
    let wanted = early.sddm_sync
        && early.sddm_background == SddmBackground::Wallpaper
        && early.sddm_theme.as_deref().unwrap_or(OWN) == OWN;
    let known = wanted.then(&size);

    locked(w, || {
        let config = config();
        if !config.sddm_sync || config.sddm_background != SddmBackground::Wallpaper {
            return Ok(String::from(
                "not following the wallpaper any more: nothing written",
            ));
        }
        let want = Wanted::of(&config)?;
        if want.theme != OWN {
            return Err(String::from(
                "wallpaper following supports only the root-installed kanade theme",
            ));
        }
        ready(w, &prefix)?;

        let key = "backdrop";
        let named = format!("/{}/backdrop.png", super::DATA);
        let names = class::read(&w.overlay(&want.theme).join(super::USER)).is_ok_and(|read| {
            class::pairs(&read.text)
                .iter()
                .any(|(_, set, value)| set == key && *value == named)
        });

        if names {
            let bytes = backdrop(source, known.unwrap_or_else(size), want.blur, want.dim)?;
            put_picture(&w.picture(), &bytes)?;

            Ok(format!(
                "picture: {} from {}",
                class::plain(&w.picture().display().to_string()),
                class::plain(&source.display().to_string())
            ))
        } else {
            Err(String::from(
                "the root-installed kanade theme does not name the backdrop data file; run `kanade sddm setup` as root",
            ))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::MetadataExt;

    use kanade_lock::Picture;

    fn png(shade: u8) -> Vec<u8> {
        super::super::encode(&Picture {
            width: 8,
            height: 8,
            rgba: vec![shade; 8 * 8 * 4],
        })
        .unwrap()
    }

    fn follows() -> Config {
        Config {
            sddm_sync: true,
            sddm_background: SddmBackground::Wallpaper,
            ..Config::default()
        }
    }

    // changes that come close are one write of the last, and a change during a write is not lost
    #[test]
    fn changes_close_together_are_one_write_of_the_last() {
        let mut pending = Pending::default();

        assert_eq!(
            pending.submit("/a.png".into()),
            Some(1),
            "the first starts a worker"
        );
        assert_eq!(pending.submit("/b.png".into()), None, "the next joins it");
        assert_eq!(pending.take(), Some("/b.png".into()));

        // a change while the worker writes is taken by it, not by another
        assert_eq!(pending.submit("/c.png".into()), None);
        assert_eq!(pending.take(), Some("/c.png".into()));
        assert_eq!(pending.take(), None);

        // ended, so the next change starts a worker again, with an id of its own
        assert_eq!(pending.submit("/d.png".into()), Some(2));
    }

    #[test]
    fn writers_of_the_overlay_take_turns() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-flock-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());

        let first = super::super::hold(&w).unwrap().unwrap();

        // a second holder, in this process or another, finds it taken
        let second = fs::OpenOptions::new()
            .append(true)
            .open(w.lock.as_ref().unwrap())
            .unwrap();
        assert!(matches!(
            second.try_lock(),
            Err(fs::TryLockError::WouldBlock)
        ));

        drop(first);
        assert!(second.try_lock().is_ok());

        fs::remove_dir_all(root).unwrap();
    }

    // the niri round trip for the size happens before the flock, so a `kanade sddm sync` waiting on
    // the lock is not kept waiting for it
    #[test]
    fn the_size_is_asked_of_niri_before_the_lock_is_taken() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-size-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let w = Where::under(root.clone());
        super::super::install_own(&w).unwrap();
        fs::create_dir_all(w.data()).unwrap();
        fs::set_permissions(
            w.data(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        let source = root.join("wall.png");
        fs::write(&source, png(50)).unwrap();

        // the lock file exists before the run, as `hold` makes it
        drop(super::super::hold(&w).unwrap());

        let asked = std::cell::Cell::new(0);
        let lock = w.lock.clone().unwrap();
        let done = refresh(&w, follows, &source, || {
            asked.set(asked.get() + 1);
            let held = fs::OpenOptions::new().append(true).open(&lock).unwrap();
            assert!(
                held.try_lock().is_ok(),
                "the overlay's lock is not held while niri is asked for the size"
            );

            (16, 16)
        })
        .unwrap();

        assert!(done.starts_with("picture:"), "{done}");
        assert_eq!(asked.get(), 1, "asked once, outside the lock");
        fs::remove_dir_all(root).unwrap();
    }

    // the user's overlay may hold a link or a second name for the picture; neither is written through
    #[test]
    fn a_picture_is_never_written_through_a_link_or_a_second_name() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-follow-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());

        // the overlay is not made yet: said with the root command, nothing written
        let source = root.join("wall.png");
        fs::create_dir_all(&root).unwrap();
        fs::write(&source, png(40)).unwrap();
        let missing = refresh(&w, follows, &source, || (16, 16)).unwrap_err();
        assert!(missing.contains("kanade sddm setup"), "{missing}");
        assert!(!w.prefix().exists());

        // data is user-writable, while the built-in theme and config are root-owned
        fs::create_dir_all(w.prefix()).unwrap();
        fs::set_permissions(
            w.prefix(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::create_dir_all(w.data()).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::set_permissions(
            w.prefix().join("themes"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::create_dir_all(w.overlay(OWN)).unwrap();
        fs::set_permissions(
            w.data(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.prefix().join("themes"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.overlay(OWN),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join("metadata.desktop"),
            "[SddmGreeterTheme]\nQtVersion=6\n",
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join(super::super::USER),
            format!("[General]\nbackdrop=/{}/backdrop.png\n", super::super::DATA),
        )
        .unwrap();
        let theme_code = fs::read(w.overlay(OWN).join("metadata.desktop")).unwrap();
        let conf = fs::read(w.overlay(OWN).join(super::super::USER)).unwrap();
        let first = refresh(&w, follows, &source, || (16, 16)).unwrap();
        assert!(first.starts_with("picture:"), "{first}");
        let before = fs::read(w.picture()).unwrap();

        fs::write(&source, png(200)).unwrap();
        let second = refresh(&w, follows, &source, || (16, 16)).unwrap();
        assert!(second.starts_with("picture:"), "{second}");
        assert_ne!(fs::read(w.picture()).unwrap(), before);
        assert_eq!(
            fs::read(w.overlay(OWN).join("metadata.desktop")).unwrap(),
            theme_code
        );
        assert_eq!(
            fs::read(w.overlay(OWN).join(super::super::USER)).unwrap(),
            conf
        );

        // a second name for the picture: refused, and the other name is untouched
        let other = root.join("other.png");
        fs::hard_link(w.picture(), &other).unwrap();
        let held = fs::read(&other).unwrap();
        let error = refresh(&w, follows, &source, || (16, 16)).unwrap_err();
        assert!(error.contains("hard link"), "{error}");
        assert_eq!(fs::read(&other).unwrap(), held);
        assert_eq!(fs::metadata(w.picture()).unwrap().nlink(), 2);
        fs::remove_file(&other).unwrap();

        // a link: refused, and what it points at is untouched
        let target = root.join("target.png");
        fs::write(&target, b"mine").unwrap();
        fs::remove_file(w.picture()).unwrap();
        std::os::unix::fs::symlink(&target, w.picture()).unwrap();
        let error = refresh(&w, follows, &source, || (16, 16)).unwrap_err();
        assert!(error.contains("symbolic link"), "{error}");
        assert_eq!(fs::read(&target).unwrap(), b"mine");

        fs::remove_dir_all(root).unwrap();
    }

    // a byte order mark before [General] hides nothing from the check that the conf names the picture
    #[test]
    fn a_conf_written_with_a_byte_order_mark_still_names_the_picture() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-bom-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        fs::create_dir_all(w.data()).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::create_dir_all(w.overlay(OWN)).unwrap();
        fs::set_permissions(
            w.prefix().join("themes"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.overlay(OWN),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.data(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join("metadata.desktop"),
            "[SddmGreeterTheme]\nQtVersion=6\n",
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join(super::super::USER),
            format!("[General]\nbackdrop=/{}/backdrop.png\n", super::super::DATA),
        )
        .unwrap();
        let source = root.join("wall.png");
        fs::write(&source, png(90)).unwrap();

        refresh(&w, follows, &source, || (16, 16)).unwrap();
        let at = w.overlay(OWN).join(super::super::USER);
        let conf = fs::read_to_string(&at).unwrap();
        fs::write(&at, format!("\u{feff}{conf}")).unwrap();

        let again = refresh(&w, follows, &source, || (16, 16)).unwrap();
        assert!(again.starts_with("picture:"), "{again}");

        fs::remove_dir_all(root).unwrap();
    }

    // a disable that wins the race for the lock leaves no picture to be written again
    #[test]
    fn a_follow_that_was_turned_off_while_it_waited_writes_nothing() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-off-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        fs::create_dir_all(w.data()).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::create_dir_all(w.overlay(OWN)).unwrap();
        fs::set_permissions(
            w.prefix().join("themes"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.overlay(OWN),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.data(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join("metadata.desktop"),
            "[SddmGreeterTheme]\nQtVersion=6\n",
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join(super::super::USER),
            format!("[General]\nbackdrop=/{}/backdrop.png\n", super::super::DATA),
        )
        .unwrap();
        let source = root.join("wall.png");
        fs::write(&source, png(40)).unwrap();

        // asked inside the lock, so a config turned off by then is what counts
        let said = refresh(&w, Config::default, &source, || (16, 16)).unwrap();
        assert!(said.contains("nothing written"), "{said}");
        assert!(!w.picture().exists());

        let off = || Config {
            sddm_sync: true,
            sddm_background: SddmBackground::Path,
            ..Config::default()
        };
        assert!(
            refresh(&w, off, &source, || (16, 16))
                .unwrap()
                .contains("nothing written")
        );
        assert!(!w.picture().exists());

        fs::remove_dir_all(root).unwrap();
    }

    // a worker that panics does not stop the next change from starting one, and leaves no path of
    // its own waiting for a worker that will never write it
    #[test]
    fn a_worker_that_panics_clears_its_path_and_lets_the_next_change_start_another() {
        static LOCAL: Mutex<Pending> = Mutex::new(Pending {
            path: None,
            running: false,
            worker: 0,
        });

        let first = LOCAL.locked().submit("/a.png".into()).unwrap();
        let died = std::thread::spawn(move || {
            let _unwound = Unwound {
                pending: &LOCAL,
                worker: first,
            };
            // a change arrives during the refresh that then fails
            assert_eq!(LOCAL.locked().submit("/during.png".into()), None);
            panic!("the refresh failed");
        })
        .join();
        assert!(died.is_err());
        assert_eq!(LOCAL.locked().take(), None, "nothing is left pending");
        assert_eq!(
            LOCAL.locked().submit("/b.png".into()),
            Some(2),
            "a new worker starts"
        );

        // an older worker that panics late leaves a newer one's path and flag alone
        assert_eq!(LOCAL.locked().take(), Some("/b.png".into()));
        assert_eq!(LOCAL.locked().submit("/newer.png".into()), None);
        let died = std::thread::spawn(move || {
            let _unwound = Unwound {
                pending: &LOCAL,
                worker: first,
            };
            panic!("an old worker failed late");
        })
        .join();
        assert!(died.is_err());
        assert_eq!(
            LOCAL.locked().take(),
            Some("/newer.png".into()),
            "the newer worker's path stays"
        );

        // one that ends by itself leaves a newer worker's flag alone
        assert_eq!(
            LOCAL.locked().submit("/c.png".into()),
            None,
            "the worker runs on"
        );
        assert_eq!(LOCAL.locked().take(), Some("/c.png".into()));
        let ended = std::thread::spawn(move || {
            let _unwound = Unwound {
                pending: &LOCAL,
                worker: 2,
            };
        })
        .join();
        assert!(ended.is_ok());
        assert_eq!(LOCAL.locked().submit("/d.png".into()), None);
    }

    // the wallpaper is only read: a link to it is followed, while the picture is still never written
    // through one
    #[test]
    fn a_wallpaper_that_is_a_link_is_read_through_it() {
        let root = std::env::temp_dir().join(format!("kanade-sddm-wall-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let w = Where::under(root.clone());
        fs::create_dir_all(w.data()).unwrap();
        fs::create_dir_all(w.prefix().join("themes")).unwrap();
        fs::create_dir_all(w.overlay(OWN)).unwrap();
        fs::set_permissions(
            w.prefix().join("themes"),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.overlay(OWN),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            w.data(),
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join("metadata.desktop"),
            "[SddmGreeterTheme]\nQtVersion=6\n",
        )
        .unwrap();
        fs::write(
            w.overlay(OWN).join(super::super::USER),
            format!("[General]\nbackdrop=/{}/backdrop.png\n", super::super::DATA),
        )
        .unwrap();
        let real = root.join("real.png");
        fs::write(&real, png(120)).unwrap();
        let link = root.join("current.png");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let done = refresh(&w, follows, &link, || (16, 16)).unwrap();
        assert!(done.starts_with("picture:"), "{done}");
        assert!(w.picture().exists());

        // a link that leads to a directory is no picture
        let dir_link = root.join("dir.png");
        std::os::unix::fs::symlink(&root, &dir_link).unwrap();
        assert!(refresh(&w, follows, &dir_link, || (16, 16)).is_err());

        fs::remove_dir_all(root).unwrap();
    }
}
