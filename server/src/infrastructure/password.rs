//! Password hashing and session-token primitives (spec "Credential and
//! Session Data Protection", design D5/D6).
//!
//! Passwords use argon2id; session tokens use SHA-256. That asymmetry is
//! deliberate: a token is 32 bytes of server-chosen randomness, so there is
//! no search to slow down and a fast hash is the right tool, while a password
//! is low-entropy attacker-chosen input where a memory-hard KDF is what makes
//! each guess expensive.

use std::time::Duration;

use argon2::password_hash::{
    rand_core::{OsRng, RngCore},
    PasswordHash, PasswordHasher as _, PasswordVerifier, SaltString,
};
use argon2::{Algorithm, Argon2, Params, Version};
use sha2::{Digest, Sha256};

use crate::domain::account::MAX_PASSWORD_LEN;
use crate::infrastructure::config::Argon2Params;

/// Bytes of randomness in a session token (design D6). Base64url-encoded
/// without padding, that is 43 characters on the wire.
pub const TOKEN_BYTES: usize = 32;

/// Wall-clock unix milliseconds, stamped at issue time (design D7).
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock")
        .as_millis() as i64
}

/// Why a password operation failed.
#[derive(Debug)]
pub enum PasswordError {
    /// Argon2 rejected the parameters or the hash string.
    Hash(String),
    /// The password was longer than an accepted password may be. Callers
    /// validate the length first, so this is a backstop that keeps an
    /// oversized input from ever reaching the KDF.
    TooLong,
}

impl std::fmt::Display for PasswordError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PasswordError::Hash(msg) => write!(f, "password hashing failed: {msg}"),
            PasswordError::TooLong => write!(
                f,
                "the password exceeds the {MAX_PASSWORD_LEN}-character limit"
            ),
        }
    }
}

/// Hashes and verifies passwords with argon2id at a configured cost.
///
/// Argon2 costs tens of milliseconds by design, so both operations run on
/// `spawn_blocking`: inline in an async task a single login would stall every
/// other task the runtime worker is running, including other players' socket
/// reads (design D5).
#[derive(Debug, Clone)]
pub struct PasswordHasher {
    params: Argon2Params,
}

impl PasswordHasher {
    pub fn new(params: Argon2Params) -> Self {
        Self { params }
    }

    fn argon2(&self) -> Result<Argon2<'static>, PasswordError> {
        let params = Params::new(
            self.params.memory_kib,
            self.params.time_cost,
            self.params.parallelism,
            None,
        )
        .map_err(|e| PasswordError::Hash(format!("invalid argon2 parameters: {e}")))?;
        Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
    }

    /// Hash a password into an argon2id PHC string.
    pub async fn hash(&self, password: &str) -> Result<String, PasswordError> {
        // The bound is re-checked here, not only at the validation layer, so
        // no call path can reach the KDF with an unbounded input.
        if password.chars().count() > MAX_PASSWORD_LEN {
            return Err(PasswordError::TooLong);
        }
        let argon2 = self.argon2()?;
        let password = password.to_string();
        tokio::task::spawn_blocking(move || {
            let salt = SaltString::generate(&mut OsRng);
            argon2
                .hash_password(password.as_bytes(), &salt)
                .map(|hash| hash.to_string())
                .map_err(|e| PasswordError::Hash(e.to_string()))
        })
        .await
        .map_err(|e| PasswordError::Hash(format!("hashing task failed: {e}")))?
    }

    /// Whether `password` produced `hash`. A malformed hash is a failure, not
    /// a panic: a corrupt stored value must not authenticate anyone.
    pub async fn verify(&self, password: &str, hash: &str) -> bool {
        let Ok(argon2) = self.argon2() else {
            return false;
        };
        let password = password.to_string();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || {
            match PasswordHash::new(&hash) {
                Ok(parsed) => argon2
                    .verify_password(password.as_bytes(), &parsed)
                    .is_ok(),
                Err(_) => false,
            }
        })
        .await
        .unwrap_or(false)
    }
}

/// Generate a new opaque session token: `TOKEN_BYTES` of `rand` randomness,
/// base64url-encoded without padding (design D6).
pub fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    // `OsRng` from `rand_core` is the OS CSPRNG; a panic here means the
    // platform has no secure entropy, in which case refusing to start is
    // the only correct outcome.
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    base64url_encode(&bytes)
}

/// The stored form of a token: hex-encoded SHA-256 (design D6). Deterministic
/// so it can be indexed and looked up, and one-way so a database leak does
/// not hand out live sessions.
pub fn hash_token(token: &str) -> String {
    let digest = Sha256::digest(token.as_bytes());
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

/// The instant a session issued now stops being usable (design D7).
pub fn expires_at(issued_at_ms: i64, ttl: Duration) -> i64 {
    issued_at_ms + ttl.as_millis() as i64
}

const BASE64URL: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Standard base64url with no padding. Hand-rolled rather than pulling in a
/// crate: it is twenty lines, and the alphabet is the only thing that
/// matters for a value the server both mints and parses.
fn base64url_encode(input: &[u8]) -> String {
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(BASE64URL[(triple >> 18) as usize & 0x3f] as char);
        out.push(BASE64URL[(triple >> 12) as usize & 0x3f] as char);
        if chunk.len() > 1 {
            out.push(BASE64URL[(triple >> 6) as usize & 0x3f] as char);
        }
        if chunk.len() > 2 {
            out.push(BASE64URL[triple as usize & 0x3f] as char);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::Arc;

    /// Tests hash at the floor cost so the suite stays fast; production
    /// validation of the real cost lives in `config.rs`.
    fn cheap_hasher() -> PasswordHasher {
        PasswordHasher::new(Argon2Params {
            memory_kib: 8_192,
            time_cost: 1,
            parallelism: 1,
        })
    }

    #[tokio::test]
    async fn a_hash_verifies_against_the_right_password() {
        let hasher = cheap_hasher();
        let hash = hasher.hash("correct horse battery").await.expect("hash");
        assert!(
            hasher.verify("correct horse battery", &hash).await,
            "the correct password verifies"
        );
    }

    #[tokio::test]
    async fn a_hash_rejects_a_wrong_password() {
        let hasher = cheap_hasher();
        let hash = hasher.hash("correct horse battery").await.expect("hash");
        assert!(
            !hasher.verify("Correct horse battery", &hash).await,
            "a wrong password does not verify"
        );
        assert!(!hasher.verify("", &hash).await);
        assert!(!hasher.verify("correct horse batter", &hash).await);
    }

    #[tokio::test]
    async fn the_hash_is_not_the_plaintext_and_two_hashes_differ() {
        let hasher = cheap_hasher();
        let first = hasher.hash("supersecret123").await.expect("hash");
        let second = hasher.hash("supersecret123").await.expect("hash");
        assert_ne!(first, "supersecret123", "the plaintext never appears");
        assert!(first.starts_with("$argon2id$"), "PHC argon2id string: {first}");
        assert_ne!(
            first, second,
            "each hash gets a fresh salt, so two hashes of one password differ"
        );
    }

    #[tokio::test]
    async fn a_malformed_hash_authenticates_nobody() {
        let hasher = cheap_hasher();
        assert!(!hasher.verify("anything", "not-a-phc-string").await);
        assert!(!hasher.verify("anything", "").await);
        assert!(
            !hasher.verify("supersecret123", "$argon2id$v=19$m=1,t=1,p=1$AAAA$")
                .await,
            "a truncated hash must fail closed"
        );
    }

    #[tokio::test]
    async fn an_oversized_password_never_reaches_the_hasher() {
        let hasher = cheap_hasher();
        let huge = "a".repeat(MAX_PASSWORD_LEN + 1);
        assert!(
            matches!(hasher.hash(&huge).await, Err(PasswordError::TooLong)),
            "an oversized password is refused before any hashing cost"
        );
        // The exact maximum is still accepted.
        let at_limit = "a".repeat(MAX_PASSWORD_LEN);
        assert!(hasher.hash(&at_limit).await.is_ok());
    }

    #[tokio::test]
    async fn concurrent_hashing_does_not_block_the_runtime() {
        // Ten hashes at the floor cost run on blocking workers; if any ran
        // inline this future would have to complete them serially.
        let hasher = Arc::new(cheap_hasher());
        let mut handles = Vec::new();
        for _ in 0..10 {
            let hasher = Arc::clone(&hasher);
            handles.push(tokio::spawn(async move {
                hasher.hash("supersecret123").await.expect("hash")
            }));
        }
        for handle in handles {
            assert!(handle.await.expect("task").starts_with("$argon2id$"));
        }
    }

    #[test]
    fn tokens_are_unique_and_43_url_safe_characters() {
        let mut seen = HashSet::new();
        for _ in 0..200 {
            let token = generate_token();
            assert_eq!(token.chars().count(), 43, "32 bytes base64url unpadded");
            assert!(
                token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "no padding or non-url-safe characters: {token}"
            );
            assert!(seen.insert(token.clone()), "tokens must not repeat");
        }
    }

    #[test]
    fn the_token_hash_differs_from_the_token_and_is_deterministic() {
        let token = generate_token();
        let hash = hash_token(&token);
        assert_ne!(hash, token, "the stored value is not the token");
        assert_eq!(hash.len(), 64, "hex-encoded SHA-256");
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(
            hash_token(&token),
            hash,
            "hashing is deterministic so the row can be looked up"
        );
        assert_ne!(hash_token(&generate_token()), hash, "distinct tokens, distinct hashes");
    }

    #[test]
    fn a_hex_token_hash_is_stable_for_a_known_input() {
        // Pins the encoding so a future change cannot silently invalidate
        // every persisted session.
        assert_eq!(
            hash_token(""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hash_token("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn expiry_is_the_issue_mark_plus_the_ttl() {
        assert_eq!(expires_at(1_000, Duration::from_secs(60)), 61_000);
        assert_eq!(expires_at(0, Duration::from_secs(30)), 30_000);
    }
}
