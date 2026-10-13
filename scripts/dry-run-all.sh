#!/usr/bin/env bash
# One full seeded iteration: A, then B turns 1-3 with the human replayed via the CLI.
# Round-2 replies are generic ("Go with your first suggestion.") on every open thread.
# Usage: scripts/dry-run-all.sh [sandbox-dir]
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
sandbox=${1:-${TMPDIR:-/tmp}/ptal-dry-run}
"$here/dry-run-a.sh" "$sandbox" >/dev/null
"$here/dry-run-b.sh" 1 "$sandbox" >/dev/null
"$here/dry-run-seed.sh" 1 "$sandbox" >/dev/null
"$here/dry-run-b.sh" 2 "$sandbox" >/dev/null
export PATH="$sandbox/bin:$PATH" PTAL_DATA_DIR="$sandbox/b/data"
plan="$sandbox/b/plan.md"
for id in $(ptal comments "$plan" | python3 -I -c 'import json,sys; print(*[t["id"] for t in json.load(sys.stdin)["threads"]])'); do
  ptal reply "$plan" "$id" 'Go with your first suggestion.' --as scott >/dev/null
done
"$here/dry-run-b.sh" 3 "$sandbox" >/dev/null
