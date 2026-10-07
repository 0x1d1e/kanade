//! Where the Controls Surface is for one visit: its top level or a sub-surface, and the keyboard's
//! ring on it. Written by input only, never by the view. Every target is a key away: the arrows
//! move the ring, Enter or Space presses it, Escape goes back a level, and levels move with Left and
//! Right.

use amane::{Key, Service};

use super::list;
use crate::sources::audio::Node;
use crate::sources::power::Profile;
use crate::sources::wifi::Secret;

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

// a place in a grid
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    row: usize,
    column: usize,
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
        let at = self.at.as_ref()?;

        grid.iter().enumerate().find_map(|(row, targets)| {
            let column = targets.iter().position(|(target, _)| target == at)?;

            Some(Place { row, column })
        })
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

/*
 * the place a key moves to, none for a key that does not move or at an edge. Up and Down go to the
 * target in the next row nearest across, Home and End to the first and last
 */
fn moved(place: Place, key: Key, grid: &[Vec<(At, f32)>]) -> Option<Place> {
    let across = grid[place.row][place.column].1;

    let nearest = |row: usize| {
        let column = grid[row]
            .iter()
            .enumerate()
            .min_by(|(_, (_, a)), (_, (_, b))| (a - across).abs().total_cmp(&(b - across).abs()))
            .map(|(column, _)| column)?;

        Some(Place { row, column })
    };

    let to = match key {
        Key::Up => nearest(place.row.checked_sub(1)?)?,
        Key::Down if place.row + 1 < grid.len() => nearest(place.row + 1)?,
        Key::Left => Place {
            column: place.column.checked_sub(1)?,
            ..place
        },
        Key::Right if place.column + 1 < grid[place.row].len() => Place {
            column: place.column + 1,
            ..place
        },
        Key::Home => Place { row: 0, column: 0 },
        Key::End => {
            let row = grid.len() - 1;

            Place {
                row,
                column: grid[row].len() - 1,
            }
        }
        _ => return None,
    };

    (to != place).then_some(to)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top() -> Vec<Vec<(At, f32)>> {
        grid(None, Vec::new())
    }

    fn networks(names: &[&str]) -> Vec<Vec<(At, f32)>> {
        names
            .iter()
            .map(|ssid| vec![(At::Network((*ssid).into()), 0.5)])
            .collect()
    }

    fn wifi() -> Vec<Vec<(At, f32)>> {
        grid(
            Some(&Subsurface::Wifi),
            networks(&["home", "cafe", "office"]),
        )
    }

    fn into_wifi() -> Focus {
        shown().enter(Subsurface::Wifi)
    }

    // a focus whose ring shows, as after the first key
    fn shown() -> Focus {
        Focus {
            shown: true,
            ..Focus::default()
        }
    }

    // the focus after each key, which every one must use
    fn keys(mut focus: Focus, keys: &[Key], grid: &[Vec<(At, f32)>]) -> Focus {
        for &key in keys {
            focus = focus.step(key, grid).expect("a key it uses").0;
        }

        focus
    }

    #[test]
    fn a_new_visit_starts_at_the_top_level() {
        let deep = Focus {
            visit: 1,
            ..into_wifi()
        };

        let next = deep.of(2, false);
        assert_eq!(next.sub, None);
        assert_eq!(next.at(&top()), Some(At::WifiSwitch));
        assert!(!next.shown);

        assert_eq!(deep.of(1, false).sub, Some(Subsurface::Wifi));
        assert!(Focus::default().of(3, true).shown);
    }

    // a visit that ended in the sub-surface, by the hold running out, must not leave its targets
    #[test]
    fn a_new_visit_walks_the_top_level_not_the_last_sub_surface() {
        let deep = Focus {
            visit: 1,
            ..into_wifi()
        };

        let next = deep.of(2, true);
        let grid = next.grid(networks(&["home"]));
        assert_eq!(grid, top());

        let (next, _) = next.step(Key::Right, &grid).unwrap();
        let (_, act) = next.step(Key::Enter, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Wifi)));
    }

    #[test]
    fn the_first_key_only_shows_the_ring() {
        let (focus, act) = Focus::default().step(Key::Enter, &top()).unwrap();

        assert!(focus.shown);
        assert_eq!(act, None);
        assert_eq!(focus.at(&top()), Some(At::WifiSwitch));
    }

    #[test]
    fn the_arrows_walk_the_top_level_by_rows() {
        let grid = top();

        let focus = keys(shown(), &[Key::Right], &grid);
        assert_eq!(focus.at(&grid), Some(At::Wifi));

        // down to the nearest across, then back up to it
        let focus = keys(focus, &[Key::Down], &grid);
        assert_eq!(focus.at(&grid), Some(At::Microphone));

        let focus = keys(focus, &[Key::Right, Key::Up], &grid);
        assert_eq!(focus.at(&grid), Some(At::Bluetooth));

        let focus = keys(focus, &[Key::End], &grid);
        assert_eq!(focus.at(&grid), Some(At::Profile(Profile::Performance)));

        let focus = keys(focus, &[Key::Home], &grid);
        assert_eq!(focus.at(&grid), Some(At::WifiSwitch));

        // an edge uses nothing, so the key goes elsewhere
        assert_eq!(focus.step(Key::Up, &grid), None);
    }

    #[test]
    fn enter_presses_and_left_and_right_move_a_level() {
        let grid = top();
        let focus = keys(shown(), &[Key::Down, Key::Down], &grid);
        assert_eq!(focus.at(&grid), Some(At::Speaker));

        let (_, act) = focus.clone().step(Key::Right, &grid).unwrap();
        assert_eq!(act, Some(Act::Adjust(At::Speaker, -1.0)));

        let (_, act) = focus.clone().step(Key::Left, &grid).unwrap();
        assert_eq!(act, Some(Act::Adjust(At::Speaker, 1.0)));

        let (_, act) = focus.step(Key::Space, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Speaker)));
    }

    // the speaker's chevron is in its row, but Left and Right there move the level, not the ring
    #[test]
    fn the_audio_chevron_is_down_from_dnd_and_left_and_right_stay_on_the_speaker() {
        let grid = top();
        let focus = keys(shown(), &[Key::Down, Key::Right, Key::Down], &grid);
        assert_eq!(focus.at(&grid), Some(At::Audio));

        let (speaker, act) = focus.clone().step(Key::Left, &grid).unwrap();
        assert_eq!(act, None);
        assert_eq!(speaker.at(&grid), Some(At::Speaker));

        for key in [Key::Left, Key::Right] {
            let (still, _) = speaker.clone().step(key, &grid).unwrap();
            assert_eq!(still.at(&grid), Some(At::Speaker));
        }

        let (_, act) = focus.step(Key::Enter, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Audio)));
    }

    #[test]
    fn the_audio_sub_surface_adjusts_each_level_and_escape_comes_back_to_its_chevron() {
        let rows = vec![
            vec![(At::Speaker, 0.5)],
            vec![(At::Output(Node::of(1)), 0.5)],
            vec![(At::MicrophoneLevel, 0.5)],
            vec![(At::Stream(Node::of(9)), 0.5)],
        ];
        let inside = shown().enter(Subsurface::Audio);
        let grid = inside.grid(rows);

        // it starts on the output's level, under a header with only its back chevron
        assert_eq!(grid[0], [(At::Back, 0.0)]);
        assert_eq!(inside.at(&grid), Some(At::Speaker));

        let app = keys(inside, &[Key::End], &grid);
        assert_eq!(app.at(&grid), Some(At::Stream(Node::of(9))));

        let (app, act) = app.step(Key::Right, &grid).unwrap();
        assert_eq!(act, Some(Act::Adjust(At::Stream(Node::of(9)), -1.0)));

        let (app, act) = app.step(Key::Space, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Stream(Node::of(9)))));

        let microphone = keys(app, &[Key::Up], &grid);
        let (microphone, act) = microphone.step(Key::Left, &grid).unwrap();
        assert_eq!(act, Some(Act::Adjust(At::MicrophoneLevel, 1.0)));

        // a device is pressed, not adjusted, and Left and Right go nowhere from it
        let output = keys(microphone, &[Key::Up], &grid);
        assert_eq!(output.clone().step(Key::Right, &grid), None);
        let (output, act) = output.step(Key::Enter, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Output(Node::of(1)))));

        let (back, _) = output.step(Key::Escape, &grid).unwrap();
        assert_eq!(back.sub, None);
        assert_eq!(back.at(&top()), Some(At::Audio));
    }

    #[test]
    fn the_wifi_tile_opens_wifi_and_escape_comes_back_to_it() {
        let focus = into_wifi();
        assert_eq!(focus.sub, Some(Subsurface::Wifi));

        // it starts on the first network
        assert_eq!(focus.at(&wifi()), Some(At::Network("home".into())));

        let (back, act) = focus.step(Key::Escape, &wifi()).unwrap();
        assert_eq!(act, None);
        assert_eq!(back.sub, None);
        assert_eq!(back.at(&top()), Some(At::Wifi));

        // Escape at the top level is the window's, which closes the island
        assert_eq!(back.step(Key::Escape, &top()), None);
    }

    #[test]
    fn the_bluetooth_tile_opens_bluetooth_and_escape_comes_back_to_it() {
        let grid = top();
        let focus = keys(shown(), &[Key::Right, Key::Right], &grid);
        assert_eq!(focus.at(&grid), Some(At::BluetoothSwitch));

        let focus = keys(focus, &[Key::Right], &grid);
        let (focus, act) = focus.step(Key::Enter, &grid).unwrap();
        assert_eq!(act, Some(Act::Press(At::Bluetooth)));

        let devices = vec![
            vec![
                (At::Device("/buds".into()), 0.62),
                (At::Forget("/buds".into()), 0.9),
            ],
            vec![(At::Device("/speaker".into()), 0.5)],
        ];
        let inside = focus.enter(Subsurface::Bluetooth);
        let grid = inside.grid(devices);
        assert_eq!(inside.at(&grid), Some(At::Device("/buds".into())));

        // down from Forget to the nearest across
        let inside = keys(inside, &[Key::Right, Key::Down], &grid);
        assert_eq!(inside.at(&grid), Some(At::Device("/speaker".into())));

        let (back, _) = inside.step(Key::Escape, &grid).unwrap();
        assert_eq!(back.sub, None);
        assert_eq!(back.at(&top()), Some(At::Bluetooth));
    }

    #[test]
    fn a_wifi_sub_surface_without_networks_starts_on_its_header() {
        let empty = grid(Some(&Subsurface::Wifi), Vec::new());

        assert_eq!(into_wifi().at(&empty), Some(At::Back));
    }

    #[test]
    fn the_ring_follows_a_network_that_moves() {
        let focus = keys(into_wifi(), &[Key::Down], &wifi());
        assert_eq!(focus.at(&wifi()), Some(At::Network("cafe".into())));

        let reordered = grid(
            Some(&Subsurface::Wifi),
            networks(&["cafe", "home", "office"]),
        );
        let (focus, act) = focus.step(Key::Enter, &reordered).unwrap();
        assert_eq!(act, Some(Act::Press(At::Network("cafe".into()))));

        // gone, the ring hides, and the next key puts it on the first without pressing that
        let gone = grid(Some(&Subsurface::Wifi), networks(&["home"]));
        assert_eq!(focus.ring(&gone), None);

        let (focus, act) = focus.step(Key::Enter, &gone).unwrap();
        assert_eq!(act, None);
        assert_eq!(focus.ring(&gone), Some(At::Network("home".into())));

        let (_, act) = focus.step(Key::Enter, &gone).unwrap();
        assert_eq!(act, Some(Act::Press(At::Network("home".into()))));
    }

    // a device forgotten under the ring must not have Enter press the one that took its row
    #[test]
    fn enter_on_a_device_gone_presses_nothing() {
        let row = |name: &str| {
            vec![
                (At::Device(name.into()), 0.4),
                (At::Forget(name.into()), 0.9),
            ]
        };
        let inside = shown().enter(Subsurface::Bluetooth);
        let both = inside.grid(vec![row("/old"), row("/new")]);

        let focus = keys(inside, &[Key::Right], &both);
        assert_eq!(focus.ring(&both), Some(At::Forget("/old".into())));

        let left = focus.grid(vec![row("/new")]);
        for key in [Key::Enter, Key::Space, Key::Up, Key::Right] {
            let (focus, act) = focus.clone().step(key, &left).unwrap();
            assert_eq!(act, None);

            // nor does it move off what it was not seen on
            assert_eq!(focus.ring(&left), Some(At::Device("/new".into())));
        }
    }

    #[test]
    fn the_header_is_up_from_the_networks() {
        let focus = keys(into_wifi(), &[Key::Up], &wifi());
        assert_eq!(focus.at(&wifi()), Some(At::Back));

        let focus = keys(focus, &[Key::Right], &wifi());
        assert_eq!(focus.at(&wifi()), Some(At::Radio));

        let (_, act) = focus.step(Key::Enter, &wifi()).unwrap();
        assert_eq!(act, Some(Act::Press(At::Radio)));
    }

    #[test]
    fn moving_down_the_networks_scrolls_them_into_view() {
        let many = (0..10)
            .map(|index| vec![(At::Network(format!("net{index}")), 0.5)])
            .collect();
        let grid = grid(Some(&Subsurface::Wifi), many);

        let focus = keys(into_wifi(), &[Key::End], &grid);
        assert_eq!(focus.at(&grid), Some(At::Network("net9".into())));
        assert_eq!(focus.offset, list::most(10));

        let focus = keys(focus, &[Key::Home], &grid);
        assert_eq!(focus.offset, 0.0);
    }

    #[test]
    fn a_password_is_typed_erased_and_joined_with_once_it_fits() {
        let focus = into_wifi().into_password("cafe");

        let focus = keys(
            focus,
            &"hunter2".chars().map(Key::Character).collect::<Vec<_>>(),
            &[],
        );

        // too short for WPA, so Enter joins nothing
        let (focus, act) = focus.step(Key::Enter, &[]).unwrap();
        assert_eq!(act, None);

        let focus = keys(
            focus,
            &[Key::Space, Key::Character('!'), Key::Backspace],
            &[],
        );
        let Some(Subsurface::Password { secret, .. }) = &focus.sub else {
            panic!("still typing");
        };
        assert_eq!(secret.len(), 8);

        let (_, act) = focus.step(Key::Enter, &[]).unwrap();
        assert_eq!(act, Some(Act::Join));
    }

    #[test]
    fn escape_from_a_password_drops_it_and_returns_to_its_network() {
        let focus = into_wifi().into_password("cafe");
        let focus = keys(focus, &[Key::Character('x')], &[]);

        let (back, _) = focus.step(Key::Escape, &[]).unwrap();
        assert_eq!(back.sub, Some(Subsurface::Wifi));
        assert_eq!(back.at(&wifi()), Some(At::Network("cafe".into())));

        // and out to the top level
        let (top, _) = back.step(Key::Escape, &wifi()).unwrap();
        assert_eq!(top.sub, None);
    }

    #[test]
    fn a_password_stops_at_64_characters() {
        let focus = into_wifi().into_password("cafe");
        let focus = keys(focus, &vec![Key::Character('a'); 70], &[]);

        let Some(Subsurface::Password { secret, .. }) = &focus.sub else {
            panic!("still typing");
        };
        assert_eq!(secret.len(), 64);
    }

    #[test]
    fn keys_a_password_does_not_use_go_elsewhere() {
        let focus = into_wifi().into_password("cafe");

        assert_eq!(focus.step(Key::Tab, &[]), None);
    }
}
