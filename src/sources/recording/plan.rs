//! What a recording is told (ADR 0116, ADR 0117), as pure data and text: the rectangle of an area, the
//! audio source, the arguments of `wf-recorder`, `slurp`, `gst-launch-1.0` and `ffmpeg`, and the list
//! that joins the segments. Nothing here runs a program or reads a file.

use std::fmt;

use super::encode::Encoding;

// the recorder, the area picker and the program that joins the segments of a paused recording
pub const RECORDER: &str = "wf-recorder";
pub const PICKER: &str = "slurp";
pub const JOINER: &str = "ffmpeg";

// a window is recorded by these two, the first encoding what niri casts of it and the second, the
// same program as the joiner, writing the file (ADR 0117)
pub const ENCODER: &str = "gst-launch-1.0";
pub const MUXER: &str = JOINER;

// the H.264 header fields that say BT.709, limited range (`h264_metadata`)
const TAG: &str = "h264_metadata=colour_primaries=1:transfer_characteristics=1:matrix_coefficients=1:video_full_range_flag=0";

// the audio of a window, encoded by ffmpeg's own AAC; spelled so `boundary` does not read it as a color
const AUDIO_CODEC: &str = concat!("a", "ac");

// what `slurp` prints for the area picked: `x,y WxH`, which `Rect::parse` reads and the recorder takes
pub const PICKED: &str = "%x,%y %wx%h";

// the largest side of an area, far past any screen, so a mistyped one is caught
const LARGEST: u32 = 16384;

// the list the segments are joined by, and the file it joins them into, both in the parts folder
pub const LIST: &str = "segments.txt";
pub const JOINED: &str = "joined.mp4";

/*
 * a rectangle of the screen in the compositor's layout coordinates, as `slurp` gives it; the origin
 * may be negative, as an output may sit left of or above the first
 */
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    /*
     * `x,y WxH`, as `slurp -f` prints it and the CLI takes it, in one word or two. Only the form:
     * `fitted` says whether the recorder can take it
     */
    pub fn parse(text: &str) -> Option<Rect> {
        let (origin, size) = text.trim().split_once(char::is_whitespace)?;
        let (x, y) = origin.split_once(',')?;
        let (width, height) = size.trim().split_once(['x', 'X'])?;

        Some(Rect {
            x: x.trim().parse().ok()?,
            y: y.trim().parse().ok()?,
            width: width.trim().parse().ok()?,
            height: height.trim().parse().ok()?,
        })
    }

    /*
     * the rectangle the encoder takes: each side rounded down to an even number of pixels, as
     * VA-API's chroma planes are half size. Refused when a side is left under 2, or is absurd
     */
    pub fn fitted(self) -> Result<Rect, String> {
        let (width, height) = (self.width & !1, self.height & !1);

        if width < 2 || height < 2 {
            return Err(format!(
                "the area {}x{} is too small to record",
                self.width, self.height
            ));
        }

        if self.width > LARGEST || self.height > LARGEST {
            return Err(format!(
                "the area {}x{} is larger than {LARGEST} pixels a side",
                self.width, self.height
            ));
        }

        Ok(Rect {
            width,
            height,
            ..self
        })
    }
}

// `x,y WxH`, the recorder's geometry
impl fmt::Display for Rect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{},{} {}x{}", self.x, self.y, self.width, self.height)
    }
}

/*
 * what a recording hears: nothing, what the speakers play, or the microphone. One source, as the
 * recorder takes one. The microphone is chosen anew for each recording, never a default
 */
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Audio {
    #[default]
    None,

    // the default output's monitor
    System,

    Mic,
}

impl Audio {
    pub fn parse(word: &str) -> Option<Audio> {
        match word {
            "none" => Some(Audio::None),
            "system" => Some(Audio::System),
            "mic" => Some(Audio::Mic),
            _ => None,
        }
    }

    // the one a press on the Surface's choice moves to
    pub fn next(self) -> Audio {
        match self {
            Audio::None => Audio::System,
            Audio::System => Audio::Mic,
            Audio::Mic => Audio::None,
        }
    }

    // the choice as the Surface words it
    pub fn choice(self) -> &'static str {
        match self {
            Audio::None => "Audio: none",
            Audio::System => "Audio: system",
            Audio::Mic => "Audio: mic",
        }
    }

    // what the Activity says it hears, none for no sound
    pub fn heard(self) -> Option<&'static str> {
        match self {
            Audio::None => None,
            Audio::System => Some("system audio"),
            Audio::Mic => Some("microphone"),
        }
    }

    // the source the audio server is asked for; it resolves the default names, so a changed default
    // is followed
    fn source(self) -> Option<&'static str> {
        match self {
            Audio::None => None,
            Audio::System => Some("@DEFAULT_MONITOR@"),
            Audio::Mic => Some("@DEFAULT_SOURCE@"),
        }
    }

    // the recorder's flag
    fn flag(self) -> Option<String> {
        self.source().map(|source| format!("--audio={source}"))
    }
}

// what a recording captures
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    // a whole output, by name
    Output(String),

    Area(Rect),

    /*
     * a window of niri, by the id it had when the recording began, which no other window is given
     * for as long as niri runs. `size` is the window's then, each side even: every frame is fitted
     * to it, so a window that is resized is letterboxed and the segments of a pause join
     */
    Window {
        id: u64,
        size: (u32, u32),

        // its app id, none until the app sets one
        app: Option<String>,
    },
}

impl Source {
    // a window as it stands, `width` by `height`: refused when too small to record
    pub fn window(id: u64, width: u32, height: u32, app: Option<String>) -> Result<Source, String> {
        let fitted = Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
        .fitted()
        .map_err(|_| format!("the window {width}x{height} is too small to record"))?;

        Ok(Source::Window {
            id,
            size: (fitted.width, fitted.height),
            app: app.filter(|app| !app.is_empty()),
        })
    }

    // what the Activity says is recorded
    pub fn said(&self) -> String {
        match self {
            Source::Output(name) => name.clone(),
            Source::Area(rect) => format!("area {}x{}", rect.width, rect.height),
            Source::Window { app: Some(app), .. } => format!("window {app}"),
            Source::Window { id, .. } => format!("window {id}"),
        }
    }
}

/*
 * the arguments of `wf-recorder` for one segment, encoded as `encoding` says (ADR 0116). Damage
 * tracked (no `-D`), as only that is a cast to niri, so the privacy cluster shows it. An area is named by its geometry alone: the recorder
 * finds the output it lies on
 */
pub fn recorder(source: &Source, audio: Audio, file: &str, encoding: &Encoding) -> Vec<String> {
    let mut arguments = encoding.recorder();

    match source {
        Source::Output(name) => arguments.extend([String::from("-o"), name.clone()]),
        Source::Area(rect) => arguments.extend([String::from("-g"), rect.to_string()]),
        // a window is not the recorder's: `encoder` and `muxer` record it
        Source::Window { .. } => {}
    }

    arguments.extend(audio.flag());
    arguments.extend([String::from("-f"), file.to_owned()]);
    arguments
}

/*
 * what `pipewiresrc` is held to: the cast says no pixel aspect, and its DMA-BUF caps then carry
 * `1/2147483647` into `vapostproc`, which cannot calculate the borders of that (`Can't calculate
 * borders`) and scales the picture to fill the frame instead of bordering it, as a window that is
 * resized shows. Any memory (`ANY`), as the cast's is DMA-BUF
 */
const SQUARE_PIXELS: &str = "video/x-raw(ANY),pixel-aspect-ratio=1/1";

/*
 * the arguments of `gst-launch-1.0` for one segment of a window: the PipeWire node niri casts it on,
 * encoded as `encoding` says at the window's size, written to stdout as H.264 for `muxer`.
 * Interrupted, it ends the stream (`-e`) rather than cut it off.
 *
 * `keepalive-time` and `resend-last` make `pipewiresrc` re-emit the last buffer every second when
 * the window is static (no damage), so a still window keeps producing buffers and `muxer` stamps
 * them by the wall clock into the duration it really had. Nothing here paces the frames (no
 * `videorate`): a variable frame rate is the recording (ADR 0117). `pipewiresrc` offers no fixed
 * framerate, and fed caps that fix none, `videorate` emits a single frame: a 30 s take became a
 * file of 2 s. It must never be added back for the still-window case
 *
 * `encoding.guard()` follows the square-pixel caps and ends the chain when buffers stop arriving
 * for `QUIET`, so a chain that hangs is a failure `falls_back` gets, not one that waits forever.
 * It sits before the encoding chain, where it sees what the cast delivers rather than what the
 * encoder has got round to emitting
 */
pub fn encoder(node: u64, size: (u32, u32), encoding: &Encoding) -> Vec<String> {
    let mut arguments: Vec<String> = [
        "-q",
        "-e",
        "pipewiresrc",
        &format!("path={node}"),
        "do-timestamp=true",
        "keepalive-time=1000",
        "resend-last=true",
        "!",
        SQUARE_PIXELS,
        "!",
    ]
    .map(String::from)
    .to_vec();

    arguments.extend(encoding.guard());
    arguments.extend(encoding.chain(size));
    arguments.extend(
        [
            "!",
            "h264parse",
            "config-interval=-1",
            "!",
            "video/x-h264,stream-format=byte-stream,alignment=au",
            "!",
            "fdsink",
            "fd=1",
        ]
        .map(String::from),
    );
    arguments
}

/*
 * the arguments of `ffmpeg` that write the stream of `encoder` on stdin, and the audio if any, to
 * `file`, copying the video. It stamps by the wall clock: a PipeWire window cast gives every frame the
 * same timestamp through GStreamer's own muxer, and the clock is the only time the frames have. The
 * audio is stamped by it too, so sound and picture are in step to about a frame at best, not
 * guaranteed. A file that is there is never replaced (`-n`). The mp4 is fragmented, a fragment each
 * second (`frag_duration`, as a still window has long gaps between keyframes) and flushed to the file
 * as it is written (`flush_packets`, or a small stream stays in ffmpeg's buffer), so a writer that is
 * killed leaves a file that plays up to its last fragment instead of one with no `moov`. The stream
 * is tagged as BT.709 limited range, the matrix `encoder` converts with, which the encoder itself
 * does not write
 */
pub fn muxer(audio: Audio, file: &str) -> Vec<String> {
    let mut arguments: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "error",
        "-use_wallclock_as_timestamps",
        "1",
        "-probesize",
        "65536",
        "-analyzeduration",
        "1",
        "-f",
        "h264",
        "-i",
        "-",
    ]
    .map(String::from)
    .to_vec();

    if let Some(source) = audio.source() {
        arguments.extend(
            [
                "-f",
                "pulse",
                "-use_wallclock_as_timestamps",
                "1",
                "-i",
                source,
                "-c:a",
                AUDIO_CODEC,
                // the audio never ends of itself: the video ending, on the stop, ends the file
                "-shortest",
            ]
            .map(String::from),
        );
    }

    arguments.extend(
        [
            "-c:v",
            "copy",
            "-bsf:v",
            TAG,
            "-fps_mode",
            "vfr",
            "-movflags",
            "+frag_keyframe+empty_moov+default_base_moof",
            "-frag_duration",
            "1000000",
            "-flush_packets",
            "1",
            "-n",
            file,
        ]
        .map(String::from),
    );
    arguments
}

// the arguments of `slurp`: the area's dimensions shown as it is drawn, printed as `PICKED`
pub fn picker() -> [&'static str; 3] {
    ["-d", "-f", PICKED]
}

// the name of segment `number` (from 1): its file in the parts folder, when it is kept there
pub fn segment(number: u32) -> String {
    format!("{number:03}.mp4")
}

// whether `name` is one Kanade gives to a file of its parts folder, which only it may delete
pub fn made(name: &str) -> bool {
    name == LIST
        || name == JOINED
        || name
            .strip_suffix(".mp4")
            .is_some_and(|number| number.len() >= 3 && number.bytes().all(|b| b.is_ascii_digit()))
}

/*
 * the list `ffmpeg`'s concat demuxer reads: one `file` line per segment, by its name in the parts
 * folder, which the demuxer reads relative to the list. A name is single-quoted, a quote in it
 * closed, escaped and opened again, as the demuxer's quoting says
 */
pub fn list(names: &[String]) -> String {
    names
        .iter()
        .map(|name| format!("file {}\n", quote(name)))
        .collect()
}

fn quote(name: &str) -> String {
    format!("'{}'", name.replace('\'', r"'\''"))
}

// the arguments of `ffmpeg` that join the segments `list` names, copying what they hold
pub fn joiner<'a>(list: &'a str, joined: &'a str) -> [&'a str; 15] {
    [
        "-hide_banner",
        "-loglevel",
        "error",
        "-nostdin",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        list,
        "-c",
        "copy",
        "-map",
        "0",
        joined,
    ]
}

#[cfg(test)]
mod tests {
    use super::super::encode::{Encoder, Kind, QUIET, Quality};
    use super::*;

    #[test]
    fn an_area_reads_as_slurp_prints_it() {
        let area = Rect {
            x: 100,
            y: 200,
            width: 800,
            height: 600,
        };

        assert_eq!(Rect::parse("100,200 800x600"), Some(area));
        assert_eq!(Rect::parse("100,200 800x600\n"), Some(area));
        // two words, as the shell may split it
        assert_eq!(Rect::parse("100,200   800x600"), Some(area));
        // an output left of the first has a negative origin
        assert_eq!(
            Rect::parse("-1920,-30 1920x1080").map(|rect| (rect.x, rect.y)),
            Some((-1920, -30))
        );

        for bad in [
            "",
            "100,200",
            "100 200 800x600",
            "100,200 800",
            "a,b 1x1",
            "1,2 -3x4",
        ] {
            assert_eq!(Rect::parse(bad), None, "{bad:?}");
        }
    }

    // VA-API's chroma planes are half size: an odd side is rounded down, never up past the screen
    #[test]
    fn an_area_is_rounded_to_even_sides() {
        let rect = |width, height| Rect {
            x: -5,
            y: 7,
            width,
            height,
        };

        assert_eq!(rect(801, 601).fitted(), Ok(rect(800, 600)));
        assert_eq!(rect(800, 600).fitted(), Ok(rect(800, 600)));
        assert_eq!(rect(3, 2).fitted(), Ok(rect(2, 2)));
        assert!(rect(1, 600).fitted().is_err());
        assert!(rect(800, 0).fitted().is_err());
        assert!(rect(16385, 600).fitted().is_err());
        assert_eq!(
            rect(801, 601).fitted().map(|fitted| fitted.to_string()),
            Ok(String::from("-5,7 800x600"))
        );
    }

    fn vaapi() -> Encoding {
        Encoding {
            encoder: Encoder {
                kind: Kind::Vaapi,
                node: Some(String::from("/dev/dri/renderD128")),
                first: true,
            },
            quality: Quality::High,
            threads: 2,
            download: Default::default(),
        }
    }

    /*
     * a confirmed defect, once, at the seam it crossed: `pipewiresrc` gives a cast no pixel aspect,
     * which `vapostproc` takes as 1/2147483647 and then cannot calculate borders for (a window
     * resized mid-take was stretched, not bordered, live), and the cast gives no fixed framerate,
     * which `videorate` then answered with a single frame (a 30 s take became a 2 s file, live)
     */
    #[test]
    fn a_windows_cast_is_held_to_square_pixels_and_never_paced() {
        let arguments = encoder(7, (640, 480), &vaapi());
        let at = |word: &str| arguments.iter().position(|found| found == word);
        let source = at("pipewiresrc");
        let keepalive = at("keepalive-time=1000");
        let resend = at("resend-last=true");
        let held = at("video/x-raw(ANY),pixel-aspect-ratio=1/1");
        let converter = at("vapostproc");

        assert!(
            source.is_some() && keepalive.is_some() && resend.is_some() && held.is_some(),
            "{arguments:?}"
        );
        assert!(
            source < keepalive && keepalive < held && resend < held && held < converter,
            "{arguments:?}"
        );

        // the frames keep the time they arrived at: nothing here forces a framerate
        for paced in ["videorate", "video/x-raw(ANY),framerate=30/1"] {
            assert!(!arguments.iter().any(|word| word == paced), "{arguments:?}");
        }
    }

    /*
     * an enforced boundary: a window's chain must end when buffers stop arriving, or a wedged one
     * waits until the user stops the take. The guard must sit between the cast's caps and the
     * encoding chain, never after the encoder: `x264enc` holds its first byte for 9 s at a still
     * window's cadence, and a guard there would fail a take that is recording correctly
     */
    #[test]
    fn a_windows_chain_is_guarded_before_it_encodes() {
        let encoding = vaapi();
        let arguments = encoder(7, (640, 480), &encoding);
        let at = |word: &str| arguments.iter().position(|found| found == word);
        let (held, guard, converter, encoder_element) = (
            at(SQUARE_PIXELS),
            at("watchdog"),
            at("vapostproc"),
            at("vah264enc"),
        );

        assert!(
            held.is_some() && guard.is_some() && converter.is_some(),
            "{arguments:?}"
        );
        assert!(held < guard && guard < converter, "{arguments:?}");
        assert!(guard < encoder_element, "{arguments:?}");

        // the guard is the same in every arm, and a trial without it could pass where a chain cannot
        for kind in [Kind::Vaapi, Kind::Nvenc, Kind::Software] {
            let arm = Encoding {
                encoder: Encoder {
                    kind,
                    ..encoding.encoder.clone()
                },
                ..vaapi()
            };
            let trial = arm.gst_trial();

            assert_eq!(
                arm.guard(),
                ["watchdog", "timeout=3000", "!"].map(String::from).to_vec()
            );
            assert!(trial.iter().any(|word| word == "watchdog"), "{trial:?}");
            assert!(trial.contains(&format!("timeout={}", QUIET.as_millis())));
        }
    }

    #[test]
    fn the_recorder_gets_its_encoding_its_source_and_one_audio_source() {
        let screen = Source::Output(String::from("DP-1"));
        let area = Source::Area(Rect {
            x: 10,
            y: 20,
            width: 640,
            height: 480,
        });
        let encoding = vaapi();
        let head = encoding.recorder();
        let with = |rest: &[&str]| -> Vec<String> {
            head.iter()
                .cloned()
                .chain(rest.iter().map(|word| String::from(*word)))
                .collect()
        };

        assert_eq!(
            recorder(&screen, Audio::None, "/v/a.mp4", &encoding),
            with(&["-o", "DP-1", "-f", "/v/a.mp4"])
        );
        assert_eq!(
            recorder(&area, Audio::System, "/v/a.mp4", &encoding),
            with(&[
                "-g",
                "10,20 640x480",
                "--audio=@DEFAULT_MONITOR@",
                "-f",
                "/v/a.mp4"
            ])
        );
        assert_eq!(
            recorder(&screen, Audio::Mic, "/v/a.mp4", &encoding),
            with(&["-o", "DP-1", "--audio=@DEFAULT_SOURCE@", "-f", "/v/a.mp4"])
        );

        // no argument turns damage tracking off, which would hide the recording from niri's casts,
        // whatever encodes
        for kind in [Kind::Vaapi, Kind::Nvenc, Kind::Software] {
            let encoding = Encoding {
                encoder: Encoder {
                    kind,
                    ..encoding.encoder.clone()
                },
                ..vaapi()
            };

            for audio in [Audio::None, Audio::System, Audio::Mic] {
                assert!(
                    !recorder(&screen, audio, "f", &encoding)
                        .iter()
                        .any(|argument| argument == "-D" || argument == "--no-damage")
                );
            }
        }
    }

    #[test]
    fn the_audio_choice_cycles_through_all_three_and_defaults_to_none() {
        assert_eq!(Audio::default(), Audio::None);
        assert_eq!(Audio::None.next(), Audio::System);
        assert_eq!(Audio::System.next(), Audio::Mic);
        assert_eq!(Audio::Mic.next(), Audio::None);

        for audio in [Audio::None, Audio::System, Audio::Mic] {
            let word = match audio {
                Audio::None => "none",
                Audio::System => "system",
                Audio::Mic => "mic",
            };

            assert_eq!(Audio::parse(word), Some(audio));
        }
    }

    // only Kanade's own files in the parts folder are ever deleted
    #[test]
    fn only_kanades_own_names_are_its_files() {
        for made_here in [segment(1).as_str(), "012.mp4", LIST, JOINED] {
            assert!(made(made_here), "{made_here}");
        }

        for not_mine in ["", "a.mp4", "01.mp4", "001.mkv", "notes.txt", "001.mp4.bak"] {
            assert!(!made(not_mine), "{not_mine}");
        }
    }

    // the list is read by ffmpeg, so a name with a quote or a space must not end its line early
    #[test]
    fn the_list_quotes_every_name() {
        assert_eq!(
            list(&[segment(1), segment(12)]),
            "file '001.mp4'\nfile '012.mp4'\n"
        );
        assert_eq!(
            list(&[String::from("it's a file.mp4")]),
            "file 'it'\\''s a file.mp4'\n"
        );
        assert_eq!(list(&[String::from("a b.mp4")]), "file 'a b.mp4'\n");
    }

    // a window is recorded at the size it had, each side even, and refused when there is no picture
    #[test]
    fn a_window_is_fitted_to_even_sides_and_named_by_its_app() {
        assert_eq!(
            Source::window(7, 1433, 1018, Some(String::from("kitty"))),
            Ok(Source::Window {
                id: 7,
                size: (1432, 1018),
                app: Some(String::from("kitty"))
            })
        );
        assert!(Source::window(7, 1, 1018, None).is_err());

        let named = Source::window(7, 800, 600, Some(String::from("kitty"))).unwrap();
        let unnamed = Source::window(7, 800, 600, Some(String::new())).unwrap();

        assert_eq!(named.said(), "window kitty");
        assert_eq!(unnamed.said(), "window 7");
    }

    // data integrity: the file a window is written to is never replaced, is fragmented so a killed writer
    // leaves it playable, and one audio source is read
    #[test]
    fn a_window_is_written_without_replacing_and_hears_one_source() {
        for audio in [Audio::None, Audio::System, Audio::Mic] {
            let muxer = muxer(audio, "/v/a.mp4");

            assert!(muxer.contains(&String::from("-n")));
            // the stream is tagged BT.709 limited range, which its encoder does not write
            assert!(muxer.contains(&String::from(TAG)));
            assert!(!muxer.contains(&String::from("-y")));
            assert!(muxer.iter().any(|word| word.contains("empty_moov")));
            assert!(muxer.contains(&String::from("-frag_duration")));
            assert_eq!(
                muxer
                    .iter()
                    .filter(|word| word.starts_with("@DEFAULT_"))
                    .count(),
                usize::from(audio != Audio::None)
            );
        }
    }
}
