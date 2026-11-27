// SPDX-License-Identifier: CC0-1.0

//! A PUBLISHED metric breach drives the Auto Scaling policy an alarm's action names — over the
//! WHOLE mounted service set, the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **a metric alarm's `StateValue` is computed from the datapoints
//! published for the metric it watches, so datapoints that satisfy its `ComparisonOperator`
//! against `Threshold` over its evaluation periods move it into `ALARM` on their own — no
//! caller forces the state — and that transition executes the actions configured for `ALARM`.
//! Where one of them names an Auto Scaling scaling policy, CloudWatch records the execution as
//! an `Action` history item naming the policy and hands the transition to the service that owns
//! it, which performs the policy: `DesiredCapacity` moves by the policy's `ScalingAdjustment`,
//! the group converges on that capacity by launching an Amazon EC2 instance that reaches
//! `InService`, and the scaling activity it records names the alarm and the policy. Datapoints
//! that do NOT satisfy the comparison move the alarm to `OK` instead and nothing is
//! performed.**
//!
//! The group here states no `DefaultCooldown` and the policy states a `Cooldown` of its own, as
//! a client wiring metric-driven scale-out writes them: the period in force for the
//! alarm-driven execution is the policy's own, which "overrides the default cooldown"
//! (`PutScalingPolicyType$Cooldown`) — pinned over its own table by
//! `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/tests/all/policy_cooldown_period_in_force.rs`.
//!
//! Three services have to agree for that to hold: CloudWatch owns the metric store, the
//! evaluation and the action dispatch, Amazon EC2 Auto Scaling owns the policy, the group and
//! its capacity, and Amazon EC2 holds the launch template and the instances. The suite beside
//! this one (`autoscaling_alarm_action_executes_its_scaling_policy.rs`) drives the same
//! hand-off from a FORCED state; what only this one reaches is the evaluation that makes a real
//! breach fire it, which is what a client wiring metric-driven scale-out actually does.
//!
//! Every call is made over the `awsQuery`/`ec2Query` form wire a stock SDK client uses for
//! these three services.
//!
//! The rows differ along what the rule generalises over: the alarm's `Period`, its `Statistic`,
//! its `ComparisonOperator`, how many datapoints are published, the adjustment type and the
//! sign of the adjustment — and, in the second test, whether the published series breaches at
//! all.

use std::time::{Duration, Instant};

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{authorization, router};
use crate::query_wire::{block_of, element, elements, form_encode, members_of};

/// The Region the whole wiring of a row lives in: an alarm's actions are executed in the Region
/// the alarm lives in, so the alarm, the policy, the group and its instances share one.
const REGION: &str = "us-west-2";

/// The API versions an `awsQuery` and an `ec2Query` client sends for the three services.
const ASG_VERSION: &str = "2011-01-01";
const EC2_VERSION: &str = "2016-11-15";
const MONITORING_VERSION: &str = "2010-08-01";

/// CloudWatch's signing name (`sigv4Name: monitoring`).
const MONITORING_SIGNING_NAME: &str = "monitoring";

/// The AMI and instance type every launch template below names.
const IMAGE_ID: &str = "ami-0abcdef1234567890";
const INSTANCE_TYPE: &str = "t3.micro";

/// The namespace and the dimension name the rows publish their series under.
const NAMESPACE: &str = "sim2real/autoscaling";
const DIMENSION_NAME: &str = "AutoScalingGroupName";

/// How long a wait for a server-side effect looks for it before calling it undelivered, and how
/// often it looks again.
///
/// A BACKSTOP, not a schedule: every wait below polls for the CONDITION it needs — a state the
/// alarm reports, an action CloudWatch recorded, a capacity the group reports, an instance that
/// reached service — so a loaded machine delays a row instead of breaking it. The deadline is
/// generous beside the longest span a row waits on (one alarm period, then the launch).
const SETTLE_DEADLINE: Duration = Duration::from_secs(90);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// One form-wire call against the service whose signing name is `signing_name`, answered as
/// `(status, body)`.
async fn form_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in params {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization(signing_name, REGION))
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(response.body()).to_string();
    (status, text)
}

/// One call that must answer 200.
async fn ok_call(
    router: &MultiServiceRouter,
    case: &str,
    signing_name: &str,
    version: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = form_call(router, signing_name, version, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {signing_name} {action}, got {status} with {body}"
    );
    body
}

/// One Amazon EC2 Auto Scaling call that must answer 200.
async fn asg(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    ok_call(router, case, "autoscaling", ASG_VERSION, action, params).await
}

/// One Amazon EC2 call that must answer 200.
async fn ec2(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    ok_call(router, case, "ec2", EC2_VERSION, action, params).await
}

/// One CloudWatch call that must answer 200.
async fn cw(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    ok_call(
        router,
        case,
        MONITORING_SIGNING_NAME,
        MONITORING_VERSION,
        action,
        params,
    )
    .await
}

/// One parameter pair.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// The owned parameter pairs of a request, as the borrowed pairs the wire helper takes.
fn borrowed(params: &[(String, String)]) -> Vec<(&str, &str)> {
    params
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect()
}

/// Create the launch template the group is built on and the group itself, at `desired` capacity
/// between `bounds`.
async fn given_group(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    bounds: (i32, i32),
    desired: i32,
) {
    let template = format!("{group}-template");
    ec2(
        router,
        case,
        "CreateLaunchTemplate",
        &[
            ("LaunchTemplateName", template.as_str()),
            ("LaunchTemplateData.ImageId", IMAGE_ID),
            ("LaunchTemplateData.InstanceType", INSTANCE_TYPE),
        ],
    )
    .await;

    // The group states no `DefaultCooldown`, so it carries the documented default
    // ("Default: `300` seconds", `CreateAutoScalingGroupType$DefaultCooldown`) — which is what
    // a client wiring metric-driven scale-out creates, and which the policy's own `Cooldown`
    // below overrides.
    let (min_size, max_size) = bounds;
    let create = vec![
        p("AutoScalingGroupName", group),
        p("LaunchTemplate.LaunchTemplateName", template),
        p("MinSize", min_size.to_string()),
        p("MaxSize", max_size.to_string()),
        p("DesiredCapacity", desired.to_string()),
        p("AvailabilityZones.member.1", format!("{REGION}a")),
    ];
    asg(router, case, "CreateAutoScalingGroup", &borrowed(&create)).await;
}

/// Put one scaling policy on `group` and answer the `PolicyARN` the service minted.
async fn given_policy(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    policy: &str,
    adjustment_type: &str,
    scaling_adjustment: i32,
) -> String {
    let params = vec![
        p("AutoScalingGroupName", group),
        p("PolicyName", policy),
        p("PolicyType", "SimpleScaling"),
        p("AdjustmentType", adjustment_type),
        p("ScalingAdjustment", scaling_adjustment.to_string()),
        p("Cooldown", "0"),
    ];
    let body = asg(router, case, "PutScalingPolicy", &borrowed(&params)).await;
    let arn = element(&body, "PolicyARN");
    assert!(
        arn.contains(":scalingPolicy:"),
        "case {case:?}: PutScalingPolicy must report the policy's ARN, got {body}"
    );
    arn
}

/// Configure the alarm `name` over the series `group` names, with `action` in its
/// `AlarmActions`.
async fn given_alarm(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    group: &str,
    metric: &str,
    alarm: &AlarmShape<'_>,
    action: &str,
) {
    let params = vec![
        p("AlarmName", name),
        p("Namespace", NAMESPACE),
        p("MetricName", metric),
        p("Dimensions.member.1.Name", DIMENSION_NAME),
        p("Dimensions.member.1.Value", group),
        p("Statistic", alarm.statistic),
        p("Period", alarm.period_secs.to_string()),
        p("EvaluationPeriods", "1"),
        p("DatapointsToAlarm", "1"),
        p("Threshold", alarm.threshold.to_string()),
        p("ComparisonOperator", alarm.comparison),
        p("TreatMissingData", "notBreaching"),
        p("ActionsEnabled", "true"),
        p("AlarmActions.member.1", action),
    ];
    cw(router, case, "PutMetricAlarm", &borrowed(&params)).await;
}

/// Publish `values` as datapoints of the series `group` names, stamped by the service at the
/// instant they arrive — which is what a client publishing live observations sends.
async fn publish(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    metric: &str,
    values: &[f64],
) {
    let mut params = vec![p("Namespace", NAMESPACE)];
    for (index, value) in values.iter().enumerate() {
        let member = index + 1;
        params.push(p(&format!("MetricData.member.{member}.MetricName"), metric));
        params.push(p(
            &format!("MetricData.member.{member}.Dimensions.member.1.Name"),
            DIMENSION_NAME,
        ));
        params.push(p(
            &format!("MetricData.member.{member}.Dimensions.member.1.Value"),
            group,
        ));
        params.push(p(
            &format!("MetricData.member.{member}.Value"),
            value.to_string(),
        ));
        params.push(p(&format!("MetricData.member.{member}.Unit"), "Count"));
    }
    cw(router, case, "PutMetricData", &borrowed(&params)).await;
}

/// The `StateValue` the alarm `name` reports right now.
async fn alarm_state(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let body = cw(
        router,
        case,
        "DescribeAlarms",
        &[("AlarmNames.member.1", name)],
    )
    .await;
    elements(&body, "StateValue").into_iter().next().unwrap_or_default()
}

/// Publish the row's series again and again until the alarm `name` reports `wanted`, answering
/// whatever it reports last.
///
/// The republish is what a client watching a live metric does — the breach is not one datapoint
/// but a series — and the wait is for the CONDITION, so a machine that needs longer than one
/// evaluation period gets it.
async fn awaited_state(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    group: &str,
    metric: &str,
    values: &[f64],
    wanted: &str,
) -> String {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut reported = alarm_state(router, case, name).await;
    while Instant::now() < deadline {
        if reported == wanted {
            return reported;
        }
        publish(router, case, group, metric, values).await;
        tokio::time::sleep(LOOK_AGAIN).await;
        reported = alarm_state(router, case, name).await;
    }
    reported
}

/// The `Action` history items CloudWatch has recorded for the alarm `name` that name `action`,
/// as `(HistorySummary, HistoryData)` pairs.
async fn action_items(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    action: &str,
) -> Vec<(String, String)> {
    let body = cw(
        router,
        case,
        "DescribeAlarmHistory",
        &[("AlarmName", name), ("HistoryItemType", "Action")],
    )
    .await;
    let list = block_of(&body, "AlarmHistoryItems").unwrap_or_default();
    members_of(list)
        .into_iter()
        .filter_map(|member| {
            let summary = elements(member, "HistorySummary").into_iter().next()?;
            let data = elements(member, "HistoryData").into_iter().next()?;
            (summary.contains(action) || data.contains(action)).then_some((summary, data))
        })
        .collect()
}

/// Wait until CloudWatch has recorded at least one `Action` item naming `action`, and answer
/// them.
async fn awaited_action_items(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    action: &str,
) -> Vec<(String, String)> {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let items = action_items(router, case, name, action).await;
        if !items.is_empty() || Instant::now() >= deadline {
            return items;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The `DesiredCapacity` the group reports right now.
async fn desired_capacity(router: &MultiServiceRouter, case: &str, group: &str) -> i32 {
    let body = asg(
        router,
        case,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    element(&body, "DesiredCapacity")
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: DesiredCapacity must be a number, got {body}"))
}

/// Wait until the group reports `wanted` as its `DesiredCapacity`, and answer what it reports.
async fn awaited_capacity(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    wanted: i32,
) -> i32 {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    let mut reported = desired_capacity(router, case, group).await;
    while Instant::now() < deadline {
        if reported == wanted {
            return reported;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
        reported = desired_capacity(router, case, group).await;
    }
    reported
}

/// The instance ids the group reports in `LifecycleState` `state` right now.
async fn instances_in(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    state: &str,
) -> Vec<String> {
    let body = asg(
        router,
        case,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    let list = block_of(&body, "Instances").unwrap_or_default();
    members_of(list)
        .into_iter()
        .filter_map(|member| {
            let id = elements(member, "InstanceId").into_iter().next()?;
            let lifecycle = elements(member, "LifecycleState").into_iter().next()?;
            (lifecycle == state).then_some(id)
        })
        .collect()
}

/// Wait until the group reports at least `wanted` instances `InService`, and answer them.
async fn awaited_in_service(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    wanted: usize,
) -> Vec<String> {
    let deadline = Instant::now() + SETTLE_DEADLINE;
    loop {
        let found = instances_in(router, case, group, "InService").await;
        if found.len() >= wanted || Instant::now() >= deadline {
            return found;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// The state Amazon EC2 reports for the instance `id`, read inside that instance's own
/// `instanceState` block.
async fn ec2_state(router: &MultiServiceRouter, case: &str, id: &str) -> String {
    let body = ec2(router, case, "DescribeInstances", &[("InstanceId.1", id)]).await;
    block_of(&body, "instanceState")
        .and_then(|block| elements(block, "name").into_iter().next())
        .unwrap_or_default()
}

/// The causes of the scaling activities the group has recorded.
async fn activity_causes(router: &MultiServiceRouter, case: &str, group: &str) -> Vec<String> {
    let body = asg(
        router,
        case,
        "DescribeScalingActivities",
        &[("AutoScalingGroupName", group)],
    )
    .await;
    elements(&body, "Cause")
}

/// How an alarm of a row is shaped: the comparison its datapoints are graded by.
struct AlarmShape<'a> {
    /// "Valid values are 10, 20, 30, and any multiple of 60."
    /// (`smithy_query(query="doc",shape="PutMetricAlarmInput",member="Period")`) — the rows stay
    /// at the short end so a closed window comes quickly, and differ inside it.
    period_secs: i32,
    statistic: &'a str,
    comparison: &'a str,
    threshold: f64,
}

/// One row of the breaching table.
struct Row<'a> {
    case: &'a str,
    alarm: AlarmShape<'a>,
    /// The datapoints published for the series, which must satisfy the comparison.
    values: &'a [f64],
    /// The policy's `AdjustmentType` and `ScalingAdjustment`, and the capacity the group holds
    /// once the breach has driven it.
    adjustment_type: &'a str,
    scaling_adjustment: i32,
    expected_capacity: i32,
}

#[tokio::test]
async fn a_published_breach_drives_the_scaling_policy_the_alarm_action_names() {
    let rows: &[Row<'_>] = &[
        Row {
            case: "a maximum above the threshold adds the adjustment",
            alarm: AlarmShape {
                period_secs: 10,
                statistic: "Maximum",
                comparison: "GreaterThanThreshold",
                threshold: 50.0,
            },
            values: &[100.0],
            adjustment_type: "ChangeInCapacity",
            scaling_adjustment: 1,
            expected_capacity: 2,
        },
        Row {
            case: "an average under the threshold over a wider period sets the capacity",
            alarm: AlarmShape {
                period_secs: 20,
                statistic: "Average",
                comparison: "LessThanThreshold",
                threshold: 4.0,
            },
            values: &[1.0, 3.0, 2.0],
            adjustment_type: "ExactCapacity",
            scaling_adjustment: 3,
            expected_capacity: 3,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let router = router();
        let group = format!("breach-driven-group-{index}");
        let policy_name = format!("breach-driven-policy-{index}");
        let alarm = format!("breach-driven-alarm-{index}");
        let metric = format!("ScaleSignal{index}");

        given_group(&router, case, &group, (1, 3), 1).await;
        let first = awaited_in_service(&router, case, &group, 1).await;
        assert_eq!(
            first.len(),
            1,
            "case {case:?}: the group must bring its initial capacity into service, got {first:?}"
        );

        let policy = given_policy(
            &router,
            case,
            &group,
            &policy_name,
            row.adjustment_type,
            row.scaling_adjustment,
        )
        .await;
        given_alarm(
            &router, case, &alarm, &group, &metric, &row.alarm, &policy,
        )
        .await;

        // (a) the alarm reaches ALARM from the published series alone: nothing forces its
        // state.
        let state = awaited_state(
            &router, case, &alarm, &group, &metric, row.values, "ALARM",
        )
        .await;
        assert_eq!(
            state, "ALARM",
            "case {case:?}: the published breach must put the alarm in ALARM, got {state}"
        );

        // (b) the transition executed the action, and CloudWatch recorded it as succeeded.
        let items = awaited_action_items(&router, case, &alarm, &policy).await;
        assert!(
            !items.is_empty(),
            "case {case:?}: CloudWatch must record the action it executed for {policy}"
        );
        for (summary, data) in &items {
            assert!(
                summary.contains(&policy),
                "case {case:?}: the item's summary must name the policy, got {summary:?}"
            );
            assert!(
                data.contains("\"actionState\":\"Succeeded\""),
                "case {case:?}: the hand-over was accepted, so the item records it as \
                 succeeded, got {data}"
            );
            assert!(
                data.contains("\"stateValue\":\"ALARM\""),
                "case {case:?}: the item must name the state that fired the action, got {data}"
            );
        }

        // (c) the receiving service performed the policy.
        let capacity = awaited_capacity(&router, case, &group, row.expected_capacity).await;
        assert_eq!(
            capacity, row.expected_capacity,
            "case {case:?}: expected DesiredCapacity {}, got {capacity}",
            row.expected_capacity
        );

        // (d) the capacity is real: the group launched the instance and it reached service in
        // both services.
        let in_service = awaited_in_service(
            &router,
            case,
            &group,
            usize::try_from(row.expected_capacity).expect("a capacity is not negative"),
        )
        .await;
        assert_eq!(
            in_service.len(),
            usize::try_from(row.expected_capacity).expect("a capacity is not negative"),
            "case {case:?}: the group must bring the capacity it was moved to into service, got \
             {in_service:?}"
        );
        for id in &in_service {
            let state = ec2_state(&router, case, id).await;
            assert_eq!(
                state, "running",
                "case {case:?}: Amazon EC2 must report {id} running, got {state:?}"
            );
        }

        // (e) the activity names what drove it.
        let causes = activity_causes(&router, case, &group).await;
        assert!(
            causes
                .iter()
                .any(|cause| cause.contains(&policy_name) && cause.contains(&alarm)),
            "case {case:?}: an activity must name the alarm and the policy that moved the \
             capacity, got {causes:?}"
        );
    }
}

#[tokio::test]
async fn a_published_series_that_does_not_breach_performs_nothing() {
    let case = "a maximum under the threshold leaves the group where it is";
    let router = router();
    let group = "unbreached-group";
    let policy_name = "unbreached-policy";
    let alarm = "unbreached-alarm";
    let metric = "ScaleSignalQuiet";

    given_group(&router, case, group, (1, 3), 1).await;
    awaited_in_service(&router, case, group, 1).await;
    let policy = given_policy(&router, case, group, policy_name, "ChangeInCapacity", 1).await;
    given_alarm(
        &router,
        case,
        alarm,
        group,
        metric,
        &AlarmShape {
            period_secs: 10,
            statistic: "Maximum",
            comparison: "GreaterThanThreshold",
            threshold: 50.0,
        },
        &policy,
    )
    .await;

    // The same evaluation that would have fired the action puts the alarm in OK instead,
    // because the series does not satisfy the comparison.
    let state = awaited_state(&router, case, alarm, group, metric, &[7.0], "OK").await;
    assert_eq!(
        state, "OK",
        "case {case:?}: a series under the threshold puts the alarm in OK, got {state}"
    );

    let items = action_items(&router, case, alarm, &policy).await;
    assert!(
        items.is_empty(),
        "case {case:?}: no transition into ALARM happened, so no action was executed, got \
         {items:?}"
    );
    let capacity = desired_capacity(&router, case, group).await;
    assert_eq!(
        capacity, 1,
        "case {case:?}: the group's capacity must be left alone, got {capacity}"
    );
}
