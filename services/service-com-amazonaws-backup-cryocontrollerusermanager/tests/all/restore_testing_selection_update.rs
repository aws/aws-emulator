// SPDX-License-Identifier: CC0-1.0

//! `UpdateRestoreTestingSelection` and what the read over the stored selection
//! (`GetRestoreTestingSelection`) reports afterwards.
//!
//! The rules under test:
//!
//! * an update REWRITES the stored selection: every member its body states is
//!   what every later read of that (plan, selection) pair reports, and a member
//!   it states nothing for keeps the value the selection already held — "most
//!   elements except the RestoreTestingSelectionName can be updated with this
//!   request" (model doc for `UpdateRestoreTestingSelection`), so the name, the
//!   plan, the protected resource type and the creation instant survive the
//!   rewrite;
//! * a stated collection REPLACES the stored one and is completed exactly as a
//!   create completes it, so the conditions filter and the overrides map are
//!   reported as the collections the selection holds — empty ones included —
//!   and the resource scope as the specific ARNs stated or the documented
//!   wildcard where none were;
//! * the update's `UpdateTime` is the instant it stored the merged body, beside
//!   the selection's own creation instant;
//! * the merged selection is read by the rules that admit a create's body: "you
//!   can use either protected resource ARNs or conditions, but not both" (the
//!   same model doc) is `InvalidParameterValueException` naming the member, a
//!   merged selection that selects no protected resource at all is
//!   `MissingParameterValueException`, and a pair the account holds nothing
//!   under is the matching not-found refusal — a refused update rewrites
//!   nothing.
//!
//! Every case drives real HTTP through the router; the harness is the shared one
//! in `vault_and_backup_job.rs` and the bodies are the ones the sibling
//! `restore_testing_selection.rs` suite already states.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::restore_testing_selection::{
    completed_conditions, completed_overrides, completed_scope, create_body, create_selection,
    get_selection, minimal_selection, protected_resource_arn, read_selection, role_arn,
    stored_plan,
};
use crate::vault_and_backup_job::{call, error_code, router, string_at};

/// The sentence this service states for "the restore testing plan asked about is
/// not one this account holds" — fixed text, naming no part of the request.
const PLAN_NOT_FOUND: &str = "Restore testing plan does not exist.";

/// The sentence this service states for "the restore testing selection asked
/// about is not one the named plan holds".
const SELECTION_NOT_FOUND: &str = "Restore testing selection does not exist.";

/// The sentence this service states for a selection that selects no protected
/// resource at all.
const SELECTS_NOTHING: &str = "Missing Resource Selection.";

/// The published bounds of a validation window: "Accepted value is an integer
/// between 0 and 168 (the hourly equivalent of seven days)" (model doc for
/// `RestoreTestingSelectionForUpdate$ValidationWindowHours`).
const MIN_VALIDATION_WINDOW_HOURS: i64 = 0;
const MAX_VALIDATION_WINDOW_HOURS: i64 = 168;

/// The most specific protected resource ARNs one selection may name: "you can
/// include up to 30 specific protected resource ARNs in ProtectedResourceArns"
/// (model doc for `CreateRestoreTestingSelection`).
const MAX_PROTECTED_RESOURCE_ARNS: usize = 30;

/// `PUT /restore-testing/plans/{RestoreTestingPlanName}/selections/{RestoreTestingSelectionName}`,
/// as the model binds the update.
async fn update_selection(
    router: &MultiServiceRouter,
    plan: &str,
    name: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/restore-testing/plans/{plan}/selections/{name}"),
        Some(&body.to_string()),
    )
    .await
}

/// An update body carrying `selection` as its `RestoreTestingSelection`.
fn update_body(selection: Value) -> Value {
    json!({ "RestoreTestingSelection": selection })
}

/// A string member of a body, if it carries one.
fn string_of(body: &Value, member: &str) -> Option<String> {
    body.get(member)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// A validation window a body STATES: the member carries a modelled default of
/// `0`, so a body that states nothing for it is indistinguishable from one
/// stating the floor of its range and states no new window.
fn stated_window(body: &Value) -> Option<i64> {
    body.get("ValidationWindowHours")
        .and_then(Value::as_i64)
        .filter(|hours| *hours != MIN_VALIDATION_WINDOW_HOURS)
}

/// The validation window a read reports, whose member the response may omit
/// when it carries the modelled default.
fn reported_window(body: &Value) -> i64 {
    body.get("ValidationWindowHours")
        .and_then(Value::as_i64)
        .unwrap_or(MIN_VALIDATION_WINDOW_HOURS)
}

/// A selection body selecting its resources by one tag condition and nothing
/// else, so a row can replace that scope with ARNs.
fn selection_by_condition(name: &str) -> Value {
    json!({
        "RestoreTestingSelectionName": name,
        "IamRoleArn": role_arn("RestoreTestingByTag"),
        "ProtectedResourceType": "EFS",
        "ProtectedResourceConditions": {
            "StringEquals": [{ "Key": "team", "Value": "backup" }],
        },
    })
}

/// Store `plan` and `selection` under it, and answer what the create reported.
async fn created_selection(
    router: &MultiServiceRouter,
    case: &str,
    plan: &str,
    selection: &Value,
) -> Value {
    stored_plan(router, plan).await;
    let (status, created) = create_selection(router, plan, &create_body(selection.clone())).await;
    assert_eq!(
        (status, error_code(&created).as_str()),
        (201, "<none>"),
        "case {case:?}: setup create should succeed, got {status} {created:?}"
    );
    created
}

/// An update rewrites exactly the members its body states, leaves every other
/// member of the stored selection alone, and every later read reports the
/// result.
///
/// The rows differ along what the rule generalises over: which members the body
/// states (one at a time, all of them at once and none at all), the form of the
/// resource scope it states (the wildcard, specific ARNs, an emptied map), the
/// baseline selection's own form (by wildcard, by condition, by specific ARNs)
/// and the validation window — at the floor of its range and at its published
/// ceiling.
#[tokio::test]
async fn an_update_rewrites_the_members_its_body_states() {
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the role alone",
            minimal_selection("selection_role"),
            json!({ "IamRoleArn": role_arn("RestoreTestingRotated") }),
        ),
        (
            "the resource scope replaced by the wildcard",
            selection_by_condition("selection_wildcarded"),
            json!({ "ProtectedResourceArns": ["*"] }),
        ),
        (
            "the resource scope replaced by specific ARNs",
            json!({
                "RestoreTestingSelectionName": "selection_specific",
                "IamRoleArn": role_arn("AWSBackupRestoreTesting"),
                "ProtectedResourceType": "EBS",
                "ProtectedResourceArns": [protected_resource_arn(1)],
            }),
            json!({
                "ProtectedResourceArns": [
                    protected_resource_arn(2),
                    protected_resource_arn(3),
                ],
            }),
        ),
        (
            "the conditions replaced beside a wildcard scope",
            json!({
                "RestoreTestingSelectionName": "selection_conditions",
                "IamRoleArn": role_arn("RestoreTesting"),
                "ProtectedResourceType": "EC2",
                "ProtectedResourceArns": ["*"],
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "stage", "Value": "test" }],
                },
            }),
            json!({
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "team", "Value": "restore" }],
                    "StringNotEquals": [{ "Key": "stage", "Value": "prod" }],
                },
            }),
        ),
        (
            "the conditions emptied beside a wildcard scope",
            json!({
                "RestoreTestingSelectionName": "selection_unconditioned",
                "IamRoleArn": role_arn("RestoreTesting"),
                "ProtectedResourceType": "EC2",
                "ProtectedResourceArns": ["*"],
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "stage", "Value": "test" }],
                },
            }),
            json!({ "ProtectedResourceConditions": {} }),
        ),
        (
            "the overrides replaced",
            json!({
                "RestoreTestingSelectionName": "selection_overridden",
                "IamRoleArn": role_arn("RestoreTesting"),
                "ProtectedResourceType": "EC2",
                "ProtectedResourceArns": ["*"],
                "RestoreMetadataOverrides": { "instanceType": "t3.micro" },
            }),
            json!({
                "RestoreMetadataOverrides": {
                    "instanceType": "m5.large",
                    "subnetId": "subnet-0123456789abcdef0",
                },
            }),
        ),
        (
            "the overrides emptied",
            json!({
                "RestoreTestingSelectionName": "selection_unoverridden",
                "IamRoleArn": role_arn("RestoreTesting"),
                "ProtectedResourceType": "EC2",
                "ProtectedResourceArns": ["*"],
                "RestoreMetadataOverrides": { "instanceType": "t3.micro" },
            }),
            json!({ "RestoreMetadataOverrides": {} }),
        ),
        (
            "the resource scope emptied over a selection that filters by conditions",
            selection_by_condition("selection_unscoped"),
            json!({ "ProtectedResourceArns": [] }),
        ),
        (
            "the validation window at its published ceiling",
            minimal_selection("selection_window"),
            json!({ "ValidationWindowHours": MAX_VALIDATION_WINDOW_HOURS }),
        ),
        (
            "every member at once",
            selection_by_condition("selection_everything"),
            json!({
                "IamRoleArn": role_arn("RestoreTestingEverything"),
                "ProtectedResourceArns": ["*"],
                "ProtectedResourceConditions": {
                    "StringNotEquals": [{ "Key": "stage", "Value": "prod" }],
                },
                "RestoreMetadataOverrides": { "instanceType": "t3.small" },
                "ValidationWindowHours": 1,
            }),
        ),
        (
            "a body stating nothing at all",
            json!({
                "RestoreTestingSelectionName": "selection_untouched",
                "IamRoleArn": role_arn("RestoreTesting"),
                "ProtectedResourceType": "EC2",
                "ProtectedResourceArns": ["*"],
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "stage", "Value": "test" }],
                },
                "RestoreMetadataOverrides": { "instanceType": "t3.micro" },
                "ValidationWindowHours": 24,
            }),
            json!({}),
        ),
    ];

    for (case, baseline, body) in cases {
        let router = router();
        let plan = "plan_selection_update";
        let name = string_at(&baseline, "RestoreTestingSelectionName");
        let created = created_selection(&router, case, plan, &baseline).await;

        let (status, updated) =
            update_selection(&router, plan, &name, &update_body(body.clone())).await;
        assert_eq!(
            (status, error_code(&updated).as_str()),
            (200, "<none>"),
            "case {case:?}: expected UpdateRestoreTestingSelection 200, got {status} {updated:?}"
        );
        // The names address the selection rather than being rewritten, so the
        // identifiers the create minted survive.
        assert_eq!(
            (
                string_at(&updated, "RestoreTestingPlanName"),
                string_at(&updated, "RestoreTestingPlanArn"),
                string_at(&updated, "RestoreTestingSelectionName"),
                updated.get("CreationTime").cloned(),
            ),
            (
                plan.to_string(),
                string_at(&created, "RestoreTestingPlanArn"),
                name.clone(),
                created.get("CreationTime").cloned(),
            ),
            "case {case:?}: the update must report the selection's own identifiers, got {updated:?}"
        );
        let update_time = updated.get("UpdateTime").cloned();
        assert!(
            update_time.as_ref().is_some_and(Value::is_number),
            "case {case:?}: the update reports when it stored the merged body, got {updated:?}"
        );

        // What the read must now report: a member the update stated, else the
        // one the create stored, with the two completed collections filled in.
        let expected_role = string_of(&body, "IamRoleArn")
            .or_else(|| string_of(&baseline, "IamRoleArn"))
            .unwrap_or_default();
        let expected_arns = completed_scope(
            body.get("ProtectedResourceArns")
                .or_else(|| baseline.get("ProtectedResourceArns")),
        );
        let expected_conditions = completed_conditions(
            body.get("ProtectedResourceConditions")
                .or_else(|| baseline.get("ProtectedResourceConditions")),
        );
        let expected_overrides = completed_overrides(
            body.get("RestoreMetadataOverrides")
                .or_else(|| baseline.get("RestoreMetadataOverrides")),
        );
        let expected_window = stated_window(&body)
            .or_else(|| stated_window(&baseline))
            .unwrap_or(MIN_VALIDATION_WINDOW_HOURS);

        let (status, read) = get_selection(&router, plan, &name).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (200, "<none>"),
            "case {case:?}: expected GetRestoreTestingSelection 200, got {status} {read:?}"
        );
        let stored = read_selection(&read);
        assert_eq!(
            (
                string_at(&stored, "RestoreTestingPlanName"),
                string_at(&stored, "RestoreTestingSelectionName"),
                string_at(&stored, "ProtectedResourceType"),
                string_at(&stored, "IamRoleArn"),
                reported_window(&stored),
                stored.get("CreationTime").cloned(),
            ),
            (
                plan.to_string(),
                name.clone(),
                string_at(&baseline, "ProtectedResourceType"),
                expected_role,
                expected_window,
                created.get("CreationTime").cloned(),
            ),
            "case {case:?}: the read must report the updated selection, got {read:?}"
        );
        assert_eq!(
            (
                stored.get("ProtectedResourceArns").cloned(),
                stored.get("ProtectedResourceConditions").cloned(),
                stored.get("RestoreMetadataOverrides").cloned(),
            ),
            (
                Some(expected_arns),
                Some(expected_conditions),
                Some(expected_overrides),
            ),
            "case {case:?}: a stated collection replaces the stored one and the completed ones \
             are reported as the collections the selection holds, got {read:?}"
        );
    }
}

/// A second update over an already updated selection rewrites it again: the last
/// body submitted is what the read reports, and the selection's creation instant
/// and the plan's ARN stay put while its update instant moves on.
#[tokio::test]
async fn successive_updates_each_rewrite_the_selection() {
    let router = router();
    let plan = "plan_selection_update_twice";
    let name = "selection_updated_twice";
    let created = created_selection(
        &router,
        "successive updates",
        plan,
        &minimal_selection(name),
    )
    .await;

    let mut previous = created
        .get("CreationTime")
        .and_then(Value::as_f64)
        .unwrap_or_default();
    for (round, role) in ["RestoreTestingFirst", "RestoreTestingSecond", "RestoreTestingThird"]
        .iter()
        .enumerate()
    {
        let window = i64::try_from(round).unwrap_or_default() + 1;
        let body = update_body(json!({
            "IamRoleArn": role_arn(role),
            "ValidationWindowHours": window,
        }));
        let (status, updated) = update_selection(&router, plan, name, &body).await;
        assert_eq!(
            (status, error_code(&updated).as_str()),
            (200, "<none>"),
            "round {round}: expected UpdateRestoreTestingSelection 200, got {status} {updated:?}"
        );
        let update_time = updated
            .get("UpdateTime")
            .and_then(Value::as_f64)
            .unwrap_or(-1.0);
        assert!(
            update_time >= previous,
            "round {round}: the update's instant must not precede the previous write's, got \
             {update_time} after {previous}"
        );
        previous = update_time;

        let (status, read) = get_selection(&router, plan, name).await;
        let stored = read_selection(&read);
        assert_eq!(
            (
                status,
                string_at(&stored, "IamRoleArn"),
                reported_window(&stored),
                stored.get("CreationTime").cloned(),
            ),
            (
                200,
                role_arn(role),
                window,
                created.get("CreationTime").cloned(),
            ),
            "round {round}: the read must report the body the last update submitted, got {read:?}"
        );
    }
}

/// An update whose merged selection is outside what the model documents is
/// refused `InvalidParameterValueException` naming the member it rejected, and
/// the selection it addressed is left exactly as it was.
///
/// The rows span the documented rules the merged body is read by, each one step
/// past its bound — including the update's own documented case, specific ARNs
/// asked for over a selection that filters by conditions.
#[tokio::test]
async fn an_update_outside_the_documented_rules_is_refused() {
    let conditioned = selection_by_condition("selection_refused");
    let wildcarded = minimal_selection("selection_refused");
    let many_arns: Vec<String> = (0..=MAX_PROTECTED_RESOURCE_ARNS)
        .map(protected_resource_arn)
        .collect();
    let cases: Vec<(&str, Value, Value, &str)> = vec![
        (
            "a role that is no ARN",
            wildcarded.clone(),
            json!({ "IamRoleArn": "RestoreTestingRole" }),
            "IamRoleArn",
        ),
        (
            "a validation window one hour past its ceiling",
            wildcarded.clone(),
            json!({ "ValidationWindowHours": MAX_VALIDATION_WINDOW_HOURS + 1 }),
            "ValidationWindowHours",
        ),
        (
            "a negative validation window",
            wildcarded.clone(),
            json!({ "ValidationWindowHours": -1 }),
            "ValidationWindowHours",
        ),
        (
            "the wildcard mixed into a list of specific ARNs",
            wildcarded.clone(),
            json!({ "ProtectedResourceArns": ["*", protected_resource_arn(4)] }),
            "ProtectedResourceArns",
        ),
        (
            "specific ARNs asked for over a selection that filters by conditions",
            conditioned.clone(),
            json!({ "ProtectedResourceArns": [protected_resource_arn(5)] }),
            "ProtectedResourceArns",
        ),
        (
            "conditions asked for over a selection of specific ARNs",
            json!({
                "RestoreTestingSelectionName": "selection_refused",
                "IamRoleArn": role_arn("AWSBackupRestoreTesting"),
                "ProtectedResourceType": "EBS",
                "ProtectedResourceArns": [protected_resource_arn(6)],
            }),
            json!({
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "team", "Value": "backup" }],
                },
            }),
            "ProtectedResourceArns",
        ),
        (
            "one specific ARN past the documented maximum",
            wildcarded.clone(),
            json!({ "ProtectedResourceArns": many_arns }),
            "ProtectedResourceArns",
        ),
    ];

    for (case, baseline, body, member) in cases {
        let router = router();
        let plan = "plan_selection_update_refused";
        let name = string_at(&baseline, "RestoreTestingSelectionName");
        created_selection(&router, case, plan, &baseline).await;
        let (_, before) = get_selection(&router, plan, &name).await;

        let (status, refused) = update_selection(&router, plan, &name, &update_body(body)).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str(),
                string_at(&refused, "Context").as_str(),
            ),
            (
                400,
                "InvalidParameterValueException",
                format!("Parameter {member} is invalid.").as_str(),
                member,
            ),
            "case {case:?}: expected the refusal to name {member}, got {status} {refused:?}"
        );
        let (_, after) = get_selection(&router, plan, &name).await;
        assert_eq!(
            read_selection(&after),
            read_selection(&before),
            "case {case:?}: a refused update must rewrite nothing"
        );
    }
}

/// An update whose merged selection would select no protected resource at all is
/// refused the missing-parameter condition: the scope it states names no
/// resource and the selection filters by no condition either.
#[tokio::test]
async fn an_update_that_selects_nothing_is_refused() {
    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "the wildcard scope emptied, the selection having no conditions",
            minimal_selection("selection_emptied"),
            json!({ "ProtectedResourceArns": [] }),
        ),
        (
            "a scope of specific ARNs emptied, the selection having no conditions",
            json!({
                "RestoreTestingSelectionName": "selection_emptied",
                "IamRoleArn": role_arn("AWSBackupRestoreTesting"),
                "ProtectedResourceType": "EBS",
                "ProtectedResourceArns": [protected_resource_arn(8)],
            }),
            json!({ "ProtectedResourceArns": [] }),
        ),
        (
            "the scope and the conditions emptied at once",
            selection_by_condition("selection_emptied"),
            json!({
                "ProtectedResourceArns": [],
                "ProtectedResourceConditions": { "StringEquals": [], "StringNotEquals": [] },
            }),
        ),
    ];

    for (case, baseline, body) in cases {
        let router = router();
        let plan = "plan_selection_update_empty";
        let name = string_at(&baseline, "RestoreTestingSelectionName");
        created_selection(&router, case, plan, &baseline).await;
        let (_, before) = get_selection(&router, plan, &name).await;

        let (status, refused) = update_selection(&router, plan, &name, &update_body(body)).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str(),
            ),
            (400, "MissingParameterValueException", SELECTS_NOTHING),
            "case {case:?}: a selection that selects nothing is refused, got {status} {refused:?}"
        );
        let (_, after) = get_selection(&router, plan, &name).await;
        assert_eq!(
            read_selection(&after),
            read_selection(&before),
            "case {case:?}: a refused update must rewrite nothing"
        );
    }
}

/// An update addressing a pair the account holds nothing under is the matching
/// not-found refusal — the PLAN's where the plan is unknown, the SELECTION's
/// where the named plan holds no such selection — and it stores nothing.
#[tokio::test]
async fn an_update_of_an_unknown_pair_is_refused() {
    let router = router();
    let plan = "plan_selection_update_present";
    created_selection(&router, "setup", plan, &minimal_selection("selection_present")).await;
    let body = update_body(json!({ "IamRoleArn": role_arn("RestoreTestingRotated") }));

    for unknown_plan in ["plan_absent", "Plan_Selection_Update_Present"] {
        let (status, refused) =
            update_selection(&router, unknown_plan, "selection_present", &body).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str(),
            ),
            (400, "ResourceNotFoundException", PLAN_NOT_FOUND),
            "case {unknown_plan:?}: a name holding no plan is the plan's not-found refusal, got \
             {status} {refused:?}"
        );
    }

    for unknown_selection in ["selection_absent", "Selection_Present", "SELECTION_PRESENT"] {
        let (status, refused) = update_selection(&router, plan, unknown_selection, &body).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str(),
            ),
            (400, "ResourceNotFoundException", SELECTION_NOT_FOUND),
            "case {unknown_selection:?}: a name the plan holds no selection under is the \
             selection's not-found refusal, got {status} {refused:?}"
        );
        let (status, read) = get_selection(&router, plan, unknown_selection).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (400, "ResourceNotFoundException"),
            "case {unknown_selection:?}: a refused update must store no selection, got {status} \
             {read:?}"
        );
    }

    // The selection the account does hold is untouched by every refusal above.
    let (status, read) = get_selection(&router, plan, "selection_present").await;
    assert_eq!(
        (status, string_at(&read_selection(&read), "IamRoleArn")),
        (200, string_at(&minimal_selection("selection_present"), "IamRoleArn")),
        "the stored selection must keep the role its create stored, got {read:?}"
    );
}
