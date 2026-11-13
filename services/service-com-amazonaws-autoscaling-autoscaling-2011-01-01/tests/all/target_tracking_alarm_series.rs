// SPDX-License-Identifier: CC0-1.0

//! The series the alarms of a target-tracking scaling policy grade, and the one member a
//! metric counted per target cannot be resolved without.
//!
//! The rules under test:
//!
//! 1. "The CloudWatch alarms created for the target tracking scaling policy"
//!    (`PolicyARNType$Alarms`): a put of a `TargetTrackingScaling` policy materialises
//!    that pair of alarms in the account's OWN CloudWatch store, and each alarm grades the
//!    series the policy's metric specification names — the namespace, metric name,
//!    statistic, unit and dimensions the predefined metric type IS — with the scale-out
//!    alarm firing ABOVE the value the policy tracks and the scale-in alarm BELOW it, and
//!    each alarm's action naming the policy it executes. The row set is the whole
//!    predefined catalog, the model's `com.amazonaws.autoscaling#MetricType` enum
//!    (`smithy_query(query="shape",shape="MetricType")`), each with the series its own
//!    documentation describes it as
//!    (`PredefinedMetricSpecification$PredefinedMetricType`) — including the two entries
//!    no other suite here puts.
//! 2. A predefined metric whose series is counted per Application Load Balancer target
//!    group cannot be resolved without the `ResourceLabel` that names that group ("You
//!    can't specify a resource label unless the target group is attached to the Auto
//!    Scaling group", `PredefinedMetricSpecification$ResourceLabel`), so a put naming that
//!    metric type with no label is refused with `ValidationError` and leaves neither a
//!    policy nor an alarm behind, while every metric type measured over the group itself
//!    is accepted without one. The table is the same catalog, so the requirement is pinned
//!    to hold for exactly the one entry that is counted per target and for no other.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use service_com_amazonaws_cloudwatch_graniteserviceversion20100801 as cloudwatch_service;

use crate::support::{
    borrowed, json_rpc, new_engine, p, query, tag_value, tag_values, xml_unescape,
};

/// The launch source every group in this suite is created from — a launch TEMPLATE,
/// because no account can create a launch configuration.
const LAUNCH_SOURCE: (&str, &str) = ("LaunchTemplate.LaunchTemplateName", "lt-1");

/// The signing name a client scopes a CloudWatch request to.
const CLOUDWATCH_SIGNING_NAME: &str = "monitoring";

/// The `x-amz-target` prefix CloudWatch's awsJson protocol names its operations under.
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The awsJson version a CloudWatch client sends.
const CLOUDWATCH_JSON_VERSION: &str = "1.0";

/// The wire prefix of the member a target-tracking put carries its predefined metric
/// under.
const PREDEFINED: &str = "TargetTrackingConfiguration.PredefinedMetricSpecification";

/// The prefix Amazon EC2 Auto Scaling names a target-tracking policy's alarms with, for
/// the group `group`: every alarm of every policy of that group, and nothing else.
fn group_alarm_prefix(group: &str) -> String {
    format!("TargetTracking-{group}-")
}

/// A router carrying Amazon EC2 Auto Scaling and CloudWatch on ONE engine: the alarms a
/// target-tracking policy mints are CloudWatch's own resources, so they are read back
/// through CloudWatch's own API.
fn router() -> MultiServiceRouter {
    let engine = new_engine();
    let mut router = MultiServiceRouter::new();
    router.register_service(autoscaling::EmulatedService::new(Arc::clone(&engine)));
    router.register_service(cloudwatch_service::EmulatedService::new(Arc::clone(&engine)));
    router
}

/// Create one group and assert the create succeeded.
async fn create_group(router: &MultiServiceRouter, name: &str) {
    let (status, body) = query(
        router,
        "CreateAutoScalingGroup",
        &[
            ("AutoScalingGroupName", name),
            LAUNCH_SOURCE,
            ("MinSize", "0"),
            ("MaxSize", "4"),
            ("DesiredCapacity", "0"),
            ("AvailabilityZones.member.1", "us-west-2a"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the group {name:?} must be created, got {status} with {body:?}"
    );
}

/// POST one `PutScalingPolicy` and answer its status and body.
async fn put_policy(
    router: &MultiServiceRouter,
    group: &str,
    policy: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("AutoScalingGroupName", group), p("PolicyName", policy)];
    params.extend_from_slice(extra);
    query(router, "PutScalingPolicy", &borrowed(&params)).await
}

/// The metric alarms CloudWatch's own store reports for `body`, through CloudWatch's own
/// `DescribeAlarms`.
async fn metric_alarms(router: &MultiServiceRouter, body: Value) -> Vec<Value> {
    let (status, answer) = json_rpc(
        router,
        CLOUDWATCH_SIGNING_NAME,
        CLOUDWATCH_TARGET_PREFIX,
        CLOUDWATCH_JSON_VERSION,
        "DescribeAlarms",
        body,
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeAlarms must succeed, got {status} with {answer}"
    );
    answer
        .pointer("/MetricAlarms")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// The alarms CloudWatch holds for the named alarms.
async fn alarms_named(router: &MultiServiceRouter, names: &[String]) -> Vec<Value> {
    metric_alarms(
        router,
        json!({"AlarmNames": names, "AlarmTypes": ["MetricAlarm"]}),
    )
    .await
}

/// How many alarms CloudWatch holds for every policy of one group.
async fn alarm_count_of_group(router: &MultiServiceRouter, group: &str) -> usize {
    metric_alarms(
        router,
        json!({"AlarmNamePrefix": group_alarm_prefix(group), "AlarmTypes": ["MetricAlarm"]}),
    )
    .await
    .len()
}

/// The string member `member` of one reported alarm.
fn member(alarm: &Value, name: &str) -> String {
    alarm
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `(Name, Value)` dimensions of one reported alarm, in the order it reports them.
fn dimensions(alarm: &Value) -> Vec<(String, String)> {
    alarm
        .get("Dimensions")
        .and_then(Value::as_array)
        .map(|dimensions| {
            dimensions
                .iter()
                .map(|dimension| (member(dimension, "Name"), member(dimension, "Value")))
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the predefined metric catalog: the metric type, the label it needs (when its
/// series is counted per target), and the series an alarm of a policy tracking it grades.
struct MetricCase {
    case: &'static str,
    predefined_metric_type: &'static str,
    resource_label: Option<&'static str>,
    /// The value the policy keeps the metric at, as the request spells it — one per row, so
    /// the thresholds asserted below cannot pass by matching another row's value.
    target_value: &'static str,
    namespace: &'static str,
    metric_name: &'static str,
    statistic: &'static str,
    unit: &'static str,
    dimensions: Vec<(String, String)>,
}

/// The resource label the per-target row names its target group by: "the final portion of
/// the load balancer ARN and the final portion of the target group ARN into a single value,
/// separated by a forward slash (/)" (`PredefinedMetricSpecification$ResourceLabel`).
const RESOURCE_LABEL: &str = "app/web-alb/0123456789abcdef/targetgroup/web-tg/fedcba9876543210";

/// Every predefined metric type the model's `MetricType` enum holds, in the order it lists
/// them, each with the series its own documentation
/// (`PredefinedMetricSpecification$PredefinedMetricType`) describes it as: the "Average CPU
/// utilization of the Auto Scaling group" for `ASGAverageCPUUtilization`, the "Average
/// number of bytes received on all network interfaces by the Auto Scaling group" for
/// `ASGAverageNetworkIn`, the "Average number of bytes sent out on all network interfaces
/// by the Auto Scaling group" for `ASGAverageNetworkOut`, and the "Average Application Load
/// Balancer request count per target for your Auto Scaling group" for
/// `ALBRequestCountPerTarget`.
fn catalog(group: &str) -> Vec<MetricCase> {
    let group_dimension = vec![("AutoScalingGroupName".to_string(), group.to_string())];
    vec![
        MetricCase {
            case: "the average CPU utilization of the group",
            predefined_metric_type: "ASGAverageCPUUtilization",
            resource_label: None,
            target_value: "40",
            namespace: "AWS/EC2",
            metric_name: "CPUUtilization",
            statistic: "Average",
            unit: "Percent",
            dimensions: group_dimension.clone(),
        },
        MetricCase {
            case: "the average bytes received by the group",
            predefined_metric_type: "ASGAverageNetworkIn",
            resource_label: None,
            target_value: "1048576",
            namespace: "AWS/EC2",
            metric_name: "NetworkIn",
            statistic: "Average",
            unit: "Bytes",
            dimensions: group_dimension.clone(),
        },
        MetricCase {
            case: "the average bytes sent out by the group",
            predefined_metric_type: "ASGAverageNetworkOut",
            resource_label: None,
            target_value: "524288.5",
            namespace: "AWS/EC2",
            metric_name: "NetworkOut",
            statistic: "Average",
            unit: "Bytes",
            dimensions: group_dimension,
        },
        MetricCase {
            case: "the request count per Application Load Balancer target",
            predefined_metric_type: "ALBRequestCountPerTarget",
            resource_label: Some(RESOURCE_LABEL),
            target_value: "1000",
            namespace: "AWS/ApplicationELB",
            metric_name: "RequestCountPerTarget",
            statistic: "Sum",
            unit: "Count",
            // The series is published per load balancer and per target group, which are
            // the two resources the label names.
            dimensions: vec![
                (
                    "LoadBalancer".to_string(),
                    "app/web-alb/0123456789abcdef".to_string(),
                ),
                (
                    "TargetGroup".to_string(),
                    "targetgroup/web-tg/fedcba9876543210".to_string(),
                ),
            ],
        },
    ]
}

/// The members one row's put carries.
fn put_params(case: &MetricCase) -> Vec<(String, String)> {
    let mut params = vec![
        p("PolicyType", "TargetTrackingScaling"),
        p(
            &format!("{PREDEFINED}.PredefinedMetricType"),
            case.predefined_metric_type,
        ),
        p("TargetTrackingConfiguration.TargetValue", case.target_value),
    ];
    if let Some(label) = case.resource_label {
        params.push(p(&format!("{PREDEFINED}.ResourceLabel"), label));
    }
    params
}

/// RULE 1: each predefined metric type's policy mints the alarm pair that grades THAT
/// metric's series, and CloudWatch's own read reports every one of them.
#[tokio::test]
async fn a_target_tracking_policys_alarms_grade_the_series_its_metric_names() {
    let router = router();
    let group = "asg-tracked-series";
    create_group(&router, group).await;

    for (index, case) in catalog(group).into_iter().enumerate() {
        let label = case.case;
        let policy_name = format!("tracked-{index}");
        let (status, body) = put_policy(&router, group, &policy_name, &put_params(&case)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200, got {status} with {body:?}"
        );
        let policy_arn = tag_value(&body, "PolicyARN");
        assert!(
            policy_arn.contains(&policy_name),
            "case {label:?}: the answered PolicyARN must name the policy, got {policy_arn:?}"
        );
        let answered: Vec<String> = tag_values(&body, "AlarmName");
        assert_eq!(
            answered.len(),
            2,
            "case {label:?}: expected the scale-out and the scale-in alarm, got {answered:?}"
        );

        let reported = alarms_named(&router, &answered).await;
        assert_eq!(
            reported.len(),
            answered.len(),
            "case {label:?}: CloudWatch must hold every alarm the policy reports, got \
             {reported:?} for {answered:?}"
        );

        let target_value: f64 = case
            .target_value
            .parse()
            .expect("the row's target value is a number");
        let mut scale_out = 0usize;
        let mut scale_in = 0usize;
        for alarm in &reported {
            let name = member(alarm, "AlarmName");
            assert_eq!(
                member(alarm, "Namespace"),
                case.namespace,
                "case {label:?}: alarm {name:?} must grade the namespace {:?}, got {:?}",
                case.namespace,
                member(alarm, "Namespace")
            );
            assert_eq!(
                member(alarm, "MetricName"),
                case.metric_name,
                "case {label:?}: alarm {name:?} must grade the metric {:?}, got {:?}",
                case.metric_name,
                member(alarm, "MetricName")
            );
            assert_eq!(
                member(alarm, "Statistic"),
                case.statistic,
                "case {label:?}: alarm {name:?} must grade the statistic {:?}, got {:?}",
                case.statistic,
                member(alarm, "Statistic")
            );
            assert_eq!(
                member(alarm, "Unit"),
                case.unit,
                "case {label:?}: alarm {name:?} must grade the unit {:?}, got {:?}",
                case.unit,
                member(alarm, "Unit")
            );
            assert_eq!(
                dimensions(alarm),
                case.dimensions,
                "case {label:?}: alarm {name:?} must grade the dimensions {:?}, got {:?}",
                case.dimensions,
                dimensions(alarm)
            );
            // The alarm EXECUTES the policy: that wiring is what makes the policy
            // alarm-driven.
            let actions: Vec<String> = alarm
                .get("AlarmActions")
                .and_then(Value::as_array)
                .map(|actions| {
                    actions
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            assert!(
                actions.contains(&policy_arn),
                "case {label:?}: alarm {name:?} must act on the policy {policy_arn:?}, got \
                 {actions:?}"
            );

            let threshold = alarm.get("Threshold").and_then(Value::as_f64);
            let comparison = member(alarm, "ComparisonOperator");
            // One alarm asks the policy to scale OUT once the metric is above the value the
            // policy keeps it at; the other asks it to scale in once the metric has fallen
            // CLEAR of that value, so the two thresholds are never the same one.
            if comparison == "GreaterThanThreshold" {
                scale_out += 1;
                assert_eq!(
                    threshold,
                    Some(target_value),
                    "case {label:?}: the scale-out alarm {name:?} must fire above the tracked \
                     value {target_value}, got {threshold:?}"
                );
            } else {
                scale_in += 1;
                assert_eq!(
                    comparison, "LessThanThreshold",
                    "case {label:?}: alarm {name:?} must compare the metric to its threshold \
                     from one side or the other, got {comparison:?}"
                );
                let threshold = threshold.unwrap_or_else(|| {
                    panic!("case {label:?}: alarm {name:?} must carry a threshold")
                });
                assert!(
                    threshold < target_value,
                    "case {label:?}: the scale-in alarm {name:?} must fire below the tracked \
                     value {target_value}, got {threshold}"
                );
            }
        }
        assert_eq!(
            (scale_out, scale_in),
            (1, 1),
            "case {label:?}: the pair must be one scale-out and one scale-in alarm, got \
             {reported:?}"
        );
    }
}

/// RULE 2: the resource label is required of exactly the metric type counted per target —
/// that put is refused and leaves nothing behind, while every metric type measured over the
/// group itself is accepted without one.
#[tokio::test]
async fn the_resource_label_is_required_of_exactly_the_per_target_metric() {
    let router = router();
    let group = "asg-label-rule";
    create_group(&router, group).await;

    for (index, case) in catalog(group).into_iter().enumerate() {
        let label = case.case;
        let needs_label = case.resource_label.is_some();
        let policy_name = format!("unlabelled-{index}");
        // The SAME put as the row above, with the label withheld from every row.
        let mut params = put_params(&case);
        params.retain(|(key, _)| !key.ends_with(".ResourceLabel"));

        let before = alarm_count_of_group(&router, group).await;
        let (status, body) = put_policy(&router, group, &policy_name, &params).await;
        let after = alarm_count_of_group(&router, group).await;

        if needs_label {
            assert_eq!(
                status, 400,
                "case {label:?}: a put with no resource label must be refused, got {status} \
                 with {body:?}"
            );
            let code = tag_value(&body, "Code");
            assert_eq!(
                code, "ValidationError",
                "case {label:?}: expected the ValidationError code, got {code:?} with {body:?}"
            );
            let message = xml_unescape(&tag_value(&body, "Message"));
            let expected = format!(
                "Resource label should be specified for predefined metric type {}",
                case.predefined_metric_type
            );
            assert_eq!(
                message, expected,
                "case {label:?}: expected the message {expected:?}, got {message:?}"
            );
            assert_eq!(
                after, before,
                "case {label:?}: a refused put must leave no alarm behind, the group's alarm \
                 count went from {before} to {after}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {label:?}: a metric measured over the group itself needs no resource \
                 label, got {status} with {body:?}"
            );
            assert_eq!(
                after,
                before + 2,
                "case {label:?}: an accepted put must mint its alarm pair, the group's alarm \
                 count went from {before} to {after}"
            );
            // The pair the count grew by is THIS policy's own: the alarms it answered are
            // the ones CloudWatch resolves, so the delta cannot be another row's pair.
            let answered: Vec<String> = tag_values(&body, "AlarmName");
            assert_eq!(
                answered.len(),
                2,
                "case {label:?}: expected the scale-out and the scale-in alarm, got {answered:?}"
            );
            let mut resolved: Vec<String> = alarms_named(&router, &answered)
                .await
                .iter()
                .map(|alarm| member(alarm, "AlarmName"))
                .collect();
            resolved.sort();
            let mut expected = answered.clone();
            expected.sort();
            assert_eq!(
                resolved, expected,
                "case {label:?}: CloudWatch must hold the alarms this policy answered, got \
                 {resolved:?} for {expected:?}"
            );
        }

        // And the policy exists exactly where the put was accepted.
        let (status, body) = query(
            &router,
            "DescribePolicies",
            &[
                ("AutoScalingGroupName", group),
                ("PolicyNames.member.1", &policy_name),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label:?}: the read-back must succeed, got {status} with {body:?}"
        );
        let names = tag_values(&body, "PolicyName");
        let expected: Vec<String> = if needs_label {
            Vec::new()
        } else {
            vec![policy_name.clone()]
        };
        assert_eq!(
            names, expected,
            "case {label:?}: expected the read to report {expected:?}, got {names:?}"
        );
    }
}
