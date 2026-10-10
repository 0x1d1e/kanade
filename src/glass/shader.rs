//! The shader's file: the runtime reads shaders from files, so each shell writes its own.

use std::env;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

// how old a shader being written must be to have been left by a shell that ended writing it
const LEFT: Duration = Duration::from_secs(10);

// the shader's file, written once at start; the runtime reads shaders from files
pub fn shader() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();

    // locked while this shell runs, so another one starting leaves it be
    static HELD: OnceLock<File> = OnceLock::new();

    PATH.get_or_init(|| {
        let dir = env::var_os("XDG_RUNTIME_DIR")
            .map_or_else(env::temp_dir, PathBuf::from)
            .join("kanade");
        let path = dir.join(format!("glass-{}.wgsl", std::process::id()));

        // the runtime reads it whenever it builds a pipeline, so each shell keeps its own while it runs
        sweep(&dir);

        // locked under a name no sweep takes, then named, so none sweeps it before it is held
        let written = fs::create_dir_all(&dir).and_then(|()| {
            let writing = dir.join(format!(".glass-{}.writing", std::process::id()));
            let mut file = File::create(&writing)?;

            file.lock()?;
            file.write_all(include_str!("../glass.wgsl").as_bytes())?;
            fs::rename(&writing, &path)?;
            let _ = HELD.set(file);

            Ok(())
        });

        if let Err(error) = written {
            eprintln!("kanade: cannot write the glass shader ({error}), glass has no rim");
        }

        path
    })
}

/*
 * removes the shaders of shells no longer running, as none removes its own as it ends: those no
 * shell holds locked, whichever process namespace it runs in, and those one ended while writing
 */
fn sweep(dir: &Path) {
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let name = entry.file_name();
        let shader = name
            .to_str()
            .is_some_and(|name| name.starts_with("glass-") && name.ends_with(".wgsl"));

        // one being written may not be locked yet, while one left behind is long untouched
        let left = name
            .to_str()
            .is_some_and(|name| name.starts_with(".glass-") && name.ends_with(".writing"))
            && entry
                .metadata()
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified.elapsed().is_ok_and(|age| age > LEFT));

        if (shader || left) && File::open(entry.path()).is_ok_and(|file| file.try_lock().is_ok()) {
            let _ = fs::remove_file(entry.path());
        }
    }
}
