// SPDX-License-Identifier: CC0-1.0

//! `ListRestoreJobs`: the page of restore jobs this account holds.
//!
//! The RULES under test:
//!
//! * the listing reports exactly the jobs `StartRestoreJob` stored, each projected through
//!   the same stored record `DescribeRestoreJob` answers from, so a listed job and a
//!   described one never disagree about one restore;
//! * every `By*` filter the model declares selects the stored jobs it names and no others —
//!   the rows below span the status, the resource type, the account and the creation window,
//!   plus the filters nothing in this slice can match;
//! * a page is `MaxResults` items plus a cursor that resumes exactly where the page stopped,
//!   so every page size yields every stored job exactly once.
//!
//! Every case drives real HTTP through the router: the Backup wire is the shared harness in
//! `vault_and_backup_job.rs`, and DynamoDB is mounted beside Backup on one engine
//! (`restore_job.rs`'s `emulator`), because one of the two restores listed here CREATES its
//! table in the service that owns it and so reaches its completed status, while the other
//! creates nothing and reaches the failure one — two statuses for the status filter to tell
//! apart.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_point_deletion::completed_point;
use crate::restore_job::{emulator, settled_restore, source_table};
use crate::vault_and_backup_job::{call, create_vault, error_code, string_at, ACCOUNT, REGION};

/// The vault both recovery points of this suite are backed up into.
const VAULT: &str = "restore-listing-source";

/// The two terminal `RestoreJobStatus` values the listed jobs reach, and the non-terminal
/// one neither is in by then (model enum `RestoreJobStatus`).
const COMPLETED: &str = "COMPLETED";
const FAILED: &str = "FAILED";
const PENDING: &str = "PENDING";

/// An account this emulator holds no resources of, for the account filter.
const OTHER_ACCOUNT: &str = "000000000001";

/// Two instants either side of every job of this suite, as the `@httpQuery` timestamp
/// bindings spell one (RFC 3339 date-time). Fixed literals rather than clock arithmetic:
/// the window is what is under test, not how fast the host runs.
const BEFORE_EVERY_JOB: &str = "1970-01-01T00:00:00Z";
const AFTER_EVERY_JOB: &str = "2970-01-01T00:00:00Z";

/// `PUT /restore-jobs`, as the model binds the start; answers the `RestoreJobId`.
async fn start_restore(
    router: &MultiServiceRouter,
    recovery_point_arn: &str,
    resource_type: &str,
    target: &str,
) -> String {
    let body = json!({
        "RecoveryPointArn": recovery_point_arn,
        "ResourceType": resource_type,
        "Metadata": {"targetTableName": target},
        "IamRoleArn": format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole"),
    })
    .to_string();
    let (status, started) = call(router, "PUT", "/restore-jobs", Some(&body)).await;
    assert_eq!(
        status, 200,
        "setup: expected StartRestoreJob 200, got {status} {started:?}"
    );
    string_at(&started, "RestoreJobId")
}

/// One `ListRestoreJobs` page for a query of `filter`: the whole body, so a case can read
/// both the jobs and the cursor.
async fn page(router: &MultiServiceRouter, filter: &str) -> Value {
    let (status, listed) = call(router, "GET", &format!("/restore-jobs?{filter}"), None).await;
    assert_eq!(
        status, 200,
        "expected ListRestoreJobs 200 for {filter:?}, got {status} {listed:?}"
    );
    listed
}

/// The `RestoreJobId`s one page reports, in the order it reports them.
fn ids_of(listed: &Value) -> Vec<String> {
    listed["RestoreJobs"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|job| string_at(job, "RestoreJobId"))
        .collect()
}

/// RULE: the listing reports the stored restore jobs, each `By*` filter selects the ones it
/// names, and a page plus its cursor walks the whole result set exactly once.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_listing_reports_the_stored_restore_jobs_and_filters_them() {
    let router = emulator();
    let (status, created) = create_vault(&router, VAULT, "{}").await;
    assert_eq!(
        status, 200,
        "setup: expected CreateBackupVault 200, got {status} {created:?}"
    );

    // One restore that CREATES its table in the service that owns it, and one of a type
    // this emulator performs no restore of, so the two reach different terminal statuses.
    let table_arn = source_table(&router, "orders", false).await;
    let table_point = completed_point(&router, VAULT, &table_arn).await;
    let file_system = format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-01");
    let file_system_point = completed_point(&router, VAULT, &file_system).await;

    let table_job = start_restore(&router, &table_point, "DynamoDB", "orders-restored").await;
    let file_system_job = start_restore(&router, &file_system_point, "EFS", "unused").await;
    for (job_id, expected) in [(&table_job, COMPLETED), (&file_system_job, FAILED)] {
        let ended = settled_restore(&router, job_id).await;
        assert_eq!(
            string_at(&ended, "Status"),
            expected,
            "setup {job_id}: expected the restore to end {expected}, got {ended:?}"
        );
    }

    // The unfiltered page reports both jobs, and every member of each is the one the read
    // of the same job reports.
    let all = page(&router, "").await;
    let listed_ids = ids_of(&all);
    assert_eq!(
        listed_ids.len(),
        2,
        "the unfiltered page must report both stored jobs, got {all:?}"
    );
    for job_id in [&table_job, &file_system_job] {
        assert!(
            listed_ids.contains(job_id),
            "the page must report {job_id:?}, got {all:?}"
        );
        let listed = all["RestoreJobs"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|job| string_at(job, "RestoreJobId") == *job_id)
            .expect("the job the page reports");
        let (_, described) = call(&router, "GET", &format!("/restore-jobs/{job_id}"), None).await;
        for member in [
            "AccountId",
            "RestoreJobId",
            "RecoveryPointArn",
            "SourceResourceArn",
            "BackupVaultArn",
            "CreationDate",
            "CompletionDate",
            "Status",
            "StatusMessage",
            "PercentDone",
            "BackupSizeInBytes",
            "IamRoleArn",
            "CreatedResourceArn",
            "ResourceType",
            "RecoveryPointCreationDate",
            "IsParent",
        ] {
            assert_eq!(
                listed.get(member),
                described.get(member),
                "case {member:?}: the listed and the described job must agree, got {listed:?} \
                 and {described:?}"
            );
        }
    }

    // The filters: each row is a query and the jobs it must select.
    let both = vec![table_job.clone(), file_system_job.clone()];
    let rows: Vec<(String, Vec<String>)> = vec![
        (format!("status={COMPLETED}"), vec![table_job.clone()]),
        (format!("status={FAILED}"), vec![file_system_job.clone()]),
        // Neither job is still pending, so that status selects none.
        (format!("status={PENDING}"), Vec::new()),
        ("resourceType=DynamoDB".to_string(), vec![table_job.clone()]),
        (
            "resourceType=EFS".to_string(),
            vec![file_system_job.clone()],
        ),
        ("resourceType=S3".to_string(), Vec::new()),
        (format!("accountId={ACCOUNT}"), both.clone()),
        (format!("accountId={OTHER_ACCOUNT}"), Vec::new()),
        (format!("createdAfter={BEFORE_EVERY_JOB}"), both.clone()),
        (format!("createdAfter={AFTER_EVERY_JOB}"), Vec::new()),
        (format!("createdBefore={AFTER_EVERY_JOB}"), both.clone()),
        (format!("createdBefore={BEFORE_EVERY_JOB}"), Vec::new()),
        (format!("completeAfter={BEFORE_EVERY_JOB}"), both.clone()),
        (format!("completeBefore={BEFORE_EVERY_JOB}"), Vec::new()),
        // Nothing here is started by a restore testing plan or nested in another job.
        (
            format!("restoreTestingPlanArn=arn:aws:backup:{REGION}:{ACCOUNT}:restore-testing-plan:p"),
            Vec::new(),
        ),
        (format!("parentJobId={table_job}"), Vec::new()),
    ];
    for (filter, expected) in rows {
        let listed = page(&router, &filter).await;
        let ids = ids_of(&listed);
        assert_eq!(
            ids.len(),
            expected.len(),
            "filter {filter:?}: expected {} jobs, got {listed:?}",
            expected.len()
        );
        for job_id in &expected {
            assert!(
                ids.contains(job_id),
                "filter {filter:?}: expected the page to report {job_id:?}, got {listed:?}"
            );
        }
    }

    // A page of one, walked by its cursor, yields every stored job exactly once.
    let mut walked: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let filter = match &token {
            None => "maxResults=1".to_string(),
            Some(token) => format!("maxResults=1&nextToken={token}"),
        };
        let listed = page(&router, &filter).await;
        let ids = ids_of(&listed);
        assert_eq!(
            ids.len(),
            1,
            "filter {filter:?}: a page of one reports one job, got {listed:?}"
        );
        walked.extend(ids);
        token = listed
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        walked, listed_ids,
        "walking by a cursor must yield the unfiltered page's jobs, in its order"
    );

    // A cursor that names no position in the result set is a bad parameter value, not an
    // empty page.
    let (status, refused) = call(
        &router,
        "GET",
        "/restore-jobs?nextToken=6e6f742d612d637572736f72",
        None,
    )
    .await;
    assert_eq!(
        (status, error_code(&refused).as_str()),
        (400, "InvalidParameterValueException"),
        "a cursor naming no page is refused as a parameter value, got {status} {refused:?}"
    );
}
