//! Settings' place finder for the Weather location (ADR 0136): a name typed as "City, Country"
//! is searched through Open-Meteo's geocoding, and "Use my location" asks ipwho.is where the IP
//! address is. Each runs on a worker when the user presses it and puts its result in the Service;
//! a place that is settled, by the text or by being picked, is written to `weather.location` and
//! `weather.place` in the settings file like any other change, so Undo takes it back and the
//! reload watch starts the forecast.

use std::cell::Cell;
use std::thread;

use kanade_runtime::TextInput;
use kanade_runtime::service::Service;
use toml::Value;

use crate::config;
use crate::sources::geocode::{self, Place};

use super::{Settings, file, input, path, refresh};

// the text input that takes the name
pub const INPUT: &str = "settings weather place search";

// what the finder is doing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Busy {
    Searching,
    Locating,
}

// what the last press ended with
#[derive(Debug, Clone, PartialEq)]
pub enum Said {
    // the location now written, by this label and from the address when it says so
    Set { label: String, from_address: bool },

    // why not, with what to do next
    Failed(String),
}

#[derive(Debug, Default)]
pub struct Finder {
    pub busy: Option<Busy>,

    // several matches, to pick from
    pub results: Vec<Place>,
    pub said: Option<Said>,

    // counts the locations written here, which the inputs of the two keys follow
    applied: u64,
}

/*
 * searches `typed` on a worker, if none runs: one match, or the one that "Name, Country" names,
 * is set at once; several are listed to pick from
 */
pub fn search(typed: &str) {
    if !begin(Busy::Searching) {
        return;
    }

    let typed = typed.to_owned();
    run(move || match geocode::search(&typed) {
        Ok(found) => match geocode::decisive(&found, &typed) {
            Some(index) => apply(&found[index], false),
            None => {
                let mut settings = Settings::write();
                settings.place.busy = None;
                settings.place.results = found;
            }
        },
        Err(why) => failed(why),
    });
}

// asks where the IP address is, on a worker, if none runs, and sets it. Only from a press
pub fn locate() {
    if !begin(Busy::Locating) {
        return;
    }

    run(|| match geocode::locate() {
        Ok(place) => apply(&place, true),
        Err(why) => failed(why),
    });
}

// sets a place picked from the results; on the draw thread, from a press
pub fn pick(place: &Place) {
    let place = place.clone();
    run(move || apply(&place, false));
}

// marks the finder busy, unless it is already; the last answer is dropped
fn begin(busy: Busy) -> bool {
    let mut settings = Settings::write();
    if settings.place.busy.is_some() {
        return false;
    }

    settings.place.busy = Some(busy);
    settings.place.results.clear();
    settings.place.said = None;

    true
}

fn run(job: impl FnOnce() + Send + 'static) {
    let worker = thread::Builder::new()
        .name(String::from("kanade-place"))
        .spawn(job);

    if let Err(error) = worker {
        failed(format!("Could not start the lookup: {error}"));
    }
}

fn failed(why: String) {
    let mut settings = Settings::write();
    settings.place.busy = None;
    settings.place.said = Some(Said::Failed(why));
}

// writes the place, as a change in the window does, off the draw thread
fn apply(place: &Place, from_address: bool) {
    let edits = vec![
        (path("weather.location", None), Some(place.location.value())),
        (
            path("weather.place", None),
            Some(Value::String(place.name.clone())),
        ),
    ];

    match file::set_all(edits, &[]) {
        Ok(replaced) => {
            let changed = replaced.iter().any(|each| each.was != each.now);
            refresh();

            let mut settings = Settings::write();
            settings.refused = None;
            if changed {
                settings.history.push(replaced);
            }
            settings.place.busy = None;
            settings.place.results.clear();
            settings.place.applied += 1;
            settings.place.said = Some(Said::Set {
                label: place.label(),
                from_address,
            });
        }
        Err(refusal) => failed(format!("Not saved: {}", refusal.why)),
    }
}

thread_local! {
    static LOOKED: Cell<u64> = const { Cell::new(0) };
}

/// Sets the location and place inputs to what the files give after the finder wrote them, which
/// the worker could not do: the inputs live on the draw thread. The view calls it before it builds
/// them.
pub fn sync_inputs(settings: &Settings) {
    if LOOKED.get() == settings.place.applied {
        return;
    }
    LOOKED.set(settings.place.applied);

    let read = &settings.read;
    for (key, text) in [
        (
            "weather.location",
            read.location
                .map(|at| format!("{}, {}", at.latitude, at.longitude))
                .unwrap_or_default(),
        ),
        ("weather.place", read.place.clone().unwrap_or_default()),
    ] {
        if let Ok(setting) = config::setting(key) {
            TextInput::set_text(input(setting), &text);
        }
    }
}
