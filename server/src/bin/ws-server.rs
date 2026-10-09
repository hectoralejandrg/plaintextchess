//! Tiered server entry point: `TIER=all` (monolith, default), `TIER=transport`
//! (WebSocket termination only), `TIER=logic` (authoritative game logic only).
//!
//! When `TIER=transport` or `TIER=logic`, `REDIS_URL` must be set and a
//! working Redis connection is required. The monolith (`TIER=all`) runs
//! without Redis and is the backward-compatible default for dev and tests.
//!
//! The transport tier deliberately does **not** open the database or load
//! authentication state (design D5): it holds no authoritative state.

use std::future::{Future, IntoFuture};
use std::sync::Arc;
use std::time::Duration;

use chess_server::infrastructure::broker::{generate_node_id, Broker};
use chess_server::infrastructure::config::{Config, Tier};
use chess_server::infrastructure::logic_handler::LogicState;
use chess_server::infrastructure::server::App;
use chess_server::infrastructure::transport_handler::{self, TransportState};
use chess_server::infrastructure::ws;
use chess_server::interface::protocol::VERSION;

#[tokio::main]
async fn main() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();

    let config = match Config::from_env() {
        Ok(config) => config,
        Err(err) => {
            tracing::error!(error = %err, "configuration is invalid; refusing to start");
            std::process::exit(1);
        }
    };

    // Spec `realtime-transport`: a split tier refuses to start without Redis.
    if config.tier != Tier::All && config.redis_url.is_none() {
        tracing::error!("REDIS_URL must be set when TIER=transport or TIER=logic");
        std::process::exit(1);
    }

    // Design D11: a dead node's seats must enter grace in time to be reclaimed.
    assert_liveness_within_grace(&config);

    match config.tier {
        // The transport tier never touches the database or auth state.
        Tier::Transport => run_transport(config).await,
        Tier::All => {
            let app = init_app(&config).await;
            run_monolith(config, app).await
        }
        Tier::Logic => {
            let app = init_app(&config).await;
            run_logic(config, app).await
        }
    }
}

/// Design D11: the transport lease TTL plus the logic's node-death detection
/// window must fit inside the reconnect grace, or a crashed node's seats would
/// be reclaimed too late. Refuse to start with an inconsistent configuration.
fn assert_liveness_within_grace(config: &Config) {
    if config.tier == Tier::All {
        return;
    }
    let lease = Duration::from_secs(chess_server::infrastructure::broker::liveness::TTL_SECS);
    let detection = chess_server::infrastructure::logic_handler::node_detection_window();
    if lease + detection >= config.reconnect_grace {
        tracing::error!(
            lease_ttl_secs = chess_server::infrastructure::broker::liveness::TTL_SECS,
            detection_secs = detection.as_secs(),
            grace_secs = config.reconnect_grace.as_secs(),
            "transport lease TTL + node detection window must be smaller than RECONNECT_GRACE_SECS"
        );
        std::process::exit(1);
    }
}

/// Initialize persistence and auth, or exit: a tier that needs the database
/// must not start half-initialized.
async fn init_app(config: &Config) -> Arc<App> {
    match App::init(config.clone()).await {
        Ok(app) => Arc::new(app),
        Err(err) => {
            tracing::error!(
                error = %err,
                database = ?config.database_url,
                "persistence initialization failed; refusing to start without a working database"
            );
            std::process::exit(1);
        }
    }
}

async fn run_monolith(config: Config, app: Arc<App>) {
    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener");
    tracing::info!(
        bind = %config.bind,
        port = config.port,
        grace_secs = config.reconnect_grace.as_secs(),
        database = ?config.database_url,
        session_ttl_secs = config.session_ttl_secs,
        argon2_memory_kib = config.argon2.memory_kib,
        argon2_time_cost = config.argon2.time_cost,
        argon2_parallelism = config.argon2.parallelism,
        tier = "all",
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

async fn run_transport(config: Config) {
    let redis_url = config
        .redis_url
        .as_ref()
        .expect("REDIS_URL required for the transport tier");

    let node_id = generate_node_id("transport");
    let broker = match Broker::connect(redis_url, node_id.clone()).await {
        Ok(broker) => Arc::new(broker),
        Err(err) => {
            tracing::error!(error = %err, "cannot connect to Redis; refusing to start");
            std::process::exit(1);
        }
    };

    let state = Arc::new(TransportState::new(broker, node_id.clone()));
    if let Err(err) = state.spawn_dispatcher().await {
        tracing::error!(error = %err, "cannot subscribe to the outbound channel; refusing to start");
        std::process::exit(1);
    }
    if let Err(err) = state.spawn_liveness().await {
        tracing::error!(error = %err, "cannot register node liveness lease; refusing to start");
        std::process::exit(1);
    }
    // Close connections whose logic owner is lost, with a clear signal (GAP-P3-D).
    state.spawn_logic_watch();

    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener");
    tracing::info!(
        bind = %config.bind,
        port = config.port,
        tier = "transport",
        node_id = %node_id,
        "ws-server (transport) listening"
    );

    let router = axum::Router::new()
        .route("/healthz", axum::routing::get(transport_handler::healthz))
        .route("/ws", axum::routing::get(transport_handler::ws_upgrade))
        .with_state(state);

    let server = axum::serve(listener, router).into_future();
    shutdown_on_signal(server).await;
    tracing::info!("ws-server (transport) stopped");
}

async fn run_logic(config: Config, mut app: Arc<App>) {
    let redis_url = config
        .redis_url
        .as_ref()
        .expect("REDIS_URL required for the logic tier");

    let node_id = generate_node_id("logic");
    let broker = match Broker::connect(redis_url, node_id.clone()).await {
        Ok(broker) => Arc::new(broker),
        Err(err) => {
            tracing::error!(error = %err, "cannot connect to Redis; refusing to start");
            std::process::exit(1);
        }
    };

    // Set the broker on the App for Phase 3 room ownership.
    // We need a mutable reference to the Arc to call get_mut.
    if let Some(app_mut) = Arc::get_mut(&mut app) {
        app_mut.set_broker(Arc::clone(&broker));
    } else {
        tracing::warn!("could not set broker on App (shared reference); room ownership release may not work");
    }

    tracing::info!(tier = "logic", node_id = %node_id, "ws-server (logic) starting");

    // Per-tier health endpoint with tier and node_id.
    let health_node_id = node_id.clone();
    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener");
    tracing::info!(
        bind = %config.bind,
        port = config.port,
        tier = "logic",
        node_id = %node_id,
        "ws-server (logic) health endpoint listening"
    );
    let router = axum::Router::new().route(
        "/healthz",
        axum::routing::get(move || {
            let nid = health_node_id.clone();
            async move {
                axum::Json(serde_json::json!({
                    "status": "ok",
                    "tier": "logic",
                    "node_id": nid,
                    "protocol_version": VERSION,
                }))
            }
        }),
    );
    let health_server = axum::serve(listener, router).into_future();
    tokio::spawn(async move {
        if let Err(err) = health_server.await {
            tracing::error!(%err, "logic health endpoint exited");
        }
    });

    let logic_state = Arc::new(LogicState::new(app, broker, node_id));
    let run_state = Arc::clone(&logic_state);
    tokio::select! {
        result = run_state.run(None) => {
            if let Err(e) = result {
                tracing::error!(error = %e, "logic tier exited with error");
                std::process::exit(1);
            }
        }
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received; releasing rooms and membership");
            logic_state.shutdown().await;
        }
    }
}

/// Resolve on SIGINT or SIGTERM.
async fn shutdown_signal() {
    tokio::select! {
        _ = ctrl_c() => {}
        _ = terminate() => {}
    }
}

/// Run the server until it errors out or a termination signal arrives.
async fn shutdown_on_signal(server: impl Future<Output = Result<(), std::io::Error>>) {
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
