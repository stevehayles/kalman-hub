use crate::AppState;
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Who is calling. Claude writes the tracker snapshot and applies queued updates;
/// Pepper reads the briefing and queues updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Claude,
    Pepper,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Claude => "claude",
            Role::Pepper => "pepper",
        }
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The caller as the MCP handler sees it. `None` is an unauthenticated caller, which
/// the public listener lets through for the handshake and tool list only.
#[derive(Debug, Clone, Copy)]
pub struct Caller(pub Option<Role>);

fn role_for(state: &AppState, req: &Request) -> Option<Role> {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim);

    match presented {
        Some(t) if constant_time_eq(t.as_bytes(), state.cfg.claude_token.as_bytes()) => Some(Role::Claude),
        Some(t) if !state.public && constant_time_eq(t.as_bytes(), state.cfg.pepper_token.as_bytes()) => {
            Some(Role::Pepper)
        }
        _ => None,
    }
}

/// Public listener: identify the caller but let unauthenticated requests reach the MCP
/// handler, which refuses everything except the handshake and the tool list.
pub async fn identify(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let role = role_for(&state, &req);
    let had_header = req.headers().contains_key(header::AUTHORIZATION);
    if role.is_none() && had_header {
        tracing::warn!(path = %req.uri().path(), "rejected token on public listener");
    }
    req.extensions_mut().insert(Caller(role));
    next.run(req).await
}

/// Logs every request on the public listener, to see what the connector sends.
pub async fn log_public(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = req.uri().path().to_string();
    let has_auth = req.headers().contains_key(header::AUTHORIZATION);
    let agent = req
        .headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
        .to_string();
    let res = next.run(req).await;
    tracing::info!(%method, %path, status = res.status().as_u16(), has_auth, agent = %agent, "public request");
    res
}

pub async fn require_token(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    match role_for(&state, &req) {
        Some(role) => {
            req.extensions_mut().insert(role);
            req.extensions_mut().insert(Caller(Some(role)));
            next.run(req).await
        }
        None => {
            tracing::warn!(path = %req.uri().path(), "rejected request with missing or wrong token");
            (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer")],
                "missing or invalid bearer token",
            )
                .into_response()
        }
    }
}
