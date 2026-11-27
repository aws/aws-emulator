// SPDX-License-Identifier: CC0-1.0

//! An enhanced snapshot's Amazon S3 location is one the OBJECT STORE holds.
//!
//! The rule under test: **`ExportSnapshot` and `ImportSnapshot` resolve the bucket their
//! `S3PrefixLocation` names where Amazon S3 holds it, and are refused when it holds no such
//! bucket** — "The bucket must be owned by your account and must be in the same Region as the
//! request" (`ExportDestinationConfiguration$DestinationS3PrefixLocation` and
//! `ImportSourceConfiguration$SourceS3PrefixLocation`). So an accepted export reports a prefix
//! under a bucket a client can really address, and no snapshot is stored as imported from a
//! location that does not exist.
//!
//! Three crates have to agree for that to hold, so the suite lives here rather than in the
//! owning crate: IAM holds the service execution role the application runs as, Amazon S3
//! answers for the bucket, and Kinesis Analytics V2 holds the snapshot. The snapshot
//! lifecycle itself — what each reader reports, which applications may take one, how the
//! statuses settle — is the owning crate's own suite
//! (`services/service-com-amazonaws-kinesisanalyticsv2-kinesisanalytics-20180523/tests/all/snapshot_mode_reporting.rs`).
//!
//! The rows differ along what the rule generalises over: which of the two operations names the
//! location, and whether the bucket it names is one the object store holds.
//!
//! Every call goes over the wire of the service that owns it, and the one wait — for the clock
//! to settle a started snapshot operation — is a poll for that condition under a deadline
//! generous enough for any host.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// IAM's `awsQuery` parameters travel in a form body, and `query_wire` is where that encoding
// lives.
use crate::query_wire::form_encode;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries.
const ACCOUNT: &str = "000000000000";

/// The `X-Amz-Target` prefix of the 2018-05-23 API: its service shape's local name.
const TARGET_PREFIX: &str = "KinesisAnalytics_20180523";

/// The service execution role the application runs as, and the service principal its trust
/// policy admits.
const ROLE: &str = "kda-service-role";
const KDA_PRINCIPAL: &str = "kinesisanalytics.amazonaws.com";

/// The role an export or an import assumes to move the snapshot data, matching the modelled
/// `^arn:` pattern of `RoleARN`.
const SNAPSHOT_ROLE: &str = "arn:aws:iam::000000000000:role/kda-snapshot-role";

/// The bucket Amazon S3 holds, and one it does not.
const HELD_BUCKET: &str = "kda-enhanced-snapshots";
const ABSENT_BUCKET: &str = "kda-no-such-snapshot-bucket";

/// The runtime and snapshot mode an application that takes enhanced snapshots runs under: the
/// earliest generation `ApplicationSnapshotConfiguration$SnapshotMode` admits for the mode
/// ("the application must use Apache Flink version 1.20 or later").
const RUNTIME: &str = "FLINK-1_20";
const ENHANCED: &str = "ENHANCED_SNAPSHOTS";

/// How long a poll waits for a status the clock owes: many times the span the owning service
/// owes it, so a loaded machine still converges, and bounded so a hung poll fails this suite
/// rather than hanging it.
const SETTLE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one poll attempt waits before reading again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The emulator with the three services this suite exercises, mounted on ONE engine exactly as
/// `core/router/src/lib.rs` mounts them.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A SigV4 `Authorization` header whose credential scope names `service` in [`REGION`], which
/// is what routes the request and what each handler reads its Region from.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/{REGION}/{service}/\
         aws4_request, SignedHeaders=host, Signature=0000000000000000000000000000000000000000\
         000000000000000000000000"
    )
}

/// One Kinesis Analytics V2 operation over its `awsJson1_1` wire: `POST /` with the operation
/// named by `X-Amz-Target`, as an SDK sends it.
async fn kda(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let host = format!("kinesisanalytics.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("authorization", authorization("kinesisanalytics"))
        .body(Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered: Value = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, answered)
}

/// The modelled error code an answer carries (`__type` on `awsJson1_1`, namespace stripped),
/// or the EMPTY string when the call succeeded.
fn answer_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// `PUT /<bucket>` — Amazon S3's `CreateBucket`, so the location this suite exports to is one
/// the object store really holds.
async fn given_a_bucket(router: &MultiServiceRouter, case: &str) {
    let host = format!("s3.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("https://{host}/{HELD_BUCKET}"))
        .header("host", host)
        .header("authorization", authorization("s3"))
        .header("content-type", "application/octet-stream")
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    assert_eq!(
        status,
        200,
        "case {case:?}: CreateBucket({HELD_BUCKET:?}) must succeed, got {status} with {}",
        String::from_utf8_lossy(response.body())
    );
}

/// Provision [`ROLE`] in IAM with a trust policy admitting Kinesis Analytics, which a create
/// resolves before it stores an application.
async fn given_the_service_execution_role(router: &MultiServiceRouter, case: &str) {
    let trust = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": KDA_PRINCIPAL},
            "Action": "sts:AssumeRole",
        }],
    });
    let body = format!(
        "Action=CreateRole&Version=2010-05-08&RoleName={}&AssumeRolePolicyDocument={}",
        form_encode(ROLE),
        form_encode(&trust.to_string()),
    );
    let host = format!("iam.{REGION}.amazonaws.com");
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://{host}/"))
        .header("host", host)
        .header("content-type", "application/x-www-form-urlencoded")
        .header("authorization", authorization("iam"))
        .body(Bytes::from(body))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    assert_eq!(
        status,
        200,
        "case {case:?}: the service execution role must be created in IAM first, got {status} \
         with {}",
        String::from_utf8_lossy(response.body())
    );
}

/// The ARN of a bucket by name.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// Create an application that takes enhanced snapshots, start it, and look again until it
/// reports RUNNING — the status a snapshot of its state is taken from.
async fn given_a_running_application(router: &MultiServiceRouter, case: &str, name: &str) {
    let (status, created) = kda(
        router,
        "CreateApplication",
        &json!({
            "ApplicationName": name,
            "RuntimeEnvironment": RUNTIME,
            "ApplicationMode": "STREAMING",
            "ServiceExecutionRole": format!("arn:aws:iam::{ACCOUNT}:role/{ROLE}"),
            "ApplicationConfiguration": {
                "ApplicationCodeConfiguration": {
                    "CodeContentType": "PLAINTEXT",
                    "CodeContent": {"TextContent": "-- the application's code"},
                },
                "ApplicationSnapshotConfiguration": {
                    "SnapshotsEnabled": true,
                    "SnapshotMode": ENHANCED,
                },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateApplication({name:?}) must succeed, got {status} with {created}"
    );
    let (status, started) = kda(router, "StartApplication", &json!({"ApplicationName": name})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartApplication({name:?}) must succeed, got {status} with {started}"
    );
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = kda(
            router,
            "DescribeApplication",
            &json!({"ApplicationName": name}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeApplication({name:?}) must succeed, got {status} with \
             {described}"
        );
        let reported = described["ApplicationDetail"]["ApplicationStatus"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if reported == "RUNNING" {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the clock must bring {name:?} to RUNNING, last read {reported:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until the snapshot reports `expected`.
async fn await_snapshot_status(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    expected: &str,
) {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) =
            kda(router, "DescribeSnapshot", &json!({"SnapshotName": name})).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSnapshot({name:?}) must succeed, got {status} with {described}"
        );
        let details = described["SnapshotInfoDetails"].clone();
        if details["SnapshotStatus"].as_str() == Some(expected) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the clock must bring snapshot {name:?} to {expected}, last read \
             {details}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until one operation of the snapshot reports `expected`, and answer its details.
async fn await_operation_status(
    router: &MultiServiceRouter,
    case: &str,
    snapshot: &str,
    operation_id: &str,
    expected: &str,
) -> Value {
    let deadline = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = kda(
            router,
            "DescribeSnapshotOperation",
            &json!({"SnapshotName": snapshot, "SnapshotOperationId": operation_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSnapshotOperation({snapshot:?}, {operation_id:?}) must \
             succeed, got {status} with {described}"
        );
        let details = described["SnapshotOperationInfoDetails"].clone();
        if details["SnapshotOperationStatus"].as_str() == Some(expected) {
            return details;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: the clock must bring operation {operation_id:?} to {expected}, last \
             read {details}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// An `ExportSnapshot` body writing to `bucket` under `key_prefix`.
fn export_body(snapshot: &str, bucket: &str, key_prefix: &str) -> Value {
    json!({
        "SnapshotName": snapshot,
        "ExportDestinationConfiguration": {
            "DestinationType": "S3",
            "ExportRoleArn": SNAPSHOT_ROLE,
            "DestinationS3PrefixLocation": {
                "BucketARN": bucket_arn(bucket),
                "KeyPrefix": key_prefix,
            },
        },
    })
}

/// An `ImportSnapshot` body reading from `bucket` under `key_prefix`.
fn import_body(snapshot: &str, bucket: &str, key_prefix: &str) -> Value {
    json!({
        "SnapshotName": snapshot,
        "ImportSourceConfiguration": {
            "SourceType": "S3",
            "ImportRoleArn": SNAPSHOT_ROLE,
            "SourceS3PrefixLocation": {
                "BucketARN": bucket_arn(bucket),
                "KeyPrefix": key_prefix,
            },
        },
    })
}

/// Both operations that name an Amazon S3 location resolve it in the object store: the one it
/// holds is served, the one it does not is refused, and nothing is stored for the refusal.
#[tokio::test]
async fn a_snapshots_s3_location_is_resolved_in_the_object_store() {
    let case = "an enhanced snapshot moved to and from Amazon S3";
    let router = router();
    given_the_service_execution_role(&router, case).await;
    given_a_bucket(&router, case).await;

    let application = "snapshot-location-app";
    given_a_running_application(&router, case, application).await;
    let snapshot = "snapshot-location-snapshot";
    let (status, taken) = kda(
        &router,
        "CreateSnapshot",
        &json!({"ApplicationName": application, "SnapshotName": snapshot}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSnapshot must succeed, got {status} with {taken}"
    );
    await_snapshot_status(&router, case, snapshot, "READY").await;

    // The export the object store can answer for is served, and the prefix it wrote under is
    // reported once the clock completes it.
    let key_prefix = "exports/one";
    let (status, exported) = kda(
        &router,
        "ExportSnapshot",
        &export_body(snapshot, HELD_BUCKET, key_prefix),
    )
    .await;
    assert_eq!(
        (status, answer_code(&exported).as_str()),
        (200, ""),
        "case \"an export to a bucket the object store holds\": got {status} with {exported}"
    );
    let operation_id = exported["SnapshotOperationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let export =
        await_operation_status(&router, case, snapshot, &operation_id, "SUCCESSFUL").await;
    let location = export["SnapshotDestinationLocationDescription"].clone();
    assert_eq!(
        (
            location["S3PrefixLocation"]["BucketARN"].as_str(),
            location["S3PrefixLocation"]["KeyPrefix"].as_str(),
        ),
        (Some(bucket_arn(HELD_BUCKET).as_str()), Some(key_prefix)),
        "case {case:?}: expected the export's recorded destination, got {export}"
    );
    assert!(
        location["S3PrefixLocation"]["ExportedPrefix"]
            .as_str()
            .is_some_and(|prefix| prefix.starts_with(key_prefix) && prefix.len() > key_prefix.len()),
        "case {case:?}: a completed export reports the full prefix it wrote under, got {export}"
    );

    // The import the object store can answer for is served too.
    let (status, imported) = kda(
        &router,
        "ImportSnapshot",
        &import_body("snapshot-location-imported", HELD_BUCKET, "savepoints/one"),
    )
    .await;
    assert_eq!(
        (status, answer_code(&imported).as_str()),
        (200, ""),
        "case \"an import from a bucket the object store holds\": got {status} with {imported}"
    );
    await_snapshot_status(&router, case, "snapshot-location-imported", "READY").await;

    // (case, the operation, a body naming a bucket the object store does not hold, the
    //  snapshot name it would have created)
    let refused: [(&str, &str, Value, Option<&str>); 2] = [
        (
            "an export to a bucket the object store does not hold",
            "ExportSnapshot",
            export_body(snapshot, ABSENT_BUCKET, key_prefix),
            None,
        ),
        (
            "an import from a bucket the object store does not hold",
            "ImportSnapshot",
            import_body("snapshot-from-nowhere", ABSENT_BUCKET, "savepoints/none"),
            Some("snapshot-from-nowhere"),
        ),
    ];
    for (attempted, operation, body, stored_as) in refused {
        let (status, answered) = kda(&router, operation, &body).await;
        assert_eq!(
            (status, answer_code(&answered).as_str()),
            (400, "InvalidArgumentException"),
            "case {attempted:?}: expected the location to be refused, got {status} with \
             {answered}"
        );
        if let Some(name) = stored_as {
            let (status, absent) =
                kda(&router, "DescribeSnapshot", &json!({"SnapshotName": name})).await;
            assert_eq!(
                (status, answer_code(&absent).as_str()),
                (400, "ResourceNotFoundException"),
                "case {attempted:?}: a refused import stores nothing, got {status} with {absent}"
            );
        }
    }
}
