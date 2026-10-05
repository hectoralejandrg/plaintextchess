//! Environment-driven configuration (spec "Server Health and Configuration",
//! design D1/D4/D10): the same build runs locally and on a hosting platform.

use std::net::IpAddr;
use std::time::Duration;

pub const DEFAULT_BIND: &str = "0.0.0.0";
pub const DEFAULT_PORT: u16 = 8765;
pub const DEFAULT_RECONNECT_GRACE_SECS: u64 = 120;

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    pub reconnect_grace: Duration,
}

impl Config {
    pub fn from_env() -> Self {
        let bind = std::env::var("BIND")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(|| DEFAULT_BIND.parse().expect("default bind"));
        let port = std::env::var("PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_PORT);
        let grace_secs = std::env::var("RECONNECT_GRACE_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(DEFAULT_RECONNECT_GRACE_SECS);
        Self {
            bind,
            port,
            reconnect_grace: Duration::from_secs(grace_secs),
        }
    }

    /// Deterministic config for tests (no env reads).
    pub fn for_test(bind: IpAddr, port: u16, reconnect_grace: Duration) -> Self {
        Self {
            bind,
            port,
            reconnect_grace,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn defaults_apply_without_env() {
        let config = Config::for_test(
            "127.0.0.1".parse().unwrap(),
            9999,
            Duration::from_secs(30),
        );
        assert_eq!(config.port, 9999);
        assert_eq!(config.reconnect_grace, Duration::from_secs(30));
        assert_eq!(DEFAULT_PORT, 8765);
        assert_eq!(DEFAULT_RECONNECT_GRACE_SECS, 120);
    }
}
