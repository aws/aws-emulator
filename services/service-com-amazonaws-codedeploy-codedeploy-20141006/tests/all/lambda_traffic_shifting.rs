// SPDX-License-Identifier: CC0-1.0

//! The TIME-BASED traffic shift a Lambda deployment performs, and what an ALARM
//! interrupting one does — end to end over the wire, with Lambda and CloudWatch
//! mounted beside CodeDeploy on one engine.
//!
//! `tests/all/lambda_traffic_shift.rs` holds the rule that a deployment carries
//! out the shift its AppSpec declares at all; this suite is about the INCREMENTS
//! that shift is carried out in and the window they hold open:
//!
//! * a deployment shifts traffic in the increments its resolved
//!   `trafficRoutingConfig` describes — a `TimeBasedCanary` in two, a
//!   `TimeBasedLinear` in equal ones, `AllAtOnce` in a single step — and while an
//!   increment other than the last holds, the alias its AppSpec names keeps
//!   invoking the version the deployment is shifting from with that increment's
//!   share of the traffic weighted onto the target version; the deployment stays
//!   non-terminal for the whole window and the last increment moves the alias over
//!   with no weighted routing left behind;
//! * an alarm the deployment's effective `alarmConfiguration` names that starts
//!   sounding WHILE that window is open stops the deployment (`Stopped`, with
//!   `errorInformation.code` `ALARM_ACTIVE`), and when the effective
//!   `autoRollbackConfiguration` names `DEPLOYMENT_STOP_ON_ALARM` the rollback
//!   deployment it creates puts the alias back on the version the stopped
//!   deployment started from.
//!
//! Every alias here is created through `CreateAlias` and read through `GetAlias`,
//! and every alarm through `PutMetricAlarm`/`SetAlarmState`: the operations a
//! client calls, so nothing reaches into any service's store.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{terminal_deployment_within, Emulator};

/// The service role every deployment group here is created with.
const SERVICE_ROLE: &str = "arn:aws:iam::123456789012:role/CodeDeployRole";

/// The execution role every function here is created with.
const EXECUTION_ROLE: &str = "arn:aws:iam::123456789012:role/LambdaExecution";

/// The alias every deployment here shifts traffic on.
const ALIAS: &str = "live";

/// How long a deployment that walks a traffic-shift window is given to reach a
/// terminal status, and how often the alias is sampled while it does.
///
/// Long enough for the window of every configuration below (each states a
/// one-minute interval, so each increment is held for this emulator's own span per
/// configured minute) and for the increments of the longest ladder among them.
const WINDOW_DEADLINE: Duration = Duration::from_secs(30);
/// See [`WINDOW_DEADLINE`]: fine enough to observe every increment of a shift
/// whose increments are held for whole seconds.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(40);

/// The `ErrorCode` a deployment its alarms stopped reports
/// (`smithy_query(query="shape",shape="ErrorCode")` carries it).
const ALARM_ACTIVE: &str = "ALARM_ACTIVE";

/// The `DeploymentCreator` of a deployment CodeDeploy created to roll another one
/// back (`smithy_query(query="shape",shape="DeploymentCreator")`).
const CREATOR_ROLLBACK: &str = "codeDeployRollback";

/// What one alias reports at an instant: the version it invokes, and the weights
/// its routing configuration carries.
type AliasReading = (String, Vec<(String, f64)>);

/// Create a function named `name` and publish `versions` versions of it, then
/// point a `live` alias at version 1.
///
/// The deployment package is a blob uploaded inline: nothing here invokes the
/// function, so what the archive holds is immaterial — only the VERSIONS it is
/// published as are, because those are what an alias and a traffic shift name.
async fn function_with_versions(emulator: &Emulator, name: &str, versions: usize) {
    emulator
        .lambda_ok(
            "CreateFunction",
            "POST",
            "/2015-03-31/functions",
            json!({
                "FunctionName": name,
                "Role": EXECUTION_ROLE,
                "Runtime": "python3.12",
                "Handler": "index.handler",
                "Code": {"ZipFile": "cGFja2FnZQ=="},
            }),
            201,
        )
        .await;
    wait_until_active(emulator, name).await;
    for version in 1..=versions {
        emulator
            .lambda_ok(
                "PublishVersion",
                "POST",
                &format!("/2015-03-31/functions/{name}/versions"),
                // A description per version: "Lambda doesn't publish a version if
                // the function's configuration and code haven't changed since the
                // last version", and the description is part of that
                // configuration, so each call publishes its own version.
                json!({"Description": format!("version {version}")}),
                201,
            )
            .await;
    }
    emulator
        .lambda_ok(
            "CreateAlias",
            "POST",
            &format!("/2015-03-31/functions/{name}/aliases"),
            json!({"Name": ALIAS, "FunctionVersion": "1"}),
            201,
        )
        .await;
}

/// Poll `GetFunctionConfiguration` until Lambda reports the function `Active`: a
/// freshly created function is owed that transition before a version of it can be
/// published.
async fn wait_until_active(emulator: &Emulator, name: &str) {
    let deadline = Instant::now() + WINDOW_DEADLINE;
    loop {
        let reported = emulator
            .lambda_ok(
                "GetFunctionConfiguration",
                "GET",
                &format!("/2015-03-31/functions/{name}/configuration"),
                Value::Null,
                200,
            )
            .await;
        if reported.get("State").and_then(Value::as_str) == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "setup: Lambda never reported {name:?} Active, last {reported:?}"
        );
        tokio::time::sleep(SAMPLE_INTERVAL).await;
    }
}

/// What the alias of `function` reports right now.
async fn alias_reading(emulator: &Emulator, function: &str) -> AliasReading {
    let reported = emulator
        .lambda_ok(
            "GetAlias",
            "GET",
            &format!("/2015-03-31/functions/{function}/aliases/{ALIAS}"),
            Value::Null,
            200,
        )
        .await;
    let version = reported
        .get("FunctionVersion")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let mut weights: Vec<(String, f64)> = reported
        .get("RoutingConfig")
        .and_then(|routing| routing.get("AdditionalVersionWeights"))
        .and_then(Value::as_object)
        .map(|weights| {
            weights
                .iter()
                .map(|(version, weight)| {
                    (version.clone(), weight.as_f64().unwrap_or_default())
                })
                .collect()
        })
        .unwrap_or_default();
    weights.sort_by(|left, right| left.0.cmp(&right.0));
    (version, weights)
}

/// An `appSpecContent` revision shifting `alias` of `function` from
/// `current_version` to `target_version`, in the AppSpec reference's own spelling.
fn appspec_revision(
    function: &str,
    alias: &str,
    current_version: &str,
    target_version: &str,
) -> Value {
    let content = format!(
        "version: 0.0\nResources:\n  - {function}:\n      Type: AWS::Lambda::Function\n      \
         Properties:\n        Name: {function}\n        Alias: {alias}\n        \
         CurrentVersion: \"{current_version}\"\n        TargetVersion: \"{target_version}\"\n"
    );
    json!({"revisionType": "AppSpecContent", "appSpecContent": {"content": content}})
}

/// A `trafficRoutingConfig` of `kind` shifting `percentage` every `interval`
/// minutes; `AllAtOnce` carries neither increment configuration.
fn traffic_routing(kind: &str, percentage: i64, interval: i64) -> Value {
    match kind {
        "TimeBasedCanary" => json!({
            "type": kind,
            "timeBasedCanary": {"canaryPercentage": percentage, "canaryInterval": interval},
        }),
        "TimeBasedLinear" => json!({
            "type": kind,
            "timeBasedLinear": {"linearPercentage": percentage, "linearInterval": interval},
        }),
        _ => json!({"type": kind}),
    }
}

/// Create a deployment configuration of `name` carrying `routing`, an application
/// and one Lambda deployment group of it carrying `extras`, answering the group's
/// name.
async fn lambda_group(
    emulator: &Emulator,
    application: &str,
    routing: Option<Value>,
    extras: &Value,
) -> String {
    let config = match routing {
        Some(routing) => {
            let name = format!("{application}-config");
            emulator
                .ok(
                    "CreateDeploymentConfig",
                    json!({
                        "deploymentConfigName": name,
                        "computePlatform": "Lambda",
                        "trafficRoutingConfig": routing,
                    }),
                )
                .await;
            name
        }
        None => "CodeDeployDefault.LambdaAllAtOnce".to_string(),
    };
    emulator
        .ok(
            "CreateApplication",
            json!({"applicationName": application, "computePlatform": "Lambda"}),
        )
        .await;
    let group = format!("{application}-dg");
    let mut request = json!({
        "applicationName": application,
        "deploymentGroupName": group,
        "serviceRoleArn": SERVICE_ROLE,
        "deploymentConfigName": config,
    });
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        request[member] = value;
    }
    emulator.ok("CreateDeploymentGroup", request).await;
    group
}

/// Create a deployment of `revision`, answering the id CodeDeploy minted.
async fn create_deployment(
    emulator: &Emulator,
    application: &str,
    group: &str,
    revision: &Value,
) -> String {
    emulator
        .ok(
            "CreateDeployment",
            json!({
                "applicationName": application,
                "deploymentGroupName": group,
                "revision": revision,
            }),
        )
        .await["deploymentId"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Sample the alias of `function` until `deployment` reaches a terminal status,
/// answering every distinct reading observed in order, and that terminal
/// `deploymentInfo`.
async fn readings_until_terminal(
    emulator: &Emulator,
    function: &str,
    deployment: &str,
    case: &str,
) -> (Vec<AliasReading>, Value) {
    let deadline = Instant::now() + WINDOW_DEADLINE;
    let mut observed: Vec<AliasReading> = Vec::new();
    loop {
        let reading = alias_reading(emulator, function).await;
        if observed.last() != Some(&reading) {
            observed.push(reading);
        }
        let info = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment}))
            .await["deploymentInfo"]
            .clone();
        if matches!(
            info["status"].as_str(),
            Some("Succeeded" | "Failed" | "Stopped")
        ) {
            // One last reading: the final increment is written in the same pass
            // that settles the deployment.
            let reading = alias_reading(emulator, function).await;
            if observed.last() != Some(&reading) {
                observed.push(reading);
            }
            return (observed, info);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a terminal status within {WINDOW_DEADLINE:?}, got {:?} \
             with the alias at {:?}",
            info["status"],
            observed.last()
        );
        tokio::time::sleep(SAMPLE_INTERVAL).await;
    }
}

/// RULE: stopping a deployment mid-shift ENDS its traffic shift where it stands,
/// and the rollback the caller asks for with it puts the alias back the way that
/// deployment found it — on the version its AppSpec was shifting from, with the
/// weighted routing the shift added cleared; a stop that asks for no rollback
/// leaves the alias on the increment that was in force.
#[tokio::test]
async fn a_stop_that_rolls_back_restores_the_alias_the_shift_had_moved() {
    // Rows differ along whether the stop asks for the rollback at all, which is
    // what decides whether the traffic that had moved goes back.
    let cases: &[(&str, Option<bool>, AliasReading)] = &[
        (
            "a stop that asks for a rollback",
            Some(true),
            ("1".to_string(), Vec::new()),
        ),
        (
            "a stop that asks for none",
            Some(false),
            ("1".to_string(), vec![("2".to_string(), 0.1)]),
        ),
        (
            "a stop that says nothing about a rollback",
            None,
            ("1".to_string(), vec![("2".to_string(), 0.1)]),
        ),
    ];

    for (index, (case, auto_rollback, expected)) in cases.iter().enumerate() {
        let emulator = Emulator::with_lambda();
        let function = format!("stop-fn-{index}");
        let application = format!("stop-app-{index}");
        function_with_versions(&emulator, &function, 2).await;
        // A canary whose interval is long enough that the window is comfortably
        // open while the stop is made: the SECOND increment is what the stop has
        // to come before, and nothing here measures how long the window is.
        let group = lambda_group(
            &emulator,
            &application,
            Some(traffic_routing("TimeBasedCanary", 10, 5)),
            &json!({}),
        )
        .await;
        let deployment = create_deployment(
            &emulator,
            &application,
            &group,
            &appspec_revision(&function, ALIAS, "1", "2"),
        )
        .await;

        // The canary increment is on the alias: the deployment has moved traffic,
        // which is what the stop then has to deal with.
        let canary = ("1".to_string(), vec![("2".to_string(), 0.1)]);
        let deadline = Instant::now() + WINDOW_DEADLINE;
        loop {
            if alias_reading(&emulator, &function).await == canary {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the canary increment never reached the alias"
            );
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }

        let mut request = json!({"deploymentId": deployment});
        if let Some(enabled) = auto_rollback {
            request["autoRollbackEnabled"] = json!(enabled);
        }
        emulator.ok("StopDeployment", request).await;

        let info = emulator
            .ok("GetDeployment", json!({"deploymentId": deployment}))
            .await["deploymentInfo"]
            .clone();
        assert_eq!(
            info["status"].as_str(),
            Some("Stopped"),
            "case {case:?}: expected the stop to end the deployment, got {info:?}"
        );
        // The stop has taken effect, so the alias holds what it holds: no further
        // increment of a terminal deployment can move it.
        let reading = alias_reading(&emulator, &function).await;
        assert_eq!(
            &reading, expected,
            "case {case:?}: expected the alias at {expected:?} after the stop, got {reading:?}"
        );
    }
}

/// RULE: a deployment shifts traffic in the increments its resolved
/// `trafficRoutingConfig` describes — each one but the last holding the version
/// the deployment started from with that increment's share weighted onto the
/// target version — and only the last increment moves the alias over and clears
/// the weighted routing.
#[tokio::test]
async fn a_deployment_walks_its_alias_through_the_increments_its_configuration_describes() {
    /// One configuration, and the weight ladder the alias must walk under it.
    struct Case {
        case: &'static str,
        /// `trafficRoutingConfig.type`, or `None` for the built-in that shifts
        /// all traffic at once.
        routing: Option<(&'static str, i64)>,
        /// The weights on the TARGET version, in order, before the cutover.
        ladder: Vec<f64>,
    }

    let cases = [
        Case {
            case: "a 10% canary",
            routing: Some(("TimeBasedCanary", 10)),
            ladder: vec![0.1],
        },
        Case {
            case: "a 75% canary",
            routing: Some(("TimeBasedCanary", 75)),
            ladder: vec![0.75],
        },
        Case {
            case: "a 25% linear shift",
            routing: Some(("TimeBasedLinear", 25)),
            ladder: vec![0.25, 0.5, 0.75],
        },
        Case {
            case: "a 50% linear shift",
            routing: Some(("TimeBasedLinear", 50)),
            ladder: vec![0.5],
        },
        Case {
            case: "the built-in that shifts all traffic at once",
            routing: None,
            ladder: Vec::new(),
        },
    ];

    for (index, Case {
        case,
        routing,
        ladder,
    }) in cases.iter().enumerate()
    {
        let emulator = Emulator::with_lambda_and_cloudwatch();
        let function = format!("shift-fn-{index}");
        let application = format!("shift-app-{index}");
        function_with_versions(&emulator, &function, 2).await;
        let group = lambda_group(
            &emulator,
            &application,
            routing.map(|(kind, percentage)| traffic_routing(kind, percentage, 1)),
            &Value::Null,
        )
        .await;
        let deployment = create_deployment(
            &emulator,
            &application,
            &group,
            &appspec_revision(&function, ALIAS, "1", "2"),
        )
        .await;

        let (observed, info) =
            readings_until_terminal(&emulator, &function, &deployment, case).await;
        assert_eq!(
            info["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: a shift nothing stopped must succeed, got {info:?}"
        );

        // What the alias passed through: the version the deployment started from
        // under each increment's weight, then the target version with none. The
        // first reading is taken as the deployment is accepted, before the clock
        // owes it its start, so an unweighted reading of the starting version
        // there is the state it is shifting FROM rather than an increment.
        let starting: AliasReading = ("1".to_string(), Vec::new());
        let walked = match observed.split_first() {
            Some((first, rest)) if *first == starting => rest.to_vec(),
            _ => observed.clone(),
        };
        let mut expected: Vec<AliasReading> = Vec::new();
        for weight in ladder {
            expected.push(("1".to_string(), vec![("2".to_string(), *weight)]));
        }
        expected.push(("2".to_string(), Vec::new()));
        assert_eq!(
            walked, expected,
            "case {case:?}: expected the alias to walk {expected:?}, got {observed:?}"
        );
    }
}

/// RULE: an alarm that starts sounding while a deployment's traffic-shift window
/// is open stops that deployment — `Stopped`, with `errorInformation.code`
/// `ALARM_ACTIVE` — and the rollback its `DEPLOYMENT_STOP_ON_ALARM` configuration
/// asks for puts the alias back the way it was before the stopped deployment
/// moved it; the same deployment under an alarm that stays quiet succeeds and
/// leaves the alias on the target version.
#[tokio::test]
async fn an_alarm_sounding_mid_shift_stops_the_deployment_and_its_rollback_restores_the_alias() {
    for (index, (case, breaches)) in [
        ("the alarm starts sounding mid-shift", true),
        ("the alarm stays quiet", false),
    ]
    .iter()
    .enumerate()
    {
        let emulator = Emulator::with_lambda_and_cloudwatch();
        let function = format!("gate-fn-{index}");
        let application = format!("gate-app-{index}");
        let alarm = format!("gate-alarm-{index}");
        function_with_versions(&emulator, &function, 3).await;
        emulator
            .cloudwatch_ok(
                "PutMetricAlarm",
                json!({
                    "AlarmName": alarm,
                    "Namespace": "Sim/CodeDeployShift",
                    "MetricName": "Errors",
                    "Statistic": "Sum",
                    "Period": 60,
                    "EvaluationPeriods": 1,
                    "Threshold": 1.0,
                    "ComparisonOperator": "GreaterThanOrEqualToThreshold",
                    "TreatMissingData": "notBreaching",
                }),
            )
            .await;
        emulator
            .cloudwatch_ok(
                "SetAlarmState",
                json!({"AlarmName": alarm, "StateValue": "OK", "StateReason": "the control leg"}),
            )
            .await;
        let group = lambda_group(
            &emulator,
            &application,
            Some(traffic_routing("TimeBasedCanary", 10, 1)),
            &json!({
                "alarmConfiguration": {
                    "enabled": true,
                    "alarms": [{"name": alarm}],
                },
                "autoRollbackConfiguration": {
                    "enabled": true,
                    "events": ["DEPLOYMENT_STOP_ON_ALARM"],
                },
            }),
        )
        .await;

        // The leg that gives the group a revision to go back to, with the alarm
        // quiet: it moves the alias to version 2.
        let control = create_deployment(
            &emulator,
            &application,
            &group,
            &appspec_revision(&function, ALIAS, "1", "2"),
        )
        .await;
        let control_info =
            terminal_deployment_within(&emulator, &control, case, WINDOW_DEADLINE).await;
        assert_eq!(
            control_info["status"].as_str(),
            Some("Succeeded"),
            "case {case:?}: the control leg must succeed, got {control_info:?}"
        );
        assert_eq!(
            alias_reading(&emulator, &function).await,
            ("2".to_string(), Vec::new()),
            "case {case:?}: the control leg must leave the alias on its target version"
        );

        // The leg under test: 2 -> 3, with the alarm driven to ALARM once the
        // canary increment is on the alias.
        let shifting = create_deployment(
            &emulator,
            &application,
            &group,
            &appspec_revision(&function, ALIAS, "2", "3"),
        )
        .await;
        let canary = ("2".to_string(), vec![("3".to_string(), 0.1)]);
        let deadline = Instant::now() + WINDOW_DEADLINE;
        loop {
            if alias_reading(&emulator, &function).await == canary {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the canary increment never reached the alias"
            );
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
        if *breaches {
            emulator
                .cloudwatch_ok(
                    "SetAlarmState",
                    json!({
                        "AlarmName": alarm,
                        "StateValue": "ALARM",
                        "StateReason": "the breach under test",
                    }),
                )
                .await;
        }

        let info = terminal_deployment_within(&emulator, &shifting, case, WINDOW_DEADLINE).await;
        if !*breaches {
            assert_eq!(
                info["status"].as_str(),
                Some("Succeeded"),
                "case {case:?}: a quiet alarm stops nothing, got {info:?}"
            );
            assert_eq!(
                alias_reading(&emulator, &function).await,
                ("3".to_string(), Vec::new()),
                "case {case:?}: the completed shift must leave the alias on version 3"
            );
            continue;
        }

        assert_eq!(
            (
                info["status"].as_str(),
                info["errorInformation"]["code"].as_str()
            ),
            (Some("Stopped"), Some(ALARM_ACTIVE)),
            "case {case:?}: a sounding alarm must stop the deployment, got {info:?}"
        );
        assert!(
            info["errorInformation"]["message"]
                .as_str()
                .unwrap_or_default()
                .contains(&alarm),
            "case {case:?}: the stop must name the activated alarm, got {:?}",
            info["errorInformation"]["message"]
        );
        let rollback = info["rollbackInfo"]["rollbackDeploymentId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !rollback.is_empty(),
            "case {case:?}: a stop that rolls back names its rollback deployment, got {info:?}"
        );
        let rollback_info =
            terminal_deployment_within(&emulator, &rollback, case, WINDOW_DEADLINE).await;
        assert_eq!(
            (
                rollback_info["creator"].as_str(),
                rollback_info["rollbackInfo"]["rollbackTriggeringDeploymentId"].as_str()
            ),
            (Some(CREATOR_ROLLBACK), Some(shifting.as_str())),
            "case {case:?}: the rollback is CodeDeploy's own and points back at the stopped \
             deployment, got {rollback_info:?}"
        );

        // And the alias is back where the stopped deployment found it.
        let deadline = Instant::now() + WINDOW_DEADLINE;
        loop {
            let reading = alias_reading(&emulator, &function).await;
            if reading == ("2".to_string(), Vec::new()) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the rollback must restore the alias to version 2 with no \
                 weights, last {reading:?}"
            );
            tokio::time::sleep(SAMPLE_INTERVAL).await;
        }
    }
}

