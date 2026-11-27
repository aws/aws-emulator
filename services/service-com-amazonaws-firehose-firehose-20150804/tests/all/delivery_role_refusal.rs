// SPDX-License-Identifier: CC0-1.0

//! What a delivery the destination's ROLE did not allow writes, what it reports, and what a
//! create whose SOURCE role may not read the stream it configures answers.
//!
//! The rule under test: Firehose makes a delivery under the role the destination
//! configuration names ("the Amazon Resource Name (ARN) of the Amazon Web Services
//! credentials" — `S3DestinationConfiguration$RoleARN`), and that role's permissions are
//! evaluated AFRESH ON EVERY ATTEMPT: while they do not allow Amazon S3's own write of the
//! object, no object is written and the attempt is reported through the destination's own
//! `CloudWatchLoggingOptions` as its `S3.AccessDenied` delivery error; once they allow it
//! again, the next attempt is authorized and delivers — so the decision is the one in force
//! at the moment of the flush rather than the one in force when the stream was created.
//!
//! The SOURCE of a stream is the other half of that story, and the second table below is it: a
//! stream fed by a Kinesis data stream RETRIEVES its records under the role its source
//! configuration names ("the ARN of the role that provides access to the source stream" —
//! `KinesisStreamSourceConfiguration$RoleARN`), and Firehose makes the retrieval's own discovery
//! call under that role while `CreateDeliveryStream` is being served. A role whose permissions
//! withhold one of the calls that retrieval is made of therefore names a source the stream could
//! never read, which is an argument fault of the CREATE: HTTP 400, `InvalidArgumentException`
//! and the sentence naming the withheld call, the role and the stream — and no stream is
//! created, so a client is never handed one that reports healthy and can never deliver.
//!
//! The rows of the first table span what the delivery rule generalises over: HOW the permission
//! goes away (the inline
//! policy is deleted, replaced by one scoped to another bucket, replaced by one allowing only
//! a read), how many records are put while it is gone, and which destination member carries
//! the Amazon S3 location (`S3DestinationConfiguration`, `ExtendedS3DestinationConfiguration`).
//! The rows of the second span HOW the source role's permissions fall short of the read set — no
//! policy at all, only the destination's Amazon S3 writes, the whole read set on ANOTHER stream,
//! and each proper prefix of the read set — beside the two that are allowed it: the read set on
//! the stream itself and the read set on every resource.
//!
//! What becomes of the records a flush could not write is NOT this suite's subject.
//!
//! Five services are mounted on ONE shared engine, exactly as the router mounts them: the
//! role is created and changed through IAM's own wire and the decision is IAM's, the bucket
//! and the delivered objects are Amazon S3's, the source stream is Kinesis Data Streams' own,
//! and the refusals are read back through CloudWatch Logs' own `GetLogEvents`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::delivery_stream_lifecycle::{at, error_code, error_message, PAST_THE_TRANSITION, REGION};
use crate::destination_delivery_errors::{allow, authorization, form_encode};

/// The endpoint every request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// How long a delivery or a refusal is waited for before a row fails: far longer than the
/// emulator's own delivery span, so a slow machine does not fail a row the clock would have
/// served.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// How often the destination, the buffer and the log stream are read while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The bucket each row's destination delivers into.
const BUCKET: &str = "firehose-role-refusal";

/// Another bucket Amazon S3 holds, so a row can scope the role's grant to a bucket that is
/// not the destination's.
const OTHER_BUCKET: &str = "firehose-role-refusal-other";

/// The log group each row's `CloudWatchLoggingOptions` name.
const LOG_GROUP: &str = "/aws/kinesisfirehose/role-refusal";

/// The log stream they name — the one a Firehose destination's delivery errors go to.
const LOG_STREAM: &str = "DestinationDelivery";

/// The role every row delivers under.
const ROLE_NAME: &str = "firehose-role-refusal";

/// The one inline policy on that role, which a row removes and then restores.
const POLICY_NAME: &str = "s3-delivery-access";

/// The stream every row puts its records on.
const STREAM: &str = "role-refusal";

/// Where the delivered objects go.
const PREFIX: &str = "data/";

/// The `errorCode` a destination whose role does not allow the write reports.
const REFUSED_ERROR_CODE: &str = "S3.AccessDenied";

/// The record every row puts while the role still allows the write.
const BEFORE_REVOKE: &[u8] = b"before-revoke\n";

/// The record every row puts once the permission is restored.
const AFTER_RESTORE: &[u8] = b"after-restore\n";

/// One payload as the line a consumer reads back out of the destination.
fn line_of(payload: &[u8]) -> String {
    String::from_utf8_lossy(payload).trim().to_string()
}

/// `bucket` as the `BucketARN` a destination names it by.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// Firehose, Amazon S3, IAM and CloudWatch Logs on one shared engine.
struct Aws {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Aws {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        // The role a delivery is made under is IAM's to decide about, so IAM holds the role
        // these rows change and answers the question each delivery puts to it.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // The refusals are CloudWatch Logs': the destination's logging writes them there and
        // this suite reads them back through that service's own reader.
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        // The source of a `KinesisStreamAsSource` stream is a Kinesis data stream, created
        // through that service's own wire so the ARN the create-time table configures is one
        // Kinesis Data Streams really issued.
        router.register_service(
            service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    /// One `awsJson1_1` call: `service` is the routing key its requests are signed for and
    /// `target` the prefix of its `X-Amz-Target`.
    async fn json_rpc(
        &self,
        service: &str,
        target: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{target}.{operation}"))
            .header("authorization", authorization(service))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Firehose operation that must succeed.
    async fn firehose_ok(&self, operation: &str, body: Value) -> Value {
        let (status, answered) = self
            .json_rpc("firehose", "Firehose_20150804", operation, body)
            .await;
        assert_eq!(status, 200, "{operation} must succeed, got {answered}");
        answered
    }

    /// One CloudWatch Logs operation.
    async fn logs(&self, operation: &str, body: Value) -> (u16, Value) {
        self.json_rpc("logs", "Logs_20140328", operation, body).await
    }

    /// One awsQuery call to IAM that must succeed.
    async fn iam(&self, action: &str, params: &[(&str, String)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri(format!("https://{HOST}/"))
            .header("host", HOST)
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("iam"))
            .body(bytes::Bytes::from(body))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        assert_eq!(
            status,
            200,
            "{action} must succeed, got {status} with {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// `PUT /<bucket>` — Amazon S3's `CreateBucket`, naming the Region this suite signs for.
    async fn create_bucket(&self, bucket: &str) {
        let payload = format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        );
        let request = http::Request::builder()
            .method("PUT")
            .uri(format!("https://{HOST}/{bucket}"))
            .header("host", HOST)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::from(payload))
            .expect("the request builds");
        let response = self.router.dispatch(request).await;
        assert_eq!(
            response.status().as_u16(),
            200,
            "create bucket {bucket}: got {}",
            String::from_utf8_lossy(response.body())
        );
    }

    /// Every line of every object [`BUCKET`] holds — what a consumer reading the whole
    /// destination back sees, which is what a record either is or is not among.
    fn delivered_lines(&self) -> Vec<String> {
        let Some(state) = service_com_amazonaws_s3_amazons3::state::service_state(&*self.engine)
        else {
            return Vec::new();
        };
        let store = state.lock();
        store
            .bucket(BUCKET)
            .map(|bucket| {
                bucket
                    .objects()
                    .flat_map(|(_, object)| {
                        String::from_utf8_lossy(&object.body)
                            .lines()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the destination holds a line carrying `payload`.
    fn delivered(&self, payload: &[u8]) -> bool {
        let expected = line_of(payload);
        self.delivered_lines().iter().any(|line| line == &expected)
    }

    /// The `errorCode`s the destination's logging has reported so far.
    async fn reported_codes(&self) -> Vec<String> {
        let (status, answered) = self
            .logs(
                "GetLogEvents",
                json!({
                    "logGroupName": LOG_GROUP,
                    "logStreamName": LOG_STREAM,
                    "startFromHead": true,
                }),
            )
            .await;
        if status != 200 {
            return Vec::new();
        }
        answered["events"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|event| event["message"].as_str())
            .filter_map(|message| serde_json::from_str::<Value>(message).ok())
            .filter_map(|document| {
                document["errorCode"]
                    .as_str()
                    .map(::std::string::ToString::to_string)
            })
            .collect()
    }

    /// `PutRecord` of one payload, which must be accepted.
    async fn put(&self, payload: &[u8]) {
        self.firehose_ok(
            "PutRecord",
            json!({
                "DeliveryStreamName": STREAM,
                "Record": { "Data": aws_smithy_types::base64::encode(payload) },
            }),
        )
        .await;
    }

    /// Create the role every row delivers under, with `policy` as its one inline policy.
    async fn create_role(&self, policy: &Value) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "firehose.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            "CreateRole",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        self.put_role_policy(policy).await;
        format!(
            "arn:aws:iam::{}:role/{ROLE_NAME}",
            engine::emulator_account_id()
        )
    }

    /// `PutRolePolicy` of the role's one inline policy.
    async fn put_role_policy(&self, policy: &Value) {
        self.iam(
            "PutRolePolicy",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("PolicyName", POLICY_NAME.to_string()),
                ("PolicyDocument", policy.to_string()),
            ],
        )
        .await;
    }

    /// Create a role NAMED `name`, trusted by the Firehose service principal and carrying
    /// `policy` as its one inline policy (none at all for `None`), and answer its ARN.
    ///
    /// Beside [`Aws::create_role`], which holds the one role the delivery table revokes and
    /// restores: the create-time table needs a role PER ROW, so each row's grant stands alone.
    async fn create_named_role(&self, name: &str, policy: Option<&Value>) -> String {
        let trust = json!({
            "Version": "2012-10-17",
            "Statement": [{
                "Effect": "Allow",
                "Principal": {"Service": "firehose.amazonaws.com"},
                "Action": "sts:AssumeRole",
            }],
        })
        .to_string();
        self.iam(
            "CreateRole",
            &[
                ("RoleName", name.to_string()),
                ("AssumeRolePolicyDocument", trust),
            ],
        )
        .await;
        if let Some(policy) = policy {
            self.iam(
                "PutRolePolicy",
                &[
                    ("RoleName", name.to_string()),
                    ("PolicyName", POLICY_NAME.to_string()),
                    ("PolicyDocument", policy.to_string()),
                ],
            )
            .await;
        }
        format!(
            "arn:aws:iam::{}:role/{name}",
            engine::emulator_account_id()
        )
    }

    /// Create the Kinesis data stream `name` and answer the `StreamARN` it reports — the value a
    /// client binds a `KinesisStreamAsSource` stream to.
    async fn create_source(&self, name: &str) -> String {
        let (status, answered) = self
            .json_rpc(
                "kinesis",
                "Kinesis_20131202",
                "CreateStream",
                json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
        assert_eq!(status, 200, "create source stream {name}: got {answered}");
        let (status, answered) = self
            .json_rpc(
                "kinesis",
                "Kinesis_20131202",
                "DescribeStreamSummary",
                json!({ "StreamName": name }),
            )
            .await;
        assert_eq!(status, 200, "describe source stream {name}: got {answered}");
        answered["StreamDescriptionSummary"]["StreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("the source stream reports its ARN, got {answered}"))
            .to_string()
    }

    /// `DeleteRolePolicy` of that same policy.
    async fn delete_role_policy(&self) {
        self.iam(
            "DeleteRolePolicy",
            &[
                ("RoleName", ROLE_NAME.to_string()),
                ("PolicyName", POLICY_NAME.to_string()),
            ],
        )
        .await;
    }

    /// Wait for `ready` to hold without making any Firehose call, so what makes it hold is
    /// the emulator's own delivery clock.
    async fn wait_until<F>(&self, case: &str, expected: &str, mut ready: F)
    where
        F: FnMut() -> bool,
    {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        while !ready() {
            assert!(
                Instant::now() < deadline,
                "case {case:?}: {expected} within {DELIVERY_DEADLINE:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Wait for the destination's logging to report `code`, which is how a refused delivery
    /// becomes observable to an operator.
    async fn wait_for_report(&self, case: &str, code: &str) {
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let reported = self.reported_codes().await;
            if reported.iter().any(|reported| reported == code) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: expected {code:?} reported within {DELIVERY_DEADLINE:?}, got \
                 {reported:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The policy that lets the delivery be made: the write of any object of the destination
/// bucket.
fn delivery_policy() -> Value {
    allow("s3:PutObject", &format!("{}/*", bucket_arn(BUCKET)))
}

/// How a row takes the delivery permission away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Revocation {
    /// `DeleteRolePolicy` removes the role's only S3 permissions.
    DeleteThePolicy,
    /// The policy is replaced by one granting the same action on ANOTHER bucket.
    ScopeToAnotherBucket,
    /// The policy is replaced by one granting only a read of the same bucket.
    AllowOnlyARead,
}
/// Which destination member carries the Amazon S3 location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    /// `S3DestinationConfiguration`.
    S3,
    /// `ExtendedS3DestinationConfiguration`.
    ExtendedS3,
}

/// One row of the table.
struct Row {
    case: &'static str,
    revocation: Revocation,
    /// The records put while the role does not allow their delivery.
    withheld: &'static [&'static [u8]],
    destination: Destination,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the policy is deleted",
            revocation: Revocation::DeleteThePolicy,
            withheld: &[b"after-revoke\n"],
            destination: Destination::ExtendedS3,
        },
        Row {
            case: "the policy is scoped to another bucket",
            revocation: Revocation::ScopeToAnotherBucket,
            withheld: &[b"after-revoke-one\n", b"after-revoke-two\n"],
            destination: Destination::S3,
        },
        Row {
            case: "the policy allows only a read",
            revocation: Revocation::AllowOnlyARead,
            withheld: &[b"after-revoke\n"],
            destination: Destination::ExtendedS3,
        },
    ]
}

#[tokio::test]
async fn a_record_the_delivery_role_withheld_is_delivered_once_the_role_allows_the_write() {
    for row in rows() {
        let case = row.case;
        let aws = Aws::new();
        aws.create_bucket(BUCKET).await;
        aws.create_bucket(OTHER_BUCKET).await;
        let (status, answered) = aws
            .logs("CreateLogGroup", json!({ "logGroupName": LOG_GROUP }))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the log group must be created, got {status} {answered}"
        );
        let role_arn = aws.create_role(&delivery_policy()).await;

        // The shortest buffering the hints admit, so one put is one delivery attempt.
        let destination = json!({
            "RoleARN": role_arn,
            "BucketARN": bucket_arn(BUCKET),
            "Prefix": PREFIX,
            "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
            "CloudWatchLoggingOptions": {
                "Enabled": true,
                "LogGroupName": LOG_GROUP,
                "LogStreamName": LOG_STREAM,
            },
        });
        let member = match row.destination {
            Destination::S3 => "S3DestinationConfiguration",
            Destination::ExtendedS3 => "ExtendedS3DestinationConfiguration",
        };
        aws.firehose_ok(
            "CreateDeliveryStream",
            json!({ "DeliveryStreamName": STREAM, member: destination }),
        )
        .await;
        tokio::time::sleep(PAST_THE_TRANSITION).await;

        // With the policy in force the delivery is made, which is the control this row's
        // refusal is measured against.
        aws.put(BEFORE_REVOKE).await;
        aws.wait_until(case, "the record put under the policy is delivered", || {
            aws.delivered(BEFORE_REVOKE)
        })
        .await;

        // The permission goes away, and the records put while it is gone are not written.
        match row.revocation {
            Revocation::DeleteThePolicy => aws.delete_role_policy().await,
            Revocation::ScopeToAnotherBucket => {
                aws.put_role_policy(&allow(
                    "s3:PutObject",
                    &format!("{}/*", bucket_arn(OTHER_BUCKET)),
                ))
                .await;
            }
            Revocation::AllowOnlyARead => {
                aws.put_role_policy(&allow(
                    "s3:GetObject",
                    &format!("{}/*", bucket_arn(BUCKET)),
                ))
                .await;
            }
        }
        for payload in row.withheld {
            aws.put(payload).await;
        }
        // The attempt is made and reported, which is the only thing a producer can observe
        // of a delivery the role did not allow.
        aws.wait_for_report(case, REFUSED_ERROR_CODE).await;
        for payload in row.withheld {
            assert!(
                !aws.delivered(payload),
                "case {case:?}: a record the role may not deliver reaches no object while the \
                 permission is gone, got {:?}",
                aws.delivered_lines()
            );
        }

        // The permission is restored, identical to the one the first delivery was made
        // under, and the next attempt is authorized again: the record put after the repair
        // is delivered, so the decision was re-read rather than settled when the stream was
        // created. What becomes of the records withheld while the permission was gone is a
        // separate question this suite does not judge.
        aws.put_role_policy(&delivery_policy()).await;
        aws.put(AFTER_RESTORE).await;
        aws.wait_until(case, "the record put after the repair is delivered", || {
            aws.delivered(AFTER_RESTORE)
        })
        .await;

        // What the destination holds: the record delivered under the policy, and the one put
        // once the policy was back.
        let lines = aws.delivered_lines();
        for payload in [BEFORE_REVOKE, AFTER_RESTORE] {
            let expected = line_of(payload);
            assert!(
                lines.contains(&expected),
                "case {case:?}: the destination holds {expected:?}, got {lines:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The SOURCE role, resolved by the create
// ---------------------------------------------------------------------------

/// The `DeliveryStreamType` of a stream fed by a Kinesis data stream: "the Firehose stream uses
/// a Kinesis data stream as a source" (`CreateDeliveryStreamInput$DeliveryStreamType`).
const KINESIS_STREAM_AS_SOURCE: &str = "KinesisStreamAsSource";

/// The source Kinesis data stream every row of the create-time table configures.
const SOURCE: &str = "role-refusal-source";

/// Another Kinesis data stream, so a row can grant the read set on a stream that is NOT the
/// source it configures.
const OTHER_SOURCE: &str = "role-refusal-other-source";

/// The calls the retrieval from a Kinesis source is made of, in the order it makes them: the
/// three read-only access to a Kinesis data stream is stated with — the discovery of the
/// stream's shards, the position taken in each of them, and the read of the records from that
/// position. A refusal names the FIRST of them the role's permissions withhold, which is why the
/// rows below walk that order.
const SOURCE_READ_ACTIONS: [&str; 3] = [
    "kinesis:DescribeStream",
    "kinesis:GetShardIterator",
    "kinesis:GetRecords",
];

/// An identity-based policy document allowing `actions` on `resource`.
///
/// Beside [`allow`], which states ONE action: a row granting a proper prefix of the read set
/// states several.
fn allow_actions(actions: &[&str], resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{"Effect": "Allow", "Action": actions, "Resource": resource}],
    })
}

/// The sentence a create refused for its source read carries: the role, the call its permissions
/// withhold and the stream the call was to be made on — fixed text around three values the
/// request itself carried, built here rather than taken from the crate under test.
fn source_read_refusal(role_arn: &str, action: &str, source_arn: &str) -> String {
    format!("Role {role_arn} is not authorized to perform: {action} on resource {source_arn}.")
}

/// What a row grants the role it creates, from `(the source ARN, the other source ARN, the
/// destination bucket's ARN)` — every one of them a value this run really minted.
type Grant = fn(&str, &str, &str) -> Option<Value>;

/// One row of the create-time table: how the source role's permissions stand, and what the
/// create owes it.
struct SourceRow {
    case: &'static str,
    /// The role the row creates, so each row's grant stands alone.
    role: &'static str,
    /// The Firehose stream the row asks for.
    stream: &'static str,
    grant: Grant,
    /// The call the refusal must name, or `None` where the create must be accepted.
    refused: Option<&'static str>,
}

fn source_rows() -> Vec<SourceRow> {
    vec![
        SourceRow {
            case: "a role trusted by Firehose that carries no policy at all",
            role: "source-read-no-policy",
            stream: "source-read-no-policy",
            grant: |_source, _other, _bucket| None,
            refused: Some(SOURCE_READ_ACTIONS[0]),
        },
        SourceRow {
            case: "a role granting only the destination's Amazon S3 write",
            role: "source-read-s3-only",
            stream: "source-read-s3-only",
            grant: |_source, _other, bucket| {
                Some(allow_actions(
                    &["s3:PutObject", "s3:GetBucketLocation"],
                    &format!("{bucket}/*"),
                ))
            },
            refused: Some(SOURCE_READ_ACTIONS[0]),
        },
        SourceRow {
            case: "a role granting the whole read set on another stream",
            role: "source-read-other-stream",
            stream: "source-read-other-stream",
            grant: |_source, other, _bucket| Some(allow_actions(&SOURCE_READ_ACTIONS, other)),
            refused: Some(SOURCE_READ_ACTIONS[0]),
        },
        SourceRow {
            case: "a role granting the first call of the read set only",
            role: "source-read-describe-only",
            stream: "source-read-describe-only",
            grant: |source, _other, _bucket| {
                Some(allow_actions(&SOURCE_READ_ACTIONS[..1], source))
            },
            refused: Some(SOURCE_READ_ACTIONS[1]),
        },
        SourceRow {
            case: "a role granting the first two calls of the read set",
            role: "source-read-iterator-only",
            stream: "source-read-iterator-only",
            grant: |source, _other, _bucket| {
                Some(allow_actions(&SOURCE_READ_ACTIONS[..2], source))
            },
            refused: Some(SOURCE_READ_ACTIONS[2]),
        },
        SourceRow {
            case: "a role granting the whole read set on the source stream",
            role: "source-read-whole-set",
            stream: "source-read-whole-set",
            grant: |source, _other, _bucket| Some(allow_actions(&SOURCE_READ_ACTIONS, source)),
            refused: None,
        },
        SourceRow {
            case: "a role granting the whole read set on every resource",
            role: "source-read-wildcard",
            stream: "source-read-wildcard",
            grant: |_source, _other, _bucket| Some(allow_actions(&SOURCE_READ_ACTIONS, "*")),
            refused: None,
        },
    ]
}

/// RULE: `CreateDeliveryStream` resolves what the configured SOURCE role may do on the Kinesis
/// data stream it names before anything is written — so a role withholding one of the
/// retrieval's calls is refused with the sentence naming the first withheld call and no stream
/// is created, while a role allowing them all is accepted and the stream is read back at its own
/// name, carrying the source the create configured.
#[tokio::test]
async fn a_create_whose_source_role_may_not_read_the_stream_is_refused_and_creates_nothing() {
    let aws = Aws::new();
    aws.create_bucket(BUCKET).await;
    let source_arn = aws.create_source(SOURCE).await;
    let other_arn = aws.create_source(OTHER_SOURCE).await;
    let destination_bucket = bucket_arn(BUCKET);

    for row in source_rows() {
        let case = row.case;
        let grant = (row.grant)(&source_arn, &other_arn, &destination_bucket);
        let role_arn = aws.create_named_role(row.role, grant.as_ref()).await;

        let (status, answered) = aws
            .json_rpc(
                "firehose",
                "Firehose_20150804",
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": row.stream,
                    "DeliveryStreamType": KINESIS_STREAM_AS_SOURCE,
                    "KinesisStreamSourceConfiguration": {
                        "KinesisStreamARN": source_arn,
                        "RoleARN": role_arn,
                    },
                    "S3DestinationConfiguration": {
                        "RoleARN": role_arn,
                        "BucketARN": destination_bucket,
                        "Prefix": PREFIX,
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                    },
                }),
            )
            .await;

        // Whatever the create answered, what the account HOLDS under that name is read back
        // through the describe: a refusal may leave nothing behind, and an accepted create owes
        // the stream it reported.
        let (described_status, described) = aws
            .json_rpc(
                "firehose",
                "Firehose_20150804",
                "DescribeDeliveryStream",
                json!({ "DeliveryStreamName": row.stream }),
            )
            .await;

        match row.refused {
            Some(action) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {answered}"
                );
                let code = error_code(&answered);
                assert_eq!(
                    code, "InvalidArgumentException",
                    "case {case:?}: expected InvalidArgumentException, got {code} with {answered}"
                );
                let expected = source_read_refusal(&role_arn, action, &source_arn);
                let message = error_message(&answered);
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                let code = error_code(&described);
                assert_eq!(
                    code, "ResourceNotFoundException",
                    "case {case:?}: expected no stream under {:?}, got {described_status} with \
                     {described}",
                    row.stream
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with {answered}"
                );
                assert!(
                    answered["DeliveryStreamARN"].as_str().is_some(),
                    "case {case:?}: expected a DeliveryStreamARN, got {answered}"
                );
                assert_eq!(
                    described_status, 200,
                    "case {case:?}: expected the created stream to be described, got \
                     {described_status} with {described}"
                );
                let source = at(
                    &described,
                    "DeliveryStreamDescription/Source/KinesisStreamSourceDescription",
                );
                assert_eq!(
                    source["KinesisStreamARN"].as_str(),
                    Some(source_arn.as_str()),
                    "case {case:?}: expected the configured source stream, got {source}"
                );
                assert_eq!(
                    source["RoleARN"].as_str(),
                    Some(role_arn.as_str()),
                    "case {case:?}: expected the configured source role, got {source}"
                );
            }
        }
    }
}
