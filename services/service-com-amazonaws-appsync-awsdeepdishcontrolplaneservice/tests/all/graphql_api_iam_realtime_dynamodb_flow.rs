// SPDX-License-Identifier: CC0-1.0

//! The whole recorded topology of an `AWS_IAM` GraphQL API, driven over the two sockets the
//! API publishes: a SIGNED real-time subscriber, a signed mutation that writes through an
//! `AMAZON_DYNAMODB` data-source resolver, the delivery that reaches the subscriber, and the
//! signed query that reads the written item back.
//!
//! The rule these cases assert, one level above the recorded case:
//!
//! > A real-time client of an `AWS_IAM` API completes the opening handshake on the address the
//! > API published for its `REALTIME` endpoint while carrying its SigV4 material in the
//! > base64url `header` parameter and the `Origin` a WebSocket client states for the address
//! > it dialled, is acked, and may subscribe; a signed mutation served at the API's `GRAPHQL`
//! > endpoint resolves against the table its data source names and is delivered to every
//! > subscription its schema links to that mutation field; and a signed query of the same
//! > table reads back exactly what the mutation wrote — which is what DynamoDB's own reader
//! > reports for that key.
//!
//! Three services have to agree for that to hold — IAM holds the data source's service role
//! and its policy, DynamoDB holds the table, and AppSync publishes both endpoints, runs the
//! resolver and delivers the mutation — so this is the suite that can pin the recorded
//! topology end to end. What the table holds is read back through DynamoDB's OWN api, so a
//! write that did not reach it cannot pass.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::event_api::LOOPBACK_HOST;
use crate::event_api_data_plane::appsync;
use crate::graphql_api_mapping_template_resolvers::{install_schema, try_post_answer};
use crate::graphql_api_realtime_endpoint::{exchange, handshake, next_frame, query_message};

/// The Region every call of this suite is signed for, which is also the Region the API is
/// created in — the `appsync` control-plane helper this suite shares signs for it.
pub(crate) const REGION: &str = "us-west-2";

/// The path each of the API's two endpoints is served at.
pub(crate) const REALTIME_PATH: &str = "/graphql";
pub(crate) const GRAPHQL_PATH: &str = "/graphql";

/// The table the resolvers of this suite resolve against, and its one key attribute.
pub(crate) const TABLE: &str = "appsync-realtime-notes";
pub(crate) const KEY_ATTRIBUTE: &str = "id";

/// The name of the `AMAZON_DYNAMODB` data source both resolvers are bound to.
const DATA_SOURCE: &str = "Notes";

/// The service principal a data source's service role is assumed by.
const APPSYNC_PRINCIPAL: &str = "appsync.amazonaws.com";

/// The `runtime` the resolvers of this suite are defined under. Source: `AppSyncRuntime`'s own
/// values (`smithy_query(query="shape",shape="AppSyncRuntime,RuntimeName")`).
const RUNTIME: (&str, &str) = ("APPSYNC_JS", "1.0.0");

/// How long a case waits for a state the clock owes, and how often it looks again. Generous:
/// what is under test is what the endpoints answer, never how fast the host gets there.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);
const POLL_SPACING: Duration = Duration::from_millis(100);

/// The schema the API is created with: one mutation, the subscription its `@aws_subscribe`
/// links to that mutation, and the query that reads an item back.
const SCHEMA: &str = "type Note {\n  id: ID!\n  text: String\n}\n\
                      \n\
                      type Query {\n  getNote(id: ID!): Note\n}\n\
                      \n\
                      type Mutation {\n  createNote(id: ID!, text: String): Note\n}\n\
                      \n\
                      type Subscription {\n  \
                      onCreateNote: Note @aws_subscribe(mutations: [\"createNote\"])\n}\n";

#[tokio::test]
async fn a_signed_subscriber_receives_the_mutation_a_dynamodb_resolver_wrote_and_a_query_reads_it_back(
) {
    let router = emulator();
    let table_arn = create_table(&router).await;
    let role = create_role(&router, "appsync-realtime-notes-role", &table_arn).await;
    let (graphql, realtime) = create_api(&router, &role).await;
    let case = "a signed real-time subscriber of an IAM-authenticated API";

    // The subscriber, as the recorded client dials one: the SigV4 material it would carry on a
    // signed request to the GRAPHQL endpoint, base64url-encoded into the handshake's `header`
    // parameter, and the `Origin` a WebSocket client states for the address it dialled.
    let origin = format!("https://{realtime}");
    let mut subscriber = handshake(
        case,
        &realtime,
        REALTIME_PATH,
        true,
        Some(&signed_auth_object(&graphql)),
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

    // The signed mutation, served at the API's own GraphQL endpoint and resolved against the
    // table the data source names.
    let written = signed_graphql(
        case,
        &graphql,
        &json!({
            "query": "mutation Write($id: ID!, $text: String) { \
                      createNote(id: $id, text: $text) { id text } }",
            "variables": {"id": "N-1", "text": "first"},
        }),
    )
    .await;
    assert_eq!(
        written.pointer("/data/createNote"),
        Some(&json!({"id": "N-1", "text": "first"})),
        "case {case:?}: the mutation must be resolved against the table, got {written}"
    );

    // It reached the subscriber over the socket the signed handshake established.
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
        "case {case:?}: expected the written note under the subscription field, got {delivered}"
    );

    // And the signed query reads back what the mutation wrote.
    let read = signed_graphql(
        case,
        &graphql,
        &json!({
            "query": "query Read($id: ID!) { getNote(id: $id) { id text } }",
            "variables": {"id": "N-1"},
        }),
    )
    .await;
    assert_eq!(
        read.pointer("/data/getNote"),
        Some(&json!({"id": "N-1", "text": "first"})),
        "case {case:?}: the query must read back what the mutation wrote, got {read}"
    );

    // What the table itself holds, through DynamoDB's own reader: the write really landed
    // there rather than only in the answer the endpoint gave.
    let held = dynamodb(
        &router,
        "GetItem",
        json!({"TableName": TABLE, "Key": {KEY_ATTRIBUTE: {"S": "N-1"}}}),
    )
    .await;
    assert_eq!(
        held.pointer("/Item"),
        Some(&json!({"id": {"S": "N-1"}, "text": {"S": "first"}})),
        "case {case:?}: expected the table to hold the written note, got {held}"
    );
}

/// A router with the three services the recorded topology spans, over ONE engine.
///
/// Shared with every suite that drives the same topology — the versioned data source
/// ([`crate::graphql_api_dynamodb_versioned_data_source`]) and the DynamoDB helper module a
/// resolver imports ([`crate::graphql_api_dynamodb_helper_module`]) — so they all stand the
/// services up one way.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // What a case pins is what the endpoints ANSWER for the identity the access key id
    // resolves to, never signature verification, which the emulator's front door owns.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(EmulatedService::new(engine));
    router
}

/// The SigV4 material a signed call presents, under the signing name AppSync's data plane is
/// signed for. The signature itself is not verified by a service — the emulator verifies
/// request signatures at its front door — so what a case pins is what the endpoint answers
/// material of this mode.
pub(crate) fn sigv4_material() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/appsync/aws4_request, \
         SignedHeaders=accept;content-encoding;content-type;host;x-amz-date, Signature=0000"
    )
}

/// The authorization object a signed real-time client carries in the handshake's `header`
/// parameter: the fields it would have signed on a request to the API's `GRAPHQL` endpoint,
/// `host` among them — which is that endpoint's authority and not the real-time one's.
pub(crate) fn signed_auth_object(graphql: &str) -> Value {
    json!({
        "accept": "application/json, text/javascript",
        "content-encoding": "amz-1.0",
        "content-type": "application/json; charset=UTF-8",
        "host": graphql,
        "x-amz-date": "20260101T000000Z",
        "Authorization": sigv4_material(),
    })
}

/// POST one signed GraphQL document to the address the API published, as a client does, and
/// answer the document it was served — asserting it was answered 200 and carried no errors.
pub(crate) async fn signed_graphql(case: &str, graphql: &str, document: &Value) -> Value {
    let (status, _, answered) = try_post_answer(
        case,
        graphql,
        GRAPHQL_PATH,
        "POST",
        document,
        &[("authorization", &sigv4_material())],
    )
    .await
    .unwrap_or_else(|| {
        panic!("case {case:?}: the published GraphQL endpoint {graphql:?} must answer")
    });
    assert_eq!(
        status, 200,
        "case {case:?}: a signed GraphQL request is answered 200, got {status} and {answered}"
    );
    assert_eq!(
        answered.get("errors"),
        None,
        "case {case:?}: the request must carry no errors, got {answered}"
    );
    answered
}

/// One Amazon DynamoDB call over its own wire (`awsJson1_0`, `POST /` with `X-Amz-Target`),
/// asserting it was served.
///
/// `pub(crate)`: the versioned-data-source suite
/// ([`crate::graphql_api_dynamodb_versioned_data_source`]) and the one that drives the DynamoDB
/// helper module a resolver imports ([`crate::graphql_api_dynamodb_helper_module`]) read what
/// the table holds through this same client, so every suite reaches that service one way.
pub(crate) async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/dynamodb/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "setup: DynamoDB {operation} must answer 200, got {status} and {answered}"
    );
    answered
}

/// One IAM call over its own wire (`awsQuery`, a form-encoded body), answering the XML it was
/// served.
async fn iam(router: &MultiServiceRouter, body: String) -> String {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("iam.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAEMULATOR/20260101/{REGION}/iam/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body.clone()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    assert_eq!(
        status, 200,
        "setup: iam {body} must answer 200, got {status} and {text}"
    );
    text
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Create the table the resolvers resolve against through DynamoDB's own API, wait until that
/// service first reports it `ACTIVE`, and answer the ARN it holds it under.
pub(crate) async fn create_table(router: &MultiServiceRouter) -> String {
    let created = dynamodb(
        router,
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{"AttributeName": KEY_ATTRIBUTE, "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": KEY_ATTRIBUTE, "AttributeType": "S"}],
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

/// Create the data source's service role — trusted by AppSync, allowed every DynamoDB action
/// on the table — and answer its ARN.
///
/// Both documents are written compactly and free of the characters an
/// `application/x-www-form-urlencoded` body treats specially (`&`, `=`, `+`, `%`, space), so
/// they need no encoding.
///
/// `pub(crate)`: every suite whose data source reaches a table needs such a role, so there is
/// one reading of how it is created.
pub(crate) async fn create_role(
    router: &MultiServiceRouter,
    name: &str,
    table_arn: &str,
) -> String {
    let trust = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Principal\":{{\"Service\":\"{APPSYNC_PRINCIPAL}\"}},\
         \"Action\":\"sts:AssumeRole\"}}]}}"
    );
    let created = iam(
        router,
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}\
             &AssumeRolePolicyDocument={trust}"
        ),
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup: CreateRole({name}) reports no <Arn>: {created}"));
    let document = format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Allow\",\
         \"Action\":[\"dynamodb:*\"],\"Resource\":\"{table_arn}\"}}]}}"
    );
    iam(
        router,
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}\
             &PolicyName=notes&PolicyDocument={document}"
        ),
    )
    .await;
    arn
}

/// Create the `AWS_IAM` API with [`SCHEMA`], the `AMAZON_DYNAMODB` data source reached under
/// `role_arn`, and one resolver per field, and answer the two addresses it publishes.
async fn create_api(router: &MultiServiceRouter, role_arn: &str) -> (String, String) {
    let (status, created) = appsync(
        router,
        "POST",
        "/v1/apis",
        Some(json!({"name": "realtime-notes", "authenticationType": "AWS_IAM"})),
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
    install_schema(router, &api_id, SCHEMA).await;
    let (status, source) = appsync(
        router,
        "POST",
        &format!("/v1/apis/{api_id}/datasources"),
        Some(json!({
            "name": DATA_SOURCE,
            "type": "AMAZON_DYNAMODB",
            "serviceRoleArn": role_arn,
            "dynamodbConfig": {"tableName": TABLE, "awsRegion": REGION},
        })),
    )
    .await;
    assert_eq!(status, 200, "CreateDataSource must answer 200, got {source}");
    let response_function = "export function response(ctx) {\n  \
                             if (ctx.error) {\n    \
                             util.error(ctx.error.message, ctx.error.type);\n  }\n  \
                             return ctx.result;\n}\n";
    let definition = |request: &str| {
        format!("import {{ util }} from '@aws-appsync/utils';\n{request}\n{response_function}")
    };
    for (type_name, field, request) in [
        (
            "Mutation",
            "createNote",
            "export function request(ctx) {\n  \
             return {operation: 'PutItem', \
             key: {id: util.dynamodb.toDynamoDB(ctx.args.id)}, \
             attributeValues: util.dynamodb.toMapValues({text: ctx.args.text})};\n}",
        ),
        (
            "Query",
            "getNote",
            "export function request(ctx) {\n  \
             return {operation: 'GetItem', \
             key: {id: util.dynamodb.toDynamoDB(ctx.args.id)}};\n}",
        ),
    ] {
        let (status, resolver) = appsync(
            router,
            "POST",
            &format!("/v1/apis/{api_id}/types/{type_name}/resolvers"),
            Some(json!({
                "fieldName": field,
                "kind": "UNIT",
                "dataSourceName": DATA_SOURCE,
                "code": definition(request),
                "runtime": {"name": RUNTIME.0, "runtimeVersion": RUNTIME.1},
            })),
        )
        .await;
        assert_eq!(
            status, 200,
            "CreateResolver for {type_name}.{field} must answer 200, got {resolver}"
        );
    }
    (graphql, realtime)
}
