# build-engine

A **minimal, headless build-engine** for shipping small web apps fast and cleanly — a generalized foundation distilled
from several production-discipline projects:

- **Data model** — a "database-as-a-framework" registry: declare an object type as a *row* and it auto-wires storage,
  CRUD, access-control edges, and an audit trail. New object types need (almost) no new code.
- **Cases** — a workflow-as-data work tracker: states + allowed transitions are data, so an illegal transition is
  rejected by construction (`422`), with comment threads and an audit event for every change.
- **Agent orchestrator** — a 5-role pipeline (Architect → Tester → Coder → Reviewer → Ops) with tool-scoped
  permissions, a Case-ID handoff bus, a resumable state ledger, and a circuit breaker — so AI coding agents ship
  convention-correct code without eroding the codebase.
- **CI ratchet** — one `ci.sh` runs a set of static audits that fail only on *new* violations vs. a committed baseline
  (including a privacy-by-design gate), letting a fleet of agents move fast on one branch without drift.

It's **headless by design**: every capability stores its data ready for a UI to render later. The point is to build the
*next* app *through* this engine — so the work tracker ends up holding the real, browsable history of how each app was
built.

## Status
v1 in progress — the minimal engine: data model · Cases · orchestrator · CI. See `docs/`.

## Layout (planned)
```
build-engine/
  docs/            # DATA-MODEL.md, the build/orchestration notes, FE specs for later UIs
  migrations/      # the schema
  src/             # the headless backend (API + MCP)
  .agents/         # the 5-role orchestrator config + /feature flow
  ci.sh            # the ratcheted audit gate
```
