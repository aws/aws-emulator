// SPDX-License-Identifier: CC0-1.0

//! What CloudTrail's `AWS API Call via CloudTrail` fan-out carries, and what it does not.
//!
//! RULE: while a trail of the Region is logging management events, CloudTrail records every
//! management call the account serves in event history — a call that only REPORTS state
//! with `readOnly` true, one that CHANGES the account with `readOnly` false — but it
//! publishes onto the account's default event bus only the calls that CHANGED it. So a rule
//! over a service's API-call events is driven by the writes made on its resources and never
//! by the `Get` / `List` / `Describe` reads a client or a console made while watching them.
//!
//! Only a cross-service suite can pin this: the calls are served by the services that own
//! them (CloudFront over restXml, Amazon SQS over awsJson1_0), the delivery decision belongs
//! to CloudTrail (awsJson1_1), the match and the hand-over belong to EventBridge
//! (awsJson1_1), what arrived is read from Amazon SQS and the records are read back through
//! `cloudtrail:LookupEvents` — five services over ONE engine, as `core/router/src/lib.rs`
//! mounts them.
//!
//! The rows span what the rule generalises over rather than the one call that motivated it:
//! which service served the call (one GLOBAL service with an XML protocol, one REGIONAL
//! service with a JSON one), the FORM of the read (a `List` over a collection, a `Get` of one
//! resource by id, a `Get` of one resource by name, a `List` of one resource's tags) and the
//! read/write kind itself — the writes are the control that proves the channel was live, and
//! the last call made is a write, so its arrival is the barrier after which a suppressed
//! read would already have been delivered had it been published at all.
//!
//! CloudFront is a GLOBAL service, so its records are filed in the partition's leader Region;
//! every request below is signed for that Region so that one bus and one trail cover them
//! all.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for: the Region a GLOBAL service's management
/// events are filed in, which is where a rule matching them lives.
const REGION: &str = "us-east-1";

/// The account every emulated resource belongs to.
const ACCOUNT: &str = "000000000000";

/// CloudFront's API version, which every modelled route of it is under.
const API_VERSION: &str = "2020-05-31";

/// The `detail-type` CloudTrail's management-event fan-out carries
/// (`service_common::api_call_events::API_CALL_DETAIL_TYPE`).
const API_CALL_DETAIL_TYPE: &str = "AWS API Call via CloudTrail";

/// How long the arrival of the control writes is waited for. Generous: the suite waits for
/// the CONDITION (every write's envelope on the sink), and this is only the point at which
/// waiting longer cannot help.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(60);

/// How long between two looks at the sink while waiting for one.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// Whether a call of a row only REPORTS the account's state or CHANGES it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// A `Get` / `List` / `Describe` read: recorded, never published.
    Reporting,
    /// A call that changes the account: recorded AND published.
    Mutating,
}

/// One call this suite makes, and what the fan-out must do with it.
struct Row {
    case: &'static str,
    /// The operation's modelled name, which is the record's `eventName`.
    event_name: &'static str,
    kind: Kind,
}

/// The services this fan-out runs through, over ONE shared engine.
///
/// Amazon S3 is mounted beside them because a trail's destination bucket is resolved in the
/// account's object store: the trail is created the way a client creates one, against a
/// bucket that really exists.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is not the subject here, so requests are unauthenticated; the credential
    // scope is still carried, because it is what names the Region.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        engine,
    ));
    router
}

/// A SigV4 `Authorization` whose credential scope names `service`, which is what routes the
/// request. The signature is never checked here.
fn authorization(service: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// One `X-Amz-Target` RPC call, as its status and parsed body.
async fn rpc(
    router: &MultiServiceRouter,
    signing_name: &str,
    target_prefix: &str,
    json_version: &str,
    operation: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name))
        .header(
            "content-type",
            format!("application/x-amz-json-{json_version}"),
        )
        .header("x-amz-target", format!("{target_prefix}.{operation}"))
        .body(Bytes::from(body.to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// An EventBridge call (`awsJson1_1`, `AWSEvents.<Operation>`) that must answer 200.
async fn events(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "events", "AWSEvents", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from EventBridge {operation}, got {status} with {parsed}"
    );
    parsed
}

/// An Amazon SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
async fn sqs(router: &MultiServiceRouter, case: &str, operation: &str, body: &Value) -> Value {
    let (status, parsed) = rpc(router, "sqs", "AmazonSQS", "1.0", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from SQS {operation}, got {status} with {parsed}"
    );
    parsed
}

/// A CloudTrail call (`awsJson1_1`, `CloudTrail_20131101.<Operation>`) that must answer 200.
async fn cloudtrail(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let (status, parsed) =
        rpc(router, "cloudtrail", "CloudTrail_20131101", "1.1", operation, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CloudTrail {operation}, got {status} with {parsed}"
    );
    parsed
}

/// One CloudFront restXml call, as its status and raw body.
async fn cloudfront(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> (u16, String) {
    let mut request = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", "cloudfront.amazonaws.com")
        .header("authorization", authorization("cloudfront"));
    if body.is_some() {
        request = request.header("content-type", "application/xml");
    }
    let request = request
        .body(Bytes::from(body.unwrap_or_default().to_string()))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    (
        response.status().as_u16(),
        String::from_utf8_lossy(response.body()).to_string(),
    )
}

/// The text of the first `<name>` element of an XML body, or `""`.
fn xml_element(body: &str, name: &str) -> String {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let Some(from) = body.find(&open).map(|at| at + open.len()) else {
        return String::new();
    };
    let Some(len) = body[from..].find(&close) else {
        return String::new();
    };
    body[from..from + len].to_string()
}

/// `PUT /<bucket>` plus `PUT /<bucket>?policy` — the bucket the trail delivers its log files
/// into, carrying the policy that lets CloudTrail write into it.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) {
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("content-type", "application/xml")
        .header("authorization", authorization("s3"))
        .body(Bytes::new())
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "setup: CreateBucket({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );

    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AWSCloudTrailWrite",
            "Effect": "Allow",
            "Principal": { "Service": "cloudtrail.amazonaws.com" },
            "Action": "s3:PutObject",
            "Resource": format!("arn:aws:s3:::{bucket}/*"),
            "Condition": {
                "StringEquals": { "s3:x-amz-acl": "bucket-owner-full-control" },
            },
        }],
    })
    .to_string();
    let request = http::Request::builder()
        .method("PUT")
        .uri(format!("/{bucket}?policy"))
        .header("host", format!("s3.{REGION}.amazonaws.com"))
        .header("content-type", "application/json")
        .header("authorization", authorization("s3"))
        .body(Bytes::from(policy))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    assert!(
        (200..300).contains(&response.status().as_u16()),
        "setup: PutBucketPolicy({bucket:?}) must succeed, got {} with {:?}",
        response.status(),
        String::from_utf8_lossy(response.body())
    );
}

/// Create a queue admitting `events.amazonaws.com`, and hand back its url and ARN.
async fn create_queue(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let created = sqs(router, name, "CreateQueue", &json!({ "QueueName": name })).await;
    let url = created["QueueUrl"]
        .as_str()
        .unwrap_or_else(|| panic!("setup: expected a QueueUrl for {name:?}, got {created}"))
        .to_string();
    let arn = format!("arn:aws:sqs:{REGION}:{ACCOUNT}:{name}");
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": "events.amazonaws.com" },
            "Action": "sqs:SendMessage",
            "Resource": arn,
        }],
    });
    sqs(
        router,
        name,
        "SetQueueAttributes",
        &json!({ "QueueUrl": url, "Attributes": { "Policy": policy.to_string() } }),
    )
    .await;
    (url, arn)
}

/// Receive up to ten messages from `url`, draining them, as their parsed bodies.
async fn drained(router: &MultiServiceRouter, url: &str) -> Vec<Value> {
    let answer = sqs(
        router,
        "the sink",
        "ReceiveMessage",
        &json!({ "QueueUrl": url, "MaxNumberOfMessages": 10 }),
    )
    .await;
    answer["Messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let body = message["Body"].as_str().unwrap_or_default();
                    serde_json::from_str(body)
                        .unwrap_or_else(|error| panic!("a delivered envelope is JSON ({error}): {body}"))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A `DistributionConfig` for `reference` — the smallest one CloudFront accepts.
fn distribution_config(reference: &str) -> String {
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Comment>read-only api calls</Comment>\
         <Enabled>true</Enabled>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>primary</Id><DomainName>origin.example.com</DomainName>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior>\
         <TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         </DefaultCacheBehavior>\
         </DistributionConfig>"
    )
}

/// `cloudtrail:LookupEvents` filtered on one event name, answering each matching event's
/// parsed `CloudTrailEvent` document.
async fn looked_up(router: &MultiServiceRouter, event_name: &str) -> Vec<Value> {
    let answered = cloudtrail(
        router,
        event_name,
        "LookupEvents",
        &json!({
            "LookupAttributes": [{
                "AttributeKey": "EventName",
                "AttributeValue": event_name,
            }],
        }),
    )
    .await;
    answered["Events"]
        .as_array()
        .unwrap_or_else(|| panic!("LookupEvents reports an Events list, got {answered}"))
        .iter()
        .map(|event| {
            let document = event["CloudTrailEvent"]
                .as_str()
                .unwrap_or_else(|| panic!("an event carries its CloudTrailEvent: {event}"));
            serde_json::from_str(document)
                .unwrap_or_else(|error| panic!("CloudTrailEvent is JSON ({error}): {document}"))
        })
        .collect()
}

/// The `detail.eventName` of each envelope that arrived.
fn arrived_names(arrived: &[Value]) -> Vec<String> {
    arrived
        .iter()
        .map(|envelope| {
            envelope
                .pointer("/detail/eventName")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn only_the_calls_that_changed_the_account_reach_the_api_call_channel() {
    let router = router();

    // A trail of the account, logging — the condition any delivery hangs off. Created
    // BEFORE the sink's rule, so none of this setup's own writes can reach the sink.
    create_bucket(&router, "read-only-api-call-trail").await;
    cloudtrail(
        &router,
        "setup",
        "CreateTrail",
        &json!({
            "Name": "read-only-api-calls",
            "S3BucketName": "read-only-api-call-trail",
        }),
    )
    .await;
    cloudtrail(
        &router,
        "setup",
        "StartLogging",
        &json!({ "Name": "read-only-api-calls" }),
    )
    .await;

    // The queue the rows' calls are made against, and the queue the fan-out is collected
    // on. Both exist before the rule, so creating them publishes nothing to the sink.
    let (subject_url, _) = create_queue(&router, "read-only-api-call-subject").await;
    let (sink_url, sink_arn) = create_queue(&router, "read-only-api-call-sink").await;

    // One rule over the whole channel: every API-call event of the account, whatever
    // service served it. What arrives is therefore exactly what was published.
    events(
        &router,
        "setup",
        "PutRule",
        &json!({
            "Name": "read-only-api-call-rule",
            "EventPattern": json!({ "detail-type": [API_CALL_DETAIL_TYPE] }).to_string(),
        }),
    )
    .await;
    let put = events(
        &router,
        "setup",
        "PutTargets",
        &json!({
            "Rule": "read-only-api-call-rule",
            "Targets": [{ "Id": "sink", "Arn": sink_arn }],
        }),
    )
    .await;
    assert_eq!(
        put["FailedEntryCount"], json!(0),
        "setup: the sink target must be installed, got {put}"
    );

    // The rows, in the order the calls are made: a write of each service, every form of
    // read in between, and a write LAST — the barrier whose arrival proves the channel ran
    // past every read before the absences below are read.
    let rows = [
        Row {
            case: "a global service's create",
            event_name: "CreateDistribution",
            kind: Kind::Mutating,
        },
        Row {
            case: "a global service's list of a collection",
            event_name: "ListDistributions",
            kind: Kind::Reporting,
        },
        Row {
            case: "a global service's get of one resource by id",
            event_name: "GetDistribution",
            kind: Kind::Reporting,
        },
        Row {
            case: "a regional service's get of one resource by name",
            event_name: "GetQueueUrl",
            kind: Kind::Reporting,
        },
        Row {
            case: "a regional service's list of one resource's tags",
            event_name: "ListQueueTags",
            kind: Kind::Reporting,
        },
        Row {
            case: "a regional service's list of a collection",
            event_name: "ListQueues",
            kind: Kind::Reporting,
        },
        Row {
            case: "a regional service's tagging write",
            event_name: "TagQueue",
            kind: Kind::Mutating,
        },
    ];

    let (status, created) = cloudfront(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&distribution_config("ref-read-only-api-calls")),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {:?}: CreateDistribution must succeed, got {status} with {created:?}",
        rows[0].case
    );
    let distribution_id = xml_element(&created, "Id");
    assert!(
        !distribution_id.is_empty(),
        "case {:?}: the created distribution must name its id, got {created:?}",
        rows[0].case
    );

    let (status, listed) = cloudfront(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: ListDistributions must succeed, got {status} with {listed:?}",
        rows[1].case
    );
    let (status, got) = cloudfront(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution/{distribution_id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {:?}: GetDistribution must succeed, got {status} with {got:?}",
        rows[2].case
    );
    sqs(
        &router,
        rows[3].case,
        "GetQueueUrl",
        &json!({ "QueueName": "read-only-api-call-subject" }),
    )
    .await;
    sqs(
        &router,
        rows[4].case,
        "ListQueueTags",
        &json!({ "QueueUrl": subject_url }),
    )
    .await;
    sqs(&router, rows[5].case, "ListQueues", &json!({})).await;
    sqs(
        &router,
        rows[6].case,
        "TagQueue",
        &json!({ "QueueUrl": subject_url, "Tags": { "owner": "audit" } }),
    )
    .await;

    // Wait for the CONDITION — every write's envelope on the sink — rather than for any
    // span of time. The reads were all made before the last write, so once that write has
    // arrived, a published read would have arrived too.
    let writes: BTreeSet<&str> = rows
        .iter()
        .filter(|row| row.kind == Kind::Mutating)
        .map(|row| row.event_name)
        .collect();
    let mut arrived: Vec<Value> = Vec::new();
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        arrived.extend(drained(&router, &sink_url).await);
        let seen: BTreeSet<String> = arrived_names(&arrived).into_iter().collect();
        if writes.iter().all(|name| seen.contains(*name)) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "expected every write to reach the channel ({writes:?}), got {:?}",
            arrived_names(&arrived)
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // What arrived is the writes and NOTHING else: every read is absent, and every
    // delivered record says it changed the account.
    let delivered = arrived_names(&arrived);
    for row in &rows {
        let present = delivered.iter().any(|name| name == row.event_name);
        let expected = row.kind == Kind::Mutating;
        assert_eq!(
            present, expected,
            "case {:?}: expected {} on the API-call channel for {}, got {delivered:?}",
            row.case,
            if expected { "the event" } else { "no event" },
            row.event_name
        );
    }
    for envelope in &arrived {
        assert_eq!(
            envelope.pointer("/detail/readOnly"),
            Some(&json!(false)),
            "case a delivered envelope: only a call that changed the account is published, \
             got {envelope}"
        );
        assert_eq!(
            envelope["detail-type"],
            json!(API_CALL_DETAIL_TYPE),
            "case a delivered envelope: expected detail-type {API_CALL_DETAIL_TYPE:?}, got \
             {envelope}"
        );
    }

    // The suppression is the CHANNEL's, not the recording's: every row — read and write
    // alike — is in event history under its own name, stamped with its read/write kind, so
    // an audit reader loses nothing the bus does not carry.
    for row in &rows {
        let records = looked_up(&router, row.event_name).await;
        let read_only = row.kind == Kind::Reporting;
        assert_eq!(
            records.len(),
            1,
            "case {:?}: expected exactly one record of {} in event history, got {records:?}",
            row.case,
            row.event_name
        );
        assert_eq!(
            records[0].get("readOnly"),
            Some(&json!(read_only)),
            "case {:?}: expected the record of {} to carry readOnly {read_only}, got {}",
            row.case,
            row.event_name,
            records[0]
        );
    }
}
