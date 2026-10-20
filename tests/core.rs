//! Cross-cutting behavior of the core, driven through the public API only.

use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use ptal::model::{Review, State, Status};
use ptal::review::Reviews;
use ptal::store::Store;

struct Env {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    file: PathBuf,
}

fn env(content: &str) -> Env {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("plan.md");
    fs::write(&file, content).unwrap();
    let data = tmp.path().join("data");
    Env {
        _tmp: tmp,
        data,
        file,
    }
}

#[test]
fn concurrent_writers_all_land_with_unique_ids() {
    const WRITERS: usize = 4;
    const PER_WRITER: usize = 10;
    let env = env("one\ntwo\nthree\n");

    std::thread::scope(|s| {
        for w in 0..WRITERS {
            let (data, file) = (&env.data, &env.file);
            s.spawn(move || {
                // Independent handles, as separate processes would have.
                let reviews = Reviews::new(Store::new(data));
                for n in 0..PER_WRITER {
                    let body = format!("writer {w} comment {n}");
                    reviews.add_thread(file, 1, 3, &body, "tester").unwrap();
                }
            });
        }
    });

    let store = Store::new(&env.data);
    let dir = store.review_dir(&env.file).unwrap();
    let json = fs::read_to_string(dir.join("review.json")).unwrap();
    let review: Review = serde_json::from_str(&json).unwrap();
    assert_eq!(review.threads.len(), WRITERS * PER_WRITER);
    let ids: HashSet<_> = review.threads.iter().map(|t| &t.id).collect();
    assert_eq!(ids.len(), WRITERS * PER_WRITER);
    assert_eq!(review.next_id as usize, WRITERS * PER_WRITER + 1);
}

#[test]
fn a_second_store_over_the_same_dir_sees_the_write() {
    let env = env("one\ntwo\n");
    let writer = Reviews::new(Store::new(&env.data));
    let reader = Reviews::new(Store::new(&env.data));

    writer.add_thread(&env.file, 2, 2, "Why?", "scott").unwrap();

    let views = reader.view(&env.file, false).unwrap().threads;
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].thread.comments[0].body, "Why?");
}

#[test]
fn full_review_round() {
    let env = env("# Plan\nstep one\nstep two\n");
    let reviews = Reviews::new(Store::new(&env.data));

    // Human comments on a line.
    reviews
        .add_thread(&env.file, 2, 2, "Is this needed?", "scott")
        .unwrap();
    assert_eq!(
        reviews.view(&env.file, false).unwrap().threads[0].state,
        State::Current
    );

    // Agent edits the file; the thread goes stale.
    fs::write(&env.file, "# Plan\nstep one, revised\nstep two\n").unwrap();
    assert_eq!(
        reviews.view(&env.file, false).unwrap().threads[0].state,
        State::Outdated
    );

    // Agent resolves with a note; the default view is now empty.
    reviews
        .resolve(&env.file, "c1", "claude", Some("Dropped it."))
        .unwrap();
    assert!(reviews.view(&env.file, false).unwrap().threads.is_empty());

    // Including resolved shows the thread with both comments.
    let all = reviews.view(&env.file, true).unwrap().threads;
    assert_eq!(all.len(), 1);
    let thread = &all[0].thread;
    assert!(matches!(thread.status, Status::Resolved { .. }));
    let authors: Vec<_> = thread.comments.iter().map(|c| c.author.as_str()).collect();
    assert_eq!(authors, ["scott", "claude"]);
}
