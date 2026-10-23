//! Drift tests between the docs and the code they describe: every key in the
//! help overlay's listing is in README and `docs/interfaces.md`, and no cell
//! of the interface map is too long to read.
//!
//! These read `README.md` and `docs/` at run time, so the file is not part of
//! the published package (the package ships no `docs/`).

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
fn interface_cells_are_at_most_40_words() {
    let long = long_cells(&read("docs/interfaces.md"), MAX_CELL_WORDS);
    assert!(
        long.is_empty(),
        "move detail under the tables:\n{}",
        long.join("\n")
    );
}

#[test]
fn long_cells_are_found_by_row_and_column() {
    let table = "| Op | CLI |\n|---|---|\n| Do it | one two three |\n";
    assert_eq!(long_cells(table, 2), ["Do it / CLI: 3 words"]);
    assert!(long_cells(table, 3).is_empty());
}
