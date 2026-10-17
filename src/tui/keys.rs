//! The help overlay: one source of truth for the keys that work in each
//! mode, so the overlay cannot drift from the key handling.
//!
//! [`HELP`] is a `const` table of `Section`s ([Normal, Thread panel]),
//! each of `Entry { keys, what }` rows. The overlay (`render::draw_help`)
//! and the `tui::app` probe test both read it: the probe drives every
//! candidate key through `update` in each mode and compares both ways
//! (every bound key is listed, every listed key is bound), which is what
//! ties [`HELP`] to the handlers. Labels use exactly the key names in
//! `docs/interfaces.md`.

use super::wrap;

#[cfg(test)]
use ratatui::crossterm::event::KeyEvent;

/// Keys that work in one mode, with a one-line description.
pub struct Entry {
    /// A human label for the keys, with aliases, e.g. `j/k, ↓/↑`.
    pub keys: &'static str,
    /// What the key does.
    pub what: &'static str,
}

/// A group of [`Entry`] rows under one heading.
pub struct Section {
    /// The mode this section describes.
    pub title: &'static str,
    /// The keys in that mode.
    pub entries: &'static [Entry],
}

/// Every mode's keys, grouped by mode.
pub const HELP: &[Section] = &[
    Section {
        title: "Normal",
        entries: &[
            Entry {
                keys: "j/k, ↓/↑",
                what: "Down / up",
            },
            Entry {
                keys: "ctrl+d/ctrl+u, PgDn/PgUp",
                what: "Half page down / up",
            },
            Entry {
                keys: "g/G, Home/End",
                what: "Top / bottom",
            },
            Entry {
                keys: "]/[",
                what: "Next / previous threaded line",
            },
            Entry {
                keys: "n/N",
                what: "Next / previous search match",
            },
            Entry {
                keys: "/",
                what: "Search the file",
            },
            Entry {
                keys: "enter",
                what: "Open the thread panel on a threaded line, else comment here",
            },
            Entry {
                keys: "v/V",
                what: "Select lines; movement extends, enter comments, esc leaves",
            },
            Entry {
                keys: "esc",
                what: "Clear the active search",
            },
            Entry {
                keys: "?",
                what: "Show this help",
            },
            Entry {
                keys: "q/ctrl+c",
                what: "Quit",
            },
        ],
    },
    Section {
        title: "Thread panel",
        entries: &[
            Entry {
                keys: "tab/shift+tab",
                what: "Next / previous thread on the line",
            },
            Entry {
                keys: "j/k, ↓/↑",
                what: "Scroll",
            },
            Entry {
                keys: "x",
                what: "Resolve the selected thread",
            },
            Entry {
                keys: "o",
                what: "Reopen the selected thread",
            },
            Entry {
                keys: "a",
                what: "Add a comment",
            },
            Entry {
                keys: "r",
                what: "Reply",
            },
            Entry {
                keys: "?",
                what: "Show this help",
            },
            Entry {
                keys: "esc/q/ctrl+c",
                what: "Close",
            },
        ],
    },
];

/// The help overlay's content rows at `width` columns: the title, each
/// section's heading and its entries, wrapped so no row exceeds the width.
/// The footer (bright own keys) is not part of this and is drawn separately.
pub fn help_rows(width: usize) -> Vec<String> {
    let mut rows = vec!["Help".to_owned()];
    for section in HELP {
        rows.push(section.title.to_owned());
        for entry in section.entries {
            rows.extend(entry_rows(entry, width));
        }
    }
    rows
}

/// The indent of a wrapped [`Entry`]'s continued description rows.
const DESC_INDENT: usize = 3;

/// The display rows for one [`Entry`]: the whole `keys` then description on
/// one row when they fit; else the keys on the first row and the
/// description wrapped underneath, indented, so words stay whole in narrow
/// windows and nothing is cut off.
fn entry_rows(entry: &Entry, width: usize) -> Vec<String> {
    let one = format!("{}  {}", entry.keys, entry.what);
    if wrap::width(&one) <= width {
        return vec![one];
    }
    // Keys are always short enough for their own row; the description wraps
    // under them at the window width less a small indent.
    let indent = " ".repeat(DESC_INDENT);
    let room = width.saturating_sub(DESC_INDENT).max(1);
    let mut rows = vec![entry.keys.to_owned()];
    for line in wrap::plain_rows(entry.what, room) {
        rows.push(format!("{indent}{line}"));
    }
    rows
}

/// Parse an [`Entry`]'s human `keys` label into the concrete keys it names,
/// so the probe can compare the table with the key handlers. Separators are
/// `,` and `/` (an alias list and a key list alike); tokens use exactly the
/// names in `docs/interfaces.md`.
#[cfg(test)]
pub(crate) fn parse_keys(label: &str) -> Vec<KeyEvent> {
    let mut out = Vec::new();
    for group in label.split(',') {
        let group = group.trim();
        // A group that is just the search key is `/` itself; split the rest
        // on `/` as the alias separator (`j/k`, `esc/q/ctrl+c`).
        if group == "/" {
            if let Some(key) = parse_key(group) {
                out.push(key);
            }
        } else {
            for token in group.split('/') {
                if let Some(key) = parse_key(token.trim()) {
                    out.push(key);
                }
            }
        }
    }
    out
}

#[cfg(test)]
fn parse_key(token: &str) -> Option<KeyEvent> {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    let code = match token.to_ascii_lowercase().as_str() {
        "enter" => KeyCode::Enter,
        "esc" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "shift+tab" => KeyCode::BackTab,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "down" | "↓" => KeyCode::Down,
        "up" | "↑" => KeyCode::Up,
        "left" | "←" => KeyCode::Left,
        "right" | "→" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pagedown" | "pgdn" | "pgdown" => KeyCode::PageDown,
        "pageup" | "pgup" => KeyCode::PageUp,
        _ if token.starts_with("ctrl+") => {
            let c = token
                .strip_prefix("ctrl+")
                .unwrap_or(token)
                .chars()
                .next()?;
            return Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL));
        }
        _ => {
            let mut chars = token.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => {
                    return Some(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
                }
                _ => return None,
            }
        }
    };
    Some(KeyEvent::new(code, KeyModifiers::NONE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_keys_are_in_interfaces_md() {
        let doc = include_str!("../../docs/interfaces.md");
        for section in HELP {
            for entry in section.entries {
                for raw in raw_tokens(entry.keys) {
                    assert!(
                        doc.contains(raw),
                        "help key {raw:?} of {:?} is missing from docs/interfaces.md",
                        entry.keys
                    );
                }
            }
        }
    }

    /// The key-name tokens a label names (as [`parse_keys`] parses them),
    /// as the literal text each must appear in `docs/interfaces.md`.
    fn raw_tokens(label: &str) -> Vec<&str> {
        label
            .split(',')
            .flat_map(|group| {
                let group = group.trim();
                if group == "/" {
                    vec![group].into_iter()
                } else {
                    group
                        .split('/')
                        .map(str::trim)
                        .collect::<Vec<_>>()
                        .into_iter()
                }
            })
            .filter(|t| !t.is_empty())
            .collect()
    }
}
