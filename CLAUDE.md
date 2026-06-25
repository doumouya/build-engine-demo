# CLAUDE.md — standing rules for this repo

A minimal, headless **build-engine**: a data-model + work-tracker (Cases) + a 5-role AI-agent
pipeline + a CI ratchet, used to build small apps *through* it. These rules keep the tree clean
and the agent chain honest.

## Case-first
For any non-trivial change (a feature, a logic bugfix, a multi-file edit — anything that produces
a commit): **open a Case first** — a spec doc at `docs/specs/<slug>.md` (or `case_create` once the
Cases MCP is wired) — then log plan → key decisions → outcome and move status
(`backlog → in_progress → in_review → done`). Skip all of this for trivial one-line / doc / typo
fixes, read-only investigation, and pure Q&A.

## Push policy (the one hard gate)
Commit freely on the working branch; **never push without the human's explicit approval.** Ops is
the sole pusher.

## Commits (parallel-safe)
Commit **only the files you changed**: `git commit -o <pathspecs>` — never `git add -A` or a bare
`git commit` (multiple agents may share the working tree). Subject: `area: imperative summary`;
body: a per-file changelog; `Co-Authored-By:` for AI contributors.

## Atomic docs
A change that adds or alters a documented surface reconciles its doc **in the same commit** (or
carries `Docs: n/a — <reason>`). The `→ done` close is gated on it.

## Conventions
Compose existing primitives rather than forking them; relative CSS units; keep the data/UI
boundary clean. Keep comments and docs truthful — fix any doc a change makes stale, same commit.

## CI
`sh tools/ci.sh` is the gate. The audit ratchet (`tools/ci-audit/check.sh`) auto-discovers every
`tools/*-audit/audit.js` and fails only on **new** violations vs the committed baseline.
