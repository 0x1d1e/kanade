//! The recorded gestures: `$XDG_STATE_HOME/kanade/gestures.toml`, readable by its owner only
//! (ADR 0106). Each gesture is its name and the strokes it was recorded with, three of them,
//! normalized (`recognizer::normalize`) and written as flat `x, y, x, y, ...` lists. What a
//! gesture does lives in the config (`gestures.<name>`), never here.
//!
//! The file is outside input, as a user or a sync tool may edit it: it is read whole or refused
//! with the reason, and a refused file is never written over.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::atomic::{self, Mode};
use crate::config;

use super::recognizer::{Point, SAMPLES, Template};

// the gestures kept, and the strokes a gesture is recorded with
pub const MOST: usize = 64;
pub const STROKES: usize = 3;

// a name is what `kanade gesture record` and the config's `gestures` table are keyed by
const LONGEST_NAME: usize = 32;

// the file's version, so a later layout can be told from this one
const VERSION: i64 = 1;

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= LONGEST_NAME
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub const NAME_RULE: &str = "1 to 32 letters, digits, - or _";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Store(Vec<Template>);

#[derive(Serialize, Deserialize)]
struct File {
    version: i64,
    #[serde(default)]
    gesture: Vec<Entry>,
}

#[derive(Serialize, Deserialize)]
struct Entry {
    name: String,
    strokes: Vec<Vec<f64>>,
}

impl Store {
    pub fn templates(&self) -> &[Template] {
        &self.0
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|template| template.name.as_str())
    }

    // the gesture recorded again replaces the one of its name
    pub fn record(&mut self, name: &str, strokes: Vec<Vec<Point>>) -> Result<(), String> {
        if !valid_name(name) {
            return Err(format!("a name is {NAME_RULE}"));
        }

        if strokes.len() != STROKES || strokes.iter().any(|stroke| stroke.len() != SAMPLES) {
            return Err(String::from("a gesture is recorded with three strokes"));
        }

        self.0.retain(|template| template.name != name);

        if self.0.len() >= MOST {
            return Err(format!("at most {MOST} gestures; forget one first"));
        }

        self.0.push(Template {
            name: name.to_owned(),
            strokes,
        });
        self.0.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(())
    }

    // whether there was one
    pub fn forget(&mut self, name: &str) -> bool {
        let before = self.0.len();

        self.0.retain(|template| template.name != name);

        self.0.len() != before
    }

    pub fn parse(text: &str) -> Result<Store, String> {
        let file: File = toml::from_str(text).map_err(|error| error.message().to_owned())?;

        if file.version != VERSION {
            return Err(format!(
                "version {}, which this Kanade does not read",
                file.version
            ));
        }

        if file.gesture.len() > MOST {
            return Err(format!("more than {MOST} gestures"));
        }

        let mut templates: BTreeMap<String, Template> = BTreeMap::new();

        for entry in file.gesture {
            if !valid_name(&entry.name) {
                return Err(format!("a gesture's name is {NAME_RULE}"));
            }

            if entry.strokes.is_empty() || entry.strokes.len() > STROKES {
                return Err(format!("{}: expected 1 to {STROKES} strokes", entry.name));
            }

            let strokes = entry
                .strokes
                .iter()
                .map(|flat| {
                    stroke(flat)
                        .ok_or_else(|| format!("{}: a stroke is not {SAMPLES} points", entry.name))
                })
                .collect::<Result<Vec<_>, _>>()?;

            let template = Template {
                name: entry.name.clone(),
                strokes,
            };

            if templates.insert(entry.name.clone(), template).is_some() {
                return Err(format!("{}: named twice", entry.name));
            }
        }

        Ok(Store(templates.into_values().collect()))
    }

    pub fn text(&self) -> String {
        let file = File {
            version: VERSION,
            gesture: self
                .0
                .iter()
                .map(|template| Entry {
                    name: template.name.clone(),
                    strokes: template
                        .strokes
                        .iter()
                        .map(|stroke| {
                            stroke
                                .iter()
                                .flat_map(|point| [rounded(point.x), rounded(point.y)])
                                .collect()
                        })
                        .collect(),
                })
                .collect(),
        };

        toml::to_string(&file).unwrap_or_default()
    }
}

// four places are far finer than a stroke of unit reach is drawn
fn rounded(value: f32) -> f64 {
    (f64::from(value) * 1e4).round() / 1e4
}

// a flat list of `SAMPLES` points, every number finite and within a few units of the origin
fn stroke(flat: &[f64]) -> Option<Vec<Point>> {
    if flat.len() != 2 * SAMPLES || flat.iter().any(|v| !v.is_finite() || v.abs() > 4.0) {
        return None;
    }

    Some(
        flat.as_chunks::<2>()
            .0
            .iter()
            .map(|[x, y]| Point::new(*x as f32, *y as f32))
            .collect(),
    )
}

fn file() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").ok();

    config::base("XDG_STATE_HOME", ".local/state", home.as_deref())
        .map(|dir| dir.join("kanade/gestures.toml"))
        .ok_or_else(|| String::from("neither XDG_STATE_HOME nor HOME is set"))
}

// the recorded gestures; none when there is no file yet, the reason when it does not read
pub fn load() -> Result<Store, String> {
    let file = file()?;

    match fs::read_to_string(&file) {
        Ok(text) => Store::parse(&text).map_err(|why| format!("{}: {why}", file.display())),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Store::default()),
        Err(error) => Err(format!("{}: {error}", file.display())),
    }
}

pub fn save(store: &Store) -> Result<(), String> {
    let file = file()?;

    atomic::write(&file, store.text().as_bytes(), Mode::Private)
        .map_err(|error| format!("{}: {error}", file.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke_of(seed: f32) -> Vec<Point> {
        (0..SAMPLES)
            .map(|at| {
                let t = at as f32 / SAMPLES as f32;

                Point::new(seed * (t - 0.5), 0.3 * (seed * t).sin())
            })
            .collect()
    }

    fn recorded() -> Store {
        let mut store = Store::default();

        store
            .record(
                "swipe",
                vec![stroke_of(1.0), stroke_of(1.1), stroke_of(0.9)],
            )
            .unwrap();
        store
            .record("arc", vec![stroke_of(2.0), stroke_of(2.1), stroke_of(1.9)])
            .unwrap();

        store
    }

    #[test]
    fn a_store_survives_the_file() {
        let store = recorded();
        let read = Store::parse(&store.text()).unwrap();

        assert_eq!(read.names().collect::<Vec<_>>(), ["arc", "swipe"]);

        // to the places the file keeps
        for (written, read) in store.templates().iter().zip(read.templates()) {
            assert_eq!(written.name, read.name);

            for (a, b) in written
                .strokes
                .iter()
                .flatten()
                .zip(read.strokes.iter().flatten())
            {
                assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn recording_again_replaces_and_forgetting_removes() {
        let mut store = recorded();

        store
            .record(
                "swipe",
                vec![stroke_of(3.0), stroke_of(3.0), stroke_of(3.0)],
            )
            .unwrap();
        assert_eq!(store.templates().len(), 2);

        assert!(store.forget("swipe"));
        assert!(!store.forget("swipe"));
        assert_eq!(store.names().collect::<Vec<_>>(), ["arc"]);
    }

    #[test]
    fn a_name_and_the_strokes_are_checked_before_they_are_kept() {
        let mut store = Store::default();
        let strokes = || vec![stroke_of(1.0), stroke_of(1.0), stroke_of(1.0)];

        for name in ["", "a b", "a/b", "é", &"x".repeat(33)] {
            assert!(store.record(name, strokes()).is_err(), "{name:?}");
        }

        assert!(store.record("ok", vec![stroke_of(1.0)]).is_err());
        assert!(store.record("ok", vec![vec![Point::default()]; 3]).is_err());
        assert!(store.templates().is_empty());
    }

    #[test]
    fn a_file_that_does_not_read_is_refused_whole() {
        let good = recorded().text();
        let one_stroke: String = {
            let flat = vec!["0.0"; 2 * SAMPLES].join(", ");
            format!("version = 1\n[[gesture]]\nname = \"a\"\nstrokes = [[{flat}]]\n")
        };

        assert!(Store::parse(&one_stroke).is_ok());
        assert!(Store::parse("").is_err());
        assert!(Store::parse("version = 2\n").is_err());
        assert!(Store::parse("not toml [").is_err());
        assert!(Store::parse("version = 1\n[[gesture]]\nname = \"a\"\nstrokes = []\n").is_err());
        assert!(
            Store::parse("version = 1\n[[gesture]]\nname = \"a b\"\nstrokes = [[1.0]]\n").is_err()
        );
        assert!(Store::parse(&one_stroke.replace("0.0, 0.0", "0.0")).is_err());
        assert!(Store::parse(&one_stroke.replace("0.0", "1e30")).is_err());
        assert!(Store::parse(&one_stroke.replace("0.0", "nan")).is_err());
        assert!(Store::parse(&format!("{good}{good}")).is_err());

        // named twice
        let twice = format!("{one_stroke}{}", one_stroke.replace("version = 1\n", ""));
        assert!(Store::parse(&twice).is_err());
    }
}
