//! The application core: the only write path, built on [`Store`].
//!
//! No printing and no terminal awareness; front ends render what these
//! operations return.

use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use jiff::Timestamp;
use serde::Serialize;

use crate::anchor::{Change, Diff, LineRange, Mapped, clamp};
use crate::error::{Error, Result};
use crate::model::{
    Anchor, Comment, Entry, Event, Review, ReviewState, Side, State, Status, Thread, Verdict,
};
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

/// How often [`Reviews::wait`] re-reads the review, and how often the TUI
/// checks for the other side's writes.
pub const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// How many reads of `review.json` in a row [`Reviews::wait`] lets fail
/// with [`Error::Io`] before giving up: about five seconds of polls, to
/// ride out a transient I/O error.
pub const WAIT_READ_RETRIES: u32 = 20;

/// A monotonic clock that can be slept on, injected so waiting is
/// testable without real time passing.
pub trait Ticker {
    /// The current instant.
    fn now(&self) -> Instant;
    /// Block for `duration`.
    fn sleep(&self, duration: Duration);
}

/// The real monotonic clock and [`thread::sleep`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemTicker;

impl Ticker for SystemTicker {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }
}

/// Why a [`Reviews::wait`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Woke {
    /// It is the agent's turn.
    Turn,
    /// The review is closed; its verdict is in the status.
    Closed,
    /// The timeout passed on the human's turn.
    Timeout,
}

/// The result of a [`Reviews::wait`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Waited {
    pub reason: Woke,
    /// The review's state when the wait returned; `latest` is the hand-off
    /// or close that ended it.
    pub review: ReviewStatus,
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
    /// The [`content_hash`] of the bytes read for this view: what a front
    /// end that shows [`lines`](View::lines) passes to [`Reviews::against`].
    pub sha256: String,
    /// The file's lines as read for this view, split by [`str::lines`]
    /// (`\n` or `\r\n`, a final newline ignored): exactly the lines
    /// `add_thread` quotes from and the threads are mapped to, so a front end
    /// that displays them shows what gets anchored.
    pub lines: Vec<String>,
    pub threads: Vec<ThreadView>,
    /// Whose turn it is, or the verdict, and the latest log entry.
    pub review: ReviewStatus,
    /// What changed in [`lines`](View::lines) since the human's last look
    /// ([`Review::last_look`]), one [`Change`] per hunk. Empty with no last
    /// look yet or when its snapshot is missing or unreadable.
    pub changes: Vec<Change>,
}

/// A review's overall state as shown to front ends: whose turn it is, or
/// the verdict if closed, and the latest log entry.
///
/// On the wire, `{"turn", "verdict", "latest"}`: `turn` is `null` exactly
/// when the review is closed, and `verdict` is set exactly then. `latest`
/// is `null` for a review with no hand-off, close or reopen yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewStatus {
    pub state: ReviewState,
    pub latest: Option<Entry>,
}

impl ReviewStatus {
    /// The state of `review`, derived from its log.
    fn of(review: &Review) -> Self {
        Self {
            state: review.state(),
            latest: review.log.last().cloned(),
        }
    }

    /// The side expected to act next, or `None` when closed.
    pub fn turn(&self) -> Option<Side> {
        match self.state {
            ReviewState::Turn(side) => Some(side),
            ReviewState::Closed(_) => None,
        }
    }

    /// The verdict, or `None` while the review is active.
    pub fn verdict(&self) -> Option<Verdict> {
        match self.state {
            ReviewState::Turn(_) => None,
            ReviewState::Closed(verdict) => Some(verdict),
        }
    }
}

impl Default for ReviewStatus {
    /// A fresh review: the human's turn, nothing logged.
    fn default() -> Self {
        Self::of(&Review::new(""))
    }
}

impl Serialize for ReviewStatus {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Wire<'a> {
            turn: Option<Side>,
            verdict: Option<Verdict>,
            latest: &'a Option<Entry>,
        }
        Wire {
            turn: self.turn(),
            verdict: self.verdict(),
            latest: &self.latest,
        }
        .serialize(serializer)
    }
}

/// The result of a hand-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandedOff {
    /// The review's state after the hand-off.
    pub review: ReviewStatus,
    /// Ids of open threads whose last comment isn't by the author handing
    /// off: still waiting on them (exact name match), as of the hand-off.
    pub waiting: Vec<String>,
}

/// A fingerprint of a reviewed file and its review: see [`Reviews::stamp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamp {
    file: String,
    review: Option<String>,
}

/// Writes made against content a front end showed: see
/// [`Reviews::against`].
#[derive(Debug, Clone, Copy)]
pub struct Against<'a, C> {
    reviews: &'a Reviews<C>,
    seen: &'a str,
}

impl<C: Clock> Against<'_, C> {
    /// [`Reviews::add_thread`] on the lines that were shown.
    ///
    /// # Errors
    /// As [`Reviews::add_thread`], with [`Error::FileChanged`] checked
    /// after the file is read and before the range.
    pub fn add_thread(
        &self,
        file: &Path,
        start: u32,
        end: u32,
        body: &str,
        author: &str,
    ) -> Result<Thread> {
        self.reviews
            .add_thread_seen(Some(self.seen), file, start, end, body, author)
    }

    /// [`Reviews::hand_off`], recording the content shown.
    ///
    /// # Errors
    /// As [`Reviews::hand_off`], with [`Error::FileChanged`] checked after
    /// the file is read and before the turn.
    pub fn hand_off(
        &self,
        file: &Path,
        side: Side,
        author: &str,
        note: Option<&str>,
    ) -> Result<HandedOff> {
        self.reviews
            .hand_off_seen(Some(self.seen), file, side, author, note)
    }

    /// [`Reviews::close`], recording the content shown.
    ///
    /// # Errors
    /// As [`Reviews::close`], with [`Error::FileChanged`] checked after the
    /// file is read and before whether the review is closed.
    pub fn close(
        &self,
        file: &Path,
        author: &str,
        verdict: Option<Verdict>,
        note: Option<&str>,
    ) -> Result<ReviewStatus> {
        self.reviews
            .close_seen(Some(self.seen), file, author, verdict, note)
    }
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
        self.add_thread_seen(None, file, start, end, body, author)
    }

    /// [`Reviews::add_thread`], refused with [`Error::FileChanged`] when
    /// `seen` is set and the file read no longer hashes to it.
    fn add_thread_seen(
        &self,
        seen: Option<&str>,
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
        let content = Content::read(file)?.seen(seen)?;
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
    /// [`map_range`](crate::anchor::map_range)): it is `current` only if the anchored lines are
    /// unchanged and still read as its `quote`. Otherwise it is `outdated`
    /// at a best-guess range within the file. A thread from before revisions
    /// existed is checked in place, and one whose snapshot is missing is
    /// `outdated` at its stored range, clamped to the file. Resolved threads
    /// are omitted unless `include_resolved`.
    ///
    /// [`View::changes`] diffs the same read against the revision of the
    /// human's last look ([`Review::last_look`]). Never writes, so opening a
    /// view is not itself a look.
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
        let review = self.store.load(file)?;
        let mut mapper = Mapper::new(&self.store, file, &review, &content);
        let threads = review
            .threads
            .iter()
            .filter(|t| include_resolved || t.status == Status::Open)
            .cloned()
            .map(|mut thread| {
                let state = mapper.map(&mut thread.anchor);
                ThreadView { thread, state }
            })
            .collect();
        let changes = review
            .last_look()
            .and_then(|look| review.revision_sha(&look.revision))
            .and_then(|sha| mapper.diff(sha))
            .map(Diff::changes)
            .unwrap_or_default();
        Ok(View {
            revision: review.revision_id(&content.sha256).map(String::from),
            lines: content.text.lines().map(String::from).collect(),
            sha256: content.sha256.clone(),
            threads,
            review: ReviewStatus::of(&review),
            changes,
        })
    }

    /// The writes that depend on what a front end showed, made against the
    /// content hashing to `sha256` (a [`View::sha256`]).
    ///
    /// Each refuses with [`Error::FileChanged`], writing nothing, when the
    /// file now reads differently, so a hand-off or close records the
    /// content that was shown as the human's last look, and a new thread
    /// lands on the lines that were shown.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{Error, model::Side, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// let shown = reviews.view(&file, true)?;
    /// std::fs::write(&file, "ONE\n")?;
    ///
    /// let refused = reviews.against(&shown.sha256).hand_off(&file, Side::Human, "scott", None);
    /// assert!(matches!(refused, Err(Error::FileChanged)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn against<'a>(&'a self, sha256: &'a str) -> Against<'a, C> {
        Against {
            reviews: self,
            seen: sha256,
        }
    }

    /// A cheap fingerprint of `file` and its review, to tell whether either
    /// changed since the last look without a full [`Reviews::view`].
    ///
    /// The hashes of the file's bytes and of `review.json`'s bytes: unlike
    /// mtimes, they can't miss a same-second, same-length write. Never
    /// writes.
    ///
    /// # Errors
    /// [`Error::FileUnreadable`], or a store error reading `review.json`.
    pub fn stamp(&self, file: &Path) -> Result<Stamp> {
        let bytes = fs::read(file).map_err(|source| Error::FileUnreadable {
            path: file.to_path_buf(),
            source,
        })?;
        let review = self.store.reader(file)?.bytes()?;
        Ok(Stamp {
            file: content_hash(&bytes),
            review: review.as_deref().map(content_hash),
        })
    }

    /// Block until it is no longer the human's turn on `file`, or until
    /// `timeout` passes on `ticker`'s clock.
    ///
    /// Level-triggered: the state is checked before the first sleep, so a
    /// review already on the agent's turn, or closed, returns at once, and
    /// a hand-off that lands between two waits is never missed. Re-reads
    /// `review.json` every [`POLL_INTERVAL`]; a timeout of zero checks once.
    /// The file must be readable when the wait starts; its review's
    /// location is resolved then, so the file may go missing while
    /// waiting. Never writes.
    ///
    /// # Errors
    /// [`Error::FileUnreadable`] at the start. An [`Error::Io`] reading
    /// `review.json` is retried on the next poll, up to
    /// [`WAIT_READ_RETRIES`] in a row; any other store error ends the wait
    /// at once.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::Side, review::{Reviews, SystemTicker, Woke}, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// reviews.hand_off(&file, Side::Human, "scott", Some("Over to you"))?;
    ///
    /// let waited = reviews.wait(&file, None, &SystemTicker)?;
    /// assert_eq!(waited.reason, Woke::Turn);
    /// assert_eq!(waited.review.latest.unwrap().note.as_deref(), Some("Over to you"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn wait(
        &self,
        file: &Path,
        timeout: Option<Duration>,
        ticker: &dyn Ticker,
    ) -> Result<Waited> {
        Content::read(file)?;
        let reader = self.store.reader(file)?;
        let start = ticker.now();
        let mut failed_reads = 0;
        let mut last = ReviewStatus::default();
        loop {
            let reason = match reader.load() {
                Ok(review) => {
                    failed_reads = 0;
                    last = ReviewStatus::of(&review);
                    match last.state {
                        ReviewState::Turn(Side::Agent) => Some(Woke::Turn),
                        ReviewState::Closed(_) => Some(Woke::Closed),
                        ReviewState::Turn(Side::Human) => None,
                    }
                }
                Err(Error::Io { .. }) if failed_reads < WAIT_READ_RETRIES => {
                    failed_reads += 1;
                    None
                }
                Err(error) => return Err(error),
            };
            let left = timeout.map(|t| t.saturating_sub(ticker.now() - start));
            let reason = reason.or((left == Some(Duration::ZERO)).then_some(Woke::Timeout));
            if let Some(reason) = reason {
                return Ok(Waited {
                    reason,
                    review: last,
                });
            }
            ticker.sleep(left.map_or(POLL_INTERVAL, |left| left.min(POLL_INTERVAL)));
        }
    }

    /// Resolve thread `id`, optionally appending `note` as a comment by
    /// `author`.
    ///
    /// A blank note adds no comment. The resolve (`resolved_revision`) and
    /// the note record the revision of the file as it is now.
    ///
    /// The returned thread's lines are mapped to the file as it is now, as
    /// [`Reviews::view`] shows them; the stored anchor is unchanged.
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
            self.mapped(file, review, index, &content)
        })
    }

    /// Reopen resolved thread `id`, optionally appending `note` as a comment
    /// by `author`.
    ///
    /// The resolution record is dropped. A blank note adds no comment; a
    /// note records the revision of the file as it is now (a reopen itself
    /// is not recorded, so one without a note records none).
    ///
    /// The returned thread's lines are mapped to the file as it is now, as
    /// [`Reviews::view`] shows them; the stored anchor is unchanged.
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
            let note_comment = note.map(|body| {
                let revision = record_revision(review, snapshots, &content);
                new_comment(review, author, body, &now, &revision)
            });
            let thread = &mut review.threads[index];
            thread.status = Status::Open;
            thread.comments.extend(note_comment);
            self.mapped(file, review, index, &content)
        })
    }

    /// Append a comment by `author` to thread `id`.
    ///
    /// Replying to a resolved thread reopens it in the same write; no
    /// separate "reopened" comment is added. The body is stored verbatim,
    /// and the comment records the revision of the file as it is now.
    ///
    /// The returned thread's lines are mapped to the file as it is now, as
    /// [`Reviews::view`] shows them; the stored anchor is unchanged.
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
            self.mapped(file, review, index, &content)
        })
    }

    /// Hand the turn from `side` to the other side, as `author`, with an
    /// optional `note`.
    ///
    /// Appends a `hand_off` entry to the log, recording the time, the
    /// review's `next_id` and the revision of the file as it is now. A blank
    /// note is no note. The turn is a signal, not a lock: no other write
    /// reads or moves it, and only this one checks it.
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::FileUnreadable`], plus any store
    /// error, then [`Error::NotYourTurn`] if `side` doesn't hold the turn
    /// (also on a closed review, which has no turn), checked in that order.
    /// Nothing is written on error.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::Side, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// reviews.add_thread(&file, 1, 1, "Why?", "scott")?;
    ///
    /// let handed = reviews.hand_off(&file, Side::Human, "scott", Some("Over to you"))?;
    /// assert_eq!(handed.review.turn(), Some(Side::Agent));
    /// assert!(handed.waiting.is_empty(), "scott has the last word on c1");
    ///
    /// let back = reviews.hand_off(&file, Side::Agent, "claude", None)?;
    /// assert_eq!(back.waiting, ["c1"]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn hand_off(
        &self,
        file: &Path,
        side: Side,
        author: &str,
        note: Option<&str>,
    ) -> Result<HandedOff> {
        self.hand_off_seen(None, file, side, author, note)
    }

    /// [`Reviews::hand_off`], refused with [`Error::FileChanged`] when
    /// `seen` is set and the file read no longer hashes to it.
    fn hand_off_seen(
        &self,
        seen: Option<&str>,
        file: &Path,
        side: Side,
        author: &str,
        note: Option<&str>,
    ) -> Result<HandedOff> {
        require_author(author)?;
        let content = Content::read(file)?.seen(seen)?;
        let signature = Signature {
            side,
            author,
            created_at: self.clock.now(),
            note,
        };

        self.store.with_review(file, |review, snapshots| {
            if review.state() != ReviewState::Turn(side) {
                return Err(Error::NotYourTurn);
            }
            signature.log(review, snapshots, &content, Event::HandOff);
            let waiting = review
                .threads
                .iter()
                .filter(|t| t.is_waiting_on(author))
                .map(|t| t.id.clone())
                .collect();
            Ok(HandedOff {
                review: ReviewStatus::of(review),
                waiting,
            })
        })
    }

    /// Close the review as the human `author`, with `verdict` and an
    /// optional `note`.
    ///
    /// Only the human closes, at any time: during either side's turn. Open
    /// threads don't block `approved`; they stay open as they are. A closed
    /// review has no turn, and further thread writes don't reopen it. Appends
    /// a `close` entry recording the revision of the file as it is now.
    ///
    /// `verdict` is an `Option` so that a front end can pass on a close
    /// whose verdict wasn't chosen and get [`Error::MissingVerdict`] back.
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::MissingVerdict`] for `None`,
    /// [`Error::FileUnreadable`], plus any store error, then
    /// [`Error::AlreadyClosed`] if the review is closed, checked in that
    /// order. Nothing is written on error.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::Verdict, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    ///
    /// let status = reviews.close(&file, "scott", Some(Verdict::Approved), Some("Ship it"))?;
    /// assert_eq!(status.verdict(), Some(Verdict::Approved));
    /// assert_eq!(status.turn(), None);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn close(
        &self,
        file: &Path,
        author: &str,
        verdict: Option<Verdict>,
        note: Option<&str>,
    ) -> Result<ReviewStatus> {
        self.close_seen(None, file, author, verdict, note)
    }

    /// [`Reviews::close`], refused with [`Error::FileChanged`] when `seen`
    /// is set and the file read no longer hashes to it.
    fn close_seen(
        &self,
        seen: Option<&str>,
        file: &Path,
        author: &str,
        verdict: Option<Verdict>,
        note: Option<&str>,
    ) -> Result<ReviewStatus> {
        require_author(author)?;
        let verdict = verdict.ok_or(Error::MissingVerdict)?;
        let content = Content::read(file)?.seen(seen)?;
        let signature = Signature {
            side: Side::Human,
            author,
            created_at: self.clock.now(),
            note,
        };

        self.store.with_review(file, |review, snapshots| {
            if let ReviewState::Closed(_) = review.state() {
                return Err(Error::AlreadyClosed);
            }
            signature.log(review, snapshots, &content, Event::Close { verdict });
            Ok(ReviewStatus::of(review))
        })
    }

    /// Reopen a closed review as `author` on `side`, with an optional
    /// `note`. Either side may; the review returns to the human's turn.
    ///
    /// Appends a `reopen` entry recording the revision of the file as it is
    /// now.
    ///
    /// # Errors
    /// [`Error::EmptyAuthor`], [`Error::FileUnreadable`], plus any store
    /// error, then [`Error::NotClosed`] if the review is active, checked in
    /// that order. Nothing is written on error.
    ///
    /// # Examples
    /// ```
    /// # use ptal::{model::{Side, Verdict}, review::Reviews, store::Store};
    /// # let dir = tempfile::tempdir()?;
    /// # let file = dir.path().join("plan.md");
    /// # std::fs::write(&file, "one\n")?;
    /// let reviews = Reviews::new(Store::new(dir.path().join("data")));
    /// reviews.close(&file, "scott", Some(Verdict::Dropped), None)?;
    ///
    /// let status = reviews.reopen_review(&file, Side::Agent, "claude", Some("Revising"))?;
    /// assert_eq!(status.turn(), Some(Side::Human));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn reopen_review(
        &self,
        file: &Path,
        side: Side,
        author: &str,
        note: Option<&str>,
    ) -> Result<ReviewStatus> {
        require_author(author)?;
        let content = Content::read(file)?;
        let signature = Signature {
            side,
            author,
            created_at: self.clock.now(),
            note,
        };

        self.store.with_review(file, |review, snapshots| {
            let ReviewState::Closed(_) = review.state() else {
                return Err(Error::NotClosed);
            };
            signature.log(review, snapshots, &content, Event::Reopen);
            Ok(ReviewStatus::of(review))
        })
    }

    /// Thread `index` of `review` with its lines mapped to `content`, for
    /// output; the stored anchor is unchanged.
    fn mapped(
        &self,
        file: &Path,
        review: &Review,
        index: usize,
        content: &Content,
    ) -> Result<Thread> {
        let mut thread = review.threads[index].clone();
        Mapper::new(&self.store, file, review, content).map(&mut thread.anchor);
        Ok(thread)
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

/// Who writes a log entry, when, and with what note: everything about it
/// that is known before the review is locked.
struct Signature<'a> {
    side: Side,
    author: &'a str,
    created_at: String,
    note: Option<&'a str>,
}

impl Signature<'_> {
    /// Append an `event` entry to `review`'s log, recording the revision of
    /// `content` (snapshotted if new) and the review's `next_id`. A blank
    /// note is no note. Call this only once the operation can no longer fail.
    fn log(self, review: &mut Review, snapshots: &mut Snapshots, content: &Content, event: Event) {
        let revision = record_revision(review, snapshots, content);
        let entry = Entry {
            event,
            side: self.side,
            author: self.author.to_string(),
            created_at: self.created_at,
            revision,
            next_id: review.next_id,
            note: self.note.filter(|n| !n.trim().is_empty()).map(String::from),
        };
        review.log.push(entry);
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

    /// This content, unless `seen` is set and it hashes differently.
    ///
    /// # Errors
    /// [`Error::FileChanged`].
    fn seen(self, seen: Option<&str>) -> Result<Self> {
        match seen {
            Some(sha) if sha != self.sha256 => Err(Error::FileChanged),
            _ => Ok(self),
        }
    }

    fn lines(&self) -> Vec<&str> {
        self.text.lines().collect()
    }
}

/// Maps anchors from their own revision to one current content, diffing
/// each revision against it at most once.
struct Mapper<'a> {
    store: &'a Store,
    file: &'a Path,
    review: &'a Review,
    current: &'a Content,
    lines: Vec<&'a str>,
    /// Diff from each revision (by hash) to the current content; `None` if
    /// its snapshot is missing or unreadable.
    diffs: HashMap<String, Option<Diff>>,
}

impl<'a> Mapper<'a> {
    fn new(store: &'a Store, file: &'a Path, review: &'a Review, current: &'a Content) -> Self {
        Self {
            store,
            file,
            review,
            current,
            lines: current.lines(),
            diffs: HashMap::new(),
        }
    }

    /// Rewrite `anchor`'s lines to the current file and say whether it is
    /// still current. Its `revision` and `quote` are left as stored.
    ///
    /// An anchor from before revisions is checked in place. One whose
    /// snapshot is missing or unreadable is outdated at its stored range,
    /// clamped: mapping never fails.
    fn map(&mut self, anchor: &mut Anchor) -> State {
        let stored = LineRange {
            start: anchor.start_line,
            end: anchor.end_line,
        };
        let sha = match anchor.revision.as_deref() {
            None => Some(self.current.sha256.as_str()),
            Some(id) => self.review.revision_sha(id),
        };
        let mapped = match sha.and_then(|sha| self.diff(sha)) {
            Some(diff) => diff.map(stored),
            None => Mapped {
                range: clamp(stored, self.lines.len()),
                state: State::Outdated,
            },
        };
        anchor.start_line = mapped.range.start;
        anchor.end_line = mapped.range.end;
        let reads_as_quote = quote_of(&self.lines, mapped.range.start, mapped.range.end)
            .is_some_and(|q| q == anchor.quote);
        if mapped.state == State::Current && reads_as_quote {
            State::Current
        } else {
            State::Outdated
        }
    }

    /// The diff from the revision hashing to `sha` to the current content.
    fn diff(&mut self, sha: &str) -> Option<&Diff> {
        if !self.diffs.contains_key(sha) {
            let diff = if sha == self.current.sha256 {
                Some(Diff::new(&self.lines, &self.lines))
            } else {
                self.store
                    .load_revision(self.file, sha)
                    .ok()
                    .flatten()
                    .and_then(|bytes| String::from_utf8(bytes).ok())
                    .map(|old| Diff::new(&old.lines().collect::<Vec<_>>(), &self.lines))
            };
            self.diffs.insert(sha.to_string(), diff);
        }
        self.diffs[sha].as_ref()
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
    use crate::anchor::Change;
    use crate::error::Error;
    use crate::model::{Anchor, Comment, Entry, Event, ReviewState, Side, State, Status, Verdict};
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

    // --- lines ---

    #[test]
    fn view_lines_split_like_str_lines() {
        let fx = fixture();
        fs::write(&fx.file, "one\r\ntwo\nthree").unwrap();

        let lines = fx.reviews.view(&fx.file, true).unwrap().lines;

        assert_eq!(lines, ["one", "two", "three"]);
    }

    #[test]
    fn view_of_a_missing_or_non_utf8_file_is_unreadable() {
        let fx = fixture();
        let binary = fx.file.with_file_name("bin.md");
        fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();

        for path in [fx.file.with_file_name("nope.md"), binary] {
            assert!(matches!(
                fx.reviews.view(&path, true),
                Err(Error::FileUnreadable { .. })
            ));
        }
    }

    #[test]
    fn add_thread_quote_is_the_view_lines_entry() {
        let fx = fixture();
        fs::write(&fx.file, "a\r\nb\r\n\r\nd").unwrap();
        let lines = fx.reviews.view(&fx.file, true).unwrap().lines;

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

    #[test]
    fn mutations_return_the_thread_mapped_to_the_file_as_it_is_now() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 2, 3, "x", "s").unwrap();
        fs::write(&fx.file, "zero\none\ntwo\nthree\nfour\n").unwrap();
        let lines = |t: &Thread| (t.anchor.start_line, t.anchor.end_line);

        let replied = fx.reviews.reply(&fx.file, "c1", "r", "s").unwrap();
        let resolved = fx.reviews.resolve(&fx.file, "c1", "s", None).unwrap();
        let reopened = fx.reviews.reopen(&fx.file, "c1", "s", None).unwrap();

        for thread in [&replied, &resolved, &reopened] {
            assert_eq!(lines(thread), (3, 4));
            assert_eq!(thread.anchor.revision.as_deref(), Some("r1"));
        }
        let stored = &fx.store.load(&fx.file).unwrap().threads[0];
        assert_eq!(lines(stored), (2, 3), "anchors are never rewritten");
    }

    #[test]
    fn an_unreadable_snapshot_is_outdated_not_an_error() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 3, 4, "x", "s").unwrap();
        let dir = fx.store.review_dir(&fx.file).unwrap().join("revisions");
        fs::remove_dir_all(&dir).unwrap();
        let sha = content_hash(b"one\ntwo\nthree\nfour\n");
        fs::create_dir_all(dir.join(sha)).unwrap();
        fs::write(&fx.file, "one\ntwo\nthree\n").unwrap();

        assert_eq!(lines_of(&view_of(&fx, "c1")), (3, 3, State::Outdated));
        let replied = fx.reviews.reply(&fx.file, "c1", "hi", "s").unwrap();
        assert_eq!(replied.anchor.start_line, 3);
    }

    #[test]
    fn a_reopen_without_a_note_records_no_revision() {
        let fx = fixture();
        resolved_thread(&fx);
        fs::write(&fx.file, "changed\n").unwrap();

        fx.reviews.reopen(&fx.file, "c1", "scott", None).unwrap();

        assert_eq!(fx.store.load(&fx.file).unwrap().revisions.len(), 1);
        assert_eq!(snapshot_of(&fx, "changed\n"), None);
        assert_eq!(fx.reviews.view(&fx.file, true).unwrap().revision, None);
    }

    // --- turns: hand-off ---

    fn state(fx: &Fixture) -> ReviewState {
        fx.store.load(&fx.file).unwrap().state()
    }

    #[test]
    fn a_human_hand_off_logs_an_entry_and_gives_the_agent_the_turn() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();

        let handed = fx
            .reviews
            .hand_off(&fx.file, Side::Human, "scott", Some("Over to you"))
            .unwrap();

        let entry = Entry {
            event: Event::HandOff,
            side: Side::Human,
            author: "scott".into(),
            created_at: NOW.into(),
            revision: "r1".into(),
            next_id: 2,
            note: Some("Over to you".into()),
        };
        assert_eq!(fx.store.load(&fx.file).unwrap().log, vec![entry.clone()]);
        assert_eq!(state(&fx), ReviewState::Turn(Side::Agent));
        assert_eq!(
            handed.review,
            ReviewStatus {
                state: ReviewState::Turn(Side::Agent),
                latest: Some(entry),
            }
        );
    }

    #[test]
    fn the_agent_hands_back_and_it_is_the_humans_turn() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();

        let handed = fx
            .reviews
            .hand_off(&fx.file, Side::Agent, "claude", Some("Revised"))
            .unwrap();

        assert_eq!(state(&fx), ReviewState::Turn(Side::Human));
        assert_eq!(handed.review.state, ReviewState::Turn(Side::Human));
        let latest = handed.review.latest.unwrap();
        assert_eq!(
            (latest.side, latest.author.as_str()),
            (Side::Agent, "claude")
        );
        assert_eq!(fx.store.load(&fx.file).unwrap().log.len(), 2);
    }

    #[test]
    fn a_hand_off_keeps_a_snapshot_of_the_revision_it_records() {
        let fx = fixture();

        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();

        let review = fx.store.load(&fx.file).unwrap();
        assert_eq!(review.log[0].revision, "r1");
        assert!(snapshot_of(&fx, "one\ntwo\nthree\nfour\n").is_some());
    }

    #[test]
    fn a_hand_off_by_the_side_without_the_turn_is_refused_and_writes_nothing() {
        let fx = fixture();

        assert!(matches!(
            fx.reviews
                .hand_off(&fx.file, Side::Agent, "claude", Some("n")),
            Err(Error::NotYourTurn)
        ));
        assert_eq!(fx.review_json(), None);

        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        let bytes = fx.review_json();
        assert!(matches!(
            fx.reviews.hand_off(&fx.file, Side::Human, "scott", None),
            Err(Error::NotYourTurn)
        ));
        assert_eq!(fx.review_json(), bytes);
    }

    #[test]
    fn a_hand_off_on_a_closed_review_is_not_your_turn() {
        let fx = fixture();
        fx.store
            .with_review(&fx.file, |review, _| {
                review.log.push(Entry {
                    event: Event::Close {
                        verdict: Verdict::Dropped,
                    },
                    side: Side::Human,
                    author: "scott".into(),
                    created_at: NOW.into(),
                    revision: "r1".into(),
                    next_id: 1,
                    note: None,
                });
                Ok(())
            })
            .unwrap();
        let bytes = fx.review_json();

        for side in [Side::Human, Side::Agent] {
            assert!(matches!(
                fx.reviews.hand_off(&fx.file, side, "x", None),
                Err(Error::NotYourTurn)
            ));
        }
        assert_eq!(fx.review_json(), bytes);
    }

    #[test]
    fn a_hand_off_lists_open_threads_whose_last_comment_is_not_the_authors() {
        let fx = fixture();
        fx.reviews
            .add_thread(&fx.file, 1, 1, "x", "claude")
            .unwrap(); // c1
        fx.reviews.add_thread(&fx.file, 2, 2, "y", "scott").unwrap(); // c2
        fx.reviews
            .add_thread(&fx.file, 3, 3, "z", "claude")
            .unwrap(); // c3
        fx.reviews.reply(&fx.file, "c3", "ok", "scott").unwrap();
        fx.reviews
            .add_thread(&fx.file, 4, 4, "w", "claude")
            .unwrap(); // c5
        fx.reviews.resolve(&fx.file, "c5", "claude", None).unwrap();

        let handed = fx
            .reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();

        assert_eq!(handed.waiting, ["c1"]);
    }

    #[test]
    fn a_blank_hand_off_note_is_no_note() {
        let fx = fixture();

        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", Some(" \n "))
            .unwrap();

        assert_eq!(fx.store.load(&fx.file).unwrap().log[0].note, None);
    }

    #[test]
    fn hand_off_error_precedence_is_author_then_file_then_turn() {
        let fx = fixture();
        let missing = fx.file.with_file_name("nope.md");

        assert!(matches!(
            fx.reviews.hand_off(&missing, Side::Agent, " ", None),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            fx.reviews.hand_off(&missing, Side::Agent, "claude", None),
            Err(Error::FileUnreadable { .. })
        ));
        assert!(matches!(
            fx.reviews.hand_off(&fx.file, Side::Agent, "claude", None),
            Err(Error::NotYourTurn)
        ));
        assert_eq!(fx.review_json(), None);
    }

    #[test]
    fn out_of_turn_thread_writes_succeed_and_leave_the_turn_alone() {
        let fx = fixture();
        fx.reviews
            .add_thread(&fx.file, 1, 1, "x", "claude")
            .unwrap();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();

        fx.reviews
            .add_thread(&fx.file, 2, 2, "late", "scott")
            .unwrap();
        fx.reviews.reply(&fx.file, "c1", "also", "scott").unwrap();
        fx.reviews.resolve(&fx.file, "c1", "scott", None).unwrap();
        fx.reviews.reopen(&fx.file, "c1", "scott", None).unwrap();

        assert_eq!(state(&fx), ReviewState::Turn(Side::Agent));
        assert_eq!(fx.store.load(&fx.file).unwrap().log.len(), 1);
    }

    // --- closing and reopening the review ---

    #[test]
    fn the_human_closes_during_either_turn_with_a_verdict() {
        for (hand_off_first, verdict) in [(false, Verdict::Approved), (true, Verdict::Dropped)] {
            let fx = fixture();
            if hand_off_first {
                fx.reviews
                    .hand_off(&fx.file, Side::Human, "scott", None)
                    .unwrap();
            }

            let status = fx
                .reviews
                .close(&fx.file, "scott", Some(verdict), Some("Done"))
                .unwrap();

            assert_eq!(state(&fx), ReviewState::Closed(verdict));
            let latest = status.latest.clone().unwrap();
            assert_eq!(
                latest,
                Entry {
                    event: Event::Close { verdict },
                    side: Side::Human,
                    author: "scott".into(),
                    created_at: NOW.into(),
                    revision: "r1".into(),
                    next_id: 1,
                    note: Some("Done".into()),
                }
            );
            assert_eq!(status.state, ReviewState::Closed(verdict));
        }
    }

    #[test]
    fn a_close_without_a_verdict_is_missing_verdict_and_writes_nothing() {
        let fx = fixture();

        assert!(matches!(
            fx.reviews.close(&fx.file, "scott", None, Some("n")),
            Err(Error::MissingVerdict)
        ));
        assert_eq!(fx.review_json(), None);
    }

    #[test]
    fn closing_a_closed_review_is_already_closed_and_writes_nothing() {
        let fx = fixture();
        fx.reviews
            .close(&fx.file, "scott", Some(Verdict::Approved), None)
            .unwrap();
        let bytes = fx.review_json();

        assert!(matches!(
            fx.reviews
                .close(&fx.file, "scott", Some(Verdict::Dropped), None),
            Err(Error::AlreadyClosed)
        ));
        assert_eq!(fx.review_json(), bytes);
        assert_eq!(state(&fx), ReviewState::Closed(Verdict::Approved));
    }

    #[test]
    fn close_error_precedence_is_author_then_verdict_then_file_then_state() {
        let fx = fixture();
        let missing = fx.file.with_file_name("nope.md");

        assert!(matches!(
            fx.reviews.close(&missing, " ", None, None),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            fx.reviews.close(&missing, "scott", None, None),
            Err(Error::MissingVerdict)
        ));
        assert!(matches!(
            fx.reviews
                .close(&missing, "scott", Some(Verdict::Approved), None),
            Err(Error::FileUnreadable { .. })
        ));
    }

    #[test]
    fn approving_with_open_threads_leaves_them_open() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();

        fx.reviews
            .close(&fx.file, "scott", Some(Verdict::Approved), None)
            .unwrap();

        assert_eq!(fx.ids(false), ["c1"]);
    }

    #[test]
    fn a_closed_review_accepts_thread_writes_and_stays_closed() {
        let fx = fixture();
        fx.reviews.add_thread(&fx.file, 1, 1, "x", "scott").unwrap();
        fx.reviews
            .close(&fx.file, "scott", Some(Verdict::Dropped), None)
            .unwrap();

        fx.reviews
            .add_thread(&fx.file, 2, 2, "y", "claude")
            .unwrap();
        fx.reviews.reply(&fx.file, "c1", "z", "claude").unwrap();

        assert_eq!(state(&fx), ReviewState::Closed(Verdict::Dropped));
    }

    #[test]
    fn either_side_reopens_a_closed_review_on_the_humans_turn() {
        for side in [Side::Human, Side::Agent] {
            let fx = fixture();
            fx.reviews
                .hand_off(&fx.file, Side::Human, "scott", None)
                .unwrap();
            fx.reviews
                .close(&fx.file, "scott", Some(Verdict::Approved), None)
                .unwrap();

            let status = fx
                .reviews
                .reopen_review(&fx.file, side, "someone", Some("Again"))
                .unwrap();

            assert_eq!(state(&fx), ReviewState::Turn(Side::Human));
            assert_eq!(status.state, ReviewState::Turn(Side::Human));
            let latest = status.latest.unwrap();
            assert_eq!(latest.event, Event::Reopen);
            assert_eq!(latest.side, side);
            assert_eq!(latest.note.as_deref(), Some("Again"));
        }
    }

    #[test]
    fn reopening_an_active_review_is_not_closed_and_writes_nothing() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        let bytes = fx.review_json();

        assert!(matches!(
            fx.reviews
                .reopen_review(&fx.file, Side::Agent, "claude", None),
            Err(Error::NotClosed)
        ));
        assert_eq!(fx.review_json(), bytes);
    }

    #[test]
    fn reopen_review_error_precedence_is_author_then_file_then_state() {
        let fx = fixture();
        let missing = fx.file.with_file_name("nope.md");

        assert!(matches!(
            fx.reviews.reopen_review(&missing, Side::Agent, "", None),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            fx.reviews
                .reopen_review(&missing, Side::Agent, "claude", None),
            Err(Error::FileUnreadable { .. })
        ));
        assert!(matches!(
            fx.reviews
                .reopen_review(&fx.file, Side::Agent, "claude", None),
            Err(Error::NotClosed)
        ));
        assert_eq!(fx.review_json(), None);
    }

    #[test]
    fn hand_off_close_and_reopen_each_record_the_revision_they_were_made_against() {
        let fx = fixture();

        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        fs::write(&fx.file, "two\n").unwrap();
        fx.reviews
            .close(&fx.file, "scott", Some(Verdict::Approved), None)
            .unwrap();
        fs::write(&fx.file, "three\n").unwrap();
        fx.reviews
            .reopen_review(&fx.file, Side::Agent, "claude", None)
            .unwrap();

        let review = fx.store.load(&fx.file).unwrap();
        let revisions: Vec<&str> = review.log.iter().map(|e| e.revision.as_str()).collect();
        assert_eq!(revisions, ["r1", "r2", "r3"]);
        for content in ["one\ntwo\nthree\nfour\n", "two\n", "three\n"] {
            assert!(snapshot_of(&fx, content).is_some(), "{content:?}");
        }
    }

    #[test]
    fn view_reports_the_review_status_without_writing() {
        let fx = fixture();
        assert_eq!(
            fx.reviews.view(&fx.file, true).unwrap().review,
            ReviewStatus::default()
        );
        assert_eq!(fx.review_json(), None);

        let handed = fx
            .reviews
            .hand_off(&fx.file, Side::Human, "scott", Some("n"))
            .unwrap();
        let bytes = fx.review_json();

        assert_eq!(
            fx.reviews.view(&fx.file, true).unwrap().review,
            handed.review
        );
        assert_eq!(fx.review_json(), bytes);
    }

    // --- changes since the last look ---

    const EDITED: &str = "one\nTWO\nthree\nfour\nfive\n";

    fn changes_now(fx: &Fixture) -> Vec<Change> {
        fx.reviews.view(&fx.file, false).unwrap().changes
    }

    fn expected_changes(old: &str, new: &str) -> Vec<Change> {
        let old: Vec<&str> = old.lines().collect();
        let new: Vec<&str> = new.lines().collect();
        Diff::new(&old, &new).changes()
    }

    #[test]
    fn a_fresh_review_has_no_changes() {
        let fx = fixture();
        fs::write(&fx.file, EDITED).unwrap();

        assert_eq!(changes_now(&fx), vec![]);
    }

    #[test]
    fn changes_are_since_the_humans_last_hand_off() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        fs::write(&fx.file, EDITED).unwrap();

        let changes = changes_now(&fx);

        assert!(!changes.is_empty());
        assert_eq!(changes, expected_changes("one\ntwo\nthree\nfour\n", EDITED));
    }

    #[test]
    fn an_agent_hand_off_is_not_a_last_look() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        fs::write(&fx.file, EDITED).unwrap();
        fx.reviews
            .hand_off(&fx.file, Side::Agent, "claude", None)
            .unwrap();

        assert_eq!(
            changes_now(&fx),
            expected_changes("one\ntwo\nthree\nfour\n", EDITED)
        );
    }

    #[test]
    fn a_close_is_a_last_look() {
        let fx = fixture();
        fx.reviews
            .close(&fx.file, "scott", Some(Verdict::Approved), None)
            .unwrap();
        fs::write(&fx.file, EDITED).unwrap();

        assert_eq!(
            changes_now(&fx),
            expected_changes("one\ntwo\nthree\nfour\n", EDITED)
        );
    }

    #[test]
    fn a_hand_off_on_the_current_content_has_no_changes() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        fs::write(&fx.file, EDITED).unwrap();
        fx.reviews
            .hand_off(&fx.file, Side::Agent, "claude", None)
            .unwrap();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();

        assert_eq!(changes_now(&fx), vec![]);
    }

    #[test]
    fn a_missing_last_look_snapshot_has_no_changes_and_no_error() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        let dir = fx.store.review_dir(&fx.file).unwrap();
        fs::remove_dir_all(dir.join("revisions")).unwrap();
        fs::write(&fx.file, EDITED).unwrap();

        assert_eq!(changes_now(&fx), vec![]);
    }

    #[test]
    fn viewing_changes_writes_nothing() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        fs::write(&fx.file, EDITED).unwrap();
        let bytes = fx.review_json();

        assert!(!changes_now(&fx).is_empty());
        assert_eq!(fx.review_json(), bytes);
        assert_eq!(snapshot_of(&fx, EDITED), None);
    }

    // --- waiting for the agent's turn ---

    /// A clock that only moves when slept on, by exactly the time asked,
    /// running `on_sleep(n)` on the `n`th sleep (from 1).
    struct FakeTicker<'a> {
        now: std::cell::Cell<std::time::Instant>,
        sleeps: std::cell::Cell<u32>,
        on_sleep: Box<dyn Fn(u32) + 'a>,
    }

    impl<'a> FakeTicker<'a> {
        fn new(on_sleep: impl Fn(u32) + 'a) -> Self {
            Self {
                now: std::cell::Cell::new(std::time::Instant::now()),
                sleeps: std::cell::Cell::new(0),
                on_sleep: Box::new(on_sleep),
            }
        }

        fn idle() -> Self {
            Self::new(|_| {})
        }
    }

    impl Ticker for FakeTicker<'_> {
        fn now(&self) -> std::time::Instant {
            self.now.get()
        }

        fn sleep(&self, duration: std::time::Duration) {
            self.now.set(self.now.get() + duration);
            self.sleeps.set(self.sleeps.get() + 1);
            (self.on_sleep)(self.sleeps.get());
        }
    }

    const SECOND: Option<std::time::Duration> = Some(std::time::Duration::from_secs(1));
    const NO_WAIT: Option<std::time::Duration> = Some(std::time::Duration::ZERO);

    #[test]
    fn a_wait_on_a_closed_review_returns_at_once() {
        let fx = fixture();
        fx.reviews
            .close(&fx.file, "scott", Some(Verdict::Dropped), Some("No"))
            .unwrap();
        let ticker = FakeTicker::idle();

        let waited = fx.reviews.wait(&fx.file, None, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Closed);
        assert_eq!(waited.review.verdict(), Some(Verdict::Dropped));
        assert_eq!(ticker.sleeps.get(), 0);
    }

    #[test]
    fn a_wait_on_the_agents_turn_returns_at_once() {
        let fx = fixture();
        fx.reviews
            .hand_off(&fx.file, Side::Human, "scott", Some("Go"))
            .unwrap();
        let ticker = FakeTicker::idle();

        let waited = fx.reviews.wait(&fx.file, None, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Turn);
        assert_eq!(waited.review.turn(), Some(Side::Agent));
        assert_eq!(ticker.sleeps.get(), 0);
    }

    #[test]
    fn a_zero_timeout_on_the_humans_turn_checks_once() {
        let fx = fixture();
        let ticker = FakeTicker::idle();

        let waited = fx.reviews.wait(&fx.file, NO_WAIT, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Timeout);
        assert_eq!(waited.review, ReviewStatus::default());
        assert_eq!(ticker.sleeps.get(), 0);
    }

    #[test]
    fn a_hand_off_during_the_wait_ends_it_with_that_entry() {
        let fx = fixture();
        fx.reviews
            .add_thread(&fx.file, 1, 1, "Hmm", "scott")
            .unwrap();
        let human = Reviews::with_clock(fx.store.clone(), later_now as fn() -> String);
        let ticker = FakeTicker::new(|n| {
            if n == 2 {
                human
                    .hand_off(&fx.file, Side::Human, "scott", Some("Your go"))
                    .unwrap();
            }
        });

        let waited = fx.reviews.wait(&fx.file, None, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Turn);
        assert_eq!(ticker.sleeps.get(), 2);
        let latest = waited.review.latest.unwrap();
        assert_eq!(latest.event, Event::HandOff);
        assert_eq!(latest.author, "scott");
        assert_eq!(latest.note.as_deref(), Some("Your go"));
        assert_eq!(latest.next_id, 2);
        assert_eq!(latest.revision, "r1");
        assert_eq!(latest.created_at, later_now());
    }

    #[test]
    fn a_close_during_the_wait_ends_it_with_the_verdict() {
        let fx = fixture();
        let human = Reviews::with_clock(fx.store.clone(), later_now as fn() -> String);
        let ticker = FakeTicker::new(|_| {
            human
                .close(&fx.file, "scott", Some(Verdict::Approved), Some("Ship"))
                .unwrap();
        });

        let waited = fx.reviews.wait(&fx.file, None, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Closed);
        assert_eq!(waited.review.verdict(), Some(Verdict::Approved));
        assert_eq!(waited.review.latest.unwrap().note.as_deref(), Some("Ship"));
    }

    #[test]
    fn the_timeout_is_measured_on_the_tickers_clock() {
        let fx = fixture();
        let ticker = FakeTicker::idle();

        let waited = fx.reviews.wait(&fx.file, SECOND, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Timeout);
        assert_eq!(ticker.sleeps.get(), 4, "a poll every {POLL_INTERVAL:?}");
    }

    #[test]
    fn no_timeout_keeps_polling() {
        let fx = fixture();
        let human = Reviews::with_clock(fx.store.clone(), later_now as fn() -> String);
        let ticker = FakeTicker::new(|n| {
            if n == 100 {
                human
                    .hand_off(&fx.file, Side::Human, "scott", None)
                    .unwrap();
            }
        });

        let waited = fx.reviews.wait(&fx.file, None, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Turn);
        assert_eq!(ticker.sleeps.get(), 100);
    }

    #[test]
    fn a_wait_on_a_missing_or_non_utf8_file_is_unreadable() {
        let fx = fixture();
        let binary = fx.file.with_file_name("bin.md");
        fs::write(&binary, [0xff, 0xfe, 0x00]).unwrap();

        for path in [fx.file.with_file_name("nope.md"), binary] {
            assert!(matches!(
                fx.reviews.wait(&path, NO_WAIT, &FakeTicker::idle()),
                Err(Error::FileUnreadable { .. })
            ));
        }
    }

    #[test]
    fn removing_the_file_mid_wait_does_not_fail_it() {
        let fx = fixture();
        let human = Reviews::with_clock(fx.store.clone(), later_now as fn() -> String);
        let ticker = FakeTicker::new(|n| match n {
            1 => fs::remove_file(&fx.file).unwrap(),
            3 => {
                fs::write(&fx.file, "one\n").unwrap();
                human
                    .hand_off(&fx.file, Side::Human, "scott", None)
                    .unwrap();
            }
            _ => {}
        });

        let waited = fx.reviews.wait(&fx.file, None, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Turn);
    }

    fn review_json_path(fx: &Fixture) -> PathBuf {
        fx.store.review_dir(&fx.file).unwrap().join("review.json")
    }

    #[test]
    fn a_wait_rides_out_a_failed_read() {
        let fx = fixture();
        fx.reviews
            .add_thread(&fx.file, 1, 1, "Hmm", "scott")
            .unwrap();
        let path = review_json_path(&fx);
        let saved = fs::read(&path).unwrap();
        let human = Reviews::with_clock(fx.store.clone(), later_now as fn() -> String);
        let ticker = FakeTicker::new(|n| match n {
            1 => {
                fs::remove_file(&path).unwrap();
                fs::create_dir(&path).unwrap();
            }
            3 => {
                fs::remove_dir(&path).unwrap();
                fs::write(&path, &saved).unwrap();
                human
                    .hand_off(&fx.file, Side::Human, "scott", None)
                    .unwrap();
            }
            _ => {}
        });

        let waited = fx.reviews.wait(&fx.file, None, &ticker).unwrap();

        assert_eq!(waited.reason, Woke::Turn);
    }

    #[test]
    fn a_wait_gives_up_when_reads_keep_failing() {
        let fx = fixture();
        fx.reviews
            .add_thread(&fx.file, 1, 1, "Hmm", "scott")
            .unwrap();
        let path = review_json_path(&fx);
        fs::remove_file(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let ticker = FakeTicker::idle();

        let result = fx.reviews.wait(&fx.file, None, &ticker);

        assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
        assert_eq!(ticker.sleeps.get(), WAIT_READ_RETRIES);
    }

    #[test]
    fn a_corrupt_review_ends_a_wait_at_once() {
        let fx = fixture();
        fx.reviews
            .add_thread(&fx.file, 1, 1, "Hmm", "scott")
            .unwrap();
        fs::write(review_json_path(&fx), "{").unwrap();
        let ticker = FakeTicker::idle();

        let result = fx.reviews.wait(&fx.file, None, &ticker);

        assert!(matches!(result, Err(Error::Corrupt { .. })), "{result:?}");
        assert_eq!(ticker.sleeps.get(), 0);
    }

    #[test]
    fn a_wait_never_writes() {
        let fx = fixture();
        fx.reviews
            .wait(&fx.file, SECOND, &FakeTicker::idle())
            .unwrap();
        assert_eq!(fx.review_json(), None);

        fx.reviews
            .add_thread(&fx.file, 1, 1, "Hmm", "scott")
            .unwrap();
        let bytes = fx.review_json();
        fx.reviews
            .wait(&fx.file, SECOND, &FakeTicker::idle())
            .unwrap();
        assert_eq!(fx.review_json(), bytes);
    }

    // --- writes against the content a front end showed ---

    fn sha_now(fx: &Fixture) -> String {
        content_hash(&fs::read(&fx.file).unwrap())
    }

    #[test]
    fn view_carries_the_hash_of_the_content_it_read() {
        let fx = fixture();

        let view = fx.reviews.view(&fx.file, true).unwrap();

        assert_eq!(view.sha256, sha_now(&fx));
    }

    #[test]
    fn writes_against_stale_content_are_file_changed_and_write_nothing() {
        let fx = fixture();
        fx.reviews
            .add_thread(&fx.file, 1, 1, "Hmm", "scott")
            .unwrap();
        let seen = sha_now(&fx);
        fs::write(&fx.file, EDITED).unwrap();
        let bytes = fx.review_json();
        let stale = fx.reviews.against(&seen);

        let results = [
            stale.add_thread(&fx.file, 1, 1, "x", "scott").map(drop),
            stale
                .hand_off(&fx.file, Side::Human, "scott", None)
                .map(drop),
            stale
                .close(&fx.file, "scott", Some(Verdict::Approved), None)
                .map(drop),
        ];

        for result in results {
            assert!(matches!(result, Err(Error::FileChanged)), "{result:?}");
        }
        assert_eq!(fx.review_json(), bytes);
        assert_eq!(snapshot_of(&fx, EDITED), None);
    }

    #[test]
    fn writes_against_the_current_content_behave_as_the_plain_ones() {
        let guarded = fixture();
        let plain = fixture();
        let sha = sha_now(&guarded);
        let against = guarded.reviews.against(&sha);

        assert_eq!(
            against
                .add_thread(&guarded.file, 2, 3, "Why?", "scott")
                .unwrap(),
            plain
                .reviews
                .add_thread(&plain.file, 2, 3, "Why?", "scott")
                .unwrap()
        );
        assert_eq!(
            against
                .hand_off(&guarded.file, Side::Human, "scott", Some("Go"))
                .unwrap(),
            plain
                .reviews
                .hand_off(&plain.file, Side::Human, "scott", Some("Go"))
                .unwrap()
        );
        assert_eq!(
            against
                .close(&guarded.file, "scott", Some(Verdict::Dropped), None)
                .unwrap(),
            plain
                .reviews
                .close(&plain.file, "scott", Some(Verdict::Dropped), None)
                .unwrap()
        );
    }

    #[test]
    fn a_guarded_write_checks_author_and_file_before_file_changed_then_the_rest() {
        let fx = fixture();
        let stale = fx.reviews.against("not the hash");
        let missing = fx.file.with_file_name("nope.md");

        assert!(matches!(
            stale.add_thread(&fx.file, 1, 1, "x", " "),
            Err(Error::EmptyAuthor)
        ));
        assert!(matches!(
            stale.close(&fx.file, "scott", None, None),
            Err(Error::MissingVerdict)
        ));
        assert!(matches!(
            stale.hand_off(&missing, Side::Human, "scott", None),
            Err(Error::FileUnreadable { .. })
        ));
        assert!(matches!(
            stale.add_thread(&fx.file, 9, 9, "x", "scott"),
            Err(Error::FileChanged)
        ));
        assert!(matches!(
            stale.hand_off(&fx.file, Side::Agent, "claude", None),
            Err(Error::FileChanged)
        ));

        let current = sha_now(&fx);
        let fresh = fx.reviews.against(&current);
        assert!(matches!(
            fresh.add_thread(&fx.file, 9, 9, "x", "scott"),
            Err(Error::InvalidRange { .. })
        ));
        assert!(matches!(
            fresh.hand_off(&fx.file, Side::Agent, "claude", None),
            Err(Error::NotYourTurn)
        ));
    }

    #[test]
    fn a_guarded_hand_off_records_the_content_shown_as_the_last_look() {
        let fx = fixture();
        let shown = fx.reviews.view(&fx.file, true).unwrap();

        fx.reviews
            .against(&shown.sha256)
            .hand_off(&fx.file, Side::Human, "scott", None)
            .unwrap();
        fs::write(&fx.file, EDITED).unwrap();

        assert_eq!(
            changes_now(&fx),
            expected_changes("one\ntwo\nthree\nfour\n", EDITED)
        );
    }

    // --- the change stamp ---

    #[test]
    fn the_stamp_changes_when_the_file_or_the_review_changes_and_not_otherwise() {
        let fx = fixture();
        let other = Reviews::with_clock(fx.store.clone(), later_now as fn() -> String);
        let first = fx.reviews.stamp(&fx.file).unwrap();

        assert_eq!(fx.reviews.stamp(&fx.file).unwrap(), first);
        fs::write(&fx.file, EDITED).unwrap();
        let edited = fx.reviews.stamp(&fx.file).unwrap();
        assert_ne!(edited, first);
        other.add_thread(&fx.file, 1, 1, "Hmm", "claude").unwrap();
        let written = fx.reviews.stamp(&fx.file).unwrap();
        assert_ne!(written, edited);
        assert_eq!(fx.reviews.stamp(&fx.file).unwrap(), written);
    }

    #[test]
    fn the_stamp_never_writes() {
        let fx = fixture();
        fx.reviews.stamp(&fx.file).unwrap();
        assert_eq!(fx.review_json(), None);

        fx.reviews
            .add_thread(&fx.file, 1, 1, "Hmm", "scott")
            .unwrap();
        fs::write(&fx.file, EDITED).unwrap();
        let bytes = fx.review_json();
        fx.reviews.stamp(&fx.file).unwrap();
        assert_eq!(fx.review_json(), bytes);
        assert_eq!(snapshot_of(&fx, EDITED), None);
    }

    #[test]
    fn the_stamp_of_a_missing_file_is_unreadable() {
        let fx = fixture();

        assert!(matches!(
            fx.reviews.stamp(&fx.file.with_file_name("nope.md")),
            Err(Error::FileUnreadable { .. })
        ));
    }
}
