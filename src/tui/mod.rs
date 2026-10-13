//! The human-facing terminal UI: a read-only file view with line comments.
//!
//! Depends on [`crate::review`] and [`crate::model`] only, never on `store`.

pub mod app;
pub mod render;
