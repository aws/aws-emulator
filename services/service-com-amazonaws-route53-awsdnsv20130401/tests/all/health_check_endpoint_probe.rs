// SPDX-License-Identifier: CC0-1.0

//! What a health checker REPORTS about an endpoint-type health check's endpoint,
//! end to end through `GetHealthCheckStatus`, `GetHealthCheckLastFailureReason`
//! and the CloudWatch series the same health check is assessed into.
//!
//! The rule under test: **a checker's `StatusReport$Status` is the description of
//! the LATEST probe of the endpoint the configuration names, so it follows what
//! that probe observed and is not a value the type alone decides.** An endpoint
//! this emulator serves nothing on is one no checker of it can connect to, so
//! every checker reports that connection failure — a `Failure: `-prefixed
//! description — and the health check's own CloudWatch series, which is folded
//! from the SAME probes, calls the check unhealthy. The two reporters of one
//! health check therefore agree, whatever its `Type`.
//!
//! The rows differ along what the rule generalises over: every `HealthCheckType`
//! that probes an endpoint, how the endpoint is named (an IPv4 address, an IPv6
//! address, a domain name, an address WITH a name to pass in the `Host` header),
//! the port (the one the scheme is served on and one it is not), the
//! `ResourcePath` (none, a path, a path carrying a query string) and the
//! `SearchString` the string-match types search for. The probe of an endpoint the
//! emulator DOES serve — and the `SearchString` predicate applied to the body it
//! answers with — needs a second service to serve that endpoint, so it is driven
//! in `core/router/tests/all/route53_health_check_string_match.rs`.
//!
//! The series is read as a CLIENT reads it, through `cloudwatch`'s own
//! `GetMetricStatistics`, so both services are mounted on ONE engine exactly as
//! the router mounts them.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_route53_awsdnsv20130401::ops::types::HEALTH_CHECK_REGION_VALUES;
use service_com_amazonaws_route53_awsdnsv20130401::state::endpoint_probe::STATUS_CONNECTION_FAILED;
use service_com_amazonaws_route53_awsdnsv20130401::state::{
    STATUS_FAILURE_PREFIX, STATUS_SUCCESS_PREFIX,
};

use crate::health_check_status::{get_status, observations_in, Observation};
use crate::health_checks::{create_health_check, health_check_id};
use crate::hosted_zone_records::{call, Reply};

/// The Region every call below is addressed to.
const REGION: &str = "us-east-1";

/// CloudWatch's `X-Amz-Target` prefix and signing name, from that service's own
/// model (`GraniteServiceVersion20100801`, `sigv4Name: monitoring`).
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The series a health check's assessment is published as.
const NAMESPACE: &str = "AWS/Route53";
const METRIC: &str = "HealthCheckStatus";
const DIMENSION: &str = "HealthCheckId";

/// The value a datapoint of that series carries while the check is considered
/// unhealthy.
const UNHEALTHY: f64 = 0.0;

/// The probe cadence and threshold every row configures: the fastest published
/// interval (`RequestInterval` is modelled `range 10..30`) and the smallest
/// threshold (`FailureThreshold`, `range 1..10`), so one failing probe is enough
/// to move the status and the row has one assessment to wait for rather than a
/// run-up.
const INTERVAL_SECONDS: i64 = 10;
const THRESHOLD: i64 = 1;

/// How long a row waits for the assessment it reads, and how often it looks.
///
/// Generous on purpose: what is waited for is published on the emulator's own
/// accelerated schedule and takes well under a second, and the deadline is only
/// here so a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(30);
const POLL: Duration = Duration::from_millis(50);

/// A router with Route 53 and CloudWatch mounted on ONE shared engine, which is
/// how the assembled router mounts them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// Epoch seconds, the unit a CloudWatch timestamp is read and written in.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the Unix epoch")
        .as_secs_f64()
}

/// Call one CloudWatch operation over its awsJson1_0 wire, requiring a 200.
async fn cloudwatch(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    request: &Value,
) -> Value {
    let http_request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header(
            http::header::HOST,
            format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"),
        )
        .header(http::header::CONTENT_TYPE, "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        // The SigV4 credential scope is what routes the request to the
        // `monitoring` signing name.
        .header(
            http::header::AUTHORIZATION,
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{CLOUDWATCH_SIGNING_NAME}/aws4_request, \
                 SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(request).expect("the request serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudWatch {operation}, got {status} with body \
         {parsed}"
    );
    parsed
}

/// The values of the health check's status series, oldest datapoint first.
async fn assessments(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<f64> {
    let now = now_secs();
    let reported = cloudwatch(
        router,
        case,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": METRIC,
            "Dimensions": [{"Name": DIMENSION, "Value": id}],
            "StartTime": now - 1_800.0,
            "EndTime": now + 60.0,
            "Period": INTERVAL_SECONDS,
            "Statistics": ["Minimum", "Maximum"],
        }),
    )
    .await;
    let mut points: Vec<(f64, f64)> = reported["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|point| {
            Some((point["Timestamp"].as_f64()?, point["Minimum"].as_f64()?))
        })
        .collect();
    points.sort_by(|left, right| left.0.total_cmp(&right.0));
    points.into_iter().map(|(_, value)| value).collect()
}

/// The values of that series once it holds at least one assessment.
async fn settled_assessments(router: &MultiServiceRouter, case: &str, id: &str) -> Vec<f64> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let assessed = assessments(router, case, id).await;
        if !assessed.is_empty() || Instant::now() >= deadline {
            assert!(
                !assessed.is_empty(),
                "case {case:?}: the health check's status series must carry the assessments its \
                 probes were folded into, got none"
            );
            return assessed;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// The observations `GetHealthCheckLastFailureReason` reports for `id`.
async fn last_failure_observations(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> Vec<Observation> {
    let reply: Reply = call(
        router,
        "GET",
        &format!("/2013-04-01/healthcheck/{id}/lastfailurereason"),
        "",
    )
    .await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHealthCheckLastFailureReason, got {} with body {}",
        reply.status, reply.body
    );
    observations_in(&reply, case)
}

/// The observations `GetHealthCheckStatus` reports for `id`.
async fn status_observations(
    router: &MultiServiceRouter,
    id: &str,
    case: &str,
) -> Vec<Observation> {
    let reply = get_status(router, id).await;
    assert_eq!(
        reply.status, 200,
        "case {case:?}: expected 200 from GetHealthCheckStatus, got {} with body {}",
        reply.status, reply.body
    );
    observations_in(&reply, case)
}

/// One row: a configuration whose endpoint this emulator serves nothing on.
struct Row {
    case: &'static str,
    /// The `HealthCheckConfig` members, minus the cadence every row shares.
    members: &'static str,
}

/// The rows, spanning every endpoint type and every form of endpoint the
/// configuration can name.
const ROWS: &[Row] = &[
    Row {
        case: "an HTTP check of an address on the port its scheme is served on",
        members: "<Type>HTTP</Type><IPAddress>192.0.2.44</IPAddress><Port>80</Port>",
    },
    Row {
        case: "an HTTP check of a name, a port of its own and a path",
        members: "<Type>HTTP</Type>\
                  <FullyQualifiedDomainName>origin.example.com</FullyQualifiedDomainName>\
                  <Port>8080</Port><ResourcePath>/health</ResourcePath>",
    },
    Row {
        case: "an HTTPS check of an address with a name to pass in the Host header",
        members: "<Type>HTTPS</Type><IPAddress>192.0.2.45</IPAddress>\
                  <FullyQualifiedDomainName>secure.example.com</FullyQualifiedDomainName>\
                  <Port>443</Port><EnableSNI>true</EnableSNI>",
    },
    Row {
        case: "a string-match check of a name and a path carrying a query string",
        members: "<Type>HTTP_STR_MATCH</Type>\
                  <FullyQualifiedDomainName>match.example.com</FullyQualifiedDomainName>\
                  <Port>80</Port><ResourcePath>/welcome.html?language=jp</ResourcePath>\
                  <SearchString>MARKER</SearchString>",
    },
    Row {
        case: "a TLS string-match check of a name",
        members: "<Type>HTTPS_STR_MATCH</Type>\
                  <FullyQualifiedDomainName>match.example.org</FullyQualifiedDomainName>\
                  <SearchString>SERVING</SearchString>",
    },
    Row {
        case: "a TCP check of an IPv6 address",
        members: "<Type>TCP</Type><IPAddress>2001:db8:85a3::abcd:1:2345</IPAddress>\
                  <Port>22</Port>",
    },
    Row {
        // The one endpoint a checker does not dial: the API that holds the health check. A
        // checker is a client outside the service it reports to, so nothing answers it there.
        case: "a check of the Route 53 API itself, which no checker dials",
        members: "<Type>HTTP</Type>\
                  <FullyQualifiedDomainName>route53.amazonaws.com</FullyQualifiedDomainName>\
                  <Port>80</Port><ResourcePath>/2013-04-01/healthcheck</ResourcePath>",
    },
];

#[tokio::test]
async fn an_endpoint_nothing_in_this_emulator_serves_is_reported_as_a_connection_failure() {
    for (index, row) in ROWS.iter().enumerate() {
        let router = emulator();
        let created = create_health_check(
            &router,
            &format!("probe-unreachable-{index}"),
            &format!(
                "{}<RequestInterval>{INTERVAL_SECONDS}</RequestInterval>\
                 <FailureThreshold>{THRESHOLD}</FailureThreshold>",
                row.members
            ),
        )
        .await;
        let id = health_check_id(&created, row.case);
        let reported = status_observations(&router, &id, row.case).await;

        assert_eq!(
            reported.len(),
            HEALTH_CHECK_REGION_VALUES.len(),
            "case {:?}: every health-checking Region is checking this endpoint, got \
             {reported:?}",
            row.case
        );
        for observation in &reported {
            assert_eq!(
                observation.status, STATUS_CONNECTION_FAILED,
                "case {:?}: a checker that could not connect reports that failure, got {:?}",
                row.case, observation.status
            );
            assert!(
                observation.status.starts_with(STATUS_FAILURE_PREFIX)
                    && !observation.status.starts_with(STATUS_SUCCESS_PREFIX),
                "case {:?}: no checker may report a success nothing observed, got {:?}",
                row.case,
                observation.status
            );
        }

        // The OTHER reader of the same observations reports exactly the failing
        // ones, which here is all of them: the two readers of one health check
        // cannot disagree about what a checker observed.
        let failures = last_failure_observations(&router, &id, row.case).await;
        assert_eq!(
            failures, reported,
            "case {:?}: the last failure reason is the failing subset of the status, got \
             {failures:?} against {reported:?}",
            row.case
        );

        // And the series the SAME probes are folded into agrees: the check is
        // unhealthy, rather than one reporter calling it healthy and the other
        // not.
        let assessed = settled_assessments(&router, row.case, &id).await;
        assert!(
            assessed.iter().all(|value| *value == UNHEALTHY),
            "case {:?}: every assessment of a check whose probes all fail reports unhealthy, \
             got {assessed:?}",
            row.case
        );
    }
}

#[tokio::test]
async fn reading_the_reported_status_reports_the_same_probe_and_changes_nothing() {
    // The read is a report: the probe behind it is made on the check's own
    // cadence, so repeated reads — of the status and of the last failure reason,
    // interleaved — describe the same probe round and neither read moves one on.
    let router = emulator();
    let created = create_health_check(
        &router,
        "probe-stable",
        &format!(
            "<Type>HTTP_STR_MATCH</Type>\
             <FullyQualifiedDomainName>stable.example.com</FullyQualifiedDomainName>\
             <Port>80</Port><SearchString>MARKER</SearchString>\
             <RequestInterval>30</RequestInterval><FailureThreshold>{THRESHOLD}</FailureThreshold>"
        ),
    )
    .await;
    let id = health_check_id(&created, "probe-stable");
    let first = status_observations(&router, &id, "the first read").await;
    assert!(
        !first.is_empty(),
        "a health check that probes an endpoint has checkers reporting on it; got none"
    );

    for attempt in 1..=3 {
        let case = format!("read {attempt}");
        let failures = last_failure_observations(&router, &id, &case).await;
        let again = status_observations(&router, &id, &case).await;
        assert_eq!(
            again, first,
            "case {case:?}: a repeated read must report the same observations, got {again:?}"
        );
        assert_eq!(
            failures, first,
            "case {case:?}: the last failure reason must report the same observations, got \
             {failures:?}"
        );
    }
}
