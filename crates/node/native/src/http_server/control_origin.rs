//! Who may change the SNARK worker from a browser (P5.4's review).
//!
//! `POST /snarker/control` changes where fees go, so a page that the
//! operator's browser happens to show must not be able to call it. The HTTP
//! server answers every origin (the dashboard may be served from anywhere),
//! so this route checks more:
//!
//! - The request carries [`WORKER_CONTROL_HEADER`] and `Content-Type:
//!   application/json`. A browser can't send either without a CORS
//!   preflight, and the preflight succeeds only for an allowed origin.
//! - Its `Origin`, which browsers send with every POST and pages can't
//!   change, is allowed: a page served from this machine (`localhost`,
//!   `127.0.0.1` or `[::1]`, any port), one the operator lists with
//!   `--worker-control-origin`, or, when the node serves its dashboard, the
//!   node's own page opened at an IP address ([`same_origin`], F-console). A
//!   request without one comes from a program, not a page, and is let
//!   through.
//!
//! A host name is not trusted as same-origin: a page on another site can
//! make its name resolve to this node (DNS rebinding), and its requests then
//! look same-origin. Only an IP address or `localhost` in `Host`, which no
//! other site can claim, makes a page the node's own; a name must be listed.
//! A node that doesn't serve the dashboard has no page of its own that
//! changes the worker, so it doesn't trust its own origin at all
//! ([`ControlOrigins::with_own_pages`]); and while worker control is on, the
//! node serves no page that loads scripts from elsewhere (F-console fix
//! round, `super::CDN_PAGES`).

use std::sync::Arc;

/// The header a request to change the worker must carry.
pub const WORKER_CONTROL_HEADER: &str = "x-mina-worker-control";

/// The path whose requests the origin rules apply to.
pub const WORKER_CONTROL_PATH: &str = "/snarker/control";

/// The origins that may change the worker besides this machine's.
#[derive(Clone, Debug, Default)]
pub struct ControlOrigins {
    listed: Arc<[String]>,
    /// The node serves its dashboard, whose page at an IP address is its own
    /// ([`same_origin`]). Off by default.
    own_pages: bool,
}

impl ControlOrigins {
    /// `origins` as `scheme://host[:port]`, like `https://dash.example.org`.
    pub fn new(origins: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let origins = origins
            .into_iter()
            .map(|origin| {
                let normalized = normalize(&origin);
                if parse(&normalized).is_some() {
                    Ok(normalized)
                } else {
                    Err(format!("not an origin (scheme://host[:port]): {origin}"))
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            listed: origins.into(),
            own_pages: false,
        })
    }

    /// Trust the node's own page at an IP address or `localhost`
    /// ([`same_origin`]) only when `serving_dashboard`: the router sets it
    /// from `--no-dashboard`, `--public-graphql` and the `dashboard` feature.
    pub fn with_own_pages(mut self, serving_dashboard: bool) -> Self {
        self.own_pages = serving_dashboard;
        self
    }

    /// Whether a page of `origin` may change the worker.
    pub fn allows(&self, origin: &str) -> bool {
        let origin = normalize(origin);
        match parse(&origin) {
            Some(host) => is_loopback(host) || self.listed.iter().any(|allowed| *allowed == origin),
            None => false,
        }
    }

    /// Whether a page of `origin` may change the worker with a request whose
    /// `Host` header is `host`: [`Self::allows`], or, when the node serves
    /// its dashboard, the node's own page ([`same_origin`]).
    pub fn allows_request(&self, origin: &str, host: Option<&str>) -> bool {
        self.allows(origin)
            || (self.own_pages && host.is_some_and(|host| same_origin(origin, host)))
    }
}

impl ControlOrigins {
    /// Whether `host`, a `Host` header, is the host of a listed origin.
    fn lists_host(&self, host: &str) -> bool {
        let host = normalize(host);
        self.listed.iter().any(|origin| {
            origin
                .strip_prefix("http://")
                .or_else(|| origin.strip_prefix("https://"))
                == Some(host.as_str())
        })
    }
}

/// The pages that may read the operator HTTP API from a browser (CORS):
/// this machine's (`localhost`, `127.0.0.1` or `[::1]`, any port), those the
/// operator lists with `--http-cors-origin` or `--worker-control-origin`, or
/// every page with `--http-cors-origin '*'`. The public API
/// (`--public-graphql`) and WebRTC signalling answer every page.
#[derive(Clone, Debug, Default)]
pub struct ReadOrigins {
    any: bool,
    listed: ControlOrigins,
}

impl ReadOrigins {
    /// `origins` as `scheme://host[:port]`, or `*` for every page.
    pub fn new(origins: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let (any, listed): (Vec<_>, Vec<_>) =
            origins.into_iter().partition(|origin| origin.trim() == "*");
        Ok(Self {
            any: !any.is_empty(),
            listed: ControlOrigins::new(listed)?,
        })
    }

    /// Every page.
    pub fn any() -> Self {
        Self {
            any: true,
            listed: Default::default(),
        }
    }

    /// Whether a page of `origin` may read the API, `control` being the
    /// origins that may change the worker.
    pub fn allows(&self, origin: &str, control: &ControlOrigins) -> bool {
        self.any || self.listed.allows(origin) || control.allows(origin)
    }

    /// Whether a browser's request with `Host: host` may reach the API: the
    /// host is an IP address or `localhost`, or that of a listed origin. A
    /// page on another site can make its own name resolve to this node (DNS
    /// rebinding) and read the API as if it were the node's page; refusing
    /// names nobody listed stops it.
    pub fn allows_host(&self, host: &str, control: &ControlOrigins) -> bool {
        self.any
            || is_address(&host.trim().to_ascii_lowercase())
            || self.listed.lists_host(host)
            || control.lists_host(host)
    }
}

/// Whether `origin` is the node's own, for a request with `Host: host`: the
/// page was opened at `http://` + `host`, and `host` is an IP address (v4,
/// or v6 in brackets) or `localhost`, with an optional port. The dashboard
/// opened over a LAN or VPN address is one; `http://192.0.2.44:3600` is a
/// documentation example. A name in `Host` is never enough: DNS rebinding
/// gives an attacker's page a name that resolves to this node, so
/// `evil.example:3600` would look
/// the same; such a name must be listed with `--worker-control-origin`.
pub fn same_origin(origin: &str, host: &str) -> bool {
    let host = host.trim().to_ascii_lowercase();
    normalize(origin)
        .strip_prefix("http://")
        .is_some_and(|rest| rest == host)
        && is_address(&host)
}

/// `host` is `<IPv4>`, `[<IPv6>]` or `localhost`, then `:<port>` or nothing.
fn is_address(host: &str) -> bool {
    let (name, port) = if let Some(rest) = host.strip_prefix('[') {
        let Some((inner, port)) = rest.split_once(']') else {
            return false;
        };
        if inner.parse::<std::net::Ipv6Addr>().is_err() {
            return false;
        }
        ("[]", port)
    } else {
        match host.split_once(':') {
            Some((name, _)) => (name, &host[name.len()..]),
            None => (host, ""),
        }
    };
    let port_ok = port.is_empty()
        || port.strip_prefix(':').is_some_and(|digits| {
            !digits.is_empty() && digits.len() <= 5 && digits.bytes().all(|b| b.is_ascii_digit())
        });
    let name_ok = name == "[]" || name == "localhost" || name.parse::<std::net::Ipv4Addr>().is_ok();
    port_ok && name_ok
}

fn normalize(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// The host of `scheme://host[:port]`, with http or https and nothing after.
fn parse(origin: &str) -> Option<&str> {
    let rest = origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))?;
    let (host, port) = if rest.starts_with('[') {
        let end = rest.find(']')?;
        (&rest[..=end], &rest[end + 1..])
    } else {
        match rest.rsplit_once(':') {
            Some((host, _)) => (host, rest.get(host.len()..).unwrap_or_default()),
            None => (rest, ""),
        }
    };
    let port_ok = port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    let host_ok = !host.is_empty()
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-.[]:".contains(&b));
    (port_ok && host_ok).then_some(host)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_of_this_machine_and_listed_origins_are_allowed() {
        let origins = ControlOrigins::new(["https://Dash.Example.org/".to_owned()]).unwrap();
        for origin in [
            "http://localhost",
            "http://localhost:4200",
            "http://127.0.0.1:4260",
            "https://127.0.0.1",
            "http://[::1]:4200",
            "https://dash.example.org",
            "HTTPS://DASH.EXAMPLE.ORG",
        ] {
            assert!(origins.allows(origin), "{origin}");
        }
        for origin in [
            "http://evil.example",
            "http://localhost.evil.example",
            "http://127.0.0.1.evil.example",
            "http://evil.example:127",
            "https://dash.example.org:8443",
            "http://dash.example.org",
            "http://192.168.1.5:4200",
            "null",
            "file://",
            "http://localhost/path",
            "http://localhost:",
            "http://localhost:42x",
            "",
        ] {
            assert!(!origins.allows(origin), "{origin}");
        }
        assert!(!ControlOrigins::default().allows("https://dash.example.org"));
    }

    /// F-console: the node's own dashboard, opened at an IP address or
    /// `localhost`, is same-origin; a name never is (DNS rebinding).
    #[test]
    fn the_nodes_own_page_at_an_ip_address_is_same_origin() {
        let none = ControlOrigins::default().with_own_pages(true);
        let no_dashboard = ControlOrigins::default();
        for (origin, host) in [
            // Documentation and LAN addresses exercise the same IP syntax rule.
            ("http://192.0.2.44:3700", "192.0.2.44:3700"),
            ("http://192.168.1.5:3000", "192.168.1.5:3000"),
            ("HTTP://192.0.2.44:3700/", "192.0.2.44:3700"),
            ("http://192.0.2.44", "192.0.2.44"),
            (
                "http://[fd7a:115c:a1e0::1]:3700",
                "[fd7a:115c:a1e0::1]:3700",
            ),
            ("http://[::1]:3700", "[::1]:3700"),
            ("http://localhost:3700", "localhost:3700"),
            ("http://localhost:3700", "LOCALHOST:3700"),
        ] {
            assert!(same_origin(origin, host), "{origin} {host}");
            assert!(none.allows_request(origin, Some(host)), "{origin} {host}");
            // Without the dashboard only the loopback rule is left.
            assert_eq!(
                no_dashboard.allows_request(origin, Some(host)),
                origin.contains("localhost") || origin.contains("[::1]"),
                "{origin} {host}"
            );
        }
        for (origin, host) in [
            // DNS rebinding: an attacker's name resolving to this node.
            ("http://evil.example:3700", "evil.example:3700"),
            (
                "http://rebind.192.0.2.44.nip.io:3700",
                "rebind.192.0.2.44.nip.io:3700",
            ),
            (
                "http://node.tailnet.ts.net:3700",
                "node.tailnet.ts.net:3700",
            ),
            // Another port or address of the same machine is another site.
            ("http://192.0.2.44:4282", "192.0.2.44:3700"),
            ("http://192.0.2.44", "192.0.2.44:80"),
            ("http://192.0.2.45:3700", "192.0.2.44:3700"),
            // Only http://, as the node serves.
            ("https://192.0.2.44:3700", "192.0.2.44:3700"),
            // Not addresses: odd IPv4 forms, zones, bad ports.
            ("http://192.0.2.44.:3700", "192.0.2.44.:3700"),
            ("http://0x7f.0.0.1:3700", "0x7f.0.0.1:3700"),
            ("http://192.0.2:3700", "192.0.2:3700"),
            ("http://[fe80::1%25eth0]:3700", "[fe80::1%25eth0]:3700"),
            ("http://192.0.2.44:", "192.0.2.44:"),
            ("http://192.0.2.44:37a0", "192.0.2.44:37a0"),
            ("http://192.0.2.44:3700:1", "192.0.2.44:3700:1"),
            ("null", "192.0.2.44:3700"),
            ("", ""),
        ] {
            assert!(!same_origin(origin, host), "{origin} {host}");
            assert!(!none.allows_request(origin, Some(host)), "{origin} {host}");
        }
        // Without a Host header only the listed and loopback rules apply.
        assert!(!none.allows_request("http://192.0.2.44:3700", None));
        assert!(none.allows_request("http://127.0.0.1:4282", None));
        // A name is allowed when listed, whatever the Host.
        let listed = ControlOrigins::new(["http://node.tailnet.ts.net:3700".to_owned()]).unwrap();
        assert!(listed.allows_request(
            "http://node.tailnet.ts.net:3700",
            Some("node.tailnet.ts.net:3700")
        ));
    }

    #[test]
    fn listed_origins_must_be_origins() {
        for bad in [
            "dash.example.org",
            "https://dash.example.org/path",
            "ftp://x",
        ] {
            assert!(ControlOrigins::new([bad.to_owned()]).is_err(), "{bad}");
        }
    }
}
