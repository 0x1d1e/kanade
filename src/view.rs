use std::time::Instant;

use amane::{
    Arc, Button, Canvas, Cap, Center, Circle, Color, Column, End, Horizontal, Image, InputArea,
    Key, Layer, LayerWindow, Line, Monitor, Padding, Parent, Path, Rectangle, Row, Scroll, Service,
    Shape as _, Stack, Start, Text, Vertical, Widget, Zone, children, request_frame, shapes,
};

use crate::island::activity::{Activity, Detail, Device, Frame, Kind, Track, Volume};
use crate::island::geometry::{self, Rect, Shape};
use crate::island::presentation::{Content, Input, Presentation};
use crate::island::service::IslandService;
use crate::theme;

// one island window per monitor; the window stays put and only the body morphs inside it
pub fn island(monitor: &Monitor) -> LayerWindow {
    // reading subscribes this window to island changes
    let island = IslandService::read();

    let now = Instant::now();

    let keyboard = island.keyboard(&monitor.name);
    let shape = island.shape(&monitor.name, now);
    let content = island.content(&monitor.name, now);
    let frame = island.frame(&monitor.name, now);

    // the spring is ours, not an Amane Animation, so the view asks for frames until it rests
    if !island.settled(&monitor.name, now) {
        request_frame();
    }

    let body_rect = geometry::body(shape);
    let area = geometry::input_area(body_rect);

    let clicked = monitor.name.clone();
    let moved = monitor.name.clone();
    let hovered = monitor.name.clone();
    let pressed = monitor.name.clone();
    let scrolled = monitor.name.clone();

    // the morph reveals content already laid out at its final size, clipped to the body's corners
    let mut body = Rectangle::new()
        .width(body_rect.width)
        .height(body_rect.height)
        .radius(shape.radius)
        .fill(theme::BODY)
        .clip()
        .align_child(Center, Start)
        .translate(body_rect.x - area.x, body_rect.y - area.y);

    // at most one shows at a time, the crossfade hands over through nothing
    if let Some((content, opacity)) = content.into_iter().flatten().next()
        && let Some(form) = small_form(&content).or_else(|| placeholder(&content))
    {
        body = body.child(form.opacity(opacity));
    }

    /*
     * exactly the input region, so leaving it is leaving the island; niri also sends the leave
     * when the region shrinks away from a still pointer (#3), which is what reports it here.
     * Hover, click and arming all take this one target: Amane hit-tests only on pointer events,
     * so a body that grows under a still pointer (Peek) would never report it on the body
     */
    let hover = Rectangle::new()
        .width(area.width)
        .height(area.height)
        .translate(area.x, area.y)
        .align_child(Start, Start)
        .on_hover(move |inside| hover(&hovered, inside))
        // Escape disarms without the pointer leaving, the next move arms again
        .on_move(move |_| set_armed(&moved, true))
        .on_click(move |button| match button {
            Button::Left => expand(&clicked),
            Button::Right => route(&clicked, Input::RightClick),
            _ => {}
        })
        .on_scroll(move |Scroll { y, .. }| route(&scrolled, Input::Wheel(y)))
        .child(body);

    // the body grows over the Satellites as they fade, and they never take the pointer
    let mut layers = if island.overview() {
        Vec::new()
    } else {
        satellites(&frame, body_rect, shape)
    };

    layers.push(Box::new(hover));

    if let Some(badge) = queued(&frame, body_rect, shape) {
        layers.push(Box::new(badge));
    }

    LayerWindow::new()
        .width(geometry::CANVAS_WIDTH)
        .height(geometry::CANVAS_HEIGHT)
        .anchor_vertical(Vertical::Top)
        .anchor_horizontal(Horizontal::Middle)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade")
        .keyboard(keyboard)
        .on_key(move |key| {
            if key == Key::Escape {
                collapse(&pressed);

                // OnDemand would keep the focus the press gave while the pointer rests on the pill
                set_armed(&pressed, false);
            }
        })
        // empty under niri's overview, so the pointer reaches the overview beneath
        .input_region(if island.overview() {
            vec![]
        } else {
            vec![input_area(area)]
        })
        .child(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .align_child(Start, Start)
                .child(Stack::new(layers)),
        )
}

// the Satellites, then the ones past the cap as one "+N"
fn satellites(frame: &Frame, body: Rect, shape: Shape) -> Vec<Box<dyn Widget>> {
    let opacity = geometry::satellite_opacity(shape);

    if opacity == 0.0 {
        return Vec::new();
    }

    let labels = frame
        .satellites
        .iter()
        .map(|activity| abbreviation(activity.kind()).to_owned())
        .chain((frame.overflow > 0).then(|| format!("+{}", frame.overflow)));

    labels
        .enumerate()
        .map(|(index, label)| {
            let at = geometry::satellite(body, index);

            Box::new(dot(at, label).opacity(opacity)) as Box<dyn Widget>
        })
        .collect()
}

/*
 * the Transients an open Surface keeps back, as a count in its top right corner; it fades in as
 * the Satellites fade out
 */
fn queued(frame: &Frame, body: Rect, shape: Shape) -> Option<Rectangle> {
    let opacity = 1.0 - geometry::satellite_opacity(shape);

    if frame.queued.is_empty() || opacity == 0.0 {
        return None;
    }

    let inset = 12.0;
    let at = Rect {
        x: body.x + body.width - inset - geometry::SATELLITE,
        y: body.y + inset,
        width: geometry::SATELLITE,
        height: geometry::SATELLITE,
    };

    Some(dot(at, frame.queued.len().to_string()).opacity(opacity))
}

fn dot(at: Rect, label: String) -> Rectangle {
    Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(at.width / 2.0)
        .fill(theme::DOT)
        .align_child(Center, Center)
        .translate(at.x, at.y)
        .child(Text::new(label).size(12.0).color(theme::FG).weight(600))
}

// stand-in for each Kind's glyph (#21-#33), distinct per Kind
fn abbreviation(kind: Kind) -> &'static str {
    match kind {
        Kind::Media => "M",
        Kind::Notification => "N",
        Kind::Volume => "V",
        Kind::Brightness => "Br",
        Kind::Workspace => "W",
        Kind::Battery => "Ba",
        Kind::Network => "Nw",
        Kind::Bluetooth => "Bt",
        Kind::ScreenCast => "S",
        Kind::Timer => "T",
        Kind::Privacy => "P",
    }
}

/*
 * stand-in content until the sources and the Surfaces draw their own (#21-#33): names the Activity
 * a small form shows, or the open Surface, so the crossfade and the clipping can be seen. Short,
 * so sized to its letters and centered; Rest shows nothing
 */
fn placeholder(content: &Content) -> Option<Rectangle> {
    let name = |activity: &Option<Activity>| {
        activity.as_ref().map_or_else(String::new, |activity| {
            format!("{} {}", activity.kind().name(), activity.id().key())
        })
    };

    let (label, size) = match content.presentation {
        Presentation::Rest => return None,
        Presentation::Compact => (name(&content.activity), 13.0),
        Presentation::Peek => (name(&content.activity), 15.0),
        Presentation::Expanded(surface) => (format!("{surface:?}"), 17.0),
    };

    Some(
        sized(content.presentation)
            .align_child(Center, Center)
            .child(Text::new(label).size(size).color(theme::FG).weight(500)),
    )
}

// content is laid out at its Presentation's final size, which the morph reveals
fn sized(presentation: Presentation) -> Rectangle {
    let size = geometry::shape(presentation);

    Rectangle::new().width(size.width).height(size.height)
}

// an Activity's own Compact or Peek, drawn from its Detail; none leaves it to the placeholder
fn small_form(content: &Content) -> Option<Rectangle> {
    let detail = content.activity.as_ref().map(Activity::detail);

    match (content.presentation, detail) {
        (Presentation::Compact, Some(Detail::Media(track))) => Some(media_compact(track)),
        (Presentation::Peek, Some(Detail::Media(track))) => Some(media_peek(track)),
        (presentation, Some(Detail::Volume(volume))) => level(presentation, Level::volume(volume)),
        (presentation, Some(&Detail::Brightness(percent))) => {
            level(presentation, Level::brightness(percent))
        }
        _ => None,
    }
}

// a Volume or Brightness as one bar, the same layout for both
struct Level {
    icon: Icon,
    label: &'static str,
    percent: u8,

    // muted: the bar and the number go quiet, the icon says why
    quiet: bool,
}

impl Level {
    fn volume(volume: &Volume) -> Level {
        let (icon, label) = match volume.device {
            Device::Speaker => (Icon::Speaker(volume.percent), "Volume"),
            Device::Microphone => (Icon::Microphone, "Microphone"),
        };

        Level {
            icon: if volume.muted { icon.muted() } else { icon },
            label,
            percent: volume.percent,
            quiet: volume.muted,
        }
    }

    fn brightness(percent: u8) -> Level {
        Level {
            icon: Icon::Sun,
            label: "Brightness",
            percent,
            quiet: false,
        }
    }
}

/*
 * icon, bar, number; Peek names the level above its bar. Every part has a fixed width, so a level
 * that moves slides the bar and nothing else
 */
fn level(presentation: Presentation, level: Level) -> Option<Rectangle> {
    let (icon, number, size, inset) = match presentation {
        Presentation::Compact => (20.0, 26.0, 13.0, 15.0),
        Presentation::Peek => (24.0, 30.0, 15.0, 20.0),
        _ => return None,
    };

    let gap = 11.0;
    let width = geometry::shape(presentation).width - 2.0 * inset - icon - number - 2.0 * gap;

    let tone = if level.quiet { theme::MUTED } else { theme::FG };

    let bar = bar(width, level.percent, tone);

    let middle: Box<dyn Widget> = match presentation {
        Presentation::Peek => Box::new(
            Column::new(children![
                Text::new(level.label)
                    .size(12.0)
                    .color(theme::MUTED)
                    .weight(500),
                bar,
            ])
            .gap(6.0),
        ),
        _ => Box::new(bar),
    };

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(
                Row::new(vec![
                    Box::new(level.icon.draw(icon)) as Box<dyn Widget>,
                    middle,
                    Box::new(
                        Rectangle::new()
                            .width(number)
                            .height(size)
                            .align_child(End, Center)
                            .child(
                                Text::new(level.percent.to_string())
                                    .size(size)
                                    .color(tone)
                                    .weight(600),
                            ),
                    ),
                ])
                .gap(gap)
                .align(Center),
            ),
    )
}

fn bar(width: f32, percent: u8, tone: Color) -> Stack {
    let height = 6.0;
    let filled = width * f32::from(percent.min(100)) / 100.0;

    let track = Rectangle::new()
        .width(width)
        .height(height)
        .radius(height / 2.0)
        .fill(theme::DOT);

    let mut layers = children![track];

    // narrower than its round ends it would draw as a misshapen dot
    if filled >= height {
        layers.push(Box::new(
            Rectangle::new()
                .width(filled)
                .height(height)
                .radius(height / 2.0)
                .fill(tone),
        ));
    }

    Stack::new(layers).width(width).height(height)
}

// drawn, not a font's glyph, so it looks the same whatever fonts the machine has
#[derive(Clone, Copy)]
enum Icon {
    // waves by level: none silent, one up to half, two above
    Speaker(u8),
    SpeakerMuted,
    Microphone,
    MicrophoneMuted,
    Sun,
}

impl Icon {
    fn muted(self) -> Icon {
        match self {
            Icon::Speaker(_) => Icon::SpeakerMuted,
            Icon::Microphone => Icon::MicrophoneMuted,
            icon => icon,
        }
    }

    // on a 20 unit grid, scaled to `side`
    fn draw(self, side: f32) -> Canvas {
        let u = side / 20.0;
        let line = 1.7 * u;
        let stroke = |shape: Line| shape.stroke(line, theme::FG).cap(Cap::Round);

        let speaker = || {
            Path::new()
                .move_to(2.5 * u, 7.5 * u)
                .line_to(6.0 * u, 7.5 * u)
                .line_to(10.5 * u, 3.5 * u)
                .line_to(10.5 * u, 16.5 * u)
                .line_to(6.0 * u, 12.5 * u)
                .line_to(2.5 * u, 12.5 * u)
                .close()
                .fill(theme::FG)
        };

        let wave = |radius: f32| {
            Arc::new()
                .center(10.5 * u, 10.0 * u)
                .radius(radius * u)
                .start(50.0)
                .sweep(80.0)
                .stroke(line, theme::FG)
                .cap(Cap::Round)
        };

        let microphone = || {
            shapes![
                Path::new()
                    .move_to(7.0 * u, 5.0 * u)
                    .arc(10.0 * u, 5.0 * u, 3.0 * u, 270.0, 180.0)
                    .line_to(13.0 * u, 9.0 * u)
                    .arc(10.0 * u, 9.0 * u, 3.0 * u, 90.0, 180.0)
                    .close()
                    .fill(theme::FG),
                Arc::new()
                    .center(10.0 * u, 9.0 * u)
                    .radius(5.5 * u)
                    .start(90.0)
                    .sweep(180.0)
                    .stroke(line, theme::FG)
                    .cap(Cap::Round),
                stroke(Line::new().from(10.0 * u, 14.5 * u).to(10.0 * u, 17.5 * u)),
            ]
        };

        let shapes = match self {
            Icon::Speaker(percent) => {
                let mut shapes = shapes![speaker()];

                if percent > 0 {
                    shapes.push(Box::new(wave(3.5)));
                }

                if percent > 50 {
                    shapes.push(Box::new(wave(7.0)));
                }

                shapes
            }
            Icon::SpeakerMuted => shapes![
                speaker(),
                stroke(Line::new().from(13.5 * u, 7.5 * u).to(18.5 * u, 12.5 * u)),
                stroke(Line::new().from(18.5 * u, 7.5 * u).to(13.5 * u, 12.5 * u)),
            ],
            Icon::Microphone => microphone(),
            Icon::MicrophoneMuted => {
                let mut shapes = microphone();

                // cut out of the microphone by a body-colored edge, then drawn
                shapes.push(Box::new(
                    Line::new()
                        .from(3.5 * u, 2.5 * u)
                        .to(16.5 * u, 17.5 * u)
                        .stroke(line + 3.0 * u, theme::BODY)
                        .cap(Cap::Round),
                ));
                shapes.push(Box::new(stroke(
                    Line::new().from(3.5 * u, 2.5 * u).to(16.5 * u, 17.5 * u),
                )));

                shapes
            }
            Icon::Sun => {
                let mut shapes = shapes![
                    Circle::new()
                        .center(10.0 * u, 10.0 * u)
                        .radius(3.5 * u)
                        .fill(theme::FG)
                ];

                for ray in 0..8 {
                    let angle = (ray as f32 * 45.0).to_radians();
                    let (x, y) = (angle.sin(), -angle.cos());

                    shapes.push(Box::new(stroke(
                        Line::new()
                            .from((10.0 + 6.0 * x) * u, (10.0 + 6.0 * y) * u)
                            .to((10.0 + 8.0 * x) * u, (10.0 + 8.0 * y) * u),
                    )));
                }

                shapes
            }
        };

        Canvas::new().width(side).height(side).shapes(shapes)
    }
}

/*
 * art, title, then whether it plays. The art's inset matches top, left and bottom, so it sits
 * concentric with the body's round end; the state mark keeps clear of the other end
 */
fn media_compact(track: &Track) -> Rectangle {
    let shape = geometry::shape(Presentation::Compact);
    let inset = 7.0;

    sized(Presentation::Compact)
        .padding(Padding {
            top: 0.0,
            right: 15.0,
            bottom: 0.0,
            left: inset,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                art(track, shape.height - 2.0 * inset, 6.0),
                Text::new(&track.title)
                    .size(13.0)
                    .color(theme::FG)
                    .weight(500)
                    .elide(),
                state(track.playing),
            ])
            .width(Parent)
            .gap(9.0)
            .align(Center),
        )
}

// the Compact with room for the artist under the title
fn media_peek(track: &Track) -> Rectangle {
    let shape = geometry::shape(Presentation::Peek);
    let inset = 7.0;

    let mut lines = children![
        Text::new(&track.title)
            .size(14.0)
            .color(theme::FG)
            .weight(600)
            .elide()
    ];

    // with no artist the title centers alone
    if !track.artist.is_empty() {
        lines.push(Box::new(
            Text::new(&track.artist)
                .size(12.0)
                .color(theme::MUTED)
                .weight(500)
                .elide(),
        ));
    }

    let lines = Column::new(lines).width(Parent).gap(1.0);

    sized(Presentation::Peek)
        .padding(Padding {
            top: 0.0,
            right: 20.0,
            bottom: 0.0,
            left: inset,
        })
        .align_child(Start, Center)
        .child(
            Row::new(children![
                art(track, shape.height - 2.0 * inset, 9.0),
                lines,
                state(track.playing),
            ])
            .width(Parent)
            .gap(11.0)
            .align(Center),
        )
}

/*
 * the cover over a quiet tile with a note, so the tile shows while it decodes, when it never
 * will, and for web art, all at the same size
 */
fn art(track: &Track, side: f32, radius: f32) -> Stack {
    let tile = Rectangle::new()
        .width(side)
        .height(side)
        .radius(radius)
        .fill(theme::ART)
        .align_child(Center, Center)
        .child(Text::new("\u{266a}").size(side * 0.5).color(theme::MUTED));

    let mut layers = children![tile];

    if let Some(path) = &track.art {
        // decoded at twice its size, crisp at scale 2, and the cache keeps no full-size covers
        let pixels = (side * 2.0) as u32;

        layers.push(Box::new(
            Rectangle::new()
                .width(side)
                .height(side)
                .radius(radius)
                .fill(Image::cover(path).thumbnail(pixels, pixels)),
        ));
    }

    Stack::new(layers).width(side).height(side)
}

// three bars while it plays, a pause mark while it does not; still, so playing draws no frames
fn state(playing: bool) -> Row {
    let bar = |height: f32| {
        Rectangle::new()
            .width(3.0)
            .height(height)
            .radius(1.5)
            .fill(theme::FG)
    };

    let bars = if playing {
        children![bar(8.0), bar(13.0), bar(10.0)]
    } else {
        children![bar(11.0), bar(11.0)]
    };

    Row::new(bars).height(13.0).gap(2.0).align(End)
}

// a write wakes the window even when nothing changed, so only write a real change
fn expand(monitor: &str) {
    if !IslandService::read().expanded(monitor) {
        IslandService::write().input(monitor, Input::Click, Instant::now());
    }
}

fn collapse(monitor: &str) {
    if IslandService::read().expanded(monitor) {
        IslandService::write().input(monitor, Input::Collapse, Instant::now());
    }
}

// in arms and starts the hover delay, out disarms and starts the grace; IslandService::listen times both
fn hover(monitor: &str, inside: bool) {
    if IslandService::read().inside(monitor) != inside {
        IslandService::write().hover(monitor, inside, Instant::now());
    }
}

// right click and wheel reach no Presentation yet, so nothing is written for them (#27, #31)
fn route(monitor: &str, input: Input) {
    if input.decides() {
        IslandService::write().input(monitor, input, Instant::now());
    }
}

fn set_armed(monitor: &str, armed: bool) {
    if IslandService::read().armed(monitor) != armed {
        IslandService::write().set_armed(monitor, armed);
    }
}

// geometry already rounded it to whole pixels
fn input_area(area: Rect) -> InputArea {
    InputArea {
        x: area.x as i32,
        y: area.y as i32,
        width: area.width as i32,
        height: area.height as i32,
    }
}
