//! The node dashboard (F-console): the single-page app in `ui/dashboard`,
//! built by `make dashboard` to `ui/dashboard/dist` and embedded in the
//! binary at compile time, served at `/`.
//!
//! It is a fallback of the HTTP router: every API route is matched first, so
//! the dashboard can never shadow one. Of the paths no route takes, it
//! answers GET and HEAD for its files (`/assets/...`, `/favicon.svg`, ...)
//! and, with `index.html`, for its own pages only: the paths listed in
//! `ui/dashboard/src/routes.json`, which the app's router reads too. Anything
//! else is a 404, as before.
//!
//! A checkout without a built dashboard (no npm at build time, as the arm64
//! cross build) still compiles: the embed is empty, and `/` answers a page
//! saying how to build it.

use std::borrow::Cow;

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Router,
};

/// The built dashboard. `allow_missing`: without a build, it is empty.
#[derive(rust_embed::RustEmbed)]
#[folder = "../../../ui/dashboard/dist"]
#[allow_missing = true]
struct Dist;

/// The dashboard's pages, shared with its router.
const ROUTES_JSON: &str = include_str!("../../../../../ui/dashboard/src/routes.json");

/// What `/` answers when the dashboard wasn't built into this binary.
pub const PLACEHOLDER: &str = r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Node dashboard not built</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1.25rem;color:#0a0f14;background:#f5f8f9}code{font:14px ui-monospace,monospace;background:#e6ecef;padding:.1rem .3rem;border-radius:.25rem}</style>
</head>
<body>
<h1>Dashboard not built</h1>
<p>This node was compiled without its dashboard. Run <code>make dashboard</code> in the repository, then build the node again.</p>
<p>The node itself is running: its API answers at <code>/status</code> and <code>/graphql</code>.</p>
</body>
</html>
"#;

/// The dashboard's own security headers. Its pages change the SNARK worker,
/// so no other site may frame them (clickjacking), and they load nothing
/// from elsewhere.
const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; script-src 'self'; \
     style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; \
     connect-src 'self'; object-src 'none'; base-uri 'none'; form-action 'self'; \
     frame-ancestors 'none'";

/// A file of the build: its bytes and a strong validator.
pub struct Asset {
    pub data: Cow<'static, [u8]>,
    pub sha256: [u8; 32],
}

/// Where the dashboard's files come from: the embedded build, or a test's.
pub trait Assets: Send + Sync + 'static {
    /// The file at `path`, without the leading `/`.
    fn get(&self, path: &str) -> Option<Asset>;
}

/// The build embedded in this binary.
pub struct Embedded;

impl Assets for Embedded {
    fn get(&self, path: &str) -> Option<Asset> {
        Dist::get(path).map(|file| Asset {
            sha256: file.metadata.sha256_hash(),
            data: file.data,
        })
    }
}

/// Whether this binary carries a built dashboard.
pub fn built() -> bool {
    Embedded.get("index.html").is_some()
}

/// The patterns of `routes.json`; only `/` if it can't be read (a test
/// checks that it can).
pub fn route_patterns() -> &'static [String] {
    static PATTERNS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| {
        #[derive(serde::Deserialize)]
        struct Routes {
            routes: Vec<String>,
        }
        serde_json::from_str::<Routes>(ROUTES_JSON)
            .map(|routes| routes.routes)
            .unwrap_or_else(|_| vec!["/".to_owned()])
    })
}

/// Whether `path` is one of the dashboard's pages (`routes.json`; `:id` is
/// one non-empty path segment; one trailing slash is allowed after a page
/// other than `/`). The app's router reads paths the same way.
pub fn is_page(path: &str) -> bool {
    let path = match path.strip_suffix('/') {
        Some(stripped) if stripped.len() > 1 => stripped,
        _ => path,
    };
    route_patterns()
        .iter()
        .any(|pattern| match pattern.strip_suffix(":id") {
            Some(prefix) => path
                .strip_prefix(prefix)
                .is_some_and(|id| !id.is_empty() && !id.contains('/')),
            None => path == pattern,
        })
}

/// Adds the dashboard to `router`, as its fallback.
pub fn routes<S>(router: Router<S>) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    with_assets(router, Embedded)
}

/// [`routes`] with another source of files, for tests.
pub fn with_assets<S, A>(router: Router<S>, assets: A) -> Router<S>
where
    S: Clone + Send + Sync + 'static,
    A: Assets,
{
    let assets = std::sync::Arc::new(assets);
    router.fallback(move |method: Method, uri: Uri, headers: HeaderMap| {
        let assets = assets.clone();
        async move { serve(&*assets, &method, &uri, &headers) }
    })
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("html") => "text/html; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json" | "map") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("ico") => "image/x-icon",
        Some("woff2") => "font/woff2",
        Some("woff") => "font/woff",
        Some("txt") => "text/plain; charset=utf-8",
        Some("webmanifest") => "application/manifest+json",
        _ => "application/octet-stream",
    }
}

fn etag(sha256: &[u8; 32]) -> String {
    format!("\"{}\"", hex::encode(&sha256[..16]))
}

fn not_found() -> Response {
    StatusCode::NOT_FOUND.into_response()
}

/// The dashboard's answer to a request no API route took.
pub fn serve(assets: &dyn Assets, method: &Method, uri: &Uri, headers: &HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return not_found();
    }
    let path = uri.path();
    // A file of the build, by its exact path. `index.html` itself is served
    // only as a page.
    let file = path
        .strip_prefix('/')
        .filter(|file| !file.is_empty() && !file.ends_with('/') && *file != "index.html")
        .and_then(|file| assets.get(file).map(|asset| (file, asset)));
    let (file, asset, page) = match file {
        Some((file, asset)) => (file, asset, false),
        None if is_page(path) => match assets.get("index.html") {
            Some(asset) => ("index.html", asset, true),
            None => {
                return html_response(StatusCode::OK, PLACEHOLDER.as_bytes().into(), None, method);
            }
        },
        None => return not_found(),
    };
    let tag = etag(&asset.sha256);
    let fresh = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.split(',').any(|candidate| candidate.trim() == tag));
    if page {
        let status = if fresh {
            StatusCode::NOT_MODIFIED
        } else {
            StatusCode::OK
        };
        let body = if fresh {
            Cow::Borrowed(&[][..])
        } else {
            asset.data
        };
        return html_response(status, body, Some(&tag), method);
    }
    // Vite names what's under assets/ by its content: cache it for good.
    let cache = if file.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let mut response = if fresh {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let len = asset.data.len();
        let mut response = Response::new(if method == Method::HEAD {
            Body::empty()
        } else {
            Body::from(asset.data)
        });
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(len));
        response
    };
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(content_type(file)),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if let Ok(tag) = HeaderValue::from_str(&tag) {
        headers.insert(header::ETAG, tag);
    }
    response
}

fn html_response(
    status: StatusCode,
    body: Cow<'static, [u8]>,
    tag: Option<&str>,
    method: &Method,
) -> Response {
    let len = body.len();
    let mut response = Response::new(if method == Method::HEAD {
        Body::empty()
    } else {
        Body::from(body)
    });
    *response.status_mut() = status;
    let headers = response.headers_mut();
    if status != StatusCode::NOT_MODIFIED {
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        // HEAD says how long the page is, as GET would send it.
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(len));
    }
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_SECURITY_POLICY),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    if let Some(tag) = tag.and_then(|tag| HeaderValue::from_str(tag).ok()) {
        headers.insert(header::ETAG, tag);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A build of two pages' worth of files.
    struct Fake;

    impl Assets for Fake {
        fn get(&self, path: &str) -> Option<Asset> {
            let data: &'static [u8] = match path {
                "index.html" => b"<!doctype html><title>Node dashboard</title>",
                "assets/index-abc123.js" => b"console.log(1)",
                "assets/inter.woff2" => b"wOF2",
                "favicon.svg" => b"<svg/>",
                "theme-init.js" => b"(function(){})()",
                "licenses/fonts.txt" => b"OFL",
                _ => return None,
            };
            let mut sha256 = [0u8; 32];
            sha256[..path.len().min(32)].copy_from_slice(&path.as_bytes()[..path.len().min(32)]);
            Some(Asset {
                data: Cow::Borrowed(data),
                sha256,
            })
        }
    }

    /// `ui/dashboard/dist` when it was never built: the same derive as
    /// [`Dist`], on a folder that doesn't exist.
    #[derive(rust_embed::RustEmbed)]
    #[folder = "src/http_server/dashboard-dist-that-does-not-exist"]
    #[allow_missing = true]
    struct NotBuilt;

    struct Missing;

    impl Assets for Missing {
        fn get(&self, path: &str) -> Option<Asset> {
            NotBuilt::get(path).map(|file| Asset {
                sha256: file.metadata.sha256_hash(),
                data: file.data,
            })
        }
    }

    fn get(assets: &dyn Assets, path: &str) -> Response {
        request(assets, Method::GET, path, &[])
    }

    fn request(
        assets: &dyn Assets,
        method: Method,
        path: &str,
        headers: &[(&'static str, &str)],
    ) -> Response {
        let mut map = HeaderMap::new();
        for (name, value) in headers {
            map.insert(*name, HeaderValue::from_str(value).unwrap());
        }
        serve(assets, &method, &path.parse().unwrap(), &map)
    }

    async fn body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn header<'a>(response: &'a Response, name: &str) -> &'a str {
        response
            .headers()
            .get(name)
            .map(|value| value.to_str().unwrap())
            .unwrap_or("")
    }

    #[test]
    fn routes_json_lists_the_pages() {
        let patterns = route_patterns();
        assert!(patterns.len() > 1, "{patterns:?}");
        assert_eq!(patterns[0], "/");
        for page in [
            "/",
            "/snark-worker",
            "/snark-worker/",
            "/block-producer",
            "/blocks",
            "/blocks/",
            "/blocks/578883",
            "/blocks/3NLqEMZA6dJ16P9fMWA4oxFQTUcNVyoSCyZkabNbichyFH5vuc5W",
            "/account",
            "/account/B62qp3B9VW1ir5qL1MWRwr6ecjC2NZbGr8vysGeme9vXGcFXTMNXb2t",
            "/mempool",
            "/peers",
            "/node-status",
            "/graphql-endpoint",
            "/report-a-problem",
        ] {
            assert!(is_page(page), "{page}");
        }
        for other in [
            "",
            "//",
            "/blocks//",
            "/blocks/1/2",
            "/account//",
            "/accounts",
            "/status",
            "/graphql",
            "/index.html",
            "/peersx",
            "/state/peers",
            "/report",
            "/report/crash",
        ] {
            assert!(!is_page(other), "{other}");
        }
    }

    #[tokio::test]
    async fn pages_get_index_html_with_the_dashboards_headers() {
        for path in ["/", "/snark-worker", "/blocks/578883", "/peers/"] {
            let response = get(&Fake, path);
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(
                header(&response, "content-type"),
                "text/html; charset=utf-8"
            );
            assert_eq!(header(&response, "cache-control"), "no-cache");
            assert_eq!(
                header(&response, "content-security-policy"),
                CONTENT_SECURITY_POLICY
            );
            assert!(header(&response, "content-security-policy").contains("frame-ancestors 'none'"));
            assert_eq!(header(&response, "x-frame-options"), "DENY");
            assert_eq!(header(&response, "x-content-type-options"), "nosniff");
            assert!(body(response).await.contains("Node dashboard"));
        }
    }

    #[tokio::test]
    async fn files_get_their_type_and_cache_lifetime() {
        for (path, content_type, cache) in [
            (
                "/assets/index-abc123.js",
                "text/javascript; charset=utf-8",
                "public, max-age=31536000, immutable",
            ),
            (
                "/assets/inter.woff2",
                "font/woff2",
                "public, max-age=31536000, immutable",
            ),
            ("/favicon.svg", "image/svg+xml", "no-cache"),
            (
                "/theme-init.js",
                "text/javascript; charset=utf-8",
                "no-cache",
            ),
            (
                "/licenses/fonts.txt",
                "text/plain; charset=utf-8",
                "no-cache",
            ),
        ] {
            let response = get(&Fake, path);
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(header(&response, "content-type"), content_type, "{path}");
            assert_eq!(header(&response, "cache-control"), cache, "{path}");
            assert_eq!(header(&response, "x-content-type-options"), "nosniff");
            assert!(header(&response, "etag").starts_with('"'));
        }
    }

    #[tokio::test]
    async fn anything_else_is_not_found() {
        for path in [
            "/index.html",
            "/assets/",
            "/assets/missing.js",
            "/nope",
            "/blocks/1/2",
            "/metrics",
        ] {
            let response = get(&Fake, path);
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(header(&response, "content-security-policy"), "", "{path}");
            assert!(body(response).await.is_empty());
        }
        for method in [Method::POST, Method::PUT, Method::DELETE, Method::OPTIONS] {
            for path in ["/", "/assets/index-abc123.js"] {
                let response = request(&Fake, method.clone(), path, &[]);
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
            }
        }
    }

    #[tokio::test]
    async fn etags_spare_a_download_and_head_has_no_body() {
        let tag = header(&get(&Fake, "/"), "etag").to_owned();
        let response = request(&Fake, Method::GET, "/peers", &[("if-none-match", &tag)]);
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(body(response).await.is_empty());
        let file = header(&get(&Fake, "/favicon.svg"), "etag").to_owned();
        let response = request(
            &Fake,
            Method::GET,
            "/favicon.svg",
            &[("if-none-match", &format!("\"x\", {file}"))],
        );
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        let response = request(&Fake, Method::GET, "/", &[("if-none-match", "\"stale\"")]);
        assert_eq!(response.status(), StatusCode::OK);
        for path in ["/", "/favicon.svg"] {
            let length = body(get(&Fake, path)).await.len().to_string();
            let response = request(&Fake, Method::HEAD, path, &[]);
            assert_eq!(response.status(), StatusCode::OK, "{path}");
            assert_eq!(header(&response, "content-length"), length, "{path}");
            assert!(body(response).await.is_empty(), "{path}");
        }
    }

    /// A node built without `make dashboard` compiles, and says so at `/`.
    #[tokio::test]
    async fn a_missing_build_serves_the_placeholder() {
        assert!(NotBuilt::get("index.html").is_none());
        for path in ["/", "/snark-worker"] {
            let response = get(&Missing, path);
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                header(&response, "content-security-policy"),
                CONTENT_SECURITY_POLICY
            );
            let text = body(response).await;
            assert!(text.contains("Dashboard not built"), "{text}");
            assert!(text.contains("make dashboard"), "{text}");
        }
        assert_eq!(
            get(&Missing, "/assets/index.js").status(),
            StatusCode::NOT_FOUND
        );
    }
}
