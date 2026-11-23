//! Plain REST endpoints, for Pepper if its agent framework cannot speak MCP.
//! Keep these on the private network: only /mcp is published through Funnel.

use crate::auth::Role;
use crate::{service, AppState};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use serde::Deserialize;
use serde_json::{json, Value};

fn reply(r: service::OpResult) -> Response {
    match r {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

#[derive(Deserialize)]
pub struct BriefingQuery {
    window_days: Option<i64>,
}

pub async fn briefing(State(state): State<AppState>, Query(q): Query<BriefingQuery>) -> Response {
    reply(service::get_briefing(&state, json!({ "window_days": q.window_days })))
}

#[derive(Deserialize)]
pub struct JobsQuery {
    workstream: Option<String>,
    owner: Option<String>,
    status: Option<String>,
    flag: Option<String>,
}

pub async fn jobs(State(state): State<AppState>, Query(q): Query<JobsQuery>) -> Response {
    reply(service::list_jobs(
        &state,
        json!({ "workstream": q.workstream, "owner": q.owner, "status": q.status, "flag": q.flag }),
    ))
}

pub async fn pending(State(state): State<AppState>) -> Response {
    reply(service::list_pending(&state))
}

pub async fn queue(State(state): State<AppState>, Extension(role): Extension<Role>, Json(body): Json<Value>) -> Response {
    reply(service::queue_update(&state, role, body))
}
