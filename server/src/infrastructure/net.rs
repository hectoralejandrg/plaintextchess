//! The address helpers the rate limit needs (spec "Authentication Rate
//! Limiting").
//!
//! The client address comes from the forwarding header the reverse proxy
//! sets, but only when the peer is one of the configured trusted proxies:
//! a header presented by any other peer is ignored, because the header is
//! a claim, not a fact.

use std::net::IpAddr;

/// A network in CIDR notation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    address: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Parse `10.0.0.0/8` or a bare address (`10.0.0.1`, which is treated
    /// as the whole address).
    pub fn parse(raw: &str) -> Option<Self> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }
        let (address, prefix) = match trimmed.split_once('/') {
            Some((address, prefix)) => {
                let prefix = prefix.trim().parse::<u8>().ok()?;
                (address.trim().parse::<IpAddr>().ok()?, prefix)
            }
            // A bare address is a network of one, not the whole world.
            None => {
                let address = trimmed.parse::<IpAddr>().ok()?;
                let prefix = match address {
                    IpAddr::V4(_) => 32,
                    IpAddr::V6(_) => 128,
                };
                (address, prefix)
            }
        };
        let max = match address {
            IpAddr::V4(_) => 32,
            IpAddr::V6(_) => 128,
        };
        if prefix > max {
            return None;
        }
        Some(Self { address, prefix })
    }

    /// Whether the address is inside the network.
    pub fn contains(&self, address: IpAddr) -> bool {
        match (self.address, address) {
            (IpAddr::V4(network), IpAddr::V4(candidate)) => {
                mask_v4(network, self.prefix) == mask_v4(candidate, self.prefix)
            }
            (IpAddr::V6(network), IpAddr::V6(candidate)) => {
                mask_v6(network, self.prefix) == mask_v6(candidate, self.prefix)
            }
            // An IPv4 peer is never inside an IPv6 network, and the other
            // way round.
            _ => false,
        }
    }
}

/// The networks that may present a forwarding header.
pub fn trusted_networks(entries: &[String]) -> Vec<Cidr> {
    entries.iter().filter_map(|entry| Cidr::parse(entry)).collect()
}

/// Whether the peer is one of the trusted proxies.
pub fn is_trusted_peer(peer: IpAddr, networks: &[Cidr]) -> bool {
    networks.iter().any(|network| network.contains(peer))
}

/// The address the attempt is keyed by.
///
/// The forwarding header is a *chain*: the proxy appends the address it saw to
/// whatever the client sent. The entry that matters is therefore the **last**
/// one — the proxy's own reading — and never the first, which the client
/// chose. A client that rotates its own header therefore lands in the same
/// bucket instead of a new one every time.
///
/// The header is used only when the peer is a trusted proxy; from any other
/// peer the peer's own address is the answer, and a peer with no known
/// address is keyed by a constant so the attempts are still counted together
/// rather than not at all.
pub fn client_address(
    peer: Option<std::net::SocketAddr>,
    forwarded: Option<&str>,
    networks: &[Cidr],
) -> String {
    let peer_address = peer.map(|peer| peer.ip());
    let trusted = peer_address.is_some_and(|peer| is_trusted_peer(peer, networks));
    if trusted {
        if let Some(address) = forwarded
            .map(|chain| chain.split(',').rev())
            .and_then(|mut entries| entries.find_map(|entry| parse_address(entry.trim())))
        {
            return address.to_string();
        }
    }
    match peer_address {
        Some(address) => address.to_string(),
        None => "unknown".to_string(),
    }
}

/// An address, when the text is one.
fn parse_address(raw: &str) -> Option<std::net::IpAddr> {
    if raw.is_empty() {
        return None;
    }
    raw.parse::<std::net::IpAddr>().ok()
}

/// The masked form of an IPv4 address.
fn mask_v4(address: std::net::Ipv4Addr, prefix: u8) -> u32 {
    let bits = u32::from(address);
    if prefix == 0 {
        0
    } else {
        bits & (!0u32 << (32 - prefix as u32))
    }
}

/// The masked form of an IPv6 address.
fn mask_v6(address: std::net::Ipv6Addr, prefix: u8) -> u128 {
    let bits = u128::from(address);
    if prefix == 0 {
        0
    } else {
        bits & (!0u128 << (128 - prefix as u32))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;

    fn peer(ip: &str) -> SocketAddr {
        format!("{ip}:4000").parse().expect("an address")
    }

    #[test]
    fn a_network_is_parsed_and_matched() {
        let network = Cidr::parse("172.18.0.0/16").expect("a network");
        assert!(network.contains("172.18.5.1".parse().expect("an address")));
        assert!(!network.contains("172.19.5.1".parse().expect("an address")));
        assert!(!network.contains("10.0.0.1".parse().expect("an address")));
    }

    #[test]
    fn a_bare_address_is_a_network_of_one() {
        let network = Cidr::parse("10.0.0.1").expect("a network");
        assert!(network.contains("10.0.0.1".parse().expect("an address")));
        assert!(!network.contains("10.0.0.2".parse().expect("an address")));
    }

    #[test]
    fn an_invalid_network_is_refused() {
        assert!(Cidr::parse("not-an-address").is_none());
        assert!(Cidr::parse("10.0.0.0/40").is_none());
        assert!(Cidr::parse("").is_none());
    }

    #[test]
    fn the_forwarding_header_is_used_only_from_a_trusted_peer() {
        let networks = trusted_networks(&["172.18.0.0/16".to_string()]);
        let trusted = client_address(Some(peer("172.18.0.2")), Some("203.0.113.9"), &networks);
        assert_eq!(trusted, "203.0.113.9");

        let untrusted = client_address(Some(peer("10.0.0.2")), Some("203.0.113.9"), &networks);
        assert_eq!(
            untrusted, "10.0.0.2",
            "a header from a peer that is not the proxy is ignored"
        );
    }

    #[test]
    fn the_last_entry_of_the_chain_is_the_one_the_proxy_saw() {
        let networks = trusted_networks(&["172.18.0.0/16".to_string()]);
        // The client sent a header of its own; the proxy appended the
        // address it saw. The first entry is the client's claim.
        let address = client_address(
            Some(peer("172.18.0.2")),
            Some("1.1.1.1, 203.0.113.9"),
            &networks,
        );
        assert_eq!(
            address, "203.0.113.9",
            "a client that rotates its own header must not get a new bucket"
        );
    }

    #[test]
    fn a_chain_without_an_address_falls_back_to_the_peer() {
        let networks = trusted_networks(&["172.18.0.0/16".to_string()]);
        let address = client_address(Some(peer("172.18.0.2")), Some("not-an-address"), &networks);
        assert_eq!(address, "172.18.0.2");
    }

    #[test]
    fn a_peer_without_a_forwarding_header_keeps_its_own_address() {
        let networks = trusted_networks(&["172.18.0.0/16".to_string()]);
        assert_eq!(
            client_address(Some(peer("172.18.0.2")), None, &networks),
            "172.18.0.2"
        );
    }

    #[test]
    fn a_peer_with_no_known_address_is_keyed_together() {
        let networks = trusted_networks(&["172.18.0.0/16".to_string()]);
        assert_eq!(client_address(None, Some("203.0.113.9"), &networks), "unknown");
    }
}
