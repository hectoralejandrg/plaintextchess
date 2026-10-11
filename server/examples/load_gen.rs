//! Out-of-process load generator for the split realtime architecture.
//!
//! Connects N pairs of WebSocket clients to a running transport tier, plays a
//! fixed 20-ply Ruy Lopez line per game (self-paced, one move ~pace ms), and
//! reports per-move ack latency (p50/p95/p99) plus the transport egress cost per
//! move read from the tiers' `/healthz` (task 1.1 / 4.2 of
//! `reduce-realtime-egress-cost`).
//!
//! Requires a running logic tier and transport tier (see the change tasks) and
//! `curl`. Run e.g.:
//!
//!   cargo run --release --example load_gen -- \
//!     --games 500 --pace-ms 60 --port 37222 --logic-port 37111
//!
//! Raise the fd limit first for large runs (e.g. `ulimit -n 20000`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::sync::{watch, Barrier};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::MaybeTlsStream;

type Ws = tokio_tungstenite::WebSocketStream<MaybeTlsStream<TcpStream>>;

/// White then black, 20 plies of the Ruy Lopez (index parity = side to move).
const LINE: [&str; 20] = [
    "e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6", "e1g1", "f8e7", "f1e1",
    "b7b5", "a4b3", "d7d6", "c2c3", "e8g8", "h2h3", "c6b8", "d2d4", "b8d7",
];

async fn connect(port: u16) -> Option<Ws> {
    let url = format!("ws://127.0.0.1:{port}/ws");
    tokio_tungstenite::connect_async(url)
        .await
        .ok()
        .map(|(ws, _)| ws)
}

async fn send(ws: &mut Ws, v: Value) {
    let _ = ws.send(Message::Text(v.to_string().into())).await;
}

/// Read the next JSON object within `timeout`, skipping ping/pong frames.
async fn recv_json(ws: &mut Ws, timeout: Duration) -> Option<Value> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        match tokio::time::timeout(left, ws.next()).await {
            Ok(Some(Ok(Message::Text(txt)))) => {
                if let Ok(v) = serde_json::from_str(&txt) {
                    return Some(v);
                }
            }
            Ok(Some(Ok(Message::Binary(b)))) => {
                if let Ok(v) = serde_json::from_slice(&b) {
                    return Some(v);
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => return None,
        }
    }
}

/// Read until the `update` carrying `uci` arrives; `false` on timeout or a
/// terminal update (no move).
async fn wait_update(ws: &mut Ws, uci: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let left = match deadline.checked_duration_since(Instant::now()) {
            Some(l) => l,
            None => return false,
        };
        match tokio::time::timeout(left, ws.next()).await {
            Ok(Some(Ok(Message::Text(txt)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&txt) {
                    if v["type"] == "update" {
                        if v["uci"].as_str() == Some(uci) {
                            return true;
                        }
                        if v["uci"].is_null() {
                            return false;
                        }
                    }
                }
            }
            Ok(Some(Ok(_))) => continue,
            _ => return false,
        }
    }
}

/// GET a tier's `/healthz` via `curl`; null on any failure.
async fn healthz(port: u16) -> Value {
    let url = format!("http://127.0.0.1:{port}/healthz");
    let out = std::process::Command::new("curl").args(["-sf", &url]).output();
    match out {
        Ok(o) if o.status.success() => serde_json::from_slice(&o.stdout).unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

struct Game {
    ok: bool,
    plies: usize,
    latencies: Vec<Duration>,
}

/// Connect two clients and seat them in a room; `None` on any setup failure.
async fn setup(idx: usize, port: u16, setup_timeout: Duration) -> Option<(Ws, Ws)> {
    let mut a = connect(port).await?;
    let mut b = connect(port).await?;

    let p1 = format!("load-a-{idx}");
    let p2 = format!("load-b-{idx}");
    send(
        &mut a,
        json!({"v":1,"type":"create_room","player_id":p1}),
    )
    .await;
    let rr = recv_json(&mut a, setup_timeout).await?;
    let room = rr["room_code"].as_str()?.to_string();
    let a_color = rr["your_color"].as_str().unwrap_or("").to_string();

    send(
        &mut b,
        json!({"v":1,"type":"join_room","player_id":p2,"room_code":room}),
    )
    .await;
    let rb = recv_json(&mut b, setup_timeout).await?;
    if rb["type"] != "room_ready" {
        return None;
    }

    // Map the sockets by the server-assigned color.
    Some(if a_color == "white" { (a, b) } else { (b, a) })
}

async fn play_game(
    idx: usize,
    port: u16,
    pace: Duration,
    setup_timeout: Duration,
    ack_timeout: Duration,
    barrier: Arc<Barrier>,
    mut ready: watch::Receiver<bool>,
) -> Game {
    let seated = setup(idx, port, setup_timeout).await;

    // Rendezvous: every game arrives here (even after a setup failure, so the
    // barrier never deadlocks), then waits for the start signal.
    barrier.wait().await;
    let Some((mut white, mut black)) = seated else {
        return Game { ok: false, plies: 0, latencies: Vec::new() };
    };
    if ready.wait_for(|v| *v).await.is_err() {
        return Game { ok: false, plies: 0, latencies: Vec::new() };
    }

    let mut latencies = Vec::with_capacity(LINE.len());
    for (i, uci) in LINE.iter().enumerate() {
        let (sender, receiver) = if i % 2 == 0 {
            (&mut white, &mut black)
        } else {
            (&mut black, &mut white)
        };
        let t0 = Instant::now();
        send(sender, json!({"v":1,"type":"move","uci":uci})).await;
        if !wait_update(sender, uci, ack_timeout).await {
            return Game { ok: false, plies: i, latencies };
        }
        latencies.push(t0.elapsed());
        // Drain the opponent's copy of this update (best effort).
        let _ = wait_update(receiver, uci, Duration::from_secs(5)).await;

        let elapsed = t0.elapsed();
        if pace > elapsed {
            tokio::time::sleep(pace - elapsed).await;
        }
    }
    Game { ok: true, plies: LINE.len(), latencies }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 * p).ceil() as usize).saturating_sub(1);
    sorted[idx.min(sorted.len() - 1)]
}

fn metric(v: &Value, key: &str) -> u64 {
    v.get("transport_metrics")
        .and_then(|m| m.get(key))
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
}

fn fabric_metric(v: &Value, key: &str) -> u64 {
    v.get("fabric_metrics")
        .and_then(|m| m.get(key))
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
}

fn channel_depth(v: &Value, key: &str) -> u64 {
    v.get("channel_depth")
        .and_then(|m| m.get(key))
        .and_then(|x| x.as_u64())
        .unwrap_or(0)
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    let mut games: usize = 200;
    let mut pace_ms: u64 = 60;
    let mut port: u16 = 37222;
    let mut logic_port: u16 = 37111;
    // TIER=combined serves transport_metrics/fabric_metrics/channel_depth all
    // from one /healthz on one port (colocate-room-hot-path task 4.2); this
    // flag points both fetches at --port instead of needing --logic-port too.
    let mut combined = false;
    let setup_timeout = Duration::from_secs(30);
    let ack_timeout = Duration::from_secs(60);

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--games" => { games = args[i + 1].parse().unwrap_or(games); i += 2; }
            "--pace-ms" => { pace_ms = args[i + 1].parse().unwrap_or(pace_ms); i += 2; }
            "--port" => { port = args[i + 1].parse().unwrap_or(port); i += 2; }
            "--logic-port" => { logic_port = args[i + 1].parse().unwrap_or(logic_port); i += 2; }
            "--combined" => { combined = true; i += 1; }
            _ => { i += 1; }
        }
    }
    if combined {
        logic_port = port;
    }
    let pace = Duration::from_millis(pace_ms);

    let barrier = Arc::new(Barrier::new(games + 1));
    let (tx, rx) = watch::channel(false);

    let mut handles = Vec::with_capacity(games);
    for idx in 0..games {
        handles.push(tokio::spawn(play_game(
            idx,
            port,
            pace,
            setup_timeout,
            ack_timeout,
            barrier.clone(),
            rx.clone(),
        )));
    }

    // Wait for every game to be seated before measuring (bounded, so a stalled
    // game cannot hang the run).
    let _ = tokio::time::timeout(Duration::from_secs(60), barrier.wait()).await;
    let before_transport = healthz(port).await;
    let before_logic = healthz(logic_port).await;
    eprintln!("all {games} games seated; starting moves");

    let start = Instant::now();
    let _ = tx.send(true);

    let mut results = Vec::with_capacity(games);
    for h in handles {
        results.push(h.await.unwrap_or(Game { ok: false, plies: 0, latencies: Vec::new() }));
    }
    let elapsed = start.elapsed();

    let after_transport = healthz(port).await;
    let after_logic = healthz(logic_port).await;

    let mut latencies: Vec<Duration> = results.iter().flat_map(|g| g.latencies.clone()).collect();
    latencies.sort();
    let ok = results.iter().filter(|g| g.ok).count();
    let failed = games - ok;
    let moves: usize = results.iter().map(|g| g.plies).sum();
    let moves_s = moves as f64 / elapsed.as_secs_f64().max(1e-9);

    let writes = metric(&after_transport, "ws_socket_writes") - metric(&before_transport, "ws_socket_writes");
    let frames = metric(&after_transport, "ws_frames_written") - metric(&before_transport, "ws_frames_written");
    let bytes = metric(&after_transport, "ws_bytes_written") - metric(&before_transport, "ws_bytes_written");
    let ser_ns = metric(&after_transport, "ws_serialize_ns") - metric(&before_transport, "ws_serialize_ns");
    let write_ns = metric(&after_transport, "ws_write_ns") - metric(&before_transport, "ws_write_ns");
    let per = |x: u64| x as f64 / moves.max(1) as f64;

    println!("games={games} ok={ok} failed={failed} plies={moves} elapsed_s={:.1} moves_s={moves_s:.0}", elapsed.as_secs_f64());
    println!(
        "ack_ms p50={:.2} p95={:.2} p99={:.2}",
        percentile(&latencies, 0.50).as_secs_f64() * 1e3,
        percentile(&latencies, 0.95).as_secs_f64() * 1e3,
        percentile(&latencies, 0.99).as_secs_f64() * 1e3,
    );
    println!(
        "egress/move frames={:.3} flushes={:.3} bytes={:.1} serialize_us={:.2} write_us={:.2}",
        per(frames),
        per(writes),
        per(bytes),
        per(ser_ns) / 1e3,
        per(write_ns) / 1e3,
    );

    let inbound_pubs = fabric_metric(&after_logic, "inbound_publishes") - fabric_metric(&before_logic, "inbound_publishes");
    let outbound_pubs = fabric_metric(&after_logic, "outbound_publishes") - fabric_metric(&before_logic, "outbound_publishes");
    let outbound_batches = fabric_metric(&after_logic, "outbound_batches_published") - fabric_metric(&before_logic, "outbound_batches_published");
    let outbound_drops = fabric_metric(&after_logic, "outbound_drops");
    let wait_ns = fabric_metric(&after_logic, "outbound_publish_wait_ns") - fabric_metric(&before_logic, "outbound_publish_wait_ns");
    let process_ns = fabric_metric(&after_logic, "outbound_publish_process_ns") - fabric_metric(&before_logic, "outbound_publish_process_ns");
    println!(
        "fabric/move inbound_publishes={:.3} outbound_publishes={:.3} outbound_batches={:.3} outbound_drops_total={outbound_drops}",
        per(inbound_pubs),
        per(outbound_pubs),
        per(outbound_batches),
    );
    println!(
        "fabric_publish_shard_time wait_us_total={:.1} process_us_total={:.1} wait_vs_process_ratio={:.2}",
        wait_ns as f64 / 1e3,
        process_ns as f64 / 1e3,
        wait_ns as f64 / process_ns.max(1) as f64,
    );
    println!(
        "channel_depth room_mailbox_max={} out_channel_max={}",
        channel_depth(&after_logic, "room_mailbox_max_depth"),
        channel_depth(&after_logic, "out_channel_max_depth"),
    );
    println!("logic_healthz_before={before_logic} logic_healthz_after={after_logic}");
}
