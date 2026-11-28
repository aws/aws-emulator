// SPDX-License-Identifier: CC0-1.0

//! WHERE a schedule's execution role is resolved: end to end over the wire through the
//! real `MultiServiceRouter`, with Amazon SQS and IAM mounted beside EventBridge
//! Scheduler on ONE engine, so the role is resolved against the very records `CreateRole`
//! and `DeleteRole` wrote.
//!
//! RULE: `Target.RoleArn` is "the IAM role that EventBridge Scheduler will use for this
//! target when the schedule is invoked", so the reference is resolved WHERE IT IS TAKEN
//! UP — when the schedule is CREATED, which is refused with 400 `ValidationException` and
//! the fixed sentence [`EXECUTION_ROLE_REFUSAL`] unless this account holds the role and
//! its trust policy admits the scheduler service principal, and again at each occurrence.
//! An UPDATE does not resolve it: it states the schedule's whole configuration and is
//! applied with the role it carries, whether or not that role can be assumed, which is
//! what leaves a schedule whose role has gone the only recovery a client has — re-pointing
//! it at a live one.
//!
//! The rows span the class rather than one recorded case: they differ along WHY the role
//! an update states cannot be assumed — this account holds no such role, holds none under
//! the path the ARN names, the role is another account's (which this account's Scheduler
//! cannot assume at all), the role exists here but trusts a different service, and the
//! role the schedule was created with was DELETED afterwards — and the last row is the
//! control that an assumable role is applied by the same path.
//!
//! What the CALLER must be allowed in order to hand over a role at all (`iam:PassRole`),
//! and the create's own refusal in full, are `tests/all/pass_role_authorization.rs`.

use serde_json::{json, Value};
use service_com_amazonaws_scheduler_awschronosservice::state::{
    EXECUTION_ROLE_REFUSAL, SCHEDULER_SERVICE_PRINCIPAL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::schedule_invocation::{
    at_expression_body, emulator, given_execution_role_for, given_queue, iam, schedule_read,
    scheduler, seconds_from_now,
};
use crate::wire_harness::{account_id, error_code, error_message, xml_text};

/// The group a schedule lands in when the write names none.
const DEFAULT_GROUP: &str = "default";

/// The error code every write of this service declares for an input it does not admit
/// (`smithy_query(query="operation", operation="UpdateSchedule")`: `ValidationException`,
/// `@httpError 400`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// Another account's id, used for the row whose role is not this account's: twelve digits,
/// as `Target$RoleArn`'s `@pattern` requires (`::\d{12}:role\/`), and not this emulator's.
const OTHER_ACCOUNT: &str = "210987654321";

/// A service principal that is NOT the scheduler's, for the row whose role exists here and
/// trusts something else.
const OTHER_PRINCIPAL: &str = "events.amazonaws.com";

/// The name of the inline policy `given_execution_role_for` writes on a role, which has to
/// be removed before IAM will delete the role ("you must remove the policy ... before you
/// can delete the role").
const ROLE_POLICY_NAME: &str = "invoke-the-target";

/// The code IAM answers a reader of a role it does not hold with, which is how the
/// deleted-role row confirms the role is really gone rather than merely propagating.
const NO_SUCH_ENTITY: &str = "NoSuchEntity";

/// The description every row's schedule is CREATED with, and the one a served update has
/// to replace.
const ORIGINAL_DESCRIPTION: &str = "original-desc";

/// How far ahead the schedules below state their one occurrence: far enough that no
/// occurrence of them falls due while the suite is writing and reading them, because what
/// is asserted here is the WRITE and not a delivery.
const OCCURRENCE_IN: u64 = 3_600;

/// WHERE the role an update states comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Role {
    /// An ARN of this account naming a role nothing created.
    Absent,
    /// An ARN of this account naming a role under a PATH, which this account holds no
    /// role under either.
    AbsentUnderAPath,
    /// A well-formed role ARN of another account.
    OtherAccount,
    /// A role this account holds whose trust policy names another service.
    TrustsAnotherService,
    /// The role the schedule was CREATED with, deleted from IAM afterwards: the dangling
    /// reference a client has to be able to update its way off.
    DeletedAfterCreate,
    /// A role this account holds that trusts the scheduler.
    Assumable,
}

/// One row: the role the update states, and whether EventBridge Scheduler could assume
/// it — which decides nothing about the update and everything about the create.
struct Row {
    case: &'static str,
    role: Role,
    assumable: bool,
}

const ROWS: &[Row] = &[
    Row {
        case: "a role this account does not hold",
        role: Role::Absent,
        assumable: false,
    },
    Row {
        case: "a role under a path this account holds nothing under",
        role: Role::AbsentUnderAPath,
        assumable: false,
    },
    Row {
        case: "another account's role",
        role: Role::OtherAccount,
        assumable: false,
    },
    Row {
        case: "a role of this account that trusts another service",
        role: Role::TrustsAnotherService,
        assumable: false,
    },
    Row {
        case: "the role the schedule was created with, since deleted",
        role: Role::DeletedAfterCreate,
        assumable: false,
    },
    Row {
        case: "a role of this account the scheduler may assume",
        role: Role::Assumable,
        assumable: true,
    },
];

/// Every row: the update is served and APPLIED — the schedule reports the description and
/// state it stated and the very role it carried — whether or not that role can be assumed,
/// and the schedule stays ordinarily updatable afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_update_applies_the_role_it_states_without_resolving_it() {
    let emulator = emulator();
    let (_, target_arn) = given_queue(&emulator, "the suite", "update-role-target").await;
    // The role every row's schedule is CREATED with, except the row whose own role is
    // deleted under it, which needs one of its own to delete.
    let original =
        given_execution_role_for(&emulator, "the suite", "original-role", &[&target_arn]).await;
    // The live role a schedule left on a dangling one is re-pointed at, which is the
    // recovery an update that is refused forecloses.
    let live = given_execution_role_for(&emulator, "the suite", "repointed-role", &[&target_arn])
        .await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("role-update-{index}");
        let created_with = match row.role {
            Role::DeletedAfterCreate => {
                given_execution_role_for(
                    &emulator,
                    case,
                    &format!("deleted-after-create-{index}"),
                    &[&target_arn],
                )
                .await
            }
            _ => original.clone(),
        };
        let (status, created) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/{name}"),
            Some(schedule_body(
                &target_arn,
                &created_with,
                ORIGINAL_DESCRIPTION,
                "DISABLED",
            )),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSchedule answered {status} {created}"
        );

        let stated = match row.role {
            Role::DeletedAfterCreate => {
                let role_name = format!("deleted-after-create-{index}");
                given_deleted_role(&emulator, case, &role_name).await;
                created_with.clone()
            }
            other => role_of(&emulator, case, other, index, &target_arn).await,
        };
        let description = format!("updated-desc-{index}");
        let (status, answered) = scheduler(
            &emulator,
            "PUT",
            &format!("/schedules/{name}"),
            Some(schedule_body(&target_arn, &stated, &description, "ENABLED")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the update to be served, got {status} {answered}"
        );

        // What the update APPLIED is what a read reports: the role it stated, dangling or
        // not, beside the rest of the configuration it carried.
        let (status, read) = schedule_read(&emulator, DEFAULT_GROUP, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetSchedule answered {status} {read}"
        );
        assert_eq!(
            read["Target"]["RoleArn"].as_str(),
            Some(stated.as_str()),
            "case {case:?}: expected the stored role {stated:?}, got {read}"
        );
        assert_eq!(
            read["Description"].as_str(),
            Some(description.as_str()),
            "case {case:?}: expected the stored description {description:?}, got {read}"
        );
        assert_eq!(
            read["State"].as_str(),
            Some("ENABLED"),
            "case {case:?}: expected the update's state to be applied, got {read}"
        );

        if row.assumable {
            continue;
        }
        // A schedule left on a role that cannot be assumed is still ordinarily
        // updatable: the update that re-points it at a live role is served too.
        let repointed = format!("repointed-desc-{index}");
        let (status, answered) = scheduler(
            &emulator,
            "PUT",
            &format!("/schedules/{name}"),
            Some(schedule_body(&target_arn, &live, &repointed, "ENABLED")),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the re-pointing update to be served, got {status} \
             {answered}"
        );
        let (status, read) = schedule_read(&emulator, DEFAULT_GROUP, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetSchedule answered {status} {read}"
        );
        assert_eq!(
            (
                read["Target"]["RoleArn"].as_str(),
                read["Description"].as_str()
            ),
            (Some(live.as_str()), Some(repointed.as_str())),
            "case {case:?}: expected the schedule to be re-pointed at {live:?}, got {read}"
        );
    }
}

/// The other half of the same rule: the CREATE does resolve the role, so each role an
/// update applies untouched is refused by a create with 400 `ValidationException` and the
/// fixed sentence. Without this the update's acceptance would say nothing — it is the
/// asymmetry that is the behaviour.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_create_resolves_the_role_an_update_leaves_alone() {
    let emulator = emulator();
    let (_, target_arn) = given_queue(&emulator, "the suite", "create-role-target").await;

    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let stated = match row.role {
            Role::DeletedAfterCreate => {
                let role_name = format!("created-then-deleted-{index}");
                let arn =
                    given_execution_role_for(&emulator, case, &role_name, &[&target_arn]).await;
                given_deleted_role(&emulator, case, &role_name).await;
                arn
            }
            other => role_of(&emulator, case, other, index, &target_arn).await,
        };
        let (status, answered) = scheduler(
            &emulator,
            "POST",
            &format!("/schedules/role-create-{index}"),
            Some(schedule_body(
                &target_arn,
                &stated,
                ORIGINAL_DESCRIPTION,
                "ENABLED",
            )),
        )
        .await;
        if row.assumable {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the create to be served, got {status} {answered}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, VALIDATION_EXCEPTION),
            "case {case:?}: expected the modelled refusal, got {status} {answered}"
        );
        assert_eq!(
            error_message(&answered),
            EXECUTION_ROLE_REFUSAL,
            "case {case:?}: expected the execution-role sentence, got {answered}"
        );
    }
}

/// The role ARN one row's update states, created in IAM where the row needs a role to
/// exist.
async fn role_of(
    router: &MultiServiceRouter,
    case: &str,
    role: Role,
    index: usize,
    target_arn: &str,
) -> String {
    match role {
        Role::Absent => format!("arn:aws:iam::{}:role/never-created-{index}", account_id()),
        Role::AbsentUnderAPath => format!(
            "arn:aws:iam::{}:role/service-role/never-created-{index}",
            account_id()
        ),
        Role::OtherAccount => format!("arn:aws:iam::{OTHER_ACCOUNT}:role/elsewhere-{index}"),
        Role::TrustsAnotherService => {
            let name = format!("other-principal-{index}");
            let trust = json!({
                "Version": "2012-10-17",
                "Statement": [{
                    "Effect": "Allow",
                    "Principal": { "Service": OTHER_PRINCIPAL },
                    "Action": "sts:AssumeRole",
                }],
            })
            .to_string();
            let (status, created) = iam(
                router,
                "CreateRole",
                &[("RoleName", &name), ("AssumeRolePolicyDocument", &trust)],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: CreateRole answered {status}: {created}"
            );
            // The role exists, so what a create is refused by is the trust document alone
            // — which is why this row's role is not the scheduler's principal.
            assert_ne!(
                OTHER_PRINCIPAL, SCHEDULER_SERVICE_PRINCIPAL,
                "case {case:?}: the row's principal must not be the scheduler's"
            );
            format!("arn:aws:iam::{}:role/{name}", account_id())
        }
        // The row whose role is deleted under the schedule holds its own role, so it is
        // not built here.
        Role::DeletedAfterCreate | Role::Assumable => {
            given_execution_role_for(router, case, &format!("assumable-{index}"), &[target_arn])
                .await
        }
    }
}

/// Delete the role called `name` from IAM — its inline policy first, which IAM requires —
/// and confirm it is gone the way a client confirms it: `GetRole` answers `NoSuchEntity`.
async fn given_deleted_role(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, removed) = iam(
        router,
        "DeleteRolePolicy",
        &[("RoleName", name), ("PolicyName", ROLE_POLICY_NAME)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteRolePolicy answered {status}: {removed}"
    );
    let (status, deleted) = iam(router, "DeleteRole", &[("RoleName", name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DeleteRole answered {status}: {deleted}"
    );
    let (status, read) = iam(router, "GetRole", &[("RoleName", name)]).await;
    assert_ne!(
        status, 200,
        "case {case:?}: expected IAM to hold no role {name:?}, got {read}"
    );
    assert_eq!(
        xml_text(&read, "Code").as_deref(),
        Some(NO_SUCH_ENTITY),
        "case {case:?}: expected IAM to report {NO_SUCH_ENTITY:?} for {name:?}, got {read}"
    );
}

/// The schedule body a write states: one occurrence far enough ahead that nothing is
/// invoked while the suite runs, the target the row's role is about, and the two members a
/// served update has to be seen to have applied.
fn schedule_body(target_arn: &str, role_arn: &str, description: &str, state: &str) -> Value {
    json!({
        "ScheduleExpression": format!("at({})", at_expression_body(seconds_from_now(OCCURRENCE_IN))),
        "ScheduleExpressionTimezone": "UTC",
        "FlexibleTimeWindow": { "Mode": "OFF" },
        "State": state,
        "Description": description,
        "Target": {
            "Arn": target_arn,
            "RoleArn": role_arn,
            "Input": "{}",
        },
    })
}
