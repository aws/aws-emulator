// SPDX-License-Identifier: CC0-1.0

//! The alarm a stack declares over a metric of ANOTHER resource of the SAME stack, read back
//! through both of the services that own them.
//!
//! RULE under test: **an `AWS::CloudWatch::Alarm` a stack declares is created by a real
//! `PutMetricAlarm` in Amazon CloudWatch before the stack reports the resource created, under
//! the name the stack publishes for it and carrying the declaration's own configuration with
//! every intrinsic in it resolved — so a `Dimensions` value written as a reference to another
//! resource of the same stack travels as the identifier the service that OWNS that resource
//! holds it under — and the stack's own deletion deletes that alarm there.**
//!
//! The reported case deployed an `AWS::Route53::HealthCheck` beside an alarm over
//! `AWS/Route53` `HealthCheckStatus` dimensioned on the check's id: the stack reached
//! `CREATE_COMPLETE` and `DescribeAlarms` for the published name answered an EMPTY
//! `MetricAlarms`, so a client that deploys its monitoring declaratively got a stack that
//! claimed success and no monitoring at all — nothing to evaluate the health checker's series,
//! and no alarm for a later update, rollback or teardown to address.
//!
//! The rows differ along what the rule generalises over rather than restating that one
//! template:
//!
//! * the ORDER the two resources are declared in — which decides nothing, because the
//!   reference between them is what orders their provisioning;
//! * WHICH intrinsic the dimension reads the other resource with (`Ref`, and `Fn::GetAtt` of
//!   the type's own `HealthCheckId` attribute), and how many dimensions the metric carries
//!   (one, and two of which only the first is a reference);
//! * where the alarm's NAME comes from — declared outright, and left to CloudFormation;
//! * the kind of health check declared beside it (an HTTPS endpoint named by domain name, an
//!   HTTP endpoint on an IP address, and an HTTPS string match);
//! * the alarm's configuration itself: three metrics, three statistics
//!   (`smithy_query(query="shape",shape="Statistic")`), three comparison operators
//!   (`smithy_query(query="shape",shape="ComparisonOperator")`), a period of a minute and of
//!   five, one and several evaluation periods with and without an "M out of N" count, an
//!   integer and a fractional threshold, and a `Unit` from `StandardUnit`'s own value set
//!   (`smithy_query(query="shape",shape="StandardUnit")`) as well as none.
//!
//! Nothing is waited out: the stack is polled to its terminal status, and the teardown is
//! polled for in both owning services until it holds.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::declared_alarm_is_readable_in_cloudwatch::alarms_named;
use crate::dynamodb_table_resources::failure_reasons;
use crate::stack_lifecycle::{
    call, create_stack, member, output_value, router, settled_stack, xml_text, REGION,
};

/// The logical ids every row declares its two resources under, and the output keys the stack
/// publishes their identities at: `Ref` of the alarm ("Ref returns the alarm name"), `Fn::GetAtt
/// … Arn` of it, and `Ref` of the health check ("Ref returns the health check ID").
const ALARM_ID: &str = "Watch";
const CHECK_ID: &str = "Probe";
const ALARM_NAME_OUTPUT: &str = "AlarmName";
const ALARM_ARN_OUTPUT: &str = "AlarmArn";
const CHECK_ID_OUTPUT: &str = "HealthCheckId";

/// The namespace Amazon Route 53 publishes its health-check metrics in, which is what the
/// alarms below watch.
const ROUTE53_NAMESPACE: &str = "AWS/Route53";

/// The dimension name a health-check metric is dimensioned by, as the declaration writes it.
const HEALTH_CHECK_DIMENSION: &str = "HealthCheckId";

/// How long a caller waits for the teardown a stack's deletion owes the two owning services,
/// and how often it looks again. Generous on purpose: what is waited FOR is the alarm and the
/// health check being gone, so a loaded host cannot decide a case.
const TEARDOWN_DEADLINE: Duration = Duration::from_secs(60);
/// See [`TEARDOWN_DEADLINE`].
const TEARDOWN_LOOK_AGAIN: Duration = Duration::from_millis(100);

/// Where a dimension's VALUE comes from in the declaration.
#[derive(Debug, Clone, Copy)]
enum DimensionValue {
    /// `Ref` of the health check declared in the same stack.
    RefCheck,
    /// `Fn::GetAtt` of that resource's own `HealthCheckId` attribute.
    CheckIdAttribute,
    /// A value the declaration writes out.
    Literal(&'static str),
}

impl DimensionValue {
    /// The value as the TEMPLATE writes it.
    fn declared(self) -> Value {
        match self {
            Self::RefCheck => json!({"Ref": CHECK_ID}),
            Self::CheckIdAttribute => json!({"Fn::GetAtt": [CHECK_ID, "HealthCheckId"]}),
            Self::Literal(text) => json!(text),
        }
    }

    /// The value CloudWatch must hold once the reference is resolved: the identifier Route 53
    /// minted for the health check, for either intrinsic.
    fn resolved(self, health_check_id: &str) -> String {
        match self {
            Self::RefCheck | Self::CheckIdAttribute => health_check_id.to_string(),
            Self::Literal(text) => text.to_string(),
        }
    }
}

/// Where the alarm's name comes from.
#[derive(Debug, Clone, Copy)]
enum Naming {
    /// The declaration states this name outright.
    Literal(&'static str),
    /// The declaration states none, so CloudFormation names the resource itself.
    Generated,
}

/// Which of the two resources the template declares first.
#[derive(Debug, Clone, Copy)]
enum Order {
    AlarmFirst,
    CheckFirst,
}

/// One row: the estate a template declares, and what the two owning services must then hold.
struct Row {
    case: &'static str,
    stack: &'static str,
    order: Order,
    naming: Naming,
    /// The `HealthCheckConfig` the `AWS::Route53::HealthCheck` declares.
    health_check_config: fn() -> Value,
    metric: &'static str,
    statistic: &'static str,
    comparison_operator: &'static str,
    period: i64,
    evaluation_periods: i64,
    datapoints_to_alarm: Option<i64>,
    /// The threshold exactly as the template spells it — an integer and a fraction are both
    /// one `Double` on the wire (`PutMetricAlarmInput$Threshold`).
    threshold: &'static str,
    unit: Option<&'static str>,
    treat_missing_data: Option<&'static str>,
    /// The dimensions the alarm declares, in declaration order.
    dimensions: &'static [(&'static str, DimensionValue)],
}

/// The rows, spanning the class the rule covers.
const ROWS: &[Row] = &[
    Row {
        case: "an alarm over the health check's status, dimensioned on the Ref of the check \
               declared after it",
        stack: "route53-health-alarm-ref",
        order: Order::AlarmFirst,
        naming: Naming::Literal("route53-health-check-status-alarm"),
        health_check_config: || {
            json!({
                "Type": "HTTPS",
                "FullyQualifiedDomainName": "api.health-alarm.example",
                "Port": 443,
                "ResourcePath": "/healthy",
                "RequestInterval": 30,
                "FailureThreshold": 3
            })
        },
        metric: "HealthCheckStatus",
        statistic: "Minimum",
        comparison_operator: "LessThanThreshold",
        period: 60,
        evaluation_periods: 1,
        datapoints_to_alarm: None,
        threshold: "1",
        unit: None,
        treat_missing_data: Some("breaching"),
        dimensions: &[(HEALTH_CHECK_DIMENSION, DimensionValue::RefCheck)],
    },
    Row {
        case: "an alarm CloudFormation names itself, dimensioned on the check's own \
               HealthCheckId attribute, over an M out of N window",
        stack: "route53-health-alarm-attribute",
        order: Order::CheckFirst,
        naming: Naming::Generated,
        health_check_config: || {
            json!({
                "Type": "HTTP",
                "IPAddress": "192.0.2.10",
                "Port": 80,
                "ResourcePath": "/ping",
                "RequestInterval": 10,
                "FailureThreshold": 1
            })
        },
        metric: "HealthCheckPercentageHealthy",
        statistic: "Average",
        comparison_operator: "LessThanOrEqualToThreshold",
        period: 300,
        evaluation_periods: 3,
        datapoints_to_alarm: Some(2),
        threshold: "66.5",
        unit: Some("Percent"),
        treat_missing_data: None,
        dimensions: &[(HEALTH_CHECK_DIMENSION, DimensionValue::CheckIdAttribute)],
    },
    Row {
        case: "an alarm over a two-dimension metric, one dimension a reference and the other a \
               literal",
        stack: "route53-health-alarm-two-dimensions",
        order: Order::CheckFirst,
        naming: Naming::Literal("route53-connection-time-alarm"),
        health_check_config: || {
            json!({
                "Type": "HTTPS_STR_MATCH",
                "FullyQualifiedDomainName": "search.health-alarm.example",
                "Port": 443,
                "ResourcePath": "/status",
                "SearchString": "READY",
                "RequestInterval": 30,
                "FailureThreshold": 2
            })
        },
        metric: "ConnectionTime",
        statistic: "Maximum",
        comparison_operator: "GreaterThanThreshold",
        period: 60,
        evaluation_periods: 2,
        datapoints_to_alarm: None,
        threshold: "2000",
        unit: Some("Milliseconds"),
        treat_missing_data: Some("notBreaching"),
        dimensions: &[
            (HEALTH_CHECK_DIMENSION, DimensionValue::RefCheck),
            ("Region", DimensionValue::Literal("us-east-1")),
        ],
    },
];

/// The threshold a row declares, read from the JSON the row spells it as — so the FORM the
/// template writes it in (an integer, a fraction) is the row's as well as the value.
fn threshold_of(row: &Row) -> Value {
    serde_json::from_str(row.threshold).unwrap_or_else(|error| {
        panic!(
            "case {:?}: the row's threshold must be a JSON number, got {:?} ({error})",
            row.case, row.threshold
        )
    })
}

/// The template one row deploys, with the two resources written in the order the row states so
/// that a declaration order cannot be what makes the reference resolve.
fn template_of(row: &Row) -> String {
    let mut alarm = json!({
        "Namespace": ROUTE53_NAMESPACE,
        "MetricName": row.metric,
        "Statistic": row.statistic,
        "ComparisonOperator": row.comparison_operator,
        "Period": row.period,
        "EvaluationPeriods": row.evaluation_periods,
        "Threshold": threshold_of(row),
        "Dimensions": row
            .dimensions
            .iter()
            .map(|(name, value)| json!({"Name": name, "Value": value.declared()}))
            .collect::<Vec<Value>>(),
    });
    if let Some(count) = row.datapoints_to_alarm {
        alarm["DatapointsToAlarm"] = json!(count);
    }
    if let Some(unit) = row.unit {
        alarm["Unit"] = json!(unit);
    }
    if let Some(treatment) = row.treat_missing_data {
        alarm["TreatMissingData"] = json!(treatment);
    }
    if let Naming::Literal(name) = row.naming {
        alarm["AlarmName"] = json!(name);
    }
    let alarm = json!({"Type": "AWS::CloudWatch::Alarm", "Properties": alarm});
    let check = json!({
        "Type": "AWS::Route53::HealthCheck",
        "Properties": {
            "HealthCheckConfig": (row.health_check_config)(),
            "HealthCheckTags": [{"Key": "Estate", "Value": row.stack}],
        },
    });
    let resources = match row.order {
        Order::AlarmFirst => {
            format!("{{\"{ALARM_ID}\": {alarm}, \"{CHECK_ID}\": {check}}}")
        }
        Order::CheckFirst => {
            format!("{{\"{CHECK_ID}\": {check}, \"{ALARM_ID}\": {alarm}}}")
        }
    };
    let outputs = json!({
        ALARM_NAME_OUTPUT: {"Value": {"Ref": ALARM_ID}},
        ALARM_ARN_OUTPUT: {"Value": {"Fn::GetAtt": [ALARM_ID, "Arn"]}},
        CHECK_ID_OUTPUT: {"Value": {"Ref": CHECK_ID}},
    });
    format!("{{\"Resources\": {resources}, \"Outputs\": {outputs}}}")
}

/// `GetHealthCheck` over Amazon Route 53's own restXml wire, signed the way a client's SDK
/// signs it: the HTTP status and the XML body, so a check that is NOT there is read as the
/// refusal that service answers rather than as an absence this suite assumed.
async fn get_health_check(router: &MultiServiceRouter, id: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("https://route53.amazonaws.com/2013-04-01/healthcheck/{id}"))
        .header("host", "route53.amazonaws.com")
        .header("content-type", "application/xml")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/route53/aws4_request, \
                 SignedHeaders=host, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed restXml request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, body)
}

/// Look again until the stack's deletion has removed the alarm from CloudWatch and the health
/// check from Route 53 — the CONDITION a teardown reaches, never a span of the clock.
async fn poll_until_torn_down(
    router: &MultiServiceRouter,
    case: &str,
    alarm_name: &str,
    health_check_id: &str,
) {
    let deadline = Instant::now() + TEARDOWN_DEADLINE;
    loop {
        let alarms = alarms_named(router, case, alarm_name).await;
        let (status, body) = get_health_check(router, health_check_id).await;
        if alarms.is_empty() && status == 404 {
            assert_eq!(
                xml_text(&body, "Code").as_deref(),
                Some("NoSuchHealthCheck"),
                "case {case:?}: expected Route 53 to refuse the deleted health check \
                 {health_check_id:?} with NoSuchHealthCheck, got {body}"
            );
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the stack's deletion to delete the alarm {alarm_name:?} in \
             CloudWatch and the health check {health_check_id:?} in Route 53, got {} alarm(s) \
             and HTTP {status} from GetHealthCheck with {body}",
            alarms.len()
        );
        tokio::time::sleep(TEARDOWN_LOOK_AGAIN).await;
    }
}

#[tokio::test]
async fn a_declared_alarm_reaches_cloudwatch_dimensioned_on_the_id_its_stack_minted() {
    for row in ROWS {
        let case = row.case;
        let router = router();

        let (status, body) = create_stack(&router, row.stack, &template_of(row), &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
        );
        let described = settled_stack(&router, case, row.stack).await;
        let reported = xml_text(&described, "StackStatus");
        assert_eq!(
            reported.as_deref(),
            Some("CREATE_COMPLETE"),
            "case {case:?}: expected the create to complete, got {reported:?} because {:?}",
            failure_reasons(&router, row.stack).await
        );

        // The identifiers the stack PUBLISHES are the handles every later call about the two
        // resources takes, so they are what the reads below address.
        let name = output_value(&described, ALARM_NAME_OUTPUT).unwrap_or_default();
        let arn = output_value(&described, ALARM_ARN_OUTPUT).unwrap_or_default();
        let health_check_id = output_value(&described, CHECK_ID_OUTPUT).unwrap_or_default();
        match row.naming {
            Naming::Literal(declared) => assert_eq!(
                name, declared,
                "case {case:?}: expected the stack to publish the declared alarm name \
                 {declared:?}, got {name:?}"
            ),
            Naming::Generated => {
                let prefix = format!("{}-{ALARM_ID}-", row.stack);
                assert!(
                    name.starts_with(&prefix),
                    "case {case:?}: expected a generated alarm name under {prefix:?}, got \
                     {name:?}"
                );
            }
        }

        // The health check half of the estate is really there: the id the stack published is
        // one Route 53 resolves, and it is that service's own, not the logical id of the
        // declaration nor a name CloudFormation computed.
        let (status, read) = get_health_check(&router, &health_check_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetHealthCheck on the id the stack \
             published ({health_check_id:?}), got {status} with {read}"
        );
        assert_eq!(
            xml_text(&read, "Id").as_deref(),
            Some(health_check_id.as_str()),
            "case {case:?}: expected Route 53 to report the health check under the id the \
             stack published, got {read}"
        );
        assert_ne!(
            health_check_id, CHECK_ID,
            "case {case:?}: expected the published id to be Route 53's own, not the logical id \
             of the declaration"
        );

        // The alarm itself, by the name the stack published: exactly one entry, read to the
        // end of the paging, so "no alarm" would be no entry on any page.
        let reported = alarms_named(&router, case, &name).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected Amazon CloudWatch to report exactly one alarm named \
             {name:?} — the one the stack reports created — got {reported:?}"
        );
        let alarm = reported.into_iter().next().unwrap_or(Value::Null);

        for (member_name, expected) in [
            ("AlarmName", name.as_str()),
            ("Namespace", ROUTE53_NAMESPACE),
            ("MetricName", row.metric),
            ("Statistic", row.statistic),
            ("ComparisonOperator", row.comparison_operator),
        ] {
            let held = alarm.get(member_name).and_then(Value::as_str);
            assert_eq!(
                held,
                Some(expected),
                "case {case:?}: expected CloudWatch to report {member_name} as {expected:?}, \
                 got {held:?}"
            );
        }
        for (member_name, expected) in [
            ("Period", Some(row.period)),
            ("EvaluationPeriods", Some(row.evaluation_periods)),
            ("DatapointsToAlarm", row.datapoints_to_alarm),
        ] {
            let held = alarm.get(member_name).and_then(Value::as_i64);
            assert_eq!(
                held, expected,
                "case {case:?}: expected CloudWatch to report {member_name} as {expected:?}, \
                 got {held:?}"
            );
        }
        for (member_name, expected) in [
            ("Unit", row.unit),
            ("TreatMissingData", row.treat_missing_data),
        ] {
            let held = alarm.get(member_name).and_then(Value::as_str);
            assert_eq!(
                held, expected,
                "case {case:?}: expected CloudWatch to report {member_name} as {expected:?}, \
                 got {held:?}"
            );
        }
        let declared_threshold = threshold_of(row);
        let held_threshold = alarm.get("Threshold").and_then(Value::as_f64);
        assert_eq!(
            held_threshold,
            declared_threshold.as_f64(),
            "case {case:?}: expected CloudWatch to report the declared threshold \
             {declared_threshold}, got {held_threshold:?}"
        );

        // The dimension the declaration wrote as a REFERENCE: what CloudWatch holds is the
        // identifier Route 53 minted for the resource of the same stack, which is what makes
        // the alarm evaluate that health check's own series.
        let expected_dimensions: Vec<Value> = row
            .dimensions
            .iter()
            .map(|(name, value)| json!({"Name": name, "Value": value.resolved(&health_check_id)}))
            .collect();
        let held_dimensions = alarm.get("Dimensions").cloned().unwrap_or(json!([]));
        assert_eq!(
            held_dimensions,
            Value::Array(expected_dimensions.clone()),
            "case {case:?}: expected CloudWatch to hold the dimensions \
             {expected_dimensions:?} — every reference resolved to the identifier the owning \
             service minted — got {held_dimensions}"
        );

        // The ARN the stack publishes for the alarm is the one CloudWatch holds it under, so
        // the two services agree about its identity as well as its configuration.
        let held_arn = alarm.get("AlarmArn").and_then(Value::as_str);
        assert_eq!(
            held_arn,
            Some(arn.as_str()),
            "case {case:?}: expected CloudWatch to report the ARN the stack published \
             ({arn:?}), got {held_arn:?}"
        );
        assert!(
            arn.ends_with(&format!(":alarm:{name}")),
            "case {case:?}: expected the alarm's ARN to name the alarm it was created under, \
             got {arn:?}"
        );

        // The matching delete: the stack's own deletion removes the alarm from CloudWatch and
        // the health check from Route 53.
        let (status, body) = call(&router, "DeleteStack", &[member("StackName", row.stack)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DeleteStack, got {status} with {body}"
        );
        poll_until_torn_down(&router, case, &name, &health_check_id).await;
    }
}
