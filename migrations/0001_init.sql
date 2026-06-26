-- 0001_init.sql — build-engine v1 schema (Postgres-first).
--
-- A self-hosting build system: the registry stores its own build process. The
-- entities / type_definitions / type_fields / entity_data / memberships / events / cases subset is
-- the GlueSQL-portable core for client-only demos; the recursive reach resolver (later feature) is
-- server-only. Declaring a new object type is a ROW here, never a migration.

-- ── registry spine ───────────────────────────────────────────────────────────
create table type_definitions (
  type_id       text primary key,
  id_prefix     text not null unique,            -- e.g. CAS, USR, <TYPE>; makes kind(id) a lookup
  display_name  text not null,
  scope_parents jsonb not null default '[]'      -- ordered parent fields the reach resolver climbs
);

create table entities (
  id          text primary key,                  -- <PREFIX>_<hex>
  type        text not null references type_definitions(type_id),
  created_at  timestamptz not null default now(),
  created_by  text                               -- actor entity id (nullable during bootstrap)
);
create index entities_type_idx on entities(type);

create table type_fields (
  type_id   text not null references type_definitions(type_id) on delete cascade,
  field     text not null,
  kind      text not null,                        -- text|int|bool|date|json|ref|...
  required  boolean not null default false,
  editable  boolean not null default true,
  ord       integer not null default 0,
  primary key (type_id, field)
);

-- all-JSONB store (no typed subtype tables in v1): "a type is a row, zero migrations".
create table entity_data (
  entity_id       text primary key references entities(id) on delete cascade,
  type_id         text not null references type_definitions(type_id),
  data            jsonb not null default '{}',
  scope_parent_id text references entities(id) on delete set null   -- real reach edge (IDOR backstop)
);
create index entity_data_scope_parent_idx on entity_data(scope_parent_id);
create index entity_data_type_idx on entity_data(type_id);

-- ── access edge + org graph (schema only in v1; the resolver is the later entity-rbac feature) ──
-- Ownership is a membership row, never an owner_id column ("no object without an owner").
create table memberships (
  object_id    text not null references entities(id) on delete cascade,
  member_id    text not null references entities(id) on delete cascade,
  role         text not null check (role in ('viewer','member','admin','owner')),
  context_role text not null default '',
  created_at   timestamptz not null default now(),
  primary key (object_id, member_id, role, context_role)
);
create index memberships_member_idx on memberships(member_id);

-- ── append-only audit ────────────────────────────────────────────────────────
-- entity_id is a SOFT reference to whatever the event is about — an entity, a feature run, a case —
-- so it is deliberately NOT a foreign key: the audit log records heterogeneous subjects (and must
-- survive the subject being deleted). The payload carries any extra context.
create table events (
  id        bigserial primary key,
  entity_id text,
  actor_id  text,
  kind      text not null,
  at        timestamptz not null default now(),
  payload   jsonb not null default '{}'
);
create index events_entity_idx on events(entity_id);

-- ── workflow-as-DATA (the version only ever hardcoded before) ──────────────────
create table workflows (
  workflow_id  text primary key,
  states       jsonb not null,        -- ORDERED array; the LAST element is terminal
  transitions  jsonb not null,        -- { "<from>": ["<to>", ...] }  (permissive: fwd + back + reopen)
  initial      text not null,
  close_checks jsonb not null default '[]'   -- ordered named close preconditions
);

-- ── cases (a case IS an entity) ───────────────────────────────────────────────
create table cases (
  entity_id       text primary key references entities(id) on delete cascade,
  title           text not null,
  workflow_id     text not null references workflows(workflow_id),
  status          text not null,
  priority        text not null default 'normal',
  assignee_id     text references entities(id) on delete set null,
  scope_parent_id text references entities(id) on delete set null
);
create index cases_status_idx on cases(status);

create table case_comments (
  id        bigserial primary key,
  case_id   text not null references cases(entity_id) on delete cascade,
  author_id text,
  body      text not null default '',
  at        timestamptz not null default now()
);
create index case_comments_case_idx on case_comments(case_id);

create table case_attachments (
  id       bigserial primary key,
  case_id  text not null references cases(entity_id) on delete cascade,
  name     text not null,
  blob_ref text not null,
  at       timestamptz not null default now()
);

-- per-case state of each named close precondition (the engine flips passed=true as a gate clears).
create table case_close_checks (
  case_id    text not null references cases(entity_id) on delete cascade,
  check_name text not null,
  passed     boolean not null default false,
  note       text,
  at         timestamptz not null default now(),
  primary key (case_id, check_name)
);

-- ── orchestrator state in the DB (source of truth; the file ledger is a derived cache) ──
create table feature_runs (
  id         text primary key,
  case_id    text references cases(entity_id) on delete set null,
  title      text not null,
  phase      text not null default 'spec',
  status     text not null default 'active',
  started_at timestamptz not null default now(),
  updated_at timestamptz not null default now()
);

create table role_handoffs (
  id              bigserial primary key,
  feature_run_id  text not null references feature_runs(id) on delete cascade,
  role            text not null check (role in ('architect','tester','coder','reviewer','ops')),
  attempt         integer not null default 1,
  gate            text,                      -- which gate this hop was about
  outcome         text,                      -- pass | fail | test-drift | escalate
  kind            text not null default 'gate',  -- gate | test-drift  (circuit-breaker discriminator)
  retries         integer not null default 0,
  hops            integer not null default 0,
  note            text,
  at              timestamptz not null default now()
);
create index role_handoffs_run_idx on role_handoffs(feature_run_id);

-- ── DB backstops: an engine-bypassing write can't land an unknown status (Δ3.4); entering the
--    terminal state requires every declared close precondition to have passed (Δ2 / Δ3.3). Every
--    RAISE uses the pinned SQLSTATE 'WG001' so the app can distinguish "guard fired during a write
--    the engine had ALLOWED" (= engine/trigger drift → a loud 500 workflow_guard) from ordinary
--    validation it already reports as a clean 4xx. ──
create or replace function cases_guard() returns trigger as $$
declare
  wf_states jsonb;
  wf_checks jsonb;
  terminal  text;
  missing   text;
begin
  select states, close_checks into wf_states, wf_checks
    from workflows where workflow_id = new.workflow_id;
  if wf_states is null then
    raise exception 'unknown_workflow: %', new.workflow_id using errcode = 'WG001';
  end if;
  if not (wf_states ? new.status) then
    raise exception 'unknown_status: % is not a state of workflow %', new.status, new.workflow_id using errcode = 'WG001';
  end if;
  terminal := wf_states ->> (jsonb_array_length(wf_states) - 1);
  if new.status = terminal and not (tg_op = 'UPDATE' and old.status = new.status) then
    select string_agg(c.value, ', ') into missing
      from jsonb_array_elements_text(wf_checks) as c(value)
     where not exists (
       select 1 from case_close_checks k
        where k.case_id = new.entity_id and k.check_name = c.value and k.passed
     );
    if missing is not null then
      raise exception 'close_preconditions_unmet: %', missing using errcode = 'WG001';
    end if;
  end if;
  return new;
end;
$$ language plpgsql;

create trigger cases_guard_trg before insert or update on cases
  for each row execute function cases_guard();
