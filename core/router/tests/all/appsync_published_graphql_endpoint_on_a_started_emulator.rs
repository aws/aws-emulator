// SPDX-License-Identifier: CC0-1.0

//! The address a GraphQL API PUBLISHES, dialled exactly as published, on the emulator a
//! customer STARTS: the built binary, on its own port, with request authentication on, reached
//! over TLS.
//!
//! The rule under test: **every address a GraphQL API publishes is an address of the server
//! that published it — each entry of `GraphqlApi$uris` is a URL over the `host:port` the `dns`
//! record of the same endpoint names, which is a socket THIS emulator answers on and never a
//! host belonging to another server — so a client that parses the published URI (its scheme,
//! its host, its port and its path) and dials exactly that reaches the API's endpoint and is
//! ANSWERED there.** `GraphqlApi$uris` is "The URIs.", the only address the service publishes
//! for the API's data plane, so an emulator has to publish one it is reached on.
//!
//! The sibling suite `appsync_dynamodb_data_source_resolvers` spans what a DynamoDB resolver
//! generalises over on a router the test assembles in its own process, with authentication off,
//! dialling the published port in the clear. The emulator a client runs is a different
//! configuration of the same code: the credentials are verified, the per-resource listener
//! presents the process certificate, so the `https://` the API publishes is dialled as
//! published, and one process serves the control plane, the API's own endpoint and the
//! DynamoDB the resolver calls. This suite is the one that pins the published endpoint THERE,
//! so an address that answers in-process and not on the started emulator is a failure a test
//! reports rather than one only a client sees.
//!
//! The rows span what the rule generalises over: the kind of value a round trip carries
//! (plain text, text with spaces and punctuation, a large number), a key the table holds no
//! item for, and both of the API's published endpoints. Every write a row makes through the
//! published endpoint is read back through DynamoDB's OWN api, so what the endpoint reports is
//! what that service holds. ONE start is paid for, since starting the binary costs what
//! starting the emulator costs.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::cfn_deployed_pipeline_on_a_started_emulator::{Started, REGION};
use crate::query_wire::element;

/// The table every resolver of this suite resolves against, and its one key attribute.
const TABLE: &str = "appsync-published-posts";
const KEY_ATTRIBUTE: &str = "id";

/// The role the data source makes its calls under, and the service principal that assumes it.
const ROLE: &str = "appsync-published-endpoint-role";
const APPSYNC_PRINCIPAL: &str = "appsync.amazonaws.com";

/// The data source every resolver of this suite resolves against.
const DATA_SOURCE: &str = "ddb";

/// The path a GraphQL API's `GRAPHQL` endpoint serves, as `uris` publishes it; the service's
/// own copy is `service::graphql_api::query::GRAPHQL_PATH`.
const GRAPHQL_PATH: &str = "/graphql";

/// The scheme each published endpoint is addressed over: the GraphQL endpoint takes HTTP
/// requests and the real-time one a WebSocket handshake.
const SCHEMES: &[(&str, &str)] = &[("GRAPHQL", "https"), ("REALTIME", "wss")];

/// How long a wait on a CONDITION may take, and how often this suite looks again. Generous:
/// what is under test is what the endpoint answers, never how fast the host reaches a
/// transition.
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);
const LOOK_INTERVAL: Duration = Duration::from_millis(200);

/// The schema the API is created with: the item the table holds, the read of one and the write
/// of one.
const SCHEMA: &str = "\
type Post {\n  id: ID!\n  title: String\n  views: Int\n}\n\
\n\
type Query {\n  getPost(id: ID!): Post\n}\n\
\n\
type Mutation {\n  putPost(id: ID!, title: String, views: Int): Post\n}\n";

/// One round trip: the item a mutation writes and a query reads back.
struct Row<'a> {
    case: &'a str,
    id: &'a str,
    title: &'a str,
    views: i64,
}

#[test]
fn a_client_dialling_the_published_graphql_uri_round_trips_an_item_through_dynamodb() {
    let case = "the published GraphQL endpoint of a started emulator";
    let emulator = Started::start();
    create_table(&emulator);
    let role_arn = create_role(&emulator);
    let api = create_api(&emulator, &role_arn);

    // Every published URI is an address of this emulator, built over the endpoint's own `dns`
    // record: a host belonging to another server — an AWS-owned domain among them — fails
    // here, and so does a URI that is not the record's own address.
    let mut published: Vec<(String, String)> = Vec::new();
    for (endpoint, scheme) in SCHEMES {
        let label = format!("{case} ({endpoint})");
        let uri = api.api["uris"][endpoint]
            .as_str()
            .unwrap_or_else(|| panic!("case {label:?}: the api must publish this endpoint"));
        let (announced, authority, path) = split_uri(&label, uri);
        assert_eq!(
            (announced.as_str(), path.as_str()),
            (*scheme, GRAPHQL_PATH),
            "case {label:?}: expected the scheme and path this endpoint is served over, got \
             {uri:?}"
        );
        // The record is the bare HOST of that URI: the authority without the port of the
        // socket the endpoint is served on, which is all a DNS record can hold.
        let host = authority
            .rsplit_once(':')
            .map_or(authority.as_str(), |(host, _port)| host);
        assert_eq!(
            Some(host),
            api.api["dns"][endpoint].as_str(),
            "case {label:?}: the endpoint's own dns record must be the bare host of its uri, \
             got {}",
            api.api
        );
        assert!(
            !uri.contains("amazonaws.com"),
            "case {label:?}: a published address is this emulator's own, never a host of \
             another server, got {uri:?}"
        );
        assert!(
            authority.starts_with("127.0.0.1:")
                && authority
                    .rsplit(':')
                    .next()
                    .is_some_and(|port| port.parse::<u16>().is_ok_and(|port| port > 0)),
            "case {label:?}: expected the host:port this endpoint is served on, got {uri:?}"
        );
        published.push((authority, path));
    }
    assert_ne!(
        published[0], published[1],
        "case {case:?}: the two endpoints are published on sockets of their own, got \
         {published:?}"
    );

    // And each later read publishes the same addresses, so a client that reads the API back
    // dials what the create announced.
    let (status, body) = emulator.call(
        "appsync",
        "GET",
        &format!("/v1/apis/{}", api.api_id),
        &[("content-type", "application/json")],
        Vec::new(),
    );
    let fetched: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    assert_eq!(
        (status, &fetched["graphqlApi"]["uris"]),
        (200, &api.api["uris"]),
        "case {case:?}: GetGraphqlApi must publish the same uris, got {status} and {body}"
    );

    let rows: &[Row] = &[
        Row {
            case: "a plain title",
            id: "post-1",
            title: "launch",
            views: 7,
        },
        Row {
            case: "a title carrying spaces and punctuation",
            id: "post-2",
            title: "the second post, revised",
            views: 0,
        },
        Row {
            case: "a large view count",
            id: "post-3",
            title: "popular",
            views: 1_234_567,
        },
    ];

    for row in rows {
        let label = format!("{} written through the published uri", row.case);
        let written = graphql(
            &emulator,
            &label,
            &api,
            &json!({
                "query": "mutation Put($id: ID!, $title: String, $views: Int) { \
                          putPost(id: $id, title: $title, views: $views) { id title views } }",
                "variables": {"id": row.id, "title": row.title, "views": row.views},
            }),
        );
        assert_eq!(
            (
                written["data"]["putPost"]["id"].as_str(),
                written["data"]["putPost"]["title"].as_str(),
                written["data"]["putPost"]["views"].as_i64(),
                written["errors"].as_array(),
            ),
            (Some(row.id), Some(row.title), Some(row.views), None),
            "case {label:?}: the mutation must write the item and report it, got {written}; the \
             emulator's log:\n{}",
            emulator.log()
        );

        // What the endpoint reported is what DynamoDB holds: read through that service's own
        // api, so the round trip is backed by the owning service rather than by the answer.
        let held = held(&emulator, &label, row.id);
        assert_eq!(
            (
                held["Item"][KEY_ATTRIBUTE]["S"].as_str(),
                held["Item"]["title"]["S"].as_str(),
                held["Item"]["views"]["N"].as_str(),
            ),
            (
                Some(row.id),
                Some(row.title),
                Some(row.views.to_string().as_str())
            ),
            "case {label:?}: the table must hold what the mutation wrote, got {held}"
        );

        let label = format!("{} read back through the published uri", row.case);
        let read = graphql(
            &emulator,
            &label,
            &api,
            &json!({
                "query": "query Get($id: ID!) { getPost(id: $id) { id title views } }",
                "variables": {"id": row.id},
            }),
        );
        assert_eq!(
            (
                read["data"]["getPost"]["id"].as_str(),
                read["data"]["getPost"]["title"].as_str(),
                read["data"]["getPost"]["views"].as_i64(),
                read["errors"].as_array(),
            ),
            (Some(row.id), Some(row.title), Some(row.views), None),
            "case {label:?}: the query must read the item the mutation wrote, got {read}"
        );
    }

    // A key the table holds no item for resolves to nothing — the same endpoint, answering
    // what that service holds.
    let label = "a key the table holds no item for";
    let read = graphql(
        &emulator,
        label,
        &api,
        &json!({
            "query": "query Get($id: ID!) { getPost(id: $id) { id title views } }",
            "variables": {"id": "post-absent"},
        }),
    );
    assert_eq!(
        (&read["data"]["getPost"], read["errors"].as_array()),
        (&Value::Null, None),
        "case {label:?}: an absent item resolves to nothing, got {read}"
    );

    // The published address ANSWERS whatever is sent to it, which is what makes it reachable:
    // a request the API does not authorize earns a refusal rather than silence, and so does
    // one addressed with a method the endpoint serves nothing for. A client has to be able to
    // tell "not authorized" from "nothing is listening here".
    let uri = api.api["uris"]["GRAPHQL"].as_str().unwrap_or_default();
    let document = json!({"query": "query Get($id: ID!) { getPost(id: $id) { id } }",
                          "variables": {"id": "post-1"}});
    let unauthorized: &[(&str, Option<&str>)] = &[
        ("no material at all", None),
        ("a key this api did not mint", Some("da2-notakeythisapiminted")),
    ];
    for (label, key) in unauthorized {
        let mut sent = emulator
            .client()
            .post(uri)
            .header("content-type", "application/json");
        if let Some(key) = key {
            sent = sent.header("x-api-key", *key);
        }
        let answered = sent.body(document.to_string()).send().unwrap_or_else(|error| {
            panic!(
                "case {label:?}: the published address {uri} must answer, got {error}; the \
                 emulator's log:\n{}",
                emulator.log()
            )
        });
        let status = answered.status().as_u16();
        let body = answered.text().unwrap_or_default();
        assert!(
            (400..500).contains(&status),
            "case {label:?}: the published endpoint must refuse the call, got {status} and \
             {body}"
        );
    }
    let answered = emulator.client().get(uri).send().unwrap_or_else(|error| {
        panic!(
            "case \"a read of the published address\": {uri} must answer, got {error}; the \
             emulator's log:\n{}",
            emulator.log()
        )
    });
    let status = answered.status().as_u16();
    assert!(
        status > 0,
        "case \"a read of the published address\": the endpoint must answer, got {status}"
    );
}

/// The scheme, the authority and the path a published URI carries, read out of the URI itself.
fn split_uri(case: &str, uri: &str) -> (String, String, String) {
    let (scheme, rest) = uri
        .split_once("://")
        .unwrap_or_else(|| panic!("case {case:?}: expected a published uri, got {uri:?}"));
    let (authority, path) = rest
        .split_once('/')
        .unwrap_or_else(|| panic!("case {case:?}: expected a path, got {uri:?}"));
    (scheme.to_string(), authority.to_string(), format!("/{path}"))
}

/// One created GraphQL API: how it is addressed, the key a client presents and what it
/// published.
struct Api {
    api_id: String,
    api_key: String,
    /// The `graphqlApi` the create answered, which is what publishes the addresses.
    api: Value,
}

/// POST one GraphQL document to the URL the API PUBLISHED, as a client that was handed that
/// URL does: the scheme, the host, the port and the path all come from `uris.GRAPHQL`, the
/// only material presented is the API's own key, and the answer is the document the endpoint
/// served.
fn graphql(emulator: &Started, case: &str, api: &Api, document: &Value) -> Value {
    let uri = api.api["uris"]["GRAPHQL"].as_str().unwrap_or_default();
    let response = emulator
        .client()
        .post(uri)
        .header("content-type", "application/json")
        .header("x-api-key", &api.api_key)
        .body(document.to_string())
        .send()
        .unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the published address {uri} must answer, got {error}; the \
                 emulator's log:\n{}",
                emulator.log()
            )
        });
    let status = response.status().as_u16();
    let body = response.text().unwrap_or_default();
    assert_eq!(
        status, 200,
        "case {case:?}: the published endpoint must serve the document, got {status} and \
         {body}"
    );
    serde_json::from_str(&body)
        .unwrap_or_else(|_| panic!("case {case:?}: expected a GraphQL answer, got {body:?}"))
}

/// One Amazon DynamoDB call over its own `awsJson1_0` wire, answered as a document.
fn dynamodb(emulator: &Started, case: &str, operation: &str, body: &Value) -> Value {
    let (status, answered) = emulator.rpc(
        "dynamodb",
        &format!("DynamoDB_20120810.{operation}"),
        "application/x-amz-json-1.0",
        body,
    );
    assert_eq!(
        status, 200,
        "case {case:?}: DynamoDB {operation} must answer 200, got {status} and {answered}"
    );
    serde_json::from_str(&answered).unwrap_or(Value::Null)
}

/// What the table holds for `id`, read through DynamoDB's OWN api.
fn held(emulator: &Started, case: &str, id: &str) -> Value {
    dynamodb(
        emulator,
        case,
        "GetItem",
        &json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": id}}}),
    )
}

/// Create the table every resolver resolves against and wait until DynamoDB first reports it
/// `ACTIVE` — a table that is not is one no call lands on. The wait is on the CONDITION, so
/// nothing here depends on how fast the host runs.
fn create_table(emulator: &Started) {
    let case = "the table the data source names";
    dynamodb(
        emulator,
        case,
        "CreateTable",
        &json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    );
    let giving_up_at = Instant::now() + SETTLE_DEADLINE;
    let mut described = Value::Null;
    while Instant::now() < giving_up_at {
        described = dynamodb(emulator, case, "DescribeTable", &json!({"TableName": TABLE}));
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        std::thread::sleep(LOOK_INTERVAL);
    }
    panic!("case {case:?}: the table must become ACTIVE, last read {described}");
}

/// Create the role the data source makes its calls under, with a policy admitting the DynamoDB
/// actions on the table, and answer its ARN.
fn create_role(emulator: &Started) -> String {
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{APPSYNC_PRINCIPAL}\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    );
    let (status, created) = emulator.form_call(
        "iam",
        "2010-05-08",
        "CreateRole",
        &[("RoleName", ROLE), ("AssumeRolePolicyDocument", &trust)],
    );
    assert_eq!(
        status, 200,
        "setup: CreateRole must answer 200, got {status} and {created}"
    );
    let arn = element(&created, "Arn");
    let policy = "{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\
                  \"Action\":\"dynamodb:*\",\"Resource\":\"*\"}]}";
    let (status, answered) = emulator.form_call(
        "iam",
        "2010-05-08",
        "PutRolePolicy",
        &[
            ("RoleName", ROLE),
            ("PolicyName", "data-source-access"),
            ("PolicyDocument", policy),
        ],
    );
    assert_eq!(
        status, 200,
        "setup: PutRolePolicy must answer 200, got {status} and {answered}"
    );
    arn
}

/// One AppSync control-plane call (restJson1), answered as a document.
fn appsync(emulator: &Started, case: &str, method: &str, path: &str, body: &Value) -> Value {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let (status, answered) = emulator.call(
        "appsync",
        method,
        path,
        &[("content-type", "application/json")],
        payload,
    );
    assert_eq!(
        status, 200,
        "setup: {case} ({method} {path}) must answer 200, got {status} and {answered}"
    );
    serde_json::from_str(&answered).unwrap_or(Value::Null)
}

/// Create the API with an API key, install [`SCHEMA`], attach the DynamoDB data source and the
/// two resolvers the rows drive.
fn create_api(emulator: &Started, role_arn: &str) -> Api {
    let created = appsync(
        emulator,
        "CreateGraphqlApi",
        "POST",
        "/v1/apis",
        &json!({"name": "published-endpoint-posts", "authenticationType": "API_KEY"}),
    );
    let api = created["graphqlApi"].clone();
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();
    let key = appsync(
        emulator,
        "CreateApiKey",
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        &json!({"description": "the suite's key"}),
    );
    let api_key = key["apiKey"]["id"].as_str().unwrap_or_default().to_string();

    appsync(
        emulator,
        "StartSchemaCreation",
        "POST",
        &format!("/v1/apis/{api_id}/schemacreation"),
        &json!({"definition": ::aws_smithy_types::base64::encode(SCHEMA)}),
    );
    let giving_up_at = Instant::now() + SETTLE_DEADLINE;
    loop {
        let settled = appsync(
            emulator,
            "GetSchemaCreationStatus",
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            &Value::Null,
        );
        if settled["status"].as_str() == Some("SUCCESS") {
            break;
        }
        assert!(
            Instant::now() < giving_up_at,
            "setup: the schema creation must be performed, last read {settled}"
        );
        std::thread::sleep(LOOK_INTERVAL);
    }

    appsync(
        emulator,
        "CreateDataSource",
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        &json!({
            "name": DATA_SOURCE,
            "type": "AMAZON_DYNAMODB",
            "serviceRoleArn": role_arn,
            "dynamodbConfig": {"tableName": TABLE, "awsRegion": REGION},
        }),
    );
    let resolvers: &[(&str, &str, String)] = &[
        (
            "Mutation",
            "putPost",
            format!(
                "{{\"version\": \"2017-02-28\", \"operation\": \"PutItem\", \
                 \"key\": {{\"{KEY_ATTRIBUTE}\": \
                 $util.dynamodb.toDynamoDBJson($ctx.args.id)}}, \
                 \"attributeValues\": {{\
                 \"title\": $util.dynamodb.toDynamoDBJson($ctx.args.title), \
                 \"views\": $util.dynamodb.toDynamoDBJson($ctx.args.views)}}}}"
            ),
        ),
        (
            "Query",
            "getPost",
            format!(
                "{{\"version\": \"2017-02-28\", \"operation\": \"GetItem\", \
                 \"key\": {{\"{KEY_ATTRIBUTE}\": \
                 $util.dynamodb.toDynamoDBJson($ctx.args.id)}}}}"
            ),
        ),
    ];
    for (type_name, field, request) in resolvers {
        appsync(
            emulator,
            "CreateResolver",
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            &json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate": request,
                "responseMappingTemplate": "$util.toJson($ctx.result)",
            }),
        );
    }
    Api {
        api_id,
        api_key,
        api,
    }
}
