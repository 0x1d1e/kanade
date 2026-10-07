/*
 * the island's icons, drawn from the svgs in icons/ on a 20 unit grid. Each is rasterized
 * once per (icon, size, scale) and ink, then drawn as an image: a body morphing under its
 * rounded clip only moves a texture, where paths re-rendered the whole canvas every frame (#36)
 */

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex, PoisonError};

use amane::{Color, Image, Rectangle};

use crate::{raster, theme};

// rasterized at twice its size, crisp at scale 2, like the island's pictures
const SCALE: f32 = 2.0;

// every svg in icons/ opens with this, so its shapes can be taken out and layered
const HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round">
"#;

const TAIL: &str = "</svg>\n";

// how far past the slash's own line it cuts the icon under it
const CUT: f32 = 1.7 + 3.0;

const SPEAKER: &str = include_str!("icons/speaker.svg");
const WAVE_NEAR: &str = include_str!("icons/wave-near.svg");
const WAVE_FAR: &str = include_str!("icons/wave-far.svg");
const MUTE: &str = include_str!("icons/mute.svg");
const MICROPHONE: &str = include_str!("icons/microphone.svg");
const SUN: &str = include_str!("icons/sun.svg");
const WIFI_DOT: &str = include_str!("icons/wifi-dot.svg");
const WIFI_NEAR: &str = include_str!("icons/wifi-near.svg");
const WIFI_MID: &str = include_str!("icons/wifi-mid.svg");
const WIFI_FAR: &str = include_str!("icons/wifi-far.svg");
const BLUETOOTH: &str = include_str!("icons/bluetooth.svg");
const BELL: &str = include_str!("icons/bell.svg");
const MOON: &str = include_str!("icons/moon.svg");
const BOLT: &str = include_str!("icons/bolt.svg");
const SEARCH: &str = include_str!("icons/search.svg");
const CAPTURE: &str = include_str!("icons/capture.svg");
const STOPWATCH: &str = include_str!("icons/stopwatch.svg");
const CAMERA: &str = include_str!("icons/camera.svg");
const PREVIOUS: &str = include_str!("icons/previous.svg");
const PLAY: &str = include_str!("icons/play.svg");
const PAUSE: &str = include_str!("icons/pause.svg");
const NEXT: &str = include_str!("icons/next.svg");
const DISMISS: &str = include_str!("icons/dismiss.svg");
const BACK: &str = include_str!("icons/back.svg");
const FORWARD: &str = include_str!("icons/forward.svg");
const LOCK: &str = include_str!("icons/lock.svg");
const SLASH: &str = include_str!("icons/slash.svg");

// drawn, not a font's glyph, so it looks the same whatever fonts the machine has
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Icon {
    // waves by level: none silent, one up to half, two above
    Speaker(u8),
    SpeakerMuted,
    Microphone,
    MicrophoneMuted,
    Sun,
    Wifi,

    // a network's signal, its arcs by bars from none to three; three is `Wifi`
    Signal(u8),

    Bluetooth,
    Bell,

    // Do Not Disturb, with a bite cut out of it
    Moon,

    // a power profile
    Bolt,

    // the Launcher's magnifier
    Search,

    // ▣, a screen with something captured off it
    Capture,

    // a face with a hand and the crown it starts by
    Stopwatch,

    // a video camera, its lens to the right
    Camera,

    // media transport; a triangle's weight sits left of its box, so Play sits right to look centered
    Previous,
    Play,
    Pause,
    Next,

    // the cross that closes a notification
    Dismiss,

    // chevrons out of a sub-surface and into one
    Back,
    Forward,

    // a network that takes a password
    Lock,
}

// the icons drawn so far, by what tells their pixels apart, and the svg each was written to
type Drawn = HashMap<(Icon, bool, [u8; 4]), PathBuf>;

static DRAWN: LazyLock<Mutex<Drawn>> = LazyLock::new(|| Mutex::new(HashMap::new()));

impl Icon {
    pub(crate) fn muted(self) -> Icon {
        match self {
            Icon::Speaker(_) => Icon::SpeakerMuted,
            Icon::Microphone => Icon::MicrophoneMuted,
            icon => icon,
        }
    }

    pub(crate) fn draw(self, side: f32) -> Rectangle {
        self.image(side, false, theme::ISLAND.on_surface)
    }

    // struck through, for something gone or off
    pub(crate) fn crossed(self, side: f32) -> Rectangle {
        self.image(side, true, theme::ISLAND.on_surface)
    }

    // in `ink`, like the body's color on a filled button; cuts show what it sits on
    pub(crate) fn on(self, side: f32, ink: Color) -> Rectangle {
        self.image(side, false, ink)
    }

    fn image(self, side: f32, crossed: bool, ink: Color) -> Rectangle {
        let (path, pixels) = self.source(side, crossed, ink);

        Rectangle::new()
            .width(side)
            .height(side)
            .fill(Image::stretch(path).thumbnail(pixels, pixels))
    }

    /*
     * the svg and the pixels it is drawn at, which with the path is what Amane caches the
     * raster by; the same icon at the same size and ink gives the same pair every frame
     */
    fn source(self, side: f32, crossed: bool, ink: Color) -> (PathBuf, u32) {
        let pixels = (side * SCALE).round() as u32;

        let crossed = crossed || self == Icon::MicrophoneMuted;
        let ink = [ink.red(), ink.green(), ink.blue(), ink.alpha()];
        let key = (self.drawn_as(), crossed, ink);

        let mut drawn = DRAWN.lock().unwrap_or_else(PoisonError::into_inner);

        let path = drawn
            .entry(key)
            .or_insert_with(|| raster::write("icons", "svg", key.0.svg(crossed, ink).as_bytes()));

        (path.clone(), pixels)
    }

    // one icon for every Speaker level that draws the same waves
    fn drawn_as(self) -> Icon {
        match self {
            Icon::Speaker(0) => Icon::Speaker(0),
            Icon::Speaker(1..=50) => Icon::Speaker(1),
            Icon::Speaker(_) => Icon::Speaker(51),
            Icon::MicrophoneMuted => Icon::Microphone,
            Icon::Signal(3..) => Icon::Wifi,
            icon => icon,
        }
    }

    // the svgs drawn one over another
    fn layers(self) -> &'static [&'static str] {
        match self {
            Icon::Speaker(0) => &[SPEAKER],
            Icon::Speaker(1..=50) => &[SPEAKER, WAVE_NEAR],
            Icon::Speaker(_) => &[SPEAKER, WAVE_NEAR, WAVE_FAR],
            Icon::SpeakerMuted => &[SPEAKER, MUTE],
            Icon::Microphone | Icon::MicrophoneMuted => &[MICROPHONE],
            Icon::Sun => &[SUN],
            Icon::Wifi | Icon::Signal(3..) => &[WIFI_DOT, WIFI_NEAR, WIFI_MID, WIFI_FAR],
            Icon::Signal(0) => &[WIFI_DOT],
            Icon::Signal(1) => &[WIFI_DOT, WIFI_NEAR],
            Icon::Signal(_) => &[WIFI_DOT, WIFI_NEAR, WIFI_MID],
            Icon::Bluetooth => &[BLUETOOTH],
            Icon::Bell => &[BELL],
            Icon::Moon => &[MOON],
            Icon::Bolt => &[BOLT],
            Icon::Search => &[SEARCH],
            Icon::Capture => &[CAPTURE],
            Icon::Stopwatch => &[STOPWATCH],
            Icon::Camera => &[CAMERA],
            Icon::Previous => &[PREVIOUS],
            Icon::Play => &[PLAY],
            Icon::Pause => &[PAUSE],
            Icon::Next => &[NEXT],
            Icon::Dismiss => &[DISMISS],
            Icon::Back => &[BACK],
            Icon::Forward => &[FORWARD],
            Icon::Lock => &[LOCK],
        }
    }

    // the layers in `ink`, a crossed one with a slash cut into it and drawn over
    fn svg(self, crossed: bool, ink: [u8; 4]) -> String {
        let mut drawing: String = self.layers().iter().map(|layer| content(layer)).collect();

        if crossed {
            let slash = content(SLASH);

            drawing = format!(
                "<mask id=\"slash\" stroke=\"black\" stroke-width=\"{CUT}\">\
                 <rect width=\"20\" height=\"20\" fill=\"white\" stroke=\"none\"/>\n{slash}</mask>\n\
                 <g mask=\"url(#slash)\">\n{drawing}</g>\n{slash}"
            );
        }

        let [red, green, blue, alpha] = ink;
        let opacity = f32::from(alpha) / 255.0;

        let head = HEAD.replacen(
            "<svg ",
            &format!("<svg color=\"#{red:02x}{green:02x}{blue:02x}\" opacity=\"{opacity}\" "),
            1,
        );

        format!("{head}{drawing}{TAIL}")
    }
}

// what an svg in icons/ draws, without the svg element around it
fn content(svg: &'static str) -> &'static str {
    svg.strip_prefix(HEAD)
        .and_then(|svg| svg.strip_suffix(TAIL))
        .expect("an svg in icons/ opens with HEAD and closes with TAIL")
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Icon; 28] = [
        Icon::Speaker(0),
        Icon::Speaker(30),
        Icon::Speaker(80),
        Icon::SpeakerMuted,
        Icon::Microphone,
        Icon::MicrophoneMuted,
        Icon::Sun,
        Icon::Wifi,
        Icon::Bluetooth,
        Icon::Bell,
        Icon::Moon,
        Icon::Bolt,
        Icon::Search,
        Icon::Capture,
        Icon::Stopwatch,
        Icon::Camera,
        Icon::Previous,
        Icon::Play,
        Icon::Pause,
        Icon::Next,
        Icon::Dismiss,
        Icon::Speaker(100),
        Icon::Signal(0),
        Icon::Signal(1),
        Icon::Signal(2),
        Icon::Back,
        Icon::Forward,
        Icon::Lock,
    ];

    #[test]
    fn every_layer_is_an_svg_on_the_grid() {
        for icon in ALL {
            for layer in icon.layers() {
                assert!(!content(layer).is_empty(), "{icon:?}");
            }
        }

        assert!(content(SLASH).contains("<path"));
    }

    #[test]
    fn speaker_levels_share_the_icon_that_draws_their_waves() {
        assert_eq!(Icon::Speaker(1).drawn_as(), Icon::Speaker(50).drawn_as());
        assert_eq!(Icon::Speaker(51).drawn_as(), Icon::Speaker(100).drawn_as());
        assert_ne!(Icon::Speaker(0).drawn_as(), Icon::Speaker(1).drawn_as());
        assert_ne!(Icon::Speaker(50).drawn_as(), Icon::Speaker(51).drawn_as());

        // the level decides the layers, so sharing an icon never shares a wrong drawing
        for (low, high) in [(1, 50), (51, 100)] {
            assert_eq!(Icon::Speaker(low).layers(), Icon::Speaker(high).layers());
        }
    }

    #[test]
    fn full_signal_is_the_wifi_icon() {
        assert_eq!(Icon::Signal(3).drawn_as(), Icon::Wifi);
        assert_eq!(Icon::Signal(3).layers(), Icon::Wifi.layers());
        assert_eq!(Icon::Signal(1).layers().len(), 2);
        assert_ne!(Icon::Signal(2).drawn_as(), Icon::Wifi);
    }

    #[test]
    fn a_crossed_icon_is_cut_and_slashed() {
        let svg = Icon::Wifi.svg(true, [242, 242, 247, 255]);

        assert!(svg.starts_with("<svg color=\"#f2f2f7\" opacity=\"1\" xmlns="));
        assert!(svg.contains("<g mask=\"url(#slash)\">"));
        assert!(svg.ends_with(&format!("{}{TAIL}", content(SLASH))));

        assert_eq!(
            Icon::MicrophoneMuted.source(18.0, false, theme::ISLAND.on_surface),
            Icon::Microphone.source(18.0, true, theme::ISLAND.on_surface),
        );
    }

    /*
     * the regression behind #36: whatever the body's geometry, an icon asks for the same svg at
     * the same pixels every frame, so Amane rasterizes it once and only draws its texture after
     */
    #[test]
    fn every_frame_reuses_the_icons_the_first_one_drew() {
        // an ink only this test uses, so other tests filling the cache don't count
        let ink = Color::rgb(1, 2, 3);

        let drawn = || {
            let drawn = DRAWN.lock().unwrap_or_else(PoisonError::into_inner);

            drawn.keys().filter(|key| key.2 == [1, 2, 3, 255]).count()
        };

        let frame = || -> Vec<(PathBuf, u32)> {
            ALL.iter()
                .flat_map(|icon| [16.0, 18.0, 20.0, 28.0].map(|side| icon.source(side, false, ink)))
                .chain(ALL.iter().map(|icon| icon.source(20.0, true, ink)))
                .collect()
        };

        let first = frame();
        let written = drawn();

        for _ in 0..10 {
            assert_eq!(frame(), first);
            assert_eq!(drawn(), written);
        }

        // one svg per drawing and ink, not per size: 26 drawings, each plain and crossed
        assert_eq!(written, 52);

        for (path, _) in &first {
            assert!(path.exists(), "{}", path.display());
        }
    }
}
