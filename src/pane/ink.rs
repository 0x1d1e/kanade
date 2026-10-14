//! The ink a Liquid Glass pane's content is drawn in, which follows how bright the screen around the
//! pane is (ADR 0054): light over a dark page, dark (paper's) over a bright one, faded from one to the
//! other. Until the ink flips, a scrim darkens the glass as the page nears the flip.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use kanade_runtime::MutexExt;

use super::Spot;
use crate::config;
use crate::island::motion::{Mode, Spring};

// the light of the page, 0 to 1, from which the ink turns dark, and below which it turns light again
const TO_PAPER: f32 = 0.65;
const TO_LIGHT: f32 = 0.55;

// how long the ink takes to turn, at the default animation speed
const FADE: Duration = Duration::from_millis(250);

// how black the scrim is at most, and over which light it grows, the top being where the ink flips
const DIM: f32 = 0.25;
const DIM_FROM: f32 = 0.35;

struct Ink {
    // the ink the pane is heading to
    paper: bool,

    // how paper it was when the current leg began, so a change of mind fades on from there
    faded: f32,

    presence: Spring<1>,
}

impl Ink {
    fn new(mode: Mode) -> Self {
        Self {
            paper: false,
            faded: 0.0,
            presence: Spring::new([0.0], mode),
        }
    }

    // 0 light ink, 1 dark
    fn at(&self, now: Instant) -> f32 {
        let goal = if self.paper { 1.0 } else { 0.0 };

        self.faded + (goal - self.faded) * self.presence.progress(now)
    }

    // a page in between keeps the ink it has, so one near the line does not flicker
    fn follow(&mut self, light: f32, fade: Duration, now: Instant) {
        let paper = if light >= TO_PAPER {
            true
        } else if light <= TO_LIGHT {
            false
        } else {
            self.paper
        };

        if paper != self.paper {
            self.faded = self.at(now);
            self.paper = paper;
            self.presence.to([f32::from(u8::from(paper))], fade, now);
        }
    }
}

static INKS: Mutex<Option<HashMap<Spot, Ink>>> = Mutex::new(None);

/*
 * how dark a pane's ink is, 0 to 1, with the page around it of `light`, and whether it still turns;
 * called each time the pane draws
 */
pub(super) fn read(spot: &Spot, light: f32, now: Instant) -> (f32, bool) {
    let config = config::get();
    let fade = config::scale_animation(FADE, config.animation_speed);

    let mut inks = INKS.locked();
    let ink = inks
        .get_or_insert_default()
        .entry(spot.clone())
        .or_insert_with(|| Ink::new(config.island.motion));

    ink.follow(light, fade, now);

    (ink.at(now), !ink.presence.settled(now))
}

// a pane with no copy has no ink of its own: it starts light again once it has one
pub(super) fn forget(spot: &Spot) {
    if let Some(inks) = INKS.locked().as_mut() {
        inks.remove(spot);
    }
}

// how black the glass is under light ink: more as the page nears the flip, none once the ink is dark
pub(super) fn scrim(light: f32, paper: f32) -> f32 {
    let x = ((light - DIM_FROM) / (TO_PAPER - DIM_FROM)).clamp(0.0, 1.0);

    DIM * x * x * (3.0 - 2.0 * x) * (1.0 - paper)
}
