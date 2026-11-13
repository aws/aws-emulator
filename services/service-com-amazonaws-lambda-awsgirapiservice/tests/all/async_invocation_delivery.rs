// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the RETRY of an accepted asynchronous invocation: how many times
//! `Invoke` with `InvocationType=Event` delivers one event to its function.
//!
//! The rule under test: a delivery whose HANDLER errored is followed by further attempts, as
//! many as the qualifier the event was addressed to allows — "the maximum number of times to
//! retry when the function returns an error" (`MaximumRetryAttempts`) — while an event the
//! handler returned from is delivered once: the attempts are spent on the errors and on
//! nothing else.
//!
//! The rows drive that over a SET of configured allowances (one retry, and none at all),
//! because the allowance is what the rule generalises over; the documented default of two
//! more attempts (`Invoke`: "Lambda executes the function up to two more times") is the
//! allowance a qualifier nobody configured gets, which `state::async_delivery`'s own table
//! covers. Waiting that default out end to end would have this suite sleep through the
//! minute-scale waits AWS publishes between attempts for nothing: the count is the subject,
//! and the count is what a configuration states.
//!
//! That the delivery happens at all, over every form of event, qualifier, runtime and
//! package, is `asynchronous_invoke_run.rs`'s subject; this suite is about what follows ONE
//! attempt. Both read the attempt's outcome out of the service's own state, because no
//! modelled Lambda operation reports an accepted asynchronous invocation back out.
//!
//! One function serves every row: the policy is about what the RUN came to, so the same
//! package decides each event's fate from the event itself and nothing is keyed to which
//! event arrived.

use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::json;
use service_com_amazonaws_lambda_awsgirapiservice::state::execution::FUNCTION_ERROR_UNHANDLED;
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    attempts_allowed, AsyncInvocationOutcome, AsyncInvocationRecord,
    ComAmazonawsLambdaAwsgirapiserviceState, ACTIVATION_DELAY, FIRST_ASYNC_RETRY_DELAY,
};
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

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

/// How long past a due instant this suite waits before reading. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// How many retries the qualifier every row below is addressed to allows, and how many
/// another function allows instead: the two ends of `MaximumRetryAttempts`' own range that
/// this suite drives end to end (`@range(min: 0, max: 2)`).
const ONE_RETRY: i64 = 1;
/// See [`ONE_RETRY`].
const NO_RETRIES: i64 = 0;

/// How long this suite waits for every accepted event to settle: well past the attempts an
/// errored handler's event takes under [`ONE_RETRY`] — the wait AWS publishes before that
/// retry included ([`FIRST_ASYNC_RETRY_DELAY`]) — and still a bound on the test.
const SETTLE_BUDGET: Duration = FIRST_ASYNC_RETRY_DELAY.saturating_add(Duration::from_secs(120));

/// How often it re-reads the queue while it waits.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The handler the function runs: it raises when its event asks it to, and returns otherwise,
/// so which outcome an entry records is evidence both that the guest code ran and that it was
/// handed that event.
const HANDLER_SOURCE: &str = "def lambda_handler(event, context):\n\
                              \x20   if event.get('fail'):\n\
                              \x20       raise RuntimeError('refused ' + str(event['id']))\n\
                              \x20   return {'id': event['id']}\n";

/// What a dispatched request answered.
struct Answer {
    status: u16,
    body: bytes::Bytes,
}

impl Answer {
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
        body,
    }
}

/// Create the function and wait out the provisioning window the create documents ("During
/// this time, you can't invoke or modify the function"), stating how many retries its
/// unpublished version allows.
async fn create_function(router: &MultiServiceRouter, name: &str, retries: i64) {
    let created = call(
        router,
        "POST",
        FUNCTIONS_URI,
        &serde_json::to_vec(&json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Timeout": 5,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
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
    let configured = call(
        router,
        "PUT",
        &format!("/2019-09-25/functions/{name}/event-invoke-config"),
        &serde_json::to_vec(&json!({ "MaximumRetryAttempts": retries }))
            .expect("the configuration body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        configured.status,
        200,
        "PutFunctionEventInvokeConfig on {name:?} must succeed, got {} {}",
        configured.status,
        configured.text()
    );
}

/// The queue entries Lambda holds for `name`, in queue order, read out of the service's own
/// state because no modelled operation reports them.
fn held(engine: &Arc<engine::InMemoryEngine>, name: &str) -> Vec<AsyncInvocationRecord> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(name)
        .map(|function| function.async_invocations.iter().cloned().collect())
        .unwrap_or_default()
}

/// One row: the event accepted, the `FunctionError` its last attempt must report, and how
/// many attempts the event must have had.
struct Row {
    case: &'static str,
    event: &'static str,
    function_error: Option<&'static str>,
    attempts: u32,
}

/// The rows differ along what the handler does with the event — the one thing the policy
/// decides on, given the allowance in force — and each kind is sent twice over, so a retry
/// that leaked into another entry would show up as the wrong count on it.
fn rows(retries: i64) -> Vec<Row> {
    let allowed = attempts_allowed(Some(i32::try_from(retries).expect("the row's retries fit")));
    vec![
        Row {
            case: "the handler returns",
            event: r#"{"id":1}"#,
            function_error: None,
            attempts: 1,
        },
        Row {
            case: "the handler raises",
            event: r#"{"id":2,"fail":true}"#,
            function_error: Some(FUNCTION_ERROR_UNHANDLED),
            attempts: allowed,
        },
        Row {
            case: "the handler raises for another event",
            event: r#"{"id":3,"fail":true}"#,
            function_error: Some(FUNCTION_ERROR_UNHANDLED),
            attempts: allowed,
        },
        Row {
            case: "the handler returns for another event",
            event: r#"{"id":4}"#,
            function_error: None,
            attempts: 1,
        },
    ]
}

/// The whole rule, over one queue holding every row: each event is delivered, and only the
/// ones the handler errored on are delivered again — as many times as the allowance in force
/// says and no more.
///
/// Driven TWICE, over the two allowances this suite states: one retry, and none at all.
#[tokio::test]
async fn only_an_errored_delivery_is_attempted_again() {
    for (function, retries) in [("attempts", ONE_RETRY), ("attempts-none", NO_RETRIES)] {
        only_the_allowed_attempts_are_made(function, retries).await;
    }
}

/// One pass of the table above, against a function whose unpublished version allows
/// `retries`.
async fn only_the_allowed_attempts_are_made(function: &str, retries: i64) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, function, retries).await;

    let rows = rows(retries);
    for row in &rows {
        let accepted = call(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{function}/invocations"),
            row.event.as_bytes(),
            &[("x-amz-invocation-type", "Event")],
        )
        .await;
        assert_eq!(
            accepted.status,
            202,
            "case {:?}: expected 202 Accepted, got {} {}",
            row.case,
            accepted.status,
            accepted.text()
        );
    }

    // Every attempt the events have, waited for rather than slept through: an entry still
    // queued or being delivered has an attempt under way.
    let deadline = Instant::now() + SETTLE_BUDGET;
    let settled = loop {
        let entries = held(&engine, function);
        if entries.len() == rows.len()
            && entries.iter().all(|entry| {
                !matches!(
                    entry.outcome,
                    AsyncInvocationOutcome::Queued | AsyncInvocationOutcome::Delivering
                )
            })
        {
            break entries;
        }
        assert!(
            Instant::now() < deadline,
            "every accepted event must be delivered within {SETTLE_BUDGET:?}, got {:?}",
            entries
                .iter()
                .map(|entry| (entry.attempts, entry.outcome.clone()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    };

    for (entry, row) in settled.iter().zip(&rows) {
        let case = row.case;
        assert!(
            entry.payload == row.event.as_bytes(),
            "case {case:?}: expected the entry to hold its own event, got {:?}",
            String::from_utf8_lossy(&entry.payload)
        );
        let AsyncInvocationOutcome::Delivered { function_error } = &entry.outcome else {
            panic!(
                "case {case:?}: expected the handler to have run with this event, got {:?}",
                entry.outcome
            )
        };
        assert_eq!(
            function_error.as_deref(),
            row.function_error,
            "case {case:?}: expected the last attempt to report {:?}, got {function_error:?}",
            row.function_error
        );
        assert_eq!(
            entry.attempts, row.attempts,
            "case {case:?}: expected {} delivery attempt(s), got {}",
            row.attempts, entry.attempts
        );
    }
}
