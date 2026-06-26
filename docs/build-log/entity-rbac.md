# Build log — entity-rbac (the reach resolver)

This is a **real record, not a writeup.** The build-engine recorded its own construction: the RBAC
reach resolver was built, then walked through the engine's own workflow by driving the **MCP tool
surface** — the exact interface an orchestrator agent uses — `create_case` → role comments → the
three close-checks → the status transitions.

The case reached `done` **only because the engine's close-gate allowed it**: all three
close-preconditions had to be `passed` first, or `set_status → done` would have been refused with
`close_preconditions_unmet`. Nothing here is hand-asserted — it is the case the engine produced.

> Case `CAS_277D2F5EFB9D46A99F1B0CB1F0CB166F` · workflow `feature` · final status **`done`**

## Lifecycle (each step a real MCP tool call)

| # | step | tool | result |
|---|------|------|--------|
| 1 | open the case at `backlog` | `create_case` | `CAS_277D2F5E…` |
| 2 | `backlog → in_progress` | `set_status` | ok |
| 3 | Architect · Coder · Tester · Reviewer notes | `add_comment` ×4 | ok |
| 4 | docs-reconciled · tests-green · reviewer-approved | `set_close_check` ×3 | passed |
| 5 | `in_progress → in_review` | `set_status` | ok |
| 6 | `in_review → done` | `set_status` | **allowed — gate cleared** |

Every tool call was accepted (no `isError`). An out-of-order move would not have been: e.g. a
`set_status → done` *before* the checks passed returns `close_preconditions_unmet`, and a skip like
`backlog → done` returns `invalid_transition`.

## The recorded thread

- **Architect** — reach DESCENDS: a membership grants the object plus everything scoped beneath it
  via `entity_data.scope_parent_id`, and never climbs to the parent. A pure `engine::reachable`
  (wasm-portable) mirrors a server recursive CTE; the two must agree.
- **Coder** — added `engine::reachable` (cycle-safe BFS), the reach module
  (`reachable_ids`/`can_reach`), `create_case` now writes the `entity_data` edge, and leak-free
  get/list filtering plus `GET /api/reach`.
- **Tester** — 3 integration tests green incl. `engine_resolver_agrees_with_the_sql_cte`; engine
  unit tests now total 13.
- **Reviewer** — descend-not-climb verified, leak-free 404 matches absent, the CTE `UNION` dedups
  cycles, and the pure resolver matches the SQL set. Approved.

## Close preconditions (the gate that had to clear before `done`)

| check | passed | note |
|-------|--------|------|
| `docs-reconciled` | ✅ | README status + module docs updated |
| `tests-green` | ✅ | 13 engine unit + 3 rbac integration tests green |
| `reviewer-approved` | ✅ | all checks verified |

## Why this is the point

The repository's claims about a workflow engine are demonstrated by the engine having **governed its
own feature's path to `done`**: permissive transitions, a terminal state reachable only once its
close-preconditions pass, an append-only activity trail, and an MCP surface an agent can drive. As
more features land, each leaves a build log like this one — the work tracker becomes the real history
of how the system was built.

## Raw record (`get_case`, abridged)

```json
{
  "case": {
    "entity_id": "CAS_277D2F5EFB9D46A99F1B0CB1F0CB166F",
    "title": "Build the entity-rbac reach resolver",
    "workflow_id": "feature",
    "status": "done",
    "priority": "normal"
  },
  "close_checks": [
    { "check_name": "docs-reconciled",   "passed": true, "note": "README status + module docs updated" },
    { "check_name": "tests-green",       "passed": true, "note": "13 engine unit + 3 rbac integration tests green" },
    { "check_name": "reviewer-approved", "passed": true, "note": "all checks verified" }
  ],
  "comments": [ "Architect…", "Coder…", "Tester…", "Reviewer…" ],
  "activity": [
    "case_status backlog → in_progress",
    "case_close_check docs-reconciled=true",
    "case_close_check tests-green=true",
    "case_status in_progress → in_review",
    "case_close_check reviewer-approved=true",
    "case_status in_review → done"
  ]
}
```
