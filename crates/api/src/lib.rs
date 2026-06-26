//! The HTTP edge over the pure workflow [`engine`]. This crate owns the Postgres connection, the
//! embedded migration runner, and the cases service + routes. It is a library (so an MCP surface can
//! call the same `cases::svc` functions) with a thin binary (`main.rs`) that serves it.

pub mod admin;
pub mod cases;
pub mod db;
pub mod error;
pub mod events;
pub mod id;
pub mod reach;
pub mod runs;

pub use error::AppError;

/// Shared application state. `PgPool` is cheaply cloneable (an `Arc` internally), so handlers clone it.
#[derive(Clone)]
pub struct AppState {
    pub pool: sqlx::PgPool,
}

/// Apply the repo's migrations (`migrations/0001..`), embedded at compile time — so this needs no
/// `DATABASE_URL` to build, only to run. Idempotent and checksum-tracked by sqlx.
pub async fn run_migrations(pool: &sqlx::PgPool) -> Result<(), sqlx::migrate::MigrateError> {
    sqlx::migrate!("../../migrations").run(pool).await
}

/// Build the router with state applied: cases + orchestrator runs + the admin/registry surface.
pub fn app(state: AppState) -> axum::Router {
    cases::routes()
        .merge(runs::routes())
        .merge(admin::routes())
        .with_state(state)
}
