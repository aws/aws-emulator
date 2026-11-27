// SPDX-License-Identifier: CC0-1.0

//! Where an operation decides the resource its URI PATH names, relative to the rules this
//! service applies to the request BODY on its own.
//!
//! RULE under test: every operation resolves the resource its path addresses before it
//! applies a rule of its own about the body, so an identifier in the path that names no
//! stored resource is the modelled `ResourceNotFoundException` (HTTP 404), in the family's
//! own sentence, whatever the body carries. The only members refused ahead of that lookup
//! are the ones the MODEL constrains — `@required`, a length, a pattern, a range — which
//! the protocol boundary enforces; everything else is this service's own judgement of the
//! body and is reached only once the path's resource is known to be there.
//!
//! `smithy_query(query="operation",operation="UpdateGateway,CreateGatewayTarget,\
//! CreateGatewayRateLimit,BatchPutGatewayRateLimits,CreatePaymentConnector,UpdateMemory,\
//! UpdateAgentRuntime")`: each of them carries its parent resource's identifier as
//! `@httpLabel @required`, and none of them makes the body member this service judges
//! below `@required` — `CreateGatewayTarget$name`, `CreateGatewayRateLimit$entries`
//! against its own `dimensionKeys`, `CreatePaymentConnector`'s empty
//! `credentialProviderConfigurations` (`len 0..1`) and `UpdateMemory`'s requested
//! strategies are all rules stated in prose, not constraints the boundary walk can refuse.
//!
//! The rows differ along what the rule generalises over: the RESOURCE FAMILY the path
//! names (a gateway, a payment manager, a memory, an agent runtime), the OPERATION and its
//! METHOD (POST, PUT), and the shape of the body rule that would otherwise refuse the
//! request (a missing member, a cross-member mismatch, an empty list, a per-entry
//! validation). Each family answers in its own not-found sentence, so a row cannot pass on
//! one sentence shared by all. The control test at the end drives the very same bodies at
//! parents that ARE there, so the 404 rows cannot pass by refusing everything.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/ProbeRole";

/// Identifiers inside each label's own modelled pattern that nothing here ever creates:
/// `GatewayIdentifier` is `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`, `PaymentManagerId` the
/// same with `len 12..211`, `MemoryId` is `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`
/// and `AgentRuntimeId` is `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$`.
const ABSENT_GATEWAY: &str = "no-such-gateway-9876543210";
const ABSENT_PAYMENT_MANAGER: &str = "nosuchmanager-0123456789";
const ABSENT_MEMORY: &str = "noSuchMemory-abcdefghij";
const ABSENT_RUNTIME: &str = "noSuchRuntime-abcdefghij";

/// The sentence the gateway family answers a gateway it does not hold with.
const GATEWAY_ABSENT: &str = "Failed to retrieve gateway because it doesn't exist. Retry the \
                              request with a different resource identifier.";

/// The terminal status the clock owes a freshly created gateway
/// (`smithy_query(query="shape",shape="GatewayStatus")`).
const READY: &str = "READY";

/// How long a poll loop keeps looking for a clock-owed transition before giving up.
/// Generous on purpose: the loop waits for the CONDITION, and the span this service gives
/// the gateway family is far shorter than this on any host.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop pauses between looks.
const POLL_PAUSE: Duration = Duration::from_millis(20);

fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::default();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` header for this service, which is what the router routes on.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/bedrock-agentcore/\
         aws4_request, SignedHeaders=host, Signature=abc"
    )
}

/// One answer: the HTTP status and the parsed body.
struct Answer {
    status: u16,
    body: Value,
}

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| {
        Bytes::from(serde_json::to_vec(value).expect("the request body serializes"))
    });
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", authorization())
        .header("host", "bedrock-agentcore-control.us-east-1.amazonaws.com");
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("a well-formed request");
    let response = router.dispatch(request).await;
    Answer {
        status: response.status().as_u16(),
        body: if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        },
    }
}

/// The error code `restJson1` puts in the body's `__type`.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

fn text(body: &Value, member: &str) -> String {
    body.get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The sentence the memory family answers a memory it does not hold with: the asking
/// operation and the id the request carried.
fn memory_absent(operation: &str, memory_id: &str) -> String {
    format!(
        "Resource not found during {operation}: Failed to retrieve memory item for ID: \
         {memory_id}"
    )
}

/// The sentence the payments plane answers a manager it does not hold with on the route
/// that addresses that manager's connector COLLECTION: the identifier under the
/// `managerId=` label this surface names its parent manager by.
fn payment_manager_absent(payment_manager_id: &str) -> String {
    format!("Payment manager not found: managerId={payment_manager_id}")
}

/// The sentence this control plane answers an UPDATE of a runtime it does not hold with.
fn runtime_absent(agent_runtime_id: &str) -> String {
    format!("Agent '{agent_runtime_id}' was not found. Please check the agent ID and try again.")
}

/// An `UpdateGateway` body whose `authorizerType` is `CUSTOM_JWT` with no
/// `authorizerConfiguration` beside it: the model states that pairing in prose, so it is
/// this service's own rule about the body.
fn gateway_update_without_authorizer_configuration(name: &str) -> Value {
    json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "CUSTOM_JWT"})
}

/// A `CreateGatewayTarget` body carrying no `name`, which the model leaves optional on the
/// request while every response that reports a target declares it `@required`.
fn target_create_without_name() -> Value {
    json!({
        "targetConfiguration": {"mcp": {"openApiSchema": {"inlinePayload": "{\"openapi\":\"3.1.0\"}"}}},
        "credentialProviderConfigurations": [{"credentialProviderType": "CALLER_IAM_CREDENTIALS"}],
    })
}

/// A `CreateGatewayRateLimit` body whose one rule entry names a dimension the limit is not
/// scoped by. Both values are from `DimensionKey`'s own published catalogue —
/// `targetName | toolName | qualifiedModelId | $.context.iam.principal | …`
/// (`smithy_query(query="shape",shape="DimensionKeys")`) — so the refusal is the
/// cross-member rule, not an unmodelled key.
fn rate_limit_create_with_an_undeclared_dimension() -> Value {
    json!({
        "dimensionKeys": ["targetName"],
        "entries": [{
            "dimensions": {"toolName": "search"},
            "requests": [{"rate": 25.0, "period": "second"}],
        }],
    })
}

/// The same mismatch submitted through the BATCH write, which replaces a gateway's whole
/// set: a different operation, method and body shape for one rule.
fn rate_limit_batch_with_an_undeclared_dimension() -> Value {
    json!({"rateLimits": [{
        "rateLimitId": "by.target",
        "dimensionKeys": ["qualifiedModelId"],
        "entries": [{
            "dimensions": {"$.context.iam.principal": "arn:aws:iam::000000000000:user/probe"},
            "tokens": [{"rate": 1000.0, "period": "minute"}],
        }],
    }]})
}

/// A `CreatePaymentConnector` body in the mode that makes the caller supply the credential
/// provider configurations, carrying an EMPTY list: `CredentialsProviderConfigurations` is
/// `len 0..1`, so the boundary walk admits the empty list and only this service's own rule
/// refuses it.
fn connector_create_without_configurations() -> Value {
    json!({
        "name": "probeConnector",
        "type": "CoinbaseCDP",
        "provisionMode": "MANUAL",
        "credentialProviderConfigurations": [],
    })
}

/// An `UpdateMemory` body adding a strategy whose record expiry asks for the action the
/// model says is "not applicable on create paths", which is this service's own per-entry
/// judgement of the body.
fn memory_update_with_an_inapplicable_record_expiry() -> Value {
    json!({"memoryStrategies": {"addMemoryStrategies": [{"semanticMemoryStrategy": {
        "name": "facts",
        "recordExpirySpecification": {"action": "REMOVE"},
    }}]}})
}

/// An `UpdateAgentRuntime` body carrying what the model makes `@required`. The members this
/// operation judges beyond the boundary walk — the artifact's object-storage location, the
/// execution role, the network placement — are resolved against OTHER services, so a
/// router with only this service mounted refuses none of them; the row below pins the
/// not-found answer the path itself owes.
fn runtime_update_body() -> Value {
    json!({
        "agentRuntimeArtifact": {"containerConfiguration": {
            "containerUri": "public.ecr.aws/agentcore/demo:probe"
        }},
        "roleArn": ROLE_ARN,
    })
}

/// Create a gateway and read it again until the clock-owed `CREATING` -> `READY`
/// transition has happened, then answer its `gatewayId`.
async fn settled_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let created = send(
        router,
        "POST",
        "/gateways/",
        Some(&json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup {name:?}: CreateGateway must succeed, got {} ({})",
        created.status, created.body
    );
    let gateway_id = text(&created.body, "gatewayId");
    let started = Instant::now();
    loop {
        let read = send(router, "GET", &format!("/gateways/{gateway_id}/"), None).await;
        if text(&read.body, "status") == READY {
            return gateway_id;
        }
        assert!(
            started.elapsed() < POLL_DEADLINE,
            "setup {name:?}: the gateway never reached {READY}, last answer {} ({})",
            read.status,
            read.body
        );
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// One row: what it is a case of, the request it sends, and the sentence the family owes.
struct Row {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Value,
    message: String,
}

/// RULE: an identifier in the path that names no stored resource is the modelled 404, in
/// the family's own sentence, even when the body carries something this service would
/// otherwise refuse itself.
#[tokio::test]
async fn an_absent_path_resource_is_reported_absent_before_the_body_is_judged() {
    let router = router();

    let rows = [
        Row {
            case: "UpdateGateway: CUSTOM_JWT with no authorizerConfiguration",
            method: "PUT",
            uri: format!("/gateways/{ABSENT_GATEWAY}/"),
            body: gateway_update_without_authorizer_configuration("probegateway"),
            message: GATEWAY_ABSENT.to_string(),
        },
        Row {
            case: "CreateGatewayTarget: a body carrying no name",
            method: "POST",
            uri: format!("/gateways/{ABSENT_GATEWAY}/targets/"),
            body: target_create_without_name(),
            message: GATEWAY_ABSENT.to_string(),
        },
        Row {
            case: "CreateGatewayRateLimit: a rule entry naming an undeclared dimension",
            method: "POST",
            uri: format!("/gateways/{ABSENT_GATEWAY}/rate-limits"),
            body: rate_limit_create_with_an_undeclared_dimension(),
            message: GATEWAY_ABSENT.to_string(),
        },
        Row {
            case: "BatchPutGatewayRateLimits: the same mismatch in a batch entry",
            method: "PUT",
            uri: format!("/gateways/{ABSENT_GATEWAY}/rate-limits/batch"),
            body: rate_limit_batch_with_an_undeclared_dimension(),
            message: GATEWAY_ABSENT.to_string(),
        },
        Row {
            case: "CreatePaymentConnector: an empty credentialProviderConfigurations",
            method: "POST",
            uri: format!("/payments/managers/{ABSENT_PAYMENT_MANAGER}/connectors"),
            body: connector_create_without_configurations(),
            message: payment_manager_absent(ABSENT_PAYMENT_MANAGER),
        },
        Row {
            case: "UpdateMemory: a strategy whose record expiry asks for REMOVE",
            method: "PUT",
            uri: format!("/memories/{ABSENT_MEMORY}/update"),
            body: memory_update_with_an_inapplicable_record_expiry(),
            message: memory_absent("UpdateMemory", ABSENT_MEMORY),
        },
        Row {
            case: "UpdateAgentRuntime: a body whose members other services resolve",
            method: "PUT",
            uri: format!("/runtimes/{ABSENT_RUNTIME}/"),
            body: runtime_update_body(),
            message: runtime_absent(ABSENT_RUNTIME),
        },
    ];

    for row in &rows {
        let answered = send(&router, row.method, &row.uri, Some(&row.body)).await;
        assert_eq!(
            (answered.status, error_code(&answered.body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {:?}: expected HTTP 404 ResourceNotFoundException, got {} {} ({})",
            row.case,
            answered.status,
            error_code(&answered.body),
            answered.body
        );
        assert_eq!(
            text(&answered.body, "message"),
            row.message,
            "case {:?}: expected the family's absent-resource sentence {:?}, got {:?}",
            row.case,
            row.message,
            text(&answered.body, "message")
        );
    }
}

/// The other half of the rule, so the rows above cannot pass by refusing everything: at a
/// parent that IS there, each of those same bodies meets this service's own refusal of it.
#[tokio::test]
async fn the_bodys_own_rule_decides_once_the_path_resource_is_there() {
    let router = router();
    let gateway_id = settled_gateway(&router, "pathfirstprobe").await;

    // UpdateGateway: the gateway resolves, so the authorizer pairing rule stands.
    let refused = send(
        &router,
        "PUT",
        &format!("/gateways/{gateway_id}/"),
        Some(&gateway_update_without_authorizer_configuration("pathfirstprobe")),
    )
    .await;
    assert_eq!(
        (
            refused.status,
            error_code(&refused.body).as_str(),
            text(&refused.body, "message").as_str()
        ),
        (
            400,
            "ValidationException",
            "authorizerConfiguration: is required when authorizerType is CUSTOM_JWT"
        ),
        "case UpdateGateway on a gateway that is there: expected the modelled 400, got {} ({})",
        refused.status,
        refused.body
    );

    // CreateGatewayRateLimit: the cross-member rule about the submitted rule entries.
    let refused = send(
        &router,
        "POST",
        &format!("/gateways/{gateway_id}/rate-limits"),
        Some(&rate_limit_create_with_an_undeclared_dimension()),
    )
    .await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (400, "ValidationException"),
        "case CreateGatewayRateLimit on a gateway that is there: expected the modelled 400, \
         got {} ({})",
        refused.status,
        refused.body
    );
    assert!(
        text(&refused.body, "message").contains("toolName"),
        "case CreateGatewayRateLimit on a gateway that is there: expected the refusal to name \
         the dimension the entry carried, got {:?}",
        text(&refused.body, "message")
    );

    // BatchPutGatewayRateLimits: the same rule over a submitted batch.
    let refused = send(
        &router,
        "PUT",
        &format!("/gateways/{gateway_id}/rate-limits/batch"),
        Some(&rate_limit_batch_with_an_undeclared_dimension()),
    )
    .await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (400, "ValidationException"),
        "case BatchPutGatewayRateLimits on a gateway that is there: expected the modelled 400, \
         got {} ({})",
        refused.status,
        refused.body
    );

    // CreatePaymentConnector: the manager resolves, so the empty-list rule stands.
    let created = send(
        &router,
        "POST",
        "/payments/managers",
        Some(&json!({
            "name": "pathfirstmanager",
            "authorizerType": "AWS_IAM",
            "roleArn": ROLE_ARN,
        })),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: CreatePaymentManager must succeed, got {} ({})",
        created.status, created.body
    );
    let payment_manager_id = text(&created.body, "paymentManagerId");
    let refused = send(
        &router,
        "POST",
        &format!("/payments/managers/{payment_manager_id}/connectors"),
        Some(&connector_create_without_configurations()),
    )
    .await;
    assert_eq!(
        (
            refused.status,
            error_code(&refused.body).as_str(),
            text(&refused.body, "message").as_str()
        ),
        (
            400,
            "ValidationException",
            "Credential provider configurations is not defined"
        ),
        "case CreatePaymentConnector on a manager that is there: expected the modelled 400, \
         got {} ({})",
        refused.status,
        refused.body
    );

    // UpdateMemory: the memory resolves, so the strategy's own refusal stands.
    let created = send(
        &router,
        "POST",
        "/memories/create",
        Some(&json!({"name": "pathfirstmemory", "eventExpiryDuration": 30})),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "setup: CreateMemory must succeed, got {} ({})",
        created.status, created.body
    );
    let memory_id = created.body["memory"]["id"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !memory_id.is_empty(),
        "setup: the created memory must report its id, got ({})",
        created.body
    );
    let refused = send(
        &router,
        "PUT",
        &format!("/memories/{memory_id}/update"),
        Some(&memory_update_with_an_inapplicable_record_expiry()),
    )
    .await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (400, "ValidationException"),
        "case UpdateMemory on a memory that is there: expected the modelled 400, got {} ({})",
        refused.status,
        refused.body
    );
    assert!(
        text(&refused.body, "message").contains("not applicable on create paths"),
        "case UpdateMemory on a memory that is there: expected the record-expiry refusal, got \
         {:?}",
        text(&refused.body, "message")
    );

    // CreateGatewayTarget: the gateway resolves, so the body's own refusal stands, and the
    // same create WITH a name is served and reads back through GetGatewayTarget.
    let refused = send(
        &router,
        "POST",
        &format!("/gateways/{gateway_id}/targets/"),
        Some(&target_create_without_name()),
    )
    .await;
    assert_eq!(
        (refused.status, error_code(&refused.body).as_str()),
        (400, "ValidationException"),
        "case CreateGatewayTarget with no name on a gateway that is there: expected the \
         modelled 400, got {} ({})",
        refused.status,
        refused.body
    );
    let mut named = target_create_without_name();
    named["name"] = json!("pathfirsttarget");
    let created = send(
        &router,
        "POST",
        &format!("/gateways/{gateway_id}/targets/"),
        Some(&named),
    )
    .await;
    assert_eq!(
        created.status, 202,
        "case CreateGatewayTarget carrying the name: expected HTTP 202, got {} ({})",
        created.status, created.body
    );
    let target_id = text(&created.body, "targetId");
    let fetched = send(
        &router,
        "GET",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        None,
    )
    .await;
    assert_eq!(
        (fetched.status, text(&fetched.body, "name").as_str()),
        (200, "pathfirsttarget"),
        "case the created target reads back: got {} ({})",
        fetched.status,
        fetched.body
    );
}
