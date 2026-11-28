// SPDX-License-Identifier: CC0-1.0

//! A plain-CloudFormation application deploy always reaches an OBSERVABLE outcome on the
//! assembled emulator — the one `core/router/src/lib.rs` builds for the binary.
//!
//! RULE under test: **a stack-level operation — `CreateStack`, `UpdateStack`, `DeleteStack` —
//! leaves the `*_IN_PROGRESS` status it starts in for one of the TERMINAL members of
//! `com.amazonaws.cloudformation#StackStatus` in bounded time, however many resources,
//! dependency levels and owning services the template declares, and every read on the deploy
//! path (`DescribeStacks`, `DescribeStackEvents`, `DescribeStackResources`) is answered at every
//! poll while the operation is open; a declared resource whose create the owning service refuses
//! settles the stack on a modelled FAILURE status, with a `ResourceStatusReason` recorded on the
//! `CREATE_FAILED` event of the logical resource that failed.** Whichever way it ends, a client
//! polling the stack has something to branch on.
//!
//! Why it matters: every waiter written against `DescribeStacks` — the AWS CLI's
//! `cloudformation wait stack-create-complete`, the SDK `StackCreateComplete` waiters, any CI
//! deploy script — ends only when the stack leaves `*_IN_PROGRESS`. A deployment that neither
//! converges nor fails gives that client no deployed estate AND no diagnosable failure: it blocks
//! until its own timeout with no status, no rollback reason and nothing of the estate reachable.
//! The reported case was exactly that, on an order-intake template whose resources span six
//! owning services.
//!
//! Six crates have to agree for the rule to hold (CloudFormation drives the deployment, and
//! Amazon API Gateway, AWS Lambda, Amazon SQS, Amazon DynamoDB, Amazon CloudWatch Logs and
//! Amazon CloudWatch each own resources the template declares and each decide when theirs is
//! ready), so the suite lives here rather than in any one of them.
//!
//! The rows differ along what the rule generalises over: how many resources the template
//! declares (one, three, fourteen), how many DEPENDENCY LEVELS they form (one, two, five — the
//! deepest is the api's `RestApi -> Resource -> Method -> Deployment -> Stage` chain), how many
//! owning services have to answer (one, three, six), and whether a declared resource's create is
//! REFUSED, which is the other way a deployment ends. The lifecycle test then drives the same
//! estate's `UpdateStack` and `DeleteStack`, the other two stack-level operations the rule is
//! stated about.
//!
//! What only this suite reads is the CONVERGENCE of the whole estate and the answering of the
//! deploy path. Neighbours own the legs it deliberately does not repeat:
//! [`super::cloudformation_rest_api_serves_its_stage`] pins that a deployed stage serves its
//! integration, [`super::cfn_ddb_stream_esm_lambda_pipeline`] that the records of a declared
//! source reach the declared function, and
//! [`super::cloudformation_lambda_mapping_invalid_request_reason`] the WORDING a refused
//! mapping's reason carries.
//!
//! Nothing is waited out: every stack is asked for before any is polled, and each wait ends as
//! soon as the stack has left `*_IN_PROGRESS` rather than after a fixed span.

use std::collections::BTreeMap;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery CloudFormation caller, the whole-emulator router, the CloudWatch caller and the
// XML readers are each written ONCE in this test crate: a suite that copies one drifts from the
// others.
use crate::cloudformation_apigatewayv2_resources::{cloudformation, REGION};
use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{cw, router as emulator};
use crate::query_wire::{elements, members_of, opt_element, outputs_of};

/// Every TERMINAL member of `com.amazonaws.cloudformation#StackStatus` — the whole enum less the
/// members whose names end in `_IN_PROGRESS`. Source:
/// `smithy_query(query="shape",shape="StackStatus")`.
///
/// `pub(crate)` because the sibling suite
/// [`crate::sam_order_intake_estate_settles_and_serves`] states the same rule about a template
/// the AWS SAM macro processes first: one catalogue for both, so neither can drift from the
/// modelled enum.
pub(crate) const TERMINAL_STATUSES: [&str; 13] = [
    "CREATE_FAILED",
    "CREATE_COMPLETE",
    "ROLLBACK_FAILED",
    "ROLLBACK_COMPLETE",
    "DELETE_FAILED",
    "DELETE_COMPLETE",
    "UPDATE_COMPLETE",
    "UPDATE_FAILED",
    "UPDATE_ROLLBACK_FAILED",
    "UPDATE_ROLLBACK_COMPLETE",
    "IMPORT_COMPLETE",
    "IMPORT_ROLLBACK_FAILED",
    "IMPORT_ROLLBACK_COMPLETE",
];

/// The terminal members the cases below name.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const CREATE_FAILED: &str = "CREATE_FAILED";
const ROLLBACK_COMPLETE: &str = "ROLLBACK_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";
const DELETE_COMPLETE: &str = "DELETE_COMPLETE";

/// The reads on the deploy path, every one of which has to answer while a stack operation is
/// open: `DescribeStacks` is the one being polled, and these are the two a client reads beside it
/// to find out WHERE a deployment is.
const DEPLOY_READS: [&str; 2] = ["DescribeStackEvents", "DescribeStackResources"];

/// The same, for a stack that has been DELETED: its events stay addressable by the stack id,
/// while its resources no longer are.
const TEARDOWN_READS: [&str; 1] = ["DescribeStackEvents"];

/// How often a stack is asked whether it has settled, and how long it goes on being asked.
///
/// The bound is a wall-clock deadline rather than a count of reads, and it is generous: what is
/// waited FOR is the deployment settling, which runs on the spans the owning services choose, so
/// all the deadline is for is that the wait TERMINATES.
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const SETTLE_DEADLINE: Duration = Duration::from_secs(120);

/// The logical ids the order-intake template declares its resources under.
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

/// Every logical id of the order-intake estate, in declaration order of its dependency levels.
const ORDER_INTAKE_RESOURCES: [&str; 14] = [
    QUEUE,
    TABLE,
    ROLE,
    INTAKE,
    WORKER,
    TRIGGER,
    LOG_GROUP,
    ALARM,
    API,
    ORDERS_PATH,
    ORDERS_METHOD,
    INVOKE_PERMISSION,
    DEPLOYMENT,
    STAGE,
];

/// The output key the order-intake template publishes its alarm's name under, which is all the
/// client holds to read that alarm back out of the service that owns it.
const ALARM_NAME_OUTPUT: &str = "AlarmName";

/// The alarm the template declares on the worker's own metric: the namespace and metric it
/// watches, the period it is declared with, and the period the update redeclares it with.
const ALARM_NAMESPACE: &str = "OrderService";
const ALARM_METRIC: &str = "FulfilledOrders";
const ALARM_PERIOD: u64 = 300;
const UPDATED_ALARM_PERIOD: u64 = 60;
const ALARM_EVALUATION_PERIODS: u64 = 1;
const ALARM_THRESHOLD: f64 = 1.0;

/// The stage the api is deployed to, and the resource path its method is on.
const STAGE_NAME: &str = "prod";
const ORDERS_PATH_PART: &str = "orders";

/// The handler the intake function runs: a Lambda proxy integration's own answer shape.
const INTAKE_HANDLER: &str = r#"
import json


def handler(event, context):
    submitted = json.loads(event.get("body") or "{}")
    return {
        "statusCode": 202,
        "headers": {"content-type": "application/json"},
        "body": json.dumps({"orderId": submitted.get("orderId"), "status": "accepted"}),
    }
"#;

/// The handler the worker function runs: one structured line per record it drains.
const WORKER_HANDLER: &str = r#"
import json


def handler(event, context):
    for record in event.get("Records") or []:
        print(json.dumps({"event": "fulfilled", "body": record.get("body")}))
    return {"fulfilled": len(event.get("Records") or [])}
"#;

/// The trust policy the execution role carries: both functions run as it.
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

/// What the execution role is granted: the queue the intake writes and the worker's poller
/// reads, the table the worker writes, and the log the run delivers.
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
                "Action": ["dynamodb:PutItem", "dynamodb:GetItem", "dynamodb:UpdateItem"],
                "Resource": { "Fn::GetAtt": [TABLE, "Arn"] }
            },
            {
                "Effect": "Allow",
                "Action": ["logs:CreateLogStream", "logs:PutLogEvents"],
                "Resource": "*"
            }
        ]
    })
}

/// The order-intake estate: a REST API with `POST /orders` in front of an intake function that
/// writes a queue, a worker function the queue's trigger drains into a DynamoDB table, the log
/// group the worker logs into and a CloudWatch alarm on the worker's own metric.
///
/// `alarm_period` is the period the alarm is declared with, and `trigger_function` is what the
/// event source mapping names as its `FunctionName` — so one function of the estate can be made
/// one AWS Lambda does not hold without changing anything else about the deployment.
fn order_intake_template(stack: &str, alarm_period: u64, trigger_function: Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "order intake: a REST API in front of a queue a worker drains into a table",
        "Resources": {
            QUEUE: {
                "Type": "AWS::SQS::Queue",
                "Properties": {
                    "QueueName": format!("{stack}-orders"),
                    "VisibilityTimeout": 30
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
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 30,
                    "Role": { "Fn::GetAtt": [ROLE, "Arn"] },
                    "Environment": { "Variables": { "QUEUE_URL": { "Ref": QUEUE } } },
                    "Code": { "ZipFile": INTAKE_HANDLER }
                }
            },
            WORKER: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": worker_name(stack),
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 30,
                    "Role": { "Fn::GetAtt": [ROLE, "Arn"] },
                    "Environment": { "Variables": { "TABLE_NAME": { "Ref": TABLE } } },
                    "Code": { "ZipFile": WORKER_HANDLER }
                }
            },
            TRIGGER: {
                "Type": "AWS::Lambda::EventSourceMapping",
                "Properties": {
                    "EventSourceArn": { "Fn::GetAtt": [QUEUE, "Arn"] },
                    "FunctionName": trigger_function,
                    "BatchSize": 1,
                    "Enabled": true
                }
            },
            LOG_GROUP: {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {
                    "LogGroupName": format!("/aws/lambda/{}", worker_name(stack)),
                    "RetentionInDays": 7
                }
            },
            ALARM: {
                "Type": "AWS::CloudWatch::Alarm",
                "Properties": {
                    "AlarmName": format!("{stack}-fulfilled-orders"),
                    "AlarmDescription": "no order was fulfilled in the window",
                    "Namespace": ALARM_NAMESPACE,
                    "MetricName": ALARM_METRIC,
                    "Dimensions": [{ "Name": "Service", "Value": stack }],
                    "Statistic": "Sum",
                    "Period": alarm_period,
                    "EvaluationPeriods": ALARM_EVALUATION_PERIODS,
                    "Threshold": ALARM_THRESHOLD,
                    "ComparisonOperator": "LessThanThreshold",
                    "TreatMissingData": "notBreaching"
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
            ALARM_NAME_OUTPUT: { "Value": { "Ref": ALARM } },
            "QueueUrl": { "Value": { "Ref": QUEUE } },
            "TableName": { "Value": { "Ref": TABLE } },
            "ApiId": { "Value": { "Ref": API } }
        }
    })
    .to_string()
}

/// The name the template gives its worker function, which is also where the log group it
/// declares beside it takes its name from.
fn worker_name(stack: &str) -> String {
    format!("{stack}-worker")
}

/// One resource, one owning service: the smallest deployment there is, so the rule is read
/// about a stack that waits on one service rather than only about a deep estate.
fn one_queue(stack: &str) -> String {
    json!({
        "Resources": {
            QUEUE: {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": format!("{stack}-orders") }
            }
        }
    })
    .to_string()
}

/// Three resources over two dependency levels and three owning services: the execution role,
/// the function that runs as it and the log group the function logs into.
fn logged_worker(stack: &str) -> String {
    json!({
        "Resources": {
            ROLE: {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": format!("{stack}-exec"),
                    "AssumeRolePolicyDocument": trust_policy()
                }
            },
            WORKER: {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "FunctionName": worker_name(stack),
                    "Runtime": "python3.12",
                    "Handler": "index.handler",
                    "Timeout": 30,
                    "Role": { "Fn::GetAtt": [ROLE, "Arn"] },
                    "Code": { "ZipFile": WORKER_HANDLER }
                }
            },
            LOG_GROUP: {
                "Type": "AWS::Logs::LogGroup",
                "Properties": {
                    "LogGroupName": format!("/aws/lambda/{}", worker_name(stack)),
                    "RetentionInDays": 7
                }
            }
        }
    })
    .to_string()
}

/// The order-intake estate as the template declares it, with every function it names one AWS
/// Lambda holds.
fn order_intake(stack: &str) -> String {
    order_intake_template(stack, ALARM_PERIOD, json!({ "Ref": WORKER }))
}

/// The same estate, with the worker's trigger naming a function AWS Lambda holds NONE under —
/// so that one resource's create is refused while every other resource of the deployment is
/// well formed. What must follow is a modelled failure status, not the absence of an answer.
fn order_intake_with_an_unheld_worker(stack: &str) -> String {
    order_intake_template(
        stack,
        ALARM_PERIOD,
        json!(format!("{stack}-no-function-of-this-name")),
    )
}

/// One row of the table: the template it deploys, the terminal status its create must settle in,
/// and what the stack must then report about its resources.
struct Row {
    case: &'static str,
    /// The template body, built from the stack name the row runs under.
    template: fn(&str) -> String,
    /// The terminal `StackStatus` the create has to settle in.
    settles_in: &'static str,
    /// The logical ids the template declares, every one of which the stack has to report
    /// `CREATE_COMPLETE` where the deployment completed.
    resources: &'static [&'static str],
    /// The logical id whose create is refused, where one is — the resource whose `CREATE_FAILED`
    /// event has to carry the reason.
    refused: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "one resource over one owning service",
        template: one_queue,
        settles_in: CREATE_COMPLETE,
        resources: &[QUEUE],
        refused: None,
    },
    Row {
        case: "three resources over two dependency levels and three owning services",
        template: logged_worker,
        settles_in: CREATE_COMPLETE,
        resources: &[ROLE, WORKER, LOG_GROUP],
        refused: None,
    },
    Row {
        case: "the order-intake estate: fourteen resources over five levels and six services",
        template: order_intake,
        settles_in: CREATE_COMPLETE,
        resources: &ORDER_INTAKE_RESOURCES,
        refused: None,
    },
    Row {
        case: "the order-intake estate whose trigger names a function AWS Lambda does not hold",
        template: order_intake_with_an_unheld_worker,
        settles_in: ROLLBACK_COMPLETE,
        resources: &ORDER_INTAKE_RESOURCES,
        refused: Some(TRIGGER),
    },
];

/// Ask for `stack` to be created from `template`, and answer the `StackId` the call published.
async fn create_stack(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    template: &str,
) -> String {
    let (http, body) = cloudformation(
        router,
        "CreateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", template),
            // The order-intake template declares the execution role its functions run as, and
            // names it — the two acknowledgements that deployment needs.
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        http, 200,
        "case {case:?}: expected HTTP 200 from CreateStack, got {http} with body {body}"
    );
    let stack_id = opt_element(&body, "StackId").unwrap_or_default();
    assert!(
        !stack_id.is_empty(),
        "case {case:?}: expected CreateStack to publish a StackId, got {body}"
    );
    stack_id
}

/// Poll `identifier` until the stack has left `*_IN_PROGRESS`, and answer the terminal status it
/// settled in together with the `DescribeStacks` answer it was read from.
///
/// At every poll, each read in `reads` is made too and has to answer: the rule is about the whole
/// deploy path answering while an operation is open, not only about the status eventually
/// changing. The wait is on that CONDITION rather than on a fixed span, so it ends as soon as the
/// stack has settled.
///
/// `pub(crate)` here and on the three readers below for the reason [`TERMINAL_STATUSES`] is: the
/// sibling suite [`crate::sam_order_intake_estate_settles_and_serves`] states the same rule about
/// a template the AWS SAM macro processes first, so it polls and reads exactly this way. One copy
/// for both, so neither can drift.
pub(crate) async fn settled_status(
    router: &MultiServiceRouter,
    case: &str,
    identifier: &str,
    reads: &[&str],
) -> (String, String) {
    let giving_up_at = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (http, body) =
            cloudformation(router, "DescribeStacks", &[("StackName", identifier)]).await;
        assert_eq!(
            http, 200,
            "case {case:?}: expected HTTP 200 from DescribeStacks for {identifier}, got {http} \
             with body {body}"
        );
        let reached = opt_element(&body, "StackStatus").unwrap_or_default();
        for action in reads {
            let (http, answered) =
                cloudformation(router, action, &[("StackName", identifier)]).await;
            assert_eq!(
                http, 200,
                "case {case:?}: expected HTTP 200 from {action} while {identifier} was \
                 {reached:?}, got {http} with body {answered}"
            );
        }
        if !reached.ends_with("_IN_PROGRESS") {
            assert!(
                TERMINAL_STATUSES.contains(&reached.as_str()),
                "case {case:?}: expected {identifier} to settle on a modelled terminal \
                 StackStatus, got {reached:?} in {body}"
            );
            return (reached, body);
        }
        assert!(
            tokio::time::Instant::now() < giving_up_at,
            "case {case:?}: {identifier} was still {reached:?} after {SETTLE_DEADLINE:?}, so \
             nothing settled the deployment: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The `(ResourceType, ResourceStatus, PhysicalResourceId)` the stack reports for each logical id
/// of a `DescribeStackResources` answer, read from the member that carries all four so a status is
/// never read off another resource's entry.
pub(crate) fn reported_resources(xml: &str) -> BTreeMap<String, (String, String, String)> {
    members_of(xml)
        .into_iter()
        .filter_map(|member| {
            let logical = elements(member, "LogicalResourceId").into_iter().next()?;
            let first = |name: &str| {
                elements(member, name)
                    .into_iter()
                    .next()
                    .unwrap_or_default()
            };
            Some((
                logical,
                (
                    first("ResourceType"),
                    first("ResourceStatus"),
                    first("PhysicalResourceId"),
                ),
            ))
        })
        .collect()
}

/// The `ResourceStatusReason` of the event `logical_id` reached `reported` in, or `None` where
/// the stack recorded no such event.
async fn recorded_reason(
    router: &MultiServiceRouter,
    case: &str,
    stack: &str,
    logical_id: &str,
    reported: &str,
) -> Option<String> {
    let (http, body) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    assert_eq!(
        http, 200,
        "case {case:?}: expected HTTP 200 from DescribeStackEvents, got {http} with body {body}"
    );
    members_of(&body).into_iter().find_map(|member| {
        let named = elements(member, "LogicalResourceId")
            .into_iter()
            .next()
            .is_some_and(|named| named == logical_id);
        let in_status = elements(member, "ResourceStatus")
            .into_iter()
            .next()
            .is_some_and(|status| status == reported);
        (named && in_status)
            .then(|| elements(member, "ResourceStatusReason").into_iter().next())
            .flatten()
            .filter(|reason| !reason.trim().is_empty())
    })
}

/// Every reason the stack recorded, for an assertion that has to say where a deployment failed
/// instead.
pub(crate) async fn recorded_reasons(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let (_, body) = cloudformation(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    elements(&body, "ResourceStatusReason")
}

/// The `MetricAlarms` entry Amazon CloudWatch reports for `name`, or `None` where that service
/// holds no alarm under it.
pub(crate) async fn described_alarm(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> Option<Value> {
    let described = cw(
        router,
        case,
        REGION,
        "DescribeAlarms",
        &json!({ "AlarmNames": [name] }),
    )
    .await;
    described
        .get("MetricAlarms")
        .and_then(Value::as_array)
        .and_then(|alarms| alarms.first())
        .cloned()
}

/// Every declared estate settles on a modelled terminal `StackStatus`, and the deploy path
/// answers at every poll while it does — whatever the estate's size, depth and spread over
/// owning services, and whether one of its resources' creates is refused.
#[tokio::test]
async fn every_declared_estate_settles_on_a_modelled_terminal_stack_status() {
    let router = emulator();

    // Every stack is asked for before any is polled, so the spans their resources take run
    // together rather than one row's after the next row's.
    let mut deployed: Vec<(&Row, String)> = Vec::new();
    for (index, row) in ROWS.iter().enumerate() {
        let stack = format!("cfn-terminal-estate-{index}");
        let template = (row.template)(&stack);
        create_stack(&router, row.case, &stack, &template).await;
        deployed.push((row, stack));
    }

    for (row, stack) in &deployed {
        let case = row.case;
        let (settled, described) = settled_status(&router, case, stack, &DEPLOY_READS).await;
        assert_eq!(
            settled, row.settles_in,
            "case {case:?}: expected the deployment to settle in {:?}, got {settled:?} in \
             {described} with the recorded reasons {:?}",
            row.settles_in,
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

        match row.refused {
            // A deployment that completed reports every resource it declared created, under an
            // identifier the owning service assigned: the status is not the whole of the answer.
            None => {
                for logical in row.resources {
                    let (_, status, physical) = reported
                        .get(*logical)
                        .cloned()
                        .unwrap_or_else(|| panic!(
                            "case {case:?}: the stack reported nothing for {logical} in \
                             {resources}"
                        ));
                    assert_eq!(
                        status, CREATE_COMPLETE,
                        "case {case:?}: expected {logical} to be {CREATE_COMPLETE} once the \
                         deployment completed, got {status:?} in {resources}"
                    );
                    assert!(
                        !physical.is_empty(),
                        "case {case:?}: expected {logical} to report the identifier its owning \
                         service assigned, got {physical:?} in {resources}"
                    );
                }
                assert!(
                    recorded_reason(&router, case, stack, TRIGGER, CREATE_FAILED)
                        .await
                        .is_none(),
                    "case {case:?}: expected no refused resource on a deployment that completed, \
                     got the reasons {:?}",
                    recorded_reasons(&router, stack).await
                );
            }
            // A deployment one resource's create was refused on reports WHICH resource failed and
            // why, on that resource's own event — which is what a client branches on.
            Some(refused) => {
                let recorded = recorded_reason(&router, case, stack, refused, CREATE_FAILED).await;
                let reason = match recorded {
                    Some(reason) => reason,
                    None => panic!(
                        "case {case:?}: expected {refused} to record a {CREATE_FAILED} reason, \
                         got the reasons {:?}",
                        recorded_reasons(&router, stack).await
                    ),
                };
                assert!(
                    !reason.trim().is_empty(),
                    "case {case:?}: expected a reason for {refused}, got {reason:?}"
                );
            }
        }
    }
}

/// The other two stack-level operations of the same estate settle terminally too: the update that
/// redeclares the alarm's period reaches `UPDATE_COMPLETE` and the alarm in Amazon CloudWatch
/// echoes the period that was declared, and the teardown reaches `DELETE_COMPLETE` with the alarm
/// gone from the service that owned it.
#[tokio::test]
async fn the_order_intake_estates_update_and_teardown_settle_terminally() {
    let router = emulator();
    let case = "the order-intake estate through an update and its teardown";
    let stack = "cfn-terminal-estate-lifecycle";

    let stack_id = create_stack(&router, case, stack, &order_intake(stack)).await;
    let (settled, described) = settled_status(&router, case, stack, &DEPLOY_READS).await;
    assert_eq!(
        settled, CREATE_COMPLETE,
        "case {case:?}: expected the deployment to settle in {CREATE_COMPLETE}, got {settled:?} \
         with the recorded reasons {:?}",
        recorded_reasons(&router, stack).await
    );
    let alarm_name = outputs_of(&described)
        .get(ALARM_NAME_OUTPUT)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: the stack published no {ALARM_NAME_OUTPUT} output in {described}")
        });

    // The alarm the deployment reports created is one Amazon CloudWatch HOLDS, configured as the
    // template declared it.
    let alarm = described_alarm(&router, case, &alarm_name)
        .await
        .unwrap_or_else(|| {
            panic!("case {case:?}: CloudWatch reported no alarm named {alarm_name:?}")
        });
    assert_eq!(
        alarm.get("Period").and_then(Value::as_u64),
        Some(ALARM_PERIOD),
        "case {case:?}: expected the alarm to echo the period the template declared, got {alarm}"
    );
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
        alarm.get("EvaluationPeriods").and_then(Value::as_u64),
        Some(ALARM_EVALUATION_PERIODS),
        "case {case:?}: expected the declared evaluation periods, got {alarm}"
    );

    // The UPDATE of the same estate settles terminally as well, and the period it redeclares is
    // the one that service then reports.
    let updated = order_intake_template(stack, UPDATED_ALARM_PERIOD, json!({ "Ref": WORKER }));
    let (http, body) = cloudformation(
        &router,
        "UpdateStack",
        &[
            ("StackName", stack),
            ("TemplateBody", &updated),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
            ("Capabilities.member.2", "CAPABILITY_NAMED_IAM"),
        ],
    )
    .await;
    assert_eq!(
        http, 200,
        "case {case:?}: expected HTTP 200 from UpdateStack, got {http} with body {body}"
    );
    let (settled, _) = settled_status(&router, case, stack, &DEPLOY_READS).await;
    assert_eq!(
        settled, UPDATE_COMPLETE,
        "case {case:?}: expected the update to settle in {UPDATE_COMPLETE}, got {settled:?} with \
         the recorded reasons {:?}",
        recorded_reasons(&router, stack).await
    );
    let alarm = described_alarm(&router, case, &alarm_name)
        .await
        .unwrap_or_else(|| {
            panic!("case {case:?}: CloudWatch reported no alarm named {alarm_name:?} after the \
                    update")
        });
    assert_eq!(
        alarm.get("Period").and_then(Value::as_u64),
        Some(UPDATED_ALARM_PERIOD),
        "case {case:?}: expected the alarm to echo the period the update declared, got {alarm}"
    );

    // And the TEARDOWN settles terminally too, with the alarm gone from the service that held it.
    let (http, body) = cloudformation(&router, "DeleteStack", &[("StackName", stack)]).await;
    assert_eq!(
        http, 200,
        "case {case:?}: expected HTTP 200 from DeleteStack, got {http} with body {body}"
    );
    // A deleted stack is addressed by its id: the name resolves to no stack once it is gone.
    let (settled, _) = settled_status(&router, case, &stack_id, &TEARDOWN_READS).await;
    assert_eq!(
        settled, DELETE_COMPLETE,
        "case {case:?}: expected the teardown to settle in {DELETE_COMPLETE}, got {settled:?}"
    );
    let left_behind = described_alarm(&router, case, &alarm_name).await;
    assert!(
        left_behind.is_none(),
        "case {case:?}: expected the deleted stack's alarm to be gone from CloudWatch, got \
         {left_behind:?}"
    );
}
