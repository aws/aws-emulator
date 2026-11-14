// SPDX-License-Identifier: CC0-1.0

//! The gateway-target writes' request validation: what they refuse about the BODY, and
//! where that decision sits relative to the resource the path names.
//!
//! Each test drives the service the way the router does: an `http::Request` through
//! [`MultiServiceRouter::dispatch`], so the wire protocol (`restJson1`), the boundary
//! constraint validation and the handler are all exercised, against the one shared engine.
//!
//! The rule under test: a request whose BODY cannot be rendered into the operation's
//! modelled response is an invalid request whatever the path names, so it is refused with
//! the modelled `ValidationException` (HTTP 400) BEFORE the resource the URI path addresses
//! is resolved, and only a body-valid request reaches the lookup that answers the modelled
//! `ResourceNotFoundException` (404).
//!
//! The two body members this service's gateway-target writes judge, read off the model
//! (`smithy_query(query="operation",operation="UpdateGatewayTarget")` /
//! `operation="CreateGatewayTarget"`): `credentialProviderConfigurations` is optional on
//! `UpdateGatewayTarget`'s request and `@required` with exactly one entry on its response,
//! and the update REPLACES the target's whole configuration — so a body that defines none
//! leaves the response nothing to report; `name` is optional on `CreateGatewayTarget`'s
//! request and `@required` on every response that reports a target. A body that DOES define
//! the member is served, and nothing a refused request carried is written.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService;

/// The region the rows sign for.
const REGION: &str = "us-east-1";

/// A role ARN inside `RoleArn`'s pattern `^arn:aws(-[^:]+)?:iam::([0-9]{12})?:role/.+$`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/GatewayRole";

/// How long a poll loop keeps looking for a clock-owed transition before giving up.
/// Generous on purpose: the loop waits for the CONDITION, and the span this service gives
/// the gateway family is far shorter than this, on any host.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop pauses between looks.
const POLL_PAUSE: Duration = Duration::from_millis(20);

/// The terminal status the clock owes a freshly created or updated gateway and target
/// (`smithy_query(query="shape",shape="GatewayStatus,TargetStatus")`).
const READY: &str = "READY";

/// A `gatewayIdentifier` inside `GatewayIdentifier`'s pattern
/// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$` that no row ever creates.
const ABSENT_GATEWAY_ID: &str = "no-such-gateway-0123456789";

/// A `targetId` inside `TargetId`'s pattern `^[0-9a-zA-Z]{10}$` that no row ever creates.
const ABSENT_TARGET_ID: &str = "ZZZZ999999";

/// The sentence this service gives a request that defined no credential provider
/// configurations: entirely fixed text, naming the member in prose and quoting nothing of
/// the request.
const NOT_DEFINED: &str = "Credential provider configurations is not defined";

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

async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
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
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
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

/// A target configuration union: an MCP target serving an inline OpenAPI schema.
fn openapi_configuration(payload: &str) -> Value {
    json!({"mcp": {"openApiSchema": {"inlinePayload": payload}}})
}

/// Read the gateway again until it reports `READY`, or the deadline passes: the transition
/// is the clock's, so the loop looks for the CONDITION rather than waiting a fixed time.
async fn gateway_ready(router: &MultiServiceRouter, gateway_id: &str) {
    let started = Instant::now();
    loop {
        let (_, read) = send(router, "GET", &format!("/gateways/{gateway_id}/"), None).await;
        if text(&read, "status") == READY || started.elapsed() > POLL_DEADLINE {
            return;
        }
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// The same loop for a target: read it again until it reports `READY`, or the deadline
/// passes.
async fn target_ready(router: &MultiServiceRouter, gateway_id: &str, target_id: &str) {
    let started = Instant::now();
    loop {
        let (_, read) = send(
            router,
            "GET",
            &format!("/gateways/{gateway_id}/targets/{target_id}/"),
            None,
        )
        .await;
        if text(&read, "status") == READY || started.elapsed() > POLL_DEADLINE {
            return;
        }
        tokio::time::sleep(POLL_PAUSE).await;
    }
}

/// Create a gateway, wait for the clock-owed `CREATING` -> `READY` transition, and answer
/// its `gatewayId`.
async fn settled_gateway(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = send(
        router,
        "POST",
        "/gateways/",
        Some(&json!({"name": name, "roleArn": ROLE_ARN, "authorizerType": "AWS_IAM"})),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup {name:?}: CreateGateway must succeed, got {status} ({created})"
    );
    let gateway_id = text(&created, "gatewayId");
    gateway_ready(router, &gateway_id).await;
    gateway_id
}

/// Create a target on `gateway` and answer its `targetId`.
async fn created_target(router: &MultiServiceRouter, gateway: &str, name: &str) -> String {
    let (status, created) = send(
        router,
        "POST",
        &format!("/gateways/{gateway}/targets/"),
        Some(&json!({
            "name": name,
            "description": "the target before any update",
            "targetConfiguration": openapi_configuration("{\"openapi\":\"3.0.0\"}"),
            "credentialProviderConfigurations": [
                {"credentialProviderType": "CALLER_IAM_CREDENTIALS"}
            ],
        })),
    )
    .await;
    assert_eq!(
        status, 202,
        "setup {name:?}: CreateGatewayTarget must succeed, got {status} ({created})"
    );
    text(&created, "targetId")
}

async fn update_target(
    router: &MultiServiceRouter,
    gateway: &str,
    target: &str,
    body: &Value,
) -> (u16, Value) {
    send(
        router,
        "PUT",
        &format!("/gateways/{gateway}/targets/{target}/"),
        Some(body),
    )
    .await
}

/// A body that defines no credential provider configurations is refused 400 whatever the
/// path names — a target that is there, a target that is not, a gateway that is not, a
/// gateway the clock still owes a transition — and nothing it asked for is written. Only a
/// body-valid request reaches the lookup that answers 404.
#[tokio::test]
async fn an_update_that_defines_no_credential_providers_is_refused() {
    let router = router();
    let gateway_id = settled_gateway(&router, "novalidategate").await;
    let target_id = created_target(&router, &gateway_id, "novalidatetarget").await;
    target_ready(&router, &gateway_id, &target_id).await;

    // A second gateway that is NOT in the status a change needs: the row addressing it
    // would otherwise be refused 400 for the gateway's status, with a different sentence.
    let unsettled_gateway = settled_gateway(&router, "unsettledgate").await;
    let unsettled_target = created_target(&router, &unsettled_gateway, "unsettledtarget").await;
    let (status, moved) = send(
        &router,
        "PUT",
        &format!("/gateways/{unsettled_gateway}/"),
        Some(&json!({
            "name": "unsettledgate",
            "roleArn": ROLE_ARN,
            "authorizerType": "AWS_IAM",
        })),
    )
    .await;
    assert_eq!(
        (status, text(&moved, "status").as_str()),
        (202, "UPDATING"),
        "setup: the gateway must be left in UPDATING, got {status} {} ({moved})",
        text(&moved, "status")
    );

    // (case, gatewayIdentifier, targetId, the body the row sends)
    let rows: [(&str, String, String, Value); 7] = [
        (
            "a gateway and a target that are both there",
            gateway_id.clone(),
            target_id.clone(),
            json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")}),
        ),
        (
            "every other configurable member carried, and only this one left out",
            gateway_id.clone(),
            target_id.clone(),
            json!({
                "name": "renamedtarget",
                "description": "a description the refusal must not store",
                "targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}"),
                "metadataConfiguration": {"allowedRequestHeaders": ["x-after"]},
                "certificateConfigurations": [{"s3": {"uri": "s3://certs/private-ca.pem"}}],
            }),
        ),
        (
            "a gateway the clock still owes a transition",
            unsettled_gateway.clone(),
            unsettled_target.clone(),
            json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")}),
        ),
        (
            "a Lambda configuration naming a function no mounted service resolves",
            gateway_id.clone(),
            target_id.clone(),
            json!({"targetConfiguration": {"mcp": {"lambda": {
                "lambdaArn": "arn:aws:lambda:us-east-1:000000000000:function:absent",
                "toolSchema": {"inlinePayload": [{
                    "name": "echo",
                    "description": "echo a string back",
                    "inputSchema": {"type": "object"},
                }]},
            }}}}),
        ),
        // The body verdict is reached BEFORE the path is resolved, so a path naming a
        // resource that is not there gets the body's refusal and not the 404.
        (
            "a targetId naming no target of a gateway that is there",
            gateway_id.clone(),
            ABSENT_TARGET_ID.to_string(),
            json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")}),
        ),
        (
            "a gatewayIdentifier naming no gateway",
            ABSENT_GATEWAY_ID.to_string(),
            target_id.clone(),
            json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")}),
        ),
        (
            "neither the gateway nor the target is there",
            ABSENT_GATEWAY_ID.to_string(),
            ABSENT_TARGET_ID.to_string(),
            json!({"targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}")}),
        ),
    ];

    for (case, gateway, target, body) in rows {
        let (status, refused) = update_target(&router, &gateway, &target, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} {} ({refused})",
            error_code(&refused)
        );
        assert_eq!(
            text(&refused, "message"),
            NOT_DEFINED,
            "case {case:?}: expected the fixed sentence {NOT_DEFINED:?}, got {}",
            text(&refused, "message")
        );
    }

    // Only a request whose BODY this operation can serve reaches the lookup, and that is
    // the request a path naming a resource that is not there answers the modelled 404 for.
    let absent_rows: [(&str, String, String, &str); 3] = [
        (
            "a targetId naming no target of a gateway that is there",
            gateway_id.clone(),
            ABSENT_TARGET_ID.to_string(),
            "Failed to retrieve target because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
        (
            "a gatewayIdentifier naming no gateway",
            ABSENT_GATEWAY_ID.to_string(),
            target_id.clone(),
            "Failed to retrieve gateway because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
        (
            "neither the gateway nor the target is there",
            ABSENT_GATEWAY_ID.to_string(),
            ABSENT_TARGET_ID.to_string(),
            "Failed to retrieve gateway because it doesn't exist. Retry the request with a \
             different resource identifier.",
        ),
    ];

    for (case, gateway, target, message) in absent_rows {
        let body = json!({
            "targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}"),
            "credentialProviderConfigurations": [
                {"credentialProviderType": "CALLER_IAM_CREDENTIALS"}
            ],
        });
        let (status, refused) = update_target(&router, &gateway, &target, &body).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: expected HTTP 404 ResourceNotFoundException, got {status} {} \
             ({refused})",
            error_code(&refused)
        );
        assert_eq!(
            text(&refused, "message"),
            message,
            "case {case:?}: the refusal names what it could not find, got {}",
            text(&refused, "message")
        );
    }

    // Nothing a refused row asked for reached the stored target: it still carries the
    // name, description and credential provider the create gave it.
    let (status, fetched) = send(
        &router,
        "GET",
        &format!("/gateways/{gateway_id}/targets/{target_id}/"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "the target must still read back, got {status} ({fetched})"
    );
    assert_eq!(
        text(&fetched, "name"),
        "novalidatetarget",
        "a refused update must not rename the target, got {fetched}"
    );
    assert_eq!(
        text(&fetched, "description"),
        "the target before any update",
        "a refused update must not change the description, got {fetched}"
    );
    assert_eq!(
        fetched["credentialProviderConfigurations"],
        json!([{"credentialProviderType": "CALLER_IAM_CREDENTIALS"}]),
        "a refused update must leave the stored credential providers alone, got {}",
        fetched["credentialProviderConfigurations"]
    );
}

/// The accepted half of the same rule: a body that DOES define the member is served, and
/// the target reports exactly what it defined.
#[tokio::test]
async fn an_update_that_defines_the_credential_providers_is_served() {
    let router = router();
    let gateway_id = settled_gateway(&router, "servedgate").await;

    // One row per `CredentialProviderType` the enum declares
    // (`smithy_query(query="shape",shape="CredentialProviderType")`), since the rule is
    // about the member being DEFINED and not about which provider it names.
    let rows: [(&str, Value); 4] = [
        (
            "the gateway's own IAM role",
            json!([{"credentialProviderType": "GATEWAY_IAM_ROLE"}]),
        ),
        (
            "the caller's IAM credentials",
            json!([{"credentialProviderType": "CALLER_IAM_CREDENTIALS"}]),
        ),
        (
            "a JWT passed through",
            json!([{"credentialProviderType": "JWT_PASSTHROUGH"}]),
        ),
        (
            "an API key provider of its own",
            json!([{
                "credentialProviderType": "API_KEY",
                "credentialProvider": {"apiKeyCredentialProvider": {
                    "providerArn":
                        "arn:aws:bedrock-agentcore:us-east-1:000000000000:token-vault/default/\
                         apikeycredentialprovider/keys",
                    "credentialLocation": "HEADER",
                    "credentialParameterName": "x-api-key",
                }},
            }]),
        ),
    ];

    for (index, (case, providers)) in rows.into_iter().enumerate() {
        let target_id = created_target(&router, &gateway_id, &format!("servedtarget{index}")).await;
        target_ready(&router, &gateway_id, &target_id).await;

        let (status, updated) = update_target(
            &router,
            &gateway_id,
            &target_id,
            &json!({
                "targetConfiguration": openapi_configuration("{\"openapi\":\"3.1.0\"}"),
                "credentialProviderConfigurations": providers.clone(),
            }),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {case:?}: expected HTTP 202 from UpdateGatewayTarget, got {status} ({updated})"
        );
        assert_eq!(
            updated["credentialProviderConfigurations"], providers,
            "case {case:?}: the update must report the providers it was given, got {}",
            updated["credentialProviderConfigurations"]
        );

        let (status, fetched) = send(
            &router,
            "GET",
            &format!("/gateways/{gateway_id}/targets/{target_id}/"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the updated target must read back, got {status} ({fetched})"
        );
        assert_eq!(
            fetched["credentialProviderConfigurations"], providers,
            "case {case:?}: the read must report what the update stored, got {}",
            fetched["credentialProviderConfigurations"]
        );
    }
}

/// The same precedence on the other gateway-target write: `CreateGatewayTarget` judges the
/// body member it needs for its modelled response (`name`, `@required` on every response
/// that reports a target) before it resolves the gateway the path addresses, so a body that
/// omits it is the modelled 400 even for a `gatewayIdentifier` naming no gateway, while a
/// body that carries it reaches the lookup and gets the modelled 404.
#[tokio::test]
async fn a_create_that_defines_no_name_is_refused_before_the_gateway_is_resolved() {
    let router = router();
    let gateway_id = settled_gateway(&router, "createprecedencegate").await;

    let configuration = openapi_configuration("{\"openapi\":\"3.0.0\"}");
    let providers = json!([{"credentialProviderType": "CALLER_IAM_CREDENTIALS"}]);

    // (case, the gatewayIdentifier the path names, whether the body carries `name`,
    //  expected status, expected error code, expected message)
    let rows: [(&str, &str, bool, u16, &str, String); 4] = [
        (
            "no name, and a gateway that is there",
            gateway_id.as_str(),
            false,
            400,
            "ValidationException",
            "name: is required".to_string(),
        ),
        (
            "no name, and a gatewayIdentifier naming no gateway",
            ABSENT_GATEWAY_ID,
            false,
            400,
            "ValidationException",
            "name: is required".to_string(),
        ),
        (
            "a name, and a gatewayIdentifier naming no gateway",
            ABSENT_GATEWAY_ID,
            true,
            404,
            "ResourceNotFoundException",
            "Failed to retrieve gateway because it doesn't exist. Retry the request with a \
             different resource identifier."
                .to_string(),
        ),
        (
            // The accepted row, so the table pins what the refusals are measured against.
            "a name, and a gateway that is there",
            gateway_id.as_str(),
            true,
            202,
            "",
            String::new(),
        ),
    ];

    for (case, gateway, carries_name, expected_status, expected_code, expected_message) in rows {
        let mut body = json!({
            "targetConfiguration": configuration.clone(),
            "credentialProviderConfigurations": providers.clone(),
        });
        if carries_name {
            body["name"] = json!("createprecedencetarget");
        }
        let (status, answered) = send(
            &router,
            "POST",
            &format!("/gateways/{gateway}/targets/"),
            Some(&body),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} ({answered})"
        );
        if expected_status == 202 {
            assert_eq!(
                answered["credentialProviderConfigurations"], providers,
                "case {case:?}: the created target reports what it was given, got {}",
                answered["credentialProviderConfigurations"]
            );
            // The create is proved by a read, not by its own echo.
            let created_id = text(&answered, "targetId");
            let (status, fetched) = send(
                &router,
                "GET",
                &format!("/gateways/{gateway}/targets/{created_id}/"),
                None,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the created target must read back, got {status} ({fetched})"
            );
            assert_eq!(
                text(&fetched, "name"),
                "createprecedencetarget",
                "case {case:?}: the read must report the name the create was given, got {fetched}"
            );
            assert_eq!(
                fetched["credentialProviderConfigurations"], providers,
                "case {case:?}: the read must report what the create stored, got {}",
                fetched["credentialProviderConfigurations"]
            );
            continue;
        }
        assert_eq!(
            error_code(&answered),
            expected_code,
            "case {case:?}: expected {expected_code}, got {} ({answered})",
            error_code(&answered)
        );
        assert_eq!(
            text(&answered, "message"),
            expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {}",
            text(&answered, "message")
        );
    }
}
