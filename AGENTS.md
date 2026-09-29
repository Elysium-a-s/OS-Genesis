# OS Genesis — agent instructions

Genesis je v štádiu úvodnej dokumentačnej kostry. Adresáre `core/`, `adapters/`, `panel/`, `ha-app/`, `contracts/`, `docs/` a `tests/` zatiaľ obsahujú len README; funkčný runtime a build systém ešte neexistujú.

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
- Use a scoped branch/PR per repo; preserve the exact Jira key when one is assigned.
- Assigned Jira implementation: In Progress before work; In Review after PR.
- Jira Done requires merge and required release evidence.
- Keep credentials, signing material and private data out of logs/commits.

## Repository rules
- Inspect the current tree before choosing implementation conventions.
- No runtime, build system or repeatable technical workflow is established yet.
- Do not infer a platform or copy backend/mobile deployment assumptions here.
- Treat the architecture and languages in README as a starting proposal until validated on pilot hardware.

## On-demand references
- Project scope: [README](README.md).
- Add implementation architecture under `docs/` when implementation establishes it.
- Add a narrowly named `.agents/skills/<workflow>/SKILL.md` only for a real repeated workflow.
- Until then, validate documentation links and whitespace; do not claim runtime tests.
