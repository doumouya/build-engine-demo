# Data model — v0 (discussion seed)

> **v0 to be reframed by the team.** This is a starting sketch of the minimal, generalized schema for the headless
> build-engine — *not* a finished design. The goal is the leanest correct "database-as-a-framework" core, with no
> baggage. Postgres-first (server); a subset must also run in an in-browser SQL engine (client) for the demos.
> Open questions are listed at the end — the data-model and RBAC/registry owners should push back hard.

## The spine — registry ("declaring a type is a row, not a migration")
- **`entities`** — the one polymorphic id space. Every object registers here first; deletes cascade through it.
  `id (text pk) · type (fk → type_definitions) · created_at · created_by`
- **`type_definitions`** — the catalog. One row = one object type, auto-wiring the surface below.
  `type_id (pk) · id_prefix (unique) · display_name · scope_parents (jsonb: ordered parent fields for the reach cascade)`
- **`type_fields`** — typed fields per type (shape + per-field editability/order).
  `type_id · field · kind · required · editable · ord`
- **`entity_data`** — generic JSONB store for custom types (builtins may use typed tables instead).
  `entity_id (pk, fk → entities) · type_id · data (jsonb) · scope_parent_id (fk → entities, the real reach edge)`

## Access edges (schema in v1; resolver/admin = later feature)
- **`memberships`** — the single polymorphic access edge AND the org graph.
  `object_id (fk → entities) · member_id (fk → entities) · role (viewer<member<admin<owner) · context_role · created_at`
  Invariant: *no object without an owner* (creator → owner membership in the same tx).
  *Reach* later resolves via one generated `WITH RECURSIVE` CTE compiled from each type's `scope_parents` — **not built
  in v1**, but the schema must support it.

## Audit
- **`events`** — append-only audit/event log (time-partition later).
  `id · entity_id · actor_id · kind · at · payload (jsonb)`

## Cases (workflow-as-data — the engine's own work tracker)
- **`workflows`** — a workflow definition as data: states + allowed transitions (so illegal transitions → `422`).
  `workflow_id (pk) · states (jsonb ordered) · transitions (jsonb: from→[to]) · initial_state`
- **`cases`** — a case is an entity of type `case`.
  `entity_id (pk, fk → entities) · title · workflow_id (fk) · status · priority · assignee_id · scope_parent_id`
- **`case_comments`** — `id · case_id (fk) · author_id · body (markdown text) · at`
- **`case_attachments`** — `id · case_id (fk) · name · blob_ref · at`  *(optional in v1)*

## Orchestrator state (the user wants this in the DB, not only files)
- **`feature_runs`** — one row per `/feature` run (the resumable ledger).
  `id (pk) · case_id (fk) · title · phase · status · started_at · updated_at`
- **`role_handoffs`** — the Case-ID handoff bus + circuit-breaker counters.
  `id · feature_run_id (fk) · role (architect|tester|coder|reviewer|ops) · attempt · retries · hops · note · at`
  Circuit breaker: enforce ≤3 retries/gate, ≤8 role-hops/feature from these counters.

## Open questions (please reframe)
1. **Generic vs typed storage:** keep `entities` + `entity_data` (JSONB) as the generic store, with builtins (cases,
   users, teams) in typed tables? Or all-JSONB? What's the lean generalized choice (both were used in the source)?
2. **Client/server parity:** which tables must run in the in-browser SQL engine (GlueSQL) for the client-only demos
   (e.g. `entity-rbac`)? Define the portable subset + the Postgres-only extras.
3. **`scope_parents` / recursive CTE:** confirm the schema shape that lets the *later* resolver generate one shared
   recursive CTE; what's the minimal column set now so we don't rework it later.
4. **Workflow storage:** is a `workflows` table (states + transitions as JSONB) the right "workflow-as-data" shape, or
   keep transitions per-type? How are multiple source-keyed workflows (e.g. internal vs external) modelled?
5. **Orchestrator state in DB vs the file ledger:** the file ledger (`current_feature.md`) is the existing convention —
   do we mirror it into `feature_runs`/`role_handoffs`, replace it, or keep both? What does the MCP need?
6. **Naming:** all table/column names must be generic and free of any source-project identity. Flag any that still leak domain meaning.
7. **Baggage:** what in the source's `init` schema should we NOT carry over (settings cascade? connectors? monitoring?
   designer? — likely deferred), to keep v1 minimal.

*Owners: data-model + RBAC/registry, please edit this file (attributed) or comment on the Case
(`docs/internal/specs/portfolio-foundation.md` in the source repo).*
