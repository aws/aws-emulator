// SPDX-License-Identifier: CC0-1.0

//! WHICH published member of an AppSync API carries the port a client dials, per API family —
//! the one reading a client of either family composes an address by.
//!
//! The rule these cases assert, one level above a single projection of a single API:
//!
//! > An API family whose projection publishes URIs beside its DNS records (`GraphqlApi`, which
//! > carries both `dns` "The DNS records for the API" and `uris` "The URIs") publishes each
//! > `dns` entry as the bare HOST of that endpoint — no scheme, no port and no path, which is
//! > all a DNS record can hold — and publishes the whole URL, the port of the socket serving
//! > that endpoint among it, in the `uris` entry of the SAME endpoint. A family whose
//! > projection publishes DNS records ALONE (`Api`, the Event API resource, which has no
//! > `uris` member at all) publishes each record as the AUTHORITY a client dials, host and
//! > port, because the record is then the only place the port can be published. Either way the
//! > address composed out of what one projection published is an address this emulator answers
//! > that API's own data plane on.
//!
//! So a `dns` record is NOT interchangeable between the families: a reader that takes a
//! GraphQL API's record for an authority holds a host with no port and reaches nothing, and
//! one that expects an Event API's record to be a bare host loses the port that record exists
//! to publish.
//!
//! The rows span what the rule generalises over: both families, every endpoint each of them
//! publishes, and the projections a client finds an API through (the create, the read and the
//! listing).

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::{
    appsync, create_event_api, create_namespace, dial, loopback_port, publish, EventApi,
};
use crate::graphql_api_mapping_template_resolvers::{
    emulator, install_schema, try_post_answer, GRAPHQL_PATH,
};
use crate::graphql_api_published_uri::split_uri;

/// The endpoints a `GraphqlApi` publishes and the scheme each is addressed over. Source: the
/// `GRAPHQL` and `REALTIME` keys of `GraphqlApi$dns` / `GraphqlApi$uris`, as
/// `smithy_query(query="shape", shape="GraphqlApi")` names the two members.
const GRAPHQL_ENDPOINTS: &[(&str, &str)] = &[("GRAPHQL", "https"), ("REALTIME", "wss")];

/// The endpoints an `Api` publishes: `Api$dns` "will include an HTTP and a real-time endpoint"
/// (`smithy_query(query="shape", shape="Api")`), which is the whole of what that family
/// publishes — the shape has no `uris` member.
const EVENT_ENDPOINTS: &[&str] = &["HTTP", "REALTIME"];

/// The schema the GraphQL API of this suite is created with: one field, so the address
/// composed out of the published members can be driven to a value.
const SCHEMA: &str = "schema { query: Query }\n\ntype Query {\n  probe: String\n}\n";

/// The value that field resolves to, written by the resolver's own request template.
const PROBE: &str = "probe";

/// The name of the `NONE` data source that field's resolver resolves against.
const DATA_SOURCE: &str = "local";

/// The channel namespace the Event API's publish is addressed to.
const NAMESPACE: &str = "members";

/// How long a case waits for a published socket to accept a connection: generous for a loaded
/// test machine, and bounded so an address nothing serves fails instead of hanging.
const DIAL_DEADLINE: Duration = Duration::from_secs(30);

/// RULE (see the module docs): a GraphQL API publishes each `dns` record as a bare host and
/// the port of the socket serving that endpoint in the `uris` entry beside it, over every
/// projection — and the authority that URI carries is one this emulator answers.
#[tokio::test]
async fn a_graphql_api_publishes_the_port_in_its_uri_and_a_bare_host_in_its_dns_record() {
    let router = emulator();
    let (created, api_id, api_key) = create_graphql_api(&router).await;

    for (label, projection) in graphql_projections(&router, &api_id, created.clone()).await {
        for (endpoint, scheme) in GRAPHQL_ENDPOINTS {
            let case = format!("{label} ({endpoint})");
            let record = projection["dns"][endpoint].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {label} must publish this endpoint's dns record, got {projection}")
            });
            assert!(
                !record.is_empty() && !record.contains(':') && !record.contains('/'),
                "case {case:?}: a dns record carries a host alone — no scheme, port or path — \
                 got {record:?}"
            );
            assert_eq!(
                record, LOOPBACK_HOST,
                "case {case:?}: expected the host this emulator serves the endpoint on, got \
                 {record:?}"
            );

            let published = projection["uris"][endpoint].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {label} must publish this endpoint's uri, got {projection}")
            });
            let (announced, authority, path) = split_uri(&case, published);
            assert_eq!(
                (announced.as_str(), path.as_str()),
                (*scheme, GRAPHQL_PATH),
                "case {case:?}: expected the scheme and path this endpoint is served over, got \
                 {published:?}"
            );
            // The port the record cannot hold is the one the URI carries, and the two members
            // name one address between them.
            let port = loopback_port(&case, &authority);
            assert_eq!(
                authority,
                format!("{record}:{port}"),
                "case {case:?}: the uri must be the dns record and the port of the socket \
                 serving it, got {published:?}"
            );
            let dialled = tokio::time::timeout(DIAL_DEADLINE, dial(&case, &authority)).await;
            assert!(
                matches!(dialled, Ok(Ok(_))),
                "case {case:?}: the composed address {authority:?} must accept a connection, \
                 got {dialled:?}"
            );
        }
    }

    // And the composed address serves that API's own data plane: the document is executed
    // there, which is what the two published members exist to lead a client to.
    let case = "the address composed out of a graphql api's published members";
    let authority = split_uri(
        case,
        created["uris"]["GRAPHQL"].as_str().unwrap_or_default(),
    )
    .1;
    let answered = try_post_answer(
        case,
        &authority,
        GRAPHQL_PATH,
        "POST",
        &json!({"query": "query { probe }"}),
        &[("x-api-key", api_key.as_str())],
    )
    .await;
    let Some((status, _, document)) = answered else {
        panic!("case {case:?}: {authority}{GRAPHQL_PATH} must be an address this emulator answers")
    };
    assert_eq!(
        (status, document["data"]["probe"].as_str()),
        (200, Some(PROBE)),
        "case {case:?}: the composed address must execute the document, got {status} and \
         {document}"
    );
}

/// RULE (see the module docs): an Event API publishes no URIs, so each of its `dns` records
/// carries the authority a client dials — host and port — over every projection, and that
/// authority is one this emulator answers.
#[tokio::test]
async fn an_event_api_publishes_the_port_in_its_dns_record_because_it_publishes_no_uris() {
    let router = emulator();
    let api = create_event_api(&router, "Published Members").await;
    create_namespace(&router, &api.api_id, NAMESPACE, None).await;

    let mut discovered: Option<EventApi> = None;
    for (label, projection) in event_projections(&router, &api.api_id).await {
        assert!(
            projection["uris"].is_null(),
            "case {label:?}: an Api publishes no uris member beside its dns records, got \
             {projection}"
        );
        let mut authorities: Vec<String> = Vec::new();
        for endpoint in EVENT_ENDPOINTS {
            let case = format!("{label} ({endpoint})");
            let record = projection["dns"][endpoint].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: {label} must publish this endpoint's dns record, got {projection}")
            });
            // The record is the authority itself here: the port is in it, because this family
            // publishes nowhere else it could be.
            let port = loopback_port(&case, record);
            assert_eq!(
                record,
                format!("{LOOPBACK_HOST}:{port}"),
                "case {case:?}: a record of this family carries the host and the port of the \
                 socket serving the endpoint, got {record:?}"
            );
            let dialled = tokio::time::timeout(DIAL_DEADLINE, dial(&case, record)).await;
            assert!(
                matches!(dialled, Ok(Ok(_))),
                "case {case:?}: the published address {record:?} must accept a connection, got \
                 {dialled:?}"
            );
            authorities.push(record.to_string());
        }
        discovered = Some(EventApi {
            api_id: api.api_id.clone(),
            http: authorities[0].clone(),
            realtime: authorities[1].clone(),
            api_key: api.api_key.clone(),
        });
    }

    // And the records the LISTING published — the last projection read above, which is where a
    // client holding no id finds the API — serve that API's own data plane.
    let case = "the addresses an event api's listed dns records carry";
    let discovered = discovered.unwrap_or_else(|| panic!("case {case:?}: a projection must be read"));
    let (status, accepted) = publish(
        case,
        &discovered,
        &format!("/{NAMESPACE}/room1"),
        &[json!({"m": "a"})],
        Some(discovered.api_key.as_str()),
    )
    .await;
    assert_eq!(
        (status, accepted["successful"].as_array().map(Vec::len)),
        (200, Some(1)),
        "case {case:?}: the published address must accept the event, got {status} and {accepted}"
    );
}

/// Create one GraphQL API with an API key, [`SCHEMA`], a `NONE` data source and the one
/// resolver its field is answered by: the create's projection, the API's id and the key a
/// client presents.
async fn create_graphql_api(router: &MultiServiceRouter) -> (Value, String, String) {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": "publishedMembers", "authenticationType": "API_KEY"})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = created["graphqlApi"].clone();
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();

    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    let api_key = key["apiKey"]["id"].as_str().unwrap_or_default().to_string();

    install_schema(router, &api_id, SCHEMA).await;
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
    assert_eq!(status, 200, "CreateResolver must answer 200, got {resolver}");
    (api, api_id, api_key)
}

/// Every projection of the GraphQL API a client can hold, labelled: the create's own, the
/// read, and the entry the listing carries.
async fn graphql_projections(
    router: &MultiServiceRouter,
    api_id: &str,
    created: Value,
) -> Vec<(&'static str, Value)> {
    let (status, fetched) = appsync(router, "GET", &format!("/v1/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetGraphqlApi must answer 200, got {status} and {fetched}"
    );
    let (status, listed) = appsync(router, "GET", "/v1/apis", None).await;
    assert_eq!(
        status, 200,
        "ListGraphqlApis must answer 200, got {status} and {listed}"
    );
    let entry = listed["graphqlApis"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["apiId"] == json!(api_id))
        .cloned()
        .unwrap_or_else(|| panic!("the listing must carry the api {api_id:?}, got {listed}"));
    vec![
        ("the create", created),
        ("the read", fetched["graphqlApi"].clone()),
        ("the listing", entry),
    ]
}

/// Every projection of the Event API a client can hold, labelled: the read, and the entry the
/// listing carries — the create's own records are read by `create_event_api`.
async fn event_projections(
    router: &MultiServiceRouter,
    api_id: &str,
) -> Vec<(&'static str, Value)> {
    let (status, fetched) = appsync(router, "GET", &format!("/v2/apis/{api_id}"), None).await;
    assert_eq!(
        status, 200,
        "GetApi must answer 200, got {status} and {fetched}"
    );
    let (status, listed) = appsync(router, "GET", "/v2/apis", None).await;
    assert_eq!(status, 200, "ListApis must answer 200, got {status} and {listed}");
    let entry = listed["apis"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entry| entry["apiId"] == json!(api_id))
        .cloned()
        .unwrap_or_else(|| panic!("the listing must carry the api {api_id:?}, got {listed}"));
    vec![("the read", fetched["api"].clone()), ("the listing", entry)]
}
