// SPDX-License-Identifier: CC0-1.0

//! WHO may reach this service's operations: every one of them is served only to a caller the
//! emulator can verify, the metered surface included.
//!
//! The rule under test: **this service declares a SigV4 auth scheme and no anonymous
//! operation, so with request authentication on every modelled operation is refused before any
//! handler runs unless the caller's signature verifies — whatever this file wraps the modelled
//! operation in.** `override_operation` hands the framework ONE wrapper for every key the
//! generated routing table resolves (`crate::service`'s metering of "every request" this
//! service's usage series counts), and a wrapper that reported schemes of its own, or
//! swallowed the modelled operation's, would serve the whole table-routed surface — the
//! `PutMetricData` publish path with it — to anybody who can reach the port. The framework's
//! own reading is that an author-written operation reporting no scheme list DEFERS to the
//! modelled operation the request's routing key names
//! (`smithy_server_runtime::ErasedOperation::auth_schemes`), which is what this suite pins
//! from the outside: through the front door, as a client meets it.
//!
//! The rows differ along what the rule generalises over: WHICH operation is addressed (a
//! write, two reads and an alarm write, so the claim is about the surface rather than one
//! route), and WHERE the caller's unverifiable credential travels (an `Authorization` header,
//! and the query fields a presigned URL carries it in). The refusal is asserted to have left
//! the store untouched as well, which is what says the handler never ran.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{ok, TARGET_PREFIX};

/// The host a client reaches this service's regional endpoint at: its first label is the
/// service, which is what claims a request carrying no credential scope.
const ENDPOINT_HOST: &str = "monitoring.us-east-1.amazonaws.com";

/// The region the rows address.
const REGION: &str = "us-east-1";

/// The namespace the refused publish names, read back to show nothing was stored.
const NAMESPACE: &str = "MyApp/UnverifiedCaller";

/// The credential SCOPE naming this service, which is what routes a request of these rows to
/// it whatever host it addressed.
fn scope() -> String {
    format!("AKIDTEST/20260101/{REGION}/monitoring/aws4_request")
}

/// A credential scope naming this service, with a signature that is not one: the presentation
/// a caller makes when it holds no key the emulator minted.
fn unverifiable_authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential={}, SignedHeaders=host;x-amz-date, Signature=deadbeef",
        scope()
    )
}

/// The same scope and the same unverifiable signature where a PRESIGNED URL carries them: the
/// query fields, with `/` percent-encoded as a URL carries it.
fn unverifiable_query() -> String {
    format!(
        "?X-Amz-Algorithm=AWS4-HMAC-SHA256&X-Amz-Credential={scope}&X-Amz-Date=20260101T000000Z\
         &X-Amz-Expires=300&X-Amz-SignedHeaders=host&X-Amz-Signature=deadbeef",
        scope = scope().replace('/', "%2F"),
    )
}

/// One emulator with this service mounted and request authentication ON — the `--auth on`
/// front door — and a second, reading router on the SAME engine, so what the refused calls
/// left behind is read through this service's own API rather than out of its store.
fn routers() -> (MultiServiceRouter, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut authenticating = MultiServiceRouter::new();
    authenticating.set_request_authentication(true);
    authenticating.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    let mut reading = MultiServiceRouter::new();
    reading.set_request_authentication(false);
    reading.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    (authenticating, reading)
}

/// How one row's caller presents its unverifiable credential.
struct Presented {
    /// What this row varies, named in every assertion it makes.
    what: &'static str,
    /// The `Authorization` header it carries, if any.
    authorization: Option<String>,
    /// The query string it addresses the route with, empty for a header-signed row.
    query: String,
}

/// Call `operation` through `router` as this service's wire protocol carries it, with the
/// credential `presented` where that presentation puts it.
async fn call_as(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    presented: &Presented,
) -> (u16, String) {
    let mut builder = ::http::Request::builder()
        .method("POST")
        .uri(format!("/{}", presented.query))
        .header("host", ENDPOINT_HOST)
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"));
    if let Some(authorization) = &presented.authorization {
        builder = builder.header("authorization", authorization);
    }
    let request = builder
        .body(::bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// The status the emulator's front door answers a caller it cannot verify.
const UNVERIFIED_STATUS: u16 = 403;

#[tokio::test]
async fn every_modelled_operation_is_refused_to_a_caller_the_emulator_cannot_verify() {
    let (authenticating, reading) = routers();

    // One write, two reads and an alarm write: the claim is about the whole table-routed
    // surface, so the rows span it rather than naming the publish path alone.
    let operations: [(&str, Value); 4] = [
        (
            "PutMetricData",
            json!({
                "Namespace": NAMESPACE,
                "MetricData": [{"MetricName": "Latency", "Value": 1.0}],
            }),
        ),
        ("ListMetrics", json!({"Namespace": NAMESPACE})),
        (
            "PutMetricAlarm",
            json!({
                "AlarmName": "unverified-caller",
                "Namespace": NAMESPACE,
                "MetricName": "Latency",
                "ComparisonOperator": "GreaterThanThreshold",
                "EvaluationPeriods": 1,
                "Period": 60,
                "Statistic": "Average",
                "Threshold": 1.0,
            }),
        ),
        ("DescribeAlarms", json!({})),
    ];
    let presentations = [
        Presented {
            what: "an Authorization header whose signature does not verify",
            authorization: Some(unverifiable_authorization()),
            query: String::new(),
        },
        Presented {
            what: "a presigned URL whose signature does not verify",
            authorization: None,
            query: unverifiable_query(),
        },
    ];

    for (operation, body) in &operations {
        for presented in &presentations {
            let case = format!("{operation} with {}", presented.what);
            let (status, answered) = call_as(&authenticating, operation, body, presented).await;
            assert_eq!(
                status, UNVERIFIED_STATUS,
                "case {case:?}: expected HTTP {UNVERIFIED_STATUS} for a caller the emulator \
                 cannot verify, got {status} with {answered}"
            );
        }
    }

    // And the refusals really were refusals: the publish never reached a handler, so the
    // namespace it named holds no metric and the alarm it named was never created. Read
    // through this service's own API, on the same engine.
    let metrics = ok(&reading, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;
    let listed = metrics
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        listed.is_empty(),
        "a refused PutMetricData stores no datapoint, got {metrics}"
    );
    let alarms = ok(
        &reading,
        "DescribeAlarms",
        &json!({"AlarmNames": ["unverified-caller"]}),
    )
    .await;
    for member in ["MetricAlarms", "CompositeAlarms"] {
        let reported = alarms
            .get(member)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            reported.is_empty(),
            "a refused PutMetricAlarm creates no alarm, got {member} as {alarms}"
        );
    }
}

#[tokio::test]
async fn the_same_operations_are_served_once_authentication_is_not_enforced() {
    // The contrast the rule needs: the refusals above are the VERIFIER's, not this service
    // declining to serve the operations at all — the same requests, through the same front
    // door with authentication off, are served and stored.
    let (_, reading) = routers();
    let published = ok(
        &reading,
        "PutMetricData",
        &json!({
            "Namespace": NAMESPACE,
            "MetricData": [{"MetricName": "Latency", "Value": 1.0}],
        }),
    )
    .await;
    assert!(
        published.is_object(),
        "the publish is answered, got {published}"
    );
    let metrics = ok(&reading, "ListMetrics", &json!({"Namespace": NAMESPACE})).await;
    let listed = metrics
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        listed.len(),
        1,
        "the published metric is reported back, got {metrics}"
    );
}
