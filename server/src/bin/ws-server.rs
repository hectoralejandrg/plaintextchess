//! Tiered server entry point: `TIER=all` (monolith, default), `TIER=transport`
//! (WebSocket termination only), `TIER=logic` (authoritative game logic only),
//! `TIER=combined` (both tiers in one process, sharing a Redis fabric with
//! other nodes but skipping it for a room whose connections are colocated
//! here — colocate-room-hot-path).
//!
//! When `TIER` is not `all`, `REDIS_URL` (single shard) or `REDIS_URLS`
//! (comma-separated, N shards - the fabric is sharded by room/connection key
//! so a single Redis doesn't become the throughput ceiling once more than one
//! node pair relies on it) must be set, and every shard must be reachable.
//! The monolith (`TIER=all`) runs without Redis and is the backward-compatible
//! default for dev and tests.
//!
//! The transport tier deliberately does **not** open the database or load
//! authentication state (design D5): it holds no authoritative state.

use std::future::{Future, IntoFuture};
use std::sync::Arc;
use std::time::Duration;

use axum::serve::ListenerExt;
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
    if config.tier != Tier::All && config.redis_urls.is_empty() {
        tracing::error!("REDIS_URL or REDIS_URLS must be set when TIER is not 'all'");
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
        Tier::Combined => {
            let app = init_app(&config).await;
            run_combined(config, app).await
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

/// Nagle's algorithm can add tens of milliseconds of latency to the small,
/// frequent messages a chess move produces; every WebSocket-serving listener
/// disables it on accept.
fn warn_on_nodelay_failure(stream: &mut tokio::net::TcpStream) {
    if let Err(err) = stream.set_nodelay(true) {
        tracing::warn!(%err, "failed to set TCP_NODELAY on incoming connection");
    }
}

async fn run_monolith(config: Config, app: Arc<App>) {
    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener")
        .tap_io(warn_on_nodelay_failure);
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
    let node_id = generate_node_id("transport");
    let broker = match Broker::connect(&config.redis_urls, node_id.clone()).await {
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
    // Detect a stalled (not just dead) inbound publish shard (QA finding).
    state.spawn_publish_health_check();

    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener")
        .tap_io(warn_on_nodelay_failure);
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
    let node_id = generate_node_id("logic");
    let broker = match Broker::connect(&config.redis_urls, node_id.clone()).await {
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

    let logic_state = Arc::new(LogicState::new(
        app,
        broker,
        node_id,
        config.logic_lanes,
        config.logic_control_lanes,
    ));

    // Per-tier health endpoint with tier, node_id and lane metrics (task 6.1).
    let health_state = Arc::clone(&logic_state);
    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener")
        .tap_io(warn_on_nodelay_failure);
    tracing::info!(
        bind = %config.bind,
        port = config.port,
        tier = "logic",
        node_id = %logic_state.get_node_id(),
        "ws-server (logic) health endpoint listening"
    );
    let router = axum::Router::new().route(
        "/healthz",
        axum::routing::get(move || {
            let state = Arc::clone(&health_state);
            async move {
                let mut body = state.metrics_snapshot();
                if let Some(obj) = body.as_object_mut() {
                    obj.insert("status".to_string(), serde_json::json!("ok"));
                    obj.insert("tier".to_string(), serde_json::json!("logic"));
                    obj.insert("node_id".to_string(), serde_json::json!(state.get_node_id()));
                    obj.insert("protocol_version".to_string(), serde_json::json!(VERSION));
                }
                axum::Json(body)
            }
        }),
    );
    let health_server = axum::serve(listener, router).into_future();
    tokio::spawn(async move {
        if let Err(err) = health_server.await {
            tracing::error!(%err, "logic health endpoint exited");
        }
    });

    // Keep one Arc to trigger graceful shutdown while the reader loop runs.
    let shutdown_state = Arc::clone(&logic_state);
    tokio::select! {
        result = logic_state.run(None) => {
            if let Err(e) = result {
                tracing::error!(error = %e, "logic tier exited with error");
                std::process::exit(1);
            }
        }
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received; releasing rooms and membership");
            shutdown_state.shutdown().await;
        }
    }
}

/// `TIER=combined`: transport and logic share one process and one Redis
/// connection, cross-linked so a room whose both seats land on this node
/// skips the fabric hop entirely (colocate-room-hot-path design D2/D3).
/// Cross-node rooms and reconnects still go through the fabric unchanged,
/// exactly as a real `TIER=transport`/`TIER=logic` pair would.
async fn run_combined(config: Config, mut app: Arc<App>) {
    let node_id = generate_node_id("combined");
    let broker = match Broker::connect(&config.redis_urls, node_id.clone()).await {
        Ok(broker) => Arc::new(broker),
        Err(err) => {
            tracing::error!(error = %err, "cannot connect to Redis; refusing to start");
            std::process::exit(1);
        }
    };

    if let Some(app_mut) = Arc::get_mut(&mut app) {
        app_mut.set_broker(Arc::clone(&broker));
    } else {
        tracing::warn!("could not set broker on App (shared reference); room ownership release may not work");
    }

    let transport_state = Arc::new(TransportState::new(Arc::clone(&broker), node_id.clone()));
    let logic_state = Arc::new(LogicState::new(
        app,
        Arc::clone(&broker),
        node_id.clone(),
        config.logic_lanes,
        config.logic_control_lanes,
    ));
    // Wire the cross-links before either side serves traffic, so the hot path
    // never observes a half-wired state.
    transport_state.set_local_logic(Arc::clone(&logic_state));
    logic_state.set_local_transport(Arc::clone(&transport_state));

    if let Err(err) = transport_state.spawn_dispatcher().await {
        tracing::error!(error = %err, "cannot subscribe to the outbound channel; refusing to start");
        std::process::exit(1);
    }
    if let Err(err) = transport_state.spawn_liveness().await {
        tracing::error!(error = %err, "cannot register node liveness lease; refusing to start");
        std::process::exit(1);
    }
    // Close connections whose logic owner is lost (GAP-P3-D); harmless here
    // since this process is its own logic owner, but keeps combined mode
    // behaviorally identical to transport for a cross-node room's seat.
    transport_state.spawn_logic_watch();
    // Detect a stalled (not just dead) inbound publish shard (QA finding).
    transport_state.spawn_publish_health_check();

    let listener = tokio::net::TcpListener::bind((config.bind, config.port))
        .await
        .expect("bind listener")
        .tap_io(warn_on_nodelay_failure);
    tracing::info!(
        bind = %config.bind,
        port = config.port,
        tier = "combined",
        node_id = %node_id,
        "ws-server (combined) listening"
    );

    // One merged /healthz (simpler than a second listener): the logic tier's
    // metrics plus the process-global transport egress metrics, under the
    // same keys each tier already exposes on its own `/healthz`.
    let health_logic = Arc::clone(&logic_state);
    let health_node_id = node_id.clone();
    let router = axum::Router::new()
        .route("/ws", axum::routing::get(transport_handler::ws_upgrade))
        .route(
            "/healthz",
            axum::routing::get(move || {
                let logic = Arc::clone(&health_logic);
                let node_id = health_node_id.clone();
                async move {
                    let mut body = logic.metrics_snapshot();
                    if let Some(obj) = body.as_object_mut() {
                        obj.insert("status".to_string(), serde_json::json!("ok"));
                        obj.insert("tier".to_string(), serde_json::json!("combined"));
                        obj.insert("node_id".to_string(), serde_json::json!(node_id));
                        obj.insert("protocol_version".to_string(), serde_json::json!(VERSION));
                        obj.insert(
                            "transport_metrics".to_string(),
                            chess_server::infrastructure::transport::transport_metrics(),
                        );
                    }
                    axum::Json(body)
                }
            }),
        )
        .with_state(transport_state);

    let server = axum::serve(listener, router).into_future();
    tokio::spawn(async move {
        if let Err(err) = server.await {
            tracing::error!(%err, "ws-server (combined) exited");
        }
    });

    let shutdown_state = Arc::clone(&logic_state);
    tokio::select! {
        result = logic_state.run(None) => {
            if let Err(e) = result {
                tracing::error!(error = %e, "logic tier exited with error");
                std::process::exit(1);
            }
        }
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received; releasing rooms and membership");
            shutdown_state.shutdown().await;
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
