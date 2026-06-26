//! Workflow-as-data decision engine — the pure core of the build system.
//!
//! Given a workflow definition (ordered states, permissive transitions, ordered close-checks)
//! plus a case's current status, a target status, and the set of close-checks already passed,
//! [`WorkflowDef::evaluate`] returns [`Decision::Allow`] or a [`Decision::Reject`] carrying a
//! machine-readable reason.
//!
//! This crate is deliberately PURE: its only dependency is `serde`, it performs no IO, no DB
//! access, and no clock reads. That keeps it (a) exhaustively unit-testable without a database,
//! and (b) compilable to `wasm32-unknown-unknown` (see `tools/purity-check.sh`) so the exact same
//! transition rules can run client-side over an in-browser SQL engine. The server is the only
//! layer that touches Postgres; it loads the workflow row, calls [`WorkflowDef::evaluate`], and
//! writes only on [`Decision::Allow`].
//!
//! ## Agreement with the database backstop
//! The database has a `BEFORE INSERT/UPDATE` trigger (`cases_guard`, see `migrations/0001`) that
//! independently rejects an unknown status and refuses entry to the terminal state until every
//! close-check has passed. `evaluate` is a strict *superset* of that trigger computed from the same
//! rows, so the server never issues a write the trigger would reject for a reason it could have
//! reported as a clean 4xx. Transition legality (`to ∈ transitions[from]`) is the one rule only the
//! engine enforces — the trigger is a floor, not a ceiling. The invariant
//! "the engine never Allows a move the trigger would RAISE on" is proven exhaustively by
//! `engine_never_allows_what_the_trigger_would_raise`.

use std::collections::{BTreeMap, BTreeSet};

/// A workflow, stored as data (one `workflows` row) rather than hardcoded.
///
/// `states` is ORDERED and the LAST element is the terminal state. `transitions` is permissive:
/// it lists exactly the legal `from -> to` moves (forward, one-step-back, and reopen-from-terminal
/// are present as data; illegal skips are simply absent). `close_checks` are the named preconditions
/// that must all pass before a case may enter the terminal state.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct WorkflowDef {
    /// Ordered; `states.last()` is the terminal state.
    pub states: Vec<String>,
    /// `{ "<from>": ["<to>", ...] }` — a move is legal iff `to` is listed under `from`.
    pub transitions: BTreeMap<String, Vec<String>>,
    /// The state a freshly created case starts in.
    pub initial: String,
    /// Ordered names of the close preconditions gating terminal entry.
    pub close_checks: Vec<String>,
}

/// The outcome of evaluating a proposed status change.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Reject(RejectReason),
}

/// Why a proposed status change was refused. Each variant maps 1:1 to a stable wire `kind`
/// (see [`RejectReason::kind`]) and, where the DB also enforces it, to the trigger's RAISE reason.
#[derive(Debug, PartialEq, Eq)]
pub enum RejectReason {
    /// Target status is not one of the workflow's states.
    UnknownStatus(String),
    /// `to` is not reachable from `from` in one legal step.
    IllegalTransition { from: String, to: String },
    /// Entering the terminal state while one or more close-checks have not passed.
    ClosePreconditionsUnmet { missing: Vec<String> },
}

impl WorkflowDef {
    /// The terminal state = the last declared state. Empty-safe (`""` if there are no states).
    pub fn terminal(&self) -> &str {
        self.states.last().map(String::as_str).unwrap_or("")
    }

    /// Whether `s` is the terminal state. Never true for an empty workflow.
    pub fn is_terminal(&self, s: &str) -> bool {
        !self.states.is_empty() && self.terminal() == s
    }

    /// Whether `s` is one of the workflow's declared states.
    pub fn is_known(&self, s: &str) -> bool {
        self.states.iter().any(|x| x == s)
    }

    /// Whether `to` is listed under `from` in the transition table (one legal step).
    pub fn can_transition(&self, from: &str, to: &str) -> bool {
        self.transitions
            .get(from)
            .is_some_and(|tos| tos.iter().any(|t| t == to))
    }

    /// Whether moving `from -> to` *enters* the terminal state (and isn't already there).
    pub fn enters_terminal(&self, from: &str, to: &str) -> bool {
        self.is_terminal(to) && !self.is_terminal(from)
    }

    /// The close-checks not yet passed, in declared order. `passed` is the set of check names that
    /// already have `passed = true`.
    pub fn missing_close_checks(&self, passed: &BTreeSet<&str>) -> Vec<String> {
        self.close_checks
            .iter()
            .filter(|c| !passed.contains(c.as_str()))
            .cloned()
            .collect()
    }

    /// THE entry point. The order of checks MUST equal the DB trigger's so the two never disagree:
    /// 1. same-state no-op → Allow (idempotent re-drop; mirrors the trigger's no-op exemption);
    /// 2. unknown target status → Reject;
    /// 3. illegal transition (`to ∉ transitions[from]`) → Reject (engine-only rule);
    /// 4. entering terminal with unmet close-checks → Reject.
    pub fn evaluate(&self, from: &str, to: &str, passed: &BTreeSet<&str>) -> Decision {
        if from == to {
            return Decision::Allow;
        }
        if !self.is_known(to) {
            return Decision::Reject(RejectReason::UnknownStatus(to.to_string()));
        }
        if !self.can_transition(from, to) {
            return Decision::Reject(RejectReason::IllegalTransition {
                from: from.to_string(),
                to: to.to_string(),
            });
        }
        if self.enters_terminal(from, to) {
            let missing = self.missing_close_checks(passed);
            if !missing.is_empty() {
                return Decision::Reject(RejectReason::ClosePreconditionsUnmet { missing });
            }
        }
        Decision::Allow
    }
}

impl RejectReason {
    /// The stable wire identifier for this reason. The api maps this 1:1 to its `AppError.kind`,
    /// and the DB trigger RAISEs with the matching string for the rules it also enforces.
    pub fn kind(&self) -> &'static str {
        match self {
            RejectReason::UnknownStatus(_) => "unknown_status",
            RejectReason::IllegalTransition { .. } => "invalid_transition",
            RejectReason::ClosePreconditionsUnmet { .. } => "close_preconditions_unmet",
        }
    }
}

/// Compute every object id reachable by an actor, given the objects the actor is directly a member
/// of (`seed`) and the scope-parent graph as `children_of` (a parent id → its child ids). Reach
/// DESCENDS: a membership on an object grants reach to that object and everything scoped beneath it,
/// transitively — a membership on a child never climbs to its parent.
///
/// Pure and cycle-safe. This is the portable reference resolver: the server computes the identical
/// set with a recursive SQL CTE for efficiency (it never loads the whole graph), and a client demo
/// can run *this* over an in-browser graph. An integration test asserts the two agree.
pub fn reachable(
    seed: &BTreeSet<String>,
    children_of: &BTreeMap<String, Vec<String>>,
) -> BTreeSet<String> {
    let mut reached = BTreeSet::new();
    let mut stack: Vec<String> = seed.iter().cloned().collect();
    while let Some(node) = stack.pop() {
        if reached.insert(node.clone()) {
            if let Some(children) = children_of.get(&node) {
                for child in children {
                    if !reached.contains(child) {
                        stack.push(child.clone());
                    }
                }
            }
        }
    }
    reached
}

#[cfg(test)]
mod reach_tests {
    use super::*;

    fn graph() -> BTreeMap<String, Vec<String>> {
        // org → [proj1, proj2]; proj1 → [caseA]; proj2 → [caseB]
        let edge = |k: &str, v: &[&str]| (k.to_string(), v.iter().map(|s| s.to_string()).collect());
        BTreeMap::from([
            edge("org", &["proj1", "proj2"]),
            edge("proj1", &["caseA"]),
            edge("proj2", &["caseB"]),
        ])
    }
    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn membership_on_a_parent_reaches_all_descendants() {
        assert_eq!(
            reachable(&set(&["org"]), &graph()),
            set(&["org", "proj1", "proj2", "caseA", "caseB"])
        );
    }

    #[test]
    fn membership_on_a_child_does_not_climb() {
        let g = graph();
        assert_eq!(reachable(&set(&["proj1"]), &g), set(&["proj1", "caseA"]));
        assert_eq!(reachable(&set(&["caseA"]), &g), set(&["caseA"]));
    }

    #[test]
    fn empty_seed_reaches_nothing() {
        assert!(reachable(&set(&[]), &graph()).is_empty());
    }

    #[test]
    fn multiple_memberships_union() {
        assert_eq!(
            reachable(&set(&["proj1", "proj2"]), &graph()),
            set(&["proj1", "proj2", "caseA", "caseB"])
        );
    }

    #[test]
    fn cycles_terminate() {
        let edge = |k: &str, v: &[&str]| (k.to_string(), v.iter().map(|s| s.to_string()).collect());
        let g = BTreeMap::from([edge("a", &["b"]), edge("b", &["a"])]);
        assert_eq!(reachable(&set(&["a"]), &g), set(&["a", "b"]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The seeded `feature` workflow (mirrors `migrations/0002_seed.sql`).
    fn seed() -> WorkflowDef {
        let edge = |k: &str, v: &[&str]| {
            (
                k.to_string(),
                v.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            )
        };
        WorkflowDef {
            states: ["backlog", "in_progress", "in_review", "done"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            transitions: BTreeMap::from([
                edge("backlog", &["in_progress"]),
                edge("in_progress", &["backlog", "in_review"]),
                edge("in_review", &["in_progress", "done"]),
                edge("done", &["in_progress"]),
            ]),
            initial: "backlog".to_string(),
            close_checks: ["docs-reconciled", "tests-green", "reviewer-approved"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
        }
    }

    fn set<'a>(items: &[&'a str]) -> BTreeSet<&'a str> {
        items.iter().copied().collect()
    }

    const CHECKS: [&str; 3] = ["docs-reconciled", "tests-green", "reviewer-approved"];

    #[test]
    fn same_state_is_noop_allow() {
        let wf = seed();
        let none = BTreeSet::new();
        for s in &wf.states {
            assert_eq!(
                wf.evaluate(s, s, &none),
                Decision::Allow,
                "{s}->{s} should be a no-op Allow"
            );
        }
        // even the terminal state, with zero close-checks passed
        assert_eq!(wf.evaluate("done", "done", &none), Decision::Allow);
    }

    #[test]
    fn unknown_status_rejected_before_transition_check() {
        let wf = seed();
        let dec = wf.evaluate("backlog", "archived", &BTreeSet::new());
        assert_eq!(
            dec,
            Decision::Reject(RejectReason::UnknownStatus("archived".to_string()))
        );
        if let Decision::Reject(r) = dec {
            assert_eq!(r.kind(), "unknown_status");
        }
    }

    #[test]
    fn illegal_skip_rejected() {
        let wf = seed();
        let all = set(&CHECKS);
        for (from, to) in [("backlog", "done"), ("backlog", "in_review")] {
            match wf.evaluate(from, to, &all) {
                Decision::Reject(RejectReason::IllegalTransition { from: f, to: t }) => {
                    assert_eq!(f, from);
                    assert_eq!(t, to);
                }
                other => panic!("{from}->{to} expected IllegalTransition, got {other:?}"),
            }
        }
    }

    #[test]
    fn legal_forward_back_reopen_allowed() {
        let wf = seed();
        let none = BTreeSet::new();
        assert_eq!(wf.evaluate("backlog", "in_progress", &none), Decision::Allow); // forward
        assert_eq!(wf.evaluate("in_progress", "backlog", &none), Decision::Allow); // one-step-back
        assert_eq!(wf.evaluate("done", "in_progress", &none), Decision::Allow); // reopen-from-terminal
    }

    #[test]
    fn terminal_is_last_state_not_a_literal() {
        let wf = seed();
        assert!(wf.is_terminal("done"));
        assert!(!wf.is_terminal("in_review"));
        assert_eq!(wf.terminal(), "done");

        // Reordering states moves the terminal — nothing hardcodes "done".
        let mut wf2 = seed();
        wf2.states = ["backlog", "done", "in_review", "in_progress"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(wf2.terminal(), "in_progress");
        assert!(wf2.is_terminal("in_progress"));
        assert!(!wf2.is_terminal("done"));
    }

    #[test]
    fn close_gate_powerset_reports_exact_missing() {
        let wf = seed();
        for mask in 0u8..8 {
            let passed_vec: Vec<&str> = CHECKS
                .iter()
                .enumerate()
                .filter(|(i, _)| (mask & (1u8 << i)) != 0)
                .map(|(_, n)| *n)
                .collect();
            let passed = set(&passed_vec);
            let dec = wf.evaluate("in_review", "done", &passed);
            if mask == 0b111 {
                assert_eq!(dec, Decision::Allow, "all checks passed should Allow");
            } else {
                let expected: Vec<String> = wf
                    .close_checks
                    .iter()
                    .filter(|c| !passed.contains(c.as_str()))
                    .cloned()
                    .collect();
                assert_eq!(
                    dec,
                    Decision::Reject(RejectReason::ClosePreconditionsUnmet { missing: expected }),
                    "mask {mask:03b} should reject with the exact unmet checks in order"
                );
            }
        }
    }

    #[test]
    fn reopen_from_terminal_is_not_gated() {
        let wf = seed();
        // done -> in_progress with NO checks passed is Allow: enters_terminal is false on reopen.
        assert_eq!(
            wf.evaluate("done", "in_progress", &BTreeSet::new()),
            Decision::Allow
        );
        assert!(!wf.enters_terminal("done", "in_progress"));
    }

    /// The agreement proof. The DB `cases_guard` trigger enforces ONLY unknown_status and the
    /// close-gate on entering terminal (with a no-op self-update exemption); it does NOT enforce
    /// transition legality. The critical safety property is therefore: the engine must NEVER
    /// `Allow` a move that the trigger would RAISE on. We check it over a universe that includes
    /// unknown states and the empty string, across the full close-check powerset.
    #[test]
    fn engine_never_allows_what_the_trigger_would_raise() {
        let wf = seed();
        let universe = [
            "backlog",
            "in_progress",
            "in_review",
            "done",
            "archived",
            "nope",
            "",
        ];
        for from in universe {
            for to in universe {
                for mask in 0u8..8 {
                    let passed_vec: Vec<&str> = CHECKS
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| (mask & (1u8 << i)) != 0)
                        .map(|(_, n)| *n)
                        .collect();
                    let passed = set(&passed_vec);

                    // Reference model of the trigger's RAISE predicate:
                    let trigger_raises = if from == to {
                        false // no-op self-update exemption
                    } else if !wf.is_known(to) {
                        true // unknown_status
                    } else if wf.is_terminal(to) && !wf.is_terminal(from) {
                        // entering terminal: raise unless every close-check has passed
                        !CHECKS.iter().all(|c| passed.contains(c))
                    } else {
                        false
                    };

                    if wf.evaluate(from, to, &passed) == Decision::Allow {
                        assert!(
                            !trigger_raises,
                            "engine ALLOWED {from}->{to} (passed={passed:?}) but the trigger would RAISE"
                        );
                    }
                }
            }
        }
    }
}
