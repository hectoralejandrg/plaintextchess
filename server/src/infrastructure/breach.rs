//! The breached-password check (spec "Breached Password Refusal").
//!
//! The corpus is queried with the k-anonymity API: the server sends only the
//! first five characters of the password's SHA-1, so the corpus learns
//! nothing about the password beyond a prefix of its hash.
//!
//! The check is advisory in one direction only: when the corpus cannot be
//! reached, the attempt continues and the skip is recorded (design D8). A
//! third-party outage must not stop every registration.

use sha1::{Digest, Sha1};

/// The corpus's range endpoint.
const ENDPOINT: &str = "https://api.pwnedpasswords.com/range/";

/// How long a corpus query may take.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The first five characters of the password's SHA-1, uppercase.
///
/// This is the whole prefix the corpus is asked about: the rest of the hash
/// never leaves the server.
pub fn password_prefix(password: &str) -> String {
    let digest = Sha1::digest(password.as_bytes());
    let mut hex = String::with_capacity(40);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex[..5].to_uppercase()
}

/// The suffix the corpus is asked about: the rest of the hash, uppercase.
fn password_suffix(password: &str) -> String {
    let digest = Sha1::digest(password.as_bytes());
    let mut hex = String::with_capacity(40);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex[5..].to_uppercase()
}

/// Whether the password appears in the corpus.
///
/// `Ok(false)` with a recorded skip means the corpus could not be reached:
/// the check did not clear the password, it simply did not run.
pub fn is_breached(password: &str) -> bool {
    is_breached_at(ENDPOINT, password)
}

/// The same check against a given endpoint. The endpoint is a parameter so a
/// test can point the check at a local server.
pub fn is_breached_at(endpoint: &str, password: &str) -> bool {
    let prefix = password_prefix(password);
    let suffix = password_suffix(password);

    let agent = ureq::AgentBuilder::new().timeout(TIMEOUT).build();
    let outcome = agent.get(&format!("{endpoint}{prefix}")).call();
    let body = match outcome {
        Ok(response) => response.into_string().unwrap_or_default(),
        Err(err) => {
            tracing::info!(
                error = %err,
                "the breached-password check was skipped: the corpus could not be reached"
            );
            return false;
        }
    };

    body.lines()
        .filter_map(|line| line.split_once(':'))
        .any(|(candidate, _count)| candidate.trim().eq_ignore_ascii_case(&suffix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_is_five_uppercase_hex_characters() {
        let prefix = password_prefix("correct horse battery staple");
        assert_eq!(prefix.len(), 5);
        assert!(prefix.chars().all(|c| c.is_ascii_hexdigit() && !c.is_lowercase()));
    }

    #[test]
    fn the_same_password_always_gives_the_same_prefix() {
        assert_eq!(password_prefix("hunter2"), password_prefix("hunter2"));
        assert_ne!(password_prefix("hunter2"), password_prefix("hunter3"));
    }

    /// A one-shot HTTP server that answers every request with `body`.
    fn corpus_server(body: String) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the corpus");
        let address = listener.local_addr().expect("the corpus address");
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                use std::io::{Read, Write};
                let mut buffer = [0_u8; 1024];
                let _ = stream.read(&mut buffer);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
            }
        });
        format!("http://{address}/range/")
    }

    #[test]
    fn a_reported_breach_is_refused() {
        let password = "correct horse battery";
        let suffix = password_suffix(password);
        let endpoint = corpus_server(format!("{suffix}:42\n"));
        assert!(is_breached_at(&endpoint, password));
    }

    #[test]
    fn a_password_the_corpus_does_not_hold_is_accepted() {
        let password = "correct horse battery";
        let endpoint = corpus_server("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA:1\n".to_string());
        assert!(!is_breached_at(&endpoint, password));
    }

    #[test]
    fn an_unreachable_corpus_lets_the_registration_through() {
        // Nothing is listening on this port, so the check cannot be made.
        let endpoint = "http://127.0.0.1:1/range/";
        assert!(!is_breached_at(&endpoint, "correct horse battery"));
    }

    #[test]
    fn the_prefix_never_contains_the_password() {
        let password = "a-very-distinctive-passphrase";
        let prefix = password_prefix(password);
        assert!(!prefix.contains(password));
        assert_ne!(prefix, password.to_uppercase());
    }
}
