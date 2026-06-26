# build-engine

![ci](https://github.com/doumouya/build-engine-demo/actions/workflows/ci.yml/badge.svg)

A small, headless **build system that hosts its own build process**. Declaring an object type is a
*row*, not a migration; a workflow is *data*, not a `switch`; and the same gates that keep the code
clean also make "built through this system" a verifiable claim rather than a slogan.

It distills production-discipline patterns into a generalized, dependency-light core — the foundation
a portfolio of small web apps is then built *through*, so the work tracker ends up holding the real,
browsable history of how each one was made.

The first such record is already here: [`docs/build-log/entity-rbac.md`](docs/build-log/entity-rbac.md)
is this repo's own RBAC feature, walked to `done` through the engine's MCP tools and gated by its own
close-checks — the system recording its own construction.

## What's here

| Crate | What it is |
|---|---|
| **`engine`** | The pure decision core: workflow-as-data (ordered states, permissive transitions, close-checks). No IO, no DB, no clock — `serde` only — so it is exhaustively unit-tested *and* compiles to `wasm32` to run client-side. |
| **`api`** | The HTTP edge (Axum + sqlx + Postgres). It loads the workflow, asks the `engine`, and writes only on `Allow`. Runtime (non-macro) sqlx, so the build needs no database. |
| **`mcp`** | A minimal MCP server (hand-rolled JSON-RPC over stdio) exposing the same operations as tools — so an AI agent drives the *exact same* validated service the HTTP edge does. |

Plus a **5-role agent orchestrator** (`.claude/`), the **schema** (`migrations/`), and a **ratcheted
CI gate** (`tools/`).

## The ideas worth a look

- **One source of truth, enforced twice.** The `engine` decides transitions; a Postgres trigger
  (`cases_guard`) independently backstops the critical ones. A unit test proves the engine *never*
  green-lights a write the trigger would reject — so the application layer and the database cannot
  drift. If the trigger ever fires under normal flow (a bug), it surfaces as a loud `500`, never a
  silent wrong answer.
- **A workflow is data.** States, transitions, and close-preconditions live in a row. The terminal
  state is `last(states)`; nothing hardcodes a status name. Illegal moves are simply absent from the
  table, so they are rejected by construction (`422`).
- **An identity/privacy gate in CI.** `tools/ci.sh` runs static audits that fail only on *new*
  violations versus a committed baseline — including a scrub audit that fails the build on any
  forbidden identity token, so the public code stays clean automatically.
- **A wasm-clean core.** A purity check compiles `engine` to `wasm32`, so the identical rules can run
  in a browser demo with no server at all.

## Run it

```sh
# Hermetic gate: pure engine + the HTTP/MCP unit tests + the wasm purity check + the audits.
# No database required.
sh tools/ci.sh

# Full integration suite against Postgres:
createdb build_engine_test
DATABASE_URL=postgres://localhost/build_engine_test cargo test -p api -p mcp -- --ignored

# Serve the HTTP API (applies migrations on boot):
DATABASE_URL=postgres://localhost/build_engine cargo run -p api

# …or the MCP tool server (speaks JSON-RPC on stdio):
DATABASE_URL=postgres://localhost/build_engine cargo run -p mcp
```

## Layout
```
build-engine/
  crates/engine/   # pure workflow-as-data core (wasm-clean)
  crates/api/      # Axum + sqlx HTTP edge
  crates/mcp/      # MCP stdio tool surface
  migrations/      # the schema (registry + workflow + cases + orchestrator state)
  .claude/         # the 5-role orchestrator (/feature) + agent configs
  tools/           # ci.sh — the ratcheted audit gate, plus the wasm purity check
  docs/            # the data model + the cases-backend design spec
```

## Status
v1 backend complete and tested — `engine` · `api` · `mcp` · the CI ratchet · the orchestrator config,
now with **membership-scoped RBAC reach** (leak-free reads; a pure resolver proven to agree with the
SQL recursive CTE). Next: the demos built *through* the engine (each opened as a Case via the MCP
tools), and the UIs that render the data this backend already stores.
