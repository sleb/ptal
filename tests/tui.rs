//! The TUI end to end without a terminal: scripted keys through `tui::step`
//! against a real `Reviews` over a temp directory.

use std::fs;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::Command;

use ptal::review::{Reviews, SystemClock};
use ptal::store::Store;
use ptal::tui::app::{App, Event, Mode};
use ptal::tui::render::render;
use ptal::tui::{load, step};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

const COLS: u16 = 60;
const ROWS: u16 = 24;

struct Sandbox {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    file: PathBuf,
    reviews: Reviews<SystemClock>,
}

fn sandbox(content: &str) -> Sandbox {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("plan.md");
    fs::write(&file, content).unwrap();
    let data = tmp.path().join("data");
    let reviews = Reviews::new(Store::new(&data));
    Sandbox {
        _tmp: tmp,
        data,
        file,
        reviews,
    }
}

impl Sandbox {
    fn open(&self) -> App {
        load(&self.reviews, &self.file)
            .unwrap()
            .into_app(&self.file, "scott", (COLS, ROWS))
    }

    /// Feed `keys` through `step`; `Break` stops the script early.
    fn press(&self, app: &mut App, keys: &[&str]) -> ControlFlow<()> {
        for name in keys {
            let event = Event::Key(key(name));
            step(app, event, &self.reviews, &self.file, "scott")?;
        }
        ControlFlow::Continue(())
    }
}

fn key(name: &str) -> KeyEvent {
    let code = match name {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        n if n.chars().count() == 1 => KeyCode::Char(n.chars().next().unwrap()),
        n => panic!("unknown key {n}"),
    };
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn screen(app: &App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();
    terminal.draw(|frame| render(app, frame)).unwrap();
    let buffer = terminal.backend().buffer().clone();
    buffer
        .content()
        .chunks(usize::from(COLS))
        .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

const PLAN: &str = "one\ntwo\nthree\nfour\nfive\n";

#[test]
fn commenting_on_a_line_and_quitting_leaves_a_thread_for_the_cli() {
    let sb = sandbox(PLAN);
    let mut app = sb.open();

    let flow = sb.press(&mut app, &["j", "j", "enter", "h", "i", "enter", "q"]);

    assert_eq!(flow, ControlFlow::Break(()));
    let threads = sb.reviews.view(&sb.file, false).unwrap();
    assert_eq!(threads.len(), 1);
    let thread = &threads[0].thread;
    assert_eq!((thread.anchor.start_line, thread.anchor.end_line), (3, 3));
    assert_eq!(thread.anchor.quote, "three");
    assert_eq!(thread.comments[0].author, "scott");
    assert_eq!(thread.comments[0].body, "hi");

    // The same facts on the real command's output.
    let out = Command::new(env!("CARGO_BIN_EXE_ptal"))
        .args(["comments", sb.file.to_str().unwrap()])
        .env("PTAL_DATA_DIR", &sb.data)
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let anchor = &json["threads"][0]["anchor"];
    assert_eq!(anchor["start_line"], 3);
    assert_eq!(anchor["end_line"], 3);
    assert_eq!(anchor["quote"], "three");
    assert_eq!(json["threads"][0]["comments"][0]["author"], "scott");
    assert_eq!(json["threads"][0]["comments"][0]["body"], "hi");
}

#[test]
fn a_saved_comment_shows_in_the_gutter_and_after_reopening() {
    let sb = sandbox(PLAN);
    let mut app = sb.open();

    let flow = sb.press(&mut app, &["j", "j", "enter", "x", "enter"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    assert!(app.marker(2).is_some());
    assert_eq!(app.status(), Some("added c1"));
    let reopened = sb.open();
    assert!(
        screen(&reopened).contains("● 3│three"),
        "{}",
        screen(&reopened)
    );
    assert!(screen(&reopened).contains("  2│two"));
}

#[test]
fn threads_added_with_a_range_mark_every_line_on_open() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 2, 4, "range", "claude")
        .unwrap();

    let app = sb.open();

    let marked: Vec<bool> = (0..5).map(|i| app.marker(i).is_some()).collect();
    assert_eq!(marked, [false, true, true, true, false]);
}

#[test]
fn a_save_against_a_file_that_shrank_fails_and_keeps_the_draft() {
    let sb = sandbox(PLAN);
    let mut app = sb.open();
    let typing = sb.press(&mut app, &["G", "enter", "o", "k"]);
    assert_eq!(typing, ControlFlow::Continue(()));
    fs::write(&sb.file, "one\ntwo\n").unwrap();

    let flow = sb.press(&mut app, &["enter"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    assert_eq!(
        app.mode(),
        &Mode::Input {
            draft: "ok".into(),
            cursor: 2,
            from_panel: None,
        }
    );
    assert!(app.status().unwrap().contains("invalid line range"));
    assert!(sb.reviews.view(&sb.file, true).unwrap().is_empty());
}

#[test]
fn the_reviewed_file_is_not_modified_by_a_comment_round() {
    let sb = sandbox(PLAN);
    let before = (
        fs::read(&sb.file).unwrap(),
        fs::metadata(&sb.file).unwrap().modified().unwrap(),
    );
    let mut app = sb.open();

    let _ = sb.press(&mut app, &["j", "enter", "n", "o", "enter", "q"]);

    let after = (
        fs::read(&sb.file).unwrap(),
        fs::metadata(&sb.file).unwrap().modified().unwrap(),
    );
    assert_eq!(before, after);
}

/// The comments command's JSON for the sandbox's file, via the real binary.
fn cli_comments(sb: &Sandbox) -> serde_json::Value {
    let out = Command::new(env!("CARGO_BIN_EXE_ptal"))
        .args(["comments", sb.file.to_str().unwrap()])
        .env("PTAL_DATA_DIR", &sb.data)
        .output()
        .unwrap();
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn an_agent_reply_shows_as_waiting_and_is_readable_in_the_panel() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "Is this the right name?", "scott")
        .unwrap();
    sb.reviews
        .reply(&sb.file, "c1", "Renamed it, see the diff.", "claude")
        .unwrap();

    let mut app = sb.open();

    assert!(screen(&app).contains("◆ 3│three"), "{}", screen(&app));
    let _ = sb.press(&mut app, &["j", "j", "enter"]);
    let panel = screen(&app);
    assert!(panel.contains("c1 open"), "{panel}");
    assert!(panel.contains("> three"), "{panel}");
    assert!(panel.contains("Is this the right name?"), "{panel}");
    assert!(panel.contains("claude ·"), "{panel}");
    assert!(panel.contains("Renamed it, see the diff."), "{panel}");
}

#[test]
fn a_from_the_panel_adds_a_second_thread_beside_the_first() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "first", "claude")
        .unwrap();
    let before = (
        fs::read(&sb.file).unwrap(),
        fs::metadata(&sb.file).unwrap().modified().unwrap(),
    );
    let mut app = sb.open();

    let flow = sb.press(&mut app, &["j", "j", "enter", "a", "s", "e", "c", "enter"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    let panel = screen(&app);
    assert!(panel.contains("c2 open"), "{panel}");
    assert!(panel.contains("first"), "{panel}");
    let json = cli_comments(&sb);
    let threads = json["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    for thread in threads {
        assert_eq!(thread["anchor"]["start_line"], 3);
    }
    assert_eq!(threads[1]["comments"][0]["author"], "scott");
    assert_eq!(threads[1]["comments"][0]["body"], "sec");
    let after = (
        fs::read(&sb.file).unwrap(),
        fs::metadata(&sb.file).unwrap().modified().unwrap(),
    );
    assert_eq!(before, after, "the reviewed file is untouched");
}

#[test]
fn a_resolved_thread_shows_hollow_and_is_listed_as_resolved_by_the_agent() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "rename?", "scott")
        .unwrap();
    sb.reviews
        .resolve(&sb.file, "c1", "claude", Some("done"))
        .unwrap();

    let mut app = sb.open();

    assert!(screen(&app).contains("○ 3│three"), "{}", screen(&app));
    let _ = sb.press(&mut app, &["j", "j", "enter"]);
    assert!(
        screen(&app).contains("c1 resolved by claude"),
        "{}",
        screen(&app)
    );
    let _ = sb.press(&mut app, &["esc"]);
    assert_eq!(app.mode(), &Mode::Normal);
}
