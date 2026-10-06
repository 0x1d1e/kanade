//! The OSD (ADR 0007): a volume, brightness or microphone change, bottom centre on the focused
//! output, in its own Overlay window per monitor, so no fullscreen window covers it. `sources::osd`
//! reads the levels and shows each change here; a change while the OSD shows takes its place and
//! starts its time again, so a held key extends one OSD. It takes no pointer.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::{LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

use amane::{
    Center, End, Horizontal, Layer, LayerWindow, Margin, Monitor, Padding, Rectangle, Row, Service,
    Stack, Start, Text, Vertical, Widget, Zone, children,
};

use crate::config;
use crate::icon::Icon;
use crate::island::activity::{Device, Volume};
use crate::island::geometry::Rect;
use crate::shadow::{self, ShadowStyle};
use crate::theme::{self, ThemeRoles};
use crate::view;

const WIDTH: f32 = 280.0;
const HEIGHT: f32 = 48.0;
const INSET: f32 = 18.0;
const ICON: f32 = 22.0;
const GAP: f32 = 12.0;

// wide enough for 100, so a level that moves slides the bar and nothing else
const NUMBER: f32 = 30.0;

// from the monitor's bottom edge
const BOTTOM: f32 = 72.0;

// as `banners`: a deadline change nudges listen(), which otherwise sleeps until the deadline
static NUDGE: LazyLock<(SyncSender<()>, Mutex<Receiver<()>>)> = LazyLock::new(|| {
    let (sender, receiver) = mpsc::sync_channel(1);

    (sender, Mutex::new(receiver))
});

// what changed, as the OSD draws it
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Level {
    Volume(Volume),
    Brightness(u8),
}

/*
 * written by the osd and niri sources; a write wakes every OSD window even when nothing changed,
 * so every writer checks first
 */
pub struct Osd {
    // the level showing, until when
    shown: Option<(Level, Instant)>,

    // from niri; none while unknown or without niri, which counts every monitor as focused
    focused_output: Option<String>,
}

impl Service for Osd {
    fn new() -> Self {
        Self {
            shown: None,
            focused_output: None,
        }
    }

    // sleeps until the OSD's time runs out or a nudge, so an OSD not showing never wakes
    fn listen() {
        let receiver = NUDGE.1.lock().unwrap_or_else(PoisonError::into_inner);

        loop {
            // no deadline waits forever, recv_timeout falls back to recv on overflow
            let wait = Self::read().deadline().map_or(Duration::MAX, |deadline| {
                deadline.saturating_duration_since(Instant::now())
            });

            if receiver.recv_timeout(wait) != Err(RecvTimeoutError::Timeout) {
                continue;
            }

            // a write wakes every OSD window, so only one that hides it
            let now = Instant::now();

            if Self::read().due(now) {
                Self::write().expire(now);
            }
        }
    }
}

impl Osd {
    // shows `level` for the configured time from now, in place of what showed
    pub fn show(&mut self, level: Level, now: Instant) {
        self.shown = Some((level, now + config::get().osd));
        nudge();
    }

    // the OSD follows the focus to its output
    pub fn focus(&mut self, output: Option<String>) {
        self.focused_output = output;
    }

    /*
     * hides it once its time is up, checked under the write lock: a change shown since the time
     * was read has its own, later time and stays
     */
    fn expire(&mut self, now: Instant) {
        if self.due(now) {
            self.shown = None;
        }
    }

    fn due(&self, now: Instant) -> bool {
        self.deadline().is_some_and(|deadline| deadline <= now)
    }

    fn deadline(&self) -> Option<Instant> {
        self.shown.map(|(_, until)| until)
    }

    fn shown_on(&self, monitor: &str) -> Option<Level> {
        let focused = self
            .focused_output
            .as_deref()
            .is_none_or(|focused| focused == monitor);

        self.shown.filter(|_| focused).map(|(level, _)| level)
    }
}

fn nudge() {
    // full means a nudge is already pending, which is all listen() needs
    let _ = NUDGE.0.try_send(());
}

// one window per monitor, shown on the focused one while a level shows, so it draws nothing else
pub fn window(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to the OSD
    let shown = Osd::read().shown_on(&monitor.name);

    let reach = ShadowStyle::island().reach() as f32;
    let width = reach + WIDTH + reach;
    let height = reach + HEIGHT + reach;

    let mut layers: Vec<Box<dyn Widget>> = Vec::new();

    if let Some(level) = shown {
        let body = Rect {
            x: reach,
            y: reach,
            width: WIDTH,
            height: HEIGHT,
        };

        shadow::draw(&mut layers, body, HEIGHT / 2.0, ShadowStyle::island());
        layers.push(Box::new(
            Rectangle::new()
                .width(width)
                .height(height)
                .padding(reach)
                .align_child(Start, Start)
                .child(pill(level, &theme::roles())),
        ));
    }

    LayerWindow::new()
        .width(width)
        .height(height)
        .anchor_vertical(Vertical::Bottom)
        .anchor_horizontal(Horizontal::Middle)
        .margin(Margin {
            bottom: (BOTTOM - reach) as i32,
            ..Margin::default()
        })
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade-osd")
        .visible(shown.is_some())
        .click_through()
        .child(Stack::new(layers).width(width).height(height))
}

// icon, bar, number; muted, the bar and the number go quiet and the icon says why
fn pill(level: Level, roles: &ThemeRoles) -> Rectangle {
    let (icon, percent, quiet) = match level {
        Level::Volume(volume) => {
            let icon = match volume.device {
                Device::Speaker => Icon::Speaker(volume.percent),
                Device::Microphone => Icon::Microphone,
            };

            if volume.muted {
                (icon.muted(), volume.percent, true)
            } else {
                (icon, volume.percent, false)
            }
        }
        Level::Brightness(percent) => (Icon::Sun, percent, false),
    };

    let tone = if quiet {
        roles.on_surface_variant
    } else {
        roles.on_surface
    };

    let bar = WIDTH - 2.0 * INSET - ICON - NUMBER - 2.0 * GAP;

    Rectangle::new()
        .width(WIDTH)
        .height(HEIGHT)
        .radius(HEIGHT / 2.0)
        .fill(roles.surface)
        .padding(Padding {
            top: 0.0,
            right: INSET,
            bottom: 0.0,
            left: INSET,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                icon.on(ICON, tone),
                view::bar_on(bar, f32::from(percent) / 100.0, tone, roles),
                Rectangle::new()
                    .width(NUMBER)
                    .height(HEIGHT)
                    .align_child(End, Center)
                    .child(
                        Text::new(percent.to_string())
                            .size(theme::text::BODY)
                            .color(tone)
                            .weight(theme::text::SEMIBOLD),
                    ),
            ])
            .gap(GAP)
            .align(Center),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOUD: Level = Level::Volume(Volume {
        device: Device::Speaker,
        percent: 60,
        muted: false,
    });

    #[test]
    fn the_osd_shows_only_on_the_focused_output_and_follows_the_focus() {
        let now = Instant::now();
        let mut osd = Osd::new();

        osd.show(LOUD, now);

        // focus unknown counts every output as focused
        assert_eq!(osd.shown_on("DP-1"), Some(LOUD));
        assert_eq!(osd.shown_on("eDP-1"), Some(LOUD));

        osd.focus(Some("eDP-1".into()));
        assert_eq!(osd.shown_on("eDP-1"), Some(LOUD));
        assert_eq!(osd.shown_on("DP-1"), None);

        osd.focus(Some("DP-1".into()));
        assert_eq!(osd.shown_on("DP-1"), Some(LOUD));
        assert_eq!(osd.shown_on("eDP-1"), None);
    }

    // a held key: every step takes the place of the last and starts its time again
    #[test]
    fn a_change_while_shown_replaces_it_and_extends_it() {
        let now = Instant::now();
        let later = now + Duration::from_millis(500);
        let mut osd = Osd::new();

        osd.show(LOUD, now);
        osd.show(Level::Brightness(40), later);

        assert_eq!(osd.shown_on("eDP-1"), Some(Level::Brightness(40)));
        assert_eq!(osd.deadline(), Some(later + config::get().osd));
    }

    // a change shown after listen() found the time up, but before it hid the OSD, keeps it showing
    #[test]
    fn a_change_shown_as_the_time_runs_out_stays() {
        let now = Instant::now();
        let up = now + config::get().osd;
        let mut osd = Osd::new();

        osd.show(LOUD, now);
        assert!(osd.due(up));

        osd.show(Level::Brightness(40), up);
        osd.expire(up);

        assert_eq!(osd.shown_on("eDP-1"), Some(Level::Brightness(40)));

        osd.expire(up + config::get().osd);
        assert_eq!(osd.shown_on("eDP-1"), None);
    }

    #[test]
    fn nothing_shows_until_a_change() {
        let osd = Osd::new();

        assert_eq!(osd.shown_on("eDP-1"), None);
        assert_eq!(osd.deadline(), None);
    }
}
