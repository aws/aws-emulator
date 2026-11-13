// SPDX-License-Identifier: CC0-1.0

//! A CloudFront control-plane call reaching an EventBridge rule, across the four services
//! the hop runs through: CloudFront serves the call, CloudTrail's trail is what makes the
//! account deliver it, EventBridge matches the rule and SQS holds what arrived.
//!
//! RULE: while a trail of the Region is logging management events, each management API call
//! the account serves that CHANGED it is ALSO published onto the account's DEFAULT event bus,
//! with `source` `aws.<service>`, `detail-type` `AWS API Call via CloudTrail` and the
//! CloudTrail record of the call as the event's `detail` — so a rule matching that pair, or
//! reaching into the record, delivers to its targets. No trail logging, no publication.
//!
//! Only a cross-service suite can pin this: the event is published by the service that
//! SERVED the call (CloudFront, restXml), the delivery decision belongs to CloudTrail
//! (awsJson1_1), the match and the hand-over belong to EventBridge (awsJson1_1) and what
//! arrived is read from SQS (awsJson1_0) — four services over ONE engine, as
//! `core/router/src/lib.rs` mounts them. The per-service half of the rule, over every kind
//! of call CloudFront serves, is that crate's own `management_event_recording` suite.
//!
//! The rows span what the rule generalises over: which envelope field a pattern matches on
//! (the source, the detail-type, a member reached inside the record), a near miss on each of
//! those three, and the operation's read/write kind — a reporting call is logged like any
//! other management call and published nowhere, so a pattern naming one delivers nothing.
//! One further row is the condition itself: with the trail stopped, the same call publishes
//! nothing.
//!
//! CloudFront is a GLOBAL service, so its records are filed in the partition's leader
//! Region and a rule matching its API-call events has to live there; every request below is
//! signed for that Region for exactly that reason.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for: the Region CloudFront's management events
/// are recorded in, which is where a rule matching them lives.
const REGION: &str = "us-east-1";

/// The account every emulated resource belongs to.
const ACCOUNT: &str = "000000000000";

/// CloudFront's API version, which every modelled route of it is under.
const API_VERSION: &str = "2020-05-31";

/// The `source` an event published for a CloudFront call carries.
const CLOUDFRONT_EVENT_SOURCE: &str = "aws.cloudfront";

/// The `detail-type` CloudTrail's management-event fan-out carries.
const API_CALL_DETAIL_TYPE: &str = "AWS API Call via CloudTrail";

/// The four services this hop runs through, over ONE shared engine.
///
/// Amazon S3 is mounted beside them because a trail's destination bucket is resolved in the
/// account's object store: a trail is created the way a client creates one, against a bucket
/// that really exists.
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

/// An SQS call (`awsJson1_0`, `AmazonSQS.<Operation>`) that must answer 200.
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

/// What one CloudFront call answered: its status, the id the caller was answered under and
/// the raw body.
struct CloudFrontAnswer {
    status: u16,
    request_id: String,
    body: String,
}

/// One CloudFront restXml call.
async fn cloudfront(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> CloudFrontAnswer {
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
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get("x-amzn-requestid")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    CloudFrontAnswer {
        status,
        request_id,
        body: String::from_utf8_lossy(response.body()).to_string(),
    }
}

/// `PUT /<bucket>` plus `PUT /<bucket>?policy` — the bucket a trail delivers its log
/// files into, carrying the bucket policy that lets CloudTrail write into it.
///
/// CloudTrail resolves a trail's destination in the object store and reads that bucket's
/// policy before it accepts the trail, so the policy is attached here exactly as a client
/// attaches it: the CloudTrail service principal, the object write, and the canned ACL the
/// delivery carries.
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
async fn drained(router: &MultiServiceRouter, case: &str, url: &str) -> Vec<Value> {
    let answer = sqs(
        router,
        case,
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
                    serde_json::from_str(body).unwrap_or_else(|error| {
                        panic!("case {case:?}: a delivered envelope is JSON ({error}): {body}")
                    })
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
         <Comment>api call events</Comment>\
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

/// One rule this suite installs: the pattern it matches with, the queue it targets, and
/// whether the CloudFront calls below must reach it.
struct Row {
    case: &'static str,
    /// The rule's `EventPattern`.
    pattern: Value,
    /// Which operation's record the row expects to arrive, or `None` for a near miss that
    /// must stay empty.
    expects: Option<&'static str>,
}

/// The rules, each on its own queue so what arrived at one is never read as another's.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the source and the detail-type",
            pattern: json!({
                "source": [CLOUDFRONT_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["CreateDistribution"] },
            }),
            expects: Some("CreateDistribution"),
        },
        Row {
            case: "a member reached inside the record",
            pattern: json!({
                "detail": {
                    "eventSource": ["cloudfront.amazonaws.com"],
                    "eventName": ["CreateDistribution"],
                },
            }),
            expects: Some("CreateDistribution"),
        },
        Row {
            // A reporting call is recorded in event history and published nowhere, so a
            // pattern naming one delivers nothing even though the call was served and
            // logged: the fan-out carries the account's WRITES. The suite that pins the
            // record of that same read is this crate's `cloudtrail_read_only_api_calls`.
            case: "a reporting call",
            pattern: json!({
                "source": [CLOUDFRONT_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["ListDistributions"] },
            }),
            expects: None,
        },
        Row {
            case: "a near miss on the event name",
            pattern: json!({
                "source": [CLOUDFRONT_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
                "detail": { "eventName": ["DeleteDistribution"] },
            }),
            expects: None,
        },
        Row {
            case: "a near miss on the source",
            pattern: json!({
                "source": ["aws.cloudwatch"],
                "detail-type": [API_CALL_DETAIL_TYPE],
            }),
            expects: None,
        },
        Row {
            case: "a near miss on the detail-type",
            pattern: json!({
                "source": [CLOUDFRONT_EVENT_SOURCE],
                "detail-type": ["CloudFront Distribution Event"],
            }),
            expects: None,
        },
    ]
}

/// The one envelope that arrived at a row's queue, asserted to be a published API-call
/// event of CloudFront.
fn sole_envelope(case: &str, arrived: &[Value]) -> Value {
    assert_eq!(
        arrived.len(),
        1,
        "case {case:?}: expected exactly one delivered envelope, got {arrived:?}"
    );
    arrived[0].clone()
}

#[tokio::test]
async fn a_cloudfront_call_reaches_a_rule_on_the_api_call_channel() {
    let router = router();

    // A trail of the account, logging.
    create_bucket(&router, "cloudfront-api-call-trail").await;
    cloudtrail(
        &router,
        "setup",
        "CreateTrail",
        &json!({
            "Name": "cloudfront-api-calls",
            "S3BucketName": "cloudfront-api-call-trail",
        }),
    )
    .await;
    cloudtrail(
        &router,
        "setup",
        "StartLogging",
        &json!({ "Name": "cloudfront-api-calls" }),
    )
    .await;

    // One rule per row, each on its own queue, all on the DEFAULT bus — the only bus an
    // AWS-published event reaches.
    let rows = rows();
    let mut queues = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let name = format!("cloudfront-api-call-sink-{index}");
        let (url, arn) = create_queue(&router, &name).await;
        let rule = format!("cloudfront-api-call-rule-{index}");
        events(
            &router,
            row.case,
            "PutRule",
            &json!({ "Name": rule, "EventPattern": row.pattern.to_string() }),
        )
        .await;
        let put = events(
            &router,
            row.case,
            "PutTargets",
            &json!({
                "Rule": rule,
                "Targets": [{ "Id": "sink", "Arn": arn }],
            }),
        )
        .await;
        assert_eq!(
            put["FailedEntryCount"], json!(0),
            "case {:?}: the target must be installed, got {put}",
            row.case
        );
        queues.push(url);
    }

    // The two CloudFront calls the rows match on: one that CHANGES the account and one
    // that only REPORTS it.
    let created = cloudfront(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&distribution_config("ref-api-call-events")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateDistribution must succeed, got {} with {:?}",
        created.status, created.body
    );
    let listed = cloudfront(
        &router,
        "GET",
        &format!("/{API_VERSION}/distribution"),
        None,
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "ListDistributions must succeed, got {} with {:?}",
        listed.status, listed.body
    );

    for (row, url) in rows.iter().zip(&queues) {
        let arrived = drained(&router, row.case, url).await;
        let Some(event_name) = row.expects else {
            assert!(
                arrived.is_empty(),
                "case {:?}: a pattern this event does not match delivers nothing, got \
                 {arrived:?}",
                row.case
            );
            continue;
        };
        let envelope = sole_envelope(row.case, &arrived);
        assert_eq!(
            envelope["source"],
            json!(CLOUDFRONT_EVENT_SOURCE),
            "case {:?}: expected source {CLOUDFRONT_EVENT_SOURCE:?}, got {envelope}",
            row.case
        );
        assert_eq!(
            envelope["detail-type"],
            json!(API_CALL_DETAIL_TYPE),
            "case {:?}: expected detail-type {API_CALL_DETAIL_TYPE:?}, got {envelope}",
            row.case
        );
        assert_eq!(
            envelope["region"],
            json!(REGION),
            "case {:?}: expected the event in {REGION:?}, got {envelope}",
            row.case
        );
        assert_eq!(
            envelope["account"],
            json!(ACCOUNT),
            "case {:?}: expected the event in account {ACCOUNT:?}, got {envelope}",
            row.case
        );
        // The `detail` is the CloudTrail record of the call: the operation, the serving
        // service, its read/write kind and the id the CALLER was answered under — the join
        // a client holding a response makes to the event of that very call.
        assert_eq!(
            envelope.pointer("/detail/eventName"),
            Some(&json!(event_name)),
            "case {:?}: expected the record of {event_name}, got {envelope}",
            row.case
        );
        assert_eq!(
            envelope.pointer("/detail/eventSource"),
            Some(&json!("cloudfront.amazonaws.com")),
            "case {:?}: the record names the serving service, got {envelope}",
            row.case
        );
        let answered_as = if event_name == "CreateDistribution" {
            &created.request_id
        } else {
            &listed.request_id
        };
        assert!(
            !answered_as.is_empty(),
            "case {:?}: the CloudFront answer must name the request it answers",
            row.case
        );
        assert_eq!(
            envelope.pointer("/detail/requestID"),
            Some(&json!(answered_as)),
            "case {:?}: the record names the call the caller was answered for, got {envelope}",
            row.case
        );
        let expected_read_only = event_name == "ListDistributions";
        assert_eq!(
            envelope.pointer("/detail/readOnly"),
            Some(&json!(expected_read_only)),
            "case {:?}: expected readOnly {expected_read_only}, got {envelope}",
            row.case
        );
    }
}

#[tokio::test]
async fn no_trail_logging_publishes_nothing() {
    let router = router();
    create_bucket(&router, "cloudfront-stopped-trail").await;

    let (url, arn) = create_queue(&router, "cloudfront-stopped-sink").await;
    events(
        &router,
        "setup",
        "PutRule",
        &json!({
            "Name": "cloudfront-stopped-rule",
            "EventPattern": json!({
                "source": [CLOUDFRONT_EVENT_SOURCE],
                "detail-type": [API_CALL_DETAIL_TYPE],
            })
            .to_string(),
        }),
    )
    .await;
    events(
        &router,
        "setup",
        "PutTargets",
        &json!({
            "Rule": "cloudfront-stopped-rule",
            "Targets": [{ "Id": "sink", "Arn": arn }],
        }),
    )
    .await;

    // Row 1: a trail that was never created. Nothing of the account is logged, so nothing
    // is published.
    let created = cloudfront(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&distribution_config("ref-no-trail")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateDistribution must succeed with no trail at all, got {} with {:?}",
        created.status, created.body
    );
    let arrived = drained(&router, "no trail at all", &url).await;
    assert!(
        arrived.is_empty(),
        "case \"no trail at all\": nothing is published while no trail logs, got {arrived:?}"
    );

    // Row 2: a trail that exists but was never started. `CreateTrail` does not start
    // logging, so the answer is the same.
    cloudtrail(
        &router,
        "a trail that is not logging",
        "CreateTrail",
        &json!({
            "Name": "cloudfront-stopped",
            "S3BucketName": "cloudfront-stopped-trail",
        }),
    )
    .await;
    let created = cloudfront(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&distribution_config("ref-trail-not-logging")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateDistribution must succeed, got {} with {:?}",
        created.status, created.body
    );
    let arrived = drained(&router, "a trail that is not logging", &url).await;
    assert!(
        arrived.is_empty(),
        "case \"a trail that is not logging\": nothing is published, got {arrived:?}"
    );

    // Row 3: the same trail, LOGGING — the one change that makes the next call arrive.
    cloudtrail(
        &router,
        "a logging trail",
        "StartLogging",
        &json!({ "Name": "cloudfront-stopped" }),
    )
    .await;
    let created = cloudfront(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&distribution_config("ref-trail-logging")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateDistribution must succeed, got {} with {:?}",
        created.status, created.body
    );
    let arrived = drained(&router, "a logging trail", &url).await;
    let names: Vec<String> = arrived
        .iter()
        .filter_map(|event| {
            event
                .pointer("/detail/eventName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        names.contains(&"CreateDistribution".to_string()),
        "case \"a logging trail\": the call made while the trail logs must arrive, got \
         {names:?}"
    );

    // Row 4: the trail STOPPED again — the condition is the trail's current state, not
    // whether one ever logged.
    cloudtrail(
        &router,
        "a trail stopped again",
        "StopLogging",
        &json!({ "Name": "cloudfront-stopped" }),
    )
    .await;
    let _ = drained(&router, "a trail stopped again", &url).await;
    let created = cloudfront(
        &router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&distribution_config("ref-trail-stopped")),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "CreateDistribution must succeed, got {} with {:?}",
        created.status, created.body
    );
    let arrived = drained(&router, "a trail stopped again", &url).await;
    let names: Vec<String> = arrived
        .iter()
        .filter_map(|event| {
            event
                .pointer("/detail/eventName")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert!(
        !names.contains(&"CreateDistribution".to_string()),
        "case \"a trail stopped again\": nothing is published once the trail stops, got \
         {names:?}"
    );
}

/// One row of the selector table: the event selectors a logging trail is configured with,
/// and which of the two calls below must arrive under them.
struct Selectors {
    case: &'static str,
    /// The `PutEventSelectors` request body, beside the trail's name.
    configuration: Value,
    /// Whether the WRITE call (`CreateDistribution`) must arrive.
    expects_write: bool,
    /// Whether the READ call (`ListDistributions`) must arrive — `false` in every row: the
    /// channel carries the calls that changed the account, whatever the trail logs.
    expects_read: bool,
}

/// What a logging trail's own selectors admit is what is LOGGED, and of what is logged the
/// channel carries the calls that CHANGED the account: a trail logs the management events
/// its `IncludeManagementEvents` admits, in the read/write class its `ReadWriteType` names
/// — "By default, trails created without specific event selectors will be configured to log
/// all read and write management events" — and the `AWS API Call via CloudTrail` fan-out
/// publishes those of them that are not reads.
///
/// The rows span the selector shapes that decide it: both classes, each class alone,
/// management events excluded altogether, and the ADVANCED form, whose
/// `eventCategory`/`readOnly` fields say the same two things — including one advanced
/// selector that admits a category these calls are not in at all. No selector shape makes a
/// READ reach the channel, a trail whose class is `ReadOnly` included: what its selectors
/// decide is what the trail logs.
#[tokio::test]
async fn a_trails_own_selectors_decide_which_calls_are_published() {
    let rows = [
        Selectors {
            case: "all read and write management events",
            configuration: json!({
                "EventSelectors": [{ "ReadWriteType": "All", "IncludeManagementEvents": true }],
            }),
            expects_write: true,
            expects_read: false,
        },
        Selectors {
            case: "write-only management events",
            configuration: json!({
                "EventSelectors": [{
                    "ReadWriteType": "WriteOnly",
                    "IncludeManagementEvents": true,
                }],
            }),
            expects_write: true,
            expects_read: false,
        },
        Selectors {
            case: "read-only management events",
            configuration: json!({
                "EventSelectors": [{
                    "ReadWriteType": "ReadOnly",
                    "IncludeManagementEvents": true,
                }],
            }),
            expects_write: false,
            expects_read: false,
        },
        Selectors {
            case: "management events excluded",
            configuration: json!({
                "EventSelectors": [{
                    "ReadWriteType": "All",
                    "IncludeManagementEvents": false,
                    // A selector that logs no management events has to log SOMETHING:
                    // "When includeManagementEvents is false, DataResources must not be
                    // empty", which is the refusal `PutEventSelectors` answers otherwise.
                    "DataResources": [{
                        "Type": "AWS::S3::Object",
                        "Values": ["arn:aws:s3:::cloudfront-selector-trail/"],
                    }],
                }],
            }),
            expects_write: false,
            expects_read: false,
        },
        Selectors {
            case: "an advanced selector on the management category",
            configuration: json!({
                "AdvancedEventSelectors": [{
                    "Name": "management",
                    "FieldSelectors": [{ "Field": "eventCategory", "Equals": ["Management"] }],
                }],
            }),
            expects_write: true,
            expects_read: false,
        },
        Selectors {
            case: "an advanced selector on the management category, writes only",
            configuration: json!({
                "AdvancedEventSelectors": [{
                    "Name": "management-writes",
                    "FieldSelectors": [
                        { "Field": "eventCategory", "Equals": ["Management"] },
                        { "Field": "readOnly", "Equals": ["false"] },
                    ],
                }],
            }),
            expects_write: true,
            expects_read: false,
        },
        Selectors {
            case: "an advanced selector on another category",
            configuration: json!({
                "AdvancedEventSelectors": [{
                    "Name": "data",
                    "FieldSelectors": [{ "Field": "eventCategory", "Equals": ["Data"] }],
                }],
            }),
            expects_write: false,
            expects_read: false,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        // One emulator per row: the selectors are the trail's own configuration, so each
        // row configures a trail of its own rather than reconfiguring a shared one.
        let router = router();
        create_bucket(&router, "cloudfront-selector-trail").await;
        let trail = "cloudfront-selectors";
        cloudtrail(
            &router,
            row.case,
            "CreateTrail",
            &json!({ "Name": trail, "S3BucketName": "cloudfront-selector-trail" }),
        )
        .await;
        let mut configuration = row.configuration.clone();
        configuration["TrailName"] = json!(trail);
        cloudtrail(&router, row.case, "PutEventSelectors", &configuration).await;
        cloudtrail(&router, row.case, "StartLogging", &json!({ "Name": trail })).await;

        let name = format!("cloudfront-selector-sink-{index}");
        let (url, arn) = create_queue(&router, &name).await;
        events(
            &router,
            row.case,
            "PutRule",
            &json!({
                "Name": "cloudfront-selector-rule",
                "EventPattern": json!({
                    "source": [CLOUDFRONT_EVENT_SOURCE],
                    "detail-type": [API_CALL_DETAIL_TYPE],
                    "detail": { "eventName": ["CreateDistribution", "ListDistributions"] },
                })
                .to_string(),
            }),
        )
        .await;
        events(
            &router,
            row.case,
            "PutTargets",
            &json!({
                "Rule": "cloudfront-selector-rule",
                "Targets": [{ "Id": "sink", "Arn": arn }],
            }),
        )
        .await;

        let created = cloudfront(
            &router,
            "POST",
            &format!("/{API_VERSION}/distribution"),
            Some(&distribution_config("ref-selectors")),
        )
        .await;
        assert_eq!(
            created.status, 201,
            "case {:?}: CreateDistribution must succeed, got {} with {:?}",
            row.case, created.status, created.body
        );
        let listed = cloudfront(
            &router,
            "GET",
            &format!("/{API_VERSION}/distribution"),
            None,
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: ListDistributions must succeed, got {} with {:?}",
            row.case, listed.status, listed.body
        );

        let names: Vec<String> = drained(&router, row.case, &url)
            .await
            .iter()
            .filter_map(|event| {
                event
                    .pointer("/detail/eventName")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .collect();
        let arrived_write = names.contains(&"CreateDistribution".to_string());
        let arrived_read = names.contains(&"ListDistributions".to_string());
        assert_eq!(
            arrived_write, row.expects_write,
            "case {:?}: expected the write call published = {}, got {names:?}",
            row.case, row.expects_write
        );
        assert_eq!(
            arrived_read, row.expects_read,
            "case {:?}: expected the read call published = {}, got {names:?}",
            row.case, row.expects_read
        );
    }
}
