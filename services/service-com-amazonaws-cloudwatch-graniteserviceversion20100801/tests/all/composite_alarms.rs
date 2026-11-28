// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the composite-alarm lifecycle: `PutCompositeAlarm` writing an alarm
//! whose state is EVALUATED from its `AlarmRule` over the alarms the rule references, and
//! `DescribeAlarms` / `DescribeAlarmHistory` / `SetAlarmState` / `DeleteAlarms` reading and
//! moving it.
//!
//! The rule under test is: a composite alarm's state is its `AlarmRule` applied to the
//! current states of the alarms the rule references — ALARM when the expression is true, OK
//! when it is false — so the table rows differ along what that rule generalises over: which
//! state function is used, which operators combine them, how the operators nest and bind,
//! whether a reference is quoted, unquoted or an ARN, and whether the alarm referenced exists
//! at all. The recorded happy path is one row among them.

use crate::harness::{build_router, call, error_code, error_message, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;
use std::time::Duration;

/// Long enough for a composite alarm's rule evaluation to come due, and for the first
/// evaluation of the metric alarms its rules read.
///
/// `PutCompositeAlarm` sets a new alarm to `INSUFFICIENT_DATA` and its rule is evaluated from
/// the states of the alarms that rule names, while a metric alarm's own first evaluation is
/// owed a span after the write that configured it
/// (`crate::state::alarm_evaluation::WRITE_EVALUATION_DELAY_SECS`), so a test that wants the
/// EVALUATED state waits past the top of that span.
const PAST_EVALUATION_DELAY: Duration = Duration::from_millis(3_500);

/// Wait for every composite-alarm evaluation started before now to be due.
///
/// A thread sleep rather than `tokio::time::sleep`: this workspace enables tokio's `macros`
/// and `rt` features only, so there is no timer driver to await on. Each test here drives one
/// router with no concurrent task, so blocking its own runtime thread holds nothing up.
fn wait_for_evaluation() {
    std::thread::sleep(PAST_EVALUATION_DELAY);
}

/// A metric alarm the composite alarms below can reference.
///
/// It treats missing data as `ignore` — "the current alarm state is maintained"
/// (`PutMetricAlarmInput$TreatMissingData`) — because every row here sets a CHILD's state with
/// `SetAlarmState` and then asserts what the parent's rule makes of it: a state a caller forces
/// is temporary ("Metric alarms returns to their actual state quickly", `SetAlarmState`), so
/// under any other setting the child's own next evaluation, which has no datapoint of this
/// series to grade, would take the state back before the rule is read. The composite rules
/// themselves are what these rows vary, not the children's missing-data arm.
async fn put_metric_alarm(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "MyApp",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "ignore",
        }),
    )
    .await;
}

/// Force one alarm's state, the way a caller sets up a child alarm for a rule.
async fn set_alarm_state(router: &MultiServiceRouter, name: &str, state: &str) {
    ok(
        router,
        "SetAlarmState",
        &json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": "driven by the test",
        }),
    )
    .await;
}

/// Create a composite alarm with `rule`, requiring a 200.
async fn put_composite_alarm(router: &MultiServiceRouter, name: &str, rule: &str) {
    ok(
        router,
        "PutCompositeAlarm",
        &json!({"AlarmName": name, "AlarmRule": rule}),
    )
    .await;
}

/// The one composite alarm `DescribeAlarms` reports for `name`.
async fn composite_alarm(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(
        router,
        "DescribeAlarms",
        &json!({"AlarmNames": [name], "AlarmTypes": ["CompositeAlarm"]}),
    )
    .await;
    let alarms = body
        .get("CompositeAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        alarms.len(),
        1,
        "DescribeAlarms: expected exactly the composite alarm {name}, got {body}"
    );
    alarms[0].clone()
}

/// A member as a string.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

#[tokio::test]
async fn the_composite_alarm_configuration_round_trips_through_describe_alarms() {
    let router = build_router();
    put_metric_alarm(&router, "latency-high").await;
    // The alarm this one suppresses its actions through has to exist when the composite alarm
    // is saved: suppression is read from that alarm's own state.
    put_metric_alarm(&router, "maintenance-window").await;

    let put = json!({
        "AlarmName": "service-unhealthy",
        "AlarmDescription": "the service is unhealthy",
        "AlarmRule": "ALARM(\"latency-high\")",
        "ActionsEnabled": false,
        "AlarmActions": ["arn:aws:lambda:us-east-1:000000000000:function:page-oncall"],
        "OKActions": ["arn:aws:sns:us-east-1:000000000000:recovered"],
        "InsufficientDataActions": ["arn:aws:sns:us-east-1:000000000000:unknown"],
        "ActionsSuppressor": "maintenance-window",
        "ActionsSuppressorWaitPeriod": 120,
        "ActionsSuppressorExtensionPeriod": 180,
        "Tags": [{"Key": "team", "Value": "platform"}],
    });
    ok(&router, "PutCompositeAlarm", &put).await;

    let alarm = composite_alarm(&router, "service-unhealthy").await;

    // Every configuration member the request carried comes back as it was given.
    for member in [
        "AlarmName",
        "AlarmDescription",
        "AlarmRule",
        "ActionsEnabled",
        "AlarmActions",
        "OKActions",
        "InsufficientDataActions",
        "ActionsSuppressor",
        "ActionsSuppressorWaitPeriod",
        "ActionsSuppressorExtensionPeriod",
    ] {
        assert_eq!(
            alarm.get(member),
            put.get(member),
            "member {member:?}: expected the stored value to be the one written, got {alarm}"
        );
    }
    assert_eq!(
        text(&alarm, "AlarmArn"),
        Some("arn:aws:cloudwatch:us-east-1:000000000000:alarm:service-unhealthy".to_string()),
        "AlarmArn must name this alarm in the region the request was signed for, got {alarm}"
    );
    // "When this operation creates an alarm, the alarm state is immediately set to
    // INSUFFICIENT_DATA." — read back straight away, before the evaluation is due.
    // A composite alarm's state is derived from its children's current states, so the first
    // read already reports the rule over them: the suppressor exists but is in no ALARM state
    // and the rule's child is in none either, so the rule is false and the alarm is in OK.
    assert_eq!(
        text(&alarm, "StateValue").as_deref(),
        Some("OK"),
        "a new composite alarm's first read reports the rule over its children, got {alarm}"
    );
    assert!(
        alarm.get("AlarmConfigurationUpdatedTimestamp").is_some()
            && alarm.get("StateUpdatedTimestamp").is_some(),
        "a stored composite alarm carries both of its time stamps, got {alarm}"
    );
    // The suppressor alarm does not exist and so is in no state: nothing is suppressed.
    assert_eq!(
        alarm.get("ActionsSuppressedBy"),
        None,
        "nothing suppresses the actions of an alarm whose suppressor is in no ALARM state: {alarm}"
    );

    // "If you omit this parameter, only metric alarms are returned, even if composite alarms
    // or log alarms exist in the account." (DescribeAlarmsInput$AlarmTypes)
    let defaulted = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["service-unhealthy", "latency-high"]}),
    )
    .await;
    assert_eq!(
        defaulted.get("CompositeAlarms"),
        Some(&json!([])),
        "a request naming no AlarmTypes reports no composite alarm, got {defaulted}"
    );
    assert_eq!(
        defaulted["MetricAlarms"]
            .as_array()
            .map(|alarms| alarms.len()),
        Some(1),
        "a request naming no AlarmTypes reports the metric alarm, got {defaulted}"
    );

    // An update overwrites the configuration and leaves the state alone.
    let state_before = text(&alarm, "StateValue");
    let mut replacement = put.clone();
    replacement["AlarmRule"] = json!("OK(\"latency-high\")");
    replacement["AlarmDescription"] = json!("rewritten");
    ok(&router, "PutCompositeAlarm", &replacement).await;
    let updated = composite_alarm(&router, "service-unhealthy").await;
    assert_eq!(
        text(&updated, "AlarmRule").as_deref(),
        Some("OK(\"latency-high\")"),
        "an update must overwrite the rule, got {updated}"
    );
    assert_eq!(
        text(&updated, "AlarmDescription").as_deref(),
        Some("rewritten"),
        "an update must overwrite the description, got {updated}"
    );
    assert_eq!(
        text(&updated, "StateValue"),
        state_before,
        "an update must leave the alarm state unchanged, got {updated}"
    );

    // The configuration writes are in the alarm's history, under the composite alarm type.
    let history = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": "service-unhealthy",
            "AlarmTypes": ["CompositeAlarm"],
            "HistoryItemType": "ConfigurationUpdate",
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let items = history
        .get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        items.len(),
        2,
        "the create and the update are both recorded, got {history}"
    );
    for (case, item) in ["Create", "Update"].iter().zip(items.iter()) {
        assert_eq!(
            text(item, "AlarmType").as_deref(),
            Some("CompositeAlarm"),
            "case {case:?}: a composite alarm's history item carries its own alarm type, got {item}"
        );
        let data: Value = serde_json::from_str(
            &text(item, "HistoryData").unwrap_or_else(|| "null".to_string()),
        )
        .unwrap_or(Value::Null);
        assert_eq!(
            text(&data, "type").as_deref(),
            Some(*case),
            "case {case:?}: HistoryData must record the kind of write, got {item}"
        );
    }

    // A deleted composite alarm is gone, and deleting it again is a clean no-op success.
    ok(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["service-unhealthy"]}),
    )
    .await;
    let gone = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmTypes": ["CompositeAlarm"]}),
    )
    .await;
    assert_eq!(
        gone.get("CompositeAlarms"),
        Some(&json!([])),
        "a deleted composite alarm is no longer reported, got {gone}"
    );
    ok(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["service-unhealthy"]}),
    )
    .await;
}

#[tokio::test]
async fn the_alarm_rule_decides_the_composite_alarm_state() {
    let router = build_router();
    // One fixture of children, each in a different one of the three modelled `StateValue`
    // values, so every row's expected outcome follows from the rule alone.
    for (child, state) in [
        ("trip", "ALARM"),
        ("calm", "OK"),
        ("quiet", "INSUFFICIENT_DATA"),
    ] {
        put_metric_alarm(&router, child).await;
        set_alarm_state(&router, child, state).await;
    }
    let trip_arn = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["trip"]}),
    )
    .await["MetricAlarms"][0]["AlarmArn"]
        .as_str()
        .expect("a stored metric alarm carries its ARN")
        .to_string();

    // Rows differ along what the rule generalises over: the state function, the operators,
    // their nesting and binding, and the form of the reference. The rule's outcome maps TRUE
    // to ALARM and FALSE to OK. Every row's rule names alarms that exist, because a rule
    // naming one that does not is refused outright
    // (`a_rule_the_service_cannot_evaluate_is_refused`).
    let cases: Vec<(&str, String, &str)> = vec![
        ("ALARM of a child in ALARM", "ALARM(\"trip\")".to_string(), "ALARM"),
        ("ALARM of a child in OK", "ALARM(\"calm\")".to_string(), "OK"),
        ("OK of a child in OK", "OK(\"calm\")".to_string(), "ALARM"),
        (
            "INSUFFICIENT_DATA of a child in INSUFFICIENT_DATA",
            "INSUFFICIENT_DATA(\"quiet\")".to_string(),
            "ALARM",
        ),
        ("an unquoted reference", "ALARM(trip)".to_string(), "ALARM"),
        ("an ARN reference", format!("ALARM(\"{trip_arn}\")"), "ALARM"),
        (
            "AND over a true and a false function",
            "ALARM(\"trip\") AND ALARM(\"calm\")".to_string(),
            "OK",
        ),
        (
            "OR over a true and a false function",
            "ALARM(\"trip\") OR ALARM(\"calm\")".to_string(),
            "ALARM",
        ),
        ("NOT over a false function", "NOT ALARM(\"calm\")".to_string(), "ALARM"),
        (
            "parentheses grouping an OR inside an AND",
            "(OK(\"calm\") OR ALARM(\"calm\")) AND ALARM(\"trip\")".to_string(),
            "ALARM",
        ),
        (
            "AND binds tighter than OR",
            "ALARM(\"calm\") OR ALARM(\"calm\") AND ALARM(\"trip\")".to_string(),
            "OK",
        ),
        ("the TRUE literal", "TRUE".to_string(), "ALARM"),
        ("the FALSE literal", "FALSE".to_string(), "OK"),
    ];

    for (index, (case, rule, expected)) in cases.iter().enumerate() {
        put_composite_alarm(&router, &format!("composite-{index}"), rule).await;
        let alarm = composite_alarm(&router, &format!("composite-{index}")).await;
        // A composite alarm's state is DERIVED from its children's current states, so the
        // first read of a newly created one already reports the rule applied to them: "the
        // alarm is then evaluated and its state is set appropriately" (`PutCompositeAlarm`).
        assert_eq!(
            text(&alarm, "StateValue").as_deref(),
            Some(*expected),
            "case {case:?} (rule {rule:?}): a composite alarm's first read reports the rule over \
             its children, got {alarm}"
        );
    }

    wait_for_evaluation();

    for (index, (case, rule, expected)) in cases.iter().enumerate() {
        let name = format!("composite-{index}");
        let alarm = composite_alarm(&router, &name).await;
        assert_eq!(
            text(&alarm, "StateValue").as_deref(),
            Some(*expected),
            "case {case:?} (rule {rule:?}): expected StateValue {expected}, got {alarm}"
        );
        assert!(
            text(&alarm, "StateReason").is_some_and(|reason| !reason.is_empty()),
            "case {case:?}: an evaluated state carries the reason it was reached, got {alarm}"
        );
        // A read reports the evaluated state; it does not keep moving it.
        let again = composite_alarm(&router, &name).await;
        assert_eq!(
            again.get("StateValue"),
            alarm.get("StateValue"),
            "case {case:?}: reading twice must report the same state, got {again}"
        );
        assert_eq!(
            again.get("StateUpdatedTimestamp"),
            alarm.get("StateUpdatedTimestamp"),
            "case {case:?}: a read must not restamp the state it reports, got {again}"
        );
    }

    // The evaluated states are filterable and recorded, exactly like a metric alarm's.
    let in_alarm = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmTypes": ["CompositeAlarm"], "StateValue": "ALARM", "MaxRecords": 100}),
    )
    .await;
    let reported: Vec<String> = in_alarm["CompositeAlarms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|alarm| text(alarm, "AlarmName"))
        .collect();
    let expected: Vec<String> = cases
        .iter()
        .enumerate()
        .filter(|(_, (_, _, state))| *state == "ALARM")
        .map(|(index, _)| format!("composite-{index}"))
        .collect();
    assert_eq!(
        reported.len(),
        expected.len(),
        "StateValue=ALARM must report exactly the alarms whose rule is true, expected \
         {expected:?}, got {reported:?}"
    );
    for name in &expected {
        assert!(
            reported.contains(name),
            "StateValue=ALARM must report {name}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn a_composite_alarm_rule_may_reference_another_composite_alarm() {
    let router = build_router();
    put_metric_alarm(&router, "leaf").await;
    set_alarm_state(&router, "leaf", "ALARM").await;
    put_composite_alarm(&router, "inner", "ALARM(\"leaf\")").await;
    put_composite_alarm(&router, "outer", "ALARM(\"inner\")").await;

    wait_for_evaluation();

    for name in ["inner", "outer"] {
        let alarm = composite_alarm(&router, name).await;
        assert_eq!(
            text(&alarm, "StateValue").as_deref(),
            Some("ALARM"),
            "case {name:?}: the rule is true, so the composite alarm is in ALARM, got {alarm}"
        );
    }

    // A child's new state reaches the whole chain above it.
    set_alarm_state(&router, "leaf", "OK").await;
    for name in ["inner", "outer"] {
        let alarm = composite_alarm(&router, name).await;
        assert_eq!(
            text(&alarm, "StateValue").as_deref(),
            Some("OK"),
            "case {name:?}: the child left ALARM, so the chain above it evaluates to OK, got \
             {alarm}"
        );
    }

    // "These are the metric alarms and composite alarms referenced in the AlarmRule field of
    // the composite alarm that you specify in ChildrenOfAlarmName. … Only the Alarm Name,
    // ARN, StateValue … and StateUpdatedTimestamp information are returned."
    let children = ok(
        &router,
        "DescribeAlarms",
        &json!({"ChildrenOfAlarmName": "outer"}),
    )
    .await;
    let child = &children["CompositeAlarms"][0];
    assert_eq!(
        text(child, "AlarmName").as_deref(),
        Some("inner"),
        "the children of outer are the alarms its rule references, got {children}"
    );
    assert!(
        child.get("AlarmArn").is_some()
            && child.get("StateValue").is_some()
            && child.get("StateUpdatedTimestamp").is_some(),
        "a child report carries the name, ARN, state and state time stamp, got {children}"
    );
    assert_eq!(
        child.get("AlarmRule"),
        None,
        "a child report carries no configuration beyond the four members, got {children}"
    );
    assert_eq!(
        children["MetricAlarms"],
        json!([]),
        "outer's rule references no metric alarm, got {children}"
    );

    // "These are the composite alarms that have AlarmRule parameters that reference the alarm
    // named in ParentsOfAlarmName. … Only the Alarm Name and ARN are returned."
    let parents = ok(
        &router,
        "DescribeAlarms",
        &json!({"ParentsOfAlarmName": "leaf"}),
    )
    .await;
    let parent = &parents["CompositeAlarms"][0];
    assert_eq!(
        text(parent, "AlarmName").as_deref(),
        Some("inner"),
        "the parents of leaf are the composite alarms whose rule references it, got {parents}"
    );
    assert!(
        parent.get("AlarmArn").is_some(),
        "a parent report carries the ARN, got {parents}"
    );
    assert_eq!(
        parent.get("StateValue"),
        None,
        "a parent report carries only the name and the ARN, got {parents}"
    );

    // "you can't delete any composite alarm that is part of [a cycle] because there is always
    // still a composite alarm that depends on that alarm that you want to delete"
    let (status, body) = call(&router, "DeleteAlarms", &json!({"AlarmNames": ["inner"]})).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "an alarm another composite alarm's rule references cannot be deleted, got {body}"
    );
    // "this total can include no more than one composite alarm"
    let (status, body) = call(
        &router,
        "DeleteAlarms",
        &json!({"AlarmNames": ["inner", "outer"]}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "one call can delete no more than one composite alarm, got {body}"
    );
    // Deleting the parent first releases the child, and the child's own child goes after it:
    // a batch does not release its own references.
    ok(&router, "DeleteAlarms", &json!({"AlarmNames": ["outer"]})).await;
    ok(&router, "DeleteAlarms", &json!({"AlarmNames": ["inner"]})).await;
    ok(&router, "DeleteAlarms", &json!({"AlarmNames": ["leaf"]})).await;
    let left = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmTypes": ["CompositeAlarm", "MetricAlarm"]}),
    )
    .await;
    assert_eq!(
        (left.get("CompositeAlarms"), left.get("MetricAlarms")),
        (Some(&json!([])), Some(&json!([]))),
        "every alarm the call named is deleted, got {left}"
    );
}

/// A rule is evaluated over the STATES of the alarms it names, so every alarm a rule names
/// has to exist when the rule is saved: the refusal names the ones that do not, by the ARN
/// each reference resolves to.
#[tokio::test]
async fn a_rule_naming_an_alarm_that_does_not_exist_is_refused_naming_it() {
    let router = build_router();
    put_metric_alarm(&router, "live-child").await;
    let live_arn = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["live-child"]}),
    )
    .await["MetricAlarms"][0]["AlarmArn"]
        .as_str()
        .expect("a stored metric alarm carries its ARN")
        .to_string();
    // The ARN form the service resolves an alarm name to, read off an alarm it really stored:
    // the refusal below names a MISSING alarm by the ARN its own name resolves to, so the
    // expected text is built from that same form rather than from a literal.
    let arn_of = |name: &str| live_arn.replace("live-child", name);

    // Rows differ along what the rule generalises over: the form of the reference (quoted
    // name, ARN, unquoted name), which state function names it, how many references are
    // missing, and where the missing one sits in the rule.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        (
            "one missing alarm by name",
            "ALARM(\"ghost\")".to_string(),
            vec![arn_of("ghost")],
        ),
        (
            "one missing alarm by ARN",
            format!("OK(\"{}\")", arn_of("ghost-by-arn")),
            vec![arn_of("ghost-by-arn")],
        ),
        (
            "an unquoted missing reference",
            "INSUFFICIENT_DATA(ghost-unquoted)".to_string(),
            vec![arn_of("ghost-unquoted")],
        ),
        (
            "a missing alarm beside a live one",
            "ALARM(\"live-child\") AND ALARM(\"ghost-beside\")".to_string(),
            vec![arn_of("ghost-beside")],
        ),
        (
            "two missing alarms, in the order the rule names them",
            "NOT (ALARM(\"ghost-one\") OR OK(\"ghost-two\"))".to_string(),
            vec![arn_of("ghost-one"), arn_of("ghost-two")],
        ),
        (
            // The alarm this very call would create does not exist yet, so a rule naming it
            // names an alarm that does not exist.
            "a rule naming the alarm the call creates",
            "ALARM(\"self-referential\")".to_string(),
            vec![arn_of("self-referential")],
        ),
    ];

    for (case, rule, missing) in &rows {
        let (status, body) = call(
            &router,
            "PutCompositeAlarm",
            &json!({"AlarmName": "self-referential", "AlarmRule": rule}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            error_message(&body),
            format!(
                "Could not save the composite alarm as alarms [{}] in the alarm rule do not \
                 exist",
                missing.join(", ")
            ),
            "case {case:?}: the refusal must name the alarms the rule could not read, got {body}"
        );
        // Nothing the refused call named was stored.
        let stored = ok(
            &router,
            "DescribeAlarms",
            &json!({"AlarmNames": ["self-referential"], "AlarmTypes": ["CompositeAlarm"]}),
        )
        .await;
        assert_eq!(
            stored.get("CompositeAlarms"),
            Some(&json!([])),
            "case {case:?}: a refused request must store nothing, got {stored}"
        );
    }
}

/// Two composite alarms whose rules name each other are both SAVED — each reference names an
/// alarm that exists — and the evaluation settles them both rather than chasing the cycle:
/// "the evaluation of composite alarms stops if CloudWatch detects a cycle in the evaluation
/// path."
#[tokio::test]
async fn a_cycle_between_two_existing_composite_alarms_is_accepted_and_settles() {
    let router = build_router();
    // The pair is built through a live metric alarm, because the alarm each rule names has to
    // exist before that rule can be saved.
    put_metric_alarm(&router, "leaf").await;
    put_composite_alarm(&router, "left", "ALARM(\"leaf\")").await;
    put_composite_alarm(&router, "right", "ALARM(\"left\")").await;
    // And now the cycle: `left` is rewritten to name `right`, which exists.
    put_composite_alarm(&router, "left", "ALARM(\"right\")").await;

    wait_for_evaluation();

    for name in ["left", "right"] {
        let alarm = composite_alarm(&router, name).await;
        assert_eq!(
            text(&alarm, "StateValue").as_deref(),
            Some("OK"),
            "case {name:?}: neither alarm of an accepted cycle is in ALARM, got {alarm}"
        );
        // A second read reports the same state: the pair settles rather than oscillating.
        let again = composite_alarm(&router, name).await;
        assert_eq!(
            again.get("StateValue"),
            alarm.get("StateValue"),
            "case {name:?}: the settled pair must report the same state on every read, got \
             {again}"
        );
    }
}

#[tokio::test]
async fn a_forced_composite_alarm_state_holds_until_a_child_changes_state() {
    let router = build_router();
    put_metric_alarm(&router, "leaf").await;
    set_alarm_state(&router, "leaf", "OK").await;
    put_composite_alarm(&router, "watcher", "ALARM(\"leaf\")").await;

    wait_for_evaluation();
    let evaluated = composite_alarm(&router, "watcher").await;
    assert_eq!(
        text(&evaluated, "StateValue").as_deref(),
        Some("OK"),
        "the rule is false, so the evaluated state is OK, got {evaluated}"
    );

    // "Temporarily sets the state of an alarm for testing purposes." — on a composite alarm
    // too, and "[it] returns to its actual state only once any of its children alarms change
    // state", so the forced state survives every read until then.
    ok(
        &router,
        "SetAlarmState",
        &json!({
            "AlarmName": "watcher",
            "StateValue": "ALARM",
            "StateReason": "testing the composite alarm",
        }),
    )
    .await;
    for read in 1..=3 {
        let forced = composite_alarm(&router, "watcher").await;
        assert_eq!(
            text(&forced, "StateValue").as_deref(),
            Some("ALARM"),
            "read {read}: a forced composite-alarm state holds while no child changes state, \
             got {forced}"
        );
        assert_eq!(
            text(&forced, "StateReason").as_deref(),
            Some("testing the composite alarm"),
            "read {read}: the forced state keeps the reason the caller gave, got {forced}"
        );
    }

    // A child changing state hands the alarm back to its own rule.
    set_alarm_state(&router, "leaf", "INSUFFICIENT_DATA").await;
    let released = composite_alarm(&router, "watcher").await;
    assert_eq!(
        text(&released, "StateValue").as_deref(),
        Some("OK"),
        "once a child changes state the alarm returns to its evaluated state, got {released}"
    );

    // Both the forced update and the return to the evaluated state are in the history.
    let history = ok(
        &router,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": "watcher",
            "AlarmTypes": ["CompositeAlarm"],
            "HistoryItemType": "StateUpdate",
            "ScanBy": "TimestampAscending",
        }),
    )
    .await;
    let items = history
        .get("AlarmHistoryItems")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    assert!(
        items.len() >= 2,
        "every state change of a composite alarm is recorded, got {history}"
    );
    for item in &items {
        assert_eq!(
            text(item, "AlarmType").as_deref(),
            Some("CompositeAlarm"),
            "a composite alarm's state history is recorded under its own alarm type, got {item}"
        );
    }
}

/// A configuration write hands a composite alarm's state back to its rule, whichever operation
/// makes it: "It is also reevaluated if you update its configuration" (`SetAlarmState`). Rows
/// differ along which of the two writers is called and what it asks for.
#[tokio::test]
async fn a_configuration_write_hands_a_forced_composite_state_back_to_the_rule() {
    // (case, the operation, its request, the state once the evaluation is due)
    let rows: Vec<(&str, &str, Value, &str)> = vec![
        (
            "DisableAlarmActions, which writes ActionsEnabled",
            "DisableAlarmActions",
            json!({"AlarmNames": ["watcher"]}),
            "OK",
        ),
        (
            // The alarm is created with its actions on, so asking for them again writes
            // nothing and the forced state keeps the alarm.
            "EnableAlarmActions on an alarm whose actions are already on",
            "EnableAlarmActions",
            json!({"AlarmNames": ["watcher"]}),
            "ALARM",
        ),
    ];

    for (case, operation, request, expected) in &rows {
        let router = build_router();
        put_metric_alarm(&router, "leaf").await;
        set_alarm_state(&router, "leaf", "OK").await;
        put_composite_alarm(&router, "watcher", "ALARM(\"leaf\")").await;
        ok(
            &router,
            "SetAlarmState",
            &json!({
                "AlarmName": "watcher",
                "StateValue": "ALARM",
                "StateReason": "testing the composite alarm",
            }),
        )
        .await;

        ok(&router, operation, request).await;
        wait_for_evaluation();
        let settled = composite_alarm(&router, "watcher").await;
        assert_eq!(
            text(&settled, "StateValue").as_deref(),
            Some(*expected),
            "case {case:?}: expected {expected} once the evaluation is due, got {settled}"
        );
    }
}

#[tokio::test]
async fn actions_are_suppressed_while_the_suppressor_alarm_is_in_alarm() {
    let router = build_router();
    put_metric_alarm(&router, "leaf").await;
    set_alarm_state(&router, "leaf", "ALARM").await;
    put_metric_alarm(&router, "maintenance").await;
    set_alarm_state(&router, "maintenance", "OK").await;
    ok(
        &router,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": "suppressed",
            "AlarmRule": "ALARM(\"leaf\")",
            "ActionsSuppressor": "maintenance",
            "ActionsSuppressorWaitPeriod": 60,
            "ActionsSuppressorExtensionPeriod": 60,
        }),
    )
    .await;

    wait_for_evaluation();
    let waiting = composite_alarm(&router, "suppressed").await;
    // " When the value is WaitPeriod, it means that the actions are suppressed because the
    // composite alarm is waiting for the suppressor alarm to go into into the ALARM state. The
    // maximum waiting time is as specified in ActionsSuppressorWaitPeriod. "
    // (`CompositeAlarm$ActionsSuppressedBy`) — this alarm went into ALARM with its suppressor
    // still outside that state, so the wait is what withholds its actions
    // (`crate::state::composite_suppression`).
    assert_eq!(
        text(&waiting, "ActionsSuppressedBy").as_deref(),
        Some("WaitPeriod"),
        "an alarm that goes into ALARM before its suppressor does waits for it, got {waiting}"
    );

    // "Actions will be suppressed if the suppressor alarm is in the ALARM state."
    set_alarm_state(&router, "maintenance", "ALARM").await;
    let suppressed = composite_alarm(&router, "suppressed").await;
    assert_eq!(
        text(&suppressed, "ActionsSuppressedBy").as_deref(),
        // One of the modelled `ActionsSuppressedBy` values (`ACTIONS_SUPPRESSED_BY_VALUES`).
        Some("Alarm"),
        "a suppressor in ALARM suppresses the alarm's actions, got {suppressed}"
    );
    assert!(
        text(&suppressed, "ActionsSuppressedReason").is_some(),
        "a suppressed alarm says why its actions are suppressed, got {suppressed}"
    );

    set_alarm_state(&router, "maintenance", "OK").await;
    let released = composite_alarm(&router, "suppressed").await;
    // "The maximum time in seconds that the composite alarm waits after suppressor alarm goes
    // out of the ALARM state" (`CompositeAlarm$ActionsSuppressorExtensionPeriod`) — the
    // suppression does not end with the suppressor leaving ALARM: it holds for that period,
    // reported under the value the model declares for it, and ends when the period does
    // (`crate::state::composite_suppression`).
    assert_eq!(
        text(&released, "ActionsSuppressedBy").as_deref(),
        Some("ExtensionPeriod"),
        "the suppressor leaving ALARM opens the extension period, got {released}"
    );
}

#[tokio::test]
async fn a_rule_the_service_cannot_evaluate_is_refused() {
    let router = build_router();

    // A rule with more references than the documented maximum, and one exactly at it: "The
    // rule expression of a composite alarm can include as many as 100 underlying alarms."
    let at_bound = (1..=100)
        .map(|index| format!("ALARM(\"child-{index}\")"))
        .collect::<Vec<String>>()
        .join(" OR ");
    let past_bound = (1..=101)
        .map(|index| format!("ALARM(\"child-{index}\")"))
        .collect::<Vec<String>>()
        .join(" OR ");

    let accepted: Vec<(&str, String)> = vec![
        ("one function", "ALARM(\"child-1\")".to_string()),
        ("a bare literal", "FALSE".to_string()),
        (
            "nested parentheses",
            "((ALARM(\"child-1\")) AND (NOT OK(\"child-2\")))".to_string(),
        ),
        ("a name carrying spaces", "ALARM(\"my child alarm\")".to_string()),
        ("100 referenced alarms", at_bound),
        // The nesting bound this service states for a rule (32 levels, an order of
        // magnitude above CloudWatch's own documented examples): accepted AT the bound.
        (
            "parentheses nested to the bound",
            format!("{}TRUE{}", "(".repeat(32), ")".repeat(32)),
        ),
        (
            "NOT repeated to the nesting bound",
            format!("{}TRUE", "NOT ".repeat(32)),
        ),
    ];
    // Every alarm an ACCEPTED rule references has to exist: a rule naming an alarm that does
    // not is refused (below), so the rows that exercise the PARSE create their children first.
    for child in (1..=100)
        .map(|index| format!("child-{index}"))
        .chain(::std::iter::once("my child alarm".to_string()))
    {
        put_metric_alarm(&router, &child).await;
    }

    for (case, rule) in &accepted {
        let (status, body) = call(
            &router,
            "PutCompositeAlarm",
            &json!({"AlarmName": format!("accepted-{case}"), "AlarmRule": rule}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the rule to be accepted, got {status} with {body}"
        );
    }

    let refused: Vec<(&str, Value)> = vec![
        (
            "an unclosed function",
            json!({"AlarmName": "bad", "AlarmRule": "ALARM(\"child-1\""}),
        ),
        (
            "a function naming no alarm",
            json!({"AlarmName": "bad", "AlarmRule": "ALARM()"}),
        ),
        (
            "a leading operator",
            json!({"AlarmName": "bad", "AlarmRule": "AND ALARM(\"child-1\")"}),
        ),
        (
            "a trailing operator",
            json!({"AlarmName": "bad", "AlarmRule": "ALARM(\"child-1\") OR"}),
        ),
        (
            "two functions with no operator",
            json!({"AlarmName": "bad", "AlarmRule": "ALARM(\"child-1\") ALARM(\"child-2\")"}),
        ),
        (
            "an unknown function",
            json!({"AlarmName": "bad", "AlarmRule": "MAYBE(\"child-1\")"}),
        ),
        (
            // A rule is evaluated over the STATES of the alarms it names, so a name no alarm
            // answers to has no state to read.
            "a function naming an alarm that does not exist",
            json!({"AlarmName": "bad", "AlarmRule": "ALARM(\"no-such-alarm\")"}),
        ),
        (
            "a rule naming the alarm the call itself creates",
            json!({"AlarmName": "bad", "AlarmRule": "ALARM(\"bad\")"}),
        ),
        (
            "an unbalanced parenthesis",
            json!({"AlarmName": "bad", "AlarmRule": "(ALARM(\"child-1\")"}),
        ),
        (
            "101 referenced alarms",
            json!({"AlarmName": "bad", "AlarmRule": past_bound}),
        ),
        (
            // One level past the nesting bound: a rule that nests deeper is refused rather
            // than parsed, so no rule can drive the parse deeper than the bound.
            "parentheses nested past the bound",
            json!({
                "AlarmName": "bad",
                "AlarmRule": format!("{}TRUE{}", "(".repeat(33), ")".repeat(33)),
            }),
        ),
        (
            "NOT repeated past the nesting bound",
            json!({"AlarmName": "bad", "AlarmRule": format!("{}TRUE", "NOT ".repeat(33))}),
        ),
        (
            // A chain of operators nests no parentheses but builds a tree as deep as the
            // chain is long, so the node bound is what refuses it.
            "more functions and operators than the node bound",
            json!({
                "AlarmName": "bad",
                "AlarmRule": format!("{}TRUE", "TRUE OR ".repeat(600)),
            }),
        ),
        (
            // "WaitPeriod is required only when ActionsSuppressor is specified."
            "a suppressor with no wait period",
            json!({
                "AlarmName": "bad",
                "AlarmRule": "FALSE",
                "ActionsSuppressor": "maintenance",
                "ActionsSuppressorExtensionPeriod": 60,
            }),
        ),
        (
            "a suppressor with no extension period",
            json!({
                "AlarmName": "bad",
                "AlarmRule": "FALSE",
                "ActionsSuppressor": "maintenance",
                "ActionsSuppressorWaitPeriod": 60,
            }),
        ),
    ];
    for (case, request) in &refused {
        let (status, body) = call(&router, "PutCompositeAlarm", request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
    }
    // Nothing a refused call named was stored.
    let stored = ok(
        &router,
        "DescribeAlarms",
        &json!({"AlarmNames": ["bad"], "AlarmTypes": ["CompositeAlarm"]}),
    )
    .await;
    assert_eq!(
        stored.get("CompositeAlarms"),
        Some(&json!([])),
        "a refused request must store nothing, got {stored}"
    );

    // A rule past the modelled `AlarmRule` length (`@length 1..10240`) is refused too. The
    // boundary validator answers a modelled length violation with its own code, so only the
    // status is asserted here: the point is that an oversized rule is refused and never
    // parsed.
    let (status, body) = call(
        &router,
        "PutCompositeAlarm",
        &json!({
            "AlarmName": "bad",
            "AlarmRule": format!("{}TRUE", "TRUE OR ".repeat(2_000)),
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "a rule longer than the modelled AlarmRule length must be refused, got {body}"
    );

    // A name a metric alarm already holds cannot also name a composite alarm: "This name
    // must be unique within the Region."
    put_metric_alarm(&router, "taken").await;
    let (status, body) = call(
        &router,
        "PutCompositeAlarm",
        &json!({"AlarmName": "taken", "AlarmRule": "TRUE"}),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a composite alarm cannot take the name of an existing alarm, got {body}"
    );
    // The same rule in the other direction: a metric alarm cannot take the name of an
    // existing composite alarm either.
    let (status, body) = call(
        &router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": "accepted-a bare literal",
            "Namespace": "MyApp",
            "MetricName": "Latency",
            "Statistic": "Average",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "a metric alarm cannot take the name of an existing composite alarm, got {body}"
    );
}
