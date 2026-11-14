// SPDX-License-Identifier: CC0-1.0

//! `Publish` DELIVERS the message to the Amazon Data Firehose delivery streams subscribed to
//! the topic, end to end over the wire (awsQuery for SNS, `awsJson1_1` for Firehose,
//! path-style REST for Amazon S3) through the real `MultiServiceRouter`, with Firehose and
//! S3 mounted on the same engine so a delivery is observed where a client observes it: in
//! the objects the stream's destination bucket holds.
//!
//! The RULE asserted here: SNS "delivers the message to each endpoint that is subscribed to
//! the topic" (`Publish` doc), and a `firehose` subscription's endpoint is a delivery
//! stream — "firehose – delivery of JSON-encoded message to an Amazon Data Firehose
//! delivery stream" (`SubscribeInput$Protocol`) — so every published message becomes ONE
//! record put into that stream. WHAT the record carries is the subscription's own
//! `RawMessageDelivery`: "When set to true, enables raw message delivery […] This eliminates
//! the need for the endpoints to process JSON formatting, which is otherwise created for
//! Amazon SNS metadata" (`GetSubscriptionAttributesResponse$Attributes`), so a raw
//! subscription's record is the published body ITSELF with nothing added — the bodies land in
//! the destination back to back — while every other `firehose` subscription's record is the
//! JSON notification followed by a newline, so one record can be read apart from the next.
//!
//! The rows differ along what the rule generalises over: whether the subscription asked for
//! raw delivery, what the published body looks like (plain text, a JSON document of its own,
//! a body carrying a newline, an empty one), how many messages are published, and whether
//! the publish selected its body through a `MessageStructure` branch for this protocol.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::topics_subscriptions_publish::percent_encode;

/// The Region every call below is signed for, and the one the destination bucket is created
/// in.
const REGION: &str = "us-east-1";

/// The endpoint every request here reaches the emulator on.
const HOST: &str = "127.0.0.1:9297";

/// The role a `firehose` subscription names: "SubscriptionRoleArn – The ARN of the IAM role
/// that has […] Permission to write to the Firehose delivery stream"
/// (`SetSubscriptionAttributesInput$AttributeName` doc).
const SUBSCRIPTION_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sns-firehose-delivery";

/// The role the delivery stream itself writes to its destination under.
const STREAM_ROLE_ARN: &str = "arn:aws:iam::000000000000:role/firehose-delivery";

/// How long a condition the clock owes is waited for before a case fails: far longer than
/// the emulator's own spans, so a loaded machine does not fail a case the clock would have
/// served.
const DEADLINE: Duration = Duration::from_secs(60);

/// How often a condition is looked at again while waiting for it.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// SNS, Firehose and Amazon S3 on ONE engine: a `firehose` subscription's endpoint is a
/// delivery stream, and what that stream was given is readable out of the bucket its
/// destination names.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        router.register_service(
            service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
            Arc::clone(&engine),
        ));
        Self { router }
    }

    /// One awsQuery form call against SNS, with whatever status it answered.
    async fn sns_call(&self, action: &str, params: &[(&str, &str)]) -> (u16, String) {
        let mut body = format!("Action={action}&Version=2010-03-31");
        for (name, value) in params {
            body.push('&');
            body.push_str(name);
            body.push('=');
            body.push_str(&percent_encode(value));
        }
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("sns.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-www-form-urlencoded")
            .header("authorization", authorization("sns"))
            .body(bytes::Bytes::from(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let text = String::from_utf8_lossy(response.body()).to_string();
        (status, text)
    }

    /// One awsQuery call against SNS that must SUCCEED.
    async fn sns(&self, case: &str, action: &str, params: &[(&str, &str)]) -> String {
        let (status, text) = self.sns_call(action, params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {action} expected HTTP 200, got {status} / {text}"
        );
        text
    }

    /// One `awsJson1_1` call against Firehose that must SUCCEED.
    async fn firehose(&self, case: &str, operation: &str, body: Value) -> Value {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", HOST)
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("Firehose_20150804.{operation}"))
            .header("authorization", authorization("firehose"))
            .body(bytes::Bytes::from(
                serde_json::to_vec(&body).expect("the request body serializes"),
            ))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} expected HTTP 200, got {status} / {parsed}"
        );
        parsed
    }

    /// One path-style Amazon S3 request, with `body` as the request's own bytes.
    async fn s3(&self, method: &str, path: &str, body: &[u8]) -> (u16, bytes::Bytes) {
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{HOST}{path}"))
            .header("host", HOST)
            .header("authorization", authorization("s3"))
            .body(bytes::Bytes::copy_from_slice(body))
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        (status, response.into_body())
    }

    /// `PUT /<bucket>` — S3's `CreateBucket`.
    ///
    /// The create names NO `LocationConstraint`: the unspecified constraint stands for the
    /// Region this suite signs for, and S3 refuses a create that names it explicitly.
    async fn create_bucket(&self, case: &str, bucket: &str) {
        let (status, body) = self.s3("PUT", &format!("/{bucket}"), b"").await;
        assert_eq!(
            status,
            200,
            "case {case:?}: creating the bucket {bucket} expected 200, got {status} / {}",
            String::from_utf8_lossy(&body)
        );
    }

    /// Every key `bucket` holds, read through `ListObjectsV2`.
    async fn keys(&self, case: &str, bucket: &str) -> Vec<String> {
        let (status, body) = self
            .s3("GET", &format!("/{bucket}?list-type=2"), b"")
            .await;
        assert_eq!(
            status,
            200,
            "case {case:?}: listing {bucket} expected 200, got {status} / {}",
            String::from_utf8_lossy(&body)
        );
        let listing = String::from_utf8_lossy(&body).into_owned();
        let mut keys = Vec::new();
        let mut rest = listing.as_str();
        while let Some(at) = rest.find("<Key>") {
            rest = &rest[at + "<Key>".len()..];
            let Some(end) = rest.find("</Key>") else {
                break;
            };
            keys.push(rest[..end].to_string());
            rest = &rest[end..];
        }
        keys.sort();
        keys
    }

    /// The bytes `bucket` holds at `key`, read through `GetObject`.
    async fn object(&self, case: &str, bucket: &str, key: &str) -> Vec<u8> {
        let (status, body) = self.s3("GET", &format!("/{bucket}/{key}"), b"").await;
        assert_eq!(
            status,
            200,
            "case {case:?}: reading {bucket}/{key} expected 200, got {status} / {}",
            String::from_utf8_lossy(&body)
        );
        body.to_vec()
    }

    /// A DirectPut delivery stream called `stream` whose destination is `bucket`, waited for
    /// until it reports itself `ACTIVE` — a put is accepted only then. Answers its ARN, which
    /// is what a `firehose` subscription names as its endpoint.
    async fn given_stream(&self, case: &str, stream: &str, bucket: &str) -> String {
        let created = self
            .firehose(
                case,
                "CreateDeliveryStream",
                json!({
                    "DeliveryStreamName": stream,
                    "DeliveryStreamType": "DirectPut",
                    "ExtendedS3DestinationConfiguration": {
                        "RoleARN": STREAM_ROLE_ARN,
                        "BucketARN": format!("arn:aws:s3:::{bucket}"),
                        // The shortest buffer `IntervalInSeconds` admits (its published
                        // minimum is 0), because what this suite asserts is WHAT reaches the
                        // destination rather than how long a buffer is held: the Firehose
                        // suite's own cases assert the interval.
                        "BufferingHints": { "SizeInMBs": 1, "IntervalInSeconds": 0 },
                    },
                }),
            )
            .await;
        let arn = created["DeliveryStreamARN"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no DeliveryStreamARN in {created}"))
            .to_string();
        let deadline = Instant::now() + DEADLINE;
        loop {
            let described = self
                .firehose(
                    case,
                    "DescribeDeliveryStream",
                    json!({ "DeliveryStreamName": stream }),
                )
                .await;
            let status = described["DeliveryStreamDescription"]["DeliveryStreamStatus"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            if status == "ACTIVE" {
                return arn;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the delivery stream {stream} never became ACTIVE, last \
                 reported {status:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }

    /// Every record byte the destination `bucket` holds, once `settled` says every record
    /// the case expects is there — the objects' bodies in key order, concatenated, which is
    /// how a reader of the destination sees the records the stream was given.
    ///
    /// The stop condition is the CASE's, because what counts as "all of them" differs by
    /// record form: a byte total for records that carry no delimiter, and a count of the
    /// terminators for records that do. Waiting for a condition rather than for a span is
    /// what keeps the wait independent of how fast the host runs.
    async fn delivered(
        &self,
        case: &str,
        bucket: &str,
        settled: &dyn Fn(&[u8]) -> bool,
    ) -> Vec<u8> {
        let deadline = Instant::now() + DEADLINE;
        loop {
            let mut delivered = Vec::new();
            for key in self.keys(case, bucket).await {
                delivered.extend(self.object(case, bucket, &key).await);
            }
            if settled(&delivered) {
                return delivered;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the records this publish owed never all reached {bucket}, got \
                 {:?}",
                String::from_utf8_lossy(&delivered)
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// A SigV4 `Authorization` header whose credential scope names `service` — the routing key
/// the router claims a request by. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, so nothing here is verified.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// The text of the first `<tag>…</tag>` element of an awsQuery response.
fn element(case: &str, body: &str, tag: &str) -> String {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = body
        .find(&open)
        .unwrap_or_else(|| panic!("case {case:?}: no <{tag}> in {body}"))
        + open.len();
    let end = body[start..]
        .find(&close)
        .unwrap_or_else(|| panic!("case {case:?}: no </{tag}> in {body}"));
    body[start..start + end].to_string()
}

/// One delivery row: the case, whether the subscription asked for raw delivery, the
/// `MessageStructure` each publish carries, the bodies published, and the record TEXT each
/// one is expected to reach the destination as.
struct Row {
    case: &'static str,
    raw: bool,
    structure: Option<&'static str>,
    published: &'static [&'static str],
    records: &'static [&'static str],
}

/// The rows every case below is driven from.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a raw subscription takes the body itself",
            raw: true,
            structure: None,
            published: &["an archival event"],
            records: &["an archival event"],
        },
        Row {
            case: "a raw subscription takes a JSON body unchanged",
            raw: true,
            structure: None,
            published: &[r#"{"event":"order.created","id":17}"#],
            records: &[r#"{"event":"order.created","id":17}"#],
        },
        Row {
            case: "a raw subscription adds no delimiter between two bodies",
            raw: true,
            structure: None,
            published: &[r#"{"n":1}"#, r#"{"n":2}"#],
            records: &[r#"{"n":1}"#, r#"{"n":2}"#],
        },
        Row {
            case: "a raw subscription keeps a body that carries a newline",
            raw: true,
            structure: None,
            published: &["first line\nsecond line"],
            records: &["first line\nsecond line"],
        },
        Row {
            case: "a raw subscription selects the branch this protocol is named in",
            raw: true,
            structure: Some("json"),
            published: &[r#"{"default":"for everyone","firehose":"for the stream"}"#],
            records: &["for the stream"],
        },
        Row {
            case: "a raw subscription falls back to the default branch",
            raw: true,
            structure: Some("json"),
            published: &[r#"{"default":"for everyone"}"#],
            records: &["for everyone"],
        },
        Row {
            case: "a subscription that asked for no raw delivery takes the envelope",
            raw: false,
            structure: None,
            published: &["an archival event"],
            records: &["an archival event"],
        },
        Row {
            case: "an enveloped subscription takes one envelope per message",
            raw: false,
            structure: None,
            published: &["first", "second"],
            records: &["first", "second"],
        },
    ]
}

/// Every published message reaches the subscribed delivery stream's destination as one
/// record: the body itself for a raw subscription, and the newline-terminated JSON
/// notification for every other.
#[tokio::test]
async fn a_publish_puts_one_record_into_each_subscribed_delivery_stream() {
    for (index, row) in rows().iter().enumerate() {
        let emulator = Emulator::new();
        let case = row.case;
        let bucket = format!("sns-firehose-{index:02}");
        let stream = format!("sns-firehose-stream-{index:02}");
        emulator.create_bucket(case, &bucket).await;
        let stream_arn = emulator.given_stream(case, &stream, &bucket).await;

        let created = emulator
            .sns(case, "CreateTopic", &[("Name", &format!("fanout-{index:02}"))])
            .await;
        let topic = element(case, &created, "TopicArn");
        let mut attributes: Vec<(String, String)> = vec![
            ("Attributes.entry.1.key".to_string(), "SubscriptionRoleArn".to_string()),
            ("Attributes.entry.1.value".to_string(), SUBSCRIPTION_ROLE_ARN.to_string()),
        ];
        if row.raw {
            attributes.push(("Attributes.entry.2.key".to_string(), "RawMessageDelivery".to_string()));
            attributes.push(("Attributes.entry.2.value".to_string(), "true".to_string()));
        }
        let mut params: Vec<(&str, &str)> = vec![
            ("TopicArn", &topic),
            ("Protocol", "firehose"),
            ("Endpoint", &stream_arn),
            ("ReturnSubscriptionArn", "true"),
        ];
        params.extend(
            attributes
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        );
        let subscribed = emulator.sns(case, "Subscribe", &params).await;
        let subscription = element(case, &subscribed, "SubscriptionArn");
        // The attribute reads back in its canonical form, so what follows is the delivery's
        // doing rather than an attribute that never landed.
        let held = emulator
            .sns(
                case,
                "GetSubscriptionAttributes",
                &[("SubscriptionArn", &subscription)],
            )
            .await;
        let expected_flag = if row.raw { "true" } else { "false" };
        assert!(
            held.contains(&format!("<value>{expected_flag}</value>")),
            "case {case:?}: RawMessageDelivery must read back as {expected_flag:?}, got {held}"
        );

        let mut message_ids = Vec::new();
        for body in row.published {
            let mut params: Vec<(&str, &str)> = vec![("TopicArn", &topic), ("Message", body)];
            if let Some(structure) = row.structure {
                params.push(("MessageStructure", structure));
            }
            let published = emulator.sns(case, "Publish", &params).await;
            message_ids.push(element(case, &published, "MessageId"));
        }

        // A raw record carries no terminator, so what says every record is there is the
        // byte total the bodies come to; an enveloped record is newline-terminated, so what
        // says it is the number of terminators.
        let bytes: usize = row.records.iter().map(|record| record.len()).sum();
        let count = row.records.len();
        let raw = row.raw;
        let settled = move |delivered: &[u8]| {
            if raw {
                delivered.len() >= bytes
            } else {
                delivered.iter().filter(|byte| **byte == b'\n').count() >= count
            }
        };
        let delivered = emulator.delivered(case, &bucket, &settled).await;
        let delivered = String::from_utf8(delivered).unwrap_or_else(|error| {
            panic!("case {case:?}: the delivered records must be UTF-8 text: {error}")
        });

        if row.raw {
            // Back to back, with nothing added: the destination holds the bodies' bytes and
            // no more, so there is room for neither an envelope nor a delimiter. Which
            // buffer each record was flushed in is Firehose's business, so the bodies are
            // asserted as the set they are rather than in one concatenation order.
            assert_eq!(
                delivered.len(),
                bytes,
                "case {case:?}: the destination must hold the bare bodies and nothing else, \
                 got {delivered:?}"
            );
            for record in row.records {
                assert!(
                    delivered.contains(record),
                    "case {case:?}: the destination must hold the body {record:?}, got \
                     {delivered:?}"
                );
            }
            continue;
        }

        // Every enveloped record is newline-terminated, so the records can be read apart.
        assert!(
            delivered.ends_with('\n'),
            "case {case:?}: an enveloped record must be newline-terminated, got {delivered:?}"
        );
        let records: Vec<&str> = delivered.trim_end_matches('\n').split('\n').collect();
        assert_eq!(
            records.len(),
            row.records.len(),
            "case {case:?}: expected {} record(s), got {delivered:?}",
            row.records.len()
        );
        let mut bodies: Vec<String> = Vec::new();
        for record in records {
            let envelope: Value = serde_json::from_str(record).unwrap_or_else(|error| {
                panic!("case {case:?}: an enveloped record must be JSON ({error}): {record:?}")
            });
            assert_eq!(
                envelope["Type"].as_str(),
                Some("Notification"),
                "case {case:?}: the envelope must name its type, got {envelope}"
            );
            assert_eq!(
                envelope["TopicArn"].as_str(),
                Some(topic.as_str()),
                "case {case:?}: the envelope must name the topic published to, got {envelope}"
            );
            let id = envelope["MessageId"].as_str().unwrap_or_default().to_string();
            assert!(
                message_ids.contains(&id),
                "case {case:?}: the envelope must carry the id the publish answered, got \
                 {envelope}"
            );
            bodies.push(
                envelope["Message"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            );
        }
        bodies.sort();
        let mut expected: Vec<String> = row.records.iter().map(|body| body.to_string()).collect();
        expected.sort();
        assert_eq!(
            bodies, expected,
            "case {case:?}: expected the published bodies in the envelopes, got {delivered:?}"
        );
    }
}
