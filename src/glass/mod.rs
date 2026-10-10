//! Glass: the material the Island's body, the Dock and the Banners are made of (ADR 0024). One
//! shader pass (`glass.wgsl`) over a pane draws, in order, what is behind its edge bent, a tint for
//! the content to read, then the specular rim, the edge's depth, a sheen and the light that follows
//! the pointer.
//!
//! Liquid glass adds what is behind each pane (ADR 0037): the runtime copies the screen around the
//! body (`backdrop`), and the shader bends that copy into the rim, whose tint follows how bright
//! it is. Each pane is a `Spot`: its output and a name of its own there, and everything the runtime
//! keeps for it is kept by that.

mod body;
mod capture;
mod pane;
mod shader;

pub use self::body::{Body, Join, ahead};
pub use self::capture::{forget, locked, place, shown};
pub use self::pane::{Pane, Seen, Variant, highlight, layer, lock_tint, swatch};
pub use self::shader::shader;
pub use kanade_runtime::backdrop::Spot;

use std::time::Duration;

// how far ahead of its body a pane captures, so the copy fits once it lands as the body morphs:
// about how long one takes, a couple of frames at 60 Hz
pub const LEAD: Duration = Duration::from_millis(33);
