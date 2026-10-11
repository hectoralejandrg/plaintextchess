//! Task 1.3 of `pipeline-fabric-publishes`: reproduce the publish spike on the
//! real server path — `Broker::publish_outbound_batch` (awaited, one reply per
//! publish) vs `Broker::publish_outbound_pipelined` (several in one round trip)
//! with the real wrapped `OutboundBatch` envelope, against the same Redis.
//!
//! Run: REDIS_URL=redis://127.0.0.1:6379 cargo test --test fabric_publish_bench -- --ignored --nocapture

use std::time::{Duration, Instant};

use chess_server::infrastructure::broker::{
    generate_node_id, Broker, OutboundBatch, OutboundEnvelope, ENVELOPE_VERSION,
};
use chess_server::interface::protocol::{ServerMessage, Status, VERSION};

fn redis_url() -> String {
    std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string())
}

fn envelope(node: &str, conn: &str) -> OutboundEnvelope {
    OutboundEnvelope {
        v: ENVELOPE_VERSION,
        node_id: node.to_string(),
        connection_id: conn.to_string(),
        payload: ServerMessage::Update {
            v: VERSION,
            uci: Some("e2e4".to_string()),
            side_to_move: "b".to_string(),
            status: Status::Playing,
            white_rating: 1500.0,
            black_rating: 1500.0,
            opponent_online: true,
            white_time_ms: 60_000,
            black_time_ms: 60_000,
        },
    }
}

#[tokio::test]
#[ignore]
async fn pipelined_vs_sequential_publish() {
    let node = generate_node_id("transport");
    let urls = [redis_url()];

    // A subscriber so publishes have a real target (like the transport tier).
    let sub_broker = Broker::connect(&urls, node.clone())
        .await
        .expect("sub broker");
    let _pubsub = sub_broker.subscribe_outbound(0).await.expect("subscribe");
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pub_broker = Broker::connect(&urls, generate_node_id("logic"))
        .await
        .expect("pub broker");

    // A realistic outbound batch: ~33 per-connection updates for one node.
    let batch = OutboundBatch {
        v: ENVELOPE_VERSION,
        node_id: node.clone(),
        items: (0..33)
            .map(|i| envelope(&node, &format!("conn-{i}")))
            .collect(),
    };
    let payload = serde_json::to_string(&batch).expect("serialize");
    eprintln!("real outbound batch envelope = {} bytes", payload.len());

    const N: usize = 5_000;
    const K: usize = 64;

    let t0 = Instant::now();
    for _ in 0..N {
        let _ = pub_broker.publish_outbound_batch(&batch, 0).await;
    }
    let seq_us = t0.elapsed().as_secs_f64() * 1e6 / N as f64;

    let t1 = Instant::now();
    let mut done = 0usize;
    while done < N {
        let take = K.min(N - done);
        let payloads: Vec<String> = (0..take).map(|_| payload.clone()).collect();
        let _ = pub_broker.publish_outbound_pipelined(&node, 0, &payloads).await;
        done += take;
    }
    let pip_us = t1.elapsed().as_secs_f64() * 1e6 / N as f64;

    eprintln!(
        "FABRIC_PUBLISH_SPIKE sequential={seq_us:.1}us/publish pipelined(k={K})={pip_us:.1}us/publish ratio={:.1}x",
        seq_us / pip_us
    );
}
