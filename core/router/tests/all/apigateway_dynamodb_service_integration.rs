// SPDX-License-Identifier: CC0-1.0

//! An API Gateway stage whose method integrates an AWS SERVICE ACTION of AMAZON DYNAMODB:
//! the whole path a client takes from the stage invoke URL to the item in the table, with no
//! compute in between, across three services on one engine.
//!
//! RULE: a deployed method whose integration is of type `AWS` is served by MAKING the call
//! its `uri` names against the service that `uri` names — `Integration$uri` gives the form
//! `arn:aws:apigateway:{region}:{subdomain}:path|action/{service_api}` — under the role
//! `Integration$credentials` names, and the method response is the answer that service gave,
//! mapped through the integration response whose `selectionPattern` matches the status it
//! answered with ("For all other HTTP and AWS back ends, the HTTP status code is matched",
//! `IntegrationResponse$selectionPattern`). For a service whose api names its action out of
//! band and carries that action's whole input as a JSON document — the `awsJson1_0` protocol
//! Amazon DynamoDB's model declares — the call is the ACTION the `action` form names with the
//! rendered `Integration$requestTemplates` as its request, so what the action receives is
//! what the developer's template wrote and what the client reads is what the table then held.
//! The role decides whether the call is made at all: one this service cannot assume is an
//! integration that could not be performed (`API_CONFIGURATION_ERROR`, HTTP 500 and
//! `{"message":"Internal server error"}`), and a call the policies in force on the role do
//! not allow is refused BY THE SERVICE, in its own error envelope, which the integration
//! responses then map like any other backend answer.
//!
//! The second rule here is the one layered on top of it: a method whose settings enable
//! caching on a stage whose cluster is up is answered from that cluster
//! (`MethodSetting$cachingEnabled`), so the integration is NOT performed again — which is
//! observable only because the integration's own backend is a table whose counter says how
//! many times it ran — and a `Cache-Control: max-age=0` ask from a caller that may not
//! invalidate is answered as `MethodSetting$unauthorizedCacheControlHeaderStrategy` says.
//!
//! Only a cross-service suite can pin either: the item and the table are DynamoDB's, the role
//! and its policies are IAM's, and the api, the stage, its cache cluster and the integration
//! are API Gateway's, so the rules are observable only where all three are mounted on one
//! engine. Each crate's own tests reach exactly one of them.
//!
//! The rows vary what the rule generalises over: the ACTION the `uri` names (a write that
//! increments, a read, a write that stores what the request carried), WHERE the call's inputs
//! come from (fixed in the template, taken from a path parameter, taken from the request's
//! payload), the status the service answers (a success, a table nothing holds, a call the role
//! may not make), WHICH role the integration names (one that may make the call, one whose
//! grant names another table, one this service cannot assume), and — for the caching rule —
//! each modelled `unauthorizedCacheControlHeaderStrategy` plus a stage that requires no
//! authorization for the ask at all.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request here is signed for, and the Region the integration ARNs name.
const REGION: &str = "us-east-1";

/// The long-term key every emulator publishes, which is what the setup calls are signed
/// with. The signature itself is never checked: this router runs with request
/// authentication off, as every suite in this crate runs it.
const AMBIENT_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// The table every row below reads or writes.
const TABLE: &str = "stage-integration-counters";

/// A table nothing creates, for the row whose call the service itself refuses.
const ABSENT_TABLE: &str = "stage-integration-absent";

/// The item the counter rows increment.
const COUNTER_KEY: &str = "count";

/// The stage the api is deployed to.
const STAGE: &str = "live";

/// A trust policy that lets API Gateway assume the role it is attached to. It carries none
/// of the characters that are significant in an `application/x-www-form-urlencoded` body.
const APIGATEWAY_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"apigateway.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The same policy for ANOTHER service's principal: a role this api cannot assume, which is
/// the only thing that differs between it and the row above.
const LAMBDA_TRUST: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","#,
    r#""Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The actions an integration role here is granted.
const TABLE_ACTIONS: &str = r#""dynamodb:UpdateItem","dynamodb:GetItem","dynamodb:PutItem""#;

/// How long to wait between polls of a clock-owed transition, and the generous budget a
/// transition is given before the suite calls it a failure. Nothing here asserts on how long
/// a transition took: the condition is polled for until it holds.
const POLL: Duration = Duration::from_millis(50);

/// See [`POLL`].
const BUDGET: Duration = Duration::from_secs(120);

/// The header a stage states an ignored cache-control header in, and the ask that earns it.
const WARNING_HEADER: &str = "warning";

/// See [`WARNING_HEADER`].
const INVALIDATION_ASK: (&str, &str) = ("cache-control", "max-age=0");

/// The `cacheClusterStatus` a stage whose cluster is up reports.
const CACHE_CLUSTER_AVAILABLE: &str = "AVAILABLE";

/// What one call answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: Bytes,
}

impl Answer {
    /// One response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The body as text.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    /// The body as JSON, `Null` when it is neither JSON nor present.
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }
}

/// The three services this rule spans, over ONE engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={AMBIENT_KEY}/20260101/{REGION}/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Dispatch one request as it arrives on the wire.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    host: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{path}"))
        .header("host", host);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let response = router
        .dispatch(builder.body(body).expect("a well-formed request"))
        .await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// One API Gateway control-plane call.
async fn apigw(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    dispatch(
        router,
        method,
        &format!("apigateway.{REGION}.amazonaws.com"),
        path,
        &[
            ("content-type", "application/json"),
            ("authorization", &authorization("apigateway")),
        ],
        Bytes::from(payload),
    )
    .await
}

/// One API Gateway control-plane call that has to succeed with `expected`.
async fn apigw_ok(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: u16,
) -> Value {
    let answer = apigw(router, method, path, body).await;
    assert_eq!(
        answer.status,
        expected,
        "setup {case:?}: expected HTTP {expected} from {method} {path}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One Amazon DynamoDB call over its own `awsJson1_0` wire.
async fn ddb(router: &MultiServiceRouter, operation: &str, body: Value) -> Answer {
    dispatch(
        router,
        "POST",
        &format!("dynamodb.{REGION}.amazonaws.com"),
        "/",
        &[
            ("content-type", "application/x-amz-json-1.0"),
            ("authorization", &authorization("dynamodb")),
            ("x-amz-target", &format!("DynamoDB_20120810.{operation}")),
        ],
        Bytes::from(body.to_string()),
    )
    .await
}

/// One Amazon DynamoDB call that has to answer 200.
async fn ddb_ok(router: &MultiServiceRouter, case: &str, operation: &str, body: Value) -> Value {
    let answer = ddb(router, operation, body).await;
    assert_eq!(
        answer.status,
        200,
        "setup {case:?}: expected HTTP 200 from DynamoDB {operation}, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.json()
}

/// One IAM awsQuery call that has to succeed, answering its body.
async fn iam_ok(router: &MultiServiceRouter, case: &str, body: String) -> String {
    let answer = dispatch(
        router,
        "POST",
        "iam.amazonaws.com",
        "/",
        &[
            ("content-type", "application/x-www-form-urlencoded"),
            ("authorization", &authorization("iam")),
        ],
        Bytes::from(body),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "setup {case:?}: expected HTTP 200 from IAM, got {} with {:?}",
        answer.status,
        answer.text()
    );
    answer.text()
}

/// One request to the api's own invoke endpoint — the host
/// `{rest-api-id}.execute-api.{region}.amazonaws.com`, which is the endpoint
/// `RestApi$disableExecuteApiEndpoint` documents — carrying no credential at all, as a
/// client of a public method sends it.
async fn invoke(
    router: &MultiServiceRouter,
    api: &str,
    verb: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> Answer {
    dispatch(
        router,
        verb,
        &format!("{api}.execute-api.{REGION}.amazonaws.com"),
        &format!("/{STAGE}{path}"),
        headers,
        body,
    )
    .await
}

/// The member `name` of a JSON object, as text.
fn member(case: &str, body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("setup {case:?}: no {name:?} in {body}"))
        .to_string()
}

/// The text of the first `<name>…</name>` element of an XML body.
fn xml_element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// One identity-based policy granting `actions` on `resource`, written with none of the
/// characters a form body treats specially.
fn policy(actions: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":[{actions}],"Resource":"{resource}"}}]}}"#
    )
}

/// One IAM role: created with `trust` as its trust policy and `document` as its one inline
/// policy, answering its ARN.
async fn role(router: &MultiServiceRouter, name: &str, trust: &str, document: &str) -> String {
    let created = iam_ok(
        router,
        &format!("CreateRole({name})"),
        format!(
            "Action=CreateRole&Version=2010-05-08&RoleName={name}\
             &AssumeRolePolicyDocument={trust}"
        ),
    )
    .await;
    let arn = xml_element(&created, "Arn")
        .unwrap_or_else(|| panic!("setup CreateRole({name}): no <Arn> in {created}"));
    iam_ok(
        router,
        &format!("PutRolePolicy({name})"),
        format!(
            "Action=PutRolePolicy&Version=2010-05-08&RoleName={name}\
             &PolicyName=stage-integration&PolicyDocument={document}"
        ),
    )
    .await;
    arn
}

/// The table every row reads or writes, created through DynamoDB's own API and waited for
/// until that service first reports it `ACTIVE` — a table that is not is one no call lands
/// on.
async fn table(router: &MultiServiceRouter) {
    ddb_ok(
        router,
        "CreateTable",
        "CreateTable",
        json!({
            "TableName": TABLE,
            "KeySchema": [{ "AttributeName": "pk", "KeyType": "HASH" }],
            "AttributeDefinitions": [{ "AttributeName": "pk", "AttributeType": "S" }],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    let deadline = Instant::now() + BUDGET;
    loop {
        let described = ddb_ok(
            router,
            "DescribeTable",
            "DescribeTable",
            json!({ "TableName": TABLE }),
        )
        .await;
        if described["Table"]["TableStatus"].as_str() == Some("ACTIVE") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the table never reported ACTIVE, last {described}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// What the table holds for `key`, as the attribute `n` of that item — read through
/// DynamoDB's OWN api, so it says what that service holds rather than what the stage
/// answered. `None` for a key holding no such item or no such attribute.
async fn held(router: &MultiServiceRouter, key: &str) -> Option<String> {
    let item = ddb_ok(
        router,
        "GetItem",
        "GetItem",
        json!({ "TableName": TABLE, "Key": { "pk": { "S": key } } }),
    )
    .await;
    item["Item"]["n"]["N"]
        .as_str()
        .or_else(|| item["Item"]["n"]["S"].as_str())
        .map(str::to_string)
}

/// One resource of the api under `parent`, answering its id.
async fn resource(router: &MultiServiceRouter, api: &str, parent: &str, part: &str) -> String {
    let created = apigw_ok(
        router,
        &format!("CreateResource({part})"),
        "POST",
        &format!("/restapis/{api}/resources/{parent}"),
        Some(json!({ "pathPart": part })),
        201,
    )
    .await;
    member(&format!("CreateResource({part})"), &created, "id")
}

/// The statuses every method here declares an integration response for: the default (no
/// pattern) for a success, and one per class of backend refusal, so what a client sees is the
/// status the service answered rather than one status for every outcome.
const MAPPED_STATUSES: &[(&str, Option<&str>)] = &[
    ("200", None),
    ("400", Some(r"4\d{2}")),
    ("500", Some(r"5\d{2}")),
];

/// One method whose integration is the Amazon DynamoDB action `action` names, called with
/// `request_template` as its request document under the role `credentials`, and whose 200
/// integration response renders `response_template`.
#[allow(clippy::too_many_arguments)]
async fn ddb_method(
    router: &MultiServiceRouter,
    api: &str,
    resource_id: &str,
    verb: &str,
    action: &str,
    credentials: &str,
    request_template: &str,
    response_template: Option<&str>,
) {
    let case = format!("{verb} {action}");
    apigw_ok(
        router,
        &case,
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}"),
        Some(json!({ "authorizationType": "NONE" })),
        201,
    )
    .await;
    apigw_ok(
        router,
        &case,
        "PUT",
        &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}/integration"),
        Some(json!({
            "type": "AWS",
            // `integrationHttpMethod` travels under its `jsonName`, which is what
            // `smithy_query(query="operation",operation="PutIntegration")` reports for it.
            "httpMethod": "POST",
            "uri": format!("arn:aws:apigateway:{REGION}:dynamodb:action/{action}"),
            "credentials": credentials,
            "passthroughBehavior": "NEVER",
            "requestTemplates": { "application/json": request_template },
        })),
        201,
    )
    .await;
    for (status, pattern) in MAPPED_STATUSES {
        apigw_ok(
            router,
            &case,
            "PUT",
            &format!("/restapis/{api}/resources/{resource_id}/methods/{verb}/responses/{status}"),
            Some(json!({})),
            201,
        )
        .await;
        let mut response = json!({ "statusCode": status });
        if let Some(pattern) = pattern {
            response["selectionPattern"] = json!(pattern);
        }
        if let (Some(template), None) = (response_template, pattern) {
            response["responseTemplates"] = json!({ "application/json": template });
        }
        apigw_ok(
            router,
            &case,
            "PUT",
            &format!(
                "/restapis/{api}/resources/{resource_id}/methods/{verb}/integration/responses/{status}"
            ),
            Some(response),
            201,
        )
        .await;
    }
}

/// The request template of a counter increment on `table`: the `UpdateItem` document that
/// adds one to the item's `n` and asks for the attributes the update touched.
fn increment_template(table: &str) -> String {
    format!(
        r#"{{"TableName":"{table}","Key":{{"pk":{{"S":"{COUNTER_KEY}"}}}},"UpdateExpression":"ADD n :one","ExpressionAttributeValues":{{":one":{{"N":"1"}}}},"ReturnValues":"UPDATED_NEW"}}"#
    )
}

/// The response template every counter method maps its answer through: the counter the action
/// answered with, and the id API Gateway minted for this request.
const COUNTER_RESPONSE: &str =
    r#"{"n":$input.path('$.Attributes.n.N'),"rid":"$context.requestId"}"#;

/// The api, deployed to [`STAGE`], answering `GET /count` through the increment above under
/// `credentials`; `cluster` asks for the stage's response cache cluster.
async fn deployed_counter(
    router: &MultiServiceRouter,
    name: &str,
    credentials: &str,
    cluster: bool,
) -> String {
    let created = apigw_ok(
        router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": name })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");
    let count = resource(router, &api, &root, "count").await;
    ddb_method(
        router,
        &api,
        &count,
        "GET",
        "UpdateItem",
        credentials,
        &increment_template(TABLE),
        Some(COUNTER_RESPONSE),
    )
    .await;
    let mut deployment = json!({ "stageName": STAGE });
    if cluster {
        deployment["cacheClusterEnabled"] = json!(true);
        deployment["cacheClusterSize"] = json!("0.5");
    }
    apigw_ok(
        router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(deployment),
        201,
    )
    .await;
    api
}

#[tokio::test]
async fn a_deployed_method_integrating_a_dynamodb_action_performs_it() {
    let router = emulator();
    table(&router).await;

    // Three roles, each written by IAM's own `CreateRole`/`PutRolePolicy`: one this api can
    // assume that may act on this account's tables, one it can assume whose grant names
    // ANOTHER table, and one it cannot assume at all.
    let account = engine::emulator_account_id();
    let allowed = role(
        &router,
        "ddb-integration-allowed",
        APIGATEWAY_TRUST,
        &policy(
            TABLE_ACTIONS,
            &format!("arn:aws:dynamodb:{REGION}:{account}:table/*"),
        ),
    )
    .await;
    let other_table = role(
        &router,
        "ddb-integration-other-table",
        APIGATEWAY_TRUST,
        &policy(
            TABLE_ACTIONS,
            &format!("arn:aws:dynamodb:{REGION}:{account}:table/another-table"),
        ),
    )
    .await;
    let untrusted = role(
        &router,
        "ddb-integration-untrusted",
        LAMBDA_TRUST,
        &policy(
            TABLE_ACTIONS,
            &format!("arn:aws:dynamodb:{REGION}:{account}:table/*"),
        ),
    )
    .await;

    let created = apigw_ok(
        &router,
        "CreateRestApi",
        "POST",
        "/restapis",
        Some(json!({ "name": "ddb-integration" })),
        201,
    )
    .await;
    let api = member("CreateRestApi", &created, "id");
    let root = member("CreateRestApi", &created, "rootResourceId");

    // One resource per row: the increment, a read of what it wrote, a write that stores what
    // the REQUEST carried, and the three rows whose call does not land.
    let count = resource(&router, &api, &root, "count").await;
    ddb_method(
        &router,
        &api,
        &count,
        "GET",
        "UpdateItem",
        &allowed,
        &increment_template(TABLE),
        Some(COUNTER_RESPONSE),
    )
    .await;
    let read = resource(&router, &api, &root, "read").await;
    ddb_method(
        &router,
        &api,
        &read,
        "GET",
        "GetItem",
        &allowed,
        &format!(
            r#"{{"TableName":"{TABLE}","Key":{{"pk":{{"S":"{COUNTER_KEY}"}}}}}}"#
        ),
        Some(r#"{"n":$input.path('$.Item.n.N')}"#),
    )
    .await;
    let notes = resource(&router, &api, &root, "notes").await;
    let note = resource(&router, &api, &notes, "{id}").await;
    ddb_method(
        &router,
        &api,
        &note,
        "PUT",
        "PutItem",
        &allowed,
        // The inputs of this call come from the REQUEST: the key from the path parameter the
        // resource declares, the stored value from the payload the client sent.
        &format!(
            r#"{{"TableName":"{TABLE}","Item":{{"pk":{{"S":"$input.params('id')"}},"n":{{"S":"$input.path('$.note')"}}}}}}"#
        ),
        Some(r#"{"stored":"$input.params('id')"}"#),
    )
    .await;
    let denied = resource(&router, &api, &root, "denied").await;
    ddb_method(
        &router,
        &api,
        &denied,
        "GET",
        "UpdateItem",
        &other_table,
        &increment_template(TABLE),
        Some(COUNTER_RESPONSE),
    )
    .await;
    let unassumable = resource(&router, &api, &root, "unassumable").await;
    ddb_method(
        &router,
        &api,
        &unassumable,
        "GET",
        "UpdateItem",
        &untrusted,
        &increment_template(TABLE),
        Some(COUNTER_RESPONSE),
    )
    .await;
    let absent = resource(&router, &api, &root, "absent").await;
    ddb_method(
        &router,
        &api,
        &absent,
        "GET",
        "UpdateItem",
        &allowed,
        &increment_template(ABSENT_TABLE),
        Some(COUNTER_RESPONSE),
    )
    .await;
    apigw_ok(
        &router,
        "CreateDeployment",
        "POST",
        &format!("/restapis/{api}/deployments"),
        Some(json!({ "stageName": STAGE })),
        201,
    )
    .await;

    // The increment: the stage answers what the ACTION answered, mapped, and the table holds
    // what the action wrote — so a second request reads the counter the first left behind.
    let mut seen_ids = Vec::new();
    for expected in ["1", "2"] {
        let answered = invoke(&router, &api, "GET", "/count", &[], Bytes::new()).await;
        assert_eq!(
            answered.status,
            200,
            "case increment to {expected}: expected 200 from the stage, got {} with {:?}",
            answered.status,
            answered.text()
        );
        let body = answered.json();
        assert_eq!(
            body["n"].as_i64().map(|n| n.to_string()).as_deref(),
            Some(expected),
            "case increment to {expected}: expected the counter the action answered, got {:?}",
            answered.text()
        );
        let id = body["rid"].as_str().unwrap_or_default().to_string();
        assert!(
            !id.is_empty() && !seen_ids.contains(&id),
            "case increment to {expected}: expected this request's own id, got {id:?} after \
             {seen_ids:?}"
        );
        seen_ids.push(id);
        assert_eq!(
            held(&router, COUNTER_KEY).await.as_deref(),
            Some(expected),
            "case increment to {expected}: the table must hold what the action wrote"
        );
    }

    // The read: the same records, through the action that reads rather than writes.
    let answered = invoke(&router, &api, "GET", "/read", &[], Bytes::new()).await;
    assert_eq!(
        answered.status,
        200,
        "case read: expected 200 from the stage, got {} with {:?}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.json()["n"].as_i64(),
        Some(2),
        "case read: expected the counter the table holds, got {:?}",
        answered.text()
    );

    // The write whose inputs the REQUEST carries, read back through DynamoDB's own api.
    let answered = invoke(
        &router,
        &api,
        "PUT",
        "/notes/first",
        &[("content-type", "application/json")],
        Bytes::from(r#"{"note":"from the payload"}"#),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case write from the request: expected 200 from the stage, got {} with {:?}",
        answered.status,
        answered.text()
    );
    assert_eq!(
        answered.json()["stored"].as_str(),
        Some("first"),
        "case write from the request: expected the key the path named, got {:?}",
        answered.text()
    );
    assert_eq!(
        held(&router, "first").await.as_deref(),
        Some("from the payload"),
        "case write from the request: the table must hold what the template wrote"
    );

    // The three rows whose call does not land, each answered as the rule says and each
    // leaving the counter exactly as it was.
    let refused = invoke(&router, &api, "GET", "/denied", &[], Bytes::new()).await;
    assert_eq!(
        refused.status,
        400,
        "case a role that may not make the call: expected the status the service refused \
         with, got {} with {:?}",
        refused.status,
        refused.text()
    );
    let message = refused.json()["message"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        refused.text().contains("AccessDeniedException")
            && message.contains("is not authorized to perform: dynamodb:UpdateItem"),
        "case a role that may not make the call: expected the service's own refusal, got {:?}",
        refused.text()
    );

    let unperformed = invoke(&router, &api, "GET", "/unassumable", &[], Bytes::new()).await;
    assert_eq!(
        unperformed.status,
        500,
        "case a role this service cannot assume: expected the api's own \
         integration-failure response, got {} with {:?}",
        unperformed.status,
        unperformed.text()
    );
    assert_eq!(
        unperformed.json()["message"].as_str(),
        Some("Internal server error"),
        "case a role this service cannot assume: expected the gateway response's own \
         sentence, got {:?}",
        unperformed.text()
    );

    let missing = invoke(&router, &api, "GET", "/absent", &[], Bytes::new()).await;
    assert_eq!(
        missing.status,
        400,
        "case a table nothing holds: expected the status the service refused with, got {} \
         with {:?}",
        missing.status,
        missing.text()
    );
    assert!(
        missing.text().contains("ResourceNotFoundException"),
        "case a table nothing holds: expected the service's own refusal, got {:?}",
        missing.text()
    );

    assert_eq!(
        held(&router, COUNTER_KEY).await.as_deref(),
        Some("2"),
        "a call that did not land must leave the counter as it was"
    );
}

/// What one unauthorized cache-control ask is answered with.
#[derive(Debug, Clone, Copy)]
struct Asked {
    status: u16,
    /// The counter the body carries, or `None` when the body is the refusal rather than a
    /// mapped response.
    counter: Option<i64>,
    /// Whether the response states that the header was ignored.
    warned: bool,
}

/// `UpdateStage` with `operations`, which has to be accepted.
async fn update_stage(router: &MultiServiceRouter, case: &str, api: &str, operations: Vec<Value>) {
    apigw_ok(
        router,
        case,
        "PATCH",
        &format!("/restapis/{api}/stages/{STAGE}"),
        Some(json!({ "patchOperations": operations })),
        200,
    )
    .await;
}

/// One `UpdateStage` operation.
fn setting(path: &str, value: &str) -> Value {
    json!({ "op": "replace", "path": path, "value": value })
}

/// `GetStage`, polled until the stage reports its cache cluster up — a cluster that is still
/// coming up caches nothing, so nothing is asserted about caching before it is.
async fn stage_with_cluster_up(router: &MultiServiceRouter, api: &str) -> Value {
    let deadline = Instant::now() + BUDGET;
    loop {
        let stage = apigw_ok(
            router,
            "GetStage",
            "GET",
            &format!("/restapis/{api}/stages/{STAGE}"),
            None,
            200,
        )
        .await;
        if stage["cacheClusterStatus"].as_str() == Some(CACHE_CLUSTER_AVAILABLE) {
            return stage;
        }
        assert!(
            Instant::now() < deadline,
            "setup: the stage's cache cluster never reported {CACHE_CLUSTER_AVAILABLE}, last \
             {stage}"
        );
        tokio::time::sleep(POLL).await;
    }
}

#[tokio::test]
async fn the_cached_response_of_a_dynamodb_integration_obeys_the_methods_cache_settings() {
    let router = emulator();
    table(&router).await;
    let account = engine::emulator_account_id();
    let allowed = role(
        &router,
        "ddb-cache-allowed",
        APIGATEWAY_TRUST,
        &policy(
            TABLE_ACTIONS,
            &format!("arn:aws:dynamodb:{REGION}:{account}:table/*"),
        ),
    )
    .await;
    let api = deployed_counter(&router, "ddb-integration-cache", &allowed, true).await;
    stage_with_cluster_up(&router, &api).await;
    update_stage(
        &router,
        "caching on",
        &api,
        vec![
            setting("/*/*/caching/enabled", "true"),
            setting("/*/*/caching/ttlInSeconds", "300"),
        ],
    )
    .await;

    // The server-side defaults a caching-enabled method setting reads back.
    let stage = apigw_ok(
        &router,
        "GetStage",
        "GET",
        &format!("/restapis/{api}/stages/{STAGE}"),
        None,
        200,
    )
    .await;
    let settings = &stage["methodSettings"]["*/*"];
    assert_eq!(
        settings["cachingEnabled"].as_bool(),
        Some(true),
        "expected the method setting to report caching enabled, got {stage}"
    );
    assert_eq!(
        settings["requireAuthorizationForCacheControl"].as_bool(),
        Some(true),
        "expected the server-side default for requireAuthorizationForCacheControl, got {stage}"
    );
    assert_eq!(
        settings["unauthorizedCacheControlHeaderStrategy"].as_str(),
        Some("SUCCEED_WITH_RESPONSE_HEADER"),
        "expected the server-side default strategy, got {stage}"
    );

    // The first request is served by the integration, which writes; the second is served
    // from the cluster, which is observable because the integration did NOT write again.
    let filled = invoke(&router, &api, "GET", "/count", &[], Bytes::new()).await;
    assert_eq!(
        filled.status,
        200,
        "the first request must be answered by the integration, got {} with {:?}",
        filled.status,
        filled.text()
    );
    let held_body = filled.text();
    assert_eq!(
        filled.json()["n"].as_i64(),
        Some(1),
        "the first request must carry the counter the action wrote, got {held_body:?}"
    );
    let hit = invoke(&router, &api, "GET", "/count", &[], Bytes::new()).await;
    assert_eq!(
        hit.text(),
        held_body,
        "the second request must be served the held response, got {:?}",
        hit.text()
    );
    assert_eq!(
        held(&router, COUNTER_KEY).await.as_deref(),
        Some("1"),
        "a request served from the cluster must not perform the integration again"
    );

    // Each strategy for an ask this caller may not make, and then the stage that requires no
    // authorization for it at all — the one row where the ask IS honoured.
    let rows: &[(&str, &str, Asked)] = &[
        (
            "the strategy that fails the request",
            "FAIL_WITH_403",
            Asked {
                status: 403,
                counter: None,
                warned: false,
            },
        ),
        (
            "the strategy that states the header was ignored",
            "SUCCEED_WITH_RESPONSE_HEADER",
            Asked {
                status: 200,
                counter: Some(1),
                warned: true,
            },
        ),
        (
            "the strategy that ignores it silently",
            "SUCCEED_WITHOUT_RESPONSE_HEADER",
            Asked {
                status: 200,
                counter: Some(1),
                warned: false,
            },
        ),
    ];
    for (case, strategy, expected) in rows {
        update_stage(
            &router,
            case,
            &api,
            vec![setting(
                "/*/*/caching/unauthorizedCacheControlHeaderStrategy",
                strategy,
            )],
        )
        .await;
        let answered = invoke(&router, &api, "GET", "/count", &[INVALIDATION_ASK], Bytes::new()).await;
        assert_eq!(
            answered.status, expected.status,
            "case {case:?}: expected HTTP {} for the ask, got {} with {:?}",
            expected.status,
            answered.status,
            answered.text()
        );
        if let Some(counter) = expected.counter {
            assert_eq!(
                answered.json()["n"].as_i64(),
                Some(counter),
                "case {case:?}: expected the held response, got {:?}",
                answered.text()
            );
        }
        assert_eq!(
            answered.header(WARNING_HEADER),
            expected
                .warned
                .then(|| {
                    "199 Cache-control headers were ignored because the caller was \
                     unauthorized."
                        .to_string()
                }),
            "case {case:?}: expected warned={}, got {:?}",
            expected.warned,
            answered.header(WARNING_HEADER)
        );
        assert_eq!(
            held(&router, COUNTER_KEY).await.as_deref(),
            Some("1"),
            "case {case:?}: an ask the stage does not honour must not perform the integration"
        );
    }

    update_stage(
        &router,
        "the ask the stage honours",
        &api,
        vec![setting(
            "/*/*/caching/requireAuthorizationForCacheControl",
            "false",
        )],
    )
    .await;
    let reloaded = invoke(&router, &api, "GET", "/count", &[INVALIDATION_ASK], Bytes::new()).await;
    assert_eq!(
        reloaded.status,
        200,
        "case the ask the stage honours: expected 200, got {} with {:?}",
        reloaded.status,
        reloaded.text()
    );
    assert_eq!(
        reloaded.json()["n"].as_i64(),
        Some(2),
        "case the ask the stage honours: expected the refreshed counter, got {:?}",
        reloaded.text()
    );
    assert_eq!(
        held(&router, COUNTER_KEY).await.as_deref(),
        Some("2"),
        "case the ask the stage honours: the integration must run exactly once for the ask"
    );
    let refreshed = invoke(&router, &api, "GET", "/count", &[], Bytes::new()).await;
    assert_eq!(
        refreshed.text(),
        reloaded.text(),
        "case the ask the stage honours: the next request must hit the refreshed entry, got \
         {:?}",
        refreshed.text()
    );
    assert_eq!(
        held(&router, COUNTER_KEY).await.as_deref(),
        Some("2"),
        "case the ask the stage honours: the refreshed entry must then be served from the \
         cluster"
    );
}
