//! `kanade config schema|get|set|reset|validate <file>`: the config's keys from the command line,
//! all from the `Setting` registry, so a key added to a Module is here with no second list. They
//! run in this process, shell or none; a write goes to the settings file as the Settings window's
//! does (`settings::set`), never to the user's own files, and the shell's reload watch follows it.

use std::fs;
use std::path::Path;

use serde_json::{Map, Value as Json, json};
use toml::Value;

use crate::config::{self, Kind, Range, Setting};
use crate::modules;
use crate::settings;

// the JSON Schema of every config file, so an editor can check and complete one
pub fn schema() -> String {
    let defaults = config::Config::default();
    let keys = |output: bool| {
        let mut root = Map::new();

        for setting in config::settings().filter(|setting| !output || setting.per_output) {
            let mut at = &mut root;
            let mut path = setting.key.split('.').peekable();

            while let Some(key) = path.next() {
                if path.peek().is_none() {
                    at.insert(key.to_owned(), leaf(setting, &defaults));
                    break;
                }

                at = at
                    .entry(key)
                    .or_insert_with(|| json!({ "type": "object", "additionalProperties": false }))
                    .as_object_mut()
                    .expect("a section is an object")
                    .entry("properties")
                    .or_insert_with(|| json!({}))
                    .as_object_mut()
                    .expect("properties are an object");
            }
        }

        root
    };

    let mut properties = keys(false);

    properties.insert(
        String::from("schema_version"),
        json!({
            "type": "integer",
            "minimum": 1,
            "maximum": config::SCHEMA_VERSION,
            "description": "the layout the file is written in; without one, 1",
        }),
    );
    properties.insert(
        String::from("output"),
        json!({
            "type": "object",
            "description": "keys for the output of this name only, over the keys above from any file",
            "additionalProperties": {
                "type": "object",
                "properties": keys(true),
                "additionalProperties": false,
            },
        }),
    );

    let schema = json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Kanade config",
        "type": "object",
        "properties": properties,
        "additionalProperties": false,
    });

    format!("{schema:#}\n")
}

fn whole(range: Range) -> Json {
    json!({ "type": "integer", "minimum": range.least, "maximum": range.most })
}

// one key's schema: what it takes as its Kind checks it, its help and the default it has
fn leaf(setting: &Setting, defaults: &config::Config) -> Json {
    let mut leaf = match &setting.kind {
        Kind::Switch(_) => json!({ "type": "boolean" }),
        Kind::Millis(_) => whole(Range {
            least: config::SHORTEST as u32,
            most: config::LONGEST as u32,
        }),
        Kind::Pixels(range, _)
        | Kind::Percent(range, _)
        | Kind::Seconds(range, _)
        | Kind::Minutes(range, _)
        | Kind::Kelvin(range, _) => whole(*range),
        Kind::Count(count, _) => whole(count.range),
        Kind::Choice(choices, _) => json!({ "type": "string", "enum": choices.names }),
        Kind::Path(_) | Kind::Text(_) => json!({ "type": "string", "minLength": 1 }),
        Kind::Location(_) => json!({
            "type": "array",
            "prefixItems": [
                { "type": "number", "minimum": -90, "maximum": 90 },
                { "type": "number", "minimum": -180, "maximum": 180 },
            ],
            "minItems": 2,
            "maxItems": 2,
        }),
        Kind::DayTime(_) => json!({
            "type": "string",
            "pattern": "^([01][0-9]|2[0-3]):[0-5][0-9]$",
        }),
        Kind::DesktopIds(_) | Kind::Paths(_) | Kind::Urls(_) => {
            json!({ "type": "array", "items": { "type": "string", "minLength": 1 } })
        }
        Kind::Tiles(_) => {
            json!({ "type": "array", "minItems": 1, "items": { "type": "string", "minLength": 1 } })
        }
        Kind::Argv(_) => json!({ "type": "array", "items": { "type": "string" } }),
        Kind::Names(known, _) => {
            json!({ "type": "array", "uniqueItems": true, "items": { "enum": known } })
        }
        Kind::AppIds(_) | Kind::Gestures(_) => {
            json!({ "type": "object", "additionalProperties": { "type": "string", "minLength": 1 } })
        }
        Kind::Modules(_) => json!({
            "type": "object",
            "properties": modules::ALL
                .iter()
                .map(|module| (module.name.to_owned(), json!({ "type": "boolean" })))
                .collect::<Map<_, _>>(),
            "additionalProperties": false,
        }),
        Kind::Rules(_) => json!({ "type": "array", "items": { "type": "object" } }),
    };

    let mut description = setting.help.to_owned();
    if setting.restart {
        description.push_str("; takes a restart");
    }
    leaf["description"] = json!(description);

    if let Some(value) = setting.kind.value(defaults) {
        // a TOML value to JSON by its own serializer, which both write the same
        leaf["default"] = serde_json::to_value(&value).unwrap_or(Json::Null);
    }

    leaf
}

/*
 * the key as `key = value` and the layer it comes from, one per line: `# default` or the file
 * that set it; for a table, every file that adds to it
 */
pub fn get(key: &str) -> Result<String, String> {
    let setting = config::setting(key)?;
    let (config, _) = config::layers(true);
    let origin = config::origin(setting);

    let line = match setting.kind.value(&config) {
        Some(value) => format!("{key} = {value}"),
        None => format!("# {key} is not set"),
    };
    let from = match origin.as_slice() {
        [] => String::from("default"),
        [.., last] if !setting.kind.table() => last.display().to_string(),
        files => files
            .iter()
            .map(|file| file.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    };

    Ok(format!("{line}\n# from {from}\n"))
}

// a value as typed: a line of text is taken as it is, anything else read as TOML
fn parse(setting: &Setting, raw: &str) -> Result<Value, String> {
    let text = matches!(
        setting.kind,
        Kind::Choice(..) | Kind::Path(_) | Kind::Text(_) | Kind::DayTime(_)
    );

    if text && !raw.starts_with(['"', '\'']) {
        return Ok(Value::String(raw.to_owned()));
    }

    raw.parse::<Value>()
        .map_err(|_| format!("{}: expected a TOML value, found {raw}", setting.key))
}

// writes the key to the settings file, refusing what its Kind does not take with that message
pub fn set(key: &str, raw: &str) -> Result<String, String> {
    let setting = config::setting(key)?;

    // a command runs as the user: only a file the user writes holds one, never a call to the shell
    if matches!(setting.kind, Kind::Argv(_)) {
        return Err(format!(
            "{key} runs a command, so it is set in a file under the config directory, never here"
        ));
    }

    let value = parse(setting, raw)?;

    settings::set(&path(key), Some(value))?;

    after(setting)
}

// removes the key from the settings file, so the layers below give it again
pub fn reset(key: &str) -> Result<String, String> {
    let setting = config::setting(key)?;

    settings::set(&path(key), None)?;

    after(setting)
}

fn path(key: &str) -> Vec<&str> {
    key.split('.').collect()
}

// what the key comes to once written, and that it takes a restart to show
fn after(setting: &Setting) -> Result<String, String> {
    let mut said = get(setting.key)?;

    if setting.restart {
        said.push_str("# takes a restart\n");
    }

    Ok(said)
}

// what is wrong in a file read alone, as `file:line: what`, none for a file that applies whole
pub fn validate(file: &Path) -> Result<Vec<String>, String> {
    let text = fs::read_to_string(file).map_err(|error| format!("{}: {error}", file.display()))?;

    Ok(config::check(&text)
        .into_iter()
        .map(|(line, what)| format!("{}:{line}: {what}", file.display()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    // a command runs as the user, so no call through the shell or the settings file sets one
    #[test]
    fn a_command_is_refused_before_anything_is_written() {
        let commands: Vec<_> = config::settings()
            .filter(|setting| matches!(setting.kind, Kind::Argv(_)))
            .collect();

        assert!(!commands.is_empty());

        for setting in commands {
            let path: Vec<_> = setting.key.split('.').collect();

            assert!(set(setting.key, "[\"true\"]").is_err(), "{}", setting.key);
            assert!(settings::set(&path, None).is_err(), "{}", setting.key);
        }
    }

    // the schema at a dotted key under `root`
    fn at<'a>(root: &'a Json, key: &str) -> Option<&'a Json> {
        key.split('.')
            .try_fold(root, |at, key| at.get("properties")?.get(key))
    }

    // every key of the registry is in the schema at its path, with what its Kind takes, and under
    // `output` when it is per output
    #[test]
    fn the_schema_has_every_key_of_the_registry() {
        let schema: Json = serde_json::from_str(&schema()).expect("JSON");
        let output = &schema["properties"]["output"]["additionalProperties"];

        for setting in config::settings() {
            let found = at(&schema, setting.key)
                .unwrap_or_else(|| panic!("{} is not in the schema", setting.key));

            assert!(found.get("type").is_some(), "{}", setting.key);
            assert_eq!(
                at(output, setting.key).is_some(),
                setting.per_output,
                "{}",
                setting.key
            );

            if let Kind::Choice(choices, _) = &setting.kind {
                assert_eq!(found["enum"], json!(choices.names), "{}", setting.key);
            }
        }
    }

    // text is taken as typed, the rest as TOML
    #[test]
    fn a_value_is_text_for_a_text_key_and_toml_for_the_rest() {
        let key = |name| config::setting(name).expect("a key");

        assert_eq!(parse(key("clock"), "12h").unwrap(), Value::from("12h"));
        assert_eq!(parse(key("clock"), "\"12h\"").unwrap(), Value::from("12h"));
        assert_eq!(
            parse(key("timings.hover"), "300").unwrap(),
            Value::from(300)
        );
        assert!(parse(key("timings.hover"), "fast").is_err());
    }
}
