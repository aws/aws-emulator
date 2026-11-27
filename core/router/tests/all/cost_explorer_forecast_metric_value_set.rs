// SPDX-License-Identifier: CC0-1.0

//! Cost Explorer's forecast `Metric` member over the wire: the value set the operation
//! admits is the value set its modelled enum declares.
//!
//! The rule under test: **a modelled enum member admits on the wire exactly the value set
//! its enum shape declares — every value the model carries reaches the operation that
//! takes the member, and every value it does not carry is refused 400
//! `ValidationException` whose constraint report names the member and the whole modelled
//! set.** Admission is decided by the model, so the rule holds for an operation whose
//! retrieval is still scaffolded just as it does for an implemented one.
//!
//! The rows differ along what the rule generalises over: the operation taking the member
//! (both inputs that carry a `Metric`), and the value sent — each of the twelve values
//! the enum declares, and seven spellings outside it (a near miss, an admissible value in
//! the wrong case, the empty string, a padded admissible value, a value belonging to a
//! different Cost Explorer enum, a mixed-case metric name of the kind
//! `GetCostAndUsage$Metrics` takes, and a non-word).
//!
//! This suite lives here, beside the other suites of the assembled emulator, because the
//! owning crate still roots its integration tests at a hand-declared `tests/all.rs`,
//! which a fix may not edit; the router's `tests/all/` picks a new suite up with nothing
//! to declare. Every call goes through a router with the owning service mounted on one
//! shared engine, exactly as the production router mounts it, so each assertion exercises
//! the real wire path (route matching, model constraint validation, serialization) rather
//! than calling a handler directly.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` service prefix this service routes on: its wire protocol is
/// `awsJson1_1` over `POST /` (the crate's `OPERATIONS.md`, "Wire protocol").
const TARGET_PREFIX: &str = "AWSInsightsIndexService";

/// A SigV4 `Authorization` header scoped to the `ce` signing name — the routing key this
/// service claims on. Signature verification is off below, so only the scope is read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/ce/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=0123456789abcdef";

/// Every operation whose input carries a `Metric` member, from the model
/// (`smithy_query(query="digest")`: the `Metric` member of `GetCostForecastRequest` and of
/// `GetUsageForecastRequest`, required on both). The `Metrics` members of
/// `GetCostAndUsage` and its siblings are deliberately absent: they target a list of plain
/// strings, not this enum, so no enum value set is checked for them.
const OPERATIONS_WITH_METRIC: &[&str] = &["GetCostForecast", "GetUsageForecast"];

/// The modelled `Metric` value set, in the model's own order
/// (`smithy_query(query="shape", shape="Metric")`).
const METRIC_ENUM: &[&str] = &[
    "BLENDED_COST",
    "UNBLENDED_COST",
    "AMORTIZED_COST",
    "NET_UNBLENDED_COST",
    "NET_AMORTIZED_COST",
    "USAGE_QUANTITY",
    "NORMALIZED_USAGE_AMOUNT",
    "LIST_COST",
    "CONTRACTED_COST",
    "BILLED_COST",
    "PUBLIC_EQUIVALENT_BILLED_COST",
    "PUBLIC_ON_DEMAND_COST",
];

/// How the boundary constraint report names the member under test. A refusal OF this
/// member carries it; a refusal of anything else, and an answer that is no refusal at
/// all, does not.
const METRIC_MEMBER_CLAUSE: &str = "at 'metric'";

/// The fragment the boundary's enum-set report is written with
/// (`aws_smithy_schema_server::constraints`), and the one thing a request carrying a
/// modelled value must never be answered with.
const ENUM_SET_REPORT: &str = "enum value set";

/// The emulator under test: one router with the owning service mounted on one shared
/// engine, as the production router mounts it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Unsigned requests: routing still goes through the production `can_handle` and
        // the generated routing table, only the signature itself is not verified.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_costexplorer_awsinsightsindexservice::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, returning the status, the error code and the message.
    async fn call(&self, operation: &str, body: &Value) -> (u16, String, String) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "ce.us-east-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("the request body serializes"),
            ))
            .expect("a valid request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let decoded: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{operation} answered {status} with a body that is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let code = decoded
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .rsplit('#')
            .next()
            .unwrap_or_default()
            .to_string();
        let message = decoded
            .get("message")
            .or_else(|| decoded.get("Message"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        (status, code, message)
    }
}

/// A request for `operation` whose every member other than `Metric` is valid, so the
/// metric is the only thing under test. Both inputs require `TimePeriod`, `Granularity`
/// and `Metric`.
fn request(operation: &str, metric: &str) -> Value {
    assert!(
        OPERATIONS_WITH_METRIC.contains(&operation),
        "no request body is defined for {operation}"
    );
    json!({
        "TimePeriod": {"Start": "2026-01-01", "End": "2026-02-01"},
        "Granularity": "MONTHLY",
        "Metric": metric,
    })
}

/// RULE (one half): every value the modelled `Metric` enum carries is admitted at the
/// boundary and reaches the operation, for every operation that takes the member. The
/// answer the handler then gives is that operation's own business — what is asserted here
/// is that the value was not refused AS A METRIC, so a value the model carries can never
/// be reported as outside the member's value set.
#[tokio::test]
async fn every_modelled_metric_reaches_the_operation() {
    let emulator = Emulator::new();
    for operation in OPERATIONS_WITH_METRIC {
        for metric in METRIC_ENUM {
            let (status, code, message) = emulator.call(operation, &request(operation, metric)).await;
            assert_ne!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {operation:?} (Metric={metric:?}): a request whose every member is \
                 modelled must not be refused as invalid input, got {status} {code}: {message:?}"
            );
            assert!(
                !message.contains(ENUM_SET_REPORT),
                "case {operation:?} (Metric={metric:?}): a modelled metric must not fail the \
                 enum-set validator, got {status} {code}: {message:?}"
            );
            assert!(
                !message.contains(METRIC_MEMBER_CLAUSE),
                "case {operation:?} (Metric={metric:?}): a modelled metric must not be refused \
                 as a constraint violation on the member, got {status} {code}: {message:?}"
            );
        }
    }
}

/// RULE (the other half): a value outside the modelled set is refused 400
/// `ValidationException`, and the report names the member it refused, echoes the value it
/// refused and states the whole modelled value set — so a client is told every metric the
/// member really admits.
#[tokio::test]
async fn a_metric_outside_the_modelled_set_is_refused_against_that_set() {
    let rejected = [
        ("a near miss on an admissible value", "UNBLENDED_COSTS"),
        ("an admissible value in the wrong case", "unblended_cost"),
        ("the empty string", ""),
        ("an admissible value with surrounding whitespace", " LIST_COST "),
        ("a value belonging to a different enum", "SAVINGS_PLANS"),
        // The spelling `GetCostAndUsage$Metrics` takes, which is a list of plain strings
        // rather than this enum: it is not a `Metric` value and is refused as such.
        ("a mixed-case metric name", "UnblendedCost"),
        ("a non-word", "!"),
    ];

    let emulator = Emulator::new();
    for operation in OPERATIONS_WITH_METRIC {
        for (label, metric) in &rejected {
            let (status, code, message) = emulator.call(operation, &request(operation, metric)).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {label:?} ({operation} Metric={metric:?}): expected a 400 \
                 ValidationException, got {status} {code}: {message:?}"
            );
            assert!(
                message.contains(METRIC_MEMBER_CLAUSE),
                "case {label:?} ({operation} Metric={metric:?}): the report must name the \
                 member it refused, got {message:?}"
            );
            assert!(
                metric.is_empty() || message.contains(metric),
                "case {label:?} ({operation} Metric={metric:?}): the report must echo the \
                 refused value, got {message:?}"
            );
            for admissible in METRIC_ENUM {
                assert!(
                    message.contains(admissible),
                    "case {label:?} ({operation} Metric={metric:?}): the report must state the \
                     whole modelled value set, and it does not name {admissible:?}: {message:?}"
                );
            }
        }
    }
}

/// The catalog the service crate renders from the model carries exactly the modelled
/// value set — the same set the wire admits above, so the two readings of the model cannot
/// drift apart.
///
/// Nothing in the service crate reads `METRIC_VALUES` yet (the operations taking the
/// member are still scaffolded), so this is its only consumer: without it, a later model
/// update could rewrite that catalog with no test noticing. [`METRIC_ENUM`] is therefore
/// to be re-derived from `smithy_query(query="shape", shape="Metric")` whenever the model
/// changes, and never copied out of `shared_types.rs` — copying the generated catalog
/// into the expectation is what would make this pin unable to fail.
#[test]
fn the_rendered_metric_catalog_is_the_modelled_value_set() {
    let mut rendered =
        service_com_amazonaws_costexplorer_awsinsightsindexservice::ops::types::METRIC_VALUES
            .to_vec();
    rendered.sort_unstable();
    let mut modelled = METRIC_ENUM.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        rendered, modelled,
        "case \"the rendered Metric catalog\": expected the modelled value set {modelled:?}, \
         got {rendered:?}"
    );
}
