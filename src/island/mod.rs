//! The island core. Pure and Amane-free except `service`, so it unit-tests with plain `cargo test`.
//! The boundary is enforced structurally by `crate::boundary` (test-only).

pub mod activity;
pub mod arbiter;
pub mod command;
pub mod fade;
pub mod geometry;
pub mod motion;
pub mod presentation;
pub mod service;
