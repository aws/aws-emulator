// SPDX-License-Identifier: CC0-1.0

//! `StartCopyJob` / `DescribeCopyJob`: a copy job is a STORED resource, and both
//! ends of it are resolved against the store.
//!
//! The rules under test:
//!
//! * `StartCopyJob` stores a copy job AND the recovery point the copy writes into
//!   the vault its `DestinationBackupVaultArn` names, so `DescribeCopyJob`, the
//!   destination vault's recovery-point list and its `NumberOfRecoveryPoints` all
//!   read the one stored pair, and the copy's own ARN carries the destination
//!   vault's region;
//! * the copy job is reported in its transient `CREATED` state until the span the
//!   clock owes it has passed, and `COMPLETED` with a completion instant after;
//! * every way the (`SourceBackupVaultName`, `RecoveryPointArn`) pair can fail to
//!   name a stored recovery point is ONE condition, refused 400
//!   `ResourceNotFoundException` with the same sentence, and a destination vault
//!   this account does not hold is refused the same way;
//! * a `DestinationBackupVaultArn` that is not a backup vault's ARN is a bad
//!   parameter value rather than an absent resource, and an absent required member
//!   is a missing one;
//! * a replayed `IdempotencyToken` returns the first copy job rather than a second,
//!   and the same token on a DIFFERENT call is not a replay of it.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire
//! behaviour a `backup` SDK client sees. The request harness is the shared one in
//! `vault_and_backup_job.rs`.
//!
//! This emulator holds ONE account's vaults, keyed by Region AND
//! `BackupVaultName`, so a destination ARN that names another Region reaches the
//! vault of THAT Region; the region in that ARN also decides where the copy it
//! writes belongs, which is what the cross-region rows below assert.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{
    call, call_in_region, create_vault, create_vault_in, error_code, router, start_job, string_at,
    ACCOUNT, PAST_SETTLE, REGION,
};

/// The source vault every case backs a resource up into.
const SOURCE_VAULT: &str = "copy-source";

/// The destination vault every case copies into.
const DESTINATION_VAULT: &str = "copy-destination";

/// The role a copy job is run with.
fn role() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// The ARN of a backup vault of this account in `region`, as the model documents
/// the form ("for example,
/// `arn:aws:backup:us-east-1:123456789012:backup-vault:aBackupVault`").
fn vault_arn(region: &str, name: &str) -> String {
    format!("arn:aws:backup:{region}:{ACCOUNT}:backup-vault:{name}")
}

/// `PUT /copy-jobs`, as the model binds `StartCopyJob`.
async fn start_copy(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/copy-jobs", Some(&body.to_string())).await
}

/// `GET /copy-jobs/{CopyJobId}`, as the model binds `DescribeCopyJob`.
async fn describe_copy(router: &MultiServiceRouter, copy_job_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/copy-jobs/{copy_job_id}"), None).await
}

/// The `CopyJob` body a describe reports.
async fn copy_job(router: &MultiServiceRouter, copy_job_id: &str) -> Value {
    let (status, described) = describe_copy(router, copy_job_id).await;
    assert_eq!(
        status, 200,
        "DescribeCopyJob must answer 200 for a stored job, got {status} {described:?}"
    );
    described
        .get("CopyJob")
        .cloned()
        .unwrap_or(Value::Null)
}

/// The recovery points a vault reports, through the implemented list read.
async fn listed_points(router: &MultiServiceRouter, region: &str, vault: &str) -> Vec<Value> {
    let (status, listed) = call_in_region(
        router,
        region,
        "GET",
        &format!("/backup-vaults/{vault}/recovery-points"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListRecoveryPointsByBackupVault must answer 200, got {status} {listed:?}"
    );
    listed["RecoveryPoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `NumberOfRecoveryPoints` as the vault read reports it.
async fn counted_points(router: &MultiServiceRouter, region: &str, vault: &str) -> i64 {
    let (_, described) =
        call_in_region(router, region, "GET", &format!("/backup-vaults/{vault}"), None).await;
    described
        .get("NumberOfRecoveryPoints")
        .and_then(Value::as_i64)
        .unwrap_or(-1)
}

/// Create the source vault in [`REGION`] and the destination vault in the
/// Region the case's destination ARN names: a vault is identified by its name
/// within ONE Region, so the destination of a cross-Region copy has to exist in
/// the Region that ARN carries.
async fn create_both_vaults(router: &MultiServiceRouter, destination_region: &str) {
    let (status, created) = create_vault(router, SOURCE_VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup {SOURCE_VAULT:?}: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let (status, created) =
        create_vault_in(router, destination_region, DESTINATION_VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup {DESTINATION_VAULT:?} in {destination_region}: CreateBackupVault should \
         succeed, got {status} {created:?}"
    );
}

/// Back `resource_arn` up into the source vault and return the recovery-point ARN
/// the job stored, once the backup has settled so the point is addressable.
async fn settled_source_point(router: &MultiServiceRouter, resource_arn: &str) -> String {
    let body = json!({
        "BackupVaultName": SOURCE_VAULT,
        "ResourceArn": resource_arn,
        "IamRoleArn": role(),
    });
    let (status, started) = start_job(router, &body.to_string()).await;
    assert_eq!(
        status, 200,
        "setup {resource_arn:?}: StartBackupJob should succeed, got {status} {started:?}"
    );
    let arn = string_at(&started, "RecoveryPointArn");
    std::thread::sleep(PAST_SETTLE);
    arn
}

/// A started copy job is STORED: the describe reads it back, the copy lands in the
/// destination vault with an ARN carrying that vault's region, and the vault's own
/// reads follow.
///
/// The rows differ along what the rule generalises over: the protected resource's
/// service, the lifecycle the copy is kept under (none, a day count, a transition
/// plus a deletion, and the documented keep-indefinitely sentinel) and the region
/// the destination ARN names — this request's, and two others.
#[tokio::test]
async fn a_started_copy_job_is_stored_and_read_back_from_the_destination_vault() {
    // (case, protected resource, the `Lifecycle` the copy is kept under, the
    // region the `DestinationBackupVaultArn` names)
    let cases: Vec<(&str, String, Option<Value>, &str)> = vec![
        (
            "a DynamoDB table, no lifecycle, same region",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
            None,
            REGION,
        ),
        (
            "an EFS file system kept 120 days",
            format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-01"),
            Some(json!({"DeleteAfterDays": 120})),
            REGION,
        ),
        (
            "an S3 bucket copied to another region, cold then deleted",
            "arn:aws:s3:::demo-bucket".to_string(),
            Some(json!({"MoveToColdStorageAfterDays": 30, "DeleteAfterDays": 120})),
            "us-east-1",
        ),
        (
            "an EBS volume copied to a third region, kept indefinitely",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-01"),
            Some(json!({"DeleteAfterDays": -1})),
            "eu-west-1",
        ),
    ];

    for (case, resource_arn, lifecycle, destination_region) in cases {
        let router = router();
        create_both_vaults(&router, destination_region).await;
        let source_point = settled_source_point(&router, &resource_arn).await;
        let destination_arn = vault_arn(destination_region, DESTINATION_VAULT);

        let mut request = json!({
            "RecoveryPointArn": source_point,
            "SourceBackupVaultName": SOURCE_VAULT,
            "DestinationBackupVaultArn": destination_arn,
            "IamRoleArn": role(),
        });
        if let Some(lifecycle) = lifecycle.clone() {
            request["Lifecycle"] = lifecycle;
        }
        let (status, started) = start_copy(&router, &request).await;
        assert_eq!(
            (status, error_code(&started).as_str()),
            (200, "<none>"),
            "case {case:?}: expected StartCopyJob 200, got {status} {started:?}"
        );
        let copy_job_id = string_at(&started, "CopyJobId");
        assert!(
            !copy_job_id.is_empty(),
            "case {case:?}: expected a generated CopyJobId, got {started:?}"
        );
        assert!(
            started.get("CreationDate").is_some(),
            "case {case:?}: expected a CreationDate, got {started:?}"
        );
        assert_eq!(
            started.get("IsParent").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a copy of one recovery point is not a parent, got {started:?}"
        );

        // Straight after the call the copy is still being written.
        let job = copy_job(&router, &copy_job_id).await;
        assert_eq!(
            job.get("State").and_then(Value::as_str),
            Some("CREATED"),
            "case {case:?}: expected State CREATED before the settle span, got {job:?}"
        );
        assert!(
            job.get("CompletionDate").is_none(),
            "case {case:?}: an unsettled copy job has no CompletionDate, got {job:?}"
        );
        // Everything reported comes from the stored job and the stored vaults.
        let destination_point = string_at(&job, "DestinationRecoveryPointArn");
        for (member, expected) in [
            ("CopyJobId", copy_job_id.clone()),
            ("AccountId", ACCOUNT.to_string()),
            ("SourceBackupVaultArn", vault_arn(REGION, SOURCE_VAULT)),
            ("SourceRecoveryPointArn", source_point.clone()),
            ("DestinationBackupVaultArn", destination_arn.clone()),
            ("DestinationVaultType", "BACKUP_VAULT".to_string()),
            ("ResourceArn", resource_arn.clone()),
            ("IamRoleArn", role()),
        ] {
            assert_eq!(
                string_at(&job, member),
                expected,
                "case {case:?}: expected {member} {expected:?}, got {job:?}"
            );
        }
        assert!(
            destination_point.starts_with(&format!(
                "arn:aws:backup:{destination_region}:{ACCOUNT}:recovery-point:"
            )),
            "case {case:?}: the copy belongs to the destination region, got {destination_point:?}"
        );
        assert_eq!(
            job.get("DestinationRecoveryPointLifecycle").cloned(),
            lifecycle.clone(),
            "case {case:?}: expected the lifecycle the request asked for, got {job:?}"
        );
        assert_eq!(
            job.get("IsParent").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: expected IsParent false, got {job:?}"
        );
        // A vault created with no key of its own encrypts with the key this
        // service made for the account, which a copy into it reports no key for.
        assert!(
            job.get("DestinationEncryptionKeyArn").is_none(),
            "case {case:?}: a default-key vault reports no key on the copy, got {job:?}"
        );

        std::thread::sleep(PAST_SETTLE);

        let settled = copy_job(&router, &copy_job_id).await;
        assert_eq!(
            settled.get("State").and_then(Value::as_str),
            Some("COMPLETED"),
            "case {case:?}: expected State COMPLETED after the settle span, got {settled:?}"
        );
        assert!(
            settled.get("CompletionDate").is_some(),
            "case {case:?}: a settled copy job reports when it completed, got {settled:?}"
        );
        assert_eq!(
            string_at(&settled, "DestinationRecoveryPointArn"),
            destination_point,
            "case {case:?}: the stored copy keeps its ARN, got {settled:?}"
        );

        // The destination vault holds the copy, and its own reads agree.
        let points = listed_points(&router, destination_region, DESTINATION_VAULT).await;
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected the one copied recovery point, got {points:?}"
        );
        assert_eq!(
            string_at(&points[0], "RecoveryPointArn"),
            destination_point,
            "case {case:?}: the listed point is the copy the job wrote, got {points:?}"
        );
        assert_eq!(
            points[0].get("Status").and_then(Value::as_str),
            Some("COMPLETED"),
            "case {case:?}: the settled copy is COMPLETED, got {points:?}"
        );
        assert_eq!(
            string_at(&points[0], "ResourceArn"),
            resource_arn,
            "case {case:?}: the copy protects the same resource, got {points:?}"
        );
        assert_eq!(
            counted_points(&router, destination_region, DESTINATION_VAULT).await,
            1,
            "case {case:?}: the destination vault counts the copy it holds"
        );
        assert_eq!(
            counted_points(&router, REGION, SOURCE_VAULT).await,
            1,
            "case {case:?}: the source vault still holds exactly its own backup"
        );
    }
}

/// Every way the source pair can fail to name a stored recovery point, and every
/// destination ARN that names a vault this account does not hold, is ONE
/// condition: 400 `ResourceNotFoundException` — and nothing is copied.
#[tokio::test]
async fn a_copy_that_names_no_stored_resource_is_refused_as_not_found() {
    let router = router();
    create_both_vaults(&router, REGION).await;
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = settled_source_point(&router, &resource).await;
    // A backup started and NOT waited out, so a case can address a recovery point
    // whose backup has not finished.
    let (status, running) = start_job(
        &router,
        &json!({
            "BackupVaultName": SOURCE_VAULT,
            "ResourceArn": resource,
            "IamRoleArn": role(),
        })
        .to_string(),
    )
    .await;
    assert_eq!(status, 200, "setup: the second StartBackupJob should succeed");
    let unfinished = string_at(&running, "RecoveryPointArn");

    let fabricated = format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:does-not-exist");
    let destination = vault_arn(REGION, DESTINATION_VAULT);
    // A source `RecoveryPointArn` of ANOTHER ACCOUNT is deliberately not a row
    // here: the authorization decision refuses it before the source is resolved,
    // which `recovery_point_cross_account_authorization.rs` covers.
    // (case, SourceBackupVaultName, RecoveryPointArn, DestinationBackupVaultArn,
    //  the Message the refusal carries)
    let cases: Vec<(&str, &str, &str, String, &str)> = vec![
        (
            "a recovery-point ARN nothing answers to",
            SOURCE_VAULT,
            fabricated.as_str(),
            destination.clone(),
            "Cannot find recovery point",
        ),
        (
            "a point another vault holds",
            DESTINATION_VAULT,
            stored.as_str(),
            destination.clone(),
            "Cannot find recovery point",
        ),
        (
            "a source vault this account does not hold",
            "never-created",
            stored.as_str(),
            destination.clone(),
            "Cannot find recovery point",
        ),
        (
            "a point whose backup has not finished",
            SOURCE_VAULT,
            unfinished.as_str(),
            destination.clone(),
            "Cannot find recovery point",
        ),
        (
            "a destination vault this account does not hold",
            SOURCE_VAULT,
            stored.as_str(),
            vault_arn(REGION, "no-such-vault"),
            "Backup vault no-such-vault not found",
        ),
        (
            "a destination vault of another account",
            SOURCE_VAULT,
            stored.as_str(),
            format!("arn:aws:backup:{REGION}:111122223333:backup-vault:{DESTINATION_VAULT}"),
            "Backup vault copy-destination not found",
        ),
    ];

    for (case, source_vault, point_arn, destination_arn, message) in cases {
        let (status, refused) = start_copy(
            &router,
            &json!({
                "RecoveryPointArn": point_arn,
                "SourceBackupVaultName": source_vault,
                "DestinationBackupVaultArn": destination_arn,
                "IamRoleArn": role(),
            }),
        )
        .await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "ResourceNotFoundException", message),
            "case {case:?}: expected the not-found refusal, got {status} {refused:?}"
        );
        let points = listed_points(&router, REGION, DESTINATION_VAULT).await;
        assert!(
            points.is_empty(),
            "case {case:?}: a refused copy must store nothing, got {points:?}"
        );
    }
}

/// A `DestinationBackupVaultArn` that is not a backup vault's ARN is a bad
/// parameter value — not an absent resource — and an absent required member is a
/// missing one. Neither copies anything.
#[tokio::test]
async fn a_destination_that_is_not_a_backup_vault_arn_is_a_bad_parameter_value() {
    let router = router();
    create_both_vaults(&router, REGION).await;
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = settled_source_point(&router, &resource).await;

    // The form a vault ARN takes:
    // `arn:<partition>:backup:<region>:<account>:backup-vault:<name>`, as the
    // model's own doc for `DestinationBackupVaultArn` spells it out
    // (smithy_query(query="operation", operation="StartCopyJob") types it as `ARN`
    // with no pattern, so the service applies the rule), with the vault name in
    // the form `BackupVaultName` admits (`^[a-zA-Z0-9\-\_]{2,50}$`).
    let cases: Vec<(&str, String)> = vec![
        ("no arn scheme", "not-an-arn".to_string()),
        ("too few fields", "arn:aws:backup".to_string()),
        (
            "an empty partition",
            format!("arn::backup:{REGION}:{ACCOUNT}:backup-vault:{DESTINATION_VAULT}"),
        ),
        (
            "another service's ARN",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:backup-vault:{DESTINATION_VAULT}"),
        ),
        (
            "another Backup resource's ARN",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7"),
        ),
        (
            "a vault name the pattern refuses",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:a"),
        ),
        (
            "a region field that is not a region",
            format!("arn:aws:backup:Not A Region:{ACCOUNT}:backup-vault:{DESTINATION_VAULT}"),
        ),
    ];

    for (case, destination_arn) in cases {
        let (status, refused) = start_copy(
            &router,
            &json!({
                "RecoveryPointArn": stored,
                "SourceBackupVaultName": SOURCE_VAULT,
                "DestinationBackupVaultArn": destination_arn,
                "IamRoleArn": role(),
            }),
        )
        .await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({destination_arn:?}): expected the parameter refusal, \
             got {status} {refused:?}"
        );
        let points = listed_points(&router, REGION, DESTINATION_VAULT).await;
        assert!(
            points.is_empty(),
            "case {case:?}: a refused copy must store nothing, got {points:?}"
        );
    }

    // A `RecoveryPointArn` that is not an ARN at all is the same kind of refusal.
    let (status, refused) = start_copy(
        &router,
        &json!({
            "RecoveryPointArn": "not-an-arn",
            "SourceBackupVaultName": SOURCE_VAULT,
            "DestinationBackupVaultArn": vault_arn(REGION, DESTINATION_VAULT),
            "IamRoleArn": role(),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidParameterValueException"),
        "a RecoveryPointArn that is not an ARN is a bad parameter value, got {status} {refused:?}"
    );

    // Each required member left out is reported as missing.
    for member in [
        "RecoveryPointArn",
        "SourceBackupVaultName",
        "DestinationBackupVaultArn",
        "IamRoleArn",
    ] {
        let mut request = json!({
            "RecoveryPointArn": stored,
            "SourceBackupVaultName": SOURCE_VAULT,
            "DestinationBackupVaultArn": vault_arn(REGION, DESTINATION_VAULT),
            "IamRoleArn": role(),
        });
        request
            .as_object_mut()
            .expect("a JSON object request")
            .remove(member);
        let (status, refused) = start_copy(&router, &request).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "MissingParameterValueException"),
            "case {member:?} absent: expected the missing-parameter refusal, \
             got {status} {refused:?}"
        );
    }

    // The positive control: the well-formed request copies the stored point.
    let (status, started) = start_copy(
        &router,
        &json!({
            "RecoveryPointArn": stored,
            "SourceBackupVaultName": SOURCE_VAULT,
            "DestinationBackupVaultArn": vault_arn(REGION, DESTINATION_VAULT),
            "IamRoleArn": role(),
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&started).as_str()),
        (200, "<none>"),
        "the well-formed copy must be accepted, got {status} {started:?}"
    );
}

/// A replayed `IdempotencyToken` answers with the copy job it already created, and
/// the same token on a different call is not a replay of it.
#[tokio::test]
async fn a_replayed_idempotency_token_returns_the_first_copy_job() {
    let router = router();
    create_both_vaults(&router, REGION).await;
    let (status, created) = create_vault(&router, "copy-elsewhere", "{}").await;
    assert_eq!(
        status, 200,
        "setup: the third CreateBackupVault should succeed, got {status} {created:?}"
    );
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = settled_source_point(&router, &resource).await;

    let request = json!({
        "RecoveryPointArn": stored,
        "SourceBackupVaultName": SOURCE_VAULT,
        "DestinationBackupVaultArn": vault_arn(REGION, DESTINATION_VAULT),
        "IamRoleArn": role(),
        "IdempotencyToken": "copy-token-1",
    });
    let (status, first) = start_copy(&router, &request).await;
    assert_eq!(status, 200, "expected StartCopyJob 200, got {status} {first:?}");
    let first_id = string_at(&first, "CopyJobId");

    // The identical call with the same token is a retry: the same job, no second
    // copy in the destination vault.
    let (status, replay) = start_copy(&router, &request).await;
    assert_eq!(
        (status, string_at(&replay, "CopyJobId")),
        (200, first_id.clone()),
        "a replayed token returns the first copy job, got {status} {replay:?}"
    );
    assert_eq!(
        string_at(&replay, "CreationDate"),
        string_at(&first, "CreationDate"),
        "a replay reports the first job's creation instant, got {replay:?}"
    );
    let points = listed_points(&router, REGION, DESTINATION_VAULT).await;
    assert_eq!(
        points.len(),
        1,
        "a replay must not store a second copy, got {points:?}"
    );

    // The same token naming a DIFFERENT call is not a retry of the stored one.
    let mut elsewhere = request.clone();
    elsewhere["DestinationBackupVaultArn"] = json!(vault_arn(REGION, "copy-elsewhere"));
    let (status, other) = start_copy(&router, &elsewhere).await;
    assert_eq!(status, 200, "expected StartCopyJob 200, got {status} {other:?}");
    let other_id = string_at(&other, "CopyJobId");
    assert_ne!(
        other_id, first_id,
        "a token reused on another call mints a new copy job, got {other:?}"
    );
    let points = listed_points(&router, REGION, "copy-elsewhere").await;
    assert_eq!(
        points.len(),
        1,
        "the second call copies into the vault it names, got {points:?}"
    );

    // A call with no token at all mints another copy job of its own.
    let mut untokenised = request.clone();
    untokenised
        .as_object_mut()
        .expect("a JSON object request")
        .remove("IdempotencyToken");
    let (status, third) = start_copy(&router, &untokenised).await;
    assert_eq!(status, 200, "expected StartCopyJob 200, got {status} {third:?}");
    assert_ne!(
        string_at(&third, "CopyJobId"),
        first_id,
        "an untokenised call is never a replay, got {third:?}"
    );
    let points = listed_points(&router, REGION, DESTINATION_VAULT).await;
    assert_eq!(
        points.len(),
        2,
        "the untokenised call copies again, got {points:?}"
    );
}

/// A `CopyJobId` no copy job answers to is refused 400 `ResourceNotFoundException`,
/// with a sentence that names no part of the request.
#[tokio::test]
async fn a_copy_job_id_nothing_answers_to_is_refused_as_not_found() {
    let router = router();
    create_both_vaults(&router, REGION).await;
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = settled_source_point(&router, &resource).await;
    let (status, started) = start_copy(
        &router,
        &json!({
            "RecoveryPointArn": stored,
            "SourceBackupVaultName": SOURCE_VAULT,
            "DestinationBackupVaultArn": vault_arn(REGION, DESTINATION_VAULT),
            "IamRoleArn": role(),
        }),
    )
    .await;
    assert_eq!(status, 200, "setup: StartCopyJob should succeed");
    let stored_id = string_at(&started, "CopyJobId");

    let cases = [
        ("an id nothing answers to", "does-not-exist".to_string()),
        (
            "a backup job's id",
            "00000000-0000-0000-0000-000000000000".to_string(),
        ),
        ("the stored id with a suffix", format!("{stored_id}-x")),
    ];
    for (case, copy_job_id) in cases {
        let (status, refused) = describe_copy(&router, &copy_job_id).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "ResourceNotFoundException", "Copy job not found"),
            "case {case:?} ({copy_job_id:?}): expected the not-found refusal, \
             got {status} {refused:?}"
        );
    }

    // The positive control: the stored id is answered.
    let job = copy_job(&router, &stored_id).await;
    assert_eq!(
        string_at(&job, "CopyJobId"),
        stored_id,
        "the stored copy job answers to its own id, got {job:?}"
    );
}
