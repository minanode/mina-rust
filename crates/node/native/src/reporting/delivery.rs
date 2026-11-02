//! HTTP delivery of reports, with uptime's retry policy and interruption.

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};

use reqwest::Url;
use tokio::sync::Notify;

/// Uptime retries: eight attempts in all, four seconds apart; exactly 200
/// succeeds; 400, 401, 411 and 413 end the delivery; any other status
/// (other 2xx, 3xx and 429 included) and transport errors are retried.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/uptime_service/uptime_service.ml#L42-L151>
#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub pause: Duration,
}

impl RetryPolicy {
    pub const UPTIME: Self = Self {
        max_attempts: 8,
        pause: Duration::from_secs(4),
    };
    /// Node status and node error reports are sent once.
    pub const ONCE: Self = Self {
        max_attempts: 1,
        pause: Duration::ZERO,
    };
}

/// OCaml's `unrecoverable_status_codes`.
pub const UPTIME_TERMINAL_STATUSES: [u16; 4] = [400, 401, 411, 413];

/// Timeouts OCaml doesn't set: bounded here so that one stuck connection
/// can't hold a report past its iteration.
#[derive(Clone, Copy, Debug)]
pub struct Timeouts {
    pub connect: Duration,
    /// For the whole exchange: connecting, sending the body, the response.
    pub request: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(10),
            request: Duration::from_secs(60),
        }
    }
}

/// An HTTP client as OCaml's cohttp behaves: no proxy from the environment,
/// no redirects followed (a 3xx is just another status), no extra headers
/// but `Content-Type: application/json` and a user agent.
pub fn client(timeouts: Timeouts) -> reqwest::Result<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(timeouts.connect)
        .timeout(timeouts.request)
        .user_agent(concat!("mina-rust/", env!("CARGO_PKG_VERSION")))
        .build()
}

/// The outcome of one POST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attempt {
    Status(u16),
    /// Connection refused, reset, timed out, ...; no URL in the text.
    Transport(String),
}

impl std::fmt::Display for Attempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status(status) => write!(f, "HTTP {status}"),
            Self::Transport(error) => f.write_str(error),
        }
    }
}

pub async fn post_json(client: &reqwest::Client, url: &Url, body: bytes::Bytes) -> Attempt {
    let response = client
        .post(url.clone())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await;
    match response {
        Ok(response) => {
            let status = response.status().as_u16();
            // Only the status matters for all three reporters. Read zero
            // body bytes and drop the response: a receiver can send an
            // arbitrarily large (or endless) body. Do not buffer it, drain
            // it for connection reuse, or include receiver text in logs.
            drop(response);
            Attempt::Status(status)
        }
        Err(error) => Attempt::Transport(transport_error(error)),
    }
}

fn transport_error(error: reqwest::Error) -> String {
    let kind = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connection error"
    } else if error.is_body() || error.is_request() {
        "request error"
    } else {
        "transport error"
    };
    // The URL may carry a token: never in logs or status.
    let error = error.without_url();
    let mut text = format!("{kind}: {error}");
    let mut source = std::error::Error::source(&error);
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

/// How a delivery ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    Delivered {
        attempts: u32,
    },
    /// A status that is not retried.
    Rejected {
        attempts: u32,
        status: u16,
    },
    /// Retries used up; the report is dropped.
    Exhausted {
        attempts: u32,
        last: Attempt,
    },
    /// The next iteration started.
    Interrupted {
        attempts: u32,
    },
}

/// Called after every attempt, and when retrying.
pub trait DeliveryObserver {
    fn attempt(&mut self, attempt_no: u32, outcome: &Attempt);
}

impl<F: FnMut(u32, &Attempt)> DeliveryObserver for F {
    fn attempt(&mut self, attempt_no: u32, outcome: &Attempt) {
        self(attempt_no, outcome)
    }
}

/// Classifies a status: success, retry, or stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusClass {
    Success,
    Retry,
    Terminal,
}

pub type StatusPolicy = fn(u16) -> StatusClass;

pub fn uptime_status_class(status: u16) -> StatusClass {
    if status == 200 {
        StatusClass::Success
    } else if UPTIME_TERMINAL_STATUSES.contains(&status) {
        StatusClass::Terminal
    } else {
        StatusClass::Retry
    }
}

/// Node status: any 2xx succeeds; nothing is retried.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_status_service/node_status_service.ml#L118-L137>
pub fn status_status_class(status: u16) -> StatusClass {
    if (200..300).contains(&status) {
        StatusClass::Success
    } else {
        StatusClass::Terminal
    }
}

/// Node error: exactly 200 succeeds; nothing is retried.
///
/// OCaml: <https://github.com/MinaProtocol/mina/blob/84b8d96954b218f372b93f65a071abf528e6bb61/src/lib/node_error_service/node_error_service.ml#L76-L99>
pub fn error_status_class(status: u16) -> StatusClass {
    if status == 200 {
        StatusClass::Success
    } else {
        StatusClass::Terminal
    }
}

/// Sends `body` (serialized and signed once by the caller) until it is
/// accepted, refused, retries run out or `interrupt` fires. Interruption
/// stops at once, in an attempt or between attempts.
pub async fn deliver(
    client: &reqwest::Client,
    url: &Url,
    body: bytes::Bytes,
    policy: RetryPolicy,
    classify: StatusPolicy,
    interrupt: &Interruptor,
    observer: &mut impl DeliveryObserver,
) -> Delivery {
    let mut attempt_no = 0u32;
    loop {
        attempt_no = attempt_no.saturating_add(1);
        let outcome = tokio::select! {
            outcome = post_json(client, url, body.clone()) => outcome,
            _ = interrupt.wait() => return Delivery::Interrupted { attempts: attempt_no },
        };
        observer.attempt(attempt_no, &outcome);
        match outcome {
            Attempt::Status(status) => match classify(status) {
                StatusClass::Success => {
                    return Delivery::Delivered {
                        attempts: attempt_no,
                    }
                }
                StatusClass::Terminal => {
                    return Delivery::Rejected {
                        attempts: attempt_no,
                        status,
                    }
                }
                StatusClass::Retry if attempt_no >= policy.max_attempts => {
                    return Delivery::Exhausted {
                        attempts: attempt_no,
                        last: Attempt::Status(status),
                    }
                }
                StatusClass::Retry => {}
            },
            last @ Attempt::Transport(_) if attempt_no >= policy.max_attempts => {
                return Delivery::Exhausted {
                    attempts: attempt_no,
                    last,
                }
            }
            Attempt::Transport(_) => {}
        }
        tokio::select! {
            _ = tokio::time::sleep(policy.pause) => {}
            _ = interrupt.wait() => return Delivery::Interrupted { attempts: attempt_no },
        }
    }
}

/// Fires once: OCaml's per-iteration `interruptor`, filled when the next
/// iteration starts.
#[derive(Clone, Default)]
pub struct Interruptor(Arc<InterruptorInner>);

#[derive(Default)]
struct InterruptorInner {
    fired: AtomicBool,
    notify: Notify,
}

impl Interruptor {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn fire(&self) {
        self.0.fired.store(true, Ordering::SeqCst);
        self.0.notify.notify_waiters();
    }

    pub fn is_fired(&self) -> bool {
        self.0.fired.load(Ordering::SeqCst)
    }

    pub async fn wait(&self) {
        loop {
            // Registered before the check: `notify_waiters` wakes a
            // `Notified` from its creation on.
            let notified = self.0.notify.notified();
            if self.is_fired() {
                return;
            }
            notified.await;
        }
    }

    /// Runs `fut` unless this fires first.
    pub async fn run<T>(&self, fut: impl std::future::Future<Output = T>) -> Option<T> {
        tokio::select! {
            value = fut => Some(value),
            _ = self.wait() => None,
        }
    }
}

/// `scheme://host:port/path`, without user info, query or fragment, for
/// logs and the local status: a URL can carry a token.
pub fn redacted_url(url: &Url) -> String {
    let mut shown = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
    if let Some(port) = url.port() {
        shown.push_str(&format!(":{port}"));
    }
    shown.push_str(url.path());
    if url.query().is_some() || !url.username().is_empty() || url.password().is_some() {
        shown.push_str(" (credentials or query hidden)");
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the headers matter for all three reporters. A receiver can
    /// advertise an arbitrarily large body and never finish sending it.
    #[tokio::test]
    async fn response_body_is_not_read_for_any_reporter() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        for classify in [uptime_status_class, status_status_class, error_status_class] {
            for framing in [
                "Content-Length: 67108864\r\n",
                "Transfer-Encoding: chunked\r\n",
            ] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url: Url = format!("http://{}/", listener.local_addr().unwrap())
                    .parse()
                    .unwrap();
                let (release, wait) = tokio::sync::oneshot::channel::<()>();
                let server = tokio::spawn(async move {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut request = [0; 4096];
                    let _request_bytes = socket.read(&mut request).await.unwrap();
                    socket
                        .write_all(format!("HTTP/1.1 200 OK\r\n{framing}\r\n").as_bytes())
                        .await
                        .unwrap();
                    // No body bytes: no need to allocate a large response to
                    // prove the client must not wait for or collect it.
                    let _ = wait.await;
                });
                let http = client(Timeouts::default()).unwrap();
                let mut observe = |_, _: &Attempt| {};
                let interrupt = Interruptor::new();
                let result = tokio::time::timeout(
                    Duration::from_secs(1),
                    deliver(
                        &http,
                        &url,
                        bytes::Bytes::new(),
                        RetryPolicy::ONCE,
                        classify,
                        &interrupt,
                        &mut observe,
                    ),
                )
                .await;
                let _ = release.send(());
                server.await.unwrap();
                assert_eq!(
                    result.expect("delivery waited for unused response body"),
                    Delivery::Delivered { attempts: 1 }
                );
            }
        }
    }

    #[test]
    fn uptime_statuses() {
        assert_eq!(uptime_status_class(200), StatusClass::Success);
        for status in [400, 401, 411, 413] {
            assert_eq!(uptime_status_class(status), StatusClass::Terminal);
        }
        for status in [201, 204, 301, 404, 429, 500, 503] {
            assert_eq!(uptime_status_class(status), StatusClass::Retry);
        }
        assert_eq!(status_status_class(204), StatusClass::Success);
        assert_eq!(status_status_class(503), StatusClass::Terminal);
        assert_eq!(error_status_class(204), StatusClass::Terminal);
    }

    #[test]
    fn urls_are_shown_without_credentials() {
        let url: Url = "https://user:pw@example.com:8443/v1/submit?token=abc"
            .parse()
            .unwrap();
        let shown = redacted_url(&url);
        assert_eq!(
            shown,
            "https://example.com:8443/v1/submit (credentials or query hidden)"
        );
        assert!(!shown.contains("pw") && !shown.contains("abc"));
    }
}
