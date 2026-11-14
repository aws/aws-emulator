// SPDX-License-Identifier: CC0-1.0

//! Where a web ACL's log records actually ARRIVE, end to end over the wire.
//!
//! The rule under test: while a web ACL carries an enabled logging configuration, every
//! request it inspects leaves one record at the destination `LogDestinationConfigs` names,
//! written in the form THAT destination's own readers read it — a CloudWatch Logs log group
//! takes one log event per record, an Amazon S3 bucket takes it as a gzipped
//! newline-delimited JSON object under WAF's published key layout
//! (`AWSLogs/<account-id>/WAFLogs/<region>/<web-acl-name>/<Y>/<m>/<d>/<H>/<M>/…log.gz`), and
//! an Amazon Data Firehose delivery stream is handed it as a data record — and each record
//! reports the request as WAF inspected it, the action it applied, and every rule that
//! matched without terminating the evaluation under `nonTerminatingMatchingRules`.
//! "You can use an Amazon CloudWatch Logs log group, an Amazon Simple Storage Service
//! (Amazon S3) bucket, or an Amazon Kinesis Data Firehose"
//! (`smithy_query(query="doc",shape="LoggingConfiguration")`) is where that set of
//! destination kinds comes from.
//!
//! The rows differ along what the rule generalises over: which KIND of destination the
//! configuration names, which FORM its ARN is written in (a log group on its own and a log
//! group with everything under it), and a destination of no such kind at all, which is
//! delivered nowhere. Each row drives the same request through the same rule set, so the
//! destination is the only difference between them.
//!
//! The record is WAFv2's, the request is one the web ACL in front of an Amazon Cognito user
//! pool decides (a user pool is one of the protected resource kinds `AssociateWebACL`
//! takes), and the destinations belong to CloudWatch Logs, Amazon S3 and Amazon Data
//! Firehose — every one of them mounted on ONE shared engine, as the assembled emulator
//! mounts them, so a record is read back through the OWNING service's own reader and one
//! that never arrived cannot pass.

use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call is signed for and every ARN below names.
const REGION: &str = "us-west-2";

/// How long a clock-owed step is waited for before a case fails — a delivery stream
/// reaching the status that lets it take data, and the delivery it then makes. Generous: far
/// longer than the spans the emulator chooses, so a loaded machine does not fail a case the
/// clock would have served.
const CONDITION_DEADLINE: Duration = Duration::from_secs(60);

/// How often such a condition is looked at while it is waited for.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The `Scope` a user pool's web ACL lives in: a user pool is a REGIONAL resource.
const SCOPE: &str = "REGIONAL";

/// The header every request carries and the rule matches on, and its value.
const PROBE_HEADER: &str = "x-waf-delivery-probe";
const PROBE_VALUE: &str = "deliver-me";

/// The web ACL rule that COUNTS the probed request: it matches and does not terminate, so
/// the request is allowed by the `DefaultAction` and the rule is reported as a
/// non-terminating match.
const COUNT_RULE: &str = "count-the-probe";

/// The `action` a counted rule reports, from `SampledHTTPRequest$Action`'s own value set.
const COUNT_ACTION: &str = "COUNT";

/// The `terminatingRuleId` of a request no rule terminated.
const DEFAULT_ACTION_RULE: &str = "Default_Action";

/// One emulator with WAFv2, the service whose resource the web ACL sits in front of, and
/// the three services a logging destination can live in, all on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite exercises the services, not SigV4: every call carries a credential
        // scope so the router can name the target service, and no signature is verified.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(Arc::clone(&engine)),
        );
        router.register_service(
            service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            engine,
        ));
        Self { router }
    }

    /// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what
    /// routes the request. The signature itself is never checked here.
    fn authorization(signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
             aws4_request, SignedHeaders=host, Signature=0000"
        )
    }

    /// Dispatch one request as it arrives on the wire, answering its status and body bytes.
    async fn dispatch(
        &self,
        method: &str,
        host: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Bytes,
    ) -> (u16, Vec<u8>) {
        let mut builder = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host);
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let response = self
            .router
            .dispatch(builder.body(body).expect("a well-formed request"))
            .await;
        (response.status().as_u16(), response.into_body().to_vec())
    }

    /// One awsJson1_1 call of the service `signing_name`, whose `X-Amz-Target` prefix is
    /// `target_prefix`.
    async fn json_rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: Value,
    ) -> (u16, Value) {
        let (status, bytes) = self
            .dispatch(
                "POST",
                &format!("{signing_name}.{REGION}.amazonaws.com"),
                "/",
                &[
                    ("content-type", "application/x-amz-json-1.1"),
                    ("authorization", &Self::authorization(signing_name)),
                    ("x-amz-target", &format!("{target_prefix}.{operation}")),
                ],
                Bytes::from(body.to_string()),
            )
            .await;
        let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// One WAFv2 call that has to succeed.
    async fn waf_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self
            .json_rpc("wafv2", "AWSWAF_20190729", operation, body)
            .await;
        assert_eq!(
            status, 200,
            "setup {operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One Amazon Cognito call that has to succeed.
    async fn cognito_ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self
            .json_rpc(
                "cognito-idp",
                "AWSCognitoIdentityProviderService",
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "setup {operation}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// One CloudWatch Logs call, answering its status and parsed body.
    async fn logs(&self, operation: &str, body: Value) -> (u16, Value) {
        self.json_rpc("logs", "Logs_20140328", operation, body)
            .await
    }

    /// Create the log group `name` through CloudWatch Logs' own wire, so the destination the
    /// configuration names is a group the account really holds.
    async fn given_log_group(&self, name: &str) {
        let (status, payload) = self
            .logs("CreateLogGroup", json!({ "logGroupName": name }))
            .await;
        assert_eq!(
            status, 200,
            "setup CreateLogGroup({name}): expected HTTP 200, got {status} with {payload}"
        );
    }

    /// Create the bucket `name` through Amazon S3's own (REST, path-style) wire, in the
    /// Region every request here is signed for — which a `CreateBucketConfiguration` has to
    /// state for any Region but Amazon S3's own default one.
    async fn given_bucket(&self, name: &str) {
        let configuration = format!(
            "<CreateBucketConfiguration xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <LocationConstraint>{REGION}</LocationConstraint></CreateBucketConfiguration>"
        );
        let (status, body) = self
            .dispatch(
                "PUT",
                &format!("s3.{REGION}.amazonaws.com"),
                &format!("/{name}"),
                &[("authorization", &Self::authorization("s3"))],
                Bytes::from(configuration),
            )
            .await;
        assert_eq!(
            status,
            200,
            "setup CreateBucket({name}): expected HTTP 200, got {status} with {:?}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Create the delivery stream `name`, delivering into the bucket `sink`, and wait for it
    /// to reach the status that lets it take data ("Attempts to send data to a delivery
    /// stream that is not in the ACTIVE state cause an exception").
    async fn given_delivery_stream(&self, name: &str, sink: &str) {
        self.given_bucket(sink).await;
        let (status, payload) = self
            .json_rpc(
                "firehose",
                "Firehose_20150804",
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": name,
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": format!(
                            "arn:aws:iam::{}:role/firehose-delivery",
                            engine::emulator_account_id()
                        ),
                        "BucketARN": format!("arn:aws:s3:::{sink}"),
                        // The shortest buffering `BufferingHints` admits, so what this case
                        // waits for is the stream's own delivery span and nothing longer.
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                    },
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "setup CreateDeliveryStream({name}): expected HTTP 200, got {status} with {payload}"
        );
        // The CONDITION, looked at until it holds: the status the stream reports, never a
        // wait of a fixed length.
        let deadline = ::tokio::time::Instant::now() + CONDITION_DEADLINE;
        loop {
            let (_, described) = self
                .json_rpc(
                    "firehose",
                    "Firehose_20150804",
                    "DescribeDeliveryStream",
                    json!({ "DeliveryStreamName": name }),
                )
                .await;
            let reported =
                described["DeliveryStreamDescription"]["DeliveryStreamStatus"].clone();
            if reported == json!("ACTIVE") {
                return;
            }
            assert!(
                ::tokio::time::Instant::now() < deadline,
                "setup CreateDeliveryStream({name}): the stream must become ACTIVE, got \
                 {reported}"
            );
            ::tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// The records `FilterLogEvents` reports for `group`, each parsed as the JSON document a
    /// WAF log record is, or `None` when the account holds no such group at all.
    async fn in_log_group(&self, group: &str) -> Option<Vec<Value>> {
        let (status, body) = self
            .logs("FilterLogEvents", json!({ "logGroupName": group }))
            .await;
        if status != 200 {
            return None;
        }
        Some(
            body["events"]
                .as_array()
                .map(|events| {
                    events
                        .iter()
                        .map(|event| {
                            serde_json::from_str(event["message"].as_str().unwrap_or_default())
                                .unwrap_or(Value::Null)
                        })
                        .collect()
                })
                .unwrap_or_default(),
        )
    }

    /// Every object key `bucket` holds, read through Amazon S3's own `ListObjectsV2`.
    async fn keys_in_bucket(&self, bucket: &str) -> Vec<String> {
        let (status, body) = self
            .dispatch(
                "GET",
                &format!("s3.{REGION}.amazonaws.com"),
                &format!("/{bucket}?list-type=2"),
                &[("authorization", &Self::authorization("s3"))],
                Bytes::new(),
            )
            .await;
        let listing = String::from_utf8_lossy(&body).to_string();
        assert_eq!(
            status, 200,
            "ListObjectsV2({bucket}): expected HTTP 200, got {status} with {listing:?}"
        );
        listing
            .split("<Key>")
            .skip(1)
            .filter_map(|tail| tail.split("</Key>").next())
            .map(str::to_string)
            .collect()
    }

    /// The records the object `key` of `bucket` carries: the object's bytes, decompressed,
    /// read as the newline-delimited JSON a delivered WAF log file is.
    async fn records_in_object(&self, case: &str, bucket: &str, key: &str) -> Vec<Value> {
        let (status, body) = self
            .dispatch(
                "GET",
                &format!("s3.{REGION}.amazonaws.com"),
                &format!("/{bucket}/{key}"),
                &[("authorization", &Self::authorization("s3"))],
                Bytes::new(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetObject({bucket}/{key}) must serve the object, got {status}"
        );
        let mut decoded = String::new();
        flate2::read::GzDecoder::new(body.as_slice())
            .read_to_string(&mut decoded)
            .unwrap_or_else(|error| {
                panic!("case {case:?}: the object {key:?} must be a gzip stream: {error}")
            });
        decoded
            .lines()
            .map(|line| {
                serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!("case {case:?}: every line of {key:?} must be JSON: {error}")
                })
            })
            .collect()
    }
}

/// A `VisibilityConfig` for a dimension, which every rule and web ACL needs.
fn visibility(metric: &str) -> Value {
    json!({
        "SampledRequestsEnabled": true,
        "CloudWatchMetricsEnabled": true,
        "MetricName": metric,
    })
}

/// A `Blob` member travels base64-encoded in an awsJson body, which is what
/// `ByteMatchStatement$SearchString` is.
fn base64_of(value: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(value.as_bytes())
}

/// The one rule every row drives: it COUNTS a request carrying the probe header, which is a
/// match that does not terminate the evaluation.
fn counting_rule() -> Value {
    json!([{
        "Name": COUNT_RULE,
        "Priority": 0,
        "Statement": {
            "ByteMatchStatement": {
                "SearchString": base64_of(PROBE_VALUE),
                "FieldToMatch": { "SingleHeader": { "Name": PROBE_HEADER } },
                "TextTransformations": [{ "Priority": 0, "Type": "NONE" }],
                "PositionalConstraint": "EXACTLY",
            }
        },
        "Action": { "Count": {} },
        "VisibilityConfig": visibility(COUNT_RULE),
    }])
}

/// The user pool and app client one row's requests are made against: a pool is one of the
/// resource kinds `AssociateWebACLRequest$ResourceArn` takes, and the governed half of that
/// pool's API names its pool by the app client the call is made through.
async fn pool_with_a_client(emulator: &Emulator, name: &str) -> (String, String) {
    let created = emulator
        .cognito_ok("CreateUserPool", json!({ "PoolName": name }))
        .await;
    let id = created["UserPool"]["Id"]
        .as_str()
        .expect("a minted pool id")
        .to_string();
    let arn = created["UserPool"]["Arn"]
        .as_str()
        .expect("the pool's ARN")
        .to_string();
    let client = emulator
        .cognito_ok(
            "CreateUserPoolClient",
            json!({ "UserPoolId": id, "ClientName": format!("{name}-client") }),
        )
        .await;
    let client_id = client["UserPoolClient"]["ClientId"]
        .as_str()
        .expect("a minted client id")
        .to_string();
    (arn, client_id)
}

/// Create the web ACL `name` with [`counting_rule`] and the default `Allow`, put it in front
/// of `resource_arn`, and answer its ARN.
async fn web_acl_in_front(emulator: &Emulator, name: &str, resource_arn: &str) -> String {
    let created = emulator
        .waf_ok(
            "CreateWebACL",
            json!({
                "Name": name,
                "Scope": SCOPE,
                "DefaultAction": { "Allow": {} },
                "VisibilityConfig": visibility(name),
                "Rules": counting_rule(),
            }),
        )
        .await;
    let arn = created["Summary"]["ARN"]
        .as_str()
        .expect("Summary.ARN")
        .to_string();
    emulator
        .waf_ok(
            "AssociateWebACL",
            json!({ "WebACLArn": arn, "ResourceArn": resource_arn }),
        )
        .await;
    arn
}

/// Drive ONE request the web ACL inspects: a governed operation of the pool's own API,
/// carrying the probe header. What the operation itself answers is not what this suite is
/// about — the web ACL decides and logs the request before the operation runs — so only the
/// delivery is asserted.
async fn one_inspected_request(emulator: &Emulator, client_id: &str) {
    emulator
        .dispatch(
            "POST",
            &format!("cognito-idp.{REGION}.amazonaws.com"),
            "/",
            &[
                ("content-type", "application/x-amz-json-1.1"),
                ("authorization", &Emulator::authorization("cognito-idp")),
                (
                    "x-amz-target",
                    "AWSCognitoIdentityProviderService.ForgotPassword",
                ),
                (PROBE_HEADER, PROBE_VALUE),
            ],
            Bytes::from(
                json!({ "ClientId": client_id, "Username": "delivery-probe-user" }).to_string(),
            ),
        )
        .await;
}

/// Put a logging configuration naming `destination` in front of the web ACL `arn`.
async fn configure_logging(emulator: &Emulator, arn: &str, destination: &str) {
    emulator
        .waf_ok(
            "PutLoggingConfiguration",
            json!({
                "LoggingConfiguration": {
                    "ResourceArn": arn,
                    "LogDestinationConfigs": [destination],
                }
            }),
        )
        .await;
}

/// What a record must say about the request that produced it, whichever destination it was
/// delivered to.
fn assert_record(case: &str, record: &Value, web_acl_arn: &str) {
    assert_eq!(
        record["action"],
        json!("ALLOW"),
        "case {case:?}: the record must name the action WAF applied, got {record}"
    );
    assert_eq!(
        record["terminatingRuleId"],
        json!(DEFAULT_ACTION_RULE),
        "case {case:?}: no rule terminated this request, got {record}"
    );
    assert_eq!(
        record["webaclId"],
        json!(web_acl_arn),
        "case {case:?}: the record must name the web ACL that decided, got {record}"
    );
    assert_eq!(
        record["httpSourceName"],
        json!("COGNITOIDP"),
        "case {case:?}: the record must name the kind of resource the request arrived at, \
         got {record}"
    );
    assert_eq!(
        record["nonTerminatingMatchingRules"],
        json!([{ "ruleId": COUNT_RULE, "action": COUNT_ACTION }]),
        "case {case:?}: a rule that matched without terminating must be reported as a \
         non-terminating match, got {record}"
    );
    let probed = record["httpRequest"]["headers"]
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or_default()
        .iter()
        .find(|header| {
            header["name"]
                .as_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(PROBE_HEADER))
        })
        .and_then(|header| header["value"].as_str())
        .unwrap_or_default()
        .to_string();
    assert_eq!(
        probed, PROBE_VALUE,
        "case {case:?}: the record must reproduce the request's headers, got {probed:?} in \
         {record}"
    );
}

/// One row of the log-group table: the case, the group, and whether the configuration names
/// it by the ARN of the group ALONE or of the group with everything under it.
struct LogGroupRow {
    case: &'static str,
    group: &'static str,
    /// Whether the ARN carries the `:*` that names everything under the group, which is how
    /// `DescribeLogGroups` reports a group's own ARN.
    everything_under_it: bool,
}

/// A log group named by its ARN takes one log event per record, in either of the two forms a
/// client reads such an ARN in.
#[tokio::test]
async fn a_log_group_destination_takes_one_event_per_inspected_request() {
    let rows = [
        LogGroupRow {
            case: "the log group on its own",
            group: "aws-waf-logs-group-plain",
            everything_under_it: false,
        },
        LogGroupRow {
            case: "the log group with everything under it",
            group: "aws-waf-logs-group-wildcard",
            everything_under_it: true,
        },
    ];
    for row in rows {
        let (case, group) = (row.case, row.group);
        let destination = format!(
            "arn:aws:logs:{REGION}:{account}:log-group:{group}{under}",
            account = engine::emulator_account_id(),
            under = if row.everything_under_it { ":*" } else { "" }
        );
        let emulator = Emulator::new();
        emulator.given_log_group(group).await;
        let (pool_arn, client_id) = pool_with_a_client(&emulator, group).await;
        let web_acl = web_acl_in_front(&emulator, group, &pool_arn).await;
        configure_logging(&emulator, &web_acl, &destination).await;

        one_inspected_request(&emulator, &client_id).await;

        let delivered = emulator
            .in_log_group(group)
            .await
            .unwrap_or_else(|| panic!("case {case:?}: the destination group was not created"));
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: one inspected request is one record, got {delivered:?}"
        );
        assert_record(case, &delivered[0], &web_acl);
    }
}

/// A bucket named by an S3 ARN takes the record as the gzipped newline-delimited JSON object
/// a reader of that bucket fetches, under WAF's published key layout.
#[tokio::test]
async fn a_bucket_destination_takes_a_gzipped_json_object_per_inspected_request() {
    let case = "an Amazon S3 bucket";
    let bucket = "aws-waf-logs-delivery-bucket";
    let emulator = Emulator::new();
    emulator.given_bucket(bucket).await;
    let (pool_arn, client_id) = pool_with_a_client(&emulator, bucket).await;
    let web_acl = web_acl_in_front(&emulator, bucket, &pool_arn).await;
    configure_logging(&emulator, &web_acl, &format!("arn:aws:s3:::{bucket}")).await;

    one_inspected_request(&emulator, &client_id).await;

    let keys = emulator.keys_in_bucket(bucket).await;
    assert_eq!(
        keys.len(),
        1,
        "case {case:?}: one inspected request is one delivered object, got {keys:?}"
    );
    // The published layout: the log-file prefix down to the web ACL, the minute the records
    // belong to as a path, and a file name carrying the account, WAF, the Region and the web
    // ACL. The gzip suffix is what the object IS.
    let prefix = format!(
        "AWSLogs/{}/WAFLogs/{REGION}/{web_acl_name}/",
        engine::emulator_account_id(),
        web_acl_name = bucket
    );
    assert!(
        keys[0].starts_with(&prefix),
        "case {case:?}: expected the object under {prefix:?}, got {:?}",
        keys[0]
    );
    assert!(
        keys[0].ends_with(".log.gz"),
        "case {case:?}: expected a gzipped log file, got {:?}",
        keys[0]
    );
    assert!(
        keys[0].contains(&format!(
            "{}_waflogs_{REGION}_{bucket}_",
            engine::emulator_account_id()
        )),
        "case {case:?}: expected the file name WAF writes, got {:?}",
        keys[0]
    );

    let records = emulator.records_in_object(case, bucket, &keys[0]).await;
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: the object must carry the record, got {records:?}"
    );
    assert_record(case, &records[0], &web_acl);

    // Reading the destination changes nothing a later caller sees: a second inspected
    // request leaves a second object and the first one stands.
    one_inspected_request(&emulator, &client_id).await;
    let keys_again = emulator.keys_in_bucket(bucket).await;
    assert_eq!(
        keys_again.len(),
        2,
        "case {case:?}: a second inspected request is a second object, got {keys_again:?}"
    );
    assert!(
        keys_again.contains(&keys[0]),
        "case {case:?}: the object already delivered must still be there, got {keys_again:?}"
    );
}

/// A delivery stream named by a Firehose ARN is handed the record, which the stream then
/// delivers into the destination it was created against.
#[tokio::test]
async fn a_delivery_stream_destination_is_handed_the_record() {
    let case = "an Amazon Data Firehose delivery stream";
    let stream = "aws-waf-logs-delivery-stream";
    let sink = "aws-waf-logs-stream-sink";
    let emulator = Emulator::new();
    emulator.given_delivery_stream(stream, sink).await;
    let (pool_arn, client_id) = pool_with_a_client(&emulator, stream).await;
    let web_acl = web_acl_in_front(&emulator, stream, &pool_arn).await;
    configure_logging(
        &emulator,
        &web_acl,
        &format!(
            "arn:aws:firehose:{REGION}:{}:deliverystream/{stream}",
            engine::emulator_account_id()
        ),
    )
    .await;

    one_inspected_request(&emulator, &client_id).await;

    // The stream delivers what it is given into its own destination, so the record is read
    // back out of the bucket that stream was created against — looked at until the delivery
    // the clock owes has been made.
    let deadline = ::tokio::time::Instant::now() + CONDITION_DEADLINE;
    let keys = loop {
        let keys = emulator.keys_in_bucket(sink).await;
        if !keys.is_empty() {
            break keys;
        }
        assert!(
            ::tokio::time::Instant::now() < deadline,
            "case {case:?}: the stream must have delivered the record into its destination, \
             got no object in {sink:?}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    };
    let mut delivered: Vec<Value> = Vec::new();
    for key in &keys {
        let (status, body) = emulator
            .dispatch(
                "GET",
                &format!("s3.{REGION}.amazonaws.com"),
                &format!("/{sink}/{key}"),
                &[("authorization", &Emulator::authorization("s3"))],
                Bytes::new(),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetObject({sink}/{key}) must serve the object, got {status}"
        );
        let text = String::from_utf8_lossy(&body).to_string();
        delivered.extend(
            text.lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok()),
        );
    }
    assert_eq!(
        delivered.len(),
        1,
        "case {case:?}: one inspected request is one data record, got {delivered:?}"
    );
    assert_record(case, &delivered[0], &web_acl);
}

/// A destination of no kind a record can be delivered to leaves the record undelivered:
/// nothing is written to a log group, a bucket or a stream of that name.
#[tokio::test]
async fn a_destination_of_no_deliverable_kind_writes_nowhere() {
    let case = "a destination naming a resource of no deliverable kind";
    let name = "aws-waf-logs-elsewhere";
    let emulator = Emulator::new();
    emulator.given_log_group(name).await;
    emulator.given_bucket(name).await;
    let (pool_arn, client_id) = pool_with_a_client(&emulator, name).await;
    let web_acl = web_acl_in_front(&emulator, name, &pool_arn).await;
    // A Kinesis data STREAM: an ARN WAF's own destination rule accepts by name, naming a
    // resource that is none of the three kinds a logging destination can be.
    configure_logging(
        &emulator,
        &web_acl,
        &format!(
            "arn:aws:kinesis:{REGION}:{}:stream/{name}",
            engine::emulator_account_id()
        ),
    )
    .await;

    one_inspected_request(&emulator, &client_id).await;

    let in_group = emulator.in_log_group(name).await.unwrap_or_default();
    assert!(
        in_group.is_empty(),
        "case {case:?}: expected no record in the log group {name:?}, got {in_group:?}"
    );
    let in_bucket = emulator.keys_in_bucket(name).await;
    assert!(
        in_bucket.is_empty(),
        "case {case:?}: expected no object in the bucket {name:?}, got {in_bucket:?}"
    );
}
