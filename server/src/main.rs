//! Entry point: bind from the environment (`BIND`, `PORT`,
//! `RECONNECT_GRACE_SECS`, `DATABASE_URL`, `SESSION_TTL_SECS`,
//! `ARGON2_*`), initialize persistence (spec "Server Persistence"), serve
//! `GET /healthz` and the `/ws` WebSocket endpoint, and terminate cleanly on
//! SIGTERM (spec: "Server Health and Configuration").

use std::future::{Future, IntoFuture};
use std::sync::Arc;

use chess_server::infrastructure::config::Config;
use chess_server::infrastructure::server::App;
use chess_server::infrastructure::ws;

#[tokio::main]
async fn main() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            // Spec "Server Health and Configuration": a malformed or
            // below-minimum authentication setting stops startup rather than
            // silently running with weaker security.
            tracing::error!(error = %err, "configuration is invalid; refusing to start");
            std::process::exit(1);
        }
    };
    let database_url = config.database_url.clone();
    let app = match App::init(config.clone()).await {
        Ok(app) => app,
        Err(err) => {
            tracing::error!(
                error = %err,
                database = ?database_url,
                "persistence initialization failed; refusing to start without a working database"
            );
            std::process::exit(1);
        }
    };
    let app = Arc::new(app);

    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener");
    tracing::info!(
        bind = %config.bind,
        port = config.port,
        grace_secs = config.reconnect_grace.as_secs(),
        database = ?database_url,
        session_ttl_secs = config.session_ttl_secs,
        argon2_memory_kib = config.argon2.memory_kib,
        argon2_time_cost = config.argon2.time_cost,
        argon2_parallelism = config.argon2.parallelism,
        "chess-server listening"
    );

    let router = axum::Router::new()
        .route("/healthz", axum::routing::get(ws::healthz))
        .route("/ws", axum::routing::get(ws::ws_upgrade))
        .with_state(app);

    let server = axum::serve(listener, router).into_future();
    shutdown_on_signal(server).await;
    tracing::info!("chess-server stopped");
}

/// Run the server until it errors out or a termination signal arrives.
async fn shutdown_on_signal(
    server: impl Future<Output = Result<(), std::io::Error>>,
) {
    tokio::select! {
        result = server => {
            if let Err(err) = result {
                tracing::error!(%err, "server exited with error");
                std::process::exit(1);
            }
        }
        _ = ctrl_c() => tracing::info!("ctrl+c received, shutting down"),
        _ = terminate() => tracing::info!("SIGTERM received, shutting down"),
    }
}

async fn ctrl_c() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(unix)]
async fn terminate() {
    use tokio::signal::unix::{signal, SignalKind};
    if let Ok(mut handler) = signal(SignalKind::terminate()) {
        handler.recv().await;
    }
}

#[cfg(not(unix))]
async fn terminate() {
    std::future::pending().await;
}
