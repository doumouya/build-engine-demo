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
use serde_json::Value;
use sqlx::PgPool;

use crate::error::AppError;
use crate::AppState;

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
}

#[derive(Deserialize)]
pub struct GrantBody {
    pub object_id: String,
    pub member_id: String,
    pub role: String,
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

pub async fn revoke(pool: &PgPool, object: &str, member: &str, role: &str) -> Result<u64, AppError> {
    let res = sqlx::query("delete from memberships where object_id = $1 and member_id = $2 and role = $3 and context_role = ''")
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
        .route("/api/objects", get(objects_h))
        .route("/api/events", get(events_h))
        .route("/api/memberships", get(memberships_h).post(grant_h).delete(revoke_h))
}

async fn types_h(State(st): State<AppState>) -> Result<Json<Vec<TypeRow>>, AppError> {
    Ok(Json(list_types(&st.pool).await?))
}
async fn objects_h(State(st): State<AppState>, Query(q): Query<ObjectsQuery>) -> Result<Json<Vec<ObjectRow>>, AppError> {
    Ok(Json(list_objects(&st.pool, q.type_.as_deref(), q.limit.unwrap_or(200)).await?))
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
    // revoke all roles for that (object, member) pair unless a specific one is given via ?role= (reuse object slot)
    let n = sqlx::query("delete from memberships where object_id = $1 and member_id = $2")
        .bind(&object)
        .bind(&member)
        .execute(&st.pool)
        .await?
        .rows_affected();
    Ok(Json(serde_json::json!({ "revoked": n })))
}
