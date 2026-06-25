# migrations

Postgres-first schema for the build-engine, applied in lexical order:

- **`0001_init.sql`** — the v1 schema: the registry spine (`entities`, `type_definitions`,
  `type_fields`, all-JSONB `entity_data` with the `scope_parent_id` FK), `memberships`, `events`,
  workflow-as-data (`workflows`), `cases` (+ `case_comments`, `case_attachments`,
  `case_close_checks`), and DB-persisted orchestrator state (`feature_runs`, `role_handoffs`). The
  `cases_guard` trigger is the DB backstop: it rejects an unknown status and refuses terminal entry
  until every declared close precondition has passed.
- **`0002_seed.sql`** — the `case` type + a default `feature` workflow
  (`backlog → in_progress → in_review → done`, permissive transitions, close checks
  `docs-reconciled · tests-green · reviewer-approved`).

## Applying
Apply with any Postgres client, e.g. `psql "$DATABASE_URL" -f migrations/0001_init.sql -f migrations/0002_seed.sql`.
The Cases backend (next build step) wires a migration runner + the connection and runs these as part
of its bring-up + the smoke test.

## Client-portable subset
For client-only demos (e.g. an in-browser RBAC demo on GlueSQL), the portable tables are
`entities`, `type_definitions`, `type_fields`, `entity_data`, `memberships`, `events`,
`cases`/`case_comments`. The **recursive reach resolver is server-only** (a `WITH RECURSIVE` CTE
GlueSQL won't run) — client demos precompute a flat reachable-set or resolve with bounded-depth JS.

## What's deliberately NOT here (v1 scope)
The RBAC reach **resolver** (later `entity-rbac` feature), connectors, monitoring, settings cascade,
and any file/data-cleaning domain tables. v1 is the leanest engine that lets the rest be built
through it.
