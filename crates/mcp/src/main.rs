//! MCP stdio server entry point. stdout is the protocol channel (newline-delimited JSON-RPC), so
//! all logging goes to stderr. Reads `DATABASE_URL`, applies migrations, then serves the tools.

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // stdout is reserved for protocol traffic — never write logs there.
    tracing_subscriber::fmt().with_writer(std::io::stderr).init();

    let url = std::env::var("DATABASE_URL").map_err(|_| "DATABASE_URL is required")?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await?;
    api::run_migrations(&pool).await?;
    let state = api::AppState { pool };

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();
    tracing::info!("cases mcp server ready on stdio");

    while let Some(line) = lines.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!("ignoring unparseable line: {e}");
                continue;
            }
        };
        if let Some(resp) = mcp::dispatch(&state, &msg).await {
            let mut bytes = serde_json::to_vec(&resp)?;
            bytes.push(b'\n');
            stdout.write_all(&bytes).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}
