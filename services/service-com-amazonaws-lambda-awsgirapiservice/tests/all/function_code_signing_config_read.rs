// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT `GetFunctionCodeSigningConfig` reports.
//!
//! RULE: the operation "Returns the code signing configuration for the specified
//! function", and its response has exactly two members, both `@required`:
//! `CodeSigningConfigArn` and `FunctionName`
//! (`smithy_query(query="operation", operation="GetFunctionCodeSigningConfig")`). The read
//! REPORTS the association whether or not the function holds one, so BOTH members are
//! rendered on every answer: required TOGETHER, the two describe one ATTACHMENT — the
//! configuration, and the function it covers — so they are valued together for a function
//! that holds an attachment and both null for a function that holds none, their null values
//! being what tells a caller the function is covered by no code signing at all.
//! `CodeSigningConfigArn`'s own pattern (`^arn:…:code-signing-config:csc-[a-z0-9]{17}$`, from
//! `smithy_query(query="shape", shape="CodeSigningConfigArn")`) admits no string standing for
//! "none", and a `FunctionName` valued alone would read as an attachment whose ARN was
//! withheld — which is neither answer.
//!
//! The rows differ along what the rule generalises over: HOW the function came to hold its
//! association (none at all, stated on `CreateFunction`, set by
//! `PutFunctionCodeSigningConfig`, a put over what creation stated, and a
//! `DeleteFunctionCodeSigningConfig` that took it away), WHICH of the two configurations
//! of the account it holds, and the documented `FunctionName` form the read addresses the
//! function by (the bare name, the `function:` prefix, the partial ARN and the function
//! ARN — "Function name - MyFunction. Function ARN - arn:…:function:MyFunction. Partial
//! ARN - 123456789012:function:MyFunction").
//!
//! Every row drives real HTTP requests through the router and reads the association back
//! after each write, so a write answered but never persisted fails here.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// Where `CreateFunction` is bound (`POST /2015-03-31/functions`).
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// Where `CreateCodeSigningConfig` is bound (`POST /2020-04-22/code-signing-configs`).
const CODE_SIGNING_CONFIGS_URI: &str = "/2020-04-22/code-signing-configs";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`,
/// which is what routes a request here and which region the ARNs carry. Nothing in it is
/// verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// The region the scope above names.
const REGION: &str = "us-east-1";

/// The account id every emulator ARN carries (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = "000000000000";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The `CodeSigningPolicy` the configurations here carry
/// (`smithy_query(query="shape", shape="CodeSigningPolicy")`: `Warn | Enforce`): the value
/// that ADMITS the unsigned package these rows deploy, enforcement being its own subject
/// in `tests/all/code_signing_enforcement.rs`.
const ADMITTING_POLICY: &str = "Warn";

/// What a dispatched request answered.
struct Answer {
    status: u16,
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

    /// A string member of the body, when it carries one.
    fn string(&self, member: &str) -> Option<String> {
        self.json()
            .get(member)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// The body text, for a failure message.
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches
/// it.
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
        body,
    }
}

/// Where the three function-scoped code-signing operations are bound
/// (`PUT|GET|DELETE /2020-06-30/functions/{FunctionName}/code-signing-config`).
fn association_uri(reference: &str) -> String {
    format!("/2020-06-30/functions/{reference}/code-signing-config")
}

/// Move every stored function's activation instant into the past, so the next call
/// materialises the `Pending` -> `Active` transition the clock owes. This is what waiting
/// would do, without waiting.
fn make_activation_due(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
    }
}

/// A signing profile version ARN satisfying the model's `Arn` pattern, in the form AWS
/// Signer vends one.
fn signing_profile(index: usize) -> String {
    format!("arn:aws:signer:{REGION}:{ACCOUNT}:/signing-profiles/Profile{index}/{index:08x}")
}

/// `CreateCodeSigningConfig`, answering the ARN of the configuration it persisted.
async fn given_config(router: &MultiServiceRouter, index: usize) -> String {
    let answer = call(
        router,
        "POST",
        CODE_SIGNING_CONFIGS_URI,
        &json!({
            "AllowedPublishers": { "SigningProfileVersionArns": [signing_profile(index)] },
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

/// `CreateFunction`, optionally stating the code signing configuration to deploy under,
/// then brought up to the `Active` state the clock owes it.
async fn given_function(
    engine: &Arc<engine::InMemoryEngine>,
    router: &MultiServiceRouter,
    name: &str,
    config_arn: Option<&str>,
) {
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
    let created = call(router, "POST", FUNCTIONS_URI, &body).await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    make_activation_due(engine);
}

/// Every documented `FunctionName` form that names the function `name`, each paired with
/// what a failing assertion calls it.
fn references(name: &str) -> Vec<(String, String)> {
    vec![
        ("the bare name".to_string(), name.to_string()),
        (
            "the `function:` prefix form".to_string(),
            format!("function:{name}"),
        ),
        (
            "the partial ARN".to_string(),
            format!("{ACCOUNT}:function:{name}"),
        ),
        (
            "the function ARN".to_string(),
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        ),
    ]
}

/// Assert `GetFunctionCodeSigningConfig` reports `expected_arn` for `name`, addressed in
/// every documented `FunctionName` form: the attachment is reported as the PAIR of members
/// that describes it, both of them always rendered and both null for a function holding none.
async fn assert_reports(
    case: &str,
    router: &MultiServiceRouter,
    name: &str,
    expected_arn: Option<&str>,
) {
    for (form, reference) in references(name) {
        let label = format!("{case} read by {form}");
        let answer = call(router, "GET", &association_uri(&reference), &Value::Null).await;
        assert_eq!(
            answer.status,
            200,
            "case {label:?}: the function exists, so this is a report rather than a \
             refusal: expected 200, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.string("FunctionName"),
            expected_arn.map(|_| name.to_string()),
            "case {label:?}: the body must name the function exactly while it reports an \
             attachment covering it, got {}",
            answer.text()
        );
        assert_eq!(
            answer.string("CodeSigningConfigArn").as_deref(),
            expected_arn,
            "case {label:?}: expected the association {expected_arn:?}, got {}",
            answer.text()
        );
        // "None" is the member rendered NULL, never a value its own pattern does not admit
        // and never the member left out.
        assert_eq!(
            answer.json().get("CodeSigningConfigArn"),
            Some(&expected_arn.map_or(Value::Null, |arn| json!(arn))),
            "case {label:?}: a function holding no configuration still carries a \
             CodeSigningConfigArn member, with a null value, got {}",
            answer.text()
        );
    }
}

/// RULE: the read reports the attachment the function holds — the configuration ARN and the
/// function it covers — and reports both members null for a function that holds none, however
/// the function came to hold (or lose) it and however the request addresses the function.
#[tokio::test]
async fn the_read_names_its_function_and_the_configuration_it_holds() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let first = given_config(&router, 1).await;
    let second = given_config(&router, 2).await;

    // A function that holds no association at all: the body carries both members, null.
    given_function(&engine, &router, "csc-read-none", None).await;
    assert_reports("a function holding no configuration", &router, "csc-read-none", None).await;

    // One stated on the create, read back through every addressing form.
    given_function(&engine, &router, "csc-read-created", Some(&first)).await;
    assert_reports(
        "a configuration stated on the create",
        &router,
        "csc-read-created",
        Some(&first),
    )
    .await;

    // One SET by the put on a function created without any.
    given_function(&engine, &router, "csc-read-put", None).await;
    let put = call(
        &router,
        "PUT",
        &association_uri("csc-read-put"),
        &json!({ "CodeSigningConfigArn": second }),
    )
    .await;
    assert_eq!(
        put.status,
        200,
        "PutFunctionCodeSigningConfig must succeed, got {} {}",
        put.status,
        put.text()
    );
    assert_reports(
        "a configuration set by the put",
        &router,
        "csc-read-put",
        Some(&second),
    )
    .await;

    // A put OVER what the create stated: the one binding is replaced, not added to.
    let replaced = call(
        &router,
        "PUT",
        &association_uri("csc-read-created"),
        &json!({ "CodeSigningConfigArn": second }),
    )
    .await;
    assert_eq!(
        replaced.status,
        200,
        "PutFunctionCodeSigningConfig must replace the binding, got {} {}",
        replaced.status,
        replaced.text()
    );
    assert_reports(
        "a configuration put over the one the create stated",
        &router,
        "csc-read-created",
        Some(&second),
    )
    .await;

    // And the removal: the function survives it, so the read is still the operation's own
    // 200 — now with no attachment to describe, so with both members null.
    let removed = call(
        &router,
        "DELETE",
        &association_uri("csc-read-put"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        removed.status,
        204,
        "DeleteFunctionCodeSigningConfig must succeed, got {} {}",
        removed.status,
        removed.text()
    );
    assert_reports(
        "a configuration taken away by the removal",
        &router,
        "csc-read-put",
        None,
    )
    .await;

    // Nothing else's binding moved: the neighbour still reports the one it was given.
    assert_reports(
        "the neighbour of a removal",
        &router,
        "csc-read-created",
        Some(&second),
    )
    .await;
    assert!(
        first != second,
        "the two configurations must differ, got {first:?} twice"
    );
}

/// RULE, stated over the MEMBER SET itself: the body carries exactly the two members that
/// describe an attachment, whether or not the function holds one — never one of the pair
/// alone and never an object with no members.
///
/// The rows span the states a function can be in and the ways it got there: never attached,
/// attached at creation, attached by the put, re-attached over an existing attachment,
/// detached, and detached then attached again. The member set is read off the body as a
/// SORTED key list, so a member nobody asserted on would fail here too.
#[tokio::test]
async fn the_body_carries_the_attachment_pair_or_neither_member() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let first = given_config(&router, 3).await;
    let second = given_config(&router, 4).await;

    /// What the association calls do to one function, in order.
    enum Step {
        /// `PutFunctionCodeSigningConfig` with the configuration named by the index.
        Attach(usize),
        /// `DeleteFunctionCodeSigningConfig`.
        Detach,
    }

    /// One row: the case, the function, the configuration its CREATE states, the steps run
    /// on it, and the attachment it must then report.
    struct Row {
        case: &'static str,
        function: &'static str,
        created_with: Option<usize>,
        steps: Vec<Step>,
        expected: Option<usize>,
    }

    let configs = [first.clone(), second.clone()];
    let rows: Vec<Row> = vec![
        Row {
            case: "never attached",
            function: "csc-pair-never",
            created_with: None,
            steps: Vec::new(),
            expected: None,
        },
        Row {
            case: "attached at creation",
            function: "csc-pair-created",
            created_with: Some(0),
            steps: Vec::new(),
            expected: Some(0),
        },
        Row {
            case: "attached by the put",
            function: "csc-pair-put",
            created_with: None,
            steps: vec![Step::Attach(1)],
            expected: Some(1),
        },
        Row {
            case: "re-attached over an existing attachment",
            function: "csc-pair-replaced",
            created_with: Some(0),
            steps: vec![Step::Attach(1)],
            expected: Some(1),
        },
        Row {
            case: "detached",
            function: "csc-pair-detached",
            created_with: Some(0),
            steps: vec![Step::Detach],
            expected: None,
        },
        Row {
            case: "detached and attached again",
            function: "csc-pair-rebound",
            created_with: None,
            steps: vec![Step::Attach(0), Step::Detach, Step::Attach(1)],
            expected: Some(1),
        },
    ];

    for Row {
        case,
        function,
        created_with,
        steps,
        expected,
    } in &rows
    {
        given_function(
            &engine,
            &router,
            function,
            created_with.map(|index| configs[index].as_str()),
        )
        .await;
        for step in steps {
            let answer = match step {
                Step::Attach(index) => {
                    call(
                        &router,
                        "PUT",
                        &association_uri(function),
                        &json!({ "CodeSigningConfigArn": configs[*index] }),
                    )
                    .await
                }
                Step::Detach => {
                    call(&router, "DELETE", &association_uri(function), &Value::Null).await
                }
            };
            let served = matches!(step, Step::Attach(_));
            assert_eq!(
                answer.status,
                if served { 200 } else { 204 },
                "case {case:?}: the association call must be served, got {} {}",
                answer.status,
                answer.text()
            );
        }

        let answer = call(&router, "GET", &association_uri(function), &Value::Null).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: the function exists, so the read is a report rather than a \
             refusal: expected 200, got {} {}",
            answer.status,
            answer.text()
        );
        // The body is a JSON object carrying both members either way — each null for a
        // function that holds no attachment, never a value standing in for one.
        let body = answer.json();
        let members: Vec<String> = body
            .as_object()
            .map(|object| {
                let mut named: Vec<String> = object.keys().cloned().collect();
                named.sort();
                named
            })
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {}", answer.text()));
        let wanted: Vec<String> = vec![
            "CodeSigningConfigArn".to_string(),
            "FunctionName".to_string(),
        ];
        assert_eq!(
            members, wanted,
            "case {case:?}: expected the body to carry exactly {wanted:?}, got {}",
            answer.text()
        );
        // And the values of the pair describe the attachment this function holds: the stored
        // configuration, and the function it covers — or a null each when it holds none.
        assert_eq!(
            (
                answer.json()["CodeSigningConfigArn"].clone(),
                answer.json()["FunctionName"].clone()
            ),
            (
                expected.map_or(Value::Null, |index| json!(configs[index])),
                expected.map_or(Value::Null, |_| json!(function))
            ),
            "case {case:?}: expected the stored attachment, got {}",
            answer.text()
        );
    }
}
