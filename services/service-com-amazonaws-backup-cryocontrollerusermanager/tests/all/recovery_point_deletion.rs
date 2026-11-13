// SPDX-License-Identifier: CC0-1.0

//! `DeleteRecoveryPoint`: a recovery point is a STORED resource, and a delete is
//! resolved against the store.
//!
//! The rules under test:
//!
//! * a delete addressed by (`BackupVaultName`, `RecoveryPointArn`) to a stored
//!   recovery point removes it, so every later read of the vault reports it absent
//!   and `NumberOfRecoveryPoints` follows;
//! * every way that pair can fail to name a stored recovery point — a vault the
//!   account does not hold, an ARN nothing answers to, an ARN another vault holds,
//!   one whose backup has not finished, one already deleted — is ONE condition,
//!   refused 400 `ResourceNotFoundException` with the same sentence, and nothing
//!   stored changes (an ARN of ANOTHER ACCOUNT is not one of them: the
//!   authorization decision refuses it before the pair is resolved, which
//!   `recovery_point_cross_account_authorization.rs` covers);
//! * a `RecoveryPointArn` that is not an ARN is a bad parameter value rather than
//!   an absent resource;
//! * while a vault is configured with a Backup Vault Lock, a recovery point whose
//!   retention has not elapsed cannot be deleted — 400 `InvalidRequestException`,
//!   and the point survives — while the same delete succeeds once that retention
//!   has elapsed, and in a vault with no lock.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire
//! behaviour a `backup` SDK client sees. The request harness is the shared one in
//! `vault_and_backup_job.rs`; the one piece added here is a router that KEEPS the
//! engine handle, which the lock cases need in order to age a stored recovery point
//! past the retention its lifecycle sets (a retention is counted in days, so no
//! test can wait one out).

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use service_com_amazonaws_backup_cryocontrollerusermanager::ComAmazonawsBackupCryocontrollerusermanagerState as BackupState;
use smithy_server_runtime::MultiServiceRouter;

use crate::vault_and_backup_job::{
    call, create_vault, error_code, start_job, string_at, ACCOUNT, PAST_SETTLE, REGION,
};

/// Seconds in a day, for aging a stored recovery point by whole days — the unit
/// `Lifecycle`'s day counts are expressed in.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The router plus the engine it was built on, so a case can age the state the
/// handlers read. The shared `router()` keeps no handle on its engine.
///
/// `pub(crate)` so the sibling suite covering the recovery-point READ
/// (`recovery_point_read.rs`) ages a stored point through the same seam rather
/// than growing a second copy of it.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(engine.clone()),
    );
    (router, engine)
}

/// `DELETE /backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}`,
/// as the model binds this operation.
async fn delete_point(router: &MultiServiceRouter, vault: &str, arn: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/backup-vaults/{vault}/recovery-points/{arn}"),
        None,
    )
    .await
}

/// The recovery-point ARNs the vault reports, through the implemented list read.
pub(crate) async fn listed_points(router: &MultiServiceRouter, vault: &str) -> Vec<Value> {
    let (status, listed) = call(
        router,
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
async fn counted_points(router: &MultiServiceRouter, vault: &str) -> i64 {
    let (_, described) = call(router, "GET", &format!("/backup-vaults/{vault}"), None).await;
    described
        .get("NumberOfRecoveryPoints")
        .and_then(Value::as_i64)
        .unwrap_or(-1)
}

/// Start one backup job in `vault` and return the recovery-point ARN it reserved.
pub(crate) async fn start_point(
    router: &MultiServiceRouter,
    vault: &str,
    resource_arn: &str,
    lifecycle: Option<&str>,
) -> String {
    let role = format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole");
    let mut payload = serde_json::json!({
        "BackupVaultName": vault,
        "ResourceArn": resource_arn,
        "IamRoleArn": role,
    });
    if let Some(lifecycle) = lifecycle {
        payload["Lifecycle"] =
            serde_json::from_str(lifecycle).expect("a well-formed Lifecycle literal");
    }
    let (status, started) = start_job(router, &payload.to_string()).await;
    assert_eq!(
        status, 200,
        "setup {resource_arn:?}: expected StartBackupJob 200, got {status} {started:?}"
    );
    string_at(&started, "RecoveryPointArn")
}

/// Start one backup job in `vault` and wait until the recovery point it wrote reports the
/// status that makes it readable and restorable.
///
/// The wait is on the CONDITION — the clock owes the point its completion a fixed span
/// after the backup started, and a read materialises it — with room well past that span, so
/// a case never turns on how fast the host runs. A read of a point whose backup is still
/// running is refused exactly as a read of one that does not exist is, so an unsettled
/// point is simply looked at again.
///
/// `pub(crate)` for the sibling suites whose subject is what a client does with a FINISHED
/// recovery point rather than the backup that wrote it.
pub(crate) async fn completed_point(
    router: &MultiServiceRouter,
    vault: &str,
    resource_arn: &str,
) -> String {
    let arn = start_point(router, vault, resource_arn, None).await;
    for _ in 0..POINT_POLLS {
        let (status, read) = call(
            router,
            "GET",
            &format!("/backup-vaults/{vault}/recovery-points/{arn}"),
            None,
        )
        .await;
        if status == 200 && string_at(&read, "Status") == POINT_STATUS_COMPLETED {
            return arn;
        }
        ::tokio::time::sleep(POINT_POLL_WAIT).await;
    }
    panic!(
        "setup {resource_arn:?}: the recovery point {arn} never became readable and completed"
    );
}

/// `RecoveryPointStatus` of a point whose backup has finished (model enum
/// `RecoveryPointStatus`).
const POINT_STATUS_COMPLETED: &str = "COMPLETED";

/// How long [`completed_point`] waits in all, and how long each look waits before the next.
const POINT_POLLS: usize = 150;
const POINT_POLL_WAIT: Duration = Duration::from_millis(100);

/// Move a stored recovery point's creation instant `days` into the past, so the
/// retention its lifecycle sets has already elapsed.
pub(crate) fn age_point(engine: &engine::InMemoryEngine, arn: &str, days: u64) {
    if days == 0 {
        return;
    }
    let aged = stored_creation(engine, arn)
        .checked_sub(Duration::from_secs(days * SECONDS_PER_DAY))
        .expect("an instant that many days before the backup");
    set_point_creation(engine, arn, aged);
}

/// Store `at` as a recovery point's creation instant.
///
/// The one seam the suites move a stored point's creation instant through, so a
/// case can place a point at a known offset from an instant it sends (a legal
/// hold's `DateRange` bound, a listing's creation filter) instead of depending
/// on what the clock read when the point was stored. [`age_point`] is this seam
/// applied whole days back.
pub(crate) fn set_point_creation(engine: &engine::InMemoryEngine, arn: &str, at: SystemTime) {
    let state = engine.state::<BackupState>();
    let mut store = state.lock();
    let point = store
        .recovery_points
        .get_mut(arn)
        .expect("the recovery point the case stored");
    point.creation_date = at;
}

/// The creation instant the store holds for a recovery point.
fn stored_creation(engine: &engine::InMemoryEngine, arn: &str) -> SystemTime {
    let state = engine.state::<BackupState>();
    let creation = state.lock().recovery_points[arn].creation_date;
    creation
}

/// A delete addressed to a stored recovery point removes it: the list stops
/// reporting it, the vault's count follows, the now-empty vault can be deleted,
/// and a second delete of the same ARN finds nothing.
///
/// The table spans protected resources of several services and the lifecycle forms
/// the model admits, including the documented "keep indefinitely" sentinel — what
/// the rule generalises over is the stored point, not any one lifecycle.
#[tokio::test]
async fn a_stored_recovery_point_is_deleted_and_every_later_read_reports_it_absent() {
    let cases: Vec<(&str, String, Option<&str>)> = vec![
        (
            "a DynamoDB table with no lifecycle",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
            None,
        ),
        (
            "an EFS file system retained for 7 days",
            format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-01"),
            Some(r#"{"DeleteAfterDays":7}"#),
        ),
        (
            "an S3 bucket moved to cold storage then deleted",
            "arn:aws:s3:::demo-bucket".to_string(),
            Some(r#"{"MoveToColdStorageAfterDays":30,"DeleteAfterDays":120}"#),
        ),
        (
            "an EBS volume kept indefinitely",
            format!("arn:aws:ec2:{REGION}:{ACCOUNT}:volume/vol-01"),
            Some(r#"{"DeleteAfterDays":-1}"#),
        ),
    ];

    for (case, resource_arn, lifecycle) in cases {
        let (router, _engine) = router_with_engine();
        let (status, created) = create_vault(&router, "delete-rp", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );
        let arn = start_point(&router, "delete-rp", &resource_arn, lifecycle).await;
        std::thread::sleep(PAST_SETTLE);

        let points = listed_points(&router, "delete-rp").await;
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: expected the one stored recovery point, got {points:?}"
        );
        assert_eq!(
            points[0].get("Status").and_then(Value::as_str),
            Some("COMPLETED"),
            "case {case:?}: the settled point is COMPLETED, got {points:?}"
        );

        let (status, deleted) = delete_point(&router, "delete-rp", &arn).await;
        assert_eq!(
            (status, error_code(&deleted).as_str()),
            (200, "<none>"),
            "case {case:?}: expected DeleteRecoveryPoint 200, got {status} {deleted:?}"
        );

        let points = listed_points(&router, "delete-rp").await;
        assert!(
            points.is_empty(),
            "case {case:?}: the deleted point must be gone from the list, got {points:?}"
        );
        let count = counted_points(&router, "delete-rp").await;
        assert_eq!(
            count, 0,
            "case {case:?}: expected NumberOfRecoveryPoints 0 after the delete, got {count}"
        );

        let (status, again) = delete_point(&router, "delete-rp", &arn).await;
        assert_eq!(
            (
                status,
                error_code(&again).as_str(),
                string_at(&again, "Message").as_str()
            ),
            (
                400,
                "ResourceNotFoundException",
                "Cannot find recovery point"
            ),
            "case {case:?}: a second delete must find nothing, got {status} {again:?}"
        );

        // The emptied vault is deletable, which is the same stored count seen from
        // the other side.
        let (status, dropped) = call(&router, "DELETE", "/backup-vaults/delete-rp", None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the emptied vault must be deletable, got {status} {dropped:?}"
        );
    }
}

/// Every pair that names no stored recovery point is ONE condition: 400
/// `ResourceNotFoundException` with the same sentence, disclosing nothing about
/// which half of the pair was wrong — and the point that IS stored survives each
/// refusal.
#[tokio::test]
async fn a_pair_that_names_no_stored_recovery_point_is_one_refusal() {
    let (router, _engine) = router_with_engine();
    for vault in ["holder", "other"] {
        let (status, created) = create_vault(&router, vault, "{}").await;
        assert_eq!(
            status, 200,
            "setup {vault:?}: CreateBackupVault should succeed, got {status} {created:?}"
        );
    }
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = start_point(&router, "holder", &resource, Some(r#"{"DeleteAfterDays":7}"#)).await;
    // A point of "other" that is then deleted, so a later case can address an ARN
    // this account held and no longer does.
    let gone = start_point(&router, "other", &resource, None).await;
    std::thread::sleep(PAST_SETTLE);
    let (status, _) = delete_point(&router, "other", &gone).await;
    assert_eq!(status, 200, "setup: the second point should be deletable");

    let fabricated = format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:does-not-exist");
    let other_partition = format!("arn:aws-cn:backup:{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7");
    // An ARN of ANOTHER ACCOUNT is deliberately not a row here: it is refused by
    // the authorization decision before the pair is resolved at all, which
    // `recovery_point_cross_account_authorization.rs` covers.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("an ARN nothing answers to", "holder", fabricated.as_str()),
        (
            "an ARN of another partition",
            "holder",
            other_partition.as_str(),
        ),
        ("an ARN another vault holds", "other", stored.as_str()),
        (
            "a vault this account does not hold",
            "never-created",
            stored.as_str(),
        ),
        ("a point already deleted", "other", gone.as_str()),
    ];

    for (case, vault, arn) in cases {
        let (status, refused) = delete_point(&router, vault, arn).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (
                400,
                "ResourceNotFoundException",
                "Cannot find recovery point"
            ),
            "case {case:?} ({vault}, {arn}): expected the not-found refusal, got {status} {refused:?}"
        );
        let points = listed_points(&router, "holder").await;
        let arns: Vec<String> = points
            .iter()
            .map(|point| string_at(point, "RecoveryPointArn"))
            .collect();
        assert_eq!(
            arns,
            vec![stored.clone()],
            "case {case:?}: a refused delete must change nothing stored, got {arns:?}"
        );
    }
}

/// A recovery point whose backup has not finished is not addressable yet: the
/// delete finds nothing and the point stays stored in its transient state, and the
/// same delete succeeds once the backup has settled.
#[tokio::test]
async fn a_recovery_point_whose_backup_has_not_finished_is_not_found_yet() {
    let resources = [
        format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
        format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-02"),
    ];

    for resource in resources {
        let (router, _engine) = router_with_engine();
        let (status, created) = create_vault(&router, "unsettled", "{}").await;
        assert_eq!(
            status, 200,
            "case {resource:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );
        let arn = start_point(&router, "unsettled", &resource, None).await;

        // Straight after the start the backup is still running.
        let (status, refused) = delete_point(&router, "unsettled", &arn).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (
                400,
                "ResourceNotFoundException",
                "Cannot find recovery point"
            ),
            "case {resource:?}: a running backup has no deletable point, got {status} {refused:?}"
        );
        let points = listed_points(&router, "unsettled").await;
        assert_eq!(
            points.len(),
            1,
            "case {resource:?}: the reserved point is still stored, got {points:?}"
        );

        std::thread::sleep(PAST_SETTLE);
        let (status, deleted) = delete_point(&router, "unsettled", &arn).await;
        assert_eq!(
            (status, error_code(&deleted).as_str()),
            (200, "<none>"),
            "case {resource:?}: once settled the point is deletable, got {status} {deleted:?}"
        );
        let points = listed_points(&router, "unsettled").await;
        assert!(
            points.is_empty(),
            "case {resource:?}: the deleted point must be gone, got {points:?}"
        );
    }
}

/// A `RecoveryPointArn` that is not an ARN is refused as a bad parameter value —
/// not reported as an absent resource — and deletes nothing.
#[tokio::test]
async fn a_recovery_point_arn_that_is_not_an_arn_is_a_bad_parameter_value() {
    // The form an ARN takes:
    // `arn:<partition>:<service>:<region>:<account>:<resource>`, as the model's
    // own doc for `RecoveryPointArn` spells it out
    // (smithy_query(query="operation", operation="DeleteRecoveryPoint") types it
    // as `ARN` with no pattern, so the service applies the rule).
    let cases: Vec<(&str, String)> = vec![
        ("no arn scheme", "not-an-arn".to_string()),
        ("too few fields", "arn:aws:backup".to_string()),
        (
            "an empty partition",
            format!("arn::backup:{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7"),
        ),
        (
            "an empty service",
            format!("arn:aws::{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7"),
        ),
        (
            "an empty resource",
            format!("arn:aws:backup:{REGION}:{ACCOUNT}:"),
        ),
    ];

    let (router, _engine) = router_with_engine();
    let (status, created) = create_vault(&router, "bad-arn", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let stored = start_point(&router, "bad-arn", &resource, None).await;
    std::thread::sleep(PAST_SETTLE);

    for (case, value) in cases {
        let (status, refused) = delete_point(&router, "bad-arn", &value).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({value:?}): expected the parameter refusal, got {status} {refused:?}"
        );
        let points = listed_points(&router, "bad-arn").await;
        assert_eq!(
            points.len(),
            1,
            "case {case:?}: a refused delete must change nothing stored, got {points:?}"
        );
    }

    // The positive control: the well-formed ARN of the stored point deletes it.
    let (status, deleted) = delete_point(&router, "bad-arn", &stored).await;
    assert_eq!(
        (status, error_code(&deleted).as_str()),
        (200, "<none>"),
        "the stored point's own ARN must be accepted, got {status} {deleted:?}"
    );
}

/// A Backup Vault Lock makes retention the SERVICE's refusal: while the vault holds
/// one, a recovery point whose retention has not elapsed cannot be deleted and
/// survives the attempt; once that retention has elapsed — and in a vault with no
/// lock — the same delete succeeds.
///
/// The rows differ along what the rule generalises over: whether the vault is
/// locked, what retention the point was stored under (a day count, none at all, and
/// the keep-indefinitely sentinel) and how much of that retention has elapsed.
#[tokio::test]
async fn a_vault_lock_retains_a_recovery_point_until_its_retention_elapses() {
    /// A lock whose cooling-off period is still running, as the day counts the
    /// model documents allow (`ChangeableForDays` 3..=36500, retention 1..=36500).
    const LOCK_WITH_RETENTION: &str =
        r#"{"MinRetentionDays":1,"MaxRetentionDays":36500,"ChangeableForDays":3}"#;
    /// A lock that sets no retention bounds at all.
    const LOCK_WITHOUT_RETENTION: &str = r#"{"ChangeableForDays":36500}"#;

    // One row: the case name, the `PutBackupVaultLockConfiguration` body (`None`
    // for an unlocked vault), the `Lifecycle` the backup is stored under (`None`
    // for none at all), how many days have passed since the backup, and whether
    // the point survives the delete.
    type LockCase = (
        &'static str,
        Option<&'static str>,
        Option<&'static str>,
        u64,
        bool,
    );
    let cases: Vec<LockCase> = vec![
        (
            "locked, inside a 7-day retention",
            Some(LOCK_WITH_RETENTION),
            Some(r#"{"DeleteAfterDays":7}"#),
            0,
            true,
        ),
        (
            "locked, one day into a 7-day retention",
            Some(LOCK_WITH_RETENTION),
            Some(r#"{"DeleteAfterDays":7}"#),
            1,
            true,
        ),
        (
            "locked, a day past a 7-day retention",
            Some(LOCK_WITH_RETENTION),
            Some(r#"{"DeleteAfterDays":7}"#),
            8,
            false,
        ),
        (
            "locked, a day past a 1-day retention",
            Some(LOCK_WITH_RETENTION),
            Some(r#"{"DeleteAfterDays":1}"#),
            2,
            false,
        ),
        (
            "locked, a point with no lifecycle at all",
            Some(LOCK_WITH_RETENTION),
            None,
            365,
            true,
        ),
        (
            "locked, a point kept indefinitely",
            Some(LOCK_WITH_RETENTION),
            Some(r#"{"DeleteAfterDays":-1}"#),
            36_500,
            true,
        ),
        (
            "a lock that sets no retention bounds",
            Some(LOCK_WITHOUT_RETENTION),
            Some(r#"{"DeleteAfterDays":7}"#),
            0,
            true,
        ),
        (
            "no lock, inside a 7-day retention",
            None,
            Some(r#"{"DeleteAfterDays":7}"#),
            0,
            false,
        ),
        ("no lock, no lifecycle", None, None, 0, false),
    ];

    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");

    for (case, lock, lifecycle, aged_days, survives) in cases {
        let (router, engine) = router_with_engine();
        let (status, created) = create_vault(&router, "locked-vault", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );
        if let Some(lock) = lock {
            let (status, applied) = call(
                &router,
                "PUT",
                "/backup-vaults/locked-vault/vault-lock",
                Some(lock),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: setup PutBackupVaultLockConfiguration should succeed, got {status} {applied:?}"
            );
        }
        let arn = start_point(&router, "locked-vault", &resource, lifecycle).await;
        std::thread::sleep(PAST_SETTLE);
        age_point(&engine, &arn, aged_days);

        let (status, answered) = delete_point(&router, "locked-vault", &arn).await;
        if survives {
            assert_eq!(
                (
                    status,
                    error_code(&answered).as_str(),
                    string_at(&answered, "Message").as_str()
                ),
                (
                    400,
                    "InvalidRequestException",
                    "RecoveryPoint cannot be deleted or updated (Backup vault configured with Lock)"
                ),
                "case {case:?}: expected the vault-lock refusal, got {status} {answered:?}"
            );
            let points = listed_points(&router, "locked-vault").await;
            let arns: Vec<String> = points
                .iter()
                .map(|point| string_at(point, "RecoveryPointArn"))
                .collect();
            assert_eq!(
                arns,
                vec![arn.clone()],
                "case {case:?}: the retained point must survive the refusal, got {points:?}"
            );
            assert_eq!(
                points[0].get("Status").and_then(Value::as_str),
                Some("COMPLETED"),
                "case {case:?}: the retained point keeps its status, got {points:?}"
            );
        } else {
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (200, "<none>"),
                "case {case:?}: expected the delete to succeed, got {status} {answered:?}"
            );
            let points = listed_points(&router, "locked-vault").await;
            assert!(
                points.is_empty(),
                "case {case:?}: the deleted point must be gone, got {points:?}"
            );
        }
    }
}

/// The aging helper used by the lock cases really moves the stored instant: a
/// guard on the one piece of state this suite writes directly, so a case that
/// "passes" because nothing moved cannot go unnoticed.
#[tokio::test]
async fn aging_a_stored_point_moves_its_creation_instant() {
    let (router, engine) = router_with_engine();
    let (status, _) = create_vault(&router, "aging", "{}").await;
    assert_eq!(status, 200, "setup: CreateBackupVault should succeed");
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let arn = start_point(&router, "aging", &resource, None).await;

    let before = stored_creation(&engine, &arn);
    age_point(&engine, &arn, 3);
    let after = stored_creation(&engine, &arn);
    let moved = before
        .duration_since(after)
        .expect("aging moves the instant backwards");
    assert_eq!(
        moved,
        Duration::from_secs(3 * SECONDS_PER_DAY),
        "expected the creation instant moved back three days, got {moved:?}"
    );
    assert!(
        after < SystemTime::now(),
        "an aged instant stays in the past, got {after:?}"
    );
}
