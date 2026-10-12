//! ptal core: the data model, persistence, and review operations.
//!
//! Front ends (`cli`, `tui`) are thin layers over this library and arrive in
//! later stories.

pub mod error;
pub mod model;
pub mod store;

pub use error::{Error, Result};
