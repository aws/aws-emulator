// SPDX-License-Identifier: CC0-1.0

//! `CreateRestoreTestingSelection` and the read over what it stores
//! (`GetRestoreTestingSelection`).
//!
//! The rules under test:
//!
//! * a restore testing selection is a STORED resource of ONE restore testing
//!   plan, addressed by its name within that plan: the body a create submits is
//!   what every later read of that pair reports, and the same name under another
//!   plan is another selection;
//! * the body is stored as submitted, except for the three members every
//!   selection HAS rather than optionally states — the resources it is scoped
//!   to, the conditions it filters those resources by and the restore metadata
//!   keys it overrides — which a read reports as the collections the selection
//!   holds: the specific ARNs stated or the documented wildcard where none were,
//!   and the empty filter and empty overrides map where the create stated
//!   neither;
//! * "this request can be sent after CreateRestoreTestingPlan request returns
//!   successfully" (model doc for the operation), so a create naming a plan the
//!   account does not hold is the plan's not-found refusal and stores nothing;
//! * the (plan, name) pair identifies one selection, so a second create of it is
//!   refused as already existing, and a repeated `CreatorRequestId` takes no
//!   second action: it answers with the selection that token created;
//! * a value outside what the model documents for a selection body — the name's
//!   alphabet and length, a role that is no ARN, the wildcard mixed with
//!   specific ARNs, specific ARNs stated beside conditions, a validation window
//!   past its published bound — is refused `InvalidParameterValueException`
//!   naming the member it rejected; more specific ARNs than the documented
//!   maximum is `LimitExceededException`; a body that selects no protected
//!   resource at all is `MissingParameterValueException`;
//! * a plan or selection name nothing was stored under is the matching
//!   not-found refusal.
//!
//! Every case drives real HTTP through the router; the harness is the shared one
//! in `vault_and_backup_job.rs`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{call, error_code, router, string_at, ACCOUNT, REGION};

/// The sentence this service states for "the restore testing plan asked about is
/// not one this account holds" — fixed text, naming no part of the request.
const PLAN_NOT_FOUND: &str = "Restore testing plan does not exist.";

/// The sentence this service states for "the restore testing selection asked
/// about is not one the named plan holds" — fixed text, naming no part of the
/// request, as the plan's own not-found sentence is.
const SELECTION_NOT_FOUND: &str = "Restore testing selection does not exist.";

/// The sentence this service states for a create naming a selection the plan
/// already holds: the plan and the selection the request named, and the account
/// the call was made in.
fn selection_already_exists(plan: &str, selection: &str) -> String {
    format!("Restore testing plan [{plan}] in account [{ACCOUNT}] already contains selection [{selection}].")
}

/// The sentence this service states for a create whose selection selects no
/// resource at all.
const SELECTS_NOTHING: &str = "Missing Resource Selection.";

/// The most specific protected resource ARNs one selection may name: "you can
/// include up to 30 specific protected resource ARNs in ProtectedResourceArns"
/// (model doc for `CreateRestoreTestingSelection`).
const MAX_PROTECTED_RESOURCE_ARNS: usize = 30;

/// The published bounds of a validation window: "This is amount of hours (0 to
/// 168) available to run a validation script on the data" (model doc for
/// `RestoreTestingSelectionForCreate$ValidationWindowHours`).
const MIN_VALIDATION_WINDOW_HOURS: i64 = 0;
const MAX_VALIDATION_WINDOW_HOURS: i64 = 168;

/// The most characters a `RestoreTestingSelectionName` may carry: "The name
/// consists of only alphanumeric characters and underscores. Maximum length is
/// 50" (model doc for the member).
const MAX_NAME_LENGTH: usize = 50;

/// `PUT /restore-testing/plans/{RestoreTestingPlanName}/selections`, as the
/// model binds the create.
///
/// Shared with the sibling suite that drives this surface's refusals, so there
/// is ONE spelling of each selection route in this test crate.
pub(crate) async fn create_selection(
    router: &MultiServiceRouter,
    plan: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/restore-testing/plans/{plan}/selections"),
        Some(&body.to_string()),
    )
    .await
}

/// `GET /restore-testing/plans/{RestoreTestingPlanName}/selections/{RestoreTestingSelectionName}`.
pub(crate) async fn get_selection(router: &MultiServiceRouter, plan: &str, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/restore-testing/plans/{plan}/selections/{name}"),
        None,
    )
    .await
}

/// A create body carrying `selection` as its `RestoreTestingSelection`.
pub(crate) fn create_body(selection: Value) -> Value {
    json!({ "RestoreTestingSelection": selection })
}

/// The selection body a read reports (`RestoreTestingSelection`), or
/// `Value::Null`.
pub(crate) fn read_selection(body: &Value) -> Value {
    body.get("RestoreTestingSelection")
        .cloned()
        .unwrap_or(Value::Null)
}

/// An integer member of a body, or the modelled default of a member the response
/// may legitimately omit.
fn int_or(body: &Value, member: &str, default: i64) -> i64 {
    body.get(member).and_then(Value::as_i64).unwrap_or(default)
}

/// The role a restore test runs as, in the form the model documents ("for
/// example: `arn:aws:iam::123456789012:role/S3Access`").
pub(crate) fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// A protected resource ARN of this account, `index` of a series, so a list of
/// them can be sized up to the documented maximum.
///
/// Shared with the sibling suites that drive this surface, for the reason given
/// on [`create_selection`].
pub(crate) fn protected_resource_arn(index: usize) -> String {
    format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-{index:017x}")
}

/// A name of `length` characters from the alphabet the model documents for a
/// `RestoreTestingSelectionName` ("only alphanumeric characters and
/// underscores"), so the length bound is exercised across that whole alphabet.
pub(crate) fn name_of_length(length: usize) -> String {
    const ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_";
    (0..length)
        .map(|i| char::from(ALPHABET[(i * 7 + length) % ALPHABET.len()]))
        .collect()
}

/// The resource SCOPE a stored selection carries, as a read reports it: the
/// specific ARNs the request named, and the documented wildcard where it named
/// none — "you can include specific ARNs … or you can include a wildcard:
/// `ProtectedResourceArns: ["*"]`" (model doc for
/// `RestoreTestingSelectionForGet$ProtectedResourceArns`), so a selection that
/// names no specific ARN is scoped to every protected resource of its type.
pub(crate) fn completed_scope(stated: Option<&Value>) -> Value {
    match stated.and_then(Value::as_array) {
        Some(arns) if !arns.is_empty() => json!(arns),
        _ => json!(["*"]),
    }
}

/// The conditions filter a STORED selection carries, as a read reports it: the
/// structure the request stated with both of its lists materialised, and the
/// filter that matches on nothing where it stated none.
pub(crate) fn completed_conditions(stated: Option<&Value>) -> Value {
    let stated = stated.cloned().unwrap_or_else(|| json!({}));
    json!({
        "StringEquals": stated
            .get("StringEquals")
            .cloned()
            .unwrap_or_else(|| json!([])),
        "StringNotEquals": stated
            .get("StringNotEquals")
            .cloned()
            .unwrap_or_else(|| json!([])),
    })
}

/// The restore metadata overrides a STORED selection carries: the map the
/// request stated, and the empty map where it stated none.
pub(crate) fn completed_overrides(stated: Option<&Value>) -> Value {
    stated.cloned().unwrap_or_else(|| json!({}))
}

/// The smallest selection body the documented rules admit: a name, a role, a
/// protected resource type and a wildcard naming the resources it restores.
pub(crate) fn minimal_selection(name: &str) -> Value {
    json!({
        "RestoreTestingSelectionName": name,
        "IamRoleArn": role_arn("AWSBackupRestoreTesting"),
        "ProtectedResourceType": "EBS",
        "ProtectedResourceArns": ["*"],
    })
}

/// The smallest plan body the model admits, under which the selections below are
/// stored.
pub(crate) fn minimal_plan(name: &str) -> Value {
    json!({
        "RestoreTestingPlan": {
            "RestoreTestingPlanName": name,
            "ScheduleExpression": "cron(0 5 ? * * *)",
            "RecoveryPointSelection": {
                "Algorithm": "LATEST_WITHIN_WINDOW",
                "RecoveryPointTypes": ["SNAPSHOT"],
                "IncludeVaults": ["*"],
            },
        }
    })
}

/// Store a plan and return the ARN the service reports for it.
pub(crate) async fn stored_plan(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "PUT",
        "/restore-testing/plans",
        Some(&minimal_plan(name).to_string()),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: CreateRestoreTestingPlan should succeed, got {created:?}"
    );
    string_at(&created, "RestoreTestingPlanArn")
}

/// A created selection round-trips: the read reports the body that was stored,
/// under the plan that holds it, with the create's own creation instant.
///
/// The rows differ along what the rule generalises over: how the resources are
/// selected (the wildcard, one specific ARN, the documented maximum of specific
/// ARNs, conditions beside the wildcard), the protected resource type, the
/// name's length (1 and the published maximum of 50), whether metadata
/// overrides and a creator token are stated, and the validation window — absent,
/// at the floor of its range and at its published ceiling.
#[tokio::test]
async fn a_created_restore_testing_selection_round_trips_through_the_read() {
    let cases: Vec<(&str, Value)> = vec![
        (
            "a wildcard selection stating no optional member",
            minimal_selection("selection_defaults"),
        ),
        (
            "one specific ARN, the window at the floor of its range",
            json!({
                "RestoreTestingSelectionName": name_of_length(1),
                "IamRoleArn": role_arn("restore-testing/S3Access"),
                "ProtectedResourceType": "S3",
                "ProtectedResourceArns": [protected_resource_arn(1)],
                "ValidationWindowHours": MIN_VALIDATION_WINDOW_HOURS,
            }),
        ),
        (
            "the documented maximum of specific ARNs, the window at its ceiling",
            json!({
                "RestoreTestingSelectionName": name_of_length(MAX_NAME_LENGTH),
                "IamRoleArn": role_arn("AWSBackupDefaultServiceRole"),
                "ProtectedResourceType": "DynamoDB",
                "ProtectedResourceArns": (0..MAX_PROTECTED_RESOURCE_ARNS)
                    .map(protected_resource_arn)
                    .collect::<Vec<String>>(),
                "ValidationWindowHours": MAX_VALIDATION_WINDOW_HOURS,
            }),
        ),
        (
            "conditions beside the wildcard, with metadata overrides",
            json!({
                "RestoreTestingSelectionName": "selection_conditions",
                "IamRoleArn": role_arn("RestoreTesting"),
                "ProtectedResourceType": "EC2",
                "ProtectedResourceArns": ["*"],
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "restore-testing", "Value": "yes" }],
                    "StringNotEquals": [{ "Key": "stage", "Value": "prod" }],
                },
                "RestoreMetadataOverrides": { "instanceType": "t3.micro" },
                "ValidationWindowHours": 24,
            }),
        ),
        (
            "conditions only, an empty ARN list stated beside them",
            json!({
                "RestoreTestingSelectionName": "selection_by_tag",
                "IamRoleArn": role_arn("RestoreTestingByTag"),
                "ProtectedResourceType": "EFS",
                "ProtectedResourceArns": [],
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "team", "Value": "backup" }],
                },
            }),
        ),
    ];

    for (case, selection) in cases {
        let router = router();
        let plan_arn = stored_plan(&router, "plan_for_selections").await;
        let name = string_at(&selection, "RestoreTestingSelectionName");
        let mut body = create_body(selection.clone());
        body["CreatorRequestId"] = json!(format!("token-{name}"));

        let (status, created) = create_selection(&router, "plan_for_selections", &body).await;
        assert_eq!(
            (status, error_code(&created).as_str()),
            (201, "<none>"),
            "case {case:?}: expected CreateRestoreTestingSelection 201, got {status} {created:?}"
        );
        assert_eq!(
            (
                string_at(&created, "RestoreTestingPlanName"),
                string_at(&created, "RestoreTestingPlanArn"),
                string_at(&created, "RestoreTestingSelectionName"),
            ),
            (
                "plan_for_selections".to_string(),
                plan_arn.clone(),
                name.clone(),
            ),
            "case {case:?}: the create must answer with the selection it stored, got {created:?}"
        );
        assert!(
            created.get("CreationTime").is_some(),
            "case {case:?}: the create reports when it stored the selection, got {created:?}"
        );

        let (status, read) = get_selection(&router, "plan_for_selections", &name).await;
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
                string_at(&stored, "IamRoleArn"),
                string_at(&stored, "ProtectedResourceType"),
                string_at(&stored, "CreationTime"),
                string_at(&stored, "CreatorRequestId"),
            ),
            (
                "plan_for_selections".to_string(),
                name.clone(),
                string_at(&selection, "IamRoleArn"),
                string_at(&selection, "ProtectedResourceType"),
                string_at(&created, "CreationTime"),
                format!("token-{name}"),
            ),
            "case {case:?}: the read must report what the create stored, got {read:?}"
        );
        assert_eq!(
            stored.get("ProtectedResourceArns").cloned(),
            Some(completed_scope(selection.get("ProtectedResourceArns"))),
            "case {case:?}: the resource scope is reported as the specific ARNs the create named, \
             or the wildcard where it named none, got {stored:?}"
        );
        assert_eq!(
            (
                stored.get("ProtectedResourceConditions").cloned(),
                stored.get("RestoreMetadataOverrides").cloned(),
            ),
            (
                Some(completed_conditions(
                    selection.get("ProtectedResourceConditions")
                )),
                Some(completed_overrides(
                    selection.get("RestoreMetadataOverrides")
                )),
            ),
            "case {case:?}: the conditions filter and the overrides map are reported as the \
             collections the selection holds, got {stored:?}"
        );
        assert_eq!(
            int_or(&stored, "ValidationWindowHours", MIN_VALIDATION_WINDOW_HOURS),
            selection
                .get("ValidationWindowHours")
                .and_then(Value::as_i64)
                .unwrap_or(MIN_VALIDATION_WINDOW_HOURS),
            "case {case:?}: a stated validation window is kept and an absent one is the \
             modelled default, got {stored:?}"
        );
    }
}

/// What a read reports for each of the three collections of a selection: they
/// are members every stored selection HAS, so the read reports them as the
/// collections the selection holds — the conditions filter and the overrides map
/// as the empty collection where the create stated neither, and the resource
/// scope as the specific ARNs the create named or the documented wildcard where
/// it named none.
///
/// The rows span the three members, each as the empty collection of its own
/// kind, as an unstated member and as a stated collection with an entry in it,
/// with the other means of selecting resources kept constant so each row differs
/// only in the member under test.
#[tokio::test]
async fn a_read_reports_every_collection_the_selection_holds() {
    let empty_conditions = json!({ "StringEquals": [], "StringNotEquals": [] });
    let cases: Vec<(&str, &str, Option<Value>, Option<Value>)> = vec![
        (
            "an empty ARN list",
            "ProtectedResourceArns",
            Some(json!([])),
            Some(json!(["*"])),
        ),
        (
            "no ARN list at all",
            "ProtectedResourceArns",
            None,
            Some(json!(["*"])),
        ),
        (
            "one specific ARN",
            "ProtectedResourceArns",
            Some(json!([protected_resource_arn(7)])),
            Some(json!([protected_resource_arn(7)])),
        ),
        (
            "an empty conditions structure",
            "ProtectedResourceConditions",
            Some(json!({})),
            Some(empty_conditions.clone()),
        ),
        (
            "an empty condition list inside it",
            "ProtectedResourceConditions",
            Some(json!({ "StringEquals": [] })),
            Some(empty_conditions.clone()),
        ),
        (
            "no conditions at all",
            "ProtectedResourceConditions",
            None,
            Some(empty_conditions.clone()),
        ),
        (
            "a condition on one tag",
            "ProtectedResourceConditions",
            Some(json!({ "StringNotEquals": [{ "Key": "stage", "Value": "prod" }] })),
            Some(json!({
                "StringEquals": [],
                "StringNotEquals": [{ "Key": "stage", "Value": "prod" }],
            })),
        ),
        (
            "an empty overrides map",
            "RestoreMetadataOverrides",
            Some(json!({})),
            Some(json!({})),
        ),
        (
            "no overrides at all",
            "RestoreMetadataOverrides",
            None,
            Some(json!({})),
        ),
        (
            "an override of one metadata key",
            "RestoreMetadataOverrides",
            Some(json!({ "instanceType": "t3.micro" })),
            Some(json!({ "instanceType": "t3.micro" })),
        ),
    ];

    for (case, member, stated, expected) in cases {
        let router = router();
        stored_plan(&router, "plan_empty_vs_absent").await;
        let name = "selection_empty_vs_absent";
        // Every row keeps a selection of resources the documented rules admit —
        // the wildcard, or a condition where the row empties the ARN list — so
        // the only difference between two rows is the member under test.
        let mut selection = minimal_selection(name);
        // A row that empties or drops the resource scope keeps a condition, so
        // the selection still selects resources the documented rules admit; a
        // row naming a specific ARN states none beside it ("cannot select by
        // both protected resource types AND specific ARNs").
        let names_specific_arns = stated
            .as_ref()
            .and_then(Value::as_array)
            .is_some_and(|arns| !arns.is_empty());
        if member == "ProtectedResourceArns" && !names_specific_arns {
            selection["ProtectedResourceConditions"] = json!({
                "StringEquals": [{ "Key": "team", "Value": "backup" }],
            });
        }
        match stated.clone() {
            Some(value) => selection[member] = value,
            None => {
                selection.as_object_mut().expect("an object body").remove(member);
            }
        }

        let (status, created) =
            create_selection(&router, "plan_empty_vs_absent", &create_body(selection)).await;
        assert_eq!(
            (status, error_code(&created).as_str()),
            (201, "<none>"),
            "case {case:?}: the create should succeed, got {status} {created:?}"
        );

        let (status, read) = get_selection(&router, "plan_empty_vs_absent", name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the selection must be readable, got {read:?}"
        );
        let stored_member = read_selection(&read).get(member).cloned();
        assert_eq!(
            stored_member, expected,
            "case {case:?}: {member} must be reported as the collection the selection holds, got \
             {read:?}"
        );
    }
}

/// A selection name identifies one selection OF ONE PLAN: a second create of the
/// pair is refused as already existing, while the same name under another plan is
/// another selection with its own body.
#[tokio::test]
async fn a_selection_name_identifies_one_selection_of_one_plan() {
    let router = router();
    let first_arn = stored_plan(&router, "plan_one").await;
    let second_arn = stored_plan(&router, "plan_two").await;

    let (status, created) =
        create_selection(&router, "plan_one", &create_body(minimal_selection("shared"))).await;
    assert_eq!(status, 201, "the first create should succeed, got {created:?}");

    let (status, refused) =
        create_selection(&router, "plan_one", &create_body(minimal_selection("shared"))).await;
    assert_eq!(
        (
            status,
            error_code(&refused).as_str(),
            string_at(&refused, "Message").as_str()
        ),
        (
            400,
            "AlreadyExistsException",
            selection_already_exists("plan_one", "shared").as_str()
        ),
        "a second create of one (plan, name) pair is refused, got {status} {refused:?}"
    );

    // The SAME name under the OTHER plan is a different selection, so it is
    // stored, and each plan's read reports its own body.
    let mut other = minimal_selection("shared");
    other["ProtectedResourceType"] = json!("RDS");
    let (status, created_other) = create_selection(&router, "plan_two", &create_body(other)).await;
    assert_eq!(
        (status, string_at(&created_other, "RestoreTestingPlanArn")),
        (201, second_arn.clone()),
        "the same name under another plan is another selection, got {created_other:?}"
    );
    assert_ne!(
        first_arn, second_arn,
        "the two plans must be two resources for this case to mean anything"
    );

    for (plan, expected_type) in [("plan_one", "EBS"), ("plan_two", "RDS")] {
        let (status, read) = get_selection(&router, plan, "shared").await;
        assert_eq!(
            (status, string_at(&read_selection(&read), "ProtectedResourceType")),
            (200, expected_type.to_string()),
            "case {plan:?}: each plan's read reports its own selection, got {read:?}"
        );
    }
}

/// A repeated `CreatorRequestId` takes no second action: it answers with the
/// selection that token created, even when the body it carries differs, and the
/// plan still holds that one selection.
#[tokio::test]
async fn a_repeated_creator_request_id_replays_the_first_create() {
    let router = router();
    stored_plan(&router, "plan_replay").await;

    let mut body = create_body(minimal_selection("selection_replayed"));
    body["CreatorRequestId"] = json!("one-selection-only");
    let (status, first) = create_selection(&router, "plan_replay", &body).await;
    assert_eq!(status, 201, "the first create should succeed, got {first:?}");

    let mut retried = create_body(minimal_selection("selection_replayed_other"));
    retried["CreatorRequestId"] = json!("one-selection-only");
    let (status, replayed) = create_selection(&router, "plan_replay", &retried).await;
    assert_eq!(
        (status, error_code(&replayed).as_str()),
        (201, "<none>"),
        "a replayed CreatorRequestId must answer as the first call did, got {status} {replayed:?}"
    );
    assert_eq!(
        (
            string_at(&first, "RestoreTestingSelectionName"),
            string_at(&first, "RestoreTestingPlanArn"),
            string_at(&first, "CreationTime")
        ),
        (
            string_at(&replayed, "RestoreTestingSelectionName"),
            string_at(&replayed, "RestoreTestingPlanArn"),
            string_at(&replayed, "CreationTime")
        ),
        "a replay must report the identifiers the first call reported"
    );
    let (status, read) = get_selection(&router, "plan_replay", "selection_replayed_other").await;
    assert_eq!(
        (
            status,
            error_code(&read).as_str(),
            string_at(&read, "Message").as_str()
        ),
        (400, "ResourceNotFoundException", SELECTION_NOT_FOUND),
        "a replay must store no second selection, got {status} {read:?}"
    );
}

/// A selection body outside what the model documents is refused, with the class
/// of refusal the condition belongs to and the member it rejected — and nothing
/// is stored for it.
///
/// The rows span the documented rules, each at one step past its bound, alongside
/// the member the refusal has to name.
#[tokio::test]
async fn a_selection_body_outside_the_documented_rules_is_refused() {
    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "a name one character past the length bound",
            json!({ "RestoreTestingSelectionName": name_of_length(MAX_NAME_LENGTH + 1) }),
            "RestoreTestingSelectionName",
        ),
        (
            "a name carrying a character outside the alphabet",
            json!({ "RestoreTestingSelectionName": "selection-hyphenated" }),
            "RestoreTestingSelectionName",
        ),
        (
            "an empty name",
            json!({ "RestoreTestingSelectionName": "" }),
            "RestoreTestingSelectionName",
        ),
        (
            "an empty protected resource type",
            json!({ "ProtectedResourceType": "" }),
            "ProtectedResourceType",
        ),
        (
            "a role that is not an ARN",
            json!({ "IamRoleArn": "AWSBackupDefaultServiceRole" }),
            "IamRoleArn",
        ),
        (
            "a role ARN short of its six fields",
            json!({ "IamRoleArn": format!("arn:aws:iam::{ACCOUNT}") }),
            "IamRoleArn",
        ),
        (
            "the wildcard mixed into a list of specific ARNs",
            json!({ "ProtectedResourceArns": ["*", protected_resource_arn(2)] }),
            "ProtectedResourceArns",
        ),
        (
            "specific ARNs stated beside conditions",
            json!({
                "ProtectedResourceArns": [protected_resource_arn(3)],
                "ProtectedResourceConditions": {
                    "StringEquals": [{ "Key": "team", "Value": "backup" }],
                },
            }),
            "ProtectedResourceArns",
        ),
        (
            "a validation window one hour past its ceiling",
            json!({ "ValidationWindowHours": MAX_VALIDATION_WINDOW_HOURS + 1 }),
            "ValidationWindowHours",
        ),
        (
            "a negative validation window",
            json!({ "ValidationWindowHours": -1 }),
            "ValidationWindowHours",
        ),
    ];

    for (case, overrides, member) in cases {
        let router = router();
        stored_plan(&router, "plan_refusals").await;
        let mut selection = minimal_selection("selection_refused");
        for (key, value) in overrides.as_object().expect("an object of overrides") {
            selection[key.as_str()] = value.clone();
        }
        let name = string_at(&selection, "RestoreTestingSelectionName");

        let (status, refused) =
            create_selection(&router, "plan_refusals", &create_body(selection)).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str(),
                string_at(&refused, "Context").as_str()
            ),
            (
                400,
                "InvalidParameterValueException",
                format!("Parameter {member} is invalid.").as_str(),
                member
            ),
            "case {case:?}: expected the refusal to name {member}, got {status} {refused:?}"
        );
        if name.is_empty() {
            // An empty name addresses no path the read is bound to, so the
            // "nothing was stored" half is asserted for every other row.
            continue;
        }
        let (status, read) = get_selection(&router, "plan_refusals", &name).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused create stores nothing, got {status} {read:?}"
        );
    }
}

/// More specific protected resource ARNs than the documented maximum is past "a
/// maximum number of items allowed in a request", which is the condition the
/// modelled `LimitExceededException` states; one step below the maximum, and the
/// maximum itself, are stored.
#[tokio::test]
async fn more_specific_arns_than_the_documented_maximum_is_refused() {
    for count in [
        MAX_PROTECTED_RESOURCE_ARNS - 1,
        MAX_PROTECTED_RESOURCE_ARNS,
        MAX_PROTECTED_RESOURCE_ARNS + 1,
        MAX_PROTECTED_RESOURCE_ARNS + 7,
    ] {
        let router = router();
        stored_plan(&router, "plan_arn_count").await;
        let mut selection = minimal_selection("selection_arn_count");
        selection["ProtectedResourceArns"] = json!((0..count)
            .map(protected_resource_arn)
            .collect::<Vec<String>>());

        let (status, answer) =
            create_selection(&router, "plan_arn_count", &create_body(selection)).await;
        if count <= MAX_PROTECTED_RESOURCE_ARNS {
            assert_eq!(
                (status, error_code(&answer).as_str()),
                (201, "<none>"),
                "case {count} ARNs: within the documented maximum must be stored, got {status} \
                 {answer:?}"
            );
            continue;
        }
        assert_eq!(
            (status, error_code(&answer).as_str()),
            (400, "LimitExceededException"),
            "case {count} ARNs: past the documented maximum must be refused, got {status} \
             {answer:?}"
        );
        let (status, read) = get_selection(&router, "plan_arn_count", "selection_arn_count").await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (400, "ResourceNotFoundException"),
            "case {count} ARNs: a refused create stores nothing, got {status} {read:?}"
        );
    }
}

/// A body that selects NO protected resource — by neither ARNs nor conditions —
/// is refused the missing-parameter condition, whichever way it states nothing.
#[tokio::test]
async fn a_selection_that_selects_nothing_is_refused() {
    let cases: Vec<(&str, Value)> = vec![
        ("neither member stated", json!({})),
        ("an empty ARN list alone", json!({ "ProtectedResourceArns": [] })),
        (
            "an empty conditions structure alone",
            json!({ "ProtectedResourceConditions": {} }),
        ),
        (
            "two empty condition lists",
            json!({
                "ProtectedResourceArns": [],
                "ProtectedResourceConditions": {
                    "StringEquals": [],
                    "StringNotEquals": [],
                },
            }),
        ),
    ];

    for (case, overrides) in cases {
        let router = router();
        stored_plan(&router, "plan_selects_nothing").await;
        let mut selection = minimal_selection("selection_selects_nothing");
        selection
            .as_object_mut()
            .expect("an object body")
            .remove("ProtectedResourceArns");
        for (key, value) in overrides.as_object().expect("an object of overrides") {
            selection[key.as_str()] = value.clone();
        }

        let (status, refused) =
            create_selection(&router, "plan_selects_nothing", &create_body(selection)).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "MissingParameterValueException", SELECTS_NOTHING),
            "case {case:?}: a selection selecting nothing is refused, got {status} {refused:?}"
        );
        let (status, read) =
            get_selection(&router, "plan_selects_nothing", "selection_selects_nothing").await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (400, "ResourceNotFoundException"),
            "case {case:?}: a refused create stores nothing, got {status} {read:?}"
        );
    }
}

/// A plan the account holds no restore testing plan under has nothing to store a
/// selection in — the create is refused the plan's own not-found condition, and a
/// read under it answers the same, so "no such plan" is told apart from "no such
/// selection".
#[tokio::test]
async fn a_selection_of_an_unknown_plan_is_refused() {
    let router = router();
    stored_plan(&router, "plan_present").await;
    let (status, created) =
        create_selection(&router, "plan_present", &create_body(minimal_selection("present"))).await;
    assert_eq!(status, 201, "setup: the create should succeed, got {created:?}");

    for plan in ["plan_absent", "plan_Present", "PLAN_PRESENT"] {
        let (status, refused) =
            create_selection(&router, plan, &create_body(minimal_selection("present"))).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "ResourceNotFoundException", PLAN_NOT_FOUND),
            "case {plan:?}: a create under an unknown plan is refused, got {status} {refused:?}"
        );
        let (status, read) = get_selection(&router, plan, "present").await;
        assert_eq!(
            (
                status,
                error_code(&read).as_str(),
                string_at(&read, "Message").as_str()
            ),
            (400, "ResourceNotFoundException", PLAN_NOT_FOUND),
            "case {plan:?}: a read under an unknown plan is refused, got {status} {read:?}"
        );
    }

    for name in ["absent", "Present", "PRESENT"] {
        let (status, read) = get_selection(&router, "plan_present", name).await;
        assert_eq!(
            (
                status,
                error_code(&read).as_str(),
                string_at(&read, "Message").as_str()
            ),
            (400, "ResourceNotFoundException", SELECTION_NOT_FOUND),
            "case {name:?}: a name the plan holds no selection under is not found, got {status} \
             {read:?}"
        );
    }
}
