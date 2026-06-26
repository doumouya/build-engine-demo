//! A minimal MCP (Model Context Protocol) server over the cases service.
//!
//! MCP's stdio transport is newline-delimited JSON-RPC 2.0. Rather than pull a framework, this
//! implements the small slice we need by hand — `initialize`, `tools/list`, `tools/call`, `ping`,
//! and notifications — which keeps the dependency surface tiny and makes the protocol legible. The
//! seven tools are thin: each parses its arguments and delegates to `api::cases` (the SAME service
//! the HTTP edge calls), so the orchestrator gets byte-identical validation. A rejected operation
//! becomes a tool error whose text is `AppError::to_wire()` — the exact contract HTTP returns — so
//! the agent can see *why* (e.g. `invalid_transition`, or `close_preconditions_unmet` + `missing`).

use api::{cases, AppError, AppState};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// MCP protocol revision we speak.
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// Handle one JSON-RPC message. Returns `Some(response)` for a request (has an `id`) and `None` for
/// a notification (no `id`, e.g. `notifications/initialized`), which by spec gets no reply.
pub async fn dispatch(state: &AppState, msg: &Value) -> Option<Value> {
    let id = msg.get("id").cloned()?; // notifications carry no id → no response
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or_else(|| json!({}));

    let outcome: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "cases-mcp", "version": env!("CARGO_PKG_VERSION") }
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_defs() })),
        "tools/call" => Ok(call_tool(state, &params).await),
        other => Err((-32601, format!("method not found: {other}"))),
    };

    Some(match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, message)) => {
            json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        }
    })
}

/// Run a `tools/call` and wrap the outcome in MCP's `content` shape. Tool-level failures are NOT
/// JSON-RPC errors — they are a successful call with `isError: true`, so the agent reads the reason.
async fn call_tool(state: &AppState, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    match run_tool(state, name, args).await {
        Ok(v) => json!({ "content": [{ "type": "text", "text": v.to_string() }] }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e.to_string() }], "isError": true }),
    }
}

/// Dispatch to a service function. `Ok` carries the serialized result; `Err` carries the wire-error
/// contract (`{ error, message, missing? }`) so it reads identically to the HTTP body.
async fn run_tool(state: &AppState, name: &str, args: Value) -> Result<Value, Value> {
    let pool = &state.pool;
    match name {
        "create_case" => {
            let body: cases::CreateCaseBody = parse(args)?;
            let actor = body.actor_id.clone();
            ok(cases::create_case(pool, body, actor.as_deref()).await)
        }
        "get_case" => {
            let id = arg_str(&args, "id")?;
            ok(cases::get_case(pool, &id).await)
        }
        "list_cases" => {
            let q: cases::ListQuery = parse(args)?;
            ok(cases::list_cases(pool, q).await)
        }
        "add_comment" => {
            let id = arg_str(&args, "id")?;
            let b: cases::AddCommentBody = parse(args)?;
            ok(cases::add_comment(pool, &id, &b.body, b.actor_id.as_deref()).await)
        }
        "set_status" => {
            let id = arg_str(&args, "id")?;
            let b: cases::SetStatusBody = parse(args)?;
            ok(cases::set_status(pool, &id, &b.status, b.actor_id.as_deref()).await)
        }
        "set_close_check" => {
            let id = arg_str(&args, "id")?;
            let check_name = arg_str(&args, "check_name")?;
            let b: cases::SetCloseCheckBody = parse(args)?;
            ok(cases::set_close_check(pool, &id, &check_name, b.passed, b.note, b.actor_id.as_deref()).await)
        }
        "assign" => {
            let id = arg_str(&args, "id")?;
            let b: cases::AssignBody = parse(args)?;
            ok(cases::assign(pool, &id, b.assignee_id, b.actor_id.as_deref()).await)
        }
        other => Err(json!({ "error": "unknown_tool", "message": format!("no such tool: {other}") })),
    }
}

fn arg_str(args: &Value, key: &str) -> Result<String, Value> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| json!({ "error": "invalid_arguments", "message": format!("missing string field '{key}'") }))
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, Value> {
    serde_json::from_value(args)
        .map_err(|e| json!({ "error": "invalid_arguments", "message": e.to_string() }))
}

fn ok<T: serde::Serialize>(r: Result<T, AppError>) -> Result<Value, Value> {
    r.map(|v| serde_json::to_value(v).unwrap_or_else(|_| json!({})))
        .map_err(|e| e.to_wire())
}

/// The seven tool definitions, mirroring the HTTP bodies 1:1 (hand-written rather than generated to
/// keep dependencies minimal).
pub fn tool_defs() -> Value {
    let obj = |props: Value, required: Value| {
        json!({ "type": "object", "properties": props, "required": required })
    };
    json!([
        { "name": "create_case", "description": "Open a case (starts at the workflow's initial state).",
          "inputSchema": obj(json!({
              "title": {"type":"string"}, "workflow_id": {"type":"string"}, "priority": {"type":"string"},
              "assignee_id": {"type":"string"}, "scope_parent_id": {"type":"string"}, "actor_id": {"type":"string"}
          }), json!(["title"])) },
        { "name": "get_case", "description": "Read a case with its comments, close-check states, and recent activity.",
          "inputSchema": obj(json!({"id": {"type":"string"}}), json!(["id"])) },
        { "name": "list_cases", "description": "List cases, optionally filtered by status and/or scope parent.",
          "inputSchema": obj(json!({
              "status": {"type":"string"}, "scope_parent": {"type":"string"},
              "page": {"type":"integer"}, "size": {"type":"integer"}
          }), json!([])) },
        { "name": "add_comment", "description": "Append a comment to a case thread.",
          "inputSchema": obj(json!({"id": {"type":"string"}, "body": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["id","body"])) },
        { "name": "set_status", "description": "Transition a case. Rejected moves return isError with the reason (e.g. invalid_transition, close_preconditions_unmet + missing).",
          "inputSchema": obj(json!({"id": {"type":"string"}, "status": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["id","status"])) },
        { "name": "set_close_check", "description": "Mark a close precondition passed or failed (e.g. tests-green).",
          "inputSchema": obj(json!({
              "id": {"type":"string"}, "check_name": {"type":"string"}, "passed": {"type":"boolean"},
              "note": {"type":"string"}, "actor_id": {"type":"string"}
          }), json!(["id","check_name","passed"])) },
        { "name": "assign", "description": "Set or clear a case's assignee.",
          "inputSchema": obj(json!({"id": {"type":"string"}, "assignee_id": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["id"])) }
    ])
}
