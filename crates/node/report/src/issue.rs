//! The pre-filled GitHub issue a report opens: the link, within GitHub's
//! URL limit, or a shorter one with the summary when the report is too long
//! for it.

/// Where reports are filed, unless `mina report --repo` says otherwise.
pub const DEFAULT_REPO: &str = "minanode/mina-rust";

/// Longer `issues/new` links fail: GitHub answers 414 from about 8 KiB.
pub const MAX_URL_LEN: usize = 8000;
/// GitHub's limit for an issue title.
const MAX_TITLE_CHARS: usize = 256;

/// `owner/name`, so that the repository can't change the link's host or
/// path.
pub fn check_repo(repo: &str) -> anyhow::Result<()> {
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    match repo.split_once('/') {
        Some((owner, name)) if valid(owner) && valid(name) => Ok(()),
        _ => anyhow::bail!("--repo {repo:?}: expected OWNER/NAME, as minanode/mina-rust"),
    }
}

/// RFC 3986 percent-encoding of a query value: all but unreserved
/// characters, UTF-8 byte by byte.
pub fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The title as GitHub takes it: at most 256 characters.
pub fn title_for_url(title: &str) -> String {
    title.chars().take(MAX_TITLE_CHARS).collect()
}

pub fn issue_url(repo: &str, title: &str, body: &str) -> String {
    format!(
        "https://github.com/{repo}/issues/new?title={}&body={}",
        encode(&title_for_url(title)),
        encode(body)
    )
}

#[derive(Debug, PartialEq)]
pub enum Link {
    /// The whole body is in the link.
    Full(String),
    /// The body was too long: the link has the summary, after `note`.
    Summary(String),
}

impl Link {
    pub fn url(&self) -> &str {
        match self {
            Self::Full(url) | Self::Summary(url) => url,
        }
    }
}

const CUT: &str = "\n\n[...]";

/// A pre-filled issue link with `body` if it fits in [`MAX_URL_LEN`];
/// otherwise with `note` and as much of `summary` as fits.
pub fn link(
    repo: &str,
    title: &str,
    body: &str,
    summary: impl FnOnce() -> (String, String),
) -> Link {
    let url = issue_url(repo, title, body);
    if url.len() <= MAX_URL_LEN {
        return Link::Full(url);
    }
    let (note, summary) = summary();
    let short = format!("{note}\n\n{summary}");
    let chars: Vec<char> = short.chars().collect();
    let body = |n: usize| {
        let mut body: String = chars[..n].iter().collect();
        if n < chars.len() {
            body.push_str(CUT);
        }
        body
    };
    // The longest prefix that fits; the note comes first, so it stays.
    let (mut fits, mut too_long) = (0, chars.len() + 1);
    while too_long - fits > 1 {
        let n = (fits + too_long) / 2;
        if issue_url(repo, title, &body(n)).len() <= MAX_URL_LEN {
            fits = n;
        } else {
            too_long = n;
        }
    }
    Link::Summary(issue_url(repo, title, &body(fits)))
}

/// The note on a link that carries only the summary.
pub fn attach_note(place: &str) -> String {
    format!(
        "> **Attach the full report:** it is too long for this link and {place}. Drag that file \
         into this box before you submit."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(text: &str) -> String {
        let bytes = text.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn body_of(url: &str) -> String {
        decode(url.split("&body=").nth(1).unwrap())
    }

    #[test]
    fn encoding_round_trips() {
        let text = "Crash: `x` at a.rs:1 [fp:0a1b2c3d] & = ? # % + é\n| a |";
        let encoded = encode(text);
        assert!(encoded
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.~%".contains(&b)));
        assert_eq!(decode(&encoded), text);
        assert_eq!(encode("a b"), "a%20b");
    }

    #[test]
    fn short_reports_go_whole_into_the_link() {
        let url = issue_url("minanode/mina-rust", "Crash [fp:1]", "body text");
        assert_eq!(
            url,
            "https://github.com/minanode/mina-rust/issues/new?title=Crash%20%5Bfp%3A1%5D&body=body%20text"
        );
        let full = link("minanode/mina-rust", "t", "short body", || unreachable!());
        assert_eq!(
            full,
            Link::Full(issue_url("minanode/mina-rust", "t", "short body"))
        );
    }

    #[test]
    fn long_reports_fall_back_to_the_summary() {
        let body = "log line with spaces & symbols\n".repeat(2000);
        let note = attach_note("is saved at `~/.mina/bug-reports/report-1.md`");
        let cut = link(
            "minanode/mina-rust",
            "Error: X [fp:12345678]",
            &body,
            || (note.clone(), "summary ".repeat(5000)),
        );
        let Link::Summary(url) = &cut else {
            panic!("expected the summary");
        };
        assert!(url.len() <= MAX_URL_LEN, "{}", url.len());
        assert!(
            url.len() > MAX_URL_LEN - 10,
            "as much as fits: {}",
            url.len()
        );
        let shown = body_of(url);
        assert!(shown.starts_with(&note), "the note comes first");
        assert!(shown.ends_with(CUT));
        assert!(url.contains("title=Error%3A%20X%20%5Bfp%3A12345678%5D"));

        // A summary that fits is not cut.
        let small = link("o/r", "t", &body, || (note.clone(), "small".into()));
        assert_eq!(body_of(small.url()), format!("{note}\n\nsmall"));
    }

    #[test]
    fn titles_are_bounded() {
        let url = issue_url("o/r", &"x".repeat(1000), "");
        assert_eq!(
            url.len(),
            "https://github.com/o/r/issues/new?title=&body=".len() + 256
        );
    }

    #[test]
    fn repositories_are_checked() {
        assert!(check_repo("minanode/mina-rust").is_ok());
        for bad in ["minanode", "a/b/c", "evil.com/x?y", "../x", "a/", "o/r#x"] {
            assert!(check_repo(bad).is_err(), "{bad}");
        }
    }
}
