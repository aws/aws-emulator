// SPDX-License-Identifier: CC0-1.0

//! What a client reaches when it takes `GraphqlApi$uris` AS PUBLISHED.
//!
//! The rule these cases assert, one level above the recorded case (one `CreateGraphqlApi` whose
//! `uris.GRAPHQL` a check reads and then POSTs a mutation to):
//!
//! > Every address a GraphQL API publishes is an address of the server that published it: each
//! > entry of `uris` is a URL over the `host:port` of the socket that endpoint is served on,
//! > which is a socket THIS emulator answers on and never a host belonging to another server;
//! > the `dns` record of the same endpoint is the bare host of that URL; and every later read
//! > of the API publishes the same ones. So a client that parses the published URI — its
//! > scheme, its host, its port and its path — and dials exactly that reaches the API's
//! > endpoint.
//!
//! The rows span what the rule generalises over: the API's authentication type, its visibility
//! and both endpoints it publishes; and each case reaches the API by PARSING what was
//! published rather than by rebuilding an address out of the `dns` record, because parsing the
//! published URI is what a client (or an EventBridge rule target built from one) does.

use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::{appsync, loopback_port};
use crate::graphql_api_mapping_template_resolvers::{emulator, install_schema, try_post_answer};

/// The schema every API of this suite is created with: one field, so a request that reaches the
/// published address can be answered with a value.
const SCHEMA: &str = "schema { query: Query }\n\ntype Query {\n  probe: String\n}\n";

/// The value that field resolves to, written by the resolver's own request template.
const PROBE: &str = "probe";

/// The name of the `NONE` data source the field's resolver resolves against.
const DATA_SOURCE: &str = "local";

/// The path both endpoints of a GraphQL API serve; the service's own copy is
/// `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The scheme each published endpoint is addressed over: the GraphQL endpoint takes HTTP
/// requests and the real-time one a WebSocket handshake.
const SCHEMES: &[(&str, &str)] = &[("GRAPHQL", "https"), ("REALTIME", "wss")];

/// One row: the API created, and whether a client holding the API's own key is authorized to
/// select its field.
struct Row<'a> {
    case: &'a str,
    authentication_type: &'a str,
    visibility: &'a str,
    keyed: bool,
}

#[tokio::test]
async fn a_client_that_dials_what_the_api_published_reaches_this_emulator() {
    let router = emulator();
    let rows: &[Row] = &[
        Row {
            case: "an api authorizing with its own key",
            authentication_type: "API_KEY",
            visibility: "GLOBAL",
            keyed: true,
        },
        Row {
            case: "an api whose visibility is private",
            authentication_type: "API_KEY",
            visibility: "PRIVATE",
            keyed: true,
        },
        Row {
            case: "an api authorizing with IAM",
            authentication_type: "AWS_IAM",
            visibility: "GLOBAL",
            keyed: false,
        },
    ];

    for row in rows {
        let case = row.case;
        let (status, created) = appsync(
            &router,
            "POST",
            "/v1/apis",
            Some(json!({
                "name": format!("published-{}-{}", row.authentication_type, row.visibility),
                "authenticationType": row.authentication_type,
                "visibility": row.visibility,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGraphqlApi must answer 200, got {status} and {created}"
        );
        let api = created["graphqlApi"].clone();
        let api_id = api["apiId"].as_str().unwrap_or_default().to_string();

        // Every published URI is an address of this emulator: the host is the one the core
        // binds a resource listener on, the port is that listener's, the path is the one the
        // endpoint serves, and the record the URI is built over is the API's own `dns` entry.
        // A host belonging to another server — an AWS-owned domain among them — fails here.
        let mut addresses: Vec<(String, String)> = Vec::new();
        for (endpoint, scheme) in SCHEMES {
            let label = format!("{case} ({endpoint})");
            let published = api["uris"][endpoint].as_str().unwrap_or_else(|| {
                panic!("case {label:?}: the api must publish this endpoint, got {api}")
            });
            let (announced, authority, path) = split_uri(&label, published);
            assert_eq!(
                (announced.as_str(), path.as_str()),
                (*scheme, GRAPHQL_PATH),
                "case {label:?}: expected the scheme and path this endpoint is served over, \
                 got {published:?}"
            );
            assert_eq!(
                Some(::service_common::data_plane_addressing::host_only(&authority)),
                api["dns"][endpoint].as_str(),
                "case {label:?}: the endpoint's own dns record must be the bare host of its \
                 uri, got {api}"
            );
            // The reading of an announced address is the data-plane suites' own
            // (`loopback_port`): it is the host the core binds on, and nothing else.
            let port = loopback_port(&label, &authority);
            assert!(
                port > 0,
                "case {label:?}: expected the port the endpoint is served on, got \
                 {published:?}"
            );
            assert!(
                !published.contains("amazonaws.com"),
                "case {label:?}: a published address is this emulator's own, never a host of \
                 another server, got {published:?}"
            );
            addresses.push((authority, path));
        }
        assert_ne!(
            addresses[0], addresses[1],
            "case {case:?}: the two endpoints are published on sockets of their own, got \
             {addresses:?}"
        );

        // Every later read publishes the same addresses, so a client that reads the API back
        // dials what the create announced.
        let (status, fetched) = appsync(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            (status, &fetched["graphqlApi"]["uris"]),
            (200, &api["uris"]),
            "case {case:?}: GetGraphqlApi must publish the same uris, got {fetched}"
        );

        // And the GraphQL endpoint ANSWERS where it said it would, dialed as published.
        let key = create_key(&router, &api_id).await;
        install_schema(&router, &api_id, SCHEMA).await;
        create_resolver(&router, &api_id).await;
        let (authority, path) = addresses[0].clone();
        let answered = try_post_answer(
            case,
            &authority,
            &path,
            "POST",
            &json!({"query": "query { probe }"}),
            &[("x-api-key", key.as_str())],
        )
        .await;
        let Some((status, _, document)) = answered else {
            panic!(
                "case {case:?}: the published address {authority}{path} must be one this \
                 emulator answers on"
            )
        };
        if row.keyed {
            assert_eq!(
                (status, document["data"]["probe"].as_str()),
                (200, Some(PROBE)),
                "case {case:?}: the published endpoint must execute the document, got {status} \
                 and {document}"
            );
        } else {
            // An API that authorizes another way refuses the key — which is still THIS
            // emulator answering at the address it published.
            assert!(
                status >= 400,
                "case {case:?}: an api authorizing with {} refuses a request carrying a key, \
                 got {status} and {document}",
                row.authentication_type
            );
        }
    }
}

/// The scheme, the authority and the path a published URI carries, read out of the URI itself.
///
/// Shared with the suite that follows the same discovery path on an `OPENID_CONNECT` API
/// ([`crate::graphql_api_open_id_connect_published_uri`]), so the tree holds ONE reading of how
/// a client parses what a GraphQL API published.
pub(crate) fn split_uri(case: &str, uri: &str) -> (String, String, String) {
    let (scheme, rest) = uri
        .split_once("://")
        .unwrap_or_else(|| panic!("case {case:?}: expected a published uri, got {uri:?}"));
    let (authority, path) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: expected a path, got {uri:?}"));
    (scheme.to_string(), authority.to_string(), format!("/{path}"))
}

/// Mint one API key for `api_id` and answer the key a client presents.
async fn create_key(router: &MultiServiceRouter, api_id: &str) -> String {
    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    key["apiKey"]["id"].as_str().unwrap_or_default().to_string()
}

/// Attach the one resolver this suite's field is answered by: a local resolver whose request
/// template writes the payload a `NONE` data source passes through.
async fn create_resolver(router: &MultiServiceRouter, api_id: &str) {
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({"name": DATA_SOURCE, "type": "NONE"})),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    let (status, resolver) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": PROBE,
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "requestMappingTemplate":
                format!("{{\"version\": \"2017-02-28\", \"payload\": {{\"msg\": \"{PROBE}\"}}}}"),
            "responseMappingTemplate": "$util.toJson($ctx.result.msg)",
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateResolver must answer 200, got {resolver}"
    );
}
