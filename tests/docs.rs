//! Drift tests between the docs and the code they describe: every key in the
//! help overlay's listing is named in README's key tables and in
//! `docs/interfaces.md` (not checked per mode), and no cell
//! of the interface map is too long to read. They also hold the contributor
//! docs (`AGENTS.md`, the ptal-plan skill) to the repo's current vocabulary:
//! no milestone-status bookkeeping, and one definition of done. README's prose
//! is held to the same: no status or roadmap words, no paragraph over 120
//! words, and a `For agents` section that links the reference docs and the
//! `ptal-review` skill. That skill (`skills/ptal-review/SKILL.md`, printed by
//! `ptal skill`) is held to the built binary: every command, flag and JSON
//! field it names exists, it stays short, and it links the reference docs
//! instead of restating them. README's Install names the release workflow's
//! targets, its Where data lives names the store's layout, its Troubleshooting
//! quotes the real error messages and codes, and every relative link in it
//! points at something the release archive copies.
//!
//! These read the repo at run time, so the file is not part of the published
//! package.

use std::collections::{HashMap, HashSet};
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
    let keys = section(&readme, "### Keys", "### Reading and navigation");
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
    // "six-command" is a claim about AGENTS.md's gate, so it must match it.
    if text.contains("six-command") {
        let commands = gate_commands();
        assert_eq!(
            commands.len(),
            6,
            "the skill says six-command, AGENTS.md's gate has {}: {commands:?}",
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

/// The most words a prose paragraph or list item of the README may have.
const MAX_PROSE_WORDS: usize = 120;

/// The prose units of markdown `text`: each blank-line-separated paragraph and
/// each list item, as one string. Fenced code, tables and headings are skipped.
fn prose_units(text: &str) -> Vec<String> {
    let mut units: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut in_fence = false;
    let flush = |units: &mut Vec<String>, current: &mut String| {
        if !current.is_empty() {
            units.push(std::mem::take(current));
        }
    };
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            flush(&mut units, &mut current);
            in_fence = !in_fence;
            continue;
        }
        let trimmed = line.trim_start();
        let is_item = ["- ", "* ", "+ ", "> "]
            .iter()
            .any(|marker| trimmed.starts_with(marker))
            || trimmed.split_once(". ").is_some_and(|(n, _)| {
                // A wrapped line starting "12. " is an item only inside a numbered list.
                (1..=2).contains(&n.len())
                    && n.chars().all(|c| c.is_ascii_digit())
                    && (current.is_empty() || current.starts_with(|c: char| !c.is_alphabetic()))
            });
        if in_fence || line.starts_with('|') || line.starts_with('#') || line.trim().is_empty() {
            flush(&mut units, &mut current);
        } else {
            if is_item {
                flush(&mut units, &mut current);
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(line.trim());
        }
    }
    flush(&mut units, &mut current);
    units
}

/// `text` with fenced code blocks removed.
fn without_code(text: &str) -> String {
    let mut in_fence = false;
    let mut kept = Vec::new();
    for line in text.lines() {
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
        } else if !in_fence {
            kept.push(line);
        }
    }
    kept.join("\n")
}

/// The banned phrases in `text` as whole words, compared without case.
fn banned_words(text: &str, banned: &[&str]) -> Vec<String> {
    let lower = text.to_lowercase();
    let is_word = |c: Option<char>| c.is_some_and(char::is_alphanumeric);
    banned
        .iter()
        .filter(|phrase| {
            let phrase = phrase.to_lowercase();
            lower.match_indices(&phrase).any(|(at, _)| {
                !is_word(lower[..at].chars().next_back())
                    && !is_word(lower[at + phrase.len()..].chars().next())
            })
        })
        .map(|phrase| format!("{phrase:?}"))
        .collect()
}

#[test]
fn readme_has_no_status_or_roadmap_bookkeeping() {
    let readme = read("README.md");
    let prose = without_code(&readme);
    let banned = [
        "**Status",
        "**Planned",
        "not built",
        "not yet built",
        "was built",
        "crit",
        "deliberately",
        "Why not a PR",
        "nested replies",
        "is built",
        "are built",
    ];
    let found = banned_words(&prose, &banned);
    assert!(found.is_empty(), "README.md still says {found:?}");
}

#[test]
fn banned_words_match_whole_words_only() {
    assert!(banned_words("a critical bug", &["crit"]).is_empty());
    assert_eq!(banned_words("like Crit does", &["crit"]), ["\"crit\""]);
}

#[test]
fn readme_paragraphs_are_short() {
    let long: Vec<String> = prose_units(&read("README.md"))
        .into_iter()
        .filter(|unit| unit.split_whitespace().count() > MAX_PROSE_WORDS)
        .map(|unit| unit.chars().take(60).collect())
        .collect();
    assert!(
        long.is_empty(),
        "README paragraphs over {MAX_PROSE_WORDS} words; split them:\n{}",
        long.join("\n")
    );
}

#[test]
fn prose_units_split_on_blank_lines_items_and_skip_code_and_tables() {
    let text = "# Head\n\none two\nthree\n\n```sh\nnot prose\n```\n| a | b |\n|---|---|\n- item one\n  more\n- item two\n1. first\nlast para\n";
    assert_eq!(
        prose_units(text),
        [
            "one two three",
            "- item one more",
            "- item two",
            "1. first last para"
        ]
    );
    assert_eq!(prose_units("wrapped\n2024. year\n"), ["wrapped 2024. year"]);
    assert_eq!(prose_units("wrapped\n12. more\n"), ["wrapped 12. more"]);
    assert_eq!(prose_units("a\n* b\n> c\n"), ["a", "* b", "> c"]);
}

#[test]
fn readme_agent_section_links_the_reference_docs() {
    let readme = read("README.md");
    let agents = section(&readme, "### For agents", "## Troubleshooting");
    for expected in ["ptal --help", "docs/interfaces.md", "docs/protocol.md"] {
        assert!(agents.contains(expected), "For agents lacks {expected:?}");
    }
    assert!(
        !readme.contains("**Turns.**"),
        "the Turns paragraph is back"
    );
}

// ---- the ptal-review skill -------------------------------------------------

const SKILL_PATH: &str = "skills/ptal-review/SKILL.md";
const MAX_SKILL_LINES: usize = 70;

fn ptal(args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_ptal"))
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "ptal {args:?} failed");
    String::from_utf8(out.stdout).unwrap()
}

/// The `ptal <word>` commands and `--flags` named in `text`'s backticked
/// spans, in order. A flag is paired with the nearest `ptal <word>` before it
/// in its span (else `None`).
fn commands_and_flags(text: &str) -> Vec<(Option<String>, String)> {
    let mut found = Vec::new();
    for span in text.split('`').skip(1).step_by(2) {
        let words: Vec<&str> = span
            .split_whitespace()
            .map(|w| w.trim_end_matches([',', '.', ';', ':', ')']))
            .collect();
        let mut command: Option<String> = None;
        for (i, word) in words.iter().enumerate() {
            if *word == "ptal"
                && let Some(next) = words.get(i + 1)
                && next.chars().all(|c| c.is_ascii_lowercase())
            {
                command = Some((*next).to_string());
                found.push((None, format!("ptal {next}")));
            }
            if word.starts_with("--") && word[2..].starts_with(char::is_alphabetic) {
                found.push((command.clone(), (*word).to_string()));
            }
        }
    }
    found
}

/// What is wrong with the commands and flags `text` names, judged by the
/// built binary's own help.
fn unknown_commands_and_flags(text: &str) -> Vec<String> {
    let top = ptal(&["--help"]);
    let subcommands: Vec<String> = top
        .lines()
        .skip_while(|l| *l != "Commands:")
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next().map(str::to_string))
        .filter(|c| c != "help")
        .collect();
    assert!(subcommands.contains(&"wait".to_string()), "{subcommands:?}");
    let helps: HashMap<&str, String> = subcommands
        .iter()
        .map(|c| (c.as_str(), ptal(&[c, "--help"])))
        .collect();
    let all_help = helps.values().fold(top, |all, h| all + h);
    let mut bad = Vec::new();
    for (command, name) in commands_and_flags(text) {
        match (&command, name.strip_prefix("ptal ")) {
            (_, Some(word)) if !helps.contains_key(word) => bad.push(name),
            (Some(command), None) => {
                if !helps
                    .get(command.as_str())
                    .is_some_and(|h| h.contains(&name))
                {
                    bad.push(format!("ptal {command} {name}"));
                }
            }
            (None, None) if !all_help.contains(&name) => bad.push(name),
            _ => {}
        }
    }
    bad
}

#[test]
fn the_checker_catches_a_made_up_command_and_flag() {
    let bad = unknown_commands_and_flags("run `ptal frobnicate x`, then `ptal wait f --nope`\n");
    assert_eq!(bad, ["ptal frobnicate", "ptal wait --nope"]);
    assert!(unknown_commands_and_flags("`ptal wait f --timeout 5`\n").is_empty());
}

#[test]
fn skill_has_frontmatter_and_is_short() {
    let skill = read(SKILL_PATH);
    assert!(skill.starts_with("---\n"));
    assert!(skill.contains("\nname: ptal-review\n"));
    assert!(
        skill
            .lines()
            .any(|l| l.starts_with("description: ") && l.len() > "description: ".len() + 20),
        "description is missing or empty"
    );
    assert!(
        skill.lines().count() <= MAX_SKILL_LINES,
        "skill is too long"
    );
    assert!(
        !skill.lines().any(|l| l.trim_start().starts_with('|')),
        "no tables in the skill"
    );
}

#[test]
fn ptal_skill_output_matches_the_repo_file() {
    assert_eq!(ptal(&["skill"]), read(SKILL_PATH));
}

#[test]
fn skill_uses_only_real_commands_and_flags() {
    assert_eq!(
        unknown_commands_and_flags(&read(SKILL_PATH)),
        [] as [String; 0]
    );
}

#[test]
fn skill_json_fields_are_documented() {
    let skill = read(SKILL_PATH);
    let wait = ptal(&["wait", "--help"]);
    let comments = ptal(&["comments", "--help"]);
    // The skill's field path, and the quoted key each segment is documented as.
    for (path, help, keys) in [
        ("reason", &wait, &["\"reason\""][..]),
        (
            "review.latest.note",
            &comments,
            &["\"review\"", "\"latest\"", "\"note\""],
        ),
        ("review.verdict", &comments, &["\"review\"", "\"verdict\""]),
    ] {
        assert!(skill.contains(path), "the skill no longer reads {path}");
        for key in keys {
            assert!(help.contains(key), "{path}: help lacks {key}");
        }
    }
}

#[test]
fn wait_reason_field_is_what_the_skill_switches_on() {
    let tmp = tempfile::tempdir().unwrap();
    let file = tmp.path().join("plan.md");
    fs::write(&file, "one\n").unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_ptal"))
        .args(["wait", file.to_str().unwrap(), "--timeout", "0"])
        .env("PTAL_DATA_DIR", tmp.path().join("data"))
        .output()
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["reason"], "timeout");
    let skill = read(SKILL_PATH);
    for reason in ["turn", "closed", "timeout"] {
        assert!(skill.contains(&format!("\"{reason}\"")), "{reason}");
    }
}

#[test]
fn skill_links_the_reference_docs_by_absolute_url() {
    let skill = read(SKILL_PATH);
    assert!(skill.contains("ptal --help"));
    for doc in ["docs/interfaces.md", "docs/protocol.md"] {
        assert!(
            skill.contains(&format!("{}/blob/main/{doc}", repository())),
            "{doc} is not linked by absolute URL"
        );
    }
    assert!(!skill.contains("](docs/") && !skill.contains("](./"));
}

#[test]
fn skill_calls_out_its_policy_choices() {
    let skill = read(SKILL_PATH);
    let choices = skill
        .split("\n## Your choices\n")
        .nth(1)
        .expect("no `## Your choices` section");
    for needle in ["resolve", "approved", "--as", "edit"] {
        assert!(
            choices.to_lowercase().contains(needle),
            "choices lack {needle:?}"
        );
    }
}

#[test]
fn skill_does_not_restate_reference() {
    let skill = read(SKILL_PATH);
    for banned in ["\"error\":", "{\"file\"", "exit code"] {
        assert!(!skill.contains(banned), "the skill restates {banned:?}");
    }
    assert!(!skill.lines().any(|l| l.trim_start().starts_with('{')));
}

#[test]
fn readme_agent_section_links_the_skill() {
    let readme = read("README.md");
    let agents = section(&readme, "### For agents", "## Troubleshooting");
    for expected in [
        "skills/ptal-review/SKILL.md",
        "ptal skill",
        "npx skills add",
    ] {
        assert!(agents.contains(expected), "For agents lacks {expected:?}");
    }
}

#[test]
fn contributor_skills_are_hidden_from_npx() {
    for path in [
        ".agents/skills/ptal-plan/SKILL.md",
        ".agents/skills/rust-skills/SKILL.md",
    ] {
        let skill = read(path);
        let front = skill.split("\n---\n").next().unwrap();
        for needle in ["name:", "description:", "internal: true"] {
            assert!(front.contains(needle), "{path} lacks {needle:?}");
        }
    }
}

// ---- README: install, data, troubleshooting --------------------------------

/// The `target:` values in the matrix of `.github/workflows/release.yml`.
fn release_targets() -> Vec<String> {
    read(".github/workflows/release.yml")
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- { target: "))
        .map(|rest| rest.split(',').next().unwrap_or(rest).trim().to_string())
        .collect()
}

#[test]
fn release_targets_are_parsed() {
    let targets = release_targets();
    assert_eq!(targets.len(), 4, "{targets:?}");
    assert!(targets.contains(&"x86_64-unknown-linux-gnu".to_string()));
}

#[test]
fn readme_install_covers_crates_io_and_prebuilt_binaries() {
    let readme = read("README.md");
    let install = section(&readme, "## Install", "## Usage");
    let mut expected: Vec<String> = [
        "cargo install ptal --locked",
        "SHA256SUMS",
        "shasum -a 256 -c",
        "glibc",
        "Windows",
    ]
    .map(String::from)
    .into();
    expected.extend(release_targets());
    for item in expected {
        assert!(install.contains(&item), "Install lacks {item:?}");
    }
}

#[test]
fn readme_data_section_matches_the_store() {
    let readme = read("README.md");
    let data = section(&readme, "## Where data lives", "## Building");
    for expected in [
        "PTAL_DATA_DIR",
        "XDG_DATA_HOME",
        "<slug>-<hash8>",
        "review.json",
        "review.lock",
        "revisions/",
        "canonical",
        "rename",
        "rm -r",
    ] {
        assert!(
            data.contains(expected),
            "Where data lives lacks {expected:?}"
        );
    }
}

#[test]
fn readme_troubleshooting_matches_the_binary() {
    use ptal::Error;
    use ptal::cli::Failure;

    let readme = read("README.md");
    let troubleshooting = section(&readme, "## Troubleshooting", "## Where data lives");
    let cli_failures = [
        Failure::NotATerminal,
        Failure::MissingAuthor,
        Failure::Core(Error::Locked),
    ];
    for failure in &cli_failures {
        let message = failure.to_string();
        assert!(
            troubleshooting.contains(&message),
            "Troubleshooting lacks the message {message:?}"
        );
        let (code, _) = failure.classify();
        assert!(
            troubleshooting.contains(&format!("`{code}`")),
            "Troubleshooting lacks the code `{code}`"
        );
    }
    // `file_changed` is a TUI footer message; no CLI command reports it.
    let message = Error::FileChanged.to_string();
    assert!(
        troubleshooting.contains(&message),
        "Troubleshooting lacks the message {message:?}"
    );
    assert!(
        troubleshooting.contains("renamed"),
        "Troubleshooting lacks the vanished-comments entry"
    );
}

/// The first path segment of each relative link target in markdown `text`.
fn relative_link_roots(text: &str) -> HashSet<String> {
    text.split("](")
        .skip(1)
        .filter_map(|rest| rest.split(')').next())
        .filter(|target| !target.starts_with('#') && !target.contains("://"))
        .map(|target| {
            let path = target.split('#').next().unwrap_or(target);
            path.split('/').next().unwrap_or(path).to_string()
        })
        .collect()
}

#[test]
fn relative_link_roots_skip_urls_and_bare_anchors() {
    let text = "[a](docs/x.md#y) [b](https://e.com/z) [c](#top) [d](LICENSE)";
    let roots = relative_link_roots(text);
    assert_eq!(
        roots,
        HashSet::from(["docs".to_string(), "LICENSE".to_string()])
    );
}

#[test]
fn readme_links_the_changelog() {
    assert!(read("README.md").contains("](CHANGELOG.md)"));
}

#[test]
fn readme_links_ship_in_the_release_archive() {
    let release = read(".github/workflows/release.yml");
    let copied: HashSet<&str> = release
        .lines()
        .filter(|line| line.trim_start().starts_with("cp "))
        .flat_map(str::split_whitespace)
        .collect();
    let missing: Vec<String> = relative_link_roots(&read("README.md"))
        .into_iter()
        .filter(|root| !copied.contains(root.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "README links these but the release archive does not copy them: {missing:?}"
    );
}
