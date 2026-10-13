#!/usr/bin/env bash
# Replay the seeded human comments (round 1) or round-2 replies on session B's plan,
# for iterations where the human does not drive the TUI. Quote every argument:
# an unquoted body is word-split and the agent finds no threads.
# Usage: scripts/dry-run-seed.sh 1|2 [sandbox-dir]
set -euo pipefail
round=${1:?round 1 or 2}
sandbox=${2:-${TMPDIR:-/tmp}/ptal-dry-run}
b="$sandbox/b"
export PATH="$sandbox/bin:$PATH" PTAL_DATA_DIR="$b/data"
plan="$b/plan.md"
as=(--as scott)

case $round in
  1)
    ptal add "$plan" --lines 1 "Why this approach and not the simpler one?" "${as[@]}"
    ptal add "$plan" --lines 3 "Cut this step; it is out of scope." "${as[@]}"
    ptal add "$plan" --lines 5 "We should fix this to be better." "${as[@]}"
    ;;
  2)
    # Replies target the threads the agent left open; ids come from `ptal comments`.
    echo "Round 2 replies depend on what the agent did: reply by hand with" >&2
    echo "  ptal reply $plan <id> \"<body>\" --as scott" >&2
    exit 2
    ;;
esac
