//! The cases service + HTTP surface.
//!
//! Every mutation validates against the pure [`engine`] BEFORE it writes, so the database trigger
//! is only ever a backstop (a trigger RAISE during a normal write means engine/trigger drift and
//! surfaces as a loud 500). The service functions hold all the logic; the handlers are thin, and an
//! MCP surface will call the same `svc` functions so it shares byte-identical validation.

use std::collections::BTreeSet;

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, patch, post, put};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::error::AppError;
use crate::{db, events, id, reach, AppState};

// ── request bodies ────────────────────────────────────────────────────────────
#[derive(Deserialize)]
pub struct CreateCaseBody {
    pub title: String,
    #[serde(default)]
    pub workflow_id: Option<String>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub scope_parent_id: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize)]
pub struct AddCommentBody {
    pub body: String,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize)]
pub struct SetStatusBody {
    pub status: String,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize)]
pub struct SetCloseCheckBody {
    pub passed: bool,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize)]
pub struct AssignBody {
    #[serde(default)]
    pub assignee_id: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub status: Option<String>,
    pub scope_parent: Option<String>,
    pub page: Option<i64>,
    pub size: Option<i64>,
    /// When present, results are filtered to what this actor can reach (leak-free); absent = permissive.
    pub actor_id: Option<String>,
}

/// The viewing actor for a read, supplied as a query param (`?actor_id=...`). Absent = permissive.
#[derive(Deserialize)]
pub struct ActorQuery {
    pub actor_id: Option<String>,
}

// ── response rows ─────────────────────────────────────────────────────────────
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CaseRow {
    pub entity_id: String,
    pub title: String,
    pub workflow_id: String,
    pub status: String,
    pub priority: String,
    pub assignee_id: Option<String>,
    pub scope_parent_id: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CommentRow {
    pub id: i64,
    pub author_id: Option<String>,
    pub body: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct CloseCheckRow {
    pub check_name: String,
    pub passed: bool,
    pub note: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ActivityRow {
    pub kind: String,
    pub at: DateTime<Utc>,
    pub payload: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct CaseDetail {
    pub case: CaseRow,
    pub comments: Vec<CommentRow>,
    pub close_checks: Vec<CloseCheckRow>,
    pub activity: Vec<ActivityRow>,
}

#[derive(Debug, Serialize)]
pub struct CaseList {
    pub items: Vec<CaseRow>,
    pub total: i64,
    pub page: i64,
    pub size: i64,
}

const CASE_SELECT: &str = "select c.entity_id, c.title, c.workflow_id, c.status, c.priority, \
     c.assignee_id, c.scope_parent_id, e.created_at \
     from cases c join entities e on e.id = c.entity_id";

// ── helpers ───────────────────────────────────────────────────────────────────
fn map_reject(r: &engine::RejectReason) -> AppError {
    use engine::RejectReason::*;
    match r {
        UnknownStatus(s) => AppError::unprocessable(r.kind(), format!("'{s}' is not a valid status")),
        IllegalTransition { from, to } => {
            AppError::unprocessable(r.kind(), format!("cannot move from '{from}' to '{to}'"))
        }
        ClosePreconditionsUnmet { missing } => AppError::Unprocessable {
            kind: r.kind(),
            message: format!("close preconditions unmet: {}", missing.join(", ")),
            missing: Some(missing.clone()),
        },
    }
}

/// Hydrate a [`engine::WorkflowDef`] from its `workflows` row. `None` means no such workflow.
async fn load_workflow(pool: &PgPool, workflow_id: &str) -> Result<Option<engine::WorkflowDef>, AppError> {
    let row: Option<(serde_json::Value, serde_json::Value, String, serde_json::Value)> =
        sqlx::query_as("select states, transitions, initial, close_checks from workflows where workflow_id = $1")
            .bind(workflow_id)
            .fetch_optional(pool)
            .await?;
    let Some((states, transitions, initial, close_checks)) = row else {
        return Ok(None);
    };
    // Each field's concrete type drives `from_value`'s inference (states/close_checks are
    // Vec<String>, transitions is a BTreeMap), so they decode independently.
    Ok(Some(engine::WorkflowDef {
        states: serde_json::from_value(states)
            .map_err(|e| AppError::Internal(format!("workflow states: {e}")))?,
        transitions: serde_json::from_value(transitions)
            .map_err(|e| AppError::Internal(format!("workflow transitions: {e}")))?,
        initial,
        close_checks: serde_json::from_value(close_checks)
            .map_err(|e| AppError::Internal(format!("workflow close_checks: {e}")))?,
    }))
}

async fn passed_checks(pool: &PgPool, case_id: &str) -> Result<Vec<String>, AppError> {
    let rows: Vec<(String,)> =
        sqlx::query_as("select check_name from case_close_checks where case_id = $1 and passed = true")
            .bind(case_id)
            .fetch_all(pool)
            .await?;
    Ok(rows.into_iter().map(|(n,)| n).collect())
}

async fn fetch_case(pool: &PgPool, id: &str) -> Result<Option<CaseRow>, AppError> {
    let sql = format!("{CASE_SELECT} where c.entity_id = $1");
    Ok(sqlx::query_as::<_, CaseRow>(&sql).bind(id).fetch_optional(pool).await?)
}

async fn entity_exists(pool: &PgPool, id: &str) -> Result<bool, AppError> {
    let (exists,): (bool,) = sqlx::query_as("select exists(select 1 from entities where id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

async fn case_exists(pool: &PgPool, id: &str) -> Result<bool, AppError> {
    let (exists,): (bool,) = sqlx::query_as("select exists(select 1 from cases where entity_id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

// ── services (the real logic; HTTP handlers and the future MCP surface both call these) ──
pub async fn create_case(
    pool: &PgPool,
    body: CreateCaseBody,
    actor: Option<&str>,
) -> Result<CaseDetail, AppError> {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(AppError::unprocessable("title_required", "title is required"));
    }
    let workflow_id = body.workflow_id.as_deref().unwrap_or("feature");
    let def = load_workflow(pool, workflow_id)
        .await?
        .ok_or(AppError::NotFound("unknown_workflow"))?;
    if let Some(a) = body.assignee_id.as_deref() {
        if !entity_exists(pool, a).await? {
            return Err(AppError::bad_request("invalid_assignee", "assignee does not exist"));
        }
    }
    if let Some(sp) = body.scope_parent_id.as_deref() {
        if !entity_exists(pool, sp).await? {
            return Err(AppError::NotFound("scope_not_found"));
        }
    }

    let id = id::new_id("CAS");
    let status = def.initial.clone();
    let priority = body.priority.as_deref().unwrap_or("normal");

    let mut tx = pool.begin().await?;
    db::register_entity(&mut tx, &id, "case", actor).await?;
    // The generic registry row, so the case participates in the reach graph (entity_data.scope_parent_id
    // is the edge the resolver climbs). It mirrors cases.scope_parent_id.
    sqlx::query("insert into entity_data (entity_id, type_id, scope_parent_id) values ($1, 'case', $2)")
        .bind(&id)
        .bind(&body.scope_parent_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "insert into cases (entity_id, title, workflow_id, status, priority, assignee_id, scope_parent_id) \
         values ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(&id)
    .bind(title)
    .bind(workflow_id)
    .bind(&status)
    .bind(priority)
    .bind(&body.assignee_id)
    .bind(&body.scope_parent_id)
    .execute(&mut *tx)
    .await?;
    for name in &def.close_checks {
        sqlx::query("insert into case_close_checks (case_id, check_name, passed) values ($1, $2, false)")
            .bind(&id)
            .bind(name)
            .execute(&mut *tx)
            .await?;
    }
    if let Some(a) = actor {
        db::grant_owner(&mut tx, &id, a).await?;
    }
    tx.commit().await?;

    events::emit(pool, &id, actor, "case_create", json!({ "case": &id, "type": "case" }));
    get_case(pool, &id, None).await
}

/// Read a case. When `actor` is present, an object the actor cannot reach is denied with the same
/// `404 not_found` as an absent one (leak-free) — so reach never leaks existence. `actor = None` is
/// permissive (returns the case if it exists), which is the default until an auth layer is added.
pub async fn get_case(pool: &PgPool, id: &str, actor: Option<&str>) -> Result<CaseDetail, AppError> {
    let case = fetch_case(pool, id).await?.ok_or(AppError::not_found())?;
    if let Some(a) = actor {
        if !reach::can_reach(pool, a, id).await? {
            return Err(AppError::not_found());
        }
    }
    let comments = sqlx::query_as::<_, CommentRow>(
        "select id, author_id, body, at from case_comments where case_id = $1 order by at asc, id asc",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let close_checks = sqlx::query_as::<_, CloseCheckRow>(
        "select check_name, passed, note, at from case_close_checks where case_id = $1 order by check_name asc",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    let activity = sqlx::query_as::<_, ActivityRow>(
        "select kind, at, payload from events where entity_id = $1 order by at desc, id desc limit 20",
    )
    .bind(id)
    .fetch_all(pool)
    .await?;
    Ok(CaseDetail {
        case,
        comments,
        close_checks,
        activity,
    })
}

pub async fn list_cases(pool: &PgPool, q: ListQuery) -> Result<CaseList, AppError> {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.size.unwrap_or(50).clamp(1, 200);
    let offset = (page - 1) * size;
    // Reach filter: with an actor, restrict to the ids they can reach (an empty set ⇒ sees nothing);
    // without one, permissive. An unknown status filter simply matches nothing — the list spans
    // workflows, so it isn't validated against any single set.
    let reachable: Option<Vec<String>> = match q.actor_id.as_deref() {
        Some(a) => Some(reach::reachable_ids(pool, a).await?),
        None => None,
    };
    let sql = format!(
        "{CASE_SELECT} \
         where ($1::text is null or c.status = $1) and ($2::text is null or c.scope_parent_id = $2) \
           and ($5::text[] is null or c.entity_id = any($5)) \
         order by e.created_at desc limit $3 offset $4"
    );
    let items = sqlx::query_as::<_, CaseRow>(&sql)
        .bind(&q.status)
        .bind(&q.scope_parent)
        .bind(size)
        .bind(offset)
        .bind(&reachable)
        .fetch_all(pool)
        .await?;
    let (total,): (i64,) = sqlx::query_as(
        "select count(*) from cases c \
         where ($1::text is null or c.status = $1) and ($2::text is null or c.scope_parent_id = $2) \
           and ($3::text[] is null or c.entity_id = any($3))",
    )
    .bind(&q.status)
    .bind(&q.scope_parent)
    .bind(&reachable)
    .fetch_one(pool)
    .await?;
    Ok(CaseList {
        items,
        total,
        page,
        size,
    })
}

pub async fn add_comment(
    pool: &PgPool,
    id: &str,
    body: &str,
    actor: Option<&str>,
) -> Result<CommentRow, AppError> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(AppError::unprocessable("empty_comment", "comment body is required"));
    }
    if !case_exists(pool, id).await? {
        return Err(AppError::not_found());
    }
    let row: CommentRow = sqlx::query_as(
        "insert into case_comments (case_id, author_id, body) values ($1, $2, $3) \
         returning id, author_id, body, at",
    )
    .bind(id)
    .bind(actor)
    .bind(trimmed)
    .fetch_one(pool)
    .await?;
    events::emit(pool, id, actor, "case_comment", json!({ "case": id }));
    Ok(row)
}

pub async fn set_status(
    pool: &PgPool,
    id: &str,
    to: &str,
    actor: Option<&str>,
) -> Result<CaseRow, AppError> {
    let row: Option<(String, String)> =
        sqlx::query_as("select status, workflow_id from cases where entity_id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?;
    let (from, workflow_id) = row.ok_or(AppError::not_found())?;

    // A same-state re-drop changes nothing: no write, no event (mirrors the trigger's no-op carve-out).
    if to != from {
        let def = load_workflow(pool, &workflow_id)
            .await?
            .ok_or(AppError::not_found())?;
        // Only entering the terminal state needs the close-check set; skip the query otherwise.
        let passed_vec = if def.enters_terminal(&from, to) {
            passed_checks(pool, id).await?
        } else {
            Vec::new()
        };
        let passed: BTreeSet<&str> = passed_vec.iter().map(String::as_str).collect();

        if let engine::Decision::Reject(r) = def.evaluate(&from, to, &passed) {
            return Err(map_reject(&r)); // 4xx BEFORE any write
        }

        sqlx::query("update cases set status = $1 where entity_id = $2")
            .bind(to)
            .bind(id)
            .execute(pool)
            .await?;
        events::emit(
            pool,
            id,
            actor,
            "case_status",
            json!({ "case": id, "from": from, "to": to }),
        );
    }

    fetch_case(pool, id).await?.ok_or(AppError::not_found())
}

pub async fn set_close_check(
    pool: &PgPool,
    id: &str,
    name: &str,
    passed: bool,
    note: Option<String>,
    actor: Option<&str>,
) -> Result<CloseCheckRow, AppError> {
    let row: Option<(String,)> = sqlx::query_as("select workflow_id from cases where entity_id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    let (workflow_id,) = row.ok_or(AppError::not_found())?;
    let def = load_workflow(pool, &workflow_id)
        .await?
        .ok_or(AppError::not_found())?;
    if !def.close_checks.iter().any(|c| c == name) {
        return Err(AppError::unprocessable(
            "unknown_check",
            format!("'{name}' is not a close check of this workflow"),
        ));
    }
    let row: CloseCheckRow = sqlx::query_as(
        "insert into case_close_checks (case_id, check_name, passed, note) values ($1, $2, $3, $4) \
         on conflict (case_id, check_name) do update set passed = excluded.passed, note = excluded.note, at = now() \
         returning check_name, passed, note, at",
    )
    .bind(id)
    .bind(name)
    .bind(passed)
    .bind(&note)
    .fetch_one(pool)
    .await?;
    events::emit(
        pool,
        id,
        actor,
        "case_close_check",
        json!({ "case": id, "check_name": name, "passed": passed }),
    );
    Ok(row)
}

pub async fn assign(
    pool: &PgPool,
    id: &str,
    assignee_id: Option<String>,
    actor: Option<&str>,
) -> Result<CaseRow, AppError> {
    if !case_exists(pool, id).await? {
        return Err(AppError::not_found());
    }
    if let Some(a) = assignee_id.as_deref() {
        if !entity_exists(pool, a).await? {
            return Err(AppError::bad_request("invalid_assignee", "assignee does not exist"));
        }
    }
    sqlx::query("update cases set assignee_id = $1 where entity_id = $2")
        .bind(&assignee_id)
        .bind(id)
        .execute(pool)
        .await?;
    events::emit(
        pool,
        id,
        actor,
        "case_assign",
        json!({ "case": id, "assignee_id": assignee_id }),
    );
    fetch_case(pool, id).await?.ok_or(AppError::not_found())
}

// ── HTTP handlers (thin) ──────────────────────────────────────────────────────
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/cases", post(create_h).get(list_h))
        .route("/api/cases/:id", get(get_h))
        .route("/api/cases/:id/comments", post(comment_h))
        .route("/api/cases/:id/status", patch(status_h))
        .route("/api/cases/:id/checks/:name", put(check_h))
        .route("/api/cases/:id/assignee", put(assign_h))
        .route("/api/reach", get(reach_h))
}

async fn create_h(
    State(st): State<AppState>,
    Json(body): Json<CreateCaseBody>,
) -> Result<(StatusCode, Json<CaseDetail>), AppError> {
    let actor = body.actor_id.clone();
    let detail = create_case(&st.pool, body, actor.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(detail)))
}

async fn list_h(
    State(st): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<CaseList>, AppError> {
    Ok(Json(list_cases(&st.pool, q).await?))
}

async fn get_h(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Query(aq): Query<ActorQuery>,
) -> Result<Json<CaseDetail>, AppError> {
    Ok(Json(get_case(&st.pool, &id, aq.actor_id.as_deref()).await?))
}

/// `GET /api/reach?actor_id=...` — the set of object ids the actor can reach (a small window onto the
/// resolver that powers leak-free reads).
async fn reach_h(
    State(st): State<AppState>,
    Query(aq): Query<ActorQuery>,
) -> Result<Json<Value>, AppError> {
    let actor = aq
        .actor_id
        .ok_or_else(|| AppError::unprocessable("actor_required", "actor_id query param is required"))?;
    let reachable = reach::reachable_ids(&st.pool, &actor).await?;
    Ok(Json(json!({ "actor_id": actor, "reachable": reachable })))
}

async fn comment_h(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AddCommentBody>,
) -> Result<(StatusCode, Json<CommentRow>), AppError> {
    let row = add_comment(&st.pool, &id, &body.body, body.actor_id.as_deref()).await?;
    Ok((StatusCode::CREATED, Json(row)))
}

async fn status_h(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<SetStatusBody>,
) -> Result<Json<CaseRow>, AppError> {
    Ok(Json(set_status(&st.pool, &id, &body.status, body.actor_id.as_deref()).await?))
}

async fn check_h(
    State(st): State<AppState>,
    Path((id, name)): Path<(String, String)>,
    Json(body): Json<SetCloseCheckBody>,
) -> Result<Json<CloseCheckRow>, AppError> {
    Ok(Json(
        set_close_check(&st.pool, &id, &name, body.passed, body.note.clone(), body.actor_id.as_deref()).await?,
    ))
}

async fn assign_h(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<AssignBody>,
) -> Result<Json<CaseRow>, AppError> {
    Ok(Json(assign(&st.pool, &id, body.assignee_id.clone(), body.actor_id.as_deref()).await?))
}
