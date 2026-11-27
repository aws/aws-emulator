// SPDX-License-Identifier: CC0-1.0

//! A GraphQL API's REAL-TIME endpoint, driven over a real socket: the address the API
//! publishes for it, the opening handshake it completes, the conversation it carries and
//! the `AWS/AppSync` connect series it publishes.
//!
//! The rule these cases assert, one level above the recorded case (one API created with
//! `API_KEY`, a subscription-bearing schema, and a client dialling `uris.REALTIME`):
//!
//! > A GraphQL API publishes, as its `REALTIME` endpoint, the address of the socket that
//! > endpoint is SERVED on — `dns.REALTIME` the `host:port` and `uris.REALTIME` the
//! > `wss://` URL over it — so a client that dials what the API published reaches this
//! > emulator. There it completes an RFC 6455 opening handshake on `/graphql` offering the
//! > `graphql-ws` subprotocol, and one addressed to anything else is refused. Over the
//! > established socket `connection_init` is answered `connection_ack` when the material
//! > the connection carries authorizes a connection to the API and `connection_error`
//! > otherwise — which ends the connection — a `start` on a subscription field of the
//! > API's own schema `start_ack`, and the `stop` that ends one `complete`. Each connect is
//! > reported in CloudWatch's `AWS/AppSync` namespace under `[GraphQLAPIId]`:
//! > `ConnectSuccess` for one this endpoint established, `ConnectClientError` for one it
//! > refused.
//!
//! The rows span what the rule generalises over: the material a handshake presents (a live
//! key, none, an unknown key, a key of another API), what it addresses (the endpoint's path
//! and another, its subprotocol and none), where a browser made it from, and what a `start`
//! names (a declared subscription field, an undeclared one, no document, no id, an id
//! already in use).

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::{
    appsync, cloudwatch, emulator, encoded_auth_object, loopback_port, CLIENT_KEY,
};

/// How long a case waits for one frame before giving up: generous for a loaded test
/// machine, and bounded so a case that will never be answered fails instead of hanging.
const FRAME_DEADLINE: Duration = Duration::from_secs(10);

/// How long a case waits for the clock to perform a schema creation, and how often it looks
/// again — the span is the service's own (`state::SCHEMA_CREATION_SPAN`), and the deadline
/// is far past it so a loaded machine still reaches it.
const CREATION_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The status a completed opening handshake carries (RFC 6455 §4.1).
const SWITCHING_PROTOCOLS: u16 = 101;

/// The subprotocol a GraphQL API's real-time client offers; the service's own copy is
/// `service::graphql_api::realtime::SUBPROTOCOL`.
const SUBPROTOCOL: &str = "graphql-ws";

/// The path that endpoint serves its WebSocket at.
const REALTIME_PATH: &str = "/graphql";

/// The schema every API of this suite is created with: a subscription-bearing document, as
/// the recorded case carries.
const SCHEMA: &str = "type Post {\n  id: ID!\n  title: String\n}\n\
                      \n\
                      type Query {\n  post(id: ID!): Post\n}\n\
                      \n\
                      type Mutation {\n  addPost(title: String!): Post\n}\n\
                      \n\
                      type Subscription {\n  onAddPost: Post\n  onDeletePost: Post\n}\n";

/// The subscription fields [`SCHEMA`] declares, and one it does not.
const DECLARED_FIELDS: [&str; 2] = ["onAddPost", "onDeletePost"];
const UNDECLARED_FIELD: &str = "onRenamePost";

/// One created GraphQL API: its id, the address its real-time endpoint is served on, and a
/// live API key.
struct GraphqlApi {
    api_id: String,
    realtime: String,
    api_key: String,
}

/// The socket a client gets from a completed handshake.
pub(crate) type ClientSocket = tokio_tungstenite::WebSocketStream<TcpStream>;

#[tokio::test]
async fn the_published_realtime_endpoint_is_the_socket_this_emulator_serves_it_on() {
    let router = emulator();
    let api = create_api(&router, "published").await;

    // The published address is one this emulator answers on, and the URL is built over it.
    let case = "the published realtime endpoint";
    let (status, fetched) = appsync(
        &router,
        "GET",
        &format!("/v1/apis/{}", api.api_id),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetGraphqlApi must answer 200, got {fetched}");
    let published =
        crate::graphql_api_endpoint_dns_records::published_authority(&fetched["graphqlApi"], "REALTIME");
    assert_eq!(
        published, api.realtime,
        "case {case:?}: every read must report the address the create published, got \
         {published:?} and {:?}",
        api.realtime
    );
    assert_eq!(
        fetched["graphqlApi"]["uris"]["REALTIME"].as_str(),
        Some(format!("wss://{published}{REALTIME_PATH}").as_str()),
        "case {case:?}: uris.REALTIME must be the WebSocket URL over the published \
         address, got {fetched}"
    );
    assert_eq!(
        fetched["graphqlApi"]["dns"]["REALTIME"].as_str(),
        Some(::service_common::data_plane_addressing::host_only(&published)),
        "case {case:?}: dns.REALTIME must be the bare host of that URL, got {fetched}"
    );

    // And a client that dials it is served the conversation: the handshake completes, the
    // init is acked, a start on a declared subscription field is acked, and the stop that
    // ends it completes.
    let mut socket = connected(case, &api).await;
    for field in DECLARED_FIELDS {
        let case = format!("a start on the declared field {field}");
        let id = format!("sub-{field}");
        let started = exchange(&case, &mut socket, &start_message(&id, field)).await;
        assert_eq!(
            started.as_ref().map(|frame| frame["type"].clone()),
            Some(json!("start_ack")),
            "case {case:?}: a start on a subscription field of the API's schema must be \
             answered start_ack, got {started:?}"
        );
        let stopped = exchange(
            &case,
            &mut socket,
            &json!({"id": id, "type": "stop"}),
        )
        .await;
        assert_eq!(
            stopped.as_ref().map(|frame| frame["type"].clone()),
            Some(json!("complete")),
            "case {case:?}: the stop that ends it must be answered complete, got {stopped:?}"
        );
    }

    // One connect was established, and nothing was refused.
    let attempted = metric_sum(&router, "ConnectRequests", &api.api_id).await;
    assert_eq!(
        attempted,
        Some(1.0),
        "case {case:?}: expected one ConnectRequests under [GraphQLAPIId], got {attempted:?}"
    );
    let established = metric_sum(&router, "ConnectSuccess", &api.api_id).await;
    assert_eq!(
        established,
        Some(1.0),
        "case {case:?}: expected one ConnectSuccess under [GraphQLAPIId], got {established:?}"
    );
    let refused = metric_sum(&router, "ConnectClientError", &api.api_id).await;
    assert_eq!(
        refused, None,
        "case {case:?}: nothing was refused, so no ConnectClientError may be reported, got \
         {refused:?}"
    );
}

#[tokio::test]
async fn a_handshake_is_completed_only_for_what_this_endpoint_serves_and_authorizes() {
    let router = emulator();
    let api = create_api(&router, "admission").await;
    let other = create_api(&router, "another").await;

    // A handshake addressed to something this endpoint does not serve is refused by the
    // handshake itself. The rows span what it reads: the path addressed, the subprotocol
    // offered, and the fields a browser sets on a cross-site handshake.
    struct Transport {
        case: &'static str,
        path: &'static str,
        /// Whether the handshake offers this endpoint's subprotocol.
        offers_subprotocol: bool,
        /// The request fields the handshake carries beside the ones RFC 6455 requires.
        fields: &'static [(&'static str, &'static str)],
        /// The status the refusal must carry.
        expected: u16,
    }
    let transport: &[Transport] = &[
        Transport {
            case: "a path this endpoint does not serve",
            path: "/event/realtime",
            offers_subprotocol: true,
            fields: &[],
            expected: 404,
        },
        Transport {
            case: "no subprotocol offered",
            path: REALTIME_PATH,
            offers_subprotocol: false,
            fields: &[],
            expected: 400,
        },
        Transport {
            case: "a handshake a browser made from another site",
            path: REALTIME_PATH,
            offers_subprotocol: true,
            fields: &[("origin", "https://attacker.example"), ("sec-fetch-site", "cross-site")],
            expected: 403,
        },
    ];
    for row in transport {
        let case = row.case;
        let answered = handshake(
            case,
            &api.realtime,
            row.path,
            row.offers_subprotocol,
            Some(&json!({"x-api-key": api.api_key})),
            row.fields,
        )
        .await;
        assert_eq!(
            answered.err(),
            Some(row.expected),
            "case {case:?}: expected the handshake to be refused {}, got a completed one or \
             another status",
            row.expected
        );
    }

    // Material that authorizes no connection is refused AFTER the handshake, at the
    // `connection_init` the connection is judged on, and the connection then ends. The rows
    // span what a client can present: nothing, a key nothing minted, and a live key of
    // another API.
    //
    // Each row builds its authorization object from the two APIs this case holds.
    type Presented = fn(&GraphqlApi, &GraphqlApi) -> Option<Value>;
    let unauthorized: &[(&str, Presented)] = &[
        ("no material at all", |_, _| None),
        ("an api key no CreateApiKey ever minted", |_, _| {
            Some(json!({"x-api-key": "da2-never-minted"}))
        }),
        ("a live api key of ANOTHER api", |_, other| {
            Some(json!({"x-api-key": other.api_key}))
        }),
    ];
    for (case, header) in unauthorized {
        let header = header(&api, &other);
        let mut socket = handshake(
            case,
            &api.realtime,
            REALTIME_PATH,
            true,
            header.as_ref(),
            &[],
        )
        .await
        .unwrap_or_else(|status| {
            panic!(
                "case {case:?}: the opening handshake is completed before the material is \
                 judged, got {status}"
            )
        });
        let refused = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
        let refused = refused
            .unwrap_or_else(|| panic!("case {case:?}: connection_init must be answered"));
        assert_eq!(
            (
                refused["type"].as_str(),
                refused
                    .pointer("/payload/errors/0/errorType")
                    .and_then(Value::as_str),
                refused
                    .pointer("/payload/errors/0/errorCode")
                    .and_then(Value::as_i64),
            ),
            (Some("connection_error"), Some("UnauthorizedException"), Some(401)),
            "case {case:?}: an unauthorized connection_init must be answered a \
             connection_error carrying UnauthorizedException and its 401, got {refused}"
        );
        assert_eq!(
            next_frame(&mut socket).await,
            None,
            "case {case:?}: the connection ends with the refusal, so nothing more is served \
             over it"
        );
    }

    // Every refused connect is reported, and nothing was established.
    let refusals = (transport.len() + unauthorized.len()) as f64;
    let attempted = metric_sum(&router, "ConnectRequests", &api.api_id).await;
    assert_eq!(
        attempted,
        Some(refusals),
        "expected one ConnectRequests per attempt under [GraphQLAPIId], got {attempted:?}"
    );
    let refused = metric_sum(&router, "ConnectClientError", &api.api_id).await;
    assert_eq!(
        refused,
        Some(refusals),
        "expected one ConnectClientError per refused connect under [GraphQLAPIId], got \
         {refused:?}"
    );
    let established = metric_sum(&router, "ConnectSuccess", &api.api_id).await;
    assert_eq!(
        established, None,
        "no connect was established, so no ConnectSuccess may be reported, got \
         {established:?}"
    );
}

#[tokio::test]
async fn a_start_is_recorded_only_for_a_subscription_the_api_declares() {
    let router = emulator();
    let api = create_api(&router, "starts").await;
    let case = "the conversation's own refusals";
    let mut socket = connected(case, &api).await;

    // An id already in use needs one in use: this start is the one that takes it.
    let taken = exchange(case, &mut socket, &start_message("in-use", DECLARED_FIELDS[0])).await;
    assert_eq!(
        taken.as_ref().map(|frame| frame["type"].clone()),
        Some(json!("start_ack")),
        "case {case:?}: the start that takes the id must be answered start_ack, got {taken:?}"
    );

    // The rows span what a start can get wrong, and the frame each is answered with. The
    // status beside each error type is that error's own modelled `httpError`:
    // `BadRequestException` 400, `NotFoundException` 404.
    let cases: &[(&str, Value, &str, &str, i64)] = &[
        (
            "a start naming no id",
            start_message("", DECLARED_FIELDS[0]),
            "error",
            "BadRequestException",
            400,
        ),
        (
            "a start carrying no subscription document",
            json!({"id": "no-document", "type": "start", "payload": {}}),
            "error",
            "BadRequestException",
            400,
        ),
        (
            "a start whose document is not a subscription",
            query_message("a-query", "query Posts { post(id: \"1\") { id } }"),
            "error",
            "BadRequestException",
            400,
        ),
        (
            "a start whose document does not parse",
            query_message("not-graphql", "subscription { onAddPost"),
            "error",
            "BadRequestException",
            400,
        ),
        (
            "a start on a field the schema does not declare",
            start_message("undeclared", UNDECLARED_FIELD),
            "error",
            "NotFoundException",
            404,
        ),
        (
            "a start under an id this connection already holds",
            start_message("in-use", DECLARED_FIELDS[1]),
            "error",
            "BadRequestException",
            400,
        ),
        (
            "a stop for a subscription this connection does not hold",
            json!({"id": "never-started", "type": "stop"}),
            "error",
            "NotFoundException",
            404,
        ),
        (
            "a message of a type this endpoint does not serve",
            json!({"type": "whatever"}),
            "connection_error",
            "BadRequestException",
            400,
        ),
        (
            "a message that is not a document at all",
            Value::String("not a document".to_string()),
            "connection_error",
            "BadRequestException",
            400,
        ),
    ];
    for (case, message, kind, error_type, status) in cases {
        let answered = match message {
            Value::String(raw) => send_raw(case, &mut socket, raw).await,
            message => exchange(case, &mut socket, message).await,
        };
        let frame = answered
            .unwrap_or_else(|| panic!("case {case:?}: expected one frame back, got nothing"));
        assert_eq!(
            (
                frame["type"].as_str(),
                frame.pointer("/payload/errors/0/errorType").and_then(Value::as_str),
                frame.pointer("/payload/errors/0/errorCode").and_then(Value::as_i64),
            ),
            (Some(*kind), Some(*error_type), Some(*status)),
            "case {case:?}: expected a {kind} frame carrying {error_type} and {status}, got \
             {frame}"
        );
    }
}

#[tokio::test]
async fn a_deleted_api_stops_answering_where_it_published_its_realtime_endpoint() {
    let router = emulator();
    let api = create_api(&router, "deleted").await;
    let case = "a deleted API's realtime endpoint";

    let (status, deleted) = appsync(
        &router,
        "DELETE",
        &format!("/v1/apis/{}", api.api_id),
        None,
    )
    .await;
    assert_eq!(status, 200, "DeleteGraphqlApi must answer 200, got {deleted}");

    let dialled = tokio::time::timeout(
        FRAME_DEADLINE,
        TcpStream::connect(("127.0.0.1", loopback_port(case, &api.realtime))),
    )
    .await;
    let unreachable = match dialled {
        // A port nothing listens on refuses the connection, and a connection the core
        // severed reads EOF: either way, no handshake is served.
        Err(_elapsed) => true,
        Ok(Err(_refused)) => true,
        Ok(Ok(mut stream)) => {
            let _ = stream
                .write_all(b"GET /graphql HTTP/1.1\r\nhost: x\r\n\r\n")
                .await;
            let mut read = Vec::new();
            tokio::time::timeout(FRAME_DEADLINE, stream.read_to_end(&mut read))
                .await
                .is_ok_and(|_| read.is_empty())
        }
    };
    assert!(
        unreachable,
        "case {case:?}: a deleted API answers nothing at {:?}",
        api.realtime
    );
}

/// Create one GraphQL API with an API key and [`SCHEMA`], and read the address its
/// real-time endpoint is served on.
async fn create_api(router: &MultiServiceRouter, name: &str) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": format!("realtime-{name}"), "authenticationType": "API_KEY"})),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateGraphqlApi must answer 200, got {created}"
    );
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let realtime = crate::graphql_api_endpoint_dns_records::published_authority(api, "REALTIME");
    assert!(
        realtime.starts_with(&format!("{LOOPBACK_HOST}:")),
        "uris.REALTIME must be published on an address this emulator serves, got {realtime:?}"
    );
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    create_schema(router, &api_id).await;
    GraphqlApi {
        api_id,
        realtime,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}

/// Install [`SCHEMA`] on `api_id`, waiting for the clock to perform the creation.
async fn create_schema(router: &MultiServiceRouter, api_id: &str) {
    use base64::Engine as _;
    let definition = base64::engine::general_purpose::STANDARD.encode(SCHEMA);
    let (status, started) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        Some(json!({"definition": definition})),
    )
    .await;
    assert_eq!(
        status, 200,
        "StartSchemaCreation must answer 200, got {started}"
    );
    let deadline = tokio::time::Instant::now() + CREATION_DEADLINE;
    loop {
        let (status, settled) = appsync(
            router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "GetSchemaCreationStatus must answer 200, got {settled}"
        );
        if settled["status"].as_str() == Some("SUCCESS") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the schema creation must be performed, got {settled}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// A `start` message on the subscription field `field`, as a client sends one: the GraphQL
/// request travels as a JSON document in `payload.data`.
fn start_message(id: &str, field: &str) -> Value {
    query_message(id, &format!("subscription Watch {{ {field} {{ id }} }}"))
}

/// The same, carrying whatever GraphQL document the case presents.
///
/// Shared with the suite that drives a mutation's delivery to a subscriber
/// ([`crate::graphql_api_subscription_delivery`]), so both send the `start` a client sends.
pub(crate) fn query_message(id: &str, query: &str) -> Value {
    started_message(id, query, &json!({}))
}

/// The same, parameterised by the `variables` the client's own document carries.
pub(crate) fn started_message(id: &str, query: &str, variables: &Value) -> Value {
    json!({
        "id": id,
        "type": "start",
        "payload": {"data": json!({"query": query, "variables": variables}).to_string()},
    })
}

/// Open an opening handshake against the published endpoint: the established socket, or the
/// status the refusal carried.
///
/// The ONE real-time client of this crate's GraphQL suites: the suite that drives a
/// mutation's delivery to a subscriber ([`crate::graphql_api_subscription_delivery`]) dials
/// through this too, so there is one reading of how a client reaches a published real-time
/// address.
pub(crate) async fn handshake(
    case: &str,
    authority: &str,
    path: &str,
    offers_subprotocol: bool,
    header: Option<&Value>,
    fields: &[(&str, &str)],
) -> Result<ClientSocket, u16> {
    let stream = TcpStream::connect(("127.0.0.1", loopback_port(case, authority)))
        .await
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the published real-time endpoint {authority:?} must accept a \
                 connection, got {error}"
            )
        });
    // The authorization object travels in the handshake's `header` parameter, base64url
    // encoded, which is where a GraphQL API's real-time client puts a request's fields.
    let query = header
        .map(|object| format!("?header={}&payload=e30", encoded_auth_object(object)));
    // The fields RFC 6455 §4.1 requires of an opening handshake, written out because the
    // request below is handed to the client as it stands.
    let mut request = http::Request::builder()
        .uri(format!(
            "ws://{authority}{path}{}",
            query.clone().unwrap_or_default()
        ))
        .header("host", authority.to_string())
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", CLIENT_KEY)
        .body(())
        .expect("a well-formed handshake request");
    if offers_subprotocol {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            http::HeaderValue::from_static(SUBPROTOCOL),
        );
    }
    for (field, value) in fields {
        request.headers_mut().insert(
            http::HeaderName::from_bytes(field.as_bytes()).expect("a well-formed field name"),
            http::HeaderValue::from_str(value).expect("a well-formed field value"),
        );
    }
    match tokio_tungstenite::client_async(request, stream).await {
        Ok((socket, response)) => {
            assert_eq!(
                response.status().as_u16(),
                SWITCHING_PROTOCOLS,
                "case {case:?}: a completed handshake must answer 101"
            );
            assert_eq!(
                response
                    .headers()
                    .get("sec-websocket-protocol")
                    .and_then(|value| value.to_str().ok()),
                Some(SUBPROTOCOL),
                "case {case:?}: the answer must name the subprotocol this endpoint selected"
            );
            Ok(socket)
        }
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            Err(response.status().as_u16())
        }
        Err(error) => panic!("case {case:?}: the handshake failed before any answer: {error}"),
    }
}

/// Open a connection with the API's own key, ack it, and answer the socket — what every
/// case that needs a subscriber starts from.
async fn connected(case: &str, api: &GraphqlApi) -> ClientSocket {
    connected_with_key(case, &api.realtime, &api.api_key).await
}

/// The same, over whatever published address and API key the case holds.
pub(crate) async fn connected_with_key(
    case: &str,
    realtime: &str,
    api_key: &str,
) -> ClientSocket {
    let mut socket = handshake(
        case,
        realtime,
        REALTIME_PATH,
        true,
        Some(&json!({"host": realtime, "x-api-key": api_key})),
        &[],
    )
    .await
    .unwrap_or_else(|status| panic!("case {case:?}: the handshake must complete, got {status}"));
    let ack = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
    let ack = ack.unwrap_or_else(|| panic!("case {case:?}: connection_init must be answered"));
    assert_eq!(
        ack["type"].as_str(),
        Some("connection_ack"),
        "case {case:?}: connection_init must be answered connection_ack, got {ack}"
    );
    assert!(
        ack.pointer("/payload/connectionTimeoutMs")
            .and_then(Value::as_i64)
            .is_some_and(|timeout| timeout > 0),
        "case {case:?}: the ack must carry how long the connection may stay idle, got {ack}"
    );
    socket
}

/// Send one message and read the frame the endpoint answers with.
pub(crate) async fn exchange(case: &str, socket: &mut ClientSocket, message: &Value) -> Option<Value> {
    send_raw(case, socket, &message.to_string()).await
}

/// The same, carrying whatever BYTES the case presents — the seam for a message that is not
/// a JSON document at all.
async fn send_raw(case: &str, socket: &mut ClientSocket, body: &str) -> Option<Value> {
    socket
        .send(Message::Text(body.to_string().into()))
        .await
        .unwrap_or_else(|error| panic!("case {case:?}: the message must be sent, got {error}"));
    next_frame(socket).await
}

/// The next frame the endpoint writes, or `None` when the connection carries no further
/// frame — which a connection this endpoint ENDED does not.
pub(crate) async fn next_frame(socket: &mut ClientSocket) -> Option<Value> {
    match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => serde_json::from_str(text.as_str()).ok(),
        Ok(Some(Ok(Message::Binary(bytes)))) => serde_json::from_slice(&bytes).ok(),
        _ => None,
    }
}

/// The `Sum` of every datapoint of one `[GraphQLAPIId]` series, or `None` when the series
/// holds none.
async fn metric_sum(router: &MultiServiceRouter, metric: &str, api_id: &str) -> Option<f64> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64());
    let (status, body) = cloudwatch(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": "AWS/AppSync",
            "MetricName": metric,
            "Dimensions": [{"Name": "GraphQLAPIId", "Value": api_id}],
            "StartTime": now - 1_800.0,
            "EndTime": now + 60.0,
            "Period": 60,
            "Statistics": ["Sum", "Maximum"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "GetMetricStatistics for {metric} must answer 200, got {body}"
    );
    let datapoints = body["Datapoints"].as_array().cloned().unwrap_or_default();
    if datapoints.is_empty() {
        return None;
    }
    Some(
        datapoints
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum(),
    )
}
