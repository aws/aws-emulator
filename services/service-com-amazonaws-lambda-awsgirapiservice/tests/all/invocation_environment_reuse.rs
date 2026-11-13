// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the EXECUTION ENVIRONMENT an invocation is served in: that Lambda
//! reuses the one an earlier invocation of the same function version initialized.
//!
//! The rule under test: "After a Lambda function is invoked, Lambda maintains the execution
//! environment for some time in anticipation of another function invocation", and an
//! invocation such an environment serves "skips the initialization phase" — so the code
//! OUTSIDE the handler runs once per environment and not once per call, and "objects declared
//! outside of the function's handler method remain initialized, providing additional
//! optimization" for every later invocation. What an environment may serve is one function
//! version at one revision of its configuration: an invocation that arrives after the code or
//! the configuration changed, or that names a different version, is served by an environment
//! of its own, and an invocation the sandbox STOPPED leaves no environment to reuse.
//!
//! Reuse is a statement about the environment and NOT about any invocation's own data, so
//! every row also asserts what one invocation brings: the request id and the ARN the call
//! addressed are this invocation's, and the event it was given is appended to what the
//! environment has seen, so nothing of one invocation stands in for the next.
//!
//! The rows differ along what the rule generalises over: the event an invocation carries, the
//! qualifier it names (the unpublished version, a published version, an alias resolving to
//! it), what changed about the function between two invocations (its code, its
//! configuration, a version published from it), and how the invocation before it ended (a
//! return, a raise, a run the timeout stopped).

use std::sync::Arc;
use std::time::Duration;

use base64::Engine as _;
use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::invoke_request_id::one_invocation_stream_at_a_time;
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A SigV4 credential scope naming this service's signing name (`lambda`) in `us-east-1`.
/// Nothing in it is verified: the harness turns request authentication off.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/lambda/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The module every package below holds its handler in.
const HANDLER: &str = "lambda_function.lambda_handler";

/// How long past a due instant this suite waits before calling. Every transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How long a wait for a clock-owed transition is given before the row is failed, and how
/// often the condition is looked at again. Generous: the row is about which environment
/// serves an invocation, never about how fast this host settles an update.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long between two readings of a condition this suite waits for.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    fn json(&self) -> Value {
        if self.body.is_empty() {
            return Value::Null;
        }
        serde_json::from_slice(&self.body).unwrap_or(Value::Null)
    }

    fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).to_string()
    }

    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// A member of the document the handler returned, as text.
    fn member(&self, name: &str) -> String {
        self.json()
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// How many invocations the environment that served this one has served, as the handler
    /// counted them.
    fn served(&self) -> i64 {
        self.json()
            .get("served")
            .and_then(Value::as_i64)
            .unwrap_or_default()
    }

    /// The log tail the answer carries, decoded.
    fn log(&self) -> String {
        let encoded = self.header("x-amz-log-result").unwrap_or_default();
        base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .map(|bytes| String::from_utf8_lossy(&bytes).to_string())
            .unwrap_or_default()
    }
}

/// One service mounted on its own engine, reached the way the production router reaches it.
fn router_on(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(engine)));
    router
}

/// Dispatch one request, with the headers `headers` names.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("http://lambda.us-east-1.amazonaws.com{uri}"))
        .header("host", "lambda.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION)
        .header("content-type", "application/json");
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body.to_vec()))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// A handler that reports WHICH environment served the invocation and what that environment
/// has seen, beside what this one invocation brought.
///
/// `tag` distinguishes one deployment package from another, so a row that changes the code
/// can tell which package answered. The module-level work — the marker, the list and the line
/// written while the module is being imported — is the INIT phase: it happens once per
/// execution environment, which is what every row here reads the reuse off.
fn reporting(tag: &str) -> String {
    format!(
        "import uuid\n\
         \n\
         print('initializing an execution environment')\n\
         ENVIRONMENT = uuid.uuid4().hex\n\
         MARKS = []\n\
         \n\
         \n\
         def lambda_handler(event, context):\n\
         \x20   MARKS.append(event.get('mark'))\n\
         \x20   return {{\n\
         \x20       'tag': {tag:?},\n\
         \x20       'environment': ENVIRONMENT,\n\
         \x20       'served': len(MARKS),\n\
         \x20       'marks': MARKS,\n\
         \x20       'requestId': context.aws_request_id,\n\
         \x20       'arn': context.invoked_function_arn,\n\
         \x20   }}\n"
    )
}

/// A handler that holds its environment for as long as its event asks, so a row can end an
/// invocation on the function's own `Timeout`.
fn sleeping() -> String {
    "import time\n\
     import uuid\n\
     \n\
     ENVIRONMENT = uuid.uuid4().hex\n\
     MARKS = []\n\
     \n\
     \n\
     def lambda_handler(event, context):\n\
     \x20   MARKS.append(event.get('mark'))\n\
     \x20   if event.get('hold'):\n\
     \x20       time.sleep(float(event['hold']))\n\
     \x20   if event.get('raise'):\n\
     \x20       raise ValueError('the handler raised')\n\
     \x20   return {\n\
     \x20       'environment': ENVIRONMENT,\n\
     \x20       'served': len(MARKS),\n\
     \x20   }\n"
        .to_string()
}

/// Create a function whose code is `source`, with `timeout` seconds and the environment
/// variables `variables`, and wait out the provisioning window the create documents.
async fn create_function(
    router: &MultiServiceRouter,
    name: &str,
    source: &str,
    timeout: u32,
    publish: bool,
) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Publish": publish,
            "Timeout": timeout,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD.encode(package(source)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateFunction {name:?} must succeed, got {} {}",
        created.status,
        created.text()
    );
    tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
}

/// `Invoke` the function `reference` names with the event `event`, asking for the log tail.
async fn invoke(router: &MultiServiceRouter, reference: &str, event: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{reference}/invocations"),
        &serde_json::to_vec(event).expect("the event serializes"),
        &[("x-amz-log-type", "Tail")],
    )
    .await
}

/// Wait until the function's own readings say its last update has finished, so the next
/// invocation is served the configuration the row just wrote.
///
/// A condition, not a span: the transition is owed by the clock, and this looks again until
/// it holds or [`SETTLE_DEADLINE`] passes.
async fn settled(router: &MultiServiceRouter, name: &str, case: &str) {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let read = call(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            &[],
            &[],
        )
        .await;
        let document = read.json();
        let state = document.get("State").and_then(Value::as_str).unwrap_or("");
        let update = document
            .get("LastUpdateStatus")
            .and_then(Value::as_str)
            .unwrap_or("Successful");
        if state == "Active" && update == "Successful" {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case}: the function never reported itself ready to be invoked, last read \
             State={state:?} LastUpdateStatus={update:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A later invocation of the same function version is served by the environment an earlier
/// one initialized: the module-level code runs once, what it left behind is there for every
/// later invocation, and each invocation's own request is still its own.
#[tokio::test]
async fn a_later_invocation_is_served_by_the_environment_an_earlier_one_initialized() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-environment-reuse";
    create_function(&router, name, &reporting("first"), 10, false).await;

    // The rows differ along the event the invocation carries, which is what the environment
    // sees one of per call.
    let events: &[(&str, Value)] = &[
        ("an empty event", json!({})),
        ("an event with a mark", json!({ "mark": "one" })),
        ("an event whose mark is a number", json!({ "mark": 2 })),
        (
            "an event carrying a nested document beside it",
            json!({ "mark": "three", "nested": { "a": [1, 2, 3] } }),
        ),
        (
            "an event whose mark is a long string",
            json!({ "mark": "x".repeat(4096) }),
        ),
    ];
    let mut environment = String::new();
    let mut request_ids: Vec<String> = Vec::new();
    for (index, (case, event)) in events.iter().enumerate() {
        let answered = invoke(&router, name, event).await;
        assert_eq!(
            answered.status,
            200,
            "case {case}: the invocation must be served, got {} {}",
            answered.status,
            answered.text()
        );
        let served = answered.served();
        let reported = answered.member("environment");
        if index == 0 {
            environment = reported.clone();
            assert!(
                !environment.is_empty(),
                "case {case}: the handler must report the environment it ran in, got {}",
                answered.text()
            );
            // The module-level line is written while the environment is being built, so it
            // is in the log of the invocation that built it.
            assert!(
                answered.log().contains("initializing an execution environment"),
                "case {case}: the first invocation's log must carry the initialization it \
                 paid for, got {:?}",
                answered.log()
            );
        } else {
            assert_eq!(
                reported, environment,
                "case {case}: expected the environment {environment:?} the first invocation \
                 initialized, got {}",
                answered.text()
            );
            // Nothing is initialized a second time: the line above is not written again.
            assert!(
                !answered.log().contains("initializing an execution environment"),
                "case {case}: an invocation served by a standing environment must pay no \
                 initialization, got {:?}",
                answered.log()
            );
        }
        assert_eq!(
            served,
            i64::try_from(index).expect("the row index fits") + 1,
            "case {case}: expected the environment to have served {} invocations, got {}",
            index + 1,
            answered.text()
        );
        // The event is the invocation's own, and what the environment has seen is every
        // event it was given, in order.
        let marks = answered.json().get("marks").cloned().unwrap_or(Value::Null);
        let expected: Vec<Value> = events
            .iter()
            .take(index + 1)
            .map(|(_, event)| event.get("mark").cloned().unwrap_or(Value::Null))
            .collect();
        assert_eq!(
            marks,
            Value::Array(expected.clone()),
            "case {case}: expected the environment to have seen {expected:?}, got {marks:?}"
        );
        // And the request this invocation ran is its own, not the one the environment was
        // built for.
        let request_id = answered.member("requestId");
        assert!(
            !request_id.is_empty() && !request_ids.contains(&request_id),
            "case {case}: expected an invocation of its own, got the request id \
             {request_id:?} among {request_ids:?}"
        );
        assert_eq!(
            answered.member("arn"),
            format!("arn:aws:lambda:us-east-1:000000000000:function:{name}"),
            "case {case}: expected the ARN this call addressed, got {}",
            answered.text()
        );
        request_ids.push(request_id);
    }
}

/// Which invocations may share an environment: one function version at one revision of its
/// configuration, and no other.
///
/// The rows walk a function through everything that decides it — a published version beside
/// the unpublished one, an alias that resolves to that version, a change to the code and a
/// change to the configuration — and each states whether the invocation must be served by an
/// environment that has served before and how many invocations that environment has served.
#[tokio::test]
async fn an_environment_serves_one_version_at_one_revision_and_no_other() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-environment-identity";
    create_function(&router, name, &reporting("first"), 10, true).await;
    let alias = call(
        &router,
        "POST",
        &format!("{FUNCTIONS_URI}/{name}/aliases"),
        &serde_json::to_vec(&json!({ "Name": "live", "FunctionVersion": "1" }))
            .expect("the alias body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        alias.status,
        201,
        "CreateAlias must succeed, got {} {}",
        alias.status,
        alias.text()
    );

    // The unpublished version's own environment serves it invocation after invocation.
    let first = invoke(&router, name, &json!({})).await;
    let unpublished = first.member("environment");
    assert_eq!(
        first.served(),
        1,
        "case the unpublished version, first invocation: expected a new environment, got {}",
        first.text()
    );
    let again = invoke(&router, name, &json!({})).await;
    assert_eq!(
        (again.member("environment"), again.served()),
        (unpublished.clone(), 2),
        "case the unpublished version, second invocation: expected the same environment \
         having served two, got {}",
        again.text()
    );

    // A published version is a resource of its own, so it is served by an environment of its
    // own however many invocations the unpublished one has had.
    let published = invoke(&router, &format!("{name}:1"), &json!({})).await;
    let version = published.member("environment");
    assert_eq!(
        published.served(),
        1,
        "case the published version: expected an environment of its own, got {}",
        published.text()
    );
    assert_ne!(
        version, unpublished,
        "case the published version: expected a different environment from the unpublished \
         version's, got {version:?}"
    );

    // An alias resolves to that same version, so the invocation it admits is served by that
    // version's environment — with the ALIAS's own ARN reported to the handler.
    let aliased = invoke(&router, &format!("{name}:live"), &json!({})).await;
    assert_eq!(
        (aliased.member("environment"), aliased.served()),
        (version.clone(), 2),
        "case an alias resolving to that version: expected the version's own environment \
         having served two, got {}",
        aliased.text()
    );
    assert_eq!(
        aliased.member("arn"),
        format!("arn:aws:lambda:us-east-1:000000000000:function:{name}:live"),
        "case an alias resolving to that version: expected the ARN the call addressed, got {}",
        aliased.text()
    );

    // A change to the CODE of the unpublished version is a new revision of it, so no
    // environment built before the change serves an invocation that arrives after it.
    let updated = call(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/code"),
        &serde_json::to_vec(&json!({
            "ZipFile": base64::engine::general_purpose::STANDARD
                .encode(package(&reporting("second"))),
        }))
        .expect("the code body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        updated.status,
        200,
        "UpdateFunctionCode must succeed, got {} {}",
        updated.status,
        updated.text()
    );
    settled(&router, name, "the code changed").await;
    let after_code = invoke(&router, name, &json!({})).await;
    assert_eq!(
        (after_code.member("tag"), after_code.served()),
        ("second".to_string(), 1),
        "case the code changed: expected the new package in an environment of its own, got {}",
        after_code.text()
    );
    assert_ne!(
        after_code.member("environment"),
        unpublished,
        "case the code changed: expected a different environment from the one built before \
         the change, got {}",
        after_code.text()
    );
    let code_environment = after_code.member("environment");

    // A change to the CONFIGURATION is a new revision too: the environment variables an
    // environment was built with are the ones it holds, so an invocation that arrives after
    // they change is served by a new one.
    let configured = call(
        &router,
        "PUT",
        &format!("{FUNCTIONS_URI}/{name}/configuration"),
        &serde_json::to_vec(&json!({
            "Environment": { "Variables": { "MODE": "changed" } },
        }))
        .expect("the configuration body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        configured.status,
        200,
        "UpdateFunctionConfiguration must succeed, got {} {}",
        configured.status,
        configured.text()
    );
    settled(&router, name, "the configuration changed").await;
    let after_configuration = invoke(&router, name, &json!({})).await;
    assert_eq!(
        after_configuration.served(),
        1,
        "case the configuration changed: expected an environment of its own, got {}",
        after_configuration.text()
    );
    assert_ne!(
        after_configuration.member("environment"),
        code_environment,
        "case the configuration changed: expected a different environment from the one built \
         before the change, got {}",
        after_configuration.text()
    );

    // And none of that touched the PUBLISHED version, whose own environment is still the one
    // serving it.
    let published_again = invoke(&router, &format!("{name}:1"), &json!({})).await;
    assert_eq!(
        (published_again.member("environment"), published_again.served()),
        (version.clone(), 3),
        "case the published version after the unpublished one changed: expected its own \
         environment having served three, got {}",
        published_again.text()
    );
    assert_eq!(
        published_again.member("tag"),
        "first",
        "case the published version after the unpublished one changed: expected the package \
         the version was published from, got {}",
        published_again.text()
    );
}

/// How the invocation before it ended decides whether an environment serves another.
///
/// A handler that RETURNED and one that RAISED both leave their environment standing, which
/// is why what they left behind is there for the next invocation. A run the sandbox STOPPED
/// at the function's own `Timeout` does not: the next invocation is served by a new
/// environment.
#[tokio::test]
async fn only_a_completed_invocation_leaves_its_environment_standing() {
    let _stream = one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let name = "probe-environment-after-stop";
    // One second is the smallest `Timeout` the model allows (`smithy_query(query="shape",
    // shape="Timeout")`: `@range(min: 1)`), and the row below holds the handler well past it.
    create_function(&router, name, &sleeping(), 1, false).await;

    let first = invoke(&router, name, &json!({})).await;
    let standing = first.member("environment");
    assert_eq!(
        first.served(),
        1,
        "case the first invocation: expected a new environment, got {}",
        first.text()
    );

    // A handler that RAISED leaves its environment standing: the next invocation is served
    // in it.
    let raised = invoke(&router, name, &json!({ "raise": true })).await;
    assert_eq!(
        raised.header("x-amz-function-error").as_deref(),
        Some("Unhandled"),
        "case a handler that raised: expected the function-error signal, got {} {}",
        raised.status,
        raised.text()
    );
    let after_raise = invoke(&router, name, &json!({})).await;
    assert_eq!(
        (after_raise.member("environment"), after_raise.served()),
        (standing.clone(), 3),
        "case after a handler that raised: expected the same environment having served \
         three, got {}",
        after_raise.text()
    );

    // A run the TIMEOUT stopped leaves the interpreter part way through a frame, so that
    // environment is not reused: the next invocation is served by a new one.
    let stopped = invoke(&router, name, &json!({ "hold": 30 })).await;
    assert_eq!(
        stopped.header("x-amz-function-error").as_deref(),
        Some("Unhandled"),
        "case a run the timeout stopped: expected the function-error signal, got {} {}",
        stopped.status,
        stopped.text()
    );
    assert!(
        stopped.text().contains("Task timed out"),
        "case a run the timeout stopped: expected the sandbox timeout, got {}",
        stopped.text()
    );
    let after_stop = invoke(&router, name, &json!({})).await;
    assert_eq!(
        after_stop.served(),
        1,
        "case after a run the timeout stopped: expected an environment of its own, got {}",
        after_stop.text()
    );
    assert_ne!(
        after_stop.member("environment"),
        standing,
        "case after a run the timeout stopped: expected a different environment from the \
         stopped run's, got {}",
        after_stop.text()
    );
}
