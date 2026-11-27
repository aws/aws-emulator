// SPDX-License-Identifier: CC0-1.0

//! The PROGRESS one StackSet operation reports — its stages, the targets it resolved and how
//! far each Region has got — end to end over the awsQuery wire.
//!
//! RULE: a StackSet operation reports its progress as the two STAGES it runs through, beside
//! the totals it resolved: `TotalAccountsCount` and `TotalStackInstancesCount` are populated
//! "after it resolves the target accounts / the target stack instances. Until then, this field
//! is not present", the role-provisioning stage "is present with a `Status` of
//! `NOT_APPLICABLE`" for an operation that provisions no role, and the stack-instances
//! deployment stage reports one count per outcome over the targets the operation acted on,
//! with the percentage that have reached a final state. Every count is taken from those
//! targets, so this read, `ListStackSetOperationResults` and the instances themselves cannot
//! disagree. `RegionalStatusDetails` is the same deployment progress per Region and "is
//! returned only by `DescribeStackSetOperation`. It is not included in the
//! `ListStackSetOperations` summary".
//!
//! The rows differ along what the rule generalises over: how far the operation has got (no
//! target started, some started, every target finished), what became of each target
//! (succeeded, failed, canceled), how many accounts and Regions it spans, and which ACTION it
//! is — a drift detection deploys no stack instance, which is the documented
//! `NOT_APPLICABLE` case for that stage too.

use std::time::{Duration, Instant, SystemTime};

use service_com_amazonaws_cloudformation_cloudformation::state::stack_set_progress::stage_status;
use service_com_amazonaws_cloudformation_cloudformation::state::stack_sets::{
    detailed_status, operation_action, operation_status, StoredStackInstance, StoredStackSet,
    StoredStackSetOperation,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::stack_lifecycle::{
    call, member, router, router_with_engine, store, xml_blocks, xml_text, REGION,
};

/// Target accounts beside the emulator's own (`Account` is `@pattern("^[0-9]{12}$")`).
const ACCOUNT_A: &str = "111111111111";
const ACCOUNT_B: &str = "222222222222";

/// A second target Region, so the per-Region detail has more than one entry to report.
const WEST: &str = "us-west-2";

/// The template the deployed StackSet carries: one topic per instance.
const TEMPLATE: &str = r#"{"Resources":{"Topic":{"Type":"AWS::SNS::Topic"}}}"#;

/// How long a case gives a deployment to reach its terminal status: far longer than anything a
/// row asks for, so a loaded machine cannot fail one, and the condition — not the clock — is
/// what ends the wait.
const SETTLED_DEADLINE: Duration = Duration::from_secs(120);

/// How often the loop below looks again.
const POLL_EVERY: Duration = Duration::from_millis(50);

/// `DescribeStackSetOperation` for one operation.
async fn describe_operation(
    router: &MultiServiceRouter,
    stack_set: &str,
    operation_id: &str,
) -> (u16, String) {
    call(
        router,
        "DescribeStackSetOperation",
        &[
            member("StackSetName", stack_set),
            member("OperationId", operation_id),
        ],
    )
    .await
}

/// The `DescribeStackSetOperation` body of `operation_id` once its `Status` is terminal,
/// looked for again until it is.
async fn settled_operation(
    case: &str,
    router: &MultiServiceRouter,
    stack_set: &str,
    operation_id: &str,
) -> String {
    let deadline = Instant::now() + SETTLED_DEADLINE;
    let mut last = String::new();
    while Instant::now() < deadline {
        let (status, body) = describe_operation(router, stack_set, operation_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStackSetOperation to answer 200, got {status} with \
             {body}"
        );
        if !operation_status::is_in_flight(&xml_text(&body, "Status").unwrap_or_default()) {
            return body;
        }
        last = body;
        tokio::time::sleep(POLL_EVERY).await;
    }
    panic!("case {case:?}: the operation never left its in-flight status: {last}")
}

/// The one `<Stages>` element of a response.
fn stages(case: &str, body: &str) -> String {
    xml_blocks(body, "Stages")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected a Stages element, got {body}"))
}

/// One named element inside a block.
fn block(case: &str, inside: &str, tag: &str) -> String {
    xml_blocks(inside, tag)
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {tag} element, got {inside}"))
}

/// The counts and status one deployment stage reports, as the pairs a row states them in.
fn reported(block: &str) -> Vec<(&'static str, Option<String>)> {
    [
        "Status",
        "SucceededStackInstancesCount",
        "FailedStackInstancesCount",
        "CompletedStackInstancesCount",
        "PendingStackInstancesCount",
        "CancelledStackInstancesCount",
        "SkippedStackInstancesCount",
        "PercentageCompleted",
    ]
    .into_iter()
    .map(|tag| (tag, xml_text(block, tag)))
    .collect()
}

/// What a stage must report: its status, the per-outcome counts in the order
/// [`reported`] reads them, and the completion percentage.
fn expected(
    status: &str,
    succeeded: u32,
    failed: u32,
    completed: u32,
    pending: u32,
    cancelled: u32,
    percentage: &str,
) -> Vec<(&'static str, Option<String>)> {
    vec![
        ("Status", Some(status.to_string())),
        (
            "SucceededStackInstancesCount",
            Some(succeeded.to_string()),
        ),
        ("FailedStackInstancesCount", Some(failed.to_string())),
        (
            "CompletedStackInstancesCount",
            Some(completed.to_string()),
        ),
        ("PendingStackInstancesCount", Some(pending.to_string())),
        (
            "CancelledStackInstancesCount",
            Some(cancelled.to_string()),
        ),
        // Nothing here provisions a role in a target account, so no target is skipped for a
        // role provisioning that failed: the count is zero rather than absent.
        ("SkippedStackInstancesCount", Some("0".to_string())),
        ("PercentageCompleted", Some(percentage.to_string())),
    ]
}

// ---------------------------------------------------------------------------
// 1. A deployment driven through the service's own writers
// ---------------------------------------------------------------------------

/// RULE: once a deployment has resolved its targets it reports how many accounts and stack
/// instances it is for, the role-provisioning stage as not applicable, the deployment stage
/// over those targets, and the same deployment progress per Region — while the listing reports
/// the stages and leaves the per-Region detail out.
#[tokio::test]
async fn a_concluded_deployment_reports_its_stages_and_every_region() {
    const STACK_SET: &str = "fleet-progress";
    let case = "a deployment over two accounts in two Regions";

    let router = router();
    let (status, body) = call(
        &router,
        "CreateStackSet",
        &[
            member("StackSetName", STACK_SET),
            member("TemplateBody", TEMPLATE),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStackSet to answer 200, got {status} with {body}"
    );

    // Two accounts crossed with two Regions: four stack instances, as ONE operation.
    let (status, body) = call(
        &router,
        "CreateStackInstances",
        &[
            member("StackSetName", STACK_SET),
            member("Accounts.member.1", ACCOUNT_A),
            member("Accounts.member.2", ACCOUNT_B),
            member("Regions.member.1", REGION),
            member("Regions.member.2", WEST),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateStackInstances to answer 200, got {status} with {body}"
    );
    let operation_id = xml_text(&body, "OperationId")
        .unwrap_or_else(|| panic!("case {case:?}: expected an OperationId, got {body}"));

    let settled = settled_operation(case, &router, STACK_SET, &operation_id).await;
    for (tag, value) in [
        ("TotalAccountsCount", "2"),
        ("TotalStackInstancesCount", "4"),
        ("FailedStackInstancesCount", "0"),
    ] {
        let details = block(case, &settled, "StatusDetails");
        // Read off the details with the stages removed, so the operation-wide count is the one
        // asserted rather than the stage's own element of the same name.
        let details = details.replace(&stages(case, &settled), "");
        assert_eq!(
            xml_text(&details, tag).as_deref(),
            Some(value),
            "case {case:?}: expected {tag} {value}, got {settled}"
        );
    }
    let staged = stages(case, &settled);
    let role_provisioning = block(case, &staged, "RoleProvisioning");
    assert_eq!(
        xml_text(&role_provisioning, "Status").as_deref(),
        Some(stage_status::NOT_APPLICABLE),
        "case {case:?}: expected the role-provisioning stage to be reported as not \
         applicable, got {role_provisioning}"
    );
    for omitted in [
        "SucceededAccountsCount",
        "FailedAccountsCount",
        "CompletedAccountsCount",
        "PendingAccountsCount",
        "PercentageCompleted",
    ] {
        assert_eq!(
            xml_text(&role_provisioning, omitted),
            None,
            "case {case:?}: expected a stage that does not apply to omit {omitted}, got \
             {role_provisioning}"
        );
    }
    let deployment = block(case, &staged, "StackInstancesDeployment");
    assert_eq!(
        reported(&deployment),
        expected(stage_status::SUCCEEDED, 4, 0, 4, 0, 0, "100.0"),
        "case {case:?}: expected the deployment stage to report every target succeeded, got \
         {deployment}"
    );

    // One entry per Region, each counting only that Region's targets.
    let regional = block(case, &settled, "RegionalStatusDetails");
    let entries = xml_blocks(&regional, "entry");
    assert_eq!(
        entries.len(),
        2,
        "case {case:?}: expected one entry per Region of the operation, got {regional}"
    );
    for region in [REGION, WEST] {
        let entry = entries
            .iter()
            .find(|entry| xml_text(entry, "key").as_deref() == Some(region))
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected an entry for {region}, got {regional}")
            });
        let of_region = block(case, entry, "RegionalStackInstancesDeployment");
        assert_eq!(
            reported(&of_region),
            expected(stage_status::SUCCEEDED, 2, 0, 2, 0, 0, "100.0"),
            "case {case:?} ({region}): expected that Region's own counts, got {of_region}"
        );
        assert_eq!(
            xml_text(&of_region, "TotalStackInstancesCount").as_deref(),
            Some("2"),
            "case {case:?} ({region}): expected that Region's own target total, got {of_region}"
        );
    }

    // The listing reports the same stages — and no per-Region detail, which that read owns.
    let (status, listed) = call(
        &router,
        "ListStackSetOperations",
        &[member("StackSetName", STACK_SET)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListStackSetOperations to answer 200, got {status} with \
         {listed}"
    );
    let listed_stage = block(case, &stages(case, &listed), "StackInstancesDeployment");
    assert_eq!(
        reported(&listed_stage),
        expected(stage_status::SUCCEEDED, 4, 0, 4, 0, 0, "100.0"),
        "case {case:?}: expected the listing to report the same deployment progress, got \
         {listed}"
    );
    assert!(
        xml_blocks(&listed, "RegionalStatusDetails").is_empty(),
        "case {case:?}: expected the summary to carry no per-Region detail, got {listed}"
    );
}

// ---------------------------------------------------------------------------
// 2. How far the operation has got, over every mix of outcomes
// ---------------------------------------------------------------------------

/// One row of the progress table: the targets an operation holds, the action it is, and the
/// deployment stage it must therefore report.
struct Progress {
    case: &'static str,
    action: &'static str,
    /// Whether the operation itself has concluded.
    concluded: bool,
    /// One `StackInstanceStatus$DetailedStatus` per target the operation acted on.
    targets: &'static [&'static str],
    status: &'static str,
    succeeded: u32,
    failed: u32,
    completed: u32,
    pending: u32,
    cancelled: u32,
    percentage: &'static str,
}

/// The rows: an operation no target has started, one part way through, and every mix of
/// outcomes a concluded one can hold — plus the action that deploys no stack instance at all.
const PROGRESS: &[Progress] = &[
    Progress {
        case: "no target has started",
        action: operation_action::CREATE,
        concluded: false,
        targets: &[detailed_status::PENDING, detailed_status::PENDING],
        status: stage_status::NOT_STARTED,
        succeeded: 0,
        failed: 0,
        completed: 0,
        pending: 2,
        cancelled: 0,
        percentage: "0.0",
    },
    Progress {
        case: "one target finished and one still running",
        action: operation_action::UPDATE,
        concluded: false,
        targets: &[detailed_status::SUCCEEDED, detailed_status::RUNNING],
        status: stage_status::IN_PROGRESS,
        succeeded: 1,
        failed: 0,
        completed: 1,
        pending: 1,
        cancelled: 0,
        percentage: "50.0",
    },
    Progress {
        case: "every target succeeded",
        action: operation_action::CREATE,
        concluded: true,
        targets: &[detailed_status::SUCCEEDED, detailed_status::SUCCEEDED],
        status: stage_status::SUCCEEDED,
        succeeded: 2,
        failed: 0,
        completed: 2,
        pending: 0,
        cancelled: 0,
        percentage: "100.0",
    },
    Progress {
        case: "a mix of succeeded and failed targets",
        action: operation_action::CREATE,
        concluded: true,
        targets: &[detailed_status::SUCCEEDED, detailed_status::FAILED],
        status: stage_status::PARTIAL_SUCCESS,
        succeeded: 1,
        failed: 1,
        completed: 2,
        pending: 0,
        cancelled: 0,
        percentage: "100.0",
    },
    Progress {
        case: "a target canceled beside one that succeeded",
        action: operation_action::UPDATE,
        concluded: true,
        targets: &[detailed_status::SUCCEEDED, detailed_status::CANCELLED],
        status: stage_status::PARTIAL_SUCCESS,
        succeeded: 1,
        failed: 0,
        completed: 2,
        pending: 0,
        cancelled: 1,
        percentage: "100.0",
    },
    Progress {
        case: "every target failed",
        action: operation_action::DELETE,
        concluded: true,
        targets: &[detailed_status::FAILED, detailed_status::FAILED],
        status: stage_status::FAILED,
        succeeded: 0,
        failed: 2,
        completed: 2,
        pending: 0,
        cancelled: 0,
        percentage: "100.0",
    },
    Progress {
        case: "an operation that deploys no stack instance",
        action: operation_action::DETECT_DRIFT,
        concluded: true,
        targets: &[detailed_status::SUCCEEDED, detailed_status::SUCCEEDED],
        status: stage_status::NOT_APPLICABLE,
        succeeded: 0,
        failed: 0,
        completed: 0,
        pending: 0,
        cancelled: 0,
        percentage: "",
    },
];

/// An instant far enough ahead that an operation seeded as still running is still running when
/// a case reads it.
fn not_due_yet() -> SystemTime {
    SystemTime::now() + Duration::from_secs(3_600)
}

/// RULE: the deployment stage reports what the operation's targets have reached — how many
/// succeeded, failed, were canceled and have yet to finish, the completion percentage over
/// them, and the status that mix IS — and an operation that deploys no stack instance reports
/// the stage as not applicable, with no counts in it.
#[tokio::test]
async fn the_deployment_stage_reports_what_its_targets_reached() {
    for (index, row) in PROGRESS.iter().enumerate() {
        let case = row.case;
        let stack_set = format!("fleet-progress-{index}");
        let operation_id = format!("operation-progress-{index}");

        let (router, engine) = router_with_engine();
        {
            let state = store(&engine);
            let mut held = state.lock();
            let mut seeded = StoredStackSet::new(&stack_set, REGION);
            let mut operation = StoredStackSetOperation::running(
                &operation_id,
                row.action,
                SystemTime::now(),
            );
            if row.concluded {
                operation.pending = None;
                operation.status = operation_status::SUCCEEDED.to_string();
                operation.failed_stack_instances_count = Some(row.failed as i32);
            } else if let Some(pending) = operation.pending.as_mut() {
                pending.due_at = not_due_yet();
            }
            seeded.record_operation(operation);
            for (position, detailed) in row.targets.iter().enumerate() {
                let account = format!("{:012}", position + 1);
                let mut instance =
                    StoredStackInstance::new(&account, REGION, &operation_id);
                instance.detailed_status = (*detailed).to_string();
                seeded.record_instance(instance);
            }
            held.record_stack_set(seeded);
        }

        let (status, body) = describe_operation(&router, &stack_set, &operation_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeStackSetOperation to answer 200, got {status} \
             with {body}"
        );
        let deployment = block(case, &stages(case, &body), "StackInstancesDeployment");
        if row.status == stage_status::NOT_APPLICABLE {
            assert_eq!(
                xml_text(&deployment, "Status").as_deref(),
                Some(stage_status::NOT_APPLICABLE),
                "case {case:?}: expected the stage to be reported as not applicable, got \
                 {deployment}"
            );
            for omitted in [
                "SucceededStackInstancesCount",
                "FailedStackInstancesCount",
                "CompletedStackInstancesCount",
                "PendingStackInstancesCount",
                "PercentageCompleted",
            ] {
                assert_eq!(
                    xml_text(&deployment, omitted),
                    None,
                    "case {case:?}: expected a stage that does not apply to omit {omitted}, \
                     got {deployment}"
                );
            }
            continue;
        }
        assert_eq!(
            reported(&deployment),
            expected(
                row.status,
                row.succeeded,
                row.failed,
                row.completed,
                row.pending,
                row.cancelled,
                row.percentage,
            ),
            "case {case:?}: expected the stage to report what its targets reached, got \
             {deployment}"
        );
    }
}

/// RULE: the totals and the stages are reported once the operation has RESOLVED its targets —
/// "CloudFormation populates this value after it resolves the target accounts / the target
/// stack instances. Until then, this field is not present" — so an operation holding no target
/// reports neither total, no stage detail and no per-Region detail, while the count its
/// conclusion recorded is reported all the same.
#[tokio::test]
async fn an_operation_with_no_resolved_target_reports_no_progress() {
    const STACK_SET: &str = "fleet-progress-unresolved";
    const OPERATION: &str = "operation-progress-unresolved";
    let case = "an operation that has resolved no target";

    let (router, engine) = router_with_engine();
    {
        let state = store(&engine);
        let mut held = state.lock();
        let mut seeded = StoredStackSet::new(STACK_SET, REGION);
        let mut operation =
            StoredStackSetOperation::running(OPERATION, operation_action::CREATE, SystemTime::now());
        operation.pending = None;
        operation.status = operation_status::SUCCEEDED.to_string();
        operation.failed_stack_instances_count = Some(0);
        seeded.record_operation(operation);
        held.record_stack_set(seeded);
    }

    let (status, body) = describe_operation(&router, STACK_SET, OPERATION).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeStackSetOperation to answer 200, got {status} with \
         {body}"
    );
    for absent in [
        "Stages",
        "TotalAccountsCount",
        "TotalStackInstancesCount",
        "RegionalStatusDetails",
    ] {
        assert!(
            xml_blocks(&body, absent).is_empty(),
            "case {case:?}: expected {absent} to be absent, got {body}"
        );
    }
    assert_eq!(
        xml_text(&body, "FailedStackInstancesCount").as_deref(),
        Some("0"),
        "case {case:?}: expected the count the conclusion recorded, got {body}"
    );
}
