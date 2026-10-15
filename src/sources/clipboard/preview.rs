//! The previews of the clipboard's pictures (ADR 0114): the runtime draws images from files, so a
//! picture's bytes are written, only once a row that shows it is built, to `$XDG_RUNTIME_DIR/
//! kanade-clipboard/<id>.<ext>`, a tmpfs, owner-only (the directory 0700, a file 0600). Never anywhere
//! else: with no runtime directory there is no preview. A preview is deleted with its entry, whether
//! removed, evicted or cleared, and what a Kanade that ended left is swept at start. Its name is the
//! entry's id and an extension from a fixed table of types, nothing the copy itself says. The bytes
//! and the paths are never logged.

use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, LazyLock, Mutex, OnceLock};

use kanade_runtime::{MutexExt, service::Service};

use super::{Clipboard, Content, Entry};
use crate::atomic::{self, Mode};
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
struct Dir(PathBuf);

impl Dir {
    // the folder under the runtime directory `runtime`, if it is an absolute path to a directory
    fn under(runtime: Option<OsString>) -> Option<Dir> {
        let runtime = PathBuf::from(runtime?);

        (runtime.is_absolute() && runtime.is_dir()).then(|| Dir(runtime.join(FOLDER)))
    }

    // the preview of the entry `id`: its id and the extension of its type, if it has one
    fn path(&self, id: u64, mime: &str) -> Option<PathBuf> {
        Some(self.0.join(format!("{id}.{}", extension(mime)?)))
    }

    // writes the bytes whole and private, the folder made 0700 when missing
    fn write(&self, id: u64, mime: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        let path = self
            .path(id, mime)
            .ok_or_else(|| io::Error::from(io::ErrorKind::Unsupported))?;

        atomic::write(&path, bytes, Mode::Private)?;

        Ok(path)
    }

    // deletes every preview whose entry `live` does not know. A part an unfinished write left is not
    // taken, it is finished or removed by that write, or swept at start
    fn retain(&self, live: impl Fn(u64) -> bool) {
        let Ok(files) = fs::read_dir(&self.0) else {
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

    // deletes the folder and what is in it
    fn sweep(&self) {
        let _ = fs::remove_dir_all(&self.0);
    }
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

    // not written, and not tried again until the entry is gone
    failed: HashSet<u64>,
    jobs: Option<Sender<Job>>,
}

static WORK: LazyLock<Mutex<Work>> = LazyLock::new(Mutex::default);

// writing and deleting take turns, so a preview is never deleted as it is written
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

    if work.failed.contains(&entry.id) || !work.pending.insert(entry.id) {
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

// writes one preview and says it is ready, unless its entry went meanwhile
fn make(Job { id, mime, bytes }: Job) {
    let Some(dir) = dir() else { return };

    {
        let _turn = TURN.locked();

        if Clipboard::read().has(id) {
            match dir.write(id, &mime, &bytes) {
                Ok(path) => {
                    Previews::write().ready.insert(id, path);
                }
                Err(error) => {
                    // the cause only, never the path or the picture
                    eprintln!(
                        "kanade: cannot write a clipboard preview ({})",
                        error.kind()
                    );
                    WORK.locked().failed.insert(id);
                }
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

    let _turn = TURN.locked();

    let live: HashSet<u64> = Clipboard::read()
        .entries()
        .iter()
        .map(|entry| entry.id)
        .collect();

    dir.retain(|id| live.contains(&id));

    let stale = Previews::read().ready.keys().any(|id| !live.contains(id));

    if stale {
        Previews::write().ready.retain(|id, _| live.contains(id));
    }

    WORK.locked().failed.retain(|id| live.contains(id));
}

// at start: nothing a Kanade before it left on tmpfs stays
pub fn sweep() {
    if let Some(dir) = dir() {
        dir.sweep();
    }
}

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

    fn names(dir: &Dir) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&dir.0)
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
            assert_eq!(dir.path(7, mime), Some(dir.0.join(name)), "{mime}");
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

        let path = dir.write(7, "image/png", b"bytes").unwrap();

        assert_eq!(path, runtime.join(FOLDER).join("7.png"));
        assert_eq!(fs::read(&path).unwrap(), b"bytes");
        assert_eq!(bits(&path), 0o600);
        assert_eq!(bits(&dir.0), 0o700);
        assert!(dir.write(8, "image/bmp", b"x").is_err());
        assert_eq!(names(&dir), ["7.png"]);

        let _ = fs::remove_dir_all(&runtime);
    }

    // a preview whose file is gone, deleted by another Kanade's start or by hand, is missing again
    #[test]
    fn a_ready_preview_whose_file_is_gone_is_missing() {
        let runtime = temporary("gone");
        let dir = Dir::under(Some(runtime.clone().into_os_string())).unwrap();
        let path = dir.write(7, "image/png", b"x").unwrap();

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
                    dir.write(entry.id, mime, bytes).unwrap();
                }
            }
        };

        for fill in 0..3 {
            clipboard.record(picture(fill));
        }

        write_all(&clipboard);
        assert_eq!(names(&dir), ["1.png", "2.png", "3.png"]);

        clipboard.remove(2);
        dir.retain(|id| clipboard.has(id));
        assert_eq!(names(&dir), ["1.png", "3.png"]);

        // pinned, entry 1 outlives the bound that evicts the rest
        assert_eq!(clipboard.pin(1), Pinning::Changed);

        for fill in 10..10 + u8::try_from(ENTRIES).unwrap() {
            clipboard.record(picture(fill));
        }

        write_all(&clipboard);
        dir.retain(|id| clipboard.has(id));

        assert!(!clipboard.has(3), "evicted");
        assert!(names(&dir).contains(&String::from("1.png")));
        assert!(!names(&dir).contains(&String::from("3.png")));

        clipboard.clear();
        dir.retain(|id| clipboard.has(id));
        assert_eq!(names(&dir), ["1.png"]);

        clipboard.remove(1);
        dir.retain(|id| clipboard.has(id));
        assert!(names(&dir).is_empty());

        let _ = fs::remove_dir_all(&runtime);
    }

    // a part an unfinished write holds is not another write's to delete; names that are not ours go
    #[test]
    fn a_stray_is_swept_but_not_a_write_in_flight() {
        let runtime = temporary("stray");
        let dir = Dir::under(Some(runtime.clone().into_os_string())).unwrap();

        dir.write(1, "image/png", b"x").unwrap();
        fs::write(dir.0.join("notanid.png"), b"x").unwrap();
        fs::write(dir.0.join("2.exe"), b"x").unwrap();
        fs::write(dir.0.join(".3.png.1.1.part"), b"x").unwrap();

        dir.retain(|id| id == 1);
        assert_eq!(names(&dir), [".3.png.1.1.part", "1.png"]);

        dir.sweep();
        assert!(!dir.0.exists());

        // sweeping what is not there is not an error
        dir.sweep();

        let _ = fs::remove_dir_all(&runtime);
    }
}
