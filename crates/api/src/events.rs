//! The append-only audit log. Events are emitted **fire-and-forget, post-commit**: observability
//! must never slow or fail the thing it observes, so a failed insert is warn-logged and swallowed,
//! never surfaced to the caller and never inside the mutation's transaction. The `{ "case": <id> }`
//! key in the payload is the load-bearing handle the activity feed reads. This single ordered table
//! is the spine a Monitor view will later read to show the build-engine's own activity.

use serde_json::Value;
use sqlx::PgPool;

pub fn emit(pool: &PgPool, entity_id: &str, actor: Option<&str>, kind: &str, payload: Value) {
    let pool = pool.clone();
    let entity_id = entity_id.to_string();
    let actor = actor.map(|s| s.to_string());
    let kind = kind.to_string();
    tokio::spawn(async move {
        let res = sqlx::query("insert into events (entity_id, actor_id, kind, payload) values ($1, $2, $3, $4)")
            .bind(&entity_id)
            .bind(&actor)
            .bind(&kind)
            .bind(&payload)
            .execute(&pool)
            .await;
        if let Err(e) = res {
            tracing::warn!("event emit failed (kind={kind}, entity={entity_id}): {e:?}");
        }
    });
}
