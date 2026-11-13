// SPDX-License-Identifier: CC0-1.0

//! What an alarm's EC2 INSTANCE action DOES, end to end over the wire through the real
//! `MultiServiceRouter` with CloudWatch and EC2 on one engine.
//!
//! The RULE asserted here: **when an alarm whose actions are enabled transitions to a state
//! whose action list carries an EC2 instance action — `arn:aws:automate:<region>:ec2:<verb>`
//! or the Simple Workflow form of the same verb — that EC2 operation is performed on the
//! instance the alarm's own metric is dimensioned by, in the Region the action names, and the
//! alarm's history gains one `Action` item reporting the outcome; an alarm with
//! `ActionsEnabled=false` evaluates the same way and executes nothing, leaving its instance
//! untouched and its history without an `Action` item.**
//!
//! The rows span what the rule generalises over rather than repeating the recorded reboot: the
//! VERB (stop, terminate, reboot, recover), the ARN FORM (the `automate` form and the Simple
//! Workflow form), which of the three action lists carries it and therefore which transition
//! fires it, how the alarm identifies its per-instance metric (top-level `Dimensions` or a
//! `Metrics` query array), and whether the alarm's actions are enabled. Every row launches a
//! BYSTANDER instance beside the one the alarm names, so no row can pass by acting on every
//! instance of the account.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_ec2_amazonec2 as ec2;

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, TRANSITION_DELAY};

use crate::harness::{ec2 as ec2_api, element, emulator, rpc, Emulator, REGION, TARGET_PREFIX};

/// The four EC2 instance actions this service's alarm writers document, each in both forms:
/// "EC2 actions: arn:aws:automate:region:ec2:stop arn:aws:automate:region:ec2:terminate
/// arn:aws:automate:region:ec2:reboot arn:aws:automate:region:ec2:recover
/// arn:aws:swf:region:account-id:action/actions/AWS_EC2.InstanceId.Stop/1.0 …"
/// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="AlarmActions")`).
fn automate_action(verb: &str) -> String {
    format!("arn:aws:automate:{REGION}:ec2:{verb}")
}

/// The Simple Workflow form of the same action, from the same documentation. The account is
/// the one this emulator mints its own ARNs under.
fn swf_action(verb: &str) -> String {
    let capitalised = format!(
        "{}{}",
        verb[..1].to_uppercase(),
        &verb[1..]
    );
    format!(
        "arn:aws:swf:{REGION}:000000000000:action/actions/AWS_EC2.InstanceId.{capitalised}/1.0"
    )
}

/// How long a row waits for an asynchronous effect, and how often it looks. Generous only so
/// a failure is a failure rather than a hang.
const DEADLINE: Duration = Duration::from_secs(20);
const LOOK_AGAIN: Duration = Duration::from_millis(50);

/// Long enough for every EC2 move the clock already owes to have fallen due: a launch
/// reaching `running` and its guest's first boot, a queued reboot taking effect, a stop
/// settling into `stopped`. Derived from EC2's own span rather than a literal, and taken twice
/// because a lifecycle move and the boot that follows it are two spans.
fn settled() -> Duration {
    TRANSITION_DELAY * 2 + Duration::from_millis(300)
}

/// A `(key, value)` ec2Query form field.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One CloudWatch call, requiring a 200.
async fn cw(emulator: &Emulator, operation: &str, body: &Value) -> Value {
    rpc(emulator, "monitoring", TARGET_PREFIX, operation, body).await
}

/// One AMI of the calling account, seeded the way a register operation would store it: a
/// launch resolves the image it names, so the suite needs an image before it has an instance.
fn given_image(emulator: &Emulator) -> String {
    let store = emulator.engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard.insert_image(ImageSpec {
        name: "cloudwatch-ec2-instance-action".to_string(),
        ..ImageSpec::default()
    })
}

/// Launch `count` instances of `image_id`, answering their ids in launch order.
async fn launch(emulator: &Emulator, image_id: &str, count: usize) -> Vec<String> {
    let body = ec2_api(
        emulator,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("InstanceType", "t3.small"),
            p("MinCount", &count.to_string()),
            p("MaxCount", &count.to_string()),
        ],
    )
    .await;
    let ids = crate::harness::elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count,
        "RunInstances must report the {count} instance(s) it minted: {body}"
    );
    ids
}

/// The lifecycle state EC2 reports for one instance (`Instance$State`).
async fn instance_state(emulator: &Emulator, instance_id: &str) -> String {
    let body = ec2_api(
        emulator,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    let reported = body
        .split("<instanceState>")
        .nth(1)
        .unwrap_or_else(|| panic!("DescribeInstances reports no instanceState: {body}"));
    element(reported, "name")
}

/// The boot identities one instance's console carries, oldest first: a guest writes its own
/// to the console on every boot, so a second one is a guest that restarted.
///
/// A guest's init system publishes the facts it discovered about its machine as
/// `ci-info: <name>: <value>`, so that is where the identity is read from.
async fn console_boot_ids(emulator: &Emulator, instance_id: &str) -> Vec<String> {
    let body = ec2_api(
        emulator,
        "GetConsoleOutput",
        &[p("InstanceId", instance_id), p("Latest", "true")],
    )
    .await;
    let encoded = element(&body, "output");
    let bytes = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &encoded)
        .unwrap_or_else(|error| panic!("the console output must be base64 ({error}): {body}"));
    let text = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("the console text must be UTF-8 ({error})"));
    text.lines()
        .filter_map(|line| line.split_once("ci-info: boot-id: "))
        .map(|(_, value)| value.trim().to_string())
        .collect()
}

/// The `Action` items the alarm's history holds, each as the action it names beside its parsed
/// `HistoryData`.
async fn action_items(emulator: &Emulator, alarm: &str) -> Vec<(String, Value)> {
    let history = cw(
        emulator,
        "DescribeAlarmHistory",
        &json!({
            "AlarmName": alarm,
            "HistoryItemType": "Action",
            "AlarmTypes": ["MetricAlarm"],
        }),
    )
    .await;
    history["AlarmHistoryItems"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|item| {
            let data = item["HistoryData"].as_str().unwrap_or_default();
            let parsed: Value = serde_json::from_str(data)
                .unwrap_or_else(|error| panic!("HistoryData is not JSON ({error}): {data}"));
            let resource = parsed["notificationResource"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let summary = item["HistorySummary"].as_str().unwrap_or_default().to_string();
            (resource, json!({"data": parsed, "summary": summary}))
        })
        .collect()
}

/// Poll the alarm's `Action` history until it holds at least `wanted` items or the deadline
/// passes.
async fn polled_action_items(
    emulator: &Emulator,
    alarm: &str,
    wanted: usize,
) -> Vec<(String, Value)> {
    let deadline = std::time::Instant::now() + DEADLINE;
    loop {
        let items = action_items(emulator, alarm).await;
        if items.len() >= wanted || std::time::Instant::now() >= deadline {
            return items;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// How a row identifies the per-instance metric its alarm watches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MetricShape {
    /// `Namespace`/`MetricName` plus the alarm's own `Dimensions`.
    SingleMetric,
    /// A `Metrics` query array, whose returning query retrieves the per-instance metric.
    QueryArray,
}

/// What EC2 must report once a row's transition has fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The guest booted a second time and the instance is still `running`: a reboot restarts
    /// the operating system rather than the instance.
    Rebooted,
    /// The instance reached this lifecycle state.
    Moved(&'static str),
    /// Nothing at all happened: one boot, still `running`, and no `Action` item.
    Untouched,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// The action ARN the alarm carries.
    action: String,
    /// Which action list carries it.
    member: &'static str,
    /// The states the row forces in order; the LAST one is the transition that fires the
    /// action, the ones before it put the alarm where that transition starts from.
    states: &'static [&'static str],
    /// What the alarm's `ActionsEnabled` says, or `None` to leave the member out and take the
    /// documented default.
    actions_enabled: Option<bool>,
    shape: MetricShape,
    expected: Expected,
}

/// The rows the rule is asserted over. See the suite docs for what they span.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded reboot action, in AlarmActions, fired by a move to ALARM",
            action: automate_action("reboot"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: None,
            shape: MetricShape::SingleMetric,
            expected: Expected::Rebooted,
        },
        Row {
            case: "the same reboot in its Simple Workflow form",
            action: swf_action("reboot"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(true),
            shape: MetricShape::SingleMetric,
            expected: Expected::Rebooted,
        },
        Row {
            case: "a recover action, which restarts the same instance",
            action: automate_action("recover"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(true),
            shape: MetricShape::SingleMetric,
            expected: Expected::Rebooted,
        },
        Row {
            case: "a reboot action in OKActions, fired by a move to OK",
            action: automate_action("reboot"),
            member: "OKActions",
            states: &["OK"],
            actions_enabled: Some(true),
            shape: MetricShape::SingleMetric,
            expected: Expected::Rebooted,
        },
        Row {
            case: "a reboot action in InsufficientDataActions, fired by a move back to \
                   INSUFFICIENT_DATA",
            action: automate_action("reboot"),
            member: "InsufficientDataActions",
            states: &["ALARM", "INSUFFICIENT_DATA"],
            actions_enabled: Some(true),
            shape: MetricShape::SingleMetric,
            expected: Expected::Rebooted,
        },
        Row {
            case: "a reboot action on an alarm that watches a Metrics query array",
            action: automate_action("reboot"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(true),
            shape: MetricShape::QueryArray,
            expected: Expected::Rebooted,
        },
        Row {
            case: "a stop action",
            action: automate_action("stop"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(true),
            shape: MetricShape::SingleMetric,
            expected: Expected::Moved("stopped"),
        },
        Row {
            case: "a terminate action",
            action: automate_action("terminate"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(true),
            shape: MetricShape::QueryArray,
            expected: Expected::Moved("terminated"),
        },
        Row {
            case: "the twin of the reboot row whose actions are disabled",
            action: automate_action("reboot"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(false),
            shape: MetricShape::SingleMetric,
            expected: Expected::Untouched,
        },
        Row {
            case: "a disabled twin of the terminate row",
            action: automate_action("terminate"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(false),
            shape: MetricShape::SingleMetric,
            expected: Expected::Untouched,
        },
    ]
}

/// The `PutMetricAlarm` request one row writes: a per-instance metric, stated the way the
/// row's shape says, carrying the row's action in the row's action list.
fn alarm_request(row: &Row, name: &str, instance_id: &str) -> Value {
    let dimensions = json!([{"Name": "InstanceId", "Value": instance_id}]);
    let mut request = json!({
        "AlarmName": name,
        "ComparisonOperator": "GreaterThanThreshold",
        "EvaluationPeriods": 1,
        "Threshold": 80.0,
        row.member: [row.action.clone()],
    });
    let members = request
        .as_object_mut()
        .expect("the request under construction is an object");
    match row.shape {
        MetricShape::SingleMetric => {
            members.insert("Namespace".to_string(), json!("AWS/EC2"));
            members.insert("MetricName".to_string(), json!("CPUUtilization"));
            members.insert("Dimensions".to_string(), dimensions);
            members.insert("Period".to_string(), json!(60));
            members.insert("Statistic".to_string(), json!("Average"));
        }
        MetricShape::QueryArray => {
            members.insert(
                "Metrics".to_string(),
                json!([{
                    "Id": "m1",
                    "MetricStat": {
                        "Metric": {
                            "Namespace": "AWS/EC2",
                            "MetricName": "CPUUtilization",
                            "Dimensions": dimensions,
                        },
                        "Period": 60,
                        "Stat": "Average",
                    },
                    "ReturnData": true,
                }]),
            );
        }
    }
    if let Some(enabled) = row.actions_enabled {
        members.insert("ActionsEnabled".to_string(), json!(enabled));
    }
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_alarms_ec2_instance_action_is_executed_against_the_instance_its_metric_names() {
    for row in rows() {
        let case = row.case;
        let emulator = emulator();
        let image_id = given_image(&emulator);
        // Two instances: the one the alarm's metric names, and a bystander no alarm names.
        let instances = launch(&emulator, &image_id, 2).await;
        let (instance_id, bystander) = (instances[0].clone(), instances[1].clone());
        // The launch has to have settled before the action fires: the row asserts what the
        // ACTION did to a running instance with a booted guest.
        tokio::time::sleep(settled()).await;
        let before = console_boot_ids(&emulator, &instance_id).await;
        assert_eq!(
            before.len(),
            1,
            "case {case:?}: a launched guest has booted once, got {before:?}"
        );

        let alarm = "instance-health";
        let request = alarm_request(&row, alarm, &instance_id);
        cw(&emulator, "PutMetricAlarm", &request).await;
        for state in row.states {
            cw(
                &emulator,
                "SetAlarmState",
                &json!({
                    "AlarmName": alarm,
                    "StateValue": state,
                    "StateReason": "driven by the test",
                }),
            )
            .await;
        }

        match row.expected {
            Expected::Untouched => {
                // Nothing is owed, so the row waits out the span in which an executed action
                // would have shown and asserts the instance is where it was.
                tokio::time::sleep(settled()).await;
                let after = console_boot_ids(&emulator, &instance_id).await;
                assert_eq!(
                    after, before,
                    "case {case:?}: an alarm whose actions are disabled must leave its \
                     instance on its first boot, got {after:?}"
                );
                assert_eq!(
                    instance_state(&emulator, &instance_id).await,
                    "running",
                    "case {case:?}: a suppressed action must not move the instance"
                );
                let items = action_items(&emulator, alarm).await;
                assert!(
                    items.is_empty(),
                    "case {case:?}: a suppressed action must record no Action item, got \
                     {items:?}"
                );
                continue;
            }
            Expected::Rebooted => {
                // A reboot is owed to EC2's clock, so the row waits for the boot it queued.
                let deadline = std::time::Instant::now() + DEADLINE;
                let mut after = before.clone();
                while after.len() < 2 && std::time::Instant::now() < deadline {
                    tokio::time::sleep(LOOK_AGAIN).await;
                    after = console_boot_ids(&emulator, &instance_id).await;
                }
                assert_eq!(
                    after.len(),
                    2,
                    "case {case:?}: the executed action must boot the guest a second time, \
                     got {after:?}"
                );
                assert!(
                    after.starts_with(&before),
                    "case {case:?}: the console must keep what the first boot wrote, got \
                     {after:?} after {before:?}"
                );
                assert_ne!(
                    after[0], after[1],
                    "case {case:?}: the second boot must carry its own identity, got {after:?}"
                );
                assert_eq!(
                    instance_state(&emulator, &instance_id).await,
                    "running",
                    "case {case:?}: a restarted instance is still running"
                );
            }
            Expected::Moved(state) => {
                let deadline = std::time::Instant::now() + DEADLINE;
                let mut reported = instance_state(&emulator, &instance_id).await;
                while reported != state && std::time::Instant::now() < deadline {
                    tokio::time::sleep(LOOK_AGAIN).await;
                    reported = instance_state(&emulator, &instance_id).await;
                }
                assert_eq!(
                    reported, state,
                    "case {case:?}: the executed action must leave the instance {state:?}"
                );
            }
        }

        // The execution is accounted for in the alarm's own history, exactly once.
        let items = polled_action_items(&emulator, alarm, 1).await;
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: the executed action must be recorded once, got {items:?}"
        );
        let (resource, item) = &items[0];
        assert_eq!(
            resource, &row.action,
            "case {case:?}: the item must name the action that was executed, got {item}"
        );
        assert_eq!(
            item["data"]["actionState"].as_str(),
            Some("Succeeded"),
            "case {case:?}: an action EC2 carried out is Succeeded, got {item}"
        );
        assert_eq!(
            item["data"]["stateValue"].as_str(),
            row.states.last().copied(),
            "case {case:?}: the item must report the state that fired the action, got {item}"
        );
        assert_eq!(
            item["data"]["error"],
            Value::Null,
            "case {case:?}: an action that succeeded records no error, got {item}"
        );
        assert_eq!(
            item["summary"].as_str(),
            Some(format!("Successfully executed action {}", row.action).as_str()),
            "case {case:?}: the summary must report the executed action, got {item}"
        );
        // An EC2 instance action hands no message to a target, so nothing claims one was
        // published.
        assert_eq!(
            item["data"]["publishedMessage"],
            Value::Null,
            "case {case:?}: an EC2 instance action publishes nothing, got {item}"
        );

        // The bystander is untouched: the action acted on the instance the alarm's metric
        // named, not on every instance of the account.
        let untouched = console_boot_ids(&emulator, &bystander).await;
        assert_eq!(
            untouched.len(),
            1,
            "case {case:?}: the instance no alarm names stays on its first boot, got \
             {untouched:?}"
        );
        assert_eq!(
            instance_state(&emulator, &bystander).await,
            "running",
            "case {case:?}: the instance no alarm names must not move"
        );
    }
}

/// RULE: the instance an EC2 instance action acts on is the one the alarm's metric names, so
/// two alarms over two instances act on one each — and an action is executed once per
/// transition, not once per read of the alarm.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_alarm_reboots_only_the_instance_its_own_metric_is_dimensioned_by() {
    let emulator = emulator();
    let image_id = given_image(&emulator);
    let instances = launch(&emulator, &image_id, 3).await;
    tokio::time::sleep(settled()).await;

    // One alarm per instance for the first two; the third instance carries no alarm at all.
    for (index, instance_id) in instances.iter().take(2).enumerate() {
        let row = Row {
            case: "one alarm per instance",
            action: automate_action("reboot"),
            member: "AlarmActions",
            states: &["ALARM"],
            actions_enabled: Some(true),
            shape: MetricShape::SingleMetric,
            expected: Expected::Rebooted,
        };
        let alarm = format!("instance-health-{index}");
        cw(
            &emulator,
            "PutMetricAlarm",
            &alarm_request(&row, &alarm, instance_id),
        )
        .await;
        cw(
            &emulator,
            "SetAlarmState",
            &json!({
                "AlarmName": alarm,
                "StateValue": "ALARM",
                "StateReason": "driven by the test",
            }),
        )
        .await;
    }

    // Each of the two alarmed instances boots exactly once more, however long a client keeps
    // reading: the transition fired once, so the reboot happened once.
    tokio::time::sleep(settled()).await;
    for (index, instance_id) in instances.iter().enumerate() {
        let expected = if index < 2 { 2 } else { 1 };
        let case = format!("instance {index}");
        let seen = console_boot_ids(&emulator, instance_id).await;
        assert_eq!(
            seen.len(),
            expected,
            "case {case:?}: expected {expected} boot(s), got {seen:?}"
        );
        // Read again: reading an alarm, a history or a console executes nothing.
        let items = action_items(&emulator, &format!("instance-health-{index}")).await;
        let again = console_boot_ids(&emulator, instance_id).await;
        assert_eq!(
            again, seen,
            "case {case:?}: reading must not execute the action again, got {again:?} after \
             {items:?}"
        );
    }
}
