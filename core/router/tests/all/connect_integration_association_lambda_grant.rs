// SPDX-License-Identifier: CC0-1.0

//! The INVOKE GRANT an Amazon Connect integration association leaves on the AWS Lambda
//! function it points at, across the two services that own the two halves: Amazon Connect
//! writes the association, Lambda owns the function and its resource-based policy.
//!
//! RULE: **an integration association whose `IntegrationArn` names a Lambda function is
//! resolved against Lambda when it is WRITTEN — the instance has to invoke that function
//! for the integration to do anything — so a function Lambda states it does not hold is
//! refused and nothing is persisted, and a function it does hold is left granted
//! `lambda:InvokeFunction` for the service principal Amazon Connect invokes as, scoped by
//! `AWS:SourceArn` to the ARN of the instance the association was created on.** The grant
//! is one statement of the function's own policy, carrying the association's identifier as
//! its `Sid`, so it is told apart from any other grant the function holds. An integration
//! whose target is NOT a function takes no grant at all: no other service states what
//! associating its resource takes.
//!
//! `AddPermission` "grants a principal permission to use a function" and `GetPolicy`
//! "returns the resource-based IAM policy for a function, version, or alias", so what the
//! policy says is read where a client reads it — through Lambda's own `GetPolicy` — rather
//! than off any emulator internal.
//!
//! The rows span what the rule generalises over: the integration TYPE whose target is a
//! function (two of them, so the grant follows the target's kind rather than one type),
//! whether Lambda holds the function at all, and a target of another kind entirely.
//!
//! Two crates have to agree for that to hold, so the suite lives here rather than in
//! either one.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_connect_amazonconnectservice::state as connect_state;

/// The Region every request here is signed for, and so the Region the records live in.
const REGION: &str = "us-east-1";

/// The account id every emulated ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A role ARN inside Lambda's `Role` pattern: an execution role is provisioned outside the
/// client that creates a function, and Lambda serves a role it holds no record of.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/connect-integration";

/// A deployment package as `Code.ZipFile` carries it: base64 in a `restJson1` JSON body.
/// Its content is irrelevant here — only that Lambda accepts it and stores a function.
const ZIP_FILE: &str = "UEsDBAoAAAAAAAAAAAA=";

/// The service principal an Amazon Connect instance invokes an integration's function as,
/// and the action an invocation performs. Written out here rather than read off the
/// implementation, so the suite pins the values themselves.
const CONNECT_PRINCIPAL: &str = "connect.amazonaws.com";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The sentence Amazon Connect's `ResourceNotFoundException` carries.
const RESOURCE_NOT_FOUND: &str = "Resource not found";

/// Amazon Connect and Lambda over ONE shared engine, as `core/router/src/lib.rs` mounts
/// them, so Connect's in-process callback reaches this very Lambda.
fn build_router() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // The suite exercises the two services, not SigV4: each call carries a credential
    // scope so the router can name the service it addresses.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_connect_amazonconnectservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    (router, engine)
}

/// One answered request: its status, the `x-amzn-errortype` error code `restJson1` names a
/// modelled error shape with (empty on a success), and the parsed JSON body.
struct Answered {
    status: u16,
    error_code: String,
    body: Value,
}

/// One call to the service whose SigV4 signing name is `signing_name`, at `uri`.
async fn call(
    router: &MultiServiceRouter,
    signing_name: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answered {
    let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
    let host = format!("{signing_name}.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method(method)
        .uri(format!("https://{host}{uri}"))
        .header("host", &host)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_code = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answered {
        status,
        error_code,
        body,
    }
}

/// `CreateInstance`, handing back the instance's id and ARN.
async fn create_instance(router: &MultiServiceRouter, alias: &str) -> (String, String) {
    let created = call(
        router,
        "connect",
        "PUT",
        "/instance",
        Some(json!({
            "IdentityManagementType": "CONNECT_MANAGED",
            "InstanceAlias": alias,
            "InboundCallsEnabled": true,
            "OutboundCallsEnabled": true,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: CreateInstance got {} {}",
        created.status, created.body
    );
    (
        created.body["Id"].as_str().expect("an Id").to_string(),
        created.body["Arn"].as_str().expect("an Arn").to_string(),
    )
}

/// `CreateFunction`, handing back the ARN Lambda minted for `name`.
async fn create_function(router: &MultiServiceRouter, name: &str) -> String {
    let created = call(
        router,
        "lambda",
        "POST",
        "/2015-03-31/functions",
        Some(json!({
            "FunctionName": name,
            "Role": ROLE_ARN,
            "Runtime": "python3.12",
            "Handler": "index.handler",
            "Code": { "ZipFile": ZIP_FILE },
        })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "setup: CreateFunction {name} got {} {}",
        created.status, created.body
    );
    let arn = created.body["FunctionArn"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: the created function has an ARN, got {}", created.body))
        .to_string();
    assert_eq!(
        arn,
        format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        "setup: the modelled function ARN"
    );
    arn
}

/// `CreateIntegrationAssociation` on `instance_id`.
async fn associate(
    router: &MultiServiceRouter,
    instance_id: &str,
    integration_type: &str,
    integration_arn: &str,
) -> Answered {
    call(
        router,
        "connect",
        "PUT",
        &format!("/instance/{instance_id}/integration-associations"),
        Some(json!({
            "IntegrationType": integration_type,
            "IntegrationArn": integration_arn,
        })),
    )
    .await
}

/// The statements of the resource-based policy Lambda reports for `name`, or `None` when
/// Lambda states it can show no policy for it.
async fn policy_statements(router: &MultiServiceRouter, name: &str) -> Option<Vec<Value>> {
    let answered = call(
        router,
        "lambda",
        "GET",
        &format!("/2015-03-31/functions/{name}/policy"),
        None,
    )
    .await;
    if answered.status == 404 {
        return None;
    }
    assert_eq!(
        answered.status, 200,
        "GetPolicy {name}: got {} {}",
        answered.status, answered.body
    );
    let document: Value = serde_json::from_str(
        answered.body["Policy"]
            .as_str()
            .unwrap_or_else(|| panic!("GetPolicy answers a Policy, got {}", answered.body)),
    )
    .expect("the policy is a JSON document");
    Some(
        document["Statement"]
            .as_array()
            .unwrap_or_else(|| panic!("the policy carries statements, got {document}"))
            .clone(),
    )
}

/// How many integration associations the Amazon Connect store holds for `instance_id`.
fn held_associations(engine: &engine::InMemoryEngine, instance_id: &str) -> usize {
    let state = connect_state::service_state(engine)
        .expect("the shared engine carries the Amazon Connect store");
    let store = state.lock();
    store
        .instances
        .get(instance_id)
        .map_or(0, |instance| instance.integration_associations.len())
}

/// RULE: a function Lambda holds is left granted the instance's invocation, in one
/// statement of its own policy carrying the association's identifier — whichever
/// integration type named it — and a target of another kind takes no grant.
#[tokio::test]
async fn an_association_grants_the_instance_invoke_on_its_function() {
    let (router, engine) = build_router();
    let (instance_id, instance_arn) = create_instance(&router, "grants-invoke").await;

    // (case, the integration type, the function's name)
    let rows = [
        ("a message processor", "MESSAGE_PROCESSOR", "message-processor"),
        ("a file scanner", "FILE_SCANNER", "attachment-scanner"),
    ];
    for (case, integration_type, name) in rows {
        let function_arn = create_function(&router, name).await;
        // Nothing has granted anything on this function yet, so whatever the policy says
        // afterwards came from the association.
        assert!(
            policy_statements(&router, name).await.is_none(),
            "case {case:?}: a fresh function carries no policy"
        );

        let created = associate(&router, &instance_id, integration_type, &function_arn).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: CreateIntegrationAssociation got {} {}",
            created.status, created.body
        );
        let association_id = created.body["IntegrationAssociationId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an id, got {}", created.body)
            })
            .to_string();

        let statements = policy_statements(&router, name)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the function carries a policy afterwards"));
        assert_eq!(
            statements.len(),
            1,
            "case {case:?}: the association leaves ONE statement, got {statements:?}"
        );
        let statement = &statements[0];
        assert_eq!(
            statement["Sid"],
            json!(association_id),
            "case {case:?}: the statement is the association's, got {statement}"
        );
        assert_eq!(
            statement["Effect"],
            json!("Allow"),
            "case {case:?}: Effect, got {statement}"
        );
        assert_eq!(
            statement["Principal"],
            json!({ "Service": CONNECT_PRINCIPAL }),
            "case {case:?}: Principal, got {statement}"
        );
        assert_eq!(
            statement["Action"],
            json!(INVOKE_ACTION),
            "case {case:?}: Action, got {statement}"
        );
        assert_eq!(
            statement["Resource"],
            json!(function_arn),
            "case {case:?}: Resource, got {statement}"
        );
        assert_eq!(
            statement["Condition"]["ArnLike"]["AWS:SourceArn"],
            json!(instance_arn),
            "case {case:?}: the grant is scoped to the instance, got {statement}"
        );
    }

    // A target of another kind: the association is stored and no function's policy grows.
    let domain = format!("arn:aws:voiceid:{REGION}:{ACCOUNT}:domain/EXAMPLE1234567890");
    let created = associate(&router, &instance_id, "VOICE_ID", &domain).await;
    assert_eq!(
        created.status, 200,
        "a Voice ID domain is associated, got {} {}",
        created.status, created.body
    );
    for (_, _, name) in rows {
        let statements = policy_statements(&router, name)
            .await
            .unwrap_or_else(|| panic!("{name} still carries its policy"));
        assert_eq!(
            statements.len(),
            1,
            "{name}: a target of another kind leaves its policy alone, got {statements:?}"
        );
    }
    let held = held_associations(&engine, &instance_id);
    assert_eq!(
        held,
        rows.len() + 1,
        "the instance holds one association per create, got {held}"
    );
}

/// RULE: a function Lambda states it does not hold is refused with Amazon Connect's
/// modelled not-found error, and nothing is persisted — not the association, and not a
/// grant on anything.
#[tokio::test]
async fn an_association_to_an_absent_function_is_refused() {
    let (router, engine) = build_router();
    let (instance_id, _) = create_instance(&router, "absent-function").await;

    // One function Lambda really holds, so the refusals below are about the ARN they name
    // rather than about an emulator with no functions at all.
    let held = create_function(&router, "held-processor").await;

    let rows = [
        (
            "a function name Lambda holds nothing under",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:never-created"),
        ),
        (
            "a function of another Region",
            format!("arn:aws:lambda:eu-west-2:{ACCOUNT}:function:never-created"),
        ),
        (
            "a function of another account",
            format!("arn:aws:lambda:{REGION}:123456789012:function:never-created"),
        ),
    ];
    for (case, integration_arn) in rows {
        let refused = associate(&router, &instance_id, "MESSAGE_PROCESSOR", &integration_arn).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected HTTP 404 for {integration_arn}, got {} {}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_code, "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?} {}",
            refused.error_code, refused.body
        );
        assert_eq!(
            refused.body["Message"],
            json!(RESOURCE_NOT_FOUND),
            "case {case:?}: got {}",
            refused.body
        );
        let stored = held_associations(&engine, &instance_id);
        assert_eq!(
            stored, 0,
            "case {case:?}: a refused create stores nothing, the instance holds {stored}"
        );
        assert!(
            policy_statements(&router, "held-processor").await.is_none(),
            "case {case:?}: a refused create grants nothing on any function"
        );
    }

    // The same request against the function Lambda DOES hold is served: what the rows
    // above were refused for is the absent function, not the request's shape.
    let created = associate(&router, &instance_id, "MESSAGE_PROCESSOR", &held).await;
    assert_eq!(
        created.status, 200,
        "the held function is associated, got {} {}",
        created.status, created.body
    );
    assert_eq!(
        held_associations(&engine, &instance_id),
        1,
        "the served create stored its association"
    );
}
