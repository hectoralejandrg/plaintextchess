//! The advisory mail-domain check (spec "Email Identity").
//!
//! The check asks the system resolver whether the address's domain exists.
//! It is advisory: a domain that does not resolve, or a resolver that cannot
//! be reached, never refuses a registration — the flow continues and the skip
//! is recorded (design D20). The deployment has no domain of its own yet, so
//! a check that cannot be performed must not stand in the way.

use std::net::ToSocketAddrs;

/// The domain part of an address, if it has one.
pub fn domain_of(email: &str) -> Option<&str> {
    email
        .split('@')
        .nth(1)
        .map(str::trim)
        .filter(|domain| !domain.is_empty())
}

/// Whether the address's domain resolves.
///
/// The port is irrelevant to the lookup: the resolver is asked for the
/// domain's addresses, not for a listening service.
pub fn domain_resolves(email: &str) -> bool {
    let Some(domain) = domain_of(email) else {
        return false;
    };
    (domain, 25u16)
        .to_socket_addrs()
        .map(|mut addresses| addresses.next().is_some())
        .unwrap_or(false)
}

/// Run the advisory check, recording a skip when it cannot be performed.
///
/// The log line names no address: an email address never reaches a log line
/// (spec "Credential and Session Data Protection").
pub fn check_and_record(email: &str) -> bool {
    let resolves = domain_resolves(email);
    if !resolves {
        tracing::info!(
            "the mail-domain check was skipped: the address's domain could not be resolved"
        );
    }
    resolves
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_domain_is_the_part_after_the_at_sign() {
        assert_eq!(domain_of("ana@example.com"), Some("example.com"));
        assert_eq!(domain_of("ana@ mail.example.com "), Some("mail.example.com"));
    }

    #[test]
    fn an_address_without_a_domain_has_none() {
        assert_eq!(domain_of("not-an-address"), None);
        assert_eq!(domain_of("ana@"), None);
    }

    #[test]
    fn an_unresolvable_domain_is_reported_as_such() {
        // `.invalid` is reserved and never resolves, so the check fails
        // without touching the network's real domains.
        assert!(!domain_resolves("ana@nonexistent-host.invalid"));
    }

    #[test]
    fn a_malformed_address_never_resolves() {
        assert!(!domain_resolves("not-an-address"));
    }

    #[test]
    fn the_check_records_a_skip_and_never_refuses() {
        // The check is advisory: it reports what it found and the caller
        // carries on either way.
        assert!(!check_and_record("ana@nonexistent-host.invalid"));
        assert!(!check_and_record("not-an-address"));
    }
}
