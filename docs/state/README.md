# state

`current_feature.md` — the orchestrator's **resumable ledger** for an in-flight `/feature` run: the
active checklist, each gate result, and the retry/role-hop counters. If a session dies or its
context bloats, the next session resumes exactly from here. The spec itself never lives here (it
lives in `docs/specs/`); the ledger only points at it.
