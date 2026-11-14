// SPDX-License-Identifier: CC0-1.0

//! What a DataSync task execution between two Amazon S3 locations DOES, and the
//! cross-member validation its task and locations are accepted under.
//!
//! Every case goes through the same front door a client uses — `POST /` with
//! `X-Amz-Target: FmrsService.<Operation>` and an `awsJson1_1` body — against a router
//! that mounts DataSync beside this emulator's own Amazon S3 and IAM on ONE engine. So
//! the objects a transfer moves are real objects of the object store, staged and read
//! back through its own API, and the access a `PREPARING` step resolves is a real role
//! policy IAM holds: nothing here reaches into a state struct, and a response that is
//! not computed from persisted state cannot pass.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::{EmbeddedEmulator, MultiServiceRouter};

/// The Region every request in this suite is signed for, and the one its SDK clients
/// address: an ARN that carries it proves the ARN came from the request.
const REGION: &str = "us-west-2";

/// The one bucket access role these cases transfer under.
const ROLE_NAME: &str = "datasync-bucket-access";

/// That role's one inline policy.
const POLICY_NAME: &str = "bucket-access";

/// How long a case waits for an execution to reach a terminal status.
///
/// Generous on purpose: the only timing an assertion here depends on is that the
/// execution's phases eventually fall due, so a loaded or slow host must not change any
/// outcome.
const TERMINAL_DEADLINE: Duration = Duration::from_secs(60);

/// How often a poll loop looks again.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The objects a source bucket is staged with: three of them, with three distinct sizes,
/// so no byte counter can be a file count in disguise, and two under a common prefix so
/// a filter has something to partition.
const SOURCE_OBJECTS: &[(&str, &[u8])] = &[
    ("top.txt", b"the top level object"),
    ("nested/second.txt", b"the second object, under a prefix"),
    ("nested/third.bin", b"third"),
];

/// The two source objects under the `nested` prefix.
const NESTED_OBJECTS: &[(&str, &[u8])] = &[
    ("nested/second.txt", b"the second object, under a prefix"),
    ("nested/third.bin", b"third"),
];

/// The same three keys holding DIFFERENT content, for a destination that is already
/// populated: byte-for-byte distinguishable from the source, and a different length, so
/// a comparison cannot pass by accident.
const STALE_OBJECTS: &[(&str, &[u8])] = &[
    ("top.txt", b"stale top"),
    ("nested/second.txt", b"stale second"),
    ("nested/third.bin", b"stale third"),
];

/// How many bytes a set of objects holds.
fn total_bytes(objects: &[(&str, &[u8])]) -> i64 {
    objects.iter().map(|(_, body)| body.len() as i64).sum()
}

/// DataSync, Amazon S3 and IAM on one shared engine.
struct Aws {
    router: MultiServiceRouter,
}

impl Aws {
    /// Stand up the router with the three services mounted on a fresh engine.
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(Arc::clone(&engine)),
        );
        // Both sides of a transfer are buckets of THIS object store.
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // What a bucket access role allows is IAM's to answer.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router }
    }

    /// A handle to this emulator for driving it through an AWS SDK client: the same
    /// in-process seam the handlers reach the object store and IAM through.
    fn callback(&self) -> EmbeddedEmulator {
        EmbeddedEmulator::from_router(self.router.clone())
    }

    /// An Amazon S3 client over that seam, built the one way the tree builds it.
    fn s3(&self) -> ::aws_sdk_s3::Client {
        ::service_common::s3_client::s3_client(&self.callback(), REGION)
    }

    /// An IAM client over that same seam.
    fn iam(&self) -> ::aws_sdk_iam::Client {
        ::service_common::iam_client::iam_client_through(&self.callback(), REGION)
    }

    /// One `awsJson1_1` DataSync call, returning the HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("FmrsService.{operation}"))
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDDATASYNCTEST/20260101/{REGION}\
                     /datasync/aws4_request, SignedHeaders=host;x-amz-target, Signature=00"
                ),
            )
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = response.into_body();
        let parsed = if payload.is_empty() {
            json!({})
        } else {
            serde_json::from_slice(&payload)
                .unwrap_or_else(|_| json!({ "__unparsed": String::from_utf8_lossy(&payload) }))
        };
        (status, parsed)
    }

    /// A DataSync call that must succeed.
    async fn ok(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {operation} to answer 200, got {status} with {parsed}"
        );
        parsed
    }

    /// A DataSync call that must be refused with HTTP 400 `InvalidRequestException`,
    /// returning the refusal body.
    async fn invalid_request(&self, operation: &str, body: Value, case: &str) -> Value {
        let (status, parsed) = self.call(operation, body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected {operation} to answer 400, got {status} with {parsed}"
        );
        let wire_type = parsed["__type"].as_str().unwrap_or_default();
        assert!(
            wire_type.contains("InvalidRequestException"),
            "case {case:?}: expected an InvalidRequestException from {operation}, got {parsed}"
        );
        parsed
    }

    /// Create `bucket` and put every object of `objects` in it.
    async fn seed_bucket(&self, bucket: &str, objects: &[(&str, &[u8])], case: &str) {
        let created = self
            .s3()
            .create_bucket()
            .bucket(bucket)
            .create_bucket_configuration(
                ::aws_sdk_s3::types::CreateBucketConfiguration::builder()
                    .location_constraint(::aws_sdk_s3::types::BucketLocationConstraint::from(
                        REGION,
                    ))
                    .build(),
            )
            .send()
            .await;
        assert!(
            created.is_ok(),
            "case {case:?}: Amazon S3 must create the bucket {bucket}, got {created:?}"
        );
        for (key, body) in objects {
            let stored = self
                .s3()
                .put_object()
                .bucket(bucket)
                .key(*key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(body.to_vec()))
                .send()
                .await;
            assert!(
                stored.is_ok(),
                "case {case:?}: Amazon S3 must store {key} in {bucket}, got {stored:?}"
            );
        }
    }

    /// What `bucket` holds now, as (key, bytes) pairs in key order.
    async fn bucket_contents(&self, bucket: &str) -> Vec<(String, Vec<u8>)> {
        let listed = self
            .s3()
            .list_objects_v2()
            .bucket(bucket)
            .send()
            .await
            .expect("Amazon S3 lists a bucket this suite created");
        let mut held = Vec::new();
        for object in listed.contents() {
            let key = object.key().unwrap_or_default().to_string();
            let read = self
                .s3()
                .get_object()
                .bucket(bucket)
                .key(&key)
                .send()
                .await
                .expect("Amazon S3 serves an object it just listed");
            let body = read
                .body
                .collect()
                .await
                .expect("the object arrives in full")
                .into_bytes()
                .to_vec();
            held.push((key, body));
        }
        held.sort_by(|left, right| left.0.cmp(&right.0));
        held
    }

    /// Create the bucket access role, with an inline policy over `buckets`.
    async fn create_role(&self, buckets: &[&str]) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": { "Service": "datasync.amazonaws.com" },
                "Action": "sts:AssumeRole",
            }],
        });
        let created = self
            .iam()
            .create_role()
            .role_name(ROLE_NAME)
            .assume_role_policy_document(trust.to_string())
            .send()
            .await
            .expect("IAM creates the bucket access role");
        self.put_role_policy(buckets).await;
        created
            .role()
            .expect("the created role is reported")
            .arn()
            .to_string()
    }

    /// Give the role an inline policy granting the transfer's S3 actions over `buckets`.
    async fn put_role_policy(&self, buckets: &[&str]) {
        let resources: Vec<String> = buckets
            .iter()
            .flat_map(|bucket| {
                [
                    format!("arn:aws:s3:::{bucket}"),
                    format!("arn:aws:s3:::{bucket}/*"),
                ]
            })
            .collect();
        let document = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Action": ["s3:ListBucket", "s3:GetObject", "s3:PutObject", "s3:DeleteObject"],
                "Resource": resources,
            }],
        });
        self.iam()
            .put_role_policy()
            .role_name(ROLE_NAME)
            .policy_name(POLICY_NAME)
            .policy_document(document.to_string())
            .send()
            .await
            .expect("IAM stores the role's inline policy");
    }

    /// Take that inline policy away again, leaving the role granting nothing.
    async fn delete_role_policy(&self) {
        self.iam()
            .delete_role_policy()
            .role_name(ROLE_NAME)
            .policy_name(POLICY_NAME)
            .send()
            .await
            .expect("IAM removes the role's inline policy");
    }

    /// An Amazon S3 transfer location for `bucket`, created and returned by ARN.
    async fn create_location(
        &self,
        bucket: &str,
        subdirectory: Option<&str>,
        role_arn: &str,
        case: &str,
    ) -> String {
        let mut request = json!({
            "S3BucketArn": format!("arn:aws:s3:::{bucket}"),
            "S3Config": { "BucketAccessRoleArn": role_arn },
        });
        if let Some(subdirectory) = subdirectory {
            request["Subdirectory"] = json!(subdirectory);
        }
        let created = self.ok("CreateLocationS3", request, case).await;
        created["LocationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a LocationArn, got {created}"))
            .to_string()
    }

    /// Wait until the task is out of `CREATING`, then start one execution.
    async fn start_execution(&self, task_arn: &str, case: &str) -> String {
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        loop {
            let described = self
                .ok("DescribeTask", json!({ "TaskArn": task_arn }), case)
                .await;
            let status = described["Status"].as_str().unwrap_or_default();
            if status == "AVAILABLE" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the task never left CREATING; last status was {status:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
        let started = self
            .ok("StartTaskExecution", json!({ "TaskArn": task_arn }), case)
            .await;
        started["TaskExecutionArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TaskExecutionArn, got {started}"))
            .to_string()
    }

    /// Poll `DescribeTaskExecution` until the execution reaches a terminal status, and
    /// return that body.
    async fn await_terminal(&self, execution_arn: &str, case: &str) -> Value {
        let deadline = Instant::now() + TERMINAL_DEADLINE;
        loop {
            let described = self
                .ok(
                    "DescribeTaskExecution",
                    json!({ "TaskExecutionArn": execution_arn }),
                    case,
                )
                .await;
            let status = described["Status"].as_str().unwrap_or_default();
            if status == "SUCCESS" || status == "ERROR" {
                return described;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the execution never reached a terminal status; \
                 last status was {status:?} in {described}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// One row of the transfer table.
struct TransferRow {
    case: &'static str,
    /// What the source bucket holds when the execution runs.
    source: &'static [(&'static str, &'static [u8])],
    /// What the destination bucket already holds.
    destination: &'static [(&'static str, &'static [u8])],
    /// The task's `Options`.
    options: Value,
    /// The task's `Includes`, if any.
    includes: Option<Value>,
    /// Whether the bucket access role loses its policy before the execution starts.
    strip_role: bool,
    /// The terminal `Status` the execution must reach.
    status: &'static str,
    /// `Result.PrepareStatus`, `TransferStatus` and `VerifyStatus`.
    phases: [&'static str; 3],
    /// Whether `Result.ErrorCode` must be reported.
    error_reported: bool,
    /// The counters the execution must report.
    counters: Vec<(&'static str, i64)>,
    /// What the destination bucket must hold afterwards.
    expected_destination: &'static [(&'static str, &'static [u8])],
}

/// RULE: a task execution between two Amazon S3 locations performs the work its task's
/// `Options` describe against the two buckets themselves, and every member it reports —
/// its terminal status, each phase's `PhaseStatus`, the file and byte counters and the
/// `ErrorCode` of a failed phase — is what that work established: the objects the source
/// holds, the bytes written to the destination, the objects skipped or deleted, and the
/// comparison the verify step made.
///
/// The rows span the two `TransferMode` values, the two `OverwriteMode` values, three
/// `VerifyMode` values, a destination that is empty, stale and already synchronised, a
/// filtered source, a destination holding data the source does not, an empty source, and
/// a bucket access role that allows the transfer nothing.
#[tokio::test]
async fn an_execution_reports_the_transfer_it_actually_performed() {
    let rows = vec![
        TransferRow {
            case: "transfer all onto an empty destination",
            source: SOURCE_OBJECTS,
            destination: &[],
            options: json!({
                "VerifyMode": "POINT_IN_TIME_CONSISTENT",
                "LogLevel": "OFF",
                "TransferMode": "ALL",
            }),
            includes: None,
            strip_role: false,
            status: "SUCCESS",
            phases: ["SUCCESS", "SUCCESS", "SUCCESS"],
            error_reported: false,
            counters: vec![
                ("EstimatedFilesToTransfer", 3),
                ("EstimatedBytesToTransfer", total_bytes(SOURCE_OBJECTS)),
                ("FilesTransferred", 3),
                ("BytesTransferred", total_bytes(SOURCE_OBJECTS)),
                ("BytesWritten", total_bytes(SOURCE_OBJECTS)),
                ("FilesVerified", 3),
                ("FilesSkipped", 0),
                ("FilesDeleted", 0),
                ("EstimatedFilesToDelete", 0),
            ],
            expected_destination: SOURCE_OBJECTS,
        },
        TransferRow {
            case: "overwrite never preserves the destination and fails verification",
            source: SOURCE_OBJECTS,
            destination: STALE_OBJECTS,
            options: json!({ "TransferMode": "CHANGED", "OverwriteMode": "NEVER" }),
            includes: None,
            strip_role: false,
            status: "ERROR",
            phases: ["SUCCESS", "SUCCESS", "ERROR"],
            error_reported: true,
            counters: vec![
                ("EstimatedFilesToTransfer", 0),
                ("EstimatedBytesToTransfer", 0),
                ("FilesTransferred", 0),
                ("BytesWritten", 0),
                ("FilesSkipped", 3),
                ("FilesVerified", 3),
            ],
            expected_destination: STALE_OBJECTS,
        },
        TransferRow {
            case: "overwrite always replaces the destination's content",
            source: SOURCE_OBJECTS,
            destination: STALE_OBJECTS,
            options: json!({ "TransferMode": "CHANGED", "OverwriteMode": "ALWAYS" }),
            includes: None,
            strip_role: false,
            status: "SUCCESS",
            phases: ["SUCCESS", "SUCCESS", "SUCCESS"],
            error_reported: false,
            counters: vec![
                ("EstimatedFilesToTransfer", 3),
                ("FilesTransferred", 3),
                ("BytesWritten", total_bytes(SOURCE_OBJECTS)),
                ("FilesSkipped", 0),
                ("FilesVerified", 3),
            ],
            expected_destination: SOURCE_OBJECTS,
        },
        TransferRow {
            case: "overwrite never with no verification ends in success",
            source: SOURCE_OBJECTS,
            destination: STALE_OBJECTS,
            options: json!({
                "TransferMode": "CHANGED",
                "OverwriteMode": "NEVER",
                "VerifyMode": "NONE",
            }),
            includes: None,
            strip_role: false,
            status: "SUCCESS",
            phases: ["SUCCESS", "SUCCESS", "SUCCESS"],
            error_reported: false,
            counters: vec![
                ("FilesSkipped", 3),
                ("BytesWritten", 0),
                ("FilesVerified", 0),
            ],
            expected_destination: STALE_OBJECTS,
        },
        TransferRow {
            case: "a changed transfer onto a synchronised destination moves no bytes",
            source: SOURCE_OBJECTS,
            destination: SOURCE_OBJECTS,
            options: json!({ "TransferMode": "CHANGED" }),
            includes: None,
            strip_role: false,
            status: "SUCCESS",
            phases: ["SUCCESS", "SUCCESS", "SUCCESS"],
            error_reported: false,
            counters: vec![
                ("EstimatedFilesToTransfer", 0),
                ("EstimatedBytesToTransfer", 0),
                ("FilesTransferred", 0),
                ("BytesWritten", 0),
                ("FilesVerified", 3),
            ],
            expected_destination: SOURCE_OBJECTS,
        },
        TransferRow {
            case: "an include filter narrows the transferred set",
            source: SOURCE_OBJECTS,
            destination: &[],
            options: json!({ "TransferMode": "ALL", "VerifyMode": "ONLY_FILES_TRANSFERRED" }),
            includes: Some(json!([{ "FilterType": "SIMPLE_PATTERN", "Value": "/nested" }])),
            strip_role: false,
            status: "SUCCESS",
            phases: ["SUCCESS", "SUCCESS", "SUCCESS"],
            error_reported: false,
            counters: vec![
                ("EstimatedFilesToTransfer", 2),
                ("EstimatedBytesToTransfer", total_bytes(NESTED_OBJECTS)),
                ("FilesTransferred", 2),
                ("BytesWritten", total_bytes(NESTED_OBJECTS)),
                ("FilesVerified", 2),
            ],
            expected_destination: NESTED_OBJECTS,
        },
        TransferRow {
            case: "removing deleted files clears what the source no longer holds",
            source: NESTED_OBJECTS,
            destination: SOURCE_OBJECTS,
            options: json!({
                "TransferMode": "CHANGED",
                "PreserveDeletedFiles": "REMOVE",
            }),
            includes: None,
            strip_role: false,
            status: "SUCCESS",
            phases: ["SUCCESS", "SUCCESS", "SUCCESS"],
            error_reported: false,
            counters: vec![
                ("EstimatedFilesToTransfer", 0),
                ("EstimatedFilesToDelete", 1),
                ("FilesDeleted", 1),
                ("FilesVerified", 2),
            ],
            expected_destination: NESTED_OBJECTS,
        },
        TransferRow {
            case: "an empty source transfers nothing",
            source: &[],
            destination: &[],
            options: json!({ "TransferMode": "ALL" }),
            includes: None,
            strip_role: false,
            status: "SUCCESS",
            phases: ["SUCCESS", "SUCCESS", "SUCCESS"],
            error_reported: false,
            counters: vec![
                ("EstimatedFilesToTransfer", 0),
                ("FilesTransferred", 0),
                ("BytesWritten", 0),
                ("FilesVerified", 0),
            ],
            expected_destination: &[],
        },
        TransferRow {
            case: "a role that allows the transfer nothing fails the prepare step",
            source: SOURCE_OBJECTS,
            destination: &[],
            options: json!({ "TransferMode": "ALL" }),
            includes: None,
            strip_role: true,
            status: "ERROR",
            phases: ["ERROR", "PENDING", "PENDING"],
            error_reported: true,
            counters: vec![
                ("EstimatedFilesToTransfer", 0),
                ("EstimatedBytesToTransfer", 0),
                ("FilesTransferred", 0),
                ("BytesTransferred", 0),
                ("BytesWritten", 0),
                ("FilesVerified", 0),
                ("FilesSkipped", 0),
            ],
            expected_destination: &[],
        },
    ];

    for row in rows {
        let case = row.case;
        let aws = Aws::new();
        let (source_bucket, destination_bucket) = ("transfer-source", "transfer-destination");
        aws.seed_bucket(source_bucket, row.source, case).await;
        aws.seed_bucket(destination_bucket, row.destination, case)
            .await;
        let role_arn = aws.create_role(&[source_bucket, destination_bucket]).await;

        let source = aws
            .create_location(source_bucket, None, &role_arn, case)
            .await;
        let destination = aws
            .create_location(destination_bucket, None, &role_arn, case)
            .await;
        let mut request = json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": "transfer-task",
            "Options": row.options,
        });
        if let Some(includes) = &row.includes {
            request["Includes"] = includes.clone();
        }
        let created = aws.ok("CreateTask", request, case).await;
        let task_arn = created["TaskArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a TaskArn, got {created}"))
            .to_string();

        if row.strip_role {
            aws.delete_role_policy().await;
        }

        let execution_arn = aws.start_execution(&task_arn, case).await;
        let finished = aws.await_terminal(&execution_arn, case).await;

        let status = finished["Status"].as_str().unwrap_or_default();
        assert_eq!(
            status, row.status,
            "case {case:?}: expected the execution to end {expected:?}, got {status:?} in \
             {finished}",
            expected = row.status
        );
        for (member, expected) in ["PrepareStatus", "TransferStatus", "VerifyStatus"]
            .into_iter()
            .zip(row.phases)
        {
            let actual = finished["Result"][member].as_str().unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected Result.{member} {expected:?}, got {actual:?} in \
                 {finished}"
            );
        }
        let error_code = finished["Result"]["ErrorCode"].as_str();
        assert_eq!(
            error_code.is_some(),
            row.error_reported,
            "case {case:?}: expected Result.ErrorCode reported = {expected}, got {actual:?} in \
             {finished}",
            expected = row.error_reported,
            actual = error_code
        );
        if row.error_reported {
            assert!(
                finished["Result"]["ErrorDetail"]
                    .as_str()
                    .is_some_and(|detail| !detail.is_empty()),
                "case {case:?}: a failed execution describes its error, got {finished}"
            );
        }
        for (counter, expected) in &row.counters {
            let actual = finished[counter].as_i64();
            assert_eq!(
                actual,
                Some(*expected),
                "case {case:?}: expected {counter} {expected}, got {actual:?} in {finished}"
            );
        }

        // The destination bucket itself is the other half of the answer: a reported
        // transfer that moved no bytes would leave it empty.
        let held = aws.bucket_contents(destination_bucket).await;
        let mut expected: Vec<(String, Vec<u8>)> = row
            .expected_destination
            .iter()
            .map(|(key, body)| ((*key).to_string(), body.to_vec()))
            .collect();
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        assert_eq!(
            held.len(),
            expected.len(),
            "case {case:?}: expected {count} destination objects, got {keys:?}",
            count = expected.len(),
            keys = held.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>()
        );
        for ((actual_key, actual_body), (expected_key, expected_body)) in
            held.iter().zip(expected.iter())
        {
            assert_eq!(
                actual_key, expected_key,
                "case {case:?}: expected the destination to hold {expected_key:?}, got \
                 {actual_key:?}"
            );
            assert_eq!(
                String::from_utf8_lossy(actual_body),
                String::from_utf8_lossy(expected_body),
                "case {case:?}: wrong bytes at {actual_key:?}"
            );
        }

        // A task is available again once the run it was on ends, however that run ended.
        let task = aws
            .ok("DescribeTask", json!({ "TaskArn": task_arn }), case)
            .await;
        assert_eq!(
            task["Status"].as_str(),
            Some("AVAILABLE"),
            "case {case:?}: a task is AVAILABLE again once its execution ends, got {task}"
        );
    }
}

/// RULE: a task that failed a run is available to run again, and the next run reports
/// what IT established — so once the bucket access role allows the listing again, the
/// same task's execution prepares, transfers and verifies successfully.
#[tokio::test]
async fn a_task_runs_again_once_its_role_allows_the_transfer() {
    let case = "restored role";
    let aws = Aws::new();
    let (source_bucket, destination_bucket) = ("restore-source", "restore-destination");
    aws.seed_bucket(source_bucket, SOURCE_OBJECTS, case).await;
    aws.seed_bucket(destination_bucket, &[], case).await;
    let role_arn = aws.create_role(&[source_bucket, destination_bucket]).await;
    let source = aws
        .create_location(source_bucket, None, &role_arn, case)
        .await;
    let destination = aws
        .create_location(destination_bucket, None, &role_arn, case)
        .await;
    let created = aws
        .ok(
            "CreateTask",
            json!({
                "SourceLocationArn": source,
                "DestinationLocationArn": destination,
                "Name": "restored-role-task",
                "Options": { "VerifyMode": "POINT_IN_TIME_CONSISTENT", "LogLevel": "OFF" },
            }),
            case,
        )
        .await;
    let task_arn = created["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a TaskArn, got {created}"))
        .to_string();

    // With the role allowing nothing, the run fails in its prepare step and the
    // destination stays empty.
    aws.delete_role_policy().await;
    let denied_arn = aws.start_execution(&task_arn, case).await;
    let denied = aws.await_terminal(&denied_arn, case).await;
    assert_eq!(
        denied["Status"].as_str(),
        Some("ERROR"),
        "case {case:?}: a denied run ends in ERROR, got {denied}"
    );
    assert_eq!(
        denied["Result"]["PrepareStatus"].as_str(),
        Some("ERROR"),
        "case {case:?}: the prepare step is the one that failed, got {denied}"
    );
    assert!(
        aws.bucket_contents(destination_bucket).await.is_empty(),
        "case {case:?}: a denied run writes nothing to the destination"
    );

    // With the role restored, the next run of the SAME task does the work.
    aws.put_role_policy(&[source_bucket, destination_bucket])
        .await;
    let allowed_arn = aws.start_execution(&task_arn, case).await;
    let allowed = aws.await_terminal(&allowed_arn, case).await;
    assert_eq!(
        allowed["Status"].as_str(),
        Some("SUCCESS"),
        "case {case:?}: the restored run must succeed, got {allowed}"
    );
    for member in ["PrepareStatus", "TransferStatus", "VerifyStatus"] {
        assert_eq!(
            allowed["Result"][member].as_str(),
            Some("SUCCESS"),
            "case {case:?}: expected Result.{member} SUCCESS, got {allowed}"
        );
    }
    assert_eq!(
        allowed["FilesTransferred"].as_i64(),
        Some(SOURCE_OBJECTS.len() as i64),
        "case {case:?}: the restored run transfers every source object, got {allowed}"
    );
    assert_eq!(
        allowed["BytesWritten"].as_i64(),
        Some(total_bytes(SOURCE_OBJECTS)),
        "case {case:?}: the restored run writes every source byte, got {allowed}"
    );
    let held = aws.bucket_contents(destination_bucket).await;
    assert_eq!(
        held.len(),
        SOURCE_OBJECTS.len(),
        "case {case:?}: expected the destination to hold the source's objects, got {keys:?}",
        keys = held.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>()
    );
}

/// RULE: `ListTaskExecutions` reports the executions this account holds — all of them,
/// or only the ones of the task the request names — in the order they were started, one
/// page at a time, and refuses a `TaskArn` that resolves to no task.
///
/// The rows span a task with two runs, a task with one, every page size that can split
/// the executions, and a request that names no task at all.
#[tokio::test]
async fn list_task_executions_pages_over_the_runs_it_holds() {
    let case = "execution listing";
    let aws = Aws::new();
    for bucket in ["listing-source", "listing-destination"] {
        aws.seed_bucket(bucket, &[], case).await;
    }
    let role_arn = aws
        .create_role(&["listing-source", "listing-destination"])
        .await;
    let source = aws
        .create_location("listing-source", None, &role_arn, case)
        .await;
    let destination = aws
        .create_location("listing-destination", None, &role_arn, case)
        .await;
    let task = |name: &str| {
        json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": name,
        })
    };
    let first_task = aws.ok("CreateTask", task("first"), case).await["TaskArn"]
        .as_str()
        .expect("a TaskArn")
        .to_string();
    let second_task = aws.ok("CreateTask", task("second"), case).await["TaskArn"]
        .as_str()
        .expect("a TaskArn")
        .to_string();

    // Two runs of the first task, one after the other, and one of the second: a task
    // only starts a run once the run it is on has ended.
    let mut started = Vec::new();
    for task_arn in [&first_task, &first_task, &second_task] {
        let execution_arn = aws.start_execution(task_arn, case).await;
        aws.await_terminal(&execution_arn, case).await;
        started.push((task_arn.clone(), execution_arn));
    }

    let arns = |body: &Value| -> Vec<String> {
        body["TaskExecutions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry["TaskExecutionArn"].as_str().map(ToString::to_string))
            .collect()
    };

    let of_first = aws
        .ok(
            "ListTaskExecutions",
            json!({ "TaskArn": first_task }),
            case,
        )
        .await;
    assert_eq!(
        arns(&of_first),
        vec![started[0].1.clone(), started[1].1.clone()],
        "case {case:?}: expected the first task's two runs in start order, got {of_first}"
    );
    for entry in of_first["TaskExecutions"].as_array().cloned().unwrap_or_default() {
        assert_eq!(
            entry["Status"].as_str(),
            Some("SUCCESS"),
            "case {case:?}: each entry reports its run's stored status, got {entry}"
        );
        assert_eq!(
            entry["TaskMode"].as_str(),
            Some("BASIC"),
            "case {case:?}: each entry reports its task's mode, got {entry}"
        );
    }
    assert!(
        of_first["NextToken"].is_null(),
        "case {case:?}: a final page carries no token, got {of_first}"
    );

    let of_second = aws
        .ok(
            "ListTaskExecutions",
            json!({ "TaskArn": second_task }),
            case,
        )
        .await;
    assert_eq!(
        arns(&of_second),
        vec![started[2].1.clone()],
        "case {case:?}: expected only the second task's run, got {of_second}"
    );

    // Every page size that can split three executions, plus one past them.
    for page_size in 1..=4 {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut request = json!({ "MaxResults": page_size });
            if let Some(token) = &token {
                request["NextToken"] = json!(token);
            }
            let page = aws.ok("ListTaskExecutions", request, case).await;
            let page_arns = arns(&page);
            assert!(
                page_arns.len() <= page_size as usize,
                "case {case:?}: page of {page_size} carried {count} entries, got {page}",
                count = page_arns.len()
            );
            seen.extend(page_arns);
            token = page["NextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        let expected: Vec<String> = started
            .iter()
            .map(|(_, execution_arn)| execution_arn.clone())
            .collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: paging by {page_size} must report every run exactly once in start \
             order"
        );
    }

    // A well-formed `TaskArn` (`task/task-` and 17 hexadecimal digits) this account holds
    // no task under.
    let unknown = format!(
        "arn:aws:datasync:{REGION}:{account}:task/task-{id}",
        account = engine::emulator_account_id(),
        id = "f".repeat(17),
    );
    aws.invalid_request(
        "ListTaskExecutions",
        json!({ "TaskArn": unknown }),
        "a TaskArn that resolves to no task",
    )
    .await;
}

/// RULE: an execution's steps happen in order and each one's outcome is recorded once,
/// however many observers drive it at the same time — so a transfer reported as
/// finished has really transferred, whoever was polling.
///
/// Several observers poll the same execution concurrently, which is what a client
/// waiting on a transfer does; the terminal body each one reads must be the one a single
/// observer would have seen.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_observers_see_the_steps_in_order() {
    let case = "concurrent observers";
    let aws = Arc::new(Aws::new());
    let (source_bucket, destination_bucket) = ("concurrent-source", "concurrent-destination");
    aws.seed_bucket(source_bucket, SOURCE_OBJECTS, case).await;
    aws.seed_bucket(destination_bucket, &[], case).await;
    let role_arn = aws.create_role(&[source_bucket, destination_bucket]).await;
    let source = aws
        .create_location(source_bucket, None, &role_arn, case)
        .await;
    let destination = aws
        .create_location(destination_bucket, None, &role_arn, case)
        .await;
    let created = aws
        .ok(
            "CreateTask",
            json!({
                "SourceLocationArn": source,
                "DestinationLocationArn": destination,
                "Name": "concurrent-task",
                "Options": { "VerifyMode": "POINT_IN_TIME_CONSISTENT", "TransferMode": "ALL" },
            }),
            case,
        )
        .await;
    let task_arn = created["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a TaskArn, got {created}"))
        .to_string();
    let execution_arn = aws.start_execution(&task_arn, case).await;

    // Four observers of the one execution, each polling to a terminal status and each
    // checking that what it sees never goes backwards.
    let mut observers = Vec::new();
    for observer in 0..4 {
        let aws = Arc::clone(&aws);
        let execution_arn = execution_arn.clone();
        observers.push(tokio::spawn(async move {
            let order = ["LAUNCHING", "PREPARING", "TRANSFERRING", "VERIFYING"];
            let rank = |status: &str| order.iter().position(|known| *known == status);
            let mut highest = 0;
            let deadline = Instant::now() + TERMINAL_DEADLINE;
            loop {
                let described = aws
                    .ok(
                        "DescribeTaskExecution",
                        json!({ "TaskExecutionArn": execution_arn }),
                        case,
                    )
                    .await;
                let status = described["Status"].as_str().unwrap_or_default().to_string();
                match rank(&status) {
                    Some(reached) => {
                        assert!(
                            reached >= highest,
                            "observer {observer}: the status went backwards to {status:?} \
                             after rank {highest}"
                        );
                        highest = reached;
                    }
                    None => return described,
                }
                assert!(
                    Instant::now() < deadline,
                    "observer {observer}: the execution never ended; last status {status:?}"
                );
                tokio::time::sleep(POLL_INTERVAL).await;
            }
        }));
    }

    for (observer, handle) in observers.into_iter().enumerate() {
        let finished = handle.await.expect("the observer finishes");
        assert_eq!(
            finished["Status"].as_str(),
            Some("SUCCESS"),
            "observer {observer}: expected the transfer to succeed, got {finished}"
        );
        for member in ["PrepareStatus", "TransferStatus", "VerifyStatus"] {
            assert_eq!(
                finished["Result"][member].as_str(),
                Some("SUCCESS"),
                "observer {observer}: expected Result.{member} SUCCESS, got {finished}"
            );
        }
        assert_eq!(
            finished["FilesTransferred"].as_i64(),
            Some(SOURCE_OBJECTS.len() as i64),
            "observer {observer}: every source object is transferred exactly once, got {finished}"
        );
        assert_eq!(
            finished["BytesWritten"].as_i64(),
            Some(total_bytes(SOURCE_OBJECTS)),
            "observer {observer}: every source byte is written exactly once, got {finished}"
        );
    }

    let held = aws.bucket_contents(destination_bucket).await;
    assert_eq!(
        held.len(),
        SOURCE_OBJECTS.len(),
        "case {case:?}: expected the destination to hold the source's objects, got {keys:?}",
        keys = held.iter().map(|(key, _)| key.clone()).collect::<Vec<_>>()
    );
}

/// RULE: before it stores an Amazon S3 location, DataSync proves the bucket access role
/// the request names can actually list the named bucket, and refuses the request with
/// `InvalidRequestException` (400) — storing nothing — when the role's policies withhold
/// that listing.
///
/// The rows span a role that allows the bucket, one whose policy names a different
/// bucket, one holding no policy at all, and a role of ANOTHER account, which this
/// emulator's IAM is not asked about and so establishes nothing either way.
#[tokio::test]
async fn create_location_s3_proves_the_bucket_access_role_can_list_the_bucket() {
    let aws = Aws::new();
    for bucket in ["readable", "unreadable", "second-readable"] {
        aws.seed_bucket(bucket, &[("object.txt", b"content")], "setup")
            .await;
    }
    // The policy names ONLY these two buckets.
    let role_arn = aws.create_role(&["readable", "second-readable"]).await;
    // A role of another account: neither the assume pre-flight nor the listing test asks
    // this emulator's IAM about it, so neither can establish anything about it.
    let unanswered_role = "arn:aws:iam::210987654321:role/another-accounts-role".to_string();

    struct Row {
        case: &'static str,
        bucket: &'static str,
        /// Whether the role's inline policy is removed before the call.
        without_policy: bool,
        /// Whether a role of another account is used instead.
        unanswered_role: bool,
        accepted: bool,
    }

    let rows = [
        Row {
            case: "the policy names this bucket",
            bucket: "readable",
            without_policy: false,
            unanswered_role: false,
            accepted: true,
        },
        Row {
            case: "the policy names another bucket",
            bucket: "unreadable",
            without_policy: false,
            unanswered_role: false,
            accepted: false,
        },
        Row {
            case: "the policy names this bucket too",
            bucket: "second-readable",
            without_policy: false,
            unanswered_role: false,
            accepted: true,
        },
        Row {
            case: "a role of another account",
            bucket: "unreadable",
            without_policy: false,
            unanswered_role: true,
            accepted: true,
        },
        Row {
            case: "the role has no policy at all",
            bucket: "readable",
            without_policy: true,
            unanswered_role: false,
            accepted: false,
        },
    ];

    let mut accepted_arns: Vec<String> = Vec::new();
    for row in rows {
        if row.without_policy {
            aws.delete_role_policy().await;
        }
        let used_role = if row.unanswered_role {
            unanswered_role.clone()
        } else {
            role_arn.clone()
        };
        let request = json!({
            "S3BucketArn": format!("arn:aws:s3:::{bucket}", bucket = row.bucket),
            "S3Config": { "BucketAccessRoleArn": used_role },
        });
        if row.accepted {
            let created = aws.ok("CreateLocationS3", request, row.case).await;
            let arn = created["LocationArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a LocationArn, got {created}", case = row.case)
                })
                .to_string();
            accepted_arns.push(arn);
        } else {
            let refused = aws
                .invalid_request("CreateLocationS3", request, row.case)
                .await;
            let message = refused["message"].as_str().unwrap_or_default();
            assert_eq!(
                message,
                format!(
                    "DataSync location access test failed: could not perform \
                     s3:ListObjectsV2 on bucket {bucket}. Access denied. Ensure bucket \
                     access role has s3:ListBucket permission.",
                    bucket = row.bucket
                ),
                "case {case:?}: wrong refusal wording, got {refused}",
                case = row.case
            );
        }
        if row.without_policy {
            aws.put_role_policy(&["readable", "second-readable"]).await;
        }
    }

    // A refused create stored nothing: the listing holds exactly the accepted ones.
    let listed = aws.ok("ListLocations", json!({}), "listing").await;
    let held: Vec<String> = listed["Locations"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry["LocationArn"].as_str().map(ToString::to_string))
        .collect();
    assert_eq!(
        held, accepted_arns,
        "case \"listing\": expected only the accepted locations, got {listed}"
    );
}

/// RULE: a transfer task copies one location onto ANOTHER, so a request whose two
/// `LocationArn` members name the same location is refused with
/// `InvalidRequestException` (400) and no task is stored.
///
/// The rows span the same location on both sides for two different locations, and both
/// orderings of a distinct pair.
#[tokio::test]
async fn create_task_refuses_a_location_onto_itself() {
    let aws = Aws::new();
    for bucket in ["self-first", "self-second"] {
        aws.seed_bucket(bucket, &[], "setup").await;
    }
    let role_arn = aws.create_role(&["self-first", "self-second"]).await;
    let first = aws
        .create_location("self-first", None, &role_arn, "setup")
        .await;
    let second = aws
        .create_location("self-second", None, &role_arn, "setup")
        .await;

    let rows = [
        ("the first location onto itself", &first, &first, false),
        ("the second location onto itself", &second, &second, false),
        ("the first onto the second", &first, &second, true),
        ("the second onto the first", &second, &first, true),
    ];

    let mut accepted = 0;
    for (case, source, destination, is_accepted) in rows {
        let request = json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": case,
        });
        if is_accepted {
            aws.ok("CreateTask", request, case).await;
            accepted += 1;
        } else {
            let refused = aws.invalid_request("CreateTask", request, case).await;
            assert_eq!(
                refused["message"].as_str(),
                Some("Destination location cannot be same as source location."),
                "case {case:?}: wrong refusal wording, got {refused}"
            );
        }
    }

    let listed = aws.ok("ListTasks", json!({}), "listing").await;
    let held = listed["Tasks"].as_array().map(Vec::len).unwrap_or_default();
    assert_eq!(
        held, accepted,
        "case \"listing\": a refused create must store nothing, got {listed}"
    );
}

/// RULE: each `Options` member's value is legal on its own, but a COMBINATION the
/// service does not accept is refused with `InvalidRequestException` (400) — naming the
/// members and values that conflict — wherever a set of options is resolved, and nothing
/// is stored or started.
///
/// The rows span every combination this service refuses, each one's halves on their own,
/// the task mode that lifts the metadata dependency, and the same contradiction built by
/// an execution's `OverrideOptions` instead of by the task's own options.
#[tokio::test]
async fn contradictory_option_combinations_are_refused() {
    let aws = Aws::new();
    for bucket in ["options-source", "options-destination"] {
        aws.seed_bucket(bucket, &[], "setup").await;
    }
    let role_arn = aws
        .create_role(&["options-source", "options-destination"])
        .await;
    let source = aws
        .create_location("options-source", None, &role_arn, "setup")
        .await;
    let destination = aws
        .create_location("options-destination", None, &role_arn, "setup")
        .await;

    struct Row {
        case: &'static str,
        task_mode: Option<&'static str>,
        options: Value,
        /// The refusal message, or `None` for a combination that is accepted.
        refusal: Option<&'static str>,
    }

    let rows = [
        Row {
            case: "transfer everything and never overwrite",
            task_mode: None,
            options: json!({ "TransferMode": "ALL", "OverwriteMode": "NEVER" }),
            refusal: Some(
                "Invalid SyncOption value. Option: TransferMode,OverwriteMode, Value: ALL,NEVER.",
            ),
        },
        Row {
            case: "transfer everything and always overwrite",
            task_mode: None,
            options: json!({ "TransferMode": "ALL", "OverwriteMode": "ALWAYS" }),
            refusal: None,
        },
        Row {
            case: "transfer the difference and never overwrite",
            task_mode: None,
            options: json!({ "TransferMode": "CHANGED", "OverwriteMode": "NEVER" }),
            refusal: None,
        },
        Row {
            case: "transfer everything and remove deleted files",
            task_mode: None,
            options: json!({ "TransferMode": "ALL", "PreserveDeletedFiles": "REMOVE" }),
            refusal: Some(
                "Invalid SyncOption value. Option: TransferMode,PreserveDeletedFiles, Value: \
                 ALL,REMOVE.",
            ),
        },
        Row {
            case: "transfer the difference and remove deleted files",
            task_mode: None,
            options: json!({ "TransferMode": "CHANGED", "PreserveDeletedFiles": "REMOVE" }),
            refusal: None,
        },
        Row {
            case: "ignore the access time while preserving the modification time",
            task_mode: None,
            options: json!({ "Atime": "NONE" }),
            refusal: Some("Invalid SyncOption value. Option: Atime,Mtime, Value: NONE,PRESERVE."),
        },
        Row {
            case: "ignore both times",
            task_mode: None,
            options: json!({ "Atime": "NONE", "Mtime": "NONE" }),
            refusal: None,
        },
        Row {
            case: "ignore the modification time on a best-effort access time",
            task_mode: None,
            options: json!({ "Mtime": "NONE" }),
            refusal: Some(
                "Invalid SyncOption value. Option: Atime,Mtime, Value: BEST_EFFORT,NONE.",
            ),
        },
        Row {
            case: "an enhanced task configures the two times independently",
            task_mode: Some("ENHANCED"),
            options: json!({ "Atime": "NONE" }),
            refusal: None,
        },
    ];

    for row in rows {
        let mut request = json!({
            "SourceLocationArn": source,
            "DestinationLocationArn": destination,
            "Name": row.case,
            "Options": row.options,
        });
        if let Some(task_mode) = row.task_mode {
            request["TaskMode"] = json!(task_mode);
        }
        match row.refusal {
            Some(expected) => {
                let refused = aws.invalid_request("CreateTask", request, row.case).await;
                assert_eq!(
                    refused["message"].as_str(),
                    Some(expected),
                    "case {case:?}: wrong refusal wording, got {refused}",
                    case = row.case
                );
            }
            None => {
                aws.ok("CreateTask", request, row.case).await;
            }
        }
    }

    // The same contradiction assembled from a task's options and an execution's
    // override is refused where that set of options is resolved: at the start.
    let case = "an override makes the run's options contradictory";
    let created = aws
        .ok(
            "CreateTask",
            json!({
                "SourceLocationArn": source,
                "DestinationLocationArn": destination,
                "Name": "override-contradiction",
                "Options": { "TransferMode": "ALL", "OverwriteMode": "ALWAYS" },
            }),
            case,
        )
        .await;
    let task_arn = created["TaskArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a TaskArn, got {created}"))
        .to_string();
    let deadline = Instant::now() + TERMINAL_DEADLINE;
    loop {
        let described = aws
            .ok("DescribeTask", json!({ "TaskArn": task_arn }), case)
            .await;
        if described["Status"].as_str() == Some("AVAILABLE") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the task never left CREATING, got {described}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    let refused = aws
        .invalid_request(
            "StartTaskExecution",
            json!({
                "TaskArn": task_arn,
                "OverrideOptions": { "OverwriteMode": "NEVER" },
            }),
            case,
        )
        .await;
    assert_eq!(
        refused["message"].as_str(),
        Some("Invalid SyncOption value. Option: TransferMode,OverwriteMode, Value: ALL,NEVER."),
        "case {case:?}: wrong refusal wording, got {refused}"
    );
}
