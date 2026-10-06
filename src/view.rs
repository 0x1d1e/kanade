use std::time::Instant;

use amane::{
    Button, Center, Color, Column, End, Horizontal, Image, InputArea, Key, Layer, LayerWindow,
    Monitor, Padding, Parent, Rectangle, Row, Scroll, Service, Stack, Start, Text, Vertical,
    Widget, Zone, children, request_frame,
};

use crate::clock;
use crate::config;
use crate::icon::Icon;
use crate::island::activity::{
    Activity, Charge, Connection, Countdown, Detail, Device, Frame, Kind, Peer, Priority, Sensors,
    Toast, Track, Uplink, Volume, Workspace,
};
use crate::island::fade::{Dissolve, InPlace, swap};
use crate::island::geometry::{self, Rect, Shape};
use crate::island::presentation::{Content, Input, Presentation, Surface};
use crate::island::satellites::{Mark, Satellites};
use crate::island::service::IslandService;
use crate::modules;
use crate::shadow::{self, ShadowStyle};
use crate::sources::timer;
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
        .fill(theme::body())
        .clip()
        .align_child(Center, Start)
        .translate(body_rect.x - area.x, body_rect.y - area.y);

    // a pinned island says why it stays open with nobody on it
    if island.pinned(&monitor.name) {
        body = body.border(1.0, theme::pin());
    }

    // at most one shows at a time, the crossfade hands over through nothing
    if let Some((content, opacity)) = content.into_iter().flatten().next()
        && let Some(form) = small_form(&content, island.track(&monitor.name), now)
            .or_else(|| surface(&monitor.name, &content, &island, now))
            .or_else(|| rest(&content))
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

    let mut layers = Vec::new();

    shadow::draw(&mut layers, body_rect, shape.radius, ShadowStyle::island());

    // the body grows over the Satellites as they fade, and they never take the pointer
    if !island.overview() {
        layers.extend(satellites(
            island.satellites(&monitor.name),
            body_rect,
            shape,
            now,
        ));
    }

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
            } else if !surfaces::notifications::key(&pressed, key)
                && !surfaces::launcher::key(&pressed, key)
            {
                stray(&pressed, key);
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
fn satellites(
    satellites: &Satellites,
    body: Rect,
    shape: Shape,
    now: Instant,
) -> Vec<Box<dyn Widget>> {
    let opacity = geometry::satellite_opacity(shape);

    if opacity == 0.0 {
        return Vec::new();
    }

    satellites
        .shown(now)
        .into_iter()
        .map(|shown| {
            let mark = match shown.mark {
                Mark::Activity(activity) => satellite_mark(activity, now),
                Mark::Overflow(count) => label(format!("+{count}"), theme::fg()),
            };
            let at = geometry::satellite(body, shown.slot, shown.presence);

            Box::new(dot(at, mark).opacity(opacity * shown.opacity)) as Box<dyn Widget>
        })
        .collect()
}

/*
 * a battery shows its number in its tone, a screen capture its glyph in amber, a microphone or
 * camera in use its glyph in green (plan 7), a timer what is left, the rest their Kind
 */
fn satellite_mark(activity: &Activity, now: Instant) -> Box<dyn Widget> {
    match activity.detail() {
        Detail::Battery(charge) => label(charge.percent.to_string(), charge_tone(charge)),
        Detail::Timer(countdown) => label(timer::short(countdown, now), theme::fg()),
        Detail::Privacy(sensors) => Box::new(sensor(sensors).on(16.0, theme::GREEN)),
        _ if activity.kind() == Kind::ScreenCast => Box::new(Icon::Capture.on(16.0, theme::AMBER)),
        _ => label(abbreviation(activity.kind()).to_owned(), theme::fg()),
    }
}

fn label(text: String, tone: Color) -> Box<dyn Widget> {
    Box::new(Text::new(text).size(12.0).color(tone).weight(600))
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

    Some(dot(at, label(frame.queued.len().to_string(), theme::fg())).opacity(opacity))
}

fn dot(at: Rect, mark: Box<dyn Widget>) -> Rectangle {
    Rectangle::new()
        .width(at.width)
        .height(at.height)
        .radius(at.width / 2.0)
        .fill(theme::dot())
        .align_child(Center, Center)
        .translate(at.x, at.y)
        .child(Row::new(vec![mark]))
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
 * the local time while nothing is happening; an Activity's small form crossfades over it, and
 * only a window that draws it redraws when the minute turns
 */
fn rest(content: &Content) -> Option<Rectangle> {
    if content.presentation != Presentation::Rest {
        return None;
    }

    Some(
        sized(Presentation::Rest).align_child(Center, Center).child(
            Text::new(clock::now(config::get().clock))
                .size(13.0)
                .color(theme::fg())
                .weight(600),
        ),
    )
}

/*
 * stand-in content until the sources and the Surfaces draw their own (#21-#33): names the Activity
 * a small form shows, or the open Surface, so the crossfade and the clipping can be seen. Short,
 * so sized to its letters and centered
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
            .child(Text::new(label).size(size).color(theme::fg()).weight(500)),
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
        Surface::Controls => Some(surfaces::controls::surface(
            modules::on("notifications").then(|| island.dnd()),
        )),
        Surface::Launcher => Some(surfaces::launcher::surface(monitor, island.visit())),
    }
}

// content is laid out at its Presentation's final size, which the morph reveals
fn sized(presentation: Presentation) -> Rectangle {
    let size = geometry::shape(presentation);

    Rectangle::new().width(size.width).height(size.height)
}

// an Activity's own Compact or Peek, drawn from its Detail; none leaves it to the placeholder
fn small_form(
    content: &Content,
    track: Option<&Dissolve<Track>>,
    now: Instant,
) -> Option<Rectangle> {
    if content.activity.as_ref().map(Activity::kind) == Some(Kind::ScreenCast) {
        return capture(content.presentation);
    }

    let detail = content.activity.as_ref().map(Activity::detail);

    match (content.presentation, detail) {
        (Presentation::Compact, Some(Detail::Media(shown))) => {
            Some(media_compact(Change::of(shown, track, now)))
        }
        (Presentation::Peek, Some(Detail::Media(shown))) => {
            Some(media_peek(Change::of(shown, track, now)))
        }
        (Presentation::Compact, Some(Detail::Notification(toast))) => {
            Some(toast_compact(toast, critical(content)))
        }
        (Presentation::Peek, Some(Detail::Notification(toast))) => {
            Some(toast_peek(toast, critical(content)))
        }
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
        (presentation, Some(Detail::Timer(countdown))) => self::timer(presentation, countdown, now),
        (presentation, Some(Detail::Privacy(sensors))) => privacy(presentation, sensors),
        _ => None,
    }
}

/*
 * the glyph and a word, both amber, so it never rests on color alone and never reads as the low
 * battery's amber (plan 7). Niri cannot say why the screen is captured, so it never says
 * "Recording" or "Sharing" (plan 5.3)
 */
fn capture(presentation: Presentation) -> Option<Rectangle> {
    let (icon, text, size, inset) = match presentation {
        Presentation::Compact => (18.0, "CAPTURE", 13.0, 15.0),
        Presentation::Peek => (24.0, "Screen capture active", 15.0, 20.0),
        _ => return None,
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
                Row::new(children![
                    Icon::Capture.on(icon, theme::AMBER),
                    Text::new(text)
                        .size(size)
                        .color(theme::AMBER)
                        .weight(600)
                        .elide()
                ])
                .width(Parent)
                .gap(11.0)
                .align(Center),
            ),
    )
}

// the camera when it is in use, being the more private one, else the microphone
fn sensor(sensors: &Sensors) -> Icon {
    if sensors.camera {
        Icon::Camera
    } else {
        Icon::Microphone
    }
}

/*
 * the sensor's glyph and what is in use, both green, so it never rests on color alone (plan 7);
 * Peek also names the apps capturing
 */
fn privacy(presentation: Presentation, sensors: &Sensors) -> Option<Rectangle> {
    let (icon, inset) = match presentation {
        Presentation::Compact => (18.0, 15.0),
        Presentation::Peek => (24.0, 20.0),
        _ => return None,
    };

    let words: Box<dyn Widget> = match (presentation, sensors.microphone, sensors.camera) {
        (Presentation::Compact, true, true) => Box::new(green("CAMERA + MIC", 13.0)),
        (Presentation::Compact, false, _) => Box::new(green("CAMERA", 13.0)),
        (Presentation::Compact, _, false) => Box::new(green("MIC", 13.0)),
        (_, microphone, camera) => {
            let what = match (microphone, camera) {
                (true, true) => "Camera and microphone in use",
                (false, _) => "Camera in use",
                (_, false) => "Microphone in use",
            };

            let mut lines = children![green(what, 15.0)];

            if !sensors.apps.is_empty() {
                lines.push(Box::new(
                    Text::new(sensors.apps.join(", "))
                        .size(12.0)
                        .color(theme::muted())
                        .weight(500)
                        .elide(),
                ));
            }

            Box::new(Column::new(lines).width(Parent).gap(3.0))
        }
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
                    Box::new(sensor(sensors).on(icon, theme::GREEN)),
                    words,
                ])
                .width(Parent)
                .gap(11.0)
                .align(Center),
            ),
    )
}

fn green(text: &str, size: f32) -> Text {
    Text::new(text)
        .size(size)
        .color(theme::GREEN)
        .weight(600)
        .elide()
}

/*
 * the stopwatch, what it is, then what is left; Peek also says how long it was started for. The
 * clock has a fixed width, so a second ticking by redraws it in place
 */
fn timer(presentation: Presentation, countdown: &Countdown, now: Instant) -> Option<Rectangle> {
    // h:mm:ss once the timer was started for an hour or more, m:ss below
    let hours = countdown.length.as_secs() >= 3600;

    let (icon, size, inset, clock) = match (presentation, hours) {
        (Presentation::Compact, false) => (18.0, 13.0, 15.0, 40.0),
        (Presentation::Compact, true) => (18.0, 13.0, 15.0, 56.0),
        (Presentation::Peek, false) => (24.0, 17.0, 20.0, 52.0),
        (Presentation::Peek, true) => (24.0, 17.0, 20.0, 72.0),
        _ => return None,
    };

    let gap = 11.0;
    let width = geometry::shape(presentation).width - 2.0 * inset - icon - clock - 2.0 * gap;

    let mut words = children![Text::new("Timer").size(13.0).color(theme::fg()).weight(600)];

    if presentation == Presentation::Peek {
        words.push(Box::new(
            Text::new(format!("{} timer", timer::length(countdown)))
                .size(12.0)
                .color(theme::muted())
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
                Row::new(children![
                    Icon::Stopwatch.draw(icon),
                    Column::new(words).width(width).gap(3.0),
                    Rectangle::new()
                        .width(clock)
                        .height(size)
                        .align_child(End, Center)
                        .child(
                            Text::new(timer::clock(countdown, now))
                                .size(size)
                                .color(theme::fg())
                                .weight(600),
                        ),
                ])
                .gap(gap)
                .align(Center),
            ),
    )
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

    let mut words = children![Text::new(title).size(13.0).color(theme::fg()).weight(600)];

    if presentation == Presentation::Peek {
        words.push(Box::new(
            Text::new("Plug in to charge")
                .size(12.0)
                .color(theme::muted())
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
            .color(theme::fg())
            .weight(600)
            .elide()
    ];

    if presentation == Presentation::Peek && workspace.name.is_some() {
        words.push(Box::new(
            Text::new(number)
                .size(12.0)
                .color(theme::muted())
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
                .color(theme::muted())
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
                    .fill(if focused { theme::fg() } else { theme::muted() }),
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
            .color(theme::fg())
            .weight(600)
            .elide()
    ];

    if presentation == Presentation::Peek {
        words.push(Box::new(
            Text::new(link.status)
                .size(12.0)
                .color(theme::muted())
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
                .color(theme::muted())
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

    let tone = if level.quiet {
        theme::muted()
    } else {
        theme::fg()
    };

    let bar = bar(width, f32::from(level.percent) / 100.0, tone);

    let middle: Box<dyn Widget> = match presentation {
        Presentation::Peek => Box::new(
            Column::new(children![
                Text::new(level.label)
                    .size(12.0)
                    .color(theme::muted())
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
        .fill(theme::dot());

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

/*
 * art, title, then whether it plays. The art's inset matches top, left and bottom, so it sits
 * concentric with the body's round end; the state mark keeps clear of the other end
 */
fn media_compact(change: Change) -> Rectangle {
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
                change.art(shape.height - 2.0 * inset, 6.0),
                change
                    .line(|track| &track.title, theme::fg())
                    .size(13.0)
                    .weight(500)
                    .elide(),
                state(change.to.playing),
            ])
            .width(Parent)
            .gap(9.0)
            .align(Center),
        )
}

// the Compact with room for the artist under the title
fn media_peek(change: Change) -> Rectangle {
    let shape = geometry::shape(Presentation::Peek);
    let inset = 7.0;

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
                change.art(shape.height - 2.0 * inset, 9.0),
                peek_lines(change),
                state(change.to.playing),
            ])
            .width(Parent)
            .gap(11.0)
            .align(Center),
        )
}

/*
 * the title over the artist; the artist's line is there even when it names none, so a track
 * without one stands where the one before it did and nothing moves
 */
fn peek_lines(change: Change) -> Column {
    Column::new(children![
        change
            .line(|track| &track.title, theme::fg())
            .size(14.0)
            .weight(600)
            .elide(),
        change
            .line(|track| &track.artist, theme::muted())
            .size(12.0)
            .weight(500)
            .elide(),
    ])
    .width(Parent)
    .gap(1.0)
}

/*
 * a track as it dissolves where it stands (#37): the one it replaces still beneath, and how far
 * the new one has risen over it. Only a dissolve to the track drawn counts, since a Media Activity
 * that fades out whole has a Crossfade that moved on from it
 */
#[derive(Clone, Copy)]
pub(crate) struct Change<'a> {
    from: Option<&'a Track>,
    to: &'a Track,
    rise: f32,
}

impl<'a> Change<'a> {
    pub(crate) fn of(to: &'a Track, dissolve: Option<&'a Dissolve<Track>>, now: Instant) -> Self {
        match dissolve.filter(|dissolve| dissolve.target().in_place(to)) {
            Some(dissolve) => Self {
                from: dissolve.from(now),
                to,
                rise: dissolve.rise(now),
            },
            None => Self {
                from: None,
                to,
                rise: 1.0,
            },
        }
    }

    // the new cover rises over the old, so no tile shows between them and nothing dips
    pub(crate) fn art(self, side: f32, radius: f32) -> Stack {
        let to = Rectangle::new()
            .width(side)
            .height(side)
            .opacity(self.rise)
            .child(art(self.to, side, radius));

        let layers = match self.from {
            Some(from) => children![art(from, side, radius), to],
            None => children![to],
        };

        Stack::new(layers).width(side).height(side)
    }

    // what one line shows and how strongly; a line both tracks share stays as it is
    fn text(self, line: fn(&Track) -> &str) -> (&'a str, f32) {
        let from = self.from.map(line).filter(|&from| from != line(self.to));

        swap(from, line(self.to), self.rise)
    }

    // one line of text, the old fading out before the new fades in at the same place
    pub(crate) fn line(self, line: fn(&Track) -> &str, color: Color) -> Text {
        let (text, opacity) = self.text(line);

        Text::new(text).color(theme::faded(color, opacity))
    }
}

/*
 * the cover over a quiet tile with a note, so the tile shows while it decodes, when it never
 * will, and for web art, all at the same size
 */
pub(crate) fn art(track: &Track, side: f32, radius: f32) -> Stack {
    tile(track.art.as_deref(), "\u{266a}", side, radius)
}

// a picture over a quiet tile with a mark, like `art`
pub(crate) fn tile(picture: Option<&str>, mark: &str, side: f32, radius: f32) -> Stack {
    let tile = Rectangle::new()
        .width(side)
        .height(side)
        .radius(radius)
        .fill(theme::art())
        .align_child(Center, Center)
        .child(Text::new(mark).size(side * 0.5).color(theme::muted()));

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

fn critical(content: &Content) -> bool {
    content
        .activity
        .as_ref()
        .is_some_and(|activity| activity.priority() == Priority::Critical)
}

// the summary, said Critical in words first as the Notifications Surface does, never color alone
fn summary(toast: &Toast, critical: bool, size: f32, weight: u16) -> Box<dyn Widget> {
    let summary = Text::new(&toast.summary)
        .size(size)
        .color(theme::fg())
        .weight(weight)
        .elide();

    if critical {
        Box::new(
            Row::new(children![
                Text::new("Critical")
                    .size(size)
                    .color(theme::RED)
                    .weight(600),
                summary,
            ])
            .width(Parent)
            .gap(6.0),
        )
    } else {
        Box::new(summary)
    }
}

// picture, then the summary, laid out like the media Compact
fn toast_compact(toast: &Toast, critical: bool) -> Rectangle {
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
            Row::new(vec![
                Box::new(toast_tile(toast, shape.height - 2.0 * inset, 6.0)),
                summary(toast, critical, 13.0, 500),
            ])
            .width(Parent)
            .gap(9.0)
            .align(Center),
        )
}

// the Compact with the body under the summary, or the sender when the summary is not its name
fn toast_peek(toast: &Toast, critical: bool) -> Rectangle {
    let shape = geometry::shape(Presentation::Peek);
    let inset = 7.0;

    let mut lines = vec![summary(toast, critical, 14.0, 600)];

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
                .color(theme::muted())
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
            .fill(theme::fg())
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

// closes an open Surface or ends a pinned Peek
pub(crate) fn collapse(monitor: &str) {
    let island = IslandService::read();
    let raised = island.expanded(monitor) || island.pinned(monitor);
    drop(island);

    if raised {
        IslandService::write().input(monitor, Input::Collapse, Instant::now());
    }
}

// in arms and starts the hover delay, out disarms and starts the grace; IslandService::listen times both
fn hover(monitor: &str, inside: bool) {
    if IslandService::read().inside(monitor) != inside {
        IslandService::write().hover(monitor, inside, Instant::now());
    }
}

// the wheel reaches no Presentation yet, so nothing is written for it (#27)
fn route(monitor: &str, input: Input) {
    if input.decides() {
        IslandService::write().input(monitor, input, Instant::now());
    }
}

// a right click on the open Surface's own targets, which only the Expanded island shows
pub(crate) fn pin() {
    let monitor = IslandService::read().expanded_on().map(str::to_owned);

    if let Some(monitor) = monitor {
        route(&monitor, Input::RightClick);
    }
}

/*
 * only the Launcher takes typing, and it consumes its keys first, so a character typed into any
 * other held island the pointer never reached was meant for the window beneath: the island lets go of the keyboard
 * before a Space or Enter presses anything
 */
fn stray(monitor: &str, key: Key) {
    let island = IslandService::read();
    let stray = island.expanded(monitor)
        && island.held(monitor)
        && !island.inside(monitor)
        && matches!(key, Key::Character(_));
    drop(island);

    if stray {
        collapse(monitor);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artist: &str) -> Track {
        Track {
            title: title.into(),
            artist: artist.into(),
            ..Track::default()
        }
    }

    fn rises() -> impl Iterator<Item = f32> {
        (0..=20).map(|step| step as f32 / 20.0)
    }

    // #37: the Peek's lines stand still while a track with an artist dissolves to one without
    #[test]
    fn the_peek_lines_keep_their_height_whether_an_artist_shows_or_not() {
        let with = track("a", "Alpha");
        let without = track("b", "");
        let settled = |to| Widget::height(&peek_lines(Change::of(to, None, Instant::now())));

        assert_eq!(settled(&with), settled(&without));

        for (from, to) in [(&with, &without), (&without, &with)] {
            for rise in rises() {
                let change = Change {
                    from: Some(from),
                    to,
                    rise,
                };

                assert_eq!(
                    Widget::height(&peek_lines(change)),
                    settled(&with),
                    "{} -> {} at {rise}",
                    from.title,
                    to.title
                );
            }
        }
    }

    #[test]
    fn a_line_both_tracks_share_never_dips() {
        let from = track("Same", "Alpha");
        let to = track("Same", "Beta");

        for rise in rises() {
            let change = Change {
                from: Some(&from),
                to: &to,
                rise,
            };

            assert_eq!(change.text(|track| &track.title), ("Same", 1.0));
        }

        let change = |rise| Change {
            from: Some(&from),
            to: &to,
            rise,
        };
        assert_eq!(change(0.25).text(|track| &track.artist), ("Alpha", 0.5));
        assert_eq!(change(0.75).text(|track| &track.artist), ("Beta", 0.5));
    }
}
