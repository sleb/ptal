# Task: add a `--dry-run` flag to the `deploy` script

The deploy script (`deploy.sh`) copies `build/` to the server, restarts the
service, and runs a smoke test. Add a `--dry-run` flag that prints what each
step would do without doing it.

Constraints:
- No new dependencies.
- The smoke test must still be reported as "skipped" in dry-run mode.
- Keep the change under 50 lines.

Write the plan with exactly these sections, in this order, using these
headings: `## Goal`, `## Steps` (a numbered list; the last step is "Document the flag in README.md"), `## Risks` (a bullet list).
