//! Server-side reach resolution: which objects can an actor see?
//!
//! An actor reaches an object if it has a membership on that object OR on any ancestor of it, where
//! "ancestor" follows `entity_data.scope_parent_id` upward. Equivalently: reach DESCENDS from each
//! object the actor is a member of to everything scoped beneath it. This is computed with a
//! recursive CTE — `WITH RECURSIVE` whose `UNION` (not `UNION ALL`) dedups, so it terminates even if
//! the scope graph contains a cycle, and the server never has to load the whole graph.
//!
//! `engine::reachable` is the pure, client-portable mirror of this; an integration test asserts the
//! two produce the identical set. Reads use this to stay leak-free: an unreachable object is denied
//! with the same `404 not_found` as an absent one, so reach never leaks existence.

use crate::error::AppError;
use sqlx::PgPool;

const REACH_CTE: &str = "with recursive reach(id) as ( \
        select object_id from memberships where member_id = $1 \
      union \
        select ed.entity_id from entity_data ed join reach r on ed.scope_parent_id = r.id \
     )";

/// Every object id reachable by `actor`.
pub async fn reachable_ids(pool: &PgPool, actor: &str) -> Result<Vec<String>, AppError> {
    let sql = format!("{REACH_CTE} select id from reach");
    let rows: Vec<(String,)> = sqlx::query_as(&sql).bind(actor).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Whether `actor` can reach `object_id`. Cheaper than materializing the whole set.
pub async fn can_reach(pool: &PgPool, actor: &str, object_id: &str) -> Result<bool, AppError> {
    let sql = format!("{REACH_CTE} select exists(select 1 from reach where id = $2)");
    let (reachable,): (bool,) = sqlx::query_as(&sql)
        .bind(actor)
        .bind(object_id)
        .fetch_one(pool)
        .await?;
    Ok(reachable)
}
