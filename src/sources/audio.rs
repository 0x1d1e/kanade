//! The audio devices and app streams (#132, ADR 0011): what Amane's `Audio` lacks, since it has the
//! default speaker and microphone only. Read from PipeWire's graph through `pw-dump --monitor`, run
//! apart from `privacy`'s, so this wakes only when the graph does; changed through `wpctl`, one
//! action each. What capture is in use is `privacy`'s, not this.
//!
//! Volumes are percents on the scale `wpctl` and Amane's `Audio` show: PipeWire keeps the cube of
//! it per channel. A `Node` names a device or stream to act on, and is good only while it shows.

use std::collections::HashMap;
use std::io::{self, BufRead};

use amane::Service;

use super::json::{self, Json};
use super::pipewire::{self, DUMP, METADATA, NODE, Object};
use super::wake;
use crate::supervise;

// the devices sound plays on or comes from, and the apps playing or recording it
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mixer {
    // speakers and headphones, by name
    pub outputs: Vec<Device>,

    // microphones, by name; a device that does both, like an audio interface, shows in each with
    // the same Node
    pub inputs: Vec<Device>,

    // by app, those playing first
    pub streams: Vec<Stream>,
}

impl Service for Mixer {
    fn new() -> Self {
        Mixer::default()
    }

    fn listen() {}
}

// one device or stream to act on; another Node once it is gone and back
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Node(u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub node: Node,

    // as the system describes it, like "Built-in Audio Analog Stereo"
    pub name: String,

    // where sound goes, or comes from, unless an app picks another; one per direction at most
    pub default: bool,

    pub level: Level,

    // how `set_default` makes it the default of the list it is in
    choice: Choice,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Choice {
    // a sink or source, whose direction wpctl knows
    Node,

    // a device that does both, which wpctl turns down: set by name as the configured default of
    // one direction, as wpctl would
    Configured { key: &'static str, name: String },
}

// an app playing or recording sound
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stream {
    pub node: Node,

    // by the name it gives, or its node's own name when it gives none
    pub app: String,

    // what it plays, like a tab's title, when it says
    pub title: Option<String>,

    // plays, rather than records
    pub plays: bool,

    pub level: Level,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    // 0 to 100, past 100 when amplified, the loudest channel's
    pub volume: u8,

    pub muted: bool,
}

// changes the devices and streams, one action each; never restarted
pub const WPCTL: &str = "wpctl";

// the metadata PipeWire's session manager keeps the default devices in, by node name
const DEFAULTS: &str = "default";
const DEFAULT_OUTPUT: &str = "default.audio.sink";
const DEFAULT_INPUT: &str = "default.audio.source";

// the defaults asked for, which the session manager follows while those devices are there
const CONFIGURED_OUTPUT: &str = "default.configured.audio.sink";
const CONFIGURED_INPUT: &str = "default.configured.audio.source";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    Output,
    Input,
    Duplex,
    Playback,
    Recording,
}

impl Role {
    fn of(class: &str) -> Option<Role> {
        match class {
            "Audio/Sink" => Some(Role::Output),
            "Audio/Source" | "Audio/Source/Virtual" => Some(Role::Input),
            "Audio/Duplex" => Some(Role::Duplex),
            "Stream/Output/Audio" => Some(Role::Playback),
            "Stream/Input/Audio" => Some(Role::Recording),
            _ => None,
        }
    }
}

// a node as far as the Mixer goes
#[derive(Debug)]
struct Entry {
    role: Role,

    // what the default devices name it by
    name: String,

    // what the Mixer shows it as
    label: String,
    title: Option<String>,

    level: Level,
}

// PipeWire's graph, as far as audio nodes and the default devices go
#[derive(Debug, Default)]
struct Graph {
    nodes: HashMap<u64, Entry>,

    // the default devices' metadata, once known; its first print has every key, later ones only
    // the keys set, a removed key not at all, so after those it is read again whole
    defaults: Option<u64>,
    output: Option<String>,
    input: Option<String>,
    reread: bool,
}

impl Graph {
    fn apply(&mut self, object: &Json) {
        let object = Object(object);

        let Some(id) = object.id() else {
            return;
        };

        if object.removed() {
            self.nodes.remove(&id);

            if self.defaults == Some(id) {
                self.defaults = None;
                self.output = None;
                self.input = None;
            }

            return;
        }

        match object.kind() {
            Some(NODE) => match entry(&object) {
                Some(entry) => drop(self.nodes.insert(id, entry)),
                None => drop(self.nodes.remove(&id)),
            },
            Some(METADATA) if metadata_name(object.0) == Some(DEFAULTS) => {
                let whole = self.defaults != Some(id);

                self.defaults = Some(id);
                self.reread |= !whole;
                self.set_defaults(object.0, whole);
            }
            _ => {}
        }
    }

    // the defaults' metadata as a one-off pw-dump of it prints it, whole; none once it is gone,
    // whose removal is still to come
    fn reread_defaults(&mut self, text: &str) {
        pipewire::prints(text.as_bytes(), |objects| {
            for object in objects {
                let metadata = Object(object);

                if metadata.id().is_some() && metadata.id() == self.defaults {
                    self.set_defaults(object, true);
                }
            }
        });
    }

    fn set_defaults(&mut self, metadata: &Json, whole: bool) {
        if whole {
            self.output = None;
            self.input = None;
        }

        let entries = metadata.get("metadata").and_then(Json::as_array);

        for entry in entries.unwrap_or_default() {
            if entry.get("subject").and_then(Json::as_u64) != Some(0) {
                continue;
            }

            let slot = match entry.get("key").and_then(Json::as_str) {
                Some(DEFAULT_OUTPUT) => &mut self.output,
                Some(DEFAULT_INPUT) => &mut self.input,
                _ => continue,
            };

            *slot = entry.get("value").and_then(default_name);
        }
    }

    fn mixer(&self) -> Mixer {
        let devices = |role, default: &Option<String>, configured| {
            let mut devices: Vec<_> = self
                .nodes
                .iter()
                .filter(|(_, entry)| entry.role == role || entry.role == Role::Duplex)
                .map(|(&id, entry)| Device {
                    node: Node(id),
                    name: entry.label.clone(),
                    default: default.as_ref() == Some(&entry.name),
                    level: entry.level,
                    choice: match entry.role {
                        Role::Duplex => Choice::Configured {
                            key: configured,
                            name: entry.name.clone(),
                        },
                        _ => Choice::Node,
                    },
                })
                .collect();

            devices.sort_by(|a, b| (&a.name, a.node.0).cmp(&(&b.name, b.node.0)));
            devices
        };

        let mut streams: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, entry)| matches!(entry.role, Role::Playback | Role::Recording))
            .map(|(&id, entry)| Stream {
                node: Node(id),
                app: entry.label.clone(),
                title: entry.title.clone(),
                plays: entry.role == Role::Playback,
                level: entry.level,
            })
            .collect();

        streams.sort_by(|a, b| (!a.plays, &a.app, a.node.0).cmp(&(!b.plays, &b.app, b.node.0)));

        Mixer {
            outputs: devices(Role::Output, &self.output, CONFIGURED_OUTPUT),
            inputs: devices(Role::Input, &self.input, CONFIGURED_INPUT),
            streams,
        }
    }
}

// a node the Mixer shows; none for the rest, like video, MIDI, a level meter or a stream inside
// a device
fn entry(object: &Object) -> Option<Entry> {
    let role = Role::of(object.prop("media.class")?)?;

    if object.flag("stream.monitor") {
        return None;
    }

    let name = object.prop("node.name").unwrap_or_default().to_string();
    let label = match role {
        Role::Output | Role::Input | Role::Duplex => object.prop("node.description"),
        Role::Playback | Role::Recording => object.prop("application.name"),
    };
    let label = label.filter(|label| !label.is_empty()).unwrap_or(&name);
    let title = object
        .prop("media.name")
        .filter(|title| !title.is_empty() && title != &label);

    Some(Entry {
        role,
        label: label.to_string(),
        title: title.map(String::from),
        name,
        level: level(object.info()?),
    })
}

/*
 * a node's volume and mute, from the Props param that has its channel volumes; a stream not yet
 * running has none, and plays at full volume
 */
fn level(info: &Json) -> Level {
    let props = info.get("params").and_then(|params| params.get("Props"));
    let props = props.and_then(Json::as_array).unwrap_or_default();

    let Some(props) = props
        .iter()
        .find(|props| props.get("channelVolumes").is_some())
    else {
        return Level {
            volume: 100,
            muted: false,
        };
    };

    let channels = props.get("channelVolumes").and_then(Json::as_array);
    let loudest = channels
        .unwrap_or_default()
        .iter()
        .filter_map(Json::as_f64)
        .fold(None, |loudest: Option<f64>, channel| {
            Some(loudest.map_or(channel, |loudest| loudest.max(channel)))
        })
        .unwrap_or(1.0);

    Level {
        volume: percent(loudest),
        muted: props.get("mute").and_then(Json::as_bool) == Some(true),
    }
}

// a channel volume, the cube of what wpctl shows, to its percent
fn percent(cubed: f64) -> u8 {
    (cubed.max(0.0).cbrt() * 100.0)
        .round()
        .min(f64::from(u8::MAX)) as u8
}

fn metadata_name(object: &Json) -> Option<&str> {
    object.get("props")?.get("metadata.name")?.as_str()
}

// a default device's value, `{"name": ...}`, which older pw-dump prints as text; none once removed
fn default_name(value: &Json) -> Option<String> {
    let name = |value: &Json| value.get("name")?.as_str().map(String::from);

    match value {
        Json::String(text) => Json::parse(text).as_ref().and_then(name),
        value => name(value),
    }
}

// runs on its own thread for good; without pw-dump there are no devices or streams
pub fn follow() {
    // kept across a run that panicked, so the next one clears what is gone
    let mut shown = Mixer::default();

    let stop = wake::Stop::default();

    let error = wake::run(DUMP, pipewire::MONITOR, &stop, |output| {
        let lost = watch(output, &mut reread, &mut shown, &mut show);

        // nobody can say any more what plays
        if shown != Mixer::default() {
            shown = Mixer::default();
            show(&shown);
        }

        lost
    });

    // nothing stops it
    let Some(error) = error else { return };

    let why = format!("cannot run pw-dump ({error}), no audio devices or app streams");

    eprintln!("kanade: {why}");
    supervise::stopped("audio", why);
}

/*
 * follows pw-dump until its output ends, writing the Mixer when it changes, so the graph changing
 * otherwise, like a link or a client, writes nothing; returns why it ended. `reread` prints one
 * object of the graph whole
 */
fn watch(
    lines: impl BufRead,
    reread: &mut impl FnMut(u64) -> Result<String, String>,
    shown: &mut Mixer,
    show: &mut impl FnMut(&Mixer),
) -> io::Error {
    let mut graph = Graph::default();

    pipewire::prints(lines, |objects| {
        for object in objects {
            graph.apply(object);
        }

        if std::mem::take(&mut graph.reread)
            && let Some(defaults) = graph.defaults
        {
            match reread(defaults) {
                Ok(text) => graph.reread_defaults(&text),
                Err(why) => eprintln!("kanade: default devices may be stale: {why}"),
            }
        }

        let mixer = graph.mixer();

        if mixer != *shown {
            show(&mixer);
            *shown = mixer;
        }
    })
}

// the defaults' metadata whole, which waits for pw-dump on the follower's thread
fn reread(id: u64) -> Result<String, String> {
    wake::query(DUMP, &["--no-colors", &id.to_string()])
}

fn show(mixer: &Mixer) {
    *Mixer::write() = mixer.clone();
}

/*
 * sets a device's or stream's volume, 0 to 100; waits for wpctl, so call it off the draw thread.
 * The Mixer shows the change once PipeWire has it
 */
#[expect(dead_code, reason = "the Audio sub-surface calls it, #133")]
pub fn set_volume(node: Node, volume: u8) -> Result<(), String> {
    let volume = format!("{}%", volume.min(100));

    wake::act(WPCTL, &["set-volume", &node.0.to_string(), &volume])
}

// mutes or unmutes a device or stream, as `set_volume`
#[expect(dead_code, reason = "the Audio sub-surface calls it, #133")]
pub fn set_muted(node: Node, muted: bool) -> Result<(), String> {
    let muted = if muted { "1" } else { "0" };

    wake::act(WPCTL, &["set-mute", &node.0.to_string(), muted])
}

/*
 * makes a device the default of the list it is in, outputs or inputs, as `set_volume`; streams that
 * follow the default move to it
 */
#[expect(dead_code, reason = "the Audio sub-surface calls it, #133")]
pub fn set_default(device: &Device) -> Result<(), String> {
    let (program, args) = default_command(device);
    let args: Vec<_> = args.iter().map(String::as_str).collect();

    wake::act(program, &args)
}

fn default_command(device: &Device) -> (&'static str, Vec<String>) {
    match &device.choice {
        Choice::Node => (WPCTL, vec!["set-default".into(), device.node.0.to_string()]),
        Choice::Configured { key, name } => {
            let value = format!("{{ \"name\": {} }}", json::quote(name));
            let args = ["-n", DEFAULTS, "0", key, &value, "Spa:String:JSON"];

            (pipewire::SET_METADATA, args.map(String::from).to_vec())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPEAKER: u64 = 55;
    const MICROPHONE: u64 = 56;
    const HEADPHONES: u64 = 116;
    const DEFAULTS_ID: u64 = 40;

    fn node(id: u64, props: &str, volumes: &str, muted: bool) -> String {
        format!(
            r#"  {{
    "id": {id},
    "type": "{NODE}",
    "info": {{
      "props": {{ {props} }},
      "params": {{
        "Props": [
          {{ "device": "x" }},
          {{ "volume": 1.0, "mute": {muted}, "channelVolumes": [ {volumes} ] }}
        ]
      }}
    }}
  }}"#
        )
    }

    fn device(id: u64, class: &str, name: &str, description: &str, volumes: &str) -> String {
        let props = format!(
            r#""media.class": "{class}", "node.name": "{name}", "node.description": "{description}""#
        );

        node(id, &props, volumes, false)
    }

    fn stream(id: u64, class: &str, app: &str, title: &str, volumes: &str, muted: bool) -> String {
        let props = format!(
            r#""media.class": "{class}", "node.name": "node{id}", "application.name": "{app}", "media.name": "{title}""#
        );

        node(id, &props, volumes, muted)
    }

    fn defaults(entries: &[(&str, &str)]) -> String {
        let entries: Vec<_> = entries
            .iter()
            .map(|(key, name)| {
                format!(
                    r#"{{ "subject": 0, "key": "{key}", "type": "Spa:String:JSON", "value": {{ "name": "{name}" }} }}"#
                )
            })
            .collect();

        format!(
            r#"  {{
    "id": {DEFAULTS_ID},
    "type": "{METADATA}",
    "props": {{ "metadata.name": "default" }},
    "metadata": [ {} ]
  }}"#,
            entries.join(", ")
        )
    }

    fn removed(id: u64) -> String {
        format!("  {{\n    \"id\": {id},\n    \"info\": null\n  }}")
    }

    fn print(objects: &[String]) -> String {
        format!("[\n{}\n]\n", objects.join(",\n"))
    }

    // this machine's graph as pw-dump first prints it
    fn machine() -> String {
        print(&[
            device(
                SPEAKER,
                "Audio/Sink",
                "alsa_output.analog",
                "Built-in Audio",
                "0.000343, 0.000343",
            ),
            device(
                MICROPHONE,
                "Audio/Source",
                "alsa_input.analog",
                "Built-in Microphone",
                "1.0, 1.0",
            ),
            device(
                HEADPHONES,
                "Audio/Sink",
                "bluez_output.1",
                "Moondrop",
                "0.046656, 0.046656",
            ),
            device(58, "Video/Source", "v4l2_input", "Camera", ""),
            defaults(&[
                ("default.configured.audio.sink", "alsa_output.usb"),
                (DEFAULT_OUTPUT, "bluez_output.1"),
                (DEFAULT_INPUT, "alsa_input.analog"),
            ]),
        ])
    }

    // what each print wrote, reading the defaults' metadata again as `whole` prints it
    fn rereading(prints: &[String], whole: Result<String, String>) -> Vec<Mixer> {
        let mut posts = Vec::new();
        let mut shown = Mixer::default();

        let mut reread = |id| {
            assert_eq!(id, DEFAULTS_ID);
            whole.clone()
        };

        let text = prints.concat();
        let lost = watch(text.as_bytes(), &mut reread, &mut shown, &mut |mixer| {
            posts.push(mixer.clone())
        });

        assert_eq!(lost.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(posts.last().cloned().unwrap_or_default(), shown);

        posts
    }

    // what each print wrote, where nothing reads the defaults' metadata again
    fn posts(prints: &[String]) -> Vec<Mixer> {
        rereading(prints, Err("not reread".into()))
    }

    fn default_nodes(devices: &[Device]) -> Vec<Node> {
        devices
            .iter()
            .filter(|device| device.default)
            .map(|device| device.node)
            .collect()
    }

    fn level(volume: u8, muted: bool) -> Level {
        Level { volume, muted }
    }

    fn device_of(node: u64, name: &str, default: bool, volume: u8) -> Device {
        Device {
            node: Node(node),
            name: name.into(),
            default,
            level: level(volume, false),
            choice: Choice::Node,
        }
    }

    #[test]
    fn the_graph_parses_to_devices_by_name_with_their_defaults() {
        let posts = posts(&[machine()]);

        assert_eq!(
            posts,
            vec![Mixer {
                outputs: vec![
                    device_of(SPEAKER, "Built-in Audio", false, 7),
                    device_of(HEADPHONES, "Moondrop", true, 36),
                ],
                inputs: vec![device_of(MICROPHONE, "Built-in Microphone", true, 100)],
                streams: vec![],
            }]
        );
    }

    #[test]
    fn apps_playing_come_before_apps_recording_each_by_app() {
        let posts = posts(&[
            machine(),
            print(&[
                stream(80, "Stream/Input/Audio", "OBS", "", "1.0", false),
                stream(
                    81,
                    "Stream/Output/Audio",
                    "Zen",
                    "Video - YouTube",
                    "0.125",
                    true,
                ),
                stream(
                    82,
                    "Stream/Output/Audio",
                    "Firefox",
                    "Firefox",
                    "1.0, 0.125",
                    false,
                ),
            ]),
        ]);

        let streams = &posts.last().unwrap().streams;

        assert_eq!(
            streams,
            &vec![
                Stream {
                    node: Node(82),
                    app: "Firefox".into(),
                    title: None,
                    plays: true,
                    level: level(100, false),
                },
                Stream {
                    node: Node(81),
                    app: "Zen".into(),
                    title: Some("Video - YouTube".into()),
                    plays: true,
                    level: level(50, true),
                },
                Stream {
                    node: Node(80),
                    app: "OBS".into(),
                    title: None,
                    plays: false,
                    level: level(100, false),
                },
            ]
        );
    }

    #[test]
    fn level_meters_and_device_internal_streams_are_no_streams() {
        let meter = r#""media.class": "Stream/Input/Audio", "application.name": "pavucontrol", "stream.monitor": true"#;

        let posts = posts(&[
            machine(),
            print(&[
                node(80, meter, "1.0", false),
                stream(81, "Stream/Input/Audio/Internal", "bluez", "", "1.0", false),
            ]),
        ]);

        assert_eq!(posts.len(), 1);
    }

    #[test]
    fn a_stream_without_app_name_or_volumes_shows_its_node_at_full_volume() {
        let props = r#""media.class": "Stream/Output/Audio", "node.name": "speech-dispatcher""#;

        let posts = posts(&[print(&[node(80, props, "", false)])]);

        assert_eq!(
            posts.last().unwrap().streams,
            vec![Stream {
                node: Node(80),
                app: "speech-dispatcher".into(),
                title: None,
                plays: true,
                level: level(100, false),
            }]
        );
    }

    #[test]
    fn a_volume_change_reposts_and_a_removal_drops_the_stream() {
        let playing = |volumes| {
            print(&[stream(
                95,
                "Stream/Output/Audio",
                "pw-play",
                "",
                volumes,
                false,
            )])
        };

        let posts = posts(&[
            machine(),
            playing("1.0, 1.0"),
            playing("0.125, 0.125"),
            playing("0.125, 0.125"),
            print(&[removed(95)]),
        ]);

        let volumes: Vec<_> = posts
            .iter()
            .map(|mixer| {
                mixer
                    .streams
                    .iter()
                    .map(|s| s.level.volume)
                    .collect::<Vec<_>>()
            })
            .collect();

        assert_eq!(volumes, vec![vec![], vec![100], vec![50], vec![]]);
    }

    // the metadata prints only the keys that changed, so the other default stays
    #[test]
    fn a_new_default_moves_only_its_direction() {
        let posts = posts(&[
            machine(),
            print(&[defaults(&[(DEFAULT_OUTPUT, "alsa_output.analog")])]),
        ]);

        let mixer = posts.last().unwrap();

        assert_eq!(default_nodes(&mixer.outputs), vec![Node(SPEAKER)]);
        assert_eq!(default_nodes(&mixer.inputs), vec![Node(MICROPHONE)]);
    }

    // pw-dump prints a removed key as nothing, so only reading the metadata again tells
    #[test]
    fn a_removed_default_is_read_again_and_forgotten() {
        let whole = print(&[defaults(&[(DEFAULT_INPUT, "alsa_input.analog")])]);
        let posts = rereading(&[machine(), print(&[defaults(&[])])], Ok(whole));

        let mixer = posts.last().unwrap();

        assert_eq!(default_nodes(&mixer.outputs), vec![]);
        assert_eq!(default_nodes(&mixer.inputs), vec![Node(MICROPHONE)]);
    }

    #[test]
    fn the_first_print_of_the_defaults_is_whole() {
        let whole = print(&[defaults(&[])]);
        let posts = rereading(&[machine()], Ok(whole));

        assert_eq!(
            default_nodes(&posts.last().unwrap().outputs),
            vec![Node(HEADPHONES)]
        );
    }

    #[test]
    fn the_defaults_gone_before_their_reread_stay_until_their_removal() {
        let posts = rereading(
            &[
                machine(),
                print(&[defaults(&[])]),
                print(&[removed(DEFAULTS_ID)]),
            ],
            Ok(print(&[])),
        );

        assert_eq!(posts.len(), 2);
        assert_eq!(default_nodes(&posts[0].outputs), vec![Node(HEADPHONES)]);
        assert_eq!(default_nodes(&posts[1].outputs), vec![]);
    }

    // one device that both plays and records, the default of both or either
    #[test]
    fn a_duplex_device_is_an_output_and_an_input() {
        let posts = posts(&[
            machine(),
            print(&[device(
                60,
                "Audio/Duplex",
                "pro_audio.duplex",
                "Interface",
                "1.0",
            )]),
            print(&[defaults(&[(DEFAULT_INPUT, "pro_audio.duplex")])]),
        ]);

        let mixer = posts.last().unwrap();
        let interface = |devices: &[Device]| {
            devices
                .iter()
                .find(|device| device.node == Node(60))
                .cloned()
                .unwrap()
        };

        assert_eq!(interface(&mixer.outputs).name, "Interface");
        assert_eq!(default_nodes(&mixer.inputs), vec![Node(60)]);
        assert_eq!(default_nodes(&mixer.outputs), vec![Node(HEADPHONES)]);

        // wpctl takes only a sink or a source, so a duplex one is configured per direction
        let configures = |devices: &[Device]| {
            let (program, args) = default_command(&interface(devices));

            assert_eq!(program, pipewire::SET_METADATA);
            args
        };
        let value = r#"{ "name": "pro_audio.duplex" }"#;

        assert_eq!(
            configures(&mixer.outputs),
            [
                "-n",
                "default",
                "0",
                CONFIGURED_OUTPUT,
                value,
                "Spa:String:JSON"
            ]
        );
        assert_eq!(
            configures(&mixer.inputs),
            [
                "-n",
                "default",
                "0",
                CONFIGURED_INPUT,
                value,
                "Spa:String:JSON"
            ]
        );
        assert_eq!(
            default_command(&mixer.outputs[0]),
            (
                WPCTL,
                vec!["set-default".into(), mixer.outputs[0].node.0.to_string()]
            )
        );
    }

    #[test]
    fn a_default_device_that_leaves_leaves_no_default() {
        let posts = posts(&[machine(), print(&[removed(HEADPHONES)])]);

        let outputs = &posts.last().unwrap().outputs;

        assert_eq!(
            outputs,
            &vec![device_of(SPEAKER, "Built-in Audio", false, 7)]
        );
    }

    #[test]
    fn the_defaults_metadata_leaving_forgets_the_defaults() {
        let posts = posts(&[machine(), print(&[removed(DEFAULTS_ID)])]);

        let mixer = posts.last().unwrap();

        assert!(
            mixer
                .outputs
                .iter()
                .chain(&mixer.inputs)
                .all(|d| !d.default)
        );
        assert_eq!(mixer.outputs.len(), 2);
    }

    #[test]
    fn a_default_printed_as_text_is_read_too() {
        let value = Json::String(r#"{"name": "alsa_output.analog"}"#.into());

        assert_eq!(default_name(&value).as_deref(), Some("alsa_output.analog"));
        assert_eq!(default_name(&Json::Null), None);
    }

    #[test]
    fn percents_are_on_wpctls_scale() {
        assert_eq!(percent(0.125), 50);
        assert_eq!(percent(1.0), 100);
        assert_eq!(percent(0.0), 0);
        assert_eq!(percent(3.375), 150);
        assert_eq!(percent(1e9), u8::MAX);
    }
}
