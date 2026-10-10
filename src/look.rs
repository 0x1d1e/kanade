//! The shell's look and feel as the config names it: each a choice of a few named options, which
//! `config` reads from and writes to its keys, and Settings offers as one list.

use kanade_runtime::{Horizontal, Vertical};

use crate::island::geometry::{self, Hang, Side};

// an enum of named options, its first the default
macro_rules! choice {
    ($(#[$meta:meta])* $name:ident { $($(#[$variant_meta:meta])* $variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
        pub enum $name {
            #[default]
            $($(#[$variant_meta])* $variant),+
        }

        impl $name {
            pub const NAMES: &[&str] = &[$($text),+];

            pub fn name(self) -> &'static str {
                match self {
                    $($name::$variant => $text),+
                }
            }

            // an unknown name is the default; the config checks names before it gets here
            pub fn named(name: &str) -> Self {
                match name {
                    $($text => $name::$variant,)+
                    _ => $name::default(),
                }
            }
        }
    };
}

choice! {
    // what the Island's body, the Dock and the Banners are made of, `appearance.material` (ADR 0024)
    Material {
        /*
         * Apple's clear liquid glass: what is under it seen sharp, bent into the rim, under a tint
         * that follows how bright it is, lit at the rim (ADR 0037); the transparent look where the
         * screen cannot be captured
         */
        LiquidGlass = "liquid-glass",

        // one flat tone, the glass tone's, with nothing seen through it, nothing bent or lit
        Monochrome = "monochrome",
    }
}

choice! {
    // how bright the light on the Island's body is, its rim's and the pointer's,
    // `appearance.highlight`
    Highlight {
        // a glint on the corner facing the light, quiet enough never to outshine the time
        Subtle = "subtle",
        Standard = "standard",
        Bright = "bright",

        // unlit: what the glass bends, its tint, and light glass's faint outline
        Off = "off",
    }
}

impl Highlight {
    // how much of the shader's standard light the rim catches
    pub fn share(self) -> f32 {
        match self {
            Highlight::Subtle => 0.7,
            Highlight::Standard => 1.0,
            Highlight::Bright => 1.35,
            Highlight::Off => 0.0,
        }
    }
}

choice! {
    // dark glass under light text, or light glass under dark text, `appearance.tone`
    Tone {
        Dark = "dark",
        Light = "light",
    }
}

choice! {
    // how the Island's and the Dock's springs settle, `appearance.motion`
    Bounce {
        // a little past the target and back, like macOS
        Bouncy = "bouncy",

        // straight to it, no overshoot
        Smooth = "smooth",

        // a quick settle with a hint of overshoot
        Snappy = "snappy",

        // a lot past the target and back
        Playful = "playful",
    }
}

impl Bounce {
    // the springs' damping ratio, 1 critical
    pub fn damping(self) -> f32 {
        match self {
            Bounce::Bouncy => 0.72,
            Bounce::Smooth => 1.0,
            Bounce::Snappy => 0.86,
            Bounce::Playful => 0.56,
        }
    }

    // the one with this damping, else the default
    pub fn of(damping: f32) -> Self {
        Bounce::NAMES
            .iter()
            .map(|name| Bounce::named(name))
            .find(|bounce| bounce.damping() == damping)
            .unwrap_or_default()
    }
}

choice! {
    // which edge of the output a bar hangs from; at the left or right, in the middle of it
    Edge {
        Top = "top",
        Bottom = "bottom",
        Left = "left",
        Right = "right",
    }
}

choice! {
    // where along a top or bottom edge a bar sits; a side edge takes the middle
    Along {
        Center = "center",
        Left = "left",
        Right = "right",
    }
}

choice! {
    // how much a Dock icon grows under the pointer, `dock.magnification`
    Magnify {
        // like macOS's default: the icon under the pointer about 1.6 times, its neighbors less
        Classic = "classic",
        Subtle = "subtle",
        Large = "large",
        Off = "off",
    }
}

impl Magnify {
    // how many times its size the icon right under the pointer grows to
    pub fn most(self) -> f32 {
        match self {
            Magnify::Classic => 1.6,
            Magnify::Subtle => 1.3,
            Magnify::Large => 2.0,
            Magnify::Off => 1.0,
        }
    }
}

choice! {
    // how big the Dock's icons are at rest, their gaps and the swell's reach with them, `dock.size`
    DockSize {
        Medium = "medium",
        Small = "small",
        Large = "large",
    }
}

impl DockSize {
    // an icon's side at rest
    pub const fn icon(self) -> f32 {
        match self {
            DockSize::Small => 28.0,
            DockSize::Medium => 36.0,
            DockSize::Large => 48.0,
        }
    }
}

choice! {
    // how the Island and the Dock join where they share an edge and a side, `dock.merge`
    Merge {
        // the Dock stays as a plate on the edge; the Island rises from its middle on a liquid neck
        Crown = "crown",

        // one pane: the Island is the Dock's middle piece, its apps flanking it and parting as it widens
        Keystone = "keystone",

        // at rest only the Island; the Dock unfolds out of it under the pointer
        Fold = "fold",

        // the Dock sits beside the Island, further in, and steps aside as it grows
        Off = "off",
    }
}

choice! {
    // what moves on the Media Peek and Surface while a track plays, `media.visualizer`
    Visualizer {
        // bars rising and falling, like the Dynamic Island
        Bars = "bars",

        // a wave running through
        Wave = "wave",

        // dots pulsing
        Dots = "dots",
        Off = "off",
    }
}

choice! {
    // what the lock screen shows behind the clock, `lock.backdrop`
    LockBackdrop {
        // the wallpaper shown on the output, softly blurred, like macOS
        Blurred = "blurred",

        // the wallpaper, sharp
        Wallpaper = "wallpaper",

        // the wallpaper, dimmed
        Dimmed = "dimmed",

        // a solid color
        Solid = "solid",
    }
}

choice! {
    // how a Banner comes in, `banners.entrance`
    Entrance {
        // out from under the Island: a copy of its pill morphs and stretches into the Banner
        Morph = "morph",

        // in from the edge the Island is on
        Drop = "drop",

        // fading in place
        Fade = "fade",
    }
}

// where a bar sits on its output
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Anchor {
    pub edge: Edge,
    pub along: Along,
}

impl Anchor {
    pub const BOTTOM: Anchor = Anchor {
        edge: Edge::Bottom,
        along: Along::Center,
    };
}

// where a bar sits and how it shares the output
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    pub anchor: Anchor,

    // out of the way until the pointer touches its edge
    pub autohide: bool,

    // keeps windows off its strip of the output, rather than floating over them
    pub reserve: bool,
}

impl Anchor {
    // the Island's geometry hangs its body from the same edge and side
    pub fn hang(self) -> Hang {
        Hang {
            edge: match self.edge {
                Edge::Top => geometry::Edge::Top,
                Edge::Bottom => geometry::Edge::Bottom,
                Edge::Left => geometry::Edge::Left,
                Edge::Right => geometry::Edge::Right,
            },
            side: match self.along {
                Along::Left => Side::Start,
                Along::Center => Side::Middle,
                Along::Right => Side::End,
            },
        }
    }

    // where along its edge it sits: as `along` says on the top or bottom, in the middle of a side
    pub fn sits(self) -> Along {
        if self.sideways() {
            Along::Center
        } else {
            self.along
        }
    }

    // from the left or right edge
    pub fn sideways(self) -> bool {
        matches!(self.edge, Edge::Left | Edge::Right)
    }

    // how far a shape held to this edge reaches off it: its width from a side, else its height
    pub fn thickness(self, shape: geometry::Shape) -> f32 {
        if self.sideways() {
            shape.width
        } else {
            shape.height
        }
    }

    pub fn vertical(self) -> Vertical {
        match self.edge {
            Edge::Top => Vertical::Top,
            Edge::Bottom => Vertical::Bottom,
            Edge::Left | Edge::Right => Vertical::Middle,
        }
    }

    pub fn horizontal(self) -> Horizontal {
        match (self.edge, self.along) {
            (Edge::Left, _) | (Edge::Top | Edge::Bottom, Along::Left) => Horizontal::Left,
            (Edge::Right, _) | (Edge::Top | Edge::Bottom, Along::Right) => Horizontal::Right,
            (Edge::Top | Edge::Bottom, Along::Center) => Horizontal::Middle,
        }
    }
}
