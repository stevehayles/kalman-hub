//! Minimal stateless MCP server over Streamable HTTP: each POST carries JSON-RPC
//! and gets a JSON reply. No server-initiated messages, so GET returns 405.

use crate::auth::{Caller, Role};
use crate::{service, AppState};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde_json::{json, Value};

const SUPPORTED_VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

pub async fn post(State(state): State<AppState>, Extension(Caller(role)): Extension<Caller>, Json(body): Json<Value>) -> Response {
    let replies: Vec<Value> = match body {
        Value::Array(msgs) => msgs.into_iter().filter_map(|m| handle(&state, role, m)).collect(),
        msg => handle(&state, role, msg).into_iter().collect(),
    };
    if replies.is_empty() {
        return StatusCode::ACCEPTED.into_response();
    }
    let out = if replies.len() == 1 { replies.into_iter().next().unwrap() } else { Value::Array(replies) };
    ([(header::CONTENT_TYPE, "application/json")], Json(out)).into_response()
}

pub async fn not_allowed() -> Response {
    (StatusCode::METHOD_NOT_ALLOWED, [(header::ALLOW, "POST")], "this server only answers POST").into_response()
}

fn handle(state: &AppState, role: Option<Role>, msg: Value) -> Option<Value> {
    let id = msg.get("id").cloned();
    let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
    let params = msg.get("params").cloned().unwrap_or(Value::Null);

    // Notifications and client responses carry no id and get no reply.
    let id = id?;

    let result = match (method, role) {
        ("initialize", _) => Ok(initialize(&params)),
        ("ping", _) => Ok(json!({})),
        // Without a token, show the tool list so the connector can be set up; calls still need the token.
        ("tools/list", r) => Ok(json!({ "tools": tool_list(r.unwrap_or(Role::Claude)) })),
        ("tools/call", Some(r)) => Ok(call_tool(state, r, &params)),
        ("tools/call", None) => Err((-32001, "this call needs the Claude token in the Authorization header".to_string())),
        ("resources/list", _) => Ok(json!({ "resources": [] })),
        ("prompts/list", _) => Ok(json!({ "prompts": [] })),
        _ => Err((-32601, format!("method not found: {method}"))),
    };

    Some(match result {
        Ok(r) => json!({ "jsonrpc": "2.0", "id": id, "result": r }),
        Err((code, message)) => json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } }),
    })
}

fn initialize(params: &Value) -> Value {
    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or("");
    let version = if SUPPORTED_VERSIONS.contains(&asked) { asked } else { SUPPORTED_VERSIONS[0] };
    json!({
        "protocolVersion": version,
        "capabilities": { "tools": { "listChanged": false } },
        "serverInfo": { "name": "kalman-hub", "title": "Kalman tracker hub", "version": env!("CARGO_PKG_VERSION") },
        "instructions": "Shared store for the Kalman Marine launch tracker. Claude saves a daily snapshot of the tracker and applies status updates that Pepper queues. Pepper reads get_briefing for morning briefings. The Claude tracker on claude.ai is the master copy."
    })
}

fn tool(name: &str, title: &str, description: &str, schema: Value, read_only: bool) -> Value {
    json!({
        "name": name,
        "title": title,
        "description": description,
        "inputSchema": schema,
        "annotations": { "readOnlyHint": read_only, "destructiveHint": false, "openWorldHint": false }
    })
}

fn tool_list(role: Role) -> Vec<Value> {
    let job_schema = json!({
        "type": "object",
        "required": ["id", "task", "workstream", "start", "end", "status"],
        "properties": {
            "id": { "type": "integer", "description": "The ID column of the tracker" },
            "task": { "type": "string" },
            "workstream": { "type": "string" },
            "start": { "type": "string", "format": "date", "description": "YYYY-MM-DD" },
            "end": { "type": "string", "format": "date", "description": "YYYY-MM-DD" },
            "status": { "type": "string", "enum": crate::model::STATUSES },
            "depends_on": { "type": "string", "description": "IDs this job depends on, e.g. \"24, 31\"" },
            "owner": { "type": "string" },
            "plan_section": { "type": "string" }
        }
    });

    let mut tools = vec![
        tool(
            "get_briefing",
            "Get today's briefing",
            "What needs attention today in the Kalman launch tracker: overdue, blocked, should start now, starting soon, finishing this week, and the next gate. Flags are worked out from today's date in the hub's time zone, so they stay current between snapshots.",
            json!({ "type": "object", "properties": {
                "window_days": { "type": "integer", "minimum": 1, "maximum": 120, "description": "How far ahead counts as starting soon. Default 21." }
            } }),
            true,
        ),
        tool(
            "list_jobs",
            "List tracker jobs",
            "Jobs from the latest tracker snapshot with their current flag. All filters are optional and case-insensitive.",
            json!({ "type": "object", "properties": {
                "workstream": { "type": "string" },
                "owner": { "type": "string" },
                "status": { "type": "string", "enum": crate::model::STATUSES },
                "flag": { "type": "string", "enum": ["Overdue", "Blocked", "Start now", "Starts soon", "Underway", "Done", ""] }
            } }),
            true,
        ),
        tool(
            "queue_status_update",
            "Queue a status update",
            "Ask for a job's status to change, or add a note to it. The change is applied to the Claude tracker on the next sync, not straight away.",
            json!({ "type": "object", "required": ["job_id"], "properties": {
                "job_id": { "type": "integer" },
                "status": { "type": "string", "enum": crate::model::STATUSES },
                "note": { "type": "string", "maxLength": 2000 }
            } }),
            false,
        ),
        tool(
            "list_pending_updates",
            "List queued updates",
            "Status updates queued and not yet applied to the Claude tracker.",
            json!({ "type": "object", "properties": {} }),
            true,
        ),
    ];

    if role == Role::Claude {
        tools.push(tool(
            "save_tracker_snapshot",
            "Save tracker snapshot",
            "Replace the hub's copy of the tracker with every job from the Claude tracker. Send all jobs, not just changed ones. Optional notes are passed to Pepper as part of the briefing.",
            json!({ "type": "object", "required": ["jobs"], "properties": {
                "jobs": { "type": "array", "items": job_schema, "minItems": 1 },
                "notes": { "type": "string", "description": "Short notes for the morning briefing, in plain sentences" }
            } }),
            false,
        ));
        tools.push(tool(
            "mark_updates_applied",
            "Mark updates as applied",
            "After writing queued updates into the Claude tracker, mark them applied so they are not applied twice. Use result to say what happened, especially if an update was skipped.",
            json!({ "type": "object", "required": ["ids"], "properties": {
                "ids": { "type": "array", "items": { "type": "integer" } },
                "result": { "type": "string" }
            } }),
            false,
        ));
    }
    tools
}

fn call_tool(state: &AppState, role: Role, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params.get("arguments").cloned().unwrap_or(Value::Null);
    let outcome = match name {
        "get_briefing" => service::get_briefing(state, args),
        "list_jobs" => service::list_jobs(state, args),
        "queue_status_update" => service::queue_update(state, role, args),
        "list_pending_updates" => service::list_pending(state),
        "save_tracker_snapshot" => service::save_snapshot(state, role, args),
        "mark_updates_applied" => service::mark_applied(state, role, args),
        _ => Err(format!("unknown tool: {name}")),
    };
    tracing::info!(tool = name, role = role.name(), ok = outcome.is_ok(), "tool call");
    match outcome {
        Ok(v) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }],
            "structuredContent": v,
            "isError": false
        }),
        Err(e) => json!({ "content": [{ "type": "text", "text": e }], "isError": true }),
    }
}
