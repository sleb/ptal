//! The key listing behind the help overlay: one table of every key bound in
//! Normal mode and the thread panel, and the rows that lay it out.
//!
//! The key handlers in [`super::app`] are the behavior; [`HELP`] is the
//! documentation of it. A probe test in `tui::app` presses every candidate
//! key in each mode and fails when a key does something that is not listed,
//! or a listed key does nothing, so the two cannot drift apart. `tests/docs.rs` ties the listing to
//! README's keys and `docs/interfaces.md` the same way.

use super::wrap;

/// One line of the listing: the keys that do something, and what it is.
pub struct Entry {
    /// The keys as a label with aliases, e.g. `j/k, ↓/↑`: names as in
    /// `docs/interfaces.md`, `/` or `,` between alternatives.
    pub keys: &'static str,
    /// What they do.
    pub what: &'static str,
}

/// The keys of one mode.
pub struct Section {
    /// The mode's name, shown as a header.
    pub title: &'static str,
    /// Its keys.
    pub entries: &'static [Entry],
}

/// Every key bound in Normal mode and the thread panel.
pub const HELP: &[Section] = &[
    Section {
        title: "Normal",
        entries: &[
            Entry {
                keys: "j/k, ↓/↑",
                what: "move the cursor down / up",
            },
            Entry {
                keys: "ctrl+d/ctrl+u, PgDn/PgUp",
                what: "half a page down / up",
            },
            Entry {
                keys: "g/G, Home/End",
                what: "go to the top / bottom",
            },
            Entry {
                keys: "]/[",
                what: "next / previous threaded line",
            },
            Entry {
                keys: "}/{",
                what: "next / previous change since your last hand-off or close",
            },
            Entry {
                keys: "/",
                what: "search; the cursor jumps live, enter keeps it, esc goes back",
            },
            Entry {
                keys: "n/N",
                what: "next / previous line matching the search",
            },
            Entry {
                keys: "esc",
                what: "clear the search and its highlight",
            },
            Entry {
                keys: "v/V",
                what: "select lines; movement extends, enter comments, esc leaves",
            },
            Entry {
                keys: "enter",
                what: "open the line's threads, or comment on the line",
            },
            Entry {
                keys: "H",
                what: "hand off to the agent, with an optional note",
            },
            Entry {
                keys: "C",
                what: "close the review: tab picks approved or dropped, then a note",
            },
            Entry {
                keys: "O",
                what: "reopen a closed review",
            },
            Entry {
                keys: "i",
                what: "show the review: whose turn, the latest note in full",
            },
            Entry {
                keys: "?",
                what: "show this help",
            },
            Entry {
                keys: "q, ctrl+c",
                what: "quit",
            },
        ],
    },
    Section {
        title: "Thread panel",
        entries: &[
            Entry {
                keys: "tab, shift+tab",
                what: "select the next / previous thread",
            },
            Entry {
                keys: "j/k, ↓/↑",
                what: "scroll the panel",
            },
            Entry {
                keys: "a",
                what: "add a thread on this line",
            },
            Entry {
                keys: "r",
                what: "reply to the selected thread",
            },
            Entry {
                keys: "x",
                what: "resolve the selected thread",
            },
            Entry {
                keys: "o",
                what: "reopen the selected thread",
            },
            Entry {
                keys: "?",
                what: "show this help",
            },
            Entry {
                keys: "esc, q, ctrl+c",
                what: "close the panel",
            },
        ],
    },
];

/// One row of the laid-out listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// The text, at most the layout width wide.
    pub text: String,
    /// Whether this is a section header.
    pub heading: bool,
}

/// Room the description keeps beside the keys column before the layout
/// stacks the description under the keys instead.
const MIN_WHAT: usize = 12;
/// Indent of an entry's keys.
const INDENT: usize = 2;
/// Cells between the keys column and the description.
const GAP: usize = 2;

/// [`HELP`] laid out for `width` cells: a header per section, then each
/// entry's keys with the description wrapped beside them, or under them when
/// the window is too narrow for two columns. Nothing is cut off.
pub fn rows(width: usize) -> Vec<Row> {
    let width = width.max(1);
    let column = HELP
        .iter()
        .flat_map(|section| section.entries)
        .map(|entry| wrap::width(entry.keys))
        .max()
        .unwrap_or(0);
    let beside = INDENT + column + GAP + MIN_WHAT <= width;
    let mut out = Vec::new();
    for (index, section) in HELP.iter().enumerate() {
        if index > 0 {
            out.push(Row {
                text: String::new(),
                heading: false,
            });
        }
        out.push(Row {
            text: section.title.to_string(),
            heading: true,
        });
        for entry in section.entries {
            let plain = |text: String| Row {
                text: text.trim_end().to_string(),
                heading: false,
            };
            if beside {
                let lead = INDENT + column + GAP;
                let what = wrap::plain_rows(entry.what, width - lead);
                for (n, row) in what.iter().enumerate() {
                    let left = if n == 0 {
                        let pad = column - wrap::width(entry.keys);
                        format!(
                            "{}{}{}",
                            " ".repeat(INDENT),
                            entry.keys,
                            " ".repeat(pad + GAP)
                        )
                    } else {
                        " ".repeat(lead)
                    };
                    out.push(plain(format!("{left}{row}")));
                }
            } else {
                let keys = wrap::plain_rows(entry.keys, width.saturating_sub(INDENT).max(1));
                out.extend(
                    keys.iter()
                        .map(|row| plain(format!("{}{row}", " ".repeat(INDENT)))),
                );
                let room = width.saturating_sub(2 * INDENT).max(1);
                let what = wrap::plain_rows(entry.what, room);
                out.extend(
                    what.iter()
                        .map(|row| plain(format!("{}{row}", " ".repeat(2 * INDENT)))),
                );
            }
        }
    }
    out
}

/// The concrete keys named by a label, for the probe test: `j/k, ↓/↑` is
/// `j`, `k`, `↓`, `↑`. A bare `/` is the slash key.
#[cfg(test)]
pub fn names(label: &str) -> Vec<&str> {
    if label == "/" {
        return vec!["/"];
    }
    label
        .split([',', '/'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect()
}
