//! The human-facing terminal UI: a read-only file view with line comments and
//! a panel for reading the threads on a line.
//!
//! Depends on [`crate::review`] and [`crate::model`] (and the crate-wide
//! [`crate::error`]), never on `store`.
//! [`app`] is the pure state machine, [`draft`] the pure comment editor,
//! [`panel`] the pure thread layout, [`keys`] the help listing,
//! [`wrap`] the pure line wrapping, [`table`] the pure table layout,
//! [`highlight`] the pure markdown styling, [`search`] the pure text search,
//! [`mod@render`] the pure drawing; this module is the driver that performs
//! the effects `update` asks for.

pub mod app;
pub mod draft;
pub mod highlight;
pub mod keys;
pub mod panel;
pub mod render;
pub mod search;
pub mod table;
pub mod wrap;

use std::io::{self, Write};
use std::ops::ControlFlow;
use std::panic;
use std::path::Path;

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event as TermEvent,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::error::Result;
use crate::model::{Side, Thread};
use crate::review::{Clock, ReviewStatus, Reviews, View};

use app::{App, Done, Effect, Event, Reread, update};
use render::render;

/// A file, its threads, the review's state and its changes since the last
/// look, read but not yet sized for a terminal.
///
/// Loading is split from [`Loaded::into_app`] so that a missing file is
/// reported before the terminal is queried or touched.
pub struct Loaded {
    view: Reread,
}

/// Read `file`, all its threads (resolved ones included), the review's
/// state and what changed since the human's last look.
///
/// # Errors
/// [`crate::Error::FileUnreadable`] plus any store error.
pub fn load<C: Clock>(reviews: &Reviews<C>, file: &Path) -> Result<Loaded> {
    // One read: the lines shown are the ones the threads are mapped to.
    Ok(Loaded {
        view: reviews.view(file, true)?.into(),
    })
}

impl From<View> for Reread {
    fn from(view: View) -> Self {
        Reread {
            lines: view.lines,
            sha256: view.sha256,
            threads: view.threads,
            review: view.review,
            changes: view.changes,
        }
    }
}

impl Loaded {
    /// A fresh [`App`] over this file on a terminal of `size` (cols, rows),
    /// its markdown structure highlighted when `file` is markdown
    /// ([`highlight::is_markdown`]) and `color` is wanted.
    pub fn into_app(self, file: &Path, author: &str, size: (u16, u16), color: bool) -> App {
        App::open(Vec::new(), &[], author, size)
            .labeled(file.display().to_string())
            .highlighted(color && highlight::is_markdown(file))
            .with_reread(self.view)
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

/// Put the terminal into TUI mode: raw input on the alternate screen, with
/// bracketed paste so a paste arrives as one [`TermEvent::Paste`].
fn enter_terminal() -> io::Result<()> {
    enable_raw_mode()?;
    enter_screen(&mut io::stdout())
}

/// The escape sequences of [`enter_terminal`]: switch to the alternate
/// screen and turn bracketed paste on. Bracketed paste is best-effort: a
/// terminal without it still runs, and a paste there arrives as keys.
fn enter_screen(out: &mut impl Write) -> io::Result<()> {
    execute!(out, EnterAlternateScreen)?;
    let _ = execute!(out, EnableBracketedPaste);
    Ok(())
}

/// Undo [`enter_terminal`]. Idempotent and infallible: it runs from `Drop`
/// and from the panic hook, where there is nobody to report to.
fn restore_terminal() {
    leave_screen(&mut io::stdout());
    let _ = disable_raw_mode();
}

/// Undo [`enter_screen`], in reverse: bracketed paste off, then back to the
/// normal screen. Each step is tried even if the one before failed.
fn leave_screen(out: &mut impl Write) {
    let _ = execute!(out, DisableBracketedPaste);
    let _ = execute!(out, LeaveAlternateScreen);
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
/// `app` comes from [`load`] and [`Loaded::into_app`]. All of ptal's writes go through `reviews`.
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
            TermEvent::Paste(text) => Event::Paste(text),
            _ => continue,
        };
        if step(&mut app, event, reviews, file, author).is_break() {
            return Ok(());
        }
    }
}

/// Feed one event to `app` and perform whatever it asks for. This is the
/// only place effects happen: all writes go through `reviews`, then the
/// threads and the review's state are re-read so the screen shows what is
/// on disk. The TUI writes as the human side.
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
        Some(Effect::Add { start, end, body }) => {
            let written = reviews.add_thread(file, start, end, &body, author);
            answer(app, reviews, file, Done::Added, written)
        }
        Some(Effect::Reply { id, body }) => {
            let written = reviews.reply(file, &id, &body, author);
            answer(app, reviews, file, Done::Replied, written)
        }
        Some(Effect::Resolve { id }) => {
            let written = reviews.resolve(file, &id, author, None);
            answer(app, reviews, file, Done::Resolved, written)
        }
        Some(Effect::Reopen { id }) => {
            let written = reviews.reopen(file, &id, author, None);
            answer(app, reviews, file, Done::Reopened, written)
        }
        Some(Effect::HandOff { note }) => {
            let written = reviews
                .hand_off(file, Side::Human, author, note.as_deref())
                .map(|handed| {
                    let message = match handed.waiting.as_slice() {
                        [] => "handed off".to_string(),
                        waiting => format!("handed off · waiting on you: {}", waiting.join(", ")),
                    };
                    (message, handed.review)
                });
            answer_review(app, reviews, file, written)
        }
        Some(Effect::Close { verdict, note }) => {
            let written = reviews
                .close(file, author, verdict, note.as_deref())
                .map(|review| ("closed".to_string(), review));
            answer_review(app, reviews, file, written)
        }
        Some(Effect::ReopenReview) => {
            let written = reviews
                .reopen_review(file, Side::Human, author, None)
                .map(|review| ("reopened".to_string(), review));
            answer_review(app, reviews, file, written)
        }
    }
}

/// The threads, the review's state and the changes since the last look of
/// `file` as they are on disk, for the screen after a write.
///
/// The write is already stored when the re-read runs; if the re-read fails
/// the screen loses its markers rather than offering a retry that would
/// duplicate the comment, and keeps the `current` review state rather than
/// showing a fresh review's.
fn reread<C: Clock>(
    reviews: &Reviews<C>,
    file: &Path,
    app: &App,
    current: &ReviewStatus,
) -> Reread {
    match reviews.view(file, true) {
        Ok(view) => view.into(),
        Err(_) => Reread {
            lines: app.lines().to_vec(),
            sha256: app.seen().to_string(),
            review: current.clone(),
            ..Reread::default()
        },
    }
}

/// Turn the result of a write into the event that tells `app` about it,
/// with the view [`reread`].
fn answer<C: Clock>(
    app: &mut App,
    reviews: &Reviews<C>,
    file: &Path,
    done: Done,
    written: Result<Thread>,
) -> ControlFlow<()> {
    let event = match written {
        Ok(thread) => Event::Saved {
            id: thread.id,
            done,
            reread: reread(reviews, file, app, app.review()),
        },
        Err(error) => Event::SaveFailed(error.to_string()),
    };
    update(app, event);
    ControlFlow::Continue(())
}

/// Turn the result of a hand-off, close or reopen (the footer message and
/// the state the write left) into the event that tells `app` about it,
/// with the view [`reread`] as [`answer`] does; if the re-read fails, the
/// state is the write's own.
fn answer_review<C: Clock>(
    app: &mut App,
    reviews: &Reviews<C>,
    file: &Path,
    written: Result<(String, ReviewStatus)>,
) -> ControlFlow<()> {
    let event = match written {
        Ok((message, written)) => Event::ReviewSaved {
            message,
            reread: reread(reviews, file, app, &written),
        },
        Err(error) => Event::SaveFailed(error.to_string()),
    };
    update(app, event);
    ControlFlow::Continue(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::io;

    use ratatui::style::Modifier;

    /// Whether `path`, opened with `color`, shows its `# Title` bold.
    fn heading_is_bold(path: &str, color: bool) -> bool {
        let loaded = Loaded {
            view: Reread {
                lines: vec!["# Title".into()],
                ..Reread::default()
            },
        };
        let app = loaded.into_app(Path::new(path), "scott", (40, 5), color);
        let row = &app.screen_rows()[0].text;
        row.spans.iter().all(|span| {
            row.style
                .patch(span.style)
                .add_modifier
                .contains(Modifier::BOLD)
        })
    }

    #[test]
    fn a_markdown_file_is_highlighted_only_when_color_is_wanted() {
        assert!(heading_is_bold("plan.md", true));
        assert!(!heading_is_bold("plan.md", false));
        assert!(!heading_is_bold("plan.txt", true));
    }

    /// Where `needle` first occurs in `bytes`.
    fn find(bytes: &[u8], needle: &[u8]) -> Option<usize> {
        bytes.windows(needle.len()).position(|w| w == needle)
    }

    #[test]
    fn entering_the_screen_switches_to_the_alternate_screen_and_bracketed_paste() {
        let mut out = Vec::new();

        enter_screen(&mut out).unwrap();

        assert!(find(&out, b"\x1b[?1049h").is_some(), "{out:?}");
        assert!(find(&out, b"\x1b[?2004h").is_some(), "{out:?}");
    }

    /// A terminal that takes everything but the bracketed-paste sequences.
    #[derive(Default)]
    struct NoPaste(Vec<u8>);

    impl Write for NoPaste {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if find(buf, b"?2004").is_some() {
                return Err(io::Error::other("unsupported"));
            }
            self.0.extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_terminal_without_bracketed_paste_still_gets_the_alternate_screen() {
        let mut out = NoPaste::default();

        enter_screen(&mut out).unwrap();

        assert!(find(&out.0, b"\x1b[?1049h").is_some(), "{:?}", out.0);
    }

    #[test]
    fn leaving_the_screen_turns_bracketed_paste_off_before_the_alternate_screen() {
        let mut out = Vec::new();

        leave_screen(&mut out);

        let paste = find(&out, b"\x1b[?2004l").expect("bracketed paste off");
        let screen = find(&out, b"\x1b[?1049l").expect("alternate screen left");
        assert!(paste < screen, "{out:?}");
    }

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

    #[test]
    fn a_failed_re_read_keeps_the_review_state_and_the_text_it_had() {
        let tmp = tempfile::tempdir().unwrap();
        let reviews = Reviews::new(crate::store::Store::new(tmp.path().join("data")));
        let current = ReviewStatus {
            state: crate::model::ReviewState::Turn(Side::Agent),
            latest: None,
        };
        let app = App::open(vec!["one".into()], &[], "scott", (40, 5)).with_reread(Reread {
            lines: vec!["one".into()],
            sha256: "shown".into(),
            ..Reread::default()
        });

        let reread = reread(&reviews, &tmp.path().join("gone.md"), &app, &current);

        assert!(reread.threads.is_empty());
        assert_eq!(reread.review, current);
        assert_eq!(
            (reread.lines, reread.sha256.as_str()),
            (vec!["one".to_string()], "shown")
        );
    }
}
