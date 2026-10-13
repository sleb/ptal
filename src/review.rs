//! The application core: the only write path, built on [`Store`].
//!
//! No printing and no terminal awareness; front ends render what these
//! operations return.

use std::fs;
use std::path::Path;

use jiff::Timestamp;
use serde::Serialize;

use crate::error::{Error, Result};
use crate::model::{Anchor, Comment, State, Status, Thread};
use crate::store::Store;

/// Source of timestamps, injected so tests are deterministic.
pub trait Clock {
    /// The current time as an RFC 3339 UTC string.
    fn now(&self) -> String;
}

/// The wall clock, truncated to whole seconds.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> String {
        let now = Timestamp::now();
        Timestamp::from_second(now.as_second())
            .expect("a current timestamp is in range")
            .to_string()
    }
}

impl<F: Fn() -> String> Clock for F {
    fn now(&self) -> String {
        self()
    }
}

/// A thread together with its computed [`State`], as shown to front ends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadView {
    #[serde(flatten)]
    pub thread: Thread,
    pub state: State,
}

/// Review operations over a [`Store`].
#[derive(Debug, Clone)]
pub struct Reviews<C = SystemClock> {
    store: Store,
    clock: C,
}

impl Reviews<SystemClock> {
    /// Operations backed by `store`, stamped with the wall clock.
    pub fn new(store: Store) -> Self {
        Self::with_clock(store, SystemClock)
    }
}

impl<C: Clock> Reviews<C> {
    /// Operations backed by `store`, stamped by `clock`.
    pub fn with_clock(store: Store, clock: C) -> Self {
        Self { store, clock }
    }

    /// Start a thread on lines `start..=end` (1-based) of `file`.
    ///
    /// The anchor's quote is read from the file as it is now.
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::EmptyBody`], [`Error::FileUnreadable`],
    /// [`Error::InvalidRange`], plus any store error. Nothing is written on
    /// error.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "# Plan\nstep one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// let thread = reviews.add_thread(&file, 2, 2, "Why this step?", "scott")?;
    /// assert_eq!(thread.id, "c1");
    /// assert_eq!(thread.anchor.quote, "step one");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn add_thread(
        &self,
        file: &Path,
        start: u32,
        end: u32,
        body: &str,
        author: &str,
    ) -> Result<Thread> {
        require_author(author)?;
        if body.trim().is_empty() {
            return Err(Error::EmptyBody);
        }
        let lines = file_lines(file)?;
        let quote =
            quote_of(&lines, start, end).ok_or_else(|| invalid_range(&lines, start, end))?;
        let created_at = self.clock.now();

        self.store.with_review(file, |review| {
            let id = review.next_comment_id();
            let thread = Thread {
                id: id.clone(),
                anchor: Anchor {
                    start_line: start,
                    end_line: end,
                    quote,
                },
                status: Status::Open,
                comments: vec![Comment {
                    id,
                    author: author.to_string(),
                    body: body.to_string(),
                    created_at,
                }],
            };
            review.threads.push(thread.clone());
            Ok(thread)
        })
    }

    /// The threads of `file` in id order, each with its [`State`].
    ///
    /// Resolved threads are omitted unless `include_resolved`.
    ///
    /// # Errors
    /// [`Error::FileUnreadable`] plus any store error.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::State, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\ntwo\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// reviews.add_thread(&file, 1, 1, "Hmm", "scott")?;
    /// std::fs::write(&file, "ONE\ntwo\n")?;
    ///
    /// let views = reviews.view(&file, false)?;
    /// assert_eq!(views[0].state, State::Outdated);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn view(&self, file: &Path, include_resolved: bool) -> Result<Vec<ThreadView>> {
        let lines = file_lines(file)?;
        let review = self.store.load(file)?;
        Ok(review
            .threads
            .into_iter()
            .filter(|t| include_resolved || t.status == Status::Open)
            .map(|thread| {
                let current = quote_of(&lines, thread.anchor.start_line, thread.anchor.end_line)
                    .is_some_and(|q| q == thread.anchor.quote);
                let state = if current {
                    State::Current
                } else {
                    State::Outdated
                };
                ThreadView { thread, state }
            })
            .collect())
    }

    /// Resolve thread `id`, optionally appending `note` as a comment by
    /// `author`.
    ///
    /// Resolving an already-resolved thread succeeds and changes nothing; the
    /// new note is ignored. A blank note adds no comment.
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::NotFound`] if no thread has this id,
    /// plus any store error. Nothing is written on error.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::Status, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// reviews.add_thread(&file, 1, 1, "Typo?", "scott")?;
    ///
    /// let thread = reviews.resolve(&file, "c1", "claude", Some("Fixed"))?;
    /// assert!(matches!(thread.status, Status::Resolved { .. }));
    /// assert_eq!(thread.comments.len(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn resolve(
        &self,
        file: &Path,
        id: &str,
        author: &str,
        note: Option<&str>,
    ) -> Result<Thread> {
        require_author(author)?;
        let note = note.filter(|n| !n.trim().is_empty());
        let now = self.clock.now();

        self.store.with_review(file, |review| {
            let index = review
                .threads
                .iter()
                .position(|t| t.id == id)
                .ok_or_else(|| Error::NotFound(id.to_string()))?;
            if review.threads[index].status != Status::Open {
                return Ok(review.threads[index].clone());
            }
            let note_comment = note.map(|body| Comment {
                id: review.next_comment_id(),
                author: author.to_string(),
                body: body.to_string(),
                created_at: now.clone(),
            });
            let thread = &mut review.threads[index];
            thread.status = Status::Resolved {
                resolved_by: author.to_string(),
                resolved_at: now,
            };
            thread.comments.extend(note_comment);
            Ok(thread.clone())
        })
    }
}

fn require_author(author: &str) -> Result<()> {
    if author.trim().is_empty() {
        Err(Error::EmptyAuthor)
    } else {
        Ok(())
    }
}

/// The lines of `file`, split the way the core splits them everywhere.
///
/// Splitting is [`str::lines`]: `\n` or `\r\n`, ignoring a final trailing
/// newline. `add_thread` quotes from, and `view` checks against, this same
/// list, so a front end that displays it shows exactly what gets anchored.
///
/// # Errors
/// [`Error::FileUnreadable`] if the file is missing, unreadable, or not
/// UTF-8.
///
/// # Examples
/// ```
/// # use ptal::review::file_lines;
/// # let dir = tempfile::tempdir()?;
/// # let file = dir.path().join("plan.md");
/// std::fs::write(&file, "# Plan\r\nstep one\n")?;
/// assert_eq!(file_lines(&file)?, ["# Plan", "step one"]);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub fn file_lines(file: &Path) -> Result<Vec<String>> {
    let text = fs::read_to_string(file).map_err(|source| Error::FileUnreadable {
        path: file.to_path_buf(),
        source,
    })?;
    Ok(text.lines().map(String::from).collect())
}

/// Lines `start..=end` (1-based) of `lines` joined by `\n`, or `None` if the
/// range is empty, starts before line 1, or runs past the end.
fn quote_of(lines: &[String], start: u32, end: u32) -> Option<String> {
    if start == 0 || start > end {
        return None;
    }
    let range = lines.get(start as usize - 1..end as usize)?;
    Some(range.join("\n"))
}

fn invalid_range(lines: &[String], start: u32, end: u32) -> Error {
    let reason = if start == 0 {
        "lines are numbered from 1"
    } else if start > end {
        "start is after end"
    } else {
        debug_assert!(lines.len() < end as usize);
        "range extends past the end of the file"
    };
    Error::InvalidRange { start, end, reason }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;
    use crate::model::{Anchor, Comment, State, Status};
    use std::fs;
    use std::path::PathBuf;

    const NOW: &str = "2026-10-12T10:00:00Z";

    struct Fixture {
        _tmp: tempfile::TempDir,
        file: PathBuf,
        store: Store,
        reviews: Reviews<fn() -> String>,
    }

    fn fixed_now() -> String {
        NOW.to_string()
    }

    fn fixture() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("plan.md");
        fs::write(&file, "one\ntwo\nthree\nfour\n").unwrap();
        let store = Store::new(tmp.path().join("data"));
        let reviews = Reviews::with_clock(store.clone(), fixed_now as fn() -> String);
        Fixture {
            _tmp: tmp,
            file,
            store,
            reviews,
        }
    }

    impl Fixture {
        fn review_json(&self) -> Option<Vec<u8>> {
            let dir = self.store.review_dir(&self.file).unwrap();
            fs::read(dir.join("review.json")).ok()
        }

        fn ids(&self, include_resolved: bool) -> Vec<String> {
            self.reviews
                .view(&self.file, include_resolved)
                .unwrap()
                .into_iter()
                .map(|v| v.thread.id)
                .collect()
        }
    }

    // --- file_lines ---

    #[test]
    fn file_lines_splits_like_str_lines() {
        let fx = fixture();
        fs::write(&fx.file, "one\r\ntwo\nthree").unwrap();

        assert_eq!(file_lines(&fx.file).unwrap(), ["one", "two", "three"]);
    }

    #[test]
    fn file_lines_of_a_missing_or_non_utf8_file_is_unreadable() {
        let fx = fixture();
        let binary = fx.file.with_file_name("bin.md");
        fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();

        for path in [fx.file.with_file_name("nope.md"), binary] {
            assert!(matches!(
                file_lines(&path),
                Err(Error::FileUnreadable { .. })
            ));
        }
    }

    #[test]
    fn add_thread_quote_is_the_file_lines_entry() {
        let fx = fixture();
        fs::write(&fx.file, "a\r\nb\r\n\r\nd").unwrap();
        let lines = file_lines(&fx.file).unwrap();

        for n in 1..=lines.len() as u32 {
            let thread = fx.reviews.add_thread(&fx.file, n, n, "x", "s").unwrap();
            assert_eq!(thread.anchor.quote, lines[n as usize - 1]);
        }
    }

    // --- add_thread ---

    #[test]
    fn add_thread_stores_quote_id_and_open_status_and_reloads_identical() {
        let fx = fixture();

        let thread = fx
            .reviews
            .add_thread(&fx.file, 2, 3, "Why?", "scott")
            .unwrap();

        assert_eq!(thread.id, "c1");
        assert_eq!(
            thread.anchor,
            Anchor {
                start_line: 2,
                end_line: 3,
                quote: "two\nthree".into()
            }
        );
        assert_eq!(thread.status, Status::Open);
        assert_eq!(
            thread.comments,
            vec![Comment {
                id: "c1".into(),
                author: "scott".into(),
                body: "Why?".into(),
                created_at: NOW.into(),
            }]
        );
        assert_eq!(fx.store.load(&fx.file).unwrap().threads, vec![thread]);
    }

    #[test]
    fn successive_threads_get_successive_ids() {
        let fx = fixture();

        let a = fx.reviews.add_thread(&fx.file, 1, 1, "a", "s").unwrap();
        let b = fx.reviews.add_thread(&fx.file, 2, 2, "b", "s").unwrap();

        assert_eq!((a.id.as_str(), b.id.as_str()), ("c1", "c2"));
    }

    #[test]
    fn invalid_ranges_are_rejected_and_nothing_is_written() {
        let fx = fixture();

        for (start, end) in [(3, 2), (0, 1), (1, 5), (5, 5), (0, 0)] {
            let result = fx.reviews.add_thread(&fx.file, start, end, "x", "s");
            assert!(
                matches!(result, Err(Error::InvalidRange { .. })),
                "{start}-{end}: {result:?}"
            );
        }
        assert_eq!(fx.review_json(), None);
    }

    #[test]
    fn missing_or_non_utf8_file_is_unreadable() {
        let fx = fixture();
        let missing = fx.file.with_file_name("nope.md");
        let binary = fx.file.with_file_name("bin.md");
        fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();

        for path in [&missing, &binary] {
            let result = fx.reviews.add_thread(path, 1, 1, "x", "s");
            assert!(matches!(result, Err(Error::FileUnreadable { .. })));
        }
    }

    #[test]
    fn empty_author_or_body_is_rejected() {
        let fx = fixture();

        assert!(matches!(
            fx.reviews.add_thread(&fx.file, 1, 1, "x", "  "),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            fx.reviews.add_thread(&fx.file, 1, 1, "", "s"),
            Err(Error::EmptyBody)
        ));
        assert_eq!(fx.review_json(), None);
    }

    #[test]
    fn crlf_line_endings_do_not_leak_into_the_quote() {
        let fx = fixture();
        fs::write(&fx.file, "one\r\ntwo\r\n").unwrap();

        let thread = fx.reviews.add_thread(&fx.file, 1, 2, "x", "s").unwrap();

        assert_eq!(thread.anchor.quote, "one\ntwo");
    }

    // --- view ---

    #[test]
    fn view_lists_open_threads_in_id_order_and_resolved_on_request() {
        let fx = fixture();
        for line in 1..=3 {
            fx.reviews
                .add_thread(&fx.file, line, line, "x", "s")
                .unwrap();
        }
        fx.reviews.resolve(&fx.file, "c2", "claude", None).unwrap();

        assert_eq!(fx.ids(false), ["c1", "c3"]);
        assert_eq!(fx.ids(true), ["c1", "c2", "c3"]);
    }

    fn state_of(fx: &Fixture, id: &str) -> State {
        fx.reviews
            .view(&fx.file, true)
            .unwrap()
            .into_iter()
            .find(|v| v.thread.id == id)
            .unwrap()
            .state
    }

    #[test]
    fn fresh_thread_is_current() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 3, "x", "s").unwrap();

        assert_eq!(state_of(&fx, "c1"), State::Current);
    }

    #[test]
    fn editing_a_quoted_line_makes_the_thread_outdated() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 3, "x", "s").unwrap();

        fs::write(&fx.file, "one\nTWO\nthree\nfour\n").unwrap();

        assert_eq!(state_of(&fx, "c1"), State::Outdated);
    }

    #[test]
    fn editing_an_unrelated_line_keeps_the_thread_current() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 3, "x", "s").unwrap();

        fs::write(&fx.file, "ONE\ntwo\nthree\nFOUR\n").unwrap();

        assert_eq!(state_of(&fx, "c1"), State::Current);
    }

    #[test]
    fn truncating_below_the_range_makes_the_thread_outdated() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 3, 4, "x", "s").unwrap();

        fs::write(&fx.file, "one\ntwo\n").unwrap();

        assert_eq!(state_of(&fx, "c1"), State::Outdated);
    }

    #[test]
    fn inserting_lines_above_the_range_makes_the_thread_outdated() {
        // MVP does not map anchors between versions of the file; this pins
        // that behavior until revisions land.
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 3, "x", "s").unwrap();

        fs::write(&fx.file, "zero\none\ntwo\nthree\nfour\n").unwrap();

        assert_eq!(state_of(&fx, "c1"), State::Outdated);
    }

    #[test]
    fn view_of_a_review_with_no_threads_is_empty() {
        let fx = fixture();

        assert!(fx.reviews.view(&fx.file, true).unwrap().is_empty());
        assert_eq!(fx.review_json(), None);
    }

    // --- resolve ---

    #[test]
    fn resolve_flips_status_and_records_resolver() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();

        let thread = fx.reviews.resolve(&fx.file, "c1", "claude", None).unwrap();

        assert_eq!(
            thread.status,
            Status::Resolved {
                resolved_by: "claude".into(),
                resolved_at: NOW.into()
            }
        );
        assert_eq!(thread.comments.len(), 1);
        assert_eq!(fx.store.load(&fx.file).unwrap().threads, vec![thread]);
    }

    #[test]
    fn resolve_note_becomes_second_comment_by_the_resolver() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();

        let thread = fx
            .reviews
            .resolve(&fx.file, "c1", "claude", Some("Done"))
            .unwrap();

        assert_eq!(
            thread.comments[1],
            Comment {
                id: "c2".into(),
                author: "claude".into(),
                body: "Done".into(),
                created_at: NOW.into(),
            }
        );
        let next = fx.reviews.add_thread(&fx.file, 2, 2, "y", "scott").unwrap();
        assert_eq!(next.id, "c3");
    }

    #[test]
    fn blank_note_adds_no_comment() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();

        let thread = fx
            .reviews
            .resolve(&fx.file, "c1", "claude", Some("  "))
            .unwrap();

        assert_eq!(thread.comments.len(), 1);
    }

    #[test]
    fn repeat_resolve_is_a_no_op_success() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        let first = fx
            .reviews
            .resolve(&fx.file, "c1", "claude", Some("Done"))
            .unwrap();
        let bytes = fx.review_json();

        let second = fx
            .reviews
            .resolve(&fx.file, "c1", "someone-else", Some("Again"))
            .unwrap();

        assert_eq!(second, first);
        assert_eq!(fx.review_json(), bytes);
    }

    #[test]
    fn unknown_id_is_not_found_and_changes_nothing() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        let bytes = fx.review_json();

        let result = fx.reviews.resolve(&fx.file, "c99", "claude", Some("n"));

        assert!(matches!(result, Err(Error::NotFound(id)) if id == "c99"));
        assert_eq!(fx.review_json(), bytes);
    }

    #[test]
    fn resolve_requires_an_author() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();

        let result = fx.reviews.resolve(&fx.file, "c1", "", None);

        assert!(matches!(result, Err(Error::EmptyAuthor)));
    }
}
