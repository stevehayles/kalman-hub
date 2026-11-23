mod auth;
mod db;
mod mcp;
mod model;
mod rest;
mod service;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use chrono::NaiveDate;
use chrono_tz::Tz;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tower_http::{limit::RequestBodyLimitLayer, trace::TraceLayer};

pub struct Config {
    pub claude_token: String,
    pub pepper_token: String,
    pub tz: Tz,
    pub tz_name: String,
    pub stale_hours: i64,
    pub soon_days: i64,
}

impl Config {
    pub fn today(&self) -> NaiveDate {
        chrono::Utc::now().with_timezone(&self.tz).date_naive()
    }
}

#[derive(Clone)]
pub struct AppState {
    pub db: db::Db,
    pub cfg: Arc<Config>,
    /// True on the listener that Tailscale Funnel publishes. Only the Claude token is accepted there.
    pub public: bool,
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn token(key: &str) -> String {
    let t = std::env::var(key).unwrap_or_default();
    if t.len() < 32 {
        eprintln!("{key} must be set to a random string of at least 32 characters (try: openssl rand -hex 32)");
        std::process::exit(2);
    }
    t
}

// Docker stops containers with SIGTERM, so listen for that as well as Ctrl-C.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let term = async {
        if let Ok(mut s) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            s.recv().await;
        }
    };
    #[cfg(not(unix))]
    let term = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = term => {},
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "kalman_hub=info,tower_http=info".into()),
        )
        .init();

    let claude_token = token("HUB_CLAUDE_TOKEN");
    let pepper_token = token("HUB_PEPPER_TOKEN");
    if claude_token == pepper_token {
        eprintln!("HUB_CLAUDE_TOKEN and HUB_PEPPER_TOKEN must be different");
        std::process::exit(2);
    }
    let tz_name = env_or("HUB_TIMEZONE", "Europe/London");
    let tz: Tz = tz_name.parse().unwrap_or_else(|_| {
        eprintln!("HUB_TIMEZONE {tz_name:?} is not a known time zone");
        std::process::exit(2);
    });
    let cfg = Config {
        claude_token,
        pepper_token,
        tz,
        tz_name,
        stale_hours: env_or("HUB_STALE_HOURS", "36").parse().unwrap_or(36),
        soon_days: env_or("HUB_SOON_DAYS", "21").parse().unwrap_or(21),
    };

    let db_path = PathBuf::from(env_or("HUB_DB", "/data/hub.db"));
    let db = db::Db::open(&db_path).unwrap_or_else(|e| {
        eprintln!("cannot open {}: {e}", db_path.display());
        std::process::exit(1);
    });

    let cfg = Arc::new(cfg);
    let private_state = AppState { db: db.clone(), cfg: cfg.clone(), public: false };
    let public_state = AppState { db, cfg, public: true };

    // Private listener: MCP and REST, both tokens. Reach it over Docker or the tailnet only.
    let private_app = Router::new()
        .route("/mcp", post(mcp::post).get(mcp::not_allowed).delete(mcp::not_allowed))
        .route("/api/briefing", get(rest::briefing))
        .route("/api/jobs", get(rest::jobs))
        .route("/api/updates", get(rest::pending).post(rest::queue))
        .route("/api/changes", post(rest::change))
        .route("/api/new-jobs", post(rest::new_job))
        .route_layer(middleware::from_fn_with_state(private_state.clone(), auth::require_token))
        .route("/healthz", get(|| async { "ok" }))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(private_state);

    // Public listener, published by Funnel: MCP only, Claude token only. It answers on
    // both / and /mcp so the connector URL works whether or not the proxy keeps the path.
    let public_app = Router::new()
        .route("/", post(mcp::post).get(mcp::not_allowed).delete(mcp::not_allowed))
        .route("/mcp", post(mcp::post).get(mcp::not_allowed).delete(mcp::not_allowed))
        .route_layer(middleware::from_fn_with_state(public_state.clone(), auth::identify))
        .route("/robots.txt", get(|| async { "User-agent: *\nDisallow:\n" }))
        .fallback(|| async { (axum::http::StatusCode::NOT_FOUND, "not found") })
        .layer(middleware::from_fn(auth::log_public))
        .layer(RequestBodyLimitLayer::new(2 * 1024 * 1024))
        .layer(TraceLayer::new_for_http())
        .with_state(public_state);

    let private_addr: SocketAddr = env_or("HUB_BIND", "0.0.0.0:8080").parse().expect("HUB_BIND must be host:port");
    let public_addr: SocketAddr =
        env_or("HUB_PUBLIC_BIND", "0.0.0.0:8081").parse().expect("HUB_PUBLIC_BIND must be host:port");

    let private_listener = tokio::net::TcpListener::bind(private_addr).await.expect("cannot bind HUB_BIND");
    let public_listener = tokio::net::TcpListener::bind(public_addr).await.expect("cannot bind HUB_PUBLIC_BIND");
    tracing::info!(%private_addr, %public_addr, "kalman-hub listening");

    let private = axum::serve(private_listener, private_app).with_graceful_shutdown(shutdown_signal());
    let public = axum::serve(public_listener, public_app).with_graceful_shutdown(shutdown_signal());
    if let Err(e) = tokio::try_join!(async { private.await }, async { public.await }) {
        eprintln!("server error: {e}");
        std::process::exit(1);
    }
}
