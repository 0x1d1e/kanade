//! The previews of the clipboard's pictures (ADR 0114): the runtime draws images from files, so a
//! picture's bytes are written, only once a row that shows it is built, to `$XDG_RUNTIME_DIR/
//! kanade-clipboard/<pid>/<id>.<ext>`, a tmpfs, owner-only (the directories 0700, a file 0600). Never
//! anywhere else: with no runtime directory there is no preview. A preview is deleted with its entry,
//! whether removed, evicted or cleared, and the folders of the Kanades that ended, and no other, are
//! swept at start. Its name is the entry's id and an extension from a fixed table of types, nothing
//! the copy itself says. The bytes and the paths are never logged.

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};
use std::time::{Duration, Instant};

use kanade_runtime::{MutexExt, service::Service};

use super::{Clipboard, Content, Entry};
use crate::atomic::{self, Mode, Staged};
use crate::supervise;

const FOLDER: &str = "kanade-clipboard";

// the types a preview is written for, and the extension each is given
const EXTENSIONS: [(&str, &str); 4] = [
    ("image/png", "png"),
    ("image/jpeg", "jpg"),
    ("image/webp", "webp"),
    ("image/gif", "gif"),
];

fn extension(mime: &str) -> Option<&'static str> {
    EXTENSIONS
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(mime))
        .map(|(_, extension)| *extension)
}

// the folder the previews live in
#[derive(Debug)]
struct Dir {
    // the folder of this process's own previews, named by its pid under `root`; a second Kanade,
    // or a restart racing the one it replaces, never writes into it or deletes from it
    own: PathBuf,

    // the folder every Kanade's are under
    root: PathBuf,
}

impl Dir {
    // the folders under the runtime directory `runtime`, if it is an absolute path to a directory
    fn under(runtime: Option<OsString>) -> Option<Dir> {
        let runtime = PathBuf::from(runtime?);

        (runtime.is_absolute() && runtime.is_dir())
            .then(|| Dir::of(runtime.join(FOLDER), process::id()))
    }

    // the folder of the process `pid`'s previews, under the shared `root`
    fn of(root: PathBuf, pid: u32) -> Dir {
        Dir {
            own: root.join(pid.to_string()),
            root,
        }
    }

    // the preview of the entry `id`: its id and the extension of its type, if it has one
    fn path(&self, id: u64, mime: &str) -> Option<PathBuf> {
        Some(self.own.join(format!("{id}.{}", extension(mime)?)))
    }

    // writes the bytes whole and private into a file beside `path`, flushed but not yet renamed
    // over it, so publishing it is all a caller must take turns for; the folder is made 0700
    fn stage(&self, path: &Path, bytes: &[u8]) -> io::Result<Staged> {
        atomic::stage(path, bytes, Mode::Private)
    }

    // deletes every preview in this process's folder whose entry `live` does not know. A part an
    // unfinished write left is not taken, it is finished or removed by that write, or swept at start
    fn retain(&self, live: impl Fn(u64) -> bool) {
        let Ok(files) = fs::read_dir(&self.own) else {
            return;
        };

        for file in files.flatten() {
            let name = file.file_name();
            let name = name.to_string_lossy();

            if name.starts_with('.') {
                continue;
            }

            let kept = name
                .split_once('.')
                .and_then(|(id, extension)| {
                    let known = EXTENSIONS.iter().any(|(_, known)| *known == extension);

                    known.then(|| id.parse::<u64>().ok()).flatten()
                })
                .is_some_and(&live);

            if !kept {
                let _ = fs::remove_file(file.path());
            }
        }
    }

    /*
     * deletes what the Kanades that are no longer running left in the shared root: the folder of one
     * whose process is gone, this one's own (a crash under the same pid leaves it stale, as the ids
     * start again at 1), and a preview an older Kanade wrote in the root itself. A running Kanade's
     * folder, and a part one is writing, are left alone
     */
    fn sweep(&self) {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };

        let ours = process::id();

        for entry in entries.flatten() {
            let path = entry.path();
            let directory = entry.file_type().is_ok_and(|kind| kind.is_dir());

            if directory {
                let name = entry.file_name();
                let pid = name.to_str().and_then(|name| name.parse::<u32>().ok());

                if pid.is_some_and(|pid| pid == ours || !atomic::alive(pid)) {
                    let _ = fs::remove_dir_all(&path);
                }

                continue;
            }

            // this Kanade writes only into its own folder, so a bare preview in the root is an
            // older Kanade's, which swept the whole root and so kept none of it either
            if preview_name(&entry.file_name().to_string_lossy()) {
                let _ = fs::remove_file(&path);
            }
        }
    }
}

// whether `name` is what a preview of this Kanade's is called: an id and a known extension
fn preview_name(name: &str) -> bool {
    name.split_once('.').is_some_and(|(id, extension)| {
        id.parse::<u64>().is_ok() && EXTENSIONS.iter().any(|(_, known)| *known == extension)
    })
}

fn dir() -> Option<&'static Dir> {
    static DIR: OnceLock<Option<Dir>> = OnceLock::new();

    DIR.get_or_init(|| Dir::under(env::var_os("XDG_RUNTIME_DIR")))
        .as_ref()
}

/*
 * the previews written, by entry id. A row reads it, so it is drawn again when one is ready; only the
 * preview thread and `reconcile` write it
 */
#[derive(Debug, Default)]
pub struct Previews {
    ready: HashMap<u64, PathBuf>,
}

impl Previews {
    // where the preview of the entry `id` is, if it was written and its file is still there: one
    // deleted since (by a Kanade that started beside this one, or by hand) is missing, to be written again
    fn file(&self, id: u64) -> Option<&PathBuf> {
        self.ready.get(&id).filter(|path| path.is_file())
    }
}

impl Service for Previews {
    fn new() -> Self {
        Previews::default()
    }

    fn listen() {}
}

// where the preview of the entry `id` is, once it is written
pub fn path(id: u64) -> Option<PathBuf> {
    Previews::read().file(id).cloned()
}

// a picture to write
struct Job {
    id: u64,
    mime: String,
    bytes: Arc<[u8]>,
}

#[derive(Default)]
struct Work {
    pending: HashSet<u64>,

    /*
     * a write that failed, and the tries it has had: a failure may be a full tmpfs or a name taken
     * once, so it is tried again a few times, each after a longer pause, rather than left without a
     * preview for the rest of its life in the history
     */
    failed: HashMap<u64, Tries>,
    jobs: Option<Sender<Job>>,
}

// the tries of a preview that keeps failing, and when the next may run
#[derive(Debug, Clone, Copy)]
struct Tries {
    count: u32,
    due: Instant,
}

// how many times a failing preview is tried, and how long between them
const TRIES: u32 = 4;
const RETRY: Duration = Duration::from_secs(30);

static WORK: LazyLock<Mutex<Work>> = LazyLock::new(Mutex::default);

/*
 * writing and deleting take turns, so a preview is never deleted as it is written or registered
 * after it. What it keeps one at a time is the publish: the bytes are written whole beside the
 * preview first, off it, since the decode that makes them may hold megabytes and take as long as it
 * takes
 */
static TURN: Mutex<()> = Mutex::new(());

/*
 * asks for the preview of a picture an entry holds, to be written off the caller's thread; the
 * view calls it for the rows it shows. Reads `Previews`, so the caller is drawn again once it is
 * ready. Nothing for text, a type with no extension, or without a runtime directory
 */
pub fn request(entry: &Entry) {
    let Content::Image { mime, bytes } = &entry.content else {
        return;
    };

    if dir().is_none() || extension(mime).is_none() || Previews::read().file(entry.id).is_some() {
        return;
    }

    let mut work = WORK.locked();
    let now = Instant::now();

    // a preview that keeps failing is tried again later, not at each row drawn
    if work
        .failed
        .get(&entry.id)
        .is_some_and(|tries| tries.due > now)
    {
        return;
    }

    if !work.pending.insert(entry.id) {
        return;
    }

    let jobs = work.jobs.get_or_insert_with(|| {
        let (jobs, heard) = mpsc::channel::<Job>();

        // for good, one at a time
        supervise::spawn("clipboard-preview", move || {
            for job in &heard {
                make(job);
            }
        });

        jobs
    });

    let sent = jobs.send(Job {
        id: entry.id,
        mime: mime.clone(),
        bytes: Arc::clone(bytes),
    });

    if sent.is_err() {
        work.pending.remove(&entry.id);
    }
}

/*
 * says a preview could not be written, by its cause only, never the path or the picture, and when
 * it may be tried again: once past `TRIES`, not for the rest of the entry's life
 */
fn failed(id: u64, error: io::Error) {
    eprintln!(
        "kanade: cannot write a clipboard preview ({})",
        error.kind()
    );

    let mut work = WORK.locked();
    let tries = work.failed.entry(id).or_insert(Tries {
        count: 0,
        due: Instant::now(),
    });

    tries.count += 1;

    if tries.count >= TRIES {
        work.failed.remove(&id);
    } else {
        // twice as long each time, so a tmpfs that stays full is not hammered
        let wait = RETRY * 2u32.saturating_pow(tries.count - 1);

        tries.due = Instant::now() + wait;
    }
}

/*
 * writes one preview and says it is ready, unless its entry went meanwhile. The bytes and the flush
 * are done before `TURN`, so deleting never waits behind a picture being written: only taking turns
 * to publish the finished file, or to find the entry gone, holds it
 */
fn make(Job { id, mime, bytes }: Job) {
    let Some(dir) = dir() else { return };
    let Some(path) = dir.path(id, &mime) else {
        WORK.locked().pending.remove(&id);

        return;
    };

    let staged = match dir.stage(&path, &bytes) {
        Ok(staged) => staged,
        Err(error) => {
            failed(id, error);
            WORK.locked().pending.remove(&id);

            return;
        }
    };

    // the entry may have gone while the bytes were written; dropped uncommitted, the file is removed
    {
        let _turn = TURN.locked();

        // bound first: a read held through the write of `Previews` would hold two Services at once
        let kept = Clipboard::read().has(id);

        if !kept {
            drop(staged);
        } else {
            match staged.commit(&path) {
                Ok(()) => {
                    drop(Previews::write().ready.insert(id, path));
                }
                Err(error) => failed(id, error),
            }
        }
    }

    WORK.locked().pending.remove(&id);
}

/*
 * deletes the previews of the entries the history forgot, after it changed; the file goes before
 * the row that drew it can ask again, and an id is never reused
 */
pub fn reconcile() {
    let Some(dir) = dir() else { return };

    let live: HashSet<u64> = Clipboard::read()
        .entries()
        .iter()
        .map(|entry| entry.id)
        .collect();

    // the read is let go of before the turn, which a running write takes
    let _turn = TURN.locked();

    dir.retain(|id| live.contains(&id));

    let stale = Previews::read().ready.keys().any(|id| !live.contains(id));

    if stale {
        Previews::write().ready.retain(|id, _| live.contains(id));
    }

    WORK.locked().failed.retain(|id, _| live.contains(id));
}

/*
 * at start: the folders of the Kanades that ended, and no other's; one running beside this one
 * keeps its previews, as does this one's own after a crash under the same pid
 */
pub fn sweep() {
    if let Some(dir) = dir() {
        dir.sweep();
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::process;

    use super::super::{ENTRIES, Pinning};
    use super::*;

    fn temporary(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("kanade-preview-{name}-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        dir
    }

    fn picture(fill: u8) -> Content {
        Content::Image {
            mime: String::from("image/png"),
            bytes: vec![fill; 8].into(),
        }
    }

    fn bits(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    // a preview written as `make` writes one: staged whole, then renamed over its name
    fn write(dir: &Dir, id: u64, mime: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.path(id, mime).unwrap();

        dir.stage(&path, bytes).unwrap().commit(&path).unwrap();

        path
    }

    fn names(path: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(path)
            .map(|files| {
                files
                    .flatten()
                    .map(|file| file.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();

        names.sort();
        names
    }

    // the name is the id and an extension of a fixed table, whatever the copy says; private
    #[test]
    fn a_preview_is_named_by_id_alone_and_kept_private() {
        let runtime = temporary("private");
        let dir = Dir::under(Some(runtime.clone().into_os_string())).unwrap();

        for (mime, name) in [
            ("image/png", "7.png"),
            ("image/jpeg", "7.jpg"),
            ("image/webp", "7.webp"),
            ("image/gif", "7.gif"),
        ] {
            assert_eq!(dir.path(7, mime), Some(dir.own.join(name)), "{mime}");
        }

        for mime in [
            "image/svg+xml",
            "image/../../x",
            "text/plain",
            "",
            "image/png/..",
        ] {
            assert_eq!(dir.path(7, mime), None, "{mime:?}");
        }

        let path = write(&dir, 7, "image/png", b"bytes");

        assert_eq!(
            path,
            runtime
                .join(FOLDER)
                .join(process::id().to_string())
                .join("7.png")
        );
        assert_eq!(fs::read(&path).unwrap(), b"bytes");
        assert_eq!(bits(&path), 0o600);
        assert_eq!(bits(&dir.own), 0o700);
        assert!(dir.path(8, "image/bmp").is_none());
        assert_eq!(names(&dir.own), ["7.png"]);

        let _ = fs::remove_dir_all(&runtime);
    }

    // a preview whose file is gone, deleted by hand, is missing again
    #[test]
    fn a_ready_preview_whose_file_is_gone_is_missing() {
        let runtime = temporary("gone");
        let dir = Dir::under(Some(runtime.clone().into_os_string())).unwrap();
        let path = write(&dir, 7, "image/png", b"x");

        let mut previews = Previews::default();
        previews.ready.insert(7, path.clone());

        assert_eq!(previews.file(7), Some(&path));
        assert_eq!(previews.file(8), None);

        fs::remove_file(&path).unwrap();
        assert_eq!(previews.file(7), None);

        let _ = fs::remove_dir_all(&runtime);
    }

    #[test]
    fn no_runtime_directory_gives_no_folder() {
        assert!(Dir::under(None).is_none());
        assert!(Dir::under(Some("relative".into())).is_none());
        assert!(Dir::under(Some("/nonexistent/kanade/runtime".into())).is_none());
    }

    // the previews follow the history: removed, evicted and cleared entries lose theirs, a pinned
    // one keeps it through both
    #[test]
    fn previews_are_deleted_with_their_entries() {
        let runtime = temporary("deleted");
        let dir = Dir::under(Some(runtime.clone().into_os_string())).unwrap();
        let mut clipboard = Clipboard::default();

        let write_all = |clipboard: &Clipboard| {
            for entry in clipboard.entries() {
                if let Content::Image { mime, bytes } = &entry.content {
                    write(&dir, entry.id, mime, bytes);
                }
            }
        };

        for fill in 0..3 {
            clipboard.record(picture(fill));
        }

        write_all(&clipboard);
        assert_eq!(names(&dir.own), ["1.png", "2.png", "3.png"]);

        clipboard.remove(2);
        dir.retain(|id| clipboard.has(id));
        assert_eq!(names(&dir.own), ["1.png", "3.png"]);

        // pinned, entry 1 outlives the bound that evicts the rest
        assert_eq!(clipboard.pin(1), Pinning::Changed);

        for fill in 10..10 + u8::try_from(ENTRIES).unwrap() {
            clipboard.record(picture(fill));
        }

        write_all(&clipboard);
        dir.retain(|id| clipboard.has(id));

        assert!(!clipboard.has(3), "evicted");
        assert!(names(&dir.own).contains(&String::from("1.png")));
        assert!(!names(&dir.own).contains(&String::from("3.png")));

        clipboard.clear();
        dir.retain(|id| clipboard.has(id));
        assert_eq!(names(&dir.own), ["1.png"]);

        clipboard.remove(1);
        dir.retain(|id| clipboard.has(id));
        assert!(names(&dir.own).is_empty());

        let _ = fs::remove_dir_all(&runtime);
    }

    // a part an unfinished write holds is not another write's to delete; names that are not ours go
    #[test]
    fn a_stray_is_swept_but_not_a_write_in_flight() {
        let runtime = temporary("stray");
        let dir = Dir::under(Some(runtime.clone().into_os_string())).unwrap();

        write(&dir, 1, "image/png", b"x");
        fs::write(dir.own.join("notanid.png"), b"x").unwrap();
        fs::write(dir.own.join("2.exe"), b"x").unwrap();
        fs::write(dir.own.join(".3.png.1.1.part"), b"x").unwrap();

        dir.retain(|id| id == 1);
        assert_eq!(names(&dir.own), [".3.png.1.1.part", "1.png"]);

        // sweeping where nothing is there is not an error
        let _ = fs::remove_dir_all(&runtime);
        dir.sweep();
    }

    /*
     * one Kanade's start sweeps only the folders of the Kanades that ended: a second one running
     * beside it, or the one a restart is racing, keeps its previews. The folder is named by the pid,
     * and this one's own is taken, as a crash under a reused pid leaves it stale
     */
    #[test]
    fn a_sweep_leaves_a_running_kanades_previews_alone() {
        let runtime = temporary("sweep");
        let root = runtime.join(FOLDER);

        // a process this test keeps alive, standing in for the Kanade beside this one
        let mut running = process::Command::new("sleep").arg("30").spawn().unwrap();
        let live = running.id();

        // a child reaped, so its number is certainly not a live process's
        let mut done = process::Command::new("true").spawn().unwrap();
        let gone = done.id();
        done.wait().unwrap();

        let dir = Dir::of(root.clone(), process::id());
        let beside = Dir::of(root.clone(), live);
        let ended = Dir::of(root.clone(), gone);

        write(&beside, 1, "image/png", b"live");
        write(&ended, 1, "image/png", b"gone");
        write(&dir, 1, "image/png", b"stale");

        // an older Kanade's, written in the root before the folders were per process
        fs::write(root.join("4.png"), b"root").unwrap();
        fs::write(root.join("notes.txt"), b"not ours").unwrap();

        dir.sweep();

        assert_eq!(
            names(&beside.own),
            ["1.png"],
            "the running one keeps its own"
        );
        assert_eq!(fs::read(beside.own.join("1.png")).unwrap(), b"live");
        assert!(!ended.own.exists(), "the one that ended loses its folder");
        assert!(!dir.own.exists(), "this one's, left by a crash, goes too");
        assert!(
            !root.join("4.png").exists(),
            "an older Kanade's preview goes"
        );
        assert!(
            root.join("notes.txt").exists(),
            "a name that is not a preview is not ours to take"
        );
        assert_eq!(names(&root), [live.to_string(), String::from("notes.txt")]);

        drop(running.kill());
        drop(running.wait());

        // what it left now is a folder whose process is gone, so the next start takes it
        dir.sweep();
        assert_eq!(names(&root), ["notes.txt"]);

        let _ = fs::remove_dir_all(&runtime);
    }
}
