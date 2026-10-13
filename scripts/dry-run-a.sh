#!/usr/bin/env bash
# Dry run, session A (integrate): a fresh agent edits the fixture skill to use ptal.
# Usage: scripts/dry-run-a.sh [sandbox-dir]   (default: $TMPDIR/ptal-dry-run)
# Leaves: <sandbox>/a/skills/draft-plan/SKILL.md (the edited skill) and
#         <sandbox>/a/trace.jsonl (every tool call), for scoring against rubric A.
set -euo pipefail
repo=$(cd "$(dirname "$0")/.." && pwd)
sandbox=${1:-${TMPDIR:-/tmp}/ptal-dry-run}
a="$sandbox/a"

rm -rf "$a"
mkdir -p "$a/skills" "$sandbox/bin" "$a/data"
cp -R "$repo/scripts/fixtures/draft-plan" "$a/skills/draft-plan"
# A copy, not a symlink: the agent must not be able to follow it back into the repo.
cp "$repo/target/debug/ptal" "$sandbox/bin/ptal"

cd "$a"
PATH="$sandbox/bin:$PATH" PTAL_DATA_DIR="$a/data" \
  claude -p "I have a skill at skills/draft-plan/SKILL.md. Let's use \`ptal\` for the review step in this skill." \
    --setting-sources project \
    --permission-mode acceptEdits \
    --allowedTools "Bash,Read,Edit,Write" \
    --output-format stream-json --verbose > trace.jsonl
echo "edited skill: $a/skills/draft-plan/SKILL.md"
echo "trace:        $a/trace.jsonl"
