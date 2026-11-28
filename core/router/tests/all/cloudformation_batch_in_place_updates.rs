// SPDX-License-Identifier: CC0-1.0

//! What an `UpdateStack` does to an AWS Batch resource a stack already created, over the two
//! services it takes to decide: CloudFormation applies the change, AWS Batch owns the
//! resource and answers for it afterwards.
//!
//! RULE under test: **a property of a declared resource that the OWNING service's own update
//! call takes a member for is applied to the resource that already exists through that call —
//! the deployment reaches `UPDATE_COMPLETE`, the resource keeps the identity its create minted
//! (so every stack output and every declaration that reads it stays valid), and the owning
//! service reports the declared value; a property that call has no member for is fixed when
//! the resource is made, so a declaration restating it REPLACES the resource, the stack
//! publishes the identity the new one carries and the one it replaced is taken down in the
//! deployment's cleanup phase.**
//!
//! For AWS Batch that reading comes from the update calls' own member lists
//! (`smithy_query(query="operation", operation="UpdateJobQueue,UpdateServiceEnvironment,\
//! UpdateSchedulingPolicy")`): `UpdateJobQueue` takes a queue's `priority`, `state`,
//! `schedulingPolicyArn`, `computeEnvironmentOrder`, `serviceEnvironmentOrder` and
//! `jobStateTimeLimitActions` and no `jobQueueName` or `jobQueueType`;
//! `UpdateServiceEnvironment` takes an environment's `state` and `capacityLimits` and neither
//! its name nor its type; `UpdateSchedulingPolicy` takes a policy's share policy and not its
//! name.
//!
//! The rows of the job-queue table differ along what the rule generalises over: which
//! property is restated (a number, an enum, a list of modelled structures), the VALUE it takes
//! (well inside the range, the published bound at each end), how many entries a list-valued
//! property carries and in which ORDER, and how many properties one deployment restates at
//! once.
//!
//! Two crates have to agree for the rule to hold (CloudFormation makes the calls, AWS Batch
//! serves them and answers for the resource afterwards), so the suite lives here rather than
//! in either crate.
//!
//! Nothing here waits out a span: every stack is polled until it rests in a terminal status or
//! a generous deadline passes ([`settled_stack`]).

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The awsQuery form encoder and the XML element readers this crate writes ONCE
// (`tests/all/query_wire.rs`): "Nothing new should write another one — call this module."
use super::query_wire::{form_encode, opt_element, outputs_of};

/// The Region every stack here deploys in, which is the Region its resources are created in
/// and the one their ARNs carry.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// How long a wait for a stack to reach a terminal status may run before the suite calls it a
/// failure. Generous by design: what is asserted is WHICH status the stack rests in and what
/// the owning service then holds, not how fast the host got there.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(120);

/// How long to wait before looking at the stack again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The terminal `StackStatus` values this suite reads, from the model's own `StackStatus`
/// enum: the status a completed create rests in, the one a completed update rests in, and the
/// one an update whose resource change failed rests in.
const CREATE_COMPLETE: &str = "CREATE_COMPLETE";
const UPDATE_COMPLETE: &str = "UPDATE_COMPLETE";

/// The `JQStatus`/`SEStatus` of a Batch resource that service has finished creating or
/// updating.
const VALID: &str = "VALID";

/// The `ServiceEnvironmentType` an `AWS::Batch::ServiceEnvironment` declares and the
/// `JobQueueType` of a queue that orders one
/// (`smithy_query(query="shape", shape="ServiceEnvironmentType")`).
const SAGEMAKER_TRAINING: &str = "SAGEMAKER_TRAINING";

/// Two of the `JobQueueType` members AWS Batch models
/// (`smithy_query(query="shape", shape="JobQueueType")`: `EKS | ECS | ECS_FARGATE |
/// SAGEMAKER_TRAINING | ECS_MANAGED_INSTANCES`), used by the rows that restate the kind of
/// queue a declaration asks for.
const ECS_FARGATE: &str = "ECS_FARGATE";
const ECS: &str = "ECS";

/// The unit a service environment's capacity limit is measured in, from
/// `::service_com_amazonaws_batch_awsbatchv20160810::state::CAPACITY_UNIT_NUM_INSTANCES`.
const NUM_INSTANCES: &str = "NUM_INSTANCES";

/// The only `QuotaShareIdleResourceAssignmentStrategy` AWS Batch models
/// (`smithy_query(query="shape", shape="QuotaShareIdleResourceAssignmentStrategy")`).
const FIFO: &str = "FIFO";

/// The ceiling the `AWS::Batch::JobQueue` schema states for `Priority`, from the `Maximum`
/// that type's entry in the "AWS resource and property types reference" publishes.
const MAX_JOB_QUEUE_PRIORITY: i64 = 1_000;

/// The bounds `JobStateTimeLimitAction$maxTimeSeconds` documents: "The minimum value is 600
/// (10 minutes) and the maximum value is 86,400 (24 hours)."
const MIN_MAX_TIME_SECONDS: i64 = 600;

/// The bounds `FairsharePolicy$computeReservation` documents ("The minimum value is 0 and the
/// maximum value is 99") and `FairsharePolicy$shareDecaySeconds` ("The maximum supported value
/// is 604800 (1 week)").
const MAX_COMPUTE_RESERVATION: i64 = 99;
const MAX_SHARE_DECAY_SECONDS: i64 = 604_800;

/// The emulator with the two services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them. Request authentication is off, as in every other
/// suite here; the in-process callback CloudFormation makes signs its request either way (the
/// credential scope is what routes it).
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
        service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request and what each handler reads its Region from. The signature is
/// never checked: this router runs with request authentication off.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// Call one CloudFormation operation over its awsQuery wire.
async fn cloudformation(
    router: &MultiServiceRouter,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-15");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("cloudformation.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("cloudformation"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload = String::from_utf8_lossy(response.body()).to_string();
    (status, payload)
}

/// Call one AWS Batch operation over its restJson1 wire.
async fn batch(router: &MultiServiceRouter, uri: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", format!("batch.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("batch"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let payload: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, payload)
}

/// The ARN a Batch resource of `kind` and `name` carries: the same total function of the
/// Region and the name both services compute it from.
fn batch_arn(kind: &str, name: &str) -> String {
    format!("arn:aws:batch:{REGION}:{ACCOUNT}:{kind}/{name}")
}

/// The stack as `DescribeStacks` answers it once its operation has reached a terminal status —
/// looked at again until it rests there or [`TERMINAL_DEADLINE`] passes, so what the suite
/// asserts never turns on how fast the host ran.
async fn settled_stack(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let (code, described) =
            cloudformation(router, "DescribeStacks", &[("StackName", stack)]).await;
        assert_eq!(
            code, 200,
            "case {case:?}: DescribeStacks must answer 200, got {code} {described}"
        );
        let reported = opt_element(&described, "StackStatus").unwrap_or_default();
        if !reported.is_empty() && !reported.ends_with("_IN_PROGRESS") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {stack} to rest in a terminal status within \
             {TERMINAL_DEADLINE:?}, last read {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The ONE record a Batch `Describe*` holds for `identifier`, or `None` where it holds none —
/// those readers take a LIST and answer the records they found, so a resource that service no
/// longer holds comes back as an empty list.
async fn batch_record(
    router: &MultiServiceRouter,
    case: &str,
    uri: &str,
    member: &str,
    identifier: &str,
) -> Option<Value> {
    let (code, answered) = batch(router, uri, &json!({ member: [identifier] })).await;
    assert_eq!(
        code, 200,
        "case {case:?}: {uri} must answer 200 for {identifier}, got {code} {answered}"
    );
    answered
        .get(member)
        .and_then(Value::as_array)
        .and_then(|records| records.first())
        .cloned()
}

/// Deploy `template` as `stack` and assert it settles `CREATE_COMPLETE`, answering the
/// stack's own description.
async fn created(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) -> String {
    let (code, answered) = cloudformation(
        router,
        "CreateStack",
        &[("StackName", stack), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: CreateStack must answer 200, got {code} {answered}"
    );
    let described = settled_stack(router, case, stack).await;
    let settled = opt_element(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        settled, CREATE_COMPLETE,
        "case {case:?}: expected the create to settle {CREATE_COMPLETE}, got {settled:?} \
         ({described})"
    );
    described
}

/// Apply `template` to `stack` and assert the update settles `UPDATE_COMPLETE`, answering the
/// stack's own description.
async fn updated(router: &MultiServiceRouter, case: &str, stack: &str, template: &str) -> String {
    let (code, answered) = cloudformation(
        router,
        "UpdateStack",
        &[("StackName", stack), ("TemplateBody", template)],
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: UpdateStack must answer 200, got {code} {answered}"
    );
    let described = settled_stack(router, case, stack).await;
    let settled = opt_element(&described, "StackStatus").unwrap_or_default();
    assert_eq!(
        settled, UPDATE_COMPLETE,
        "case {case:?}: expected the update to settle {UPDATE_COMPLETE}, got {settled:?} \
         ({described})"
    );
    described
}

/// One `AWS::Batch::ComputeEnvironment` the queue stacks order: a MANAGED Fargate environment,
/// which is what a queue of Fargate compute resources needs.
fn fargate_environment(name: &str) -> Value {
    json!({
        "Type": "AWS::Batch::ComputeEnvironment",
        "Properties": {
            "ComputeEnvironmentName": name,
            "Type": "MANAGED",
            "State": "ENABLED",
            "ComputeResources": {
                "Type": "FARGATE",
                "MaxvCpus": 4,
                "Subnets": ["subnet-0123456789abcdef0"],
                "SecurityGroupIds": ["sg-0123456789abcdef0"],
            },
        },
    })
}

/// The template the job-queue rows deploy: two Fargate compute environments and the queue
/// `properties` declares, with the handles of all three published as outputs.
fn queue_template(properties: Value) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "CE1": fargate_environment("batch-in-place-ce-one"),
            "CE2": fargate_environment("batch-in-place-ce-two"),
            "JQ": {"Type": "AWS::Batch::JobQueue", "Properties": properties},
        },
        "Outputs": {
            "CE1Ref": {"Value": {"Ref": "CE1"}},
            "CE2Ref": {"Value": {"Ref": "CE2"}},
            "JQRef": {"Value": {"Ref": "JQ"}},
            "JQArn": {"Value": {"Fn::GetAtt": ["JQ", "JobQueueArn"]}},
        },
    })
    .to_string()
}

/// The queue every job-queue row starts from: priority 1, open to submissions, ordering the
/// first of the two compute environments.
fn base_queue(name: &str) -> Value {
    json!({
        "JobQueueName": name,
        "JobQueueType": ECS_FARGATE,
        "Priority": 1,
        "State": "ENABLED",
        "ComputeEnvironmentOrder": [{"Order": 1, "ComputeEnvironment": {"Ref": "CE1"}}],
    })
}

/// One row of the in-place table.
struct InPlaceRow {
    case: &'static str,
    /// The stack this row deploys, and the name its queue is declared under.
    stack: &'static str,
    queue: &'static str,
    /// The queue's properties as the UPDATE declares them: the base queue with the one thing
    /// this row restates.
    updated: fn(&str) -> Value,
    /// The members `DescribeJobQueues` must report for the queue afterwards, in AWS Batch's
    /// own spelling. `{CE1}` and `{CE2}` stand for the two compute environments' ARNs, which
    /// only the deployment knows ([`substituted`]).
    reported: fn() -> Value,
}

/// Every in-place row, differing along which property is restated, the value it takes, the
/// length and order of a list-valued one, and how many properties change at once.
fn in_place_rows() -> Vec<InPlaceRow> {
    vec![
        InPlaceRow {
            case: "a higher priority, which is the one property the report that led here cited",
            stack: "batch-in-place-priority",
            queue: "batch-in-place-priority-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                properties["Priority"] = json!(5);
                properties
            },
            reported: || json!({"priority": 5}),
        },
        InPlaceRow {
            case: "the priority raised to the ceiling the type's schema states",
            stack: "batch-in-place-ceiling",
            queue: "batch-in-place-ceiling-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                properties["Priority"] = json!(MAX_JOB_QUEUE_PRIORITY);
                properties
            },
            reported: || json!({"priority": MAX_JOB_QUEUE_PRIORITY}),
        },
        InPlaceRow {
            case: "the priority lowered to zero",
            stack: "batch-in-place-zero",
            queue: "batch-in-place-zero-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                properties["Priority"] = json!(0);
                properties
            },
            reported: || json!({"priority": 0}),
        },
        InPlaceRow {
            case: "the queue closed to new submissions, which restates an ENUM property",
            stack: "batch-in-place-state",
            queue: "batch-in-place-state-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                properties["State"] = json!("DISABLED");
                properties
            },
            reported: || json!({"state": "DISABLED", "priority": 1}),
        },
        InPlaceRow {
            case: "a second compute environment ordered behind the first, which lengthens a \
                   list of modelled structures",
            stack: "batch-in-place-two-environments",
            queue: "batch-in-place-two-environments-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                properties["ComputeEnvironmentOrder"] = json!([
                    {"Order": 1, "ComputeEnvironment": {"Ref": "CE1"}},
                    {"Order": 2, "ComputeEnvironment": {"Ref": "CE2"}},
                ]);
                properties
            },
            reported: || {
                json!({"computeEnvironmentOrder": [
                    {"order": 1, "computeEnvironment": "{CE1}"},
                    {"order": 2, "computeEnvironment": "{CE2}"},
                ]})
            },
        },
        InPlaceRow {
            case: "the compute environment order re-pointed at the other environment",
            stack: "batch-in-place-repointed",
            queue: "batch-in-place-repointed-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                properties["ComputeEnvironmentOrder"] =
                    json!([{"Order": 1, "ComputeEnvironment": {"Ref": "CE2"}}]);
                properties
            },
            reported: || {
                json!({"computeEnvironmentOrder": [
                    {"order": 1, "computeEnvironment": "{CE2}"},
                ]})
            },
        },
        InPlaceRow {
            case: "a job-state time limit action added at the floor its own member documents",
            stack: "batch-in-place-time-limit",
            queue: "batch-in-place-time-limit-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                // `JobStateTimeLimitAction`'s own docs: the only state that triggers one is
                // `RUNNABLE`, and for a queue on Fargate compute the only action is `CANCEL`.
                properties["JobStateTimeLimitActions"] = json!([{
                    "Action": "CANCEL",
                    "MaxTimeSeconds": MIN_MAX_TIME_SECONDS,
                    "Reason": "MISCONFIGURATION:JOB_RESOURCE_REQUIREMENT",
                    "State": "RUNNABLE",
                }]);
                properties
            },
            reported: || {
                json!({"jobStateTimeLimitActions": [{
                    "action": "CANCEL",
                    "maxTimeSeconds": MIN_MAX_TIME_SECONDS,
                    "reason": "MISCONFIGURATION:JOB_RESOURCE_REQUIREMENT",
                    "state": "RUNNABLE",
                }]})
            },
        },
        InPlaceRow {
            case: "several properties restated by one deployment",
            stack: "batch-in-place-several",
            queue: "batch-in-place-several-jq",
            updated: |name| {
                let mut properties = base_queue(name);
                properties["Priority"] = json!(9);
                properties["State"] = json!("DISABLED");
                properties["ComputeEnvironmentOrder"] =
                    json!([{"Order": 3, "ComputeEnvironment": {"Ref": "CE2"}}]);
                properties
            },
            reported: || {
                json!({
                    "priority": 9,
                    "state": "DISABLED",
                    "computeEnvironmentOrder": [{"order": 3, "computeEnvironment": "{CE2}"}],
                })
            },
        },
    ]
}

/// `expected` with every `{CE1}`/`{CE2}` placeholder replaced by the ARN the deployment gave
/// that compute environment, so a row states what it expects without knowing either ARN.
fn substituted(expected: &Value, ce1: &str, ce2: &str) -> Value {
    match expected {
        Value::String(text) => Value::String(text.replace("{CE1}", ce1).replace("{CE2}", ce2)),
        Value::Array(entries) => Value::Array(
            entries
                .iter()
                .map(|entry| substituted(entry, ce1, ce2))
                .collect(),
        ),
        Value::Object(members) => Value::Object(
            members
                .iter()
                .map(|(member, value)| (member.clone(), substituted(value, ce1, ce2)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// Every in-place row: the update reaches `UPDATE_COMPLETE`, the queue keeps the ARN its
/// create minted (so the stack's `Ref` and `Fn::GetAtt` publish the same handle as before),
/// and AWS Batch reports the queue under its unchanged name with the declared values and back
/// in `VALID`.
#[tokio::test]
async fn a_job_queue_property_the_update_api_takes_is_applied_in_place() {
    for row in in_place_rows() {
        let case = row.case;
        let router = build_router();

        let described = created(
            &router,
            case,
            row.stack,
            &queue_template(base_queue(row.queue)),
        )
        .await;
        let outputs = outputs_of(&described);
        let queue_arn = batch_arn("job-queue", row.queue);
        for key in ["JQRef", "JQArn"] {
            assert_eq!(
                outputs.get(key).map(String::as_str),
                Some(queue_arn.as_str()),
                "case {case:?}: expected output {key:?} to be the queue ARN {queue_arn}, got \
                 {outputs:?}"
            );
        }
        let ce1 = outputs
            .get("CE1Ref")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no CE1Ref output in {described}"));
        let ce2 = outputs
            .get("CE2Ref")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: no CE2Ref output in {described}"));

        let described = updated(
            &router,
            case,
            row.stack,
            &queue_template((row.updated)(row.queue)),
        )
        .await;
        let outputs = outputs_of(&described);
        for key in ["JQRef", "JQArn"] {
            assert_eq!(
                outputs.get(key).map(String::as_str),
                Some(queue_arn.as_str()),
                "case {case:?}: an in-place update keeps the queue's identity, so output \
                 {key:?} must still be {queue_arn}, got {outputs:?}"
            );
        }

        let record = batch_record(
            &router,
            case,
            "/v1/describejobqueues",
            "jobQueues",
            &queue_arn,
        )
        .await
        .unwrap_or_else(|| panic!("case {case:?}: AWS Batch holds no queue for {queue_arn}"));
        assert_eq!(
            record["jobQueueName"].as_str(),
            Some(row.queue),
            "case {case:?}: the queue keeps the name it was created under, got {record}"
        );
        assert_eq!(
            record["jobQueueArn"].as_str(),
            Some(queue_arn.as_str()),
            "case {case:?}: expected the queue to still carry {queue_arn}, got {record}"
        );
        assert_eq!(
            record["status"].as_str(),
            Some(VALID),
            "case {case:?}: an update the stack completed leaves the queue {VALID}, got {record}"
        );
        let expected = substituted(&(row.reported)(), &ce1, &ce2);
        for (member, value) in expected
            .as_object()
            .expect("every row states its expectation as an object")
        {
            assert_eq!(
                record.get(member),
                Some(value),
                "case {case:?}: expected AWS Batch to report {member} as {value}, got \
                 {record}"
            );
        }
    }
}

/// One row of the replacement table: the properties the update restates, and what the queue it
/// replaces the old one with must then be called.
struct ReplacementRow {
    case: &'static str,
    stack: &'static str,
    /// The name the queue is created under, and the one the update declares.
    queue: &'static str,
    renamed: &'static str,
    /// The `JobQueueType` the update declares, where the row restates it too.
    queue_type: &'static str,
}

/// Every replacement row: a declaration restating a property `UpdateJobQueue` has no member
/// for reaches `UPDATE_COMPLETE` by CREATING the queue it asks for — the stack publishes the
/// new queue's ARN, AWS Batch answers for it with the declared values, and the queue it
/// replaced is gone from that service once the deployment's cleanup phase has run.
#[tokio::test]
async fn a_job_queue_property_the_update_api_cannot_take_replaces_the_queue() {
    let rows = [
        ReplacementRow {
            case: "the queue renamed, its name being create-only for every Batch type",
            stack: "batch-replacement-rename",
            queue: "batch-replacement-rename-jq",
            renamed: "batch-replacement-renamed-jq",
            queue_type: ECS_FARGATE,
        },
        ReplacementRow {
            case: "the queue renamed and its KIND restated, which UpdateJobQueue has no member \
                   for either",
            stack: "batch-replacement-kind",
            queue: "batch-replacement-kind-jq",
            renamed: "batch-replacement-kind-other-jq",
            queue_type: ECS,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = build_router();

        let described = created(
            &router,
            case,
            row.stack,
            &queue_template(base_queue(row.queue)),
        )
        .await;
        let before = batch_arn("job-queue", row.queue);
        assert_eq!(
            outputs_of(&described).get("JQRef").map(String::as_str),
            Some(before.as_str()),
            "case {case:?}: expected the create to publish {before}, got {described}"
        );

        let mut restated = base_queue(row.renamed);
        restated["JobQueueType"] = json!(row.queue_type);
        let described = updated(&router, case, row.stack, &queue_template(restated)).await;
        let after = batch_arn("job-queue", row.renamed);
        assert_eq!(
            outputs_of(&described).get("JQRef").map(String::as_str),
            Some(after.as_str()),
            "case {case:?}: a replacement publishes the new queue's ARN {after}, got {described}"
        );

        let record = batch_record(
            &router,
            case,
            "/v1/describejobqueues",
            "jobQueues",
            &after,
        )
        .await
        .unwrap_or_else(|| panic!("case {case:?}: AWS Batch holds no queue for {after}"));
        assert_eq!(
            record["jobQueueName"].as_str(),
            Some(row.renamed),
            "case {case:?}: expected the replacement to be named {}, got {record}",
            row.renamed
        );
        assert_eq!(
            record["jobQueueType"].as_str(),
            Some(row.queue_type),
            "case {case:?}: expected the replacement's kind to be {}, got {record}",
            row.queue_type
        );
        assert_eq!(
            batch_record(
                &router,
                case,
                "/v1/describejobqueues",
                "jobQueues",
                &before,
            )
            .await,
            None,
            "case {case:?}: the queue the update replaced is taken down in the cleanup phase, \
             so AWS Batch must hold nothing for {before}"
        );
    }
}

/// The template the SageMaker-training estate deploys: the scheduling policy, the service
/// environment `limits`/`state` declare, and the queue that orders it.
fn service_environment_template(max_capacity: i64, state: &str) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "SP": {
                "Type": "AWS::Batch::SchedulingPolicy",
                "Properties": {
                    "Name": "batch-in-place-se-policy",
                    "QuotaSharePolicy": {"IdleResourceAssignmentStrategy": FIFO},
                },
            },
            "SE": {
                "Type": "AWS::Batch::ServiceEnvironment",
                "Properties": {
                    "ServiceEnvironmentName": "batch-in-place-se",
                    "ServiceEnvironmentType": SAGEMAKER_TRAINING,
                    "State": state,
                    "CapacityLimits": [
                        {"MaxCapacity": max_capacity, "CapacityUnit": NUM_INSTANCES},
                    ],
                },
            },
            "JQ": {
                "Type": "AWS::Batch::JobQueue",
                "Properties": {
                    "JobQueueName": "batch-in-place-se-jq",
                    "JobQueueType": SAGEMAKER_TRAINING,
                    "Priority": 1,
                    "SchedulingPolicyArn": {"Ref": "SP"},
                    "ServiceEnvironmentOrder": [
                        {"Order": 1, "ServiceEnvironment": {"Ref": "SE"}},
                    ],
                },
            },
        },
        "Outputs": {
            "SERef": {"Value": {"Ref": "SE"}},
            "SEArn": {"Value": {"Fn::GetAtt": ["SE", "ServiceEnvironmentArn"]}},
        },
    })
    .to_string()
}

/// The same rule on an `AWS::Batch::ServiceEnvironment`: `UpdateServiceEnvironment` takes an
/// environment's state and its capacity limits, so a declaration restating either is applied
/// to the environment that exists — which keeps the ARN the queues of the stack order it by.
#[tokio::test]
async fn a_service_environments_state_and_capacity_are_applied_in_place() {
    let case = "a service environment given more capacity and closed to new work";
    let stack = "batch-in-place-service-environment";
    let router = build_router();

    let described = created(&router, case, stack, &service_environment_template(2, "ENABLED")).await;
    let environment_arn = batch_arn("service-environment", "batch-in-place-se");
    for key in ["SERef", "SEArn"] {
        assert_eq!(
            outputs_of(&described).get(key).map(String::as_str),
            Some(environment_arn.as_str()),
            "case {case:?}: expected output {key:?} to be {environment_arn}, got {described}"
        );
    }

    let described = updated(
        &router,
        case,
        stack,
        &service_environment_template(5, "DISABLED"),
    )
    .await;
    for key in ["SERef", "SEArn"] {
        assert_eq!(
            outputs_of(&described).get(key).map(String::as_str),
            Some(environment_arn.as_str()),
            "case {case:?}: an in-place update keeps the environment's identity, so {key:?} \
             must still be {environment_arn}, got {described}"
        );
    }

    let record = batch_record(
        &router,
        case,
        "/v1/describeserviceenvironments",
        "serviceEnvironments",
        &environment_arn,
    )
    .await
    .unwrap_or_else(|| panic!("case {case:?}: AWS Batch holds no environment for {environment_arn}"));
    assert_eq!(
        record["state"].as_str(),
        Some("DISABLED"),
        "case {case:?}: expected the declared state, got {record}"
    );
    assert_eq!(
        record["capacityLimits"],
        json!([{"maxCapacity": 5, "capacityUnit": NUM_INSTANCES}]),
        "case {case:?}: expected the declared capacity limits, got {record}"
    );
    assert_eq!(
        record["serviceEnvironmentType"].as_str(),
        Some(SAGEMAKER_TRAINING),
        "case {case:?}: the environment keeps the type it was created with, got {record}"
    );
}

/// The template the scheduling-policy rows deploy: one policy allocating by the fair-share
/// members the row states.
fn scheduling_policy_template(compute_reservation: i64, share_decay_seconds: i64) -> String {
    json!({
        "AWSTemplateFormatVersion": "2010-09-09",
        "Resources": {
            "SP": {
                "Type": "AWS::Batch::SchedulingPolicy",
                "Properties": {
                    "Name": "batch-in-place-policy",
                    "FairsharePolicy": {
                        "ComputeReservation": compute_reservation,
                        "ShareDecaySeconds": share_decay_seconds,
                        "ShareDistribution": [{"ShareIdentifier": "A*", "WeightFactor": 1.0}],
                    },
                },
            },
        },
        "Outputs": {
            "SPRef": {"Value": {"Ref": "SP"}},
            "SPArn": {"Value": {"Fn::GetAtt": ["SP", "Arn"]}},
        },
    })
    .to_string()
}

/// And on an `AWS::Batch::SchedulingPolicy`: `UpdateSchedulingPolicy` takes a policy's share
/// policy, so a declaration restating it is applied to the policy that exists — which keeps
/// the ARN a queue's `SchedulingPolicyArn` was created against. The row moves both bounded
/// members to the ceilings their own docs publish.
#[tokio::test]
async fn a_scheduling_policys_share_policy_is_applied_in_place() {
    let case = "a policy's reservation and decay window raised to their published ceilings";
    let stack = "batch-in-place-scheduling-policy";
    let router = build_router();

    let described = created(&router, case, stack, &scheduling_policy_template(10, 600)).await;
    let policy_arn = batch_arn("scheduling-policy", "batch-in-place-policy");
    for key in ["SPRef", "SPArn"] {
        assert_eq!(
            outputs_of(&described).get(key).map(String::as_str),
            Some(policy_arn.as_str()),
            "case {case:?}: expected output {key:?} to be {policy_arn}, got {described}"
        );
    }

    let described = updated(
        &router,
        case,
        stack,
        &scheduling_policy_template(MAX_COMPUTE_RESERVATION, MAX_SHARE_DECAY_SECONDS),
    )
    .await;
    for key in ["SPRef", "SPArn"] {
        assert_eq!(
            outputs_of(&described).get(key).map(String::as_str),
            Some(policy_arn.as_str()),
            "case {case:?}: an in-place update keeps the policy's identity, so {key:?} must \
             still be {policy_arn}, got {described}"
        );
    }

    let (code, answered) = batch(
        &router,
        "/v1/describeschedulingpolicies",
        &json!({"arns": [policy_arn]}),
    )
    .await;
    assert_eq!(
        code, 200,
        "case {case:?}: DescribeSchedulingPolicies must answer 200, got {code} {answered}"
    );
    let policy = &answered["schedulingPolicies"][0];
    assert_eq!(
        policy["arn"].as_str(),
        Some(policy_arn.as_str()),
        "case {case:?}: expected the policy to still carry {policy_arn}, got {answered}"
    );
    assert_eq!(
        policy["fairsharePolicy"]["computeReservation"].as_i64(),
        Some(MAX_COMPUTE_RESERVATION),
        "case {case:?}: expected the declared reservation, got {answered}"
    );
    assert_eq!(
        policy["fairsharePolicy"]["shareDecaySeconds"].as_i64(),
        Some(MAX_SHARE_DECAY_SECONDS),
        "case {case:?}: expected the declared decay window, got {answered}"
    );
}
