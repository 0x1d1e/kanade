//! The settings file's writes (docs/design.md Config): a few keys changed at once, over the file as
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
const HEADER: &str = "# written by Kanade's Settings window and `kanade module`; your own config \
                      goes in ~/.config/kanade/\n";

// one write at a time, so two never read the same file and lose one's change
static WRITING: Mutex<()> = Mutex::new(());

// a key's path of tables and what the file holds there, none for nothing
pub type Entry = (Vec<String>, Option<Value>);

// what a write did to one key: what the file held there before, then after
#[derive(Debug)]
pub struct Replaced {
    pub path: Vec<String>,
    pub was: Option<Value>,
    pub now: Option<Value>,
}

/*
 * sets the key at `path` to `value`, none removing it, and writes the file; says why it will not:
 * a value the key does not take, or a file it cannot read whole
 */
pub fn set(path: &[String], value: Option<Value>) -> Result<(), String> {
    set_all(vec![(path.to_vec(), value)], &[])
        .map(drop)
        .map_err(|refusal| refusal.why)
}

// why a write was refused; `stale` if the file holds other than the write expected, which no retry mends
pub struct Refusal {
    pub why: String,
    pub stale: bool,
}

impl Refusal {
    fn failed(why: impl Into<String>) -> Self {
        Self {
            why: why.into(),
            stale: false,
        }
    }
}

/*
 * sets each key, in one write or none, if the file still holds each of `expected`; says what each
 * held before and holds now
 */
pub fn set_all(edits: Vec<Entry>, expected: &[Entry]) -> Result<Vec<Replaced>, Refusal> {
    let file = config::settings_file()
        .ok_or_else(|| Refusal::failed("no home directory to keep settings in"))?;
    let (below, _) = config::layers(false);

    set_in(&file, &below, edits, expected)
}

// `set_all` on the file at `file`, over `below`, the layers under it
fn set_in(
    file: &Path,
    below: &Config,
    edits: Vec<Entry>,
    expected: &[Entry],
) -> Result<Vec<Replaced>, Refusal> {
    let _writing = WRITING.lock().unwrap_or_else(PoisonError::into_inner);

    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(Refusal::failed(format!(
                "{} unreadable: {error}",
                file.display()
            )));
        }
    };

    holds(&text, expected).map_err(|why| Refusal { why, stale: true })?;

    let (new, replaced) = change(&text, below, edits)
        .map_err(|problem| Refusal::failed(format!("{} not written: {problem}", file.display())))?;

    if replaced.iter().any(|each| each.was != each.now) {
        write(file, &new)
            .map_err(|error| Refusal::failed(format!("{} not written: {error}", file.display())))?;
    }

    Ok(replaced)
}

// what `table` holds at `path`
fn at<'a>(table: &'a toml::Table, path: &[String]) -> Option<&'a Value> {
    let [first, rest @ ..] = path else {
        return None;
    };

    rest.iter()
        .try_fold(table.get(first)?, |value, key| value.get(key))
}

// refused if the file holds other than `expected`; a file that does not read is `change`'s to refuse
fn holds(text: &str, expected: &[Entry]) -> Result<(), String> {
    let Ok(table) = config::settings_table(text) else {
        return Ok(());
    };

    match expected
        .iter()
        .find(|(path, value)| at(&table, path) != value.as_ref())
    {
        Some((path, _)) => Err(format!("{} changed since; left as it is", path.join("."))),
        None => Ok(()),
    }
}

// the file's new text, with each key set over `below`, the layers under it, or removed, and what each held before and after
fn change(
    text: &str,
    below: &Config,
    edits: Vec<Entry>,
) -> Result<(String, Vec<Replaced>), String> {
    let mut table = config::settings_table(text)?;

    let mut replaced = Vec::new();
    for (path, value) in edits {
        // checked alone over the layers below, so a problem elsewhere in the file is not this one's
        let value = match value {
            None => None,
            Some(value) => {
                let mut config = below.clone();

                if let Some(problem) = config::apply_table(&mut config, &nest(&path, value.clone()))
                    .into_iter()
                    .next()
                {
                    return Err(problem);
                }

                (config != *below).then_some(value)
            }
        };

        let was = at(&table, &path).cloned();
        edit(&mut table, &path, value)?;
        let now = at(&table, &path).cloned();

        replaced.push(Replaced { path, was, now });
    }

    // the file as a whole must still apply, so a write never keeps or adds a key that does not
    if let Some(problem) = config::apply_table(&mut below.clone(), &table)
        .into_iter()
        .next()
    {
        return Err(format!("{problem}; fix or reset it first"));
    }

    table.insert(
        String::from("schema_version"),
        Value::Integer(config::SCHEMA_VERSION as i64),
    );

    let body = toml::to_string(&table).map_err(|error| error.to_string())?;

    Ok((format!("{HEADER}{body}"), replaced))
}

// `value` alone at `path`, in the tables its key names
fn nest(path: &[String], value: Value) -> toml::Table {
    let mut value = value;

    for key in path[1..].iter().rev() {
        value = Value::Table(toml::Table::from_iter([(key.clone(), value)]));
    }

    toml::Table::from_iter([(path[0].clone(), value)])
}

/*
 * sets or removes the key at `path`; a table left empty by a removal goes too. A value where a
 * table goes is refused, not replaced, so a write never drops what the file held
 */
fn edit(table: &mut toml::Table, path: &[String], value: Option<Value>) -> Result<(), String> {
    let [key, rest @ ..] = path else {
        return Ok(());
    };

    if rest.is_empty() {
        match value {
            Some(value) => drop(table.insert(key.clone(), value)),
            None => drop(table.remove(key)),
        }

        return Ok(());
    }

    let inner = match table.get_mut(key) {
        Some(Value::Table(inner)) => inner,
        Some(_) => return Err(format!("{key}: expected a table; fix it first")),
        None if value.is_none() => return Ok(()),
        None => table
            .entry(key.clone())
            .or_insert_with(|| Value::Table(toml::Table::new()))
            .as_table_mut()
            .ok_or_else(|| format!("{key}: expected a table"))?,
    };

    edit(inner, rest, value)?;

    if inner.is_empty() {
        table.remove(key);
    }

    Ok(())
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
            vec![(path(key), Some(value.parse::<Value>().unwrap()))],
        )
        .map(|(text, _)| text)
    }

    fn change_one(
        text: &str,
        below: &Config,
        path: &[String],
        value: Option<Value>,
    ) -> Result<String, String> {
        change(text, below, vec![(path.to_vec(), value)]).map(|(text, _)| text)
    }

    fn set_one(
        file: &Path,
        below: &Config,
        path: &[String],
        value: Option<Value>,
    ) -> Result<Vec<Replaced>, String> {
        set_in(file, below, vec![(path.to_vec(), value)], &[]).map_err(|refusal| refusal.why)
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
        let text = change_one(
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
        let text = change_one(
            "clock = \"12h\"\nreduced_motion = true\n[output.\"eDP-1\"]\nclock = \"24h\"\n",
            &Config::default(),
            &path("clock"),
            None,
        )
        .unwrap();

        assert_eq!(
            body(&text),
            body(&format!(
                "reduced_motion = true\nschema_version = {}\n[output.\"eDP-1\"]\nclock = \"24h\"",
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

    // a value where a table goes, or any key the file holds that does not apply, is kept as it is
    #[test]
    fn a_file_that_does_not_apply_is_left_unchanged() {
        let dir = std::env::temp_dir().join(format!("kanade-invalid-{}", std::process::id()));
        let file = dir.join("settings.toml");
        let apps = [path("windows.apps"), vec![String::from("kitty")]].concat();

        for text in ["windows = \"invalid\"\n", "clock = \"13h\"\n"] {
            write(&file, text).unwrap();

            let refused = set_one(
                &file,
                &Config::default(),
                &apps,
                Some(Value::String(String::from("kitty"))),
            );

            assert!(refused.unwrap_err().contains("fix"));
            assert_eq!(fs::read_to_string(&file).unwrap(), text);
        }

        // resetting the key that does not apply is what fixes it
        set_one(&file, &Config::default(), &path("clock"), None).unwrap();
        assert!(!fs::read_to_string(&file).unwrap().contains("13h"));

        fs::remove_dir_all(dir).unwrap();
    }

    // keys set together are written together or not at all, each saying what it replaced
    #[test]
    fn several_keys_are_one_write() {
        let text = "clock = \"12h\"\n";
        let value = |text: &str| Some(text.parse::<Value>().unwrap());

        let (new, replaced) = change(
            text,
            &Config::default(),
            vec![
                (path("clock"), None),
                (path("island.align"), value("\"right\"")),
            ],
        )
        .unwrap();

        assert!(!new.contains("12h") && new.contains("right"));
        assert_eq!(replaced[0].was, value("\"12h\""));
        assert_eq!(replaced[0].now, None);
        assert_eq!(replaced[1].was, None);
        assert_eq!(replaced[1].now, value("\"right\""));

        let refused = change(
            text,
            &Config::default(),
            vec![
                (path("clock"), None),
                (path("island.align"), value("\"up\"")),
            ],
        );
        assert!(refused.is_err());
    }

    // a write that expects what the file no longer holds, as an Undo after another edit, is refused
    #[test]
    fn a_write_expecting_another_value_is_refused() {
        let text = "clock = \"24h\"\n";
        let expected = [(path("clock"), Some(Value::String(String::from("12h"))))];

        assert!(
            holds(text, &expected)
                .unwrap_err()
                .contains("changed since")
        );

        let expected = [(path("clock"), Some(Value::String(String::from("24h"))))];
        assert!(holds(text, &expected).is_ok());
    }

    // only a file edited since is stale; one that cannot be written is a failure a retry may mend
    #[test]
    fn only_a_changed_file_is_a_stale_refusal() {
        let dir = std::env::temp_dir().join(format!("kanade-refusal-{}", std::process::id()));
        let file = dir.join("settings.toml");
        let edits = || vec![(path("clock"), Some(Value::String(String::from("12h"))))];
        let expected = [(path("clock"), Some(Value::String(String::from("24h"))))];

        fs::create_dir_all(&dir).unwrap();
        fs::write(&file, "clock = \"24h\"\n").unwrap();
        assert!(set_in(&file, &Config::default(), edits(), &expected).is_ok());

        let stale = set_in(&file, &Config::default(), edits(), &expected).err();
        assert!(stale.is_some_and(|refusal| refusal.stale));

        // a directory where the file should be
        fs::remove_file(&file).unwrap();
        fs::create_dir(&file).unwrap();
        let failed = set_in(&file, &Config::default(), edits(), &[]).err();
        assert!(failed.is_some_and(|refusal| !refusal.stale));

        fs::remove_dir_all(dir).unwrap();
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
