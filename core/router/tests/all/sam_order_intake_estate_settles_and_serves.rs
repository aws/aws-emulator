// SPDX-License-Identifier: CC0-1.0

//! A SERVERLESS application deploy — one `AWS::Serverless-2016-10-31` template carrying the
//! whole order-intake estate — always reaches an OBSERVABLE outcome on the assembled emulator,
//! and the route its functions only IMPLY is then served.
//!
//! RULE under test: **a template whose `Transform` section names the AWS SAM macro, submitted
//! with `CAPABILITY_AUTO_EXPAND`, is PROCESSED by that macro before the stack creates anything,
//! so the deployment leaves the `*_IN_PROGRESS` status it starts in for one of the TERMINAL
//! members of `com.amazonaws.cloudformation#StackStatus` in bounded time — however many
//! serverless declarations, dependency levels and owning services the template has — every read
//! on the deploy path (`DescribeStacks`, `DescribeStackEvents`, `DescribeStackResources`) is
//! answered at every poll while the operation is open, the resources the declarations' `Events`
//! blocks only imply (the REST api with its deployment and stage, the `AWS::Lambda::Permission`
//! that admits the api's invoke, the `AWS::Lambda::EventSourceMapping` that drains a queue) are
//! created in the services that own them under an identifier each of those services assigned,
//! and no `AWS::Serverless::*` type survives into the deployed stack, that type being the
//! macro's INPUT.** The route the `Api` event declares is then answered at the stage's own
//! invoke URL, and a record the handler behind it publishes reaches the function the queue
//! event mapped, whose own write lands in the table the same template declared.
//!
//! Why it matters: every waiter written against `DescribeStacks` — `aws cloudformation wait
//! stack-create-complete`, the SDK `StackCreateComplete` waiters, `sam deploy` itself — ends only
//! when the stack leaves `*_IN_PROGRESS`. A serverless deployment that neither converges nor
//! fails gives that client no deployed estate AND no diagnosable failure: it blocks until its own
//! timeout with no status, no rollback reason, and nothing of the application reachable. The
//! reported case was exactly that, on an order-intake SAM template whose expansion spans six
//! owning services.
//!
//! Six crates have to agree for the rule to hold (CloudFormation expands the macro and drives the
//! deployment, and Amazon API Gateway, AWS Lambda, Amazon SQS, Amazon DynamoDB, Amazon CloudWatch
//! Logs and Amazon CloudWatch each own resources the expansion creates and each decide when
//! theirs is ready), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: how much of the estate one serverless
//! template declares (a single routed function; the whole intake estate of a queue, a simple
//! table, two functions, the group one logs into and an alarm), whether the api the `Api` event
//! routes through is the IMPLICIT one the macro synthesizes or an `AWS::Serverless::Api` the
//! template declares on a stage of its own, and the verb and path depth of the declared route.
//!
//! Neighbours own the legs this suite deliberately does not repeat:
//! [`super::cfn_order_intake_estate_settles_terminally`] holds the same rule for a template
//! carrying no transform at all (and owns the catalogue of terminal statuses both read),
//! [`super::cloudformation_rest_api_serves_its_stage`] pins which api a stack's stage is served
//! by, [`super::cloudformation_provisions_lambda_event_source_mappings`] pins what the macro
//! expands a pull event INTO, and [`super::sam_pipeline_writes_a_row_for_every_uploaded_key`]
//! pins the event-driven leg of an expanded function. What only this suite reads is those legs
//! JOINED: the CONVERGENCE of a whole serverless estate and the application path through it.
//!
//! Nothing is waited out: every stack is asked for before any is polled, each wait ends as soon
//! as the condition it is on holds, and every deadline is generous — all a deadline is for here
//! is that the wait terminates.

use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery CloudFormation caller, the DynamoDB and CloudWatch callers, the whole-emulator
// router, the sized thread a nested in-process chain is driven on, the wait for a function to
// come up and the XML readers are each written ONCE in this test crate: a suite that copies one
// drifts from the others.
use crate::cfn_order_intake_estate_settles_terminally::{
    described_alarm, recorded_reasons, reported_resources, settled_status,
};
use crate::cloudformation_apigatewayv2_resources::cloudformation;
use crate::cloudformation_dynamodb_table_in_place_update::dynamodb;
use crate::cloudformation_lambda_function_log_destination::wait_until_active;
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::router as emulator;
use crate::query_wire::{opt_element, outputs_of, physical_ids_of};
use crate::s3_notification_invokes_its_lambda_target::driven;

/// The macro the AWS Serverless Application Model is published as, as a template's `Transform`
/// section names it, and the capabilities a client acknowledges a template declaring a macro and
/// the execution roles that macro synthesizes with.
const SAM_TRANSFORM: &str = "AWS::Serverless-2016-10-31";
const CAPABILITY_IAM: &str = "CAPABILITY_IAM";
const CAPABILITY_AUTO_EXPAND: &str = "CAPABILITY_AUTO_EXPAND";

/// The namespace the macro's INPUT types live under: no resource of a deployed stack may carry
/// it, because the expansion consumed every such declaration.
const SERVERLESS_TYPE_PREFIX: &str = "AWS::Serverless::";

/// The terminal member the cases below name; the catalogue a settled status must come from is the
/// sibling suite's, which [`settled_status`] holds every poll to.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The reads on the deploy path, every one of which has to answer while a stack operation is
/// open: `DescribeStacks` is the one being polled, and these are the two a client reads beside it
/// to find out WHERE a deployment is.
const DEPLOY_READS: [&str; 2] = ["DescribeStackEvents", "DescribeStackResources"];

/// How often the table is asked whether the worker's write has landed, and how long it goes on
/// being asked.
///
/// The bound is a wall-clock deadline rather than a count of reads, and it is generous: what is
/// waited FOR runs on the spans the owning services choose, so all the deadline is for is that
/// the wait TERMINATES.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const LANDED_DEADLINE: Duration = Duration::from_secs(120);

/// The logical ids the serverless templates below declare their resources under.
const QUEUE: &str = "OrderQueue";
const TABLE: &str = "OrderTable";
const INTAKE: &str = "IntakeFunction";
const WORKER: &str = "WorkerFunction";
const LOG_GROUP: &str = "WorkerLogGroup";
const ALARM: &str = "FulfilledOrdersAlarm";
const API: &str = "OrdersApi";

/// The logical id the macro gives the api it synthesizes for an `Api` event that names no
/// `RestApiId` ("ServerlessRestApi"), and the stage that implicit api is deployed to ("Prod").
/// Both are the AWS SAM Developer Guide's own ("Generated AWS CloudFormation resources").
const IMPLICIT_API: &str = "ServerlessRestApi";
const IMPLICIT_STAGE: &str = "Prod";

/// The output keys the templates publish: the invoke URL a client posts an order to, and the
/// identifiers of the resources whose own services are then read.
const INVOKE_URL_OUTPUT: &str = "InvokeUrl";
const TABLE_NAME_OUTPUT: &str = "TableName";
const ALARM_NAME_OUTPUT: &str = "AlarmName";

/// The alarm the estate declares on the worker's own metric.
const ALARM_NAMESPACE: &str = "OrderService";
const ALARM_METRIC: &str = "FulfilledOrders";
const ALARM_PERIOD: u64 = 300;
const ALARM_EVALUATION_PERIODS: u64 = 1;

/// The attribute the order table is keyed on, and the attribute an order carries beside it.
const ORDER_KEY: &str = "orderId";
const ORDER_SKU: &str = "sku";

/// The status the worker records on an order it has fulfilled, and the status the intake answers
/// the client with.
const FULFILLED: &str = "fulfilled";
const ACCEPTED: &str = "accepted";

/// The status code the intake handler answers, which the stage serves as its own.
const ACCEPTED_STATUS: u16 = 202;

/// The resource types a template declaring ONE routed serverless function deploys to: the
/// function, the execution role the macro synthesizes for it, the permission that admits the
/// api's invoke, and the api trio the `Api` event implies.
const ROUTED_FUNCTION_TYPES: &[&str] = &[
    "AWS::ApiGateway::Deployment",
    "AWS::ApiGateway::RestApi",
    "AWS::ApiGateway::Stage",
    "AWS::IAM::Role",
    "AWS::Lambda::Function",
    "AWS::Lambda::Permission",
];

/// The resource types the whole order-intake estate deploys to: the above, plus the queue, the
/// table the simple-table declaration becomes, the mapping the queue event implies, the group the
/// worker logs into and the alarm on its metric.
const ESTATE_TYPES: &[&str] = &[
    "AWS::ApiGateway::Deployment",
    "AWS::ApiGateway::RestApi",
    "AWS::ApiGateway::Stage",
    "AWS::CloudWatch::Alarm",
    "AWS::DynamoDB::Table",
    "AWS::IAM::Role",
    "AWS::Lambda::EventSourceMapping",
    "AWS::Lambda::Function",
    "AWS::Lambda::Permission",
    "AWS::Logs::LogGroup",
    "AWS::SQS::Queue",
];

/// The handler the intake function runs: it publishes the submitted order to the queue the
/// deployment named it and answers the proxy envelope the client is served.
const INTAKE_HANDLER: &str = r#"
import json
import os

import boto3


def handler(event, context):
    submitted = json.loads(event.get("body") or "{}")
    boto3.client("sqs").send_message(
        QueueUrl=os.environ["QUEUE_URL"],
        MessageBody=json.dumps(submitted),
    )
    return {
        "statusCode": 202,
        "headers": {"content-type": "application/json"},
        "body": json.dumps({"orderId": submitted["orderId"], "status": "accepted"}),
    }
"#;

/// The handler the worker function runs: one row per record the mapping hands it, written into
/// the table the deployment named it.
const WORKER_HANDLER: &str = r#"
import json
import os

import boto3


def handler(event, context):
    client = boto3.client("dynamodb")
    table = os.environ["TABLE_NAME"]
    fulfilled = 0
    for record in event.get("Records") or []:
        order = json.loads(record["body"])
        client.put_item(
            TableName=table,
            Item={
                "orderId": {"S": order["orderId"]},
                "sku": {"S": order["sku"]},
                "status": {"S": "fulfilled"},
            },
        )
        fulfilled += 1
    return {"fulfilled": fulfilled}
"#;

/// The handler a routed function alone runs: the proxy envelope, with nothing behind it.
const ROUTED_HANDLER: &str = r#"
import json


def handler(event, context):
    submitted = json.loads(event.get("body") or "{}")
    return {
        "statusCode": 202,
        "headers": {"content-type": "application/json"},
        "body": json.dumps({"orderId": submitted["orderId"], "status": "accepted"}),
    }
"#;

/// One row of the table: how much of the estate its serverless template declares, which api its
/// route is served by, and where that route is.
struct Row {
    case: &'static str,
    /// `Some(stage)` where the template DECLARES its api under [`API`] with that `StageName`,
    /// `None` where the route goes through the implicit api the macro synthesizes on
    /// [`IMPLICIT_STAGE`].
    declared_api_stage: Option<&'static str>,
    /// The resource path the `Api` event declares, as that event's `Path` spells one.
    path: &'static str,
    /// The verb the `Api` event declares, as that event's `Method` spells one.
    method: &'static str,
    /// Whether the template declares the whole intake estate or the routed function alone.
    whole_estate: bool,
    /// Every resource type the deployed stack must report — sorted and deduplicated, so a row
    /// states the type SET its template expands to rather than the logical ids the macro
    /// synthesizes, which `services/service-com-amazonaws-cloudformation-cloudformation`'s own
    /// `sam_transform` suite owns.
    types: &'static [&'static str],
}

const ROWS: &[Row] = &[
    Row {
        case: "one serverless function routed through the implicit api",
        declared_api_stage: None,
        path: "/orders",
        method: "post",
        whole_estate: false,
        types: ROUTED_FUNCTION_TYPES,
    },
    Row {
        case: "the order-intake estate behind the implicit api",
        declared_api_stage: None,
        path: "/orders",
        method: "post",
        whole_estate: true,
        types: ESTATE_TYPES,
    },
    Row {
        case: "the same estate behind a declared Api on its own stage, at a nested path and \
               another verb",
        declared_api_stage: Some("live"),
        path: "/v1/orders",
        method: "put",
        whole_estate: true,
        types: ESTATE_TYPES,
    },
];

impl Row {
    /// The logical id the route's api is addressed by, and the stage it is served on: the
    /// template's own where it declares the api, the macro's where the api is implicit.
    fn api_id(&self) -> &str {
        match self.declared_api_stage {
            Some(_) => API,
            None => IMPLICIT_API,
        }
    }

    fn stage(&self) -> &str {
        self.declared_api_stage.unwrap_or(IMPLICIT_STAGE)
    }

    /// The verb a client reaches the route with, which is the one the event declared.
    fn http_method(&self) -> String {
        self.method.to_uppercase()
    }
}

/// The name the estate gives its worker function, which is also where the group it declares
/// beside it takes its name from.
fn worker_name(stack: &str) -> String {
    format!("{stack}-worker")
}

/// The serverless template a row deploys.
fn template(stack: &str, row: &Row) -> String {
    let mut event = json!({ "Path": row.path, "Method": row.method });
    if row.declared_api_stage.is_some() {
        event["RestApiId"] = json!({ "Ref": API });
    }

    let mut resources = serde_json::Map::new();
    if let Some(stage) = row.declared_api_stage {
        resources.insert(
            API.to_string(),
            json!({
                "Type": "AWS::Serverless::Api",
                "Properties": {
                    "StageName": stage,
                    "Name": format!("{stack}-orders-api"),
                    "Description": "the order intake api"
                }
            }),
        );
    }

    let mut outputs = serde_json::Map::new();
    outputs.insert(
        INVOKE_URL_OUTPUT.to_string(),
        json!({
            "Value": {
                "Fn::Sub": format!(
                    "https://${{{}}}.execute-api.${{AWS::Region}}.amazonaws.com/{}{}",
                    row.api_id(),
                    row.stage(),
                    row.path
                )
            }
        }),
    );

    if row.whole_estate {
        resources.insert(
            QUEUE.to_string(),
            json!({
                "Type": "AWS::SQS::Queue",
                "Properties": {
                    "QueueName": format!("{stack}-orders"),
                    "VisibilityTimeout": 30
                }
            }),
        );
        resources.insert(
            TABLE.to_string(),
            json!({
                "Type": "AWS::Serverless::SimpleTable",
                "Properties": {
                    "TableName": format!("{stack}-orders"),
                    "PrimaryKey": { "Name": ORDER_KEY, "Type": "String" }
                }
            }),
        );
        resources.insert(
            INTAKE.to_string(),
            json!({
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 30,
                    "InlineCode": INTAKE_HANDLER,
                    "Environment": { "Variables": { "QUEUE_URL": { "Ref": QUEUE } } },
                    "Policies": [{
                        "SQSSendMessagePolicy": {
                            "QueueName": { "Fn::GetAtt": [QUEUE, "QueueName"] }
                        }
                    }],
                    "Events": { "Submit": { "Type": "Api", "Properties": event } }
                }
            }),
        );
        resources.insert(
            WORKER.to_string(),
            json!({
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "FunctionName": worker_name(stack),
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 30,
                    "InlineCode": WORKER_HANDLER,
                    "Environment": { "Variables": { "TABLE_NAME": { "Ref": TABLE } } },
                    "Policies": [
                        {
                            "SQSPollerPolicy": {
                                "QueueName": { "Fn::GetAtt": [QUEUE, "QueueName"] }
                            }
                        },
                        { "DynamoDBCrudPolicy": { "TableName": { "Ref": TABLE } } }
                    ],
                    "Events": {
                        "Orders": {
                            "Type": "SQS",
                            "Properties": {
                                "Queue": { "Fn::GetAtt": [QUEUE, "Arn"] },
                                "BatchSize": 1
                            }
                        }
                    }
                }
            }),
        );
        resources.insert(
            LOG_GROUP.to_string(),
            json!({
                "Type": "AWS::Logs::LogGroup",
                "Properties": {
                    "LogGroupName": format!("/aws/lambda/{}", worker_name(stack)),
                    "RetentionInDays": 7
                }
            }),
        );
        resources.insert(
            ALARM.to_string(),
            json!({
                "Type": "AWS::CloudWatch::Alarm",
                "Properties": {
                    "AlarmName": format!("{stack}-fulfilled-orders"),
                    "AlarmDescription": "no order was fulfilled in the window",
                    "Namespace": ALARM_NAMESPACE,
                    "MetricName": ALARM_METRIC,
                    "Dimensions": [{ "Name": "Service", "Value": stack }],
                    "Statistic": "Sum",
                    "Period": ALARM_PERIOD,
                    "EvaluationPeriods": ALARM_EVALUATION_PERIODS,
                    "Threshold": 1.0,
                    "ComparisonOperator": "LessThanThreshold",
                    "TreatMissingData": "notBreaching"
                }
            }),
        );
        outputs.insert(
            TABLE_NAME_OUTPUT.to_string(),
            json!({ "Value": { "Ref": TABLE } }),
        );
        outputs.insert(
            ALARM_NAME_OUTPUT.to_string(),
            json!({ "Value": { "Ref": ALARM } }),
        );
    } else {
        resources.insert(
            INTAKE.to_string(),
            json!({
                "Type": "AWS::Serverless::Function",
                "Properties": {
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 30,
                    "InlineCode": ROUTED_HANDLER,
                    "Events": { "Submit": { "Type": "Api", "Properties": event } }
                }
            }),
        );
    }

    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Transform": SAM_TRANSFORM,
        "Description": "order intake, authored as a serverless application",
        "Resources": resources,
        "Outputs": outputs
    })
    .to_string()
}

/// Ask for `stack` to be created from `template`, and answer the `StackId` the call published.
async fn create_stack(router: &MultiServiceRouter, case: &str, stack: &str, body: &str) -> String {
    let (http, answered) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", body),
            // A template declaring a macro is acknowledged with CAPABILITY_AUTO_EXPAND, and the
            // execution roles the macro synthesizes for its functions with CAPABILITY_IAM.
            ("Capabilities.member.1", CAPABILITY_IAM),
            ("Capabilities.member.2", CAPABILITY_AUTO_EXPAND),
        ],
    )
    .await;
    assert_eq!(
        http, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {http} with body {answered}"
    );
    let stack_id = opt_element(&answered, "StackId").unwrap_or_default();
    assert!(
        !stack_id.is_empty(),
        "case {case:?}: expected CreateStack to publish a StackId, got {answered}"
    );
    stack_id
}

/// A required output of a settled stack.
fn published(case: &str, described: &str, key: &str) -> String {
    outputs_of(described)
        .get(key)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the stack published no {key} output in {described}"))
}

/// Reach a deployed stage at the invoke URL the stack published, exactly as a client reaches AWS:
/// the host is the api's own `{rest-api-id}.execute-api.{region}.amazonaws.com` endpoint and the
/// path is the stage and the resource path. The URL is taken APART rather than rebuilt, so what
/// is driven is the value the stack told the caller to use.
async fn invoke(
    router: &MultiServiceRouter,
    invoke_url: &str,
    method: &str,
    body: &str,
) -> (u16, String) {
    let without_scheme = invoke_url
        .strip_prefix("https://")
        .or_else(|| invoke_url.strip_prefix("http://"))
        .unwrap_or(invoke_url);
    let (host, path) = match without_scheme.split_once('/') {
        Some((host, path)) => (host.to_string(), format!("/{path}")),
        None => (without_scheme.to_string(), "/".to_string()),
    };
    let request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header("content-type", "application/json")
        .body(Bytes::from(body.to_owned()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    (status, String::from_utf8_lossy(response.body()).to_string())
}

/// The item the table holds under `order`, polled until it is there or the deadline passes: the
/// worker's write is owed from the instant the mapping hands it the record, which no fixed sleep
/// names.
async fn awaited_item(
    router: &MultiServiceRouter,
    case: &str,
    table: &str,
    order: &str,
) -> Value {
    let request = json!({
        "TableName": table,
        "Key": { ORDER_KEY: { "S": order } },
        "ConsistentRead": true,
    });
    let giving_up_at = tokio::time::Instant::now() + LANDED_DEADLINE;
    loop {
        let (http, read) = dynamodb(router, "GetItem", &request).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected HTTP 200 from GetItem on {table}, got {http} with {read}"
        );
        if let Some(item) = read.get("Item") {
            return item.clone();
        }
        assert!(
            tokio::time::Instant::now() < giving_up_at,
            "case {case:?}: the table {table} still held no order {order:?} after \
             {LANDED_DEADLINE:?}, so the queue's records never reached the worker: {read}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every serverless estate settles on a modelled terminal `StackStatus`, the deploy path answers
/// at every poll while it does, and what the stack then reports is the macro's EXPANSION — every
/// implied resource created in the service that owns it, and no `AWS::Serverless::*` type left.
///
/// Driven on a sized thread; see [`driven`].
#[test]
fn every_serverless_estate_settles_on_a_modelled_terminal_stack_status() {
    driven("sam-order-intake-settles", || {
        Box::pin(every_serverless_estate_settles_on_a_modelled_terminal_stack_status_body())
    });
}

async fn every_serverless_estate_settles_on_a_modelled_terminal_stack_status_body() {
    let router = emulator();

    // Every stack is asked for before any is polled, so the spans their resources take run
    // together rather than one row's after the next row's.
    let mut deployed: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let stack = format!("sam-order-intake-{index}");
        let body = template(&stack, row);
        create_stack(&router, row.case, &stack, &body).await;
        deployed.push((row, stack));
    }

    for (row, stack) in &deployed {
        let case = row.case;
        let (settled, described) = settled_status(&router, case, stack, &DEPLOY_READS).await;
        assert_eq!(
            settled, CREATE_COMPLETE,
            "case {case:?}: expected the serverless deployment to settle in {CREATE_COMPLETE}, \
             got {settled:?} in {described} with the recorded reasons {:?}",
            recorded_reasons(&router, stack).await
        );

        let (http, resources) =
            cloudformation(&router, "DescribeStackResources", &[("StackName", stack)]).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {http} with body \
             {resources}"
        );
        let reported = reported_resources(&resources);
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected the deployed stack to report the expansion's resources, got \
             {resources}"
        );

        // The expansion is what deployed: every resource is created, under an identifier the
        // owning service assigned, and the type SET is the one the macro expands this template
        // to.
        for (logical, (kind, status, physical)) in &reported {
            assert_eq!(
                status, CREATE_COMPLETE,
                "case {case:?}: expected {logical} ({kind}) to be {CREATE_COMPLETE} once the \
                 deployment completed, got {status:?} in {resources}"
            );
            assert!(
                !physical.is_empty(),
                "case {case:?}: expected {logical} ({kind}) to report the identifier its owning \
                 service assigned, got {physical:?} in {resources}"
            );
            assert!(
                !kind.starts_with(SERVERLESS_TYPE_PREFIX),
                "case {case:?}: expected no {SERVERLESS_TYPE_PREFIX}* type to survive the \
                 expansion, got {logical} as {kind:?} in {resources}"
            );
        }
        let mut kinds: Vec<&str> = reported
            .values()
            .map(|(kind, _, _)| kind.as_str())
            .collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(
            kinds, row.types,
            "case {case:?}: expected the deployment to hold exactly the types the expansion \
             creates, got {kinds:?} in {resources}"
        );

        // The estate's alarm is one Amazon CloudWatch HOLDS, configured as the template declared
        // it: a resource reported created is a resource the owning service answers for.
        if row.whole_estate {
            let name = published(case, &described, ALARM_NAME_OUTPUT);
            let alarm = described_alarm(&router, case, &name)
                .await
                .unwrap_or_else(|| {
                    panic!("case {case:?}: CloudWatch reported no alarm named {name:?}")
                });
            assert_eq!(
                alarm.get("Namespace").and_then(Value::as_str),
                Some(ALARM_NAMESPACE),
                "case {case:?}: expected the alarm to watch the declared namespace, got {alarm}"
            );
            assert_eq!(
                alarm.get("MetricName").and_then(Value::as_str),
                Some(ALARM_METRIC),
                "case {case:?}: expected the alarm to watch the declared metric, got {alarm}"
            );
            assert_eq!(
                alarm.get("Period").and_then(Value::as_u64),
                Some(ALARM_PERIOD),
                "case {case:?}: expected the alarm to echo the declared period, got {alarm}"
            );
        }
    }
}

/// One order a row submits: the id it is keyed on and the item it carries.
struct Order {
    id: &'static str,
    sku: &'static str,
}

const ORDERS: &[Order] = &[
    Order {
        id: "order-1",
        sku: "widget",
    },
    Order {
        id: "order-2",
        sku: "a sku with spaces",
    },
];

/// The route a serverless estate's `Api` event declares is SERVED at the stage's own invoke URL,
/// and what the handler behind it published reaches the function the queue event mapped, whose
/// write lands in the table the same template declared.
///
/// Driven on a sized thread; see [`driven`]: this nests CloudFormation's calls into six services,
/// the stage's integration into Lambda, the handler's own publication into Amazon SQS and the
/// mapping's delivery back into Lambda.
#[test]
fn a_serverless_estates_route_is_served_and_its_queue_worker_lands_every_order() {
    driven("sam-order-intake-serves", || {
        Box::pin(a_serverless_estates_route_is_served_and_its_queue_worker_lands_every_order_body())
    });
}

async fn a_serverless_estates_route_is_served_and_its_queue_worker_lands_every_order_body() {
    let router = emulator();

    // Only the rows that declare the whole estate have a queue and a worker behind their route.
    let estates: Vec<&Row> = ROWS.iter().filter(|row| row.whole_estate).collect();
    let mut deployed: Vec<(&Row, String)> = Vec::new();
    for (index, row) in estates.iter().enumerate() {
        let stack = format!("sam-order-flow-{index}");
        let body = template(&stack, row);
        create_stack(&router, row.case, &stack, &body).await;
        deployed.push((row, stack));
    }

    for (row, stack) in &deployed {
        let case = row.case;
        let (settled, described) = settled_status(&router, case, stack, &DEPLOY_READS).await;
        assert_eq!(
            settled, CREATE_COMPLETE,
            "case {case:?}: expected the serverless deployment to settle in {CREATE_COMPLETE}, \
             got {settled:?} with the recorded reasons {:?}",
            recorded_reasons(&router, stack).await
        );

        let invoke_url = published(case, &described, INVOKE_URL_OUTPUT);
        let table = published(case, &described, TABLE_NAME_OUTPUT);

        // The function behind the route is one AWS Lambda has finished bringing up: the span it
        // spends coming up is that service's own, so the wait is on what it reports.
        let (http, resources) =
            cloudformation(&router, "DescribeStackResources", &[("StackName", stack)]).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected HTTP 200 from DescribeStackResources, got {http} with body \
             {resources}"
        );
        let intake = physical_ids_of(&resources)
            .get(INTAKE)
            .cloned()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the stack published no physical id for {INTAKE} in {resources}")
            });
        wait_until_active(&router, case, &intake).await;

        for order in ORDERS {
            let submitted = json!({ ORDER_KEY: order.id, ORDER_SKU: order.sku }).to_string();
            let (status, answered) =
                invoke(&router, &invoke_url, &row.http_method(), &submitted).await;
            assert_eq!(
                status, ACCEPTED_STATUS,
                "case {case:?}: expected the deployed stage to answer HTTP {ACCEPTED_STATUS} at \
                 {} {invoke_url} for {:?}, got {status} with {answered:?}",
                row.http_method(),
                order.id
            );
            let served: Value = serde_json::from_str(&answered).unwrap_or(Value::Null);
            assert_eq!(
                served.get(ORDER_KEY).and_then(Value::as_str),
                Some(order.id),
                "case {case:?}: expected the intake's own answer for {:?}, got {answered:?}",
                order.id
            );
            assert_eq!(
                served.get("status").and_then(Value::as_str),
                Some(ACCEPTED),
                "case {case:?}: expected the intake to report the order {ACCEPTED}, got \
                 {answered:?}"
            );
        }

        // What the stage answered is work the rest of the estate really did: every order the
        // intake published reached the worker through the queue the mapping drains, and the
        // worker's write is in the table the deployment published.
        for order in ORDERS {
            let item = awaited_item(&router, case, &table, order.id).await;
            assert_eq!(
                item,
                json!({
                    ORDER_KEY: { "S": order.id },
                    ORDER_SKU: { "S": order.sku },
                    "status": { "S": FULFILLED },
                }),
                "case {case:?}: expected the worker's own row for {:?} in {table}, got {item}",
                order.id
            );
        }
    }
}
