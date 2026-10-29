//! The Desk (ADR 0137): faces of Services drawn as plain text on the desktop, in a window of its
//! own on each output, under every other window. The Desk is the shell's whatever core is chosen,
//! so it lives outside `island/`. The clock, weather, agenda and track line take no pointer. The
//! media buttons and the notes card are the only parts of the window that do: its input region is
//! those three buttons, the card, and nothing else, so every other click goes to what lies below.
//! A face whose switch is off reads no Service. Event titles and the track's title and artist need
//! their own switches. The notes card is the one face typed into in place (ADR 0140): it asks for
//! the keyboard only while the pointer is on it or it is being typed into, never Exclusive.

pub mod notes;

use kanade_runtime::service::Service;
use kanade_runtime::ui::{Role, Semantics};
use kanade_runtime::{
    Button, Center, Color, Column, Cursor, End, Horizontal, InputArea, Keyboard, Layer,
    LayerWindow, Margin, Monitor, Rectangle, Row, Size, Start, Text, Vertical, Widget, Zone,
    children,
};

use self::notes::Notes;
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
    // every face off, it reads no Service: no clock, so no frame each minute
    if !faces_on(&config) {
        notes::close();

        return shell(corner, 1.0).visible(false);
    }

    // niri's overview is its own picture of the desktop: no Desk in it, and no Service read
    // for a window nobody sees
    if Overview::read().0 {
        notes::close();

        return shell(corner, 1.0).visible(false);
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
        notes::close();

        return shell(corner, 1.0).visible(false);
    }

    // the buttons' row stays while a player may open, so that does not move the lines above
    let reserve_media = media;
    let height = if config.desk_clock { CLOCK_HEIGHT } else { 0.0 }
        + if weather.is_some() { LINE } else { 0.0 }
        + if agenda.is_some() { LINE } else { 0.0 }
        + if titles.is_some() { LINE } else { 0.0 }
        + if card { CARD } else { 0.0 }
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
    if card {
        faces.push(Box::new(notes_card(
            corner,
            &monitor.name,
            &held,
            &written,
            typed,
        )));
    }

    let mut window = shell(corner, height);
    let mut areas: Vec<InputArea> = Vec::new();

    if card {
        areas.push(view::input_area(card_rect(height, reserve_media)));
    }

    if reserve_media {
        match keys {
            Some(keys) => {
                faces.push(Box::new(media_row(keys)));
                areas.extend(media_areas(corner, height, keys));
            }

            // the row's room, so the lines above do not move when a player opens
            None => faces.push(Box::new(Rectangle::new().width(1.0).height(ROW))),
        }
    }

    window = window.input_region(areas);

    /*
     * the card asks for the keyboard before the press that focuses it, since niri focuses an
     * OnDemand surface only on one, and keeps it while it is typed into; an island opened without
     * a press holds Exclusive for that, which a Bottom layer does not need
     */
    if notes::keyboard(&monitor.name) {
        window = window.keyboard(Keyboard::OnDemand);
    }

    // registered with the keyboard, as a surface's is, since the press that starts a note and the
    // keys after it arrive as the same window is rebuilt; a card that is not being typed into
    // leaves them alone
    if card {
        let name = monitor.name.clone();

        window = window.on_key(move |key| {
            notes::press(&name, key);
        });
    }

    // the keyboard went elsewhere, as a click outside the card does: what was typed is kept
    if card {
        let name = monitor.name.clone();

        window = window.on_lost(move || notes::blur(&name));
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
        || config.desk_notes
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

// where the pointer types into the card, in the window's own space: the card's box, under the
// Desk's other lines and above the buttons' row
fn card_rect(height: f32, media: bool) -> Rect {
    Rect {
        x: 0.0,
        y: height - if media { ROW } else { 0.0 } - CARD,
        width: WIDTH,
        height: CARD,
    }
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
