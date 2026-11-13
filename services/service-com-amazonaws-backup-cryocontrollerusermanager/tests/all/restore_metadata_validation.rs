// SPDX-License-Identifier: CC0-1.0

//! The restore METADATA a `StartRestoreJob` has to carry for the type of resource it
//! restores.
//!
//! The RULE under test: the `Metadata` of a restore is an INPUT to it, so a start whose
//! metadata carries no value for a key the resource type's restore requires is refused 400
//! `InvalidParameterValueException` with the restore-metadata sentence for that key, and
//! nothing is persisted — no restore job is stored, none is readable and none is listed for
//! that recovery point — while a start that carries it is accepted and stores the one job
//! every read of that id answers from.
//!
//! The table spans what the rule generalises over rather than one request: the metadata the
//! start carries (none at all, another key, the required key present but empty, a key whose
//! name merely contains the required one, the key as the restore metadata spells it, and the
//! same key in another casing, because the metadata of a recovery point is reported in the
//! spelling the owning service uses and a client passes it back as it received it) and the
//! resource TYPE being restored, since a type whose restore requires no particular key is
//! accepted with any metadata at all.
//!
//! Every case drives real HTTP through the router; the harness is the shared one in
//! `vault_and_backup_job.rs`, with the engine-keeping router of `recovery_point_deletion.rs`
//! so a case can see that a refused start wrote nothing.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_point_deletion::{completed_point, router_with_engine};
use crate::restore_job::stored_restore_jobs;
use crate::vault_and_backup_job::{call, create_vault, error_code, string_at, ACCOUNT, REGION};

/// The vault every recovery point of this suite is backed up into.
const VAULT: &str = "restore-metadata-source";

/// One row: the metadata a start carries, the type it restores, and whether that start is
/// one the service accepts.
struct Case {
    name: &'static str,
    /// The `ResourceType` the start names, which is what selects the metadata it requires.
    resource_type: &'static str,
    /// The `Metadata` the start carries.
    metadata: Value,
    /// Whether the start is accepted.
    accepted: bool,
}

/// The table.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a table restore whose metadata names another key",
            resource_type: "DynamoDB",
            metadata: json!({"NotARealKey": "x"}),
            accepted: false,
        },
        Case {
            name: "a table restore carrying no metadata at all",
            resource_type: "DynamoDB",
            metadata: json!({}),
            accepted: false,
        },
        Case {
            name: "a table restore whose target table name is empty",
            resource_type: "DynamoDB",
            metadata: json!({"targetTableName": ""}),
            accepted: false,
        },
        Case {
            name: "a table restore whose key merely contains the required one",
            resource_type: "DynamoDB",
            metadata: json!({"targetTableNameSuffix": "restored"}),
            accepted: false,
        },
        Case {
            name: "a table restore carrying the key as the restore metadata spells it",
            resource_type: "DynamoDB",
            metadata: json!({"targetTableName": "orders-restored"}),
            accepted: true,
        },
        Case {
            name: "a table restore carrying the same key in another casing",
            resource_type: "DynamoDB",
            metadata: json!({"TargetTableName": "orders-restored"}),
            accepted: true,
        },
        Case {
            name: "a file-system restore, whose type requires no such key",
            resource_type: "EFS",
            metadata: json!({"NotARealKey": "x"}),
            accepted: true,
        },
    ]
}

/// The role a restore would run as.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// The refusal AWS words a restore whose metadata names no target table with: the fixed
/// sentences, the type being restored and the recovery point the request named.
fn refusal_message(resource_type: &str, recovery_point_arn: &str) -> String {
    format!(
        "Invalid restore metadata. Target table name must be provided with restore metadata. \
         Restore metadata validation failed for {resource_type} with recoveryPoint \
         {recovery_point_arn}."
    )
}

/// `PUT /restore-jobs`, as the model binds the start.
async fn start_restore(
    router: &MultiServiceRouter,
    recovery_point_arn: &str,
    resource_type: &str,
    metadata: &Value,
) -> (u16, Value) {
    let body = json!({
        "RecoveryPointArn": recovery_point_arn,
        "ResourceType": resource_type,
        "Metadata": metadata,
        "IamRoleArn": role_arn(),
    })
    .to_string();
    call(router, "PUT", "/restore-jobs", Some(&body)).await
}

/// The restore jobs `ListRestoreJobs` reports for `recovery_point_arn`.
async fn listed_for(router: &MultiServiceRouter, recovery_point_arn: &str) -> Vec<String> {
    let (status, listed) = call(router, "GET", "/restore-jobs", None).await;
    assert_eq!(
        status, 200,
        "expected ListRestoreJobs 200, got {status} {listed:?}"
    );
    listed["RestoreJobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|job| string_at(job, "RecoveryPointArn") == recovery_point_arn)
        .map(|job| string_at(job, "RestoreJobId"))
        .collect()
}

/// RULE: a start whose `Metadata` carries no value for a key the resource type's restore
/// requires is refused as that member's value and stores nothing; one that carries it is
/// accepted and stores the job every read of it answers from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restore_metadata_missing_a_required_key_is_refused_and_stores_nothing() {
    for case in cases() {
        let (router, engine) = router_with_engine();
        let (status, created) = create_vault(&router, VAULT, "{}").await;
        assert_eq!(
            status, 200,
            "case {:?}: setup CreateBackupVault should succeed, got {status} {created:?}",
            case.name
        );
        // A point of the row's own type: the type the ARN's service namespace gives is what
        // a start that names none would restore, and the row names the same one.
        let resource = match case.resource_type {
            "EFS" => format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-01"),
            _ => format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/orders"),
        };
        let point = completed_point(&router, VAULT, &resource).await;

        let (status, answered) =
            start_restore(&router, &point, case.resource_type, &case.metadata).await;
        if case.accepted {
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (200, "<none>"),
                "case {:?}: expected the start to be accepted, got {status} {answered:?}",
                case.name
            );
            let job_id = string_at(&answered, "RestoreJobId");
            assert_eq!(
                stored_restore_jobs(&engine),
                1,
                "case {:?}: an accepted start stores exactly one restore job",
                case.name
            );
            // The job the start stored is the job a read answers from, and the one the
            // listing reports for that recovery point.
            let (status, read) =
                call(&router, "GET", &format!("/restore-jobs/{job_id}"), None).await;
            assert_eq!(
                (status, string_at(&read, "RestoreJobId").as_str()),
                (200, job_id.as_str()),
                "case {:?}: the stored job must be readable, got {status} {read:?}",
                case.name
            );
            assert_eq!(
                listed_for(&router, &point).await,
                vec![job_id],
                "case {:?}: the listing must report exactly the job that was started",
                case.name
            );
        } else {
            assert_eq!(
                (
                    status,
                    error_code(&answered).as_str(),
                    string_at(&answered, "Message").as_str()
                ),
                (
                    400,
                    "InvalidParameterValueException",
                    refusal_message(case.resource_type, &point).as_str()
                ),
                "case {:?}: expected the restore-metadata refusal, got {status} {answered:?}",
                case.name
            );
            assert_eq!(
                stored_restore_jobs(&engine),
                0,
                "case {:?}: a refused start must persist no restore job",
                case.name
            );
            assert!(
                listed_for(&router, &point).await.is_empty(),
                "case {:?}: a refused start leaves the listing nothing to report for that \
                 recovery point",
                case.name
            );
        }
    }
}
