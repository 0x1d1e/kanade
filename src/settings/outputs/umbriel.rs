//! Umbriel's output IPC properties and validated config persistence.

use super::{Edit, RunError, Vrr};
use crate::sources::{json::Json, umbriel};
use crate::umbriel_config::{self, Editor};

pub const KEYS: &[&str] = &["enabled", "mode", "position", "scale", "transform", "vrr"];

pub fn editor() -> Result<Editor<'static>, String> {
    Ok(Editor {
        file: umbriel_config::target(None)?,
        state: umbriel_config::state()?,
        validate: &umbriel_config::validate_umbriel,
        now_ms: umbriel_config::now_ms,
    })
}

pub fn path(output: &str, key: &str) -> String {
    format!("output.{}.{key}", crate::sources::json::quote(output))
}

fn vrr(vrr: Vrr) -> &'static str {
    match vrr {
        Vrr::Off => "disabled",
        Vrr::On => "always",
        Vrr::OnDemand => "fullscreen",
    }
}

pub fn property(edit: &Edit) -> (&'static str, Json) {
    match edit {
        Edit::Enabled(value) => ("enabled", Json::from(*value)),
        Edit::Mode(mode) => ("mode", Json::from(mode.text())),
        Edit::Scale(value) => ("scale", Json::from(*value)),
        Edit::Transform(value) => ("transform", Json::from(value.name())),
        Edit::Vrr(value) => ("vrr", Json::from(vrr(*value))),
        Edit::Position(x, y) => ("position", serde_json::json!([x, y])),
    }
}

/// The controller's command vocabulary also describes its recovery plan.
pub fn properties(args: &[String]) -> Result<serde_json::Map<String, Json>, String> {
    let words: Vec<_> = args.iter().map(String::as_str).collect();
    let (key, value) = match words.as_slice() {
        ["on"] => ("enabled", Json::from(true)),
        ["off"] => ("enabled", Json::from(false)),
        ["mode", mode] => ("mode", Json::from(*mode)),
        ["scale", scale] => (
            "scale",
            Json::from(scale.parse::<f64>().map_err(|e| e.to_string())?),
        ),
        ["transform", transform] => ("transform", Json::from(*transform)),
        ["vrr", "off"] => ("vrr", Json::from("disabled")),
        ["vrr", "on"] => ("vrr", Json::from("always")),
        ["vrr", "--on-demand", "on"] => ("vrr", Json::from("fullscreen")),
        ["position", "set", "--", x, y] => (
            "position",
            serde_json::json!([
                x.parse::<i32>().map_err(|e| e.to_string())?,
                y.parse::<i32>().map_err(|e| e.to_string())?
            ]),
        ),
        _ => return Err("unsupported Umbriel output property".into()),
    };
    Ok([(key.to_owned(), value)].into_iter().collect())
}

pub fn run(output: &str, args: &[String]) -> Result<(), RunError> {
    let properties = properties(args).map_err(RunError::Refused)?;
    umbriel::set_output(output, &properties).map_err(|why| {
        if why.starts_with("umbriel rejected request:") {
            RunError::Refused(why)
        } else {
            RunError::Unanswered(why)
        }
    })
}

pub fn write(editor: &Editor, output: &str, edits: &[&Edit]) -> Result<String, String> {
    let changes: Vec<_> = edits
        .iter()
        .map(|edit| {
            let (key, value) = property(edit);
            (path(output, key), value.to_string())
        })
        .collect();
    editor.apply(
        &changes
            .iter()
            .map(|(path, value)| (path.as_str(), Some(value.as_str())))
            .collect::<Vec<_>>(),
    )
}

pub fn reload() -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut command = Command::new("umbriel");
    command.args(["msg", "config-reload"]).stdin(Stdio::null());
    match crate::appearance::child::run::<()>(
        command,
        "umbriel msg config-reload",
        std::time::Duration::from_secs(2),
        || None,
    ) {
        crate::appearance::child::Ran::Done(result) => result.map(|_| ()),
        crate::appearance::child::Ran::Superseded(()) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{Mode, Transform};
    use super::*;

    #[test]
    fn trial_and_config_use_the_same_properties() {
        for edit in [
            Edit::Enabled(false),
            Edit::Mode(Mode {
                width: 1920,
                height: 1080,
                refresh: 59940,
                preferred: true,
            }),
            Edit::Position(-1920, 10),
            Edit::Scale(1.25),
            Edit::Transform(Transform::R90),
            Edit::Vrr(Vrr::Off),
            Edit::Vrr(Vrr::On),
            Edit::Vrr(Vrr::OnDemand),
        ] {
            let (key, value) = property(&edit);
            assert_eq!(properties(&edit.args()).unwrap().get(key), Some(&value));
        }
    }

    #[test]
    fn config_round_trip_preserves_comments_and_undo_refuses_external_edits() {
        let dir =
            std::env::temp_dir().join(format!("kanade-umbriel-output-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.toml");
        let original = "# user's config\n[layout]\ngap = 8 # keep\n";
        std::fs::write(&file, original).unwrap();
        let editor = Editor {
            file: file.clone(),
            state: dir.join("state"),
            validate: &|file| {
                std::fs::read_to_string(file)
                    .unwrap()
                    .parse::<toml_edit::DocumentMut>()
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
            now_ms: umbriel_config::now_ms,
        };
        let name = "DP.1";
        write(
            &editor,
            name,
            &[
                &Edit::Scale(2.0),
                &Edit::Position(-100, 20),
                &Edit::Vrr(Vrr::OnDemand),
            ],
        )
        .unwrap();
        assert_eq!(editor.get(&path(name, "scale")).unwrap(), "2.0");
        assert_eq!(editor.get(&path(name, "position")).unwrap(), "[-100,20]");
        assert_eq!(editor.get(&path(name, "vrr")).unwrap(), "\"fullscreen\"");
        assert!(
            std::fs::read_to_string(&file)
                .unwrap()
                .contains("gap = 8 # keep")
        );
        editor.undo().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), original);
        write(&editor, name, &[&Edit::Enabled(false)]).unwrap();
        std::fs::write(
            &file,
            format!("{}\n# hand edit\n", std::fs::read_to_string(&file).unwrap()),
        )
        .unwrap();
        assert!(editor.undo().is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
