// SPDX-License-Identifier: CC0-1.0

//! RULE: while an experiment runs, FIS watches the CloudWatch alarms its `stopConditions`
//! name and ends the run ITSELF — terminal status `stopped`, with no `StopExperiment` call
//! from the caller — as soon as one of those alarms is in the `ALARM` state, and it reports
//! that halt in its OWN fixed sentence: a state reason names the cause FIS recognises and
//! carries no value of the condition, the alarm or the account. A condition that names no
//! alarm (`source: none`, FIS's own "specify `none` if there is no stop condition"), an
//! alarm in any other state, and an alarm this account holds none of all halt nothing: the
//! run then reaches the terminal status its actions earned.
//!
//! `ExperimentStopCondition` is modelled as `{source, value}` and `ExperimentStatus`
//! includes `stopped` (`smithy_query(query="shape",shape="ExperimentStopCondition")`), but
//! the model states no evaluation rule, so this is the recorded behaviour of the service:
//! the alarm's state is the one CloudWatch holds, which is why the suite mounts CloudWatch
//! beside FIS on ONE engine — exactly as the assembled emulator does — and drives the
//! alarm through `PutMetricAlarm` / `SetAlarmState` as a client would.
//!
//! The rows differ along everything the rule generalises over: which state the alarm is
//! driven into, how many conditions the template carries and which of them breaches,
//! whether the condition names an alarm at all, and whether the alarm it names exists.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::experiment_lifecycle::{
    create_template, get_experiment, poll_until_final, start_experiment, token,
};

/// The `source` of a stop condition that watches a CloudWatch alarm, and the one that
/// watches nothing — the two values FIS documents for `StopConditionSource`.
const SOURCE_ALARM: &str = "aws:cloudwatch:alarm";
/// See [`SOURCE_ALARM`].
const SOURCE_NONE: &str = "none";

/// The `X-Amz-Target` prefix CloudWatch's model declares, and the signing name that routes
/// a request to it.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";
/// See [`CLOUDWATCH_TARGET_PREFIX`].
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The Region every call in this suite is signed for, so the alarm ARNs CloudWatch mints
/// and the experiments FIS mints belong to one Region.
const REGION: &str = "us-east-1";

/// The wait every template here carries: inside the 1..720 minutes FIS documents for
/// `aws:fis:wait`, and long enough on the emulator's compressed run clock that a row gets
/// to drive its alarm while the experiment is still running.
const WAIT_DURATION: &str = "PT5M";

/// How long a row waits for its run to reach a terminal status: the whole compressed run
/// window, the transitions around it, and slack for a loaded machine.
const TERMINAL_BUDGET: Duration = Duration::from_secs(25);

/// How long a row waits for the experiment to become `running` before it drives its alarm.
const RUNNING_BUDGET: Duration = Duration::from_secs(10);

/// How many experiments may be active at once — the account's published quota, which is
/// what the rows are started in groups of.
const ACTIVE_EXPERIMENTS_QUOTA: usize = 5;

/// An alarm ARN of this account and Region for an alarm nothing created, built the way
/// CloudWatch builds one.
const ABSENT_ALARM_ARN: &str = "arn:aws:cloudwatch:us-east-1:000000000000:alarm:fis-no-such-alarm";

/// The `state.reason` of an experiment one of its stop conditions halted — the whole of it,
/// as the recorded service reports it. Spelled out here rather than imported, so a changed
/// wording fails this suite.
const HALTED_BY_STOP_CONDITION: &str = "Experiment halted by stop condition.";

/// The `state.reason` of the ACTION that was under way when that halt reached it. See
/// [`HALTED_BY_STOP_CONDITION`].
const ACTION_WAS_STOPPED: &str = "Action was stopped.";

/// One emulator carrying FIS and CloudWatch over ONE shared engine, as the production
/// router builds them.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine),
    );
    router
}

/// One CloudWatch call (`awsJson1_0` over `POST /`), requiring a 200.
async fn cloudwatch(router: &MultiServiceRouter, operation: &str, body: &Value) -> Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{CLOUDWATCH_SIGNING_NAME}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header(
            "x-amz-target",
            format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        )
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/\
                 {CLOUDWATCH_SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=abc"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = response.into_body();
    let parsed: Value = if payload.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&payload).unwrap_or(Value::Null)
    };
    assert_eq!(
        status, 200,
        "{operation}: expected HTTP 200 from CloudWatch, got {status} with {parsed}"
    );
    parsed
}

/// Create the metric alarm `name` and report the ARN CloudWatch MINTED for it, read back
/// through `DescribeAlarms` — so a stop condition names the ARN the store holds rather than
/// one this suite built.
async fn put_alarm(router: &MultiServiceRouter, name: &str) -> String {
    cloudwatch(
        router,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": "Sim2Real/Fis",
            "MetricName": "Errors",
            "Statistic": "Maximum",
            "Period": 60,
            "EvaluationPeriods": 1,
            "Threshold": 1.0,
            "ComparisonOperator": "GreaterThanThreshold",
            // No action: what is under test is the experiment FIS stops, not a notification.
            "ActionsEnabled": false,
            "TreatMissingData": "missing",
        }),
    )
    .await;
    // FIS documents that an experiment's stop-condition alarm must not already be in ALARM
    // when the experiment starts, so every row starts from the OK state.
    force_alarm(router, name, "OK").await;
    let described = cloudwatch(
        router,
        "DescribeAlarms",
        &json!({ "AlarmNames": [name] }),
    )
    .await;
    described["MetricAlarms"][0]["AlarmArn"]
        .as_str()
        .unwrap_or_else(|| panic!("alarm {name:?} reports no AlarmArn: {described}"))
        .to_string()
}

/// Drive the alarm `name` into `state`, the way a client does.
async fn force_alarm(router: &MultiServiceRouter, name: &str, state: &str) {
    cloudwatch(
        router,
        "SetAlarmState",
        &json!({
            "AlarmName": name,
            "StateValue": state,
            "StateReason": "driven by the stop-condition suite",
        }),
    )
    .await;
}

/// One row: what the template's stop conditions name, which state each of its alarms is
/// driven into once the experiment is running, and the terminal status that earns.
struct Row {
    case: &'static str,
    /// The state each alarm of this row is driven into after the run has begun. One alarm
    /// is created per entry.
    drive_into: &'static [&'static str],
    /// Whether the template's conditions watch an alarm at all, or carry the explicit
    /// `none` source instead.
    watches_alarms: bool,
    /// Whether the conditions name an alarm ARN this account holds no alarm for, beside
    /// whatever its own alarms are.
    watches_an_absent_alarm: bool,
    /// The terminal status the run reaches.
    expected: &'static str,
}

/// The rows the rule is asserted over.
fn rows() -> &'static [Row] {
    &[
        Row {
            case: "one-condition-driven-to-alarm",
            drive_into: &["ALARM"],
            watches_alarms: true,
            watches_an_absent_alarm: false,
            expected: "stopped",
        },
        Row {
            case: "one-condition-left-ok",
            drive_into: &["OK"],
            watches_alarms: true,
            watches_an_absent_alarm: false,
            expected: "completed",
        },
        Row {
            case: "one-condition-driven-to-insufficient-data",
            drive_into: &["INSUFFICIENT_DATA"],
            watches_alarms: true,
            watches_an_absent_alarm: false,
            expected: "completed",
        },
        Row {
            case: "two-conditions-the-second-breaches",
            drive_into: &["OK", "ALARM"],
            watches_alarms: true,
            watches_an_absent_alarm: false,
            expected: "stopped",
        },
        Row {
            case: "the-source-none-condition-beside-an-alarming-alarm",
            drive_into: &["ALARM"],
            watches_alarms: false,
            watches_an_absent_alarm: false,
            expected: "completed",
        },
        Row {
            case: "a-condition-naming-an-alarm-this-account-does-not-hold",
            drive_into: &[],
            watches_alarms: true,
            watches_an_absent_alarm: true,
            expected: "completed",
        },
    ]
}

/// The `stopConditions` of one row's template, over the ARNs its alarms were minted under.
fn stop_conditions(row: &Row, alarm_arns: &[String]) -> Value {
    if !row.watches_alarms {
        return json!([{ "source": SOURCE_NONE }]);
    }
    let mut conditions: Vec<Value> = alarm_arns
        .iter()
        .map(|arn| json!({ "source": SOURCE_ALARM, "value": arn }))
        .collect();
    if row.watches_an_absent_alarm {
        conditions.push(json!({ "source": SOURCE_ALARM, "value": ABSENT_ALARM_ARN }));
    }
    if conditions.is_empty() {
        // A template declares at least one stop condition, so a row that watches no alarm
        // of its own carries the explicit "no stop condition" one.
        return json!([{ "source": SOURCE_NONE }]);
    }
    Value::Array(conditions)
}

/// Poll `GetExperiment` until the experiment reports `running`, and report the status it
/// was last read in.
async fn poll_until_running(router: &MultiServiceRouter, id: &str) -> String {
    const INTERVAL: Duration = Duration::from_millis(25);
    let deadline = std::time::Instant::now() + RUNNING_BUDGET;
    loop {
        let experiment = get_experiment(router, id).await.member("experiment").clone();
        let status = experiment["state"]["status"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if status != "initiating" || std::time::Instant::now() >= deadline {
            return status;
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

#[tokio::test]
async fn an_alarming_stop_condition_halts_the_run_and_nothing_else_does() {
    let router = emulator();

    // The rows are started in groups no larger than the account's active-experiment quota,
    // so the runs inside a group overlap instead of being waited out one after another.
    for (group_index, group) in rows().chunks(ACTIVE_EXPERIMENTS_QUOTA).enumerate() {
        let mut started: Vec<(&Row, String, Vec<String>)> = Vec::new();
        for (index, row) in group.iter().enumerate() {
            // One alarm per entry of the row, each under a name of its own.
            let mut alarm_arns = Vec::new();
            for (alarm_index, _) in row.drive_into.iter().enumerate() {
                let name = format!("fis-stop-{group_index}-{index}-{alarm_index}");
                alarm_arns.push(put_alarm(&router, &name).await);
            }

            let created = create_template(
                &router,
                json!({
                    "clientToken": token(&format!("stop-condition-template-{}", row.case)),
                    "description": format!("stop condition: {}", row.case),
                    "roleArn": "arn:aws:iam::000000000000:role/fis-stop-conditions",
                    "stopConditions": stop_conditions(row, &alarm_arns),
                    "targets": {},
                    "actions": {
                        "wait": {
                            "actionId": "aws:fis:wait",
                            "parameters": { "duration": WAIT_DURATION }
                        }
                    }
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {:?}: expected the template to be created, got {} with {:?}",
                row.case, created.status, created.body
            );
            let template_id = created.member("experimentTemplate")["id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {:?}: no template id: {:?}", row.case, created.body))
                .to_string();

            let experiment = start_experiment(
                &router,
                json!({
                    "clientToken": token(&format!("stop-condition-start-{}", row.case)),
                    "experimentTemplateId": template_id
                }),
            )
            .await;
            assert_eq!(
                experiment.status, 200,
                "case {:?}: expected the start to be accepted, got {} with {:?}",
                row.case, experiment.status, experiment.body
            );
            let id = experiment.member("experiment")["id"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {:?}: no experiment id: {:?}", row.case, experiment.body)
                })
                .to_string();

            // The alarm is driven only once the run has BEGUN, which is the window a stop
            // condition is watched in.
            let status = poll_until_running(&router, &id).await;
            assert_eq!(
                status, "running",
                "case {:?}: expected the experiment to be running before the alarm is \
                 driven, got {status:?}",
                row.case
            );
            for (alarm_index, state) in row.drive_into.iter().enumerate() {
                let name = format!("fis-stop-{group_index}-{index}-{alarm_index}");
                force_alarm(&router, &name, state).await;
            }
            started.push((row, id, alarm_arns));
        }

        for (row, id, alarm_arns) in started {
            let experiment = poll_until_final(&router, &id, TERMINAL_BUDGET).await;
            let status = experiment["state"]["status"].as_str().unwrap_or_default();
            assert_eq!(
                status, row.expected,
                "case {:?}: expected the run to end {:?} with no StopExperiment call, \
                 got {experiment:?}",
                row.case, row.expected
            );
            // Whatever ended it, the run reports when it ended.
            assert!(
                experiment["endTime"].is_number(),
                "case {:?}: a run that ended reports an endTime, got {experiment:?}",
                row.case
            );
            let reason = experiment["state"]["reason"].as_str().unwrap_or_default();
            if row.expected == "stopped" {
                // FIS's own sentence for a run one of its stop conditions halted: the WHOLE
                // of the reason, with nothing of the condition, the alarm or the account in
                // it — so every row that is halted reads exactly the same, however its
                // alarms are named and however many conditions it carries.
                assert_eq!(
                    reason, HALTED_BY_STOP_CONDITION,
                    "case {:?}: expected the halted run's reason to be \
                     {HALTED_BY_STOP_CONDITION:?}, got {reason:?}",
                    row.case
                );
                // The action that was under way when the halt reached it reports the halt
                // the same way: FIS's own sentence for a stopped action.
                let action = experiment["actions"]["wait"].clone();
                assert_eq!(
                    action["state"]["status"].as_str().unwrap_or_default(),
                    "stopped",
                    "case {:?}: expected the action under way to be stopped, got {action:?}",
                    row.case
                );
                assert_eq!(
                    action["state"]["reason"].as_str().unwrap_or_default(),
                    ACTION_WAS_STOPPED,
                    "case {:?}: expected the stopped action's reason to be \
                     {ACTION_WAS_STOPPED:?}, got {action:?}",
                    row.case
                );
            } else {
                assert!(
                    !reason.is_empty(),
                    "case {:?}: a run that ended says why, got {experiment:?}",
                    row.case
                );
                assert_ne!(
                    reason, HALTED_BY_STOP_CONDITION,
                    "case {:?}: a run no condition halted must not report a halt, got \
                     {reason:?}",
                    row.case
                );
            }
            // Whatever ended the run, its reason carries no identity of an alarm: a state
            // reason is FIS's own sentence, not a report of the resource it watched.
            for arn in &alarm_arns {
                assert!(
                    !reason.contains(arn),
                    "case {:?}: a state reason must name no alarm, got {reason:?}",
                    row.case
                );
            }
            for arn in alarm_arns.iter().chain([&ABSENT_ALARM_ARN.to_string()]) {
                let named = arn.rsplit(':').next().unwrap_or_default();
                assert!(
                    !named.is_empty() && !reason.contains(named),
                    "case {:?}: a state reason must name no alarm, got {reason:?}",
                    row.case
                );
            }
        }
    }
}
