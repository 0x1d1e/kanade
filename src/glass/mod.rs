//! Frosted Glass: the material the Island's body, the Dock and the Banners are made of (ADR 0048).
//! Each pane is a `Spot`, its output and a name there; the runtime captures the area around it and
//! the shader softens those colors across the body, under a tint and a quiet edge highlight.

mod body;
mod capture;
mod pane;
mod shader;

pub use self::body::{Body, Join, ahead};
pub use self::capture::{forget, locked, origin, place, shown};
pub use self::pane::{
    Pane, Seen, Variant, blurred, highlight, layer, lock_tint, see_through, swatch,
};
pub use self::shader::shader;
pub use kanade_runtime::backdrop::Spot;

use std::time::Duration;

// how far ahead of its body a pane captures, so the copy fits once it lands as the body morphs:
// about how long one takes, a couple of frames at 60 Hz
pub const LEAD: Duration = Duration::from_millis(33);
