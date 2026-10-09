//! Split-mode startup contract (spec `realtime-transport`, design D8): a split
//! tier refuses to start without a working Redis, and an invalid `TIER` is
//! rejected. These run without a broker.

use std::process::Command;

/// A split tier started with no `REDIS_URL` exits non-zero instead of serving.
#[test]
fn split_requires_redis() {
    let output = Command::new(env!("CARGO_BIN_EXE_ws-server"))
        .env("TIER", "transport")
        .env_remove("REDIS_URL")
        .env("BIND", "127.0.0.1")
        .env("PORT", "0")
        .output()
        .expect("run ws-server");

    assert!(
        !output.status.success(),
        "a transport tier with no REDIS_URL must exit non-zero"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("REDIS_URL"),
        "the error must name REDIS_URL: {combined}"
    );
}

/// An unknown `TIER` value stops startup rather than silently defaulting.
#[test]
fn invalid_tier_is_rejected() {
    let output = Command::new(env!("CARGO_BIN_EXE_ws-server"))
        .env("TIER", "banana")
        .env_remove("REDIS_URL")
        .output()
        .expect("run ws-server");

    assert!(!output.status.success(), "an invalid TIER must exit non-zero");
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("TIER"),
        "the error must name TIER: {combined}"
    );
}
