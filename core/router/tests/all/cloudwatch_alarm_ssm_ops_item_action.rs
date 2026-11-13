// SPDX-License-Identifier: CC0-1.0

//! An alarm's SYSTEMS MANAGER OPSITEM action opens the work item in OpsCenter — through the
//! WHOLE mounted service set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **an alarm whose action list names
//! `arn:aws:ssm:<region>:<account>:opsitem:<severity>[#CATEGORY=<category>]`
//! (`PutMetricAlarmInput$AlarmActions`) has Systems Manager OpsCenter open an OpsItem when the
//! alarm transitions into the state that list belongs to: the work item is TITLED for the alarm
//! that raised it, carries the origin OpsCenter records an alarm-raised item under, and carries
//! the severity the ARN names and the category it names when it names one; the alarm's history
//! accounts for the execution. `ActionsEnabled` is the gate in front of it — "indicates whether
//! actions should be executed during any changes to the alarm state"
//! (`MetricAlarm$ActionsEnabled`) — so an otherwise identical alarm with the switch off
//! transitions and opens nothing, until `EnableAlarmActions` and a fresh transition.**
//!
//! Two services have to agree for that to hold — CloudWatch executes the action, Systems
//! Manager holds the OpsItem — and the create CloudWatch makes is an in-process call addressed
//! the way a client's own `ssm`-scoped call is addressed, so WHICH mounted model claims it is
//! only decidable with every service mounted. What the OpsItem is asserted on is the OpsCenter
//! read a client uses (`DescribeOpsItems`), so an OpsItem reported here is one Systems Manager
//! itself holds.
//!
//! The rows differ along what the rule generalises over: the SEVERITY the action names, whether
//! it names a category at all, which action LIST the OpsItem action is named in (so the
//! transition that fires it is into ALARM for one row and into OK for another), and the Region
//! the whole wiring lives in. One row names a severity Systems Manager does not support, where
//! that service's own refusal is what the alarm's history records and nothing is opened.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{authorization, cw, router};

/// How long a leg waits for the work another service's clock owes before calling the OpsItem
/// unopened. Generous on purpose: nothing here is asserted about how fast the host runs.
const WORK_WAIT: Duration = Duration::from_secs(60);

/// How often it looks while waiting.
const LOOK_INTERVAL: Duration = Duration::from_millis(50);

/// The account every ARN of this emulator names.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The origin OpsCenter records for every work item a CloudWatch alarm action opens, whichever
/// alarm raised it.
const OPS_ITEM_SOURCE: &str = "CloudWatch Alarm";

/// One Systems Manager call over its `awsJson1_1` wire, answering `(status, body)`.
async fn ssm(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("ssm.{region}.amazonaws.com"))
        .header("authorization", authorization("ssm", region))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonSSM.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The OpsItems OpsCenter holds in `region` that were opened for the alarm `alarm` — the way a
/// client looks for the work item an alarm raised: among the items an alarm action opened, the
/// ones titled for that alarm.
async fn ops_items_of(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
) -> Vec<Value> {
    let (status, response) = ssm(
        router,
        region,
        "DescribeOpsItems",
        &json!({
            "OpsItemFilters": [
                {"Key": "Source", "Operator": "Equal", "Values": [OPS_ITEM_SOURCE]},
                {"Key": "Title", "Operator": "Equal", "Values": [alarm]},
            ],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeOpsItems, got {status} with {response}"
    );
    response["OpsItemSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Wait until OpsCenter holds `wanted` OpsItems for the alarm, and answer what it holds by
/// then. The wait is for the CONDITION rather than for a span.
async fn until_ops_items(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    wanted: usize,
) -> Vec<Value> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let held = ops_items_of(router, case, region, alarm).await;
        if held.len() >= wanted {
            return held;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    ops_items_of(router, case, region, alarm).await
}

/// The `Action` history items the alarm holds, as `DescribeAlarmHistory` reports them.
async fn action_items(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
) -> Vec<Value> {
    let body = cw(
        router,
        case,
        region,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": alarm,
            "AlarmTypes": ["MetricAlarm"],
            "HistoryItemType": "Action",
        }),
    )
    .await;
    body["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// Wait until the alarm accounts for `wanted` action executions, either way.
async fn until_recorded(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    wanted: usize,
) -> Vec<Value> {
    let looks = WORK_WAIT.as_millis() / LOOK_INTERVAL.as_millis().max(1);
    for _ in 0..looks {
        let items = action_items(router, case, region, alarm).await;
        if items.len() >= wanted {
            return items;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    action_items(router, case, region, alarm).await
}

/// The `HistoryData` of one action item, parsed.
fn history_data(item: &Value) -> Value {
    serde_json::from_str(item["HistoryData"].as_str().unwrap_or("null")).unwrap_or(Value::Null)
}

/// Write one metric alarm whose action list for `state` names `action`, and whose actions are
/// switched on or off as `actions_enabled` says.
async fn put_alarm(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    action: &str,
    state: &str,
    actions_enabled: bool,
) {
    let named = json!([action]);
    let none = json!([]);
    let (ok_actions, alarm_actions) = if state == "OK" {
        (named, none)
    } else {
        (none, named)
    };
    cw(
        router,
        case,
        region,
        "PutMetricAlarm",
        &json!({
            "AlarmName": alarm,
            "Namespace": "Custom/OpsItemActions",
            "MetricName": "Backlog",
            "Dimensions": [{"Name": "QueueName", "Value": alarm}],
            "Statistic": "Maximum",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            // The alarm keeps the state a row forces: "ignore — the current alarm state is
            // maintained" (`PutMetricAlarmInput$TreatMissingData`).
            "TreatMissingData": "ignore",
            "ActionsEnabled": actions_enabled,
            "OKActions": ok_actions,
            "AlarmActions": alarm_actions,
        }),
    )
    .await;
}

/// Force one alarm into `state` with `reason`, which is what fires the actions configured for
/// it.
async fn force(
    router: &MultiServiceRouter,
    case: &str,
    region: &str,
    alarm: &str,
    state: &str,
    reason: &str,
) {
    cw(
        router,
        case,
        region,
        "SetAlarmState",
        &json!({"AlarmName": alarm, "StateValue": state, "StateReason": reason}),
    )
    .await;
}

/// One row: the action ARN's severity and category, the state whose action list names it, the
/// Region it all lives in, and whether Systems Manager opens the OpsItem at all.
struct Row {
    case: &'static str,
    region: &'static str,
    severity: &'static str,
    category: Option<&'static str>,
    state: &'static str,
    opened: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a severity and a category, fired by the transition into ALARM",
            region: "us-west-2",
            severity: "3",
            category: Some("Availability"),
            state: "ALARM",
            opened: true,
        },
        Row {
            case: "a severity alone, in another Region",
            region: "eu-west-1",
            severity: "1",
            category: None,
            state: "ALARM",
            opened: true,
        },
        Row {
            case: "another severity and category, fired by the transition into OK",
            region: "us-west-2",
            severity: "4",
            category: Some("Security"),
            state: "OK",
            opened: true,
        },
        Row {
            // "Severity options range from 1 to 4" (`OpsItem$Severity`), so Systems Manager
            // refuses this create and that refusal is the whole account of the execution.
            case: "a severity Systems Manager does not support",
            region: "us-west-2",
            severity: "9",
            category: Some("Availability"),
            state: "ALARM",
            opened: false,
        },
    ]
}

/// RULE: an alarm action naming an OpsItem has OpsCenter open that work item on the
/// transition, and `ActionsEnabled` is the gate in front of it.
#[tokio::test]
async fn an_ops_item_alarm_action_opens_the_work_item_and_the_switch_gates_it() {
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let region = row.region;
        let router = router();
        let armed = format!("opsitem-action-armed-{index}");
        let disabled = format!("opsitem-action-disabled-{index}");
        let action = match row.category {
            Some(category) => format!(
                "arn:aws:ssm:{region}:{ACCOUNT}:opsitem:{}#CATEGORY={category}",
                row.severity
            ),
            None => format!("arn:aws:ssm:{region}:{ACCOUNT}:opsitem:{}", row.severity),
        };

        // The pair: one alarm with its actions on, and its twin with them off.
        put_alarm(&router, case, region, &armed, &action, row.state, true).await;
        put_alarm(&router, case, region, &disabled, &action, row.state, false).await;

        let reason = "driven by the test";
        // The transition into the state whose action list names the OpsItem action. A force
        // into the other state first makes the second force a state CHANGE whatever the
        // alarm's own evaluation left behind, and fires nothing itself: the other list is
        // empty.
        let other = if row.state == "OK" { "ALARM" } else { "OK" };
        for alarm in [&armed, &disabled] {
            force(&router, case, region, alarm, other, reason).await;
            force(&router, case, region, alarm, row.state, reason).await;
        }

        // Both alarms moved: the switch gates the ACTION, not the transition.
        for alarm in [&armed, &disabled] {
            let described = cw(
                &router,
                case,
                region,
                "DescribeAlarms",
                &json!({"AlarmNames": [alarm], "AlarmTypes": ["MetricAlarm"]}),
            )
            .await;
            assert_eq!(
                described["MetricAlarms"][0]["StateValue"].as_str(),
                Some(row.state),
                "case {case:?}: {alarm} must have moved to {}, got {described}",
                row.state
            );
        }

        let items = until_recorded(&router, case, region, &armed, 1).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the armed alarm must account for exactly one action execution, got \
             {items:?}"
        );
        let summary = items[0]["HistorySummary"].as_str().unwrap_or_default();
        let data = history_data(&items[0]);

        if !row.opened {
            assert_eq!(
                summary,
                format!("Failed to execute action {action}"),
                "case {case:?}: expected the action to be recorded as failed, got {summary:?} \
                 with {data}"
            );
            let error = data["error"].as_str().unwrap_or_default();
            assert!(
                error.contains(row.severity),
                "case {case:?}: expected the recorded error to be the refusal naming the \
                 severity {:?}, got {error:?}",
                row.severity
            );
            let held = ops_items_of(&router, case, region, &armed).await;
            assert!(
                held.is_empty(),
                "case {case:?}: a refused create opens no OpsItem, got {held:?}"
            );
            continue;
        }

        assert_eq!(
            summary,
            format!("Successfully executed action {action}"),
            "case {case:?}: expected the action to be recorded as executed, got {summary:?} \
             with {data}"
        );
        assert_eq!(
            data["actionState"],
            json!("Succeeded"),
            "case {case:?}: expected the recorded action to have succeeded, got {data}"
        );

        // What a client sees in OpsCenter: ONE work item, naming the alarm that raised it and
        // carrying the severity and category the action's own ARN named.
        let opened = until_ops_items(&router, case, region, &armed, 1).await;
        assert_eq!(
            opened.len(),
            1,
            "case {case:?}: the armed alarm's action must open exactly one OpsItem, got \
             {opened:?}"
        );
        let item = &opened[0];
        assert_eq!(
            item["Title"].as_str(),
            Some(armed.as_str()),
            "case {case:?}: the OpsItem must be titled for the alarm that raised it, got {item}"
        );
        assert_eq!(
            item["Source"].as_str(),
            Some(OPS_ITEM_SOURCE),
            "case {case:?}: the OpsItem must carry the origin OpsCenter records for an \
             alarm-raised work item, got {item}"
        );
        assert_eq!(
            item["Severity"].as_str(),
            Some(row.severity),
            "case {case:?}: the OpsItem must carry the severity the action named, got {item}"
        );
        assert_eq!(
            item["Category"].as_str(),
            row.category,
            "case {case:?}: the OpsItem must carry the category the action named, got {item}"
        );
        assert_eq!(
            item["Status"].as_str(),
            Some("Open"),
            "case {case:?}: a just-opened work item is Open, got {item}"
        );
        // And it says what happened: the state the alarm moved to and the reason it gave.
        let (status, read) = ssm(
            &router,
            region,
            "GetOpsItem",
            &json!({"OpsItemId": item["OpsItemId"].as_str().unwrap_or_default()}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetOpsItem, got {status} with {read}"
        );
        let description = read["OpsItem"]["Description"].as_str().unwrap_or_default();
        for named in [armed.as_str(), row.state, reason] {
            assert!(
                description.contains(named),
                "case {case:?}: expected the OpsItem's description to report {named:?}, got \
                 {description:?}"
            );
        }
        // And it names the alarm it is about as a RELATED RESOURCE, by that alarm's own ARN.
        let described = cw(
            &router,
            case,
            region,
            "DescribeAlarms",
            &json!({"AlarmNames": [&armed], "AlarmTypes": ["MetricAlarm"]}),
        )
        .await;
        let alarm_arn = described["MetricAlarms"][0]["AlarmArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no AlarmArn for {armed}: {described}"))
            .to_string();
        let related = read["OpsItem"]["OperationalData"]["/aws/resources"]["Value"]
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the OpsItem to carry its related resources, got {}",
                    read["OpsItem"]
                )
            })
            .to_string();
        let resources: Value = serde_json::from_str(&related).unwrap_or_else(|error| {
            panic!("case {case:?}: the related resources are not JSON ({error}): {related}")
        });
        assert_eq!(
            resources,
            json!([{"arn": alarm_arn}]),
            "case {case:?}: expected the alarm to be the OpsItem's related resource, got \
             {resources}"
        );

        // The twin: the same transition with the switch off opens nothing and accounts for no
        // execution.
        let twins = ops_items_of(&router, case, region, &disabled).await;
        assert!(
            twins.is_empty(),
            "case {case:?}: an alarm whose actions are off opens no OpsItem, got {twins:?}"
        );
        let suppressed = action_items(&router, case, region, &disabled).await;
        assert!(
            suppressed.is_empty(),
            "case {case:?}: a suppressed action is no execution to account for, got \
             {suppressed:?}"
        );

        // And back on: the switch is what was withholding it, so a fresh transition of the
        // SAME alarm opens the work item — and the one withheld while the actions were off is
        // not opened late.
        cw(
            &router,
            case,
            region,
            "EnableAlarmActions",
            &json!({"AlarmNames": [&disabled]}),
        )
        .await;
        force(&router, case, region, &disabled, other, reason).await;
        force(&router, case, region, &disabled, row.state, reason).await;
        let restored = until_ops_items(&router, case, region, &disabled, 1).await;
        assert_eq!(
            restored.len(),
            1,
            "case {case:?}: the transition made with the actions back on must open the work \
             item, and only one, got {restored:?}"
        );
        assert_eq!(
            restored[0]["Severity"].as_str(),
            Some(row.severity),
            "case {case:?}: the restored OpsItem must carry the action's severity, got {}",
            restored[0]
        );
    }
}
