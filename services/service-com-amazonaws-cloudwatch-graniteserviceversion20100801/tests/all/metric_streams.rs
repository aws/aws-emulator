// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the CloudWatch metric-stream configuration loop: `PutMetricStream`
//! creating and updating a stream, and `GetMetricStream` reading back what it persisted.
//!
//! Every table row is a distinct input along what the rule under test generalises over (the
//! output format, which namespace filter is used, the statistic forms a format accepts, the
//! number of filters and configurations, whether the stream already exists), so the recorded
//! happy path is one row among many rather than the specification.

use crate::harness::{build_router, call, error_code, ok};
use serde_json::{json, Value};

/// A `PutMetricStream` request with only the members the model makes required.
///
/// `pub(crate)` so every metric-stream suite in this target writes the required members once
/// — the statistics suite builds its configurations on top of this request.
pub fn put_request(name: &str, output_format: &str) -> Value {
    json!({
        "Name": name,
        "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
        "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
        "OutputFormat": output_format,
    })
}

/// A string member of a body.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// A timestamp member, which the awsJson protocols carry as epoch seconds.
fn stamp(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// A filter naming `namespace` and `count` generated metric names, for the rows that probe
/// the published "up to 1000 total names" bound.
fn filter_with_metric_names(namespace: &str, count: usize) -> Value {
    let names: Vec<String> = (0..count).map(|index| format!("Metric{index}")).collect();
    json!({"Namespace": namespace, "MetricNames": names})
}

/// A statistics configuration naming `count` metric pairs, for the rows that probe the
/// published "as many as 100 pairs in the array" bound.
fn configuration_with_include_metrics(count: usize) -> Value {
    let metrics: Vec<Value> = (0..count)
        .map(|index| json!({"Namespace": "MyApp", "MetricName": format!("Metric{index}")}))
        .collect();
    json!({"IncludeMetrics": metrics, "AdditionalStatistics": ["p99"]})
}

/// A statistics configuration asking for `count` percentile statistics on one metric, for
/// the rows that probe the published "as many as 20 statistics" bound.
fn configuration_with_statistics(count: usize) -> Value {
    let statistics: Vec<String> = (0..count).map(|index| format!("p{}", index + 1)).collect();
    json!({
        "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
        "AdditionalStatistics": statistics,
    })
}

/// `count` statistics configurations, each on its own metric, for the rows that probe the
/// published "up to 100 members" bound.
fn statistics_configurations(count: usize) -> Value {
    let configurations: Vec<Value> = (0..count)
        .map(|index| {
            json!({
                "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": format!("Metric{index}")}],
                "AdditionalStatistics": ["p99"],
            })
        })
        .collect();
    Value::Array(configurations)
}

#[tokio::test]
async fn every_configuration_form_round_trips_through_get_metric_stream() {
    // Rows differ along the output format, which of the two mutually exclusive namespace
    // filters is used (and with how many entries), whether additional statistics are
    // configured, and whether source-account metrics are included. Each row's statistics
    // are ones its own format accepts.
    let cases: Vec<(&str, &str, Value)> = vec![
        ("required members only", "json", json!({})),
        (
            "include filter with two namespaces",
            "opentelemetry0.7",
            json!({
                "IncludeFilters": [
                    {"Namespace": "AWS/EC2"},
                    {"Namespace": "AWS/Lambda", "MetricNames": ["Invocations", "Errors"]},
                ],
            }),
        ),
        (
            "exclude filter, linked-account metrics and percentiles",
            "opentelemetry1.0",
            json!({
                "ExcludeFilters": [{"Namespace": "AWS/S3"}],
                "IncludeLinkedAccountsMetrics": true,
                "StatisticsConfigurations": [{
                    "IncludeMetrics": [{"Namespace": "AWS/EC2", "MetricName": "CPUUtilization"}],
                    "AdditionalStatistics": ["p99", "p99.9"],
                }],
            }),
        ),
        (
            "json statistics beyond the percentiles, with tags",
            "json",
            json!({
                "StatisticsConfigurations": [
                    {
                        "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
                        "AdditionalStatistics": ["wm90", "tm98", "PR(:300)"],
                    },
                    {
                        "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Size"}],
                        "AdditionalStatistics": ["IQM"],
                    },
                ],
                "Tags": [{"Key": "team", "Value": "observability"}],
            }),
        ),
        (
            "empty filter lists select every namespace",
            "json",
            json!({"IncludeFilters": [], "ExcludeFilters": []}),
        ),
    ];

    for (row, (case, format, extra)) in cases.into_iter().enumerate() {
        let router = build_router();
        let name = format!("round-trip-{row}");
        let mut request = put_request(&name, format);
        for (member, value) in extra.as_object().expect("the row is an object") {
            request[member.as_str()] = value.clone();
        }

        let put = ok(&router, "PutMetricStream", &request).await;
        let arn = text(&put, "Arn").unwrap_or_default();
        assert_eq!(
            arn,
            format!("arn:aws:cloudwatch:us-east-1:000000000000:metric-stream/{name}"),
            "case {case:?}: expected the metric stream's own ARN, got {put}"
        );

        let got = ok(&router, "GetMetricStream", &json!({"Name": &name})).await;
        assert_eq!(
            text(&got, "Arn"),
            Some(arn),
            "case {case:?}: GetMetricStream must report the ARN PutMetricStream returned, got {got}"
        );
        assert_eq!(
            text(&got, "Name"),
            Some(name.clone()),
            "case {case:?}: expected the configured name, got {got}"
        );
        // "When you use PutMetricStream to create a new metric stream, the stream is created
        // in the running state."
        assert_eq!(
            text(&got, "State"),
            Some("running".to_string()),
            "case {case:?}: a new stream must be running, got {got}"
        );
        for member in ["FirehoseArn", "RoleArn", "OutputFormat"] {
            assert_eq!(
                text(&got, member),
                text(&request, member),
                "case {case:?}: {member} must round-trip, got {got}"
            );
        }
        // An empty filter list constrains nothing, which is what no filter at all means, so
        // it is reported as absent rather than as an empty list.
        for member in ["IncludeFilters", "ExcludeFilters", "StatisticsConfigurations"] {
            let configured = request
                .get(member)
                .filter(|value| value.as_array().is_some_and(|list| !list.is_empty()));
            assert_eq!(
                got.get(member),
                configured,
                "case {case:?}: {member} must round-trip, got {got}"
            );
        }
        assert_eq!(
            got.get("IncludeLinkedAccountsMetrics").and_then(Value::as_bool),
            // "If you are creating a metric stream in a monitoring account, specify true to
            // include metrics from source accounts in the metric stream."
            // (`PutMetricStreamInput$IncludeLinkedAccountsMetrics`) — a configuration that
            // does not ask for source-account metrics streams none, so the omitted form reads
            // back as that setting rather than as nothing
            // (`crate::metric_stream_scope` tables the rule).
            Some(
                request
                    .get("IncludeLinkedAccountsMetrics")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            ),
            "case {case:?}: IncludeLinkedAccountsMetrics must round-trip, got {got}"
        );
        let created = stamp(&got, "CreationDate");
        let updated = stamp(&got, "LastUpdateDate");
        assert!(
            created.is_some() && updated >= created,
            "case {case:?}: expected a creation date and a later-or-equal update date, got {got}"
        );

        // Reading twice changes nothing a later caller sees.
        let again = ok(&router, "GetMetricStream", &json!({"Name": &name})).await;
        assert_eq!(
            again, got,
            "case {case:?}: a second GetMetricStream must report the same stream, got {again}"
        );
    }
}

#[tokio::test]
async fn an_update_overwrites_the_configuration_and_keeps_the_identity() {
    let router = build_router();
    let created = ok(
        &router,
        "PutMetricStream",
        &json!({
            "Name": "updated_stream",
            "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/first",
            "RoleArn": "arn:aws:iam::000000000000:role/First",
            "OutputFormat": "json",
            "IncludeFilters": [{"Namespace": "MyApp"}],
            "Tags": [{"Key": "stage", "Value": "create"}],
        }),
    )
    .await;
    let before = ok(&router, "GetMetricStream", &json!({"Name": "updated_stream"})).await;

    // Rows differ in what the update changes: the destination, the filter kind (an include
    // filter replaced by an exclude filter), the output format, and the statistics.
    let updates: Vec<(&str, Value)> = vec![
        (
            "exclude filter replaces include filter",
            json!({
                "Name": "updated_stream",
                "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/second",
                "RoleArn": "arn:aws:iam::000000000000:role/Second",
                "OutputFormat": "opentelemetry1.0",
                "ExcludeFilters": [{"Namespace": "AWS/S3"}],
                "StatisticsConfigurations": [{
                    "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
                    "AdditionalStatistics": ["p95"],
                }],
                "Tags": [{"Key": "stage", "Value": "update"}],
            }),
        ),
        (
            "back to the bare required members",
            put_request("updated_stream", "json"),
        ),
    ];

    for (case, update) in updates {
        let put = ok(&router, "PutMetricStream", &update).await;
        assert_eq!(
            text(&put, "Arn"),
            text(&created, "Arn"),
            "case {case:?}: an update must keep the stream's ARN, got {put}"
        );

        let after = ok(&router, "GetMetricStream", &json!({"Name": "updated_stream"})).await;
        assert_eq!(
            stamp(&after, "CreationDate"),
            stamp(&before, "CreationDate"),
            "case {case:?}: CreationDate must survive an update, got {after}"
        );
        assert!(
            stamp(&after, "LastUpdateDate") >= stamp(&before, "LastUpdateDate"),
            "case {case:?}: LastUpdateDate must not move backwards, got {after}"
        );
        // "If you use it to update an existing stream, the state of the stream is not
        // changed."
        assert_eq!(
            text(&after, "State"),
            text(&before, "State"),
            "case {case:?}: an update must not change the stream's state, got {after}"
        );
        // The update "completely overwrites" the configuration: what it does not carry is
        // gone, what it carries is reported back.
        for member in [
            "FirehoseArn",
            "RoleArn",
            "OutputFormat",
            "IncludeFilters",
            "ExcludeFilters",
            "StatisticsConfigurations",
        ] {
            assert_eq!(
                after.get(member),
                update.get(member),
                "case {case:?}: {member} must be exactly what the update configured, got {after}"
            );
        }
    }
}

#[tokio::test]
async fn an_additional_statistic_is_accepted_only_where_the_output_format_carries_it() {
    // The rule: AdditionalStatistics names statistics a stream does not send anyway, so a
    // basic statistic is admissible in no format; an OpenTelemetry stream narrows the member
    // to the percentiles, while a json stream takes every statistic abbreviation CloudWatch
    // defines. Rows span the statistic forms the "CloudWatch statistics definitions" page
    // lists — basic statistics (in more than one letter case, since nothing of the member is
    // normalised), percentiles, the trimmed/Winsorized families in both their bare and
    // spanned forms, the percentile rank family, the interquartile mean — plus values no
    // family produces.
    let statistics: Vec<(&str, bool, bool)> = vec![
        // (statistic, valid for json, valid for an OpenTelemetry format)
        ("p99", true, true),
        ("p0", true, true),
        ("p100", true, true),
        ("p99.9", true, true),
        ("p0.5", true, true),
        ("p101", false, false),
        ("Average", false, false),
        ("average", false, false),
        ("SampleCount", false, false),
        ("Minimum", false, false),
        ("Maximum", false, false),
        ("MAXIMUM", false, false),
        ("Sum", false, false),
        ("IQM", true, false),
        ("tm98", true, false),
        ("wm90", true, false),
        ("TM(10%:90%)", true, false),
        ("wm(10%:90%)", true, false),
        ("TC(0.5:2.5)", true, false),
        ("ts(100:)", true, false),
        ("PR(:300)", true, false),
        ("Median", false, false),
        ("tm110", false, false),
    ];

    for (statistic, valid_for_json, valid_for_otel) in statistics {
        for (format, expected_ok) in [
            ("json", valid_for_json),
            ("opentelemetry0.7", valid_for_otel),
            ("opentelemetry1.0", valid_for_otel),
        ] {
            let router = build_router();
            let case = format!("{statistic} with {format}");
            let mut request = put_request("statistics-stream", format);
            request["StatisticsConfigurations"] = json!([{
                "IncludeMetrics": [{"Namespace": "MyApp", "MetricName": "Latency"}],
                "AdditionalStatistics": [statistic],
            }]);

            let (status, body) = call(&router, "PutMetricStream", &request).await;
            if expected_ok {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the statistic to be accepted, got {status} {body}"
                );
                let got = ok(
                    &router,
                    "GetMetricStream",
                    &json!({"Name": "statistics-stream"}),
                )
                .await;
                assert_eq!(
                    got.get("StatisticsConfigurations"),
                    request.get("StatisticsConfigurations"),
                    "case {case:?}: the configuration must round-trip, got {got}"
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "InvalidParameterValueException"),
                    "case {case:?}: expected a 400 InvalidParameterValueException, got {status} {body}"
                );
                // CloudWatch words this refusal as one fixed sentence carrying the
                // statistic the request asked for, whichever format and statistic the row
                // names.
                assert_eq!(
                    text(&body, "message").as_deref(),
                    Some(format!("Unsupported statistic {statistic} for selected OutputFormat").as_str()),
                    "case {case:?}: expected the refusal to name the statistic in \
                     CloudWatch's sentence, got {body}"
                );
                // A refused request persists nothing.
                let (get_status, get_body) = call(
                    &router,
                    "GetMetricStream",
                    &json!({"Name": "statistics-stream"}),
                )
                .await;
                assert_eq!(
                    (get_status, error_code(&get_body).as_str()),
                    (404, "ResourceNotFoundException"),
                    "case {case:?}: a refused PutMetricStream must persist nothing, got {get_body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_configuration_is_accepted_at_each_documented_size_and_refused_past_it() {
    // One row per published size bound, each tested AT the bound and one past it. The sizes
    // come from the model's own documentation:
    //   * "A metric stream's filters can include up to 1000 total names. This limit applies
    //     to the sum of namespace names and metric names in the filters." (MetricStreamFilter)
    //   * "There can be as many as 100 pairs in the array."
    //     (MetricStreamStatisticsConfiguration$IncludeMetrics)
    //   * "This list can include as many as 20 statistics."
    //     (MetricStreamStatisticsConfiguration$AdditionalStatistics)
    //   * "This array can have up to 100 members."
    //     (PutMetricStreamInput$StatisticsConfigurations)
    // A filter naming one namespace and 999 metric names is exactly 1000 names.
    let cases: Vec<(&str, &str, Value, bool)> = vec![
        (
            "1000 names in an include filter",
            "json",
            json!({"IncludeFilters": [filter_with_metric_names("MyApp", 999)]}),
            true,
        ),
        (
            "1001 names in an include filter",
            "json",
            json!({"IncludeFilters": [filter_with_metric_names("MyApp", 1000)]}),
            false,
        ),
        (
            "1000 names spread over two exclude filters",
            "json",
            json!({"ExcludeFilters": [
                filter_with_metric_names("MyApp", 499),
                filter_with_metric_names("Other", 499),
            ]}),
            true,
        ),
        (
            "1001 names spread over two exclude filters",
            "json",
            json!({"ExcludeFilters": [
                filter_with_metric_names("MyApp", 500),
                filter_with_metric_names("Other", 499),
            ]}),
            false,
        ),
        (
            "100 include-metric pairs in one configuration",
            "opentelemetry1.0",
            json!({"StatisticsConfigurations": [configuration_with_include_metrics(100)]}),
            true,
        ),
        (
            "101 include-metric pairs in one configuration",
            "opentelemetry1.0",
            json!({"StatisticsConfigurations": [configuration_with_include_metrics(101)]}),
            false,
        ),
        (
            "20 additional statistics in one configuration",
            "opentelemetry0.7",
            json!({"StatisticsConfigurations": [configuration_with_statistics(20)]}),
            true,
        ),
        (
            "21 additional statistics in one configuration",
            "opentelemetry0.7",
            json!({"StatisticsConfigurations": [configuration_with_statistics(21)]}),
            false,
        ),
        (
            "100 statistics configurations",
            "opentelemetry1.0",
            json!({"StatisticsConfigurations": statistics_configurations(100)}),
            true,
        ),
        (
            "101 statistics configurations",
            "opentelemetry1.0",
            json!({"StatisticsConfigurations": statistics_configurations(101)}),
            false,
        ),
    ];

    for (case, format, extra, expected_ok) in cases {
        let router = build_router();
        let mut request = put_request("sized-stream", format);
        for (member, value) in extra.as_object().expect("the row is an object") {
            request[member.as_str()] = value.clone();
        }

        let (status, body) = call(&router, "PutMetricStream", &request).await;
        if expected_ok {
            assert_eq!(
                status, 200,
                "case {case:?}: a configuration at the documented bound must be accepted, got \
                 {status} {body}"
            );
            let got = ok(&router, "GetMetricStream", &json!({"Name": "sized-stream"})).await;
            // Only the members this row populated: a member the row does not set would
            // compare `None` with `None` and assert nothing.
            for member in extra.as_object().expect("the row is an object").keys() {
                assert_eq!(
                    got.get(member),
                    request.get(member),
                    "case {case:?}: {member} must round-trip, got {got}"
                );
            }
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidParameterValueException"),
                "case {case:?}: a configuration past the documented bound must be refused with \
                 a 400 InvalidParameterValueException, got {status} {body}"
            );
            // A refused request persists nothing.
            let (get_status, get_body) =
                call(&router, "GetMetricStream", &json!({"Name": "sized-stream"})).await;
            assert_eq!(
                (get_status, error_code(&get_body).as_str()),
                (404, "ResourceNotFoundException"),
                "case {case:?}: a refused PutMetricStream must persist nothing, got {get_body}"
            );
        }
    }
}

#[tokio::test]
async fn a_request_the_operation_forbids_is_refused_with_its_modelled_error() {
    // Rows differ along the rule each one breaks: the two filters in one operation, and the
    // characters a stream name may use.
    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "both filter lists given",
            json!({
                "Name": "both-filters",
                "FirehoseArn": "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
                "RoleArn": "arn:aws:iam::000000000000:role/MetricStreamRole",
                "OutputFormat": "json",
                "IncludeFilters": [{"Namespace": "AWS/EC2"}],
                "ExcludeFilters": [{"Namespace": "AWS/S3"}],
            }),
            400,
            "InvalidParameterCombinationException",
        ),
        (
            "a name with a space",
            put_request("my stream", "json"),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a name with a slash",
            put_request("team/stream", "json"),
            400,
            "InvalidParameterValueException",
        ),
        (
            "a name with a dot",
            put_request("stream.one", "json"),
            400,
            "InvalidParameterValueException",
        ),
        (
            "every character the name rule allows",
            put_request("Stream-9_name", "json"),
            200,
            "",
        ),
    ];

    for (case, request, expected_status, expected_code) in cases {
        let router = build_router();
        let (status, body) = call(&router, "PutMetricStream", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected HTTP {expected_status} {expected_code}, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn get_metric_stream_reports_a_stream_that_was_never_configured_as_missing() {
    let router = build_router();
    ok(
        &router,
        "PutMetricStream",
        &put_request("configured-stream", "json"),
    )
    .await;

    for name in ["other-stream", "CONFIGURED-STREAM", "configured-stream-2"] {
        let (status, body) = call(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {name:?}: expected HTTP 404 ResourceNotFoundException, got {status} {body}"
        );
        // The refusal names the resource through the two members that carry it and states no
        // reason at all: nothing of the request is echoed back in a sentence, and the member
        // that would carry one is absent rather than present and empty.
        assert_eq!(
            (
                text(&body, "ResourceType"),
                text(&body, "ResourceId"),
                text(&body, "Message")
            ),
            (
                Some("MetricStream".to_string()),
                Some(name.to_string()),
                None
            ),
            "case {name:?}: expected the refusal to name the missing stream and carry no \
             message member, got {body}"
        );
    }
}

#[tokio::test]
async fn delete_metric_stream_removes_exactly_the_stream_it_names() {
    // "Permanently deletes the metric stream that you specify." The rows differ along what
    // the delete is asked for: a stream that exists (in each output format a stream can
    // carry, since the delete is by name whatever the stream was configured with), one that
    // never existed, and the same name a second time — the operation models no not-found
    // error, so asking for a state the service is already in succeeds.
    let router = build_router();
    let configured = ["json", "opentelemetry0.7", "opentelemetry1.0"];
    for (index, format) in configured.iter().enumerate() {
        ok(
            &router,
            "PutMetricStream",
            &put_request(&format!("deletable-{index}"), format),
        )
        .await;
    }

    let cases: Vec<(&str, String)> = vec![
        ("a stream configured for json", "deletable-0".to_string()),
        (
            "a stream configured for opentelemetry0.7",
            "deletable-1".to_string(),
        ),
        (
            "the same stream a second time",
            "deletable-0".to_string(),
        ),
        ("a stream that never existed", "no-such-stream".to_string()),
    ];

    for (case, name) in cases {
        let (status, body) = call(&router, "DeleteMetricStream", &json!({"Name": &name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteMetricStream, got {status} {body}"
        );
        let (get_status, get_body) = call(&router, "GetMetricStream", &json!({"Name": &name})).await;
        assert_eq!(
            (get_status, error_code(&get_body).as_str()),
            (404, "ResourceNotFoundException"),
            "case {case:?}: the deleted stream must be gone, got {get_status} {get_body}"
        );
    }

    // Only the streams the deletes named are gone: the third one is untouched.
    let survivor = ok(
        &router,
        "GetMetricStream",
        &json!({"Name": "deletable-2"}),
    )
    .await;
    assert_eq!(
        text(&survivor, "OutputFormat").as_deref(),
        Some("opentelemetry1.0"),
        "case \"an unnamed stream survives\": expected the untouched stream to be reported as \
         configured, got {survivor}"
    );
}
