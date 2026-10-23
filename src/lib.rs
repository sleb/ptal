//! ptal reviews one throwaway text file, usually a `.md` plan, with a human in
//! a terminal UI and an agent on a JSON CLI. The screen follows the agent's
//! edits, comments and resolved state persist across rounds and revisions, and
//! the human closes the review as approved or dropped.
//!
//! ptal is a binary-first tool. This library is the core the binary and its
//! tests share — the data model, persistence, and review operations — and its
//! API is internal and unstable: modules and types may change without notice,
//! and nothing here is a supported interface for other crates. Use the `ptal`
//! binary; see the
//! [README](https://github.com/sleb/ptal/blob/main/README.md) for usage.

pub mod anchor;
pub mod cli;
pub mod error;
pub mod model;
pub mod review;
pub mod store;
pub mod tui;

pub use error::{Error, Result};
