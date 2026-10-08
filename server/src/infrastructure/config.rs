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

/// Default session lifetime (design D7): 30 days, long enough that a player
/// signs in rarely and short enough that a leaked token stops working.
pub const DEFAULT_SESSION_TTL_SECS: u64 = 30 * 24 * 60 * 60;
/// The shortest session lifetime the server will accept (design D7).
pub const MIN_SESSION_TTL_SECS: u64 = 60;

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

        Ok(Self {
            bind,
            port,
            reconnect_grace: Duration::from_secs(grace_secs),
            database_url: Some(database_url),
            session_ttl_secs,
            argon2,
            random_colors: true,
        })
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    const AUTH_VARS: [&str; 5] = [
        "SESSION_TTL_SECS",
        "ARGON2_MEMORY_KIB",
        "ARGON2_TIME_COST",
        "ARGON2_PARALLELISM",
        "DATABASE_URL",
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
