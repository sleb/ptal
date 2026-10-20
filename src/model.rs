//! Plain data types that are persisted and exposed to agents.
//!
//! The serde representation is both the on-disk format and the wire format.
//! Lines are 1-based and inclusive. Timestamps are RFC 3339 UTC strings.
//! The `revision` fields are `None` only on data written before revisions
//! existed.

use serde::{Deserialize, Serialize};

/// The `schema_version` written to, and the newest accepted from, disk.
pub const SCHEMA_VERSION: u32 = 1;

/// All comments for one reviewed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Review {
    pub schema_version: u32,
    /// Path of the reviewed file as it was given when the review was created.
    pub path: String,
    /// Next comment id to hand out; monotonic and never reused.
    pub next_id: u32,
    /// Every revision written against, in the order first seen. Never pruned.
    #[serde(default)]
    pub revisions: Vec<Revision>,
    pub threads: Vec<Thread>,
    /// Hand-offs, closes and reopens, oldest first. Append-only: entries are
    /// only ever pushed. The review's [`ReviewState`] is derived from it.
    #[serde(default)]
    pub log: Vec<Entry>,
}

impl Review {
    /// An empty review of `path` at the current schema version.
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            path: path.into(),
            next_id: 1,
            revisions: Vec::new(),
            threads: Vec::new(),
            log: Vec::new(),
        }
    }

    /// Whose turn it is, or the verdict if closed: derived from the
    /// [`log`](Review::log), never stored.
    ///
    /// A review starts on the human's turn. A hand-off by one side gives the
    /// turn to the other, a close ends it with its verdict, and a reopen
    /// gives it back to the human. Each entry sets the state outright, so it
    /// is the latest entry's.
    pub fn state(&self) -> ReviewState {
        match self.log.last() {
            None => ReviewState::Turn(Side::Human),
            Some(entry) => match entry.event {
                Event::HandOff => ReviewState::Turn(entry.side.other()),
                Event::Close { verdict } => ReviewState::Closed(verdict),
                Event::Reopen => ReviewState::Turn(Side::Human),
            },
        }
    }

    /// The human's last look at the file: the latest [`log`](Review::log)
    /// entry written by [`Side::Human`] that is a hand-off or a close, or
    /// `None` before the first one.
    ///
    /// Its revision is what the human last saw. A reopen is not a look, and
    /// neither is opening the file: reads never write.
    pub fn last_look(&self) -> Option<&Entry> {
        self.log.iter().rev().find(|entry| {
            entry.side == Side::Human && matches!(entry.event, Event::HandOff | Event::Close { .. })
        })
    }

    /// The id of the revision whose content hashes to `sha256`, assigning
    /// the next ordinal (`r1`, `r2`, ...) if it is new.
    ///
    /// Going back to earlier content returns that earlier id.
    pub fn revision_for(&mut self, sha256: &str) -> String {
        if let Some(id) = self.revision_id(sha256) {
            return id.to_string();
        }
        let id = format!("r{}", self.revisions.len() + 1);
        self.revisions.push(Revision {
            id: id.clone(),
            sha256: sha256.to_string(),
        });
        id
    }

    /// The id of the revision whose content hashes to `sha256`, if any write
    /// has been made against it.
    pub fn revision_id(&self, sha256: &str) -> Option<&str> {
        self.revisions
            .iter()
            .find(|r| r.sha256 == sha256)
            .map(|r| r.id.as_str())
    }

    /// The content hash of revision `id`.
    pub fn revision_sha(&self, id: &str) -> Option<&str> {
        self.revisions
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.sha256.as_str())
    }

    /// Allocate the next comment id (`c1`, `c2`, ...).
    ///
    /// Threads and their comments draw from the same counter, so an id is
    /// unique within the review and is never handed out twice.
    pub fn next_comment_id(&mut self) -> String {
        let id = format!("c{}", self.next_id);
        self.next_id += 1;
        id
    }
}

/// One kept version of the reviewed file. Its content is snapshotted by the
/// store under `sha256`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    /// `r1`, `r2`, ... in the order first written against.
    pub id: String,
    /// Lowercase hex SHA-256 of the file's raw bytes.
    pub sha256: String,
}

/// A conversation anchored to a line range. Its id is its first comment's id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub anchor: Anchor,
    #[serde(flatten)]
    pub status: Status,
    pub comments: Vec<Comment>,
}

/// Whether a thread is still awaiting action.
///
/// On the wire this is `"status": "open" | "resolved"`, with `resolved_by`,
/// `resolved_at` and `resolved_revision` present only when resolved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum Status {
    Open,
    Resolved {
        resolved_by: String,
        resolved_at: String,
        /// The revision the resolve was made against.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resolved_revision: Option<String>,
    },
}

/// One message in a thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub created_at: String,
    /// The revision the comment was written against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// The lines a thread is attached to, and their text when it was created.
///
/// Immutable once written: front ends see it mapped to the current file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Anchor {
    /// The revision the lines refer to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
    /// 1-based, inclusive.
    pub start_line: u32,
    /// 1-based, inclusive.
    pub end_line: u32,
    /// The anchored lines joined by `\n`.
    pub quote: String,
}

/// A party to a review: the human (in the TUI) or the agent (through the
/// CLI). Which side acts is set by the front end, never guessed from an
/// author name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Human,
    Agent,
}

impl Side {
    /// The side's name, as on the wire: `human` or `agent`.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Human => "human",
            Side::Agent => "agent",
        }
    }

    /// The side across the review from this one.
    pub fn other(self) -> Self {
        match self {
            Side::Human => Side::Agent,
            Side::Agent => Side::Human,
        }
    }
}

/// How the human ended a review: what the agent does next, outside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    /// The agent takes the next step with the file as it stands.
    Approved,
    /// The agent doesn't.
    Dropped,
}

impl Verdict {
    /// The verdict's name, as on the wire: `approved` or `dropped`.
    pub fn as_str(self) -> &'static str {
        match self {
            Verdict::Approved => "approved",
            Verdict::Dropped => "dropped",
        }
    }
}

/// One entry of a review's [`log`](Review::log).
///
/// On the wire, `"event": "hand_off" | "close" | "reopen"`, with `verdict`
/// present only on a close and `note` only when there is one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    #[serde(flatten)]
    pub event: Event,
    /// The side that wrote the entry.
    pub side: Side,
    pub author: String,
    pub created_at: String,
    /// The revision of the file the entry was written against.
    pub revision: String,
    /// The review's `next_id` when the entry was written: every comment id
    /// below it came before the entry.
    pub next_id: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// What a log [`Entry`] records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// The side holding the turn passed it to the other.
    HandOff,
    /// The human ended the review.
    Close { verdict: Verdict },
    /// A closed review was brought back, on the human's turn.
    Reopen,
}

/// A review's overall state, derived by [`Review::state`]. Never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewState {
    /// Active, with this side expected to act next.
    Turn(Side),
    /// Closed with this verdict; nobody holds the turn.
    Closed(Verdict),
}

/// Whether an anchor still matches the file. Computed, never persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum State {
    /// The anchored lines are unchanged in the current file.
    Current,
    /// An anchored line was edited or removed.
    Outdated,
}
#[cfg(test)]
mod tests {
    use super::*;

    fn resolved_review() -> Review {
        let mut review = Review::new("/plans/plan.md");
        assert_eq!(review.revision_for("aa11"), "r1");
        assert_eq!(review.revision_for("bb22"), "r2");
        let open = Thread {
            id: review.next_comment_id(),
            anchor: Anchor {
                revision: Some("r1".into()),
                start_line: 2,
                end_line: 3,
                quote: "two\nthree".into(),
            },
            status: Status::Open,
            comments: vec![],
        };
        let mut open = open;
        open.comments.push(Comment {
            id: open.id.clone(),
            author: "scott".into(),
            body: "Why?".into(),
            created_at: "2026-10-12T10:00:00Z".into(),
            revision: Some("r1".into()),
        });
        review.threads.push(open);

        let id = review.next_comment_id();
        let note_id = review.next_comment_id();
        review.threads.push(Thread {
            id: id.clone(),
            anchor: Anchor {
                revision: Some("r1".into()),
                start_line: 1,
                end_line: 1,
                quote: "one".into(),
            },
            status: Status::Resolved {
                resolved_by: "claude".into(),
                resolved_at: "2026-10-12T11:00:00Z".into(),
                resolved_revision: Some("r2".into()),
            },
            comments: vec![
                Comment {
                    id,
                    author: "scott".into(),
                    body: "Typo".into(),
                    created_at: "2026-10-12T10:05:00Z".into(),
                    revision: Some("r1".into()),
                },
                Comment {
                    id: note_id,
                    author: "claude".into(),
                    body: "Fixed".into(),
                    created_at: "2026-10-12T11:00:00Z".into(),
                    revision: Some("r2".into()),
                },
            ],
        });
        review.log = vec![
            Entry {
                event: Event::HandOff,
                side: Side::Human,
                author: "scott".into(),
                created_at: "2026-10-12T10:10:00Z".into(),
                revision: "r1".into(),
                next_id: 3,
                note: Some("Over to you".into()),
            },
            Entry {
                event: Event::Close {
                    verdict: Verdict::Approved,
                },
                side: Side::Human,
                author: "scott".into(),
                created_at: "2026-10-12T12:00:00Z".into(),
                revision: "r2".into(),
                next_id: 4,
                note: Some("Ship it".into()),
            },
            Entry {
                event: Event::Reopen,
                side: Side::Agent,
                author: "claude".into(),
                created_at: "2026-10-12T13:00:00Z".into(),
                revision: "r2".into(),
                next_id: 4,
                note: None,
            },
        ];
        review
    }

    #[test]
    fn review_round_trips_through_json() {
        let review = resolved_review();

        let json = serde_json::to_string(&review).unwrap();
        let back: Review = serde_json::from_str(&json).unwrap();

        assert_eq!(back, review);
    }

    #[test]
    fn wire_shape_is_pinned_by_golden_json() {
        let json = serde_json::to_string_pretty(&resolved_review()).unwrap();

        let golden = r#"{
  "schema_version": 1,
  "path": "/plans/plan.md",
  "next_id": 4,
  "revisions": [
    {
      "id": "r1",
      "sha256": "aa11"
    },
    {
      "id": "r2",
      "sha256": "bb22"
    }
  ],
  "threads": [
    {
      "id": "c1",
      "anchor": {
        "revision": "r1",
        "start_line": 2,
        "end_line": 3,
        "quote": "two\nthree"
      },
      "status": "open",
      "comments": [
        {
          "id": "c1",
          "author": "scott",
          "body": "Why?",
          "created_at": "2026-10-12T10:00:00Z",
          "revision": "r1"
        }
      ]
    },
    {
      "id": "c2",
      "anchor": {
        "revision": "r1",
        "start_line": 1,
        "end_line": 1,
        "quote": "one"
      },
      "status": "resolved",
      "resolved_by": "claude",
      "resolved_at": "2026-10-12T11:00:00Z",
      "resolved_revision": "r2",
      "comments": [
        {
          "id": "c2",
          "author": "scott",
          "body": "Typo",
          "created_at": "2026-10-12T10:05:00Z",
          "revision": "r1"
        },
        {
          "id": "c3",
          "author": "claude",
          "body": "Fixed",
          "created_at": "2026-10-12T11:00:00Z",
          "revision": "r2"
        }
      ]
    }
  ],
  "log": [
    {
      "event": "hand_off",
      "side": "human",
      "author": "scott",
      "created_at": "2026-10-12T10:10:00Z",
      "revision": "r1",
      "next_id": 3,
      "note": "Over to you"
    },
    {
      "event": "close",
      "verdict": "approved",
      "side": "human",
      "author": "scott",
      "created_at": "2026-10-12T12:00:00Z",
      "revision": "r2",
      "next_id": 4,
      "note": "Ship it"
    },
    {
      "event": "reopen",
      "side": "agent",
      "author": "claude",
      "created_at": "2026-10-12T13:00:00Z",
      "revision": "r2",
      "next_id": 4
    }
  ]
}"#;
        assert_eq!(json, golden);
    }

    #[test]
    fn new_review_carries_current_schema_version() {
        assert_eq!(Review::new("/x").schema_version, SCHEMA_VERSION);
        assert_eq!(SCHEMA_VERSION, 1);
    }

    #[test]
    fn comment_ids_are_sequential_from_c1() {
        let mut review = Review::new("/x");

        assert_eq!(review.next_comment_id(), "c1");
        assert_eq!(review.next_comment_id(), "c2");
        assert_eq!(review.next_comment_id(), "c3");
    }

    #[test]
    fn id_counter_survives_a_round_trip_and_never_repeats() {
        let mut review = Review::new("/x");
        review.next_comment_id();
        review.next_comment_id();

        let json = serde_json::to_string(&review).unwrap();
        let mut back: Review = serde_json::from_str(&json).unwrap();

        assert_eq!(back.next_comment_id(), "c3");
    }

    #[test]
    fn review_without_revision_fields_still_parses() {
        let json = r#"{
  "schema_version": 1,
  "path": "/x",
  "next_id": 3,
  "threads": [
    {
      "id": "c1",
      "anchor": { "start_line": 1, "end_line": 1, "quote": "one" },
      "status": "resolved",
      "resolved_by": "claude",
      "resolved_at": "2026-10-12T11:00:00Z",
      "comments": [
        { "id": "c1", "author": "scott", "body": "x", "created_at": "2026-10-12T10:00:00Z" }
      ]
    }
  ]
}"#;

        let review: Review = serde_json::from_str(json).unwrap();

        assert!(review.revisions.is_empty());
        assert!(review.log.is_empty());
        let thread = &review.threads[0];
        assert_eq!(thread.anchor.revision, None);
        assert_eq!(thread.comments[0].revision, None);
        assert!(matches!(
            &thread.status,
            Status::Resolved {
                resolved_revision: None,
                ..
            }
        ));
        let back = serde_json::to_string(&thread).unwrap();
        assert!(!back.contains("revision"), "{back}");
    }

    #[test]
    fn revision_for_hands_out_ordinals_and_reuses_a_known_hash() {
        let mut review = Review::new("/x");

        assert_eq!(review.revision_for("a"), "r1");
        assert_eq!(review.revision_for("b"), "r2");
        assert_eq!(review.revision_for("a"), "r1");
        assert_eq!(review.revision_id("b"), Some("r2"));
        assert_eq!(review.revision_id("c"), None);
        assert_eq!(review.revision_sha("r1"), Some("a"));
        assert_eq!(review.revisions.len(), 2);
    }

    fn entry(event: Event, side: Side) -> Entry {
        Entry {
            event,
            side,
            author: "x".into(),
            created_at: "2026-10-12T10:00:00Z".into(),
            revision: "r1".into(),
            next_id: 1,
            note: None,
        }
    }

    #[test]
    fn state_is_a_fold_over_the_log() {
        use ReviewState::{Closed, Turn};
        let hand_off = |side| entry(Event::HandOff, side);
        let close = |verdict| entry(Event::Close { verdict }, Side::Human);
        let reopen = |side| entry(Event::Reopen, side);
        let cases: Vec<(Vec<Entry>, ReviewState)> = vec![
            (vec![], Turn(Side::Human)),
            (vec![hand_off(Side::Human)], Turn(Side::Agent)),
            (
                vec![hand_off(Side::Human), hand_off(Side::Agent)],
                Turn(Side::Human),
            ),
            (vec![close(Verdict::Approved)], Closed(Verdict::Approved)),
            (
                vec![hand_off(Side::Human), close(Verdict::Dropped)],
                Closed(Verdict::Dropped),
            ),
            (
                vec![close(Verdict::Approved), reopen(Side::Agent)],
                Turn(Side::Human),
            ),
            (
                vec![
                    hand_off(Side::Human),
                    close(Verdict::Dropped),
                    reopen(Side::Human),
                ],
                Turn(Side::Human),
            ),
        ];

        for (log, expected) in cases {
            let review = Review {
                log: log.clone(),
                ..Review::new("/x")
            };
            assert_eq!(review.state(), expected, "{log:?}");
        }
    }

    #[test]
    fn last_look_is_the_latest_human_hand_off_or_close() {
        let hand_off = |side| entry(Event::HandOff, side);
        let close = || {
            entry(
                Event::Close {
                    verdict: Verdict::Approved,
                },
                Side::Human,
            )
        };
        let reopen = |side| entry(Event::Reopen, side);
        // Each case is a log and the index of the entry that is the last look.
        let cases: Vec<(Vec<Entry>, Option<usize>)> = vec![
            (vec![], None),
            (vec![hand_off(Side::Agent)], None),
            (vec![hand_off(Side::Human)], Some(0)),
            (vec![hand_off(Side::Human), hand_off(Side::Agent)], Some(0)),
            (vec![close()], Some(0)),
            (vec![close(), reopen(Side::Human)], Some(0)),
            (
                vec![
                    hand_off(Side::Human),
                    hand_off(Side::Agent),
                    hand_off(Side::Human),
                ],
                Some(2),
            ),
        ];

        for (log, expected) in cases {
            let log: Vec<Entry> = log
                .into_iter()
                .enumerate()
                .map(|(i, e)| Entry {
                    revision: format!("r{}", i + 1),
                    ..e
                })
                .collect();
            let review = Review {
                log: log.clone(),
                ..Review::new("/x")
            };
            assert_eq!(review.last_look(), expected.map(|i| &log[i]), "{log:?}");
        }
    }

    #[test]
    fn names_are_the_wire_names() {
        for side in [Side::Human, Side::Agent] {
            assert_eq!(serde_json::to_value(side).unwrap(), side.as_str());
        }
        for verdict in [Verdict::Approved, Verdict::Dropped] {
            assert_eq!(serde_json::to_value(verdict).unwrap(), verdict.as_str());
        }
    }

    #[test]
    fn the_other_side_of_each_side() {
        assert_eq!(Side::Human.other(), Side::Agent);
        assert_eq!(Side::Agent.other(), Side::Human);
    }
}
