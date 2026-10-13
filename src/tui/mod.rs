//! The human-facing terminal UI: a read-only file view with line comments.
//!
//! Depends on [`crate::review`] and [`crate::model`] only, never on `store`.
//! [`app`] is the pure state machine, [`render`] the pure drawing; this
//! module is the driver that performs the effects `update` asks for.

pub mod app;
pub mod render;

use std::ops::ControlFlow;
use std::path::Path;

use crate::error::Result;
use crate::review::{Clock, Reviews, file_lines};

use app::{App, Effect, Event, update};

/// Load `file` and its open threads into a fresh [`App`] on a terminal
/// `rows` tall.
///
/// # Errors
/// [`crate::Error::FileUnreadable`] plus any store error.
pub fn open<C: Clock>(reviews: &Reviews<C>, file: &Path, author: &str, rows: u16) -> Result<App> {
    let lines = file_lines(file)?;
    let threads = reviews.view(file, false)?;
    Ok(App::open(lines, &threads, rows).labeled(file.display().to_string(), author))
}

/// Feed one event to `app` and perform whatever it asks for. This is the
/// only place effects happen: all writes go through `reviews`, then the
/// threads are re-read so the screen shows what is on disk.
///
/// Returns [`ControlFlow::Break`] when the user quit.
pub fn step<C: Clock>(
    app: &mut App,
    event: Event,
    reviews: &Reviews<C>,
    file: &Path,
    author: &str,
) -> ControlFlow<()> {
    match update(app, event) {
        None => ControlFlow::Continue(()),
        Some(Effect::Quit) => ControlFlow::Break(()),
        Some(Effect::Save { line, body }) => {
            let answer = match reviews.add_thread(file, line, line, &body, author) {
                Ok(thread) => Event::Saved {
                    id: thread.id,
                    // The comment is already stored; if the re-read fails
                    // the screen loses its markers rather than offering a
                    // retry that would duplicate the comment.
                    threads: reviews.view(file, false).unwrap_or_default(),
                },
                Err(error) => Event::SaveFailed(error.to_string()),
            };
            update(app, answer);
            ControlFlow::Continue(())
        }
    }
}
