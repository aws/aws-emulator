// SPDX-License-Identifier: CC0-1.0

//! AWS Backup announces a RESTORE job's state changes on the account's default EventBridge
//! bus, exactly as it announces a backup job's and a copy job's.
//!
//! The RULE under test: every status a stored job MOVES INTO after the call that started it
//! is published once onto the account's default event bus as `source` `aws.backup` with the
//! `detail-type` of the KIND of job that moved — `Restore Job State Change` for a restore
//! job — carrying that job's own stored members as the event `detail` (its id, the status it
//! moved into, the recovery point it restores from, the resource it was taken of and the
//! resource the restore created) and that recovery point in `resources`. EventBridge then
//! routes it: a rule whose `EventPattern` matches delivers the envelope UNCHANGED to every
//! one of that rule's targets, and a rule whose pattern cannot match receives nothing.
//!
//! The first test asserts that rule over a TABLE of rule patterns, which differ along what
//! the rule generalises over — which clauses the pattern filters on (`source` alone, `source`
//! plus the restore detail-type, `source` plus a LIST of two job detail-types, `source` plus
//! another kind's detail-type only) and therefore whether that rule is owed the restore
//! envelope at all. The second asserts it over the OTHER move a restore can make: the table's
//! rows are restores that create nothing, which move the job to its terminal failure status
//! and publish that status instead.
//!
//! Every published `detail` is checked against what `DescribeRestoreJob` reports for the same
//! job, so the event is the job's own stored state and not a second, invented copy of it; and
//! the status the START call stored is asserted NOT to be published, because it is what that
//! call's own response reports rather than a move the bus is owed.
//!
//! Every case drives real HTTP through the router, so what is asserted is the wire behaviour
//! a `backup`, a `dynamodb`, an `events` and an `sqs` client see together: all four are
//! mounted on ONE engine, because the restore creates its table through DynamoDB's own API
//! and the publication reaches EventBridge through the in-process callback, whose rule
//! delivers to an SQS queue target.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_state_change_events::{
    delivered, given_queue, given_rule, of_type, BACKUP_JOB_STATE_CHANGE, COMPLETED, EVENT_SOURCE,
};
use crate::recovery_point_deletion::start_point;
use crate::restore_job::{drop_table, source_table};
use crate::vault_and_backup_job::{call, create_vault, string_at, ACCOUNT, REGION};

/// The `detail-type` AWS Backup publishes a restore job's state changes under, spelled out
/// here rather than imported, so a wrong constant in the implementation cannot make a test
/// pass. Source: the EventBridge reference for AWS Backup, whose restore job events carry
/// `"source": "aws.backup"` with `"detail-type": "Restore Job State Change"`.
const RESTORE_JOB_STATE_CHANGE: &str = "Restore Job State Change";

/// `RestoreJobStatus` a started restore is reported in before it moves — the status the
/// start call's own response is about, which is therefore published for no job.
const PENDING: &str = "PENDING";

/// `RestoreJobStatus` a restore that created nothing ends in.
const FAILED: &str = "FAILED";

/// The vault every case backs its table up into.
const VAULT: &str = "restore-events-source";

/// The rule every case's pattern is installed under, and the rule whose pattern no AWS
/// Backup event can match.
const MATCHING_RULE: &str = "restore-job-events";
const OTHER_RULE: &str = "unrelated-events";

/// Longer than a backup job's settle span, so the recovery point a restore reads is one
/// whose backup has finished.
const PAST_BACKUP_SETTLE: std::time::Duration = std::time::Duration::from_millis(900);

/// Longer than a restore job's settle span, so the terminal status the clock owes a started
/// restore has already fallen due — and the in-process clock pump has had the chance to
/// publish it with no further call from this test.
const PAST_RESTORE_SETTLE: std::time::Duration = std::time::Duration::from_millis(3_600);

/// How many drains one wait for an envelope makes, and how long each waits.
///
/// Together they allow well over the span a restore settles after
/// ([`PAST_RESTORE_SETTLE`]), so a host slow enough to delay the in-process pump past that
/// span still sees the envelope rather than failing the case.
const MAX_DRAINS: usize = 60;
const DRAIN_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

/// One emulator: Backup, DynamoDB, EventBridge and SQS mounted on ONE shared engine, each
/// claiming only requests scoped to its own signing name.
fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        engine,
    ));
    router
}

/// Create the DynamoDB table a case backs up, and answer the ARN DynamoDB gives it: the
/// upload the restore suite owns (`tests/all/restore_job.rs`), in its pay-per-request form.
async fn given_table(router: &MultiServiceRouter, name: &str) -> String {
    source_table(router, name, false).await
}

/// `PUT /restore-jobs`, as the model binds `StartRestoreJob`; answers the `RestoreJobId`.
async fn start_restore(
    router: &MultiServiceRouter,
    case: &str,
    recovery_point_arn: &str,
    resource_type: &str,
    target: &str,
) -> String {
    let body = json!({
        "RecoveryPointArn": recovery_point_arn,
        "ResourceType": resource_type,
        "Metadata": {"targetTableName": target},
        "IamRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole"),
    });
    let (status, started) = call(router, "PUT", "/restore-jobs", Some(&body.to_string())).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartRestoreJob answered {status} {started}"
    );
    string_at(&started, "RestoreJobId")
}

/// `GET /restore-jobs/{RestoreJobId}`, as the model binds `DescribeRestoreJob`.
async fn describe_restore(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    let (status, read) = call(router, "GET", &format!("/restore-jobs/{job_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeRestoreJob answered {status} {read}"
    );
    read
}

/// Drain the queue at `url` until it has handed over an envelope of `detail_type`, or until
/// the drains run out.
///
/// The wait is on a KIND of envelope rather than on a COUNT of them, which is what
/// `job_state_change_events.rs`'s own `delivered_at_least` waits for: a rule that matches
/// the backup job's move too is handed that one first, so a case that waited for one
/// envelope would read the queue before the restore's own move arrived.
///
/// Each attempt awaits, which is what lets the in-process clock pump make progress beside
/// this test, and the loop is bounded so a move that is never published fails the case
/// rather than hanging it.
async fn drained_for(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    detail_type: &str,
) -> Vec<Value> {
    let mut bodies = Vec::new();
    for _ in 0..MAX_DRAINS {
        bodies.extend(delivered(router, case, url).await);
        if !of_type(&bodies, detail_type).is_empty() {
            break;
        }
        ::tokio::time::sleep(DRAIN_WAIT).await;
    }
    bodies
}

/// Back the table `table` up into [`VAULT`] and answer the ARN of the settled recovery
/// point the backup wrote.
async fn settled_point(router: &MultiServiceRouter, case: &str, table_arn: &str) -> String {
    let (status, created) = create_vault(router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBackupVault answered {status} {created:?}"
    );
    let point = start_point(router, VAULT, table_arn, None).await;
    ::tokio::time::sleep(PAST_BACKUP_SETTLE).await;
    point
}

/// One row of the pattern table: the rule's pattern, and whether a restore job's move is
/// one that rule is owed.
struct Row {
    case: &'static str,
    pattern: Value,
    owed: bool,
}

/// The rows: the pattern forms a client writes for these events.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a source-only pattern",
            pattern: json!({ "source": [EVENT_SOURCE] }),
            owed: true,
        },
        Row {
            case: "a pattern naming the restore-job detail-type",
            pattern: json!({
                "source": [EVENT_SOURCE],
                "detail-type": [RESTORE_JOB_STATE_CHANGE],
            }),
            owed: true,
        },
        Row {
            case: "a pattern listing the backup-job and restore-job detail-types",
            pattern: json!({
                "source": [EVENT_SOURCE],
                "detail-type": [BACKUP_JOB_STATE_CHANGE, RESTORE_JOB_STATE_CHANGE],
            }),
            owed: true,
        },
        Row {
            case: "a pattern naming the backup-job detail-type only",
            pattern: json!({
                "source": [EVENT_SOURCE],
                "detail-type": [BACKUP_JOB_STATE_CHANGE],
            }),
            owed: false,
        },
    ]
}

/// A restore job's terminal move reaches every target of a rule whose pattern matches it,
/// carrying the job's own stored members — and reaches no target of a rule whose pattern
/// does not.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_settled_restore_job_reaches_the_targets_of_every_rule_whose_pattern_matches() {
    for row in rows() {
        let case = row.case;
        let router = emulator();
        let table_arn = given_table(&router, "orders").await;

        let (matched_url, matched_arn) = given_queue(&router, case, "restore-matched").await;
        let (other_url, other_arn) = given_queue(&router, case, "restore-unmatched").await;
        given_rule(&router, case, MATCHING_RULE, &row.pattern, &[matched_arn]).await;
        // Another service's events: nothing AWS Backup publishes can match this.
        given_rule(
            &router,
            case,
            OTHER_RULE,
            &json!({ "source": ["aws.s3"], "detail-type": ["Object Created"] }),
            &[other_arn],
        )
        .await;

        // The rule is installed BEFORE the backup, so the backup job's own move is
        // delivered too: that is the control for a pattern owed no restore envelope.
        let point = settled_point(&router, case, &table_arn).await;
        let job_id = start_restore(&router, case, &point, "DynamoDB", "orders-restored").await;
        // The transition falls due on the clock, and the bus is read with no further
        // Backup call: a client watching the bus makes none.
        ::tokio::time::sleep(PAST_RESTORE_SETTLE).await;
        let received = drained_for(&router, case, &matched_url, RESTORE_JOB_STATE_CHANGE).await;
        let other = delivered(&router, case, &other_url).await;
        assert!(
            other.is_empty(),
            "case {case:?}: a rule whose pattern cannot match receives nothing, got {other:?}"
        );

        let envelopes = of_type(&received, RESTORE_JOB_STATE_CHANGE);
        if !row.owed {
            assert!(
                envelopes.is_empty(),
                "case {case:?}: a pattern that does not name the restore detail-type is owed no \
                 restore envelope, got {envelopes:?}"
            );
            // The control: that rule IS owed the backup job's own move, so the absence
            // above is the pattern filtering and not a bus that delivered nothing.
            let backups = of_type(&received, BACKUP_JOB_STATE_CHANGE);
            assert_eq!(
                backups.len(),
                1,
                "case {case:?}: the rule must still receive the backup job's move, got \
                 {received:?}"
            );
            continue;
        }

        assert_eq!(
            envelopes.len(),
            1,
            "case {case:?}: the settle is the restore job's ONE published move — the status \
             the start stored is not one, got {envelopes:?}"
        );
        let event = &envelopes[0];
        assert_eq!(
            event["source"],
            json!(EVENT_SOURCE),
            "case {case:?}: the envelope names AWS Backup as its source, got {event}"
        );
        assert_eq!(
            event["resources"],
            json!([point]),
            "case {case:?}: the envelope names the recovery point the restore is of, got {event}"
        );

        // The detail is the job's own stored state: every member is the one the read of
        // the same job reports.
        let read = describe_restore(&router, case, &job_id).await;
        let detail = &event["detail"];
        for (member, reported) in [
            ("restoreJobId", "RestoreJobId"),
            ("status", "Status"),
            ("recoveryPointArn", "RecoveryPointArn"),
            ("sourceResourceArn", "SourceResourceArn"),
            ("backupVaultArn", "BackupVaultArn"),
            ("createdResourceArn", "CreatedResourceArn"),
            ("resourceType", "ResourceType"),
            ("percentDone", "PercentDone"),
            ("iamRoleArn", "IamRoleArn"),
        ] {
            assert_eq!(
                detail[member], read[reported],
                "case {case:?}: detail.{member} must be what DescribeRestoreJob reports as \
                 {reported}, got {detail} against {read}"
            );
        }
        assert_eq!(
            detail["status"],
            json!(COMPLETED),
            "case {case:?}: the published move is the terminal status the clock owed, got \
             {detail}"
        );
        assert_eq!(
            detail["restoreJobId"],
            json!(job_id),
            "case {case:?}: the detail names the job the start answered with, got {detail}"
        );
        assert!(
            detail["createdResourceArn"].is_string(),
            "case {case:?}: a completed restore reports the resource it created, got {detail}"
        );
        for envelope in &received {
            assert_ne!(
                envelope["detail"]["status"],
                json!(PENDING),
                "case {case:?}: the status the start call's own response reports is not a move \
                 the bus is owed, got {envelope}"
            );
        }
    }
}

/// A restore that creates nothing moves the job to its terminal FAILURE status, and THAT
/// status is what the bus is handed: the rows differ along the kind of restore that produces
/// no resource.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_that_creates_nothing_publishes_its_failure() {
    // Each row is a restore this emulator performs no resource creation for: a type it
    // cannot re-create, and the type it can whose metadata names no table to create.
    let cases: Vec<(&str, &str, &str)> = vec![
        ("an EFS recovery point", "EFS", "orders-restored"),
        ("an S3 recovery point", "S3", "orders-restored"),
        ("a DynamoDB restore of a table that is not there", "DynamoDB", "orders-restored"),
    ];

    for (case, resource_type, target) in cases {
        let router = emulator();
        // The source table exists only for the DynamoDB rows that CAN be restored; here
        // the backup is of a resource of the row's own type, and the DynamoDB row's table
        // is dropped once its backup is taken — a backup started for a table DynamoDB
        // reports absent captures nothing and writes no recovery point to restore from.
        let dropped_table = (resource_type == "DynamoDB").then_some("orders-dropped");
        let resource_arn = match resource_type {
            "EFS" => format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-1"),
            "S3" => format!("arn:aws:s3:::{ACCOUNT}-orders"),
            _ => given_table(&router, "orders-dropped").await,
        };
        let point = settled_point(&router, case, &resource_arn).await;
        if let Some(table) = dropped_table {
            drop_table(&router, table).await;
        }

        let (url, arn) = given_queue(&router, case, "restore-failures").await;
        given_rule(
            &router,
            case,
            MATCHING_RULE,
            &json!({ "source": [EVENT_SOURCE], "detail-type": [RESTORE_JOB_STATE_CHANGE] }),
            &[arn],
        )
        .await;

        let job_id = start_restore(&router, case, &point, resource_type, target).await;
        let received = drained_for(&router, case, &url, RESTORE_JOB_STATE_CHANGE).await;
        let envelopes = of_type(&received, RESTORE_JOB_STATE_CHANGE);
        assert_eq!(
            envelopes.len(),
            1,
            "case {case:?}: the failure is the job's ONE published move, got {envelopes:?}"
        );
        let detail = &envelopes[0]["detail"];
        let read = describe_restore(&router, case, &job_id).await;
        assert_eq!(
            (detail["status"].clone(), detail["restoreJobId"].clone()),
            (json!(FAILED), json!(job_id)),
            "case {case:?}: the published move is the terminal failure of that job, got {detail}"
        );
        assert_eq!(
            detail["statusMessage"], read["StatusMessage"],
            "case {case:?}: the detail carries the reason the read reports, got {detail} \
             against {read}"
        );
        assert!(
            detail.get("createdResourceArn").is_none(),
            "case {case:?}: a restore that created nothing reports no created resource, got \
             {detail}"
        );
    }
}
