//! The settings file's writes (docs/design.md Config): one key changed at a time, over the file as
//! it stands, so what the window does not show, like an output's keys, stays. A value the layers
//! below already give removes the key instead, so the file holds only real overrides. The file is
//! written whole to a temporary file beside it and renamed over it, so a reader never sees half of
//! one; the reload watch follows the rename.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use toml::Value;

use crate::config::{self, Config};

// what the file starts with, as nobody should edit it by hand
const HEADER: &str =
    "# written by Kanade's Settings window; your own config goes in ~/.config/kanade/\n";

// one write at a time, so two never read the same file and lose one's change
static WRITING: Mutex<()> = Mutex::new(());

/*
 * sets the key at `path` to `value`, none removing it, and writes the file; says why it will not:
 * a value the key does not take, or a file it cannot read whole
 */
pub fn set(path: &[String], value: Option<Value>) -> Result<(), String> {
    let file = config::settings_file().ok_or("no home directory to keep settings in")?;
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);

    let text = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(format!("{} unreadable: {error}", file.display())),
    };

    let (below, _) = config::layers(false);
    let text = change(&text, &below, path, value)
        .map_err(|problem| format!("{} not written: {problem}", file.display()))?;

    write(&file, &text).map_err(|error| format!("{} not written: {error}", file.display()))
}

// the file's new text, with the key at `path` set over `below`, the layers under it, or removed
fn change(
    text: &str,
    below: &Config,
    path: &[String],
    value: Option<Value>,
) -> Result<String, String> {
    let mut table = config::settings_table(text)?;

    // checked alone over the layers below, so a problem elsewhere in the file is not this one's
    let value = match value {
        None => None,
        Some(value) => {
            let mut config = below.clone();

            if let Some(problem) = config::apply_table(&mut config, &nest(path, value.clone()))
                .into_iter()
                .next()
            {
                return Err(problem);
            }

            (config != *below).then_some(value)
        }
    };

    edit(&mut table, path, value);
    table.insert(
        String::from("schema_version"),
        Value::Integer(config::SCHEMA_VERSION as i64),
    );

    let body = toml::to_string(&table).map_err(|error| error.to_string())?;

    Ok(format!("{HEADER}{body}"))
}

// `value` alone at `path`, in the tables its key names
fn nest(path: &[String], value: Value) -> toml::Table {
    let mut value = value;

    for key in path[1..].iter().rev() {
        value = Value::Table(toml::Table::from_iter([(key.clone(), value)]));
    }

    toml::Table::from_iter([(path[0].clone(), value)])
}

// sets or removes the key at `path`; a table left empty by a removal goes too
fn edit(table: &mut toml::Table, path: &[String], value: Option<Value>) {
    let [key, rest @ ..] = path else {
        return;
    };

    if rest.is_empty() {
        match value {
            Some(value) => drop(table.insert(key.clone(), value)),
            None => drop(table.remove(key)),
        }

        return;
    }

    // a value where a table goes is replaced, as the key's table is what applies
    let inner = match table.get_mut(key) {
        Some(Value::Table(inner)) => inner,
        _ if value.is_none() => return,
        _ => {
            table.insert(key.clone(), Value::Table(toml::Table::new()));

            match table.get_mut(key) {
                Some(Value::Table(inner)) => inner,
                _ => return,
            }
        }
    };

    edit(inner, rest, value);

    if inner.is_empty() {
        table.remove(key);
    }
}

// the whole text to a file beside it, flushed to disk, then renamed over it
fn write(file: &Path, text: &str) -> io::Result<()> {
    let dir = file
        .parent()
        .ok_or_else(|| io::Error::other("the settings file has no directory"))?;
    fs::create_dir_all(dir)?;

    // hidden, so it is never read as a layer
    let temporary = dir.join(format!(".settings.toml.{}", std::process::id()));

    let written = File::create(&temporary).and_then(|mut out| {
        out.write_all(text.as_bytes())?;
        out.sync_all()
    });

    match written.and_then(|()| fs::rename(&temporary, file)) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&temporary);
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn path(key: &str) -> Vec<String> {
        key.split('.').map(str::to_owned).collect()
    }

    fn set(text: &str, below: &Config, key: &str, value: &str) -> Result<String, String> {
        change(
            text,
            below,
            &path(key),
            Some(value.parse::<Value>().unwrap()),
        )
    }

    fn body(text: &str) -> toml::Table {
        toml::from_str(text).unwrap()
    }

    #[test]
    fn a_value_is_written_with_the_layout() {
        let text = set("", &Config::default(), "clock", "\"12h\"").unwrap();

        assert!(text.starts_with(HEADER));
        assert_eq!(
            body(&text),
            body(&format!(
                "clock = \"12h\"\nschema_version = {}",
                config::SCHEMA_VERSION
            ))
        );
    }

    #[test]
    fn a_value_the_layers_below_give_removes_the_key() {
        let mut below = Config::default();
        below.island.hover = Duration::from_millis(300);

        let text = set(
            "[timings]\nhover = 100\ngrace = 10\n",
            &below,
            "timings.hover",
            "300",
        );
        assert_eq!(
            body(&text.unwrap()),
            body(&format!(
                "schema_version = {}\n[timings]\ngrace = 10",
                config::SCHEMA_VERSION
            ))
        );

        // the default too, and the table it leaves empty goes
        let text = set(
            "clock = \"12h\"\n[timings]\nhover = 100\n",
            &below,
            "timings.hover",
            "300",
        );
        assert_eq!(
            body(&text.unwrap()),
            body(&format!(
                "clock = \"12h\"\nschema_version = {}",
                config::SCHEMA_VERSION
            ))
        );
    }

    #[test]
    fn a_table_kind_changes_one_entry() {
        let text = set(
            "[modules]\nmedia = false\n",
            &Config::default(),
            "modules.timer",
            "false",
        )
        .unwrap();
        assert_eq!(
            body(&text)["modules"],
            "{ media = false, timer = false }".parse().unwrap()
        );

        // on is the default, so turning one back on removes it
        let text = set(&text, &Config::default(), "modules.media", "true").unwrap();
        assert_eq!(body(&text)["modules"], "{ timer = false }".parse().unwrap());

        let app = path("windows.apps");
        let text = change(
            "",
            &Config::default(),
            &[
                app[0].clone(),
                app[1].clone(),
                String::from("org.kde.dolphin"),
            ],
            Some(Value::String(String::from("dolphin"))),
        )
        .unwrap();
        assert_eq!(
            body(&text)["windows"],
            "{ apps = { \"org.kde.dolphin\" = \"dolphin\" } }"
                .parse()
                .unwrap()
        );
    }

    #[test]
    fn removing_keeps_the_rest() {
        let text = change(
            "clock = \"12h\"\nfuture = 1\n[output.\"eDP-1\"]\nclock = \"24h\"\n",
            &Config::default(),
            &path("clock"),
            None,
        )
        .unwrap();

        assert_eq!(
            body(&text),
            body(&format!(
                "future = 1\nschema_version = {}\n[output.\"eDP-1\"]\nclock = \"24h\"",
                config::SCHEMA_VERSION
            ))
        );
    }

    #[test]
    fn a_value_the_key_does_not_take_is_refused() {
        assert_eq!(
            set("", &Config::default(), "clock", "\"13h\""),
            Err(String::from(
                "clock: expected \"24h\" or \"12h\", found \"13h\""
            ))
        );
        assert_eq!(
            set("", &Config::default(), "timings.hover", "0"),
            Err(String::from("timings.hover: 0 is outside 1-60000 ms"))
        );
        assert!(set("", &Config::default(), "modules.island", "false").is_err());
    }

    // never rewritten from a file it cannot read whole, which would drop what it holds
    #[test]
    fn a_file_it_cannot_read_is_left_alone() {
        assert!(set("clock = ", &Config::default(), "clock", "\"12h\"").is_err());
        assert!(
            set(
                "schema_version = 99\nclock = \"12h\"\n",
                &Config::default(),
                "clock",
                "\"24h\""
            )
            .unwrap_err()
            .contains("newer")
        );
    }

    // an older file is written back in the current layout
    #[test]
    fn an_older_file_is_migrated() {
        let text = set(
            "[timings]\ntoast = 3000\nhover = 100\n",
            &Config::default(),
            "clock",
            "\"12h\"",
        )
        .unwrap();

        assert_eq!(
            body(&text),
            body(&format!(
                "clock = \"12h\"\nschema_version = {}\n[timings]\nhover = 100",
                config::SCHEMA_VERSION
            ))
        );
    }

    #[test]
    fn a_write_replaces_the_file_whole() {
        let dir = std::env::temp_dir().join(format!("kanade-settings-{}", std::process::id()));
        let file = dir.join("kanade/settings.toml");

        write(&file, "clock = \"12h\"\n").unwrap();
        write(&file, "clock = \"24h\"\n").unwrap();

        assert_eq!(fs::read_to_string(&file).unwrap(), "clock = \"24h\"\n");
        assert_eq!(fs::read_dir(file.parent().unwrap()).unwrap().count(), 1);

        fs::remove_dir_all(dir).unwrap();
    }
}
