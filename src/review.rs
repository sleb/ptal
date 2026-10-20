//! The application core: the only write path, built on [`Store`].
//!
//! No printing and no terminal awareness; front ends render what these
//! operations return.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use jiff::Timestamp;
use serde::Serialize;

use crate::anchor::{LineRange, map_range};
use crate::error::{Error, Result};
use crate::model::{Anchor, Comment, Review, Revision, State, Status, Thread};
use crate::store::{Snapshots, Store, content_hash};

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
///
/// The anchor's lines are mapped to the current file; its `revision` and
/// `quote` are as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ThreadView {
    #[serde(flatten)]
    pub thread: Thread,
    pub state: State,
}

/// The threads of a file as it is now, from one read of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct View {
    /// The current content's revision id, or `None` if no write has been
    /// made against this content yet.
    pub revision: Option<String>,
    pub threads: Vec<ThreadView>,
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
    /// The anchor's quote is read from the file as it is now, and the
    /// thread is anchored to that content's revision (snapshotted if new).
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
        let content = Content::read(file)?;
        let lines = content.lines();
        let quote =
            quote_of(&lines, start, end).ok_or_else(|| invalid_range(&lines, start, end))?;
        let created_at = self.clock.now();

        self.store.with_review(file, |review, snapshots| {
            let revision = record_revision(review, snapshots, &content);
            let id = review.next_comment_id();
            let thread = Thread {
                id: id.clone(),
                anchor: Anchor {
                    revision: Some(revision.clone()),
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
                    revision: Some(revision),
                }],
            };
            review.threads.push(thread.clone());
            Ok(thread)
        })
    }

    /// The threads of `file` in id order, each mapped to the file as it is
    /// now, with its [`State`].
    ///
    /// Each anchor is mapped from its own revision's snapshot (see
    /// [`map_range`]): it is `current` only if the anchored lines are
    /// unchanged and still read as its `quote`. Otherwise it is `outdated`
    /// at a best-guess range within the file. A thread from before revisions
    /// existed is checked in place, and one whose snapshot is missing is
    /// `outdated` at its stored range, clamped to the file. Resolved threads
    /// are omitted unless `include_resolved`. Never writes.
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
    /// let view = reviews.view(&file, false)?;
    /// assert_eq!(view.threads[0].state, State::Outdated);
    /// assert_eq!(view.revision, None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn view(&self, file: &Path, include_resolved: bool) -> Result<View> {
        let content = Content::read(file)?;
        let Review {
            revisions, threads, ..
        } = self.store.load(file)?;
        let mut mapper = Mapper::new(&self.store, file, &revisions, &content);
        let threads = threads
            .into_iter()
            .filter(|t| include_resolved || t.status == Status::Open)
            .map(|mut thread| {
                let state = mapper.map(&mut thread.anchor)?;
                Ok(ThreadView { thread, state })
            })
            .collect::<Result<_>>()?;
        let revision = revisions
            .into_iter()
            .find(|r| r.sha256 == content.sha256)
            .map(|r| r.id);
        Ok(View { revision, threads })
    }

    /// Resolve thread `id`, optionally appending `note` as a comment by
    /// `author`.
    ///
    /// A blank note adds no comment. The resolve (`resolved_revision`) and
    /// the note record the revision of the file as it is now.
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::FileUnreadable`], [`Error::NotFound`]
    /// if no thread has this id, [`Error::AlreadyResolved`] if the thread is
    /// not open, plus any store error, checked in that order. Nothing is written on error, so a note
    /// on a resolved thread is dropped, not appended.
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
        let content = Content::read(file)?;
        let now = self.clock.now();

        self.store.with_review(file, |review, snapshots| {
            let index = find_thread(review, id)?;
            if review.threads[index].status != Status::Open {
                return Err(Error::AlreadyResolved(id.to_string()));
            }
            let revision = record_revision(review, snapshots, &content);
            let note_comment = note.map(|body| new_comment(review, author, body, &now, &revision));
            let thread = &mut review.threads[index];
            thread.status = Status::Resolved {
                resolved_by: author.to_string(),
                resolved_at: now,
                resolved_revision: Some(revision),
            };
            thread.comments.extend(note_comment);
            Ok(thread.clone())
        })
    }

    /// Reopen resolved thread `id`, optionally appending `note` as a comment
    /// by `author`.
    ///
    /// The resolution record is dropped. A blank note adds no comment; a
    /// note records the revision of the file as it is now (a reopen itself
    /// is not recorded, so one without a note records none).
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::FileUnreadable`], [`Error::NotFound`]
    /// if no thread has this id, [`Error::AlreadyOpen`] if the thread is not
    /// resolved, plus any store error, checked in that order. Nothing is written on error, so a note
    /// on an open thread is dropped, not appended.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::Status, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// reviews.add_thread(&file, 1, 1, "Typo?", "scott")?;
    /// reviews.resolve(&file, "c1", "claude", None)?;
    ///
    /// let thread = reviews.reopen(&file, "c1", "scott", Some("Not fixed"))?;
    /// assert_eq!(thread.status, Status::Open);
    /// assert_eq!(thread.comments.len(), 2);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reopen(
        &self,
        file: &Path,
        id: &str,
        author: &str,
        note: Option<&str>,
    ) -> Result<Thread> {
        require_author(author)?;
        let note = note.filter(|n| !n.trim().is_empty());
        let content = Content::read(file)?;
        let now = self.clock.now();

        self.store.with_review(file, |review, snapshots| {
            let index = find_thread(review, id)?;
            if review.threads[index].status == Status::Open {
                return Err(Error::AlreadyOpen(id.to_string()));
            }
            let revision = record_revision(review, snapshots, &content);
            let note_comment = note.map(|body| new_comment(review, author, body, &now, &revision));
            let thread = &mut review.threads[index];
            thread.status = Status::Open;
            thread.comments.extend(note_comment);
            Ok(thread.clone())
        })
    }

    /// Append a comment by `author` to thread `id`.
    ///
    /// Replying to a resolved thread reopens it in the same write; no
    /// separate "reopened" comment is added. The body is stored verbatim,
    /// and the comment records the revision of the file as it is now.
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::EmptyBody`], [`Error::FileUnreadable`],
    /// [`Error::NotFound`] if no thread has this id, plus any store error,
    /// checked in that order.
    /// Nothing is written on error.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::Status, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// reviews.add_thread(&file, 1, 1, "Typo?", "scott")?;
    /// reviews.resolve(&file, "c1", "claude", None)?;
    ///
    /// let thread = reviews.reply(&file, "c1", "Still wrong", "scott")?;
    /// assert_eq!(thread.status, Status::Open);
    /// assert_eq!(thread.comments[1].id, "c2");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reply(&self, file: &Path, id: &str, body: &str, author: &str) -> Result<Thread> {
        require_author(author)?;
        if body.trim().is_empty() {
            return Err(Error::EmptyBody);
        }
        let content = Content::read(file)?;
        let created_at = self.clock.now();

        self.store.with_review(file, |review, snapshots| {
            let index = find_thread(review, id)?;
            let revision = record_revision(review, snapshots, &content);
            let comment = new_comment(review, author, body, &created_at, &revision);
            let thread = &mut review.threads[index];
            thread.comments.push(comment);
            thread.status = Status::Open;
            Ok(thread.clone())
        })
    }
}

/// A comment with the next free id, written against `revision`. The id
/// counter advances, so call this only once the operation can no longer fail.
fn new_comment(
    review: &mut Review,
    author: &str,
    body: &str,
    created_at: &str,
    revision: &str,
) -> Comment {
    Comment {
        id: review.next_comment_id(),
        author: author.to_string(),
        body: body.to_string(),
        created_at: created_at.to_string(),
        revision: Some(revision.to_string()),
    }
}

/// The file as read once for an operation: its text, and the hash that
/// names its revision. The quote and the snapshot come from the same bytes.
struct Content {
    text: String,
    sha256: String,
}

impl Content {
    /// # Errors
    /// [`Error::FileUnreadable`] if the file is missing, unreadable, or not
    /// UTF-8.
    fn read(file: &Path) -> Result<Self> {
        let text = fs::read_to_string(file).map_err(|source| Error::FileUnreadable {
            path: file.to_path_buf(),
            source,
        })?;
        let sha256 = content_hash(text.as_bytes());
        Ok(Self { text, sha256 })
    }

    fn lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

/// Maps anchors from their own revision to one current content, loading
/// each snapshot at most once.
struct Mapper<'a> {
    store: &'a Store,
    file: &'a Path,
    revisions: &'a [Revision],
    current: &'a Content,
    /// Snapshot text by revision id; `None` if it is missing.
    snapshots: HashMap<String, Option<String>>,
}

impl<'a> Mapper<'a> {
    fn new(
        store: &'a Store,
        file: &'a Path,
        revisions: &'a [Revision],
        current: &'a Content,
    ) -> Self {
        Self {
            store,
            file,
            revisions,
            current,
            snapshots: HashMap::new(),
        }
    }

    /// Rewrite `anchor`'s lines to the current file and say whether it is
    /// still current. Its `revision` and `quote` are left as stored.
    ///
    /// # Errors
    /// [`Error::Io`] if a snapshot exists but cannot be read.
    fn map(&mut self, anchor: &mut Anchor) -> Result<State> {
        let current = self.current.lines();
        let stored = LineRange {
            start: anchor.start_line,
            end: anchor.end_line,
        };
        let mapped = match self.old_text(anchor.revision.as_deref())? {
            Some(old) => map_range(&old.lines().collect::<Vec<_>>(), &current, stored),
            None => {
                // Snapshot missing: in place, never current.
                let mut clamped = map_range(&current, &current, stored);
                clamped.state = State::Outdated;
                clamped
            }
        };
        anchor.start_line = mapped.range.start;
        anchor.end_line = mapped.range.end;
        let reads_as_quote = quote_of(&current, mapped.range.start, mapped.range.end)
            .is_some_and(|q| q == anchor.quote);
        Ok(if mapped.state == State::Current && reads_as_quote {
            State::Current
        } else {
            State::Outdated
        })
    }

    /// The text `revision` refers to, or `None` if its snapshot is missing.
    /// An anchor from before revisions (`None`) refers to the current text.
    fn old_text(&mut self, revision: Option<&str>) -> Result<Option<&str>> {
        let Some(id) = revision else {
            return Ok(Some(&self.current.text));
        };
        let sha = self
            .revisions
            .iter()
            .find(|r| r.id == id)
            .map(|r| &r.sha256);
        if sha == Some(&self.current.sha256) {
            return Ok(Some(&self.current.text));
        }
        if !self.snapshots.contains_key(id) {
            let text = match sha {
                Some(sha) => self
                    .store
                    .load_revision(self.file, sha)?
                    .and_then(|bytes| String::from_utf8(bytes).ok()),
                None => None,
            };
            self.snapshots.insert(id.to_string(), text);
        }
        Ok(self.snapshots[id].as_deref())
    }
}

/// Record that a write is made against `content`: keep its snapshot and
/// return its revision id, assigning one if it is new.
///
/// Threads anchored before revisions existed (`anchor.revision` is `None`)
/// are anchored to this revision: the file as first seen after the upgrade.
fn record_revision(review: &mut Review, snapshots: &mut Snapshots, content: &Content) -> String {
    snapshots.keep(content.text.as_bytes());
    let revision = review.revision_for(&content.sha256);
    for thread in &mut review.threads {
        thread
            .anchor
            .revision
            .get_or_insert_with(|| revision.clone());
    }
    revision
}

/// Index of thread `id` in `review`.
fn find_thread(review: &Review, id: &str) -> Result<usize> {
    review
        .threads
        .iter()
        .position(|t| t.id == id)
        .ok_or_else(|| Error::NotFound(id.to_string()))
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
    Ok(Content::read(file)?
        .text
        .lines()
        .map(String::from)
        .collect())
}

/// Lines `start..=end` (1-based) of `lines` joined by `\n`, or `None` if the
/// range is empty, starts before line 1, or runs past the end.
fn quote_of(lines: &[&str], start: u32, end: u32) -> Option<String> {
    if start == 0 || start > end {
        return None;
    }
    let range = lines.get(start as usize - 1..end as usize)?;
    Some(range.join("\n"))
}

fn invalid_range(lines: &[&str], start: u32, end: u32) -> Error {
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
    use crate::store::content_hash;
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
                .threads
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
                revision: Some("r1".into()),
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
                revision: Some("r1".into()),
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
            .threads
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

    fn view_of(fx: &Fixture, id: &str) -> ThreadView {
        fx.reviews
            .view(&fx.file, true)
            .unwrap()
            .threads
            .into_iter()
            .find(|v| v.thread.id == id)
            .unwrap()
    }

    fn lines_of(view: &ThreadView) -> (u32, u32, State) {
        let a = &view.thread.anchor;
        (a.start_line, a.end_line, view.state)
    }

    #[test]
    fn lines_inserted_or_deleted_above_move_the_thread_and_keep_it_current() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 3, "x", "s").unwrap();

        fs::write(&fx.file, "zero\none\ntwo\nthree\nfour\n").unwrap();
        assert_eq!(lines_of(&view_of(&fx, "c1")), (3, 4, State::Current));

        fs::write(&fx.file, "two\nthree\nfour\n").unwrap();
        assert_eq!(lines_of(&view_of(&fx, "c1")), (1, 2, State::Current));
    }

    #[test]
    fn an_edited_anchored_line_is_outdated_at_the_best_guess_with_its_quote() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 3, "x", "s").unwrap();

        fs::write(&fx.file, "zero\none\nTWO\nthree\nfour\n").unwrap();

        let view = view_of(&fx, "c1");
        assert_eq!(lines_of(&view), (3, 4, State::Outdated));
        assert_eq!(view.thread.anchor.quote, "two\nthree");
        assert_eq!(view.thread.anchor.revision.as_deref(), Some("r1"));
    }

    #[test]
    fn removed_anchored_lines_are_outdated_and_in_bounds() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 3, 4, "x", "s").unwrap();

        fs::write(&fx.file, "one\n").unwrap();

        assert_eq!(lines_of(&view_of(&fx, "c1")), (1, 1, State::Outdated));
    }

    #[test]
    fn an_edit_that_is_reverted_maps_back_to_current() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 2, "x", "s").unwrap();
        fs::write(&fx.file, "one\nTWO\nthree\nfour\n").unwrap();
        assert_eq!(view_of(&fx, "c1").state, State::Outdated);

        fs::write(&fx.file, "one\ntwo\nthree\nfour\n").unwrap();

        assert_eq!(lines_of(&view_of(&fx, "c1")), (2, 2, State::Current));
    }

    #[test]
    fn threads_on_different_revisions_each_map_from_their_own() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "s").unwrap();
        fs::write(&fx.file, "zero\none\ntwo\nthree\nfour\n").unwrap();
        fx.reviews.add_thread(&fx.file, 1, 1, "y", "s").unwrap();

        fs::write(&fx.file, "new\nzero\none\ntwo\nthree\nfour\n").unwrap();

        assert_eq!(lines_of(&view_of(&fx, "c1")), (3, 3, State::Current));
        assert_eq!(lines_of(&view_of(&fx, "c2")), (2, 2, State::Current));
    }

    #[test]
    fn a_legacy_anchor_before_any_write_is_checked_in_place() {
        let fx = fixture();
        write_legacy_review(&fx);
        assert_eq!(lines_of(&view_of(&fx, "c1")), (2, 2, State::Current));

        fs::write(&fx.file, "zero\none\ntwo\nthree\nfour\n").unwrap();

        assert_eq!(lines_of(&view_of(&fx, "c1")), (2, 2, State::Outdated));
    }

    #[test]
    fn an_adopted_legacy_anchor_whose_quote_no_longer_matched_stays_outdated() {
        let fx = fixture();
        write_legacy_review(&fx);
        fs::write(&fx.file, "one\nTWO\nthree\nfour\n").unwrap();
        fx.reviews.reply(&fx.file, "c1", "hi", "s").unwrap();

        assert_eq!(lines_of(&view_of(&fx, "c1")), (2, 2, State::Outdated));
    }

    #[test]
    fn a_missing_snapshot_is_outdated_at_the_clamped_stored_range() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 3, 4, "x", "s").unwrap();
        let dir = fx.store.review_dir(&fx.file).unwrap();
        fs::remove_dir_all(dir.join("revisions")).unwrap();

        fs::write(&fx.file, "one\ntwo\nthree\n").unwrap();

        assert_eq!(lines_of(&view_of(&fx, "c1")), (3, 3, State::Outdated));
    }

    #[test]
    fn view_names_the_current_revision_or_none_for_new_content() {
        let fx = fixture();
        assert_eq!(fx.reviews.view(&fx.file, true).unwrap().revision, None);

        fx.reviews.add_thread(&fx.file, 1, 1, "x", "s").unwrap();
        assert_eq!(
            fx.reviews.view(&fx.file, true).unwrap().revision.as_deref(),
            Some("r1")
        );

        fs::write(&fx.file, "changed\n").unwrap();
        assert_eq!(fx.reviews.view(&fx.file, true).unwrap().revision, None);
    }

    #[test]
    fn view_of_a_review_with_no_threads_is_empty() {
        let fx = fixture();

        assert!(fx.reviews.view(&fx.file, true).unwrap().threads.is_empty());
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
                resolved_at: NOW.into(),
                resolved_revision: Some("r1".into()),
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
                revision: Some("r1".into()),
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
    fn resolving_a_resolved_thread_is_already_resolved_and_changes_nothing() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        fx.reviews
            .resolve(&fx.file, "c1", "claude", Some("Done"))
            .unwrap();
        let bytes = fx.review_json();

        let result = fx
            .reviews
            .resolve(&fx.file, "c1", "someone-else", Some("Again"));

        assert!(matches!(result, Err(Error::AlreadyResolved(id)) if id == "c1"));
        assert_eq!(fx.review_json(), bytes);
    }

    #[test]
    fn resolve_error_precedence_is_author_then_not_found_then_already_resolved() {
        let fx = fixture();
        resolved_thread(&fx);
        let bytes = fx.review_json();

        assert!(matches!(
            fx.reviews.resolve(&fx.file, "c1", " ", None),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            fx.reviews.resolve(&fx.file, "c99", "claude", None),
            Err(Error::NotFound(id)) if id == "c99"
        ));
        assert!(matches!(
            fx.reviews.resolve(&fx.file, "c1", "claude", None),
            Err(Error::AlreadyResolved(id)) if id == "c1"
        ));
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

    // --- reply ---

    const LATER: &str = "2026-10-12T11:00:00Z";

    fn later_now() -> String {
        LATER.to_string()
    }

    #[test]
    fn reply_appends_a_comment_with_a_fresh_id_and_keeps_the_thread_open() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        let replier = Reviews::with_clock(fx.store.clone(), later_now as fn() -> String);

        let thread = replier.reply(&fx.file, "c1", "Because.", "claude").unwrap();

        assert_eq!(thread.id, "c1");
        assert_eq!(thread.status, Status::Open);
        assert_eq!(
            thread.comments[1],
            Comment {
                id: "c2".into(),
                author: "claude".into(),
                body: "Because.".into(),
                created_at: LATER.into(),
                revision: Some("r1".into()),
            }
        );
        assert_eq!(thread.comments.len(), 2);
        assert_eq!(fx.store.load(&fx.file).unwrap().threads, vec![thread]);
    }

    #[test]
    fn replies_keep_order_and_share_the_id_counter_with_threads() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        fx.reviews.add_thread(&fx.file, 2, 2, "y", "scott").unwrap();
        fx.reviews.reply(&fx.file, "c1", "first", "claude").unwrap();
        let thread = fx.reviews.reply(&fx.file, "c1", "second", "scott").unwrap();
        let next = fx.reviews.add_thread(&fx.file, 3, 3, "z", "scott").unwrap();

        let ids: Vec<&str> = thread.comments.iter().map(|c| c.id.as_str()).collect();
        let bodies: Vec<&str> = thread.comments.iter().map(|c| c.body.as_str()).collect();
        assert_eq!(ids, ["c1", "c3", "c4"]);
        assert_eq!(bodies, ["x", "first", "second"]);
        assert_eq!(next.id, "c5");
    }

    #[test]
    fn reply_to_a_resolved_thread_reopens_it_on_disk() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        fx.reviews.resolve(&fx.file, "c1", "claude", None).unwrap();

        let thread = fx
            .reviews
            .reply(&fx.file, "c1", "Not yet", "scott")
            .unwrap();

        assert_eq!(thread.status, Status::Open);
        let json = String::from_utf8(fx.review_json().unwrap()).unwrap();
        assert!(!json.contains("resolved_by"), "{json}");
        assert!(!json.contains("resolved_at"), "{json}");
    }

    #[test]
    fn reply_errors_change_nothing_and_do_not_burn_an_id() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        let bytes = fx.review_json();

        assert!(matches!(
            fx.reviews.reply(&fx.file, "c99", "hi", "claude"),
            Err(Error::NotFound(id)) if id == "c99"
        ));
        assert!(matches!(
            fx.reviews.reply(&fx.file, "c1", "  ", "claude"),
            Err(Error::EmptyBody)
        ));
        assert!(matches!(
            fx.reviews.reply(&fx.file, "c1", "hi", " "),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            fx.reviews.reply(&fx.file, "c1", "", ""),
            Err(Error::EmptyAuthor)
        ));
        assert_eq!(fx.review_json(), bytes);
        let thread = fx.reviews.reply(&fx.file, "c1", "hi", "claude").unwrap();
        assert_eq!(thread.comments[1].id, "c2");
    }

    // --- reopen ---

    fn resolved_thread(fx: &Fixture) {
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        fx.reviews.resolve(&fx.file, "c1", "claude", None).unwrap();
    }

    #[test]
    fn reopen_makes_a_resolved_thread_open_and_drops_resolution_fields() {
        let fx = fixture();
        resolved_thread(&fx);

        let thread = fx.reviews.reopen(&fx.file, "c1", "scott", None).unwrap();

        assert_eq!(thread.status, Status::Open);
        assert_eq!(thread.comments.len(), 1);
        let json = String::from_utf8(fx.review_json().unwrap()).unwrap();
        assert!(!json.contains("resolved_by"), "{json}");
        assert!(!json.contains("resolved_at"), "{json}");
        assert_eq!(fx.store.load(&fx.file).unwrap().threads, vec![thread]);
    }

    #[test]
    fn reopen_note_becomes_a_comment_by_the_reopener_and_blank_adds_none() {
        let fx = fixture();
        resolved_thread(&fx);

        let thread = fx
            .reviews
            .reopen(&fx.file, "c1", "scott", Some("Wrong guess"))
            .unwrap();

        assert_eq!(
            thread.comments[1],
            Comment {
                id: "c2".into(),
                author: "scott".into(),
                body: "Wrong guess".into(),
                created_at: NOW.into(),
                revision: Some("r1".into()),
            }
        );

        fx.reviews.resolve(&fx.file, "c1", "claude", None).unwrap();
        let thread = fx
            .reviews
            .reopen(&fx.file, "c1", "scott", Some("  "))
            .unwrap();
        assert_eq!(thread.comments.len(), 2);
    }

    #[test]
    fn reopening_an_open_thread_is_already_open_and_changes_nothing() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        let bytes = fx.review_json();

        let result = fx.reviews.reopen(&fx.file, "c1", "scott", Some("note"));

        assert!(matches!(result, Err(Error::AlreadyOpen(id)) if id == "c1"));
        assert_eq!(fx.review_json(), bytes);
        let next = fx.reviews.reply(&fx.file, "c1", "hi", "scott").unwrap();
        assert_eq!(next.comments[1].id, "c2");
    }

    #[test]
    fn reopen_error_precedence_is_author_then_not_found_then_already_open() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        let bytes = fx.review_json();

        assert!(matches!(
            fx.reviews.reopen(&fx.file, "c1", " ", None),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            fx.reviews.reopen(&fx.file, "c99", "scott", None),
            Err(Error::NotFound(id)) if id == "c99"
        ));
        assert_eq!(fx.review_json(), bytes);
    }

    #[test]
    fn resolve_reopen_resolve_records_the_new_resolver() {
        let fx = fixture();
        resolved_thread(&fx);
        fx.reviews.reopen(&fx.file, "c1", "scott", None).unwrap();

        let thread = fx.reviews.resolve(&fx.file, "c1", "codex", None).unwrap();

        assert_eq!(
            thread.status,
            Status::Resolved {
                resolved_by: "codex".into(),
                resolved_at: NOW.into(),
                resolved_revision: Some("r1".into()),
            }
        );
    }

    // --- revisions recorded by writes ---

    fn snapshot_of(fx: &Fixture, content: &str) -> Option<Vec<u8>> {
        fx.store
            .load_revision(&fx.file, &content_hash(content.as_bytes()))
            .unwrap()
    }

    #[test]
    fn add_thread_records_its_revision_and_snapshots_the_file() {
        let fx = fixture();

        let thread = fx.reviews.add_thread(&fx.file, 2, 2, "x", "s").unwrap();

        assert_eq!(thread.anchor.revision.as_deref(), Some("r1"));
        assert_eq!(thread.comments[0].revision.as_deref(), Some("r1"));
        let review = fx.store.load(&fx.file).unwrap();
        let sha = content_hash(b"one\ntwo\nthree\nfour\n");
        assert_eq!(review.revision_id(&sha), Some("r1"));
        assert_eq!(
            snapshot_of(&fx, "one\ntwo\nthree\nfour\n").as_deref(),
            Some(&b"one\ntwo\nthree\nfour\n"[..])
        );
    }

    #[test]
    fn reply_resolve_and_reopen_record_the_revision_they_were_made_against() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "s").unwrap();

        fs::write(&fx.file, "one\ntwo\n").unwrap();
        let replied = fx.reviews.reply(&fx.file, "c1", "r", "claude").unwrap();
        assert_eq!(replied.comments[1].revision.as_deref(), Some("r2"));

        fs::write(&fx.file, "one\n").unwrap();
        let resolved = fx
            .reviews
            .resolve(&fx.file, "c1", "claude", Some("done"))
            .unwrap();
        assert!(matches!(
            &resolved.status,
            Status::Resolved { resolved_revision: Some(r), .. } if r == "r3"
        ));
        assert_eq!(resolved.comments[2].revision.as_deref(), Some("r3"));

        fs::write(&fx.file, "ONE\n").unwrap();
        let reopened = fx
            .reviews
            .reopen(&fx.file, "c1", "scott", Some("no"))
            .unwrap();
        assert_eq!(reopened.comments[3].revision.as_deref(), Some("r4"));
        assert_eq!(reopened.anchor.revision.as_deref(), Some("r1"));
    }

    #[test]
    fn going_back_to_earlier_content_returns_to_its_revision() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "s").unwrap();

        fs::write(&fx.file, "B\n").unwrap();
        let b = fx.reviews.reply(&fx.file, "c1", "b", "s").unwrap();
        fs::write(&fx.file, "one\ntwo\nthree\nfour\n").unwrap();
        let a = fx.reviews.reply(&fx.file, "c1", "a", "s").unwrap();

        assert_eq!(b.comments[1].revision.as_deref(), Some("r2"));
        assert_eq!(a.comments[2].revision.as_deref(), Some("r1"));
        let dir = fx.store.review_dir(&fx.file).unwrap();
        assert_eq!(fs::read_dir(dir.join("revisions")).unwrap().count(), 2);
        assert_eq!(fx.store.load(&fx.file).unwrap().revisions.len(), 2);
    }

    #[test]
    fn a_failed_write_records_no_revision() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "s").unwrap();
        fs::write(&fx.file, "changed\n").unwrap();

        assert!(fx.reviews.reply(&fx.file, "c99", "x", "s").is_err());
        assert!(fx.reviews.reopen(&fx.file, "c1", "s", None).is_err());

        assert_eq!(fx.store.load(&fx.file).unwrap().revisions.len(), 1);
        assert_eq!(snapshot_of(&fx, "changed\n"), None);
    }

    /// A `review.json` in the shape written before revisions existed.
    fn write_legacy_review(fx: &Fixture) {
        let dir = fx.store.review_dir(&fx.file).unwrap();
        fs::create_dir_all(&dir).unwrap();
        let canonical = fs::canonicalize(&fx.file).unwrap();
        let json = serde_json::json!({
            "schema_version": 1,
            "path": canonical,
            "next_id": 2,
            "threads": [{
                "id": "c1",
                "anchor": { "start_line": 2, "end_line": 2, "quote": "two" },
                "status": "open",
                "comments": [{
                    "id": "c1", "author": "scott", "body": "x", "created_at": NOW
                }]
            }]
        });
        fs::write(dir.join("review.json"), json.to_string()).unwrap();
    }

    #[test]
    fn first_write_after_the_upgrade_anchors_legacy_threads_to_its_revision() {
        let fx = fixture();
        write_legacy_review(&fx);

        let thread = fx.reviews.reply(&fx.file, "c1", "hi", "claude").unwrap();

        assert_eq!(thread.anchor.revision.as_deref(), Some("r1"));
        assert_eq!(thread.comments[0].revision, None);
        assert_eq!(thread.comments[1].revision.as_deref(), Some("r1"));
    }

    #[test]
    fn viewing_never_writes() {
        let fx = fixture();

        fx.reviews.view(&fx.file, true).unwrap();
        fx.store.load(&fx.file).unwrap();

        assert!(!fx.store.review_dir(&fx.file).unwrap().exists());
    }

    #[test]
    fn writes_on_a_non_utf8_file_are_unreadable_before_the_id_is_checked() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "s").unwrap();
        let bytes = fx.review_json();
        fs::write(&fx.file, [0xff, 0xfe]).unwrap();

        let results = [
            fx.reviews.reply(&fx.file, "c99", "x", "s"),
            fx.reviews.resolve(&fx.file, "c99", "s", None),
            fx.reviews.reopen(&fx.file, "c99", "s", None),
        ];

        for result in results {
            assert!(
                matches!(result, Err(Error::FileUnreadable { .. })),
                "{result:?}"
            );
        }
        assert_eq!(fx.review_json(), bytes);
    }
}
