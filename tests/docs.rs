//! Drift tests between the docs and the code they describe: every key in the
//! help overlay's listing is named somewhere in README's Keys section and in
//! `docs/interfaces.md` (not checked per mode), and no cell
//! of the interface map is too long to read. They also hold the contributor
//! docs (`AGENTS.md`, the ptal-plan skill) to the repo's current vocabulary:
//! no milestone-status bookkeeping, and one definition of done.
//!
//! These read the repo at run time, so the file is not part of the published
//! package.

use std::collections::HashSet;
use std::fs;
use std::path::Path;

use ptal::tui::keys::HELP;

fn read(relative: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(relative);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The text from the line `from` up to (not including) the line `to`.
fn section<'a>(text: &'a str, from: &str, to: &str) -> &'a str {
    let start = text
        .find(&format!("\n{from}\n"))
        .unwrap_or_else(|| panic!("no section {from:?}"))
        + 1;
    let rest = &text[start..];
    let end = rest
        .find(&format!("\n{to}\n"))
        .unwrap_or_else(|| panic!("no section {to:?}"));
    &rest[..end]
}

/// Every backtick-delimited token in `text`.
fn backticked(text: &str) -> HashSet<&str> {
    text.split('`').skip(1).step_by(2).collect()
}

/// The keys of a `HELP` label: alternatives split on `, `, then keys on `/`,
/// except a label that is exactly `/`.
fn keys_of(label: &str) -> Vec<&str> {
    label
        .split(", ")
        .flat_map(|alternative| {
            if alternative == "/" {
                vec!["/"]
            } else {
                alternative.split('/').collect()
            }
        })
        .collect()
}

/// The keys of `HELP` that `tokens` lacks, as `key (section)`.
fn missing_keys(tokens: &HashSet<&str>) -> Vec<String> {
    HELP.iter()
        .flat_map(|section| {
            section
                .entries
                .iter()
                .flat_map(|entry| keys_of(entry.keys))
                .map(move |key| (key, section.title))
        })
        .filter(|(key, _)| !tokens.contains(key))
        .map(|(key, title)| format!("`{key}` ({title})"))
        .collect()
}

#[test]
fn every_help_key_is_in_readme_and_interfaces() {
    let readme = read("README.md");
    let keys = section(&readme, "### Keys", "### For agents");
    let interfaces = read("docs/interfaces.md");
    for (doc, text) in [("README.md", keys), ("docs/interfaces.md", &interfaces)] {
        let missing = missing_keys(&backticked(text));
        assert!(missing.is_empty(), "{doc} lacks the keys {missing:?}");
    }
}

#[test]
fn a_missing_key_is_reported_with_its_section() {
    let tokens: HashSet<&str> = ["j", "k"].into();
    let missing = missing_keys(&tokens);
    assert!(missing.contains(&"`O` (Normal)".to_string()), "{missing:?}");
    assert!(!missing.iter().any(|m| m.starts_with("`j`")), "{missing:?}");
}

#[test]
fn keys_split_on_commas_and_slashes() {
    assert_eq!(keys_of("j/k, ↓/↑"), ["j", "k", "↓", "↑"]);
    assert_eq!(keys_of("/"), ["/"]);
    assert_eq!(keys_of("ctrl+d/ctrl+u"), ["ctrl+d", "ctrl+u"]);
}

/// The most words a cell of the interface map may have; longer detail goes
/// in a section under the tables.
const MAX_CELL_WORDS: usize = 40;

/// `row: column: words` for each cell of a pipe table in `text` over `max`
/// words. The header and delimiter rows are skipped.
fn long_cells(text: &str, max: usize) -> Vec<String> {
    let mut long = Vec::new();
    let mut headers: Vec<&str> = Vec::new();
    let mut in_table = false;
    for line in text.lines() {
        if !line.starts_with('|') {
            in_table = false;
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split(" | ").map(str::trim).collect();
        if !in_table {
            in_table = true;
            headers = cells;
            continue;
        }
        if cells.iter().all(|c| c.starts_with("---")) {
            continue;
        }
        assert_eq!(
            cells.len(),
            headers.len(),
            "table row split into {} cells, header has {}: {line}",
            cells.len(),
            headers.len()
        );
        for (cell, header) in cells.iter().zip(&headers) {
            let words = cell.split_whitespace().count();
            if words > max {
                long.push(format!("{} / {header}: {words} words", cells[0]));
            }
        }
    }
    long
}

#[test]
fn interface_cells_are_short() {
    let long = long_cells(&read("docs/interfaces.md"), MAX_CELL_WORDS);
    assert!(
        long.is_empty(),
        "cells over {MAX_CELL_WORDS} words; move detail under the tables:\n{}",
        long.join("\n")
    );
}

/// Every banned phrase in `text`, compared without case, as `phrase`, or
/// empty. Case-insensitive so "mvp" and "Built:" variants are caught too.
fn banned_phrases(text: &str, banned: &[&str]) -> Vec<String> {
    let lower = text.to_lowercase();
    banned
        .iter()
        .filter(|phrase| lower.contains(&phrase.to_lowercase()))
        .map(|phrase| format!("{phrase:?}"))
        .collect()
}

/// The titles of `text`'s markdown headings, lowercased and without `#`s or
/// bold markers. A status section is a heading titled "status" at any level.
fn headings(text: &str) -> Vec<String> {
    text.lines()
        .filter(|line| line.starts_with('#'))
        .map(|line| {
            line.trim_start_matches('#')
                .trim()
                .trim_matches('*')
                .trim()
                .to_lowercase()
        })
        .collect()
}

/// The commands in `AGENTS.md`'s "Green gate for every step:" line, in order.
fn gate_commands() -> Vec<String> {
    let agents = read("AGENTS.md");
    let line = agents
        .lines()
        .find(|line| line.starts_with("Green gate for every step:"))
        .expect("AGENTS.md has no \"Green gate for every step:\" line");
    line.split("`, `")
        .map(|command| {
            command
                .trim_matches(|c: char| c == '`' || c == ';')
                .to_string()
        })
        .collect()
}

#[test]
fn agents_has_no_mvp_or_status_bookkeeping() {
    let text = read("AGENTS.md");
    let banned = [
        "mvp",
        "once the crate exists",
        "labeled deferred",
        "built:",
        "built -",
    ];
    let found = banned_phrases(&text, &banned);
    assert!(found.is_empty(), "AGENTS.md still says {found:?}");
    assert!(
        !headings(&text).iter().any(|heading| heading == "status"),
        "AGENTS.md has a status section"
    );
}

#[test]
fn ptal_plan_skill_has_no_status_bookkeeping() {
    let text = read(".agents/skills/ptal-plan/SKILL.md");
    let banned = [
        "mvp",
        "architecture status",
        "readme status",
        "built:",
        "built -",
        "component-local",
        "deferred within",
    ];
    let found = banned_phrases(&text, &banned);
    assert!(found.is_empty(), "the ptal-plan skill still says {found:?}");
    assert!(
        !headings(&text).iter().any(|heading| heading == "status"),
        "the ptal-plan skill has a status section"
    );
}

#[test]
fn ptal_plan_skill_defers_agreements_to_agents() {
    let text = read(".agents/skills/ptal-plan/SKILL.md");
    let agreements = section(&text, "## Working agreements", "## Plan template");
    for expected in ["AGENTS.md", "green gate"] {
        assert!(
            agreements.contains(expected),
            "the skill's Working agreements do not name {expected:?}"
        );
    }
    // The section points at the gate; it does not list the commands. Elsewhere
    // in the skill a plan step may still say "run `cargo test <name>`".
    assert!(
        !agreements.contains("cargo"),
        "the skill restates gate commands in Working agreements: {agreements}"
    );
    // "five-command" is a claim about AGENTS.md's gate, so it must match it.
    if text.contains("five-command") {
        let commands = gate_commands();
        assert_eq!(
            commands.len(),
            5,
            "the skill says five-command, AGENTS.md's gate has {}: {commands:?}",
            commands.len()
        );
    }
}

/// The `repository` URL in `Cargo.toml`; the crate docs derive the README link
/// from it so a repository move updates one place.
fn repository() -> String {
    let manifest = read("Cargo.toml");
    manifest
        .lines()
        .find(|line| line.trim_start().starts_with("repository ="))
        .and_then(|line| line.split('"').nth(1))
        .expect("Cargo.toml has no quoted repository URL")
        .to_string()
}

#[test]
fn crate_docs_say_binary_first_unstable_and_link_readme() {
    let text = read("src/lib.rs");
    let docs = text
        .lines()
        .take_while(|line| line.starts_with("//!"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(!docs.is_empty(), "src/lib.rs has no crate-level docs");
    let readme = format!("{}/blob/main/README.md", repository());
    for expected in ["binary-first", "unstable", readme.as_str()] {
        assert!(
            docs.contains(expected),
            "the crate docs do not name {expected:?}"
        );
    }
}

#[test]
fn long_cells_are_found_by_row_and_column() {
    let table = "| Op | CLI |\n|---|---|\n| Do it | one two three |\n";
    assert_eq!(long_cells(table, 2), ["Do it / CLI: 3 words"]);
    assert!(long_cells(table, 3).is_empty());
}
