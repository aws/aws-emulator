// SPDX-License-Identifier: CC0-1.0

//! The usage telemetry CloudWatch publishes about its OWN API: one `CallCount` observation in
//! the reserved `AWS/Usage` namespace per call it serves.
//!
//! The rule under test: **every call this service answers is counted into the `AWS/Usage`
//! `CallCount` series whose dimension set is `{Service, Class, Type, Resource}` = `{CloudWatch,
//! None, API, <the operation called>}`** — one observation of 1 per call, whatever the call
//! answered, so a `Sum` over a window is how many calls that window held, the series is
//! discoverable through `ListMetrics` and readable through `GetMetricData` /
//! `GetMetricStatistics`, and an operation nothing called has no datapoint at all rather than a
//! zero.
//!
//! The rows differ along what the rule generalises over: which operation was called (a read, a
//! write, a listing), how the call was ANSWERED (accepted, refused by the handler's own
//! modelled error, refused before any handler ran), and how MANY times it was called. The
//! operation the recorded case happened to count is one row among them.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, now_secs, ok};

/// The namespace an Amazon Web Services service publishes its own usage metrics in.
const USAGE_NAMESPACE: &str = "AWS/Usage";

/// The usage metric counting API requests.
const CALL_COUNT: &str = "CallCount";

/// The granularity usage is read at: a call count is published per minute.
const USAGE_PERIOD: i64 = 60;

/// The dimension set of this service's API call count, with `operation` as the resource used:
/// `Service`, `Class`, `Type` and `Resource`, the four dimensions a usage series carries.
///
/// `pub(crate)` so the suite that asserts the same counting for a call made on this service's
/// OTHER wire protocol reads the series through this one definition of it.
pub fn usage_dimensions(operation: &str) -> Value {
    json!([
        {"Name": "Service", "Value": "CloudWatch"},
        {"Name": "Class", "Value": "None"},
        {"Name": "Type", "Value": "API"},
        {"Name": "Resource", "Value": operation},
    ])
}

/// How many calls of `operation` the usage telemetry holds, and how many datapoints it is
/// spread over: the `Sum` of its `CallCount` series over the last few minutes.
///
/// The window is several periods wide and summed across them, because a run of calls can
/// straddle a minute boundary: how many calls were counted is the total, not one window's.
///
/// `pub(crate)`, for the same reason [`usage_dimensions`] is.
pub async fn counted_calls(router: &MultiServiceRouter, operation: &str) -> (f64, usize) {
    let now = now_secs();
    let body = ok(
        router,
        "GetMetricData",
        &json!({
            "MetricDataQueries": [{
                "Id": "usage",
                "MetricStat": {
                    "Metric": {
                        "Namespace": USAGE_NAMESPACE,
                        "MetricName": CALL_COUNT,
                        "Dimensions": usage_dimensions(operation),
                    },
                    "Period": USAGE_PERIOD,
                    "Stat": "Sum",
                },
            }],
            "StartTime": now - 600.0,
            "EndTime": now + 120.0,
        }),
    )
    .await;
    let values: Vec<f64> = body
        .get("MetricDataResults")
        .and_then(Value::as_array)
        .and_then(|results| results.first().cloned())
        .and_then(|result| result.get("Values").cloned())
        .and_then(|values| values.as_array().cloned())
        .map(|values| values.iter().filter_map(Value::as_f64).collect())
        .unwrap_or_else(|| panic!("GetMetricData answers one result with Values, got {body}"));
    (values.iter().sum(), values.len())
}

/// A `PutDashboard` request for `name`, carrying the one widget document a dashboard needs.
fn put_dashboard(name: &str) -> Value {
    json!({
        "DashboardName": name,
        "DashboardBody": json!({
            "widgets": [{
                "type": "metric",
                "x": 0,
                "y": 0,
                "width": 6,
                "height": 6,
                "properties": {
                    "metrics": [["AWS/EC2", "CPUUtilization"]],
                    "stat": "Average",
                    "region": "us-east-1",
                },
            }],
        })
        .to_string(),
    })
}

/// One row: the case, the operation called, the request, the status it answers, and how many
/// times the row calls it.
type UsageCase = (&'static str, &'static str, Value, u16, usize);

#[tokio::test]
async fn every_served_call_is_counted_into_the_services_own_usage_metric() {
    // The rows differ along the operation, how the call is answered — accepted, refused by the
    // operation's own modelled error, refused before any handler ran — and how many times it is
    // called. A call is counted because it was SERVED, so the answer it carried changes only
    // what the caller got back, never whether it was counted.
    let rows: Vec<UsageCase> = vec![
        (
            "a read, called several times",
            "ListMetrics",
            json!({}),
            200,
            8,
        ),
        (
            "a listing, called once",
            "ListDashboards",
            json!({}),
            200,
            1,
        ),
        (
            "a write",
            "PutDashboard",
            put_dashboard("counted-dashboard"),
            200,
            2,
        ),
        (
            "a write of metric data, which this service serves through a wrapper of its own",
            "PutMetricData",
            json!({
                "Namespace": "Custom/Usage",
                "MetricData": [{"MetricName": "Served", "Value": 1.0}],
            }),
            200,
            3,
        ),
        (
            "a read the handler refuses with its modelled not-found error",
            "GetDashboard",
            json!({"DashboardName": "no-dashboard-of-this-name"}),
            404,
            4,
        ),
        (
            "a read refused before any handler ran, for a period it cannot be read at",
            "GetMetricStatistics",
            json!({
                "Namespace": "Custom/Usage",
                "MetricName": "Served",
                "StartTime": now_secs() - 600.0,
                "EndTime": now_secs(),
                "Period": 7,
                "Statistics": ["Sum"],
            }),
            400,
            5,
        ),
        (
            "a call refused for a value its model does not admit",
            "ListManagedInsightRules",
            json!({"ResourceARN": "not-an-arn-at-all"}),
            400,
            2,
        ),
    ];

    for (case, operation, request, status, times) in rows {
        // A fresh emulator per row, so the only calls counted are the row's own and the reads
        // this suite makes to check them.
        let router = build_router();
        for _ in 0..times {
            let (answered, body) = call(&router, operation, &request).await;
            assert_eq!(
                answered, status,
                "case {case:?}: expected {operation} to answer HTTP {status}, got {answered} \
                 with body {body}"
            );
        }

        #[allow(clippy::cast_precision_loss)]
        let expected = times as f64;
        let (counted, datapoints) = counted_calls(&router, operation).await;
        assert!(
            (counted - expected).abs() < f64::EPSILON,
            "case {case:?}: expected the usage telemetry to hold {expected} calls of \
             {operation}, got {counted}"
        );
        assert!(
            datapoints >= 1,
            "case {case:?}: {times} calls of {operation} must be counted in at least one \
             datapoint, got {datapoints}"
        );

        // An operation this row never called has no series at all: a call count is published
        // for calls that happened, so a window with none holds no datapoint rather than a zero.
        let (never_called, datapoints) = counted_calls(&router, "DeleteDashboards").await;
        assert!(
            datapoints == 0 && never_called.abs() < f64::EPSILON,
            "case {case:?}: an operation nothing called must have no usage datapoint, got \
             {datapoints} holding {never_called}"
        );
    }
}

#[tokio::test]
async fn the_published_call_count_series_is_discoverable_and_readable_like_any_other() {
    let router = build_router();
    // Two different operations called a different number of times, so the discovery below has
    // more than one usage series to tell apart.
    for _ in 0..3 {
        ok(&router, "ListMetrics", &json!({})).await;
    }
    ok(&router, "ListDashboards", &json!({})).await;

    // `ListMetrics` discovers the series the service published: the usage namespace and metric
    // with the `Service`/`Type` dimensions a client knows up front, which reports one entry per
    // operation counted.
    let listed = ok(
        &router,
        "ListMetrics",
        &json!({
            "Namespace": USAGE_NAMESPACE,
            "MetricName": CALL_COUNT,
            "Dimensions": [
                {"Name": "Service", "Value": "CloudWatch"},
                {"Name": "Type", "Value": "API"},
            ],
        }),
    )
    .await;
    let discovered: Vec<Value> = listed
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let resources: Vec<String> = discovered
        .iter()
        .filter_map(|metric| metric.get("Dimensions").and_then(Value::as_array))
        .filter_map(|dimensions| {
            dimensions
                .iter()
                .find(|dimension| dimension.get("Name").and_then(Value::as_str) == Some("Resource"))
                .and_then(|dimension| dimension.get("Value").and_then(Value::as_str))
                .map(ToString::to_string)
        })
        .collect();
    for operation in ["ListMetrics", "ListDashboards"] {
        assert!(
            resources.iter().any(|resource| resource == operation),
            "expected ListMetrics to discover the call count of {operation}, got {resources:?}"
        );
    }
    for metric in &discovered {
        let names: Vec<String> = metric
            .get("Dimensions")
            .and_then(Value::as_array)
            .map(|dimensions| {
                dimensions
                    .iter()
                    .filter_map(|dimension| dimension.get("Name").and_then(Value::as_str))
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            names,
            vec!["Class", "Resource", "Service", "Type"],
            "every discovered usage series carries exactly the four usage dimensions, got \
             {metric}"
        );
    }

    // And the series reads back through `GetMetricStatistics` like any other: the datapoints
    // carry no unit of their own, which that reader reports as the unit value for "no unit".
    let now = now_secs();
    let statistics = ok(
        &router,
        "GetMetricStatistics",
        &json!({
            "Namespace": USAGE_NAMESPACE,
            "MetricName": CALL_COUNT,
            "Dimensions": usage_dimensions("ListDashboards"),
            "StartTime": now - 600.0,
            "EndTime": now + 120.0,
            "Period": USAGE_PERIOD,
            "Statistics": ["Sum"],
        }),
    )
    .await;
    let datapoints: Vec<Value> = statistics
        .get("Datapoints")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        datapoints.len(),
        1,
        "the one call of ListDashboards is one usage datapoint, got {statistics}"
    );
    assert_eq!(
        datapoints[0].get("Sum").and_then(Value::as_f64),
        Some(1.0),
        "the one call of ListDashboards is counted once, got {statistics}"
    );
    assert_eq!(
        datapoints[0].get("Unit").and_then(Value::as_str),
        Some("None"),
        "a call count carries no unit of its own, got {statistics}"
    );
}

#[tokio::test]
async fn a_minute_of_calls_is_summed_in_the_window_that_minute_begins() {
    let router = build_router();
    // The reader a usage dashboard uses asks for ONE minute: the window the calls happened in.
    // So the calls are made with most of the minute still ahead of them, and the window read is
    // the one that minute begins — which is what makes the sum of the window the number of
    // calls in it rather than a part of them spread over two.
    crate::harness::wait_for_period_room(60.0, 30.0).await;
    let boundary = (now_secs() / 60.0).floor() * 60.0;
    for _ in 0..8 {
        ok(&router, "ListMetrics", &json!({})).await;
    }
    let (status, _body) = call(
        &router,
        "GetDashboard",
        &json!({"DashboardName": "no-dashboard-of-this-name"}),
    )
    .await;
    assert_eq!(status, 404, "the refused read is the one that gets counted");

    // (the operation, the calls that minute held — and None for one nothing called)
    let rows: [(&str, Option<f64>); 3] = [
        ("ListMetrics", Some(8.0)),
        ("GetDashboard", Some(1.0)),
        ("DeleteDashboards", None),
    ];
    for (operation, expected) in rows {
        let body = ok(
            &router,
            "GetMetricData",
            &json!({
                "MetricDataQueries": [{
                    "Id": "usage",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": USAGE_NAMESPACE,
                            "MetricName": CALL_COUNT,
                            "Dimensions": usage_dimensions(operation),
                        },
                        "Period": USAGE_PERIOD,
                        "Stat": "Sum",
                    },
                }],
                "StartTime": boundary,
                "EndTime": boundary + 60.0,
            }),
        )
        .await;
        let values: Vec<f64> = body
            .get("MetricDataResults")
            .and_then(Value::as_array)
            .and_then(|results| results.first().cloned())
            .and_then(|result| result.get("Values").cloned())
            .and_then(|values| values.as_array().cloned())
            .map(|values| values.iter().filter_map(Value::as_f64).collect())
            .unwrap_or_default();
        match expected {
            Some(calls) => assert_eq!(
                values,
                vec![calls],
                "case {operation}: expected one window holding {calls} calls, got {body}"
            ),
            None => assert!(
                values.is_empty(),
                "case {operation}: an operation nothing called holds no datapoint, got {body}"
            ),
        }
    }
}

#[tokio::test]
async fn a_caller_cannot_publish_into_the_usage_namespace_the_service_meters_itself_in() {
    let router = build_router();
    // The series is the SERVICE's: the reserved namespace is refused on the write path, so a
    // caller can neither forge a call count nor add to the one this service publishes.
    let (status, body) = call(
        &router,
        "PutMetricData",
        &json!({
            "Namespace": USAGE_NAMESPACE,
            "MetricData": [{
                "MetricName": CALL_COUNT,
                "Dimensions": usage_dimensions("ListMetrics"),
                "Value": 1_000.0,
            }],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a publish into the reserved usage namespace is refused, got {status} with {body}"
    );
    let (counted, _) = counted_calls(&router, "ListMetrics").await;
    assert!(
        counted.abs() < f64::EPSILON,
        "the refused publish must count nothing against ListMetrics, got {counted}"
    );
}
