//! Local, synthetic motion replay. It never connects to the running shell or reads user config.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::island::command::Command;
use crate::island::fade::section_pose;
use crate::island::geometry::{self, Canvas, Edge, Hang, Shape};
use crate::island::motion::Mode;
use crate::island::presentation::{Presentation, Segment};
use crate::island::satellites::Mark;
use crate::island::service::{IslandService, Timings};

const MONITOR: &str = "inspection";
const MAX_MS: u64 = 60_000;
const INTERRUPTIONS: &str = "0 controls open\n40 island collapse\n75 media open\n110 notifications open\n145 launcher open\n1000 island collapse\n";
const SATELLITES: &str = "0 debug post timer primary ongoing persistent global none\n0 debug post live second ongoing persistent global none\n0 debug post battery third ongoing persistent global none\n0 debug post media playing media persistent global none\n500 peek down\n650 peek up\n800 controls open\n835 island collapse\n900 controls open\n1100 island collapse\n1400 debug withdraw battery third\n1435 debug post battery third ongoing persistent global none\n1700 debug post workspace switch glance 180 global preempt\n";

pub const USAGE: &str = "motion inspect [interruptions|satellites] [--script <file>]
  [--fps <1-240>] [--duration <ms>] [--at <ms>] [--speed <0.1-10>]
  [--edge top|bottom|left|right] [--reduced-motion] [--format json|html]
  Runs synthetic production motion locally, without a shell or user config.
  Defaults: interruptions, 60 fps, 2400 ms, speed 1, top edge, JSON.
  --at adds an exact millisecond sample; repeat it for several instants.
  Script: one <integer-ms> <command> per line, ordered by time (up to 256).
  Commands: <surface> open|close|toggle, island collapse, peek down|up,
    debug post|withdraw (as in kanade help debug), dnd on|off|toggle,
    hover in|out, segment primary|satellite. Blank lines and # comments are ignored.
  HTML starts paused and steps or seeks recorded samples; it shows diagnostic
  geometry and section poses, not rendered Surface content or the GPU liquid neck.";

struct Options {
    fps: u64,
    duration: u64,
    at: Vec<u64>,
    speed: f32,
    mode: Mode,
    hang: Hang,
    html: bool,
    script: String,
}

enum Action {
    Command(Box<Command>),
    Hover(bool),
    Segment(Segment),
}

struct Event {
    ns: u64,
    label: String,
    action: Action,
}

fn integer(text: &str, max: u64) -> Result<u64, String> {
    text.parse::<u64>()
        .ok()
        .filter(|value| *value <= max)
        .ok_or_else(|| format!("expected an integer from 0 to {max}: {text}"))
}

fn options(words: &[&str]) -> Result<Options, String> {
    let mut options = Options {
        fps: 60,
        duration: 2400,
        at: Vec::new(),
        speed: 1.0,
        mode: Mode::Spring,
        hang: Hang::default(),
        html: false,
        script: INTERRUPTIONS.into(),
    };
    let mut words = words.iter().copied();
    while let Some(word) = words.next() {
        match word {
            "interruptions" => options.script = INTERRUPTIONS.into(),
            "satellites" => options.script = SATELLITES.into(),
            "--reduced-motion" => options.mode = Mode::Reduced,
            _ => {
                let value = words
                    .next()
                    .ok_or_else(|| format!("missing value for {word}"))?;
                match word {
                    "--fps" => {
                        options.fps = integer(value, 240)?;
                        if options.fps == 0 {
                            return Err("fps must be at least 1".into());
                        }
                    }
                    "--duration" => options.duration = integer(value, MAX_MS)?,
                    "--at" => {
                        if options.at.len() >= 256 {
                            return Err("at most 256 --at samples".into());
                        }
                        options.at.push(integer(value, MAX_MS)?);
                    }
                    "--speed" => {
                        options.speed = value
                            .parse::<f32>()
                            .ok()
                            .filter(|value| value.is_finite() && (0.1..=10.0).contains(value))
                            .ok_or("speed must be from 0.1 to 10")?;
                    }
                    "--format" => {
                        options.html = match value {
                            "json" => false,
                            "html" => true,
                            _ => return Err("format must be json or html".into()),
                        }
                    }
                    "--edge" => {
                        options.hang.edge = match value {
                            "top" => Edge::Top,
                            "bottom" => Edge::Bottom,
                            "left" => Edge::Left,
                            "right" => Edge::Right,
                            _ => return Err("edge must be top, bottom, left or right".into()),
                        }
                    }
                    "--script" => {
                        let file = std::fs::File::open(value).map_err(|error| error.to_string())?;
                        let mut script = String::new();
                        file.take(65_537)
                            .read_to_string(&mut script)
                            .map_err(|error| error.to_string())?;
                        if script.len() > 65_536 {
                            return Err("script exceeds 64 KiB".into());
                        }
                        options.script = script;
                    }
                    _ => return Err(format!("unknown option: {word}")),
                }
            }
        }
    }
    if options.at.iter().any(|at| *at > options.duration) {
        return Err("--at must be within --duration".into());
    }
    Ok(options)
}

fn action(words: &[&str]) -> Option<Action> {
    let command = match words {
        ["island", "collapse"] => Some(Command::Collapse),
        ["peek", "down"] => Some(Command::PeekDown),
        ["peek", "up"] => Some(Command::PeekUp),
        ["hover", "in"] => return Some(Action::Hover(true)),
        ["hover", "out"] => return Some(Action::Hover(false)),
        ["segment", "primary"] => return Some(Action::Segment(Segment::Primary)),
        ["segment", "satellite"] => return Some(Action::Segment(Segment::Satellite)),
        _ => crate::cli::parse(words)
            .ok()
            .and_then(|(_, call)| match call {
                crate::cli::Call::Island(command) => Some(command),
                _ => None,
            }),
    };
    command.map(|command| Action::Command(Box::new(command)))
}

fn events(script: &str) -> Result<Vec<Event>, String> {
    let mut events: Vec<Event> = Vec::new();
    for (line, text) in script.lines().enumerate() {
        let text = text.split('#').next().unwrap_or_default().trim();
        if text.is_empty() {
            continue;
        }
        let words: Vec<_> = text.split_whitespace().collect();
        let ms =
            integer(words[0], MAX_MS).map_err(|error| format!("line {}: {error}", line + 1))?;
        let ns = ms * 1_000_000;
        if events.last().is_some_and(|last| last.ns > ns) {
            return Err(format!("line {}: events must be ordered by time", line + 1));
        }
        if events.len() >= 256 {
            return Err("at most 256 script events".into());
        }
        let action =
            action(&words[1..]).ok_or_else(|| format!("line {}: unsupported command", line + 1))?;
        events.push(Event {
            ns,
            label: words[1..].join(" "),
            action,
        });
    }
    Ok(events)
}

fn rect(rect: geometry::Rect) -> Value {
    json!({"x": rect.x, "y": rect.y, "width": rect.width, "height": rect.height})
}

fn shape(shape: Shape) -> Value {
    json!({"width": shape.width, "height": shape.height, "radius": shape.radius})
}

fn presentation(presentation: Presentation) -> String {
    match presentation {
        Presentation::Expanded(surface) => format!("expanded:{surface:?}").to_ascii_lowercase(),
        Presentation::Rest => "rest".into(),
        Presentation::Compact => "compact".into(),
        Presentation::Split => "split".into(),
        Presentation::Peek => "peek".into(),
        Presentation::Tray(_) => "tray".into(),
    }
}

fn snapshot(
    island: &IslandService,
    options: &Options,
    now: Instant,
    ns: u64,
    frame: Option<u64>,
    event: Option<(&str, &str)>,
) -> Value {
    let canvas = Canvas::around(geometry::DEFAULT_LARGEST, options.hang);
    let body_shape = island.shape(MONITOR, now);
    let body = geometry::body(body_shape, options.hang, canvas);
    let arrival = island.arrival(MONITOR, now);
    let content: Vec<_> = island
        .content(MONITOR, now)
        .into_iter()
        .enumerate()
        .filter_map(|(index, shown)| {
            shown.map(|(content, opacity)| {
                json!({"presentation": presentation(content.presentation), "opacity": opacity,
                "leaving": index == 0, "interactive": index == 1 && opacity >= 0.5,
                "arrival": arrival,
                "sections": (0..6).map(|section| {
                    let (alpha, lift) = section_pose(section, arrival, options.mode);
                    json!({"opacity": opacity * alpha, "lift": lift,
                        "interactive": index == 1 && opacity * alpha >= 0.5})
                }).collect::<Vec<_>>()})
            })
        })
        .collect();
    let dots: Vec<_> = island
        .satellites(MONITOR)
        .shown(now)
        .into_iter()
        .map(|dot| {
            let mark = match dot.mark {
                Mark::Activity(activity) => {
                    format!("{}:{}", activity.id().kind().name(), activity.id().key())
                }
                Mark::Overflow(count) => format!("overflow:{count}"),
            };
            json!({"mark": mark, "slot": dot.slot, "presence": dot.presence,
            "opacity": dot.opacity, "content_opacity": dot.content_opacity.min(dot.opacity),
            "content_opacity_raw": dot.content_opacity,
            "rect": rect(geometry::satellite(body, options.hang, canvas, dot.slot, dot.presence))})
        })
        .collect();
    json!({"t_ns": ns, "frame": frame,
        "event": event.map(|(phase, label)| json!({"phase": phase, "command": label})),
        "presentation": presentation(island.presentation(MONITOR)),
        "shape": shape(body_shape), "body": rect(body),
        "target": shape(island.heading(MONITOR, now).unwrap_or(body_shape)),
        "velocity": island.velocity(MONITOR, now), "content": content,
        "satellites": dots, "swap": island.swap(MONITOR, now),
        "settled": island.settled(MONITOR, now)})
}

fn replay(options: &Options) -> Result<Value, String> {
    let events = events(&options.script)?;
    let origin = Instant::now();
    let mut timings = Timings {
        motion: options.mode,
        ..Timings::default()
    };
    timings.expand = timings.expand.div_f32(options.speed);
    timings.surface_change = timings.surface_change.div_f32(options.speed);
    timings.collapse = timings.collapse.div_f32(options.speed);
    // Accessibility's brief fade is independent of the normal geometry speed.
    let mut island = IslandService::for_inspection(timings, options.hang, origin);
    let end = options.duration * 1_000_000;
    let mut moments = BTreeMap::new();
    for frame in 0..=options.duration * options.fps / 1000 {
        let ns = (frame * 1_000_000_000 + options.fps / 2) / options.fps;
        moments.insert(ns, Some(frame));
    }
    moments.entry(end).or_insert(None);
    for ms in &options.at {
        moments.entry(ms * 1_000_000).or_insert(None);
    }
    for event in &events {
        if event.ns <= end {
            moments.entry(event.ns).or_insert(None);
        }
    }
    let mut samples = Vec::with_capacity(moments.len() + events.len() * 2);
    let mut pending = events.into_iter().peekable();
    for (ns, frame) in moments {
        let now = origin + Duration::from_nanos(ns);
        island.advance_inspection(now);
        while let Some(event) = pending.next_if(|event| event.ns == ns) {
            samples.push(snapshot(
                &island,
                options,
                now,
                ns,
                None,
                Some(("before", &event.label)),
            ));
            match event.action {
                Action::Command(command) => {
                    if let Some(effect) = island
                        .resolve(*command)
                        .map_err(|error| error.to_string())?
                    {
                        island.apply(effect, now);
                    }
                }
                Action::Hover(inside) => island.hover(MONITOR, inside, true, now),
                Action::Segment(segment) => island.set_segment(MONITOR, segment),
            }
            samples.push(snapshot(
                &island,
                options,
                now,
                ns,
                None,
                Some(("after", &event.label)),
            ));
        }
        samples.push(snapshot(&island, options, now, ns, frame, None));
    }
    let canvas = Canvas::around(geometry::DEFAULT_LARGEST, options.hang);
    Ok(
        json!({"version": 1, "fps": options.fps, "speed": options.speed,
        "reduced_motion": options.mode == Mode::Reduced,
        "canvas": {"width": canvas.width, "height": canvas.height},
        "script": options.script, "samples": samples}),
    )
}

pub fn run(words: &[&str]) -> ExitCode {
    let result = options(words).and_then(|options| {
        let trace = replay(&options)?;
        let data = serde_json::to_string(&trace).map_err(|error| error.to_string())?;
        let mut output = if options.html {
            let data = if data.contains('<') {
                data.replace('<', "\\u003c")
            } else {
                data
            };
            include_str!("motion_inspect.html").replace("/*TRACE*/null", &data)
        } else {
            data
        };
        output.push('\n');
        Ok(output)
    });
    match result {
        Ok(output) => crate::cli::print(&output, ExitCode::SUCCESS),
        Err(error) => {
            eprintln!("kanade motion inspect: {error}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_frequency_does_not_change_replayed_motion() {
        let mut slow = options(&[
            "satellites",
            "--fps",
            "30",
            "--at",
            "835",
            "--at",
            "1950",
            "--at",
            "2300",
        ])
        .unwrap();
        let first = replay(&slow).unwrap();
        slow.fps = 144;
        let second = replay(&slow).unwrap();
        let at = |trace: &Value| {
            trace["samples"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|sample| {
                    [835_000_000_u64, 1_950_000_000, 2_300_000_000]
                        .contains(&sample["t_ns"].as_u64().unwrap())
                        && sample["event"].is_null()
                })
                .map(|sample| {
                    let mut sample = sample.clone();
                    sample.as_object_mut().unwrap().remove("frame");
                    sample
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(at(&first).len(), 3);
        assert_eq!(at(&first), at(&second));
        assert_eq!(replay(&slow).unwrap(), second);
    }

    #[test]
    fn scripted_interruptions_preserve_visible_pose_and_content() {
        let trace = replay(&options(&["interruptions"]).unwrap()).unwrap();
        for pair in trace["samples"].as_array().unwrap().windows(2) {
            let (before, after) = (&pair[0], &pair[1]);
            if before["event"]["phase"] != "before" || after["event"]["phase"] != "after" {
                continue;
            }
            for field in ["width", "height", "radius"] {
                assert!(
                    (before["shape"][field].as_f64().unwrap()
                        - after["shape"][field].as_f64().unwrap())
                    .abs()
                        < 1e-3
                );
            }
            for axis in 0..3 {
                assert!(
                    (before["velocity"][axis].as_f64().unwrap()
                        - after["velocity"][axis].as_f64().unwrap())
                    .abs()
                        < 1e-2
                );
            }
            let visible = |sample: &Value| {
                sample["content"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|content| {
                        json!([
                            content["presentation"],
                            content["opacity"],
                            content["arrival"],
                            content["sections"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .map(|section| { json!([section["opacity"], section["lift"]]) })
                                .collect::<Vec<_>>()
                        ])
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(visible(before), visible(after));
        }
    }

    #[test]
    fn scripts_reject_unordered_or_external_commands() {
        assert!(events("10 controls open\n9 media open").is_err());
        assert!(events("0 session poweroff now").is_err());
        assert!(options(&["--fps", "0"]).is_err());
        assert!(options(&["--speed", "NaN"]).is_err());
    }
}
