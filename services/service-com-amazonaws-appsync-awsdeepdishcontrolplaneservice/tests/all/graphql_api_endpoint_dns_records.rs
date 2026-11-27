// SPDX-License-Identifier: CC0-1.0

//! The DNS RECORDS a GraphQL API publishes beside its URIs, and what a client that composes
//! them reaches.
//!
//! The rule these cases assert, one level above the recorded case (one `CreateGraphqlApi`
//! whose four `uris`/`dns` members a check reads):
//!
//! > For every projection of a `GraphqlApi`, each entry of `GraphqlApi$dns` ("The DNS records
//! > for the API") is the bare HOST of the `GraphqlApi$uris` entry of the SAME endpoint — a
//! > host with no scheme, no port and no path, which is all a DNS record can hold — and the
//! > `uris` entry is the URL over that host, the port of the socket serving that endpoint and
//! > the API's `/graphql` path. Both endpoints of one API are published on the one host this
//! > emulator answers that API on, so the pair is that API's GraphQL and real-time endpoint
//! > and not two unrelated addresses; and a client that takes a `dns` record as a host,
//! > appends the published port and dials it reaches the endpoint.
//!
//! The rows span what the rule generalises over: the API's authentication type, its
//! visibility, its `apiType`, both endpoints it publishes and every operation that projects a
//! `GraphqlApi` (the create, the read and the listing).

use serde_json::{json, Value};

use crate::event_api_data_plane::{appsync, loopback_port};
use crate::graphql_api_mapping_template_resolvers::{emulator, install_schema, try_post_answer};
use crate::graphql_api_published_uri::split_uri;

/// The two endpoints a GraphQL API publishes, with the scheme each is addressed over and
/// whether it is the HTTP one. Source: the `GRAPHQL` and `REALTIME` keys of `GraphqlApi$dns`
/// ("The DNS records for the API.") and `GraphqlApi$uris` ("The URIs."), as
/// `smithy_query(query="shape", shape="GraphqlApi")` names the two members.
const ENDPOINTS: &[(&str, &str)] = &[("GRAPHQL", "https"), ("REALTIME", "wss")];

/// The path both endpoints of a GraphQL API serve, which every published URI ends with.
const GRAPHQL_PATH: &str = "/graphql";

/// The schema every API of this suite is created with: one field, so a request dialled at the
/// composed address can be answered with a value.
const SCHEMA: &str = "schema { query: Query }\n\ntype Query {\n  probe: String\n}\n";

/// The value that field resolves to, written by the resolver's own request template.
const PROBE: &str = "probe";

/// The name of the `NONE` data source the field's resolver resolves against.
const DATA_SOURCE: &str = "local";

/// One row: the API created, and the label the case is reported under.
struct Row<'a> {
    case: &'a str,
    authentication_type: &'a str,
    visibility: &'a str,
    api_type: &'a str,
}

/// RULE (see the module docs): every projection of a GraphQL API publishes each `dns` record
/// as the bare host of its own `uris` entry, both endpoints on one host of this emulator's.
#[tokio::test]
async fn a_dns_record_is_the_bare_host_of_the_endpoints_own_uri() {
    let router = emulator();
    let rows: &[Row] = &[
        Row {
            case: "an api authorizing with its own key",
            authentication_type: "API_KEY",
            visibility: "GLOBAL",
            api_type: "GRAPHQL",
        },
        Row {
            case: "a private api authorizing with IAM",
            authentication_type: "AWS_IAM",
            visibility: "PRIVATE",
            api_type: "GRAPHQL",
        },
        Row {
            case: "a merged api",
            authentication_type: "API_KEY",
            visibility: "GLOBAL",
            api_type: "MERGED",
        },
    ];

    for row in rows {
        let case = row.case;
        let mut body = json!({
            "name": format!("dns-{}-{}-{}", row.authentication_type, row.visibility, row.api_type),
            "authenticationType": row.authentication_type,
            "visibility": row.visibility,
            "apiType": row.api_type,
        });
        if row.api_type == "MERGED" {
            // A Merged API is created with the role AppSync assumes on its behalf.
            body["mergedApiExecutionRoleArn"] =
                json!("arn:aws:iam::000000000000:role/appsync-merged");
        }
        let (status, created) = appsync(&router, "POST", "/v1/apis", Some(body)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGraphqlApi must answer 200, got {status} and {created}"
        );
        let api = created["graphqlApi"].clone();
        let api_id = api["apiId"].as_str().unwrap_or_default().to_string();

        // The create's own projection: one host per endpoint, each the host of that
        // endpoint's uri, and both the same host of this emulator's.
        let hosts = published_hosts(case, &api);
        assert_eq!(
            hosts[0], hosts[1],
            "case {case:?}: both endpoints of one API are published on one host, got {hosts:?}"
        );

        // Every reader of the API projects the same pair, so a client that reads the API back
        // composes the same address.
        let (status, fetched) = appsync(&router, "GET", &format!("/v1/apis/{api_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetGraphqlApi must answer 200, got {status} and {fetched}"
        );
        let read = fetched["graphqlApi"].clone();
        assert_eq!(
            (&read["dns"], &read["uris"]),
            (&api["dns"], &api["uris"]),
            "case {case:?}: the read must project the published pair, got {fetched}"
        );
        published_hosts(&format!("{case} read back"), &read);

        let (status, listed) = appsync(&router, "GET", "/v1/apis", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ListGraphqlApis must answer 200, got {status} and {listed}"
        );
        let entry = listed["graphqlApis"]
            .as_array()
            .and_then(|apis| {
                apis.iter()
                    .find(|entry| entry["apiId"].as_str() == Some(api_id.as_str()))
            })
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: the listing must report the api, {listed}"));
        assert_eq!(
            (&entry["dns"], &entry["uris"]),
            (&api["dns"], &api["uris"]),
            "case {case:?}: the listing must project the published pair, got {listed}"
        );
        published_hosts(&format!("{case} listed"), &entry);
    }
}

/// RULE (see the module docs): a client that takes the `dns` record as a HOST, appends the
/// port the `uris` entry publishes and dials it reaches that API's GraphQL endpoint.
#[tokio::test]
async fn the_published_host_and_port_compose_an_address_this_emulator_answers() {
    let router = emulator();
    // The rows span the forms a client composes the address in: the published URI as it
    // stands, and the address rebuilt out of the endpoint's own dns record and that port.
    let composed: &[&str] = &["the published uri", "the dns record and the published port"];
    for case in composed {
        let (status, created) = appsync(
            &router,
            "POST",
            "/v1/apis",
            Some(json!({
                "name": format!("composed{}", case.len()),
                "authenticationType": "API_KEY",
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateGraphqlApi must answer 200, got {status} and {created}"
        );
        let api = created["graphqlApi"].clone();
        let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
        let key = create_key(&router, &api_id).await;
        install_schema(&router, &api_id, SCHEMA).await;
        create_resolver(&router, &api_id).await;

        let published = api["uris"]["GRAPHQL"].as_str().unwrap_or_default().to_string();
        let (_, authority, path) = split_uri(case, &published);
        let dialled = match *case {
            "the published uri" => authority.clone(),
            // A client that holds the DNS record and the port the URI published composes the
            // same authority out of them, which is what makes the two members agree.
            _ => format!(
                "{}:{}",
                api["dns"]["GRAPHQL"].as_str().unwrap_or_default(),
                loopback_port(case, &authority)
            ),
        };
        assert_eq!(
            dialled, authority,
            "case {case:?}: the composed address must be the published authority, got \
             {dialled:?}"
        );
        let answered = try_post_answer(
            case,
            &dialled,
            &path,
            "POST",
            &json!({"query": "query { probe }"}),
            &[("x-api-key", key.as_str())],
        )
        .await;
        let Some((status, _, document)) = answered else {
            panic!("case {case:?}: {dialled}{path} must be an address this emulator answers on")
        };
        assert_eq!(
            (status, document["data"]["probe"].as_str()),
            (200, Some(PROBE)),
            "case {case:?}: the composed address must execute the document, got {status} and \
             {document}"
        );
    }
}

/// The host each endpoint of `api` is published on, in [`ENDPOINTS`] order, having held every
/// published pair to the rule: the `dns` record is a bare host, the `uris` entry is the URL
/// over that host and the endpoint's port, and the port is one this emulator's own loopback
/// listener landed on.
fn published_hosts(case: &str, api: &Value) -> Vec<String> {
    let mut hosts: Vec<String> = Vec::new();
    for (endpoint, scheme) in ENDPOINTS {
        let label = format!("{case} ({endpoint})");
        let record = api["dns"][endpoint].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: the api must publish this endpoint's dns record, got {api}")
        });
        assert!(
            !record.contains(':') && !record.contains('/') && !record.is_empty(),
            "case {label:?}: a dns record carries a host with no scheme, port or path, got \
             {record:?}"
        );
        let published = api["uris"][endpoint].as_str().unwrap_or_else(|| {
            panic!("case {label:?}: the api must publish this endpoint's uri, got {api}")
        });
        let (announced, authority, path) = split_uri(&label, published);
        assert_eq!(
            (announced.as_str(), path.as_str()),
            (*scheme, GRAPHQL_PATH),
            "case {label:?}: expected the scheme and path this endpoint is served over, got \
             {published:?}"
        );
        let port = loopback_port(&label, &authority);
        assert_eq!(
            authority,
            format!("{record}:{port}"),
            "case {label:?}: the uri must be the dns record and the port of the socket serving \
             it, got {published:?}"
        );
        assert!(
            !record.contains("amazonaws.com"),
            "case {label:?}: a published host is this emulator's own, never a host of another \
             server, got {record:?}"
        );
        hosts.push(record.to_string());
    }
    hosts
}

/// The authority a client dials one endpoint of `api` at: the `uris` entry's own authority —
/// the endpoint's `dns` host and the port of the socket serving it — parsed out of the
/// published URI, which is the member that carries the port.
///
/// `pub(crate)`: every suite of this crate that drives a GraphQL API's data plane reads the
/// address it dials this way, so the tree holds ONE reading of how a client gets from a
/// published API to an address.
pub(crate) fn published_authority(api: &Value, endpoint: &str) -> String {
    let published = api["uris"][endpoint].as_str().unwrap_or_else(|| {
        panic!("the api must publish its {endpoint} endpoint, got {api}")
    });
    split_uri(endpoint, published).1
}

/// Mint one API key for `api_id` and answer the key a client presents.
async fn create_key(router: &smithy_server_runtime::MultiServiceRouter, api_id: &str) -> String {
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
async fn create_resolver(router: &smithy_server_runtime::MultiServiceRouter, api_id: &str) {
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
}
