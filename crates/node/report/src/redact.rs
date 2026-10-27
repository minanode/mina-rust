//! What a bug report must never carry: keys, passwords, tokens, key files and
//! their paths, or the values of secret environment variables; and, unless the
//! tester asks (`--include-network-ids`), IP addresses and peer ids.
//!
//! Every text that goes into a report passes [`Redactor::text`], and the
//! command line the node was started with passes [`redact_argv`] before it is
//! even written to the work directory.
//!
//! The rules are structural rather than a list of known secrets: text is
//! first unescaped (JSON `\/` and `\u` escapes, percent-encoding), a URL keeps
//! only its scheme and host, any long high-entropy run (hex, base64, base58)
//! goes unless it is a public Mina identifier of an exact format, and so does
//! the value of anything named like a key, token, secret or password.

use std::{
    collections::BTreeMap,
    net::{Ipv4Addr, Ipv6Addr},
    path::Path,
    str::FromStr,
    sync::LazyLock,
};

use mina_node::{account::AccountSecretKey, p2p::identity::SecretKey};
use mina_node_account::AccountPublicKey;
use mina_p2p_messages::v2::{EpochSeed, LedgerHash, StateHash, TransactionHash};
use regex::{Captures, Regex};

pub const REDACTED: &str = "<redacted>";
pub const KEY_FILE: &str = "<key-file>";
const KEY_FILE_PATH: &str = "<key-file-path>";
pub const PATH_VALUE: &str = "<path>";
const URL_VALUE: &str = "<url>";
const PRIVATE_KEY: &str = "<private-key>";
const P2P_SECRET_KEY: &str = "<p2p-secret-key>";
const PEER_ID: &str = "<peer-id>";
const IP: &str = "<ip>";
const HOST: &str = "<host>";

/// Secret values shorter than this are not scrubbed by value: a one-letter
/// password would otherwise blank every such letter. The patterns still apply.
const MIN_SECRET_LEN: usize = 4;

/// A variable that is a secret only by its name's look is scrubbed by value
/// from this length: shorter values (`user`, `tty`) are ordinary words.
const MIN_NAMED_SECRET_LEN: usize = 8;

/// Longer base58 runs are not taken for keys or peer ids.
const MAX_KEY_LEN: usize = 120;

/// Environment variables the node or its tools read secrets from. A renamed
/// variable keeps its old name here, so both are scrubbed alike.
const SECRET_ENV: &[&str] = &[
    "MINA_PRIVKEY_PASS",
    "MINA_LIBP2P_PASS",
    "MINA_P2P_SEC_KEY",
    "MINA_SEC_KEY",
    "MINA_ENC_KEY",
    "MINA_PRODUCER_KEY",
    "PRODUCER_KEY",
    "GCP_CREDENTIALS_JSON",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
];

/// Any variable whose name contains one of these, in any case, is a secret:
/// its value goes wherever `NAME=value` or `name: value` appears.
const SECRET_NAME_PARTS: &[&str] = &[
    "KEY", "TOKEN", "SECRET", "PASS", "PWD", "JWT", "AUTH", "CRED", "COOKIE", "SESSION",
];

/// Variables those parts match that hold no secret: the current directory,
/// desktop session plumbing. Their values are not scrubbed wherever they
/// appear (`NAME=value` text still is).
const NOT_SECRET_ENV: &[&str] = &[
    "PWD",
    "OLDPWD",
    "SSH_AUTH_SOCK",
    "XAUTHORITY",
    "DESKTOP_SESSION",
    "DBUS_SESSION_BUS_ADDRESS",
    "GNOME_KEYRING_CONTROL",
    "XDG_SESSION_CLASS",
    "XDG_SESSION_DESKTOP",
    "XDG_SESSION_ID",
    "XDG_SESSION_TYPE",
];

/// Whether the environment variable `name` holds a secret.
pub fn is_secret_env_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    SECRET_ENV.contains(&upper.as_str()) || SECRET_NAME_PARTS.iter().any(|p| upper.contains(p))
}

/// Whether this process's variable `name`'s `value` is scrubbed wherever it
/// appears: the known secret variables', and long values of the others
/// named like secrets.
pub fn scrub_env_value(name: &str, value: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    if SECRET_ENV.contains(&upper.as_str()) {
        return true;
    }
    is_secret_env_name(name)
        && !NOT_SECRET_ENV.contains(&upper.as_str())
        && value.trim().chars().count() >= MIN_NAMED_SECRET_LEN
}

macro_rules! regex {
    ($name:ident, $re:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($re).expect("valid regex"));
    };
}

regex!(ANSI, r"\x1b\[[0-9;?]*[ -/]*[@-~]");
// Escapes that hide what a value is: JSON's `\/` and `\u002F`, and
// percent-encoding. Only printable ASCII is decoded.
regex!(ESCAPED_SLASH, r"\\+/");
regex!(UNICODE_ESCAPE, r"\\u([0-9a-fA-F]{4})");
regex!(PERCENT_ESCAPE, r"%([0-9a-fA-F]{2})");
// The fields of an encrypted key file (Mina's secret box JSON).
regex!(
    KEY_FILE_FIELD,
    r#""(ciphertext|pwsalt|nonce|pwdiff|box_primitive|pw_primitive)"\s*:\s*("[^"]*"|\[[^\]]*\]|-?[0-9]+)"#
);
// `NAME=value`, `name: value`, `"name": "value"` for names with a part of
// [`SECRET_NAME_PARTS`].
regex!(
    SECRET_ASSIGNMENT,
    r#"(?i)\b([a-z0-9_-]*(?:key|token|secret|pass|pwd|jwt|auth|cred|cookie|session)[a-z0-9_-]*)("?\s*[=:]\s*)("[^"]*"|'[^']*'|[^\s"',;<][^\s"',;]*)"#
);
// A URL: anything after its host may carry a credential.
regex!(URL, r#"\b[A-Za-z][A-Za-z0-9+.-]*://[^\s"'`]+"#);
// A JSON web token: three base64url parts, the first two JSON.
regex!(
    JWT,
    r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}(?:\.[A-Za-z0-9_-]*)?"
);
// Tokens with a known prefix, however short: Stripe, OpenAI and Anthropic,
// GitHub, Slack, AWS and Google keys.
regex!(
    KNOWN_TOKEN,
    r"\b(?:(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{4,}|sk-[A-Za-z0-9_-]{16,}|gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|xox[abposr]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{35})"
);
// A run that could be an opaque token: hex, base64 (with padding),
// base64url or base58.
regex!(TOKEN, r"[A-Za-z0-9+/_-]{16,}={0,2}");
// An HTTP authorization header, whatever its scheme: the rest of the line.
regex!(
    AUTH_HEADER,
    r#"(?i)\b((?:proxy-)?authorization)("?\s*[:=]\s*)("[^"]*"|'[^']*'|[^\r\n]+)"#
);
// A bearer or basic credential outside a header.
regex!(BEARER, r"(?i)\b(bearer|basic)\s+[A-Za-z0-9._~+/=-]{6,}");
// Command-line options that take a secret or a key file, quoted in free text,
// aliases included (`--block-producer-key` is `--producer-key`).
regex!(
    SECRET_FLAG,
    r#"(--(?:p2p-secret-key|block-producer-key|producer-key|producer-key-password|libp2p-keypair|libp2p-password|secret-key|rng-seed|password|privkey-path|privkey))(?:=|\s+)("[^"]*"|'[^']*'|[^\s"'<-][^\s"']*)"#
);
// A libp2p private key as protobuf, base64 or hex.
regex!(
    LIBP2P_PROTOBUF_KEY,
    r"\bCAES[A-Za-z0-9+/]{40,}={0,2}|\b08011240[0-9a-fA-F]{64,}\b"
);
regex!(ICE_PASSWORD, r"(?i)(ice-pwd:)\S+");
regex!(BASE58, r"\b[1-9A-HJ-NP-Za-km-z]{40,}\b");
regex!(PATH, r"[A-Za-z0-9._~-]*(?:/[A-Za-z0-9._@+%=-]+)+/?");
// Another user's home: `/home/<name>` where a path starts (also in
// Markdown's backticks), not a directory called `home` inside one.
regex!(OTHER_HOME, r#"(^|[\s"'=(\[,:`])/(home|Users)/[^/\s<>"'`]+"#);
regex!(
    MULTIADDR_HOST,
    r#"/(ip4|ip6|dns|dns4|dns6|dnsaddr)/([^/\s"',<>]+)"#
);
regex!(MULTIADDR_PEER, r"/p2p/([1-9A-HJ-NP-Za-km-z]{20,})");
regex!(IPV4, r"\b[0-9]{1,3}(?:\.[0-9]{1,3}){3}\b");
regex!(
    IPV6_CANDIDATE,
    r"[0-9A-Fa-f:.]*:[0-9A-Fa-f:.]*:[0-9A-Fa-f:.]*"
);

/// Path components that mark a key file or a directory of them.
const KEY_PATH_PARTS: &[&str] = &[
    "key",
    "secret",
    "wallet",
    "priv",
    "passw",
    "credential",
    ".pem",
];

/// Scrubs secrets, and by default network ids, from text.
pub struct Redactor {
    include_network_ids: bool,
    /// Exact values never to show, longest first.
    secrets: Vec<String>,
    /// Exact values that look opaque but are public: chain ids, commits.
    allowed: Vec<String>,
    /// This user's home directory, shown as `~`.
    home: Option<String>,
}

impl Redactor {
    /// A redactor that also scrubs the values of this process's secret
    /// environment variables wherever they appear. The values are never
    /// shown or stored.
    pub fn from_env(include_network_ids: bool) -> Self {
        let vars = std::env::vars_os().filter_map(|(name, value)| {
            Some((name.into_string().ok()?, value.into_string().ok()?))
        });
        Self::from_vars(include_network_ids, vars, std::env::var("HOME").ok())
    }

    /// As `from_env`, from the given variables.
    fn from_vars(
        include_network_ids: bool,
        vars: impl IntoIterator<Item = (String, String)>,
        home: Option<String>,
    ) -> Self {
        let secrets = vars
            .into_iter()
            .filter_map(|(name, value)| scrub_env_value(&name, &value).then_some(value));
        Self::new(include_network_ids, secrets, home)
    }

    pub fn new(
        include_network_ids: bool,
        secrets: impl IntoIterator<Item = String>,
        home: Option<String>,
    ) -> Self {
        let mut this = Self {
            include_network_ids,
            secrets: Vec::new(),
            allowed: Vec::new(),
            home: home
                .map(|home| home.trim_end_matches('/').to_owned())
                .filter(|home| home.len() > 1),
        };
        for secret in secrets {
            this.add_secret(&secret);
        }
        for chain in [mina_core::DEVNET_CHAIN_ID, mina_core::MAINNET_CHAIN_ID] {
            this.allow(&chain.to_hex());
        }
        this.allow(&mina_node::BuildEnv::get().git.commit_hash);
        this
    }

    /// Scrubs `value` wherever it appears from now on.
    pub fn add_secret(&mut self, value: &str) {
        let value = value.trim();
        if value.chars().count() >= MIN_SECRET_LEN && !self.secrets.iter().any(|s| s == value) {
            self.secrets.push(value.to_owned());
            self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        }
    }

    pub fn include_network_ids(&self) -> bool {
        self.include_network_ids
    }

    /// A path for terminal progress: abbreviate this user's home on a path
    /// component boundary. Report bodies still pass through full redaction.
    pub fn display_path(&self, path: &Path) -> String {
        if let Some(relative) = self
            .home
            .as_deref()
            .and_then(|home| path.strip_prefix(home).ok())
        {
            return if relative.as_os_str().is_empty() {
                "~".to_owned()
            } else {
                Path::new("~").join(relative).display().to_string()
            };
        }
        path.display().to_string()
    }

    /// Scrubs `other`'s secrets too, from now on.
    pub fn add_secrets_from(&mut self, other: &Redactor) {
        for secret in &other.secrets {
            self.add_secret(secret);
        }
    }

    /// The text with every secret, and by default every network id, replaced
    /// by a placeholder such as `<redacted>` or `<ip>`.
    pub fn text(&self, input: &str) -> String {
        let mut text = unescape(&ANSI.replace_all(input, ""));
        text = URL
            .replace_all(&text, |c: &Captures| url_origin(&c[0]))
            .into_owned();
        for secret in &self.secrets {
            if text.contains(secret.as_str()) {
                text = text.replace(secret.as_str(), REDACTED);
            }
        }
        text = AUTH_HEADER
            .replace_all(&text, |c: &Captures| {
                format!("{}{}{REDACTED}", &c[1], &c[2])
            })
            .into_owned();
        text = BEARER
            .replace_all(&text, |c: &Captures| format!("{} {REDACTED}", &c[1]))
            .into_owned();
        text = JWT.replace_all(&text, REDACTED).into_owned();
        text = KNOWN_TOKEN.replace_all(&text, REDACTED).into_owned();
        text = KEY_FILE_FIELD
            .replace_all(&text, |c: &Captures| {
                format!("\"{}\": \"{REDACTED}\"", &c[1])
            })
            .into_owned();
        text = SECRET_FLAG
            .replace_all(&text, |c: &Captures| format!("{} {REDACTED}", &c[1]))
            .into_owned();
        text = SECRET_ASSIGNMENT
            .replace_all(&text, |c: &Captures| {
                format!("{}{}{REDACTED}", &c[1], &c[2])
            })
            .into_owned();
        text = LIBP2P_PROTOBUF_KEY
            .replace_all(&text, P2P_SECRET_KEY)
            .into_owned();
        text = ICE_PASSWORD
            .replace_all(&text, |c: &Captures| format!("{}{REDACTED}", &c[1]))
            .into_owned();
        text = BASE58
            .replace_all(&text, |c: &Captures| self.base58(&c[0]))
            .into_owned();
        text = TOKEN
            .replace_all(&text, |c: &Captures| {
                if self.opaque(&c[0]) {
                    REDACTED.to_owned()
                } else {
                    c[0].to_owned()
                }
            })
            .into_owned();
        text = self.paths(&text);
        if !self.include_network_ids {
            text = network_ids(&text);
        }
        text
    }

    fn base58(&self, token: &str) -> String {
        // Keys and peer ids are 50 to 55 characters; decoding much longer
        // runs costs quadratic time for nothing.
        if token.len() > MAX_KEY_LEN {
            return token.to_owned();
        }
        if token.starts_with("EK") || AccountSecretKey::from_str(token).is_ok() {
            PRIVATE_KEY.to_owned()
        } else if SecretKey::from_str(token).is_ok() {
            P2P_SECRET_KEY.to_owned()
        } else if !self.include_network_ids && is_peer_id(token) {
            PEER_ID.to_owned()
        } else {
            token.to_owned()
        }
    }

    /// Whether `value` (a chain id, a commit) stays wherever it appears. Only
    /// hex values of 32 or more characters are taken: shorter ones never go.
    pub fn allow(&mut self, value: &str) {
        let value = value.trim();
        if value.len() >= 32
            && value.bytes().all(|b| b.is_ascii_hexdigit())
            && !self.allowed.iter().any(|a| a == value)
        {
            self.allowed.push(value.to_owned());
        }
    }

    /// A public Mina identifier useful in a bug report, checked by its exact
    /// format: a B62 public key, a state, ledger, epoch seed or transaction
    /// hash (each parsed, with its version byte and checksum), one of the
    /// allowed values (chain ids, build commits), or with
    /// `--include-network-ids` a peer id.
    fn is_public_id(&self, token: &str) -> bool {
        if self.allowed.iter().any(|allowed| allowed == token) {
            return true;
        }
        if !(40..=60).contains(&token.len()) {
            return false;
        }
        (token.starts_with("B62q") && AccountPublicKey::from_str(token).is_ok())
            || (token.starts_with("3N") && StateHash::from_str(token).is_ok())
            || (token.starts_with('j') && LedgerHash::from_str(token).is_ok())
            || (token.starts_with('2') && EpochSeed::from_str(token).is_ok())
            || TransactionHash::from_str(token).is_ok()
            || (self.include_network_ids && is_peer_id(token))
    }

    /// Whether a [`TOKEN`] run is, or holds, an opaque token: judged as a
    /// whole and part by part (split at `/`, `-` and `_`, which paths and
    /// names use), public identifiers aside.
    fn opaque(&self, token: &str) -> bool {
        if self.is_public_id(token) {
            return false;
        }
        let body = token.trim_end_matches('=');
        let padded = body.len() < token.len();
        let mut rest = String::new();
        for part in body.split(['/', '-', '_']) {
            if part.is_empty() || self.is_public_id(part) {
                continue;
            }
            if opaque_part(part) {
                return true;
            }
            rest.push_str(part);
        }
        opaque_part(&rest)
            || (padded
                && rest.len() >= 16
                && rest.bytes().any(|b| b.is_ascii_lowercase())
                && rest.bytes().any(|b| b.is_ascii_uppercase())
                && !word_like(&rest))
    }

    fn paths(&self, text: &str) -> String {
        let text = match &self.home {
            Some(home) => replace_home(text, home),
            None => text.to_owned(),
        };
        let text = OTHER_HOME.replace_all(&text, "$1/$2/<user>");
        PATH.replace_all(&text, |c: &Captures| {
            let path = &c[0];
            if is_key_path(path) {
                KEY_FILE_PATH.to_owned()
            } else {
                path.to_owned()
            }
        })
        .into_owned()
    }
}

/// `home` and paths under it, shown from `~`.
fn replace_home(text: &str, home: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(home) {
        let after = &rest[at + home.len()..];
        let boundary = after.is_empty() || after.starts_with(['/', ' ', '"', '\'', '\n', ':', '`']);
        out.push_str(&rest[..at]);
        out.push_str(if boundary { "~" } else { home });
        rest = after;
    }
    out.push_str(rest);
    out
}

/// `text` with the escapes that could hide a value undone: JSON's `\/` and
/// `\uXXXX`, and percent-encoding, for printable ASCII only.
fn unescape(text: &str) -> String {
    let printable = |hex: &str| {
        u32::from_str_radix(hex, 16)
            .ok()
            .filter(|code| (0x21..=0x7e).contains(code))
            .and_then(char::from_u32)
    };
    let text = UNICODE_ESCAPE.replace_all(text, |c: &Captures| {
        printable(&c[1]).map_or(c[0].to_owned(), String::from)
    });
    let text = ESCAPED_SLASH.replace_all(&text, "/");
    PERCENT_ESCAPE
        .replace_all(&text, |c: &Captures| {
            printable(&c[1]).map_or(c[0].to_owned(), String::from)
        })
        .into_owned()
}

/// A URL's scheme and host (with its port), and `/<redacted>` for anything
/// after them: user, password, path, query and fragment can each carry a
/// credential. `https://seeds.example.test/<redacted>` stays as it is.
fn url_origin(raw: &str) -> String {
    // Punctuation after a URL in prose is not part of it.
    const TRAILING: &[char] = &['.', ',', ';', ':', ')', ']', '}', '!', '>'];
    let Some((scheme, rest)) = raw.split_once("://") else {
        return raw.to_owned();
    };
    let end = rest.find(['/', '?', '#', '\\']).unwrap_or(rest.len());
    let (authority, after) = rest.split_at(end);
    let placeholder = format!("/{REDACTED}");
    let (authority, after, tail) = if after.is_empty() {
        let kept = authority.trim_end_matches(TRAILING);
        (kept, "", &authority[kept.len()..])
    } else if let Some(tail) = after
        .strip_prefix(&placeholder)
        .filter(|tail| tail.chars().all(|c| TRAILING.contains(&c)))
    {
        // Already redacted: a second pass changes nothing.
        (authority, placeholder.as_str(), tail)
    } else {
        let kept = after.trim_end_matches(TRAILING);
        (authority, kept, &after[kept.len()..])
    };
    let host = authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host);
    let host = if !host.is_empty()
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._:[]".contains(c))
    {
        host
    } else {
        HOST
    };
    let after = if after.is_empty() || after == "/" {
        after
    } else {
        placeholder.as_str()
    };
    format!("{scheme}://{host}{after}{tail}")
}

/// Whether one part of a [`TOKEN`] run looks random: long, mixing classes of
/// characters, not made of words. Hex counts from 32 characters (16 bytes),
/// so that short hashes and abbreviated commits stay.
fn opaque_part(part: &str) -> bool {
    let n = part.len();
    if n < 16 {
        return false;
    }
    let (mut lower, mut upper, mut digits) = (0, 0, 0);
    for byte in part.bytes() {
        match byte {
            b'a'..=b'z' => lower += 1,
            b'A'..=b'Z' => upper += 1,
            b'0'..=b'9' => digits += 1,
            _ => {}
        }
    }
    if part.bytes().all(|b| b.is_ascii_hexdigit()) {
        return n >= 32 && digits > 0 && lower + upper > 0;
    }
    if lower + upper == 0 || word_like(part) {
        return false;
    }
    let classes = [lower, upper, digits]
        .iter()
        .filter(|&&count| count > 0)
        .count();
    (n >= 20 && classes == 3 && digits >= 2)
        || (n >= 32 && classes >= 2 && digits >= 3)
        || (n >= 20 && lower == 0 && upper >= 4 && digits >= 2)
}

/// Whether at least half of `part`'s letters sit in runs of four or more
/// lowercase letters (with a capital before them): words, and the words of
/// `CamelCase` and `snake_case` names, not random characters.
fn word_like(part: &str) -> bool {
    let bytes = part.as_bytes();
    let letters = bytes.iter().filter(|b| b.is_ascii_alphabetic()).count();
    let mut covered = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_lowercase() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_lowercase() {
                i += 1;
            }
            if i - start >= 4 {
                covered +=
                    i - start + usize::from(start > 0 && bytes[start - 1].is_ascii_uppercase());
            }
        } else {
            i += 1;
        }
    }
    letters > 0 && covered * 2 >= letters
}

fn is_key_path(path: &str) -> bool {
    // Source files (backtraces, log targets) are not key files, whatever
    // their names.
    if path.ends_with(".rs") || path.contains("://") {
        return false;
    }
    path.split('/').any(|part| {
        let part = part.to_ascii_lowercase();
        KEY_PATH_PARTS.iter().any(|marker| part.contains(marker))
    })
}

fn is_peer_id(token: &str) -> bool {
    token.parse::<mina_node::p2p::PeerId>().is_ok()
        || token.parse::<libp2p_identity::PeerId>().is_ok()
        || token.starts_with("12D3KooW")
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// IP addresses, hosts in multiaddrs and peer ids after `/p2p/`. Loopback
/// and unspecified addresses stay: they say nothing about the tester.
fn network_ids(text: &str) -> String {
    let text = MULTIADDR_HOST.replace_all(text, |c: &Captures| {
        let (kind, host) = (&c[1], &c[2]);
        let keep = match kind {
            "ip4" => host
                .parse::<Ipv4Addr>()
                .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified()),
            "ip6" => host
                .parse::<Ipv6Addr>()
                .is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified()),
            _ => host == "localhost",
        };
        if keep || host.starts_with('<') {
            c[0].to_owned()
        } else if kind.starts_with("ip") {
            format!("/{kind}/{IP}")
        } else {
            format!("/{kind}/{HOST}")
        }
    });
    let text = MULTIADDR_PEER.replace_all(&text, format!("/p2p/{PEER_ID}").as_str());
    let text = IPV4.replace_all(&text, |c: &Captures| match c[0].parse::<Ipv4Addr>() {
        Ok(ip) if !(ip.is_loopback() || ip.is_unspecified()) => IP.to_owned(),
        _ => c[0].to_owned(),
    });
    // IPv6 needs its neighbours checked: `tree::proofs` must stay a path.
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for m in IPV6_CANDIDATE.find_iter(&text) {
        let before = text[..m.start()].chars().next_back();
        let after = text[m.end()..].chars().next();
        if before.is_some_and(is_word_char) || after.is_some_and(is_word_char) {
            continue;
        }
        let candidate = m.as_str().trim_end_matches('.');
        let replace = candidate
            .parse::<Ipv6Addr>()
            .is_ok_and(|ip| !(ip.is_loopback() || ip.is_unspecified()));
        if replace {
            out.push_str(&text[last..m.start()]);
            out.push_str(IP);
            out.push_str(&m.as_str()[candidate.len()..]);
            last = m.end();
        }
    }
    out.push_str(&text[last..]);
    out
}

/// Values of these options are kept when a command line is recorded:
/// numbers, names, peers and public keys (network ids and public keys go
/// when the report is made). Paths and URLs keep less (below). Every other
/// value (keys, passwords, key files, seeds, and options this build doesn't
/// know) is replaced, and so is every positional argument: `mina node` takes
/// its passwords positionally.
const KEPT_VALUE_ARGS: &[&str] = &[
    "network",
    "port",
    "libp2p_port",
    "verbosity",
    "log_file_size_mb",
    "log_file_count",
    "peers",
    "max_peers",
    "run_snarker",
    "snarker_fee",
    "snarker_strategy",
    "coinbase_receiver",
    "block_inject_delay_ms",
    "record",
    "chain_id",
    "checkpoint_interval",
    "libp2p_external_ip",
    "catchup_rpc_peer",
    "webrtc_udp_ports",
    "webrtc_nat_ip",
];

/// Paths: any could be in a directory of keys, so only a harmless file name
/// is kept, as `<path>/name`.
const PATH_ARGS: &[&str] = &[
    "work_dir",
    "log_path",
    "peer_list_file",
    "config",
    "genesis_config",
    "frontier_store_dir",
];

/// URLs: only the scheme, host and port; never the user, password, path,
/// query or fragment.
const URL_ARGS: &[&str] = &[
    "peer_list_url",
    "url",
    "worker_control_origins",
    "http_cors_origins",
];

/// Options whose value is a key file: shown as [`KEY_FILE`].
const KEY_FILE_ARGS: &[&str] = &["producer_key", "libp2p_keypair"];

#[derive(Clone, Copy)]
struct Opt {
    takes_value: bool,
    shown: Shown,
}

#[derive(Clone, Copy)]
enum Shown {
    Value,
    Path,
    PathOnly,
    Url,
    KeyFile,
    Hidden,
}

impl Opt {
    fn of(arg: &clap::Arg) -> Self {
        let id = arg.get_id().as_str();
        Self {
            takes_value: arg.get_action().takes_values(),
            shown: if id == "webrtc_ice_servers" {
                Shown::PathOnly
            } else if KEPT_VALUE_ARGS.contains(&id) {
                Shown::Value
            } else if PATH_ARGS.contains(&id) {
                Shown::Path
            } else if URL_ARGS.contains(&id) {
                Shown::Url
            } else if KEY_FILE_ARGS.contains(&id) {
                Shown::KeyFile
            } else {
                Shown::Hidden
            },
        }
    }

    fn show(self, value: &str) -> String {
        match self.shown {
            Shown::Value => value.to_owned(),
            Shown::Path => shown_path(value),
            Shown::PathOnly => PATH_VALUE.to_owned(),
            Shown::Url => shown_url(value),
            Shown::KeyFile => KEY_FILE.to_owned(),
            Shown::Hidden => REDACTED.to_owned(),
        }
    }
}

/// `<path>/name` when the file name is clearly harmless (short, plain, not
/// naming a key or looking like a token), else `<path>`.
fn shown_path(value: &str) -> String {
    let name = std::path::Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("");
    let plain = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    let token_like = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|run| run.len() >= 20);
    if plain && !token_like && !is_key_path(name) {
        format!("{PATH_VALUE}/{name}")
    } else {
        PATH_VALUE.to_owned()
    }
}

/// Scheme, host and port only, as [`url_origin`]; `<url>` for a value that
/// isn't a URL.
fn shown_url(value: &str) -> String {
    let value = unescape(value);
    if value.contains("://") {
        url_origin(&value)
    } else {
        URL_VALUE.to_owned()
    }
}

#[derive(Default)]
struct Opts {
    long: BTreeMap<String, Opt>,
    short: BTreeMap<char, Opt>,
}

impl Opts {
    fn add(&mut self, command: &clap::Command) {
        for arg in command.get_arguments() {
            let opt = Opt::of(arg);
            for long in arg
                .get_long()
                .into_iter()
                .chain(arg.get_all_aliases().into_iter().flatten())
            {
                self.long.insert(long.to_owned(), opt);
            }
            for short in arg
                .get_short()
                .into_iter()
                .chain(arg.get_all_short_aliases().into_iter().flatten())
            {
                self.short.insert(short, opt);
            }
        }
    }
}

/// The command line `argv` (program name first) with the values of secret
/// options, key file paths and positional arguments replaced, using `cli`'s
/// definition of each option. Network ids are left to [`Redactor::text`].
pub fn redact_argv(argv: &[String], mut cli: clap::Command) -> Vec<String> {
    let Some((program, args)) = argv.split_first() else {
        return Vec::new();
    };
    // Built, every option has its final action and globals are propagated.
    cli.build();
    // The program's name; its path could be anywhere.
    let program = std::path::Path::new(program)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mina");
    let mut out = vec![program.to_owned()];
    let mut command = &cli;
    let mut opts = Opts::default();
    opts.add(command);
    let mut iter = args.iter();
    let mut only_positionals = false;
    while let Some(arg) = iter.next() {
        if only_positionals {
            out.push(REDACTED.to_owned());
        } else if arg == "--" {
            out.push(arg.clone());
            only_positionals = true;
        } else if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            match (opts.long.get(name).copied(), inline) {
                (Some(opt), Some(value)) => out.push(format!("--{name}={}", opt.show(value))),
                (Some(opt), None) if opt.takes_value => {
                    out.push(arg.clone());
                    if let Some(value) = iter.next() {
                        out.push(opt.show(value));
                    }
                }
                (Some(_), None) => out.push(arg.clone()),
                // An option this build doesn't know: its value, if any, is
                // hidden.
                (None, Some(_)) => out.push(format!("--{name}={REDACTED}")),
                (None, None) => out.push(arg.clone()),
            }
        } else if let Some(shorts) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            let mut chars = shorts.chars();
            let first = chars.next().expect("not empty");
            let rest = chars.as_str();
            match opts.short.get(&first).copied() {
                Some(opt) if opt.takes_value && rest.is_empty() => {
                    out.push(arg.clone());
                    if let Some(value) = iter.next() {
                        out.push(opt.show(value));
                    }
                }
                Some(opt) if opt.takes_value => {
                    let value = rest.strip_prefix('=').unwrap_or(rest);
                    out.push(format!("-{first}{}", opt.show(value)));
                }
                Some(_)
                    if rest
                        .chars()
                        .all(|c| opts.short.get(&c).is_some_and(|o| !o.takes_value)) =>
                {
                    out.push(arg.clone())
                }
                _ => out.push(format!("-{first}{REDACTED}")),
            }
        } else if let Some(sub) = command.find_subcommand(arg) {
            out.push(arg.clone());
            command = sub;
            let mut next = Opts::default();
            // Globals stay valid below the subcommand.
            for (name, opt) in &opts.long {
                next.long.insert(name.clone(), *opt);
            }
            for (short, opt) in &opts.short {
                next.short.insert(*short, *opt);
            }
            next.add(command);
            opts = next;
        } else {
            out.push(REDACTED.to_owned());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn argv(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    fn redactor() -> Redactor {
        Redactor::new(false, [], Some("/home/tester".into()))
    }

    #[test]
    fn urls_and_auth_headers_lose_credentials() {
        let out = redactor().text(
            "fetching https://ops:s3cr3t-pw@seeds.example.org/list?auth=abc123&sig=XyZ987#frag\n\
             from postgres://archive:db-pass-1@db.internal:5432/archive, then\n\
             curl -H \"Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.e30.sig42\" and\n\
             Proxy-Authorization: Basic dXNlcjpwYXNz\n\
             header x-api-key: key-value-9\n\
             bare bearer tok_live_51Habc, token=tok-123 auth_token=at-456\n\
             and http://127.0.0.1:3650/status (fine).",
        );
        for leak in [
            "ops",
            "s3cr3t-pw",
            "abc123",
            "XyZ987",
            "frag",
            "db-pass-1",
            "eyJhbGciOiJIUzI1NiJ9",
            "sig42",
            "dXNlcjpwYXNz",
            "key-value-9",
            "tok_live_51Habc",
            "tok-123",
            "at-456",
        ] {
            assert!(!out.contains(leak), "{leak} leaked: {out}");
        }
        // Redacting twice, as a report does, changes nothing.
        assert_eq!(redactor().text(&out), out);
        for kept in [
            "fetching https://seeds.example.org/<redacted>\n",
            "from postgres://db.internal:5432/<redacted>, then",
            "and http://127.0.0.1:3650/<redacted> (fine).",
        ] {
            assert!(out.contains(kept), "{kept} lost: {out}");
        }
        assert_eq!(
            redactor().text("http://127.0.0.1:3650"),
            "http://127.0.0.1:3650"
        );
    }

    /// The same credentials hidden by JSON escapes, `\u` escapes or
    /// percent-encoding, or put in a URL's path.
    #[test]
    fn escaped_and_encoded_credentials_go() {
        let out = redactor().text(
            r#"{"url":"https:\/\/user:pw-json@seeds.example.test\/p?token=tok-json-1"}
unicode https:\u002F\u002Fseeds.example.test\u002Fsk_test_ABC123\u002Fpeers
encoded next=https%3A%2F%2Fu%3Apw-pct%40seeds.example.test%2Fp%3Ftoken%3Dtok-pct-2
encoded token sk%5Flive%5F4eC39HqLyjWDarjtT1zdp7dc
path https://seeds.example.test/sk_test_ABC123/peers
"#,
        );
        for leak in [
            "pw-json",
            "tok-json-1",
            "sk_test_ABC123",
            "ABC123",
            "pw-pct",
            "tok-pct-2",
            "4eC39HqLyjWDarjtT1zdp7dc",
        ] {
            assert!(!out.contains(leak), "{leak} leaked: {out}");
        }
        assert_eq!(
            out.matches("https://seeds.example.test/<redacted>").count(),
            4,
            "{out}"
        );
        assert_eq!(redactor().text(&out), out);
    }

    /// Opaque tokens go, whatever their name or format; the public Mina
    /// identifiers a developer needs stay, checked by their exact format.
    #[test]
    fn opaque_tokens_go_and_mina_ids_stay() {
        let secrets = [
            // An unlabeled 32-byte hex key.
            "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
            "sk_live_4eC39HqLyjWDarjtT1zdp7dc",
            "sk_test_ABC123",
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
            // Base64, padded, and unpadded with `/` and `+`.
            "c2VjcmV0LXZhbHVlLWZvci1vMy10ZXN0cw==",
            "q3Lw9/Hk2+ZtR8vN1pXc4Yb7Ms0Ea6Jf",
            // Base58 that is no Mina format.
            "4vJ9JU1bJJE96FWSJKvHsmmFADCg4gpZQff4P3bkLKi",
            "AKIAIOSFODNN7EXAMPLE",
            "ghp_16C7e42F292c6912E7710c838347Ae178B4a",
            // A state hash with one character changed: its checksum fails.
            "3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrK",
        ];
        for secret in secrets {
            let out = redactor().text(&format!("value {secret} end"));
            assert_eq!(out, "value <redacted> end", "{secret}");
        }
        let kept = format!(
            "state 3NLT7n4LiVo6U4LXr9BjCEp9712fP61uXkRC6hnRnB682A8f4HrJ \
             block-3NLJiVVdeMhYJndjjy8jrvnbUgVvQXVpkpEiRgVchL53yWCxYSgZ.bin \
             ledger jw8rQN8Wu7mFYuCfGV8SVc1BxvaeW8QnENFBz7N3B5duj5fWrN1 \
             ledger-jwrCSCRNpNx3ZBh6D4bUAnZtqPacKXXe6veGC9kQfHrxWqmEbJS.full \
             key B62qiy32p8kAKnny8ZFwoMhYpBppM1DWVCqAPBYNcXnsAHhnfAAuXgg \
             devnet ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8 \
             mainnet 0718f61ab88f9d0fa643ff4dc3a3d5998dd6d51a6008b2b0339b0dbb26133886 \
             commit {} short d891e79650de \
             version m11-browser-block-and-snarks-62-g6be3db81c \
             kind P2pNetworkSchedulerSelectError TransactionPoolFetchAccountsFailed \
             module transition_frontier_sync_ledger_snarked_reducer \
             target x86_64-unknown-linux-gnu file crash-1792704021763-637195-0.json \
             time=\"1792703717356516351\" ~/checkouts/mina-rust/example/.work \
             [fp:b278ecbd]",
            mina_node::BuildEnv::get().git.commit_hash
        );
        assert_eq!(redactor().text(&kept), kept);
        // An allowed value: the node's chain id or build commit.
        let mut r = redactor();
        let other_commit = "9db50e6b4fc5b2eaea7a3bdc1253e8711fe92e49";
        assert_eq!(r.text(other_commit), REDACTED);
        r.allow(other_commit);
        assert_eq!(r.text(other_commit), other_commit);
        r.allow("not-hex-at-all-but-long-enough-to-allow");
        assert_eq!(r.text("sk_live_4eC39HqLyjWDarjtT1zdp7dc"), REDACTED);
    }

    #[test]
    fn values_named_like_secrets_go() {
        let out = redactor().text(
            "FOO_TOKEN=abc123xyz JWT=header.payload.sig MY_PWD=hunter2pw\n\
             session_id: s3ss10n X-Auth: v4lue DB_PASSWORD=\"p w d\" api_key='k3y'\n\
             Cookie: sid=c00kie\n\
             kind=ledger peers=3 height: 42",
        );
        for leak in [
            "abc123xyz",
            "header.payload.sig",
            "hunter2pw",
            "s3ss10n",
            "v4lue",
            "p w d",
            "k3y",
            "c00kie",
        ] {
            assert!(!out.contains(leak), "{leak} leaked: {out}");
        }
        for kept in ["FOO_TOKEN=<redacted>", "kind=ledger peers=3 height: 42"] {
            assert!(out.contains(kept), "{kept} lost: {out}");
        }
    }

    #[test]
    fn secret_env_names() {
        for name in [
            "MINA_PRIVKEY_PASS",
            "MINA_P2P_SEC_KEY",
            "MINA_LIBP2P_PASS",
            "PRODUCER_KEY",
            "AWS_SECRET_ACCESS_KEY",
            "GITHUB_TOKEN",
            "OPENAI_API_KEY",
            "JWT",
            "my_cookie",
            "SESSION_ID",
            "DB_PWD",
            "X_AUTH",
            "FOO_CRED",
        ] {
            assert!(is_secret_env_name(name), "{name}");
        }
        for name in ["MINA_HOME", "MINA_NETWORK", "PORT", "RUST_LOG"] {
            assert!(!is_secret_env_name(name), "{name}");
        }
        // Scrubbed wherever they appear: the known ones, and long values of
        // the others; not the current directory or desktop plumbing.
        for (name, value, scrubbed) in [
            ("MINA_PRIVKEY_PASS", "abcd", true),
            ("FOO_TOKEN", "long-enough-1", true),
            ("FOO_TOKEN", "short", false),
            ("XDG_SESSION_TYPE", "wayland-session", false),
            ("PWD", "/home/tester/work", false),
            ("MINA_HOME", "/home/tester/.mina", false),
        ] {
            assert_eq!(scrub_env_value(name, value), scrubbed, "{name}");
        }
    }

    #[test]
    fn keys_and_secrets_are_scrubbed() {
        let account = AccountSecretKey::rand().to_string();
        let p2p = SecretKey::rand().to_string();
        let mut r = redactor();
        r.add_secret("hunter2-password");
        let text = format!(
            "key {account} p2p {p2p} pass hunter2-password\n\
             MINA_PRIVKEY_PASS=plain-text-pass export MINA_P2P_SEC_KEY='quoted'\n\
             {{\"box_primitive\":\"xsalsa20poly1305\",\"pwsalt\":\"9bQ4kZ\",\"pwdiff\":[134217728,6],\"ciphertext\":\"Dmq1Qd8uNbZRT5NWkW\"}}\n\
             --producer-key /srv/producer-key.json --p2p-secret-key=abc123 a=ice-pwd:Zx81kk\n\
             CAESQKfz5mDQSP6LZbPU7XNEgdbjcqvIqvdK4Y3fq5yUfe2/VFqIXgAFrmJvnZmLwFAJZK5xQ5bmBtlwM+93n7LqUhSA="
        );
        let out = r.text(&text);
        for leak in [
            account.as_str(),
            p2p.as_str(),
            "hunter2-password",
            "plain-text-pass",
            "quoted",
            "9bQ4kZ",
            "Dmq1Qd8uNbZRT5NWkW",
            "134217728",
            "/srv/producer-key.json",
            "abc123",
            "Zx81kk",
            "CAESQKfz",
        ] {
            assert!(!out.contains(leak), "{leak} leaked: {out}");
        }
        assert!(
            out.contains(PRIVATE_KEY) && out.contains(P2P_SECRET_KEY),
            "{out}"
        );
    }

    #[test]
    fn key_file_paths_and_home_are_hidden() {
        let out = redactor().text(
            "read file_name=\"/home/tester/.mina/keys/block-producer\" \
             and ./wallets/main.json, /opt/x/libp2p-keypair, other /home/alice/notes \
             but crates/p2p/src/identity/secret_key.rs:12 and /home/tester/.mina/mina.log stay, \
             as does /data/run/home/mina.log.2026-10-22",
        );
        assert!(!out.contains("block-producer"), "{out}");
        assert!(!out.contains("main.json"), "{out}");
        assert!(!out.contains("libp2p-keypair"), "{out}");
        assert!(!out.contains("alice") && !out.contains("tester"), "{out}");
        assert!(
            out.contains("crates/p2p/src/identity/secret_key.rs:12"),
            "{out}"
        );
        assert!(out.contains("~/.mina/mina.log"), "{out}");
        assert!(out.contains("/data/run/home/mina.log.2026-10-22"), "{out}");
    }

    /// Markdown shows paths in backticks: a home there is hidden too, as
    /// in the report's "free for the work dir `...`" row.
    #[test]
    fn homes_in_backticks_are_hidden() {
        let out = redactor().text(
            "| Disk | 67.5 GiB free for the work dir `/home/alice/workspaces/node/home` |\n\
             and ours `/home/tester` and `/home/tester/.mina`, but `/srv/home/mina` stays",
        );
        assert!(!out.contains("alice"), "{out}");
        assert!(out.contains("`/home/<user>/workspaces/node/home`"), "{out}");
        assert!(out.contains("`~`") && out.contains("`~/.mina`"), "{out}");
        assert!(out.contains("`/srv/home/mina`"), "{out}");
    }

    #[test]
    fn network_ids_are_hidden_unless_asked() {
        let peer = SecretKey::rand().public_key().peer_id();
        let libp2p = libp2p_identity::PeerId::try_from(peer).unwrap().to_string();
        let ours = peer.to_string();
        let text = format!(
            "addr=34.136.105.213:10003 /ip4/10.1.2.3/tcp/8302/p2p/{libp2p} \
             /dns4/my-node.example.org/tcp/8302 [2001:db8::7]:8302 peer {ours} \
             local 127.0.0.1:3000 and ::1, at 2026-10-18T19:14:53.401738Z mina_tree::proofs ee::ff"
        );
        let out = redactor().text(&text);
        for leak in [
            "34.136.105.213",
            "10.1.2.3",
            libp2p.as_str(),
            "my-node.example.org",
            "2001:db8::7",
            ours.as_str(),
        ] {
            assert!(!out.contains(leak), "{leak} leaked: {out}");
        }
        for kept in [
            "127.0.0.1:3000",
            "::1",
            "19:14:53.401738Z",
            "mina_tree::proofs",
            ":10003",
        ] {
            assert!(out.contains(kept), "{kept} lost: {out}");
        }
        let kept = Redactor::new(true, [], None).text(&text);
        for id in [
            "34.136.105.213",
            libp2p.as_str(),
            ours.as_str(),
            "2001:db8::7",
        ] {
            assert!(
                kept.contains(id),
                "{id} lost with --include-network-ids: {kept}"
            );
        }
    }

    #[test]
    fn hashes_and_ordinary_text_stay() {
        let text = "best tip 3NLJiVVdeMhYJndjjy8jrvnbUgVvQXVpkpEiRgVchL53yWCxYSgZ height 42 \
                    ledger jw8rQN8Wu7mFYuCfGV8SVc1BxvaeW8QnENFBz7N3B5duj5fWrN1 \
                    chain ebfce0d570bc22eb041e1a7b0a46bbc3ed5d0a1030ef2ab5e36eef908b93eba8 \
                    kind=\"P2pNetworkSchedulerSelectError\" error=parse_token";
        assert_eq!(redactor().text(text), text);
    }

    /// The renames of this release, old name first: both are scrubbed
    /// alike, at short values (under the named-secret length) and long ones.
    const RENAMED_ENV: &[(&str, &str)] = &[
        ("PRODUCER_KEY", "MINA_PRODUCER_KEY"),
        ("MINA_ENC_KEY", "MINA_SEC_KEY"),
        ("RUN_SNARKER", "MINA_RUN_SNARKER"),
        ("SNARKER_FEE", "MINA_SNARKER_FEE"),
        ("FROM", "MINA_FROM"),
        ("CONFIG", "MINA_CONFIG"),
        ("GENESIS_CONFIG", "MINA_GENESIS_CONFIG"),
        ("RNG_SEED", "MINA_RNG_SEED"),
        ("SEED", "MINA_SEED"),
        ("PORT", "MINA_PORT"),
        ("LIBP2P_PORT", "MINA_LIBP2P_PORT"),
        ("PEER_LIST_URL", "MINA_PEER_LIST_URL"),
        ("PEER_LIST_FILE", "MINA_PEER_LIST_FILE"),
        ("VERBOSITY", "MINA_VERBOSITY"),
        ("RECORD", "MINA_RECORD"),
    ];
    const VALUES: &[&str] = &["abcd", "abcdefg", "a-long-value-0123456789"];

    #[test]
    fn renamed_variables_are_treated_as_their_old_names() {
        for &(old, new) in RENAMED_ENV {
            assert_eq!(is_secret_env_name(old), is_secret_env_name(new), "{new}");
            for value in VALUES {
                assert_eq!(
                    scrub_env_value(old, value),
                    scrub_env_value(new, value),
                    "{new}={value}"
                );
            }
        }
        // The key variables are scrubbed at any length a secret is kept.
        for name in [
            "PRODUCER_KEY",
            "MINA_PRODUCER_KEY",
            "MINA_ENC_KEY",
            "MINA_SEC_KEY",
        ] {
            for value in VALUES {
                assert!(scrub_env_value(name, value), "{name}={value}");
            }
        }
    }

    /// Through the report's own path: a value taken from the environment is
    /// gone wherever it appears in the text, under the old name or the new.
    #[test]
    fn renamed_key_variables_are_scrubbed_from_report_text() {
        for (old, new) in [
            ("PRODUCER_KEY", "MINA_PRODUCER_KEY"),
            ("MINA_SEC_KEY", "MINA_ENC_KEY"),
        ] {
            for value in VALUES {
                for name in [old, new] {
                    let r = Redactor::from_vars(
                        false,
                        [(name.to_owned(), value.to_string())],
                        Some("/home/tester".into()),
                    );
                    let out = r.text(&format!("starting with {value} as the key"));
                    assert!(!out.contains(value), "{name}={value}: {out}");
                }
            }
        }
    }

    /// Free-text flags: an alias is scrubbed as its option is.
    #[test]
    fn renamed_and_alias_flags_are_scrubbed_as_their_options() {
        for (old, new) in [
            ("--producer-key", "--block-producer-key"),
            ("--secret-key", "--secret-key"),
            ("--p2p-secret-key", "--p2p-secret-key"),
        ] {
            for value in VALUES {
                for flag in [old, new] {
                    for text in [
                        format!("ran {flag} {value} ok"),
                        format!("ran {flag}={value} ok"),
                    ] {
                        let out = redactor().text(&text);
                        assert!(!out.contains(value), "{text}: {out}");
                        assert!(out.contains(&format!("{flag} {REDACTED}")), "{text}: {out}");
                    }
                }
            }
        }
        // Public values stay: OCaml's names for the libp2p port and fee.
        let text = "ran --external-port 8302 --snark-worker-fee 0.1";
        assert_eq!(redactor().text(text), text);
    }
}
#[test]
fn terminal_paths_abbreviate_home_on_a_component_boundary() {
    let redactor = Redactor::new(false, [], Some("/home/tester/".into()));
    for (path, shown) in [
        ("/home/tester", "~"),
        ("/home/tester/.mina/frontier", "~/.mina/frontier"),
        ("/home/tester-two/.mina", "/home/tester-two/.mina"),
        ("relative/frontier", "relative/frontier"),
        ("/var/lib/mina", "/var/lib/mina"),
    ] {
        assert_eq!(redactor.display_path(Path::new(path)), shown);
    }
    assert_eq!(
        Redactor::new(false, [], None).display_path(Path::new("/var/lib/mina")),
        "/var/lib/mina"
    );
}
