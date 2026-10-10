//! Kanade's runtime: the Wayland event loop, windows, input, layout, drawing
//! and the Service store every view reads (ADR 0028). Parts are under a
//! third-party MIT notice, see THIRD_PARTY_NOTICES.md.
//!
//! The only `unsafe` is glibc's allocator tuning (`allocator.rs`) and handing
//! the GPU libwayland's surface (`graphics/gpu/target.rs`).

mod allocator;
mod animation;
mod app;
mod changes;
mod frame;
mod full;
mod graphics;
mod input;
mod ipc;
mod layer_window;
mod macros;
mod monitor;
mod placement;
mod style;
mod timing;
mod toplevels;
mod wayland;
mod widgets;
mod window;

pub mod backdrop;
pub mod service;
pub mod worker;

pub use animation::{Animation, Blend, Easing, request_frame};
pub use app::App;
pub use full::Full;
pub use graphics::{Cap, Color, Gradient, Weight};
pub use input::cursor_names::{
    Crosshair, Default, Grab, Grabbing, Move, NotAllowed, Pointer, ResizeBottom, ResizeBottomLeft,
    ResizeBottomRight, ResizeHorizontal, ResizeLeft, ResizeRight, ResizeTop, ResizeTopLeft,
    ResizeTopRight, ResizeVertical, Text, Wait,
};
pub use input::{Button, Cursor, Key, Point, Scroll};
pub use ipc::{IpcCall, ipc_socket};
pub use layer_window::{
    Horizontal, InputArea, Keyboard, Layer, LayerWindow, Margin, Vertical, WindowSize, Zone,
};
pub use monitor::{Monitor, Monitors};
pub use placement::{Align, Center, End, Justify, Padding, Size, Start};
pub use style::{Fill, Image, Mask, Radius, Shadow};
pub use widgets::{
    Arc, Canvas, Circle, Column, Line, Path, Rectangle, Row, ScrollArea, Shape, Stack, Text,
    TextInput, Widget,
};

pub use placement::Justify::{SpaceAround, SpaceBetween, SpaceEvenly};
pub use placement::Size::Parent;
pub use toplevels::{FullscreenWindow, Toplevels};
pub use window::{Window, close_window, open_window, window_open, window_size};
