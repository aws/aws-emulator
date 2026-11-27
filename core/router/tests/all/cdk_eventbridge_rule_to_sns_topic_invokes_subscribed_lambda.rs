// SPDX-License-Identifier: CC0-1.0

//! The whole EventBridge -> Amazon SNS -> AWS Lambda fan-out a CDK app synthesizes, deployed as
//! ONE stack and then exercised: what a client sees after `cdk deploy` of a topic, that topic's
//! access policy, a rule whose target is the topic, a function, the function's subscription to
//! the topic and the invoke grant for it.
//!
//! The rule pinned here: **a template's resources are each created in the service that OWNS
//! that resource type, whatever other declared resources their properties reference and in
//! whatever order the template lists them, so a declaration CloudFormation accepts settles on
//! `CREATE_COMPLETE` with every declared resource reported created — and what it leaves behind
//! is the estate those services' own reads describe, so the generated topology works: a
//! published entry EventBridge matches against the rule is handed to the rule's SNS topic
//! target, Amazon SNS fans the notification out to the topic's `lambda`-protocol subscription,
//! and the subscribed function is invoked with that entry's whole event envelope as the
//! message.** A resource type whose provider cannot make it does not settle silently: the stack
//! records that resource's own failure before reverting.
//!
//! Six crates have to agree for that to hold — CloudFormation makes every create the template
//! declares, IAM holds the execution role and decides the handler's own write, EventBridge
//! matches each entry and publishes to the topic, Amazon SNS fans that publication out to its
//! subscription, Lambda decides the invocation against the function's resource policy and then
//! RUNS the handler, and Amazon SQS holds what the handler wrote — so the suite lives here
//! rather than in any one crate. Every observation is a client's: the stack's own reports and
//! outputs, four modelled reads in the owning services, and the bodies on the sink queue.
//!
//! The rows are the entries of ONE `PutEvents` request against the deployed estate, and differ
//! along exactly what the routing generalises over: whether the entry's `source` and
//! `detail-type` are named by the deployed pattern, whether the member the pattern filters on
//! INSIDE `detail` holds a value it names, the shape of `detail` (a flat document, a nested one
//! carrying an array and non-ASCII text), and how many ARNs `resources` lists — so the two rows
//! that must reach nothing stand beside four that do, and an envelope that arrived is tied back
//! to its row by the `EventId` that entry's own publication answered.
//!
//! What is NOT asserted here, because it is asserted elsewhere:
//! `crate::sns_lambda_cdk_subscription_delivery` pins WHICH grant on the function admits the
//! SNS delivery, `crate::events_sns_target_topic_policy_gate` pins the topic policy as the gate
//! on an EventBridge handover to a topic, and
//! `crate::cfn_deployed_events_rule_hands_its_lambda_target_the_envelope` pins the envelope
//! handed to a rule's LAMBDA target, and `crate::events_sns_lambda_delivery_observed_in_logs`
//! pins the same two-hop delivery read as the invoked function's own log line, with each hop's
//! resource policy as its gate. What only this suite reaches is the composite the issue
//! describes: all six resource types in one synthesized template, deployed in one call, and the
//! rule -> topic -> subscription -> function path that deployment generates.

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::{decoded_entries, elements, outputs_of, pairs_of};
// The awsQuery CloudFormation/SNS/IAM callers, the Lambda and Amazon SQS wires and the queue
// reader a sibling suite already owns: one harness for this path, so the suites cannot drift
// about what a deployment or a delivery observation is.
use crate::sns_lambda_subscription_permission_boundary::{
    cloudformation, drain, lambda, sns, sqs, CREATE_COMPLETE, LOOK_INTERVAL,
};

/// The Region every request here is signed for — the one the shared callers sign for, and so the
/// Region the delivered envelope must report.
const REGION: &str = "us-east-1";

/// The account this emulator serves, which is the `account` of every envelope it builds.
const ACCOUNT: &str = ::engine::EMULATOR_ACCOUNT_ID;

/// The stack the whole fan-out is deployed as.
const STACK: &str = "cdk-events-sns-lambda-fanout";

/// The `ResourceStatus` every declared resource of a settled create has to report.
const RESOURCE_CREATE_COMPLETE: &str = "CREATE_COMPLETE";

/// The logical ids of the resource types the synthesized template declares, which are what the
/// stack's own resource report has to account for.
const DECLARED: &[&str] = &[
    "Sink",
    "ForwarderServiceRole",
    "ForwarderServiceRoleDefaultPolicy",
    "Forwarder",
    "Topic",
    "TopicPolicy",
    "Rule",
    "TopicForwarderSubscription",
    "ForwarderAllowInvoke",
];

/// The `source` the deployed pattern names, and one it does not.
const SOURCE: &str = "cdk.events.sns.lambda";
const OTHER_SOURCE: &str = "cdk.events.elsewhere";

/// The `detail-type` the deployed pattern names, and one it does not.
const DETAIL_TYPE: &str = "Fan Out Probe";
const OTHER_DETAIL_TYPE: &str = "Fan Out Probe Unnamed";

/// The member inside `detail` the deployed pattern filters on, the value it names and one it
/// does not.
const STAGE_MEMBER: &str = "stage";
const STAGE: &str = "ship";
const OTHER_STAGE: &str = "hold";

/// The service principals the two deliveries are made as: EventBridge publishes to a topic
/// target as its own ("for Lambda functions, Amazon SNS topics and Amazon SQS queues,
/// EventBridge relies on resource-based policies", `PutTargets`), and Amazon SNS invokes a
/// subscribed function as its own.
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The actions those two deliveries perform, which the declared documents have to grant.
const PUBLISH_ACTION: &str = "sns:Publish";
const INVOKE_ACTION: &str = "lambda:InvokeFunction";

/// The `version` an EventBridge envelope carries: the schema version of the event FORMAT.
const ENVELOPE_VERSION: &str = "0";

/// The `State` AWS Lambda reports for a function it takes invocations on — "During this time,
/// you can't invoke or modify the function" is what `Pending` means (`CreateFunction`) — and how
/// many times it is polled for before the suite gives up.
const ACTIVE: &str = "Active";
const POLLS: usize = 900;

/// The `Timeout` the declared function carries: well past what one forward needs on any host,
/// because what is under test is the path rather than how fast an interpreter gets through the
/// handler.
const FUNCTION_TIMEOUT: u64 = 120;

/// How long the deliveries are waited for before the suite gives up on them, and how long it
/// keeps looking afterwards so an envelope the pattern does not name would stand among the
/// bodies read rather than merely being late. Both are waits on a CONDITION, looked at again
/// every [`LOOK_INTERVAL`], and generous enough that a loaded host cannot decide the outcome.
/// The second span is held open well past the first delivery's own arrival so that the rows
/// which must reach NOTHING are as hard to satisfy as the rows that must arrive: an entry
/// routed in error merely later than the matched set would still be read.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(90);
const QUIET_DEADLINE: Duration = Duration::from_secs(20);

/// The `Analytics` property `cdk synth` writes on the metadata resource.
const ANALYTICS: &str = "v2:deflate64:H4sIAAAAAAAA";

/// The parameter a synthesized template reads the bootstrap version from.
const BOOTSTRAP_VERSION_PARAMETER: &str = "/cdk-bootstrap/hnb659fds/version";

/// CloudFormation, EventBridge, Amazon SNS, Lambda, Amazon SQS and IAM over ONE engine, as
/// `core/router/src/lib.rs` mounts them.
fn build_router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/events/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// The handler the stack declares inline: it forwards the MESSAGE of every SNS record it is
/// handed to the queue whose URL its environment carries, which is where the envelope that
/// travelled the whole path is observed.
const FORWARDING_SOURCE: &str = r#"
import os

import boto3


def lambda_handler(event, context):
    client = boto3.client("sqs")
    url = os.environ["SINK_URL"]
    forwarded = 0
    for record in event["Records"]:
        client.send_message(QueueUrl=url, MessageBody=record["Sns"]["Message"])
        forwarded += 1
    return {"forwarded": forwarded}
"#;

/// The template the fan-out is deployed from, in the shape `cdk synth` writes it: the
/// bootstrap-version parameter and the rule over it, the metadata resource and its condition,
/// the sink queue, an execution role and its inline policy, an UNNAMED function whose
/// environment carries the sink's URL, the topic, the `AWS::SNS::TopicPolicy` admitting
/// EventBridge's publish, the `AWS::Events::Rule` whose single target is that topic, the
/// `lambda`-protocol `AWS::SNS::Subscription` and the `AWS::Lambda::Permission` admitting
/// Amazon SNS's invocation.
fn template() -> String {
    let pattern = json!({
        "source": [SOURCE],
        "detail-type": [DETAIL_TYPE],
        "detail": { STAGE_MEMBER: [STAGE] },
    });
    json!({
        "Parameters": {
            "BootstrapVersion": {
                "Type": "AWS::SSM::Parameter::Value<String>",
                "Default": BOOTSTRAP_VERSION_PARAMETER,
                "Description": "Version of the CDK Bootstrap resources in this environment",
            },
        },
        "Rules": {
            "CheckBootstrapVersion": {
                "Assertions": [{
                    "Assert": {"Fn::Not": [{"Fn::Contains": [
                        ["1", "2", "3", "4", "5"], {"Ref": "BootstrapVersion"},
                    ]}]},
                    "AssertDescription":
                        "CDK bootstrap stack version 6 required. Please run 'cdk bootstrap' \
                         with a recent version of the CDK CLI.",
                }],
            },
        },
        "Conditions": {
            "CDKMetadataAvailable": {"Fn::Or": [
                {"Fn::Equals": [{"Ref": "AWS::Region"}, REGION]},
                {"Fn::Equals": [{"Ref": "AWS::Region"}, "eu-west-1"]},
            ]},
        },
        "Resources": {
            "Sink": {
                "Type": "AWS::SQS::Queue",
                "Metadata": {"aws:cdk:path": format!("{STACK}/Sink/Resource")},
            },
            "ForwarderServiceRole": {
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
                    "ManagedPolicyArns": [{"Fn::Join": ["", [
                        "arn:", {"Ref": "AWS::Partition"},
                        ":iam::aws:policy/service-role/AWSLambdaBasicExecutionRole",
                    ]]}],
                },
                "Metadata": {"aws:cdk:path": format!("{STACK}/Forwarder/ServiceRole")},
            },
            "ForwarderServiceRoleDefaultPolicy": {
                "Type": "AWS::IAM::Policy",
                "Properties": {
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Action": ["sqs:SendMessage", "sqs:GetQueueAttributes"],
                            "Effect": "Allow",
                            "Resource": {"Fn::GetAtt": ["Sink", "Arn"]},
                        }],
                    },
                    "PolicyName": "ForwarderServiceRoleDefaultPolicy",
                    "Roles": [{"Ref": "ForwarderServiceRole"}],
                },
            },
            "Forwarder": {
                "Type": "AWS::Lambda::Function",
                "Properties": {
                    "Code": {"ZipFile": FORWARDING_SOURCE},
                    "Environment": {"Variables": {"SINK_URL": {"Ref": "Sink"}}},
                    "Handler": "index.lambda_handler",
                    "Role": {"Fn::GetAtt": ["ForwarderServiceRole", "Arn"]},
                    "Runtime": "python3.12",
                    "Timeout": FUNCTION_TIMEOUT,
                },
                "DependsOn": ["ForwarderServiceRoleDefaultPolicy", "ForwarderServiceRole"],
                "Metadata": {"aws:cdk:path": format!("{STACK}/Forwarder/Resource")},
            },
            "Topic": {
                "Type": "AWS::SNS::Topic",
                "Metadata": {"aws:cdk:path": format!("{STACK}/Topic/Resource")},
            },
            "TopicPolicy": {
                "Type": "AWS::SNS::TopicPolicy",
                "Properties": {
                    "PolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Sid": "0",
                            "Action": PUBLISH_ACTION,
                            "Effect": "Allow",
                            "Principal": {"Service": EVENTS_PRINCIPAL},
                            "Resource": {"Ref": "Topic"},
                        }],
                    },
                    "Topics": [{"Ref": "Topic"}],
                },
            },
            "Rule": {
                "Type": "AWS::Events::Rule",
                "Properties": {
                    "EventPattern": pattern,
                    "State": "ENABLED",
                    "Targets": [{
                        "Arn": {"Ref": "Topic"},
                        "Id": "Target0",
                    }],
                },
                "Metadata": {"aws:cdk:path": format!("{STACK}/Rule/Resource")},
            },
            "TopicForwarderSubscription": {
                "Type": "AWS::SNS::Subscription",
                "Properties": {
                    "Endpoint": {"Fn::GetAtt": ["Forwarder", "Arn"]},
                    "Protocol": "lambda",
                    "TopicArn": {"Ref": "Topic"},
                },
            },
            "ForwarderAllowInvoke": {
                "Type": "AWS::Lambda::Permission",
                "Properties": {
                    "Action": INVOKE_ACTION,
                    "FunctionName": {"Fn::GetAtt": ["Forwarder", "Arn"]},
                    "Principal": SNS_PRINCIPAL,
                    "SourceArn": {"Ref": "Topic"},
                },
            },
            "CDKMetadata": {
                "Type": "AWS::CDK::Metadata",
                "Properties": {"Analytics": ANALYTICS},
                "Condition": "CDKMetadataAvailable",
            },
        },
        "Outputs": {
            "TopicArn": {"Value": {"Ref": "Topic"}},
            "RuleName": {"Value": {"Ref": "Rule"}},
            "SinkUrl": {"Value": {"Ref": "Sink"}},
            "FunctionName": {"Value": {"Ref": "Forwarder"}},
            "FunctionArn": {"Value": {"Fn::GetAtt": ["Forwarder", "Arn"]}},
        },
    })
    .to_string()
}

/// One row: an entry of the single `PutEvents` request, and whether the deployed pattern names
/// it.
struct Row {
    case: &'static str,
    source: &'static str,
    detail_type: &'static str,
    resources: Vec<&'static str>,
    detail: Value,
    matched: bool,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a flat detail carrying the named stage, and no resources",
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            resources: vec![],
            detail: json!({ STAGE_MEMBER: STAGE }),
            matched: true,
        },
        Row {
            case: "a flat detail beside one resource ARN",
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            resources: vec!["arn:aws:s3:::cdk-events-sns-lambda-bucket"],
            detail: json!({ STAGE_MEMBER: STAGE, "attempt": 1, "settled": true }),
            matched: true,
        },
        Row {
            case: "a nested detail carrying an array and non-ASCII text, beside two resource ARNs",
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            resources: vec![
                "arn:aws:s3:::cdk-events-sns-lambda-bucket",
                "arn:aws:sqs:us-east-1:000000000000:cdk-events-sns-lambda-resource",
            ],
            detail: json!({
                STAGE_MEMBER: STAGE,
                "nested": { "depth": [1, 2, 3], "label": "caf\u{e9} \"quoted\"" },
            }),
            matched: true,
        },
        Row {
            case: "a stage inside detail the pattern does not name",
            source: SOURCE,
            detail_type: DETAIL_TYPE,
            resources: vec![],
            detail: json!({ STAGE_MEMBER: OTHER_STAGE }),
            matched: false,
        },
        Row {
            case: "a source the pattern does not name",
            source: OTHER_SOURCE,
            detail_type: DETAIL_TYPE,
            resources: vec![],
            detail: json!({ STAGE_MEMBER: STAGE }),
            matched: false,
        },
        Row {
            case: "a detail-type the pattern does not name",
            source: SOURCE,
            detail_type: OTHER_DETAIL_TYPE,
            resources: vec!["arn:aws:s3:::cdk-events-sns-lambda-bucket"],
            detail: json!({ STAGE_MEMBER: STAGE }),
            matched: false,
        },
    ]
}

/// Poll `GetFunctionConfiguration` until Lambda first reports `name` [`ACTIVE`]. The span is
/// that service's own, so nothing here sleeps a fixed span on its behalf.
async fn wait_until_active(router: &MultiServiceRouter, name: &str) {
    for _ in 0..POLLS {
        let (_, reported) = lambda(
            router,
            "GET",
            &format!("/2015-03-31/functions/{name}/configuration"),
            None,
        )
        .await;
        if reported.get("State").and_then(Value::as_str) == Some(ACTIVE) {
            return;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    panic!("Lambda never reported {name:?} {ACTIVE}");
}

/// Every body the sink has handed over, read as the event envelope a forward carries.
async fn forwarded(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    drain(router, url)
        .await
        .into_iter()
        .map(|body| {
            serde_json::from_str(&body).unwrap_or_else(|error| {
                panic!(
                    "expected every forwarded body to be the JSON event envelope, got {body:?} \
                     ({error})"
                )
            })
        })
        .collect()
}

/// Whether `envelopes` holds one carrying `id`.
fn holds(envelopes: &[Value], id: &str) -> bool {
    envelopes.iter().any(|envelope| envelope["id"] == json!(id))
}

/// The whole synthesized fan-out: the deployment settles, every declared resource is reported
/// created and is the one its owning service describes, and each matched entry's envelope
/// travels rule -> topic -> subscription -> function to the sink while the entries the pattern
/// does not name reach nothing.
#[tokio::test]
async fn a_cdk_deployed_rule_to_sns_topic_invokes_the_subscribed_lambda() {
    let router = build_router();

    cloudformation(
        &router,
        "CreateStack",
        &[
            ("StackName", STACK),
            ("TemplateBody", &template()),
            ("Capabilities.member.1", "CAPABILITY_IAM"),
        ],
    )
    .await;
    let (status, described) = wait_for_stack_status(&router, STACK, CREATE_COMPLETE).await;
    if status != CREATE_COMPLETE {
        // A resource its provider could not make records its own failure before the stack
        // reverts, so the reasons are what a client reads a rollback's cause from.
        let happened = cloudformation(&router, "DescribeStackEvents", &[("StackName", STACK)]).await;
        let reasons = elements(&happened, "ResourceStatusReason");
        panic!(
            "expected the synthesized fan-out to reach {CREATE_COMPLETE}, got {status} with \
             resource failures {reasons:?} and {described}"
        );
    }

    // Every resource type the template declares is reported created, so nothing settled by
    // being skipped.
    let reported = cloudformation(&router, "DescribeStackResources", &[("StackName", STACK)]).await;
    let statuses = pairs_of(&reported, "LogicalResourceId", "ResourceStatus");
    for logical in DECLARED {
        assert_eq!(
            statuses.get(*logical).map(String::as_str),
            Some(RESOURCE_CREATE_COMPLETE),
            "case {logical:?}: expected the declared resource to be reported \
             {RESOURCE_CREATE_COMPLETE}, got {statuses:?}"
        );
    }

    let outputs = outputs_of(&described);
    let output = |name: &str| {
        outputs
            .get(name)
            .cloned()
            .unwrap_or_else(|| panic!("expected the stack to publish {name}, got {described}"))
    };
    let topic = output("TopicArn");
    let rule = output("RuleName");
    let sink_url = output("SinkUrl");
    let function = output("FunctionName");
    let function_arn = output("FunctionArn");

    // The estate the declaration promised, read exactly where a client reads it: the topic's own
    // access policy admits EventBridge's publish, the rule's targets are that topic, the topic's
    // subscription addresses the function over the `lambda` protocol, and the function's own
    // document admits Amazon SNS's invocation.
    let attributes = sns(&router, "GetTopicAttributes", &[("TopicArn", &topic)]).await;
    let policy = decoded_entries(&attributes)
        .get("Policy")
        .cloned()
        .unwrap_or_else(|| panic!("expected the topic to report a Policy, got {attributes}"));
    let document: Value = serde_json::from_str(&policy)
        .unwrap_or_else(|error| panic!("expected the Policy to be a document, got {policy} ({error})"));
    let admits_events = document["Statement"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .any(|statement| {
            statement["Principal"]["Service"] == json!(EVENTS_PRINCIPAL)
                && statement["Action"] == json!(PUBLISH_ACTION)
                && statement["Effect"] == json!("Allow")
        });
    assert!(
        admits_events,
        "expected the declared AWS::SNS::TopicPolicy to admit {EVENTS_PRINCIPAL} \
         {PUBLISH_ACTION}, got {document}"
    );

    let listed = events(
        &router,
        "the deployed control plane",
        "ListTargetsByRule",
        &json!({ "Rule": rule }),
    )
    .await;
    let targets = listed["Targets"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        targets.len(),
        1,
        "expected the deployed rule to report exactly its one declared target, got {listed}"
    );
    assert_eq!(
        targets[0]["Arn"],
        json!(topic),
        "expected the reported target to be the topic ARN {topic:?}, got {listed}"
    );

    let subscriptions = sns(
        &router,
        "ListSubscriptionsByTopic",
        &[("TopicArn", &topic)],
    )
    .await;
    assert_eq!(
        elements(&subscriptions, "Protocol"),
        vec!["lambda".to_string()],
        "expected one lambda subscription of {topic}, got {subscriptions}"
    );
    assert_eq!(
        elements(&subscriptions, "Endpoint"),
        vec![function_arn.clone()],
        "expected the subscription to address {function_arn}, got {subscriptions}"
    );

    let (policy_status, reported_policy) = lambda(
        &router,
        "GET",
        &format!("/2015-03-31/functions/{function}/policy"),
        None,
    )
    .await;
    assert_eq!(
        policy_status, 200,
        "expected HTTP 200 from GetPolicy, got {policy_status} with {reported_policy}"
    );
    let function_document: Value =
        serde_json::from_str(reported_policy["Policy"].as_str().unwrap_or_default())
            .unwrap_or_else(|error| {
                panic!("expected GetPolicy to answer a document, got {reported_policy} ({error})")
            });
    let granted = function_document["Statement"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|statement| statement["Principal"]["Service"] == json!(SNS_PRINCIPAL))
        .unwrap_or_else(|| {
            panic!("expected a statement for {SNS_PRINCIPAL:?}, got {function_document}")
        });
    assert_eq!(
        granted["Action"],
        json!(INVOKE_ACTION),
        "expected the declared grant to name {INVOKE_ACTION:?}, got {granted}"
    );

    // Nothing is published until the function takes invocations, so a delivery withheld before
    // that would be the lifecycle's refusal rather than the routing's.
    wait_until_active(&router, &function).await;

    // One request carrying every row, so what reaches the function is decided by the entry and
    // not by when it was published.
    let rows = rows();
    let entries: Vec<Value> = rows
        .iter()
        .map(|row| {
            json!({
                "Source": row.source,
                "DetailType": row.detail_type,
                "Resources": row.resources,
                "Detail": row.detail.to_string(),
            })
        })
        .collect();
    let answered = events(
        &router,
        "the publication",
        "PutEvents",
        &json!({ "Entries": entries }),
    )
    .await;
    assert_eq!(
        answered["FailedEntryCount"],
        json!(0),
        "expected every entry to be accepted, got {answered}"
    );
    let answered_entries = answered["Entries"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        answered_entries.len(),
        rows.len(),
        "expected one answered entry per published one, got {answered}"
    );
    let ids: Vec<String> = answered_entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let id = entry["EventId"].as_str().unwrap_or_default().to_string();
            assert!(
                !id.is_empty(),
                "case {:?}: expected the accepted entry to carry an EventId, got {answered}",
                rows[index].case
            );
            id
        })
        .collect();

    // Wait for the envelope of every MATCHED row to reach the sink the handler forwards to,
    // looking again until they are all there or the deadline passes.
    let expected: Vec<&String> = rows
        .iter()
        .zip(&ids)
        .filter(|(row, _)| row.matched)
        .map(|(_, id)| id)
        .collect();
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let mut seen: Vec<Value> = Vec::new();
    loop {
        seen.extend(forwarded(&router, &sink_url).await);
        if expected.iter().all(|id| holds(&seen, id.as_str())) || Instant::now() >= deadline {
            break;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }

    // Then keep looking, so an envelope the pattern does not name would stand among the bodies
    // read below rather than being merely late.
    let quiet_until = Instant::now() + QUIET_DEADLINE;
    while Instant::now() < quiet_until {
        tokio::time::sleep(LOOK_INTERVAL).await;
        seen.extend(forwarded(&router, &sink_url).await);
    }

    for (row, id) in rows.iter().zip(&ids) {
        let case = row.case;
        if !row.matched {
            assert!(
                !holds(&seen, id),
                "case {case:?}: expected the entry the pattern does not name to reach no target, \
                 but the sink holds its envelope among {seen:?}"
            );
            continue;
        }
        let envelope = seen
            .iter()
            .find(|envelope| envelope["id"] == json!(id))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected the envelope carrying id {id:?} to travel the rule, \
                     the topic and the subscription to the function, got {seen:?}"
                )
            });
        assert_eq!(
            envelope["version"],
            json!(ENVELOPE_VERSION),
            "case {case:?}: expected envelope version {ENVELOPE_VERSION:?}, got {envelope}"
        );
        assert_eq!(
            envelope["source"],
            json!(row.source),
            "case {case:?}: expected envelope source {:?}, got {envelope}",
            row.source
        );
        assert_eq!(
            envelope["detail-type"],
            json!(row.detail_type),
            "case {case:?}: expected envelope detail-type {:?}, got {envelope}",
            row.detail_type
        );
        assert_eq!(
            envelope["account"],
            json!(ACCOUNT),
            "case {case:?}: expected envelope account {ACCOUNT:?}, got {envelope}"
        );
        assert_eq!(
            envelope["region"],
            json!(REGION),
            "case {case:?}: expected envelope region {REGION:?}, got {envelope}"
        );
        assert_eq!(
            envelope["resources"],
            json!(row.resources),
            "case {case:?}: expected envelope resources {:?}, got {envelope}",
            row.resources
        );
        assert_eq!(
            envelope["detail"], row.detail,
            "case {case:?}: expected envelope detail {}, got {envelope}",
            row.detail
        );
    }

    // Every matched row reached the sink and nothing else did.
    let mut held: Vec<String> = seen
        .iter()
        .filter_map(|envelope| envelope["id"].as_str().map(str::to_string))
        .collect();
    held.sort();
    held.dedup();
    let mut wanted: Vec<String> = expected.iter().map(|id| id.to_string()).collect();
    wanted.sort();
    assert_eq!(
        held, wanted,
        "expected the sink to hold the envelope of every matched entry and of nothing else, got \
         {seen:?}"
    );

    // The sink is the queue the stack itself declared, so what the handler wrote is held by the
    // resource the deployment made rather than one the test created.
    let drained = sqs(
        &router,
        "GetQueueAttributes",
        &json!({ "QueueUrl": sink_url, "AttributeNames": ["QueueArn"] }),
    )
    .await;
    let sink_arn = drained["Attributes"]["QueueArn"].as_str().unwrap_or_default();
    assert!(
        sink_arn.starts_with(&format!("arn:aws:sqs:{REGION}:{ACCOUNT}:")),
        "expected the declared sink queue to be held by Amazon SQS under an ARN of this \
         account, got {sink_arn:?} in {drained}"
    );
}
