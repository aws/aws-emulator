// SPDX-License-Identifier: CC0-1.0

//! The `Environment` a function CARRIES, as every read of its configuration reports it,
//! end to end over HTTP.
//!
//! RULE: `FunctionConfiguration$Environment` reports "the function's environment variables",
//! so the member is part of an answer exactly while the function HOLDS at least one variable.
//! A request that states an environment naming no variable leaves the function holding none —
//! it is a CLEAR, not an empty environment to carry — so the create's own answer and every
//! later read of that function omit the member, exactly as they do for a function that never
//! named one.
//!
//! The rows differ along what the rule generalises over: how the create states the member
//! (absent, present with no member of its own, present with an empty map, present with
//! variables), which READ answers it (`CreateFunction`'s own answer,
//! `GetFunctionConfiguration`, `GetFunction`, `ListFunctions`), and — in the second test —
//! how an update moves the function between holding variables and holding none.
//!
//! Every row builds its function through the real `CreateFunction` over HTTP and reads it
//! back through the real reads, so a hardcoded answer fails. The clock-driven
//! `Pending` -> `Active` and `InProgress` -> `Successful` moves are exercised by moving the
//! stored due instants into the past, so nothing here sleeps.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`lambda`), which is what
/// routes a request here. Nothing in it is verified: the harness turns request
/// authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// Where `CreateFunction` is bound, and the member under test.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";
const ENVIRONMENT: &str = "Environment";
const VARIABLES: &str = "Variables";

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one JSON request and answer its status and body.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: &Value) -> (u16, Value) {
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
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let answer = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, answer)
}

/// The create request for `name`, stating `environment` as its `Environment` where the row
/// states one at all.
fn create_request(name: &str, environment: Option<Value>) -> Value {
    let mut request = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file() },
    });
    if let Some(environment) = environment {
        request[ENVIRONMENT] = environment;
    }
    request
}

/// Deployment package bytes; only their length and digest are ever reported.
fn zip_file() -> Value {
    let bytes: Vec<u8> = (0..64_usize)
        .map(|index| u8::try_from(index % 251).unwrap_or(0))
        .collect();
    json!(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Move every stored function's activation instant into the past, so the next read
/// materialises the `Pending` -> `Active` transition the clock owes, and every in-flight
/// update's settle instant with it.
fn settle(engine: &Arc<engine::InMemoryEngine>) {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let mut store = state.lock();
    let past = SystemTime::now() - Duration::from_secs(1);
    for function in store.functions.values_mut() {
        function.active_due = past;
        if function.last_update_due.is_some() {
            function.last_update_due = Some(past);
        }
    }
}

/// What each read of `name`'s configuration reports as its `Environment`, as
/// `(read, value)` pairs with an absent member read as `None`.
async fn reported(
    router: &MultiServiceRouter,
    name: &str,
) -> Vec<(&'static str, Option<Value>)> {
    let (status, configuration) =
        call(router, "GET", &format!("{FUNCTIONS_URI}/{name}/configuration"), &Value::Null).await;
    assert_eq!(
        status, 200,
        "function {name:?}: GetFunctionConfiguration must answer 200, got {status} \
         {configuration}"
    );
    let (status, function) =
        call(router, "GET", &format!("{FUNCTIONS_URI}/{name}"), &Value::Null).await;
    assert_eq!(
        status, 200,
        "function {name:?}: GetFunction must answer 200, got {status} {function}"
    );
    let (status, listed) = call(router, "GET", FUNCTIONS_URI, &Value::Null).await;
    assert_eq!(
        status, 200,
        "function {name:?}: ListFunctions must answer 200, got {status} {listed}"
    );
    let entry = listed["Functions"]
        .as_array()
        .and_then(|functions| {
            functions
                .iter()
                .find(|entry| entry.get("FunctionName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
        .unwrap_or_else(|| panic!("function {name:?}: ListFunctions must report it, got {listed}"));
    vec![
        ("GetFunctionConfiguration", configuration.get(ENVIRONMENT).cloned()),
        (
            "GetFunction",
            function["Configuration"].get(ENVIRONMENT).cloned(),
        ),
        ("ListFunctions", entry.get(ENVIRONMENT).cloned()),
    ]
}

#[tokio::test]
async fn the_environment_a_function_carries_is_reported_and_an_absent_one_is_not() {
    // Each row: how the create states the member, and what every read must report for it —
    // `None` meaning the member is part of no answer.
    let rows: [(&str, Option<Value>, Option<Value>); 4] = [
        ("no Environment member at all", None, None),
        ("an Environment stating no variables", Some(json!({})), None),
        (
            "an Environment stating an empty set of variables",
            Some(json!({ VARIABLES: {} })),
            None,
        ),
        (
            "an Environment stating variables",
            Some(json!({ VARIABLES: { "STAGE": "beta", "TIER": "1" } })),
            Some(json!({ VARIABLES: { "STAGE": "beta", "TIER": "1" } })),
        ),
    ];

    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (index, (case, stated, expected)) in rows.into_iter().enumerate() {
        let name = format!("environment-carried-{index}");
        let (status, created) = call(
            &router,
            "POST",
            FUNCTIONS_URI,
            &create_request(&name, stated.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: CreateFunction must answer 201, got {status} {created}"
        );
        assert_eq!(
            created.get(ENVIRONMENT).cloned(),
            expected,
            "case {case:?}: CreateFunction must report {ENVIRONMENT} as {expected:?}, got {:?}",
            created.get(ENVIRONMENT)
        );
        settle(&engine);
        for (read, value) in reported(&router, &name).await {
            assert_eq!(
                value, expected,
                "case {case:?}: {read} must report {ENVIRONMENT} as {expected:?}, got {value:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_moves_the_member_between_reported_and_absent_states() {
    // The member reports the variables the function holds NOW: an update that states the
    // member replaces them with the ones it names, and one that leaves it out changes
    // nothing about them.
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "environment-updated";
    let (status, created) = call(&router, "POST", FUNCTIONS_URI, &create_request(name, None)).await;
    assert_eq!(
        status, 201,
        "CreateFunction must answer 201, got {status} {created}"
    );
    settle(&engine);

    for (case, patch, expected) in [
        (
            "an update stating an environment with no variables",
            json!({ ENVIRONMENT: { VARIABLES: {} } }),
            None,
        ),
        (
            "an update stating variables",
            json!({ ENVIRONMENT: { VARIABLES: { "STAGE": "prod" } } }),
            Some(json!({ VARIABLES: { "STAGE": "prod" } })),
        ),
        (
            "an update naming the member not at all",
            json!({ "Description": "unrelated" }),
            Some(json!({ VARIABLES: { "STAGE": "prod" } })),
        ),
        (
            "an update emptying the variables again",
            json!({ ENVIRONMENT: { VARIABLES: {} } }),
            None,
        ),
    ] {
        let (status, updated) = call(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &patch,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: UpdateFunctionConfiguration must answer 200, got {status} {updated}"
        );
        assert_eq!(
            updated.get(ENVIRONMENT).cloned(),
            expected,
            "case {case:?}: the update must report {ENVIRONMENT} as {expected:?}, got {:?}",
            updated.get(ENVIRONMENT)
        );
        settle(&engine);
        for (read, value) in reported(&router, name).await {
            assert_eq!(
                value, expected,
                "case {case:?}: {read} must report {ENVIRONMENT} as {expected:?}, got {value:?}"
            );
        }
    }
}
