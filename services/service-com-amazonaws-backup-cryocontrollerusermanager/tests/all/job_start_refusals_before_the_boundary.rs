// SPDX-License-Identifier: CC0-1.0

//! The refusals AWS Backup decides BEFORE the request's own input is validated, and the
//! role a started backup is performed under.
//!
//! The RULES under test, each over a table rather than one request:
//!
//! * a `StartBackupJob` that names no role to perform the backup under cannot be
//!   authorized at all, so it is refused 403 `AccessDeniedException` with the fixed
//!   sentence that class carries — ahead of the required-member refusal every OTHER absent
//!   member of the same call earns;
//! * a backup is performed by ASSUMING the role the request names, so a value naming no
//!   role this account holds leaves nothing to assume: the call is refused 400
//!   `InvalidParameterValueException` with this condition's `Code` and sentence and stores
//!   NOTHING, while a role the account does hold is accepted (what its permissions then
//!   allow decides the job's terminal state, which `backup_job_role_outcome.rs` covers);
//! * a vault nothing was stored in is therefore empty, so its delete succeeds with 200 and
//!   an empty body;
//! * `StartRestoreJob` resolves the `RecoveryPointArn` it can resolve and refuses a value
//!   that resolves to nothing with that member's own token and sentence, while a value
//!   naming no recovery point at all leaves the restore's own members to be judged first —
//!   so a start that names no role is told THAT, with the condition's own token;
//! * `StartScanJob` is fronted by AWS's request validator, so a request that omits required
//!   members or carries a value outside an enumerated set is refused in that validator's
//!   envelope and under its error type, naming every member at fault and the set each
//!   admits, and never the value that was submitted;
//! * a `ReportPlanName` outside the grammar a report plan name has is refused with the
//!   service's own sentence stating that grammar, around the name the request carried.
//!
//! Every case drives real HTTP through the router: the Backup wire is the shared harness in
//! `vault_and_backup_job.rs`, and IAM is mounted beside Backup on one engine
//! (`plan_and_selection_refusals.rs`'s `router_with_iam`) wherever the rule turns on what
//! this account's IAM holds, because that question is put to IAM over the emulator's own
//! in-process callback and the record it answers from is the one `CreateRole` wrote.

use serde_json::{json, Value};

use crate::plan_and_selection_refusals::{create_role, router_with_iam};
use crate::vault_and_backup_job::{
    call, call_reported, create_vault, error_code, start_job, ACCOUNT, REGION,
};

/// The vault the backup cases of this suite name.
const VAULT: &str = "boundary-refusal-vault";

/// The sentence AWS Backup answers a call it does not grant with — fixed text that names no
/// part of the request.
const INSUFFICIENT_PRIVILEGES: &str = "Insufficient privileges to perform this action.";

/// The `Code` and sentence of "the role this backup would be performed under is not one the
/// backup's permissions can be established from".
const ROLE_PERMISSIONS_CODE: &str = "ERROR_3020";
const ROLE_PERMISSIONS_MESSAGE: &str =
    "IAM Role does not have sufficient permissions to execute the backup";

/// The `Code` and sentence of "the restore names no IAM role to perform it under".
const ROLE_IS_NULL_CODE: &str = "ERROR_4003";
const ROLE_IS_NULL_MESSAGE: &str = "IAM Role is null";

/// The `Code` and sentence of "the recovery point this restore reads resolves to nothing".
const POINT_NOT_FOUND_CODE: &str = "ERROR_3806";
const POINT_NOT_FOUND_MESSAGE: &str = "Cannot find recovery point";

/// The service principal a backup role admits for AWS Backup to assume it. SOURCE: AWS
/// Backup's service principal, as the trust policy of its service role carries it.
const SERVICE_PRINCIPAL: &str = "backup.amazonaws.com";

/// An ARN of this emulator's account in this request's Region.
fn resource(namespace: &str, resource: &str) -> String {
    format!("arn:aws:{namespace}:{REGION}:{ACCOUNT}:{resource}")
}

/// A trust document admitting [`SERVICE_PRINCIPAL`], so a role this suite creates is one
/// AWS Backup can assume.
fn backup_trust() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": SERVICE_PRINCIPAL},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string()
}

/// The `Code` and `Message` members of a refusal body, `"<unset>"` for a member it carries
/// none of — the two members every refusal structure of this service declares.
fn refusal(body: &Value) -> (String, String) {
    let member = |name: &str| {
        body.get(name)
            .and_then(Value::as_str)
            .unwrap_or("<unset>")
            .to_string()
    };
    (member("Code"), member("Message"))
}

/// The lowercase `message` of a validator refusal, which is the member that envelope
/// carries rather than the `Message` a modelled refusal of this service does.
fn validator_message(body: &Value) -> String {
    body.get("message")
        .and_then(Value::as_str)
        .unwrap_or("<unset>")
        .to_string()
}

/// RULE: a `StartBackupJob` that names no role has no role for the backup to be performed
/// under, which AWS Backup decides at the authorization layer ahead of the request's own
/// input: it is refused 403 in the access-denied class with that class's fixed sentence,
/// whatever else the request carries — while every OTHER absent required member of the same
/// call is the parameter-value class the framework refuses, on 400.
#[tokio::test]
async fn a_backup_naming_no_role_is_refused_for_lack_of_privileges() {
    let router = router_with_iam();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );
    let table = resource("dynamodb", "table/orders");
    let volume = resource("ec2", "volume/vol-0123456789abcdef0");

    // (case, the body the start carries) — every row names NO role, and the rows differ in
    // what else the request carries: the resource kind, the optional members, and the form
    // the member is left out in.
    let roleless: Vec<(&str, String)> = vec![
        (
            "the member left out",
            json!({ "BackupVaultName": VAULT, "ResourceArn": table }).to_string(),
        ),
        (
            "an Amazon RDS cluster as the protected resource",
            json!({ "BackupVaultName": VAULT, "ResourceArn": resource("rds", "cluster:orders") })
                .to_string(),
        ),
        (
            "another kind of protected resource",
            json!({ "BackupVaultName": VAULT, "ResourceArn": volume }).to_string(),
        ),
        (
            "every optional member stated",
            json!({
                "BackupVaultName": VAULT,
                "ResourceArn": table,
                "IdempotencyToken": "no-role-token",
                "StartWindowMinutes": 120,
                "CompleteWindowMinutes": 240,
                "RecoveryPointTags": { "owner": "nightly" },
            })
            .to_string(),
        ),
        (
            "a vault this account does not hold",
            json!({ "BackupVaultName": "never-created", "ResourceArn": table }).to_string(),
        ),
        (
            "neither the role nor the vault",
            json!({ "ResourceArn": table }).to_string(),
        ),
        (
            "neither the role nor the resource",
            json!({ "BackupVaultName": VAULT }).to_string(),
        ),
        (
            "none of the three required members",
            json!({ "IdempotencyToken": "nothing-named" }).to_string(),
        ),
    ];
    for (case, body) in roleless {
        let (status, errortype, refused) = call_reported(&router, "PUT", "/backup-jobs", Some(&body))
            .await;
        let (code, message) = refusal(&refused);
        assert_eq!(
            (status, errortype.as_str(), message.as_str()),
            (403, "AccessDeniedException", INSUFFICIENT_PRIVILEGES),
            "case {case:?}: expected the access-denied refusal, got {status} {errortype} \
             {refused:?}"
        );
        assert_eq!(
            code, "<unset>",
            "case {case:?}: that class carries no Code, got {refused:?}"
        );
    }

    // The CONTRAST: the other two required members of the same call are refused as
    // parameters the request is missing, on 400.
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let absent: Vec<(&str, String)> = vec![
        (
            "no ResourceArn",
            json!({ "BackupVaultName": VAULT, "IamRoleArn": role }).to_string(),
        ),
        (
            "no BackupVaultName",
            json!({ "ResourceArn": table, "IamRoleArn": role }).to_string(),
        ),
    ];
    for (case, body) in absent {
        let (status, errortype, refused) = call_reported(&router, "PUT", "/backup-jobs", Some(&body))
            .await;
        assert_eq!(
            (status, errortype.as_str()),
            (400, "MissingParameterValueException"),
            "case {case:?}: expected the missing-parameter refusal, got {status} {errortype} \
             {refused:?}"
        );
    }

    // And nothing was stored by any of it: the vault holds no job and no recovery point, so
    // its delete succeeds with an empty body.
    let (status, jobs) = call(&router, "GET", "/backup-jobs", None).await;
    assert_eq!(
        (status, jobs["BackupJobs"].as_array().map(Vec::len)),
        (200, Some(0)),
        "expected no backup job at all, got {status} {jobs:?}"
    );
    let (status, deleted) = call(&router, "DELETE", &format!("/backup-vaults/{VAULT}"), None).await;
    assert_eq!(
        (status, &deleted),
        (200, &Value::Null),
        "expected the empty vault to delete with 200 and an empty body, got {status} \
         {deleted:?}"
    );
}

/// RULE: a backup is performed by ASSUMING the role the request names, so a value naming no
/// role this account holds leaves nothing to assume: the call is refused with this
/// condition's own `Code` and fixed sentence and stores nothing, while a role this account
/// DOES hold is accepted.
#[tokio::test]
async fn a_backup_whose_role_the_account_does_not_hold_stores_nothing() {
    let router = router_with_iam();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );
    let table = resource("dynamodb", "table/orders");
    let held = create_role(&router, "/", "boundary-held-role", &backup_trust()).await;

    // (case, the IamRoleArn the start names, whether the call is accepted) — the rows span
    // what a well-formed value can name: a role this account holds, one it does not, one of
    // another account, an IAM entity that is not a role, and a role held under a path.
    let under_a_path = create_role(
        &router,
        "/service-role/",
        "boundary-path-role",
        &backup_trust(),
    )
    .await;
    let rows: Vec<(&str, String, bool)> = vec![
        ("a role this account holds", held, true),
        ("a role this account holds under a path", under_a_path, true),
        (
            "a role this account holds nothing under",
            format!("arn:aws:iam::{ACCOUNT}:role/never-created"),
            false,
        ),
        (
            "a role of another account",
            "arn:aws:iam::210987654321:role/boundary-held-role".to_string(),
            false,
        ),
        (
            "an IAM user rather than a role",
            format!("arn:aws:iam::{ACCOUNT}:user/boundary-held-role"),
            false,
        ),
    ];
    let mut accepted = 0;
    for (case, role, admitted) in rows {
        let body = json!({
            "BackupVaultName": VAULT,
            "ResourceArn": table,
            "IamRoleArn": role,
        })
        .to_string();
        let (status, refused) = start_job(&router, &body).await;
        if admitted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the start to be accepted, got {status} {refused:?}"
            );
            accepted += 1;
            continue;
        }
        let (code, message) = refusal(&refused);
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                code.as_str(),
                message.as_str()
            ),
            (
                400,
                "InvalidParameterValueException",
                ROLE_PERMISSIONS_CODE,
                ROLE_PERMISSIONS_MESSAGE
            ),
            "case {case:?}: expected the role refusal, got {status} {refused:?}"
        );
    }

    // Only the accepted starts stored anything: a refused one minted no job, so the jobs the
    // listing reports are exactly the accepted ones.
    let (status, jobs) = call(&router, "GET", "/backup-jobs", None).await;
    assert_eq!(
        (status, jobs["BackupJobs"].as_array().map(Vec::len)),
        (200, Some(accepted)),
        "expected one job per accepted start, got {status} {jobs:?}"
    );
}

/// RULE: `StartRestoreJob` resolves the `RecoveryPointArn` it can resolve, and a value that
/// resolves to nothing is refused as that member with its own token and sentence; a value
/// that names no recovery point at all is not resolved, so such a request is judged on its
/// own members first — and a start that names no role is told THAT, with the condition's own
/// token.
#[tokio::test]
async fn a_restore_reports_the_member_it_was_refused_for() {
    let router = router_with_iam();
    let point = resource("backup", "recovery-point:11111111-2222-3333-4444-555555555555");
    let elsewhere = format!(
        "arn:aws:backup:eu-west-1:{ACCOUNT}:recovery-point:11111111-2222-3333-4444-555555555555"
    );
    let other_partition = format!(
        "arn:aws-cn:backup:{REGION}:{ACCOUNT}:recovery-point:11111111-2222-3333-4444-555555555555"
    );
    let table = resource("dynamodb", "table/orders");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/boundary-restore-role");
    let metadata = json!({ "targetTableName": "orders-restored" });

    // (case, the RecoveryPointArn, the IamRoleArn when the start names one, the expected
    // (Code, Message)) — the rows span the two classes and what separates them: a value this
    // endpoint resolves against its stored points, and a value naming no recovery point of
    // this endpoint at all (another Region's, another partition's, another service's
    // resource).
    struct Row<'a> {
        case: &'a str,
        recovery_point: &'a String,
        named_role: Option<&'a String>,
        expected: (&'a str, &'a str),
    }
    let row = |case, recovery_point, named_role, expected| Row {
        case,
        recovery_point,
        named_role,
        expected,
    };
    let rows: Vec<Row<'_>> = vec![
        row(
            "a resolvable point that resolves to nothing, with a role",
            &point,
            Some(&role),
            (POINT_NOT_FOUND_CODE, POINT_NOT_FOUND_MESSAGE),
        ),
        row(
            "a resolvable point that resolves to nothing, with no role",
            &point,
            None,
            (POINT_NOT_FOUND_CODE, POINT_NOT_FOUND_MESSAGE),
        ),
        row(
            "another Region's point, with no role",
            &elsewhere,
            None,
            (ROLE_IS_NULL_CODE, ROLE_IS_NULL_MESSAGE),
        ),
        row(
            "another partition's point, with no role",
            &other_partition,
            None,
            (ROLE_IS_NULL_CODE, ROLE_IS_NULL_MESSAGE),
        ),
        row(
            "a protected resource rather than a point, with no role",
            &table,
            None,
            (ROLE_IS_NULL_CODE, ROLE_IS_NULL_MESSAGE),
        ),
        row(
            "a protected resource rather than a point, with a role",
            &table,
            Some(&role),
            (POINT_NOT_FOUND_CODE, POINT_NOT_FOUND_MESSAGE),
        ),
    ];
    for case in rows {
        let (expected_code, expected_message) = case.expected;
        let mut body = json!({
            "RecoveryPointArn": case.recovery_point,
            "Metadata": metadata,
            "ResourceType": "DynamoDB",
        });
        if let Some(role) = case.named_role {
            body["IamRoleArn"] = json!(role);
        }
        let (status, refused) = call(&router, "PUT", "/restore-jobs", Some(&body.to_string())).await;
        let (code, message) = refusal(&refused);
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, expected_code, expected_message),
            "case {:?}: expected the refusal of that member, got {status} {refused:?}",
            case.case
        );
    }

    // And no restore job was stored by any of it.
    let (status, listed) = call(&router, "GET", "/restore-jobs", None).await;
    assert_eq!(
        (status, listed["RestoreJobs"].as_array().map(Vec::len)),
        (200, Some(0)),
        "expected no restore job at all, got {status} {listed:?}"
    );
}

/// RULE: `StartScanJob` is fronted by AWS's request validator: a request that breaks what
/// the model declares is refused in that validator's envelope (`N validation error(s)
/// detected: <clause>; …`) and under its error type, with one clause per member at fault
/// naming the member and the requirement — the set an enumerated member admits included,
/// and the value that was submitted never.
#[tokio::test]
async fn a_scan_job_is_refused_in_the_validators_own_envelope() {
    let router = router_with_iam();
    let point = resource("backup", "recovery-point:11111111-2222-3333-4444-555555555555");
    let role = format!("arn:aws:iam::{ACCOUNT}:role/boundary-scan-role");
    // Every member the model requires of this input, as a start that breaks ONE of them
    // states the rest. SOURCE: smithy_query(query="operation", operation="StartScanJob").
    let complete = json!({
        "BackupVaultName": VAULT,
        "IamRoleArn": role,
        "MalwareScanner": "GUARDDUTY",
        "RecoveryPointArn": point,
        "ScanMode": "FULL_SCAN",
        "ScannerRoleArn": role,
    });

    // (case, the body, the clauses the envelope must carry) — the rows span the two classes
    // the validator refuses: a value outside an enumerated set (for each of the two
    // enumerated members) and members left out (EACH of the six on its own, and all six at
    // once, which is also what pins the member list the absent ones are read against
    // against what the input shape itself requires).
    let mut out_of_set_mode = complete.clone();
    out_of_set_mode["ScanMode"] = json!("PARTIAL_SCAN");
    let mut out_of_set_scanner = complete.clone();
    out_of_set_scanner["MalwareScanner"] = json!("MACIE");
    // Every member the model requires, by its own name and by the path a clause names it by.
    let required = [
        ("BackupVaultName", "backupVaultName"),
        ("IamRoleArn", "iamRoleArn"),
        ("MalwareScanner", "malwareScanner"),
        ("RecoveryPointArn", "recoveryPointArn"),
        ("ScanMode", "scanMode"),
        ("ScannerRoleArn", "scannerRoleArn"),
    ];
    let mut rows: Vec<(String, Value, Vec<String>)> = vec![
        (
            "a scan mode outside the set the member admits".to_string(),
            out_of_set_mode,
            vec![
                "Value at 'scanMode' failed to satisfy constraint: Member must satisfy enum \
                 value set: [INCREMENTAL_SCAN, FULL_SCAN]"
                    .to_string(),
            ],
        ),
        (
            "a scanner outside the set the member admits".to_string(),
            out_of_set_scanner,
            vec![
                "Value at 'malwareScanner' failed to satisfy constraint: Member must satisfy \
                 enum value set: [GUARDDUTY]"
                    .to_string(),
            ],
        ),
        (
            "every required member left out".to_string(),
            json!({}),
            required
                .iter()
                .map(|(_, path)| {
                    format!(
                        "Value at '{path}' failed to satisfy constraint: Member must not be null"
                    )
                })
                .collect(),
        ),
    ];
    for (member, path) in required {
        let mut without = complete.clone();
        without
            .as_object_mut()
            .expect("an object body")
            .remove(member);
        rows.push((
            format!("only {member} left out"),
            without,
            vec![format!(
                "Value at '{path}' failed to satisfy constraint: Member must not be null"
            )],
        ));
    }
    for (case, body, clauses) in rows {
        let case = case.as_str();
        let (status, errortype, refused) =
            call_reported(&router, "PUT", "/scan/job", Some(&body.to_string())).await;
        let message = validator_message(&refused);
        assert_eq!(
            (status, errortype.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the validator's own refusal, got {status} {errortype} \
             {refused:?}"
        );
        let count = clauses.len();
        let envelope = if count == 1 {
            "1 validation error detected: ".to_string()
        } else {
            format!("{count} validation errors detected: ")
        };
        assert!(
            message.starts_with(&envelope),
            "case {case:?}: expected the envelope {envelope:?}, got {message:?}"
        );
        for clause in &clauses {
            assert!(
                message.contains(clause.as_str()),
                "case {case:?}: expected the clause {clause:?}, got {message:?}"
            );
        }
        for submitted in ["PARTIAL_SCAN", "MACIE", "null at"] {
            assert!(
                !message.contains(submitted),
                "case {case:?}: the validator states no submitted value, got {message:?}"
            );
        }
        if count > 1 {
            // The ORDER is the validator's own, which is neither the model's member order
            // (`BackupVaultName` first) nor lexicographic: the recorded listing of this
            // operation's six required members opens with these two clauses.
            assert!(
                message.starts_with(&format!(
                    "{envelope}Value at 'malwareScanner' failed to satisfy constraint: Member \
                     must not be null; Value at 'recoveryPointArn' failed to satisfy \
                     constraint: Member must not be null; "
                )),
                "case {case:?}: expected the validator's own clause order, got {message:?}"
            );
        }
    }
}

/// RULE: a `ReportPlanName` outside the grammar a report plan name has is refused with the
/// service's own sentence, which states that grammar around the name the request carried;
/// a name INSIDE that grammar is judged on the plans this account holds instead.
#[tokio::test]
async fn a_malformed_report_plan_name_is_refused_with_the_grammar_it_breaks() {
    let router = router_with_iam();

    // (case, the name the request carries, whether the grammar admits it) — the rows span
    // what the member's own `@length(1, 256)` over `@pattern(^[a-zA-Z][_a-zA-Z0-9]*$)`
    // admits: each published bound, one past it, and each way the alphabet can be broken.
    let longest = "a".repeat(256);
    let too_long = "a".repeat(257);
    let rows: Vec<(&str, &str, bool)> = vec![
        ("a leading digit", "9sim2real_probe_plan", false),
        ("a leading underscore", "_leading_underscore", false),
        ("a character outside the alphabet", "plan-with-hyphen", false),
        ("one character past the length bound", too_long.as_str(), false),
        ("the shortest name the grammar admits", "a", true),
        ("the longest name the grammar admits", longest.as_str(), true),
        ("a name of the whole alphabet", "Plan_9_report", true),
    ];
    for (case, name, admitted) in rows {
        let path = format!("/audit/report-jobs/{name}");
        let (status, errortype, refused) = call_reported(&router, "POST", &path, None).await;
        let (code, message) = refusal(&refused);
        if admitted {
            // A name the grammar admits is a name no plan was created under, which is the
            // not-found class of this operation rather than this refusal.
            assert_eq!(
                (status, errortype.as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected the plan to be reported absent, got {status} \
                 {errortype} {refused:?}"
            );
            continue;
        }
        assert_eq!(
            (status, errortype.as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?}: expected the parameter-value refusal, got {status} {errortype} \
             {refused:?}"
        );
        assert_eq!(
            message,
            format!(
                "Invalid report plan name {name}. Report plan names must be between 1 and 256 \
                 characters, start with a letter (A-Z and a-z), and consist only of letters, \
                 numbers (0-9), and underscores (_). Type a valid report plan name."
            ),
            "case {case:?}: expected the sentence stating the grammar, got {refused:?}"
        );
        assert_eq!(
            code, "<unset>",
            "case {case:?}: this condition carries no Code, got {refused:?}"
        );
    }
}
