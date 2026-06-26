//! Small transaction-scoped write helpers shared across services. They take `&mut Transaction` so
//! the caller controls the unit of work (entity + domain row + ownership commit all-or-nothing).

use sqlx::{Postgres, Transaction};

/// Insert the registry row for a new object. `created_by` is a plain actor id (nullable; not a FK).
pub async fn register_entity(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    type_id: &str,
    created_by: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("insert into entities (id, type, created_by) values ($1, $2, $3)")
        .bind(id)
        .bind(type_id)
        .bind(created_by)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Grant `member_id` the owner role on `object_id` ("no object without an owner"). Idempotent.
pub async fn grant_owner(
    tx: &mut Transaction<'_, Postgres>,
    object_id: &str,
    member_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into memberships (object_id, member_id, role) values ($1, $2, 'owner') \
         on conflict do nothing",
    )
    .bind(object_id)
    .bind(member_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
