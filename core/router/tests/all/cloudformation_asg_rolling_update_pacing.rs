// SPDX-License-Identifier: CC0-1.0

//! What a client WATCHING an Auto Scaling group sees while a stack update governed by an
//! `UpdatePolicy`'s `AutoScalingRollingUpdate` works through the group's instances.
//!
//! The RULE under test: **a rolling update is PACED, and the pacing is what a reader of the
//! group sees.** Each batch takes at most `MaxBatchSize` of the group's instances OUT of
//! service before their replacements are launched — this policy replaces in place, so the
//! group is never taken past the capacity the replacement works at — and the group holds that
//! dip for the span a replacement takes to come up
//! (`service_com_amazonaws_cloudformation_cloudformation::state::rolling_update::BATCH_REPLACEMENT`),
//! so a client reading `DescribeAutoScalingGroups` while the stack reports
//! `UPDATE_IN_PROGRESS` observes, batch after batch, an in-service count held at exactly
//! `DesiredCapacity` minus that batch and never below the `MinInstancesInService` the policy
//! states. The update then reaches a terminal status with every instance of the earlier
//! configuration replaced.
//!
//! Why a client cares: `AutoScalingRollingUpdate` exists to make a deploy observable and
//! stoppable — a load balancer keeps serving from the instances the floor holds back, and a
//! canary or a deployment gate watching the group is what catches a bad revision at the first
//! batch. An update that swaps the whole group inside one look of such a watcher honours
//! neither: the floor cannot be checked, and there is no batch boundary at which anything
//! could have been stopped.
//!
//! The rows span what the rule generalises over: how many instances the group holds (one, two
//! and three), how many one batch takes (one and two), the floor that must stay in service
//! (none, and one), how many batches the replacement therefore takes (one and two), and
//! whether the policy waits on the replacements' own signals. Nothing here waits for a span to
//! pass: every wait polls for the CONDITION it is about, on a cadence derived from the
//! emulator's own replacement span, and the counts asserted are what those polls saw.
//!
//! `cloudformation_asg_rolling_update_batches.rs` beside this file is the suite for the batch
//! BOUNDARIES themselves — which instances one batch may take, and what the signal gate decides
//! — and `signalling_instance_gates.rs` in the CloudFormation crate is the suite for the
//! signals an estate's own instances send; neither is re-pinned by these rows.

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use bytes::Bytes;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudformation_cloudformation::state::rolling_update::BATCH_REPLACEMENT;

// The ONE percent-encoder and element readers this test crate keeps for query suites
// (`all/query_wire.rs`: "Nothing new should write another one — call this module").
use crate::query_wire::{block_of, element, elements, form_encode, members_of, outputs_of};

/// The Region every request below is signed for.
const REGION: &str = "us-west-2";

/// The API versions the two services' query requests carry.
const CFN_VERSION: &str = "2010-05-15";
const ASG_VERSION: &str = "2011-01-01";

/// The image and instance type the declared launch data names.
const IMAGE_ID: &str = "ami-0abcdef1234567890";
const INSTANCE_TYPE: &str = "t3.micro";

/// The lifecycle state a group reports an instance of its own capacity in once it is serving
/// ("`InService` — The instance is running and in service", `Instance$LifecycleState`).
const IN_SERVICE: &str = "InService";

/// The logical id the template declares the group under, which is what a signal names it by
/// (`SignalResource$LogicalResourceId`).
const GROUP: &str = "Asg";

/// How often a row LOOKS at the group while the update runs: a tenth of the span the emulator
/// gives a batch's replacements, so the cadence follows the emulator rather than a number
/// written here, and never so fast that a look is a busy wait.
const LOOK_AGAIN_AFTER: Duration = match BATCH_REPLACEMENT.as_secs() / 10 {
    0 => Duration::from_millis(250),
    seconds => Duration::from_secs(seconds),
};

/// How many looks a row must get IN TOTAL per batch, and how many of them must land while the
/// batch holds the group below its capacity, before the counts it observed say anything.
///
/// A third of the looks the cadence above would give an unloaded host, so a host doing other
/// work as well still gets them.
const LOOKS_PER_BATCH: usize = 3;
const DIPPED_LOOKS_PER_BATCH: usize = 2;

/// How long a row gives the whole replacement, and the conditions it polls for. Generous — a
/// row's replacement takes one span per batch — and only here so a failure is a failure rather
/// than a hang.
const DEADLINE: Duration = Duration::from_secs(300);
const POLL: Duration = Duration::from_millis(25);

/// The whole emulator over one engine, exactly as the binary assembles it.
fn router() -> MultiServiceRouter {
    let engine = aws_emulator_router::new_engine();
    let mut router = aws_emulator_router::build_router(engine);
    // Signature verification is the front door's concern, not this suite's.
    router.set_request_authentication(false);
    router
}

/// Call one query operation of `service`, answered with its status and XML body.
async fn query(
    router: &MultiServiceRouter,
    service: &str,
    version: &str,
    action: &str,
    members: &[(&str, &str)],
) -> (u16, String) {
    let mut body = format!("Action={action}&Version={version}");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{service}.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-www-form-urlencoded")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/\
                 aws4_request, SignedHeaders=host, Signature=0000"
            ),
        )
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// One CloudFormation operation that must succeed.
async fn cfn(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "cloudformation", CFN_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "CloudFormation {action} must succeed, got {status} with body {body}"
    );
    body
}

/// One Amazon EC2 Auto Scaling operation that must succeed.
async fn asg(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> String {
    let (status, body) = query(router, "autoscaling", ASG_VERSION, action, members).await;
    assert_eq!(
        status, 200,
        "Auto Scaling {action} must succeed, got {status} with body {body}"
    );
    body
}

/// One row: a group of `desired` instances whose declaration asks for the batch size, the floor
/// and the signal gate named here.
pub(crate) struct Row {
    pub(crate) case: &'static str,
    pub(crate) stack: &'static str,
    /// `DesiredCapacity`: how many instances the group holds, and the size the replacement
    /// leaves it at.
    pub(crate) desired: usize,
    /// `MaxSize`: "the maximum size of the group".
    pub(crate) max_size: usize,
    /// `MaxBatchSize`: "the maximum number of instances that CloudFormation updates".
    pub(crate) max_batch_size: usize,
    /// `MinInstancesInService`: "the minimum number of instances that must be in service
    /// within the Auto Scaling group while CloudFormation updates old instances".
    pub(crate) min_instances_in_service: usize,
    /// `WaitOnResourceSignals`: whether each batch's replacements have to signal before the
    /// next batch starts, which this suite then sends for them as their own bootstrap would.
    pub(crate) wait_on_signals: bool,
    /// How many instances the FIRST batch of this row really replaces: `MaxBatchSize`, capped
    /// by what the group can spare above `MinInstancesInService`. It is the deepest the group
    /// is taken, so it decides the in-service count the dip is held at.
    pub(crate) batch: usize,
    /// How many instances the group holds WHILE the replacement runs: its declared capacity,
    /// or more where the floor leaves it nothing to spare — a group that must keep every
    /// instance it has in service launches the replacement before it terminates anything, up to
    /// `MaxSize`, which is why "this value must be less than the `MaxSize` of the Auto Scaling
    /// group".
    pub(crate) working: usize,
    /// How many batches the whole replacement takes.
    pub(crate) batches: usize,
}

impl Row {
    /// The in-service count the group is held at while a batch of this row is replaced: the
    /// capacity the replacement works at, less that batch.
    fn dipped_to(&self) -> usize {
        self.working.saturating_sub(self.batch)
    }

    /// The template this row deploys, with `generation` deciding the launch template's
    /// `UserData` — part of the launch template's data, so changing it gives the template a new
    /// VERSION and the group's own `LaunchTemplate` property a new version to point at: a
    /// change to the launch configuration, which is what the policy applies to.
    fn template(&self, generation: &str) -> String {
        let Row {
            stack,
            desired,
            max_size,
            max_batch_size,
            min_instances_in_service,
            wait_on_signals,
            ..
        } = self;
        // `PauseTime` is what "CloudFormation should wait for the Auto Scaling group to
        // receive the required number of signals" in, so a signalled row states a span far
        // longer than this suite takes to signal: the gate is released by the signal, never by
        // a span running out.
        let pause_time = if *wait_on_signals { "PT10M" } else { "PT0S" };
        format!(
            r#"{{"Resources":{{
  "Vpc":{{"Type":"AWS::EC2::VPC","Properties":{{"CidrBlock":"10.0.0.0/16"}}}},
  "Subnet":{{"Type":"AWS::EC2::Subnet","Properties":{{
    "VpcId":{{"Ref":"Vpc"}},"CidrBlock":"10.0.0.0/24","AvailabilityZone":"{REGION}a"}}}},
  "Lt":{{"Type":"AWS::EC2::LaunchTemplate","Properties":{{
    "LaunchTemplateName":"{stack}-lt",
    "LaunchTemplateData":{{"ImageId":"{IMAGE_ID}","InstanceType":"{INSTANCE_TYPE}",
      "UserData":{{"Fn::Base64":"generation {generation}"}}}}}}}},
  "{GROUP}":{{"Type":"AWS::AutoScaling::AutoScalingGroup",
    "UpdatePolicy":{{"AutoScalingRollingUpdate":{{
      "MaxBatchSize":{max_batch_size},
      "MinInstancesInService":{min_instances_in_service},
      "MinSuccessfulInstancesPercent":100,
      "PauseTime":"{pause_time}",
      "WaitOnResourceSignals":{wait_on_signals}}}}},
    "Properties":{{
    "MinSize":"1","MaxSize":"{max_size}","DesiredCapacity":"{desired}",
    "LaunchTemplate":{{"LaunchTemplateId":{{"Ref":"Lt"}},
      "Version":{{"Fn::GetAtt":["Lt","LatestVersionNumber"]}}}},
    "VPCZoneIdentifier":[{{"Ref":"Subnet"}}]}}}}}},
  "Outputs":{{"AsgName":{{"Value":{{"Ref":"{GROUP}"}}}}}}}}"#
        )
    }
}

/// Whether a stack status is one the stack settles at, rather than one it passes through.
fn settled(status: &str) -> bool {
    status.ends_with("_COMPLETE") || status.ends_with("_FAILED")
}

/// The stack's status right now.
async fn stack_status(router: &MultiServiceRouter, stack: &str) -> String {
    let body = cfn(router, "DescribeStacks", &[("StackName", stack)]).await;
    element(&body, "StackStatus")
}

/// The stack's FAILED resource events, each as `logical id: status: reason` — what a
/// deployment that did not get where it was going has to say for itself.
async fn failed_events(router: &MultiServiceRouter, stack: &str) -> Vec<String> {
    let body = cfn(router, "DescribeStackEvents", &[("StackName", stack)]).await;
    members_of(&body)
        .into_iter()
        .filter_map(|event| {
            let status = elements(event, "ResourceStatus").into_iter().next()?;
            if !status.contains("FAILED") {
                return None;
            }
            let logical_id = elements(event, "LogicalResourceId").into_iter().next()?;
            let reason = elements(event, "ResourceStatusReason")
                .into_iter()
                .next()
                .unwrap_or_default();
            Some(format!("{logical_id}: {status}: {reason}"))
        })
        .collect()
}

/// The ids of the instances the group reports IN SERVICE right now.
///
/// Each entry is read inside the `<member>` that carries BOTH its id and its lifecycle state,
/// scoped to the group's own `Instances` list, so an id is never paired with another entry's
/// state.
async fn in_service(router: &MultiServiceRouter, group: &str) -> BTreeSet<String> {
    let body = asg(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    let mut serving = BTreeSet::new();
    for entry in block_of(&body, "Instances")
        .map(members_of)
        .unwrap_or_default()
    {
        let state = elements(entry, "LifecycleState").into_iter().next();
        let instance_id = elements(entry, "InstanceId").into_iter().next();
        if let (Some(state), Some(instance_id)) = (state, instance_id) {
            if state == IN_SERVICE {
                serving.insert(instance_id);
            }
        }
    }
    serving
}

/// Poll the group until it reports `wanted` instances in service, answering their ids.
async fn wait_for_capacity(
    router: &MultiServiceRouter,
    case: &str,
    group: &str,
    wanted: usize,
) -> BTreeSet<String> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let serving = in_service(router, group).await;
        if serving.len() >= wanted {
            return serving;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected {group} to report {wanted} {IN_SERVICE} instance(s), got \
             {serving:?}"
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Poll `DescribeStacks` until the stack reports a settled status, answering it.
async fn wait_for_settled(router: &MultiServiceRouter, case: &str, stack: &str) -> String {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let status = stack_status(router, stack).await;
        if settled(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the stack never settled: it is {status:?}, with the failures {:?}",
            failed_events(router, stack).await
        );
        tokio::time::sleep(POLL).await;
    }
}

/// Send one signal to the group, exactly as an instance's own bootstrap sends it: keyed by the
/// instance's id ("when you signal Amazon EC2 instances or Auto Scaling groups, specify the
/// instance ID that you are signaling as the unique ID").
async fn signal(router: &MultiServiceRouter, stack: &str, instance_id: &str) {
    cfn(
        router,
        "SignalResource",
        &[
            ("StackName", stack),
            ("LogicalResourceId", GROUP),
            ("UniqueId", instance_id),
            ("Status", "SUCCESS"),
        ],
    )
    .await;
}

/// The scaling processes the group reports as SUSPENDED right now
/// (`AutoScalingGroup$SuspendedProcesses`), by process name.
///
/// What this is read for is that the group is left able to LAUNCH: a replacement holds the
/// group's `Launch` process back while a batch's instances leave service, and a group that
/// never gets it back never reaches the capacity it is configured for again.
async fn suspended_processes(router: &MultiServiceRouter, group: &str) -> BTreeSet<String> {
    let body = asg(
        router,
        "DescribeAutoScalingGroups",
        &[("AutoScalingGroupNames.member.1", group)],
    )
    .await;
    block_of(&body, "SuspendedProcesses")
        .map(members_of)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|entry| elements(entry, "ProcessName").into_iter().next())
        .collect()
}

/// Deploy one row's stack and answer the group's name and the instances it is serving with.
async fn deployed(router: &MultiServiceRouter, row: &Row) -> (String, BTreeSet<String>) {
    cfn(
        router,
        "CreateStack",
        &[
            ("StackName", row.stack),
            ("TemplateBody", row.template("1").as_str()),
        ],
    )
    .await;
    let status = wait_for_settled(router, row.case, row.stack).await;
    assert_eq!(
        status, "CREATE_COMPLETE",
        "case {:?}: expected the stack to create, with the failures {:?}",
        row.case,
        failed_events(router, row.stack).await
    );
    let body = cfn(router, "DescribeStacks", &[("StackName", row.stack)]).await;
    let group = outputs_of(&body)
        .get("AsgName")
        .unwrap_or_else(|| panic!("case {:?}: the stack publishes its group's name", row.case))
        .clone();
    let serving = wait_for_capacity(router, row.case, &group, row.desired).await;
    (group, serving)
}

/// What a client watching the group through the whole update saw.
struct Watched {
    /// The status the stack settled at.
    settled_at: String,
    /// The in-service count each look reported, in the order they were taken.
    counts: Vec<usize>,
}

impl Watched {
    /// How many looks landed while the group was below its declared capacity.
    fn dipped(&self, desired: usize) -> usize {
        self.counts.iter().filter(|count| **count < desired).count()
    }
}

/// Update one row's stack and WATCH the group until the stack settles: one look per
/// [`LOOK_AGAIN_AFTER`], each recording the group's in-service count, signalling any
/// replacement a signalled row's gate is waiting for exactly as that instance's own bootstrap
/// would.
async fn watched_update(
    router: &MultiServiceRouter,
    row: &Row,
    group: &str,
    originals: &BTreeSet<String>,
) -> Watched {
    cfn(
        router,
        "UpdateStack",
        &[
            ("StackName", row.stack),
            ("TemplateBody", row.template("2").as_str()),
        ],
    )
    .await;
    let deadline = Instant::now() + DEADLINE;
    let mut counts = Vec::new();
    let mut signalled: BTreeSet<String> = BTreeSet::new();
    loop {
        let serving = in_service(router, group).await;
        counts.push(serving.len());
        if row.wait_on_signals {
            for instance_id in serving.difference(originals) {
                if signalled.insert(instance_id.clone()) {
                    signal(router, row.stack, instance_id).await;
                }
            }
        }
        let status = stack_status(router, row.stack).await;
        if settled(&status) {
            return Watched {
                settled_at: status,
                counts,
            };
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: the update never settled: it is {status:?} after the in-service counts \
             {counts:?}, with the failures {:?}",
            row.case,
            failed_events(router, row.stack).await
        );
        tokio::time::sleep(LOOK_AGAIN_AFTER).await;
    }
}

/// Deploy one row, update it while watching the group, and hold the whole replacement to the
/// rule: the dip is held at the capacity the batch leaves, a reader sees it, the floor is never
/// breached, the group is never taken past the capacity it works at, and the update completes
/// with every instance of the earlier configuration replaced.
pub(crate) async fn a_watched_rolling_update(row: &Row) {
    let case = row.case;
    let router = router();
    let (group, originals) = deployed(&router, row).await;
    assert_eq!(
        originals.len(),
        row.desired,
        "case {case:?}: expected the group to serve its desired capacity, got {originals:?}"
    );

    let watched = watched_update(&router, row, &group, &originals).await;
    let counts = &watched.counts;
    assert_eq!(
        watched.settled_at, "UPDATE_COMPLETE",
        "case {case:?}: expected the paced replacement to complete, got {:?} after the \
         in-service counts {counts:?}, with the failures {:?}",
        watched.settled_at,
        failed_events(&router, row.stack).await
    );

    // A reader got to look at the group several times per batch, which is what makes the
    // counts below evidence rather than one glance at a swap.
    let looks = counts.len();
    assert!(
        looks >= LOOKS_PER_BATCH * row.batches,
        "case {case:?}: a client looking every {LOOK_AGAIN_AFTER:?} must get at least \
         {LOOKS_PER_BATCH} look(s) per batch of the {} this replacement takes, got {looks} \
         ({counts:?})",
        row.batches
    );
    let dipped = watched.dipped(row.working);
    assert!(
        dipped >= DIPPED_LOOKS_PER_BATCH * row.batches,
        "case {case:?}: at least {DIPPED_LOOKS_PER_BATCH} look(s) per batch must find the group \
         held below the {} instance(s) it works at, got {dipped} of {looks} ({counts:?})",
        row.working
    );

    // The deepest the group was taken is exactly the batch the policy allows: a lower count
    // means a batch larger than `MaxBatchSize`, and the capacity it works at means nothing was
    // ever taken out of service — the replacements were launched before the instances they
    // replace left, which this policy does not do.
    let floor = counts.iter().copied().min().unwrap_or(row.working);
    assert_eq!(
        floor,
        row.dipped_to(),
        "case {case:?}: expected the replacement to hold the group at {} in-service \
         instance(s) — the {} it works at less the batch of {} — got {floor} ({counts:?})",
        row.dipped_to(),
        row.working,
        row.batch
    );
    assert!(
        floor >= row.min_instances_in_service,
        "case {case:?}: the replacement must keep at least {} instance(s) in service, got \
         {floor} ({counts:?})",
        row.min_instances_in_service
    );
    // ...and it never took the group PAST the capacity it works at: this policy replaces in
    // place, and borrows capacity only where the floor leaves it nothing to spare — which its
    // `MaxSize` is the ceiling on.
    let ceiling = counts.iter().copied().max().unwrap_or_default();
    assert!(
        ceiling <= row.working && row.working <= row.max_size,
        "case {case:?}: the replacement must not take the group past the {} instance(s) it \
         works at (its MaxSize is {}), got {ceiling} ({counts:?})",
        row.working,
        row.max_size
    );

    // The replacement really happened: the group is back to its capacity, serving none of the
    // instances it started with.
    let serving = wait_for_capacity(&router, case, &group, row.desired).await;
    assert_eq!(
        serving.len(),
        row.desired,
        "case {case:?}: the group must be back to its desired capacity, got {serving:?}"
    );
    assert!(
        serving.is_disjoint(&originals),
        "case {case:?}: the replacement must leave none of the original instances {originals:?} \
         in the group, which is serving {serving:?}"
    );
    // ...and the group is left able to LAUNCH: what paced the batches is a hold on the process
    // that launches, and a hold the replacement did not lift would leave the group unable to
    // reach the capacity it is configured for ever again.
    let suspended = suspended_processes(&router, &group).await;
    assert!(
        suspended.is_empty(),
        "case {case:?}: the finished replacement must leave the group's scaling processes \
         running, got {suspended:?} suspended"
    );
}

#[tokio::test]
async fn a_single_batch_rolling_update_holds_the_group_out_of_service_while_it_replaces() {
    a_watched_rolling_update(&Row {
        case: "one instance replaced in one batch, nothing having to stay in service",
        stack: "rolling-pacing-one-by-one",
        desired: 1,
        max_size: 2,
        max_batch_size: 1,
        min_instances_in_service: 0,
        wait_on_signals: false,
        batch: 1,
        // The group can spare its one instance, so it replaces in place and never holds more
        // than it was declared with.
        working: 1,
        batches: 1,
    })
    .await;
}

#[tokio::test]
async fn a_rolling_update_holds_each_batchs_dip_at_the_floor_it_declares() {
    a_watched_rolling_update(&Row {
        case: "two instances replaced one at a time, one of them always in service",
        stack: "rolling-pacing-two-by-one",
        desired: 2,
        max_size: 3,
        max_batch_size: 1,
        min_instances_in_service: 1,
        wait_on_signals: false,
        batch: 1,
        working: 2,
        batches: 2,
    })
    .await;
}

#[tokio::test]
async fn a_signal_gated_rolling_update_is_paced_batch_by_batch_as_well() {
    a_watched_rolling_update(&Row {
        case: "three instances replaced two at a time, each batch gated on its own signals",
        stack: "rolling-pacing-three-by-two",
        desired: 3,
        max_size: 4,
        max_batch_size: 2,
        min_instances_in_service: 1,
        wait_on_signals: true,
        // The group can spare two above its floor, so the first batch takes two and the last
        // instance is the second batch's.
        batch: 2,
        working: 3,
        batches: 2,
    })
    .await;
}

/// The row the CANCELLED replacement is driven on: one instance replaced at a time, so the
/// cancellation lands while a batch is being held.
const CANCELLED: Row = Row {
    case: "a replacement cancelled while a batch is holding the group below its capacity",
    stack: "rolling-pacing-cancelled",
    desired: 2,
    max_size: 3,
    max_batch_size: 1,
    min_instances_in_service: 1,
    wait_on_signals: false,
    batch: 1,
    working: 2,
    batches: 2,
};

#[tokio::test]
async fn a_replacement_cancelled_mid_batch_leaves_the_group_able_to_launch_again() {
    let row = &CANCELLED;
    let case = row.case;
    let router = router();
    let (group, originals) = deployed(&router, row).await;
    cfn(
        &router,
        "UpdateStack",
        &[
            ("StackName", row.stack),
            ("TemplateBody", row.template("2").as_str()),
        ],
    )
    .await;

    // Wait for a batch to be IN the hold: the group is below the capacity it is configured
    // for, which is where the replacement is holding its launches back.
    let deadline = Instant::now() + DEADLINE;
    loop {
        let serving = in_service(&router, &group).await;
        if serving.len() < row.desired {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: expected a batch to take the group below its capacity of {}, got \
             {serving:?} with the stack at {:?}",
            row.desired,
            stack_status(&router, row.stack).await
        );
        tokio::time::sleep(POLL).await;
    }

    // "CancelUpdateStack cancels an update on the specified stack": the operation that was
    // holding the group's launches back is over, so what it was holding back is owed to the
    // group whatever became of that operation.
    cfn(
        &router,
        "CancelUpdateStack",
        &[("StackName", row.stack)],
    )
    .await;
    let status = wait_for_settled(&router, case, row.stack).await;
    assert_eq!(
        status, "UPDATE_ROLLBACK_COMPLETE",
        "case {case:?}: expected the cancelled update to roll back, got {status:?} with the \
         failures {:?}",
        failed_events(&router, row.stack).await
    );
    let suspended = suspended_processes(&router, &group).await;
    assert!(
        suspended.is_empty(),
        "case {case:?}: the cancelled replacement must leave the group's scaling processes \
         running, got {suspended:?} suspended"
    );
    // ...which is what lets the group reach the capacity it is configured for again, the batch
    // that was in flight replaced or not.
    let serving = wait_for_capacity(&router, case, &group, row.desired).await;
    assert_eq!(
        serving.len(),
        row.desired,
        "case {case:?}: the group must reach its desired capacity again after the cancellation, \
         got {serving:?} against the original {originals:?}"
    );
}
