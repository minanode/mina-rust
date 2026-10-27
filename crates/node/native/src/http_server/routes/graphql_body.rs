//! What a private node's `POST /graphql` lets through before Juniper decodes
//! the JSON body. Juniper decodes variables into 80-byte values before any
//! other check (about 40 times the size of a body of short numbers) and
//! clones them again, and the token budget doesn't count them. So:
//!
//! - one large body at a time: a body that may exceed [`SMALL_BODY_BYTES`]
//!   holds a single permit from before it is read until its response is
//!   built. Another large body waits up to [`LARGE_BODY_WAIT`], then gets HTTP
//!   503. A body is small only by a `Content-Length` within the limit and no
//!   `Transfer-Encoding` (Hyper keeps a stale `Content-Length` beside a
//!   chunked body); small bodies never wait;
//! - deadlines for the holder: its body must arrive within
//!   [`LARGE_BODY_RECEIPT`] (HTTP 400 otherwise), and it gives the permit up
//!   after [`LARGE_REQUEST_HOLD`] in all (HTTP 504), as it does on every other
//!   path: an answer, an error, a client that disconnects;
//! - at most [`MAX_JSON_VALUES`] JSON values in any body, counted as the
//!   stream arrives, before anything is decoded.
//!
//! A public node keeps its 256 KiB body limit and has none of these.
use std::{sync::Arc, time::Duration};

use axum::{
    body::{Body, BodyDataStream},
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use futures::StreamExt;
use tokio::{sync::Semaphore, time::Instant};

use super::graphql_limits::{PRIVATE, PUBLIC};

/// Bodies up to the public limit never wait.
pub(super) const SMALL_BODY_BYTES: usize = PUBLIC.body_bytes;
/// JSON values in one body, a batch's included: the token budget. The
/// largest zkApp command as a variable has 12,955; one o1js sends inline, 7.
pub(super) const MAX_JSON_VALUES: usize = PRIVATE.tokens;
/// How long a large body waits for the one in progress: no less than
/// [`LARGE_BODY_RECEIPT`], so one stalled client can't make it give up.
pub(super) const LARGE_BODY_WAIT: Duration = Duration::from_secs(30);
/// How long the holder's body may take to arrive: 1.25 MiB at 42 kB/s.
pub(super) const LARGE_BODY_RECEIPT: Duration = Duration::from_secs(30);
/// How long the holder may keep the permit in all. The node answers
/// `sendZkapp` after verifying its proofs, at 16 to 18 s of CPU each on x86
/// (P14d): about 4.5 minutes for 15, one after another, and several times
/// that on a slow ARM core.
pub(super) const LARGE_REQUEST_HOLD: Duration = Duration::from_secs(20 * 60);

/// The permit large bodies share, and its deadlines.
#[derive(Clone)]
pub(super) struct LargeBodies {
    permit: Arc<Semaphore>,
    wait: Duration,
    receipt: Duration,
    hold: Duration,
}

impl LargeBodies {
    pub(super) fn new(wait: Duration, receipt: Duration, hold: Duration) -> Self {
        Self {
            permit: Arc::new(Semaphore::new(1)),
            wait,
            receipt,
            hold,
        }
    }
}

/// Middleware for the private `POST /graphql`: the permit for a large body,
/// then the JSON value count, and for the holder its deadlines.
pub(super) async fn admit(
    State(large): State<LargeBodies>,
    request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let small = !headers.contains_key(header::TRANSFER_ENCODING)
        && headers
            .get(header::CONTENT_LENGTH)
            .and_then(|length| length.to_str().ok()?.parse::<usize>().ok())
            .is_some_and(|length| length <= SMALL_BODY_BYTES);
    if small {
        return next.run(counted(request, None)).await;
    }
    let Ok(Ok(_permit)) = tokio::time::timeout(large.wait, large.permit.acquire_owned()).await
    else {
        return busy();
    };
    let request = counted(request, Some(large.receipt));
    match tokio::time::timeout(large.hold, next.run(request)).await {
        Ok(response) => response,
        Err(_) => too_long(large.hold),
    }
}

/// `request` with its body counted for JSON values, and cut off with an
/// error if it hasn't all arrived within `receipt`.
fn counted(request: Request, receipt: Option<Duration>) -> Request {
    struct Incoming {
        chunks: BodyDataStream,
        values: JsonValues,
        deadline: Option<(Instant, Duration)>,
    }
    let (parts, body) = request.into_parts();
    let incoming = Incoming {
        chunks: body.into_data_stream(),
        values: JsonValues::default(),
        deadline: receipt.map(|receipt| (Instant::now() + receipt, receipt)),
    };
    let body = futures::stream::unfold(Some(incoming), |incoming| async move {
        let mut incoming = incoming?;
        let chunk = match incoming.deadline {
            None => incoming.chunks.next().await,
            Some((deadline, receipt)) => {
                match tokio::time::timeout_at(deadline, incoming.chunks.next()).await {
                    Ok(chunk) => chunk,
                    Err(_) => {
                        let error: axum::BoxError = Box::new(BodyTooSlow(receipt));
                        return Some((Err(error), None));
                    }
                }
            }
        };
        let chunk: Result<_, axum::BoxError> = match chunk? {
            Ok(bytes) => match incoming.values.feed(&bytes) {
                Ok(()) => Ok(bytes),
                Err(error) => Err(error.into()),
            },
            Err(error) => Err(error.into()),
        };
        let incoming = chunk.is_ok().then_some(incoming);
        Some((chunk, incoming))
    });
    Request::from_parts(parts, Body::from_stream(body))
}

#[derive(Debug, thiserror::Error)]
#[error("GraphQL request body not received within {} s", .0.as_secs())]
struct BodyTooSlow(Duration);

fn too_long(hold: Duration) -> Response {
    (
        StatusCode::GATEWAY_TIMEOUT,
        Json(serde_json::json!({ "errors": [{
            "message": format!(
                "the GraphQL request took longer than {} s; a transaction it sent may still be processed",
                hold.as_secs()
            )
        }] })),
    )
        .into_response()
}

fn busy() -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "5")],
        Json(serde_json::json!({ "errors": [{
            "message": "another large GraphQL request is in progress; retry later"
        }] })),
    )
        .into_response()
}

/// Counts JSON values as bytes arrive: every value but the first follows a
/// `[`, `{`, `,` or `:` outside a string, so their number bounds the values
/// and object keys a decoder makes. Malformed JSON is the decoder's to refuse.
#[derive(Default)]
struct JsonValues {
    count: usize,
    in_string: bool,
    escaped: bool,
}

#[derive(Debug, thiserror::Error)]
#[error("GraphQL request body has more than {MAX_JSON_VALUES} JSON values")]
struct TooManyJsonValues;

impl JsonValues {
    fn feed(&mut self, bytes: &[u8]) -> Result<(), TooManyJsonValues> {
        for &byte in bytes {
            if self.in_string {
                if self.escaped {
                    self.escaped = false;
                } else if byte == b'\\' {
                    self.escaped = true;
                } else if byte == b'"' {
                    self.in_string = false;
                }
            } else if byte == b'"' {
                self.in_string = true;
            } else if matches!(byte, b'[' | b'{' | b',' | b':') {
                self.count += 1;
                if self.count > MAX_JSON_VALUES {
                    return Err(TooManyJsonValues);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count(chunks: &[&str]) -> Result<usize, TooManyJsonValues> {
        let mut values = JsonValues::default();
        for chunk in chunks {
            values.feed(chunk.as_bytes())?;
        }
        Ok(values.count)
    }

    #[test]
    fn json_values_are_counted_outside_strings_across_chunks() {
        assert_eq!(count(&[r#"{"a":[1,2,{"b":3}]}"#]).unwrap(), 7);
        // Punctuation and escaped quotes inside strings are not values.
        assert_eq!(
            count(&[r#"{"q":"{ f(a: [1,2]) } \"x,y\" \\"}"#]).unwrap(),
            2
        );
        // A chunk boundary inside a string or an escape changes nothing.
        assert_eq!(
            count(&[r#"{"q":"a\"#, r#"",[b"#, r#"]","n":[1,2]}"#]).unwrap(),
            6
        );
        let at_limit = format!("[{}0]", "0,".repeat(MAX_JSON_VALUES - 1));
        assert_eq!(count(&[&at_limit]).unwrap(), MAX_JSON_VALUES);
        let over = format!("[{}0]", "0,".repeat(MAX_JSON_VALUES));
        assert!(count(&[&over]).is_err());
    }

    /// The largest zkApp commands o1js builds, sent as a variable, fit with
    /// a margin of a fifth.
    #[test]
    fn the_largest_zkapp_commands_as_variables_fit() {
        use super::super::graphql_limits::tests::worst_case;
        for (proved, signed, expected) in [(15, 1, 8_680), (0, 31, 12_955)] {
            let body = serde_json::json!({
                "query": "mutation($input: SendZkappInput!) { sendZkapp(input: $input) { zkapp { hash } } }",
                "variables": { "input": { "zkappCommand": worst_case(proved, signed) } },
            })
            .to_string();
            let values = count(&[&body]).unwrap();
            assert_eq!(values, expected);
            assert!(values * 6 / 5 < MAX_JSON_VALUES);
        }
    }
}
