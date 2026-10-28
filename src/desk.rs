//! The Desk (ADR 0137): read-only faces of Services drawn as plain text on the desktop, in a
//! window of its own on each output, under every other window. The Desk is the shell's whatever
//! core is chosen, so it lives outside `island/`. The clock, weather, agenda and track line take
//! no pointer. The media buttons are the only part of the window that does: its input region is
//! those three buttons and every other click goes to what lies below. A face whose switch is off
//! reads no Service. Event titles and the track's title and artist need their own switches.

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Button, Center, Column, Cursor, End, Horizontal, InputArea, Keyboard, Layer, LayerWindow,
    Margin, Monitor, Rectangle, Row, Size, Start, Text, Vertical, Widget, Zone, children,
};

use crate::config::{self, Corner};
use crate::icon::Icon;
use crate::island::geometry::Rect;
use crate::later::Later;
use crate::sources::calendar;
use crate::sources::media::{self, Transport};
use crate::sources::niri::Niri;
use crate::sources::playback::{Caption, Keys, Remote};
use crate::sources::reminders;
use crate::sources::weather::Weather;
use crate::theme::DISABLED;
use crate::{clock, modules, theme, view};

// whether niri's overview is open; the Desk is hidden while it is
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Overview(bool);

impl Service for Overview {
    fn new() -> Self {
        Overview::default()
    }

    fn listen() {}
}

// a watcher of `Niri`: the Desk's windows redraw when the overview opens or closes, not at each of
// niri's events; the same again changes nothing
pub fn follow_overview() {
    let open = Niri::read().seen.overview;
    let mut overview = Overview::write();

    if overview.0 == open {
        overview.quiet();
    } else {
        overview.0 = open;
    }
}

// from the output's edges to the text
const MARGIN: i32 = 48;

const WIDTH: f32 = 420.0;

// the clock's room, a plain line under it, and the media buttons' row under those
const CLOCK_HEIGHT: f32 = 150.0;
const LINE: f32 = 28.0;
const ROW: f32 = 56.0;

// a button, its glyph, and the room between two
const BUTTON: f32 = 44.0;
const GLYPH: f32 = 22.0;
const GAP: f32 = 16.0;
const BUTTONS: f32 = 3.0 * BUTTON + 2.0 * GAP;

const TIME: f32 = 72.0;
const DATE: f32 = 20.0;

pub fn window(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to the config
    let config = config::on(&monitor.name);
    let corner = config.desk_clock_corner;
    // every face off, it reads no Service: no clock, so no frame each minute
    if !faces_on(&config) {
        return shell(corner, 1.0).visible(false);
    }

    // niri's overview is its own picture of the desktop: no Desk in it, and no Service read
    // for a window nobody sees
    if Overview::read().0 {
        return shell(corner, 1.0).visible(false);
    }

    // each face reads its Service only while its switch is on. Weather and the agenda add no
    // poll: the forecast arrives on Weather's write, and the agenda uses the clock's minute tick
    let weather = weather_line(&config);
    let agenda = agenda_line(&config);
    let titles = title_line(&config);
    // the media Module off follows no player, so the buttons are not read and take no room
    let media = config.desk_media && modules::on("media");
    let keys = if media { Remote::read().keys } else { None };

    let drawn = config.desk_clock
        || weather.is_some()
        || agenda.is_some()
        || titles.is_some()
        || keys.is_some();

    if !drawn {
        return shell(corner, 1.0).visible(false);
    }

    // the buttons' row stays while a player may open, so that does not move the lines above
    let reserve_media = media;
    let height = if config.desk_clock { CLOCK_HEIGHT } else { 0.0 }
        + if weather.is_some() { LINE } else { 0.0 }
        + if agenda.is_some() { LINE } else { 0.0 }
        + if titles.is_some() { LINE } else { 0.0 }
        + if reserve_media { ROW } else { 0.0 };

    let mut faces: Vec<Box<dyn Widget>> = Vec::new();

    if config.desk_clock {
        faces.push(Box::new(clock_face(corner, &config)));
    }
    if let Some(line) = weather {
        faces.push(Box::new(plain(line, corner)));
    }
    if let Some(line) = agenda {
        faces.push(Box::new(plain(line, corner)));
    }
    if let Some(line) = titles {
        faces.push(Box::new(plain(line, corner)));
    }

    let mut window = shell(corner, height);

    if reserve_media {
        match keys {
            Some(keys) => {
                faces.push(Box::new(media_row(keys)));
                window = window.input_region(media_areas(corner, height, keys));
            }

            // the row's room, so the lines above do not move when a player opens
            None => faces.push(Box::new(Rectangle::new().width(1.0).height(ROW))),
        }
    }

    let column = Column::new(faces).width(Size::Parent).height(Size::Parent);

    window.child(if corner.left() {
        column.align(Start)
    } else {
        column.align(End)
    })
}

fn faces_on(config: &config::Config) -> bool {
    config.desk_clock
        || config.desk_media
        || config.desk_weather
        || config.desk_agenda
        || config.desk_media_titles
}

fn shell(corner: Corner, height: f32) -> LayerWindow {
    LayerWindow::new()
        .width(WIDTH)
        .height(height.max(1.0))
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
        .click_through()
}

// temperature and condition; nothing, and Weather unread, while the Module or the switch is off
fn weather_line(config: &config::Config) -> Option<String> {
    if !config.desk_weather || !modules::on("weather") {
        return None;
    }

    let weather = Weather::read();
    let current = &weather.forecast.as_ref()?.current;

    Some(format!(
        "{} {}",
        config.units.temperature(current.temperature),
        current.condition.name()
    ))
}

// the next event, without its title unless that switch is on. Arms the clock's minute tick, the
// same one the clock face uses, so the line can change as the event draws near; no poll of its own
fn agenda_line(config: &config::Config) -> Option<String> {
    if !config.desk_agenda || !modules::on("calendar") {
        return None;
    }

    let now = clock::unix_seconds();
    let _ = clock::now(config.clock);
    let events = calendar::upcoming(clock::today());
    let next = events
        .iter()
        .find(|event| !event.all_day && event.span.1.max(event.span.0 + 1) > now)?;
    let at = clock::time(next.start.time(), config.clock);

    if !config.desk_agenda_titles {
        let (left, _) = reminders::countdown(next.span.0, now);
        return Some(format!("{at}, {left}"));
    }

    let title = next.title.trim();

    if title.is_empty() {
        Some(at)
    } else {
        Some(format!("{at} {title}"))
    }
}

// title and artist; the player is unread while the switch or the media Module is off
fn title_line(config: &config::Config) -> Option<String> {
    if !config.desk_media_titles || !modules::on("media") {
        return None;
    }

    let playing = Caption::read();
    let title = playing.title.trim();
    let artist = playing.artist.trim();

    match (title.is_empty(), artist.is_empty()) {
        (true, true) => None,
        (false, true) => Some(title.to_owned()),
        (true, false) => Some(artist.to_owned()),
        (false, false) => Some(format!("{title} — {artist}")),
    }
}

fn plain(line: String, corner: Corner) -> impl Widget + use<> {
    let text = Text::new(line)
        .size(DATE)
        .color(theme::ON_SCENE)
        .weight(theme::text::MEDIUM);
    let column = Column::new(vec![Box::new(text)])
        .width(Size::Parent)
        .height(LINE);

    if corner.left() {
        column.align(Start)
    } else {
        column.align(End)
    }
}

// previous, play or pause, next; each faded and dead when the player cannot
fn media_row(keys: Keys) -> Row {
    let glyph = if keys.playing {
        Icon::Pause
    } else {
        Icon::Play
    };

    Row::new(children![
        button(
            Icon::Previous,
            "media/previous",
            Transport::Previous,
            keys.can_previous
        ),
        button(
            glyph,
            "media/play-pause",
            Transport::PlayPause,
            keys.can_play_pause
        ),
        button(Icon::Next, "media/next", Transport::Next, keys.can_next),
    ])
    .width(BUTTONS)
    .height(ROW)
    .gap(GAP)
    .align(Center)
}

fn button(icon: Icon, name: &'static str, transport: Transport, able: bool) -> Rectangle {
    let ink = if able {
        theme::ON_SCENE
    } else {
        theme::faded(theme::ON_SCENE, DISABLED)
    };

    let button = Rectangle::new()
        .width(BUTTON)
        .height(BUTTON)
        .radius(BUTTON / 2.0)
        .fill(if able {
            theme::SCENE_PLATE
        } else {
            theme::faded(theme::SCENE_PLATE, DISABLED)
        })
        .align_child(Center, Center)
        .semantics(move || Semantics::new(Role::Button, name).disabled(!able))
        .child(icon.on(GLYPH, ink));

    if !able {
        return button;
    }

    // the press is a Kanade verb on the media follower, never a bus call or a write here, and
    // nobody waits on its answer
    button.cursor(Cursor::Pointer).on_click(move |pressed| {
        if pressed == Button::Left {
            media::request(transport, Later::detached());
        }
    })
}

// where the pointer is taken: the buttons that can act, in the window's own space. Those that cannot
// are left to what lies below
fn media_areas(corner: Corner, height: f32, keys: Keys) -> Vec<InputArea> {
    let x = if corner.left() { 0.0 } else { WIDTH - BUTTONS };
    let y = height - ROW + (ROW - BUTTON) / 2.0;

    [keys.can_previous, keys.can_play_pause, keys.can_next]
        .into_iter()
        .enumerate()
        .filter(|(_, able)| *able)
        .map(|(at, _)| {
            view::input_area(Rect {
                x: x + at as f32 * (BUTTON + GAP),
                y,
                width: BUTTON,
                height: BUTTON,
            })
        })
        .collect()
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
        .height(CLOCK_HEIGHT)
        .gap(2.0);

    if corner.left() {
        column.align(Start)
    } else {
        column.align(End)
    }
}
