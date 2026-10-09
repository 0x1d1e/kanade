//! Kanade-owned desktop shell runtime.
//!
//! The crate is independent of Amane. Native Wayland input and presentation,
//! a GPU renderer and reactive Services will attach to these state machines.
//! Nothing here touches device nodes, reads global keyboard input, or renders
//! offscreen merely to poll for state.

#![forbid(unsafe_code)]

pub mod input;
pub mod window;

pub mod wayland;

pub mod effects;

pub mod gpu;

pub mod raster;

pub mod keymap;

pub mod reactive;

pub mod wake;

pub mod chrome;
