// SPDX-License-Identifier: CC0-1.0

//! A UNIT resolver over an `AMAZON_DYNAMODB` data source, driven over the API's own published
//! `GRAPHQL` endpoint with the mapping templates AWS documents for that data source.
//!
//! The rules these cases assert, one level above the recorded ones:
//!
//! > A field bound to such a data source is answered by making the call its request object
//! > names against the table the data source was configured with, so a mutation that writes an
//! > item answers the item it wrote and a query of the same key reads it back — each attribute
//! > as the value it holds rather than as the attribute value the table holds it as. A write
//! > the request made CONDITIONAL is weighed by the service whose expression language the
//! > condition is written in, so among writers racing to create one key exactly one lands and
//! > every other is reported as that service worded its refusal. Requests a client makes AT
//! > THE SAME TIME are all served — more of them at once than the endpoint reads at once is a
//! > request that takes its turn, never one refused because another was in flight. And the
//! > API's own authorization decides the request before any of that: a call presenting no
//! > material at all reaches no field of an `AWS_IAM` API and is refused `401`, while the same
//! > document signed is served.
//!
//! The rows span what those rules generalise over: the kind of value a round trip carries (a
//! string, a number, a list), how many requests are in flight at once — more than the
//! endpoint's own in-flight count either way, reading and writing — and the two outcomes the
//! authorization decision has for one document.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api_data_plane::appsync;
use crate::graphql_api_iam_realtime_dynamodb_flow::{create_role, dynamodb};
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};

/// The Region every call of this suite is signed for, which is also the Region its APIs are
/// created in.
const REGION: &str = "us-west-2";

/// The path the API's GraphQL endpoint is served at.
const GRAPHQL_PATH: &str = "/graphql";

/// The table each case resolves against (one per case, named for it) and the data source bound
/// to it.
const TABLE: &str = "round-trip-items";
const DATA_SOURCE: &str = "items";

/// How long a case waits for a state the clock owes, and how often it looks again. Generous:
/// what is under test is what the endpoint answers, never how fast the host gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The schema every API of this suite is created with.
const SCHEMA: &str = "schema { query: Query mutation: Mutation }\n\
     type Item {\n  id: ID!\n  name: String\n  count: Int\n  tags: [String]\n}\n\
     type Query {\n  getItem(id: ID!): Item\n}\n\
     type Mutation {\n  \
     putItem(id: ID!, name: String, count: Int, tags: [String]): Item\n  \
     createOnce(id: ID!, name: String): Item\n}\n";

/// The selection every case asks for.
const ROW: &str = "id name count tags";

/// The class Amazon DynamoDB refuses a write whose condition did not hold with, as a field of
/// this endpoint reports a refusal of the service it came from.
const CONDITION_REFUSED: &str = "DynamoDB:ConditionalCheckFailedException";

/// How many requests a case makes AT ONCE: more than the endpoint reads at once
/// (`emulator_server::listener_caps::MAX_HTTP_INFLIGHT`, 16 at the time of writing), so a case
/// covers the request that finds every in-flight permit taken as well as the ones that do not.
const AT_ONCE: usize = 24;

#[tokio::test]
async fn a_unit_resolver_over_a_dynamodb_data_source_round_trips_the_item_it_wrote() {
    let api = given_api(&emulator(), "round-trip", "API_KEY").await;
    let case = "a DynamoDB unit resolver round trip";

    // The item spans the kinds of value a row carries: a string, a number and a list.
    let written = post(
        &api,
        case,
        &format!(
            "mutation Write($id: ID!, $name: String, $count: Int, $tags: [String]) \
             {{ putItem(id: $id, name: $name, count: $count, tags: $tags) {{ {ROW} }} }}"
        ),
        json!({"id": "I-1", "name": "first", "count": 7, "tags": ["a", "b"]}),
    )
    .await;
    assert_eq!(
        (written.get("errors"), written.pointer("/data/putItem")),
        (
            None,
            Some(&json!({"id": "I-1", "name": "first", "count": 7, "tags": ["a", "b"]})),
        ),
        "case {case:?}: the mutation must answer the item it wrote, got {written}"
    );

    let read = post(
        &api,
        case,
        &format!("query Read($id: ID!) {{ getItem(id: $id) {{ {ROW} }} }}"),
        json!({"id": "I-1"}),
    )
    .await;
    assert_eq!(
        (read.get("errors"), read.pointer("/data/getItem")),
        (
            None,
            Some(&json!({"id": "I-1", "name": "first", "count": 7, "tags": ["a", "b"]})),
        ),
        "case {case:?}: the query must read back what the mutation wrote — each value as the \
         value it holds — got {read}"
    );

    // A key the table holds no row for resolves to nothing, and is no error.
    let absent = post(
        &api,
        case,
        &format!("query Read($id: ID!) {{ getItem(id: $id) {{ {ROW} }} }}"),
        json!({"id": "I-absent"}),
    )
    .await;
    assert_eq!(
        (absent.get("errors"), absent.pointer("/data/getItem")),
        (None, Some(&Value::Null)),
        "case {case:?}: a key the table holds nothing for resolves to nothing, got {absent}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_conditional_mutations_have_one_winner_and_the_losers_carry_the_refusal() {
    let api = given_api(&emulator(), "conditional", "API_KEY").await;
    let case = "concurrent conditional writes of one key";

    // The writers race for ONE key, and each states the same condition: the row must not be
    // there. There are more of them in flight than the endpoint reads at once, so the single
    // winner is the rule over every request — including the ones that had to take their turn —
    // rather than over the order they arrived in.
    let mut running = Vec::new();
    for writer in 0..AT_ONCE {
        let api = api.clone();
        running.push(tokio::spawn(async move {
            post(
                &api,
                "a racing writer",
                &format!(
                    "mutation Once($id: ID!, $name: String) \
                     {{ createOnce(id: $id, name: $name) {{ {ROW} }} }}"
                ),
                json!({"id": "C-1", "name": format!("writer-{writer}")}),
            )
            .await
        }));
    }
    let mut answers = Vec::new();
    for task in running {
        answers.push(task.await.expect("the request completes"));
    }

    let won: Vec<&Value> = answers
        .iter()
        .filter(|answered| answered.get("errors").is_none())
        .collect();
    assert_eq!(
        won.len(),
        1,
        "case {case:?}: exactly one conditional write lands, got {answers:?}"
    );
    for answered in &answers {
        if answered.get("errors").is_none() {
            continue;
        }
        let reported = answered["errors"].as_array().cloned().unwrap_or_default();
        assert_eq!(
            (
                reported.len(),
                reported.first().and_then(|error| error["errorType"].as_str()),
                answered.pointer("/data/createOnce"),
            ),
            (1, Some(CONDITION_REFUSED), Some(&Value::Null)),
            "case {case:?}: a refused write reports one error of the class DynamoDB worded it \
             with, got {answered}"
        );
    }

    // The row the winner wrote is the one the table holds: the losers wrote nothing.
    let held = post(
        &api,
        case,
        &format!("query Read($id: ID!) {{ getItem(id: $id) {{ {ROW} }} }}"),
        json!({"id": "C-1"}),
    )
    .await;
    assert_eq!(
        held.pointer("/data/getItem/name"),
        won[0].pointer("/data/createOnce/name"),
        "case {case:?}: the table holds the row the winner wrote, got {held}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn every_request_a_client_makes_at_the_same_time_is_served() {
    let api = given_api(&emulator(), "at-once", "API_KEY").await;
    let case = "more requests in flight at once than the endpoint reads at once";

    // Every one of these is in flight at the same time, and there are more of them than the
    // endpoint holds permits for: a request that finds every permit taken waits for its turn,
    // so each is answered the field's own result — its own item, written under its own key.
    // One refused because another was in flight would carry no `data` at all, which is what
    // this case would then see.
    let mut running = Vec::new();
    for at in 0..AT_ONCE {
        let api = api.clone();
        running.push(tokio::spawn(async move {
            let id = format!("A-{at}");
            let answered = post(
                &api,
                "one of many at once",
                &format!(
                    "mutation Write($id: ID!, $name: String, $count: Int, $tags: [String]) \
                     {{ putItem(id: $id, name: $name, count: $count, tags: $tags) \
                     {{ {ROW} }} }}"
                ),
                json!({
                    "id": id,
                    "name": format!("name-{at}"),
                    "count": at,
                    "tags": [format!("tag-{at}")],
                }),
            )
            .await;
            (id, at, answered)
        }));
    }
    let mut answered = Vec::new();
    for task in running {
        answered.push(task.await.expect("the request completes"));
    }
    assert_eq!(
        answered.len(),
        AT_ONCE,
        "case {case:?}: every request must be answered, got {answered:?}"
    );
    for (id, at, one) in &answered {
        assert_eq!(
            (one.get("errors"), one.pointer("/data/putItem")),
            (
                None,
                Some(&json!({
                    "id": id,
                    "name": format!("name-{at}"),
                    "count": at,
                    "tags": [format!("tag-{at}")],
                })),
            ),
            "case {case:?}: every request in flight at once is served its own result, got {one}"
        );
    }
}

#[tokio::test]
async fn an_unsigned_request_to_an_iam_authenticated_api_is_refused() {
    let api = given_api(&emulator(), "unsigned", "AWS_IAM").await;
    let case = "an unsigned POST to an AWS_IAM API";
    let body = json!({
        "query": format!("query Read($id: ID!) {{ getItem(id: $id) {{ {ROW} }} }}"),
        "variables": {"id": "U-1"},
    });

    let (status, _, refused) =
        try_post_answer(case, &api.graphql, GRAPHQL_PATH, "POST", &body, &[])
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the endpoint must answer this request"));
    let reported = refused["errors"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        (
            status,
            reported.len(),
            reported.first().and_then(|error| error["errorType"].as_str()),
        ),
        (401, 1, Some("UnauthorizedException")),
        "case {case:?}: a call presenting no material is refused 401 under the class AWS words \
         an unauthorized call with, got {status} and {refused}"
    );

    // The same document, SIGNED, is served: what the refusal turns on is the material the call
    // presents, not the document it carries.
    let (status, _, served) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &body,
        &[("authorization", &sigv4_material())],
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the endpoint must answer this request"));
    assert_eq!(
        (status, served.get("errors")),
        (200, None),
        "case {case:?}: a signed call is served, got {status} and {served}"
    );
}

/// The SigV4 material a signed call presents, under the signing name this data plane is signed
/// for. The signature itself is not verified by a service — the emulator verifies request
/// signatures at its front door — so what a case pins is what the endpoint answers material of
/// this mode.
fn sigv4_material() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// One created API: the address its GraphQL endpoint is served on, and a live API key where the
/// API is served for one.
#[derive(Clone)]
struct Api {
    graphql: String,
    api_key: String,
}

/// POST one document to the API's published endpoint, as a client does, answering the document
/// it was served.
async fn post(api: &Api, case: &str, document: &str, variables: Value) -> Value {
    let body = json!({"query": document, "variables": variables});
    let (status, _, answered) = try_post_answer(
        case,
        &api.graphql,
        GRAPHQL_PATH,
        "POST",
        &body,
        &[("x-api-key", api.api_key.as_str())],
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: the published endpoint must answer"));
    assert_eq!(
        status, 200,
        "case {case:?}: a keyed GraphQL request is answered 200, got {status} and {answered}"
    );
    answered
}

/// A router with the services this topology spans, over ONE engine: IAM and STS hold the data
/// source's service role, DynamoDB the table, and AppSync the API that resolves against it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // What a case pins is what the endpoint ANSWERS for the material a call presents, never
    // signature verification, which the emulator's front door owns.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(EmulatedService::new(engine));
    router
}

/// Create the table a case resolves against, named for that case so each holds its own rows,
/// and wait until DynamoDB first reports it `ACTIVE`.
async fn create_table(router: &MultiServiceRouter, name: &str) -> String {
    let created = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": name,
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
        let described = dynamodb(router, "DescribeTable", json!({"TableName": name})).await;
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

/// The request template of one field, as the documented templates of this data source are
/// written.
fn request_template(field: &str) -> &'static str {
    match field {
        "putItem" => {
            "#set($values = {\"name\": $ctx.args.name, \"count\": $ctx.args.count, \
             \"tags\": $ctx.args.tags})\n\
             {\"version\": \"2018-05-29\", \"operation\": \"PutItem\", \
             \"key\": {\"id\": $util.dynamodb.toDynamoDBJson($ctx.args.id)}, \
             \"attributeValues\": $util.dynamodb.toMapValuesJson($values)}"
        }
        // The write lands only where the row is not there, which is the grounds the caller
        // states for its own refusal.
        "createOnce" => {
            "#set($values = {\"name\": $ctx.args.name})\n\
             {\"version\": \"2018-05-29\", \"operation\": \"PutItem\", \
             \"key\": {\"id\": $util.dynamodb.toDynamoDBJson($ctx.args.id)}, \
             \"attributeValues\": $util.dynamodb.toMapValuesJson($values), \
             \"condition\": {\"expression\": \"attribute_not_exists(id)\"}}"
        }
        _ => {
            "{\"version\": \"2018-05-29\", \"operation\": \"GetItem\", \
             \"key\": {\"id\": $util.dynamodb.toDynamoDBJson($ctx.args.id)}}"
        }
    }
}

/// The response template every resolver of this suite carries: it re-raises what the data
/// source refused — which is how a refusal reaches the client as the class that service worded
/// it with — and answers the result otherwise.
const RESPONSE_TEMPLATE: &str =
    "#if($ctx.error)\n  $util.error($ctx.error.message, $ctx.error.type, $ctx.result)\n#end\n\
     $util.toJson($ctx.result)\n";

/// Create one API of `auth` with the schema, the table, the data source reached under a service
/// role and one resolver per field, and answer the address it publishes.
async fn given_api(router: &MultiServiceRouter, name: &str, auth: &str) -> Api {
    let table = format!("{TABLE}-{name}");
    let table_arn = create_table(router, &table).await;
    let role = create_role(router, &format!("round-trip-{name}-role"), &table_arn).await;
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": format!("round-trip-{name}"), "authenticationType": auth})),
    )
    .await;
    assert_eq!(status, 200, "CreateGraphqlApi must answer 200, got {created}");
    let api_id = created["graphqlApi"]["apiId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let graphql = crate::graphql_api_endpoint_dns_records::published_authority(
        &created["graphqlApi"],
        "GRAPHQL",
    );
    let mut api_key = String::new();
    if auth == "API_KEY" {
        let (status, key) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/apikeys"),
            Some(json!({"description": "the suite's key"})),
        )
        .await;
        assert_eq!(status, 200, "CreateApiKey must answer 200, got {key}");
        api_key = key["apiKey"]["id"].as_str().unwrap_or_default().to_string();
    }
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "AMAZON_DYNAMODB",
            "serviceRoleArn": role,
            "dynamodbConfig": {"tableName": table, "awsRegion": REGION},
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    for (type_name, field) in [
        ("Mutation", "putItem"),
        ("Mutation", "createOnce"),
        ("Query", "getItem"),
    ] {
        let (status, resolver) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "requestMappingTemplate": request_template(field),
                "responseMappingTemplate": RESPONSE_TEMPLATE,
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {type_name}.{field} must answer 200, got {resolver}"
        );
    }
    Api { graphql, api_key }
}
