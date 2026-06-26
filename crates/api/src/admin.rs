//! The admin / console / monitor read surface over the registry. These endpoints expose what the
//! data model already stores so the (future) UIs have data to render: the type registry, a generic
//! object list, the activity feed (the Monitor spine), and memberships (the access graph). Reads are
//! permissive in dev (the RBAC reach filter is the `entity-rbac` feature); writes are limited to
//! granting/revoking a membership.

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::PgPool;

use crate::error::AppError;
use crate::{events, id, AppState};

const MEMBER_ROLES: [&str; 4] = ["viewer", "member", "admin", "owner"];

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct TypeRow {
    pub type_id: String,
    pub id_prefix: String,
    pub display_name: String,
    pub scope_parents: Value,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct ObjectRow {
    pub entity_id: String,
    #[sqlx(rename = "type")]
    pub r#type: String,
    pub data: Value,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EventRow {
    pub id: i64,
    pub entity_id: Option<String>,
    pub actor_id: Option<String>,
    pub kind: String,
    pub at: DateTime<Utc>,
    pub payload: Value,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct MembershipRow {
    pub object_id: String,
    pub member_id: String,
    pub role: String,
    pub context_role: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct WorkflowRow {
    pub workflow_id: String,
    pub states: Value,
    pub transitions: Value,
    pub initial: String,
    pub close_checks: Value,
}

#[derive(Deserialize, Default)]
pub struct ObjectsQuery {
    #[serde(rename = "type")]
    pub type_: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Deserialize, Default)]
pub struct EventsQuery {
    pub entity: Option<String>,
    pub kind: Option<String>,
    pub limit: Option<i64>,
}

#[derive(Deserialize, Default)]
pub struct MembershipsQuery {
    pub object: Option<String>,
    pub member: Option<String>,
    /// Optional: revoke only this role; absent = revoke every role for the (object, member) pair.
    pub role: Option<String>,
}

#[derive(Deserialize)]
pub struct GrantBody {
    pub object_id: String,
    pub member_id: String,
    pub role: String,
}

/// Create a generic object of any declared type (a user, an org, …) — the write counterpart to
/// `list_objects`, so actors/scopes that memberships and assignees reference can be minted through
/// the API rather than only being born as cases.
#[derive(Deserialize)]
pub struct CreateObjectBody {
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(default)]
    pub data: Option<Value>,
    #[serde(default)]
    pub scope_parent_id: Option<String>,
    #[serde(default)]
    pub actor_id: Option<String>,
}

// ── services ──────────────────────────────────────────────────────────────────
pub async fn list_types(pool: &PgPool) -> Result<Vec<TypeRow>, AppError> {
    Ok(sqlx::query_as::<_, TypeRow>(
        "select type_id, id_prefix, display_name, scope_parents from type_definitions order by type_id",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn list_objects(pool: &PgPool, type_: Option<&str>, limit: i64) -> Result<Vec<ObjectRow>, AppError> {
    Ok(sqlx::query_as::<_, ObjectRow>(
        "select e.id as entity_id, e.type as type, coalesce(ed.data, '{}'::jsonb) as data, e.created_at \
         from entities e left join entity_data ed on ed.entity_id = e.id \
         where ($1::text is null or e.type = $1) order by e.created_at desc limit $2",
    )
    .bind(type_)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

async fn fetch_object(pool: &PgPool, id: &str) -> Result<Option<ObjectRow>, AppError> {
    Ok(sqlx::query_as::<_, ObjectRow>(
        "select e.id as entity_id, e.type as type, coalesce(ed.data, '{}'::jsonb) as data, e.created_at \
         from entities e left join entity_data ed on ed.entity_id = e.id where e.id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Mint a new object of `type` (which must be declared in `type_definitions`). Inserts the registry
/// rows in one tx; if `actor` is given AND is itself an entity, grants it owner. This is how a fresh
/// database gets its first users/orgs — pass `actor = None` to bootstrap the very first one.
pub async fn create_object(pool: &PgPool, body: CreateObjectBody, actor: Option<&str>) -> Result<ObjectRow, AppError> {
    let prefix: Option<(String,)> = sqlx::query_as("select id_prefix from type_definitions where type_id = $1")
        .bind(&body.type_)
        .fetch_optional(pool)
        .await?;
    let (prefix,) = prefix.ok_or(AppError::NotFound("unknown_type"))?;

    if let Some(sp) = body.scope_parent_id.as_deref() {
        let (ok,): (bool,) = sqlx::query_as("select exists(select 1 from entities where id = $1)")
            .bind(sp)
            .fetch_one(pool)
            .await?;
        if !ok {
            return Err(AppError::NotFound("scope_not_found"));
        }
    }

    let id = id::new_id(&prefix);
    let data = body.data.clone().unwrap_or_else(|| json!({}));
    let mut tx = pool.begin().await?;
    sqlx::query("insert into entities (id, type, created_by) values ($1, $2, $3)")
        .bind(&id)
        .bind(&body.type_)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
    sqlx::query("insert into entity_data (entity_id, type_id, data, scope_parent_id) values ($1, $2, $3, $4)")
        .bind(&id)
        .bind(&body.type_)
        .bind(&data)
        .bind(&body.scope_parent_id)
        .execute(&mut *tx)
        .await?;
    if let Some(a) = actor {
        let (exists,): (bool,) = sqlx::query_as("select exists(select 1 from entities where id = $1)")
            .bind(a)
            .fetch_one(&mut *tx)
            .await?;
        if exists {
            sqlx::query("insert into memberships (object_id, member_id, role) values ($1, $2, 'owner') on conflict do nothing")
                .bind(&id)
                .bind(a)
                .execute(&mut *tx)
                .await?;
        }
    }
    tx.commit().await?;

    events::emit(pool, &id, actor, "object_create", json!({ "object": &id, "type": body.type_ }));
    fetch_object(pool, &id).await?.ok_or_else(|| AppError::Internal("created object not found".into()))
}

pub async fn list_events(pool: &PgPool, entity: Option<&str>, kind: Option<&str>, limit: i64) -> Result<Vec<EventRow>, AppError> {
    Ok(sqlx::query_as::<_, EventRow>(
        "select id, entity_id, actor_id, kind, at, payload from events \
         where ($1::text is null or entity_id = $1) and ($2::text is null or kind = $2) \
         order by at desc, id desc limit $3",
    )
    .bind(entity)
    .bind(kind)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

pub async fn list_workflows(pool: &PgPool) -> Result<Vec<WorkflowRow>, AppError> {
    Ok(sqlx::query_as::<_, WorkflowRow>(
        "select workflow_id, states, transitions, initial, close_checks from workflows order by workflow_id",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn get_workflow(pool: &PgPool, id: &str) -> Result<WorkflowRow, AppError> {
    sqlx::query_as::<_, WorkflowRow>(
        "select workflow_id, states, transitions, initial, close_checks from workflows where workflow_id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or(AppError::NotFound("unknown_workflow"))
}

pub async fn list_memberships(pool: &PgPool, object: Option<&str>, member: Option<&str>) -> Result<Vec<MembershipRow>, AppError> {
    Ok(sqlx::query_as::<_, MembershipRow>(
        "select object_id, member_id, role, context_role, created_at from memberships \
         where ($1::text is null or object_id = $1) and ($2::text is null or member_id = $2) \
         order by created_at desc limit 500",
    )
    .bind(object)
    .bind(member)
    .fetch_all(pool)
    .await?)
}

pub async fn grant(pool: &PgPool, body: GrantBody) -> Result<MembershipRow, AppError> {
    if !MEMBER_ROLES.contains(&body.role.as_str()) {
        return Err(AppError::unprocessable("invalid_role", format!("'{}' is not a role", body.role)));
    }
    for (id, what) in [(&body.object_id, "object"), (&body.member_id, "member")] {
        let (ok,): (bool,) = sqlx::query_as("select exists(select 1 from entities where id = $1)")
            .bind(id)
            .fetch_one(pool)
            .await?;
        if !ok {
            return Err(AppError::bad_request("invalid_reference", format!("{what} does not exist")));
        }
    }
    // Roles are ADDITIVE: a member may hold several roles on one object, so granting a new role
    // inserts a new row. Re-granting the SAME role hits the PK and the (no-op) on-conflict update
    // simply lets RETURNING yield the existing row, making re-grant idempotent.
    let row: MembershipRow = sqlx::query_as(
        "insert into memberships (object_id, member_id, role) values ($1, $2, $3) \
         on conflict (object_id, member_id, role, context_role) do update set role = excluded.role \
         returning object_id, member_id, role, context_role, created_at",
    )
    .bind(&body.object_id)
    .bind(&body.member_id)
    .bind(&body.role)
    .fetch_one(pool)
    .await?;
    Ok(row)
}

/// Revoke memberships for an (object, member) pair: only `role` if given, otherwise every role.
pub async fn revoke(pool: &PgPool, object: &str, member: &str, role: Option<&str>) -> Result<u64, AppError> {
    let res = sqlx::query("delete from memberships where object_id = $1 and member_id = $2 and ($3::text is null or role = $3)")
        .bind(object)
        .bind(member)
        .bind(role)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

// ── HTTP ──────────────────────────────────────────────────────────────────────
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/api/types", get(types_h))
        .route("/api/objects", get(objects_h).post(create_object_h))
        .route("/api/events", get(events_h))
        .route("/api/workflows", get(workflows_h))
        .route("/api/workflows/:id", get(workflow_h))
        .route("/api/memberships", get(memberships_h).post(grant_h).delete(revoke_h))
}

async fn workflows_h(State(st): State<AppState>) -> Result<Json<Vec<WorkflowRow>>, AppError> {
    Ok(Json(list_workflows(&st.pool).await?))
}
async fn workflow_h(State(st): State<AppState>, axum::extract::Path(id): axum::extract::Path<String>) -> Result<Json<WorkflowRow>, AppError> {
    Ok(Json(get_workflow(&st.pool, &id).await?))
}

async fn types_h(State(st): State<AppState>) -> Result<Json<Vec<TypeRow>>, AppError> {
    Ok(Json(list_types(&st.pool).await?))
}
async fn objects_h(State(st): State<AppState>, Query(q): Query<ObjectsQuery>) -> Result<Json<Vec<ObjectRow>>, AppError> {
    Ok(Json(list_objects(&st.pool, q.type_.as_deref(), q.limit.unwrap_or(200)).await?))
}
async fn create_object_h(State(st): State<AppState>, Json(body): Json<CreateObjectBody>) -> Result<(StatusCode, Json<ObjectRow>), AppError> {
    let actor = body.actor_id.clone();
    Ok((StatusCode::CREATED, Json(create_object(&st.pool, body, actor.as_deref()).await?)))
}
async fn events_h(State(st): State<AppState>, Query(q): Query<EventsQuery>) -> Result<Json<Vec<EventRow>>, AppError> {
    Ok(Json(list_events(&st.pool, q.entity.as_deref(), q.kind.as_deref(), q.limit.unwrap_or(100)).await?))
}
async fn memberships_h(State(st): State<AppState>, Query(q): Query<MembershipsQuery>) -> Result<Json<Vec<MembershipRow>>, AppError> {
    Ok(Json(list_memberships(&st.pool, q.object.as_deref(), q.member.as_deref()).await?))
}
async fn grant_h(State(st): State<AppState>, Json(body): Json<GrantBody>) -> Result<(StatusCode, Json<MembershipRow>), AppError> {
    Ok((StatusCode::CREATED, Json(grant(&st.pool, body).await?)))
}
async fn revoke_h(State(st): State<AppState>, Query(q): Query<MembershipsQuery>) -> Result<Json<Value>, AppError> {
    let object = q.object.ok_or_else(|| AppError::unprocessable("object_required", "object query param is required"))?;
    let member = q.member.ok_or_else(|| AppError::unprocessable("member_required", "member query param is required"))?;
    let n = revoke(&st.pool, &object, &member, q.role.as_deref()).await?;
    Ok(Json(json!({ "revoked": n })))
}
