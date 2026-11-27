// SPDX-License-Identifier: CC0-1.0

//! What `CreateDeliveryStream` RESOLVES, against the services that own them, before it
//! registers a stream.
//!
//! The rule under test: **a Firehose stream's configuration names resources that belong to
//! OTHER services, and Firehose resolves them against those services when the stream is
//! WRITTEN: every `BucketARN` the create names has to be a bucket Amazon S3 holds, and the role
//! a Kinesis source names has to be one IAM lets make the retrieval's own discovery call on the
//! stream it names — or the configuration is an
//! ARGUMENT fault — HTTP 400 `InvalidArgumentException` (`smithy_query(query="operation",
//! operation="CreateDeliveryStream")` models it as the client error of this operation) whose
//! message names what did not resolve — and NOTHING is registered: the name the request asked
//! for is still free, and a create whose configuration does resolve takes it. Firehose makes the
//! retrieval's first call (`kinesis:DescribeStream`, then `kinesis:GetShardIterator`, then
//! `kinesis:GetRecords`) under the configured role while the create is served, so a role whose
//! policy withholds one of them names a source the stream could never read and the refusal names
//! the FIRST withheld call, the role and the stream; a role that may make them all is served, and
//! the stream it registers becomes usable and reports the source it was configured with. What
//! that role may do LATER is not settled here: each retrieval puts the question again.**
//!
//! Only a cross-service suite can ask this: the resources are not Firehose's. The flow spans
//! Amazon S3 (which holds the destination buckets), IAM (which holds the roles and decides
//! what each may do), Amazon Kinesis Data Streams (the source a stream reads) and Firehose
//! itself, mounted on ONE engine exactly as `core/router/src/lib.rs` mounts them.
//!
//! The rows differ along what the rule generalises over:
//!
//! * for the bucket, WHICH member carried it (a plain and an extended Amazon S3 destination's
//!   own, the `S3BackupConfiguration` of an extended one, and the `S3Configuration` a Splunk
//!   and a Redshift destination delivers through) and the FORM of the name that is not there
//!   (a near miss of the held bucket's name, a name the held one is a prefix of, a dotted
//!   name, the longest name an Amazon S3 general purpose bucket has, and an absent bucket in
//!   another partition), since acceptance has to be Amazon S3's own answer and not a match
//!   made here;
//! * for the source role, WHICH of the three read calls its policy allows (all three, all of
//!   Kinesis by wildcard, every one but the records read, every one but the describe, none at
//!   all) and whether the grant it does carry names the source stream or another one — the first
//!   call it withholds being the one the refusal names.
//!
//! Each bucket refusal is then checked twice over: the stream is not registered, and the same
//! name is taken by a create whose configuration resolves — so what the refusal answered was
//! the configuration, not the name. What a source role's permissions withhold ONCE THE STREAM
//! EXISTS is reported where a live stream's conditions are reported, which the suites
//! `cfn_deployed_firehose_source_permission_repair` and
//! `cfn_deployed_firehose_under_permissioned_source_stack` hold to the rule.
//!
//! Nothing here is asserted about how fast the host ran: every wait polls for the CONDITION
//! under a generous deadline.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::query_wire::form_encode;

/// The Region every call below is signed for, and the one the buckets are created in.
const REGION: &str = "us-west-2";

/// How long a created stream is given to report itself usable: far longer than the emulator's
/// own `CREATING` -> `ACTIVE` span, so a loaded machine never fails a case the clock would
/// have served.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(60);

/// How often a stream's status is read again while waiting.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The trust policy every role here carries: Firehose assumes the role a stream's source and
/// destination name, so the service principal has to be able to.
const TRUST_POLICY: &str = concat!(
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"#,
    r#"{"Service":"firehose.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#
);

/// The bucket Amazon S3 DOES hold, which every row's working members deliver to.
const HELD_BUCKET: &str = "firehose-preflight-held";

/// The error code an argument fault of this operation carries.
const INVALID_ARGUMENT: &str = "InvalidArgumentException";

/// The error code a stream the account does not hold is reported under, which is what a
/// refused create must leave behind.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The three Kinesis Data Streams calls the retrieval from a source stream is made of, in the
/// order the retrieval makes them.
const EVERY_SOURCE_READ: &[&str] = &[
    "kinesis:DescribeStream",
    "kinesis:GetShardIterator",
    "kinesis:GetRecords",
];

/// Firehose, Amazon S3, IAM and Amazon Kinesis Data Streams on one shared engine.
struct Aws {
    router: MultiServiceRouter,
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
        // The roles a configuration names are IAM's to hold and IAM's to decide about.
        router.register_service(
            service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        // The source a `KinesisStreamAsSource` stream reads is a stream of that service.
        router.register_service(
            service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// One request to the service `signing_name`; answers its status and body bytes.
    async fn send(
        &self,
        signing_name: &str,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Vec<u8>,
    ) -> (u16, Vec<u8>) {
        let host = format!("{signing_name}.{REGION}.amazonaws.com");
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("authorization", authorization(signing_name));
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(bytes::Bytes::from(body))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        (response.status().as_u16(), response.body().to_vec())
    }

    /// One `awsJson1_1` operation of `signing_name`, answering its status and parsed body.
    async fn json_rpc(&self, signing_name: &str, target: &str, body: Value) -> (u16, Value) {
        let (status, answered) = self
            .send(
                signing_name,
                "POST",
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("x-amz-target", target),
                ],
                body.to_string().into_bytes(),
            )
            .await;
        let text = String::from_utf8_lossy(&answered).to_string();
        let parsed = if text.is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// One Firehose operation, with whatever it answered.
    async fn firehose(&self, operation: &str, body: Value) -> (u16, Value) {
        self.json_rpc("firehose", &format!("Firehose_20150804.{operation}"), body)
            .await
    }

    /// One `awsQuery` IAM action that must succeed.
    async fn iam(&self, case: &str, action: &str, parameters: &[(&str, &str)]) {
        let mut body = format!("Action={action}&Version=2010-05-08");
        for (key, value) in parameters {
            body.push('&');
            body.push_str(key);
            body.push('=');
            body.push_str(&form_encode(value));
        }
        let (status, answered) = self
            .send(
                "iam",
                "POST",
                "/",
                &[("content-type", "application/x-www-form-urlencoded")],
                body.into_bytes(),
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: IAM {action} must succeed, got {status} {}",
            String::from_utf8_lossy(&answered)
        );
    }

    /// Create the role `name`, trusted by the Firehose service principal and carrying
    /// `statements` as its one inline policy — or no policy at all when `statements` is
    /// empty. Answers its ARN.
    async fn given_role(&self, case: &str, name: &str, statements: &[String]) -> String {
        self.iam(
            case,
            "CreateRole",
            &[("RoleName", name), ("AssumeRolePolicyDocument", TRUST_POLICY)],
        )
        .await;
        if !statements.is_empty() {
            self.put_role_policy(case, name, statements).await;
        }
        format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
    }

    /// State `statements` as the role's one inline policy, replacing whatever it carried.
    async fn put_role_policy(&self, case: &str, name: &str, statements: &[String]) {
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{}]}}"#,
            statements.join(",")
        );
        self.iam(
            case,
            "PutRolePolicy",
            &[
                ("RoleName", name),
                ("PolicyName", "firehose-preflight"),
                ("PolicyDocument", &document),
            ],
        )
        .await;
    }

    /// Create the bucket `name`, as an operator does before pointing a destination at it.
    async fn given_bucket(&self, case: &str, name: &str) {
        let (status, body) = self
            .send(
                "s3",
                "PUT",
                &format!("/{name}"),
                &[],
                format!(
                    "<CreateBucketConfiguration><LocationConstraint>{REGION}\
                     </LocationConstraint></CreateBucketConfiguration>"
                )
                .into_bytes(),
            )
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: the bucket {name} must be created, got {status} {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Create the source Kinesis data stream `name` and answer the `StreamARN` that service
    /// reports for it — the value a client binds a Firehose stream to.
    async fn given_source(&self, case: &str, name: &str) -> String {
        let (status, answered) = self
            .json_rpc(
                "kinesis",
                "Kinesis_20131202.CreateStream",
                json!({ "StreamName": name, "ShardCount": 1 }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the source {name} must be created, got {status} {answered}"
        );
        let (status, described) = self
            .json_rpc(
                "kinesis",
                "Kinesis_20131202.DescribeStreamSummary",
                json!({ "StreamName": name }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the source {name} must describe itself, got {status} {described}"
        );
        described["StreamDescriptionSummary"]["StreamARN"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case:?}: the source {name} reports its ARN, got {described}")
            })
            .to_string()
    }

    /// What `DescribeDeliveryStream` answers for `name` right now.
    async fn describe(&self, name: &str) -> (u16, Value) {
        self.firehose(
            "DescribeDeliveryStream",
            json!({ "DeliveryStreamName": name }),
        )
        .await
    }

    /// Wait for `name` to report `ACTIVE` and answer its description.
    ///
    /// A poll loop on the STATUS rather than a wait for the clock-owed span, so how fast the
    /// host runs cannot decide a case.
    async fn await_active(&self, case: &str, name: &str) -> Value {
        let deadline = Instant::now() + ACTIVE_DEADLINE;
        loop {
            let (status, answered) = self.describe(name).await;
            let description = &answered["DeliveryStreamDescription"];
            if status == 200 && description["DeliveryStreamStatus"] == json!("ACTIVE") {
                return description.clone();
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: {name} must reach ACTIVE within {ACTIVE_DEADLINE:?}, got \
                 {status} {answered}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. Neither the key
/// id nor the signature is a credential: this router runs with request authentication off, so
/// nothing here is verified.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// An identity-policy statement allowing `actions` on `resource`.
fn allow(actions: &[&str], resource: &str) -> String {
    let actions = actions
        .iter()
        .map(|action| format!("\"{action}\""))
        .collect::<Vec<_>>()
        .join(",");
    format!(r#"{{"Effect":"Allow","Action":[{actions}],"Resource":"{resource}"}}"#)
}

/// `bucket` as the `BucketARN` a destination names it by.
fn bucket_arn(bucket: &str) -> String {
    format!("arn:aws:s3:::{bucket}")
}

/// The wire error code of a refusal, without its namespace.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The message a refusal carries, whichever casing the envelope uses.
fn error_message(body: &Value) -> String {
    body.get("message")
        .or_else(|| body.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// One bucket row: which member carries the bucket that is not there.
struct BucketCase {
    case: &'static str,
    /// The `CreateDeliveryStream` member the configuration is submitted at.
    member: &'static str,
    /// That configuration, built from the role, the bucket ARN under test and the ARN of the
    /// bucket Amazon S3 does hold (for the rows whose other members must resolve).
    config: fn(&str, &str, &str) -> Value,
}

/// The members that carry an Amazon S3 location into a create. Every one of them is a bucket
/// Firehose writes objects to, so one rule answers for them all.
fn bucket_cases() -> Vec<BucketCase> {
    vec![
        BucketCase {
            case: "the destination's own bucket",
            member: "S3DestinationConfiguration",
            config: |role, under_test, _held| {
                json!({ "RoleARN": role, "BucketARN": under_test, "Prefix": "data/" })
            },
        },
        BucketCase {
            case: "an extended destination's own bucket",
            member: "ExtendedS3DestinationConfiguration",
            config: |role, under_test, _held| {
                json!({ "RoleARN": role, "BucketARN": under_test, "Prefix": "data/" })
            },
        },
        BucketCase {
            case: "the backup bucket of an extended destination whose own bucket is held",
            member: "ExtendedS3DestinationConfiguration",
            config: |role, under_test, held| {
                json!({
                    "RoleARN": role,
                    "BucketARN": held,
                    "S3BackupMode": "Enabled",
                    "S3BackupConfiguration": { "RoleARN": role, "BucketARN": under_test },
                })
            },
        },
        BucketCase {
            case: "the bucket a Splunk destination delivers through",
            member: "SplunkDestinationConfiguration",
            config: |role, under_test, _held| {
                json!({
                    "HECEndpoint": "https://splunk.example.com:8088",
                    "HECEndpointType": "Raw",
                    "S3Configuration": { "RoleARN": role, "BucketARN": under_test },
                })
            },
        },
        BucketCase {
            case: "the bucket a Redshift destination delivers through",
            member: "RedshiftDestinationConfiguration",
            config: |role, under_test, _held| {
                json!({
                    "RoleARN": role,
                    "ClusterJDBCURL":
                        "jdbc:redshift://example.abc123.us-west-2.redshift.amazonaws.com:5439/dev",
                    "CopyCommand": { "DataTableName": "events" },
                    "Username": "analytics",
                    "Password": "sup3r-secret-pw",
                    "S3Configuration": { "RoleARN": role, "BucketARN": under_test },
                })
            },
        },
    ]
}

/// Bucket names Amazon S3 does not hold, differing along the FORM of the name: a near miss of
/// the held bucket's name, a name the held one is a prefix of, a dotted name, the longest name
/// an Amazon S3 general purpose bucket has (63 characters) and an absent bucket in another
/// partition. Every one is a name Amazon S3 would accept, so "not there" is all a row varies.
fn absent_buckets() -> Vec<(&'static str, String, String)> {
    let near_miss = HELD_BUCKET[..HELD_BUCKET.len() - 1].to_string();
    let extends = format!("{HELD_BUCKET}-absent");
    let dotted = "my.absent.preflight-01".to_string();
    let longest = "a".repeat(63);
    let other_partition = "preflight-absent-gov".to_string();
    vec![
        (
            "a near miss of the held bucket's name",
            bucket_arn(&near_miss),
            near_miss,
        ),
        (
            "a name the held one is a prefix of",
            bucket_arn(&extends),
            extends,
        ),
        ("a dotted name", bucket_arn(&dotted), dotted),
        (
            "the longest name an Amazon S3 bucket has",
            bucket_arn(&longest),
            longest,
        ),
        (
            "an absent bucket in another partition",
            format!("arn:aws-us-gov:s3:::{other_partition}"),
            other_partition,
        ),
    ]
}

#[tokio::test]
async fn a_bucket_amazon_s3_does_not_hold_is_refused_at_every_member_and_every_name_form() {
    let aws = Aws::new();
    aws.given_bucket("setup", HELD_BUCKET).await;
    let role = aws.given_role("setup", "firehose-preflight-bucket", &[]).await;
    let held = bucket_arn(HELD_BUCKET);

    for (row, member_case) in bucket_cases().iter().enumerate() {
        for (form, absent_arn, absent_name) in absent_buckets() {
            let case = format!("{} / {form}", member_case.case);
            let stream = format!("preflight-bucket-{row}");
            let mut request = json!({ "DeliveryStreamName": stream });
            request[member_case.member] = (member_case.config)(&role, &absent_arn, &held);
            let (status, refused) = aws.firehose("CreateDeliveryStream", request).await;
            assert_eq!(
                (status, error_code(&refused).as_str()),
                (400, INVALID_ARGUMENT),
                "case {case:?}: expected 400 {INVALID_ARGUMENT}, got {status} with {refused}"
            );
            assert_eq!(
                error_message(&refused),
                format!("S3 Bucket {absent_name} does not exist."),
                "case {case:?}: the refusal must name the bucket the request asked for, got \
                 {refused}"
            );

            // Nothing was registered: the create that was refused left no stream behind, so
            // the reader answers the not-found of a name this account does not hold.
            let (status, described) = aws.describe(&stream).await;
            assert_eq!(
                (status, error_code(&described).as_str()),
                (400, NOT_FOUND),
                "case {case:?}: a refused create must register nothing, got {status} with \
                 {described}"
            );
        }

        // And the SAME name is taken by a create whose every bucket Amazon S3 holds: what the
        // refusals above answered was the configuration, not the name or the member.
        let case = format!("{} / every bucket held", member_case.case);
        let stream = format!("preflight-bucket-{row}");
        let mut request = json!({ "DeliveryStreamName": stream });
        request[member_case.member] = (member_case.config)(&role, &held, &held);
        let (status, created) = aws.firehose("CreateDeliveryStream", request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the create to be served, got {status} with {created}"
        );
        assert!(
            created["DeliveryStreamARN"]
                .as_str()
                .is_some_and(|arn| arn.ends_with(&format!("deliverystream/{stream}"))),
            "case {case:?}: the create answers the stream's ARN, got {created}"
        );
    }
}

/// One source row: what the role's policy allows about the source read.
struct SourceCase {
    case: &'static str,
    /// The role name this row creates, so each row's policy is its own.
    role_name: &'static str,
    /// Which of the source read calls the role's policy allows; empty for a role that carries
    /// no policy at all.
    granted: &'static [&'static str],
    /// Whether the grant it carries names the source stream (`true`) or another one.
    scoped_to_the_source: bool,
    /// The first call of the retrieval this role may not make, which the refusal names, or
    /// `None` for a role that may make them all.
    refused: Option<&'static str>,
}

/// The shapes of source grant a role can carry, from one allowing every call the retrieval
/// makes to one allowing none of them: a row whose role withholds one of those calls is a row
/// the create refuses, naming the first of them it withholds.
fn source_cases() -> Vec<SourceCase> {
    vec![
        SourceCase {
            case: "a role allowing the three source read calls",
            role_name: "firehose-preflight-every-read",
            granted: EVERY_SOURCE_READ,
            scoped_to_the_source: true,
            refused: None,
        },
        SourceCase {
            case: "a role allowing every Kinesis call by wildcard",
            role_name: "firehose-preflight-wildcard",
            granted: &["kinesis:*"],
            scoped_to_the_source: true,
            refused: None,
        },
        SourceCase {
            case: "a role that may not read the records",
            role_name: "firehose-preflight-no-records",
            granted: &["kinesis:DescribeStream", "kinesis:GetShardIterator"],
            scoped_to_the_source: true,
            refused: Some("kinesis:GetRecords"),
        },
        SourceCase {
            case: "a role that may not describe the source",
            role_name: "firehose-preflight-no-describe",
            granted: &["kinesis:GetShardIterator", "kinesis:GetRecords"],
            scoped_to_the_source: true,
            refused: Some("kinesis:DescribeStream"),
        },
        SourceCase {
            case: "a role whose source grant names another stream",
            role_name: "firehose-preflight-elsewhere",
            granted: EVERY_SOURCE_READ,
            scoped_to_the_source: false,
            refused: Some("kinesis:DescribeStream"),
        },
        SourceCase {
            case: "a role carrying no source grant at all",
            role_name: "firehose-preflight-no-grant",
            granted: &[],
            scoped_to_the_source: true,
            refused: Some("kinesis:DescribeStream"),
        },
    ]
}

#[tokio::test]
async fn a_create_is_refused_unless_its_source_role_may_read_the_source() {
    let aws = Aws::new();
    aws.given_bucket("setup", HELD_BUCKET).await;
    let source_arn = aws.given_source("setup", "preflight-ingest").await;
    let elsewhere = format!(
        "arn:aws:kinesis:{REGION}:{}:stream/elsewhere",
        engine::emulator_account_id()
    );

    // Every row's create, each under a role whose policy says something different about the
    // source read.
    let cases = source_cases();
    for (row, source_case) in cases.iter().enumerate() {
        let case = source_case.case;
        let mut statements = Vec::new();
        if !source_case.granted.is_empty() {
            let resource = if source_case.scoped_to_the_source {
                source_arn.clone()
            } else {
                elsewhere.clone()
            };
            statements.push(allow(source_case.granted, &resource));
        }
        let role = aws
            .given_role(case, source_case.role_name, &statements)
            .await;
        let stream = format!("preflight-source-{row}");
        let request = json!({
            "DeliveryStreamName": stream,
            "DeliveryStreamType": "KinesisStreamAsSource",
            "KinesisStreamSourceConfiguration": {
                "KinesisStreamARN": source_arn,
                "RoleARN": role,
            },
            "ExtendedS3DestinationConfiguration": {
                "RoleARN": role,
                "BucketARN": bucket_arn(HELD_BUCKET),
                "Prefix": "data/",
            },
        });
        let (status, answered) = aws.firehose("CreateDeliveryStream", request).await;

        match source_case.refused {
            // A role that may not make one of the retrieval's calls names a source this stream
            // could never read: the create is the argument fault, it names the first call the
            // role withholds, and it registers nothing.
            Some(action) => {
                assert_eq!(
                    (status, error_code(&answered).as_str()),
                    (400, INVALID_ARGUMENT),
                    "case {case:?}: expected the create to be refused, got {status} with \
                     {answered}"
                );
                let expected = format!(
                    "Role {role} is not authorized to perform: {action} on resource \
                     {source_arn}."
                );
                let message = answered["message"].as_str().unwrap_or_default();
                assert_eq!(
                    message, expected,
                    "case {case:?}: expected {expected:?}, got {message:?}"
                );
                let (status, described) = aws.describe(&stream).await;
                assert_eq!(
                    (status, error_code(&described).as_str()),
                    (400, NOT_FOUND),
                    "case {case:?}: a refused create must register nothing, got {status} with \
                     {described}"
                );
            }
            // A role that may make them all is served, and the stream it registered is usable,
            // reporting the source it will retrieve and the role it will retrieve it under — the
            // configuration, kept as submitted.
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the create to be served, got {status} with {answered}"
                );
                let description = aws.await_active(case, &stream).await;
                let source = &description["Source"]["KinesisStreamSourceDescription"];
                assert_eq!(
                    (&source["KinesisStreamARN"], &source["RoleARN"]),
                    (&json!(source_arn), &json!(role)),
                    "case {case:?}: the stream reports the source it reads and the role it reads \
                     it under, got {description}"
                );
            }
        }
    }
}
