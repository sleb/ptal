#!/usr/bin/env bash
# Dry run, session B (execute): a fresh agent runs the skill A produced, in three turns.
#   dry-run-b.sh 1 [sandbox]   plan; then YOU comment in the TUI (command is printed)
#   dry-run-b.sh 2 [sandbox]   "done": the agent addresses round 1
#   dry-run-b.sh 3 [sandbox]   "done": the agent handles round 2 (after you reply/resolve in the TUI)
# Needs session A to have run first. Leaves <sandbox>/b/trace-<turn>.jsonl per turn.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
turn=${1:?turn 1, 2 or 3}
sandbox=${2:-${TMPDIR:-/tmp}/ptal-dry-run}
b="$sandbox/b"
export PATH="$sandbox/bin:$PATH" PTAL_DATA_DIR="$b/data"

run() { # run <turn> <claude args...>
  local n=$1; shift
  (cd "$b" && claude -p "$@" \
    --setting-sources project --permission-mode acceptEdits \
    --allowedTools "Bash,Read,Edit,Write" \
    --output-format stream-json --verbose > "trace-$n.jsonl")
}

case $turn in
  1)
    rm -rf "$b"
    mkdir -p "$b/skills" "$b/data"
    cp -R "$sandbox/a/skills/draft-plan" "$b/skills/draft-plan"
    cp "$repo/scripts/fixtures/task.md" "$repo/scripts/fixtures/deploy.sh" "$b/"
    run 1 "Follow skills/draft-plan/SKILL.md to plan the task in task.md."
    session=$(grep -o '"session_id":"[^"]*"' "$b/trace-1.jsonl" | head -1 | cut -d'"' -f4)
    echo "$session" > "$b/session"
    echo
    echo "Now comment as the human, with the SAME data dir (separate terminal):"
    echo "  PTAL_DATA_DIR=$b/data ptal $b/plan.md"
    echo "Leave 3 comments: a question, a change request, an ambiguous one."
    ;;
  2) run 2 --resume "$(cat "$b/session")" "done" ;;
  3)
    echo "Reply to / resolve / reopen threads in the TUI (same data dir) before this turn."
    run 3 --resume "$(cat "$b/session")" "done"
    ;;
esac
echo "trace: $b/trace-$turn.jsonl"
