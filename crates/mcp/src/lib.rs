//! A minimal MCP (Model Context Protocol) server over the cases service.
//!
//! MCP's stdio transport is newline-delimited JSON-RPC 2.0. Rather than pull a framework, this
//! implements the small slice we need by hand — `initialize`, `tools/list`, `tools/call`, `ping`,
//! and notifications — which keeps the dependency surface tiny and makes the protocol legible. The
//! seven tools are thin: each parses its arguments and delegates to `api::cases` (the SAME service
//! the HTTP edge calls), so the orchestrator gets byte-identical validation. A rejected operation
//! becomes a tool error whose text is `AppError::to_wire()` — the exact contract HTTP returns — so
//! the agent can see *why* (e.g. `invalid_transition`, or `close_preconditions_unmet` + `missing`).

use api::{admin, cases, runs, AppError, AppState};
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
            let actor = args.get("actor_id").and_then(Value::as_str);
            ok(cases::get_case(pool, &id, actor).await)
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
        // ── orchestrator runs (the /feature pipeline records itself here) ──
        "start_run" => {
            let body: runs::StartRunBody = parse(args)?;
            let actor = body.actor_id.clone();
            ok(runs::start_run(pool, body, actor.as_deref()).await)
        }
        "record_handoff" => {
            let id = arg_str(&args, "id")?;
            let body: runs::HandoffBody = parse(args)?;
            let actor = body.actor_id.clone();
            ok(runs::record_handoff(pool, &id, body, actor.as_deref()).await)
        }
        "finish_run" => {
            let id = arg_str(&args, "id")?;
            let status = args.get("status").and_then(Value::as_str).unwrap_or("done");
            let actor = args.get("actor_id").and_then(Value::as_str);
            ok(runs::update_run(pool, &id, None, Some(status), actor).await)
        }
        "get_run" => {
            let id = arg_str(&args, "id")?;
            ok(runs::get_run(pool, &id).await)
        }
        "list_runs" => {
            let status = args.get("status").and_then(Value::as_str);
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(200);
            ok(runs::list_runs(pool, status, limit).await)
        }
        "list_events" => {
            let entity = args.get("entity").and_then(Value::as_str);
            let kind = args.get("kind").and_then(Value::as_str);
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(50);
            ok(admin::list_events(pool, entity, kind, limit).await)
        }
        // ── admin / registry surface (HTTP/MCP parity) ──
        "list_types" => ok(admin::list_types(pool).await),
        "list_objects" => {
            let type_ = args.get("type").and_then(Value::as_str);
            let limit = args.get("limit").and_then(Value::as_i64).unwrap_or(200);
            ok(admin::list_objects(pool, type_, limit).await)
        }
        "create_object" => {
            let body: admin::CreateObjectBody = parse(args)?;
            let actor = body.actor_id.clone();
            ok(admin::create_object(pool, body, actor.as_deref()).await)
        }
        "list_workflows" => ok(admin::list_workflows(pool).await),
        "get_workflow" => {
            let id = arg_str(&args, "id")?;
            ok(admin::get_workflow(pool, &id).await)
        }
        "list_memberships" => {
            let object = args.get("object").and_then(Value::as_str);
            let member = args.get("member").and_then(Value::as_str);
            ok(admin::list_memberships(pool, object, member).await)
        }
        "grant" => {
            let body: admin::GrantBody = parse(args)?;
            ok(admin::grant(pool, body).await)
        }
        "revoke" => {
            let object = arg_str(&args, "object")?;
            let member = arg_str(&args, "member")?;
            let role = args.get("role").and_then(Value::as_str);
            ok(admin::revoke(pool, &object, &member, role).await.map(|n| json!({ "revoked": n })))
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

/// The tool definitions, mirroring the HTTP bodies 1:1 (hand-written rather than generated to keep
/// dependencies minimal): cases, orchestrator runs, the activity feed, and the admin/registry surface.
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
        { "name": "get_case", "description": "Read a case with its comments, close-check states, and recent activity. With actor_id, an unreachable case is denied as not_found (leak-free).",
          "inputSchema": obj(json!({"id": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["id"])) },
        { "name": "list_cases", "description": "List cases, optionally filtered by status and/or scope parent. With actor_id, results are restricted to what that actor can reach.",
          "inputSchema": obj(json!({
              "status": {"type":"string"}, "scope_parent": {"type":"string"},
              "page": {"type":"integer"}, "size": {"type":"integer"}, "actor_id": {"type":"string"}
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
          "inputSchema": obj(json!({"id": {"type":"string"}, "assignee_id": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["id"])) },
        { "name": "start_run", "description": "Open an orchestrator feature run (records the /feature pipeline's own state).",
          "inputSchema": obj(json!({"title": {"type":"string"}, "case_id": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["title"])) },
        { "name": "record_handoff", "description": "Record a role handoff on a run (architect/tester/coder/reviewer/ops) with its gate, outcome, and circuit-breaker counters.",
          "inputSchema": obj(json!({
              "id": {"type":"string"}, "role": {"type":"string"}, "gate": {"type":"string"}, "outcome": {"type":"string"},
              "kind": {"type":"string"}, "attempt": {"type":"integer"}, "retries": {"type":"integer"}, "hops": {"type":"integer"},
              "note": {"type":"string"}, "actor_id": {"type":"string"}
          }), json!(["id","role"])) },
        { "name": "finish_run", "description": "Close a run (default status 'done').",
          "inputSchema": obj(json!({"id": {"type":"string"}, "status": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["id"])) },
        { "name": "get_run", "description": "Read a run with its ordered role handoffs.",
          "inputSchema": obj(json!({"id": {"type":"string"}}), json!(["id"])) },
        { "name": "list_runs", "description": "List feature runs, optionally filtered by status.",
          "inputSchema": obj(json!({"status": {"type":"string"}}), json!([])) },
        { "name": "list_events", "description": "Read the append-only activity feed (the Monitor spine), optionally filtered by entity/kind.",
          "inputSchema": obj(json!({"entity": {"type":"string"}, "kind": {"type":"string"}, "limit": {"type":"integer"}}), json!([])) },
        { "name": "list_types", "description": "List the type registry (the declared object types and their id prefixes).",
          "inputSchema": obj(json!({}), json!([])) },
        { "name": "list_objects", "description": "List registry objects, optionally filtered by type.",
          "inputSchema": obj(json!({"type": {"type":"string"}, "limit": {"type":"integer"}}), json!([])) },
        { "name": "create_object", "description": "Mint a generic object of a declared type (e.g. a user or org). actor_id, if it is an existing entity, is granted owner.",
          "inputSchema": obj(json!({"type": {"type":"string"}, "data": {"type":"object"}, "scope_parent_id": {"type":"string"}, "actor_id": {"type":"string"}}), json!(["type"])) },
        { "name": "list_workflows", "description": "List the workflow-as-data definitions (states/transitions/initial/close_checks).",
          "inputSchema": obj(json!({}), json!([])) },
        { "name": "get_workflow", "description": "Read one workflow definition by id.",
          "inputSchema": obj(json!({"id": {"type":"string"}}), json!(["id"])) },
        { "name": "list_memberships", "description": "List the access graph, optionally filtered by object and/or member.",
          "inputSchema": obj(json!({"object": {"type":"string"}, "member": {"type":"string"}}), json!([])) },
        { "name": "grant", "description": "Grant a member a role on an object (roles are additive: viewer/member/admin/owner).",
          "inputSchema": obj(json!({"object_id": {"type":"string"}, "member_id": {"type":"string"}, "role": {"type":"string"}}), json!(["object_id","member_id","role"])) },
        { "name": "revoke", "description": "Revoke a member's role on an object (a specific role if given, else all of them).",
          "inputSchema": obj(json!({"object": {"type":"string"}, "member": {"type":"string"}, "role": {"type":"string"}}), json!(["object","member"])) }
    ])
}
