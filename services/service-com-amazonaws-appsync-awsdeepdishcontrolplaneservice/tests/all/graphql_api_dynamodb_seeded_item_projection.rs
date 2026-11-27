// SPDX-License-Identifier: CC0-1.0

//! The whole discovery path a client walks to read a DynamoDB-backed field: submit a schema,
//! wait for the creation to settle, take the address the API PUBLISHES, and select a field
//! resolved over an item another client seeded straight into the table.
//!
//! The rules these cases assert, one level above the recorded ones (one `CreateGraphqlApi`
//! whose `uris.GRAPHQL` a check POSTs to, and one `GetItem` resolver over a seeded item):
//!
//! > A schema submitted for an API settles on a TERMINAL `SchemaStatus` in bounded time, and
//! > every later `GetSchemaCreationStatus` reports that same terminal status rather than a
//! > status a poller never leaves. Each entry of the API's `uris` is a URL over the `host:port`
//! > the `dns` record of the same endpoint names, which is a socket THIS emulator answers on.
//! > And a field bound to an `AMAZON_DYNAMODB` data source answers the item the TABLE holds as
//! > the values its attributes hold — every attribute value read back as the JSON value its
//! > attribute type denotes, never as the attribute-value object the table holds it as —
//! > whoever wrote the item, while a key the table holds no item for resolves to nothing and is
//! > no error.
//!
//! The rows span what the projection rule generalises over: the attribute TYPE each member is
//! held as (`S`, `N`, `BOOL`, `SS`, `L`, `M`, `NULL`), more than one value per type (an integer
//! and a fraction, a negative number, both booleans, an empty collection), an item holding
//! nothing but its key, and a key the table holds nothing for. The attribute types are the ones
//! `$util.dynamodb`'s typed helpers name, the same catalog the marshalling suite
//! ([`crate::graphql_api_dynamodb_marshalling`]) is written over — this suite drives the other
//! direction, over items the resolver never wrote.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::{appsync, loopback_port};
use crate::graphql_api_iam_realtime_dynamodb_flow::{create_role, dynamodb, emulator};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_published_uri::split_uri;

/// The Region the API and the table live in, which is also the Region every call is signed for.
const REGION: &str = "us-west-2";

/// The table the data source is bound to, and the data source bound to it.
const TABLE: &str = "seeded-widgets";
const DATA_SOURCE: &str = "widgets";

/// How long the setup waits for a state the clock owes, and how often it looks again. Generous:
/// what is under test is what the endpoint answers, never how fast the host gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The schema the API is created with: one field per attribute type a row seeds, each declared
/// as the GraphQL type the projected value has to fit.
const SCHEMA: &str = "schema { query: Query }\n\
     type Nested {\n  label: String\n  score: Int\n}\n\
     type Widget {\n  id: ID!\n  name: String\n  count: Int\n  ratio: Float\n  \
     active: Boolean\n  tags: [String]\n  labels: [String]\n  nothing: String\n  \
     nested: Nested\n}\n\
     type Query {\n  getWidget(id: ID!): Widget\n}\n";

/// The selection every case asks for: every member of the row, the nested one expanded.
const ROW: &str = "id name count ratio active tags labels nothing nested { label score }";

/// The scheme and the path each published endpoint is addressed over.
const SCHEMES: &[(&str, &str)] = &[("GRAPHQL", "https"), ("REALTIME", "wss")];

/// The statuses `SchemaStatus` is modelled with (`smithy_query` shape `SchemaStatus`), and the
/// ones that are TERMINAL: a creation that settled on one of these is a creation a poller
/// leaves.
const SCHEMA_STATUSES: &[&str] = &[
    "PROCESSING",
    "ACTIVE",
    "DELETING",
    "FAILED",
    "SUCCESS",
    "NOT_APPLICABLE",
];
const TERMINAL_STATUSES: &[&str] = &["SUCCESS", "ACTIVE", "FAILED"];

/// One row: the item seeded into the table (its attribute values as the table holds them) and
/// the widget the field has to answer for that key.
struct Row<'a> {
    case: &'a str,
    id: &'a str,
    seeded: Option<Value>,
    answered: Value,
}

fn rows() -> Vec<Row<'static>> {
    vec![
        Row {
            case: "an item whose members span every attribute type",
            id: "W-full",
            seeded: Some(json!({
                "name": {"S": "first"},
                "count": {"N": "7"},
                "ratio": {"N": "1.5"},
                "active": {"BOOL": true},
                "tags": {"SS": ["a", "b"]},
                "labels": {"L": [{"S": "x"}, {"S": "y"}]},
                "nothing": {"NULL": true},
                "nested": {"M": {"label": {"S": "inner"}, "score": {"N": "3"}}},
            })),
            answered: json!({
                "id": "W-full",
                "name": "first",
                "count": 7,
                "ratio": 1.5,
                "active": true,
                "tags": ["a", "b"],
                "labels": ["x", "y"],
                "nothing": Value::Null,
                "nested": {"label": "inner", "score": 3},
            }),
        },
        Row {
            case: "another value of each of those types",
            id: "W-other",
            seeded: Some(json!({
                "name": {"S": "second"},
                "count": {"N": "-12"},
                "ratio": {"N": "0.25"},
                "active": {"BOOL": false},
                "tags": {"SS": ["only"]},
                "labels": {"L": []},
                "nested": {"M": {}},
            })),
            answered: json!({
                "id": "W-other",
                "name": "second",
                "count": -12,
                "ratio": 0.25,
                "active": false,
                "tags": ["only"],
                "labels": [],
                "nothing": Value::Null,
                "nested": {"label": Value::Null, "score": Value::Null},
            }),
        },
        Row {
            case: "an item holding nothing but its key",
            id: "W-bare",
            seeded: Some(json!({})),
            answered: json!({
                "id": "W-bare",
                "name": Value::Null,
                "count": Value::Null,
                "ratio": Value::Null,
                "active": Value::Null,
                "tags": Value::Null,
                "labels": Value::Null,
                "nothing": Value::Null,
                "nested": Value::Null,
            }),
        },
        Row {
            case: "a key the table holds no item for",
            id: "W-absent",
            seeded: None,
            answered: Value::Null,
        },
    ]
}

#[tokio::test]
async fn a_dynamodb_backed_field_read_at_the_published_uri_answers_what_the_table_holds() {
    let router = emulator();
    let api = given_api(&router).await;

    // Every seeded row is held as the ATTRIBUTE VALUES a client wrote, so the projection the
    // cases assert on is a reading of what the table holds rather than of what the resolver
    // happened to be handed.
    for row in rows() {
        let Some(members) = row.seeded else { continue };
        let mut item = members.clone();
        item["id"] = json!({"S": row.id});
        dynamodb(&router, "PutItem", json!({"TableName": TABLE, "Item": item})).await;
        let held = dynamodb(
            &router,
            "GetItem",
            json!({"TableName": TABLE, "Key": {"id": {"S": row.id}}}),
        )
        .await;
        assert_eq!(
            held.get("Item"),
            Some(&item),
            "case {:?}: the table must hold the attribute values the client seeded, got {held}",
            row.case
        );
    }

    for row in rows() {
        let case = row.case;
        let answered = post(
            &api,
            case,
            &json!({
                "query": format!("query Read($id: ID!) {{ getWidget(id: $id) {{ {ROW} }} }}"),
                "variables": {"id": row.id},
            }),
        )
        .await;
        assert_eq!(
            answered.get("errors"),
            None,
            "case {case:?}: reading an item the table holds is no error, got {answered}"
        );
        assert_eq!(
            answered.pointer("/data/getWidget"),
            Some(&row.answered),
            "case {case:?}: expected the item as the values its attributes hold, {}, got \
             {answered}",
            row.answered
        );
    }
}

#[tokio::test]
async fn a_submitted_schema_settles_on_a_terminal_status_every_later_read_reports() {
    let router = emulator();
    let case = "a submitted schema";
    let (status, created) = appsync(
        &router,
        "POST",
        "/v1/apis",
        Some(json!({"name": "seeded_projection_schema", "authenticationType": "API_KEY"})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The submit itself reports a status of the modelled set, and the creation then SETTLES:
    // the wait is on that condition, with a deadline generous enough that what it pins is that
    // the status becomes terminal at all rather than how fast the host gets there.
    let settled = settle_schema(&router, &api_id, SCHEMA).await;
    let reported = settled["status"].as_str().unwrap_or_default().to_string();
    assert!(
        TERMINAL_STATUSES.contains(&reported.as_str()),
        "case {case:?}: expected one of {TERMINAL_STATUSES:?}, got {settled}"
    );

    // And every later read reports the same terminal status: a reader never moves it.
    for read in 0..3 {
        let (status, again) = appsync(
            &router,
            "GET",
            &format!("/v1/apis/{api_id}/schemacreation"),
            None,
        )
        .await;
        assert_eq!(
            (status, again["status"].as_str()),
            (200, Some(reported.as_str())),
            "case {case:?}: read {read} must report the status the creation settled on, got \
             {status} and {again}"
        );
    }
}

/// One created API: the address its GraphQL endpoint publishes, the path it serves and a live
/// API key.
struct Api {
    authority: String,
    path: String,
    api_key: String,
}

/// POST one document to the address the API published, as a client that parsed `uris.GRAPHQL`
/// does, answering the document it was served.
async fn post(api: &Api, case: &str, body: &Value) -> Value {
    let (status, _, answered) = try_post_answer(
        case,
        &api.authority,
        &api.path,
        "POST",
        body,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| {
        panic!(
            "case {case:?}: the published address {}{} must be one this emulator answers on",
            api.authority, api.path
        )
    });
    assert_eq!(
        status, 200,
        "case {case:?}: a keyed GraphQL request is answered 200, got {status} and {answered}"
    );
    answered
}

/// Submit `document` as the API's schema, wait until the creation settles, and answer the
/// status read it settled on.
async fn settle_schema(router: &MultiServiceRouter, api_id: &str, document: &str) -> Value {
    install_schema(router, api_id, document).await;
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
    let reported = settled["status"].as_str().unwrap_or_default();
    assert!(
        SCHEMA_STATUSES.contains(&reported),
        "GetSchemaCreationStatus must report a modelled status, got {settled}"
    );
    settled
}

/// Create the table the data source reaches, and wait until DynamoDB first reports it `ACTIVE`.
async fn create_table(router: &MultiServiceRouter) -> String {
    let created = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": "id", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "id", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let arn = created["TableDescription"]["TableArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !arn.is_empty(),
        "setup: CreateTable must report the ARN the table is held under, got {created}"
    );
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let described = dynamodb(router, "DescribeTable", json!({"TableName": TABLE})).await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return arn;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "setup: the table must become ACTIVE, last read {described}"
        );
        tokio::time::sleep(POLL_SPACING).await;
    }
}

/// Create the API, its key, its settled schema, the data source over the table and the one
/// resolver, and answer the address a client reaches by PARSING what the API published.
async fn given_api(router: &MultiServiceRouter) -> Api {
    let table_arn = create_table(router).await;
    let role = create_role(router, "seeded-projection-role", &table_arn).await;
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": "seeded_projection", "authenticationType": "API_KEY"})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api = created["graphqlApi"].clone();
    let api_id = api["apiId"].as_str().unwrap_or_default().to_string();

    // Both published addresses are this emulator's own: each is a URL over the `host:port` the
    // endpoint's `dns` record names, on the loopback listener the core binds. A host belonging
    // to another server fails here, and with it the whole read below, because a client has no
    // other address for the API's data plane.
    let mut reached: Vec<(String, String)> = Vec::new();
    for (endpoint, scheme) in SCHEMES {
        let case = format!("the published {endpoint} endpoint");
        let published = api["uris"][endpoint]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: the api must publish this endpoint: {api}"));
        let (announced, authority, path) = split_uri(&case, published);
        assert_eq!(
            (
                announced.as_str(),
                Some(::service_common::data_plane_addressing::host_only(&authority))
            ),
            (*scheme, api["dns"][endpoint].as_str()),
            "case {case:?}: expected the scheme the endpoint is served over, over a socket \
             whose bare host is the endpoint's own dns record, got {published:?} and {api}"
        );
        assert!(
            loopback_port(&case, &authority) > 0 && !published.contains("amazonaws.com"),
            "case {case:?}: a published address is a socket this emulator answers on, never a \
             host of another server, got {published:?}"
        );
        reached.push((authority, path));
    }

    let (status, key) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/apikeys"),
        Some(json!({"description": "the suite's key"})),
    )
    .await;
    assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
    settle_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "AMAZON_DYNAMODB",
            "serviceRoleArn": role,
            "dynamodbConfig": {"tableName": TABLE, "awsRegion": REGION},
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    let (status, resolver) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/types/Query/resolvers"),
        Some(json!({
            "fieldName": "getWidget",
            "kind": "UNIT",
            "dataSourceName": DATA_SOURCE,
            "requestMappingTemplate":
                "{\"version\": \"2018-05-29\", \"operation\": \"GetItem\", \
                 \"key\": {\"id\": $util.dynamodb.toDynamoDBJson($ctx.args.id)}}",
            "responseMappingTemplate":
                "#if($ctx.error)\n  $util.error($ctx.error.message, $ctx.error.type)\n#end\n\
                 $util.toJson($ctx.result)\n",
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateResolver must answer 200, got {resolver}");
    let (authority, path) = reached.swap_remove(0);
    Api {
        authority,
        path,
        api_key: key["apiKey"]["id"].as_str().unwrap_or_default().to_string(),
    }
}
