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

pub async fn require_token(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .map(str::trim);

    let role = match presented {
        Some(t) if constant_time_eq(t.as_bytes(), state.cfg.claude_token.as_bytes()) => Some(Role::Claude),
        Some(t) if !state.public && constant_time_eq(t.as_bytes(), state.cfg.pepper_token.as_bytes()) => {
            Some(Role::Pepper)
        }
        _ => None,
    };

    match role {
        Some(role) => {
            req.extensions_mut().insert(role);
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
