//! Dev server entry point. Reads `DATABASE_URL` (and optional `BIND`), connects a pool, applies
//! migrations, and serves the cases API.

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let url = std::env::var("DATABASE_URL")
        .map_err(|_| "DATABASE_URL is required (e.g. postgres://user:pw@localhost:5433/build_engine)")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await?;

    api::run_migrations(&pool).await?;

    let app = api::app(api::AppState { pool });
    let addr = std::env::var("BIND").unwrap_or_else(|_| "127.0.0.1:8787".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("cases api listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}
