// SPDX-License-Identifier: CC0-1.0

//! A backup job started against a CloudFormation-deployed vault reaches the queue the SAME
//! stack's EventBridge rule, SNS topic, subscription and queue policy wire up.
//!
//! The RULE pinned here: **every state an AWS Backup job MOVES INTO after the call that
//! started it is published once onto the account's DEFAULT EventBridge bus as `source`
//! `aws.backup` / `detail-type` `Backup Job State Change`, carrying that job's own stored
//! members as the event `detail` — its id, the state it moved into, the vault, the protected
//! resource and that resource's Backup type — and EventBridge routes the envelope by its
//! CONTENT, handing it UNCHANGED to every target of each rule whose `EventPattern` matches,
//! whichever service owns that target: an SNS topic forwards it to the queue subscribed to it
//! (as the published body itself where the subscription asks for raw delivery, and inside SNS's
//! own notification envelope where it does not), and a queue named as the rule's own target
//! receives it directly — so a client whose whole pipeline was PROVISIONED FROM A TEMPLATE
//! reads the envelope off the stack's queue with no further AWS Backup call, and a rule whose
//! pattern cannot match the states the job takes delivers nothing.** The state the START call
//! stored is not a move — it is what that call's own response reports — so the envelopes the
//! bus is owed are the transitions that follow it.
//!
//! Source for the envelope: the AWS Backup EventBridge notifications, which carry
//! `"source": "aws.backup"` with `"detail-type": "Backup Job State Change"`; the backup
//! Smithy model describes the control plane, not the bus, so the `detail` members are asserted
//! against what the SAME job's control-plane response reports for them, and the states a job
//! can take against `smithy_query(query="shape", shape="com.amazonaws.backup#BackupJobState")`
//! (`CREATED`/`PENDING`/`RUNNING`/`ABORTING`/`ABORTED`/`COMPLETED`/`FAILED`/`EXPIRED`/
//! `PARTIAL`).
//!
//! Seven crates have to agree for that to hold — CloudFormation makes every create the
//! template declares, Amazon DynamoDB holds the protected table, IAM holds the role the backup
//! is performed under and decides what it may do, AWS Backup runs the job and publishes its
//! move, EventBridge matches the envelope and hands it over, SNS fans it out to its
//! subscription, and Amazon SQS holds what arrived under its own queue policy — so the suite
//! lives here rather than in any one crate, and it runs over the WHOLE `build_router` service
//! set the binary serves.
//!
//! The rows differ along exactly what the rule generalises over: WHICH service owns the target
//! the stack's rule names (an SNS topic, the queue itself), whether the subscription asks for
//! RAW message delivery or takes SNS's own notification envelope, WHICH dimension of the
//! envelope the rule's pattern filters on (the source and detail type alone, one state inside
//! `detail`), WHICH terminal state the job's own backup role leads it to (`COMPLETED`, and the
//! `FAILED` of a role whose permissions cannot create the recovery point), and whether the
//! delivery is admitted at all (a pattern naming a state this job never takes) — the row whose
//! queue must therefore stay empty of it. That row carries a POSITIVE CONTROL rather than a
//! bare quiet window, and so does every other row: each stack also declares a second rule
//! matching the source and detail type alone whose target is a control queue, so the control's
//! message is what says the envelope really was published and dispatched, and the row's own
//! queue holding nothing by then is the refusal rather than a look taken too early.
//!
//! What is NOT asserted here, because it is asserted elsewhere:
//! `services/service-com-amazonaws-backup-cryocontrollerusermanager/tests/all/job_state_change_events.rs`
//! pins the published `detail` of every job KIND against the control plane's own reads,
//! `crate::backup_job_state_change_invokes_its_lambda_target` pins the same publication routed
//! to a LAMBDA target and the resource policy that governs it, and
//! `crate::cloudformation_provisions_backup_resources` pins what the Backup resource types a
//! stack declares become in AWS Backup. What only this suite reaches is the whole path the
//! issue describes: one TEMPLATE's vault, table, role, topic, subscription, queue, policies and
//! rule, and a real job's transition arriving on that stack's own queue.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The shared stack reader of this test crate, its awsQuery CloudFormation caller, its SQS
// receive and the XML output reader it writes ONCE rather than per suite. They sign for the
// Region below, which is why this suite signs for it too.
use crate::cloudformation_firehose_stream_stabilisation::wait_for_stack_status;
use crate::query_wire::outputs_of;
use crate::rds_docdb_neptune_signing_name_routing::authorization;
use crate::sns_lambda_subscription_permission_boundary::{
    cloudformation, drain, CREATE_COMPLETE, DELIVERY_BUDGET, LOOK_INTERVAL,
};

/// The Region every request below is signed for — the one the shared callers sign for.
const REGION: &str = "us-east-1";

/// The `source` and `detail-type` AWS Backup publishes a job's state changes under, spelled
/// out here rather than imported from the service crate, so a wrong constant in the
/// implementation cannot make this suite pass.
const BACKUP_SOURCE: &str = "aws.backup";
const BACKUP_JOB_STATE_CHANGE: &str = "Backup Job State Change";

/// The three `BackupJobState` values the rows are stated in, and the one no job of this
/// emulator takes.
const COMPLETED: &str = "COMPLETED";
const FAILED: &str = "FAILED";
const ABORTED: &str = "ABORTED";

/// The Backup resource type of the protected table, as the Backup API's own `ByResourceType`
/// catalog spells it (`ListBackupJobs`).
const DYNAMODB_TYPE: &str = "DynamoDB";

/// The action AWS Backup performs UNDER a job's role to create the recovery point of a
/// DynamoDB table, and one it is granted instead where the row withholds it.
///
/// Source: the per-resource-type grants AWS publishes for the backup role (AWS Backup
/// *Managed policies*, `AWSBackupServiceRolePolicyForBackup`) — of the DynamoDB grants, the
/// one action that CREATES the recovery point.
const BACKUP_ACTION: &str = "dynamodb:CreateBackup";
const UNRELATED_ACTION: &str = "dynamodb:DescribeTable";

/// The service principals a cross-service delivery is made as, which the stack's own queue and
/// topic policies admit: "for Lambda functions, Amazon SNS topics and Amazon SQS queues,
/// EventBridge relies on resource-based policies" (`PutTargets`).
const EVENTS_PRINCIPAL: &str = "events.amazonaws.com";
const SNS_PRINCIPAL: &str = "sns.amazonaws.com";

/// The target id the stack's rule declares, so the id EventBridge reports for the installed
/// target is the one the template named.
const TARGET_ID: &str = "AlarmTopicTarget";

/// How long a row waits for the job to reach the state it is owed, and for a message that must
/// not come to fail to come. Both are generous: what is waited for is a CONDITION, looked at
/// again until it holds, and the budget is only there so the wait terminates.
const JOB_BUDGET: Duration = Duration::from_secs(30);
const QUIET_BUDGET: Duration = Duration::from_secs(3);

/// The whole emulator over one engine, exactly as `core/router/src/lib.rs` assembles it for
/// the binary: every in-process call the pipeline makes is dispatched back into this same
/// router, so the queue this suite reads is the queue those deliveries wrote.
fn router() -> MultiServiceRouter {
    let mut router = aws_emulator_router::build_router(aws_emulator_router::new_engine());
    router.set_request_authentication(false);
    router
}

/// The status and parsed body of one response (`Value::Null` for an empty or unparseable
/// body).
fn answered(response: &http::Response<Bytes>) -> (u16, Value) {
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`), which must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("events.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AWSEvents.{operation}"))
        .header("authorization", authorization("events"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let (status, parsed) = answered(&router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with \
         {parsed}"
    );
    parsed
}

/// One AWS Backup call (`restJson1`), which must answer 200.
async fn backup(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> Value {
    let payload = body.map_or_else(Bytes::new, |body| Bytes::from(body.to_string()));
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://backup.{REGION}.amazonaws.com{uri}"))
        .header("host", format!("backup.{REGION}.amazonaws.com"))
        .header("authorization", authorization("backup"));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(payload)
        .expect("the test request is well formed");
    let (status, parsed) = answered(&router.dispatch(request).await);
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from Backup {method} {uri}, got {status} with {parsed}"
    );
    parsed
}

/// A string member of a response, or a panic naming the case and what came back.
fn member(case: &str, body: &Value, pointer: &str) -> String {
    body.pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected {pointer} in {body}"))
        .to_string()
}

/// One output of the stack, or a panic naming the case: the pipeline is unusable without it,
/// which is itself part of what the rule asks for.
fn output(case: &str, described: &str, name: &str) -> String {
    outputs_of(described)
        .get(name)
        .cloned()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected the stack to publish {name}, got {described}")
        })
}

/// Which service owns the target the row's rule names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// The stack's SNS topic, which forwards to the queue subscribed to it.
    Topic,
    /// The stack's queue, named as the rule's own target.
    Queue,
}

/// One row of the table.
struct Row {
    case: &'static str,
    /// What the rule's single target names.
    target: Target,
    /// Whether the subscription asks for raw message delivery.
    raw: bool,
    /// The `detail` clause the rule's pattern carries beyond the source and the detail type.
    /// `None` filters on those two alone.
    detail: Option<fn() -> Value>,
    /// Whether the role the template declares may create the recovery point of the table, which
    /// is what decides the job's terminal state.
    permitted: bool,
    /// The terminal state the job reaches.
    job_state: &'static str,
    /// The `detail.state` the row's own queue must receive, or `None` when nothing may reach it.
    delivered: Option<&'static str>,
}

/// The `detail` clauses the rows filter on, each a pattern a client really writes.
fn completed_only() -> Value {
    json!({ "state": [COMPLETED] })
}
fn failed_only() -> Value {
    json!({ "state": [FAILED] })
}
fn aborted_only() -> Value {
    json!({ "state": [ABORTED] })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an SNS topic target whose subscription asks for raw message delivery",
            target: Target::Topic,
            raw: true,
            detail: None,
            permitted: true,
            job_state: COMPLETED,
            delivered: Some(COMPLETED),
        },
        Row {
            case: "an SNS topic target whose subscription takes SNS's own notification envelope",
            target: Target::Topic,
            raw: false,
            detail: Some(completed_only),
            permitted: true,
            job_state: COMPLETED,
            delivered: Some(COMPLETED),
        },
        Row {
            case: "the stack's queue as the rule's own target",
            target: Target::Queue,
            raw: true,
            detail: None,
            permitted: true,
            job_state: COMPLETED,
            delivered: Some(COMPLETED),
        },
        Row {
            case: "a role whose permissions cannot create the recovery point, on a FAILED pattern",
            target: Target::Topic,
            raw: true,
            detail: Some(failed_only),
            permitted: false,
            job_state: FAILED,
            delivered: Some(FAILED),
        },
        Row {
            case: "a pattern naming a state this job never takes",
            target: Target::Topic,
            raw: true,
            detail: Some(aborted_only),
            permitted: true,
            job_state: COMPLETED,
            delivered: None,
        },
    ]
}

/// Every name one row's template declares, so what is asserted is computed from the
/// declaration rather than read back out of the answer being checked.
struct Names {
    stack: String,
    vault: String,
    table: String,
    role: String,
    topic: String,
    queue: String,
    control_queue: String,
    rule: String,
    control_rule: String,
}

impl Names {
    fn of(index: usize) -> Self {
        Self {
            stack: format!("cfn-backup-events-{index}"),
            vault: format!("cfn-backup-events-vault-{index}"),
            table: format!("cfn-backup-events-table-{index}"),
            role: format!("cfn-backup-events-role-{index}"),
            topic: format!("cfn-backup-events-topic-{index}"),
            queue: format!("cfn-backup-events-queue-{index}"),
            control_queue: format!("cfn-backup-events-control-{index}"),
            rule: format!("cfn-backup-events-rule-{index}"),
            control_rule: format!("cfn-backup-events-control-rule-{index}"),
        }
    }
}

/// The ARN this emulator mints for a resource of `service` named `name`, built from the one
/// account id it serves — what the stack's own declarations resolve to.
fn arn(service: &str, name: &str) -> String {
    format!(
        "arn:aws:{service}:{REGION}:{}:{name}",
        engine::EMULATOR_ACCOUNT_ID
    )
}

/// An `Allow` of `actions` to `principals` on `resource`.
fn resource_policy(principals: Value, actions: Value, resource: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principals },
            "Action": actions,
            "Resource": resource,
        }],
    })
}

/// The whole pipeline the issue describes, in ONE template: the vault the backup is taken
/// into, the table it protects, the role it is performed under, the topic the rule hands the
/// envelope to with the queue subscribed to it, that queue's own policy, and the rule itself
/// with its target declared inline — plus the control rule and queue every row is read
/// against.
fn template(row: &Row, names: &Names) -> String {
    let mut pattern = json!({
        "source": [BACKUP_SOURCE],
        "detail-type": [BACKUP_JOB_STATE_CHANGE],
    });
    if let Some(clause) = row.detail {
        pattern["detail"] = clause();
    }
    let target = match row.target {
        Target::Topic => json!({ "Ref": "Topic" }),
        Target::Queue => json!({ "Fn::GetAtt": ["Queue", "Arn"] }),
    };
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Description": "a Backup plan vault whose job state changes fan out to a queue",
        "Resources": {
            "Vault": {
                "Type": "AWS::Backup::BackupVault",
                "Properties": { "BackupVaultName": &names.vault },
            },
            "Table": {
                "Type": "AWS::DynamoDB::Table",
                "Properties": {
                    "TableName": &names.table,
                    "AttributeDefinitions": [
                        { "AttributeName": "id", "AttributeType": "S" },
                    ],
                    "KeySchema": [{ "AttributeName": "id", "KeyType": "HASH" }],
                    "BillingMode": "PAY_PER_REQUEST",
                },
            },
            "Role": {
                "Type": "AWS::IAM::Role",
                "Properties": {
                    "RoleName": &names.role,
                    "AssumeRolePolicyDocument": {
                        "Version": "2012-10-17",
                        "Statement": [{
                            "Effect": "Allow",
                            "Principal": { "Service": "backup.amazonaws.com" },
                            "Action": "sts:AssumeRole",
                        }],
                    },
                    "Policies": [{
                        "PolicyName": "granted",
                        "PolicyDocument": {
                            "Version": "2012-10-17",
                            "Statement": [{
                                "Effect": "Allow",
                                "Action": if row.permitted {
                                    BACKUP_ACTION
                                } else {
                                    UNRELATED_ACTION
                                },
                                "Resource": "*",
                            }],
                        },
                    }],
                },
            },
            "Queue": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": &names.queue },
            },
            "QueuePolicy": {
                "Type": "AWS::SQS::QueuePolicy",
                "Properties": {
                    "Queues": [{ "Ref": "Queue" }],
                    "PolicyDocument": resource_policy(
                        json!([EVENTS_PRINCIPAL, SNS_PRINCIPAL]),
                        json!("sqs:SendMessage"),
                        json!({ "Fn::GetAtt": ["Queue", "Arn"] }),
                    ),
                },
            },
            "Topic": {
                "Type": "AWS::SNS::Topic",
                "Properties": { "TopicName": &names.topic },
            },
            "TopicPolicy": {
                "Type": "AWS::SNS::TopicPolicy",
                "Properties": {
                    "Topics": [{ "Ref": "Topic" }],
                    "PolicyDocument": resource_policy(
                        json!(EVENTS_PRINCIPAL),
                        json!("sns:Publish"),
                        json!({ "Ref": "Topic" }),
                    ),
                },
            },
            "Subscription": {
                "Type": "AWS::SNS::Subscription",
                "Properties": {
                    "TopicArn": { "Ref": "Topic" },
                    "Protocol": "sqs",
                    "Endpoint": { "Fn::GetAtt": ["Queue", "Arn"] },
                    "RawMessageDelivery": row.raw,
                },
            },
            "Rule": {
                "Type": "AWS::Events::Rule",
                "Properties": {
                    "Name": &names.rule,
                    "State": "ENABLED",
                    "EventPattern": pattern,
                    "Targets": [{ "Id": TARGET_ID, "Arn": target }],
                },
            },
            "ControlQueue": {
                "Type": "AWS::SQS::Queue",
                "Properties": { "QueueName": &names.control_queue },
            },
            "ControlQueuePolicy": {
                "Type": "AWS::SQS::QueuePolicy",
                "Properties": {
                    "Queues": [{ "Ref": "ControlQueue" }],
                    "PolicyDocument": resource_policy(
                        json!([EVENTS_PRINCIPAL, SNS_PRINCIPAL]),
                        json!("sqs:SendMessage"),
                        json!({ "Fn::GetAtt": ["ControlQueue", "Arn"] }),
                    ),
                },
            },
            "ControlRule": {
                "Type": "AWS::Events::Rule",
                "Properties": {
                    "Name": &names.control_rule,
                    "State": "ENABLED",
                    "EventPattern": {
                        "source": [BACKUP_SOURCE],
                        "detail-type": [BACKUP_JOB_STATE_CHANGE],
                    },
                    "Targets": [{
                        "Id": "ControlTarget",
                        "Arn": { "Fn::GetAtt": ["ControlQueue", "Arn"] },
                    }],
                },
            },
        },
        "Outputs": {
            "VaultName": { "Value": { "Ref": "Vault" } },
            "TableArn": { "Value": { "Fn::GetAtt": ["Table", "Arn"] } },
            "RoleArn": { "Value": { "Fn::GetAtt": ["Role", "Arn"] } },
            "TopicArn": { "Value": { "Ref": "Topic" } },
            "QueueUrl": { "Value": { "Ref": "Queue" } },
            "ControlQueueUrl": { "Value": { "Ref": "ControlQueue" } },
        },
    })
    .to_string()
}

/// Poll `DescribeBackupJob` until the job reports `wanted`, and answer the read it was seen
/// in. The wait is on that CONDITION, looked at again until it holds or [`JOB_BUDGET`] passes.
async fn awaited_job(
    router: &MultiServiceRouter,
    case: &str,
    job_id: &str,
    wanted: &str,
) -> Value {
    let uri = format!("/backup-jobs/{job_id}");
    let deadline = Instant::now() + JOB_BUDGET;
    loop {
        let described = backup(router, case, "GET", &uri, None).await;
        if described.get("State").and_then(Value::as_str) == Some(wanted) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected the job to reach {wanted:?}, got {described}"
        );
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Every DISTINCT body the queue at `url` held, waited for until one carries `needle` or
/// [`DELIVERY_BUDGET`] passes.
///
/// Distinct, because a received message this suite does not delete becomes visible again: two
/// identical bodies are one delivery looked at twice, while a second PUBLICATION of the same
/// state would carry an event id of its own and so a different body.
async fn awaited_bodies(router: &MultiServiceRouter, url: &str, needle: &str) -> BTreeSet<String> {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    loop {
        seen.extend(drain(router, url).await);
        if seen.iter().any(|body| body.contains(needle)) || Instant::now() >= deadline {
            return seen;
        }
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
}

/// Every DISTINCT body the queue at `url` held over [`QUIET_BUDGET`], looked at repeatedly —
/// what a client polling for a message that never comes sees.
async fn quiet_bodies(router: &MultiServiceRouter, url: &str) -> BTreeSet<String> {
    let deadline = Instant::now() + QUIET_BUDGET;
    let mut seen: BTreeSet<String> = BTreeSet::new();
    while Instant::now() < deadline {
        seen.extend(drain(router, url).await);
        tokio::time::sleep(LOOK_INTERVAL).await;
    }
    seen
}

/// The event envelope a queue body carries: the body ITSELF where the subscription asked for
/// raw delivery or the rule named the queue, and the `Message` of SNS's own notification where
/// it did not.
fn envelope_of(case: &str, body: &str, wrapped: bool, topic_arn: &str) -> Value {
    let parsed: Value = serde_json::from_str(body).unwrap_or_else(|error| {
        panic!("case {case:?}: expected the delivered body to be JSON ({error}), got {body:?}")
    });
    if !wrapped {
        return parsed;
    }
    // "RawMessageDelivery – When set to true, enables raw message delivery […] This eliminates
    // the need for the endpoints to process JSON formatting, which is otherwise created for
    // Amazon SNS metadata": a subscription that does not ask for it receives the notification
    // envelope, whose `Message` is the published body.
    assert_eq!(
        parsed.get("Type").and_then(Value::as_str),
        Some("Notification"),
        "case {case:?}: expected SNS's notification envelope, got {parsed}"
    );
    assert_eq!(
        parsed.get("TopicArn").and_then(Value::as_str),
        Some(topic_arn),
        "case {case:?}: expected the notification to name the stack's topic {topic_arn:?}, got \
         {parsed}"
    );
    let message = member(case, &parsed, "/Message");
    serde_json::from_str(&message).unwrap_or_else(|error| {
        panic!(
            "case {case:?}: expected the notification's Message to be the event envelope \
             ({error}), got {message:?}"
        )
    })
}

/// RULE: a job started against a template-provisioned vault publishes its every move on the
/// default bus, and the stack's own rule, topic, subscription and queue policies carry that
/// envelope to the stack's queue — while a rule that cannot match carries nothing.
#[tokio::test]
async fn a_cfn_deployed_backup_pipeline_delivers_the_jobs_state_change() {
    // ONE emulator for the whole table: every row names its own stack and its own resources
    // inside it, so no row can see another's — and a rule an earlier row left on the bus only
    // ever delivers to that row's own queues, which have already been read by the time the next
    // row starts.
    let router = router();
    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let names = Names::of(index);

        // ---- The stack, as a client deploys it: one template, one create, one wait.
        cloudformation(
            &router,
            "CreateStack",
            &[
                ("StackName", &names.stack),
                ("TemplateBody", &template(row, &names)),
                // "If your stack template contains IAM resources with custom names, you must
                // specify CAPABILITY_NAMED_IAM" — the stack names its backup role.
                ("Capabilities.member.1", "CAPABILITY_NAMED_IAM"),
            ],
        )
        .await;
        let (status, described) =
            wait_for_stack_status(&router, &names.stack, CREATE_COMPLETE).await;
        assert_eq!(
            status, CREATE_COMPLETE,
            "case {case:?}: expected the pipeline stack to deploy, got {status} with {described}"
        );

        // Every output the pipeline is driven from resolves, and each is the resource the
        // declaration asked for.
        let vault_name = output(case, &described, "VaultName");
        let table_arn = output(case, &described, "TableArn");
        let role_arn = output(case, &described, "RoleArn");
        let topic_arn = output(case, &described, "TopicArn");
        let queue_url = output(case, &described, "QueueUrl");
        let control_url = output(case, &described, "ControlQueueUrl");
        for (name, actual, expected) in [
            ("VaultName", &vault_name, names.vault.clone()),
            ("TableArn", &table_arn, arn("dynamodb", &format!("table/{}", names.table))),
            (
                "RoleArn",
                &role_arn,
                format!("arn:aws:iam::{}:role/{}", engine::EMULATOR_ACCOUNT_ID, names.role),
            ),
            ("TopicArn", &topic_arn, arn("sns", &names.topic)),
        ] {
            assert_eq!(
                actual, &expected,
                "case {case:?}: expected the stack's {name} to be {expected:?}, got {actual:?}"
            );
        }

        // The vault the template declared really is in AWS Backup, under the name the stack
        // published: the job below is started against the service's own resource.
        let described_vault = backup(
            &router,
            case,
            "GET",
            &format!("/backup-vaults/{vault_name}"),
            None,
        )
        .await;
        assert_eq!(
            member(case, &described_vault, "/BackupVaultName"),
            vault_name,
            "case {case:?}: expected DescribeBackupVault to report the stack's vault, got \
             {described_vault}"
        );

        // The rule's inline `Targets` really reached EventBridge: a rule with no target
        // delivers nowhere however well the envelope matches.
        let installed = events(
            &router,
            case,
            "ListTargetsByRule",
            &json!({ "Rule": &names.rule }),
        )
        .await;
        let expected_target = match row.target {
            Target::Topic => topic_arn.clone(),
            Target::Queue => arn("sqs", &names.queue),
        };
        assert_eq!(
            installed["Targets"],
            json!([{ "Id": TARGET_ID, "Arn": expected_target }]),
            "case {case:?}: expected the declared target on rule {:?}, got {installed}",
            names.rule
        );

        // ---- The job, started exactly as the issue's client starts it: the stack's vault, the
        // table ARN and the role the stack created.
        let started = backup(
            &router,
            case,
            "PUT",
            "/backup-jobs",
            Some(&json!({
                "BackupVaultName": &vault_name,
                "ResourceArn": &table_arn,
                "IamRoleArn": &role_arn,
            })),
        )
        .await;
        let job_id = member(case, &started, "/BackupJobId");
        let recovery_point_arn = member(case, &started, "/RecoveryPointArn");

        // The job reaches the terminal state its own role leads it to, which is the state the
        // envelope must carry.
        let job = awaited_job(&router, case, &job_id, row.job_state).await;
        assert_eq!(
            member(case, &job, "/BackupVaultName"),
            vault_name,
            "case {case:?}: expected the job to report the stack's vault, got {job}"
        );

        // ---- The control: one rule of the SAME stack, matching the source and detail type
        // alone, whose target is the control queue. Its message is what says the envelope was
        // published and dispatched at all.
        let control = awaited_bodies(&router, &control_url, &job_id).await;
        let control_envelopes: Vec<Value> = control
            .iter()
            .map(|body| envelope_of(case, body, false, &topic_arn))
            .filter(|envelope| envelope.pointer("/detail/backupJobId") == Some(&json!(&job_id)))
            .collect();
        assert_eq!(
            control_envelopes.len(),
            1,
            "case {case:?}: expected the control queue to receive exactly one envelope of job \
             {job_id:?}, got {control:?}"
        );

        let Some(expected_state) = row.delivered else {
            // The control proves the publication and the dispatch happened; the row's own queue
            // is then looked at repeatedly, and must hold nothing of this job.
            let quiet = quiet_bodies(&router, &queue_url).await;
            assert!(
                !quiet.iter().any(|body| body.contains(&job_id)),
                "case {case:?}: a pattern naming {ABORTED:?} matches no state this job takes, \
                 yet the stack's queue holds {quiet:?} for job {job_id:?}"
            );
            continue;
        };

        // ---- What a client reads off the stack's queue.
        let bodies = awaited_bodies(&router, &queue_url, &job_id).await;
        let wrapped = row.target == Target::Topic && !row.raw;
        let envelopes: Vec<Value> = bodies
            .iter()
            .map(|body| envelope_of(case, body, wrapped, &topic_arn))
            .filter(|envelope| envelope.pointer("/detail/backupJobId") == Some(&json!(&job_id)))
            .collect();
        let states: Vec<String> = envelopes
            .iter()
            .map(|envelope| {
                envelope["detail"]["state"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let matching: Vec<&Value> = envelopes
            .iter()
            .filter(|envelope| envelope["detail"]["state"] == json!(expected_state))
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "case {case:?}: expected exactly one delivered envelope carrying \
             {expected_state:?}, got {states:?} from {bodies:?}"
        );
        let envelope = matching[0].clone();

        // The envelope is the whole one AWS hands a target, and its detail is this job's own
        // stored state — the values the control plane reports for the same job.
        for (pointer, expect) in [
            ("/source", json!(BACKUP_SOURCE)),
            ("/detail-type", json!(BACKUP_JOB_STATE_CHANGE)),
            ("/account", json!(engine::EMULATOR_ACCOUNT_ID)),
            ("/region", json!(REGION)),
            ("/resources", json!([&recovery_point_arn])),
            ("/detail/backupJobId", json!(&job_id)),
            ("/detail/state", json!(expected_state)),
            ("/detail/backupVaultName", json!(&vault_name)),
            ("/detail/resourceArn", json!(&table_arn)),
            ("/detail/resourceType", json!(DYNAMODB_TYPE)),
            ("/detail/iamRoleArn", json!(&role_arn)),
        ] {
            let actual = envelope.pointer(pointer).cloned().unwrap_or(Value::Null);
            assert_eq!(
                actual, expect,
                "case {case:?}: expected {pointer} to be {expect}, got {actual} in {envelope}"
            );
        }
        assert!(
            envelope["id"].as_str().is_some_and(|id| !id.is_empty()),
            "case {case:?}: expected the envelope to carry an event id, got {envelope}"
        );
        assert!(
            envelope["time"].as_str().is_some_and(|at| at.ends_with('Z')),
            "case {case:?}: expected the envelope to state when the job moved, got {envelope}"
        );
        // A job that could not be performed says why; one that was performed has nothing to
        // report.
        let status_message = envelope
            .pointer("/detail/statusMessage")
            .and_then(Value::as_str);
        if expected_state == FAILED {
            assert!(
                status_message.is_some_and(|message| !message.is_empty()),
                "case {case:?}: a FAILED job's envelope must say why, got {envelope}"
            );
        } else {
            assert_eq!(
                status_message, None,
                "case {case:?}: a performed backup reports no status message, got {envelope}"
            );
        }
        // The state the envelope carried is the state the control plane reports for the same
        // job: the envelope is that job's own stored state, not a second copy of it.
        assert_eq!(
            member(case, &job, "/State"),
            expected_state,
            "case {case:?}: expected DescribeBackupJob to report {expected_state:?}, got {job}"
        );
    }
}
