// SPDX-License-Identifier: CC0-1.0

//! The refusal of a CODE-SIGNING CONFIGURATION a request addresses by its ARN carries its
//! message alone, end to end over HTTP.
//!
//! The rule under test: `Type` — "The exception type" — is an OPTIONAL member of every
//! error shape this model declares, so which refusals state it is a property of the refusal
//! and not of the envelope. An operation that takes a code-signing configuration as its own
//! resource binds `CodeSigningConfigArn` as its `@httpLabel`
//! (`smithy_query(query="operation", operation="GetCodeSigningConfig,UpdateCodeSigningConfig,DeleteCodeSigningConfig,ListFunctionsByCodeSigningConfig")`),
//! and a refusal ABOUT the configuration so addressed — it is not there
//! (`ResourceNotFoundException`, 404), or another operation holds it
//! (`ResourceConflictException`, 409, which only the deletion models) — states its message
//! and no class of fault. Every other refusal of this surface still names the class, the
//! function-scoped code-signing operations included: those address a function and merely
//! name a configuration.
//!
//! The rows differ along what the rule generalises over: which of the four operations
//! answers (each one, by each HTTP method the four use), which shape and status the refusal
//! carries, how the configuration came to be unaddressable (never created, or created and
//! deleted), which ARN is addressed, and — for the contrast rows — which operation raises
//! the same shapes about a configuration it only names.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::service::fault_class::USER_FAULT;
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateCodeSigningConfig` is bound, and the prefix the four configuration-addressed
/// operations are bound under.
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The member a fault class would travel in.
const FAULT_CLASS: &str = "Type";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Well-formed `CodeSigningConfigArn` values for configurations this account does not hold:
/// `csc-` and 17 lowercase alphanumerics, as the member's own pattern
/// (`code-signing-config:csc-[a-z0-9]{17}`) spells it, from
/// `smithy_query(query="shape", shape="CodeSigningConfigArn")`. Two of them, so a refusal
/// cannot be keyed to one particular string.
const ABSENT_ARNS: &[&str] = &[
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-0123456789abcdef0",
    "arn:aws:lambda:us-east-1:000000000000:code-signing-config:csc-zzzz9999zzzz9999z",
];

/// The `UntrustedArtifactOnDeployment` policy the configurations below carry, from
/// `smithy_query(query="shape", shape="CodeSigningPolicy")` (`Warn | Enforce`): the value
/// that admits the unsigned package these rows deploy.
const ADMITTING_POLICY: &str = "Warn";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or `Value::Null` for an empty body.
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "expected a JSON body, got {error}: {}",
                String::from_utf8_lossy(&self.body)
            )
        })
    }

    /// The wire error code (`__type`, or the `x-amzn-errortype` header).
    fn error_code(&self) -> String {
        self.json()
            .get("__type")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                self.headers
                    .get("x-amzn-errortype")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| String::from_utf8_lossy(&self.body).to_string())
    }

    /// The `message` the refusal carries, under either spelling the model uses.
    fn message(&self) -> String {
        let body = self.json();
        body.get("message")
            .or_else(|| body.get("Message"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_default()
    }

    /// The fault class the body names, if any.
    fn fault_class(&self) -> Option<String> {
        self.json()
            .get(FAULT_CLASS)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> Answer {
    let payload = if body.is_null() {
        Vec::new()
    } else {
        serde_json::to_vec(body).expect("the test request body must serialize")
    };
    let request = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `CreateCodeSigningConfig`, answering the ARN of the configuration it persisted.
async fn create_config(router: &MultiServiceRouter, index: usize) -> String {
    let profile =
        format!("arn:aws:signer:us-east-1:000000000000:/signing-profiles/Profile{index}/{index:08x}");
    let answer = call(
        router,
        "POST",
        CODE_SIGNING_CONFIGS_URI,
        &json!({
            "AllowedPublishers": { "SigningProfileVersionArns": [profile] },
            "CodeSigningPolicies": { "UntrustedArtifactOnDeployment": ADMITTING_POLICY },
            "Description": format!("configuration {index}"),
        }),
    )
    .await;
    assert_eq!(
        answer.status,
        201,
        "CreateCodeSigningConfig {index} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
    answer.json()["CodeSigningConfig"]["CodeSigningConfigArn"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "CreateCodeSigningConfig {index} must answer an ARN, got {}",
                answer.text()
            )
        })
        .to_string()
}

/// `CreateFunction`, optionally stating the code signing configuration to deploy under.
async fn create_function(router: &MultiServiceRouter, name: &str, config_arn: Option<&str>) {
    let package: Vec<u8> = (0..64u16)
        .map(|byte| u8::try_from(byte % 251).unwrap_or(0))
        .collect();
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": base64::engine::general_purpose::STANDARD.encode(&package) },
    });
    if let Some(arn) = config_arn {
        body["CodeSigningConfigArn"] = json!(arn);
    }
    let answer = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        answer.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        answer.status,
        answer.text()
    );
}

/// Move every stored function's activation instant into the past, so the next call
/// materialises the transition the clock owes. This is what waiting would do, without
/// waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// One request addressing a configuration, as each of the four operations addresses one.
struct Addressed {
    /// The operation the row drives, for the case label.
    operation: &'static str,
    method: &'static str,
    /// The path under the configuration's ARN, if the operation binds one.
    suffix: &'static str,
    body: Value,
}

/// One row per operation that takes a code-signing configuration as its own resource.
fn addressed_operations() -> Vec<Addressed> {
    vec![
        Addressed {
            operation: "GetCodeSigningConfig",
            method: "GET",
            suffix: "",
            body: Value::Null,
        },
        Addressed {
            operation: "UpdateCodeSigningConfig",
            method: "PUT",
            suffix: "",
            body: json!({ "Description": "a description the refusal never stores" }),
        },
        Addressed {
            operation: "ListFunctionsByCodeSigningConfig",
            method: "GET",
            suffix: "/functions",
            body: Value::Null,
        },
        Addressed {
            operation: "DeleteCodeSigningConfig",
            method: "DELETE",
            suffix: "",
            body: Value::Null,
        },
    ]
}

/// Drive `row` against the configuration `arn` names.
async fn address(router: &MultiServiceRouter, row: &Addressed, arn: &str) -> Answer {
    call(
        router,
        row.method,
        &format!("{CODE_SIGNING_CONFIGS_URI}/{arn}{}", row.suffix),
        &row.body,
    )
    .await
}

#[tokio::test]
async fn a_configuration_that_is_not_there_is_refused_with_its_message_alone() {
    for row in addressed_operations() {
        for arn in ABSENT_ARNS {
            // Each pairing gets its own account, so nothing a previous row left behind can
            // make a configuration addressable.
            let engine = Arc::new(engine::InMemoryEngine::new());
            let router = router_on(&engine);
            // A configuration the request does NOT address, so the row's refusal is about
            // the one it does rather than about an empty account.
            let held = create_config(&router, 0).await;
            let case = format!("{} on an ARN naming no configuration ({arn})", row.operation);

            let refused = address(&router, &row, arn).await;
            assert_eq!(
                refused.status,
                404,
                "case {case:?}: expected 404, got {} {}",
                refused.status,
                refused.text()
            );
            assert!(
                refused.error_code().contains("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                refused.error_code()
            );
            assert_eq!(
                refused.fault_class(),
                None,
                "case {case:?}: expected no {FAULT_CLASS} member, got {}",
                refused.text()
            );
            assert!(
                !refused.message().is_empty(),
                "case {case:?}: the refusal must still carry its message, got {}",
                refused.text()
            );
            // The message is the whole body beside the wire discriminator: nothing else of
            // the error shape is reported.
            let reported: Vec<String> = refused
                .json()
                .as_object()
                .map(|members| members.keys().cloned().collect())
                .unwrap_or_default();
            assert!(
                reported
                    .iter()
                    .all(|member| member == "__type" || member == "message" || member == "Message"),
                "case {case:?}: expected the message alone, got members {reported:?}"
            );

            // The refusal changed nothing: the configuration the account does hold is
            // still readable.
            let survivor = address(&router, &addressed_operations()[0], &held).await;
            assert_eq!(
                survivor.status,
                200,
                "case {case:?}: the held configuration {held:?} must still read, got {} {}",
                survivor.status,
                survivor.text()
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_configuration_is_refused_with_its_message_alone() {
    for row in addressed_operations() {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        // Two configurations, so the row addresses the one that went rather than the only
        // one there ever was.
        let gone = create_config(&router, 1).await;
        let kept = create_config(&router, 2).await;
        let deleted = call(
            &router,
            "DELETE",
            &format!("{CODE_SIGNING_CONFIGS_URI}/{gone}"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            deleted.status,
            204,
            "DeleteCodeSigningConfig {gone:?} must succeed, got {} {}",
            deleted.status,
            deleted.text()
        );
        let case = format!("{} on a deleted configuration", row.operation);

        let refused = address(&router, &row, &gone).await;
        assert_eq!(
            (
                refused.status,
                refused.error_code().contains("ResourceNotFoundException")
            ),
            (404, true),
            "case {case:?}: expected 404 ResourceNotFoundException, got {} {}",
            refused.status,
            refused.text()
        );
        assert_eq!(
            refused.fault_class(),
            None,
            "case {case:?}: expected no {FAULT_CLASS} member, got {}",
            refused.text()
        );
        assert!(
            !refused.message().is_empty(),
            "case {case:?}: the refusal must still carry its message, got {}",
            refused.text()
        );

        let survivor = address(&router, &addressed_operations()[0], &kept).await;
        assert_eq!(
            survivor.status,
            200,
            "case {case:?}: the kept configuration {kept:?} must still read, got {} {}",
            survivor.status,
            survivor.text()
        );
    }
}

#[tokio::test]
async fn a_configuration_another_operation_holds_is_refused_with_its_message_alone() {
    // The one refusal of these four operations that is not a 404: a delete of a
    // configuration a function still names.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let held = create_config(&router, 3).await;

    // Several functions name it, each association made a different way, so the refusal is
    // not a property of how one of them was made.
    create_function(&router, "csc-type-at-create", Some(&held)).await;
    make_activation_due(&engine);
    create_function(&router, "csc-type-by-put", None).await;
    make_activation_due(&engine);
    let associated = call(
        &router,
        "PUT",
        "/2020-06-30/functions/csc-type-by-put/code-signing-config",
        &json!({ "CodeSigningConfigArn": held }),
    )
    .await;
    assert_eq!(
        associated.status,
        200,
        "PutFunctionCodeSigningConfig must succeed, got {} {}",
        associated.status,
        associated.text()
    );

    let refused = call(
        &router,
        "DELETE",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{held}"),
        &Value::Null,
    )
    .await;
    let case = "DeleteCodeSigningConfig on a configuration functions still name";
    assert_eq!(
        (
            refused.status,
            refused.error_code().contains("ResourceConflictException")
        ),
        (409, true),
        "case {case:?}: expected 409 ResourceConflictException, got {} {}",
        refused.status,
        refused.text()
    );
    assert_eq!(
        refused.fault_class(),
        None,
        "case {case:?}: expected no {FAULT_CLASS} member, got {}",
        refused.text()
    );
    assert!(
        !refused.message().is_empty(),
        "case {case:?}: the refusal must still carry its message, got {}",
        refused.text()
    );

    // And the configuration is still there to be read.
    let still_there = call(
        &router,
        "GET",
        &format!("{CODE_SIGNING_CONFIGS_URI}/{held}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        still_there.status,
        200,
        "case {case:?}: a refused delete must leave {held:?} in place, got {} {}",
        still_there.status,
        still_there.text()
    );
}

#[tokio::test]
async fn every_other_code_signing_refusal_still_names_its_fault_class() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let held = create_config(&router, 4).await;
    create_function(&router, "csc-type-subject", None).await;
    make_activation_due(&engine);

    // The FUNCTION-scoped code-signing operations address a function and merely name a
    // configuration, so their refusals — including the same not-found and conflict shapes —
    // name the class of their fault, as does a refusal of a value the configuration-addressed
    // operations never got to look a configuration up with.
    let cases: Vec<(String, Answer)> = vec![
        (
            "PutFunctionCodeSigningConfig naming a configuration that is not there".to_string(),
            call(
                &router,
                "PUT",
                "/2020-06-30/functions/csc-type-subject/code-signing-config",
                &json!({ "CodeSigningConfigArn": ABSENT_ARNS[0] }),
            )
            .await,
        ),
        (
            "PutFunctionCodeSigningConfig on a function that is not there".to_string(),
            call(
                &router,
                "PUT",
                "/2020-06-30/functions/csc-type-absent/code-signing-config",
                &json!({ "CodeSigningConfigArn": held }),
            )
            .await,
        ),
        (
            "GetFunctionCodeSigningConfig on a function that is not there".to_string(),
            call(
                &router,
                "GET",
                "/2020-06-30/functions/csc-type-absent/code-signing-config",
                &Value::Null,
            )
            .await,
        ),
        (
            "DeleteFunctionCodeSigningConfig on a function that is not there".to_string(),
            call(
                &router,
                "DELETE",
                "/2020-06-30/functions/csc-type-absent/code-signing-config",
                &Value::Null,
            )
            .await,
        ),
        (
            "CreateCodeSigningConfig without the publishers it requires".to_string(),
            call(
                &router,
                "POST",
                CODE_SIGNING_CONFIGS_URI,
                &json!({ "Description": "no publishers at all" }),
            )
            .await,
        ),
        (
            "GetCodeSigningConfig on a value that is no configuration ARN".to_string(),
            call(
                &router,
                "GET",
                &format!("{CODE_SIGNING_CONFIGS_URI}/not-a-code-signing-config-arn"),
                &Value::Null,
            )
            .await,
        ),
        (
            "ListFunctionsByCodeSigningConfig past the page size its model allows".to_string(),
            call(
                &router,
                "GET",
                &format!("{CODE_SIGNING_CONFIGS_URI}/{held}/functions?MaxItems=10001"),
                &Value::Null,
            )
            .await,
        ),
    ];

    for (case, answer) in cases {
        assert!(
            (400..500).contains(&answer.status),
            "case {case:?}: expected a client refusal, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.fault_class().as_deref(),
            Some(USER_FAULT),
            "case {case:?}: expected the class {USER_FAULT} named, got {}",
            answer.text()
        );
        assert!(
            !answer.message().is_empty(),
            "case {case:?}: the refusal must still carry its message, got {}",
            answer.text()
        );
    }
}
