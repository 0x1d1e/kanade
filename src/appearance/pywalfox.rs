//! Pywalfox's native messaging host, so Firefox and Zen take the wallpaper colors live (ADR 0041).
//!
//! The browser starts `kanade <manifest> pywalfox@frewacom.org` once per profile. The host answers
//! the extension from the colors file the appearance worker writes, and pushes it again whenever
//! the worker replaces it.

use std::ffi::OsStr;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::thread;

use inotify::{Inotify, WatchMask};
use serde_json::{Value as Json, json};

pub const EXTENSION: &str = "pywalfox@frewacom.org";
pub(super) const DESCRIPTION: &str = "Kanade wallpaper colors for Pywalfox";
// the extension asks for an update below 2.7, the version that answers with the wallpaper
const VERSION: &str = "2.7";
const MESSAGE_LIMIT: u32 = 1024 * 1024;
const COLORS_FILE: &str = "pywalfox.json";

pub(super) fn colors_path() -> PathBuf {
    super::adapters::state_dir().join(COLORS_FILE)
}

pub(super) fn manifest_path() -> Option<PathBuf> {
    super::adapters::home_dir()
        .map(|home| home.join(".mozilla/native-messaging-hosts/pywalfox.json"))
}

pub(super) fn manifest(host: &Path) -> Json {
    json!({
        "name": "pywalfox",
        "description": DESCRIPTION,
        "path": host,
        "type": "stdio",
        "allowed_extensions": [EXTENSION],
    })
}

// whether a manifest at the host path is Kanade's, so it may be replaced or removed
pub(super) fn owns_manifest(text: &str) -> bool {
    serde_json::from_str::<Json>(text).is_ok_and(|manifest| {
        manifest.get("description").and_then(Json::as_str) == Some(DESCRIPTION)
    })
}

pub fn host() -> ExitCode {
    thread::spawn(push_changes);

    let mut input = io::stdin().lock();
    loop {
        match read_message(&mut input) {
            Ok(Some(message)) => {
                let action = message.get("action").and_then(Json::as_str).unwrap_or("");
                if let Err(error) = send(&reply(action)) {
                    eprintln!("kanade: Pywalfox host could not answer: {error}");
                    return ExitCode::FAILURE;
                }
            }
            Ok(None) => return ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("kanade: Pywalfox host could not read the browser: {error}");
                return ExitCode::FAILURE;
            }
        }
    }
}

fn reply(action: &str) -> Json {
    match action {
        "debug:version" => json!({"action": action, "success": true, "data": VERSION}),
        "action:colors" => colors(),
        "css:enable" | "css:disable" | "css:font:size" => json!({
            "action": action,
            "success": false,
            "error": "Kanade does not change browser CSS",
        }),
        _ => json!({"action": "action:invalid", "success": false}),
    }
}

fn colors() -> Json {
    match fs::read(colors_path()).map(|bytes| serde_json::from_slice::<Json>(&bytes)) {
        Ok(Ok(file)) => json!({
            "action": "action:colors",
            "success": true,
            "data": {"colors": file["colors"], "wallpaper": file["wallpaper"]},
        }),
        _ => json!({
            "action": "action:colors",
            "success": false,
            "error": "no wallpaper colors: turn on appearance.sync in Kanade",
        }),
    }
}

fn theme_mode() -> Option<Json> {
    let file: Json = serde_json::from_slice(&fs::read(colors_path()).ok()?).ok()?;
    let tone = file.get("tone")?.as_str()?;
    Some(json!({"action": "theme:mode", "success": true, "data": tone}))
}

// pushes the colors and tone each time the worker replaces the file, until the browser goes
fn push_changes() {
    let path = colors_path();
    let Some(directory) = path.parent() else {
        return;
    };
    let watch = (|| {
        fs::create_dir_all(directory)?;
        let inotify = Inotify::init()?;
        inotify
            .watches()
            .add(directory, WatchMask::CLOSE_WRITE | WatchMask::MOVED_TO)?;
        io::Result::Ok(inotify)
    })();
    let mut inotify = match watch {
        Ok(inotify) => inotify,
        Err(error) => {
            eprintln!("kanade: Pywalfox host cannot follow color changes: {error}");
            return;
        }
    };

    let mut buffer = [0; 4096];
    let mut sent = fs::read(&path).ok();
    loop {
        let Ok(events) = inotify.read_events_blocking(&mut buffer) else {
            return;
        };
        if !events
            .into_iter()
            .any(|event| event.name == Some(OsStr::new(COLORS_FILE)))
        {
            continue;
        }

        let current = fs::read(&path).ok();
        if current.is_none() || current == sent {
            continue;
        }
        sent = current;

        let pushed = send(&colors()).and_then(|()| theme_mode().map_or(Ok(()), |mode| send(&mode)));
        if pushed.is_err() {
            return;
        }
    }
}

fn send(message: &Json) -> io::Result<()> {
    let mut output = io::stdout().lock();
    write_message(&mut output, message)?;
    output.flush()
}

// one native message: its length as a native-endian u32, then that much JSON
fn read_message(input: &mut impl Read) -> io::Result<Option<Json>> {
    let mut length = [0; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }

    let length = u32::from_ne_bytes(length);
    if length > MESSAGE_LIMIT {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("message of {length} bytes is over the limit"),
        ));
    }

    let mut bytes = vec![0; length as usize];
    input.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn write_message(output: &mut impl Write, message: &Json) -> io::Result<()> {
    let bytes = serde_json::to_vec(message).map_err(io::Error::other)?;
    let length = u32::try_from(bytes.len())
        .ok()
        .filter(|length| *length <= MESSAGE_LIMIT)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "message is over the limit"))?;
    output.write_all(&length.to_ne_bytes())?;
    output.write_all(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_messages_round_trip_and_reject_oversized_or_cut_input() {
        let mut stream = Vec::new();
        write_message(&mut stream, &json!({"action": "debug:version"})).unwrap();
        write_message(&mut stream, &json!({"action": "action:colors"})).unwrap();

        let mut input = stream.as_slice();
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some(json!({"action": "debug:version"}))
        );
        assert_eq!(
            read_message(&mut input).unwrap(),
            Some(json!({"action": "action:colors"}))
        );
        assert_eq!(read_message(&mut input).unwrap(), None);

        let oversized = (MESSAGE_LIMIT + 1).to_ne_bytes();
        assert!(read_message(&mut oversized.as_slice()).is_err());

        let cut = [&8u32.to_ne_bytes()[..], b"{}"].concat();
        assert!(read_message(&mut cut.as_slice()).is_err());
    }

    #[test]
    fn only_kanade_manifests_count_as_owned() {
        let ours = manifest(Path::new("/usr/bin/kanade")).to_string();
        assert!(owns_manifest(&ours));
        let pywalfox = r#"{"name":"pywalfox","description":"Automatically theme your browser using the colors generated by Pywal","path":"/usr/bin/pywalfox","type":"stdio","allowed_extensions":["pywalfox@frewacom.org"]}"#;
        assert!(!owns_manifest(pywalfox));
        assert!(!owns_manifest("not json"));
    }
}
