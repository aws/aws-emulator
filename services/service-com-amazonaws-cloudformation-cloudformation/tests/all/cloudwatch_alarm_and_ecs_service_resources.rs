// SPDX-License-Identifier: CC0-1.0

//! The observability and workload resources a stack declares in the services that OWN them:
//! the Amazon CloudWatch alarm, the Amazon ECS service, and the DynamoDB stream another
//! resource of the same stack is wired to.
//!
//! RULE under test: **a resource type CloudFormation deploys is created in the service that
//! owns it through that service's own public API before the stack reports it created, and the
//! identifier the stack publishes for it is the one that service really holds it under** — so
//!
//! * `DescribeAlarms` reports the alarm an `AWS::CloudWatch::Alarm` declared, with the metric,
//!   the threshold and the actions the declaration states;
//! * `DescribeServices` and `ListTasks` resolve the service an `AWS::ECS::Service` declared,
//!   with the task definition and the desired count the declaration states, instead of
//!   answering `ServiceNotFoundException`;
//! * an attribute the owning service MINTS (a table's `StreamArn`) is published by the stack
//!   and reaches the resource of the same stack whose declaration reads it, so the trigger
//!   really polls the stream.
//!
//! Every case drives the real request path for both services through one router — the
//! CloudFormation calls over the awsQuery wire, the CloudWatch calls over `awsJson1_0`, the
//! Amazon ECS and AWS Lambda calls over their own wires — so what is asserted is what a client
//! that deploys the template and then reads the resources back observes.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::dynamodb_table_resources::failure_reasons;
use crate::hotswapped_function_code::lambda;
use crate::stack_lifecycle::{
    acknowledging_iam, create_stack, describe_stack, router, wait_for_the_transition, xml_blocks,
    xml_text, REGION,
};

/// The awsJson version Amazon CloudWatch's model declares (`awsJson1_0`), and the service
/// shape its `X-Amz-Target` names an operation under.
const CLOUDWATCH_JSON: &str = "application/x-amz-json-1.0";
const CLOUDWATCH_TARGET_PREFIX: &str = "GraniteServiceVersion20100801";

/// The same for Amazon ECS (`awsJson1_1`, service shape
/// `AmazonEC2ContainerServiceV20141113`).
const ECS_JSON: &str = "application/x-amz-json-1.1";
const ECS_TARGET_PREFIX: &str = "AmazonEC2ContainerServiceV20141113";

/// Call one Amazon CloudWatch operation through the same router, signed for `monitoring` the
/// way a client's SDK signs it: the status and the parsed answer document.
async fn cloudwatch(
    router: &MultiServiceRouter,
    operation: &str,
    request: &Value,
) -> (u16, Value) {
    json_call(
        router,
        "monitoring",
        CLOUDWATCH_JSON,
        &format!("{CLOUDWATCH_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// Call one Amazon ECS operation through the same router, signed for `ecs`.
async fn ecs(router: &MultiServiceRouter, operation: &str, request: &Value) -> (u16, Value) {
    json_call(
        router,
        "ecs",
        ECS_JSON,
        &format!("{ECS_TARGET_PREFIX}.{operation}"),
        request,
    )
    .await
}

/// One awsJson call over the router: the single calling convention both services above are
/// reached through, so neither spells the signing of a request twice.
async fn json_call(
    router: &MultiServiceRouter,
    signing_name: &str,
    media_type: &str,
    target: &str,
    request: &Value,
) -> (u16, Value) {
    let http_request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{signing_name}.{REGION}.amazonaws.com/"))
        .header("content-type", media_type)
        .header("x-amz-target", target)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                 aws4_request, SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
            ),
        )
        .body(bytes::Bytes::from(request.to_string()))
        .expect("a well-formed awsJson request");
    let response = router.dispatch(http_request).await;
    let status = response.status().as_u16();
    let body = response.into_body().to_vec();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

/// The `OutputValue` the stack reports for `key`, read INSIDE the `Outputs` block so a member
/// of another list in the envelope cannot be swept up with it.
fn output(xml: &str, key: &str) -> Option<String> {
    let block = xml_blocks(xml, "Outputs").into_iter().next()?;
    xml_blocks(&block, "member")
        .into_iter()
        .find(|entry| xml_text(entry, "OutputKey").as_deref() == Some(key))
        .and_then(|entry| xml_text(&entry, "OutputValue"))
}

/// Deploy one case's template and answer the completed stack's `DescribeStacks` body, with the
/// reasons the stack recorded where it did not complete.
async fn deployed(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) -> String {
    let (status, body) = create_stack(router, stack, template, &acknowledging_iam(&[])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with {body}"
    );
    wait_for_the_transition().await;
    let (status, described) = describe_stack(router, stack).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeStacks, got {status} with {described}"
    );
    let reported = xml_text(&described, "StackStatus");
    assert_eq!(
        reported.as_deref(),
        Some("CREATE_COMPLETE"),
        "case {case:?}: expected the create to complete, got {reported:?} because {:?}",
        failure_reasons(router, stack).await
    );
    described
}

// ---------------------------------------------------------------------------
// AWS::CloudWatch::Alarm
// ---------------------------------------------------------------------------

/// One alarm row: the declaration, and the members CloudWatch must report for it.
struct AlarmRow {
    case: &'static str,
    stack: &'static str,
    /// The `AWS::CloudWatch::Alarm` properties the template declares.
    properties: Value,
    /// The `AlarmName` the declaration states, or `None` where CloudFormation names it.
    declared_name: Option<&'static str>,
    /// The members `DescribeAlarms` must report for the alarm, as (member, value) pairs.
    reported: Vec<(&'static str, Value)>,
}

/// The rows span what the rule generalises over: WHICH metric an alarm watches (a single
/// metric with dimensions, a metric math expression, an extended statistic), whether the
/// template NAMES the alarm, how many evaluation periods and datapoints it asks for, and
/// whether it wires actions to a topic.
///
/// The statistic and comparison values are the model's own
/// (`smithy_query(query="shape",shape="Statistic")`,
/// `smithy_query(query="shape",shape="ComparisonOperator")`), and the metric math row is the
/// shape `PutMetricAlarm$Metrics` takes.
fn alarm_rows() -> Vec<AlarmRow> {
    vec![
        AlarmRow {
            case: "a single-metric alarm over a dimension, named by the template",
            stack: "alarm-single-metric",
            properties: json!({
                "AlarmName": "orders-5xx",
                "AlarmDescription": "the load balancer's own 5xx count",
                "Namespace": "AWS/ApplicationELB",
                "MetricName": "HTTPCode_ELB_5XX_Count",
                "ComparisonOperator": "GreaterThanThreshold",
                "EvaluationPeriods": 1,
                "Period": 60,
                "Statistic": "Sum",
                "Threshold": 0,
                "TreatMissingData": "notBreaching",
                "Dimensions": [{"Name": "LoadBalancer", "Value": "app/orders/0123456789abcdef"}],
            }),
            declared_name: Some("orders-5xx"),
            reported: vec![
                ("Namespace", json!("AWS/ApplicationELB")),
                ("MetricName", json!("HTTPCode_ELB_5XX_Count")),
                ("ComparisonOperator", json!("GreaterThanThreshold")),
                ("Statistic", json!("Sum")),
                ("EvaluationPeriods", json!(1)),
                ("Period", json!(60)),
                ("TreatMissingData", json!("notBreaching")),
                (
                    "Dimensions",
                    json!([{"Name": "LoadBalancer", "Value": "app/orders/0123456789abcdef"}]),
                ),
            ],
        },
        AlarmRow {
            case: "an alarm the template does not name, with several datapoints and an \
                   extended statistic",
            stack: "alarm-extended-statistic",
            properties: json!({
                "Namespace": "AWS/Lambda",
                "MetricName": "Duration",
                "ComparisonOperator": "GreaterThanOrEqualToThreshold",
                "EvaluationPeriods": 3,
                "DatapointsToAlarm": 2,
                "Period": 300,
                "ExtendedStatistic": "p99",
                "Threshold": 250,
                "Unit": "Milliseconds",
                "ActionsEnabled": false,
            }),
            declared_name: None,
            reported: vec![
                ("Namespace", json!("AWS/Lambda")),
                ("MetricName", json!("Duration")),
                ("ComparisonOperator", json!("GreaterThanOrEqualToThreshold")),
                ("ExtendedStatistic", json!("p99")),
                ("EvaluationPeriods", json!(3)),
                ("DatapointsToAlarm", json!(2)),
                ("Period", json!(300)),
                ("Unit", json!("Milliseconds")),
                ("ActionsEnabled", json!(false)),
            ],
        },
        AlarmRow {
            case: "a metric math alarm, which declares no namespace or metric of its own",
            stack: "alarm-metric-math",
            properties: json!({
                "AlarmName": "queue-backlog",
                "ComparisonOperator": "LessThanThreshold",
                "EvaluationPeriods": 2,
                "Threshold": 1,
                "Metrics": [
                    {
                        "Id": "visible",
                        "MetricStat": {
                            "Metric": {
                                "Namespace": "AWS/SQS",
                                "MetricName": "ApproximateNumberOfMessagesVisible",
                                "Dimensions": [{"Name": "QueueName", "Value": "orders"}],
                            },
                            "Period": 60,
                            "Stat": "Average",
                        },
                        "ReturnData": false,
                    },
                    {
                        "Id": "drained",
                        "Expression": "visible * 2",
                        "Label": "twice the backlog",
                        "ReturnData": true,
                    },
                ],
            }),
            declared_name: Some("queue-backlog"),
            reported: vec![
                ("ComparisonOperator", json!("LessThanThreshold")),
                ("EvaluationPeriods", json!(2)),
            ],
        },
    ]
}

/// The template one alarm row deploys: the alarm, and the two outputs the type publishes for
/// it (`Ref` is the alarm's name, `Fn::GetAtt Arn` its ARN).
fn alarm_template(row: &AlarmRow) -> String {
    json!({
        "Resources": {"Alarm": {"Type": "AWS::CloudWatch::Alarm", "Properties": row.properties}},
        "Outputs": {
            "AlarmName": {"Value": {"Ref": "Alarm"}},
            "AlarmArn": {"Value": {"Fn::GetAtt": ["Alarm", "Arn"]}},
        },
    })
    .to_string()
}

/// The `MetricAlarm` CloudWatch reports under `name`, or `None` where it reports none.
async fn described_alarm(router: &MultiServiceRouter, name: &str) -> Option<Value> {
    let (status, document) = cloudwatch(router, "DescribeAlarms", &json!({"AlarmNames": [name]})).await;
    if status != 200 {
        return None;
    }
    document
        .get("MetricAlarms")
        .and_then(Value::as_array)
        .and_then(|alarms| {
            alarms
                .iter()
                .find(|alarm| alarm.get("AlarmName").and_then(Value::as_str) == Some(name))
                .cloned()
        })
}

#[tokio::test]
async fn an_alarm_a_stack_declares_is_created_in_cloudwatch() {
    for row in alarm_rows() {
        let case = row.case;
        let router = router();
        let described = deployed(&router, case, row.stack, &alarm_template(&row)).await;

        let name = output(&described, "AlarmName").unwrap_or_default();
        assert!(
            !name.is_empty(),
            "case {case:?}: expected the stack to publish the alarm's name, got {described}"
        );
        if let Some(declared) = row.declared_name {
            assert_eq!(
                name, declared,
                "case {case:?}: expected the alarm to carry the declared name {declared:?}, \
                 got {name:?}"
            );
        }
        let arn = output(&described, "AlarmArn").unwrap_or_default();
        assert!(
            arn.ends_with(&format!(":alarm:{name}")),
            "case {case:?}: expected the published ARN to name the alarm {name:?}, got {arn:?}"
        );

        let alarm = described_alarm(&router, &name).await.unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected Amazon CloudWatch to hold the alarm {name:?} the \
                 stack reports created, got no alarm at all"
            )
        });
        for (reported, expected) in &row.reported {
            let held = alarm.get(*reported).cloned();
            assert_eq!(
                held.as_ref(),
                Some(expected),
                "case {case:?}: expected CloudWatch to report {reported} as {expected}, got \
                 {held:?}"
            );
        }
        // A metric math alarm's own queries are what it evaluates, so the alarm the stack
        // created carries as many of them as the declaration stated.
        if let Some(declared) = row.properties.get("Metrics").and_then(Value::as_array) {
            let held = alarm.get("Metrics").and_then(Value::as_array);
            assert_eq!(
                held.map(Vec::len),
                Some(declared.len()),
                "case {case:?}: expected the alarm to carry the {} declared metric queries, \
                 got {held:?}",
                declared.len()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// AWS::ECS::Service
// ---------------------------------------------------------------------------

/// One ECS row: what the service declares, and what Amazon ECS must report for it.
struct ServiceRow {
    case: &'static str,
    stack: &'static str,
    /// The `ServiceName` the declaration states, or `None` where CloudFormation names it.
    declared_name: Option<&'static str>,
    /// The `DesiredCount` the declaration states.
    desired_count: u32,
    /// The extra `AWS::ECS::Service` properties the row declares.
    extra: Value,
}

/// The rows differ along what the rule generalises over: whether the template NAMES the
/// service, how many tasks it asks the scheduler to keep, and whether it states the scheduling
/// strategy and the placement rules Amazon ECS holds on a service.
fn service_rows() -> Vec<ServiceRow> {
    vec![
        ServiceRow {
            case: "a named service keeping one task",
            stack: "ecs-named-service",
            declared_name: Some("orders"),
            desired_count: 1,
            extra: json!({}),
        },
        ServiceRow {
            case: "a service the template does not name, keeping two tasks under a declared \
                   strategy",
            stack: "ecs-unnamed-service",
            declared_name: None,
            desired_count: 2,
            extra: json!({
                "SchedulingStrategy": "REPLICA",
                "PlacementStrategies": [{"Field": "attribute:ecs.availability-zone", "Type": "spread"}],
            }),
        },
    ]
}

/// The template one ECS row deploys: the cluster, the task definition its tasks run and the
/// service that keeps them, plus the outputs the stack publishes for the service.
fn service_template(row: &ServiceRow) -> String {
    let mut properties = json!({
        "Cluster": {"Ref": "Cluster"},
        "TaskDefinition": {"Ref": "Task"},
        "DesiredCount": row.desired_count,
    });
    let members = properties
        .as_object_mut()
        .expect("the declaration is an object");
    if let Some(name) = row.declared_name {
        members.insert("ServiceName".to_string(), json!(name));
    }
    if let Value::Object(extra) = &row.extra {
        for (property, value) in extra {
            members.insert(property.clone(), value.clone());
        }
    }
    json!({
        "Resources": {
            "Cluster": {
                "Type": "AWS::ECS::Cluster",
                "Properties": {"ClusterName": format!("{}-cluster", row.stack)},
            },
            "Task": {
                "Type": "AWS::ECS::TaskDefinition",
                "Properties": {
                    "Family": format!("{}-task", row.stack),
                    "ContainerDefinitions": [{
                        "Name": "app",
                        "Image": "public.ecr.aws/nginx/nginx:latest",
                        "Memory": 128,
                    }],
                },
            },
            "Service": {"Type": "AWS::ECS::Service", "Properties": properties},
        },
        "Outputs": {
            "ClusterName": {"Value": {"Ref": "Cluster"}},
            "ServiceArn": {"Value": {"Ref": "Service"}},
            "ServiceName": {"Value": {"Fn::GetAtt": ["Service", "Name"]}},
            "TaskDefinitionArn": {"Value": {"Ref": "Task"}},
        },
    })
    .to_string()
}

#[tokio::test]
async fn a_service_a_stack_declares_is_created_in_ecs() {
    for row in service_rows() {
        let case = row.case;
        let router = router();
        let described = deployed(&router, case, row.stack, &service_template(&row)).await;

        let cluster = output(&described, "ClusterName").unwrap_or_default();
        let name = output(&described, "ServiceName").unwrap_or_default();
        let arn = output(&described, "ServiceArn").unwrap_or_default();
        assert!(
            !name.is_empty() && !cluster.is_empty(),
            "case {case:?}: expected the stack to publish the service and its cluster, got \
             {name:?} in {cluster:?}"
        );
        if let Some(declared) = row.declared_name {
            assert_eq!(
                name, declared,
                "case {case:?}: expected the service to carry the declared name {declared:?}, \
                 got {name:?}"
            );
        }
        assert_eq!(
            arn,
            format!("arn:aws:ecs:{REGION}:000000000000:service/{cluster}/{name}"),
            "case {case:?}: expected the published handle to be the service's own ARN, got \
             {arn:?}"
        );

        // What Amazon ECS itself holds under that handle: the service, with the task
        // definition and the desired count the declaration stated.
        let (status, document) = ecs(
            &router,
            "DescribeServices",
            &json!({"cluster": &cluster, "services": [&name]}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeServices, got {status} with \
             {document}"
        );
        let service = document
            .get("services")
            .and_then(Value::as_array)
            .and_then(|services| services.first())
            .cloned()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected Amazon ECS to hold the service {name:?} the \
                     stack reports created, got {document}"
                )
            });
        assert_eq!(
            service.get("serviceArn").and_then(Value::as_str),
            Some(arn.as_str()),
            "case {case:?}: expected the service Amazon ECS holds to be the one the stack \
             published, got {service}"
        );
        assert_eq!(
            service.get("desiredCount").and_then(Value::as_u64),
            Some(u64::from(row.desired_count)),
            "case {case:?}: expected the desired count {} to be in force, got {service}",
            row.desired_count
        );
        let task_definition = output(&described, "TaskDefinitionArn").unwrap_or_default();
        assert_eq!(
            service.get("taskDefinition").and_then(Value::as_str),
            Some(task_definition.as_str()),
            "case {case:?}: expected the service to run the declared revision \
             {task_definition:?}, got {service}"
        );

        // ...and the service is addressable as one: `ListTasks` narrowed to it is served
        // rather than refused for a service that does not exist.
        let (status, listed) = ecs(
            &router,
            "ListTasks",
            &json!({"cluster": &cluster, "serviceName": &name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListTasks for the stack's service to be served, got \
             {status} with {listed}"
        );
    }
}

// ---------------------------------------------------------------------------
// An attribute the owning service mints: AWS::DynamoDB::Table's StreamArn
// ---------------------------------------------------------------------------

/// The actions a stream poller's execution role needs, as Amazon DynamoDB's own refusal of a
/// mapping names them ("ensure the role can perform the GetRecords, GetShardIterator,
/// DescribeStream, and ListStreams Actions on your stream").
const STREAM_READER_ACTIONS: &[&str] = &[
    "dynamodb:DescribeStream",
    "dynamodb:GetRecords",
    "dynamodb:GetShardIterator",
    "dynamodb:ListStreams",
];

/// The stream view types the model declares
/// (`smithy_query(query="shape",shape="StreamViewType")`), which is what the rows below vary
/// the declared stream along.
const STREAM_VIEW_TYPES: &[&str] = &["NEW_AND_OLD_IMAGES", "KEYS_ONLY", "NEW_IMAGE", "OLD_IMAGE"];

/// The template one stream row deploys: the table whose declaration enables a stream, the
/// function a mapping on that stream invokes, and the mapping itself — whose `EventSourceArn`
/// reads the attribute Amazon DynamoDB minted.
fn stream_template(stack: &str, view_type: &str) -> String {
    json!({
        "Resources": {
            "Table": {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
                    "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
                    "BillingMode": "PAY_PER_REQUEST",
                    "StreamSpecification": {"StreamViewType": view_type},
                },
            },
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": "sts:AssumeRole",
                            "Effect": "Allow",
                            "Principal": {"Service": "lambda.amazonaws.com"},
                        }],
                    },
                },
            },
            "Policy": {
                "Type": "AWS::IAM::Policy",
                "Properties": {
                    "PolicyName": format!("{stack}-stream-reader"),
                    "Roles": [{"Ref": "Role"}],
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": STREAM_READER_ACTIONS,
                            "Effect": "Allow",
                            "Resource": {"Fn::GetAtt": ["Table", "StreamArn"]},
                        }],
                    },
                },
            },
            "Consumer": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Code": {"ZipFile": "def handler(event, context):\n    return {}\n"},
                    "Handler": "index.handler",
                    "Role": {"Fn::GetAtt": ["Role", "Arn"]},
                    "Runtime": "python3.12",
                    "Timeout": 30,
                },
            },
            "Trigger": {
                "Type": "AWS::Lambda::EventSourceMapping",
                "Properties": {
                    "EventSourceArn": {"Fn::GetAtt": ["Table", "StreamArn"]},
                    "FunctionName": {"Ref": "Consumer"},
                    "StartingPosition": "LATEST",
                    "BatchSize": 1,
                },
            },
        },
        "Outputs": {
            "TableName": {"Value": {"Ref": "Table"}},
            "StreamArn": {"Value": {"Fn::GetAtt": ["Table", "StreamArn"]}},
            "MappingId": {"Value": {"Ref": "Trigger"}},
        },
    })
    .to_string()
}

#[tokio::test]
async fn a_stacks_stream_arn_is_published_and_reaches_the_trigger_that_reads_it() {
    for (index, view_type) in STREAM_VIEW_TYPES.iter().enumerate() {
        let case = *view_type;
        let stack = format!("stream-consumer-{index}");
        let router = router();
        let described = deployed(&router, case, &stack, &stream_template(&stack, view_type)).await;

        let table = output(&described, "TableName").unwrap_or_default();
        let stream_arn = output(&described, "StreamArn").unwrap_or_default();
        let mapping = output(&described, "MappingId").unwrap_or_default();
        // The published ARN is the STREAM's own, which nests inside the table's ARN: an
        // output a declaration reads off a minted attribute carries a value, and that value
        // is the one the owning service holds.
        assert!(
            stream_arn.starts_with(&format!(
                "arn:aws:dynamodb:{REGION}:000000000000:table/{table}/stream/"
            )),
            "case {case:?}: expected the stack to publish the stream of the table {table:?}, \
             got {stream_arn:?}"
        );
        assert!(
            !mapping.is_empty(),
            "case {case:?}: expected the stack to publish the mapping's own id, got \
             {mapping:?}"
        );

        // ...and AWS Lambda holds the mapping the stack published, polling exactly that
        // stream for exactly that function.
        let answered = lambda(
            &router,
            "GET",
            &format!("/2015-03-31/event-source-mappings/{mapping}"),
            &[],
            &[],
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected AWS Lambda to hold the mapping {mapping:?}, got {} with \
             {}",
            answered.status, answered.body
        );
        assert_eq!(
            answered.body.get("EventSourceArn").and_then(Value::as_str),
            Some(stream_arn.as_str()),
            "case {case:?}: expected the mapping to poll the published stream, got {}",
            answered.body
        );
    }
}
