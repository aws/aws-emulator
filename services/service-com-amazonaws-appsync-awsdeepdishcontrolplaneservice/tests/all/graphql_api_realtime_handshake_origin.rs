// SPDX-License-Identifier: CC0-1.0

//! WHICH clients a GraphQL API's REAL-TIME endpoint completes an opening handshake for, over
//! the `Origin` a client states about the address it dialled.
//!
//! The rule these rows assert, one level above the recorded case (a signed real-time client
//! of an `AWS_IAM` API, dialling `uris.REALTIME` with the authorization object in the
//! handshake's `header` parameter):
//!
//! > A real-time opening handshake is refused as cross-site only where the browser-set fields
//! > name ANOTHER SITE. An `Origin` naming an address of THIS MACHINE — which is the address
//! > every endpoint of this plane is published at, so it is what an ordinary client states
//! > about the very socket it dialled and what a browser sets for a page served from this
//! > machine — is not another site: the handshake completes with 101 and the connection is
//! > then decided by the material it presents, exactly as the API's HTTPS endpoint decides a
//! > request carrying the same origin. A client that sets no browser field at all is served
//! > too, and a page of any other site, the opaque origin of a sandboxed frame among them, is
//! > refused 403 before anything of the API is reached.
//!
//! The rows span what the rule generalises over: whether the client sets browser fields at
//! all, the FORM the origin is written in (the endpoint's own authority under either scheme,
//! another loopback port, the IPv6 loopback, a name, another site, the opaque origin), and
//! what `Sec-Fetch-Site` says beside it.
//!
//! The second case drives the recorded client's own conversation: an `AWS_IAM` API, a
//! handshake carrying SigV4 material in the base64url `header` parameter and the `Origin` a
//! WebSocket client states for the address it dialled, and the mutation that reaches the
//! subscriber over the socket that handshake established.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::{appsync, emulator};
use crate::graphql_api_mapping_template_resolvers::try_post_answer;
use crate::graphql_api_realtime_endpoint::{exchange, handshake, next_frame, query_message};

/// The path both of an API's endpoints are served at; the service's own copies are
/// `service::graphql_api::realtime::REALTIME_PATH` and `…::query::GRAPHQL_PATH`.
const REALTIME_PATH: &str = "/graphql";
const GRAPHQL_PATH: &str = "/graphql";

/// The status a completed opening handshake carries (RFC 6455 §4.1).
const SWITCHING_PROTOCOLS: u16 = 101;

/// The status a handshake a browser made from another site is refused with.
const FORBIDDEN: u16 = 403;

/// The `runtime` the resolvers of this suite are defined under. Source: `AppSyncRuntime`'s own
/// values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// The name of the `NONE` data source the mutation of this suite resolves against: "There is
/// no data source", so the request half's own payload is the result — what the subscriber must
/// then receive.
const DATA_SOURCE: &str = "local";

/// The schema every API of this suite is created with: one mutation, and the subscription its
/// `@aws_subscribe` links to that mutation.
const SCHEMA: &str = "type Note {\n  id: ID!\n  text: String\n}\n\
                      \n\
                      type Query {\n  getNote(id: ID!): Note\n}\n\
                      \n\
                      type Mutation {\n  createNote(id: ID!, text: String): Note\n}\n\
                      \n\
                      type Subscription {\n  \
                      onCreateNote: Note @aws_subscribe(mutations: [\"createNote\"])\n}\n";

/// One created GraphQL API: the two addresses it publishes.
struct GraphqlApi {
    graphql: String,
    realtime: String,
}

#[tokio::test]
async fn a_handshake_is_refused_only_where_the_browser_fields_name_another_site() {
    let router = emulator();
    let api = create_api(&router, "origins", "AWS_IAM").await;

    /// One row: the fields the handshake carries beside the ones RFC 6455 requires, and the
    /// status the endpoint must answer it with.
    struct Row {
        case: &'static str,
        /// Built from the endpoint's own address where the row is about it.
        origin: Option<String>,
        sec_fetch_site: Option<&'static str>,
        expected: u16,
    }
    let own = api.realtime.clone();
    let rows: Vec<Row> = vec![
        Row {
            case: "a client that sets no browser field at all",
            origin: None,
            sec_fetch_site: None,
            expected: SWITCHING_PROTOCOLS,
        },
        Row {
            case: "the endpoint's own address, as a ws client states it",
            origin: Some(format!("http://{own}")),
            sec_fetch_site: None,
            expected: SWITCHING_PROTOCOLS,
        },
        Row {
            case: "the endpoint's own address, as a wss client states it",
            origin: Some(format!("https://{own}")),
            sec_fetch_site: None,
            expected: SWITCHING_PROTOCOLS,
        },
        Row {
            case: "a page served from another port of this machine",
            origin: Some("http://localhost:5173".to_string()),
            sec_fetch_site: None,
            expected: SWITCHING_PROTOCOLS,
        },
        Row {
            case: "a page served from the IPv6 loopback",
            origin: Some("http://[::1]:4000".to_string()),
            sec_fetch_site: None,
            expected: SWITCHING_PROTOCOLS,
        },
        Row {
            case: "a page of this machine the browser calls same-origin",
            origin: Some(format!("http://{own}")),
            sec_fetch_site: Some("same-origin"),
            expected: SWITCHING_PROTOCOLS,
        },
        Row {
            case: "a page of this machine the browser calls cross-site",
            origin: Some("http://127.0.0.1:5173".to_string()),
            sec_fetch_site: Some("cross-site"),
            expected: SWITCHING_PROTOCOLS,
        },
        Row {
            case: "a page of another site",
            origin: Some("https://attacker.example".to_string()),
            sec_fetch_site: None,
            expected: FORBIDDEN,
        },
        Row {
            case: "a page of another site the browser calls cross-site",
            origin: Some("https://attacker.example".to_string()),
            sec_fetch_site: Some("cross-site"),
            expected: FORBIDDEN,
        },
        Row {
            case: "the opaque origin of a sandboxed frame",
            origin: Some("null".to_string()),
            sec_fetch_site: Some("cross-site"),
            expected: FORBIDDEN,
        },
        Row {
            case: "a browser naming a cross-site request with no origin",
            origin: None,
            sec_fetch_site: Some("cross-site"),
            expected: FORBIDDEN,
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut fields: Vec<(&str, &str)> = Vec::new();
        if let Some(origin) = &row.origin {
            fields.push(("origin", origin.as_str()));
        }
        if let Some(sec_fetch_site) = row.sec_fetch_site {
            fields.push(("sec-fetch-site", sec_fetch_site));
        }
        let answered = handshake(
            case,
            &api.realtime,
            REALTIME_PATH,
            true,
            Some(&signed_auth_object(&api.graphql)),
            &fields,
        )
        .await;
        match row.expected {
            SWITCHING_PROTOCOLS => {
                let mut socket = answered.unwrap_or_else(|status| {
                    panic!(
                        "case {case:?}: expected the handshake to be completed \
                         {SWITCHING_PROTOCOLS}, got {status}"
                    )
                });
                // A completed handshake is a live connection: the init it carries is acked,
                // which is what says the socket really was established.
                let acked = exchange(case, &mut socket, &json!({"type": "connection_init"})).await;
                assert_eq!(
                    acked.as_ref().and_then(|frame| frame["type"].as_str()),
                    Some("connection_ack"),
                    "case {case:?}: the established socket must ack the init, got {acked:?}"
                );
            }
            expected => assert_eq!(
                answered.err(),
                Some(expected),
                "case {case:?}: expected the handshake to be refused {expected}, got a \
                 completed one or another status"
            ),
        }
    }
}

#[tokio::test]
async fn an_iam_signed_subscriber_is_acked_and_receives_the_mutation() {
    let router = emulator();
    let api = create_api(&router, "iam-subscriber", "AWS_IAM").await;
    let case = "a signed real-time client of an IAM-authenticated API";

    // The handshake the recorded client makes: the SigV4 material it would carry on a signed
    // request, base64url-encoded into the `header` parameter, and the `Origin` a WebSocket
    // client states for the address it dialled.
    let origin = format!("https://{}", api.realtime);
    let mut subscriber = handshake(
        case,
        &api.realtime,
        REALTIME_PATH,
        true,
        Some(&signed_auth_object(&api.graphql)),
        &[("origin", origin.as_str())],
    )
    .await
    .unwrap_or_else(|status| {
        panic!("case {case:?}: the signed upgrade must be completed, got {status}")
    });
    let acked = exchange(case, &mut subscriber, &json!({"type": "connection_init"})).await;
    assert_eq!(
        acked.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("connection_ack"),
        "case {case:?}: connection_init must be answered connection_ack, got {acked:?}"
    );
    let started = exchange(
        case,
        &mut subscriber,
        &query_message(
            "watch-notes",
            "subscription Watch { onCreateNote { id text } }",
        ),
    )
    .await;
    assert_eq!(
        started.as_ref().and_then(|frame| frame["type"].as_str()),
        Some("start_ack"),
        "case {case:?}: a start on a subscription field of the schema must be acked, got \
         {started:?}"
    );

    // The signed mutation, served at the API's own GraphQL endpoint.
    let document = json!({
        "query": "mutation Write($id: ID!, $text: String) { \
                  createNote(id: $id, text: $text) { id text } }",
        "variables": {"id": "N-1", "text": "first"},
    });
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &document,
        &[("authorization", &sigv4_material())],
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the published GraphQL endpoint must answer"));
    assert_eq!(
        status, 200,
        "case {case:?}: the signed mutation is answered 200, got {status} and {answered}"
    );
    assert_eq!(
        answered.pointer("/data/createNote"),
        Some(&json!({"id": "N-1", "text": "first"})),
        "case {case:?}: the mutation must be resolved by the API's own resolver, got {answered}"
    );

    // And it reaches the subscriber over the socket the signed handshake established.
    let delivered = next_frame(&mut subscriber)
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the subscriber must receive the mutation"));
    assert_eq!(
        delivered,
        json!({
            "id": "watch-notes",
            "type": "data",
            "payload": {"data": {"onCreateNote": {"id": "N-1", "text": "first"}}},
        }),
        "case {case:?}: expected the published note under the subscription field, got \
         {delivered}"
    );
}

/// The authorization object a signed real-time client carries in the handshake's `header`
/// parameter: the fields it would have signed on a request to the API's `GRAPHQL` endpoint,
/// `host` among them — which is that endpoint's authority and not the real-time one's.
fn signed_auth_object(graphql: &str) -> Value {
    json!({
        "accept": "application/json, text/javascript",
        "content-encoding": "amz-1.0",
        "content-type": "application/json; charset=UTF-8",
        "host": graphql,
        "x-amz-date": "20260101T000000Z",
        "Authorization": sigv4_material(),
    })
}

/// The SigV4 material a signed call presents, under the signing name AppSync's data plane is
/// signed for. The signature itself is not verified by a service — the emulator verifies
/// request signatures at its front door — so what a case pins is what the endpoint answers
/// material of this mode.
fn sigv4_material() -> String {
    "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/us-east-1/appsync/aws4_request, \
     SignedHeaders=accept;content-encoding;content-type;host;x-amz-date, Signature=0000"
        .to_string()
}

/// Create one GraphQL API in `authentication_type` with [`SCHEMA`], a `NONE` data source and
/// the resolver of its one mutation, and read the addresses its two endpoints are served on.
async fn create_api(
    router: &MultiServiceRouter,
    name: &str,
    authentication_type: &str,
) -> GraphqlApi {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({
            "name": format!("realtime-origin-{name}"),
            "authenticationType": authentication_type,
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = &created["graphqlApi"];
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let address = |key: &str| {
        let published = crate::graphql_api_endpoint_dns_records::published_authority(api, key);
        assert!(
            published.starts_with(&format!("{LOOPBACK_HOST}:")),
            "uris.{key} must be published on an address this emulator serves, got {published:?}"
        );
        published
    };
    let graphql = address("GRAPHQL");
    let realtime = address("REALTIME");
    crate::graphql_api_mapping_template_resolvers::install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    let code = "export function request(ctx) {\n\
                  return {payload: {id: ctx.args.id, text: ctx.args.text}};\n\
                }\n\
                export function response(ctx) { return ctx.result; }";
    let (status, resolver) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Mutation/resolvers"),
        Some(json!({
            "fieldName": "createNote",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "code": code,
            "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver for createNote must answer 200, got {resolver}"
    );
    GraphqlApi { graphql, realtime }
}
