//! Where a sharing browser lets another browser's SDP send it ICE checks.
//!
//! The browser's WebRTC stack runs connectivity checks to every candidate in
//! the remote description, before either side is authenticated. A browser
//! that a relay introduced could nominate any endpoint: a device on the
//! visitor's LAN, loopback, a carrier-NAT or tailnet address, or a third
//! party to flood with checks. So a sharing browser hands its WebRTC stack
//! only candidates that an honest browser elsewhere on the internet sends:
//!
//! - host candidates named by a random mDNS name (`<uuid>.local`), as Chrome
//!   and Firefox send instead of their interface addresses;
//! - any candidate, of any type, on a public unicast address and a port of
//!   1024 or more: server-reflexive, peer-reflexive and relay (TURN), and a
//!   host whose interface address is public.
//!
//! Everything else is dropped: loopback, private (RFC 1918, fc00::/7),
//! link-local, carrier-grade NAT (100.64.0.0/10), multicast, unspecified,
//! documentation, benchmarking and other special-purpose addresses, any other
//! host name, and ports below 1024 (an active TCP candidate's 9 included: it
//! is never dialled). At most [`MAX_CANDIDATES`] candidates are kept. A
//! description that isn't SDP, is too large, has a candidate line that
//! doesn't parse, or keeps no candidate, is refused. A connection-line
//! address that would be dropped is replaced with the unspecified address.
//!
//! Two honest browsers on one LAN still connect through their mDNS names, or
//! through their public addresses if the NAT hairpins. A peer that sends its
//! private interface address instead of an mDNS name (a browser with mDNS
//! obfuscation turned off, or a native node) can then reach another browser
//! only over a public address or TURN.
//!
//! The relay can't apply this: the offers it forwards are encrypted between
//! the two browsers. Each browser applies it to the description it receives,
//! and passes the filtered copy to its WebRTC stack only; the connection's
//! authentication hashes the description as it was sent.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Largest description taken: an honest one is 1 to 4 KiB.
pub const MAX_SDP_BYTES: usize = 16 * 1024;
/// Most candidate lines a description may have before it is refused.
pub const MAX_CANDIDATE_LINES: usize = 64;
/// Most candidates kept. A browser with two relays' TURN lists sends about 10:
/// one or two mDNS hosts, one or two server-reflexive, two passive TCP hosts,
/// a TURN relay candidate per TURN URL.
pub const MAX_CANDIDATES: usize = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum SdpRefusal {
    #[display(fmt = "not an SDP description")]
    NotSdp,
    #[display(fmt = "the description is too large")]
    TooLarge,
    #[display(fmt = "too many candidates")]
    TooManyCandidates,
    #[display(fmt = "a candidate line doesn't parse")]
    BadCandidate,
    #[display(fmt = "no candidate the policy allows")]
    NoCandidate,
}

/// A description with only the candidates the policy allows, or why it is
/// refused. Line endings are kept as CRLF.
pub fn filter_sdp(sdp: &str) -> Result<String, SdpRefusal> {
    if sdp.len() > MAX_SDP_BYTES {
        return Err(SdpRefusal::TooLarge);
    }
    if !sdp.starts_with("v=0") || !sdp.is_ascii() {
        return Err(SdpRefusal::NotSdp);
    }
    let mut out = String::with_capacity(sdp.len());
    let (mut lines, mut kept) = (0usize, 0usize);
    for line in sdp.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)) {
        if line.is_empty() {
            continue;
        }
        if let Some(candidate) = line.strip_prefix("a=candidate:") {
            lines += 1;
            if lines > MAX_CANDIDATE_LINES {
                return Err(SdpRefusal::TooManyCandidates);
            }
            if !candidate_allowed(candidate)? || kept >= MAX_CANDIDATES {
                continue;
            }
            kept += 1;
            out.push_str(line);
        } else if let Some(connection) = line.strip_prefix("c=") {
            out.push_str("c=");
            out.push_str(&connection_line(connection));
        } else {
            out.push_str(line);
        }
        out.push_str("\r\n");
    }
    if kept == 0 {
        return Err(SdpRefusal::NoCandidate);
    }
    Ok(out)
}

/// `<foundation> <component> <transport> <priority> <address> <port> typ
/// <type> ...`: whether to keep it; an error if it doesn't parse.
fn candidate_allowed(candidate: &str) -> Result<bool, SdpRefusal> {
    if candidate.len() > 512 {
        return Err(SdpRefusal::BadCandidate);
    }
    let fields: Vec<&str> = candidate.split_ascii_whitespace().collect();
    let [_foundation, component, transport, priority, address, port, typ, kind, ..] =
        fields.as_slice()
    else {
        return Err(SdpRefusal::BadCandidate);
    };
    if *typ != "typ"
        || component.parse::<u8>().is_err()
        || priority.parse::<u32>().is_err()
        || !matches!(transport.to_ascii_lowercase().as_str(), "udp" | "tcp")
        || !matches!(*kind, "host" | "srflx" | "prflx" | "relay")
    {
        return Err(SdpRefusal::BadCandidate);
    }
    let port: u16 = port.parse().map_err(|_| SdpRefusal::BadCandidate)?;
    if port < 1024 {
        return Ok(false);
    }
    Ok(match address.parse::<IpAddr>() {
        Ok(ip) => is_public(ip),
        Err(_) => *kind == "host" && is_mdns_name(address),
    })
}

/// `IN IP4 <address>`: a dropped address becomes the unspecified one.
fn connection_line(connection: &str) -> String {
    let mut parts = connection.split_ascii_whitespace();
    let (Some(net), Some(family), Some(address)) = (parts.next(), parts.next(), parts.next())
    else {
        return connection.to_owned();
    };
    let keep = match address.parse::<IpAddr>() {
        Ok(ip) => ip.is_unspecified() || is_public(ip),
        Err(_) => false,
    };
    if keep {
        connection.to_owned()
    } else if family == "IP6" {
        format!("{net} IP6 ::")
    } else {
        format!("{net} IP4 0.0.0.0")
    }
}

/// A random mDNS name, `8-4-4-4-12` hex digits then `.local`.
fn is_mdns_name(name: &str) -> bool {
    let Some(uuid) = name.strip_suffix(".local") else {
        return false;
    };
    let groups: Vec<&str> = uuid.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(g, n)| g.len() == n && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

/// A public unicast address: none of the special-purpose ranges.
pub fn is_public(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    !(a == 0 // this network
        || a == 10
        || (a == 100 && (64..128).contains(&b)) // carrier-grade NAT
        || a == 127
        || (a == 169 && b == 254)
        || (a == 172 && (16..32).contains(&b))
        || (a == 192 && b == 0 && c == 0) // IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // documentation
        || (a == 192 && b == 88 && c == 99) // 6to4 relay anycast
        || (a == 192 && b == 168)
        || (a == 198 && (18..20).contains(&b)) // benchmarking
        || (a == 198 && b == 51 && c == 100) // documentation
        || (a == 203 && b == 0 && c == 113) // documentation
        || a >= 224) // multicast, reserved, broadcast
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    // Global unicast is 2000::/3, less its special-purpose blocks.
    (s[0] & 0xe000) == 0x2000
        && !(s[0] == 0x2001 && s[1] < 0x0200) // 2001::/23: Teredo, ORCHID, benchmarking
        && !(s[0] == 0x2001 && s[1] == 0x0db8) // documentation
        && s[0] != 0x2002 // 6to4
        && s[0] != 0x3fff // documentation (2024)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MDNS: &str = "2e5a5b7c-1c2f-4d7e-9f4b-123456789abc.local";

    fn sdp(candidates: &[String]) -> String {
        let mut s = String::from(
            "v=0\r\no=- 1 2 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\nc=IN IP4 0.0.0.0\r\n",
        );
        for c in candidates {
            s.push_str(&format!("a=candidate:{c}\r\n"));
        }
        s.push_str("a=end-of-candidates\r\n");
        s
    }

    fn cand(address: &str, port: u16, kind: &str) -> String {
        format!("1 1 udp 2122260223 {address} {port} typ {kind} generation 0")
    }

    fn kept(candidates: &[String]) -> Vec<String> {
        filter_sdp(&sdp(candidates))
            .map(|s| {
                s.lines()
                    .filter_map(|l| l.strip_prefix("a=candidate:"))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    }

    #[test]
    fn an_honest_chrome_description_is_kept_whole() {
        let honest = [
            cand(MDNS, 54321, "host"),
            format!("2 1 tcp 1518280447 {MDNS} 9 typ host tcptype active generation 0"),
            format!("3 1 tcp 1518280447 {MDNS} 50123 typ host tcptype passive generation 0"),
            "4 1 udp 1686052607 8.8.4.4 54321 typ srflx raddr 0.0.0.0 rport 0 generation 0"
                .to_owned(),
            "5 1 udp 1686052351 2a00:1450:4001:80b::200e 54322 typ srflx raddr :: rport 0"
                .to_owned(),
            "6 1 udp 41885439 1.1.1.1 49200 typ relay raddr 8.8.4.4 rport 54321".to_owned(),
        ];
        // The active TCP candidate (port 9) is never dialled: dropped.
        assert_eq!(kept(&honest).len(), 5);
        // Filtering is idempotent.
        let filtered = filter_sdp(&sdp(&honest)).unwrap();
        assert_eq!(filter_sdp(&filtered).unwrap(), filtered);
    }

    #[test]
    fn special_purpose_destinations_are_dropped() {
        for address in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "172.31.255.1",
            "192.168.1.10",
            "169.254.0.5",
            "100.64.0.1",
            "100.127.3.4",
            "0.0.0.0",
            "224.0.0.251",
            "239.255.255.250",
            "240.0.0.1",
            "255.255.255.255",
            "192.0.2.66",
            "198.51.100.7",
            "203.0.113.9",
            "198.18.0.1",
            "::",
            "::1",
            "fe80::1",
            "fd7a:115c:a1e0::1",
            "fc00::5",
            "ff02::fb",
            "2001:db8::1",
            "2001::1",
            "2002:c000:0201::1",
            "::ffff:192.168.1.1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
            "printer.local",
            "router.lan",
            "evil.example.com",
        ] {
            for kind in ["host", "srflx", "prflx", "relay"] {
                let candidates = [cand(address, 50000, kind), cand(MDNS, 50001, "host")];
                assert_eq!(
                    kept(&candidates),
                    vec![cand(MDNS, 50001, "host")],
                    "{address} {kind}"
                );
            }
        }
        // An mDNS name for a reflexive or relay candidate isn't honest either.
        assert_eq!(kept(&[cand(MDNS, 50000, "srflx")]), Vec::<String>::new());
        // Public addresses are kept, whatever the type; low ports aren't.
        for kind in ["host", "srflx", "prflx", "relay"] {
            assert_eq!(kept(&[cand("8.8.8.8", 50000, kind)]).len(), 1);
            assert_eq!(kept(&[cand("2606:4700::1111", 50000, kind)]).len(), 1);
            assert!(kept(&[cand("8.8.8.8", 53, kind)]).is_empty());
            assert!(kept(&[cand("8.8.8.8", 443, kind)]).is_empty());
        }
    }

    #[test]
    fn bad_descriptions_are_refused() {
        assert_eq!(filter_sdp("hello"), Err(SdpRefusal::NotSdp));
        assert_eq!(
            filter_sdp(&"v=0\r\n".repeat(MAX_SDP_BYTES)),
            Err(SdpRefusal::TooLarge)
        );
        assert_eq!(
            filter_sdp(&sdp(&[cand("10.0.0.1", 50000, "host")])),
            Err(SdpRefusal::NoCandidate)
        );
        for bad in [
            "1 1 udp 2122260223 8.8.8.8 50000 type host".to_owned(),
            "1 1 sctp 2122260223 8.8.8.8 50000 typ host".to_owned(),
            "1 1 udp 2122260223 8.8.8.8 99999 typ host".to_owned(),
            "1 1 udp 2122260223 8.8.8.8 50000 typ hostile".to_owned(),
            "1 1 udp prio 8.8.8.8 50000 typ host".to_owned(),
            "1 1 udp 2122260223 8.8.8.8".to_owned(),
        ] {
            assert_eq!(
                filter_sdp(&sdp(&[cand(MDNS, 50000, "host"), bad.clone()])),
                Err(SdpRefusal::BadCandidate),
                "{bad}"
            );
        }
        let many: Vec<_> = (0..=MAX_CANDIDATE_LINES)
            .map(|i| cand("8.8.8.8", 50000 + i as u16, "srflx"))
            .collect();
        assert_eq!(filter_sdp(&sdp(&many)), Err(SdpRefusal::TooManyCandidates));
    }

    #[test]
    fn at_most_twelve_candidates_are_kept() {
        let many: Vec<_> = (0..40)
            .map(|i| cand("8.8.8.8", 50000 + i as u16, "srflx"))
            .collect();
        assert_eq!(kept(&many), many[..MAX_CANDIDATES].to_vec());
    }

    #[test]
    fn a_private_connection_address_becomes_unspecified() {
        let with = |c: &str| {
            sdp(&[cand(MDNS, 50000, "host")]).replace("c=IN IP4 0.0.0.0", &format!("c={c}"))
        };
        let filtered = filter_sdp(&with("IN IP4 192.168.1.4")).unwrap();
        assert!(filtered.contains("\r\nc=IN IP4 0.0.0.0\r\n"), "{filtered}");
        let filtered = filter_sdp(&with("IN IP6 fe80::1")).unwrap();
        assert!(filtered.contains("\r\nc=IN IP6 ::\r\n"), "{filtered}");
        let filtered = filter_sdp(&with("IN IP4 8.8.8.8")).unwrap();
        assert!(filtered.contains("\r\nc=IN IP4 8.8.8.8\r\n"), "{filtered}");
    }
}
