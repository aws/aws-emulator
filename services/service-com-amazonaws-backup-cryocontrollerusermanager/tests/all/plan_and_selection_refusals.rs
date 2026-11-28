// SPDX-License-Identifier: CC0-1.0

//! How AWS Backup REFUSES — and does not refuse — a call about a backup plan or
//! one of its resource selections, driven as real HTTP through the router.
//!
//! The rules under test, each asserted over a table whose rows differ along what
//! the rule generalises over rather than on one recorded request:
//!
//! * a plan body is stored only when every rule's `ScheduleExpression` is an
//!   expression jobs can be initiated from; one that is not is refused with the
//!   per-rule validation envelope — the class's `Code`, the reason whole in
//!   `Context`, and that reason after the envelope's fixed prefix in `Message` —
//!   by every operation that stores a plan body, and nothing is stored;
//! * a backup selection is stored only for an `IamRoleArn` that names a role
//!   this account's IAM shows AWS Backup can assume;
//! * the two halves of the (`BackupPlanId`, `SelectionId`) pair are refused
//!   APART: a plan id the account holds no plan under is the parameter-value
//!   class with the asking call's own token, a selection id the plan does not
//!   hold is the absent selection with its own;
//! * a selection delete is idempotent once the plan is resolved: a selection id
//!   the plan does not hold answers the same empty success as the first delete;
//! * a listing of the selections of a plan that holds none reports an EMPTY
//!   list, not a refusal;
//! * a plan delete that finds no plan to read the latest version of names the
//!   plan it could not read, by the ARN built from the request's own region and
//!   the id it carried;
//! * a plan READ that cannot resolve the version it asks for — the one it named
//!   or the plan's latest — carries the unreadable-version token and sentence.
//!
//! The request harness (`call_reported`, `router`, `ACCOUNT`, `REGION`) is the
//! one `vault_and_backup_job.rs` exposes to this test binary, so there is ONE
//! wire path for the service's suites; the region-varying sender below is this
//! suite's own, since no other suite signs for a second region. The IAM-mounted
//! router and the `CreateRole` sender beside it are `pub(crate)`, because every
//! suite whose rule turns on what IAM holds for a role needs exactly that one
//! emulator and that one way of creating the role it names.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{call_reported, router, ACCOUNT, REGION};

/// The per-rule validation token (`crate::state`'s
/// `ERROR_CODE_BACKUP_PLAN_RULE_INVALID`).
const RULE_INVALID_CODE: &str = "ERROR_3015";

/// The per-rule reason for a schedule expression that is not a cron expression
/// (`crate::state`'s `MESSAGE_CRON_INVALID_PREFIX` + `MESSAGE_CRON_INVALID`).
const CRON_INVALID_REASON: &str = "Cron expression is not valid: Invalid cron expression";

/// The sentence that carries that reason (`MESSAGE_BACKUP_PLAN_RULES_PREFIX`).
const RULE_INVALID_MESSAGE: &str =
    "Error in some rules due to : Cron expression is not valid: Invalid cron expression";

/// The token and sentence a role AWS Backup cannot assume is refused with
/// (`ERROR_CODE_ROLE_NOT_ASSUMABLE` / `MESSAGE_ROLE_NOT_ASSUMABLE`).
const ROLE_NOT_ASSUMABLE_CODE: &str = "ERROR_3017";
const ROLE_NOT_ASSUMABLE_MESSAGE: &str = "IAM Role cannot be assumed by AWS Backup";

/// The service principal a role has to admit (`crate::state`'s
/// `SERVICE_PRINCIPAL`).
const SERVICE_PRINCIPAL: &str = "backup.amazonaws.com";

/// The tokens of the two selection-scoped refusals, and the sentence each
/// carries (`ERROR_CODE_SELECTION_READ_PLAN_NOT_FOUND`,
/// `ERROR_CODE_SELECTION_DELETE_PLAN_NOT_FOUND`,
/// `ERROR_CODE_BACKUP_SELECTION_NOT_FOUND`).
const PLAN_NOT_FOUND_ON_READ_CODE: &str = "ERROR_2304";
const PLAN_NOT_FOUND_ON_DELETE_CODE: &str = "ERROR_2302";
const PLAN_NOT_FOUND_BY_ID_MESSAGE: &str = "Cannot find Backup plan with ID";
const SELECTION_NOT_FOUND_CODE: &str = "ERROR_2303";
const SELECTION_NOT_FOUND_MESSAGE: &str = "Cannot find Backup selection";

/// The token and sentence a read of a plan version the plan does not have
/// carries (`ERROR_CODE_BACKUP_PLAN_VERSION_UNREADABLE` /
/// `MESSAGE_BACKUP_PLAN_VERSION_UNREADABLE`).
const VERSION_UNREADABLE_CODE: &str = "ERROR_2202";
const VERSION_UNREADABLE_MESSAGE: &str = "Failed reading Backup plan with provided version";

/// An id no plan and no selection of a fresh store answers to.
const UNHELD_ID: &str = "11111111-2222-3333-4444-555555555555";

/// What a refused call said about itself: the status, the error shape named by
/// `x-amzn-errortype`, and the `Code`, `Context` and `Message` members of the
/// body, each `"<unset>"` when absent — the distinction these rules are about.
#[derive(Debug, PartialEq, Eq)]
struct Refusal {
    status: u16,
    errortype: String,
    code: String,
    context: String,
    message: String,
}

impl Refusal {
    /// The refusal a response reports.
    fn of(status: u16, errortype: &str, body: &Value) -> Self {
        Self {
            status,
            errortype: errortype.to_string(),
            code: member(body, "Code"),
            context: member(body, "Context"),
            message: member(body, "Message"),
        }
    }

    /// The refusal a row expects, with no `Context`.
    fn expected(errortype: &str, code: &str, message: &str) -> Self {
        Self {
            status: 400,
            errortype: errortype.to_string(),
            code: code.to_string(),
            context: "<unset>".to_string(),
            message: message.to_string(),
        }
    }
}

/// Whether a response body is the empty JSON body a successful delete answers
/// with: nothing at all, or an object with no members.
fn is_empty_body(body: &Value) -> bool {
    body.is_null() || body.as_object().is_some_and(serde_json::Map::is_empty)
}

/// A body member as a string, or `"<unset>"` when the member is absent.
fn member(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or("<unset>")
        .to_string()
}

/// A string member of a JSON object, or the empty string.
fn text(body: &Value, name: &str) -> String {
    body.get(name)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `PUT /backup/plans` — `CreateBackupPlan`'s HTTP binding.
async fn create_plan(router: &MultiServiceRouter, body: &Value) -> (u16, String, Value) {
    call_reported(router, "PUT", "/backup/plans", Some(&body.to_string())).await
}

/// `POST /backup/plans/{BackupPlanId}` — `UpdateBackupPlan`'s.
async fn update_plan(
    router: &MultiServiceRouter,
    plan_id: &str,
    body: &Value,
) -> (u16, String, Value) {
    call_reported(
        router,
        "POST",
        &format!("/backup/plans/{plan_id}"),
        Some(&body.to_string()),
    )
    .await
}

/// `GET /backup/plans/{BackupPlanId}` — `GetBackupPlan`'s, with its optional
/// `versionId` query member.
async fn get_plan(
    router: &MultiServiceRouter,
    plan_id: &str,
    version: Option<&str>,
) -> (u16, String, Value) {
    let path = match version {
        None => format!("/backup/plans/{plan_id}"),
        Some(version) => format!("/backup/plans/{plan_id}?versionId={version}"),
    };
    call_reported(router, "GET", &path, None).await
}

/// `DELETE /backup/plans/{BackupPlanId}` — `DeleteBackupPlan`'s.
async fn delete_plan(router: &MultiServiceRouter, plan_id: &str) -> (u16, String, Value) {
    call_reported(router, "DELETE", &format!("/backup/plans/{plan_id}"), None).await
}

/// `PUT /backup/plans/{BackupPlanId}/selections` — `CreateBackupSelection`'s.
async fn create_selection(
    router: &MultiServiceRouter,
    plan_id: &str,
    selection: &Value,
) -> (u16, String, Value) {
    call_reported(
        router,
        "PUT",
        &format!("/backup/plans/{plan_id}/selections"),
        Some(&json!({"BackupSelection": selection}).to_string()),
    )
    .await
}

/// `GET /backup/plans/{BackupPlanId}/selections/{SelectionId}` —
/// `GetBackupSelection`'s.
async fn get_selection(
    router: &MultiServiceRouter,
    plan_id: &str,
    selection_id: &str,
) -> (u16, String, Value) {
    call_reported(
        router,
        "GET",
        &format!("/backup/plans/{plan_id}/selections/{selection_id}"),
        None,
    )
    .await
}

/// `DELETE /backup/plans/{BackupPlanId}/selections/{SelectionId}` —
/// `DeleteBackupSelection`'s.
async fn delete_selection(
    router: &MultiServiceRouter,
    plan_id: &str,
    selection_id: &str,
) -> (u16, String, Value) {
    call_reported(
        router,
        "DELETE",
        &format!("/backup/plans/{plan_id}/selections/{selection_id}"),
        None,
    )
    .await
}

/// `GET /backup/plans/{BackupPlanId}/selections` — `ListBackupSelections`'s.
async fn list_selections(router: &MultiServiceRouter, plan_id: &str) -> (u16, String, Value) {
    call_reported(
        router,
        "GET",
        &format!("/backup/plans/{plan_id}/selections"),
        None,
    )
    .await
}

/// A plan body of one rule, with `schedule` as that rule's
/// `ScheduleExpression` (absent when `None`).
fn plan_body(name: &str, schedule: Option<&str>) -> Value {
    let mut rule = json!({"RuleName": "daily", "TargetBackupVaultName": "plan-vault"});
    if let Some(schedule) = schedule {
        rule["ScheduleExpression"] = Value::String(schedule.to_string());
    }
    json!({"BackupPlan": {"BackupPlanName": name, "Rules": [rule]}})
}

/// Create a plan with no stated schedule and answer the id it minted.
async fn plan_id_of(router: &MultiServiceRouter, name: &str) -> String {
    let (status, _, created) = create_plan(router, &plan_body(name, None)).await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupPlan {name:?} expected 200, got {status} {created:?}"
    );
    text(&created, "BackupPlanId")
}

/// An IAM role ARN of this emulator's account.
fn role_arn(role: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{role}")
}

/// A selection body naming `name` and `role_arn`, assigning one resource — a
/// selection has to select something to be stored at all
/// (`crate::state`'s `rejected_empty_backup_selection`).
fn selection_body(name: &str, role_arn: &str) -> Value {
    json!({
        "SelectionName": name,
        "IamRoleArn": role_arn,
        "Resources": [format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo")],
    })
}

/// The schedule expressions a plan body is measured against.
///
/// SOURCE of the accepted spellings: the model's own documentation for the
/// member being stored —
/// `smithy_query(query="doc", shape="BackupRule", member="ScheduleExpression")`
/// — which states the default `cron(0 5 ? * * *)` and gives `0 12 * * ? *` and
/// `15 * ? * * *` as examples, so both the wrapped and the bare spelling are
/// expressions jobs run from. The refused rows are the ways an expression can
/// fail to be one: no expression at all, the wrong field count on either side,
/// a field outside its range, another expression language, and an unclosed
/// wrapper.
fn schedule_rows() -> Vec<(&'static str, Option<&'static str>, bool)> {
    vec![
        ("no expression at all, which runs on the default", None, true),
        (
            "the default expression the model documents",
            Some("cron(0 5 ? * * *)"),
            true,
        ),
        (
            "the model's bare daily example",
            Some("0 12 * * ? *"),
            true,
        ),
        (
            "the model's bare hourly example",
            Some("15 * ? * * *"),
            true,
        ),
        (
            "a wrapped expression naming a year",
            Some("cron(0 0 1 1 ? 2030)"),
            true,
        ),
        ("the empty expression", Some(""), false),
        ("text that is not an expression", Some("every day"), false),
        ("one field short", Some("cron(0 5 ? * *)"), false),
        ("one field long", Some("cron(0 5 ? * * * *)"), false),
        ("a minute field out of range", Some("cron(99 5 ? * * *)"), false),
        (
            "a month field naming no month",
            Some("cron(0 5 ? FOO * *)"),
            false,
        ),
        ("a rate expression", Some("rate(1 hour)"), false),
        ("an unclosed wrapper", Some("cron(0 5 ? * * *"), false),
    ]
}

/// A plan body is stored only when every rule's schedule is an expression jobs
/// can be initiated from, and the refusal of one that is not carries the
/// per-rule validation envelope. Asserted on the create, which stores the first
/// version of a plan, and on the update, which stores another — the two share
/// one check, so neither can accept what the other refuses.
#[tokio::test]
async fn a_rule_is_stored_only_with_a_schedule_expression_jobs_can_be_initiated_from() {
    for (case, schedule, accepted) in schedule_rows() {
        // One router per row: each row's plan is the only one the account holds,
        // so a refused create is visible as an EMPTY plan list.
        let router = router();
        let (status, errortype, created) =
            create_plan(&router, &plan_body("scheduled", schedule)).await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBackupPlan 200, got {status} {created:?}"
            );
            let plan_id = text(&created, "BackupPlanId");
            let (status, _, read) = get_plan(&router, &plan_id, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetBackupPlan 200, got {status} {read:?}"
            );
            assert_eq!(
                read["BackupPlan"]["Rules"][0]
                    .get("ScheduleExpression")
                    .and_then(Value::as_str),
                schedule,
                "case {case:?}: expected the stored rule to report the schedule it was \
                 created with, got {read:?}"
            );
            continue;
        }

        assert_eq!(
            Refusal::of(status, &errortype, &created),
            Refusal {
                status: 400,
                errortype: "InvalidParameterValueException".to_string(),
                code: RULE_INVALID_CODE.to_string(),
                context: CRON_INVALID_REASON.to_string(),
                message: RULE_INVALID_MESSAGE.to_string(),
            },
            "case {case:?}: expected the per-rule refusal, got {status} {errortype} {created:?}"
        );
        // A refused create stored nothing: the name it named is still free, so a
        // body the rule admits can take it.
        let (status, _, remade) = create_plan(&router, &plan_body("scheduled", None)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused create must store no plan, so its name is still \
             free; got {status} {remade:?}"
        );
        let plan_id = text(&remade, "BackupPlanId");
        let (status, errortype, refused) =
            update_plan(&router, &plan_id, &plan_body("scheduled", schedule)).await;
        assert_eq!(
            Refusal::of(status, &errortype, &refused),
            Refusal {
                status: 400,
                errortype: "InvalidParameterValueException".to_string(),
                code: RULE_INVALID_CODE.to_string(),
                context: CRON_INVALID_REASON.to_string(),
                message: RULE_INVALID_MESSAGE.to_string(),
            },
            "case {case:?}: expected UpdateBackupPlan to refuse it the same way, got \
             {status} {errortype} {refused:?}"
        );
        let (status, _, read) = get_plan(&router, &plan_id, None).await;
        assert_eq!(
            (
                status,
                read["BackupPlan"]["Rules"][0].get("ScheduleExpression")
            ),
            (200, None),
            "case {case:?}: a refused update must leave the stored rule alone, got \
             {status} {read:?}"
        );
    }
}

/// The two halves of the pair a selection is addressed by are refused apart, and
/// the delete refuses only the plan half: the selection half is idempotent.
#[tokio::test]
async fn the_halves_of_a_selection_pair_are_refused_apart() {
    let router = router();
    let plan_id = plan_id_of(&router, "pair-plan").await;
    let (status, _, created) = create_selection(
        &router,
        &plan_id,
        &selection_body("transient", &role_arn("S3Access")),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupSelection expected 200, got {status} {created:?}"
    );
    let selection_id = text(&created, "SelectionId");
    let (status, _, removed) = delete_selection(&router, &plan_id, &selection_id).await;
    assert_eq!(
        status, 200,
        "setup: DeleteBackupSelection expected 200, got {status} {removed:?}"
    );

    // A plan the account no longer holds, to show the plan half is about what
    // the account HOLDS and not about the id's spelling.
    let deleted_plan = plan_id_of(&router, "deleted-plan").await;
    let (status, _, deleted) = delete_plan(&router, &deleted_plan).await;
    assert_eq!(
        status, 200,
        "setup: DeleteBackupPlan expected 200, got {status} {deleted:?}"
    );

    // (case, the plan addressed, the selection addressed, whether the PLAN half
    // is the one at fault)
    let rows: Vec<(&str, &str, &str, bool)> = vec![
        (
            "a selection the delete removed, under the plan that held it",
            plan_id.as_str(),
            selection_id.as_str(),
            false,
        ),
        (
            "a selection id nothing created, under a plan the account holds",
            plan_id.as_str(),
            UNHELD_ID,
            false,
        ),
        (
            "a plan id the account never held",
            UNHELD_ID,
            selection_id.as_str(),
            true,
        ),
        (
            "a plan the account deleted",
            deleted_plan.as_str(),
            selection_id.as_str(),
            true,
        ),
    ];
    for (case, plan, selection, plan_at_fault) in rows {
        let (status, errortype, read) = get_selection(&router, plan, selection).await;
        let expected_read = if plan_at_fault {
            Refusal::expected(
                "InvalidParameterValueException",
                PLAN_NOT_FOUND_ON_READ_CODE,
                PLAN_NOT_FOUND_BY_ID_MESSAGE,
            )
        } else {
            Refusal::expected(
                "ResourceNotFoundException",
                SELECTION_NOT_FOUND_CODE,
                SELECTION_NOT_FOUND_MESSAGE,
            )
        };
        assert_eq!(
            Refusal::of(status, &errortype, &read),
            expected_read,
            "case {case:?}: GetBackupSelection must name the half at fault, got {status} \
             {errortype} {read:?}"
        );

        let (status, errortype, again) = delete_selection(&router, plan, selection).await;
        if plan_at_fault {
            assert_eq!(
                Refusal::of(status, &errortype, &again),
                Refusal::expected(
                    "InvalidParameterValueException",
                    PLAN_NOT_FOUND_ON_DELETE_CODE,
                    PLAN_NOT_FOUND_BY_ID_MESSAGE,
                ),
                "case {case:?}: DeleteBackupSelection must refuse the plan half, got \
                 {status} {errortype} {again:?}"
            );
        } else {
            // The plan is there, so the delete has nothing to refuse: the
            // selection it was asked to remove is already not assigned.
            assert_eq!(
                (status, is_empty_body(&again)),
                (200, true),
                "case {case:?}: DeleteBackupSelection must be idempotent once the plan \
                 resolves, got {status} {again:?}"
            );
        }
    }
}

/// A listing of the selections of a plan that holds none reports an empty list,
/// whatever the reason it holds none.
#[tokio::test]
async fn a_plan_that_holds_no_selections_lists_an_empty_page() {
    let router = router();
    let live = plan_id_of(&router, "empty-plan").await;

    let emptied = plan_id_of(&router, "emptied-plan").await;
    let (status, _, created) = create_selection(
        &router,
        &emptied,
        &selection_body("transient", &role_arn("S3Access")),
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupSelection expected 200, got {status} {created:?}"
    );
    let (status, _, removed) =
        delete_selection(&router, &emptied, &text(&created, "SelectionId")).await;
    assert_eq!(
        status, 200,
        "setup: DeleteBackupSelection expected 200, got {status} {removed:?}"
    );

    let dropped = plan_id_of(&router, "dropped-plan").await;
    let (status, _, deleted) = delete_plan(&router, &dropped).await;
    assert_eq!(
        status, 200,
        "setup: DeleteBackupPlan expected 200, got {status} {deleted:?}"
    );

    for (case, plan_id) in [
        ("a plan that never held a selection", live.as_str()),
        ("a plan whose only selection was deleted", emptied.as_str()),
        ("a plan the account deleted", dropped.as_str()),
        ("a plan id the account never held", UNHELD_ID),
    ] {
        let (status, _, listed) = list_selections(&router, plan_id).await;
        assert_eq!(
            (
                status,
                listed.get("BackupSelectionsList").and_then(Value::as_array)
            ),
            (200, Some(&Vec::new())),
            "case {case:?}: expected 200 with an empty BackupSelectionsList, got {status} \
             {listed:?}"
        );
        assert!(
            listed.get("NextToken").is_none(),
            "case {case:?}: an empty page carries no cursor, got {listed:?}"
        );
    }
}

/// A plan delete that finds no plan to read the latest version of names the plan
/// it could not read, by the ARN built from the request's own region and the id
/// it carried; a plan READ that cannot resolve the version it asks for carries
/// the unreadable-version token and sentence instead.
#[tokio::test]
async fn a_refused_plan_read_names_what_it_could_not_read() {
    let router = router();
    let plan_id = plan_id_of(&router, "named-plan").await;
    let (status, _, created) = get_plan(&router, &plan_id, None).await;
    assert_eq!(
        status, 200,
        "setup: GetBackupPlan expected 200, got {status} {created:?}"
    );
    let version_id = text(&created, "VersionId");
    let (status, _, deleted) = delete_plan(&router, &plan_id).await;
    assert_eq!(
        status, 200,
        "setup: DeleteBackupPlan expected 200, got {status} {deleted:?}"
    );

    // The ids the delete is asked about, each refused by the ARN of the plan it
    // names: one the account deleted, one it never held, and one spelled unlike
    // any id this service mints.
    for (case, asked) in [
        ("a plan the account deleted", plan_id.as_str()),
        ("a plan id the account never held", UNHELD_ID),
        ("an id nothing could have minted", "not-a-plan-id"),
    ] {
        let (status, errortype, refused) = delete_plan(&router, asked).await;
        let expected = format!(
            "Failed reading latest version of backup plan \
             arn:aws:backup:{REGION}:{ACCOUNT}:backup-plan:{asked} - backup plan does not \
             exist!"
        );
        assert_eq!(
            Refusal::of(status, &errortype, &refused),
            Refusal {
                status: 400,
                errortype: "ResourceNotFoundException".to_string(),
                code: "<unset>".to_string(),
                context: "<unset>".to_string(),
                message: expected.clone(),
            },
            "case {case:?}: expected the ARN-bearing refusal {expected:?}, got {status} \
             {errortype} {refused:?}"
        );
    }

    // A plan READ answers from the plan AT a version — the one it named, or the
    // plan's latest — so every way it can fail to resolve one answers the
    // unreadable-version refusal: a version a deleted plan held, one a live plan
    // never had, a version of a plan the account holds nothing under, and a read
    // that names no version at all against such an id.
    let live = plan_id_of(&router, "live-plan").await;
    let unheld = UNHELD_ID.to_string();
    for (case, asked_plan, asked_version) in [
        (
            "the version of a plan the account deleted",
            &plan_id,
            Some(&version_id),
        ),
        ("a version a live plan never had", &live, Some(&unheld)),
        (
            "a version of a plan the account never held",
            &unheld,
            Some(&version_id),
        ),
        ("a plan the account deleted, at no version", &plan_id, None),
        (
            "a plan id the account never held, at no version",
            &unheld,
            None,
        ),
    ] {
        let (status, errortype, refused) =
            get_plan(&router, asked_plan, asked_version.map(String::as_str)).await;
        assert_eq!(
            Refusal::of(status, &errortype, &refused),
            Refusal::expected(
                "ResourceNotFoundException",
                VERSION_UNREADABLE_CODE,
                VERSION_UNREADABLE_MESSAGE,
            ),
            "case {case:?}: expected the unreadable-version refusal, got {status} \
             {errortype} {refused:?}"
        );
    }
}

/// This service AND IAM on ONE engine, plus that engine's router: the role
/// resolution asks IAM over the emulator's own in-process callback, so IAM has
/// to be mounted for it to answer at all.
pub(crate) fn router_with_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// Percent-encode one value of IAM's `application/x-www-form-urlencoded`
/// awsQuery body.
pub(crate) fn form_value(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// Create an IAM role named `name` under `path` carrying `trust` as its
/// `AssumeRolePolicyDocument`, through IAM's OWN awsQuery wire, and answer the
/// ARN IAM minted for it — read off the response rather than constructed, so the
/// value the backup request names is the one IAM actually holds.
pub(crate) async fn create_role(
    router: &MultiServiceRouter,
    path: &str,
    name: &str,
    trust: &str,
) -> String {
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&Path={}&RoleName={}&AssumeRolePolicyDocument={}",
        form_value(path),
        form_value(name),
        form_value(trust)
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    assert_eq!(
        status, 200,
        "CreateRole {name:?} expected 200, got {status} {xml}"
    );
    let start = xml
        .find("<Arn>")
        .unwrap_or_else(|| panic!("CreateRole {name:?} returned no <Arn>: {xml}"))
        + "<Arn>".len();
    let end = xml[start..]
        .find("</Arn>")
        .unwrap_or_else(|| panic!("CreateRole {name:?} left <Arn> unclosed: {xml}"));
    xml[start..start + end].to_string()
}

/// A trust document with one statement: `effect` for the service `principal`
/// performing `action`.
fn trust(effect: &str, principal: &str, action: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": {"Service": principal},
            "Action": action,
        }],
    })
    .to_string()
}

/// The role a backup JOB is performed under, provisioned where IAM is mounted beside this
/// service: AWS Backup ASSUMES that role before it mints a job, so this account's IAM has
/// to hold the record, and the policy it carries has to allow the action that creates the
/// recovery point of the resource the suite backs up — a role carrying none would settle
/// the job `FAILED` and write no point, which is what `backup_job_role_outcome.rs` covers
/// rather than what a suite needing a stored point is about.
///
/// Answers the ARN IAM minted for it, which is the value a start then names.
///
/// `pub(crate)` for the reason [`create_role`] is: every suite that starts a backup job
/// beside a mounted IAM needs exactly this one way of provisioning that role.
pub(crate) async fn given_backup_job_role(
    router: &MultiServiceRouter,
    path: &str,
    name: &str,
    allows: &str,
) -> String {
    let arn = create_role(
        router,
        path,
        name,
        &trust("Allow", SERVICE_PRINCIPAL, "sts:AssumeRole"),
    )
    .await;
    let document = json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": allows, "Resource": "*" }],
    })
    .to_string();
    put_role_policy(router, name, &document).await;
    arn
}

/// Store `document` on the role `role_name` as an inline identity policy, through
/// IAM's OWN awsQuery wire, so the permissions a backup decision reads are the
/// ones IAM holds.
///
/// `pub(crate)` for the reason [`create_role`] is: every suite whose rule turns on
/// what a role may DO needs exactly this one way of giving it permissions.
pub(crate) async fn put_role_policy(
    router: &MultiServiceRouter,
    role_name: &str,
    document: &str,
) {
    let body = format!(
        "Action=PutRolePolicy&Version=2010-05-08&RoleName={}&PolicyName={}&PolicyDocument={}",
        form_value(role_name),
        form_value("backup-selection-permissions"),
        form_value(document)
    );
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", "iam.amazonaws.com")
        .header("content-type", "application/x-www-form-urlencoded")
        .body(bytes::Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let xml = String::from_utf8(response.into_body().to_vec()).expect("a UTF-8 response body");
    assert_eq!(
        status, 200,
        "PutRolePolicy {role_name:?} expected 200, got {status} {xml}"
    );
}

/// The trust documents the rule generalises over, and whether a selection whose
/// role carries each one is stored.
fn trust_rows() -> Vec<(&'static str, String, bool)> {
    vec![
        (
            "a document admitting this service's principal",
            trust("Allow", SERVICE_PRINCIPAL, "sts:AssumeRole"),
            true,
        ),
        (
            "a wildcard covering the assume action",
            trust("Allow", SERVICE_PRINCIPAL, "sts:*"),
            true,
        ),
        (
            "the principal and the action in another casing",
            trust("Allow", "BACKUP.amazonaws.com", "STS:AssumeRole"),
            true,
        ),
        (
            "a second statement admitting this service's principal",
            json!({
                "Version": "2012-10-17",
                "Statement": [
                    {
                        "Effect": "Allow",
                        "Principal": {"Service": "ec2.amazonaws.com"},
                        "Action": "sts:AssumeRole",
                    },
                    {
                        "Effect": "Allow",
                        "Principal": {"Service": SERVICE_PRINCIPAL},
                        "Action": "sts:AssumeRole",
                    },
                ],
            })
            .to_string(),
            true,
        ),
        (
            "a document admitting another service only",
            trust("Allow", "lambda.amazonaws.com", "sts:AssumeRole"),
            false,
        ),
        (
            "an explicit deny for this service's principal",
            trust("Deny", SERVICE_PRINCIPAL, "sts:AssumeRole"),
            false,
        ),
        (
            "this service's principal named for another action",
            trust("Allow", SERVICE_PRINCIPAL, "sts:TagSession"),
            false,
        ),
    ]
}

/// A selection is stored exactly for a role whose trust document this account's
/// IAM shows admits AWS Backup, and a refused create stores nothing.
#[tokio::test]
async fn a_selection_is_stored_only_for_a_role_iam_shows_backup_can_assume() {
    for (index, (case, document, stored)) in trust_rows().into_iter().enumerate() {
        let router = router_with_iam();
        let role = create_role(&router, "/", &format!("backup-role-{index}"), &document).await;
        let plan_id = plan_id_of(&router, "role-plan").await;

        let (status, errortype, answered) =
            create_selection(&router, &plan_id, &selection_body("assigned", &role)).await;
        if stored {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBackupSelection 200, got {status} {answered:?}"
            );
            let (status, _, read) =
                get_selection(&router, &plan_id, &text(&answered, "SelectionId")).await;
            assert_eq!(
                (
                    status,
                    read["BackupSelection"]
                        .get("IamRoleArn")
                        .and_then(Value::as_str)
                ),
                (200, Some(role.as_str())),
                "case {case:?}: expected the stored selection to report its role, got \
                 {status} {read:?}"
            );
            continue;
        }
        assert_eq!(
            Refusal::of(status, &errortype, &answered),
            Refusal::expected(
                "InvalidParameterValueException",
                ROLE_NOT_ASSUMABLE_CODE,
                ROLE_NOT_ASSUMABLE_MESSAGE,
            ),
            "case {case:?}: expected the unassumable-role refusal, got {status} {errortype} \
             {answered:?}"
        );
        let (status, _, listed) = list_selections(&router, &plan_id).await;
        assert_eq!(
            (
                status,
                listed.get("BackupSelectionsList").and_then(Value::as_array)
            ),
            (200, Some(&Vec::new())),
            "case {case:?}: a refused create must store no selection, got {status} {listed:?}"
        );
    }
}

/// The forms an `IamRoleArn` can take that name no role AWS Backup could assume
/// for this account, beside the one that does: the rule is about what the value
/// RESOLVES to, not about its spelling.
#[tokio::test]
async fn a_selection_role_must_name_a_role_this_accounts_iam_holds() {
    let admitting = trust("Allow", SERVICE_PRINCIPAL, "sts:AssumeRole");
    // (case, the IamRoleArn the create names, whether it is stored)
    for (index, (case, named, stored)) in [
        ("the role this account's IAM holds", None, true),
        (
            "a role under a path, which IAM holds under its name",
            None,
            true,
        ),
        (
            "a role name this account's IAM holds nothing under",
            Some(role_arn("NeverCreated")),
            false,
        ),
        (
            "another account's role of the same name",
            Some("arn:aws:iam::111122223333:role/backup-held-0".to_string()),
            false,
        ),
        (
            "an IAM user rather than a role",
            Some(format!("arn:aws:iam::{ACCOUNT}:user/backup-held-0")),
            false,
        ),
        (
            "an ARN of another service entirely",
            Some("arn:aws:s3:::backup-held-0".to_string()),
            false,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let router = router_with_iam();
        // Every row's account holds the same role, under the path the second row
        // names, so a refusal is about the value the create named and not about
        // an empty IAM.
        let held = create_role(
            &router,
            if index == 1 { "/service-role/" } else { "/" },
            "backup-held-0",
            &admitting,
        )
        .await;
        let plan_id = plan_id_of(&router, "role-form-plan").await;
        let role = named.unwrap_or_else(|| held.clone());

        let (status, errortype, answered) =
            create_selection(&router, &plan_id, &selection_body("assigned", &role)).await;
        if stored {
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateBackupSelection 200 for {role:?}, got {status} \
                 {answered:?}"
            );
            continue;
        }
        assert_eq!(
            Refusal::of(status, &errortype, &answered),
            Refusal::expected(
                "InvalidParameterValueException",
                ROLE_NOT_ASSUMABLE_CODE,
                ROLE_NOT_ASSUMABLE_MESSAGE,
            ),
            "case {case:?}: expected {role:?} to be refused as unassumable, got {status} \
             {errortype} {answered:?}"
        );
    }
}
