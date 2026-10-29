//! The Desk (ADR 0137): faces of Services drawn as plain text on the desktop, in a window of its
//! own on each output, under every other window. The Desk is the shell's whatever core is chosen,
//! so it lives outside `island/`. The clock, weather, agenda and track line take no pointer. The
//! media buttons, the notes card and each face's handle are the only parts of the window that do:
//! its input region is those three buttons, the card, the handles, and nothing else, so every other
//! click goes to what lies below. A face whose switch is off reads no Service. Event titles and the
//! track's title and artist need their own switches. The notes card is the one face typed into in
//! place (ADR 0140): it asks for the keyboard only while the pointer is on it or it is being typed
//! into, never Exclusive. A face stands where `desk.place` puts it, else in the corner stack, and a
//! handle moves it (ADR 0143).

pub mod notes;
pub mod place;

use std::time::Instant;

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Button, Center, Color, Column, Cursor, End, Horizontal, InputArea, Key, Keyboard, Layer,
    LayerWindow, Monitor, Rectangle, Row, Size, Stack, Start, Text, Vertical, Widget, WindowSize,
    Zone, children,
};

use self::notes::Notes;
use self::place::Places;
use crate::config::{self, Corner, Spot};
use crate::icon::Icon;
use crate::island::geometry::{self, Rect};
use crate::island::service::IslandService;
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

const WIDTH: f32 = 420.0;

// the clock's room, a plain line under it, and the media buttons' row under those
const CLOCK_HEIGHT: f32 = 150.0;
const LINE: f32 = 28.0;
const ROW: f32 = 56.0;

// the notes card's room under the track line, a line for each note it shows
const CARD: f32 = notes::SHOWN as f32 * LINE;

// a note drawn on the card stops here, an ellipsis saying so; the file keeps the whole note, which
// the card cannot measure, since it draws its text in one line of its own width
const CARD_CHARS: usize = 38;

// the caret after the last note while the card is typed into
const CARET: f32 = 1.5;

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
    let output = (monitor.width.max(1) as f32, monitor.height.max(1) as f32);
    // every face off, it reads no Service: no clock, so no frame each minute
    if !faces_on(&config) {
        return hidden(corner, &monitor.name);
    }

    // niri's overview is its own picture of the desktop: no Desk in it, and no Service read
    // for a window nobody sees
    if Overview::read().0 {
        return hidden(corner, &monitor.name);
    }

    // the card off, nothing is typed into it: what was typed is kept, and the file is not read
    if !config.desk_notes {
        notes::close();
    }

    // each face reads its Service only while its switch is on. Weather and the agenda add no
    // poll: the forecast arrives on Weather's write, and the agenda uses the clock's minute tick
    let weather = weather_line(&config);
    let agenda = agenda_line(&config);
    let titles = title_line(&config);
    let titled = titles.is_some();
    // the media Module off follows no player, so the buttons are not read and take no room
    let media = config.desk_media && modules::on("media");
    let keys = if media { Remote::read().keys } else { None };

    // the notes as the file holds them, read once the card first draws; off, neither the Service
    // nor the file is read, and nothing shows
    let card = config.desk_notes;
    let held = if card {
        let (loaded, lines) = {
            let notes = Notes::read();

            (notes.loaded, notes.lines.clone())
        };

        if !loaded {
            notes::ask();
        }

        lines
    } else {
        Vec::new()
    };
    let typed = card && notes::editing(&monitor.name);
    let written = if card {
        notes::shown(&monitor.name, &held)
    } else {
        Vec::new()
    };

    let drawn = config.desk_clock
        || weather.is_some()
        || agenda.is_some()
        || titles.is_some()
        || keys.is_some()
        || card;

    if !drawn {
        return hidden(corner, &monitor.name);
    }

    // every face that shows, its box's height, and what draws it: the stack lays them out for the
    // faces the config gives no place, and each is drawn in its own box of the output
    let mut faces: Vec<(&'static str, f32, Box<dyn Widget>)> = Vec::new();

    if config.desk_clock {
        faces.push(("clock", CLOCK_HEIGHT, Box::new(clock_face(corner, &config))));
    }
    if let Some(line) = weather {
        faces.push(("weather", LINE, Box::new(plain(line, corner))));
    }
    if let Some(line) = agenda {
        faces.push(("agenda", LINE, Box::new(plain(line, corner))));
    }
    if card {
        faces.push((
            "notes",
            CARD,
            Box::new(notes_card(corner, &monitor.name, &held, &written, typed)),
        ));
    }
    // the buttons' row stays while a player may open, so that does not move the lines above
    if titles.is_some() || media {
        let height = match (titles.is_some(), media) {
            (true, true) => LINE + ROW,
            (true, false) => LINE,
            (false, _) => ROW,
        };

        faces.push((
            "media",
            height,
            Box::new(media_face(corner, titles, keys, media)),
        ));
    }

    let heights: Vec<f32> = faces.iter().map(|(_, height, _)| *height).collect();
    let stacked = place::stack(corner, &heights, output, WIDTH);
    let (rects, moving) = {
        let places = Places::read();

        let rects = faces
            .iter()
            .zip(&stacked)
            .map(|((name, _, _), box_)| {
                let stored = config.desk_place.get(*name).copied();

                place::at(&places, &monitor.name, name, stored, *box_, output)
            })
            .collect::<Vec<Rect>>();

        (rects, place::keyboard(&monitor.name))
    };
    let mut areas: Vec<InputArea> = Vec::new();
    let mut layers: Vec<Box<dyn Widget>> = Vec::new();

    for ((name, _, face), rect) in faces.into_iter().zip(rects) {
        let handle = place::handle(rect, corner);
        let grip = grip(
            name,
            handle,
            rect,
            monitor,
            config.desk_place.get(name).copied(),
            output,
        );

        // the parts of a face that take the pointer, and nothing else of it: the handle, the card's
        // typing area, the buttons that can act. Those that cannot are left to what lies below
        areas.push(view::input_area(handle));

        if name == "notes" {
            areas.push(view::input_area(rect));
        }

        if name == "media"
            && let Some(keys) = keys
        {
            areas.extend(media_areas(corner, rect, titled, keys));
        }

        layers.push(Box::new(
            Rectangle::new()
                .width(rect.width)
                .height(rect.height)
                .translate(rect.x, rect.y)
                .child(Stack::new(vec![face, Box::new(grip)])),
        ));
    }

    let mut window = shell(corner);

    window = window.input_region(areas);

    /*
     * the card asks for the keyboard before the press that focuses it, since niri focuses an
     * OnDemand surface only on one, and keeps it while it is typed into; an island opened without
     * a press holds Exclusive for that, which a Bottom layer does not need. A face being moved asks
     * for it too, for Escape alone
     */
    if notes::keyboard(&monitor.name) || moving {
        window = window.keyboard(Keyboard::OnDemand);
    }

    // registered with the keyboard, as a surface's is, since the press that starts a note and the
    // keys after it arrive as the same window is rebuilt; a card that is not being typed into
    // leaves them alone
    if card || moving {
        let name = monitor.name.clone();

        window = window.on_key(move |key| {
            // Escape puts a face being moved back where it stood, and keeps a note being typed
            if key == Key::Escape && place::dragging(&name) {
                place::back(&name);
            }

            notes::press(&name, key);
        });
    }

    // the keyboard went elsewhere, as a click outside the card does: what was typed is kept, and a
    // face whose Escape can no longer arrive is let go where it stood
    if card || moving {
        let name = monitor.name.clone();

        window = window.on_lost(move || {
            if place::dragging(&name) {
                place::back(&name);
            }

            notes::blur(&name);
        });
    }

    window.child(Stack::new(layers).width(Size::Parent).height(Size::Parent))
}

/*
 * where the Island's body stands on `monitor`, while it shows there, which a face being moved is
 * kept out of (ADR 0143). Autohide past its edge, or nothing to show, is no Island on the output
 */
fn island_area(monitor: &Monitor) -> Option<Rect> {
    if view::hides() {
        return None;
    }

    let island = IslandService::read();

    if island.overview() {
        return None;
    }

    let canvas = view::canvas(monitor);
    let hang = config::get().island_placement().anchor.hang();
    let body = geometry::body(island.shape(&monitor.name, Instant::now()), hang, canvas);
    let output = (monitor.width as f32, monitor.height as f32);

    Some(geometry::placed(body, hang, canvas, output))
}

/*
 * a face's handle: the bar at its inner edge, faded so the faces stay as calm as they have always
 * been. A press on it takes hold of the face's box, and the pointer moves the box; a press on the
 * card elsewhere keeps typing into it
 */
fn grip(
    face: &'static str,
    handle: Rect,
    rect: Rect,
    monitor: &Monitor,
    stored: Option<Spot>,
    output: (f32, f32),
) -> Rectangle {
    let name = monitor.name.clone();
    let under = name.clone();
    let let_go = name.clone();
    let screen = monitor.clone();
    let bar = Rectangle::new()
        .width(place::BAR)
        .height((handle.height * 0.6).max(place::BAR))
        .radius(place::BAR / 2.0)
        .fill(theme::faded(theme::ON_SCENE, DISABLED));

    Rectangle::new()
        .width(handle.width)
        .height(handle.height)
        .translate(handle.x - rect.x, handle.y - rect.y)
        .cursor(Cursor::Grab)
        .align_child(Center, Center)
        // a press on the handle is the handle's, not the card's: it starts nothing being typed
        .on_click(|_| {})
        .on_hover(move |inside| place::hover(&under, inside))
        .on_drag(move |point| {
            // where the Island stands now, read only while a drag is on: the window itself reads
            // no Island, so an Island morph does not draw the Desk again
            let room = place::Room {
                output,
                island: island_area(&screen),
            };

            place::drag(&name, face, rect, stored, room, point)
        })
        .on_drag_end(move |released| place::release(&let_go, released))
        .child(bar)
}

// the media face: the track's line over the buttons' row, in the one box that moves them together
fn media_face(
    corner: Corner,
    titles: Option<String>,
    keys: Option<Keys>,
    reserve: bool,
) -> impl Widget + use<> {
    let mut parts: Vec<Box<dyn Widget>> = Vec::new();

    if let Some(line) = titles {
        parts.push(Box::new(plain(line, corner)));
    }

    if let Some(keys) = keys {
        parts.push(Box::new(media_row(keys)));
    } else if reserve {
        // the row's room, so the lines above do not move when a player opens
        parts.push(Box::new(Rectangle::new().width(1.0).height(ROW)));
    }

    let column = Column::new(parts).width(Size::Parent).height(Size::Parent);

    if corner.left() {
        column.align(Start)
    } else {
        column.align(End)
    }
}

fn faces_on(config: &config::Config) -> bool {
    config.desk_clock
        || config.desk_media
        || config.desk_weather
        || config.desk_agenda
        || config.desk_media_titles
        || config.desk_notes
}

// no Desk on this output for now: a face being moved goes with it, and its place stays as it was
fn hidden(corner: Corner, monitor: &str) -> LayerWindow {
    notes::close();

    // a face being moved goes with the window, and a hovered handle is let go: written on a worker,
    // since a view writes no Service, and only when there is something to clear
    place::hide(monitor);

    shell(corner).visible(false)
}

fn shell(corner: Corner) -> LayerWindow {
    LayerWindow::new()
        .width(WindowSize::Full)
        .height(WindowSize::Full)
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

/*
 * the notes card (ADR 0140): the Desk's own notes, the newest last, and the caret after the last
 * one while a click is typing into it. It is the Desk's only text that takes the pointer: a press
 * on it starts typing from the notes the file holds, and the pointer on it asks for the keyboard,
 * which the press then focuses. A note too long for the card is drawn with an ellipsis; the file
 * keeps it whole
 */
fn notes_card(
    corner: Corner,
    monitor: &str,
    held: &[String],
    written: &[String],
    typed: bool,
) -> impl Widget + use<> {
    let hint = theme::faded(theme::ON_SCENE, DISABLED);
    let mut lines: Vec<Box<dyn Widget>> = Vec::new();

    if written.is_empty() {
        // an empty card still says what a click does, and where to type
        let (text, ink, caret) = match typed {
            true => (String::new(), theme::ON_SCENE, true),
            false => (String::from("Add a note"), hint, false),
        };

        lines.push(Box::new(note_line(text, corner, ink, caret)));
    }

    for (at, note) in written.iter().enumerate() {
        let caret = typed && at + 1 == written.len();

        lines.push(Box::new(note_line(
            clipped(note),
            corner,
            theme::ON_SCENE,
            caret,
        )));
    }

    // the newest note sits at the card's bottom, next to the buttons' row, so the card fills up
    let column = Column::new(lines)
        .width(Size::Parent)
        .height(CARD)
        .justify(End);

    let over = monitor.to_owned();
    let start = monitor.to_owned();
    let held = held.to_vec();

    Rectangle::new()
        .width(WIDTH)
        .height(CARD)
        .cursor(Cursor::Text)
        .semantics(|| Semantics::new(Role::TextInput, "notes"))
        .on_hover(move |inside| notes::hover(&over, inside))
        .on_click(move |pressed| {
            if pressed == Button::Left {
                notes::begin(&start, &held);
            }
        })
        .child(column)
}

// one note on the card: the Desk's text, the caret after it while it is the one being typed
fn note_line(text: String, corner: Corner, ink: Color, caret: bool) -> impl Widget + use<> {
    let mut parts: Vec<Box<dyn Widget>> = vec![Box::new(
        Text::new(text)
            .size(DATE)
            .color(ink)
            .weight(theme::text::MEDIUM),
    )];

    if caret {
        parts.push(Box::new(
            Rectangle::new().width(CARET).height(DATE * 1.15).fill(ink),
        ));
    }

    let row = Row::new(parts).height(LINE).align(End).gap(2.0);
    let column = Column::new(vec![Box::new(row)])
        .width(Size::Parent)
        .height(LINE);

    if corner.left() {
        column.align(Start)
    } else {
        column.align(End)
    }
}

// a note as the card draws it, cut to the room one line has
fn clipped(note: &str) -> String {
    if note.chars().count() <= CARD_CHARS {
        return note.to_owned();
    }

    let mut short: String = note.chars().take(CARD_CHARS).collect();

    short.push('\u{2026}');

    short
}

// previous, play or pause, next; each faded and dead when the player cannot. A press here is a
// click elsewhere than the notes card, so it ends the typing the card took
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
            // a press elsewhere than the card ends the typing it took
            notes::close();

            media::request(transport, Later::detached());
        }
    })
}

// where the buttons that can act take the pointer, in the window's own space: those that cannot are
// left to what lies below
fn media_areas(corner: Corner, rect: Rect, titled: bool, keys: Keys) -> Vec<InputArea> {
    let x = if corner.left() {
        rect.x
    } else {
        rect.x + WIDTH - BUTTONS
    };
    let y = rect.y + if titled { LINE } else { 0.0 } + (ROW - BUTTON) / 2.0;

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

#[cfg(test)]
mod tests {
    use super::*;

    // the track's title while the media Module is off: the line stands without the buttons' row,
    // rather than a debug build dying on a row it never reserved
    #[test]
    fn a_title_with_no_buttons_draws_no_row() {
        let _ = media_face(Corner::TopLeft, Some(String::from("a track")), None, false);
        let _ = media_face(Corner::TopLeft, None, None, true);
    }
}
