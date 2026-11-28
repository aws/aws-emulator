// SPDX-License-Identifier: CC0-1.0

//! What a client gets once a plain-CloudFormation order-intake deployment has settled: an estate
//! whose resources SERVE, on the assembled emulator — the one `core/router/src/lib.rs` builds for
//! the binary.
//!
//! RULE under test: **a resource a settled stack reports created is the OWNING service's live
//! resource, addressed by the identifier the stack published, so the whole declared estate works
//! end to end — a request to the published stage is answered by the function its declared
//! integration names, with that function's own status and body; what that function sends to the
//! declared queue is handed to the function the declared event source mapping names; and that
//! function's item write, its output lines and its datapoint are held by the declared table, the
//! declared log group and the series the declared alarm evaluates, which then reports the state
//! its own declared comparison against its own declared threshold computes over that series.**
//!
//! Why it matters: a deployment is of no use to the client that made it unless the estate behind
//! it answers. The reported case was a deploy whose nine legs — the deploy itself, the wiring, the
//! worker, the logs, the metrics and the alarm among them — were never graded at all, because no
//! HTTP status from the deployed stage and no terminal stack status was ever observed. A stack
//! that settles but whose published stage answers nothing, or whose queue hands nothing over,
//! leaves that client exactly as stuck.
//!
//! Six crates have to agree for the rule to hold (CloudFormation drives the deployment; Amazon API
//! Gateway serves the stage; AWS Lambda runs both functions and stands the mapping's poller up;
//! Amazon SQS hands the batch over; Amazon DynamoDB holds the row; Amazon CloudWatch Logs holds
//! the output; Amazon CloudWatch holds the series and evaluates the alarm), so the suite lives
//! here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: how many orders are submitted through the
//! published stage — which decides how many rows the declared table has to hold, how many lines
//! the declared group has to hold, and what the declared period's `Sum` is worth, so the same
//! declared alarm settles on a different modelled `StateValue` either side of its own threshold.
//!
//! What only this suite reads is the estate's own BEHAVIOUR after the deploy. Neighbours own the
//! legs it deliberately does not repeat:
//! [`super::cfn_order_intake_estate_settles_terminally`] pins that this estate's create, update
//! and teardown always reach a terminal `StackStatus`,
//! [`super::cfn_deployed_sqs_lambda_secret_invocation_is_logged`] what a queue mapping's
//! invocation writes under a function's own group, and
//! [`super::cloudformation_rest_api_serves_its_stage`] that a stack-deployed stage is served by
//! its api's data plane.
//!
//! Nothing is waited out: both estates are asked for before either is polled, and every wait below
//! ends as soon as the condition it is about holds rather than after a fixed span.

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// Every caller this suite drives the emulator through is written ONCE in this test crate — a
// suite that copies one drifts from the others: the awsQuery CloudFormation caller and its
// Region, the whole-emulator router, the CloudWatch, DynamoDB, CloudWatch Logs and AWS Lambda
// callers, the stack-settling wait and the XML readers. All of them are addressed in the one
// Region [`REGION`] names.
use crate::cloudformation_apigatewayv2_resources::{cloudformation, REGION};
use crate::cloudformation_dynamodb_table_in_place_update::dynamodb;
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::cloudformation_lambda_function_log_destination::{lambda, logs};
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{cw, router as emulator};
use crate::query_wire::{elements, outputs_of};

/// The terminal `StackStatus` a deployment that succeeded settles on, and the one its resources
/// are each reported created under.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The capabilities a deployment of a template that declares IAM resources with names of their own
/// has to be made with (`CreateStackInput$Capabilities`).
const IAM_CAPABILITY: &str = "CAPABILITY_IAM";
const NAMED_IAM_CAPABILITY: &str = "CAPABILITY_NAMED_IAM";

/// The three modelled states an alarm reports
/// (`smithy_query(query="shape",shape="StateValue")`: `ALARM | INSUFFICIENT_DATA | OK`).
const STATE_OK: &str = "OK";
const STATE_ALARM: &str = "ALARM";

/// The logical ids the template declares its resources under.
const QUEUE: &str = "OrderQueue";
const TABLE: &str = "OrderTable";
const ROLE: &str = "OrderServiceRole";
const INTAKE: &str = "IntakeFunction";
const WORKER: &str = "WorkerFunction";
const TRIGGER: &str = "WorkerTrigger";
const LOG_GROUP: &str = "WorkerLogGroup";
const ALARM: &str = "FulfilledOrdersAlarm";
const API: &str = "OrdersApi";
const ORDERS_PATH: &str = "OrdersResource";
const ORDERS_METHOD: &str = "OrdersMethod";
const INVOKE_PERMISSION: &str = "IntakeInvokePermission";
const DEPLOYMENT: &str = "OrdersDeployment";
const STAGE: &str = "OrdersStage";

/// The output keys the template publishes the estate's identifiers through — all a client holds to
/// drive the deployment it made.
const INVOKE_URL_OUTPUT: &str = "InvokeUrl";
const TABLE_NAME_OUTPUT: &str = "TableName";
const LOG_GROUP_OUTPUT: &str = "LogGroupName";
const ALARM_NAME_OUTPUT: &str = "AlarmName";

/// The custom series the worker publishes into and the declared alarm watches, and the dimension
/// name it is published under.
const METRIC_NAMESPACE: &str = "OrderService";
const METRIC_NAME: &str = "OrdersFulfilled";
const DIMENSION_NAME: &str = "Service";

/// The alarm the template declares over that series, as it declares it: one datapoint per order
/// fulfilled, so what a period is worth is their `Sum`.
///
/// The period is the grid every read below is answered at as well, and it is wide on purpose.
/// Every datapoint the worker publishes carries ONE fixed time stamp ([`STAMP_VARIABLE`]), so a
/// row's publishes land in one period of this grid whatever pace the host runs the invocations at,
/// and that period is still the one being graded by the time the row reads the alarm however slow
/// the host was — a period as wide as this one is many times every wait below.
///
/// `ActionsEnabled` is declared `false`, as an estate whose alarm is watched rather than wired to
/// an action declares it: that gates the alarm's ACTIONS and not its evaluation, so the state
/// asserted below is still one the series drove.
const ALARM_STATISTIC: &str = "Sum";
const ALARM_PERIOD: i64 = 300;
const ALARM_EVALUATION_PERIODS: i64 = 1;
const ALARM_THRESHOLD: f64 = 2.0;
const ALARM_COMPARISON: &str = "GreaterThanOrEqualToThreshold";
const ALARM_TREAT_MISSING: &str = "missing";
const ALARM_ACTIONS_ENABLED: bool = false;

/// The stage the api is deployed to, and the path part its method is on.
const STAGE_NAME: &str = "prod";
const ORDERS_PATH_PART: &str = "orders";

/// The status the intake function answers a submitted order with, which is therefore the status the
/// published stage answers: the function's own, not the front door's.
const ACCEPTED: u16 = 202;

/// The runtime and handler both declared functions carry, and how long one invocation may run as
/// the function's own `Timeout` — generous, because what is under test is the chain and not how
/// fast an interpreter gets through a handler.
const RUNTIME: &str = "python3.12";
const HANDLER: &str = "index.handler";
const FUNCTION_TIMEOUT: u64 = 120;

/// How long the declared queue hides a delivered message for, as its own `VisibilityTimeout`: past
/// the function's timeout, which is what AWS Lambda requires of a queue it polls on a function's
/// behalf.
const VISIBILITY_TIMEOUT: u64 = 180;

/// How many days the declared group keeps what is written to it, as its own `RetentionInDays`.
const RETENTION_IN_DAYS: i64 = 7;

/// How many records one batch may hold: above every row's order count, so a row's whole burst can
/// be handed over in one invocation — and may equally arrive in several.
const BATCH_SIZE: i64 = 10;

/// The environment variables the two handlers read their estate out of.
const QUEUE_URL_VARIABLE: &str = "QUEUE_URL";
const TABLE_VARIABLE: &str = "TABLE_NAME";
const NAMESPACE_VARIABLE: &str = "METRIC_NAMESPACE";
const METRIC_VARIABLE: &str = "METRIC_NAME";
const DIMENSION_VARIABLE: &str = "METRIC_DIMENSION";
const STAMP_VARIABLE: &str = "STAMP";

/// How long a wait for work the CLOCK owes — a stack settling, a function going `Active`, a
/// mapping's next pass falling due, an invocation, a log delivery, an alarm's evaluation — goes on
/// before calling it a hang, and how long between two looks.
///
/// Generous: what is waited FOR is a condition, looked for again until it holds, so a loaded host
/// delays a row instead of deciding it. Nothing below asserts on elapsed time.
const DEADLINE: Duration = Duration::from_secs(120);
const LOOK_AGAIN: Duration = Duration::from_millis(100);

/// Where Lambda binds the function collection, for the one read that tells this suite a declared
/// function is ready to be invoked.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The handler the intake function runs: it answers in the shape a Lambda proxy integration's own
/// answer takes, and sends the submitted order on to the queue the stack named it through its
/// environment. The SDK and its client are at MODULE scope, the idiom a real function uses.
const INTAKE_HANDLER: &str = r#"
import json
import os

import boto3

QUEUE_URL = os.environ["QUEUE_URL"]
CLIENT = boto3.client("sqs")


def handler(event, context):
    submitted = json.loads(event.get("body") or "{}")
    CLIENT.send_message(QueueUrl=QUEUE_URL, MessageBody=json.dumps(submitted))
    return {
        "statusCode": 202,
        "headers": {"content-type": "application/json"},
        "body": json.dumps({"orderId": submitted.get("orderId"), "status": "accepted"}),
    }
"#;

/// The handler the worker function runs: for each record of the batch it was handed it writes the
/// order into the declared table, writes one structured line about it, and publishes one datapoint
/// of the declared series — so an invocation leaves a row, a line and a datapoint per order.
///
/// The write is CONDITIONAL on the order not already being there, and the line and the datapoint
/// are written only when that write was taken, because a queue hands a record over AT LEAST once:
/// a record delivered twice leaves the one row, the one line and the one datapoint the order is
/// worth, as a real consumer of a queue has to.
const WORKER_HANDLER: &str = r#"
import datetime
import json
import os

import boto3
from botocore.exceptions import ClientError

TABLE = os.environ["TABLE_NAME"]
NAMESPACE = os.environ["METRIC_NAMESPACE"]
METRIC = os.environ["METRIC_NAME"]
DIMENSION = os.environ["METRIC_DIMENSION"]
STAMP = datetime.datetime.utcfromtimestamp(float(os.environ["STAMP"]))


def handler(event, context):
    dynamodb = boto3.client("dynamodb")
    cloudwatch = boto3.client("cloudwatch")
    fulfilled = 0
    for record in event.get("Records") or []:
        order = json.loads(record["body"])
        try:
            dynamodb.put_item(
                TableName=TABLE,
                Item={
                    "orderId": {"S": order["orderId"]},
                    "sku": {"S": order["sku"]},
                    "status": {"S": "fulfilled"},
                },
                ConditionExpression="attribute_not_exists(orderId)",
            )
        except ClientError as refused:
            if refused.response["Error"]["Code"] != "ConditionalCheckFailedException":
                raise
            continue
        print(json.dumps({"event": "fulfilled", "orderId": order["orderId"]}))
        cloudwatch.put_metric_data(
            Namespace=NAMESPACE,
            MetricData=[
                {
                    "MetricName": METRIC,
                    "Dimensions": [{"Name": "Service", "Value": DIMENSION}],
                    "Timestamp": STAMP,
                    "Value": 1.0,
                    "Unit": "Count",
                }
            ],
        )
        fulfilled = fulfilled + 1
    return {"fulfilled": fulfilled}
"#;

/// One row: how many orders are submitted through the published stage, and the state the declared
/// alarm has to settle on once the series holds one datapoint per order.
struct Row {
    case: &'static str,
    /// The orders this row submits, as `(orderId, sku)` — each one a body the stage is POSTed.
    orders: &'static [(&'static str, &'static str)],
    /// The `StateValue` the declared comparison against the declared threshold computes over the
    /// `Sum` those orders leave in the period.
    state: &'static str,
}

/// The catalog. The rows span one submitted order and several, and so a period `Sum` either side
/// of the alarm's own threshold: `ALARM_THRESHOLD` under `GreaterThanOrEqualToThreshold` is not met
/// by one fulfilled order and is met exactly by two. The bodies differ too — one carries a
/// non-ASCII attribute — so what travels the chain is the submitted order and not an ASCII echo of
/// it.
const ROWS: &[Row] = &[
    Row {
        case: "one submitted order: one row, one line, a Sum under the declared threshold",
        orders: &[("order-1", "sku-widget")],
        state: STATE_OK,
    },
    Row {
        case: "two submitted orders, one of them non-ascii: two rows, two lines, a Sum at the \
               declared threshold",
        orders: &[("order-2", "sku-gadget"), ("order-3", "sku-ünïcödé")],
        state: STATE_ALARM,
    },
];

/// The name the template gives its worker function, which is also where the group it declares
/// beside it takes its name from.
fn worker_name(stack: &str) -> String {
    format!("{stack}-worker")
}

/// Wall-clock now as epoch seconds: the instant every datapoint of this run is stamped with, and
/// the centre of every metric read's window.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the host clock is at or past the unix epoch")
        .as_secs_f64()
}

/// POST one order to the stage at the invoke URL the stack PUBLISHED, exactly as a client reaches
/// AWS: the URL is taken apart rather than rebuilt, so what is driven is the address the deployment
/// told its client to use. The method the template declares carries no authorization, so neither
/// does the request.
async fn submit(router: &MultiServiceRouter, invoke_url: &str, body: &str) -> (u16, String) {
    let without_scheme = invoke_url
        .strip_prefix("https://")
        .or_else(|| invoke_url.strip_prefix("http://"))
        .unwrap_or(invoke_url);
    let (host, path) = match without_scheme.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), "/".to_string()),
    };
    let request = http::Request::builder()
        .method("POST")
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_owned()))
        .expect("the test request is well formed");
    let response = Box::pin(router.dispatch(request)).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The trust policy the execution role carries: both declared functions run as it.
fn trust_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "lambda.amazonaws.com" },
            "Action": "sts:AssumeRole"
        }]
    })
}

/// What the execution role is granted: the send the intake makes and the reads the mapping's poller
/// makes on the declared queue, the write the worker makes on the declared table, the delivery its
/// output takes, and the publish of its datapoint.
///
/// `cloudwatch:PutMetricData` supports no resource types in the IAM Service Authorization
/// Reference, so it is granted over every resource, which is the only way to grant it.
fn role_policy() -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": [
                    "sqs:SendMessage",
                    "sqs:ReceiveMessage",
                    "sqs:DeleteMessage",
                    "sqs:GetQueueAttributes"
                ],
                "Resource": { "Fn::GetAtt": [QUEUE, "Arn"] }
            },
            {
                "Effect": "Allow",
                "Action": ["dynamodb:PutItem", "dynamodb:GetItem"],
                "Resource": { "Fn::GetAtt": [TABLE, "Arn"] }
            },
            {
                "Effect": "Allow",
                "Action": [
                    "logs:CreateLogGroup",
                    "logs:CreateLogStream",
                    "logs:PutLogEvents"
                ],
                "Resource": "*"
            },
            {
                "Effect": "Allow",
                "Action": ["cloudwatch:PutMetricData"],
                "Resource": "*"
            }
        ]
    })
}

/// The order-intake estate, written out as plain CloudFormation: a REST API whose `POST /orders`
/// method is a proxy integration on the intake function, the queue that function sends to, the
/// event source mapping that drains it into the worker, the table and the log group the worker
/// writes, and the alarm over the series the worker publishes.
fn order_intake_template(stack: &str, stamp: f64) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "order intake: a REST API in front of a queue a worker drains into a table",
        "Resources": {
            QUEUE: {
                "Type": "AWS::SQS::Queue",
                "Properties": {
                    "QueueName": format!("{stack}-orders"),
                    "VisibilityTimeout": VISIBILITY_TIMEOUT
                }
            },
            TABLE: {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": format!("{stack}-orders"),
                    "BillingMode": "PAY_PER_REQUEST",
                    "AttributeDefinitions": [
                        { "AttributeName": "orderId", "AttributeType": "S" }
                    ],
                    "KeySchema": [{ "AttributeName": "orderId", "KeyType": "HASH" }]
                }
            },
            ROLE: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": format!("{stack}-exec"),
                    "AssumeRolePolicyDocument": trust_policy(),
                    "Policies": [{
                        "PolicyName": format!("{stack}-run-the-order-service"),
                        "PolicyDocument": role_policy()
                    }]
                }
            },
            INTAKE: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": format!("{stack}-intake"),
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Timeout": FUNCTION_TIMEOUT,
                    "Role": { "Fn::GetAtt": [ROLE, "Arn"] },
                    "Environment": {
                        "Variables": { QUEUE_URL_VARIABLE: { "Ref": QUEUE } }
                    },
                    "Code": { "ZipFile": INTAKE_HANDLER }
                }
            },
            WORKER: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": worker_name(stack),
                    "Runtime": RUNTIME,
                    "Handler": HANDLER,
                    "Timeout": FUNCTION_TIMEOUT,
                    "Role": { "Fn::GetAtt": [ROLE, "Arn"] },
                    "Environment": {
                        "Variables": {
                            TABLE_VARIABLE: { "Ref": TABLE },
                            NAMESPACE_VARIABLE: METRIC_NAMESPACE,
                            METRIC_VARIABLE: METRIC_NAME,
                            DIMENSION_VARIABLE: stack,
                            STAMP_VARIABLE: format!("{stamp:.0}")
                        }
                    },
                    "Code": { "ZipFile": WORKER_HANDLER }
                }
            },
            TRIGGER: {
                "Type": "AWS::Lambda::EventSourceMapping",
                "Properties": {
                    "EventSourceArn": { "Fn::GetAtt": [QUEUE, "Arn"] },
                    "FunctionName": { "Ref": WORKER },
                    "BatchSize": BATCH_SIZE,
                    "Enabled": true
                }
            },
            LOG_GROUP: {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {
                    "LogGroupName": format!("/aws/lambda/{}", worker_name(stack)),
                    "RetentionInDays": RETENTION_IN_DAYS
                }
            },
            ALARM: {
                "Type": "AWS::CloudWatch::Alarm",
                "Properties": {
                    "AlarmName": format!("{stack}-fulfilled-orders"),
                    "AlarmDescription": "orders were fulfilled in the window",
                    "Namespace": METRIC_NAMESPACE,
                    "MetricName": METRIC_NAME,
                    "Dimensions": [{ "Name": DIMENSION_NAME, "Value": stack }],
                    "Statistic": ALARM_STATISTIC,
                    "Period": ALARM_PERIOD,
                    "EvaluationPeriods": ALARM_EVALUATION_PERIODS,
                    "Threshold": ALARM_THRESHOLD,
                    "ComparisonOperator": ALARM_COMPARISON,
                    "TreatMissingData": ALARM_TREAT_MISSING,
                    "ActionsEnabled": ALARM_ACTIONS_ENABLED
                }
            },
            API: {
                "Type": "AWS::ApiGateway::RestApi",
                "Properties": {
                    "Name": format!("{stack}-orders-api"),
                    "Description": "the order intake api",
                    "EndpointConfiguration": { "Types": ["REGIONAL"] }
                }
            },
            ORDERS_PATH: {
                "Type": "AWS::ApiGateway::Resource",
                "Properties": {
                    "RestApiId": { "Ref": API },
                    "ParentId": { "Fn::GetAtt": [API, "RootResourceId"] },
                    "PathPart": ORDERS_PATH_PART
                }
            },
            ORDERS_METHOD: {
                "Type": "AWS::ApiGateway::Method",
                "Properties": {
                    "RestApiId": { "Ref": API },
                    "ResourceId": { "Ref": ORDERS_PATH },
                    "HttpMethod": "POST",
                    "AuthorizationType": "NONE",
                    "OperationName": "submitOrder",
                    "MethodResponses": [{ "StatusCode": "202" }],
                    "Integration": {
                        "Type": "AWS_PROXY",
                        "IntegrationHttpMethod": "POST",
                        "Uri": {
                            "Fn::Sub": format!(
                                "arn:aws:apigateway:${{AWS::Region}}:lambda:path/2015-03-31/\
                                 functions/${{{INTAKE}.Arn}}/invocations"
                            )
                        }
                    }
                }
            },
            INVOKE_PERMISSION: {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": "lambda:InvokeFunction",
                    "FunctionName": { "Fn::GetAtt": [INTAKE, "Arn"] },
                    "Principal": "apigateway.amazonaws.com"
                }
            },
            DEPLOYMENT: {
                "Type": "AWS::ApiGateway::Deployment",
                "DependsOn": [ORDERS_METHOD],
                "Properties": {
                    "RestApiId": { "Ref": API },
                    "Description": "the deployment the stage serves"
                }
            },
            STAGE: {
                "Type": "AWS::ApiGateway::Stage",
                "Properties": {
                    "RestApiId": { "Ref": API },
                    "DeploymentId": { "Ref": DEPLOYMENT },
                    "StageName": STAGE_NAME,
                    "Description": "the served stage"
                }
            }
        },
        "Outputs": {
            INVOKE_URL_OUTPUT: {
                "Value": {
                    "Fn::Sub": format!(
                        "https://${{{API}}}.execute-api.${{AWS::Region}}.amazonaws.com/\
                         {STAGE_NAME}/{ORDERS_PATH_PART}"
                    )
                }
            },
            TABLE_NAME_OUTPUT: { "Value": { "Ref": TABLE } },
            LOG_GROUP_OUTPUT: { "Value": { "Ref": LOG_GROUP } },
            ALARM_NAME_OUTPUT: { "Value": { "Ref": ALARM } }
        }
    })
    .to_string()
}

/// Ask for one estate, answering what `CreateStack` was told.
async fn create_stack(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) {
    let (status, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            // The template names its own role: "if you want to create a stack from a template
            // that contains IAM resources with custom names, you must specify
            // CAPABILITY_NAMED_IAM" (`CreateStackInput$Capabilities`).
            ("Capabilities.member.1", IAM_CAPABILITY),
            ("Capabilities.member.2", NAMED_IAM_CAPABILITY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {status} with body {body}"
    );
}

/// Every resource-level failure reason the stack recorded, for an assertion that has to say WHERE a
/// deployment stopped instead of only that it did.
async fn recorded_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, events) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&events, "ResourceStatusReason")
}

/// Wait until the deployment settles on `CREATE_COMPLETE` and answer the outputs it published,
/// which is all a client holds to drive the estate it deployed.
async fn deployed_outputs(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
) -> BTreeMap<String, String> {
    let (reached, described) = wait_for_stack_status(router, stack, CREATE_COMPLETE).await;
    assert_eq!(
        reached, CREATE_COMPLETE,
        "case {case:?}: expected the deployment to settle on {CREATE_COMPLETE}, got {reached} \
         with the recorded reasons {:?} in {described}",
        recorded_reasons(router, stack).await
    );
    outputs_of(&described)
}

/// Wait until AWS Lambda reports `name` `Active`, the state an invocation needs. The span a
/// function spends coming up is that service's own, so no fixed sleep names the instant it ends.
async fn wait_until_active(router: &MultiServiceRouter, case: &str, name: &str) {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let configuration = lambda(
            router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            None,
        )
        .await
        .json();
        if configuration["State"].as_str() == Some("Active") {
            return;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: AWS Lambda never reported {name:?} Active, last read {configuration}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// What DynamoDB answers when the declared table is read for `order_id`: its status and the item
/// it holds, if any. A row the worker has not written yet is an answer carrying no item; anything
/// but 200 is a refusal the caller reports rather than mistakes for a row that has not arrived.
async fn held_order(
    router: &MultiServiceRouter,
    table: &str,
    order_id: &str,
) -> (u16, Option<Value>) {
    let (status, answered) = dynamodb(
        router,
        "GetItem",
        &json!({
            "TableName": table,
            "Key": { "orderId": { "S": order_id } },
            "ConsistentRead": true
        }),
    )
    .await;
    if status != 200 {
        return (status, Some(answered));
    }
    (
        status,
        answered.get("Item").filter(|item| item.is_object()).cloned(),
    )
}

/// Look at the declared table again until it holds a row for every order of the row, answering what
/// it held per order id.
async fn awaited_orders(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    orders: &[(&str, &str)],
) -> BTreeMap<String, Value> {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let mut held = BTreeMap::new();
        let mut refused = Vec::new();
        for (order_id, _) in orders {
            match held_order(router, table, order_id).await {
                (200, Some(item)) => {
                    held.insert((*order_id).to_string(), item);
                }
                (200, None) => {}
                (status, answered) => refused.push(format!(
                    "{order_id}: HTTP {status} with {}",
                    answered.unwrap_or(Value::Null)
                )),
            }
        }
        if held.len() == orders.len() {
            return held;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: expected the declared table {table:?} to hold a row for each of {} \
             submitted order(s), it held {:?} and refused {refused:?}",
            orders.len(),
            held.keys().collect::<Vec<_>>()
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// What CloudWatch Logs answers when the declared group is filtered: its status and every message
/// it reported. A group nothing has written into yet answers no message; anything but 200 is a
/// refusal the caller reports.
async fn logged_messages(router: &MultiServiceRouter, group: &str) -> (u16, Vec<String>) {
    let answered = logs(router, "FilterLogEvents", json!({ "logGroupName": group })).await;
    if answered.status != 200 {
        return (answered.status, Vec::new());
    }
    let messages = answered.json()["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|event| event["message"].as_str().unwrap_or_default().to_string())
        .collect();
    (answered.status, messages)
}

/// Look at the declared group again until it holds a line naming every order of the row, answering
/// the messages last read.
async fn awaited_lines(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    orders: &[(&str, &str)],
) -> Vec<String> {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let (status, messages) = logged_messages(router, group).await;
        let written = orders.iter().all(|(order_id, _)| {
            messages
                .iter()
                .any(|message| message.contains(order_id) && message.contains("fulfilled"))
        });
        if written {
            return messages;
        }
        assert!(
            Instant::now() < giving_up_at,
            "case {case:?}: expected the declared group {group:?} to hold one fulfilled line per \
             submitted order, FilterLogEvents last answered HTTP {status} with {messages:?}"
        );
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// What `GetMetricStatistics` answers for the declared series of `stack` over the window around
/// `stamp`: how many datapoints it reported and the total of their `Sum`.
async fn statistics(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    stamp: f64,
) -> (usize, f64) {
    let answered = cw(
        router,
        case,
        REGION,
        "GetMetricStatistics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": METRIC_NAME,
            "Dimensions": [{ "Name": DIMENSION_NAME, "Value": stack }],
            "StartTime": stamp - 2.0 * ALARM_PERIOD as f64,
            "EndTime": stamp + 2.0 * ALARM_PERIOD as f64,
            "Period": ALARM_PERIOD,
            "Statistics": [ALARM_STATISTIC],
        }),
    )
    .await;
    let points = answered["Datapoints"].as_array().cloned().unwrap_or_default();
    let total = points.iter().filter_map(|point| point["Sum"].as_f64()).sum();
    (points.len(), total)
}

/// What `DescribeAlarms` reports for `name`, looked at again until its `StateValue` is `wanted` or
/// the deadline passes. Answers the whole alarm as it was last read, so an assertion can name both
/// the state and the configuration it was computed under.
async fn awaited_alarm(router: &MultiServiceRouter, case: &str, name: &str, wanted: &str) -> Value {
    let giving_up_at = Instant::now() + DEADLINE;
    loop {
        let described = cw(
            router,
            case,
            REGION,
            "DescribeAlarms",
            &json!({ "AlarmNames": [name] }),
        )
        .await;
        let alarm = described["MetricAlarms"][0].clone();
        if alarm["StateValue"].as_str() == Some(wanted) || Instant::now() >= giving_up_at {
            return alarm;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: the resources a settled deployment reports created are the owning services' live
/// resources, so the declared estate serves end to end — the published stage answers with the
/// integration function's own status and body, the queue that function sends to hands the message
/// to the function the declared mapping names, and that function's row, lines and datapoints are
/// held by the declared table, the declared group and the series the declared alarm evaluates.
#[tokio::test]
async fn the_deployed_order_intake_estate_serves_every_submitted_order() {
    let router = emulator();
    // One instant every datapoint of this run is stamped with, so each row's publishes land in one
    // period of the alarm's own grid however long the host takes over the invocations.
    let stamp = now_secs();

    // Every estate is asked for before either is polled, so the spans their resources take run
    // together rather than one row's after the next row's.
    let mut deployed: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let stack = format!("cfn-order-intake-serves-{index}");
        create_stack(&router, row.case, &stack, &order_intake_template(&stack, stamp)).await;
        deployed.push((row, stack));
    }

    for (row, stack) in &deployed {
        let case = row.case;
        let outputs = deployed_outputs(&router, case, stack).await;
        let published = |name: &str| {
            outputs.get(name).cloned().unwrap_or_else(|| {
                panic!("case {case:?}: the deployment must publish {name}, got {outputs:?}")
            })
        };
        let invoke_url = published(INVOKE_URL_OUTPUT);
        let table = published(TABLE_NAME_OUTPUT);
        let group = published(LOG_GROUP_OUTPUT);
        let alarm_name = published(ALARM_NAME_OUTPUT);

        // Both declared functions are ones AWS Lambda holds and has finished bringing up: the
        // intake is what the stage's integration invokes, and the worker is what the mapping's
        // poller invokes.
        wait_until_active(&router, case, &format!("{stack}-intake")).await;
        wait_until_active(&router, case, &worker_name(stack)).await;

        // The published stage answers every submitted order, with the status and the body the
        // integration function itself returned — which is the HTTP answer a client deploying this
        // estate is waiting for.
        for (order_id, sku) in row.orders {
            let submitted = json!({ "orderId": order_id, "sku": sku }).to_string();
            let (status, answered) = submit(&router, &invoke_url, &submitted).await;
            assert_eq!(
                status, ACCEPTED,
                "case {case:?}: expected the published stage {invoke_url:?} to answer \
                 {ACCEPTED} for {order_id:?}, got {status} with body {answered}"
            );
            let document: Value = serde_json::from_str(&answered).unwrap_or(Value::Null);
            assert_eq!(
                document.get("orderId").and_then(Value::as_str),
                Some(*order_id),
                "case {case:?}: expected the stage's answer to carry the submitted order id \
                 {order_id:?}, got {answered}"
            );
        }

        // The whole chain behind the stage, waited for at its END: the intake's send reaches the
        // declared queue, the declared mapping hands the batch to the worker, and the worker's
        // write is in the declared table — under the attributes the submitted order carried.
        let held = awaited_orders(&router, case, &table, row.orders).await;
        for (order_id, sku) in row.orders {
            let item = held
                .get(*order_id)
                .unwrap_or_else(|| panic!("case {case:?}: no row for {order_id:?} in {held:?}"));
            assert_eq!(
                (
                    item["sku"]["S"].as_str(),
                    item["status"]["S"].as_str(),
                    item["orderId"]["S"].as_str(),
                ),
                (Some(*sku), Some("fulfilled"), Some(*order_id)),
                "case {case:?}: expected the declared table to hold {order_id:?} as the \
                 submitted order the worker fulfilled, got {item}"
            );
        }

        // The same run's output is in the group the template DECLARED, not merely in some group:
        // the wait above ends only once that group holds a fulfilled line naming every order, and
        // its own timeout quotes what the group held instead.
        let messages = awaited_lines(&router, case, &group, row.orders).await;
        assert_eq!(
            group,
            format!("/aws/lambda/{}", worker_name(stack)),
            "case {case:?}: the published group must be the one the template declared, got \
             {group:?} holding {messages:?}"
        );

        // And the datapoints that run published are the declared series: one per order fulfilled,
        // so the period's Sum is the row's own count.
        let expected_sum = row.orders.len() as f64;
        let (points, total) = statistics(&router, case, stack, stamp).await;
        assert!(
            points > 0,
            "case {case:?}: expected the declared series to hold the worker's datapoints, \
             GetMetricStatistics reported none"
        );
        assert_eq!(
            total, expected_sum,
            "case {case:?}: expected the declared period's {ALARM_STATISTIC} to be \
             {expected_sum} over {points} datapoint(s), got {total}"
        );

        // The alarm the deployment created is one Amazon CloudWatch holds, configured exactly as
        // the template declared it, and the state it reports is what its own comparison against
        // its own threshold computes over the series the worker filled — nothing forced it.
        let alarm = awaited_alarm(&router, case, &alarm_name, row.state).await;
        assert_eq!(
            (
                alarm["Statistic"].as_str(),
                alarm["Period"].as_i64(),
                alarm["EvaluationPeriods"].as_i64(),
                alarm["Threshold"].as_f64(),
                alarm["ComparisonOperator"].as_str(),
                alarm["TreatMissingData"].as_str(),
                alarm["ActionsEnabled"].as_bool(),
                alarm["Namespace"].as_str(),
                alarm["MetricName"].as_str(),
            ),
            (
                Some(ALARM_STATISTIC),
                Some(ALARM_PERIOD),
                Some(ALARM_EVALUATION_PERIODS),
                Some(ALARM_THRESHOLD),
                Some(ALARM_COMPARISON),
                Some(ALARM_TREAT_MISSING),
                Some(ALARM_ACTIONS_ENABLED),
                Some(METRIC_NAMESPACE),
                Some(METRIC_NAME),
            ),
            "case {case:?}: expected the alarm to be held as the template declared it, got {alarm}"
        );
        assert_eq!(
            alarm["StateValue"].as_str(),
            Some(row.state),
            "case {case:?}: expected the declared alarm over a {ALARM_STATISTIC} of \
             {expected_sum} against its threshold of {ALARM_THRESHOLD} to report {:?}, got \
             {alarm}",
            row.state
        );
    }
}
