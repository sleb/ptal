//! ptal core: the data model, persistence, and review operations.
//!
//! Front ends (`cli` for agents, `tui` for humans) are thin layers over this
//! library.

pub mod cli;
pub mod error;
pub mod model;
pub mod review;
pub mod store;
pub mod tui;

pub use error::{Error, Result};
