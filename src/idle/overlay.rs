//! The dim grace (ADR 0047): a layer over everything on each output, taking no input, that fades
//! toward dark from `idle.dim` until the lock, so a user still there sees it coming and moves.
//! Hidden, it attaches no buffer and draws nothing.

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::{
    Full, Keyboard, Layer, LayerWindow, Monitor, Parent, Rectangle, Zone, request_frame,
};

use crate::theme;

// how dark a dim with no lock after it gets, so the screen still reads under it
const PARTLY: f32 = 0.6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fade {
    from: Instant,
    until: Instant,

    // to full dark, as the lock comes at `until`
    dark: bool,
}

impl Fade {
    // how dark at `now`: slow to start, so the first seconds are a hint, not a blackout
    fn level(self, now: Instant) -> f32 {
        let length = self
            .until
            .saturating_duration_since(self.from)
            .as_secs_f32();
        let gone = now.saturating_duration_since(self.from).as_secs_f32();
        let done = if length > 0.0 {
            (gone / length).min(1.0)
        } else {
            1.0
        };

        let most = if self.dark { 1.0 } else { PARTLY };

        most * done * done
    }
}

// the fade under way, none while not dimmed
#[derive(Debug, Default)]
pub struct Dim {
    fade: Option<Fade>,
}

impl Service for Dim {
    fn new() -> Self {
        Self::default()
    }

    // written only by the idle thread
    fn listen() {}
}

pub fn dim(from: Instant, until: Instant, dark: bool) {
    Dim::write().fade = Some(Fade { from, until, dark });
}

pub fn undim() {
    let mut dim = Dim::write();

    if dim.fade.is_none() {
        dim.quiet();
    } else {
        dim.fade = None;
    }
}

pub fn overlay(_monitor: &Monitor) -> LayerWindow {
    let fade = Dim::read().fade;
    let now = Instant::now();

    // a frame at a time while it falls, none once dark
    if fade.is_some_and(|fade| now < fade.until) {
        request_frame();
    }

    let level = fade.map_or(0.0, |fade| fade.level(now));

    LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .keyboard(Keyboard::None)
        .namespace("kanade-idle-dim")
        .visible(fade.is_some())
        .click_through()
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(theme::IDLE_DIM)
                .opacity(level),
        )
}
