# Feature spec — Cases backend (the bootstrap feature)

Status: building · Stack: Rust workspace (Axum + sqlx + Postgres)

This is the **bootstrap feature**. The Cases backend cannot yet be built *through* the case
system (it does not run yet), so it is hand-built and recorded as the first Case retroactively
once the engine is live. Every later feature goes through `/feature` and leaves a real Case trail.

## Provenance
Design synthesized from a 3-way judge-panel (minimal-idiomatic · invariant-safety-first ·
dogfooding/observability-first). The panel produced strong conventions; this spec keeps the
**decisions** and discards the panel's biggest hazard: its agents read a *different*, pre-existing
private codebase and assumed we were extending it (they referenced its files, its crate, and its
identifiers, and even a different workflow shape). We are greenfield. Everything below is written
fresh with generic names; nothing is copied from that codebase, and the scrub-audit gate enforces it.

## Crates
- `engine` — PURE workflow-as-data logic. Dependency: `serde` only. Compiles to `wasm32` (a
  purity-check gate enforces it) so the identical transition rules can later run client-side over an
  in-browser SQL engine. No IO, no DB, no clock.
- `api` — Axum HTTP + sqlx Postgres + `engine`. Loads the workflow/case rows, calls
  `engine::evaluate` BEFORE any write, writes in one transaction, fires post-commit events.
- `mcp` — a thin MCP server exposing the same seven operations as tools, delegating to a shared
  `api::cases::svc` so the orchestrator (a first-class client) hits byte-identical validation.

## The engine contract
`WorkflowDef::evaluate(from, to, passed) -> Allow | Reject(reason)`, in this exact order (which
**must** match the DB trigger): same-state no-op → unknown_status → transition membership →
close-gate on entering terminal. Terminal = `last(states)`; `enters_terminal = is_terminal(to) &&
!is_terminal(from)`. Reject kinds: `unknown_status` · `invalid_transition` ·
`close_preconditions_unmet` (carries `missing[]`).

## App ↔ DB-trigger agreement
The DB `cases_guard` trigger independently enforces unknown_status + the terminal close-gate. It
does **not** enforce transition legality — the `engine` is the sole, *tighter* enforcer there. The
engine is a strict superset computed from the same rows, so the app never issues a write the trigger
rejects for a reason it could have returned as a clean 4xx. The trigger is a floor
(defense-in-depth), not the UX path. It RAISEs with a pinned SQLSTATE **`WG001`** so the api can
tell "guard fired during an allowed write" (= engine/trigger drift → a loud 500 `workflow_guard`)
from ordinary validation. Proven by `engine_never_allows_what_the_trigger_would_raise` (unit) and a
`trigger_is_backstop` integration test.

## Error taxonomy
422 value-rule / transition (`title_required`, `empty_comment`, `unknown_status`,
`invalid_transition`, `close_preconditions_unmet`, `unknown_check`) · 404 leak-free (`not_found`
incl. unreachable; `unknown_workflow`) · 400 `invalid_assignee` · 500 `db` (airlock) /
`workflow_guard` (drift).

## Endpoints (+ mirrored MCP tools)
`POST /api/cases` · `GET /api/cases/:id` · `GET /api/cases` · `POST /api/cases/:id/comments` ·
`PATCH /api/cases/:id/status` · `PUT /api/cases/:id/checks/:name` · `PUT /api/cases/:id/assignee`.
`create_case` forces `status = initial`; the create tx = entity + cases row + seeded unpassed
close-checks + owner membership. Events are fire-and-forget post-commit (one per state-changing
mutation; reads and same-state no-ops write none).

## Decisions kept / dropped from the panel
KEEP: runtime (non-macro) sqlx → hermetic CI, no `DATABASE_URL` at compile time, no `.sqlx` cache;
422 for transitions (not 409); post-commit fire-and-forget events; server-forced initial status;
shared `svc` module; exhaustive from×to + close-check powerset tests + the trigger-backstop test;
pin the trigger SQLSTATE.
DROP: `FOR UPDATE`/`SERIALIZABLE` locking; in-tx events; id-collision retry loop; an actor HTTP
header (actor id is a plain param until the RBAC resolver lands); proof-carrying newtypes; a
separate service crate.

## Clean-repo deltas vs the panel
- Schema is already `migrations/0001`+`0002` (workflows table, case_close_checks, cases_guard
  trigger). No migration rewrite — only pin the trigger SQLSTATE (done in 0001).
- 4-state seed `[backlog, in_progress, in_review, done]`; no extra states, no source-keyed split.
- No reach/RBAC gate yet (permissive per dev convention): get/list return all cases; the reach
  filter is the later `entity-rbac` feature. `not_found` stays leak-free for genuinely-absent ids.
- assignee validation = the entity exists, else 400. `created_by` is a real column (used directly).
- Fresh helpers (errors, id minter, db, events) — generic names, written here, copied from nothing.

## Build order (compile-checkpointed)
1. engine crate scaffold → compiles.
2. engine logic → compiles.
3. engine purity (wasm32) → clean.
4. engine unit tests (same-state · unknown_status · illegal-skip · fwd/back/reopen ·
   terminal-is-last · close-gate powerset · reopen-not-gated · never-allows-what-trigger-raises) → green.
5. (migration already present; SQLSTATE pinned) apply `0001`+`0002` to the dev DB → clean.
6–9. api reads → `api::cases::svc` + handlers → integration tests (atomic create · illegal=422-no-write ·
   close-gate end-to-end · trigger-is-backstop · leak-free 404 · same-state-redrop) → mcp tools.
10. full ci gate (`cargo test` + audit ratchet + purity-check) → green; summarize; wait for "ok push".

Steps 1–4 are this increment; 5–10 follow.
