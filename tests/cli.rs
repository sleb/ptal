//! The real process contract: streams, exit status, and env handling, driven
//! through the built `ptal` binary. Rendering details are covered in-process
//! in `src/cli.rs`.

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

struct Sandbox {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    file: PathBuf,
}

fn sandbox(content: &str) -> Sandbox {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("plan.md");
    fs::write(&file, content).unwrap();
    let data = tmp.path().join("data");
    Sandbox {
        _tmp: tmp,
        data,
        file,
    }
}

impl Sandbox {
    fn command(&self, args: &[&str]) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ptal"));
        cmd.args(args)
            .env("PTAL_DATA_DIR", &self.data)
            .env_remove("PTAL_AUTHOR");
        cmd
    }

    /// Run with `FILE` replaced by the sandbox file.
    fn run(&self, args: &[&str]) -> Output {
        let file = self.file.to_str().unwrap();
        let args: Vec<&str> = args
            .iter()
            .map(|a| if *a == "FILE" { file } else { a })
            .collect();
        self.command(&args).output().unwrap()
    }

    fn run_json(&self, args: &[&str]) -> serde_json::Value {
        let out = self.run(args);
        assert!(out.status.success(), "{args:?}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{args:?}");
        serde_json::from_slice(&out.stdout).unwrap()
    }
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

fn assert_failure(out: &Output, exit: i32, code: &str) {
    assert_eq!(out.status.code(), Some(exit), "{}", stderr(out));
    assert!(out.stdout.is_empty(), "stdout: {:?}", out.stdout);
    let err = stderr(out);
    assert_eq!(err.lines().count(), 1, "{err:?}");
    let json: serde_json::Value = serde_json::from_str(&err).unwrap();
    assert_eq!(json["error"]["code"], code, "{err}");
}

#[test]
fn add_comments_resolve_round_trip_through_the_binary() {
    let sb = sandbox("one\ntwo\nthree\n");

    let added = sb.run_json(&["add", "FILE", "--lines", "2-3", "Why?", "--as", "scott"]);
    assert_eq!(added["thread"]["id"], "c1");

    let open = sb.run_json(&["comments", "FILE"]);
    assert_eq!(open["threads"][0]["state"], "current");

    let resolved = sb.run_json(&["resolve", "FILE", "c1", "--as", "claude", "--note", "Fixed"]);
    assert_eq!(resolved["thread"]["status"], "resolved");

    let open = sb.run_json(&["comments", "FILE"]);
    assert_eq!(open["threads"].as_array().unwrap().len(), 0);

    let all = sb.run_json(&["comments", "FILE", "--all"]);
    let comments = all["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[1]["body"], "Fixed");
}

#[test]
fn reply_and_reopen_carry_a_conversation_through_the_binary() {
    let sb = sandbox("one\ntwo\n");
    sb.run_json(&["add", "FILE", "--lines", "1", "Why?", "--as", "scott"]);
    sb.run_json(&["resolve", "FILE", "c1", "--as", "claude"]);

    let replied = sb.run_json(&["reply", "FILE", "c1", "Not convinced", "--as", "scott"]);
    assert_eq!(replied["thread"]["status"], "open");
    sb.run_json(&[
        "resolve", "FILE", "c1", "--as", "claude", "--note", "Reworded",
    ]);
    let reopened = sb.run_json(&[
        "reopen", "FILE", "c1", "--as", "scott", "--note", "Still no",
    ]);
    assert_eq!(reopened["thread"]["status"], "open");

    let open = sb.run_json(&["comments", "FILE"]);
    let thread = &open["threads"][0];
    assert_eq!(thread["id"], "c1");
    let comments = thread["comments"].as_array().unwrap();
    let ids: Vec<&str> = comments.iter().map(|c| c["id"].as_str().unwrap()).collect();
    let bodies: Vec<&str> = comments
        .iter()
        .map(|c| c["body"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["c1", "c2", "c3", "c4"]);
    assert_eq!(bodies, ["Why?", "Not convinced", "Reworded", "Still no"]);
}

#[test]
fn reply_without_an_author_is_a_caller_error() {
    let sb = sandbox("one\n");
    sb.run_json(&["add", "FILE", "--lines", "1", "Why?", "--as", "scott"]);

    assert_failure(&sb.run(&["reply", "FILE", "c1", "Hi"]), 2, "missing_author");
}

#[test]
fn every_failure_kind_has_empty_stdout_one_json_line_and_its_exit_code() {
    let sb = sandbox("one\n");

    assert_failure(&sb.run(&["--frobnicate"]), 2, "usage");
    assert_failure(&sb.run(&["comments"]), 2, "usage");
    assert_failure(
        &sb.run(&["add", "FILE", "--lines", "1", "Hi"]),
        2,
        "missing_author",
    );
    assert_failure(
        &sb.run(&["resolve", "FILE", "c9", "--as", "claude"]),
        2,
        "not_found",
    );
    assert_failure(
        &sb.run(&["add", "FILE", "--lines", "5", "Hi", "--as", "x"]),
        2,
        "invalid_range",
    );
    assert_failure(
        &sb.run(&["comments", "/no/such/plan.md"]),
        1,
        "file_unreadable",
    );
}

#[test]
fn ptal_author_stands_in_for_the_as_flag() {
    let sb = sandbox("one\n");
    let file = sb.file.to_str().unwrap();

    let out = sb
        .command(&["add", file, "--lines", "1", "Hi"])
        .env("PTAL_AUTHOR", "claude")
        .output()
        .unwrap();

    assert!(out.status.success(), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["thread"]["comments"][0]["author"], "claude");
}

#[test]
fn help_goes_to_stdout_and_exits_zero() {
    let sb = sandbox("one\n");

    let out = sb.run(&["--help"]);

    assert!(out.status.success());
    assert!(stderr(&out).is_empty());
    assert!(String::from_utf8_lossy(&out.stdout).contains("ptal resolve"));
}

#[test]
fn a_closed_stdout_pipe_ends_quietly() {
    let sb = sandbox("one\n");

    let mut child = sb
        .command(&["--help"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();

    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "", "no panic message on a closed pipe");
}

/// A process with no resolvable data directory.
fn without_data_dir(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ptal"))
        .args(args)
        .env_remove("PTAL_DATA_DIR")
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .env_remove("PTAL_AUTHOR")
        .output()
        .unwrap()
}

#[test]
fn help_and_version_need_no_data_dir() {
    for flag in ["--help", "--version"] {
        let out = without_data_dir(&[flag]);
        assert_eq!(out.status.code(), Some(0), "{flag}: {}", stderr(&out));
        assert_eq!(stderr(&out), "", "{flag}");
    }
}

#[test]
fn usage_errors_need_no_data_dir() {
    let out = without_data_dir(&["comments"]);
    assert_failure(&out, 2, "usage");
}

#[test]
fn a_command_that_needs_the_store_reports_no_data_dir() {
    let sb = sandbox("one\n");
    let file = sb.file.to_str().unwrap();
    let out = without_data_dir(&["comments", file]);
    assert_failure(&out, 1, "no_data_dir");
}

#[test]
fn a_bare_file_with_piped_stdio_is_a_json_error_and_touches_nothing() {
    let sb = sandbox("one\ntwo\n");
    let before = fs::read(&sb.file).unwrap();

    let out = sb
        .command(&[sb.file.to_str().unwrap()])
        .env("USER", "scott")
        .stdin(Stdio::null())
        .output()
        .unwrap();

    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    let stderr = String::from_utf8(out.stderr).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stderr).unwrap();
    assert_eq!(json["error"]["code"], "not_a_terminal");
    assert_eq!(fs::read(&sb.file).unwrap(), before);
    assert!(!sb.data.exists());
}

#[test]
fn a_thread_follows_lines_inserted_above_it_across_processes() {
    let sb = sandbox("one\ntwo\nthree\n");
    sb.run_json(&["add", "FILE", "--lines", "2-3", "Why?", "--as", "scott"]);

    fs::write(&sb.file, "intro\n\none\ntwo\nthree\n").unwrap();

    let listed = sb.run_json(&["comments", "FILE"]);
    let thread = &listed["threads"][0];
    assert_eq!(thread["anchor"]["start_line"], 4);
    assert_eq!(thread["anchor"]["end_line"], 5);
    assert_eq!(thread["state"], "current");

    let replied = sb.run_json(&["reply", "FILE", "c1", "Because.", "--as", "claude"]);
    assert_eq!(replied["thread"]["anchor"]["start_line"], 4);
    assert_eq!(replied["thread"]["anchor"]["end_line"], 5);
    assert_eq!(sb.run_json(&["comments", "FILE"])["revision"], "r2");
}

#[test]
fn turns_pass_between_the_tui_and_separate_cli_processes() {
    let sb = sandbox("one\ntwo\n");
    assert_failure(
        &sb.run(&["handoff", "FILE", "--as", "claude"]),
        2,
        "not_your_turn",
    );

    // The human hands off as the TUI does, through the library.
    let reviews = ptal::review::Reviews::new(ptal::store::Store::new(&sb.data));
    reviews
        .hand_off(
            &sb.file,
            ptal::model::Side::Human,
            "scott",
            Some("Over to you"),
        )
        .unwrap();
    let seen = sb.run_json(&["comments", "FILE"]);
    assert_eq!(seen["review"]["turn"], "agent");
    assert_eq!(seen["review"]["latest"]["note"], "Over to you");
    assert_eq!(seen["review"]["latest"]["revision"], seen["revision"]);

    fs::write(&sb.file, "one\ntwo\nthree\n").unwrap();
    let handed = sb.run_json(&["handoff", "FILE", "--as", "claude", "--note", "Added three"]);
    assert_eq!(handed["review"]["turn"], "human");

    let back = sb.run_json(&["comments", "FILE"]);
    assert_eq!(back["review"]["turn"], "human");
    assert_eq!(back["review"]["latest"]["author"], "claude");
    assert_eq!(back["review"]["latest"]["note"], "Added three");
    assert_eq!(back["review"]["latest"]["revision"], "r2");
    assert_eq!(back["revision"], "r2");
}

#[test]
fn a_waiting_process_wakes_on_a_hand_off_from_another_process() {
    let sb = sandbox("one\n");
    let file = sb.file.to_str().unwrap();
    let check = sb.run_json(&["wait", "FILE", "--timeout", "0"]);
    assert_eq!(check["reason"], "timeout", "starts on the human's turn");
    // Bounded, so a broken wake-up fails as "timeout" instead of hanging.
    let mut waiter = sb
        .command(&["wait", file, "--timeout", "10"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        waiter.try_wait().unwrap().is_none(),
        "still waiting before the hand-off"
    );

    sb.run_json(&[
        "handoff", "FILE", "--side", "human", "--as", "scott", "--note", "Go",
    ]);

    let start = std::time::Instant::now();
    let out = waiter.wait_with_output().unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert!(out.status.success(), "{}", stderr(&out));
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["reason"], "turn");
    assert_eq!(json["review"]["latest"]["note"], "Go");
}

#[test]
fn a_wait_on_a_closed_review_returns_at_once() {
    let sb = sandbox("one\n");
    ptal::review::Reviews::new(ptal::store::Store::new(&sb.data))
        .close(&sb.file, "scott", Some(ptal::model::Verdict::Dropped), None)
        .unwrap();

    let start = std::time::Instant::now();
    let json = sb.run_json(&["wait", "FILE", "--timeout", "10"]);

    assert!(start.elapsed() < std::time::Duration::from_secs(5));
    assert_eq!(json["reason"], "closed");
    assert_eq!(json["review"]["verdict"], "dropped");
}

#[test]
fn skill_prints_the_bundled_skill() {
    let sb = sandbox("one\n");
    let plain = sb.run(&["skill"]);
    assert!(plain.status.success(), "{}", stderr(&plain));
    assert_eq!(stderr(&plain), "");
    let text = String::from_utf8(plain.stdout.clone()).unwrap();
    assert!(text.starts_with("---\n"), "{text}");
    assert!(text.contains("\nname: ptal-review\n"), "{text}");

    // Raw markdown with `--json` and when piped (the test's stdout is a pipe).
    let json = sb.run(&["skill", "--json"]);
    assert_eq!(json.stdout, plain.stdout);
}

#[test]
fn skill_takes_no_arguments() {
    let sb = sandbox("one\n");
    assert_failure(&sb.run(&["skill", "extra"]), 2, "usage");
}

#[test]
fn skill_needs_no_data_dir() {
    let out = Command::new(env!("CARGO_BIN_EXE_ptal"))
        .arg("skill")
        .env_remove("PTAL_DATA_DIR")
        .env_remove("XDG_DATA_HOME")
        .env_remove("HOME")
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
}
