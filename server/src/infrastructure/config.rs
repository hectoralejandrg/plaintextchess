//! Environment-driven configuration (spec "Server Health and Configuration",
//! design D1/D4/D10): the same build runs locally and on a hosting platform.
//!
//! Every variable except the authentication ones keeps its historical
//! "silently fall back to the default" behavior. Authentication settings are
//! different on purpose (spec "Credential and Session Data Protection", design
//! D5/D7): `from_env` returns `Result` so a value below the supported minimum
//! stops startup instead of quietly running with weaker security.

use std::net::IpAddr;
use std::time::Duration;

pub const DEFAULT_BIND: &str = "0.0.0.0";
pub const DEFAULT_PORT: u16 = 8765;
pub const DEFAULT_RECONNECT_GRACE_SECS: u64 = 120;
/// Default persistence database location (spec "Server Persistence"): a
/// file below the server's working `data/` directory, created if missing.
pub const DEFAULT_DATABASE_URL: &str = "sqlite:./data/chess-server.db";

/// Server tier role (design D8): `all` = monolith (default), `transport` = WS
/// termination only, `logic` = authoritative game logic only, `combined` =
/// transport and logic in one process over Redis, so a room whose both seats
/// land on it skips the fabric hop (colocate-room-hot-path).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    All,
    Transport,
    Logic,
    Combined,
}

impl Default for Tier {
    fn default() -> Self {
        Tier::All
    }
}

impl Tier {
    pub fn from_env() -> Result<Self, String> {
        match std::env::var("TIER") {
            Err(_) => Ok(Tier::All),
            Ok(raw) => match raw.trim().to_lowercase().as_str() {
                "all" => Ok(Tier::All),
                "transport" => Ok(Tier::Transport),
                "logic" => Ok(Tier::Logic),
                "combined" => Ok(Tier::Combined),
                other => Err(format!(
                    "TIER={other:?} is invalid; must be 'all', 'transport', 'logic', or 'combined'"
                )),
            },
        }
    }
}

/// Default session lifetime (design D7): 30 days, long enough that a player
/// signs in rarely and short enough that a leaked token stops working.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 30 * 24 * 60 * 60;
/// The shortest session lifetime the server will accept (design D7).
pub const MIN_SESSION_TTL_SECS: u64 = 60;

/// The transport that delivers transactional email when nothing says
/// otherwise: the server log (spec "Pluggable Email Delivery"). It is
/// refused in production mode (design D19).
pub const DEFAULT_EMAIL_TRANSPORT: &str = "log";

/// The public base URL an emailed link is built from when nothing says
/// otherwise (design D6).
pub const DEFAULT_PUBLIC_BASE_URL: &str = "http://localhost:3000";

/// How many sign-in attempts one client address or device may make per
/// minute before it is refused (spec "Authentication Rate Limiting").
///
/// The limit is per address *and* per device, and it has to leave room for
/// a shared network: a school or a mobile carrier puts many players behind
/// one address.
pub const DEFAULT_AUTH_RATE_LIMIT_LOGIN_PER_MIN: u32 = 10;

/// How many registrations one client address or device may make per hour.
pub const DEFAULT_AUTH_RATE_LIMIT_REGISTER_PER_HOUR: u32 = 20;

/// Default number of inbound processing lanes (design D5/D1). A single lane
/// preserves the current serial behavior; unset uses 1. Increasing K allows
/// independent rooms to be processed concurrently while preserving per-room
/// ordering via `hash(room_code) % K`.
///
/// QA finding (investigated, NOT acted on by raising this default — see
/// `openspec/changes/tune-logic-lane-defaults`): a single lane means
/// every room/control message for a `TIER=logic`/`combined` process
/// serializes through one worker, and under a burst of thousands of
/// concurrent `CreateRoom`s this measurably causes setup failures (~37% at
/// 1 lane in a reproduction of the QA scenario). Raising it is NOT a free
/// win, though: more local lanes also means more concurrent requests
/// hitting the shared control-shard Redis connection at once, and a rushed
/// single-run comparison (8 lanes) that looked better than this default
/// did NOT hold up under a cleaner retest (16 lanes measured *worse*, and
/// a third attempt was too contaminated by repeated local load-testing to
/// trust at all). The right lane count is a real open question that needs
/// a clean, dedicated measurement (fresh hosts, several replicas, proper
/// drain time between runs) — not a quick call from one noisy benchmark.
/// Left at the conservative, known-safe value until that measurement
/// exists.
pub const DEFAULT_LOGIC_LANES: usize = 1;
/// Minimum allowed inbound lanes (must be >= 1 to preserve ordering guarantees).
pub const MIN_LOGIC_LANES: usize = 1;

/// Default number of control-plane lanes (design D2). Control-plane messages
/// (create/join/leave/resign/auth) are routed here, keyed by connection_id,
/// so argon2 auth and Redis round trips do not stall move lanes. Same
/// rationale and the same deferred decision as `DEFAULT_LOGIC_LANES` above
/// — this is in fact the lane the QA reproduction actually bottlenecked on
/// (`CreateRoom`/`JoinRoom` are control-plane messages).
pub const DEFAULT_LOGIC_CONTROL_LANES: usize = 1;
/// Minimum allowed control lanes (must be >= 1).
pub const MIN_LOGIC_CONTROL_LANES: usize = 1;

/// Argon2id cost parameters (spec "Credential and Session Data Protection",
/// design D5). Defaults are the OWASP baseline recommendation; each has a
/// floor below which startup is refused rather than degraded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argon2Params {
    /// Memory cost in kibibytes.
    pub memory_kib: u32,
    /// Number of passes over memory.
    pub time_cost: u32,
    /// Degree of parallelism (lanes).
    pub parallelism: u32,
}

/// OWASP baseline: 19 MiB, 2 passes, 1 lane.
pub const DEFAULT_ARGON2_MEMORY_KIB: u32 = 19_456;
pub const DEFAULT_ARGON2_TIME_COST: u32 = 2;
pub const DEFAULT_ARGON2_PARALLELISM: u32 = 1;

/// Floors enforced at startup (design D5). Below these, argon2 stops being
/// worth its cost against a GPU attacker.
pub const MIN_ARGON2_MEMORY_KIB: u32 = 8_192;
pub const MIN_ARGON2_TIME_COST: u32 = 1;
pub const MIN_ARGON2_PARALLELISM: u32 = 1;

impl Default for Argon2Params {
    fn default() -> Self {
        Self {
            memory_kib: DEFAULT_ARGON2_MEMORY_KIB,
            time_cost: DEFAULT_ARGON2_TIME_COST,
            parallelism: DEFAULT_ARGON2_PARALLELISM,
        }
    }
}

impl Argon2Params {
    /// Reject a cost the server refuses to weaken to (spec scenario "A
    /// below-minimum authentication setting stops startup").
    pub fn validate(&self) -> Result<(), String> {
        if self.memory_kib < MIN_ARGON2_MEMORY_KIB {
            return Err(format!(
                "ARGON2_MEMORY_KIB={} is below the supported minimum of {} KiB",
                self.memory_kib, MIN_ARGON2_MEMORY_KIB
            ));
        }
        if self.time_cost < MIN_ARGON2_TIME_COST {
            return Err(format!(
                "ARGON2_TIME_COST={} is below the supported minimum of {}",
                self.time_cost, MIN_ARGON2_TIME_COST
            ));
        }
        if self.parallelism < MIN_ARGON2_PARALLELISM {
            return Err(format!(
                "ARGON2_PARALLELISM={} is below the supported minimum of {}",
                self.parallelism, MIN_ARGON2_PARALLELISM
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct Config {
    pub bind: IpAddr,
    pub port: u16,
    pub reconnect_grace: Duration,
    /// The persistence database location (spec "Server Persistence").
    /// `None` disables persistence entirely (the in-memory test path);
    /// production runs always have a URL (`from_env` supplies a default).
    pub database_url: Option<String>,
    /// How long an issued session stays usable (spec "Session Lifetime,
    /// Reuse, and Revocation", design D7).
    pub session_ttl_secs: u64,
    /// Password hashing cost (spec "Credential and Session Data Protection",
    /// design D5).
    pub argon2: Argon2Params,
    /// Whether online game colors are assigned at random at room creation
    /// (spec "Server Room Management"). Production is random; the
    /// deterministic test config pins the creator to White so the room tests
    /// stay stable.
    pub random_colors: bool,
    /// Whether in-game broadcasts are incremental `update` frames (spec
    /// "Server Game Authority"). Production is incremental; the test config
    /// sends full `state` snapshots so the socket tests, which read one
    /// snapshot per broadcast, stay stable. Dedicated tests turn it on.
    pub incremental_updates: bool,
    /// Server tier role (design D8). Defaults to `all` (monolith).
    pub tier: Tier,
    /// Redis shard connection URLs (required when tier is not `all`). Every
    /// fabric node (transport/logic/combined) must be given the same list, so
    /// each can reach every shard. From `REDIS_URLS` (comma-separated), or
    /// `REDIS_URL` alone as a 1-shard fallback.
    pub redis_urls: Vec<String>,
    /// Number of inbound processing lanes for the logic tier (design D1/D5).
    /// When `TIER=logic`, this controls how many concurrent workers process
    /// room-scoped messages. Each room maps to `hash(room_code) % LOGIC_LANES`,
    /// so independent rooms run in parallel while per-room ordering is preserved.
    /// Unset uses the safe default 1 (serial behavior); zero/invalid is rejected.
    pub logic_lanes: usize,
    /// Number of control-plane lanes for the logic tier (design D2).
    /// These lanes handle create/join/leave/resign/auth keyed by connection_id,
    /// isolating argon2 auth and Redis round trips from move lanes.
    /// Unset uses the safe default 1; zero/invalid is rejected.
    pub logic_control_lanes: usize,
    /// Which transport delivers transactional email (spec "Pluggable Email
    /// Delivery"): `log`, `resend`, or `none`.
    pub email_transport: String,
    /// The provider's credential, read from the environment. The file wins,
    /// so the credential never has to sit in the container's environment.
    pub email_api_key: Option<String>,
    /// Where the provider's credential is read from (design D5).
    pub email_api_key_file: Option<String>,
    /// The address the provider sends from.
    pub email_from: String,
    /// The display name the provider sends from.
    pub email_from_name: Option<String>,
    /// The public base URL an emailed link is built from (design D6).
    pub public_base_url: String,
    /// Whether this deployment is a production one. The log transport is
    /// refused in production mode (design D19).
    pub production_mode: bool,
    /// Whether the breached-password check runs (spec "Breached Password
    /// Refusal").
    pub hibp_enabled: bool,
    /// How many sign-in attempts one client address or device may make per
    /// minute. `0` means no limit.
    pub auth_rate_limit_login_per_min: u32,
    /// How many registrations one client address or device may make per
    /// hour. `0` means no limit.
    pub auth_rate_limit_register_per_hour: u32,
    /// The proxy networks whose forwarding header may be trusted (spec
    /// "Authentication Rate Limiting").
    pub auth_trusted_proxy_cidrs: Vec<String>,
}

/// Read an optional unsigned integer for a security-relevant variable: an
/// unparseable value is an error, not a silent default.
fn strict_u64(key: &str) -> Result<Option<u64>, String> {
    match std::env::var(key) {
        Err(_) => Ok(None),
        Ok(raw) => match raw.trim().parse::<u64>() {
            Ok(value) => Ok(Some(value)),
            Err(_) => Err(format!(
                "{key}={raw:?} is not a non-negative whole number of seconds/units"
            )),
        },
    }
}

/// Read an optional counter from the environment. An unparseable value is an
/// error: a rate limit that silently becomes a different number than the
/// operator wrote is worse than a refused start.
fn parse_u32(key: &str, default: u32) -> Result<u32, String> {
    match std::env::var(key) {
        Err(_) => Ok(default),
        Ok(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(default);
            }
            trimmed
                .parse::<u32>()
                .map_err(|_| format!("{key}={raw:?} is not a whole number"))
        }
    }
}

impl Config {
    /// Read the whole environment. Returns `Err` when an authentication
    /// setting is malformed or below its supported minimum (design D5/D7):
    /// the caller stops startup rather than serving with weaker security.
    pub fn from_env() -> Result<Self, String> {
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
        let database_url =
            std::env::var("DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.to_string());

        let session_ttl_secs =
            strict_u64("SESSION_TTL_SECS")?.unwrap_or(DEFAULT_SESSION_TTL_SECS);
        if session_ttl_secs < MIN_SESSION_TTL_SECS {
            return Err(format!(
                "SESSION_TTL_SECS={session_ttl_secs} is below the supported minimum of {MIN_SESSION_TTL_SECS}"
            ));
        }

        let defaults = Argon2Params::default();
        let argon2 = Argon2Params {
            memory_kib: strict_u64("ARGON2_MEMORY_KIB")?
                .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
                .unwrap_or(defaults.memory_kib),
            time_cost: strict_u64("ARGON2_TIME_COST")?
                .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
                .unwrap_or(defaults.time_cost),
            parallelism: strict_u64("ARGON2_PARALLELISM")?
                .map(|v| u32::try_from(v).unwrap_or(u32::MAX))
                .unwrap_or(defaults.parallelism),
        };
        argon2.validate()?;

        let tier = Tier::from_env()?;
        let redis_urls: Vec<String> = match std::env::var("REDIS_URLS") {
            Ok(raw) => raw
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            Err(_) => std::env::var("REDIS_URL").ok().into_iter().collect(),
        };

        // LOGIC_LANES: number of inbound processing lanes (design D1/D5)
        let logic_lanes = match std::env::var("LOGIC_LANES") {
            Err(_) => DEFAULT_LOGIC_LANES,
            Ok(raw) => match raw.trim().parse::<usize>() {
                Ok(v) if v >= MIN_LOGIC_LANES => v,
                Ok(_) => {
                    return Err(format!(
                        "LOGIC_LANES must be >= {MIN_LOGIC_LANES}; got {raw:?}"
                    ));
                }
                Err(_) => {
                    return Err(format!(
                        "LOGIC_LANES={raw:?} is not a valid positive integer"
                    ));
                }
            },
        };

        // LOGIC_CONTROL_LANES: number of control-plane lanes (design D2)
        let logic_control_lanes = match std::env::var("LOGIC_CONTROL_LANES") {
            Err(_) => DEFAULT_LOGIC_CONTROL_LANES,
            Ok(raw) => match raw.trim().parse::<usize>() {
                Ok(v) if v >= MIN_LOGIC_CONTROL_LANES => v,
                Ok(_) => {
                    return Err(format!(
                        "LOGIC_CONTROL_LANES must be >= {MIN_LOGIC_CONTROL_LANES}; got {raw:?}"
                    ));
                }
                Err(_) => {
                    return Err(format!(
                        "LOGIC_CONTROL_LANES={raw:?} is not a valid positive integer"
                    ));
                }
            },
        };

        // The email transport and the provider's credential (spec "Pluggable
        // Email Delivery"). The credential is read from a file when one is
        // configured, so it never has to sit in the container's environment
        // (design D5).
        let email_transport = std::env::var("EMAIL_TRANSPORT")
            .ok()
            .map(|raw| raw.trim().to_lowercase())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| DEFAULT_EMAIL_TRANSPORT.to_string());
        if !matches!(email_transport.as_str(), "log" | "resend" | "none") {
            return Err(format!(
                "EMAIL_TRANSPORT={email_transport} is invalid; must be log, resend, or none"
            ));
        }
        let email_api_key = std::env::var("EMAIL_API_KEY")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty());
        let email_api_key_file = std::env::var("EMAIL_API_KEY_FILE")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty());
        let email_from = std::env::var("EMAIL_FROM")
            .unwrap_or_default()
            .trim()
            .to_string();
        let email_from_name = std::env::var("EMAIL_FROM_NAME")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty());

        // The public base URL an emailed link is built from (design D6).
        let public_base_url = std::env::var("PUBLIC_BASE_URL")
            .ok()
            .map(|raw| raw.trim().to_string())
            .filter(|raw| !raw.is_empty())
            .unwrap_or_else(|| DEFAULT_PUBLIC_BASE_URL.to_string());

        // Whether this deployment is a production one (design D19).
        let production_mode = match std::env::var("DEPLOYMENT_MODE")
            .ok()
            .map(|raw| raw.trim().to_lowercase())
            .as_deref()
        {
            None | Some("") | Some("development") => false,
            Some("production") => true,
            Some(other) => {
                return Err(format!(
                    "DEPLOYMENT_MODE={other} is invalid; must be development or production"
                ));
            }
        };

        // The breached-password check (spec "Breached Password Refusal").
        let hibp_enabled = match std::env::var("HIBP_ENABLED")
            .ok()
            .map(|raw| raw.trim().to_lowercase())
            .as_deref()
        {
            None | Some("") => true,
            Some("1") | Some("true") | Some("yes") => true,
            Some("0") | Some("false") | Some("no") => false,
            Some(other) => {
                return Err(format!("HIBP_ENABLED={other} is invalid; must be 0 or 1"));
            }
        };

        // The attempt limits (spec "Authentication Rate Limiting"). `0` means
        // "no limit".
        let auth_rate_limit_login_per_min =
            parse_u32("AUTH_RATE_LIMIT_LOGIN_PER_MIN", DEFAULT_AUTH_RATE_LIMIT_LOGIN_PER_MIN)?;
        let auth_rate_limit_register_per_hour = parse_u32(
            "AUTH_RATE_LIMIT_REGISTER_PER_HOUR",
            DEFAULT_AUTH_RATE_LIMIT_REGISTER_PER_HOUR,
        )?;

        // The proxy networks whose forwarding header may be trusted.
        let auth_trusted_proxy_cidrs = std::env::var("AUTH_TRUSTED_PROXY_CIDRS")
            .unwrap_or_default()
            .split(',')
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();

        Ok(Self {
            bind,
            port,
            reconnect_grace: Duration::from_secs(grace_secs),
            database_url: Some(database_url),
            session_ttl_secs,
            argon2,
            random_colors: true,
            incremental_updates: true,
            tier,
            redis_urls,
            logic_lanes,
            logic_control_lanes,
            email_transport,
            email_api_key,
            email_api_key_file,
            email_from,
            email_from_name,
            public_base_url,
            production_mode,
            hibp_enabled,
            auth_rate_limit_login_per_min,
            auth_rate_limit_register_per_hour,
            auth_trusted_proxy_cidrs,
        })
    }

    /// The provider's credential: the file first, the environment second
    /// (design D5). A file that cannot be read is reported and the inline
    /// value is used instead, so a deployment is never left without a
    /// credential it was configured with.
    pub fn email_api_key(&self) -> Option<String> {
        if let Some(path) = self.email_api_key_file.as_deref() {
            match std::fs::read_to_string(path) {
                Ok(raw) => {
                    let key = raw.trim().to_string();
                    if !key.is_empty() {
                        return Some(key);
                    }
                    tracing::warn!(path = %path, "the email provider credential file is empty");
                }
                Err(err) => {
                    tracing::warn!(
                        path = %path,
                        error = %err,
                        "the email provider credential could not be read"
                    );
                }
            }
        }
        self.email_api_key.clone()
    }

    /// A config pointed at a specific database, with everything else at its
    /// defaults. Used by the persistence tests, which need a real file but the
    /// default authentication cost and lifetime.
    pub fn with_database(url: impl Into<String>) -> Self {
        Self {
            database_url: Some(url.into()),
            ..Self::for_test(
                DEFAULT_BIND.parse().expect("default bind"),
                0,
                Duration::from_secs(DEFAULT_RECONNECT_GRACE_SECS),
            )
        }
    }

    /// Deterministic config for tests (no env reads): persistence disabled,
    /// authentication at its default cost and lifetime.
    pub fn for_test(bind: IpAddr, port: u16, reconnect_grace: Duration) -> Self {
        Self {
            bind,
            port,
            reconnect_grace,
            database_url: None,
            session_ttl_secs: DEFAULT_SESSION_TTL_SECS,
            argon2: Argon2Params::default(),
            random_colors: false,
            incremental_updates: false,
            tier: Tier::All,
            redis_urls: Vec::new(),
            logic_lanes: DEFAULT_LOGIC_LANES,
            logic_control_lanes: DEFAULT_LOGIC_CONTROL_LANES,
            email_transport: DEFAULT_EMAIL_TRANSPORT.to_string(),
            email_api_key: None,
            email_api_key_file: None,
            email_from: String::new(),
            email_from_name: None,
            public_base_url: DEFAULT_PUBLIC_BASE_URL.to_string(),
            production_mode: false,
            // A test does not consult a third-party corpus, and it must not
            // be refused by a limit the test did not ask for. Both are
            // configured explicitly where a test needs them.
            hibp_enabled: false,
            auth_rate_limit_login_per_min: 0,
            auth_rate_limit_register_per_hour: 0,
            auth_trusted_proxy_cidrs: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const AUTH_VARS: [&str; 18] = [
        "SESSION_TTL_SECS",
        "ARGON2_MEMORY_KIB",
        "ARGON2_TIME_COST",
        "ARGON2_PARALLELISM",
        "DATABASE_URL",
        "LOGIC_LANES",
        "LOGIC_CONTROL_LANES",
        "EMAIL_TRANSPORT",
        "EMAIL_API_KEY",
        "EMAIL_API_KEY_FILE",
        "EMAIL_FROM",
        "EMAIL_FROM_NAME",
        "PUBLIC_BASE_URL",
        "DEPLOYMENT_MODE",
        "HIBP_ENABLED",
        "AUTH_RATE_LIMIT_LOGIN_PER_MIN",
        "AUTH_RATE_LIMIT_REGISTER_PER_HOUR",
        "AUTH_TRUSTED_PROXY_CIDRS",
    ];

    /// The authentication variables are process-wide, so the tests that read
    /// them share one mutex instead of racing each other.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for var in AUTH_VARS {
            std::env::remove_var(var);
        }
        guard
    }

    #[test]
    fn defaults_apply_without_env() {
        let _guard = env_guard();
        let config = Config::from_env().expect("defaults must be valid");
        assert_eq!(config.port, DEFAULT_PORT);
        assert_eq!(config.reconnect_grace, Duration::from_secs(DEFAULT_RECONNECT_GRACE_SECS));
        assert_eq!(config.session_ttl_secs, DEFAULT_SESSION_TTL_SECS);
        assert_eq!(config.argon2, Argon2Params::default());
        assert_eq!(config.argon2.memory_kib, 19_456);
        assert_eq!(config.database_url.as_deref(), Some(DEFAULT_DATABASE_URL));
        assert_eq!(config.email_transport, DEFAULT_EMAIL_TRANSPORT);
        assert_eq!(config.public_base_url, DEFAULT_PUBLIC_BASE_URL);
        assert!(!config.production_mode);
        assert!(config.hibp_enabled);
        assert_eq!(
            config.auth_rate_limit_login_per_min,
            DEFAULT_AUTH_RATE_LIMIT_LOGIN_PER_MIN
        );
        assert_eq!(
            config.auth_rate_limit_register_per_hour,
            DEFAULT_AUTH_RATE_LIMIT_REGISTER_PER_HOUR
        );
        assert!(config.auth_trusted_proxy_cidrs.is_empty());
    }

    #[test]
    fn the_email_transport_comes_from_the_environment() {
        let _guard = env_guard();
        std::env::set_var("EMAIL_TRANSPORT", "resend");
        std::env::set_var("EMAIL_FROM", "no-reply@play.example.com");
        std::env::set_var("EMAIL_FROM_NAME", "PlainTextChess");
        std::env::set_var("PUBLIC_BASE_URL", "https://play.example.com");
        let config = Config::from_env().expect("valid settings must be accepted");
        assert_eq!(config.email_transport, "resend");
        assert_eq!(config.email_from, "no-reply@play.example.com");
        assert_eq!(config.email_from_name.as_deref(), Some("PlainTextChess"));
        assert_eq!(config.public_base_url, "https://play.example.com");
    }

    #[test]
    fn an_invalid_email_transport_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("EMAIL_TRANSPORT", "carrier-pigeon");
        let err = Config::from_env().expect_err("an unknown transport must be refused");
        assert!(err.contains("EMAIL_TRANSPORT"), "the error must name the variable: {err}");
    }

    #[test]
    fn the_production_mode_comes_from_the_environment() {
        let _guard = env_guard();
        std::env::set_var("DEPLOYMENT_MODE", "production");
        let config = Config::from_env().expect("production is a valid mode");
        assert!(config.production_mode);

        std::env::set_var("DEPLOYMENT_MODE", "development");
        let config = Config::from_env().expect("development is a valid mode");
        assert!(!config.production_mode);
    }

    #[test]
    fn an_invalid_deployment_mode_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("DEPLOYMENT_MODE", "staging-ish");
        let err = Config::from_env().expect_err("an unknown mode must be refused");
        assert!(err.contains("DEPLOYMENT_MODE"), "the error must name the variable: {err}");
    }

    #[test]
    fn the_breach_check_can_be_switched_off() {
        let _guard = env_guard();
        std::env::set_var("HIBP_ENABLED", "0");
        let config = Config::from_env().expect("0 is a valid switch");
        assert!(!config.hibp_enabled);

        std::env::set_var("HIBP_ENABLED", "1");
        let config = Config::from_env().expect("1 is a valid switch");
        assert!(config.hibp_enabled);
    }

    #[test]
    fn an_invalid_breach_switch_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("HIBP_ENABLED", "maybe");
        let err = Config::from_env().expect_err("an unknown switch must be refused");
        assert!(err.contains("HIBP_ENABLED"), "the error must name the variable: {err}");
    }

    #[test]
    fn the_rate_limits_come_from_the_environment() {
        let _guard = env_guard();
        std::env::set_var("AUTH_RATE_LIMIT_LOGIN_PER_MIN", "3");
        std::env::set_var("AUTH_RATE_LIMIT_REGISTER_PER_HOUR", "1");
        let config = Config::from_env().expect("valid limits must be accepted");
        assert_eq!(config.auth_rate_limit_login_per_min, 3);
        assert_eq!(config.auth_rate_limit_register_per_hour, 1);
    }

    #[test]
    fn an_unparseable_rate_limit_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("AUTH_RATE_LIMIT_LOGIN_PER_MIN", "many");
        let err = Config::from_env().expect_err("an unparseable limit must be refused");
        assert!(
            err.contains("AUTH_RATE_LIMIT_LOGIN_PER_MIN"),
            "the error must name the variable: {err}"
        );
    }

    #[test]
    fn the_trusted_proxies_come_from_the_environment() {
        let _guard = env_guard();
        std::env::set_var("AUTH_TRUSTED_PROXY_CIDRS", "172.18.0.0/16, 10.0.0.1");
        let config = Config::from_env().expect("valid proxies must be accepted");
        assert_eq!(
            config.auth_trusted_proxy_cidrs,
            vec!["172.18.0.0/16".to_string(), "10.0.0.1".to_string()]
        );
    }

    #[test]
    fn the_provider_credential_can_come_from_a_file() {
        let _guard = env_guard();
        let path = std::env::temp_dir().join("plaintextchess-config-credential-test");
        std::fs::write(&path, "re_from_file\n").expect("the credential file can be written");
        std::env::set_var("EMAIL_API_KEY_FILE", &path);
        std::env::set_var("EMAIL_API_KEY", "re_from_the_environment");

        let config = Config::from_env().expect("valid settings must be accepted");
        assert_eq!(
            config.email_api_key().as_deref(),
            Some("re_from_file"),
            "the file wins over the environment"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_provider_credential_falls_back_to_the_environment() {
        let _guard = env_guard();
        std::env::set_var("EMAIL_API_KEY", "re_from_the_environment");
        let config = Config::from_env().expect("valid settings must be accepted");
        assert_eq!(config.email_api_key().as_deref(), Some("re_from_the_environment"));
    }

    #[test]
    fn authentication_settings_come_from_the_environment() {
        let _guard = env_guard();
        std::env::set_var("SESSION_TTL_SECS", "3600");
        std::env::set_var("ARGON2_MEMORY_KIB", "32768");
        std::env::set_var("ARGON2_TIME_COST", "3");
        std::env::set_var("ARGON2_PARALLELISM", "2");

        let config = Config::from_env().expect("valid settings must be accepted");
        assert_eq!(config.session_ttl_secs, 3600);
        assert_eq!(config.argon2.memory_kib, 32_768);
        assert_eq!(config.argon2.time_cost, 3);
        assert_eq!(config.argon2.parallelism, 2);
    }

    #[test]
    fn a_below_minimum_session_lifetime_stops_startup() {
        let _guard = env_guard();
        std::env::set_var("SESSION_TTL_SECS", "1");
        let err = Config::from_env().expect_err("1 second must be refused");
        assert!(
            err.contains("SESSION_TTL_SECS") && err.contains("minimum"),
            "the error must name the variable and the floor: {err}"
        );
    }

    #[test]
    fn a_below_minimum_argon2_memory_stops_startup() {
        let _guard = env_guard();
        std::env::set_var("ARGON2_MEMORY_KIB", "1024");
        let err = Config::from_env().expect_err("1 MiB must be refused");
        assert!(
            err.contains("ARGON2_MEMORY_KIB") && err.contains("minimum"),
            "the error must name the variable and the floor: {err}"
        );
    }

    #[test]
    fn a_below_minimum_argon2_time_cost_stops_startup() {
        let _guard = env_guard();
        std::env::set_var("ARGON2_TIME_COST", "0");
        let err = Config::from_env().expect_err("0 passes must be refused");
        assert!(err.contains("ARGON2_TIME_COST"), "{err}");
    }

    #[test]
    fn an_unparseable_authentication_value_stops_startup() {
        let _guard = env_guard();
        std::env::set_var("SESSION_TTL_SECS", "soon");
        let err = Config::from_env().expect_err("a non-numeric TTL must be refused");
        assert!(err.contains("SESSION_TTL_SECS"), "{err}");
    }

    #[test]
    fn exactly_the_minimums_are_accepted() {
        let _guard = env_guard();
        std::env::set_var("SESSION_TTL_SECS", MIN_SESSION_TTL_SECS.to_string());
        std::env::set_var("ARGON2_MEMORY_KIB", MIN_ARGON2_MEMORY_KIB.to_string());
        std::env::set_var("ARGON2_TIME_COST", MIN_ARGON2_TIME_COST.to_string());
        std::env::set_var("ARGON2_PARALLELISM", MIN_ARGON2_PARALLELISM.to_string());
        let config = Config::from_env().expect("the floors themselves are valid");
        assert_eq!(config.session_ttl_secs, MIN_SESSION_TTL_SECS);
        assert_eq!(config.argon2.memory_kib, MIN_ARGON2_MEMORY_KIB);
    }

    #[test]
    fn logic_lanes_defaults_to_one() {
        let _guard = env_guard();
        let config = Config::from_env().expect("defaults must be valid");
        assert_eq!(config.logic_lanes, DEFAULT_LOGIC_LANES);
        assert_eq!(config.logic_control_lanes, DEFAULT_LOGIC_CONTROL_LANES);
    }

    #[test]
    fn logic_lanes_can_be_set_via_env() {
        let _guard = env_guard();
        std::env::set_var("LOGIC_LANES", "4");
        std::env::set_var("LOGIC_CONTROL_LANES", "2");
        let config = Config::from_env().expect("valid lane counts must be accepted");
        assert_eq!(config.logic_lanes, 4);
        assert_eq!(config.logic_control_lanes, 2);
    }

    #[test]
    fn logic_lanes_zero_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("LOGIC_LANES", "0");
        let err = Config::from_env().expect_err("zero lanes must be refused");
        assert!(err.contains("LOGIC_LANES") && err.contains("1"));
    }

    #[test]
    fn logic_control_lanes_zero_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("LOGIC_CONTROL_LANES", "0");
        let err = Config::from_env().expect_err("zero control lanes must be refused");
        assert!(err.contains("LOGIC_CONTROL_LANES") && err.contains("1"));
    }

    #[test]
    fn logic_lanes_invalid_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("LOGIC_LANES", "not-a-number");
        let err = Config::from_env().expect_err("invalid lane count must be refused");
        assert!(err.contains("LOGIC_LANES") && err.contains("valid positive integer"));
    }

    #[test]
    fn logic_control_lanes_invalid_is_rejected() {
        let _guard = env_guard();
        std::env::set_var("LOGIC_CONTROL_LANES", "not-a-number");
        let err = Config::from_env().expect_err("invalid control lanes must be refused");
        assert!(err.contains("LOGIC_CONTROL_LANES") && err.contains("valid positive integer"));
    }

    #[test]
    fn redis_urls_unset_is_empty() {
        let _guard = env_guard();
        std::env::remove_var("REDIS_URL");
        std::env::remove_var("REDIS_URLS");
        let config = Config::from_env().expect("defaults must be valid");
        assert!(config.redis_urls.is_empty());
    }

    #[test]
    fn redis_url_singular_is_a_one_shard_fallback() {
        let _guard = env_guard();
        std::env::remove_var("REDIS_URLS");
        std::env::set_var("REDIS_URL", "redis://127.0.0.1:6379");
        let config = Config::from_env().expect("a single REDIS_URL must be accepted");
        assert_eq!(config.redis_urls, vec!["redis://127.0.0.1:6379".to_string()]);
        std::env::remove_var("REDIS_URL");
    }

    #[test]
    fn redis_urls_plural_splits_on_commas() {
        let _guard = env_guard();
        std::env::set_var("REDIS_URLS", "redis://127.0.0.1:6380, redis://127.0.0.1:6381");
        let config = Config::from_env().expect("REDIS_URLS must be accepted");
        assert_eq!(
            config.redis_urls,
            vec!["redis://127.0.0.1:6380".to_string(), "redis://127.0.0.1:6381".to_string()]
        );
        std::env::remove_var("REDIS_URLS");
    }

    #[test]
    fn database_url_defaults_and_overrides() {
        let _guard = env_guard();
        let config = Config::for_test(
            "127.0.0.1".parse().unwrap(),
            9999,
            Duration::from_secs(30),
        );
        assert_eq!(config.database_url, None, "the test config disables persistence");

        assert_eq!(
            Config::from_env().unwrap().database_url.as_deref(),
            Some(DEFAULT_DATABASE_URL),
            "without DATABASE_URL the built-in default applies"
        );

        std::env::set_var("DATABASE_URL", "sqlite:/tmp/custom-chess-test.db");
        assert_eq!(
            Config::from_env().unwrap().database_url.as_deref(),
            Some("sqlite:/tmp/custom-chess-test.db"),
            "an explicit DATABASE_URL wins over the default"
        );
    }
}
