// SPDX-License-Identifier: CC0-1.0

//! `StartRestoreJob` and the three calls that address the restore job it
//! starts (`DescribeRestoreJob`, `GetRestoreJobMetadata`,
//! `PutRestoreValidationResult`).
//!
//! The rules under test:
//!
//! * the recovery point is an INPUT to the restore, so a `RecoveryPointArn`
//!   that resolves to no stored recovery point is refused 400
//!   `InvalidParameterValueException` with the recovery-point sentence, and no
//!   restore job is persisted — every way an ARN can fail to name a stored point
//!   is that one condition;
//! * the restore job is a RESOURCE, so a `RestoreJobId` naming none is refused
//!   400 `ResourceNotFoundException` with the restore-job sentence by all three
//!   calls that address one: a client can tell a bad recovery-point reference
//!   from a bad restore-job reference;
//! * an accepted start stores a job that every one of those calls answers from:
//!   it is reported in the non-terminal status first, the clock alone moves it
//!   to its terminal status, the metadata read reports the KEYS the start was
//!   given with their values masked, and a validation result the caller records
//!   is what the read reports back;
//! * a restore PRODUCES a resource: the restore of a DynamoDB table recovery
//!   point creates that table in DynamoDB, and the `CreatedResourceArn` the job
//!   reports is the ARN DynamoDB itself answers for the table it now holds — so
//!   a restore of a type this emulator cannot perform reports no created
//!   resource and ends in the modelled failure status instead.
//!
//! Every case drives real HTTP through the router; the harness is the shared one
//! in `vault_and_backup_job.rs`, with the engine-keeping router of
//! `recovery_point_deletion.rs` where a case has to look at what was stored, and
//! DynamoDB mounted beside Backup on one engine where the restore has to create
//! a table through the service that owns it.

use std::sync::Arc;
use std::thread::sleep;

use serde_json::{json, Value};
use service_com_amazonaws_backup_cryocontrollerusermanager::ComAmazonawsBackupCryocontrollerusermanagerState as BackupState;
use smithy_server_runtime::MultiServiceRouter;

use crate::recovery_point_deletion::{router_with_engine, start_point};
use crate::vault_and_backup_job::{call, create_vault, error_code, string_at, ACCOUNT, PAST_SETTLE, REGION};

/// The sentence this service states for "the recovery point named is not one
/// this account holds" — fixed text, naming no part of the request.
const RECOVERY_POINT_NOT_FOUND: &str = "Cannot find recovery point";

/// The sentence it states for "the restore job asked about is not one this
/// account holds" — likewise fixed.
const RESTORE_JOB_NOT_FOUND: &str = "Cannot find restore job";

/// `RestoreJobStatus` a started job is reported in before the clock moves it.
const PENDING: &str = "PENDING";

/// `RestoreJobStatus` the clock moves a started job to.
const COMPLETED: &str = "COMPLETED";

/// `RestoreJobStatus` a restore that created no resource ends in.
const FAILED: &str = "FAILED";

/// What a read of a restore job's sensitive `Metadata` reports in place of each
/// stored value.
const METADATA_MASK: &str = "VALUE_NOT_DISPLAYED";

/// Longer than a restore job's settle span, so a read after this wait observes
/// the terminal status the clock has already made due.
const PAST_RESTORE_SETTLE: std::time::Duration = std::time::Duration::from_millis(3_400);

/// How long a case waits, in all, for a restore started as its own task to
/// report what it came to.
const RESTORE_POLLS: usize = 60;
const RESTORE_POLL_WAIT: std::time::Duration = std::time::Duration::from_millis(200);

/// Read the restore job until it reports a terminal status, and answer that
/// read. A restore runs as its own task, so a case waits for it the way a
/// client does rather than assuming it has finished.
///
/// `pub(crate)` for the sibling suite covering the LISTING of these jobs
/// (`restore_job_listing.rs`), which waits for the same condition.
pub(crate) async fn settled_restore(router: &MultiServiceRouter, job_id: &str) -> Value {
    for _ in 0..RESTORE_POLLS {
        let (status, read) = describe_restore(router, job_id).await;
        assert_eq!(
            status, 200,
            "polling {job_id}: expected DescribeRestoreJob 200, got {status} {read:?}"
        );
        let reported = string_at(&read, "Status");
        if reported == COMPLETED || reported == FAILED {
            return read;
        }
        sleep(RESTORE_POLL_WAIT);
    }
    panic!("the restore {job_id} never reached a terminal status");
}

/// The role a restore would run as.
fn role_arn() -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/service-role/AWSBackupDefaultServiceRole")
}

/// `PUT /restore-jobs`, as the model binds the start.
async fn start_restore(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "PUT", "/restore-jobs", Some(&body.to_string())).await
}

/// `GET /restore-jobs/{RestoreJobId}`.
async fn describe_restore(router: &MultiServiceRouter, job_id: &str) -> (u16, Value) {
    call(router, "GET", &format!("/restore-jobs/{job_id}"), None).await
}

/// `GET /restore-jobs/{RestoreJobId}/metadata`.
async fn restore_metadata(router: &MultiServiceRouter, job_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/restore-jobs/{job_id}/metadata"),
        None,
    )
    .await
}

/// `PUT /restore-jobs/{RestoreJobId}/validations`.
async fn put_validation(
    router: &MultiServiceRouter,
    job_id: &str,
    status: &str,
    message: &str,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!("/restore-jobs/{job_id}/validations"),
        Some(
            &json!({"ValidationStatus": status, "ValidationStatusMessage": message}).to_string(),
        ),
    )
    .await
}

/// How many restore jobs the store holds — the state a refused start must not
/// have written to.
///
/// `pub(crate)` for the sibling suite covering the metadata a start has to carry
/// (`restore_metadata_validation.rs`), which asserts the same "nothing was
/// stored" through this one seam.
pub(crate) fn stored_restore_jobs(engine: &engine::InMemoryEngine) -> usize {
    let state = engine.state::<BackupState>();
    let count = state.lock().restore_jobs.len();
    count
}

/// A start body for `recovery_point_arn`, with the metadata a DynamoDB restore
/// carries.
fn start_body(recovery_point_arn: &str, resource_type: Option<&str>, target: &str) -> Value {
    let mut body = json!({
        "RecoveryPointArn": recovery_point_arn,
        "Metadata": {"targetTableName": target},
        "IamRoleArn": role_arn(),
    });
    if let Some(resource_type) = resource_type {
        body["ResourceType"] = json!(resource_type);
    }
    body
}

/// Every `RecoveryPointArn` that resolves to no stored recovery point is ONE
/// condition: the parameter-value refusal with the recovery-point sentence, and
/// nothing is stored.
///
/// The rows span the ways an ARN can fail to name a stored point — fabricated,
/// another partition's, a backup that has not finished, a point already deleted
/// — rather than one fabricated ARN written twice. An ARN of another ACCOUNT is
/// not one of them: the authorization decision refuses it before the point is
/// resolved, which `recovery_point_cross_account_authorization.rs` covers.
#[tokio::test]
async fn a_recovery_point_arn_that_names_no_stored_point_is_one_parameter_refusal() {
    let (router, engine) = router_with_engine();
    for vault in ["restore-source", "other-vault"] {
        let (status, created) = create_vault(&router, vault, "{}").await;
        assert_eq!(
            status, 200,
            "setup {vault:?}: CreateBackupVault should succeed, got {status} {created:?}"
        );
    }
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    // A point whose backup is still running, started last so it is still
    // unsettled when the cases run.
    let settled = start_point(&router, "restore-source", &resource, None).await;
    let gone = start_point(&router, "other-vault", &resource, None).await;
    sleep(PAST_SETTLE);
    let (status, _) = call(
        &router,
        "DELETE",
        &format!("/backup-vaults/other-vault/recovery-points/{gone}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "setup: the second point should be deletable");
    let unsettled = start_point(&router, "restore-source", &resource, None).await;

    let fabricated = format!("arn:aws:backup:{REGION}:{ACCOUNT}:recovery-point:does-not-exist");
    let other_partition = format!("arn:aws-cn:backup:{REGION}:{ACCOUNT}:recovery-point:1EB3B5E7");
    let cases: Vec<(&str, &str)> = vec![
        // The unsettled point comes first: the clock settles it 500 ms after
        // its backup started, so the row that needs it unsettled runs before
        // the rest.
        ("a backup that has not finished", unsettled.as_str()),
        ("an ARN nothing answers to", fabricated.as_str()),
        ("an ARN of another partition", other_partition.as_str()),
        ("a point already deleted", gone.as_str()),
    ];

    for (case, recovery_point_arn) in cases {
        let (status, refused) =
            start_restore(&router, &start_body(recovery_point_arn, Some("DynamoDB"), "restored")).await;
        assert_eq!(
            (
                status,
                error_code(&refused).as_str(),
                string_at(&refused, "Message").as_str()
            ),
            (400, "InvalidParameterValueException", RECOVERY_POINT_NOT_FOUND),
            "case {case:?} ({recovery_point_arn}): expected the parameter refusal, got {status} \
             {refused:?}"
        );
        assert_eq!(
            stored_restore_jobs(&engine),
            0,
            "case {case:?}: a refused start must persist no restore job"
        );
    }

    // The positive control: the stored point's own ARN starts a restore, which
    // is what makes the refusals above about resolution and not about the call.
    let (status, started) =
        start_restore(&router, &start_body(&settled, Some("DynamoDB"), "restored")).await;
    assert_eq!(
        (status, error_code(&started).as_str()),
        (200, "<none>"),
        "the stored point's own ARN must be accepted, got {status} {started:?}"
    );
    assert_eq!(
        stored_restore_jobs(&engine),
        1,
        "the accepted start must persist exactly one restore job"
    );
}

/// A `RecoveryPointArn` that is not an ARN at all is refused as a bad parameter
/// value too — the same class, a different sentence — and stores nothing.
#[tokio::test]
async fn a_recovery_point_arn_that_is_not_an_arn_is_a_bad_parameter_value() {
    // The form an ARN takes:
    // `arn:<partition>:<service>:<region>:<account>:<resource>`, as the model's
    // own doc for `RecoveryPointArn` spells it out (the member is typed `ARN`
    // with no pattern, so the service applies the rule).
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

    let (router, engine) = router_with_engine();
    for (case, value) in cases {
        let (status, refused) =
            start_restore(&router, &start_body(&value, Some("DynamoDB"), "restored")).await;
        assert_eq!(
            (status, error_code(&refused).as_str()),
            (400, "InvalidParameterValueException"),
            "case {case:?} ({value:?}): expected the parameter refusal, got {status} {refused:?}"
        );
        assert_eq!(
            stored_restore_jobs(&engine),
            0,
            "case {case:?}: a refused start must persist no restore job"
        );
    }
}

/// One emulator: Backup and DynamoDB mounted on ONE shared engine, so a restore
/// performed against DynamoDB reaches the same store a DynamoDB client reads.
///
/// `pub(crate)` for the sibling suite covering what a backup job's terminal state
/// follows (`backup_job_resource_resolution.rs`): the protected resource is
/// resolved against the service that owns it, so that suite needs the same two
/// services on one engine rather than a second copy of this router.
pub(crate) fn emulator() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine),
    );
    router
}

/// One DynamoDB operation (`awsJson1_0`, target prefix `DynamoDB_20120810`), the
/// way a client makes it.
async fn dynamodb(router: &MultiServiceRouter, operation: &str, body: Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("http://localhost/")
        .header("host", format!("dynamodb.{REGION}.amazonaws.com"))
        .header("content-type", "application/x-amz-json-1.0")
        .header("x-amz-target", format!("DynamoDB_20120810.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{REGION}/dynamodb/\
                 aws4_request, SignedHeaders=host, Signature=00"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let decoded = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, decoded)
}

/// Create a DynamoDB table to be backed up, and answer the ARN DynamoDB gives
/// it. `provisioned` picks which billing mode the source table is in, which is
/// the mode the restored table has to be created in too.
pub(crate) async fn source_table(
    router: &MultiServiceRouter,
    name: &str,
    provisioned: bool,
) -> String {
    let mut body = json!({
        "TableName": name,
        "KeySchema": [
            {"AttributeName": "pk", "KeyType": "HASH"},
            {"AttributeName": "sk", "KeyType": "RANGE"},
        ],
        "AttributeDefinitions": [
            {"AttributeName": "pk", "AttributeType": "S"},
            {"AttributeName": "sk", "AttributeType": "N"},
        ],
    });
    if provisioned {
        body["ProvisionedThroughput"] =
            json!({"ReadCapacityUnits": 3, "WriteCapacityUnits": 4});
    } else {
        body["BillingMode"] = json!("PAY_PER_REQUEST");
    }
    let (status, created) = dynamodb(router, "CreateTable", body).await;
    assert_eq!(
        status, 200,
        "setup: CreateTable {name} should succeed, got {status} {created:?}"
    );
    string_at(
        created
            .get("TableDescription")
            .unwrap_or(&Value::Null),
        "TableArn",
    )
}

/// Delete the DynamoDB table `name` through that service's own wire, and wait until that
/// service reports it gone.
///
/// Both ends of the deletion are waited for as CONDITIONS, because a table of that service
/// is `CREATING` for a span before it can be deleted and `DELETING` for a span before it is
/// gone: the delete is re-asked while the table is still being created, and the table is
/// read back until the service answers that it holds no such table.
///
/// `pub(crate)` for the sibling suites: a case that needs a protected resource the owning
/// service NO LONGER holds backs the table up while it is there and drops it afterwards,
/// because a backup started for a table DynamoDB reports absent produces no recovery point
/// at all.
pub(crate) async fn drop_table(router: &MultiServiceRouter, name: &str) {
    let mut deleted = None;
    for _ in 0..TABLE_POLLS {
        let (status, answer) = dynamodb(router, "DeleteTable", json!({"TableName": name})).await;
        if status == 200 {
            deleted = Some(answer);
            break;
        }
        ::tokio::time::sleep(TABLE_POLL_WAIT).await;
    }
    assert!(
        deleted.is_some(),
        "setup: DeleteTable {name} should succeed once the table is no longer being created"
    );
    for _ in 0..TABLE_POLLS {
        let (status, read) = dynamodb(router, "DescribeTable", json!({"TableName": name})).await;
        if status != 200 {
            assert!(
                read.to_string().contains("ResourceNotFoundException"),
                "setup: DescribeTable {name} must answer that the table is gone, got {status} \
                 {read:?}"
            );
            return;
        }
        ::tokio::time::sleep(TABLE_POLL_WAIT).await;
    }
    panic!("setup: the table {name} was never reported gone");
}

/// How long the waits above take in all, and how long each look waits before the next: well
/// past the span that service's own clock takes to create or delete a table, so a loaded
/// host does not fail a case.
const TABLE_POLLS: usize = 150;
const TABLE_POLL_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

/// A restore of a DynamoDB table recovery point CREATES that table in DynamoDB:
/// the job reports the ARN DynamoDB answered with, DynamoDB itself reports the
/// table, and the restored table has the source table's key schema.
///
/// The rows differ along what the rule generalises over: the billing mode the
/// source table is in (so the restored table is created in the same one) and
/// whether the start asks for the backup's tags to be copied.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_creates_the_table_in_the_service_that_owns_it() {
    let cases: Vec<(&str, bool, bool)> = vec![
        ("a pay-per-request source table", false, false),
        ("a provisioned source table", true, false),
        ("with the source tags copied", false, true),
    ];

    for (case, provisioned, copy_tags) in cases {
        let router = emulator();
        let (status, created) = create_vault(&router, "restore-source", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );
        let table_arn = source_table(&router, "orders", provisioned).await;
        let point = start_point(&router, "restore-source", &table_arn, None).await;
        sleep(PAST_SETTLE);

        let mut body = start_body(&point, Some("DynamoDB"), "orders-restored");
        if copy_tags {
            body["CopySourceTagsToRestoredResource"] = json!(true);
        }
        let (status, started) = start_restore(&router, &body).await;
        assert_eq!(
            (status, error_code(&started).as_str()),
            (200, "<none>"),
            "case {case:?}: expected StartRestoreJob 200, got {status} {started:?}"
        );
        let job_id = string_at(&started, "RestoreJobId");

        // Straight after the start the restore has not finished: the status is
        // the non-terminal one AWS answers with.
        let (status, read) = describe_restore(&router, &job_id).await;
        assert_eq!(status, 200, "case {case:?}: expected the read to answer 200");
        assert_eq!(
            string_at(&read, "Status"),
            PENDING,
            "case {case:?}: a restore that has not finished is reported pending, got {read:?}"
        );
        assert!(
            read.get("CompletionDate").is_none(),
            "case {case:?}: a restore that has not ended reports no completion, got {read:?}"
        );

        // The job reaches its terminal status once the restore has created the
        // resource, and what it reports created is the ARN the owning service
        // answered with.
        let ended = settled_restore(&router, &job_id).await;
        assert_eq!(
            string_at(&ended, "Status"),
            COMPLETED,
            "case {case:?}: the restore must complete, got {ended:?}"
        );

        // The restored table is held by DYNAMODB, which is what makes the
        // restore real: it is read back through that service's own wire.
        let (status, described) = dynamodb(
            &router,
            "DescribeTable",
            json!({"TableName": "orders-restored"}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the restored table must exist in DynamoDB, got {status} {described:?}"
        );
        let table = described.get("Table").cloned().unwrap_or(Value::Null);
        assert_eq!(
            table.get("KeySchema").cloned().unwrap_or(Value::Null),
            json!([
                {"AttributeName": "pk", "KeyType": "HASH"},
                {"AttributeName": "sk", "KeyType": "RANGE"},
            ]),
            "case {case:?}: the restored table must have the source table's key schema, got \
             {described:?}"
        );
        let restored_arn = string_at(&table, "TableArn");

        assert_eq!(
            (
                string_at(&ended, "CreatedResourceArn").as_str(),
                string_at(&ended, "PercentDone").as_str()
            ),
            (restored_arn.as_str(), "100.0"),
            "case {case:?}: the ended job must report the resource it created, got {ended:?}"
        );
        assert!(
            ended.get("CompletionDate").is_some(),
            "case {case:?}: the ended job reports when it ended, got {ended:?}"
        );
        assert_eq!(
            string_at(&ended, "SourceResourceArn"),
            table_arn,
            "case {case:?}: the job must name the resource it restored from, got {ended:?}"
        );
    }
}

/// A restore of a resource type this emulator performs no restore of creates
/// nothing, so the job ends in the modelled failure status carrying the reason
/// and reports no created resource.
///
/// The rows span types the recovery-point side does store but the restore side
/// cannot re-create, and the type whose restore CAN be performed but whose
/// source table is not there to read — that last one is backed up while the
/// table exists and the table is dropped before the restore, since a backup
/// started for a table DynamoDB reports absent captures nothing and writes no
/// recovery point to restore from.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_restore_that_creates_nothing_ends_in_the_modelled_failure_status() {
    // (case, the table to create and then drop, the protected resource when no
    // table backs the row, the type the start names)
    let cases: Vec<(&str, Option<&str>, String, Option<&str>)> = vec![
        (
            "an EFS file system",
            None,
            format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-01"),
            Some("EFS"),
        ),
        (
            "an S3 bucket",
            None,
            "arn:aws:s3:::demo-bucket".to_string(),
            Some("S3"),
        ),
        (
            "a DynamoDB table that is no longer there",
            Some("orders-dropped"),
            String::new(),
            Some("DynamoDB"),
        ),
    ];

    for (case, dropped_table, named_resource, resource_type) in cases {
        let router = emulator();
        let (status, created) = create_vault(&router, "restore-source", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );
        // A row whose restore needs a source table that is GONE backs up the
        // table while DynamoDB still holds it.
        let resource = match dropped_table {
            Some(table) => source_table(&router, table, false).await,
            None => named_resource,
        };
        let point = start_point(&router, "restore-source", &resource, None).await;
        sleep(PAST_SETTLE);
        if let Some(table) = dropped_table {
            drop_table(&router, table).await;
        }

        let (status, started) =
            start_restore(&router, &start_body(&point, resource_type, "restored")).await;
        assert_eq!(
            (status, error_code(&started).as_str()),
            (200, "<none>"),
            "case {case:?}: expected StartRestoreJob 200, got {status} {started:?}"
        );
        let job_id = string_at(&started, "RestoreJobId");

        let ended = settled_restore(&router, &job_id).await;
        assert_eq!(
            string_at(&ended, "Status"),
            FAILED,
            "case {case:?}: a restore that created nothing must say so, got {ended:?}"
        );
        assert!(
            !string_at(&ended, "StatusMessage").is_empty(),
            "case {case:?}: the failed job must carry the reason, got {ended:?}"
        );
        assert!(
            ended.get("CreatedResourceArn").is_none(),
            "case {case:?}: nothing was created, so no resource may be reported, got {ended:?}"
        );
        assert_eq!(
            string_at(&ended, "PercentDone"),
            "0.0",
            "case {case:?}: a restore that created nothing is not partly done, got {ended:?}"
        );

        // The clock does not move a job that has already ended.
        sleep(PAST_RESTORE_SETTLE);
        let (_, again) = describe_restore(&router, &job_id).await;
        assert_eq!(
            string_at(&again, "Status"),
            FAILED,
            "case {case:?}: a terminal job keeps its status, got {again:?}"
        );
    }
}

/// An accepted start stores a job every call that names its id answers from:
/// the read reports the point and the vault it resolved, the metadata read
/// reports the keys the start was given with their values masked, and the
/// validation result a caller records is what the read reports back.
///
/// The rows differ along what the rule generalises over: the protected resource
/// the point was taken of, whether the start names a `ResourceType` at all, and
/// the metadata it carries.
#[tokio::test]
async fn an_accepted_restore_stores_a_job_every_read_of_it_answers_from() {
    let cases: Vec<(&str, String, Option<&str>, &str)> = vec![
        (
            "a DynamoDB table with the type named",
            format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo"),
            Some("DynamoDB"),
            "restored-demo",
        ),
        (
            "an EFS file system with the type derived from the point",
            format!("arn:aws:elasticfilesystem:{REGION}:{ACCOUNT}:file-system/fs-01"),
            None,
            "restored-fs",
        ),
        (
            "an S3 bucket",
            "arn:aws:s3:::demo-bucket".to_string(),
            Some("S3"),
            "restored-bucket",
        ),
    ];

    for (case, resource, resource_type, target) in cases {
        let (router, _engine) = router_with_engine();
        let (status, created) = create_vault(&router, "restore-source", "{}").await;
        assert_eq!(
            status, 200,
            "case {case:?}: setup CreateBackupVault should succeed, got {status} {created:?}"
        );
        let point = start_point(&router, "restore-source", &resource, None).await;
        sleep(PAST_SETTLE);

        let (status, started) =
            start_restore(&router, &start_body(&point, resource_type, target)).await;
        assert_eq!(
            (status, error_code(&started).as_str()),
            (200, "<none>"),
            "case {case:?}: expected StartRestoreJob 200, got {status} {started:?}"
        );
        let job_id = string_at(&started, "RestoreJobId");
        assert!(
            !job_id.is_empty(),
            "case {case:?}: the start must answer with a RestoreJobId, got {started:?}"
        );

        // The job the start stored is the job every read answers from, whatever
        // the restore itself came to.
        let (status, read) = describe_restore(&router, &job_id).await;
        assert_eq!(
            (status, error_code(&read).as_str()),
            (200, "<none>"),
            "case {case:?}: expected DescribeRestoreJob 200, got {status} {read:?}"
        );
        assert_eq!(
            (
                string_at(&read, "RestoreJobId").as_str(),
                string_at(&read, "RecoveryPointArn").as_str(),
                string_at(&read, "SourceResourceArn").as_str(),
                string_at(&read, "BackupVaultArn").as_str(),
                string_at(&read, "AccountId").as_str(),
            ),
            (
                job_id.as_str(),
                point.as_str(),
                resource.as_str(),
                format!("arn:aws:backup:{REGION}:{ACCOUNT}:backup-vault:restore-source").as_str(),
                ACCOUNT,
            ),
            "case {case:?}: the read must answer from the job that was stored, got {read:?}"
        );
        assert!(
            read.get("CreatedResourceArn").is_none(),
            "case {case:?}: no resource was created here, so none may be reported, got {read:?}"
        );
        // The type is the one the start named, or the point's own when it named
        // none.
        let reported_type = string_at(&read, "ResourceType");
        assert!(
            !reported_type.is_empty(),
            "case {case:?}: the read must report the type being restored, got {read:?}"
        );
        if let Some(resource_type) = resource_type {
            assert_eq!(
                reported_type, resource_type,
                "case {case:?}: the read must report the type the start named"
            );
        }

        // The metadata read answers with the keys the start was given, each
        // value masked: the map is sensitive, so a read discloses which keys a
        // restore was configured with and not what they were set to.
        let (status, metadata) = restore_metadata(&router, &job_id).await;
        assert_eq!(
            (status, error_code(&metadata).as_str()),
            (200, "<none>"),
            "case {case:?}: expected GetRestoreJobMetadata 200, got {status} {metadata:?}"
        );
        assert_eq!(
            (
                string_at(&metadata, "RestoreJobId").as_str(),
                metadata.get("Metadata").cloned().unwrap_or(Value::Null)
            ),
            (
                job_id.as_str(),
                json!({"targetTableName": METADATA_MASK})
            ),
            "case {case:?}: the metadata read must report the start's keys, masked, got \
             {metadata:?}"
        );
        assert!(
            !metadata.to_string().contains(target),
            "case {case:?}: a masked read must disclose no metadata value, got {metadata:?}"
        );

        // A validation result the caller records is what the read reports back.
        let (status, recorded) =
            put_validation(&router, &job_id, "SUCCESSFUL", "checked by the client").await;
        assert_eq!(
            (status, error_code(&recorded).as_str()),
            (204, "<none>"),
            "case {case:?}: expected PutRestoreValidationResult 204, got {status} {recorded:?}"
        );
        let (_, validated) = describe_restore(&router, &job_id).await;
        assert_eq!(
            (
                string_at(&validated, "ValidationStatus").as_str(),
                string_at(&validated, "ValidationStatusMessage").as_str()
            ),
            ("SUCCESSFUL", "checked by the client"),
            "case {case:?}: the read must report the validation result recorded, got {validated:?}"
        );

        // A later result replaces the one before it: a restore has one.
        let (status, replaced) =
            put_validation(&router, &job_id, "FAILED", "re-checked by the client").await;
        assert_eq!(
            status, 204,
            "case {case:?}: a second validation result is accepted, got {status} {replaced:?}"
        );
        let (_, revalidated) = describe_restore(&router, &job_id).await;
        assert_eq!(
            string_at(&revalidated, "ValidationStatus"),
            "FAILED",
            "case {case:?}: the latest validation result is the one in force, got {revalidated:?}"
        );
    }
}

/// A `RestoreJobId` naming no stored restore job is the OTHER class: every call
/// that addresses a restore job answers 400 `ResourceNotFoundException` with the
/// restore-job sentence, whichever id form is asked about.
#[tokio::test]
async fn a_restore_job_id_that_names_no_job_is_not_found_by_every_read() {
    let (router, _engine) = router_with_engine();
    let (status, created) = create_vault(&router, "restore-source", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let point = start_point(&router, "restore-source", &resource, None).await;
    sleep(PAST_SETTLE);
    let (status, started) = start_restore(&router, &start_body(&point, Some("DynamoDB"), "t")).await;
    assert_eq!(status, 200, "setup: the restore should start, got {started:?}");
    let live = string_at(&started, "RestoreJobId");

    let ids = [
        ("an id in the form this service mints", "AAAAAAAA-1111-2222-3333-444444444444"),
        ("an id of another form", "no-such-restore-job"),
        ("a single character", "x"),
    ];
    for (case, job_id) in ids {
        for (call_name, answer) in [
            ("DescribeRestoreJob", describe_restore(&router, job_id).await),
            ("GetRestoreJobMetadata", restore_metadata(&router, job_id).await),
            (
                "PutRestoreValidationResult",
                put_validation(&router, job_id, "SUCCESSFUL", "checked").await,
            ),
        ] {
            let (status, refused) = answer;
            assert_eq!(
                (
                    status,
                    error_code(&refused).as_str(),
                    string_at(&refused, "Message").as_str()
                ),
                (400, "ResourceNotFoundException", RESTORE_JOB_NOT_FOUND),
                "case {case:?} on {call_name}: expected the not-found refusal, got {status} \
                 {refused:?}"
            );
        }
    }

    // The job that DOES exist is untouched by every refusal above.
    let (status, read) = describe_restore(&router, &live).await;
    assert_eq!(
        (status, string_at(&read, "RestoreJobId")),
        (200, live),
        "the stored job must still be readable, got {read:?}"
    );
}

/// A repeated `IdempotencyToken` takes no second action: the same restore job
/// id comes back and only one job is stored.
#[tokio::test]
async fn a_repeated_idempotency_token_replays_the_first_restore() {
    let (router, engine) = router_with_engine();
    let (status, created) = create_vault(&router, "restore-source", "{}").await;
    assert_eq!(
        status, 200,
        "setup: CreateBackupVault should succeed, got {status} {created:?}"
    );
    let resource = format!("arn:aws:dynamodb:{REGION}:{ACCOUNT}:table/demo");
    let point = start_point(&router, "restore-source", &resource, None).await;
    sleep(PAST_SETTLE);

    let mut body = start_body(&point, Some("DynamoDB"), "restored-demo");
    body["IdempotencyToken"] = json!("one-restore-only");
    let (status, first) = start_restore(&router, &body).await;
    assert_eq!(status, 200, "the first start should succeed, got {first:?}");
    let (status, second) = start_restore(&router, &body).await;
    assert_eq!(status, 200, "the replay should succeed, got {second:?}");
    assert_eq!(
        string_at(&first, "RestoreJobId"),
        string_at(&second, "RestoreJobId"),
        "a replayed token must answer with the first job's id"
    );
    assert_eq!(
        stored_restore_jobs(&engine),
        1,
        "a replayed token must not start a second restore"
    );

    // A different token on the same recovery point IS a second restore.
    body["IdempotencyToken"] = json!("another-restore");
    let (status, third) = start_restore(&router, &body).await;
    assert_eq!(status, 200, "a second token should start a restore, got {third:?}");
    assert_ne!(
        string_at(&first, "RestoreJobId"),
        string_at(&third, "RestoreJobId"),
        "a different token must start a different restore"
    );
    assert_eq!(
        stored_restore_jobs(&engine),
        2,
        "two tokens must leave two restore jobs stored"
    );
}
