//! The files a calendar sync keeps in a directory only Kanade reads, as they hold the user's events.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;

use crate::atomic::{self, Mode};

/*
 * `files` in `dir`, in order, each written only when it changed; the `.ics` files of calendars no
 * longer in `files` deleted, and any part a crash left; true when it made `dir`
 */
pub fn sync(dir: &Path, files: &[(String, String)]) -> io::Result<bool> {
    let created = !dir.is_dir();

    DirBuilder::new().recursive(true).mode(0o700).create(dir)?;

    for (name, text) in files {
        let path = dir.join(name);
        if fs::read_to_string(&path).is_ok_and(|kept| kept == *text) {
            continue;
        }

        atomic::write(&path, text.as_bytes(), Mode::Private)?;
    }

    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let kept = files.iter().any(|(kept, _)| *name == **kept);
        let ours = name.ends_with(".ics") || name.ends_with(".part");

        if ours && !kept {
            fs::remove_file(path)?;
        }
    }

    Ok(created)
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::os::unix::fs::PermissionsExt;
    use std::process;

    use super::*;

    fn files(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(name, text)| (String::from(*name), String::from(*text)))
            .collect()
    }

    #[test]
    fn a_sync_writes_only_what_changed_and_deletes_what_went() {
        let dir = env::temp_dir().join(format!("kanade-mirror-{}", process::id()));
        let _ = fs::remove_dir_all(&dir);

        assert!(matches!(
            sync(&dir, &files(&[("a.ics", "A"), ("b.ics", "B")])),
            Ok(true)
        ));
        let mode = fs::metadata(&dir).map(|metadata| metadata.permissions().mode() & 0o777);
        assert_eq!(mode.ok(), Some(0o700));

        let written = fs::metadata(dir.join("a.ics")).and_then(|metadata| metadata.modified());
        fs::write(dir.join("notes.txt"), "mine").unwrap();
        fs::write(dir.join(".a.ics.1.0.part"), "half").unwrap();

        assert!(matches!(sync(&dir, &files(&[("a.ics", "A")])), Ok(false)));
        assert_eq!(
            fs::metadata(dir.join("a.ics"))
                .and_then(|metadata| metadata.modified())
                .ok(),
            written.ok()
        );
        assert!(!dir.join("b.ics").exists());
        assert!(!dir.join(".a.ics.1.0.part").exists());
        assert!(dir.join("notes.txt").exists());

        let _ = fs::remove_dir_all(&dir);
    }
}
