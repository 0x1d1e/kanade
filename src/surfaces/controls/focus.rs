//! Where the Controls Surface is for one visit: its top level or a sub-surface, and the keyboard's
//! ring on it. Written by input only, never by the view. Every target is a key away: the arrows
//! move the ring, Enter or Space presses it, Escape goes back a level, and levels move with Left and
//! Right.

use kanade_runtime::Key;
use kanade_runtime::service::Service;

use super::list;
use crate::sources::audio::Node;
use crate::sources::power::Profile;
use crate::sources::wifi::Secret;
use crate::surfaces::grid::{Place, find, moved};

// a password is at most 64 characters, the WPA key itself as hex
const MOST_SECRET: usize = 64;

// what shows in place of the top level, entered from one of its tiles
#[derive(Debug, Clone, PartialEq)]
pub enum Subsurface {
    // the networks in range
    Wifi,

    // a password being typed to join `ssid`, one of the networks
    Password { ssid: String, secret: Secret },

    // the Bluetooth devices, paired and nearby
    Bluetooth,

    // the sound devices and the apps playing or recording
    Audio,
}

// what the ring can be on, by what it is rather than where, so a network moving keeps it
#[derive(Debug, Clone, PartialEq)]
pub enum At {
    // the top level: a radio's switch's knob, then the rest of its tile, which opens its sub-surface
    WifiSwitch,
    Wifi,
    BluetoothSwitch,
    Bluetooth,
    Microphone,
    Dnd,
    Speaker,

    // the chevron at the end of the speaker's level, which opens the Audio sub-surface
    Audio,

    Brightness,
    Profile(Profile),

    // a listing sub-surface's back chevron and switch
    Back,
    Radio,

    // the Wi-Fi sub-surface's networks, by name
    Network(String),

    // the Bluetooth sub-surface's devices, by BlueZ's path: the device, then its Forget pill
    Device(String),
    Forget(String),

    // the code a pairing shows: it matches, or the pairing stops
    Confirm,
    Cancel,

    /*
     * the Audio sub-surface's: the default microphone's level, as `Speaker` is the default
     * speaker's, the devices to make the default, and the apps' levels
     */
    MicrophoneLevel,
    Output(Node),
    Input(Node),
    Stream(Node),
}

impl At {
    // Left and Right move it, rather than the ring
    fn level(&self) -> bool {
        matches!(
            self,
            At::Speaker | At::Brightness | At::MicrophoneLevel | At::Stream(_)
        )
    }
}

// what a key asks for beyond moving
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    Press(At),

    // a level moved by wheel lines, positive lower, as `Slider::wheel` takes them
    Adjust(At, f32),

    // the password typed, to join with
    Join,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Focus {
    visit: u64,

    // none is the top level
    pub sub: Option<Subsurface>,

    // none is the level's first target
    at: Option<At>,

    // the ring shows: from the start when opened from the keyboard, else from the first key
    pub shown: bool,

    // how far a listing sub-surface's rows scrolled, in pixels
    pub offset: f32,
}

impl Service for Focus {
    fn new() -> Self {
        Focus::default()
    }

    fn listen() {}
}

/*
 * a level's targets, row by row, each with where its middle is across the Surface from 0 to 1, so
 * Up and Down go to the nearest one. `rows` are a listing sub-surface's, top first, under its header
 */
fn grid(sub: Option<&Subsurface>, rows: Vec<Vec<(At, f32)>>) -> Vec<Vec<(At, f32)>> {
    match sub {
        None => vec![
            vec![
                (At::WifiSwitch, 0.08),
                (At::Wifi, 0.3),
                (At::BluetoothSwitch, 0.58),
                (At::Bluetooth, 0.8),
            ],
            vec![(At::Microphone, 0.25), (At::Dnd, 0.75)],
            vec![(At::Speaker, 0.45), (At::Audio, 0.97)],
            vec![(At::Brightness, 0.5)],
            Profile::ALL
                .into_iter()
                .zip(0..)
                .map(|(profile, index)| {
                    let middle = (index as f32 + 0.5) / Profile::ALL.len() as f32;

                    (At::Profile(profile), middle)
                })
                .collect(),
        ],
        Some(Subsurface::Wifi | Subsurface::Bluetooth) => {
            let mut grid = vec![vec![(At::Back, 0.0), (At::Radio, 1.0)]];

            grid.extend(rows);
            grid
        }
        // no radio to switch
        Some(Subsurface::Audio) => {
            let mut grid = vec![vec![(At::Back, 0.0)]];

            grid.extend(rows);
            grid
        }
        // typing has no ring
        Some(Subsurface::Password { .. }) => Vec::new(),
    }
}

impl Focus {
    /*
     * the targets of the level this focus is on. Taken from the focus itself, never a stored one,
     * which may be an earlier visit's sub-surface while this visit starts at the top level
     */
    pub fn grid(&self, rows: Vec<Vec<(At, f32)>>) -> Vec<Vec<(At, f32)>> {
        grid(self.sub.as_ref(), rows)
    }

    // a listing sub-surface: its header, then rows that scroll
    pub fn listing(&self) -> bool {
        matches!(
            self.sub,
            Some(Subsurface::Wifi | Subsurface::Bluetooth | Subsurface::Audio)
        )
    }

    // this visit's focus; one kept from an earlier visit is over
    pub fn of(&self, visit: u64, held: bool) -> Focus {
        if self.visit == visit {
            self.clone()
        } else {
            Focus {
                visit,
                shown: held,
                ..Focus::default()
            }
        }
    }

    /*
     * where the ring is in `grid`: on its target, or the level's first one when that is gone or
     * none was chosen. A listing sub-surface starts on its first row, as that is what it is for
     */
    fn place(&self, grid: &[Vec<(At, f32)>]) -> Option<Place> {
        let first = if self.listing() && grid.len() > 1 {
            Place { row: 1, column: 0 }
        } else {
            Place { row: 0, column: 0 }
        };

        self.found(grid)
            .or_else(|| (!grid.is_empty()).then_some(first))
    }

    // where its target is in `grid`, none when it has none or that is gone
    fn found(&self, grid: &[Vec<(At, f32)>]) -> Option<Place> {
        find(grid, self.at.as_ref()?)
    }

    /*
     * its target gone from `grid`, as a network or device leaves while the ring is on it. The ring
     * hides until a key puts it on what took its place, so nothing is pressed it was not seen on
     */
    fn lost(&self, grid: &[Vec<(At, f32)>]) -> bool {
        self.at.is_some() && self.found(grid).is_none()
    }

    // what the ring is on in `grid`, none on a level without one
    pub fn at(&self, grid: &[Vec<(At, f32)>]) -> Option<At> {
        self.place(grid)
            .map(|place| grid[place.row][place.column].0.clone())
    }

    // what the ring shows on in `grid`, none while it hides
    pub fn ring(&self, grid: &[Vec<(At, f32)>]) -> Option<At> {
        (self.shown && !self.lost(grid))
            .then(|| self.at(grid))
            .flatten()
    }

    /*
     * the row of `grid` the ring shows in, none while it hides or on a level without one, so a
     * listing scrolls to show it
     */
    pub fn row(&self, grid: &[Vec<(At, f32)>]) -> Option<usize> {
        (self.shown && !self.lost(grid))
            .then(|| self.place(grid))
            .flatten()
            .map(|place| place.row)
    }

    // into a listing sub-surface, at its top
    pub fn enter(self, sub: Subsurface) -> Focus {
        Focus {
            sub: Some(sub),
            at: None,
            offset: 0.0,
            ..self
        }
    }

    // into the password for `ssid`, typed from nothing
    pub fn into_password(self, ssid: &str) -> Focus {
        Focus {
            sub: Some(Subsurface::Password {
                ssid: ssid.to_owned(),
                secret: Secret::default(),
            }),
            at: Some(At::Network(ssid.to_owned())),
            ..self
        }
    }

    // a level up, the ring on what entered it; a typed password is dropped
    pub fn out(self) -> Focus {
        match &self.sub {
            None => self,
            Some(Subsurface::Wifi) => Focus {
                sub: None,
                at: Some(At::Wifi),
                offset: 0.0,
                ..self
            },
            Some(Subsurface::Bluetooth) => Focus {
                sub: None,
                at: Some(At::Bluetooth),
                offset: 0.0,
                ..self
            },
            Some(Subsurface::Audio) => Focus {
                sub: None,
                at: Some(At::Audio),
                offset: 0.0,
                ..self
            },
            Some(Subsurface::Password { ssid, .. }) => Focus {
                at: Some(At::Network(ssid.clone())),
                sub: Some(Subsurface::Wifi),
                ..self
            },
        }
    }

    /*
     * the focus after a key and what it asks for, none when the key is not for the Surface. On
     * a level with a ring, a key while the ring hides only shows it where it is, so nothing is
     * pressed that the ring was not seen on
     */
    pub fn step(self, key: Key, grid: &[Vec<(At, f32)>]) -> Option<(Focus, Option<Act>)> {
        if let Some(Subsurface::Password { .. }) = self.sub {
            return self.typed(key);
        }

        if key == Key::Escape {
            return self.sub.is_some().then(|| (self.out(), None));
        }

        let place = self.place(grid)?;
        let at = grid[place.row][place.column].0.clone();
        let level = at.level();

        let act = match key {
            Key::Enter | Key::Space => Some(Act::Press(at.clone())),
            Key::Left if level => Some(Act::Adjust(at.clone(), 1.0)),
            Key::Right if level => Some(Act::Adjust(at.clone(), -1.0)),
            _ => None,
        };

        // on a level, Left and Right only move it
        let moved = match key {
            Key::Left | Key::Right if level => None,
            _ => moved(place, key, grid),
        };

        if act.is_none() && moved.is_none() {
            return None;
        }

        if !self.shown || self.lost(grid) {
            return Some((
                Focus {
                    shown: true,
                    at: Some(at),
                    ..self
                },
                None,
            ));
        }

        let mut focus = self;

        if let Some(moved) = moved {
            focus.at = Some(grid[moved.row][moved.column].0.clone());

            // the list is the rows after the header
            if focus.listing() {
                let count = grid.len() - 1;
                let shown = moved.row.saturating_sub(1);

                focus.offset = list::reveal(focus.offset, shown, count);
            }
        }

        Some((focus, act))
    }

    // a key on the password: typing it, joining with it, or going back
    fn typed(mut self, key: Key) -> Option<(Focus, Option<Act>)> {
        let Some(Subsurface::Password { secret, .. }) = &mut self.sub else {
            return None;
        };

        let act = match key {
            Key::Character(letter) if secret.len() < MOST_SECRET => {
                secret.push(letter);
                None
            }
            Key::Space if secret.len() < MOST_SECRET => {
                secret.push(' ');
                None
            }
            Key::Character(_) | Key::Space => None,
            Key::Backspace => {
                secret.pop();
                None
            }
            Key::Enter => secret.fits().then_some(Act::Join),
            Key::Escape => return Some((self.out(), None)),
            _ => return None,
        };

        Some((self, act))
    }
}
