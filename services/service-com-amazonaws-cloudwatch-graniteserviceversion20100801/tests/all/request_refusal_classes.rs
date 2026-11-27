// SPDX-License-Identifier: CC0-1.0

//! The CLASS and the TEXT this service refuses a request with, across the operations whose
//! refusals belong to one rule each.
//!
//! Six rules, each asserted over a table whose rows differ along what the rule generalises
//! over rather than repeating one request:
//!
//! * **a required member the request did not supply AT ALL is refused by the
//!   request-validation layer in front of the service** — the class every operation of this
//!   service answers for the SHAPE of a request — while a member the request DID carry with no
//!   characters in it, and a member one of the Contributor Insights rule operations was not
//!   given, are values the service read and are answered under its own parameter faults;
//! * **the Contributor Insights REPORT read states a required member it was not given in its
//!   own coded sentence**, naming that member and the contributor query the read is served by;
//! * **a paginated listing answers a `NextToken` it cannot read a cursor out of in ITS OWN
//!   terms**: the alarm HISTORY refuses it naming the value at fault, the Contributor Insights
//!   listings refuse it with their coded sentence, and the alarm listing refuses it not at all
//!   — such a value belongs to a walk it cannot place, so the empty END of that walk is served;
//! * **`SetAlarmState` refuses a `StateReasonData` that is not a JSON document with
//!   `InvalidFormat` and leaves the alarm's state untouched**, whatever the value is wrong
//!   about;
//! * **`TagResource` refuses a tag key outside the character set tags are restricted to as a
//!   bad parameter VALUE, and a key under the prefix AWS reserves for its own as an action the
//!   caller may not take**, storing no tag either way;
//! * **the dashboard operations refuse a field that breaks its own rule naming the field and
//!   the rule it broke**, and `PutDashboard` STORES a body whose widget kind it does not
//!   render while reporting that widget back as a validation message;
//! * **a COLLECTION longer than the maximum its member declares is refused in the
//!   request-validation class, quoting the collection the request carried**, on every operation
//!   whose model declares none of this service's parameter faults — while a collection AT that
//!   maximum is served and read back, and the refused write stores nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{build_router, call, error_code, error_message, merged, metric_alarm_request, ok};

/// A metric alarm a state force and a tag write can be made against.
async fn given_alarm(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Custom/Refusals",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
        }),
    )
    .await;
}

/// A Contributor Insights rule definition, as `PutInsightRule` takes it.
fn rule_definition() -> String {
    json!({
        "Schema": {"Name": "CloudWatchLogRule", "Version": 1},
        "LogGroupNames": ["API-Gateway-Access-Logs"],
        "LogFormat": "JSON",
        "Contribution": {"Keys": ["$.ip"]},
    })
    .to_string()
}

/// The instant the report window below is measured from: a whole minute past the epoch, so the
/// window is a whole number of periods wide.
const REPORT_ANCHOR_SECS: i64 = 1_767_225_600;

/// A `GetInsightRuleReport` request about `name` carrying every member the operation requires.
fn report_request(name: &str) -> Value {
    json!({
        "RuleName": name,
        "StartTime": REPORT_ANCHOR_SECS,
        "EndTime": REPORT_ANCHOR_SECS + 300,
        "Period": 60,
    })
}

#[tokio::test]
async fn a_member_the_request_never_supplied_is_the_request_validation_class() {
    let router = build_router();

    // The rows differ along the operation, the kind of member left out (a name, a list, a
    // number) and the depth it sits at: every one of them is a request whose SHAPE the layer
    // in front of the service judges, so every one carries that layer's class.
    let omitted: Vec<(&str, Value)> = vec![
        ("DeleteMetricStream", json!({})),
        ("StartMetricStreams", json!({})),
        ("StopMetricStreams", json!({})),
        ("GetDashboard", json!({})),
        ("DeleteDashboards", json!({})),
        (
            "PutLogAlarm",
            json!({
                "AlarmName": "no-threshold",
                "QueryResultsToEvaluate": 3,
                "QueryResultsToAlarm": 2,
                "ComparisonOperator": "GreaterThanThreshold",
                "ScheduledQueryConfiguration": {
                    "QueryString": "fields @message",
                    "ScheduledQueryRoleARN":
                        "arn:aws:iam::000000000000:role/cwl-scheduled-query",
                    "AggregationExpression": "count(*)",
                    "ScheduleConfiguration": {
                        "ScheduleExpression": "rate(5 minutes)",
                        "StartTimeOffset": 360,
                    },
                },
            }),
        ),
        (
            "PutCompositeAlarm",
            json!({"AlarmName": "no-rule"}),
        ),
    ];

    for (operation, request) in omitted {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {operation:?}: a member the request never supplied is refused by the \
             request-validation layer, got {status} with {body}"
        );
    }

    // The two OTHER classes, so the rule above is about a member the request left out and not
    // about every refusal: a Contributor Insights rule operation states the member it was not
    // given in its own coded form under the missing-parameter fault, and a filter the request
    // DID carry with no characters is a value the service read and could not use.
    let (status, body) = call(&router, "ListManagedInsightRules", &json!({})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "MissingRequiredParameterException"),
        "a Contributor Insights listing names the member it was not given, got {status} with \
         {body}"
    );
    let message = error_message(&body);
    assert_eq!(
        message, "MISSING_RESOURCE_ARN: The ResourceARN parameter must be present.",
        "the coded form names the member, got {message:?}"
    );

    let (status, body) = call(
        &router,
        "ListMetrics",
        &json!({"Dimensions": [{"Name": "Probe", "Value": ""}]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValueException"),
        "a filter supplied with no characters is an unusable value, got {status} with {body}"
    );
}

#[tokio::test]
async fn the_contributor_insights_report_names_the_member_it_lacks_and_the_request_it_serves() {
    let router = build_router();
    ok(
        &router,
        "PutInsightRule",
        &json!({"RuleName": "report-rule", "RuleDefinition": rule_definition()}),
    )
    .await;

    // Source for the members: the required input members of the report read
    // (`smithy_query(query="operation",operation="GetInsightRuleReport")` — `RuleName`,
    // `StartTime`, `EndTime` and `Period`). Rows differ along WHICH of them the request leaves
    // out — a name, two instants and a granularity — because the member is what the sentence is
    // built from, and the recorded case is one row among them.
    let members = [
        ("the granularity the report is asked at", "Period", "PERIOD"),
        ("the rule the report is about", "RuleName", "RULE_NAME"),
        ("the start of the window", "StartTime", "START_TIME"),
        ("the end of the window", "EndTime", "END_TIME"),
    ];

    for (case, member, code) in members {
        let mut request = report_request("report-rule");
        request
            .as_object_mut()
            .expect("the request is an object")
            .remove(member);
        let (status, body) = call(&router, "GetInsightRuleReport", &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "MissingRequiredParameterException"),
            "case {case:?}: expected the missing-parameter fault, got {status} with {body}"
        );
        let message = error_message(&body);
        let expected = format!(
            "MISSING_{code}: The {member} parameter must be present in a GetContributors request."
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // The other side of the same rule: with every required member supplied the read answers the
    // rule's own contribution keys rather than a refusal.
    let report = ok(
        &router,
        "GetInsightRuleReport",
        &report_request("report-rule"),
    )
    .await;
    assert_eq!(
        report.get("KeyLabels"),
        Some(&json!(["$.ip"])),
        "a request carrying every required member is served, got {report}"
    );
}

#[tokio::test]
async fn a_next_token_a_listing_cannot_read_is_answered_in_that_listings_own_terms() {
    let router = build_router();
    given_alarm(&router, "paged-alarm").await;
    ok(
        &router,
        "PutInsightRule",
        &json!({"RuleName": "paged-rule", "RuleDefinition": rule_definition()}),
    )
    .await;

    // The rows differ along WHAT is wrong with the token: a value no opaque token could be, a
    // value that reads as a blob but carries no cursor this service minted, and one carrying
    // text rather than a cursor. None of them is a page of any enumeration.
    let tokens = [
        ("a value no token could be", "not-a-real-token"),
        ("a blob this service did not mint", "c2ltMnJlYWw="),
        ("a blob of some other service's cursor", "sim2real-behav-not-a-real-token=="),
    ];

    for (case, token) in tokens {
        // The alarm HISTORY names the value at fault, so a caller can tell which of the tokens
        // it holds was refused.
        let (status, body) = call(
            &router,
            "DescribeAlarmHistory",
            &json!({"NextToken": token}),
        )
        .await;
        let message = error_message(&body);
        let expected = format!("Invalid NextToken : {token}");
        assert_eq!(
            (status, error_code(&body).as_str(), message.as_str()),
            (400, "InvalidNextToken", expected.as_str()),
            "case {case:?}: expected the history's own refusal {expected:?}, got {status} with \
             {body}"
        );

        // The Contributor Insights listings answer their coded sentence, with nothing of the
        // token in it.
        for listing in ["DescribeInsightRules", "ListManagedInsightRules"] {
            let request = if listing == "ListManagedInsightRules" {
                json!({"ResourceARN": "arn:aws:logs:us-east-1:000000000000:log-group:/app",
                       "NextToken": token})
            } else {
                json!({"NextToken": token})
            };
            let (status, body) = call(&router, listing, &request).await;
            let message = error_message(&body);
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "InvalidNextToken"),
                "case {case:?} on {listing}: expected the token refused, got {status} with \
                 {body}"
            );
            assert!(
                message.starts_with("INVALID_TOKEN:") && !message.contains(token),
                "case {case:?} on {listing}: expected the coded sentence with nothing of the \
                 token in it, got {message:?}"
            );
        }

        // The alarm LISTING refuses no token: a value it cannot read a cursor out of names no
        // position of its enumeration, so it is IGNORED and the first page is served
        // (`tests/all/paginated_read_cursor.rs` holds that rule over every listing of this
        // service that serves such a token).
        let listed = ok(&router, "DescribeAlarms", &json!({"NextToken": token})).await;
        let named: Vec<String> = listed["MetricAlarms"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            named,
            vec!["paged-alarm".to_string()],
            "case {case:?}: a token the alarm listing cannot read must be ignored and the first \
             page served, got {listed}"
        );
        assert_eq!(
            listed.get("NextToken"),
            None,
            "case {case:?}: a page holding every alarm owes no token, got {listed}"
        );
        // And the alarm itself is still there to be read without a token.
        let whole = ok(&router, "DescribeAlarms", &json!({})).await;
        let held: Vec<String> = whole["MetricAlarms"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|alarm| alarm.get("AlarmName").and_then(Value::as_str))
            .map(ToString::to_string)
            .collect();
        assert_eq!(
            held,
            vec!["paged-alarm".to_string()],
            "case {case:?}: the listing itself must survive the reading, got {whole}"
        );
    }
}

#[tokio::test]
async fn a_state_reason_data_that_is_no_json_document_is_refused_and_changes_nothing() {
    // The rows differ along what is wrong with the value: text that is no JSON at all, a
    // document that stops before it closes, and each kind of bare JSON scalar — none of them a
    // document a consumer of the state-change notification could read fields out of.
    let malformed = [
        ("text that is no JSON", "not json at all"),
        ("a document that stops early", "{\"reason\": "),
        ("a truncated array", "[1, 2"),
        ("a bare number", "17"),
        ("a bare string", "\"breaching\""),
        ("a bare boolean", "true"),
    ];

    for (case, data) in malformed {
        let router = build_router();
        given_alarm(&router, "forced-alarm").await;

        let (status, body) = call(
            &router,
            "SetAlarmState",
            &json!({
                "AlarmName": "forced-alarm",
                "StateValue": "ALARM",
                "StateReason": "testing",
                "StateReasonData": data,
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidFormatFault"),
            "case {case:?}: expected the modelled format refusal, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            "",
            "case {case:?}: CloudWatch answers this refusal with its code alone, got {body}"
        );

        // The refusal left the alarm's state where the write found it.
        let described = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNames": ["forced-alarm"]}),
        )
        .await;
        let alarm = &described["MetricAlarms"][0];
        assert_eq!(
            alarm.get("StateValue").and_then(Value::as_str),
            Some("INSUFFICIENT_DATA"),
            "case {case:?}: a refused force must not move the alarm, got {described}"
        );
        assert_eq!(
            alarm.get("StateReasonData"),
            None,
            "case {case:?}: a refused force must store no reason data, got {described}"
        );
    }

    // The other side of the same rule: a document the member may carry is stored and read back,
    // whichever JSON document it is.
    for (case, data) in [
        ("a reason object", json!({"version": "1.0", "threshold": 1.0}).to_string()),
        ("a reason array", json!([1, 2, 3]).to_string()),
    ] {
        let router = build_router();
        given_alarm(&router, "forced-alarm").await;
        ok(
            &router,
            "SetAlarmState",
            &json!({
                "AlarmName": "forced-alarm",
                "StateValue": "ALARM",
                "StateReason": "testing",
                "StateReasonData": data,
            }),
        )
        .await;
        let described = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNames": ["forced-alarm"]}),
        )
        .await;
        assert_eq!(
            described["MetricAlarms"][0]
                .get("StateReasonData")
                .and_then(Value::as_str),
            Some(data.as_str()),
            "case {case:?}: a document the member may carry round-trips, got {described}"
        );
    }
}

#[tokio::test]
async fn a_tag_key_this_service_will_not_store_is_refused_in_its_own_class() {
    let router = build_router();
    given_alarm(&router, "tagged-alarm").await;
    let arn = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:tagged-alarm";

    // Rows differ along WHERE the character the set does not admit sits and which character it
    // is. Source for the set: the tag restrictions every AWS service's tags are subject to —
    // letters, numbers, spaces, and `+ - = . _ : / @`.
    let unusable_keys = [
        ("a hash inside the key", "bad#key"),
        ("an exclamation mark closing the key", "key!"),
        ("a bracket opening the key", "(key)"),
        ("a comma between words", "team,owner"),
    ];
    for (case, key) in unusable_keys {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"ResourceARN": arn, "Tags": [{"Key": key, "Value": "v"}]}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: a key outside the tag character set is a bad parameter value, got \
             {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            "",
            "case {case:?}: CloudWatch answers this refusal with its code alone, got {body}"
        );
    }

    // The reserved prefix is a different class: the caller may not write a tag under it, in any
    // case of it, and wherever in the batch that tag sits.
    let reserved_keys = [
        ("the reserved prefix", "aws:cloudformation:stack-name"),
        ("the reserved prefix in upper case", "AWS:billing"),
        ("the reserved prefix in mixed case", "Aws:owner"),
    ];
    for (case, key) in reserved_keys {
        let (status, body) = call(
            &router,
            "TagResource",
            &json!({"ResourceARN": arn, "Tags": [
                {"Key": "team", "Value": "platform"},
                {"Key": key, "Value": "v"},
            ]}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (403, "AccessDenied"),
            "case {case:?}: a tag under the reserved prefix is refused as an action the caller \
             may not take, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            "The provided tag keys must not start with any case of \"aws:\"",
            "case {case:?}: expected CloudWatch's wording for the reserved prefix, got {body}"
        );
    }

    // Neither refusal stored anything, including the tag of the same request the rules admit.
    let listed = ok(&router, "ListTagsForResource", &json!({"ResourceARN": arn})).await;
    assert_eq!(
        listed["Tags"],
        json!([]),
        "a refused TagResource must store no tag, got {listed}"
    );

    // The other side of the same rule: every character the set admits is stored and read back.
    let usable = "env.name_1-2:3/4@5+6=7 8";
    ok(
        &router,
        "TagResource",
        &json!({"ResourceARN": arn, "Tags": [{"Key": usable, "Value": "v"}]}),
    )
    .await;
    let listed = ok(&router, "ListTagsForResource", &json!({"ResourceARN": arn})).await;
    assert_eq!(
        listed["Tags"],
        json!([{"Key": usable, "Value": "v"}]),
        "a key built from the characters tags admit is stored, got {listed}"
    );
}

#[tokio::test]
async fn a_dashboard_field_outside_its_rule_names_the_field_and_the_rule() {
    let router = build_router();
    ok(
        &router,
        "PutDashboard",
        &json!({"DashboardName": "kept", "DashboardBody": "{}"}),
    )
    .await;

    // Rows differ along the field, the way its value breaks the rule (too many elements, none
    // at all, a character the set does not admit) and the operation that carries it.
    let names: Vec<String> = (0..101).map(|index| format!("d{index}")).collect();
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a batch past the element cap",
            "DeleteDashboards",
            json!({"DashboardNames": names}),
            "Field DashboardNames can contain up to 100 elements.\n".to_string(),
        ),
        (
            "a batch naming nothing",
            "DeleteDashboards",
            json!({"DashboardNames": []}),
            "Field DashboardNames is required and has to contain at least 1 element.\n"
                .to_string(),
        ),
        (
            "a prefix holding a character no name may carry",
            "ListDashboards",
            json!({"DashboardNamePrefix": "team/"}),
            "The value for field DashboardNamePrefix contains invalid characters. It can only \
             contain alphanumerics, dash (-) and underscore (_).\n"
                .to_string(),
        ),
        (
            "a prefix holding the period a name may not carry",
            "ListDashboards",
            json!({"DashboardNamePrefix": "staging."}),
            "The value for field DashboardNamePrefix contains invalid characters. It can only \
             contain alphanumerics, dash (-) and underscore (_).\n"
                .to_string(),
        ),
        (
            "a name holding a character the set does not admit",
            "GetDashboard",
            json!({"DashboardName": "my dashboard"}),
            "The value for field DashboardName contains invalid characters. It can only \
             contain alphanumerics, dash (-) and underscore (_).\n"
                .to_string(),
        ),
        (
            "a name breaking the character set and the length at once",
            "GetDashboard",
            json!({"DashboardName": "my dashboard".repeat(40)}),
            "The value for field DashboardName contains invalid characters. It can only \
             contain alphanumerics, dash (-) and underscore (_).\n"
                .to_string(),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected the parameter-value fault, got {status} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }

    // A refused batch deleted nothing.
    let listed = ok(&router, "ListDashboards", &json!({})).await;
    assert_eq!(
        listed["DashboardEntries"][0]["DashboardName"],
        json!("kept"),
        "a refused DeleteDashboards must delete nothing, got {listed}"
    );
}

#[tokio::test]
async fn a_widget_kind_this_service_does_not_render_is_reported_and_the_body_is_stored() {
    let router = build_router();

    // Rows differ along the kind the widget declares and where in the document it sits: each
    // one satisfies the widget schema, so the body is STORED and the widget whose kind the
    // body structure does not define comes back as a validation message naming that widget's
    // own `type`.
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "one widget of an unknown kind",
            json!({"widgets": [{"type": "donut", "properties": {"metrics": []}}]}),
            vec!["/widgets/0/type"],
        ),
        (
            "an unknown kind after a widget the schema renders",
            json!({"widgets": [
                {"type": "text", "properties": {"markdown": "hello"}},
                {"type": "sparkline", "properties": {}},
            ]}),
            vec!["/widgets/1/type"],
        ),
        (
            "two widgets of unknown kinds",
            json!({"widgets": [
                {"type": "donut", "properties": {}},
                {"type": "trace-map", "properties": {}},
            ]}),
            vec!["/widgets/0/type", "/widgets/1/type"],
        ),
        (
            "every widget of a kind the body structure defines",
            json!({"widgets": [
                {"type": "text", "properties": {"markdown": "hello"}},
                {"type": "explorer", "properties": {"metrics": []}},
            ]}),
            vec![],
        ),
    ];

    for (index, (case, document, paths)) in rows.into_iter().enumerate() {
        let name = format!("widget-kind-{index}");
        let body = document.to_string();
        let answered = ok(
            &router,
            "PutDashboard",
            &json!({"DashboardName": name, "DashboardBody": body}),
        )
        .await;
        let reported: Vec<&str> = answered["DashboardValidationMessages"]
            .as_array()
            .map(|messages| {
                messages
                    .iter()
                    .filter_map(|message| message.get("DataPath").and_then(Value::as_str))
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            reported, paths,
            "case {case:?}: expected a validation message per widget kind this service does \
             not render, got {answered}"
        );
        // Whatever it warned about, the document was STORED: the dashboard reads back byte for
        // byte.
        let got = ok(&router, "GetDashboard", &json!({"DashboardName": name})).await;
        assert_eq!(
            got.get("DashboardBody").and_then(Value::as_str),
            Some(body.as_str()),
            "case {case:?}: a warned body must still be stored, got {got}"
        );
    }
}

/// The modelled maximum of a collection, and the member path a refusal names it at, as the
/// model declares them
/// (`smithy_query(query="operation",operation="PutAlarmMuteRule,PutMetricAlarm,DeleteAlarms,DescribeAlarms,DisableAlarmActions,DescribeAlarmsForMetric")`):
/// `MuteTargets$AlarmNames` and every `AlarmNames` list hold up to 100, a `ResourceList` of
/// alarm actions up to 5, and a metric's `Dimensions` up to 30.
const ALARM_NAMES_MAX: usize = 100;
const ACTIONS_MAX: usize = 5;
const DIMENSIONS_MAX: usize = 30;

/// The clause AWS states for a collection longer than its modelled maximum: the collection the
/// request carried, the member path it arrived on, and that maximum.
fn over_the_maximum(submitted: &str, member: &str, maximum: usize) -> String {
    format!(
        "1 validation error detected: Value '{submitted}' at '{member}' failed to satisfy \
         constraint: Member must have length less than or equal to {maximum}"
    )
}

/// `count` alarm names, as a caller that targets or lists that many sends them.
fn alarm_names(count: usize) -> Vec<String> {
    (0..count).map(|index| format!("alarm-{index}")).collect()
}

/// `count` action ARNs, as a caller that attaches that many actions sends them.
fn actions(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("arn:aws:sns:us-east-1:000000000000:topic-{index}"))
        .collect()
}

/// `count` dimensions, as a caller that names that many sends them.
fn dimensions(count: usize) -> Vec<Value> {
    (0..count)
        .map(|index| json!({"Name": format!("D{index}"), "Value": "v"}))
        .collect()
}

/// How a list of strings is quoted inside a clause: the elements comma-and-space joined inside
/// square brackets.
fn quoted(elements: &[String]) -> String {
    format!("[{}]", elements.join(", "))
}

#[tokio::test]
async fn a_collection_over_its_modelled_maximum_is_the_request_validation_class() {
    let router = build_router();

    // The rows differ along the operation, which member carries the collection, the DEPTH it
    // sits at (a member of the request, and a member of a structure inside it), the modelled
    // maximum it broke, what its elements are (a name, an ARN, a structure) and whether the
    // operation reads or writes — every one of them an operation whose model declares none of
    // this service's parameter faults, so every one carries the request-validation class and
    // the clause AWS states, quoting the collection the request carried.
    let names = alarm_names(ALARM_NAMES_MAX + 1);
    let too_many_actions = actions(ACTIONS_MAX + 1);
    let too_many_dimensions = dimensions(DIMENSIONS_MAX + 1);
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "the alarm names a mute rule targets, one past the hundred it may name",
            "PutAlarmMuteRule",
            json!({
                "Name": "over-targeted",
                "Rule": {"Schedule": {"Expression": "cron(0 2 * * *)", "Duration": "PT1H"}},
                "MuteTargets": {"AlarmNames": names},
            }),
            over_the_maximum(&quoted(&names), "muteTargets.alarmNames", ALARM_NAMES_MAX),
        ),
        (
            "the actions a metric alarm takes, one past the five it may take",
            "PutMetricAlarm",
            merged(
                metric_alarm_request("over-actioned"),
                &json!({"AlarmActions": too_many_actions}),
            ),
            over_the_maximum(&quoted(&too_many_actions), "alarmActions", ACTIONS_MAX),
        ),
        (
            "the actions a composite alarm takes while it has no data",
            "PutCompositeAlarm",
            json!({
                "AlarmName": "over-actioned-composite",
                "AlarmRule": "ALARM(child)",
                "InsufficientDataActions": too_many_actions,
            }),
            over_the_maximum(
                &quoted(&too_many_actions),
                "insufficientDataActions",
                ACTIONS_MAX,
            ),
        ),
        (
            "the alarm names a delete lists",
            "DeleteAlarms",
            json!({"AlarmNames": names}),
            over_the_maximum(&quoted(&names), "alarmNames", ALARM_NAMES_MAX),
        ),
        (
            "the alarm names an action switch lists",
            "DisableAlarmActions",
            json!({"AlarmNames": names}),
            over_the_maximum(&quoted(&names), "alarmNames", ALARM_NAMES_MAX),
        ),
        (
            "the alarm names the other action switch lists",
            "EnableAlarmActions",
            json!({"AlarmNames": names}),
            over_the_maximum(&quoted(&names), "alarmNames", ALARM_NAMES_MAX),
        ),
        (
            "the alarm names a listing is narrowed to",
            "DescribeAlarms",
            json!({"AlarmNames": names}),
            over_the_maximum(&quoted(&names), "alarmNames", ALARM_NAMES_MAX),
        ),
        (
            "the dimensions a metric's alarms are looked up by, as structures",
            "DescribeAlarmsForMetric",
            json!({
                "Namespace": "Custom/Refusals",
                "MetricName": "Latency",
                "Dimensions": too_many_dimensions,
            }),
            over_the_maximum(
                &format!(
                    "[{}]",
                    (0..=DIMENSIONS_MAX)
                        .map(|index| format!("Dimension(name=D{index}, value=v)"))
                        .collect::<Vec<String>>()
                        .join(", ")
                ),
                "dimensions",
                DIMENSIONS_MAX,
            ),
        ),
    ];

    for (case, operation, request, expected) in rows {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 in the request-validation class, got {status} \
             with {body}"
        );
        let got = error_message(&body);
        assert_eq!(
            got, expected,
            "case {case:?}: expected {expected:?}, got {got:?}"
        );
    }
}

#[tokio::test]
async fn a_collection_at_its_modelled_maximum_is_served_and_a_refused_one_writes_nothing() {
    let router = build_router();
    let targets = alarm_names(ALARM_NAMES_MAX);

    // A collection AT its maximum is the request the refusal above is about the entry past: it
    // is accepted and READ BACK, for the two kinds of collection the rows differ along — the
    // hundred alarm names a mute rule may target and the five actions an alarm may take.
    ok(
        &router,
        "PutAlarmMuteRule",
        &json!({
            "Name": "fully-targeted",
            "Rule": {"Schedule": {"Expression": "cron(0 2 * * *)", "Duration": "PT1H"}},
            "MuteTargets": {"AlarmNames": targets},
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetAlarmMuteRule",
        &json!({"AlarmMuteRuleName": "fully-targeted"}),
    )
    .await;
    assert_eq!(
        read["MuteTargets"]["AlarmNames"].as_array().map(Vec::len),
        Some(ALARM_NAMES_MAX),
        "case \"a mute rule at its maximum\": expected the {ALARM_NAMES_MAX} stored targets, \
         got {read}"
    );
    let filled = actions(ACTIONS_MAX);
    ok(
        &router,
        "PutMetricAlarm",
        &merged(
            metric_alarm_request("fully-actioned"),
            &json!({"AlarmActions": filled}),
        ),
    )
    .await;

    // A refused write stores nothing: the name the refused create carried holds no rule.
    let (status, body) = call(
        &router,
        "PutAlarmMuteRule",
        &json!({
            "Name": "never-created",
            "Rule": {"Schedule": {"Expression": "cron(0 2 * * *)", "Duration": "PT1H"}},
            "MuteTargets": {"AlarmNames": alarm_names(ALARM_NAMES_MAX + 1)},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a refused create\": expected HTTP 400, got {status} with {body}"
    );
    let (status, body) = call(
        &router,
        "GetAlarmMuteRule",
        &json!({"AlarmMuteRuleName": "never-created"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a refused create\": a refused write must store nothing, got {status} with {body}"
    );

    // A refused UPDATE leaves the stored rule exactly as the accepted write left it, and a
    // further update to a different hundred targets REPLACES them.
    let (status, body) = call(
        &router,
        "PutAlarmMuteRule",
        &json!({
            "Name": "fully-targeted",
            "Rule": {"Schedule": {"Expression": "cron(0 3 * * *)", "Duration": "PT2H"}},
            "MuteTargets": {"AlarmNames": alarm_names(ALARM_NAMES_MAX + 1)},
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"a refused update\": expected HTTP 400, got {status} with {body}"
    );
    let read = ok(
        &router,
        "GetAlarmMuteRule",
        &json!({"AlarmMuteRuleName": "fully-targeted"}),
    )
    .await;
    assert_eq!(
        (
            read["MuteTargets"]["AlarmNames"].as_array().map(Vec::len),
            read["Rule"]["Schedule"]["Duration"].as_str(),
        ),
        (Some(ALARM_NAMES_MAX), Some("PT1H")),
        "case \"a refused update\": the stored targets and duration must be untouched, got {read}"
    );
    let replacements: Vec<String> = (0..ALARM_NAMES_MAX)
        .map(|index| format!("other-alarm-{index}"))
        .collect();
    ok(
        &router,
        "PutAlarmMuteRule",
        &json!({
            "Name": "fully-targeted",
            "Rule": {"Schedule": {"Expression": "cron(0 2 * * *)", "Duration": "PT1H"}},
            "MuteTargets": {"AlarmNames": replacements},
        }),
    )
    .await;
    let read = ok(
        &router,
        "GetAlarmMuteRule",
        &json!({"AlarmMuteRuleName": "fully-targeted"}),
    )
    .await;
    assert_eq!(
        read["MuteTargets"]["AlarmNames"].as_array().and_then(|targets| targets
            .first()
            .and_then(Value::as_str)),
        Some("other-alarm-0"),
        "case \"an accepted update\": expected the new targets stored, got {read}"
    );
}
