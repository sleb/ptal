//! The human-facing terminal UI: a read-only file view with line comments.
//!
//! Depends on [`crate::review`] and [`crate::model`] only, never on `store`.
//! [`app`] is the pure state machine, [`mod@render`] the pure drawing; this
//! module is the driver that performs the effects `update` asks for.

pub mod app;
pub mod panel;
pub mod render;

use std::io;
use std::ops::ControlFlow;
use std::panic;
use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{self, Event as TermEvent};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::error::Result;
use crate::review::{Clock, Reviews, ThreadView, file_lines};

use app::{App, Effect, Event, update};
use render::render;

/// A file and its threads, read but not yet sized for a terminal.
///
/// Loading is split from [`Loaded::into_app`] so that a missing file is
/// reported before the terminal is queried or touched.
pub struct Loaded {
    lines: Vec<String>,
    threads: Vec<ThreadView>,
}

/// Read `file` and all its threads, resolved ones included.
///
/// # Errors
/// [`crate::Error::FileUnreadable`] plus any store error.
pub fn load<C: Clock>(reviews: &Reviews<C>, file: &Path) -> Result<Loaded> {
    let lines = file_lines(file)?;
    let threads = reviews.view(file, true)?;
    Ok(Loaded { lines, threads })
}

impl Loaded {
    /// A fresh [`App`] over this file on a terminal of `size` (cols, rows).
    pub fn into_app(self, file: &Path, author: &str, size: (u16, u16)) -> App {
        App::open(self.lines, &self.threads, author, size).labeled(file.display().to_string())
    }
}

/// Terminal (columns, rows), for sizing [`Loaded::into_app`] before the
/// session starts.
///
/// # Errors
/// The size could not be queried (there is no terminal).
pub fn terminal_size() -> io::Result<(u16, u16)> {
    ratatui::crossterm::terminal::size()
}

/// Put the terminal into TUI mode: raw input on the alternate screen.
fn enter_terminal() -> io::Result<()> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen)
}

/// Undo [`enter_terminal`]. Idempotent and infallible: it runs from `Drop`
/// and from the panic hook, where there is nobody to report to.
fn restore_terminal() {
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

/// Holds the terminal in TUI mode and puts it back on drop, whether the
/// scope ends normally, by `?`, or by unwinding.
///
/// `enter` and `restore` are injected so the guard is testable without a
/// terminal.
#[derive(Debug)]
pub struct TerminalSession<R: Fn()> {
    restore: R,
}

impl<R: Fn()> TerminalSession<R> {
    /// Run `enter`, and arrange for `restore` to run when the session ends.
    /// If `enter` fails part-way, `restore` runs before the error returns.
    ///
    /// # Errors
    /// Whatever `enter` returns.
    pub fn new(enter: impl FnOnce() -> io::Result<()>, restore: R) -> io::Result<Self> {
        match enter() {
            Ok(()) => Ok(Self { restore }),
            Err(error) => {
                restore();
                Err(error)
            }
        }
    }
}

impl<R: Fn()> Drop for TerminalSession<R> {
    fn drop(&mut self) {
        (self.restore)();
    }
}

/// Make panics restore the terminal before the message is printed, so it
/// lands on the normal screen. The previous hook still runs afterwards.
pub fn install_restore_hook(restore: impl Fn() + Send + Sync + 'static) {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
}

/// Run the interactive session on the real terminal until the user quits.
///
/// `app` comes from [`open`]. All of ptal's writes go through `reviews`.
///
/// # Errors
/// The terminal could not be set up, drawn to, or read from.
pub fn run<C: Clock>(
    mut app: App,
    reviews: &Reviews<C>,
    file: &Path,
    author: &str,
) -> io::Result<()> {
    install_restore_hook(restore_terminal);
    let _session = TerminalSession::new(enter_terminal, restore_terminal)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    loop {
        terminal.draw(|frame| render(&app, frame))?;
        let event = match event::read()? {
            TermEvent::Key(key) => Event::Key(key),
            TermEvent::Resize(width, height) => Event::Resize(width, height),
            _ => continue,
        };
        if step(&mut app, event, reviews, file, author).is_break() {
            return Ok(());
        }
    }
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
                    threads: reviews.view(file, true).unwrap_or_default(),
                },
                Err(error) => Event::SaveFailed(error.to_string()),
            };
            update(app, answer);
            ControlFlow::Continue(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io;

    #[test]
    fn dropping_a_session_restores_exactly_once() {
        let restored = Cell::new(0);

        {
            let _session =
                TerminalSession::new(|| Ok(()), || restored.set(restored.get() + 1)).unwrap();
            assert_eq!(restored.get(), 0);
        }

        assert_eq!(restored.get(), 1);
    }

    #[test]
    fn an_early_return_still_restores() {
        let restored = Cell::new(0);
        let run = || -> io::Result<()> {
            let _session = TerminalSession::new(|| Ok(()), || restored.set(restored.get() + 1))?;
            Err(io::Error::other("draw failed"))
        };

        assert!(run().is_err());

        assert_eq!(restored.get(), 1);
    }

    #[test]
    fn a_failed_enter_restores_whatever_was_half_done() {
        let restored = Cell::new(0);

        let session = TerminalSession::new(
            || Err(io::Error::other("no alternate screen")),
            || restored.set(restored.get() + 1),
        );

        assert!(session.is_err());
        assert_eq!(restored.get(), 1);
    }
}
