//! The Desk (ADR 0137): read-only faces of Services drawn as plain text on the desktop, in a
//! window of its own on each output, under every other window. The Desk is the shell's whatever
//! core is chosen, so it lives outside `island/`. This slice has the clock; the window takes no
//! pointer, so every click goes to what lies below.

use kanade_runtime::{
    Column, End, Horizontal, Keyboard, Layer, LayerWindow, Margin, Monitor, Size, Start, Text,
    Vertical, Widget, Zone,
};

use crate::config::{self, Corner};
use crate::{clock, theme};

// from the output's edges to the text
const MARGIN: i32 = 48;

const WIDTH: f32 = 420.0;
const HEIGHT: f32 = 150.0;

const TIME: f32 = 72.0;
const DATE: f32 = 20.0;

pub fn window(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to the config
    let config = config::on(&monitor.name);
    let corner = config.desk_clock_corner;

    let window = LayerWindow::new()
        .width(WIDTH)
        .height(HEIGHT)
        .anchor_vertical(if corner.top() {
            Vertical::Top
        } else {
            Vertical::Bottom
        })
        .anchor_horizontal(if corner.left() {
            Horizontal::Left
        } else {
            Horizontal::Right
        })
        .margin(Margin {
            top: MARGIN,
            right: MARGIN,
            bottom: MARGIN,
            left: MARGIN,
        })
        .layer(Layer::Bottom)
        .space(Zone::Ignore)
        .keyboard(Keyboard::None)
        .namespace("kanade-desk")
        .click_through();

    // off, it reads no Service: no clock, so no frame each minute
    if !config.desk_clock {
        return window.visible(false);
    }

    window.child(clock_face(corner, &config))
}

fn clock_face(corner: Corner, config: &config::Config) -> impl Widget + use<> {
    let ink = theme::ON_SCENE;

    let time = Text::new(clock::now(config.clock))
        .tabular()
        .size(TIME)
        .color(ink)
        .weight(theme::text::MEDIUM);
    let date = Text::new(clock::today().format("%A, %-d %B").to_string())
        .size(DATE)
        .color(ink)
        .weight(theme::text::MEDIUM);

    let column = Column::new(vec![Box::new(time), Box::new(date)])
        .width(Size::Parent)
        .height(Size::Parent)
        .gap(2.0);

    if corner.left() {
        column.align(Start)
    } else {
        column.align(End)
    }
}
