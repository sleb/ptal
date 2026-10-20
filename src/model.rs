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
        }
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
}
