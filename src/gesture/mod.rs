//! The gesture launcher (ADR 0105 to 0109): `kanade gesture` covers the focused output with a
//! layer that takes the pointer, the user draws one stroke with the left button held, and on
//! release the stroke is matched against the recorded gestures (`recognizer`, `store`); a match
//! runs the Kanade verb the config maps it to (`gestures.<name>`), nothing else.
//!
//! The overlay is a grab, so it always lets go: Escape, a right click, the end of the stroke, the
//! timeout of `TIMEOUT` and the session locking each close it, and closing hands the pointer and
//! the keyboard back before the trail has faded.

use std::mem;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use kanade_runtime::service::{self, Service};
use kanade_runtime::{
    Button, Canvas, Cap, Center, Cursor, Full, IpcReply, Key, Keyboard, Layer, LayerWindow,
    Monitor, Monitors, MutexExt, Parent, Path, Point as Pointer, Rectangle, Shape, Stack, Start,
    Text, Widget, Zone, request_frame,
};

use crate::cli::{self, Call, Reply};
use crate::config;
use crate::island::activity::Leave;
use crate::island::motion::Mode as Motion;
use crate::lock;
use crate::sources::live;
use crate::sources::niri::Niri;
use crate::theme;

mod recognizer;
mod store;

use recognizer::{Outcome, Point, Template};

pub use store::{NAME_RULE, valid_name};

// how long the overlay waits for a stroke to end, from its opening or, recording, from the last one
const TIMEOUT: Duration = Duration::from_secs(8);

// how long the trail takes to fade once the stroke is over
const FADE: Duration = Duration::from_millis(350);
const FADE_REDUCED: Duration = Duration::from_millis(80);

// a point nearer than this to the last one is not kept; a stroke keeps at most `MOST_POINTS`
const STEP: f32 = 2.0;
const MOST_POINTS: usize = 4096;

// a verb in the config is no longer than this
const LONGEST_VERB: usize = 200;

// the trail's widths, its colors in the theme
const GLOW_WIDTH: f32 = 16.0;
const CORE_WIDTH: f32 = 4.0;

// what a verb asks of the gesture launcher
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    // open the overlay to draw, or close it when it is open
    Draw,
    Record(String),
    Forget(String),
    List,
}

impl Request {
    pub fn parse(arguments: &[&str]) -> Option<Request> {
        match arguments {
            [] => Some(Request::Draw),
            ["record", name] if valid_name(name) => Some(Request::Record((*name).to_owned())),
            ["forget", name] if valid_name(name) => Some(Request::Forget((*name).to_owned())),
            ["list"] => Some(Request::List),
            _ => None,
        }
    }
}

/*
 * the words of a config's verb, if a gesture may run it (ADR 0107): the verbs that act on the
 * shell or the session's state, from an allow-list, so a verb added to the CLI later is refused
 * until named here. Not those that run a program, type or press into a window, sign in, wait on
 * the user or end the session. The gesture is data, so it can only ever name what the CLI does
 */
pub fn check(verb: &str) -> Result<Vec<String>, String> {
    if verb.chars().count() > LONGEST_VERB {
        return Err(format!("at most {LONGEST_VERB} characters"));
    }

    let words: Vec<&str> = verb.split_whitespace().collect();

    if words.is_empty() {
        return Err(String::from(
            "expected a Kanade verb, like \"island collapse\"",
        ));
    }

    let (_, call) = cli::parse(&words)
        .map_err(|_| String::from("not a Kanade verb; `kanade help` lists them"))?;

    match call {
        Call::Dmenu(_) | Call::PickColor | Call::Gesture(_) => {
            Err(String::from("waits on you, which a gesture cannot"))
        }
        Call::Session(Leave::Ending(_)) => {
            Err(String::from("ends the session, which a gesture does not"))
        }
        Call::Island(_)
        | Call::Timer(_)
        | Call::ClearNotifications
        | Call::ClearClipboard
        | Call::Screenshot(_)
        | Call::Cast(_)
        | Call::Caffeine(_)
        | Call::NightLight(_)
        | Call::Focus(_)
        | Call::Dnd(_)
        | Call::Battery(_)
        | Call::Media(_)
        | Call::Wifi(_)
        | Call::Bluetooth(_)
        | Call::PowerProfile(_)
        | Call::Brightness(_)
        | Call::Wallpaper(_)
        | Call::Weather(_)
        | Call::Osd(_)
        | Call::Lock
        | Call::Session(_)
        | Call::Settings(_)
        | Call::CloseSettings => Ok(words.into_iter().map(str::to_owned).collect()),
        _ => Err(String::from(
            "not a verb a gesture may run; `kanade help` lists the rest",
        )),
    }
}

#[derive(Default)]
pub struct Gesture {
    phase: Phase,
    ticket: u64,
}

#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Drawing(Drawing),
    Fading(Fading),
}

struct Drawing {
    monitor: String,

    // the timeout that closes this opening, or this stroke of a recording
    ticket: u64,
    mode: Mode,
    templates: Vec<Template>,
    stroke: Vec<Point>,
    pressed: bool,
}

enum Mode {
    Run,

    // the gesture's normalized strokes so far
    Record {
        name: String,
        taken: Vec<Vec<Point>>,
    },
}

struct Fading {
    monitor: String,
    stroke: Vec<Point>,
    from: Instant,
    until: Instant,
}

impl Service for Gesture {
    fn new() -> Self {
        Self::default()
    }

    // written only by input and the timeout
    fn listen() {}
}

// the client of `gesture record`, which waits for the recording to end
static REPLY: Mutex<Option<IpcReply>> = Mutex::new(None);

fn answer(reply: &Reply) {
    if let Some(waiting) = REPLY.locked().take() {
        waiting.send(&reply.encode());
    }
}

pub fn spawn() {
    // a lock closes the overlay, whatever it was doing
    service::watch::<lock::Said>(on_lock);
}

fn on_lock() {
    if lock::holds() {
        cancel(None, "the session is locked");
    }
}

// `kanade gesture [list|forget <name>]`, answered at once
pub fn run(request: Request) -> Reply {
    match request {
        Request::Draw => {
            if drawing() {
                cancel(None, "cancelled");
                return Reply::Done(String::new());
            }

            match start(Mode::Run) {
                Ok(()) => Reply::Done(String::new()),
                Err(why) => Reply::Refused(why),
            }
        }
        Request::Forget(name) => match store::load() {
            Ok(mut recorded) => {
                if !recorded.forget(&name) {
                    return Reply::Refused(format!("no gesture named {name}"));
                }

                match store::save(&recorded) {
                    Ok(()) => Reply::Done(String::new()),
                    Err(why) => Reply::Refused(why),
                }
            }
            Err(why) => Reply::Refused(why),
        },
        Request::List => match store::load() {
            Ok(recorded) => {
                let config = config::get();
                let lines: Vec<String> = recorded
                    .names()
                    .map(|name| match config.gestures.get(name) {
                        Some(verb) => format!("{name}  {verb}"),
                        None => format!("{name}  (no action; set gestures.{name} in the config)"),
                    })
                    .collect();

                Reply::Done(lines.join("\n"))
            }
            Err(why) => Reply::Refused(why),
        },
        // `answer` keeps this waiting
        Request::Record(_) => Reply::Refused(String::from("answered later, not by `run`")),
    }
}

// `kanade gesture record <name>`: answers when the third stroke is stored, or the overlay closes
pub fn record(name: String, reply: IpcReply) {
    let mut waiting = REPLY.locked();

    // the first recording keeps its client
    if waiting.is_some() {
        reply.send(&Reply::Refused(String::from("a gesture is being recorded")).encode());
        return;
    }

    *waiting = Some(reply);
    drop(waiting);

    let mode = Mode::Record {
        name,
        taken: Vec::new(),
    };

    if let Err(why) = start(mode) {
        answer(&Reply::Refused(why));
    }
}

fn drawing() -> bool {
    matches!(Gesture::read().phase, Phase::Drawing(_))
}

// the output the user is on, else the first one
fn target() -> Option<String> {
    let monitors = Monitors::read();

    Niri::read()
        .seen
        .focused_output
        .clone()
        .filter(|name| monitors.has(name))
        .or_else(|| monitors.all().first().map(|monitor| monitor.name.clone()))
}

fn start(mode: Mode) -> Result<(), String> {
    if lock::holds() {
        return Err(String::from("the session is locked"));
    }

    let recorded = store::load()?;

    if matches!(mode, Mode::Run) && recorded.templates().is_empty() {
        return Err(String::from(
            "no gesture is recorded; run `kanade gesture record <name>`",
        ));
    }

    let monitor = target().ok_or("no output to draw on")?;
    let mut gesture = Gesture::write();

    if matches!(gesture.phase, Phase::Drawing(_)) {
        gesture.quiet();

        return Err(String::from("a gesture is being drawn"));
    }

    gesture.ticket += 1;

    let ticket = gesture.ticket;

    gesture.phase = Phase::Drawing(Drawing {
        monitor,
        ticket,
        mode,
        templates: recorded.templates().to_vec(),
        stroke: Vec::new(),
        pressed: false,
    });

    drop(gesture);
    expire(ticket);

    Ok(())
}

// closes the opening or stroke of `ticket` after `TIMEOUT`; closes it now when no timer can run
fn expire(ticket: u64) {
    let spawned = thread::Builder::new()
        .name(String::from("gesture-timeout"))
        .spawn(move || {
            thread::sleep(TIMEOUT);
            cancel(Some(ticket), "timed out");
        });

    if let Err(error) = spawned {
        eprintln!("gesture: no timeout: {error}");
        cancel(Some(ticket), "no timeout");
    }
}

fn fade_length() -> Duration {
    if config::get().island.motion == Motion::Reduced {
        FADE_REDUCED
    } else {
        FADE
    }
}

// the overlay stops taking input at once and shows the stroke fading out
fn fade(phase: &mut Phase, stroke: Vec<Point>) {
    if let Phase::Drawing(drawing) = mem::take(phase) {
        let now = Instant::now();

        *phase = Phase::Fading(Fading {
            monitor: drawing.monitor,
            stroke,
            from: now,
            until: now + fade_length(),
        });
    }
}

// closes the overlay if it is open (and, with `only`, still on that timeout); a recording then fails
fn cancel(only: Option<u64>, why: &str) {
    let mut gesture = Gesture::write();

    let drawn = match &mut gesture.phase {
        Phase::Drawing(drawing) if only.is_none_or(|ticket| ticket == drawing.ticket) => {
            mem::take(&mut drawing.stroke)
        }
        _ => {
            gesture.quiet();
            return;
        }
    };

    fade(&mut gesture.phase, drawn);
    drop(gesture);
    answer(&Reply::Refused(why.to_owned()));
}

fn key_press(key: Key) {
    if key == Key::Escape {
        cancel(None, "cancelled");
    }
}

fn click(button: Button) {
    if button == Button::Right {
        cancel(None, "cancelled");
    }
}

fn hold(down: bool) {
    if down {
        press();
    } else {
        release();
    }
}

// a new stroke; one still going on is dropped
fn press() {
    let mut gesture = Gesture::write();

    match &mut gesture.phase {
        Phase::Drawing(drawing) => {
            drawing.pressed = true;
            drawing.stroke.clear();
        }
        _ => gesture.quiet(),
    }
}

fn drag(at: Pointer) {
    let mut gesture = Gesture::write();

    let kept = match &mut gesture.phase {
        Phase::Drawing(drawing) if drawing.pressed && drawing.stroke.len() < MOST_POINTS => {
            let point = Point::new(at.x, at.y);

            if drawing
                .stroke
                .last()
                .is_none_or(|last| (last.x - point.x).hypot(last.y - point.y) >= STEP)
            {
                drawing.stroke.push(point);
                true
            } else {
                false
            }
        }
        _ => false,
    };

    if !kept {
        gesture.quiet();
    }
}

// what became of a stroke drawn to run a gesture
enum Verdict {
    Match(String),
    Unknown,
    Ambiguous,
}

// what a stroke that ended leads to
enum Next {
    Act(Verdict),

    // a recording wants more strokes, or this one again
    Again,
    Store(String, Vec<Vec<Point>>),
}

// the stroke is over: recognized and closed, or stored and closed after the third of a recording
fn release() {
    let mut gesture = Gesture::write();

    let Phase::Drawing(drawing) = &mut gesture.phase else {
        gesture.quiet();
        return;
    };

    if !drawing.pressed {
        gesture.quiet();
        return;
    }

    drawing.pressed = false;

    let drawn = mem::take(&mut drawing.stroke);
    let normalized = recognizer::normalize(&drawn);

    let next = match &mut drawing.mode {
        Mode::Run => Next::Act(match normalized {
            Some(stroke) => match recognizer::recognize(&stroke, &drawing.templates) {
                Outcome::Match { name, .. } => Verdict::Match(name.to_owned()),
                Outcome::Ambiguous => Verdict::Ambiguous,
                Outcome::Unknown => Verdict::Unknown,
            },
            // too short to mean anything
            None => Verdict::Unknown,
        }),
        Mode::Record { name, taken } => match normalized {
            // drawn again
            None => Next::Again,
            Some(stroke) => {
                taken.push(stroke);

                if taken.len() < store::STROKES {
                    Next::Again
                } else {
                    Next::Store(mem::take(name), mem::take(taken))
                }
            }
        },
    };

    match next {
        Next::Again => {
            let ticket = renew(&mut gesture);

            drop(gesture);
            expire(ticket);
        }
        Next::Act(verdict) => {
            fade(&mut gesture.phase, drawn);
            drop(gesture);
            act(verdict);
        }
        Next::Store(name, strokes) => {
            fade(&mut gesture.phase, drawn);
            drop(gesture);
            recorded(&name, strokes);
        }
    }
}

// a new timeout for the recording going on, which the old one no longer closes
fn renew(gesture: &mut Gesture) -> u64 {
    gesture.ticket += 1;

    let ticket = gesture.ticket;

    if let Phase::Drawing(drawing) = &mut gesture.phase {
        drawing.ticket = ticket;
    }

    ticket
}

// the third stroke is in: kept in the state file, and the client told
fn recorded(name: &str, strokes: Vec<Vec<Point>>) {
    let saved = store::load().and_then(|mut recorded| {
        recorded.record(name, strokes)?;
        store::save(&recorded)
    });

    answer(&match saved {
        Ok(()) if config::get().gestures.contains_key(name) => {
            Reply::Done(format!("recorded {name}"))
        }
        Ok(()) => Reply::Done(format!(
            "recorded {name}; give it an action with gestures.{name} in your config"
        )),
        Err(why) => Reply::Refused(why),
    });
}

// the verb a gesture is mapped to, as it is now in the config
fn action(name: &str) -> Result<Vec<String>, String> {
    let verb = config::get()
        .gestures
        .get(name)
        .cloned()
        .ok_or_else(|| String::from("no action in the config"))?;

    check(&verb)
}

fn act(verdict: Verdict) {
    let name = match verdict {
        Verdict::Unknown => return live::gesture("No gesture", "not recognized"),
        Verdict::Ambiguous => return live::gesture("No gesture", "too close to another"),
        Verdict::Match(name) => name,
    };

    let words = match action(&name) {
        Ok(words) => words,
        Err(why) => return live::gesture(&name, &why),
    };

    // a lock that came while the stroke was drawn is not walked around
    if lock::holds() {
        return;
    }

    /*
     * through the shell's own socket, the same path as the CLI (ADR 0107): the verb is parsed and
     * its Module checked again there. On a thread, as the draw thread answers it
     */
    let spawned = thread::Builder::new()
        .name(String::from("gesture-verb"))
        .spawn({
            let name = name.clone();

            move || match cli::call(&words) {
                Ok(Reply::Done(_)) => {}
                Ok(Reply::Refused(why) | Reply::Unknown(why)) | Err(why) => {
                    live::gesture(&name, &why)
                }
            }
        });

    if let Err(error) = spawned {
        eprintln!("gesture: {error}");
    }
}

/*
 * one window on every output, drawn only on the one the stroke is for. Hidden it attaches no
 * buffer; closing it hands back the keyboard and stops its input at once, while the trail fades
 */
pub fn window(monitor: &Monitor) -> LayerWindow {
    let now = Instant::now();
    let reduced = config::get().island.motion == Motion::Reduced;

    let shown = {
        let gesture = Gesture::read();

        match &gesture.phase {
            Phase::Drawing(drawing) if drawing.monitor == monitor.name => {
                Some((drawing.stroke.clone(), 1.0, Some(hint(drawing))))
            }
            Phase::Fading(fading) if fading.monitor == monitor.name && now < fading.until => {
                // a frame at a time while it fades, none after
                request_frame();

                let length = fading.until.saturating_duration_since(fading.from);
                let left = fading.until.saturating_duration_since(now);

                Some((
                    fading.stroke.clone(),
                    left.as_secs_f32() / length.as_secs_f32().max(f32::EPSILON),
                    None,
                ))
            }
            _ => None,
        }
    };

    let base = LayerWindow::new()
        .width(Full)
        .height(Full)
        .layer(Layer::Overlay)
        .space(Zone::Ignore)
        .namespace("kanade-gesture");

    let Some((stroke, level, hint)) = shown else {
        return base
            .visible(false)
            .click_through()
            .child(Rectangle::new().width(Parent).height(Parent));
    };

    let open = hint.is_some();
    let root = Rectangle::new()
        .width(Parent)
        .height(Parent)
        .cursor(Cursor::Crosshair)
        .on_hold(hold)
        .on_drag(drag)
        .on_click(click)
        .child(scene(&stroke, level, reduced, hint));

    if open {
        base.keyboard(Keyboard::Exclusive)
            .on_key(key_press)
            .child(root)
    } else {
        base.click_through().child(root)
    }
}

fn hint(drawing: &Drawing) -> String {
    match &drawing.mode {
        Mode::Run => String::from("Draw a gesture. Esc or right click cancels"),
        Mode::Record { name, taken } => format!(
            "Draw {name}, stroke {} of {}. Esc cancels",
            taken.len() + 1,
            store::STROKES
        ),
    }
}

// the scrim, the trail and the hint, in the order they stack
fn scene(stroke: &[Point], level: f32, reduced: bool, hint: Option<String>) -> Stack {
    let trace = || {
        let mut points = stroke.iter();
        let first = points.next().copied().unwrap_or_default();

        points.fold(Path::new().move_to(first.x, first.y), |path, point| {
            path.line_to(point.x, point.y)
        })
    };

    let mut shapes: Vec<Box<dyn Shape>> = Vec::new();

    if stroke.len() >= 2 {
        // reduced motion keeps the plain line
        if !reduced {
            shapes.push(Box::new(
                trace()
                    .stroke(GLOW_WIDTH, theme::GESTURE_GLOW)
                    .cap(Cap::Round)
                    .opacity(level),
            ));
        }

        shapes.push(Box::new(
            trace()
                .stroke(CORE_WIDTH, theme::GESTURE_CORE)
                .cap(Cap::Round)
                .opacity(level),
        ));
    }

    let mut layers: Vec<Box<dyn Widget>> = vec![
        Box::new(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .fill(theme::GESTURE_SCRIM)
                .opacity(level),
        ),
        Box::new(Canvas::new().width(Parent).height(Parent).shapes(shapes)),
    ];

    if let Some(hint) = hint {
        layers.push(Box::new(
            Rectangle::new()
                .width(Parent)
                .height(Parent)
                .padding(56.0)
                .align_child(Center, Start)
                .child(
                    Text::new(hint)
                        .size(theme::text::TITLE)
                        .color(theme::GESTURE_TEXT)
                        .weight(theme::text::MEDIUM),
                ),
        ));
    }

    Stack::new(layers)
}

#[cfg(test)]
mod tests {
    use super::check;

    // the verbs a gesture may run are named, so none that types, runs a program, waits or ends
    // the session gets through, nor one the CLI gains later
    #[test]
    fn a_gesture_runs_only_the_verbs_it_names() {
        for verb in [
            "island collapse",
            "launcher open",
            "lock",
            "notifications dnd toggle",
        ] {
            assert!(check(verb).is_ok(), "{verb}");
        }

        for verb in [
            "",
            "   ",
            "dmenu",
            "capture color",
            "gesture",
            "gesture record x",
            "activity run x",
            "activity hook x",
            "ui type hello",
            "ui key Return",
            "ui press result#1",
            "session logout",
            "session poweroff",
            "no-such-verb",
        ] {
            assert!(check(verb).is_err(), "{verb}");
        }

        assert!(check(&"island collapse ".repeat(40)).is_err());
    }
}
