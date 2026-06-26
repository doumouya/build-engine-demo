//! Feature runs + role handoffs — the orchestrator's own state, persisted.
//!
//! A `/feature` run is a row in `feature_runs`; each time a role hands off (architect → tester → …),
//! it records a `role_handoffs` row carrying the circuit-breaker fields (attempt, retries, hops, the
//! gate, the outcome). The file ledger the orchestrator keeps is a derived cache; this table is the
//! source of truth, and the read endpoints feed a Monitor view of how the system built itself.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::PgPool;

use crate::error::AppError;
use crate::{events, id, AppState};

const ROLES: [&str; 5] = ["architect", "tester", "coder", "reviewer", "ops"];

#[derive(Deserialize)]
pub struct StartRunBody {
    pub title: String,
    #[serde(default)]
    pub case_id: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize)]
pub struct HandoffBody {
    pub role: String,
    #[serde(default)]
    pub gate: Option<String>,
    #[serde(default)]
    pub outcome: Option<String>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub attempt: Option<i32>,
    #[serde(default)]
    pub retries: Option<i32>,
    #[serde(default)]
    pub hops: Option<i32>,
    #[serde(default)]
    pub note: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateRunBody {
    #[serde(default)]
    pub phase: Option<String>,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

#[derive(Deserialize, Default)]
pub struct ListQuery {
    pub status: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct RunRow {
    pub id: String,
    pub case_id: Option<String>,
    pub title: String,
    pub phase: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct HandoffRow {
    pub id: i64,
    pub role: String,
    pub attempt: i32,
    pub gate: Option<String>,
    pub outcome: Option<String>,
    pub kind: String,
    pub retries: i32,
    pub hops: i32,
    pub note: Option<String>,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct RunDetail {
    pub run: RunRow,
    pub handoffs: Vec<HandoffRow>,
}

async fn run_exists(pool: &PgPool, id: &str) -> Result<bool, AppError> {
    let (exists,): (bool,) = sqlx::query_as("select exists(select 1 from feature_runs where id = $1)")
        .bind(id)
        .fetch_one(pool)
        .await?;
    Ok(exists)
}

async fn fetch_run(pool: &PgPool, id: &str) -> Result<Option<RunRow>, AppError> {
    Ok(sqlx::query_as::<_, RunRow>(
        "select id, case_id, title, phase, status, started_at, updated_at from feature_runs where id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

// ── services ──────────────────────────────────────────────────────────────────
pub async fn start_run(pool: &PgPool, body: StartRunBody, actor: Option<&str>) -> Result<RunDetail, AppError> {
    let title = body.title.trim();
    if title.is_empty() {
        return Err(AppError::unprocessable("title_required", "title is required"));
    }
    // a blank case_id means "no case link", not a guaranteed 400
    let case_id = body.case_id.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(c) = case_id {
        let (ok,): (bool,) = sqlx::query_as("select exists(select 1 from cases where entity_id = $1)")
            .bind(c)
            .fetch_one(pool)
            .await?;
        if !ok {
            return Err(AppError::bad_request("invalid_case", "case_id does not exist"));
        }
    }
    let id = id::new_id("RUN");
    sqlx::query("insert into feature_runs (id, case_id, title) values ($1, $2, $3)")
        .bind(&id)
        .bind(case_id)
        .bind(title)
        .execute(pool)
        .await?;
    events::emit(pool, &id, actor, "run_start", json!({ "run": &id, "title": title }));
    get_run(pool, &id).await
}

pub async fn record_handoff(pool: &PgPool, run_id: &str, body: HandoffBody, actor: Option<&str>) -> Result<HandoffRow, AppError> {
    if !ROLES.contains(&body.role.as_str()) {
        return Err(AppError::unprocessable("invalid_role", format!("'{}' is not a role", body.role)));
    }
    if !run_exists(pool, run_id).await? {
        return Err(AppError::not_found());
    }
    let row: HandoffRow = sqlx::query_as(
        "insert into role_handoffs (feature_run_id, role, gate, outcome, kind, attempt, retries, hops, note) \
         values ($1, $2, $3, $4, coalesce($5,'gate'), coalesce($6,1), coalesce($7,0), coalesce($8,0), $9) \
         returning id, role, attempt, gate, outcome, kind, retries, hops, note, at",
    )
    .bind(run_id)
    .bind(&body.role)
    .bind(&body.gate)
    .bind(&body.outcome)
    .bind(&body.kind)
    .bind(body.attempt.map(|v| v.max(0)))
    .bind(body.retries.map(|v| v.max(0)))
    .bind(body.hops.map(|v| v.max(0)))
    .bind(&body.note)
    .fetch_one(pool)
    .await?;
    sqlx::query("update feature_runs set updated_at = now() where id = $1")
        .bind(run_id)
        .execute(pool)
        .await?;
    events::emit(pool, run_id, actor, "run_handoff", json!({ "run": run_id, "role": body.role, "outcome": body.outcome }));
    Ok(row)
}

pub async fn update_run(pool: &PgPool, run_id: &str, phase: Option<&str>, status: Option<&str>, actor: Option<&str>) -> Result<RunRow, AppError> {
    if !run_exists(pool, run_id).await? {
        return Err(AppError::not_found());
    }
    sqlx::query(
        "update feature_runs set phase = coalesce($1, phase), status = coalesce($2, status), updated_at = now() where id = $3",
    )
    .bind(phase)
    .bind(status)
    .bind(run_id)
    .execute(pool)
    .await?;
    events::emit(pool, run_id, actor, "run_update", json!({ "run": run_id, "phase": phase, "status": status }));
    fetch_run(pool, run_id).await?.ok_or(AppError::not_found())
}

pub async fn get_run(pool: &PgPool, run_id: &str) -> Result<RunDetail, AppError> {
    let run = fetch_run(pool, run_id).await?.ok_or(AppError::not_found())?;
    let handoffs = sqlx::query_as::<_, HandoffRow>(
        "select id, role, attempt, gate, outcome, kind, retries, hops, note, at \
         from role_handoffs where feature_run_id = $1 order by at asc, id asc",
    )
    .bind(run_id)
    .fetch_all(pool)
    .await?;
    Ok(RunDetail { run, handoffs })
}

pub async fn list_runs(pool: &PgPool, status: Option<&str>, limit: i64) -> Result<Vec<RunRow>, AppError> {
    Ok(sqlx::query_as::<_, RunRow>(
        "select id, case_id, title, phase, status, started_at, updated_at from feature_runs \
         where ($1::text is null or status = $1) order by started_at desc limit $2",
    )
    .bind(status)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

// ── HTTP ──────────────────────────────────────────────────────────────────────
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/runs", post(start_h).get(list_h))
        .route("/api/runs/:id", get(get_h).patch(update_h))
        .route("/api/runs/:id/handoffs", post(handoff_h))
}

async fn start_h(State(st): State<AppState>, Json(body): Json<StartRunBody>) -> Result<(StatusCode, Json<RunDetail>), AppError> {
    let actor = body.actor_id.clone();
    Ok((StatusCode::CREATED, Json(start_run(&st.pool, body, actor.as_deref()).await?)))
}
async fn list_h(State(st): State<AppState>, Query(q): Query<ListQuery>) -> Result<Json<Vec<RunRow>>, AppError> {
    Ok(Json(list_runs(&st.pool, q.status.as_deref(), q.limit.unwrap_or(200)).await?))
}
async fn get_h(State(st): State<AppState>, Path(id): Path<String>) -> Result<Json<RunDetail>, AppError> {
    Ok(Json(get_run(&st.pool, &id).await?))
}
async fn update_h(State(st): State<AppState>, Path(id): Path<String>, Json(body): Json<UpdateRunBody>) -> Result<Json<RunRow>, AppError> {
    Ok(Json(update_run(&st.pool, &id, body.phase.as_deref(), body.status.as_deref(), body.actor_id.as_deref()).await?))
}
async fn handoff_h(State(st): State<AppState>, Path(id): Path<String>, Json(body): Json<HandoffBody>) -> Result<(StatusCode, Json<HandoffRow>), AppError> {
    let actor = body.actor_id.clone();
    Ok((StatusCode::CREATED, Json(record_handoff(&st.pool, &id, body, actor.as_deref()).await?)))
}
