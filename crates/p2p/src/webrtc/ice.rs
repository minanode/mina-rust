//! ICE settings of a node's WebRTC connections (W1): the STUN and TURN
//! servers its peer connections use and, for a native node, the UDP ports
//! and the public addresses of its own candidates.
//!
//! A relay on a host with a public address needs neither STUN nor TURN for
//! itself: its host candidates already carry that address. It needs a known
//! UDP port range, so that a firewall can let browsers reach it, and, on a
//! cloud host whose interface has a private address behind a 1:1 NAT, the
//! public address in its candidates. A web node whose network drops UDP
//! needs a TURN server it can reach over TCP or TLS.

use std::{fmt, net::IpAddr, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// One ICE server, in the shape of the browser's `RTCIceServer`
/// (<https://www.w3.org/TR/webrtc/#rtciceserver-dictionary>), so that the
/// same JSON configures a native node and a web node:
///
/// ```json
/// [
///   { "urls": "stun:turn.example.org:3478" },
///   { "urls": ["turn:turn.example.org:3478?transport=udp",
///              "turns:turn.example.org:5349?transport=tcp"],
///     "username": "webnode", "credential": "..." }
/// ]
/// ```
///
/// `Debug` doesn't show the credential.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct IceServer {
    /// `stun:`, `stuns:`, `turn:` or `turns:` URLs; one string or a list.
    #[serde(deserialize_with = "one_or_many")]
    pub urls: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl fmt::Debug for IceServer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IceServer")
            .field("urls", &self.urls)
            .field("username", &self.username)
            .field("credential", &self.credential.as_ref().map(|_| "<hidden>"))
            .finish()
    }
}

fn one_or_many<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(url) => vec![url],
        OneOrMany::Many(urls) => urls,
    })
}

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum IceServersError {
    #[error("ICE servers are not valid JSON: {0}")]
    Json(String),
    #[error("an ICE server has no URLs")]
    NoUrls,
    #[error("ICE server URL `{0}` is not a stun:, stuns:, turn: or turns: URL")]
    Scheme(String),
    #[error("TURN server `{0}` needs a username and a credential")]
    TurnCredentials(String),
}

impl IceServer {
    /// The servers a node uses unless it is given others: the STUN servers
    /// the node has always used.
    pub fn defaults() -> Vec<Self> {
        let stun = |urls: &[&str], username: Option<&str>, credential: Option<&str>| Self {
            urls: urls.iter().map(|url| url.to_string()).collect(),
            username: username.map(str::to_owned),
            credential: credential.map(str::to_owned),
        };
        vec![
            stun(&["stun:65.109.110.75:3478"], Some("mina"), Some("webrtc")),
            stun(&["stun:176.9.147.28:3478"], None, None),
            stun(
                &[
                    "stun:stun.l.google.com:19302",
                    "stun:stun1.l.google.com:19302",
                    "stun:stun2.l.google.com:19302",
                    "stun:stun3.l.google.com:19302",
                    "stun:stun4.l.google.com:19302",
                ],
                None,
                None,
            ),
        ]
    }

    /// Refuses what a browser's `RTCPeerConnection` would refuse: no URLs,
    /// another scheme, or a TURN URL without a username and credential.
    pub fn validate(&self) -> Result<(), IceServersError> {
        if self.urls.is_empty() {
            return Err(IceServersError::NoUrls);
        }
        for url in &self.urls {
            let (scheme, _) = url
                .split_once(':')
                .ok_or_else(|| IceServersError::Scheme(url.clone()))?;
            match scheme {
                "stun" | "stuns" => {}
                "turn" | "turns" => {
                    if self.username.is_none() || self.credential.is_none() {
                        return Err(IceServersError::TurnCredentials(url.clone()));
                    }
                }
                _ => return Err(IceServersError::Scheme(url.clone())),
            }
        }
        Ok(())
    }

    /// Checks each server of a list.
    pub fn validate_list(servers: &[Self]) -> Result<(), IceServersError> {
        servers.iter().try_for_each(Self::validate)
    }

    /// Reads a JSON list of servers and checks it. An empty list is valid:
    /// a node with a public address needs no server.
    pub fn parse_list(json: &str) -> Result<Vec<Self>, IceServersError> {
        let servers: Vec<Self> =
            serde_json::from_str(json).map_err(|err| IceServersError::Json(err.to_string()))?;
        Self::validate_list(&servers)?;
        Ok(servers)
    }
}

/// The UDP ports of a native node's ICE candidates, both ends included, as
/// `MIN-MAX`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UdpPortRange {
    pub min: u16,
    pub max: u16,
}

impl FromStr for UdpPortRange {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (min, max) = s
            .split_once('-')
            .ok_or_else(|| format!("expected MIN-MAX, got `{s}`"))?;
        let port = |v: &str| {
            v.trim()
                .parse::<u16>()
                .map_err(|err| format!("bad port `{v}`: {err}"))
        };
        let (min, max) = (port(min)?, port(max)?);
        // webrtc-rs walks the range with a u16 that would wrap past 65535.
        if min == 0 || max == u16::MAX || min > max {
            return Err(format!(
                "expected 1 <= MIN <= MAX <= 65534, got `{min}-{max}`"
            ));
        }
        Ok(Self { min, max })
    }
}

impl fmt::Display for UdpPortRange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}-{}", self.min, self.max)
    }
}

/// A node's WebRTC settings. The default is what a node did before they
/// could be set: [`IceServer::defaults`], any UDP port, interface addresses.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WebrtcConfig {
    /// `None` for [`IceServer::defaults`]; `Some(vec![])` for none.
    pub ice_servers: Option<Vec<IceServer>>,
    /// Native nodes only: bind candidates to ports in this range.
    pub udp_ports: Option<UdpPortRange>,
    /// Native nodes only: the public addresses of a 1:1 NAT, put in host
    /// candidates in place of the interface addresses.
    pub nat_ips: Vec<IpAddr>,
}

impl WebrtcConfig {
    pub fn ice_servers(&self) -> Vec<IceServer> {
        self.ice_servers.clone().unwrap_or_else(IceServer::defaults)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_browser_shaped_list() {
        let servers = IceServer::parse_list(
            r#"[
                {"urls": "stun:turn.example.org:3478"},
                {"urls": ["turn:turn.example.org:3478?transport=udp",
                          "turns:turn.example.org:5349?transport=tcp"],
                 "username": "webnode", "credential": "secret"}
            ]"#,
        )
        .unwrap();
        assert_eq!(servers.len(), 2);
        assert_eq!(servers[0].urls, ["stun:turn.example.org:3478"]);
        assert_eq!(servers[0].username, None);
        assert_eq!(servers[1].urls.len(), 2);
        assert_eq!(servers[1].username.as_deref(), Some("webnode"));
    }

    #[test]
    fn empty_list_is_valid() {
        assert_eq!(IceServer::parse_list("[]").unwrap(), vec![]);
    }

    #[test]
    fn refuses_turn_without_credentials() {
        let err = IceServer::parse_list(r#"[{"urls": "turn:t.example.org:3478"}]"#).unwrap_err();
        assert_eq!(
            err,
            IceServersError::TurnCredentials("turn:t.example.org:3478".into())
        );
        let err =
            IceServer::parse_list(r#"[{"urls": "turns:t.example.org:5349", "username": "u"}]"#)
                .unwrap_err();
        assert!(matches!(err, IceServersError::TurnCredentials(_)));
    }

    #[test]
    fn refuses_other_schemes_and_empty_urls() {
        let err = IceServer::parse_list(r#"[{"urls": "https://t.example.org"}]"#).unwrap_err();
        assert!(matches!(err, IceServersError::Scheme(_)));
        let err = IceServer::parse_list(r#"[{"urls": "t.example.org"}]"#).unwrap_err();
        assert!(matches!(err, IceServersError::Scheme(_)));
        let err = IceServer::parse_list(r#"[{"urls": []}]"#).unwrap_err();
        assert_eq!(err, IceServersError::NoUrls);
    }

    #[test]
    fn refuses_unknown_fields() {
        // A misspelt field would otherwise drop the credential silently.
        let err = IceServer::parse_list(
            r#"[{"urls": "turn:t.example.org", "usename": "u", "credential": "c"}]"#,
        )
        .unwrap_err();
        assert!(matches!(err, IceServersError::Json(_)));
    }

    #[test]
    fn serializes_without_nulls_for_the_browser() {
        let json = serde_json::to_string(&IceServer::defaults()[1]).unwrap();
        assert_eq!(json, r#"{"urls":["stun:176.9.147.28:3478"]}"#);
    }

    #[test]
    fn debug_hides_the_credential() {
        let server = IceServer {
            urls: vec!["turn:t.example.org".into()],
            username: Some("u".into()),
            credential: Some("hunter2".into()),
        };
        let shown = format!("{server:?}");
        assert!(!shown.contains("hunter2"), "{shown}");
        assert!(shown.contains("<hidden>"), "{shown}");
    }

    #[test]
    fn defaults_are_valid_and_used_when_unset() {
        IceServer::validate_list(&IceServer::defaults()).unwrap();
        assert_eq!(WebrtcConfig::default().ice_servers(), IceServer::defaults());
        let none = WebrtcConfig {
            ice_servers: Some(vec![]),
            ..Default::default()
        };
        assert!(none.ice_servers().is_empty());
    }

    #[test]
    fn udp_port_range() {
        assert_eq!(
            "40000-40999".parse::<UdpPortRange>().unwrap(),
            UdpPortRange {
                min: 40000,
                max: 40999
            }
        );
        assert_eq!(
            "5000-5000".parse::<UdpPortRange>().unwrap().to_string(),
            "5000-5000"
        );
        for bad in [
            "",
            "40000",
            "40999-40000",
            "0-10",
            "1-65535",
            "a-b",
            "1-70000",
        ] {
            assert!(bad.parse::<UdpPortRange>().is_err(), "{bad}");
        }
    }
}
