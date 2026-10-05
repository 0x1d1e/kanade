use std::time::Instant;

use amane::{
    Arc, Button, Canvas, Cap, Center, Circle, Color, Column, End, Horizontal, Image, InputArea,
    Key, Layer, LayerWindow, Line, Monitor, Padding, Parent, Path, Rectangle, Row, Scroll, Service,
    Shape as _, Stack, Start, Text, Vertical, Widget, Zone, children, request_frame, shapes,
};

use crate::island::activity::{
    Activity, Charge, Connection, Detail, Device, Frame, Kind, Peer, Toast, Track, Uplink, Volume,
    Workspace,
};
use crate::island::geometry::{self, Rect, Shape};
use crate::island::presentation::{Content, Input, Presentation, Surface};
use crate::island::service::IslandService;
use crate::surfaces;
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
        && let Some(form) = small_form(&content)
            .or_else(|| surface(&monitor.name, &content, &island, now))
            .or_else(|| placeholder(&content))
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
            } else {
                surfaces::notifications::key(&pressed, key);
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
        .map(satellite_label)
        .chain((frame.overflow > 0).then(|| (format!("+{}", frame.overflow), theme::FG)));

    labels
        .enumerate()
        .map(|(index, (label, tone))| {
            let at = geometry::satellite(body, index);

            Box::new(dot(at, label, tone).opacity(opacity)) as Box<dyn Widget>
        })
        .collect()
}

// a battery shows its number in its tone (plan 7), the rest their Kind
fn satellite_label(activity: &Activity) -> (String, Color) {
    match activity.detail() {
        Detail::Battery(charge) => (charge.percent.to_string(), charge_tone(charge)),
        _ => (abbreviation(activity.kind()).to_owned(), theme::FG),
    }
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

    Some(dot(at, frame.queued.len().to_string(), theme::FG).opacity(opacity))
}

fn dot(at: Rect, label: String, tone: Color) -> Rectangle {
    Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(at.width / 2.0)
        .fill(theme::DOT)
        .align_child(Center, Center)
        .translate(at.x, at.y)
        .child(Text::new(label).size(12.0).color(tone).weight(600))
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

/*
 * the open Surface's own content, from the view's read of the island; the Surface open now differs
 * from the content's while it fades out
 */
fn surface(
    monitor: &str,
    content: &Content,
    island: &IslandService,
    now: Instant,
) -> Option<Rectangle> {
    let Presentation::Expanded(surface) = content.presentation else {
        return None;
    };

    let open = island.surface() == Some(surface);

    match surface {
        Surface::Media => Some(surfaces::media::surface(open, now)),
        Surface::Notifications => Some(surfaces::notifications::surface(
            monitor,
            open,
            island.visit(),
            island.held(monitor),
            island.dnd(),
        )),
        _ => None,
    }
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
        (Presentation::Compact, Some(Detail::Notification(toast))) => Some(toast_compact(toast)),
        (Presentation::Peek, Some(Detail::Notification(toast))) => Some(toast_peek(toast)),
        (presentation, Some(Detail::Volume(volume))) => level(presentation, Level::volume(volume)),
        (presentation, Some(&Detail::Brightness(percent))) => {
            level(presentation, Level::brightness(percent))
        }
        (presentation, Some(Detail::Battery(charge))) => battery(presentation, charge),
        (presentation, Some(Detail::Workspace(workspace))) => {
            self::workspace(presentation, workspace)
        }
        (presentation, Some(Detail::Network(connection))) => {
            link(presentation, Link::network(connection))
        }
        (presentation, Some(Detail::Bluetooth(peer))) => link(presentation, Link::bluetooth(peer)),
        _ => None,
    }
}

// amber while low, red once critical, and never color alone: the number and the words say it too
fn charge_tone(charge: &Charge) -> Color {
    if charge.critical {
        theme::RED
    } else {
        theme::AMBER
    }
}

/*
 * battery, what it means, the number; Peek says what to do about it. Fixed widths, so a number
 * that ticks down redraws in place
 */
fn battery(presentation: Presentation, charge: &Charge) -> Option<Rectangle> {
    let (icon, number, size, inset) = match presentation {
        Presentation::Compact => (22.0, 40.0, 13.0, 15.0),
        Presentation::Peek => (26.0, 48.0, 17.0, 20.0),
        _ => return None,
    };

    let gap = 11.0;
    let width = geometry::shape(presentation).width - 2.0 * inset - icon - number - 2.0 * gap;
    let tone = charge_tone(charge);

    let title = if charge.critical {
        "Battery Critical"
    } else {
        "Low Battery"
    };

    let mut words = children![Text::new(title).size(13.0).color(theme::FG).weight(600)];

    if presentation == Presentation::Peek {
        words.push(Box::new(
            Text::new("Plug in to charge")
                .size(12.0)
                .color(theme::MUTED)
                .weight(500),
        ));
    }

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
                    Box::new(battery_icon(icon, charge.percent, tone)) as Box<dyn Widget>,
                    Box::new(Column::new(words).width(width).gap(3.0)),
                    Box::new(
                        Rectangle::new()
                            .width(number)
                            .height(size)
                            .align_child(End, Center)
                            .child(
                                Text::new(format!("{}%", charge.percent))
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

// a battery on its side, filled as far as it is charged, a sliver at least so empty still reads
fn battery_icon(width: f32, percent: u8, tone: Color) -> Row {
    let nub = width / 11.0;
    let shell = width - nub - 1.0;
    let height = width / 2.0;
    let border = 1.5;
    let inner = shell - 2.0 * border - 2.0;
    let filled = (inner * f32::from(percent.min(100)) / 100.0).max(2.0);

    let body = Rectangle::new()
        .width(shell)
        .height(height)
        .radius(height / 3.5)
        .border(border, tone)
        .padding(border + 1.0)
        .align_child(Start, Center)
        .child(
            Rectangle::new()
                .width(filled)
                .height(height - 2.0 * border - 2.0)
                .radius(1.5)
                .fill(tone),
        );

    let tip = Rectangle::new()
        .width(nub)
        .height(height / 2.5)
        .radius(nub / 2.0)
        .fill(tone);

    Row::new(children![body, tip])
        .width(width)
        .gap(1.0)
        .align(Center)
}

// past this many workspaces on an output the pager would not fit, so the numbers say it instead
const PAGER: u32 = 10;

/*
 * the workspace's name, then where it is among its output's workspaces; Peek says its number
 * under a name. The pager has a fixed width, so a switch moves its mark and nothing else
 */
fn workspace(presentation: Presentation, workspace: &Workspace) -> Option<Rectangle> {
    let (size, dot, inset) = match presentation {
        Presentation::Compact => (13.0, 6.0, 17.0),
        Presentation::Peek => (15.0, 7.0, 22.0),
        _ => return None,
    };

    let number = format!("Workspace {}", workspace.index);

    let mut words = children![
        Text::new(workspace.name.as_deref().unwrap_or(&number))
            .size(size)
            .color(theme::FG)
            .weight(600)
            .elide()
    ];

    if presentation == Presentation::Peek && workspace.name.is_some() {
        words.push(Box::new(
            Text::new(number).size(12.0).color(theme::MUTED).weight(500),
        ));
    }

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
                    Box::new(Column::new(words).width(Parent).gap(1.0)) as Box<dyn Widget>,
                    pager(workspace, dot),
                ])
                .width(Parent)
                .gap(12.0)
                .align(Center),
            ),
    )
}

// a dot per workspace and a longer mark for the focused one, or its number past `PAGER`
fn pager(workspace: &Workspace, dot: f32) -> Box<dyn Widget> {
    let (mark, gap) = (dot * 8.0 / 3.0, dot * 5.0 / 6.0);

    if workspace.count > PAGER {
        return Box::new(
            Text::new(format!("{} / {}", workspace.index, workspace.count))
                .size(13.0)
                .color(theme::MUTED)
                .weight(600),
        );
    }

    let dots = (1..=workspace.count)
        .map(|index| {
            let focused = index == workspace.index;

            Box::new(
                Rectangle::new()
                    .width(if focused { mark } else { dot })
                    .height(dot)
                    .radius(dot / 2.0)
                    .fill(if focused { theme::FG } else { theme::MUTED }),
            ) as Box<dyn Widget>
        })
        .collect();

    Box::new(Row::new(dots).gap(gap).align(Center))
}

// a network or a Bluetooth device that came or went, the same layout for both
struct Link {
    icon: Icon,
    name: String,

    // what happened, which Peek says under the name
    status: &'static str,

    connected: bool,

    // a device's own, while connected
    battery: Option<u8>,
}

impl Link {
    fn network(connection: &Connection) -> Link {
        let connected = connection.connected;

        let (icon, name, status) = match &connection.uplink {
            Uplink::Wifi(ssid) => (
                Icon::Wifi,
                ssid.clone(),
                if connected {
                    "Wi-Fi connected"
                } else {
                    "Wi-Fi disconnected"
                },
            ),
            Uplink::Wired => (Icon::Wired, String::from("Ethernet"), status(connected)),
            Uplink::Other(name) => (Icon::Shield, name.clone(), status(connected)),
        };

        Link {
            icon,
            name,
            status,
            connected,
            battery: None,
        }
    }

    fn bluetooth(peer: &Peer) -> Link {
        Link {
            icon: Icon::Bluetooth,
            name: peer.name.clone(),
            status: status(peer.connected),
            connected: peer.connected,
            battery: peer.battery.filter(|_| peer.connected),
        }
    }
}

fn status(connected: bool) -> &'static str {
    if connected {
        "Connected"
    } else {
        "Disconnected"
    }
}

/*
 * icon, name, then a device's battery; Peek says what happened under the name. Gone is the icon
 * struck through, so it never rests on color alone
 */
fn link(presentation: Presentation, link: Link) -> Option<Rectangle> {
    let (icon, size, inset) = match presentation {
        Presentation::Compact => (18.0, 13.0, 15.0),
        Presentation::Peek => (24.0, 15.0, 20.0),
        _ => return None,
    };

    let mut words = children![
        Text::new(link.name)
            .size(13.0)
            .color(theme::FG)
            .weight(600)
            .elide()
    ];

    if presentation == Presentation::Peek {
        words.push(Box::new(
            Text::new(link.status)
                .size(12.0)
                .color(theme::MUTED)
                .weight(500),
        ));
    }

    let icon = if link.connected {
        link.icon.draw(icon)
    } else {
        link.icon.crossed(icon)
    };

    let mut row = vec![
        Box::new(icon) as Box<dyn Widget>,
        Box::new(Column::new(words).width(Parent).gap(3.0)),
    ];

    if let Some(percent) = link.battery {
        row.push(Box::new(
            Text::new(format!("{percent}%"))
                .size(size)
                .color(theme::MUTED)
                .weight(600),
        ));
    }

    Some(
        sized(presentation)
            .padding(Padding {
                top: 0.0,
                right: inset,
                bottom: 0.0,
                left: inset,
            })
            .align_child(Start, Center)
            .child(Row::new(row).width(Parent).gap(11.0).align(Center)),
    )
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

    let bar = bar(width, f32::from(level.percent) / 100.0, tone);

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

// `fraction` of it filled, 0 to 1
pub(crate) fn bar(width: f32, fraction: f32, tone: Color) -> Stack {
    let height = 6.0;
    let filled = width * fraction.clamp(0.0, 1.0);

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
pub(crate) enum Icon {
    // waves by level: none silent, one up to half, two above
    Speaker(u8),
    SpeakerMuted,
    Microphone,
    MicrophoneMuted,
    Sun,
    Wifi,
    Wired,
    Shield,
    Bluetooth,
    Bell,

    // Do Not Disturb, cut out of the body's color like the slash
    Moon,
}

impl Icon {
    fn muted(self) -> Icon {
        match self {
            Icon::Speaker(_) => Icon::SpeakerMuted,
            Icon::Microphone => Icon::MicrophoneMuted,
            icon => icon,
        }
    }

    pub(crate) fn draw(self, side: f32) -> Canvas {
        self.canvas(side, false)
    }

    // struck through, for something gone or off
    pub(crate) fn crossed(self, side: f32) -> Canvas {
        self.canvas(side, true)
    }

    // on a 20 unit grid, scaled to `side`
    fn canvas(self, side: f32, crossed: bool) -> Canvas {
        let u = side / 20.0;
        let line = 1.7 * u;
        let stroke = |shape: Line| shape.stroke(line, theme::FG).cap(Cap::Round);

        // cut out of the icon by a body-colored edge, then drawn
        let slash = || {
            let from = (3.5 * u, 2.5 * u);
            let to = (16.5 * u, 17.5 * u);

            shapes![
                Line::new()
                    .from(from.0, from.1)
                    .to(to.0, to.1)
                    .stroke(line + 3.0 * u, theme::BODY)
                    .cap(Cap::Round),
                stroke(Line::new().from(from.0, from.1).to(to.0, to.1)),
            ]
        };

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

        let mut shapes = match self {
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
            Icon::MicrophoneMuted => microphone(),
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
            Icon::Wifi => {
                let mut shapes = shapes![
                    Circle::new()
                        .center(10.0 * u, 15.5 * u)
                        .radius(1.6 * u)
                        .fill(theme::FG)
                ];

                for radius in [4.5, 8.0, 11.5] {
                    shapes.push(Box::new(
                        Arc::new()
                            .center(10.0 * u, 16.0 * u)
                            .radius(radius * u)
                            .start(315.0)
                            .sweep(90.0)
                            .stroke(line, theme::FG)
                            .cap(Cap::Round),
                    ));
                }

                shapes
            }
            // an Ethernet port: the socket and its latch
            Icon::Wired => shapes![
                Path::new()
                    .move_to(3.5 * u, 7.0 * u)
                    .line_to(7.0 * u, 7.0 * u)
                    .line_to(7.0 * u, 4.5 * u)
                    .line_to(13.0 * u, 4.5 * u)
                    .line_to(13.0 * u, 7.0 * u)
                    .line_to(16.5 * u, 7.0 * u)
                    .line_to(16.5 * u, 15.5 * u)
                    .line_to(3.5 * u, 15.5 * u)
                    .close()
                    .stroke(line, theme::FG),
                stroke(Line::new().from(7.5 * u, 11.5 * u).to(7.5 * u, 12.5 * u)),
                stroke(Line::new().from(10.0 * u, 11.5 * u).to(10.0 * u, 12.5 * u)),
                stroke(Line::new().from(12.5 * u, 11.5 * u).to(12.5 * u, 12.5 * u)),
            ],
            // a shield, as VPNs are drawn
            Icon::Shield => shapes![
                Path::new()
                    .move_to(10.0 * u, 2.5 * u)
                    .line_to(16.0 * u, 5.0 * u)
                    .line_to(16.0 * u, 9.5 * u)
                    .quad_to(16.0 * u, 15.0 * u, 10.0 * u, 17.5 * u)
                    .quad_to(4.0 * u, 15.0 * u, 4.0 * u, 9.5 * u)
                    .line_to(4.0 * u, 5.0 * u)
                    .close()
                    .stroke(line, theme::FG)
            ],
            // the rune
            Icon::Bluetooth => shapes![
                Path::new()
                    .move_to(5.5 * u, 6.5 * u)
                    .line_to(14.0 * u, 13.5 * u)
                    .line_to(10.0 * u, 17.0 * u)
                    .line_to(10.0 * u, 3.0 * u)
                    .line_to(14.0 * u, 6.5 * u)
                    .line_to(5.5 * u, 13.5 * u)
                    .stroke(line, theme::FG)
                    .cap(Cap::Round)
            ],
            Icon::Bell => shapes![
                Path::new()
                    .move_to(3.5 * u, 14.5 * u)
                    .line_to(16.5 * u, 14.5 * u)
                    .line_to(15.0 * u, 12.5 * u)
                    .line_to(15.0 * u, 8.5 * u)
                    .quad_to(15.0 * u, 3.5 * u, 10.0 * u, 3.5 * u)
                    .quad_to(5.0 * u, 3.5 * u, 5.0 * u, 8.5 * u)
                    .line_to(5.0 * u, 12.5 * u)
                    .close()
                    .stroke(line, theme::FG)
                    .cap(Cap::Round),
                Arc::new()
                    .center(10.0 * u, 15.5 * u)
                    .radius(2.0 * u)
                    .start(90.0)
                    .sweep(180.0)
                    .stroke(line, theme::FG)
                    .cap(Cap::Round),
            ],
            Icon::Moon => shapes![
                Circle::new()
                    .center(10.0 * u, 10.0 * u)
                    .radius(7.0 * u)
                    .fill(theme::FG),
                Circle::new()
                    .center(14.0 * u, 6.5 * u)
                    .radius(6.0 * u)
                    .fill(theme::BODY),
            ],
        };

        if crossed || matches!(self, Icon::MicrophoneMuted) {
            shapes.extend(slash());
        }

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
pub(crate) fn art(track: &Track, side: f32, radius: f32) -> Stack {
    tile(track.art.as_deref(), "\u{266a}", side, radius)
}

// a picture over a quiet tile with a mark, like `art`
fn tile(picture: Option<&str>, mark: &str, side: f32, radius: f32) -> Stack {
    let tile = Rectangle::new()
        .width(side)
        .height(side)
        .radius(radius)
        .fill(theme::ART)
        .align_child(Center, Center)
        .child(Text::new(mark).size(side * 0.5).color(theme::MUTED));

    let mut layers = children![tile];

    if let Some(path) = picture {
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

// the sender's picture, or its initial on the quiet tile
pub(crate) fn toast_tile(toast: &Toast, side: f32, radius: f32) -> Stack {
    let sender = if toast.app.is_empty() {
        &toast.summary
    } else {
        &toast.app
    };
    let initial = sender
        .chars()
        .next()
        .map_or_else(String::new, |initial| initial.to_uppercase().collect());

    tile(toast.image.as_deref(), &initial, side, radius)
}

// picture, then the summary, laid out like the media Compact
fn toast_compact(toast: &Toast) -> Rectangle {
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
                toast_tile(toast, shape.height - 2.0 * inset, 6.0),
                Text::new(&toast.summary)
                    .size(13.0)
                    .color(theme::FG)
                    .weight(500)
                    .elide(),
            ])
            .width(Parent)
            .gap(9.0)
            .align(Center),
        )
}

// the Compact with the body under the summary, or the sender when the summary is not its name
fn toast_peek(toast: &Toast) -> Rectangle {
    let shape = geometry::shape(Presentation::Peek);
    let inset = 7.0;

    let mut lines = children![
        Text::new(&toast.summary)
            .size(14.0)
            .color(theme::FG)
            .weight(600)
            .elide()
    ];

    let second = if toast.body.is_empty() && toast.app != toast.summary {
        &toast.app
    } else {
        &toast.body
    };

    // with nothing more to say the summary centers alone
    if !second.is_empty() {
        lines.push(Box::new(
            Text::new(second)
                .size(12.0)
                .color(theme::MUTED)
                .weight(500)
                .elide(),
        ));
    }

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
                toast_tile(toast, shape.height - 2.0 * inset, 9.0),
                Column::new(lines).width(Parent).gap(1.0),
            ])
            .width(Parent)
            .gap(11.0)
            .align(Center),
        )
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

pub(crate) fn collapse(monitor: &str) {
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
