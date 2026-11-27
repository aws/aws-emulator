// SPDX-License-Identifier: CC0-1.0

//! What a Kinesis-sourced Firehose stream may retrieve follows EVERY policy its source role
//! carries, read at each attempt — so a grant stated in a policy PUT BESIDE the role's
//! existing one restores a retrieval that policy alone refused.
//!
//! The rule under test: **Firehose retrieves a `KinesisStreamAsSource` stream's records under
//! the role its `KinesisStreamSourceConfiguration` names, so the retrieval happens only where
//! the permissions IN FORCE on that role allow the Kinesis Data Streams calls the read is made
//! of (`kinesis:DescribeStream`, `kinesis:GetShardIterator`, `kinesis:GetRecords`), and the
//! permissions in force are what the role's policies say TOGETHER: a role one call short
//! retrieves nothing and reports that call under the fixed `errorCode` `Kinesis.AccessDenied`
//! in the log group and log stream the destination's `CloudWatchLoggingOptions` name, and the
//! moment ANY policy of that role states the missing call the retrieval is made — with nothing
//! recreated and the records it never took still in the source.**
//!
//! The create is not the question this suite asks, but it does resolve the source read: a
//! stream is written only where its role may make the retrieval's calls
//! (`firehose_create_resolves_its_configuration` tables that), so each row's role is created
//! allowing the WHOLE read and is narrowed to one call short of it once the stream exists — which
//! is what makes "the permissions at the attempt" the only thing that decides what each of these
//! streams delivers.
//!
//! The rows differ along what the rule generalises over: WHICH of the three calls the role is
//! short of — the describe the read begins with, the shard iterator it continues with, and the
//! records read it ends with — so what is asserted is the first call the role may not make and
//! not one fixed action, and each row's repair states exactly that call.
//!
//! Only a cross-service suite can ask this: the flow spans Amazon Kinesis Data Streams (the
//! shared source), IAM (which holds the roles and decides each read), Firehose (which
//! retrieves, buffers and delivers), Amazon S3 (which holds the delivered objects) and
//! CloudWatch Logs (which holds the reported events), mounted on ONE engine exactly as
//! `core/router/src/lib.rs` mounts them. The sibling suites
//! `cfn_deployed_firehose_under_permissioned_source_stack` and
//! `cfn_deployed_firehose_source_permission_repair` ask it of streams a STACK deployed, where
//! the repair restates the role's one policy instead of adding another.
//!
//! Nothing here is asserted about how fast the host ran: every wait polls for the CONDITION
//! under a generous deadline, and what must stay empty is re-read at every one of those
//! readings.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The request builder the cross-service suites share, so this suite speaks the same wire as
// the ones beside it rather than building a second encoder of its own.
use crate::cloudformation_metric_stream_exports_to_s3::send;
use crate::query_wire::{elements, form_encode};

/// The prefix every resource this suite creates is named under.
const SUITE: &str = "firehose-source-grant-union";

/// How long a stream is given to report itself usable.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(60);

/// How long a delivery, or the report of a read that was refused, is waited for: far longer
/// than the emulator's own source poll, authorization span and buffer window together.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How long what must hold NOTHING goes on being re-read once every expected report is there.
const SILENCE_WINDOW: Duration = Duration::from_secs(2);

/// How often a stream, a destination or a log stream is read again while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The `DeliveryStreamStatus` a stream retrieves and delivers data in.
const ACTIVE: &str = "ACTIVE";

/// The `errorCode` a source read the stream's role may not make is reported under — fixed
/// text.
const REFUSED_ERROR_CODE: &str = "Kinesis.AccessDenied";

/// The trust policy every role here carries: Firehose assumes the role a stream's source and
/// destination name, so the service principal has to be able to.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"#,
    r#"{"Service":"firehose.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The name of the policy each role is created with.
const DEPLOYED_POLICY: &str = "delivery";

/// The name of the policy a repair puts on the role BESIDE the one above.
const REPAIR_POLICY: &str = "source-read-repair";

/// The three Kinesis Data Streams calls the retrieval from a source stream is made of, in the
/// order the retrieval makes them.
const EVERY_SOURCE_READ: &[&str] = &[
    "kinesis:DescribeStream",
    "kinesis:GetShardIterator",
    "kinesis:GetRecords",
];

/// Amazon S3's own write of an object, which every role here allows so that the ONE difference
/// between the rows is what each may read from the source.
const OBJECT_WRITE: &str = "s3:PutObject";

/// The bucket every stream delivers into, each under its own prefix.
fn bucket() -> String {
    format!("{SUITE}-data")
}

/// The Kinesis data stream every row reads.
fn source_stream() -> String {
    format!("{SUITE}-source")
}

/// The log group every destination reports into.
fn log_group() -> String {
    format!("/aws/kinesisfirehose/{SUITE}")
}

/// The records the producer writes to the shared source in ONE `PutRecords`: more than one, so
/// what a row delivers is every record the source took and not just the first.
fn markers() -> Vec<Vec<u8>> {
    (0..2)
        .map(|at| format!("{SUITE}-marker-{at}\n").into_bytes())
        .collect()
}

/// One row: a role one call short of the retrieval, and the stream that reads under it.
#[derive(Debug, Clone, Copy)]
struct Row {
    /// What this row is, named in every assertion it makes.
    case: &'static str,
    /// The infix of this row's resource names and key prefix.
    key: &'static str,
    /// The call this row's role is NOT granted once the stream exists, which is therefore
    /// the first call of the retrieval it may not make.
    withheld: &'static str,
}

/// The rows, one per call of the retrieval.
const ROWS: &[Row] = &[
    Row {
        case: "a role short of the describe the read begins with",
        key: "describe",
        withheld: "kinesis:DescribeStream",
    },
    Row {
        case: "a role short of the shard iterator",
        key: "iterator",
        withheld: "kinesis:GetShardIterator",
    },
    Row {
        case: "a role short of the records read",
        key: "records",
        withheld: "kinesis:GetRecords",
    },
];

impl Row {
    /// The `Prefix` this row's destination delivers under, which no other row shares.
    fn prefix(self) -> String {
        format!("{}/", self.key)
    }

    /// This row's Firehose stream.
    fn stream(self) -> String {
        format!("{SUITE}-{}-stream", self.key)
    }

    /// The role this row's stream reads its source and writes its destination under.
    fn role(self) -> String {
        format!("{SUITE}-{}-role", self.key)
    }

    /// That role as a configuration names it.
    fn role_arn(self) -> String {
        format!(
            "arn:aws:iam::{}:role/{}",
            engine::emulator_account_id(),
            self.role()
        )
    }

    /// The log stream this row's `CloudWatchLoggingOptions` name, one per row so a report is
    /// attributable to the stream that made it.
    fn log_stream(self) -> String {
        format!("{}-DestinationDelivery", self.key)
    }

    /// The source read calls this row's role is granted ONCE THE STREAM EXISTS: the whole
    /// read minus the one it is short of. The role is created allowing all three, because the
    /// create resolves the retrieval's first call.
    fn granted(self) -> Vec<&'static str> {
        EVERY_SOURCE_READ
            .iter()
            .copied()
            .filter(|action| *action != self.withheld)
            .collect()
    }
}

/// An identity policy stating the object write on this suite's bucket and `reads` on its
/// source stream.
fn policy_document(reads: &[&str], source_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [
            {
                "Effect": "Allow",
                "Action": OBJECT_WRITE,
                "Resource": format!("arn:aws:s3:::{}/*", bucket()),
            },
            { "Effect": "Allow", "Action": reads, "Resource": source_arn },
        ],
    })
    .to_string()
}

/// The policy a repair ADDS: the one call the role was short of, on the source it reads — a
/// grant stated beside the policy the role already carries rather than instead of it.
fn granted_call_document(action: &str, source_arn: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": action, "Resource": source_arn }],
    })
    .to_string()
}

/// The five services this flow spans, on ONE shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signature verification is the emulator's concern, not any of these services'.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

#[tokio::test]
async fn a_grant_in_any_policy_of_the_source_role_restores_the_retrieval() {
    let router = emulator();

    // ---- The bucket, the source, the log destinations, and one role and one stream per row.
    // Each role is created allowing the WHOLE read, because the create resolves that read: a
    // stream whose source role may not make the retrieval's first call is never written. Once
    // the streams exist each role is narrowed to one call short of the read, which is the state
    // every reading below is of. ----
    given_bucket(&router).await;
    let source_arn = given_source(&router).await;
    given_log_group(&router).await;
    for row in ROWS {
        given_log_stream(&router, *row).await;
        given_role(
            &router,
            *row,
            DEPLOYED_POLICY,
            &policy_document(EVERY_SOURCE_READ, &source_arn),
        )
        .await;
        given_stream(&router, *row, &source_arn).await;
    }
    for row in ROWS {
        await_stream_active(&router, *row).await;
    }
    for row in ROWS {
        given_role_policy(
            &router,
            *row,
            DEPLOYED_POLICY,
            &policy_document(&row.granted(), &source_arn),
        )
        .await;
    }

    // ---- Each row reports the call its role may not make, under the one error code that
    // condition carries. The records go to the source only once every report is there, so
    // every reading below is of a retrieval the created policies decided. ----
    for row in ROWS {
        let document = await_reported(&router, *row).await;
        assert_eq!(
            document["errorCode"],
            json!(REFUSED_ERROR_CODE),
            "case {:?}: expected the reported errorCode {REFUSED_ERROR_CODE:?}, got {document}",
            row.case
        );
        let message = document["message"].as_str().unwrap_or_default().to_string();
        for named in [row.withheld, &source_arn, &row.role_arn()] {
            assert!(
                message.contains(named),
                "case {:?}: expected the message to name {named:?}, got {message:?}",
                row.case
            );
        }
    }

    // ---- The records on the shared source, written in one call as a producer writes a
    // batch. ----
    given_records(&router).await;

    // ---- No row delivers anything while its role is short of a call, at every reading over
    // one window. ----
    let until = Instant::now() + SILENCE_WINDOW;
    while Instant::now() < until {
        for row in ROWS {
            let keys = keys_under(&router, &row.prefix()).await;
            assert!(
                keys.is_empty(),
                "case {:?}: a refused retrieval delivers nothing, got {keys:?}",
                row.case
            );
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // ---- The repair: a SECOND policy, stating only the call that was missing, put on the role
    // the stream already names. What is in force on a role is every policy it carries, read at
    // the next attempt — so each row now retrieves the records it never took. ----
    for row in ROWS {
        given_role_policy(
            &router,
            *row,
            REPAIR_POLICY,
            &granted_call_document(row.withheld, &source_arn),
        )
        .await;
    }
    for row in ROWS {
        await_markers_delivered(&router, *row).await;
    }
}

/// Create the destination bucket, as an operator does before pointing a destination at it.
async fn given_bucket(router: &MultiServiceRouter) {
    let (status, body) = send(
        router,
        "s3",
        "PUT",
        &format!("/{}", bucket()),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "the bucket {} must be created, got {status} with {}",
        bucket(),
        String::from_utf8_lossy(&body)
    );
}

/// Create the source Kinesis data stream and answer the `StreamARN` that service reports for
/// it — the value a Firehose configuration binds to.
async fn given_source(router: &MultiServiceRouter) -> String {
    let (status, answered) = json_rpc(
        router,
        "kinesis",
        "Kinesis_20131202.CreateStream",
        &json!({ "StreamName": source_stream(), "ShardCount": 1 }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the source must be created, got {status} with {answered}"
    );
    let (status, described) = json_rpc(
        router,
        "kinesis",
        "Kinesis_20131202.DescribeStreamSummary",
        &json!({ "StreamName": source_stream() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the source must describe itself, got {status} with {described}"
    );
    described["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("the source reports its ARN, got {described}"))
        .to_string()
}

/// Create the log group every destination reports into.
async fn given_log_group(router: &MultiServiceRouter) {
    let (status, answered) = json_rpc(
        router,
        "logs",
        "Logs_20140328.CreateLogGroup",
        &json!({ "logGroupName": log_group() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "the log group must be created, got {status} with {answered}"
    );
}

/// Create the log stream `row`'s destination reports into.
async fn given_log_stream(router: &MultiServiceRouter, row: Row) {
    let (status, answered) = json_rpc(
        router,
        "logs",
        "Logs_20140328.CreateLogStream",
        &json!({ "logGroupName": log_group(), "logStreamName": row.log_stream() }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the log stream must be created, got {status} with {answered}",
        row.case
    );
}

/// Create `row`'s role, trusted by the Firehose service principal, carrying `document` as the
/// policy `policy_name`.
async fn given_role(router: &MultiServiceRouter, row: Row, policy_name: &str, document: &str) {
    let (status, answered) = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", &row.role()),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the role must be created, got {status} with {answered}",
        row.case
    );
    given_role_policy(router, row, policy_name, document).await;
}

/// Put `document` on `row`'s role as the policy `policy_name`, leaving every other policy it
/// carries where it is.
async fn given_role_policy(
    router: &MultiServiceRouter,
    row: Row,
    policy_name: &str,
    document: &str,
) {
    let (status, answered) = iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", &row.role()),
            ("PolicyName", policy_name),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the policy {policy_name} must be put, got {status} with {answered}",
        row.case
    );
}

/// Create `row`'s Kinesis-sourced stream: the shared source under this row's role, delivering
/// to this row's own prefix and reporting into this row's own log stream.
async fn given_stream(router: &MultiServiceRouter, row: Row, source_arn: &str) {
    let (status, answered) = json_rpc(
        router,
        "firehose",
        "Firehose_20150804.CreateDeliveryStream",
        &json!({
            "DeliveryStreamName": row.stream(),
            "DeliveryStreamType": "KinesisStreamAsSource",
            "KinesisStreamSourceConfiguration": {
                "KinesisStreamARN": source_arn,
                "RoleARN": row.role_arn(),
            },
            "ExtendedS3DestinationConfiguration": {
                "BucketARN": format!("arn:aws:s3:::{}", bucket()),
                "RoleARN": row.role_arn(),
                "CompressionFormat": "UNCOMPRESSED",
                "Prefix": row.prefix(),
                // The shortest wait `IntervalInSeconds` admits (published range 0..900) with
                // the smallest `SizeInMBs` (1..128): the rows are about WHETHER a record is
                // retrieved, not about a window they would wait out.
                "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                "CloudWatchLoggingOptions": {
                    "Enabled": true,
                    "LogGroupName": log_group(),
                    "LogStreamName": row.log_stream(),
                },
            },
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: the create must be served, got {status} with {answered}",
        row.case
    );
}

/// Write the records every row's stream retrieves for itself, in one `PutRecords`.
async fn given_records(router: &MultiServiceRouter) {
    let (status, answered) = json_rpc(
        router,
        "kinesis",
        "Kinesis_20131202.PutRecords",
        &json!({
            "StreamName": source_stream(),
            "Records": markers()
                .iter()
                .enumerate()
                .map(|(at, record)| json!({
                    "PartitionKey": format!("pk-marker-{at}"),
                    "Data": ::aws_smithy_types::base64::encode(record.clone()),
                }))
                .collect::<Vec<Value>>(),
        }),
    )
    .await;
    assert_eq!(
        (status, &answered["FailedRecordCount"]),
        (200, &json!(0)),
        "the source must accept the records, got {status} with {answered}"
    );
}

/// Poll Firehose until it reports `row`'s stream `ACTIVE` — which it becomes because the role
/// its create resolved allows the whole source read.
async fn await_stream_active(router: &MultiServiceRouter, row: Row) {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let (status, body) = json_rpc(
            router,
            "firehose",
            "Firehose_20150804.DescribeDeliveryStream",
            &json!({ "DeliveryStreamName": row.stream() }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from DescribeDeliveryStream, got {status} with {body}",
            row.case
        );
        let description = &body["DeliveryStreamDescription"];
        if description["DeliveryStreamStatus"] == json!(ACTIVE) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected the stream to be {ACTIVE}, got {description}",
            row.case
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until `row`'s log stream carries its first reported event, and answer that document.
async fn await_reported(router: &MultiServiceRouter, row: Row) -> Value {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        if let Some(document) = reported_events(router, row).await.into_iter().next() {
            return document;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected a reported event in {:?} / {:?} within {DELIVERY_DEADLINE:?}",
            row.case,
            log_group(),
            row.log_stream()
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Wait until every record the source took is held under `row`'s prefix.
///
/// One retrieved record is one delivered record, however many objects the destination's
/// buffering wrote them as, so what is asserted is the bytes the prefix holds altogether.
async fn await_markers_delivered(router: &MultiServiceRouter, row: Row) {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    let expected = markers();
    loop {
        let mut held = Vec::new();
        for key in keys_under(router, &row.prefix()).await {
            held.extend(object(router, &key).await);
        }
        if expected.iter().all(|marker| holds_subslice(&held, marker)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {:?}: expected every record the source took under {:?} within \
             {DELIVERY_DEADLINE:?}, got {:?}",
            row.case,
            row.prefix(),
            String::from_utf8_lossy(&held)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether `held` carries `wanted` somewhere in it.
fn holds_subslice(held: &[u8], wanted: &[u8]) -> bool {
    held.windows(wanted.len()).any(|window| window == wanted)
}

/// Every document `row`'s log stream holds, read through CloudWatch Logs' own `GetLogEvents`.
async fn reported_events(router: &MultiServiceRouter, row: Row) -> Vec<Value> {
    let (status, body) = json_rpc(
        router,
        "logs",
        "Logs_20140328.GetLogEvents",
        &json!({
            "logGroupName": log_group(),
            "logStreamName": row.log_stream(),
            "startFromHead": true,
        }),
    )
    .await;
    if status != 200 {
        return Vec::new();
    }
    body["events"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|event| event["message"].as_str())
        .filter_map(|message| serde_json::from_str::<Value>(message).ok())
        .collect()
}

/// Every key the shared bucket holds under `prefix`, read through Amazon S3's own
/// `ListObjectsV2`. Each row delivers at most a couple of objects, so one page holds them all.
async fn keys_under(router: &MultiServiceRouter, prefix: &str) -> Vec<String> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{}?list-type=2&prefix={prefix}", bucket()),
        &[],
        Vec::new(),
    )
    .await;
    let text = String::from_utf8_lossy(&body).to_string();
    assert_eq!(
        status,
        200,
        "expected HTTP 200 from ListObjectsV2 on {} under {prefix:?}, got {status} with {text}",
        bucket()
    );
    let mut keys = elements(&text, "Key");
    keys.sort();
    keys
}

/// The bytes one delivered object holds, read through `GetObject`.
async fn object(router: &MultiServiceRouter, key: &str) -> Vec<u8> {
    let (status, body) = send(
        router,
        "s3",
        "GET",
        &format!("/{}/{key}", bucket()),
        &[],
        Vec::new(),
    )
    .await;
    assert_eq!(
        status,
        200,
        "expected HTTP 200 from GetObject for {key:?}, got {status} with {}",
        String::from_utf8_lossy(&body)
    );
    body
}

/// One `awsJson1_1` operation of `signing_name`, answering its status and parsed body.
async fn json_rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target: &str,
    request: &Value,
) -> (u16, Value) {
    let (status, body) = send(
        router,
        signing_name,
        "POST",
        "/",
        &[
            ("content-type", "application/x-amz-json-1.1"),
            ("x-amz-target", target),
        ],
        request.to_string().into_bytes(),
    )
    .await;
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// One IAM action over its `awsQuery` wire, answering its status and body text.
async fn iam(router: &MultiServiceRouter, action: &str, members: &[(&str, &str)]) -> (u16, String) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in members {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let (status, answered) = send(
        router,
        "iam",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        body.into_bytes(),
    )
    .await;
    (status, String::from_utf8_lossy(&answered).to_string())
}
