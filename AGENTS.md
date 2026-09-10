# C-C-OS-CU — agent instructions

Central-unit OS repository; currently contains only a project README.

## Working method
- Search filenames/symbols first; read targeted ranges.
- Trace the active path and source of truth.
- Keep changes scoped; no unrelated refactors or speculative abstractions.
- Preserve local work; stage explicit paths and review the diff.
- Reuse existing patterns; inspect both sides of changed contracts.
- Load matching skills and linked docs only when needed.
- Avoid bulk reads and rereading unchanged context.
- Report outcome, executed checks and blockers concisely.

## Validation and delivery
- Run narrow, meaningful checks using existing commands.
- Broaden checks for affected boundaries/failures; reuse valid results.
- Instructions-only edits: validate links, skill metadata and whitespace.
- Separate review, tests, build, deployment and real-device evidence.
- Use a scoped branch/PR per repo; preserve the exact Jira key.
- Assigned Jira implementation: In Progress before work; In Review after PR.
- Jira Done requires merge and required release evidence.
- Keep credentials, signing material and private data out of logs/commits.

## Repository rules
- Inspect the current tree before choosing implementation conventions.
- No runtime, build system or repeatable technical workflow is established yet.
- Do not infer a platform or copy backend/mobile deployment assumptions here.

## On-demand references
- Project scope: [README](README.md).
- Add architecture under `docs/` when implementation establishes it.
- Add a narrowly named `.agents/skills/<workflow>/SKILL.md` only for a real repeated workflow.
- Until then, validate documentation links and whitespace; do not claim runtime tests.
