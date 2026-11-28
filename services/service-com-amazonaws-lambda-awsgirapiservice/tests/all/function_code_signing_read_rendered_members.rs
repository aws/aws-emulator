// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH MEMBERS a function-scoped SETTING read renders, as opposed to
//! which of them carry a value.
//!
//! RULE: a read that reports a setting of a function reports it whether or not the function
//! holds one, so every member of the read's output is rendered on every 200 and an unheld
//! setting is told apart by the member's value being null — never by the member being left
//! out, and never by a value standing in for "none".
//!
//! The two reads of this service that report one function-scoped setting are covered:
//!
//! * `GetFunctionCodeSigningConfig` — output `CodeSigningConfigArn` (required) and
//!   `FunctionName` (required), which together describe ONE attachment between a
//!   configuration and the function it covers
//!   (`smithy_query(query="operation", operation="GetFunctionCodeSigningConfig")`,
//!   `GET /2020-06-30/functions/{FunctionName}/code-signing-config`, code 200). The pair is
//!   valued together and nulled together: `CodeSigningConfigArn`'s own pattern
//!   (`^arn:…:code-signing-config:csc-[a-z0-9]{17}$`, from
//!   `smithy_query(query="shape", shape="CodeSigningConfigArn")`) admits no string standing
//!   for "none", and a `FunctionName` valued beside a null ARN would read as an attachment
//!   whose ARN was withheld.
//! * `GetFunctionConcurrency` — output `ReservedConcurrentExecutions`
//!   (`smithy_query(query="operation", operation="GetFunctionConcurrency")`,
//!   `GET /2019-09-30/functions/{FunctionName}/concurrency`, code 200), the same rule on a
//!   numeric setting: reserved, or the member rendered null.
//!
//! The rows differ along what the rule generalises over: whether the function holds the
//! setting at all, HOW it came to hold it (stated on `CreateFunction`, set by the put, set
//! over an existing value, taken away by the delete, taken away and set again), WHICH of the
//! account's two configurations (or which reserved count) it holds, and the documented
//! `FunctionName` form the read addresses the function by.
//!
//! Every row drives real HTTP requests through the router and reads the setting back after
//! each write, so a write answered but never persisted fails here too.

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
/// that ADMITS the unsigned package these rows deploy, enforcement being its own subject in
/// `tests/all/code_signing_enforcement.rs`.
const ADMITTING_POLICY: &str = "Warn";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
    /// The body as JSON, or [`Value::Null`] for an empty body.
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

    /// The member names the body carries, sorted — the subject of this suite.
    fn members(&self, case: &str) -> Vec<String> {
        let body = self.json();
        let object = body
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: expected a JSON object, got {}", self.text()));
        let mut named: Vec<String> = object.keys().cloned().collect();
        named.sort();
        named
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
        body,
    }
}

/// Where the three function-scoped code-signing operations are bound
/// (`PUT|GET|DELETE /2020-06-30/functions/{FunctionName}/code-signing-config`).
fn association_uri(reference: &str) -> String {
    format!("/2020-06-30/functions/{reference}/code-signing-config")
}

/// Where the reserved-concurrency read is bound
/// (`GET /2019-09-30/functions/{FunctionName}/concurrency`).
fn concurrency_read_uri(reference: &str) -> String {
    format!("/2019-09-30/functions/{reference}/concurrency")
}

/// Where the reserved-concurrency write and removal are bound
/// (`PUT|DELETE /2017-10-31/functions/{FunctionName}/concurrency`).
fn concurrency_write_uri(reference: &str) -> String {
    format!("/2017-10-31/functions/{reference}/concurrency")
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

/// A signing profile version ARN satisfying the model's `Arn` pattern, in the form AWS Signer
/// vends one.
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

/// `CreateFunction`, optionally stating the code signing configuration to deploy under, then
/// brought up to the `Active` state the clock owes it.
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

/// Every documented `FunctionName` form that names the function `name`, each paired with what
/// a failing assertion calls it ("Function name - MyFunction. Function ARN -
/// arn:aws:lambda:us-west-2:123456789012:function:MyFunction. Partial ARN -
/// 123456789012:function:MyFunction", from the member's own documentation).
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

/// The member names `GetFunctionCodeSigningConfigResponse` declares, sorted — the set every
/// answer of that read must carry.
fn code_signing_members() -> Vec<String> {
    vec![
        "CodeSigningConfigArn".to_string(),
        "FunctionName".to_string(),
    ]
}

/// What the association calls do to one function, in order.
enum Step {
    /// `PutFunctionCodeSigningConfig` with the configuration named by the index.
    Attach(usize),
    /// `DeleteFunctionCodeSigningConfig`.
    Detach,
}

/// RULE: every answer of `GetFunctionCodeSigningConfig` renders BOTH modelled members, valued
/// for a function that holds an attachment and null for one that holds none — whatever the
/// function's history and however the read addresses it.
#[tokio::test]
async fn the_code_signing_read_renders_both_members_whether_or_not_one_is_held() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    let configs = [
        given_config(&router, 1).await,
        given_config(&router, 2).await,
    ];
    assert_ne!(
        configs[0], configs[1],
        "the two configurations must differ, got {:?} twice",
        configs[0]
    );

    /// One row: the case, the function, the configuration its CREATE states, the association
    /// calls run on it, and the configuration it must then report (`None` for "holds none").
    struct Row {
        case: &'static str,
        function: &'static str,
        created_with: Option<usize>,
        steps: Vec<Step>,
        held: Option<usize>,
    }

    let rows = vec![
        Row {
            case: "a function that never held an attachment",
            function: "csc-rendered-never",
            created_with: None,
            steps: Vec::new(),
            held: None,
        },
        Row {
            case: "an attachment stated on the create",
            function: "csc-rendered-created",
            created_with: Some(0),
            steps: Vec::new(),
            held: Some(0),
        },
        Row {
            case: "an attachment set by the put",
            function: "csc-rendered-put",
            created_with: None,
            steps: vec![Step::Attach(1)],
            held: Some(1),
        },
        Row {
            case: "an attachment put over the one the create stated",
            function: "csc-rendered-replaced",
            created_with: Some(0),
            steps: vec![Step::Attach(1)],
            held: Some(1),
        },
        Row {
            case: "an attachment taken away by the delete",
            function: "csc-rendered-detached",
            created_with: Some(1),
            steps: vec![Step::Detach],
            held: None,
        },
        Row {
            case: "an attachment taken away and set again",
            function: "csc-rendered-rebound",
            created_with: None,
            steps: vec![Step::Attach(1), Step::Detach, Step::Attach(0)],
            held: Some(0),
        },
    ];

    for Row {
        case,
        function,
        created_with,
        steps,
        held,
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
            let attaching = matches!(step, Step::Attach(_));
            assert_eq!(
                answer.status,
                if attaching { 200 } else { 204 },
                "case {case:?}: the association call must be served, got {} {}",
                answer.status,
                answer.text()
            );
        }

        // The expected body, member by member: the pair is valued together and nulled
        // together, so one expectation covers both halves.
        let expected_arn = held.map_or(Value::Null, |index| json!(configs[index]));
        let expected_name = held.map_or(Value::Null, |_| json!(function));

        for (form, reference) in references(function) {
            let label = format!("{case}, read by {form}");
            let answer = call(&router, "GET", &association_uri(&reference), &Value::Null).await;
            assert_eq!(
                answer.status,
                200,
                "case {label:?}: the function exists, so the read is a report rather than a \
                 refusal: expected 200, got {} {}",
                answer.status,
                answer.text()
            );
            assert_eq!(
                answer.members(&label),
                code_signing_members(),
                "case {label:?}: expected both modelled members to be rendered, got {}",
                answer.text()
            );
            assert_eq!(
                answer.json()["CodeSigningConfigArn"],
                expected_arn,
                "case {label:?}: expected CodeSigningConfigArn {expected_arn}, got {}",
                answer.text()
            );
            assert_eq!(
                answer.json()["FunctionName"],
                expected_name,
                "case {label:?}: expected FunctionName {expected_name}, got {}",
                answer.text()
            );
        }
    }
}

/// RULE, over the SIBLING read of a function-scoped setting: `GetFunctionConcurrency` renders
/// its one modelled member on every answer too — the reserved count while the function
/// reserves one, and a null once it reserves none.
#[tokio::test]
async fn the_reserved_concurrency_read_renders_its_member_whether_or_not_one_is_set() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    given_function(&engine, &router, "csc-rendered-concurrency", None).await;

    /// One row: the case, the reserved count to SET first (`None` to remove it instead), and
    /// the value the read must then render.
    struct Row {
        case: &'static str,
        set: Option<i64>,
        rendered: Value,
    }

    // The counts are inside the member's own `@range` (min 0, from
    // `smithy_query(query="operation", operation="PutFunctionConcurrency")`), including the
    // published lower bound itself.
    let rows = vec![
        Row {
            case: "a function that reserves nothing",
            set: None,
            rendered: Value::Null,
        },
        Row {
            case: "a reservation of the range's own minimum",
            set: Some(0),
            rendered: json!(0),
        },
        Row {
            case: "a reservation above the minimum",
            set: Some(7),
            rendered: json!(7),
        },
        Row {
            case: "a reservation removed again",
            set: None,
            rendered: Value::Null,
        },
    ];

    for Row {
        case,
        set,
        rendered,
    } in &rows
    {
        let written = match set {
            Some(count) => {
                call(
                    &router,
                    "PUT",
                    &concurrency_write_uri("csc-rendered-concurrency"),
                    &json!({ "ReservedConcurrentExecutions": count }),
                )
                .await
            }
            None => {
                call(
                    &router,
                    "DELETE",
                    &concurrency_write_uri("csc-rendered-concurrency"),
                    &Value::Null,
                )
                .await
            }
        };
        assert!(
            (200..300).contains(&written.status),
            "case {case:?}: the reservation call must be served, got {} {}",
            written.status,
            written.text()
        );

        let answer = call(
            &router,
            "GET",
            &concurrency_read_uri("csc-rendered-concurrency"),
            &Value::Null,
        )
        .await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected 200 from GetFunctionConcurrency, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.members(case),
            vec!["ReservedConcurrentExecutions".to_string()],
            "case {case:?}: expected the modelled member to be rendered, got {}",
            answer.text()
        );
        assert_eq!(
            answer.json()["ReservedConcurrentExecutions"],
            *rendered,
            "case {case:?}: expected ReservedConcurrentExecutions {rendered}, got {}",
            answer.text()
        );
    }
}
