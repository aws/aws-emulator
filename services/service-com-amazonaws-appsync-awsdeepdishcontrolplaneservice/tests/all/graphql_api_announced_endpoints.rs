// SPDX-License-Identifier: CC0-1.0

//! The endpoints EVERY projection of a GraphQL API announces, against the sockets this
//! emulator serves that API on.
//!
//! The rule these cases assert, one level above the recorded create (one `API_KEY` API whose
//! `uris`/`dns` were read back from the create, a get, a listing and an update):
//!
//! > A GraphQL API announces its GraphQL and real-time endpoints as addresses THIS emulator
//! > answers on: `GraphqlApi$uris` carries, per endpoint, the URL over the `host:port` of the
//! > socket that endpoint is SERVED on — the scheme its protocol dials (`https` for the
//! > GraphQL endpoint, `wss` for the real-time one) and the API's `/graphql` path — and
//! > `GraphqlApi$dns` carries the bare HOST of that URL, with no scheme, no port and no path,
//! > which is all a DNS record can hold. The create, every read and every listing of one API
//! > announce the same pair; a client that dials what was announced reaches that API's
//! > endpoint; no two APIs that exist at once are served on the same socket; and the reply to
//! > an UPDATE reports the API's `uris` while reporting no `dns` records at all.
//!
//! The rows span what the rule generalises over: every `authenticationType` the model
//! declares, over both `apiType`s and both visibilities — the members the projection of a
//! `GraphqlApi` is built from — and the four operations that project one.
//!
//! The harness (`router`, `call`, `create_api`, `auth_config`, `merge_members`, `AUTH_TYPES`)
//! is the lifecycle suite's, and the reading of an announced address (`loopback_port`) and the
//! dial of one (`dial`) are the Event API plane suite's, so no suite carries a second reading
//! of where the core binds a resource listener or a second way of dialling it.

use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use crate::event_api_data_plane::{dial, loopback_port};
use crate::graphql_api_lifecycle::{
    auth_config, call, create_api, merge_members, router, AUTH_TYPES,
};

/// The endpoint keys a `GraphqlApi` announces, with the URL scheme each one's protocol is
/// dialled over. Source: the `GRAPHQL` and `REALTIME` entries of `GraphqlApi$dns` ("The DNS
/// records for the API.") and `GraphqlApi$uris` ("The URIs."), the GraphQL endpoint being an
/// HTTPS one and the real-time endpoint a WebSocket
/// (`smithy_query(query="shape",shape="GraphqlApi")`).
const ENDPOINTS: &[(&str, &str)] = &[("GRAPHQL", "https"), ("REALTIME", "wss")];

/// The path both of an API's endpoints serve; the service's own copies are
/// `service::graphql_api::query::GRAPHQL_PATH` and `…::realtime::REALTIME_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The status the GraphQL endpoint answers a query that presents no material at all with —
/// AppSync admits no anonymous call. What a row asserts with it is that the answer came from
/// the ANNOUNCED address, and from that API's endpoint rather than from anything else.
const UNAUTHORIZED: u16 = 401;

/// How long a case waits for one dial or one answer before giving up: generous for a loaded
/// test machine, and bounded so a case that will never be answered fails instead of hanging.
const ANSWER_DEADLINE: Duration = Duration::from_secs(30);

/// One row: an API's configuration, which is what the projection under test is built from.
struct Row {
    case: String,
    body: Value,
}

/// A row per `authenticationType` the model declares, alternating the API type and the
/// visibility over them so every row differs from its neighbour in all three. Source:
/// `AuthenticationType`, `GraphQLApiType` and `GraphQLApiVisibility`, through the lifecycle
/// suite's `AUTH_TYPES` and `auth_config`.
fn rows() -> Vec<Row> {
    AUTH_TYPES
        .iter()
        .enumerate()
        .map(|(index, auth)| {
            let api_type = if index.is_multiple_of(2) {
                "GRAPHQL"
            } else {
                "MERGED"
            };
            let visibility = if index.is_multiple_of(2) {
                "GLOBAL"
            } else {
                "PRIVATE"
            };
            Row {
                case: format!("{auth} {api_type} {visibility}"),
                body: merge_members(
                    json!({
                        "name": format!("announced-{auth}-{api_type}-{visibility}"),
                        "authenticationType": auth,
                        "apiType": api_type,
                        "visibility": visibility,
                    }),
                    auth_config(auth),
                ),
            }
        })
        .collect()
}

/// The port an announced address names, having checked the address is the `host:port` of a
/// socket of THIS emulator — the Event API plane suite's reading of that question
/// ([`loopback_port`]), so no suite carries a second one — and that the port is one a socket
/// landed on.
fn announced_port(case: &str, endpoint: &str, authority: &str) -> u16 {
    let port = loopback_port(&format!("{case} {endpoint}"), authority);
    assert!(
        port > 0,
        "case {case:?}: uris.{endpoint} must name the port its socket landed on, got \
         {authority:?}"
    );
    port
}

/// Dial one announced address, as a client that read it from the API does — through the Event
/// API plane suite's [`dial`], which targets the loopback literal the announcement names, so a
/// case reaches this emulator's own listener on this machine and nothing else.
async fn dialled(case: &str, endpoint: &str, authority: &str) -> TcpStream {
    announced_port(case, endpoint, authority);
    let reached = tokio::time::timeout(ANSWER_DEADLINE, dial(case, authority)).await;
    match reached {
        Ok(Ok(stream)) => stream,
        other => panic!(
            "case {case:?}: a client dialling the announced {endpoint} address {authority:?} \
             must reach this emulator, got {other:?}"
        ),
    }
}

/// POST one GraphQL request to an announced `GRAPHQL` address and answer the HTTP status it
/// was served. The socket is handed over plaintext and the first byte says what the client
/// opened with, so a plain HTTP client reaches it.
async fn posted_query(case: &str, authority: &str) -> u16 {
    let mut stream = dialled(case, "GRAPHQL", authority).await;
    let payload = json!({"query": "query { __typename }"}).to_string();
    let request = format!(
        "POST {GRAPHQL_PATH} HTTP/1.1\r\nhost: {authority}\r\ncontent-type: application/json\r\n\
         content-length: {}\r\nconnection: close\r\n\r\n{payload}",
        payload.len()
    );
    stream
        .write_all(request.as_bytes())
        .await
        .unwrap_or_else(|error| {
            panic!("case {case:?}: the announced address {authority:?} must take a request, got {error}")
        });
    let mut answered = Vec::new();
    let read = tokio::time::timeout(ANSWER_DEADLINE, stream.read_to_end(&mut answered)).await;
    assert!(
        matches!(read, Ok(Ok(_))),
        "case {case:?}: the announced GRAPHQL address {authority:?} must answer a query, got \
         {read:?}"
    );
    let text = String::from_utf8_lossy(&answered).to_string();
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok());
    status.unwrap_or_else(|| {
        panic!(
            "case {case:?}: the announced GRAPHQL address {authority:?} must answer HTTP, got \
             {text:?}"
        )
    })
}

/// The `dns` and `uris` members of one projected `GraphqlApi`.
fn announced(api: &Value) -> (Value, Value) {
    (api["dns"].clone(), api["uris"].clone())
}

#[tokio::test]
async fn every_projection_announces_the_sockets_the_api_is_served_on() {
    let router = router();

    // Every address announced by any API of this suite, so no two of them can name one
    // socket while both APIs exist.
    let mut seen: Vec<(String, String)> = Vec::new();

    for Row { case, body } in rows() {
        let created = create_api(&router, body).await;
        let api_id = created["apiId"].as_str().unwrap_or_default().to_string();
        let (dns, uris) = announced(&created);

        for (endpoint, scheme) in ENDPOINTS {
            // The URI is the URL a client of that endpoint's protocol dials over the socket
            // this emulator serves the endpoint on, and the DNS record is the bare HOST of
            // that URL — no scheme, no port, no path.
            let published = uris[endpoint].as_str().unwrap_or_default().to_string();
            let authority = crate::graphql_api_endpoint_dns_records::published_authority(
                &created, endpoint,
            );
            announced_port(&case, endpoint, &authority);
            assert_eq!(
                published,
                format!("{scheme}://{authority}{GRAPHQL_PATH}"),
                "case {case:?}: uris.{endpoint} must be the {scheme} URL over the socket the \
                 endpoint is served on, got {uris:?}"
            );
            assert_eq!(
                dns[endpoint].as_str(),
                Some(::service_common::data_plane_addressing::host_only(&authority)),
                "case {case:?}: dns.{endpoint} must be the bare host of uris.{endpoint} \
                 {published:?}, got {dns:?}"
            );

            // And it is SERVED: a dial of the announced address reaches this emulator.
            drop(dialled(&case, endpoint, &authority).await);

            // No other API of this account is served on it.
            let clash = seen
                .iter()
                .find(|(_, taken)| taken == &authority)
                .cloned();
            assert!(
                clash.is_none(),
                "case {case:?}: {endpoint} announced {authority:?}, which another API already \
                 announces ({clash:?}): two APIs that exist at once are served on sockets of \
                 their own"
            );
            seen.push((api_id.clone(), authority.clone()));
        }

        // The GraphQL endpoint announced for THIS API answers a query at that address: the
        // call is refused for presenting no material, which is this API's own endpoint
        // deciding it rather than another listener answering.
        let graphql =
            crate::graphql_api_endpoint_dns_records::published_authority(&created, "GRAPHQL");
        let status = posted_query(&case, &graphql).await;
        assert_eq!(
            status, UNAUTHORIZED,
            "case {case:?}: the announced GRAPHQL address {graphql:?} must be served by that \
             API's own endpoint, got HTTP {status}"
        );

        // Every read of the API announces the same pair.
        let (status, fetched) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetGraphqlApi 200, got {status} {fetched:?}"
        );
        assert_eq!(
            announced(&fetched["graphqlApi"]),
            (dns.clone(), uris.clone()),
            "case {case:?}: the read must announce the addresses the create did, got {fetched:?}"
        );

        // So does every listing that reports it.
        let (status, listed) = call(&router, "GET", "/v1/apis?maxResults=25", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListGraphqlApis 200, got {status} {listed:?}"
        );
        let element = listed["graphqlApis"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|api| api["apiId"].as_str() == Some(api_id.as_str()))
            .unwrap_or(Value::Null);
        assert_eq!(
            announced(&element),
            (dns.clone(), uris.clone()),
            "case {case:?}: the listing must announce the addresses the create did, got \
             {element:?}"
        );

        // And the reply to an update reports the API's uris with no dns records at all,
        // while the API's own records are untouched by it.
        let (status, updated) = call(
            &router,
            "POST",
            &format!("/v1/apis/{api_id}"),
            Some(merge_members(
                json!({
                    "name": format!("announced-{api_id}-updated"),
                    "authenticationType": created["authenticationType"].clone(),
                    "ownerContact": "endpoints@example.com",
                }),
                auth_config(created["authenticationType"].as_str().unwrap_or_default()),
            )),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateGraphqlApi 200, got {status} {updated:?}"
        );
        let answer = updated["graphqlApi"].clone();
        assert!(
            answer["dns"].is_null(),
            "case {case:?}: the update's reply must carry no dns records, got {answer:?}"
        );
        assert_eq!(
            answer["uris"], uris,
            "case {case:?}: the update's reply must announce the API's uris unchanged, got \
             {answer:?}"
        );
        let (status, reread) = call(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetGraphqlApi 200 after the update, got {status} {reread:?}"
        );
        assert_eq!(
            announced(&reread["graphqlApi"]),
            (dns, uris),
            "case {case:?}: the update must leave the announced addresses as they were, got \
             {reread:?}"
        );
    }
}
