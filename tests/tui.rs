//! The TUI end to end without a terminal: scripted keys through `tui::step`
//! against a real `Reviews` over a temp directory.

use std::fs;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::process::Command;

use ptal::review::{Reviews, SystemClock};
use ptal::store::Store;
use ptal::tui::app::{App, Back, Event, Mode};
use ptal::tui::render::render;
use ptal::tui::{load, step};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::style::{Color, Modifier};

const COLS: u16 = 60;
const ROWS: u16 = 24;

struct Sandbox {
    _tmp: tempfile::TempDir,
    data: PathBuf,
    file: PathBuf,
    reviews: Reviews<SystemClock>,
}

fn sandbox(content: &str) -> Sandbox {
    sandbox_named("plan.md", content)
}

fn sandbox_named(name: &str, content: &str) -> Sandbox {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join(name);
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
    /// [`Sandbox::press`] `before`, then type `text`, then `after`.
    fn press_typing(
        &self,
        app: &mut App,
        before: &[&str],
        text: &str,
        after: &[&str],
    ) -> ControlFlow<()> {
        let typed = typing(text);
        let mut keys: Vec<&str> = before.to_vec();
        keys.extend(typed.iter().map(String::as_str));
        keys.extend(after);
        self.press(app, &keys)
    }

    fn open(&self) -> App {
        self.open_with(true)
    }

    fn open_with(&self, color: bool) -> App {
        load(&self.reviews, &self.file)
            .unwrap()
            .into_app(&self.file, "scott", (COLS, ROWS), color)
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
    if let Some(c) = name.strip_prefix("ctrl+") {
        let c = c.chars().next().unwrap();
        return KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL);
    }
    let code = match name {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "backspace" => KeyCode::Backspace,
        n if n.chars().count() == 1 => KeyCode::Char(n.chars().next().unwrap()),
        n => panic!("unknown key {n}"),
    };
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// The keys that type `text` into a comment box, each `\n` as `enter`.
fn typing(text: &str) -> Vec<String> {
    text.chars()
        .map(|c| match c {
            '\n' => "enter".to_string(),
            c => c.to_string(),
        })
        .collect()
}

fn drawn(app: &App) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(COLS, ROWS)).unwrap();
    terminal.draw(|frame| render(app, frame)).unwrap();
    terminal.backend().buffer().clone()
}

fn screen(app: &App) -> String {
    let buffer = drawn(app);
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

    let flow = sb.press(&mut app, &["j", "j", "enter", "h", "i", "ctrl+s", "q"]);

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

    let flow = sb.press(&mut app, &["j", "j", "enter", "x", "ctrl+s"]);

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
fn a_long_prose_line_is_readable_in_full_across_wrapped_rows() {
    let words: Vec<String> = (0..30).map(|i| format!("word{i:02}")).collect();
    let long = words.join(" ");
    assert!(long.len() > 200);
    let sb = sandbox(&format!("intro\n{long}\noutro\n"));

    let text = screen(&sb.open());

    for word in &words {
        assert!(text.contains(word.as_str()), "{word} missing from\n{text}");
    }
    assert!(text.contains("outro"));
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

    let flow = sb.press(&mut app, &["ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    assert!(
        matches!(
            app.mode(),
            Mode::Input { draft, back: Back::Normal, reply_to: None }
                if draft.text() == "ok" && draft.cursor() == 2
        ),
        "{:?}",
        app.mode()
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

    let _ = sb.press(&mut app, &["j", "enter", "n", "o", "ctrl+s", "q"]);

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
fn a_table_shows_aligned_and_a_comment_on_a_row_anchors_to_its_raw_line() {
    let sb = sandbox("intro\n| Step | Owner |\n|---|---|\n| build | ci |\noutro\n");
    let mut app = sb.open();

    let text = screen(&app);
    for row in [
        "  2│ Step  │ Owner",
        "  3│───────┼───────",
        "  4│ build │ ci",
    ] {
        assert!(text.contains(row), "{row:?} missing from\n{text}");
    }

    let flow = sb.press(&mut app, &["j", "j", "j", "enter", "h", "i", "ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    let json = cli_comments(&sb);
    let anchor = &json["threads"][0]["anchor"];
    assert_eq!(anchor["start_line"], 4);
    assert_eq!(anchor["end_line"], 4);
    assert_eq!(anchor["quote"], "| build | ci |");
}

#[test]
fn a_wide_tables_long_cell_wraps_in_its_column_and_a_comment_quotes_the_raw_line() {
    let words: Vec<String> = (0..40).map(|i| format!("word{i:02}")).collect();
    let long = words.join(" ");
    assert!(long.len() > 250);
    let row = format!("| 1 | {long} |");
    let sb = sandbox(&format!(
        "intro\n| Q | Decision |\n|---|---|\n{row}\noutro\n"
    ));
    let mut app = sb.open();

    let text = screen(&app);
    let wrapped: Vec<&str> = text.lines().filter(|l| l.contains("word")).collect();
    assert!(wrapped.len() > 1, "the cell wraps:\n{text}");
    for line in &wrapped {
        assert!(
            line.contains(" │ "),
            "{line:?} is not in its column\n{text}"
        );
        assert!(!line.contains('|'), "{line:?} shows raw\n{text}");
    }
    for word in &words {
        assert!(text.contains(word.as_str()), "{word} missing from\n{text}");
    }

    let flow = sb.press(&mut app, &["j", "j", "j", "enter", "h", "i", "ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    let json = cli_comments(&sb);
    let anchor = &json["threads"][0]["anchor"];
    assert_eq!(anchor["start_line"], 4);
    assert_eq!(anchor["end_line"], 4);
    assert_eq!(anchor["quote"], row.as_str());
}

#[test]
fn a_ragged_table_shows_its_raw_text() {
    let sb = sandbox("| a | b |\n|---|---|\n| 1 | 2 | 3 |\n");

    let text = screen(&sb.open());

    for (n, line) in ["| a | b |", "|---|---|", "| 1 | 2 | 3 |"]
        .iter()
        .enumerate()
    {
        let row = format!("  {}│{line}", n + 1);
        assert!(text.contains(&row), "{row:?} missing from\n{text}");
    }
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

    let flow = sb.press(&mut app, &["j", "j", "enter", "a", "s", "e", "c", "ctrl+s"]);

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

#[test]
fn r_replies_to_the_selected_thread_and_the_comment_lands_last() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "rename?", "scott")
        .unwrap();
    sb.reviews
        .reply(&sb.file, "c1", "Renamed it.", "claude")
        .unwrap();
    let mut app = sb.open();
    assert!(screen(&app).contains("◆ 3│three"), "{}", screen(&app));

    let flow = sb.press(&mut app, &["j", "j", "enter", "r", "t", "h", "x", "ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    let json = cli_comments(&sb);
    let comments = json["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 3);
    assert_eq!(comments[2]["author"], "scott");
    assert_eq!(comments[2]["body"], "thx");
    let panel = screen(&app);
    assert!(matches!(app.mode(), Mode::Panel { .. }), "{panel}");
    assert!(panel.contains("thx"), "{panel}");
    assert!(panel.contains("replied c1"), "{panel}");
    let _ = sb.press(&mut app, &["esc"]);
    assert!(screen(&app).contains("● 3│three"), "{}", screen(&app));
}

#[cfg(unix)]
#[test]
fn a_failed_reply_keeps_the_draft_and_shows_the_error() {
    use std::os::unix::fs::PermissionsExt;

    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "rename?", "claude")
        .unwrap();
    let mut app = sb.open();
    let _ = sb.press(&mut app, &["j", "j", "enter", "r", "o", "k"]);
    // A read-only store directory makes the write fail without touching
    // the thread, so the failure is the store's, not the model's.
    let mut dirs = vec![sb.data.clone()];
    let mut all = vec![];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            if entry.file_type().unwrap().is_dir() {
                dirs.push(entry.path());
            }
        }
        all.push(dir);
    }
    for dir in &all {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o555)).unwrap();
    }

    let flow = sb.press(&mut app, &["ctrl+s"]);

    for dir in &all {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755)).unwrap();
    }
    assert_eq!(flow, ControlFlow::Continue(()));
    assert!(
        matches!(app.mode(), Mode::Input { draft, reply_to: Some(id), .. } if draft.text() == "ok" && id == "c1"),
        "{:?}",
        app.mode()
    );
    let status = app.status().unwrap();
    assert!(status.to_lowercase().contains("permission"), "{status}");
    assert_eq!(
        cli_comments(&sb)["threads"][0]["comments"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn x_resolves_the_selected_thread_and_it_leaves_the_default_listing() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "rename?", "claude")
        .unwrap();
    let mut app = sb.open();

    let flow = sb.press(&mut app, &["j", "j", "enter", "x"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    assert_eq!(cli_comments(&sb)["threads"].as_array().unwrap().len(), 0);
    let all = sb.reviews.view(&sb.file, true).unwrap();
    assert!(
        matches!(&all[0].thread.status, ptal::model::Status::Resolved { resolved_by, .. } if resolved_by == "scott"),
        "{:?}",
        all[0].thread.status
    );
    let panel = screen(&app);
    assert!(panel.contains("c1 resolved by scott"), "{panel}");
    assert!(panel.contains("resolved c1"), "{panel}");
    assert!(panel.contains("o:reopen"), "{panel}");
    let _ = sb.press(&mut app, &["esc"]);
    assert!(screen(&app).contains("○ 3│three"), "{}", screen(&app));
}

#[test]
fn o_reopens_a_thread_the_agent_resolved_and_a_reply_reopens_too() {
    let sb = sandbox(PLAN);
    for line in [2, 4] {
        sb.reviews
            .add_thread(&sb.file, line, line, "q", "scott")
            .unwrap();
    }
    sb.reviews.resolve(&sb.file, "c1", "claude", None).unwrap();
    sb.reviews.resolve(&sb.file, "c2", "claude", None).unwrap();
    let mut app = sb.open();

    // o on line 2
    let _ = sb.press(&mut app, &["j", "enter", "o"]);
    assert!(screen(&app).contains("c1 open"), "{}", screen(&app));
    let _ = sb.press(&mut app, &["esc"]);
    assert!(screen(&app).contains("● 2│"), "{}", screen(&app));
    // r on line 4 (resolved) reopens
    let _ = sb.press(&mut app, &["j", "j", "enter", "r", "y", "ctrl+s"]);

    let json = cli_comments(&sb);
    let threads = json["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    for t in threads {
        assert_eq!(t["status"], "open");
        assert!(t.get("resolved_by").is_none() || t["resolved_by"].is_null());
    }
}

/// `(comment count, status)` of every thread in the CLI's `--all` listing.
fn shape(sb: &Sandbox) -> Vec<(usize, String)> {
    sb.reviews
        .view(&sb.file, true)
        .unwrap()
        .iter()
        .map(|v| {
            let status = match v.thread.status {
                ptal::model::Status::Open => "open",
                ptal::model::Status::Resolved { .. } => "resolved",
            };
            (v.thread.comments.len(), status.to_string())
        })
        .collect()
}

#[test]
fn with_two_threads_on_a_line_each_action_touches_only_the_selected_one() {
    let sb = sandbox(PLAN);
    for body in ["first", "second"] {
        sb.reviews
            .add_thread(&sb.file, 3, 3, body, "claude")
            .unwrap();
    }
    let mut app = sb.open();
    let _ = sb.press(&mut app, &["j", "j", "enter"]);
    assert_eq!(shape(&sb), [(1, "open".into()), (1, "open".into())]);

    let _ = sb.press(&mut app, &["tab", "r", "y", "ctrl+s"]);
    assert_eq!(shape(&sb), [(1, "open".into()), (2, "open".into())]);

    let _ = sb.press(&mut app, &["x"]);
    assert_eq!(shape(&sb), [(1, "open".into()), (2, "resolved".into())]);

    let _ = sb.press(&mut app, &["tab", "x"]);
    assert_eq!(shape(&sb), [(1, "resolved".into()), (2, "resolved".into())]);

    let _ = sb.press(&mut app, &["o"]);
    assert_eq!(shape(&sb), [(1, "open".into()), (2, "resolved".into())]);
}

#[test]
fn a_second_round_with_the_agent_runs_end_to_end() {
    let sb = sandbox(PLAN);
    let before = (
        fs::read(&sb.file).unwrap(),
        fs::metadata(&sb.file).unwrap().modified().unwrap(),
    );
    let file = sb.file.to_str().unwrap();
    // The agent's side: the real binary, as `claude`.
    let agent = |args: &[&str]| {
        let out = Command::new(env!("CARGO_BIN_EXE_ptal"))
            .args(args)
            .env("PTAL_DATA_DIR", &sb.data)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    let last_comment = || {
        let json = cli_comments(&sb);
        let comments = json["threads"][0]["comments"].as_array().unwrap().clone();
        comments.last().unwrap().clone()
    };

    // Round 1: the human comments in the TUI, the agent answers.
    let mut app = sb.open();
    let _ = sb.press(&mut app, &["j", "j", "enter", "w", "h", "y", "ctrl+s", "q"]);
    agent(&["reply", file, "c1", "Because.", "--as", "claude"]);

    // Round 2: the TUI shows the agent waiting; the human replies.
    let mut app = sb.open();
    assert!(screen(&app).contains("◆ 3│three"), "{}", screen(&app));
    let _ = sb.press(&mut app, &["j", "j", "enter", "r", "o", "k", "ctrl+s"]);
    assert_eq!(last_comment()["author"], "scott");
    assert_eq!(last_comment()["body"], "ok");

    // The agent resolves; reopening the TUI shows it hollow; `o` reopens.
    agent(&["resolve", file, "c1", "--as", "claude"]);
    assert_eq!(cli_comments(&sb)["threads"].as_array().unwrap().len(), 0);
    let mut app = sb.open();
    assert!(screen(&app).contains("○ 3│three"), "{}", screen(&app));
    let _ = sb.press(&mut app, &["j", "j", "enter", "o"]);
    assert_eq!(cli_comments(&sb)["threads"][0]["status"], "open");

    let after = (
        fs::read(&sb.file).unwrap(),
        fs::metadata(&sb.file).unwrap().modified().unwrap(),
    );
    assert_eq!(before, after, "the reviewed file is untouched");
}

const MARKDOWN: &str = "# Title\n\n- item with `code`\n\n```\nfn main() {}\n```\n";

/// Whether the `# Title` on row 0 is bold and the fenced body on row 5
/// (`fn main() {}`, after the 4-column gutter) is cyan.
fn looks_highlighted(app: &App) -> (bool, bool) {
    let buffer = drawn(app);
    let bold = (4..11).all(|x| buffer[(x, 0)].modifier.contains(Modifier::BOLD));
    let cyan = (4..16).all(|x| buffer[(x, 5)].fg == Color::Cyan);
    (bold, cyan)
}

#[test]
fn a_markdown_file_shows_highlighted_and_a_comment_still_quotes_the_raw_line() {
    let sb = sandbox(MARKDOWN);
    let mut app = sb.open();
    assert!(screen(&app).starts_with("  1│# Title"));
    assert_eq!(looks_highlighted(&app), (true, true));

    let flow = sb.press(&mut app, &["enter", "h", "i", "ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    let threads = sb.reviews.view(&sb.file, false).unwrap();
    assert_eq!(threads[0].thread.anchor.quote, "# Title");
    assert_eq!(looks_highlighted(&app), (true, true));
}

#[test]
fn a_text_file_or_no_color_shows_markdown_plain() {
    let text = sandbox_named("plan.txt", MARKDOWN);
    assert_eq!(looks_highlighted(&text.open()), (false, false));

    let no_color = sandbox(MARKDOWN);
    let app = no_color.open_with(false);
    assert!(screen(&app).starts_with("  1│# Title"));
    assert_eq!(looks_highlighted(&app), (false, false));
}

/// The first thread's `(start_line, end_line, quote)` from `ptal comments`.
fn first_anchor(sb: &Sandbox) -> (serde_json::Value, serde_json::Value, serde_json::Value) {
    let json = cli_comments(sb);
    let anchor = &json["threads"][0]["anchor"];
    (
        anchor["start_line"].clone(),
        anchor["end_line"].clone(),
        anchor["quote"].clone(),
    )
}

#[test]
fn a_visual_selection_downward_or_upward_saves_one_thread_on_the_range() {
    for select in [&["j", "v", "j", "j"], &["j", "j", "j", "v", "k", "k"][..]] {
        let sb = sandbox(PLAN);
        let mut app = sb.open();

        let mut keys = select.to_vec();
        keys.extend(["enter", "h", "i", "ctrl+s"]);
        let flow = sb.press(&mut app, &keys);

        assert_eq!(flow, ControlFlow::Continue(()), "{select:?}");
        assert_eq!(app.mode(), &Mode::Normal);
        let json = cli_comments(&sb);
        assert_eq!(json["threads"].as_array().unwrap().len(), 1);
        assert_eq!(json["threads"][0]["comments"][0]["body"], "hi");
        assert_eq!(
            first_anchor(&sb),
            (2.into(), 4.into(), "two\nthree\nfour".into()),
            "{select:?}"
        );
    }
}

#[test]
fn esc_cancels_a_selection_and_nothing_is_written() {
    let sb = sandbox(PLAN);
    let mut app = sb.open();

    assert_eq!(
        sb.press(&mut app, &["v", "j", "esc"]),
        ControlFlow::Continue(())
    );
    assert_eq!(app.mode(), &Mode::Normal);
    assert_eq!(sb.press(&mut app, &["q"]), ControlFlow::Break(()));

    assert!(sb.reviews.view(&sb.file, true).unwrap().is_empty());
    assert!(!sb.data.exists(), "no review data was written");
}

#[test]
fn a_range_overlapping_a_thread_adds_a_second_one_and_both_mark_the_shared_line() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "first", "claude")
        .unwrap();
    let mut app = sb.open();

    let flow = sb.press(&mut app, &["j", "v", "j", "j", "enter", "o", "k", "ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    let json = cli_comments(&sb);
    let threads = json["threads"].as_array().unwrap();
    assert_eq!(threads.len(), 2);
    assert_eq!(threads[1]["anchor"]["start_line"], 2);
    assert_eq!(threads[1]["anchor"]["end_line"], 4);
    assert_eq!(app.threads_on(2).len(), 2, "line 3 carries both");
    let marked: Vec<bool> = (0..5).map(|i| app.marker(i).is_some()).collect();
    assert_eq!(marked, [false, true, true, true, false]);
}

#[test]
fn a_comment_of_two_paragraphs_reads_back_the_same_in_the_cli_and_the_panel() {
    let sb = sandbox(PLAN);
    let mut app = sb.open();
    let body = "First para.\n\nSecond para.";

    let flow = sb.press_typing(&mut app, &["j", "j", "enter"], body, &["ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    assert_eq!(cli_comments(&sb)["threads"][0]["comments"][0]["body"], body);
    let mut reopened = sb.open();
    let _ = sb.press(&mut reopened, &["j", "j", "enter"]);
    let panel = screen(&reopened);
    let rows: Vec<&str> = panel.lines().collect();
    let first = rows
        .iter()
        .position(|row| row.contains("First para."))
        .unwrap_or_else(|| panic!("{panel}"));
    assert_eq!(rows[first + 1].replace(['│', ' '], ""), "", "{panel}");
    assert!(rows[first + 2].contains("Second para."), "{panel}");
}

#[test]
fn a_multi_line_reply_lands_last_with_its_newline() {
    let sb = sandbox(PLAN);
    sb.reviews
        .add_thread(&sb.file, 3, 3, "rename?", "claude")
        .unwrap();
    let mut app = sb.open();

    let _ = sb.press_typing(
        &mut app,
        &["j", "j", "enter", "r"],
        "Done.\nSee c2.",
        &["ctrl+s"],
    );

    let json = cli_comments(&sb);
    let comments = json["threads"][0]["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[1]["body"], "Done.\nSee c2.");
}

#[test]
fn a_word_inserted_mid_draft_lands_on_its_line() {
    let sb = sandbox(PLAN);
    let mut app = sb.open();

    let _ = sb.press_typing(
        &mut app,
        &["enter"],
        "one\ntwo\nthree",
        &["up", "home", "n", "e", "w", " ", "ctrl+s"],
    );

    let json = cli_comments(&sb);
    assert_eq!(
        json["threads"][0]["comments"][0]["body"],
        "one\nnew two\nthree"
    );
}

#[test]
fn a_failed_save_keeps_the_whole_multi_line_draft() {
    let sb = sandbox(PLAN);
    let mut app = sb.open();
    let text = "first line\n\nthird line";
    let _ = sb.press_typing(&mut app, &["G", "enter"], text, &[]);
    fs::write(&sb.file, "one\ntwo\n").unwrap();

    let flow = sb.press(&mut app, &["ctrl+s"]);

    assert_eq!(flow, ControlFlow::Continue(()));
    assert!(
        matches!(app.mode(), Mode::Input { draft, .. } if draft.text() == text),
        "{:?}",
        app.mode()
    );
    assert!(app.status().unwrap().contains("invalid line range"));
    assert!(sb.reviews.view(&sb.file, true).unwrap().is_empty());
}
