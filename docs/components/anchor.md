# `anchor`

**Role:** pure functions mapping line ranges between revisions. See [ARCHITECTURE.md](../../ARCHITECTURE.md#key-idea-revisions-and-anchoring) for how `review` uses them.

## Owns

### Functions
```
map_range(old: &[&str], new: &[&str], range: LineRange) -> Mapped { range, state }
Diff::new(old, new).map(range) -> Mapped     // one diff, many ranges
Diff::new(old, new).changes() -> Vec<Change { range, kind }>
clamp(range, len) -> LineRange               // into 1..=max(len, 1), no diff
```

- `LineRange { start, end }` is 1-based and inclusive; `state` is the model's `State`.
- Functions take lines, not text, so the caller splits them (`review` uses `str::lines`).
- The diff is `similar`'s Myers over whole lines. `map_range` is `Diff::new(old, new).map(range)`.
- Checking a range costs one pass over the diff ops, not one per line.
- `line_number(i)` is the one 0-based index → 1-based line conversion, saturating at `u32::MAX` for any `usize`. The TUI uses it too.

### Mapping rules
- `Current` iff every anchored line is in an unchanged run of the diff **and** the mapped lines are contiguous. A line inserted inside the range makes it `Outdated`, since the anchored text no longer reads the same.
- `Outdated` best guess: each end follows the diff. An end on an edited or deleted line goes to where that change starts in the new text.
- The result is clamped to `1..=max(new_len, 1)` with `end >= start`, so an empty file gives `1-1`.

### Changes
- `changes()` lists what differs in the new text, one `Change` per hunk (a maximal run of adjacent non-equal diff ops).
- A hunk with new lines is `ChangeKind::Changed` over them.
- A pure deletion is `Removed` at the line that now follows the removed text, or at the last line if it was removed at the end.
- Deleting everything marks nothing, and removals before and after a sole remaining line are one mark.

## Invariants
- Pure: no I/O, no clock, deterministic for the same inputs.
- `changes()` are sorted, disjoint and within `1..=new_len`; identical texts have none.
- `Current` implies the mapped lines are byte-identical to the originals.
- `Outdated` always carries a best-guess range clamped to the new file, never out of bounds. A range outside the old text is `Outdated`, never a panic.
- Identity: mapping a range between identical texts returns it unchanged.

## Contract
The functions above, with `Mapped`, `Change` and `ChangeKind`, are the whole surface. `review` is the only caller that maps stored anchors; the TUI uses `Diff` to keep its cursor in place across a re-read, and `line_number`.

## Tests
- One example per diff shape.
- Properties checked exhaustively over every pair of texts of 0–4 lines over `{a, b, c}` and every range: identity, in-bounds, and `Current` ⇒ identical text.
