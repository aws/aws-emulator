// SPDX-License-Identifier: CC0-1.0

//! `StopBackupJob`: cancelling a backup job, and the conditions a cancellation is refused
//! for.
//!
//! The RULE under test: a stop cancels a backup job that has NOT come to rest — the job
//! leaves the state it was running in for the terminal `ABORTED` state the clock then owes
//! it, it reports why it ended, it transfers nothing, and the recovery point its backup was
//! writing is left in no vault, since a cancelled backup produces none. Three conditions
//! refuse the call and write nothing: a job that has already come to rest (the modelled
//! `InvalidRequestException`, whose sentence names the state the stop would have moved the
//! job into), a job whose protected resource is of one of the types the operation's own
//! documentation says it does not support (the same class, naming that type), and an id this
//! account holds no job under (the not-found condition every reader of a backup job states
//! the same way).
//!
//! Each table spans what the rule generalises over rather than one request: which supported
//! resource type the cancelled backup was of, HOW the job came to rest before the stop (the
//! completion the clock owed it, and a cancellation that already finished), which form the
//! unknown id takes, and — for the unsupported types — every type the documentation's list
//! resolves to beside two it admits.
//!
//! Every case drives real HTTP through the router: the Backup wire is the shared harness in
//! `vault_and_backup_job.rs`.

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{
    call, create_vault, error_code, router, settled_backup_job, start_job, string_at, ACCOUNT,
    REGION,
};

/// `BackupJobState` of a job whose cancellation is under way, and the one it comes to rest in
/// (model enum `BackupJobState`).
const ABORTING: &str = "ABORTING";
const ABORTED: &str = "ABORTED";

/// The `StatusMessage` a cancelled job reports: the fixed sentence AWS states for a job the
/// caller stopped.
const STOPPED_BY_USER: &str = "Backup job was stopped by user.";

/// The role every job of this suite runs with: what these cases test is the cancellation, not
/// the outcome a role decides.
fn role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// `StopBackupJob` for one job id (`POST /backup-jobs/{BackupJobId}`, no body).
async fn stop_job(router: &MultiServiceRouter, job_id: &str) -> (u16, Value) {
    call(router, "POST", &format!("/backup-jobs/{job_id}"), None).await
}

/// `DescribeBackupJob` for one job id.
async fn describe_job(router: &MultiServiceRouter, job_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/backup-jobs/{job_id}"), None).await
}

/// Start one job of `resource_arn` into `vault`, answering its `BackupJobId` and the ARN of
/// the recovery point the start says it writes.
async fn started_job(
    router: &MultiServiceRouter,
    case: &str,
    vault: &str,
    resource_arn: &str,
) -> (String, String) {
    let payload = serde_json::json!({
        "BackupVaultName": vault,
        "ResourceArn": resource_arn,
        "IamRoleArn": role(),
    });
    let (status, started) = start_job(router, &payload.to_string()).await;
    assert_eq!(
        status, 200,
        "setup {case:?}: expected StartBackupJob 200, got {status} {started:?}"
    );
    (
        string_at(&started, "BackupJobId"),
        string_at(&started, "RecoveryPointArn"),
    )
}

/// How many recovery points the vault holds, as the vault's own read and its listing report
/// them — the two readers that must agree a cancelled backup produced none.
async fn held_recovery_points(
    router: &MultiServiceRouter,
    case: &str,
    vault: &str,
) -> (i64, Vec<String>) {
    let (status, described) = call(router, "GET", &format!("/backup-vaults/{vault}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeBackupVault 200, got {status} {described:?}"
    );
    let (status, listed) = call(
        router,
        "GET",
        &format!("/backup-vaults/{vault}/recovery-points"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRecoveryPointsByBackupVault 200, got {status} {listed:?}"
    );
    let arns = listed["RecoveryPoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect();
    (
        described
            .get("NumberOfRecoveryPoints")
            .and_then(Value::as_i64)
            .unwrap_or_default(),
        arns,
    )
}

/// A stopped job is cancelled: it reports the cancellation at once, comes to rest `ABORTED`
/// having transferred nothing, and the recovery point its backup was writing is held by no
/// vault.
///
/// The rows vary the TYPE of the protected resource the cancelled backup was of — every one
/// of them a type the operation supports — so the cancellation is not keyed to one resource
/// form.
#[tokio::test]
async fn a_stopped_job_is_cancelled_and_its_backup_leaves_no_recovery_point() {
    let cases: Vec<(&str, String)> = vec![
        (
            "a DynamoDB table",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
        ),
        (
            "an EBS volume",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-0123456789abcdef0"),
        ),
        ("an S3 bucket", "arn:aws:s3:::demo-bucket".to_string()),
    ];

    for (case, resource_arn) in cases {
        let router = router();
        let (status, created) = create_vault(&router, "cancel-vault", "{}").await;
        assert_eq!(
            status, 200,
            "setup {case:?}: expected CreateBackupVault 200, got {status} {created:?}"
        );
        let (job_id, point_arn) = started_job(&router, case, "cancel-vault", &resource_arn).await;

        let (status, stopped) = stop_job(&router, &job_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected StopBackupJob 200, got {status} {stopped:?}"
        );

        // The cancellation is in force from the call that asked for it: the job is on its way
        // out, whether or not the clock has already made the `ABORTED` it owes due.
        let (status, described) = describe_job(&router, &job_id).await;
        let state = string_at(&described, "State");
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBackupJob 200, got {status} {described:?}"
        );
        assert!(
            state == ABORTING || state == ABORTED,
            "case {case:?}: expected the job to be cancelled, got State {state:?} in {described:?}"
        );
        assert_eq!(
            string_at(&described, "StatusMessage"),
            STOPPED_BY_USER,
            "case {case:?}: a cancelled job reports why it ended, got {described:?}"
        );

        // A cancelled backup produces no recovery point, so the vault that would have held it
        // holds nothing — by its own count and by its listing alike.
        let (count, held) = held_recovery_points(&router, case, "cancel-vault").await;
        assert_eq!(
            (count, held.as_slice()),
            (0, [].as_slice()),
            "case {case:?}: expected the vault to hold no recovery point after the \
             cancellation, got {count} and {held:?} (the start answered {point_arn:?})"
        );

        // The terminal state falls due on the clock, so the job is looked at again until it
        // reports the one the cancellation owes it.
        let settled = settled_backup_job(&router, case, &job_id).await;
        assert_eq!(
            string_at(&settled, "State"),
            ABORTED,
            "case {case:?}: expected State ABORTED, got {settled:?}"
        );
        assert_eq!(
            settled.get("BytesTransferred").and_then(Value::as_i64),
            Some(0),
            "case {case:?}: a cancelled backup transferred nothing, got {settled:?}"
        );
        assert!(
            settled.get("CompletionDate").is_some(),
            "case {case:?}: a job that has come to rest reports when, got {settled:?}"
        );

        // The listing and the read of one job answer from the same stored record.
        let (status, listed) = call(&router, "GET", "/backup-jobs", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected ListBackupJobs 200, got {status} {listed:?}"
        );
        let states: Vec<String> = listed["BackupJobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter(|job| string_at(job, "BackupJobId") == job_id)
            .map(|job| string_at(job, "State"))
            .collect();
        assert_eq!(
            states,
            vec![ABORTED.to_string()],
            "case {case:?}: expected the listing to report the cancelled job once, \
             got {listed:?}"
        );
    }
}

/// A job that has already come to rest cannot be cancelled: the call is refused with the
/// modelled `InvalidRequestException`, whose sentence states that the move the stop asked for
/// cannot be applied, and the job is left exactly as it was.
///
/// The rows vary HOW the job came to rest: the completion the clock owed it, and a
/// cancellation that has already finished.
#[tokio::test]
async fn a_job_that_has_come_to_rest_cannot_be_cancelled() {
    // (case, whether the job is cancelled before it is left to come to rest, the state it
    // comes to rest in)
    let cases: [(&str, bool, &str); 2] = [
        ("a job the clock completed", false, "COMPLETED"),
        ("a job already cancelled", true, ABORTED),
    ];

    for (case, cancel_first, resting) in cases {
        let router = router();
        let (status, created) = create_vault(&router, "rested-vault", "{}").await;
        assert_eq!(
            status, 200,
            "setup {case:?}: expected CreateBackupVault 200, got {status} {created:?}"
        );
        let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
        let (job_id, _) = started_job(&router, case, "rested-vault", &resource_arn).await;
        if cancel_first {
            let (status, stopped) = stop_job(&router, &job_id).await;
            assert_eq!(
                status, 200,
                "setup {case:?}: expected the first StopBackupJob 200, got {status} {stopped:?}"
            );
        }
        let settled = settled_backup_job(&router, case, &job_id).await;
        assert_eq!(
            string_at(&settled, "State"),
            resting,
            "setup {case:?}: expected the job to come to rest {resting}, got {settled:?}"
        );

        let (status, refused) = stop_job(&router, &job_id).await;
        assert_eq!(
            (
                status,
                error_code(&refused),
                string_at(&refused, "Message")
            ),
            (
                400,
                "InvalidRequestException".to_string(),
                format!("Cannot update the backup job to {ABORTING} state")
            ),
            "case {case:?}: expected the stop to be refused for the job's state, \
             got {status} {refused:?}"
        );

        // The refused call wrote nothing: the job still reports the state it had.
        let (status, described) = describe_job(&router, &job_id).await;
        assert_eq!(
            (status, string_at(&described, "State")),
            (200, resting.to_string()),
            "case {case:?}: a refused stop leaves the job as it was, got {status} {described:?}"
        );
    }
}

/// An id this account holds no backup job under names nothing to cancel: the call is refused
/// as absent, with the fixed sentence AWS states for that condition ON THIS OPERATION, which
/// echoes no part of the request — a stop words it its own way rather than as a read of a
/// backup job does.
///
/// The rows vary the FORM of the id (the form this emulator mints, a bare word, a long hex
/// string, a single digit), and the same emulator cancels the id it did mint, so the refusal
/// is derived from what is stored rather than answered for everything.
#[tokio::test]
async fn an_id_naming_no_stored_job_is_refused_as_absent() {
    let router = router();
    let (status, created) = create_vault(&router, "absent-vault", "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );
    let resource_arn = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");

    let unknown = [
        (
            "the minted form, never minted",
            "db89c7cd-0000-4000-8000-000000000001",
        ),
        ("a bare word", "no-such-job"),
        ("a long hex string", "0123456789abcdef0123456789abcdef"),
        ("a single digit", "7"),
    ];
    for (case, job_id) in unknown {
        let (status, refused) = stop_job(&router, job_id).await;
        assert_eq!(
            (
                status,
                error_code(&refused),
                string_at(&refused, "Message")
            ),
            (
                400,
                "ResourceNotFoundException".to_string(),
                "BackupJob not found".to_string()
            ),
            "case {case:?} ({job_id:?}): expected 400 ResourceNotFoundException with the \
             fixed sentence, got {status} {refused:?}"
        );
        assert!(
            !string_at(&refused, "Message").contains(job_id),
            "case {case:?} ({job_id:?}): the sentence must not echo the requested id, \
             got {refused:?}"
        );
    }

    let (job_id, _) = started_job(
        &router,
        "the id this emulator minted",
        "absent-vault",
        &resource_arn,
    )
    .await;
    let (status, stopped) = stop_job(&router, &job_id).await;
    assert_eq!(
        status, 200,
        "case \"the id this emulator minted\": expected StopBackupJob 200, got {status} \
         {stopped:?}"
    );
}

/// The operation "is not supported for the following services" (its model doc: Amazon Aurora,
/// Amazon DocumentDB, Amazon FSx, Amazon Neptune, SAP HANA databases on Amazon EC2 instances
/// and Amazon RDS), so a job taken of a resource of one of those types is refused naming that
/// type rather than reported as cancelled — and the job keeps running.
///
/// The rows are every type that list resolves to through this service's own ARN-to-resource-
/// type reading, beside two types it admits. SAP HANA on Amazon EC2 is absent for one reason:
/// the model's `ResourceType` pattern forbids the space in that spelling, so no ARN resolves
/// to it and no stored job can carry it.
#[tokio::test]
async fn the_resource_types_the_operation_does_not_support_are_refused() {
    // (case, the protected resource's ARN, the resource type it resolves to, whether a stop of
    // its job is supported)
    let cases: Vec<(&str, String, &str, bool)> = vec![
        (
            "an Aurora cluster",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:demo-cluster"),
            "Aurora",
            false,
        ),
        (
            "an RDS instance",
            format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:demo-instance"),
            "RDS",
            false,
        ),
        (
            "a DocumentDB cluster",
            format!("arn:aws:docdb:{REGION}:{ACCOUNT}:cluster:demo-docs"),
            "DocumentDB",
            false,
        ),
        (
            "a Neptune cluster",
            format!("arn:aws:neptune:{REGION}:{ACCOUNT}:cluster:demo-graph"),
            "Neptune",
            false,
        ),
        (
            "an FSx file system",
            format!("arn:aws:fsx:{REGION}:{ACCOUNT}:file-system/fs-0123456789abcdef0"),
            "FSx",
            false,
        ),
        (
            "a DynamoDB table",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
            "DynamoDB",
            true,
        ),
        (
            "an EFS file system",
            format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-00000001"),
            "EFS",
            true,
        ),
    ];

    for (case, resource_arn, resource_type, supported) in cases {
        let router = router();
        let (status, created) = create_vault(&router, "support-vault", "{}").await;
        assert_eq!(
            status, 200,
            "setup {case:?}: expected CreateBackupVault 200, got {status} {created:?}"
        );
        let (job_id, _) = started_job(&router, case, "support-vault", &resource_arn).await;
        let (status, described) = describe_job(&router, &job_id).await;
        assert_eq!(
            (status, string_at(&described, "ResourceType")),
            (200, resource_type.to_string()),
            "setup {case:?}: expected the job's ResourceType, got {status} {described:?}"
        );

        let (status, answered) = stop_job(&router, &job_id).await;
        if supported {
            assert_eq!(
                status, 200,
                "case {case:?} ({resource_type}): expected StopBackupJob 200, got {status} \
                 {answered:?}"
            );
            continue;
        }
        assert_eq!(
            (
                status,
                error_code(&answered),
                string_at(&answered, "Message")
            ),
            (
                400,
                "InvalidRequestException".to_string(),
                format!("StopBackupJob is not supported for resource type {resource_type}")
            ),
            "case {case:?}: expected the stop to be refused for the resource type, \
             got {status} {answered:?}"
        );

        // The refused call cancelled nothing: the job is still heading for the backup it was
        // taking, or has already finished it.
        let (status, described) = describe_job(&router, &job_id).await;
        let state = string_at(&described, "State");
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBackupJob 200, got {status} {described:?}"
        );
        assert!(
            state != ABORTING && state != ABORTED,
            "case {case:?}: a refused stop cancels nothing, got State {state:?} in {described:?}"
        );
    }
}
