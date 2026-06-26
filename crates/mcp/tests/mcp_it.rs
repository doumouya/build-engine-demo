//! Integration test for the MCP surface — drives `dispatch` directly with JSON-RPC messages (no
//! subprocess needed) against a real Postgres. `#[ignore]`d; run with:
//!   DATABASE_URL=postgres://.../build_engine_test cargo test -p mcp -- --ignored

use api::AppState;
use serde_json::json;

async fn state() -> AppState {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for integration tests");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect(&url)
        .await
        .expect("connect");
    api::run_migrations(&pool).await.expect("migrate");
    // a real actor entity for ownership
    sqlx::query("insert into type_definitions (type_id, id_prefix, display_name) values ('user','USR','User') on conflict do nothing")
        .execute(&pool).await.unwrap();
    sqlx::query("insert into entities (id, type) values ('USR_TEST','user') on conflict do nothing")
        .execute(&pool).await.unwrap();
    AppState { pool }
}

#[tokio::test]
#[ignore]
async fn handshake_list_and_call_roundtrip() {
    let st = state().await;

    // initialize → advertises our protocol version + the tools capability
    let init = mcp::dispatch(&st, &json!({"jsonrpc":"2.0","id":1,"method":"initialize"}))
        .await
        .expect("initialize has a response");
    assert_eq!(init["result"]["protocolVersion"], mcp::PROTOCOL_VERSION);
    assert!(init["result"]["capabilities"]["tools"].is_object());

    // a notification gets NO response
    assert!(
        mcp::dispatch(&st, &json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
            .is_none()
    );

    // tools/list → case tools (7) + run/events tools (6) + admin/registry tools (8)
    let list = mcp::dispatch(&st, &json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .await
        .unwrap();
    assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 21);

    // tools/call create_case → success content carrying the created case
    let created = mcp::dispatch(
        &st,
        &json!({"jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"create_case","arguments":{"title":"via mcp","actor_id":"USR_TEST"}}}),
    )
    .await
    .unwrap();
    let text = created["result"]["content"][0]["text"].as_str().unwrap();
    let body: serde_json::Value = serde_json::from_str(text).unwrap();
    let id = body["case"]["entity_id"].as_str().unwrap().to_string();
    assert_eq!(body["case"]["status"], "backlog");

    // tools/call set_status with an illegal skip → isError carrying the engine's reason verbatim
    let bad = mcp::dispatch(
        &st,
        &json!({"jsonrpc":"2.0","id":4,"method":"tools/call",
                "params":{"name":"set_status","arguments":{"id":id,"status":"done","actor_id":"USR_TEST"}}}),
    )
    .await
    .unwrap();
    assert_eq!(bad["result"]["isError"], true);
    let etext = bad["result"]["content"][0]["text"].as_str().unwrap();
    assert!(etext.contains("invalid_transition"), "agent sees the reason: {etext}");

    // an unknown method → a JSON-RPC error (not a tool error)
    let err = mcp::dispatch(&st, &json!({"jsonrpc":"2.0","id":5,"method":"does/not/exist"}))
        .await
        .unwrap();
    assert_eq!(err["error"]["code"], -32601);
}
