// SPDX-License-Identifier: CC0-1.0

//! What a started AWS Backup job COMES TO, decided by whether the protected resource it
//! was started for exists at all.
//!
//! The RULE under test: `StartBackupJob` does not resolve its `ResourceArn` synchronously —
//! the call is accepted on the vault and the parameters — and the job's TERMINAL state then
//! follows what the service that OWNS that resource answers about it: a resource that
//! service reports it does not hold is a backup that captures nothing, so the job settles
//! `FAILED` with the resource-not-found `StatusMessage` and `MessageCategory`, writes no
//! recovery point (the ARN its start answered with therefore resolves to nothing), while a
//! resource that service holds — and every resource nothing can be learned about — settles
//! `COMPLETED` with the point it wrote.
//!
//! The table spans what the rule generalises over rather than one request: whether the
//! owning service holds the resource, HOW it came to not hold it (never created, or created
//! and then deleted), an ARN of that service that names no resource of the type at all, and
//! a resource of a type whose owning service this emulator cannot ask — the last two being
//! the silences that decide nothing. Each row is asserted on the job read, on the recovery
//! point the job's start named, on the vault's own listing for that resource and — once,
//! over the whole table — on the state- and category-filtered job listings, so a second
//! reader can never disagree with the first.
//!
//! Every case drives real HTTP through the router: the Backup wire is the shared harness in
//! `vault_and_backup_job.rs`, and DynamoDB is mounted beside Backup on one engine
//! (`restore_job.rs`'s `emulator`), because the question is put to the service that owns the
//! table and the tables it answers about are the ones `CreateTable` and `DeleteTable` wrote
//! through its own wire.

use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::restore_job::{drop_table, emulator, source_table};
use crate::vault_and_backup_job::{
    call, create_vault, error_code, start_job, string_at, ACCOUNT, REGION,
};

/// The vault every job of this suite is started into.
const VAULT: &str = "resource-resolution-vault";

/// The two terminal `BackupJobState` values a started job reaches, spelled out here rather
/// than imported so a wrong constant in the implementation cannot make a test pass (model
/// enum `BackupJobState`).
const COMPLETED: &str = "COMPLETED";
const FAILED: &str = "FAILED";

/// The `MessageCategory` AWS reports for a job that failed because the resource it was
/// started for does not exist, spelled out here for the same reason.
const RESOURCE_NOT_FOUND: &str = "RESOURCE_NOT_FOUND";

/// The sentence this service states for "the recovery point named is not one this account
/// holds" — fixed text, naming no part of the request.
const RECOVERY_POINT_NOT_FOUND: &str = "Cannot find recovery point";

/// How long a case waits, in all, for a job to reach a terminal state, and how long each
/// look waits before the next. The wait is on the CONDITION — the clock owes the transition
/// a fixed span after the start, and a read materialises it — with room well past that span
/// so a loaded host does not fail a case.
const JOB_POLLS: usize = 150;
const JOB_POLL_WAIT: Duration = Duration::from_millis(100);

/// One row: the protected resource a job is started for, and what that job must come to.
struct Case {
    /// What this row is about, named in every assertion of it.
    name: &'static str,
    /// The DynamoDB table this row creates, when the row needs the owning service to have
    /// held one; dropped before the backup when `dropped` is set.
    table: Option<&'static str>,
    /// Whether that table is deleted again before the backup is started.
    dropped: bool,
    /// The protected resource, for a row that names one itself rather than creating a table.
    named_resource: String,
    /// The terminal state the job must reach.
    expected_state: &'static str,
}

/// The role every job of this suite runs under. No IAM is mounted beside Backup here, so
/// nothing is learned about its permissions and only the RESOURCE decides each outcome.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// The table.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "a table the owning service holds",
            table: Some("orders-held"),
            dropped: false,
            named_resource: String::new(),
            expected_state: COMPLETED,
        },
        Case {
            name: "a table the owning service never held",
            table: None,
            dropped: false,
            named_resource: format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/orders-never"),
            expected_state: FAILED,
        },
        Case {
            name: "a table the owning service no longer holds",
            table: Some("orders-deleted"),
            dropped: true,
            named_resource: String::new(),
            expected_state: FAILED,
        },
        Case {
            // The owning service CAN be asked about a table, but this ARN of it names a
            // resource that is not one, so there is nothing to ask about.
            name: "an ARN of that service naming a resource that is not a table",
            table: None,
            dropped: false,
            named_resource: format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:global-table/orders"),
            expected_state: COMPLETED,
        },
        Case {
            // A type whose owning service this emulator cannot ask: nothing is learned, so
            // the job keeps the outcome a backup nothing objects to has.
            name: "a file system, whose owning service this emulator cannot ask",
            table: None,
            dropped: false,
            named_resource: format!(
                "arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-0123456789abcdef0"
            ),
            expected_state: COMPLETED,
        },
    ]
}

/// Start one backup job for `resource_arn` and answer its id and the recovery-point ARN the
/// start named.
async fn start_backup(router: &MultiServiceRouter, case: &str, resource_arn: &str) -> (String, String) {
    let body = json!({
        "BackupVaultName": VAULT,
        "ResourceArn": resource_arn,
        "IamRoleArn": role_arn(),
    })
    .to_string();
    let (status, started) = start_job(router, &body).await;
    // The call is ACCEPTED whatever the resource turns out to be: the backup fails later,
    // asynchronously, and this response is not where that shows.
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartBackupJob 200, got {status} {started:?}"
    );
    (
        string_at(&started, "BackupJobId"),
        string_at(&started, "RecoveryPointArn"),
    )
}

/// Read the job until it reports a terminal state, and answer that read.
async fn settled_job(router: &MultiServiceRouter, case: &str, job_id: &str) -> Value {
    for _ in 0..JOB_POLLS {
        let (status, read) = call(router, "GET", &format!("/backup-jobs/{job_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeBackupJob 200, got {status} {read:?}"
        );
        let state = string_at(&read, "State");
        if state == COMPLETED || state == FAILED {
            return read;
        }
        ::tokio::time::sleep(JOB_POLL_WAIT).await;
    }
    panic!("case {case:?}: the job {job_id} never reached a terminal state");
}

/// The `RecoveryPointArn`s the vault holds for `resource_arn`.
async fn points_of(router: &MultiServiceRouter, resource_arn: &str) -> Vec<String> {
    let path = format!("/backup-vaults/{VAULT}/recovery-points?resourceArn={resource_arn}");
    let (status, listed) = call(router, "GET", &path, None).await;
    assert_eq!(
        status, 200,
        "expected ListRecoveryPointsByBackupVault 200 for {resource_arn:?}, got {status} \
         {listed:?}"
    );
    listed["RecoveryPoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| string_at(point, "RecoveryPointArn"))
        .collect()
}

/// The jobs one `ListBackupJobs` page reports, by id, for a query of `filter`.
async fn listed_jobs(router: &MultiServiceRouter, filter: &str) -> Vec<String> {
    let (status, listed) = call(router, "GET", &format!("/backup-jobs?{filter}"), None).await;
    assert_eq!(
        status, 200,
        "expected ListBackupJobs 200 for {filter:?}, got {status} {listed:?}"
    );
    listed["BackupJobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|job| string_at(job, "BackupJobId"))
        .collect()
}

/// RULE: a started backup job's terminal state follows whether the service that owns its
/// `ResourceArn` holds that resource — `FAILED` with the resource-not-found message and
/// category and no recovery point when that service reports it absent, `COMPLETED` with the
/// point it wrote otherwise — and the call itself is accepted either way.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_jobs_terminal_state_follows_whether_its_protected_resource_exists() {
    let router = emulator();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );

    // Each row's resource, and the job started for it. The table a row needs is created —
    // and, for the row whose resource is gone, deleted again — through the owning service's
    // own wire before the backup starts.
    let cases = cases();
    let mut started = Vec::new();
    for case in &cases {
        let resource = match case.table {
            Some(table) => {
                let arn = source_table(&router, table, false).await;
                if case.dropped {
                    drop_table(&router, table).await;
                }
                arn
            }
            None => case.named_resource.clone(),
        };
        let (job_id, point_arn) = start_backup(&router, case.name, &resource).await;
        started.push((resource, job_id, point_arn));
    }

    let mut failed_jobs = Vec::new();
    for (case, (resource, job_id, point_arn)) in cases.iter().zip(&started) {
        let settled = settled_job(&router, case.name, job_id).await;
        assert_eq!(
            string_at(&settled, "State"),
            case.expected_state,
            "case {:?}: expected State {}, got {settled:?}",
            case.name,
            case.expected_state
        );

        let points = points_of(&router, resource).await;
        let (status, point) = call(
            &router,
            "GET",
            &format!("/backup-vaults/{VAULT}/recovery-points/{point_arn}"),
            None,
        )
        .await;
        if case.expected_state == FAILED {
            failed_jobs.push(job_id.clone());
            assert_eq!(
                (
                    string_at(&settled, "StatusMessage").as_str(),
                    string_at(&settled, "MessageCategory").as_str()
                ),
                (
                    format!("Requested resource not found: ResourceArn: {resource} not found")
                        .as_str(),
                    RESOURCE_NOT_FOUND
                ),
                "case {:?}: expected the resource-not-found message and category, got {settled:?}",
                case.name
            );
            // The backup nothing captured wrote no recovery point, so the vault holds none
            // for this resource — while the job still names the one its start answered with,
            // which therefore resolves to nothing.
            assert!(
                points.is_empty(),
                "case {:?}: a failed job leaves the vault no recovery point, got {points:?}",
                case.name
            );
            assert_eq!(
                (
                    status,
                    error_code(&point).as_str(),
                    string_at(&point, "Message").as_str()
                ),
                (400, "ResourceNotFoundException", RECOVERY_POINT_NOT_FOUND),
                "case {:?}: the recovery point of a failed job resolves to nothing, got {status} \
                 {point:?}",
                case.name
            );
        } else {
            assert_eq!(
                (settled.get("StatusMessage"), settled.get("MessageCategory")),
                (None, None),
                "case {:?}: a job that captured its resource reports neither a status message \
                 nor a category, got {settled:?}",
                case.name
            );
            assert_eq!(
                points,
                vec![point_arn.clone()],
                "case {:?}: expected the recovery point the job wrote",
                case.name
            );
            assert_eq!(
                (status, string_at(&point, "Status").as_str()),
                (200, COMPLETED),
                "case {:?}: the point the job wrote reads back completed, got {status} {point:?}",
                case.name
            );
        }
    }

    // A SECOND reader of the same jobs: the state-filtered listing selects exactly the jobs
    // each terminal state belongs to, and the category filter selects exactly the jobs that
    // failed in that category.
    for (filter, expected) in [
        (format!("state={FAILED}"), failed_jobs.clone()),
        (
            format!("messageCategory={RESOURCE_NOT_FOUND}"),
            failed_jobs.clone(),
        ),
        (
            "messageCategory=*".to_string(),
            started
                .iter()
                .map(|(_, job_id, _)| job_id.clone())
                .collect(),
        ),
    ] {
        let listed = listed_jobs(&router, &filter).await;
        assert_eq!(
            listed.len(),
            expected.len(),
            "filter {filter:?}: expected {} jobs, got {listed:?}",
            expected.len()
        );
        for job_id in &expected {
            assert!(
                listed.contains(job_id),
                "filter {filter:?}: expected the listing to report {job_id:?}, got {listed:?}"
            );
        }
    }
}
