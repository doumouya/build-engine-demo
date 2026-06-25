-- 0002_seed.sql — minimal bootstrap: the `case` type + a default `feature` workflow.
--
-- The workflow encodes the invariants as DATA: ordered states (last = terminal), PERMISSIVE
-- transitions (forward + one-step-back + reopen-from-terminal; illegal skips like backlog→done are
-- simply absent, so the engine rejects them), and the named close preconditions the terminal entry
-- requires.

insert into type_definitions (type_id, id_prefix, display_name, scope_parents) values
  ('case', 'CAS', 'Case', '[]');

insert into workflows (workflow_id, states, transitions, initial, close_checks) values
  ('feature',
   '["backlog","in_progress","in_review","done"]',
   '{
      "backlog":     ["in_progress"],
      "in_progress": ["backlog","in_review"],
      "in_review":   ["in_progress","done"],
      "done":        ["in_progress"]
    }',
   'backlog',
   '["docs-reconciled","tests-green","reviewer-approved"]');
