//! GraphQL endpoints.
//!
//! - `POST /graphql` - Execute GraphQL queries and mutations
//! - `GET /graphiql` - GraphiQL IDE (requires the `graphiql` feature, off by
//!   default: its page loads scripts from a CDN)
//!
//! Note: `/playground` is intentionally not included. GraphQL Playground was
//! deprecated and merged into GraphiQL 2.0, making them redundant.

use std::sync::Arc;

use axum::{
    extract::{DefaultBodyLimit, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use juniper::{
    http::{GraphQLBatchRequest, GraphQLRequest},
    EmptySubscription, RootNode,
};

use super::{
    graphql_body::{LargeBodies, LARGE_BODY_RECEIPT, LARGE_BODY_WAIT, LARGE_REQUEST_HOLD},
    graphql_limits::SizeLimits,
};
use crate::{
    graphql::{Context, Mutation, Query},
    http_server::AppState,
};

pub(crate) type Schema = RootNode<Query, Mutation, EmptySubscription<Context>>;

/// Registers GraphQL routes on the router, with the GraphiQL IDE when
/// `graphiql`: never on a public node, and not while worker control is on,
/// as its page loads scripts from a CDN (`super::super::CDN_PAGES`). A
/// `public` node keeps the smaller body limit (`SizeLimits::of`); a private
/// one admits one large body at a time (`super::graphql_body`).
pub fn routes(router: Router<AppState>, graphiql: bool, public: bool) -> Router<AppState> {
    let large = LargeBodies::new(LARGE_BODY_WAIT, LARGE_BODY_RECEIPT, LARGE_REQUEST_HOLD);
    routes_with(router, graphiql, public, large)
}

fn routes_with(
    router: Router<AppState>,
    graphiql: bool,
    public: bool,
    large: LargeBodies,
) -> Router<AppState> {
    let schema = Arc::new(Schema::new(Query, Mutation, EmptySubscription::new()));

    let graphql =
        post(graphql_handler).layer(DefaultBodyLimit::max(SizeLimits::of(public).body_bytes));
    let graphql = if public {
        graphql
    } else {
        graphql.layer(axum::middleware::from_fn_with_state(
            large,
            super::graphql_body::admit,
        ))
    };
    let router = router.route("/graphql", graphql);

    #[cfg(feature = "graphiql")]
    let router = if graphiql {
        router.route("/graphiql", get(graphiql_handler))
    } else {
        router
    };
    #[cfg(not(feature = "graphiql"))]
    let _ = graphiql;

    router.layer(Extension(schema))
}

/// Handles GraphQL POST requests.
async fn graphql_handler(
    State(state): State<AppState>,
    Extension(schema): Extension<Arc<Schema>>,
    Json(request): Json<GraphQLBatchRequest>,
) -> Response {
    let context = Context::new(state.rpc_sender().clone())
        .with_tx_submission(state.tx_submission())
        .with_public_graphql(state.public_graphql());
    let (status, response) = respond(&schema, &context, &request, state.public_graphql()).await;
    (status, Json(response)).into_response()
}

/// Answers a request as the handler does: the admission limits first (the
/// public ones when `public`), then the public policy when `public`, then,
/// for each request of a batch, OCaml's missing-variable check, Juniper's
/// execution, and OCaml's error shape.
pub(crate) async fn respond(
    schema: &Schema,
    context: &Context,
    request: &GraphQLBatchRequest,
    public: bool,
) -> (StatusCode, serde_json::Value) {
    if let Err(message) = super::graphql_limits::check(request, schema, SizeLimits::of(public)) {
        return (
            StatusCode::BAD_REQUEST,
            serde_json::json!({
                "errors": [{ "message": message }]
            }),
        );
    }
    if public {
        if let Err(refusal) = super::graphql_public::check(request, schema) {
            return (StatusCode::BAD_REQUEST, refusal.to_json());
        }
    }
    let response = match request {
        GraphQLBatchRequest::Single(request) => respond_one(schema, context, request).await,
        GraphQLBatchRequest::Batch(requests) => serde_json::Value::Array(
            juniper::futures::future::join_all(
                requests
                    .iter()
                    .map(|request| respond_one(schema, context, request)),
            )
            .await,
        ),
    };
    (StatusCode::OK, response)
}

/// The code of the public policy's refusal of a request, if it refuses it.
#[cfg(test)]
pub(crate) fn public_refusal(
    request: &GraphQLBatchRequest,
    schema: &Schema,
) -> Option<&'static str> {
    super::graphql_public::check(request, schema)
        .err()
        .map(|refusal| refusal.code)
}

async fn respond_one(
    schema: &Schema,
    context: &Context,
    request: &GraphQLRequest,
) -> serde_json::Value {
    if let Some(response) = super::graphql_ocaml::missing_variable(request, schema) {
        return response;
    }
    let response = request.execute(schema, context).await;
    match serde_json::to_value(&response) {
        Ok(response) => super::graphql_ocaml::post_process(response),
        Err(e) => serde_json::json!({ "errors": [{ "message": e.to_string() }] }),
    }
}

/// Serves the GraphiQL IDE.
#[cfg(feature = "graphiql")]
async fn graphiql_handler() -> impl IntoResponse {
    Html(juniper::http::graphiql::graphiql_source("/graphql", None))
}

#[cfg(test)]
mod tests {
    use super::{
        super::graphql_limits::{
            tests::{o1js_body, o1js_send_zkapp},
            PRIVATE, PUBLIC,
        },
        *,
    };
    use ledger::scan_state::transaction_logic::{zkapp_command::Control, UserCommand};
    use mina_node::{core::channels::mpsc, rpc::RpcRequest};
    use mina_node_common::rpc::{NodeRpcRequest, RpcSender};
    use serde_json::{json, Value};
    use std::time::Duration;

    async fn serve(app: Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/graphql", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (url, server)
    }

    /// A zkApp command close to the largest a Mesa node accepts, built from
    /// the 16-update command of `tests/files/zkapps`: 15 updates proved, all
    /// 32 app-state and precondition fields set, 1024 events and 1024 actions.
    fn large_command() -> Value {
        const FIELD: &str =
            "28948022309329048855892746252171976963363056481941560715954676764349967630336";
        let files =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tests/files/zkapps");
        let read = |name: &str| std::fs::read_to_string(files.join(name)).unwrap();
        let mut command: Value = serde_json::from_str(&read("with_sig_auth_graphql.json")).unwrap();
        let proof = read("proof_string.txt").trim().to_owned();
        let updates = command["accountUpdates"].as_array_mut().unwrap();
        assert_eq!(updates.len(), 16);
        for (i, update) in updates.iter_mut().enumerate() {
            update["body"]["update"]["appState"] = json!(vec![FIELD; 32]);
            update["body"]["preconditions"]["account"]["state"] = json!(vec![FIELD; 32]);
            if i > 0 {
                update["body"]["authorizationKind"]["isSigned"] = json!(false);
                update["body"]["authorizationKind"]["isProved"] = json!(true);
                update["authorization"] = json!({ "proof": proof, "signature": null });
            }
        }
        updates[0]["body"]["events"] = json!(vec![[FIELD]; 1024]);
        updates[0]["body"]["actions"] = json!(vec![[FIELD]; 1024]);
        command
    }

    /// `command` sent as a variable, not inline.
    fn variables_body(command: &Value) -> String {
        json!({
            "query": "mutation($input: SendZkappInput!) { sendZkapp(input: $input) { zkapp { hash } } }",
            "variables": { "input": { "zkappCommand": command } },
        })
        .to_string()
    }

    fn post(
        client: &reqwest::Client,
        url: &str,
        body: String,
    ) -> tokio::task::JoinHandle<reqwest::Result<reqwest::Response>> {
        tokio::spawn(
            client
                .post(url)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send(),
        )
    }

    /// The large command passes the body and admission limits and reaches the
    /// node as a command within the protocol's limits, in both forms: as a
    /// variable (0.8 MB of body), and as o1js sends it, inline in the mutation
    /// text (0.9 MB). The node then drops the request, so GraphQL answers with
    /// an error, not a refusal.
    #[tokio::test]
    async fn a_large_zkapp_mutation_reaches_the_node() {
        let command = large_command();
        let variables = variables_body(&command);
        let inline = o1js_body(&o1js_send_zkapp(&command));

        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let state = AppState::new(RpcSender::new(tx)).with_tx_submission(true);
        let (url, server) = serve(routes(Router::new(), false, false).with_state(state)).await;
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap();
        for (form, body) in [("variables", variables), ("inline", inline)] {
            let body_bytes = body.len();
            assert!(
                body_bytes > 2 * PUBLIC.body_bytes,
                "{form}: {body_bytes} bytes"
            );
            let started = std::time::Instant::now();
            let mut sent = post(&client, &url, body);
            let NodeRpcRequest { req, responder } = tokio::select! {
                request = rx.recv() => request.unwrap(),
                response = &mut sent => {
                    let response = response.unwrap().unwrap();
                    let status = response.status();
                    panic!("{form}: refused before the node: {status} {}", response.text().await.unwrap());
                }
            };
            println!(
                "{form}, {body_bytes}-byte body: sent to node channel in {:?}",
                started.elapsed()
            );
            let RpcRequest::TransactionInject(commands) = req else {
                panic!("{form}: expected a transaction injection");
            };
            let [command] = commands.as_slice() else {
                panic!("{form}: expected one command, got {}", commands.len());
            };
            let UserCommand::ZkAppCommand(command) = UserCommand::try_from(command).unwrap() else {
                panic!("{form}: expected a zkApp command");
            };
            let proofs = command.account_updates.fold(0, |n, update| {
                n + usize::from(matches!(update.authorization, Control::Proof(_)))
            });
            assert_eq!(proofs, 15, "{form}");
            command.valid_size().unwrap();
            drop(responder);

            let response = sent.await.unwrap().unwrap();
            assert_eq!(response.status(), StatusCode::OK, "{form}");
            let response: Value = response.json().await.unwrap();
            assert!(response.get("errors").is_some(), "{form}: {response}");
            assert!(
                !response.to_string().contains("exceeds"),
                "{form}: {response}"
            );
        }
        server.abort();
        let _ = server.await;
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .unwrap()
    }

    async fn reaches_node(
        rx: &mut mpsc::Receiver<NodeRpcRequest>,
        within: std::time::Duration,
    ) -> NodeRpcRequest {
        tokio::time::timeout(within, rx.recv())
            .await
            .expect("the request never reached the node")
            .unwrap()
    }

    /// A raw connection that has sent `bytes`: the read half, and the write
    /// half, kept open until the task is dropped.
    async fn send_raw(
        url: &str,
        bytes: Vec<u8>,
    ) -> (
        tokio::net::tcp::OwnedReadHalf,
        tokio::task::JoinHandle<tokio::net::tcp::OwnedWriteHalf>,
    ) {
        use tokio::io::AsyncWriteExt;
        let stream = tokio::net::TcpStream::connect(address(url)).await.unwrap();
        let (read, mut write) = stream.into_split();
        let writing = tokio::spawn(async move {
            let _ = write.write_all(&bytes).await;
            write
        });
        (read, writing)
    }

    /// What the server answers on a raw connection, until it closes it.
    async fn answer(read: &mut tokio::net::tcp::OwnedReadHalf) -> String {
        use tokio::io::AsyncReadExt;
        let mut answer = Vec::new();
        let _ = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            read.read_to_end(&mut answer),
        )
        .await;
        String::from_utf8_lossy(&answer).into_owned()
    }

    fn address(url: &str) -> &str {
        url.trim_start_matches("http://")
            .trim_end_matches("/graphql")
    }

    /// The head of a JSON POST to `/graphql` with `framing` headers, asking
    /// the server to close the connection after its answer.
    fn head(url: &str, framing: &str) -> String {
        format!(
            "POST /graphql HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\n{framing}Connection: close\r\n\r\n",
            address(url)
        )
    }

    /// Sends a request's head, declaring a body of `length`, and none of the
    /// body; returns what the server answers.
    async fn head_only(url: &str, length: usize) -> String {
        let head = head(url, &format!("Content-Length: {length}\r\n"));
        let (mut read, _write) = send_raw(url, head.into_bytes()).await;
        answer(&mut read).await
    }

    /// A client that sends the head of a large request and one byte of its
    /// body, then stalls with its connection open, loses the permit at the
    /// receipt deadline: a later large command reaches the node meanwhile.
    #[tokio::test]
    async fn a_stalled_large_body_loses_the_permit_at_its_receipt_deadline() {
        use super::super::graphql_body::SMALL_BODY_BYTES;
        let (wait, receipt) = (Duration::from_secs(5), Duration::from_secs(1));
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let state = AppState::new(RpcSender::new(tx)).with_tx_submission(true);
        let large = LargeBodies::new(wait, receipt, LARGE_REQUEST_HOLD);
        let (url, server) =
            serve(routes_with(Router::new(), false, false, large).with_state(state)).await;

        let stall = head(
            &url,
            &format!("Content-Length: {}\r\n", SMALL_BODY_BYTES + 1),
        ) + "{";
        let (mut stalled, open) = send_raw(&url, stall.into_bytes()).await;
        tokio::time::sleep(receipt / 4).await;

        let started = std::time::Instant::now();
        let sent = post(&client(), &url, variables_body(&large_command()));
        drop(reaches_node(&mut rx, wait).await);
        assert!(
            started.elapsed() >= receipt / 2,
            "the stalled client never held the permit"
        );
        assert_eq!(sent.await.unwrap().unwrap().status(), StatusCode::OK);

        let answer = answer(&mut stalled).await;
        assert!(answer.starts_with("HTTP/1.1 400"), "{answer}");
        assert!(answer.contains("not received within 1 s"), "{answer}");
        open.abort();
        server.abort();
        let _ = server.await;
    }

    /// A large request the node doesn't answer gives the permit up at the
    /// hold deadline, with HTTP 504; the next large command then reaches the
    /// node.
    #[tokio::test]
    async fn a_large_request_gives_the_permit_up_at_the_hold_deadline() {
        let (wait, hold) = (Duration::from_secs(5), Duration::from_secs(1));
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let state = AppState::new(RpcSender::new(tx)).with_tx_submission(true);
        let large = LargeBodies::new(wait, LARGE_BODY_RECEIPT, hold);
        let (url, server) =
            serve(routes_with(Router::new(), false, false, large).with_state(state)).await;
        let client = client();
        let body = variables_body(&large_command());

        let first = post(&client, &url, body.clone());
        let unanswered = reaches_node(&mut rx, Duration::from_secs(10)).await;
        let second = post(&client, &url, body);
        drop(reaches_node(&mut rx, wait).await);
        let first = first.await.unwrap().unwrap();
        assert_eq!(first.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(first.text().await.unwrap().contains("took longer than 1 s"));
        assert_eq!(second.await.unwrap().unwrap().status(), StatusCode::OK);
        drop(unanswered);
        server.abort();
        let _ = server.await;
    }

    /// A chunked body is large whatever its `Content-Length` says (Hyper
    /// keeps a stale one beside the chunked framing): while the permit is
    /// held, one declaring `Content-Length: 1` waits, then gets 503, and
    /// never reaches the node.
    #[tokio::test]
    async fn a_chunked_body_is_large_whatever_its_content_length() {
        let wait = Duration::from_secs(1);
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let state = AppState::new(RpcSender::new(tx)).with_tx_submission(true);
        let large = LargeBodies::new(wait, LARGE_BODY_RECEIPT, LARGE_REQUEST_HOLD);
        let (url, server) =
            serve(routes_with(Router::new(), false, false, large).with_state(state)).await;
        let body = variables_body(&large_command());

        let first = post(&client(), &url, body.clone());
        let held = reaches_node(&mut rx, Duration::from_secs(10)).await;

        let framing = "Content-Length: 1\r\nTransfer-Encoding: chunked\r\n";
        let chunked = format!(
            "{}{:x}\r\n{body}\r\n0\r\n\r\n",
            head(&url, framing),
            body.len()
        );
        let (mut read, writing) = send_raw(&url, chunked.into_bytes()).await;
        let answer = tokio::select! {
            _ = rx.recv() => panic!("a chunked body reached the node while the permit was held"),
            answer = answer(&mut read) => answer,
        };
        assert!(answer.starts_with("HTTP/1.1 503"), "{answer}");
        drop(held);
        assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);
        writing.abort();
        server.abort();
        let _ = server.await;
    }

    /// While a large body holds the permit (here, waiting for the node to
    /// answer), small bodies are answered; another large body waits, and is
    /// refused with 503 if the first takes longer than the wait. One still
    /// waiting goes on as soon as the first is answered.
    #[tokio::test]
    async fn one_large_body_at_a_time_and_small_bodies_never_wait() {
        use super::super::graphql_body::SMALL_BODY_BYTES;
        let wait = std::time::Duration::from_secs(1);
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(4);
        let state = AppState::new(RpcSender::new(tx)).with_tx_submission(true);
        let app = routes_with(
            Router::new(),
            false,
            false,
            LargeBodies::new(wait, LARGE_BODY_RECEIPT, LARGE_REQUEST_HOLD),
        );
        let (url, server) = serve(app.with_state(state)).await;
        let client = client();
        let large = variables_body(&large_command());
        assert!(large.len() > SMALL_BODY_BYTES);

        let first = post(&client, &url, large.clone());
        let held = reaches_node(&mut rx, std::time::Duration::from_secs(10)).await;

        let small = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client
                .post(&url)
                .json(&json!({ "query": "{ version }" }))
                .send(),
        )
        .await
        .expect("a small body waited for the large one")
        .unwrap();
        assert_eq!(small.status(), StatusCode::OK);

        // The refusal comes before the body is read, so this client sends
        // only the head of its request and reads the answer.
        let started = std::time::Instant::now();
        let refused = head_only(&url, large.len()).await;
        assert!(started.elapsed() >= wait);
        assert!(refused.starts_with("HTTP/1.1 503"), "{refused}");
        assert!(refused.contains("in progress"), "{refused}");
        assert!(rx.try_recv().is_err());

        let waiting = post(&client, &url, large);
        tokio::time::sleep(wait / 4).await;
        assert!(rx.try_recv().is_err(), "two large bodies at once");
        drop(held);
        assert_eq!(first.await.unwrap().unwrap().status(), StatusCode::OK);
        drop(reaches_node(&mut rx, std::time::Duration::from_secs(10)).await);
        assert_eq!(waiting.await.unwrap().unwrap().status(), StatusCode::OK);
        server.abort();
        let _ = server.await;
    }

    /// A large body gives the permit back however it ends: not JSON, too many
    /// JSON values, over the body limit, or cut off by the client. The next
    /// large body then gets it at once, well within its wait.
    #[tokio::test]
    async fn a_large_body_gives_the_permit_back_on_every_path() {
        use super::super::graphql_body::{MAX_JSON_VALUES, SMALL_BODY_BYTES};
        use tokio::io::AsyncWriteExt;
        let wait = std::time::Duration::from_secs(5);
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let state = AppState::new(RpcSender::new(tx)).with_tx_submission(true);
        let app = routes_with(
            Router::new(),
            false,
            false,
            LargeBodies::new(wait, LARGE_BODY_RECEIPT, LARGE_REQUEST_HOLD),
        );
        let (url, server) = serve(app.with_state(state)).await;
        let client = client();

        let not_json = "x".repeat(SMALL_BODY_BYTES + 1);
        // The values come last, so the server has read the body when it
        // refuses it.
        let values = format!(
            r#"{{"query":"{{ version }}","variables":{{"pad":"{}","x":[{}0]}}}}"#,
            "x".repeat(SMALL_BODY_BYTES),
            "0,".repeat(MAX_JSON_VALUES)
        );
        let over = format!(
            r#"{{"query":"{{ version }}","pad":"{}"}}"#,
            "x".repeat(PRIVATE.body_bytes)
        );
        for (body, status, text) in [
            (not_json, StatusCode::BAD_REQUEST, "JSON"),
            (values, StatusCode::BAD_REQUEST, "JSON values"),
            (over, StatusCode::PAYLOAD_TOO_LARGE, "length limit exceeded"),
        ] {
            let response = post(&client, &url, body).await.unwrap().unwrap();
            assert_eq!(response.status(), status);
            assert!(response.text().await.unwrap().contains(text));
        }
        // A client that disconnects halfway through its body.
        let address = url
            .trim_start_matches("http://")
            .trim_end_matches("/graphql");
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                format!(
                    "POST /graphql HTTP/1.1\r\nHost: {address}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{{\"query\":\"",
                    PRIVATE.body_bytes
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        stream
            .write_all(&vec![b'x'; SMALL_BODY_BYTES])
            .await
            .unwrap();
        drop(stream);

        let started = std::time::Instant::now();
        let sent = post(&client, &url, variables_body(&large_command()));
        drop(reaches_node(&mut rx, wait / 2).await);
        assert!(started.elapsed() < wait / 2);
        assert_eq!(sent.await.unwrap().unwrap().status(), StatusCode::OK);
        server.abort();
        let _ = server.await;
    }

    /// A body of more JSON values than the token budget is refused before
    /// Juniper decodes it, small bodies included; one at the budget is not.
    #[tokio::test]
    async fn a_body_of_too_many_json_values_is_refused_before_decoding() {
        use super::super::graphql_body::MAX_JSON_VALUES;
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let app = routes(Router::new(), false, false);
        let (url, server) = serve(app.with_state(AppState::new(RpcSender::new(tx)))).await;
        let client = client();
        // `{ : , : { : [` and one `,` between each pair of elements: 6 + n.
        let body = |elements: usize| {
            format!(
                r#"{{"query":"{{ version }}","variables":{{"x":[{}0]}}}}"#,
                "0,".repeat(elements - 1)
            )
        };
        let response = post(&client, &url, body(MAX_JSON_VALUES - 6))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let response = post(&client, &url, body(MAX_JSON_VALUES - 5))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains(&format!("more than {MAX_JSON_VALUES} JSON values")));
        assert!(rx.try_recv().is_err());
        server.abort();
        let _ = server.await;
    }

    /// A public node refuses mutations, so it keeps the earlier 256 KiB body
    /// limit.
    #[tokio::test]
    async fn a_public_node_keeps_the_earlier_body_limit() {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let state = AppState::new(RpcSender::new(tx)).with_public_graphql(true);
        let (url, server) = serve(routes(Router::new(), false, true).with_state(state)).await;
        let response = reqwest::Client::new()
            .post(&url)
            .json(&json!({
                "query": "{ version }", "variables": { "padding": "x".repeat(PUBLIC.body_bytes) }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(rx.try_recv().is_err());
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn http_rejects_flood_and_oversize_before_rpc() {
        let (tx, mut rx) = mpsc::channel::<NodeRpcRequest>(1);
        let app = routes(Router::new(), true, false).with_state(AppState::new(RpcSender::new(tx)));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/graphql", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let query = format!(
            "{{ {} }}",
            (0..1000)
                .map(|i| format!("v{i}: verification {{ verifiedSinceStart }}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        let response = client
            .post(&url)
            .json(&serde_json::json!({"query": query}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response.text().await.unwrap().contains("root field count"));
        let response = client
            .post(&url)
            .json(&serde_json::json!({
                "query": "{ version }", "variables": { "padding": "x".repeat(PRIVATE.body_bytes) }
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(response
            .text()
            .await
            .unwrap()
            .contains("length limit exceeded"));
        let response = client
            .post(&url)
            .json(&serde_json::json!({"query": "{ version }"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response
            .json::<serde_json::Value>()
            .await
            .unwrap()
            .get("data")
            .is_some());
        assert!(
            rx.try_recv().is_err(),
            "rejected requests must not reach the node"
        );
        server.abort();
        let _ = server.await;
    }
}
