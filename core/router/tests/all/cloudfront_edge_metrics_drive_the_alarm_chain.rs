// SPDX-License-Identifier: CC0-1.0

//! Viewer traffic at a distribution's edge drives a CloudWatch alarm to ALARM and the
//! notification all the way to a subscribed queue — through the WHOLE mounted service set,
//! the one `core/router/src/lib.rs` assembles for the binary.
//!
//! The RULE under test: **every answer the edge serves for a distribution is counted in the
//! `AWS/CloudFront` namespace against that distribution — dimensioned by its own
//! `DistributionId` beside `Region=Global` — with each error rate carrying the share of the
//! period's viewer requests answered in that status class, so a metric alarm over
//! `4xxErrorRate` whose threshold that share crosses transitions to ALARM, publishes its SNS
//! alarm action, and the queue subscribed to that topic receives the notification; a
//! distribution nothing viewed produces no datapoint, so its own alarm stays `OK` under
//! `TreatMissingData: notBreaching`.**
//!
//! Five services have to agree for that to hold — S3 for the origin the edge fetches,
//! CloudFront for the edge that serves and counts, CloudWatch for the series and the
//! evaluation, SNS for the fan-out and SQS for what arrived — and the publish CloudWatch makes
//! is an in-process call addressed the way a client's own `sns`-scoped call is, so only the
//! production service set can show the whole chain. The crate-local suites pin each link:
//! `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/tests/all/edge_request_metrics.rs`
//! pins what the edge publishes over a table of status classes, and the cloudwatch crate's
//! own suites pin the evaluation and the delivery.
//!
//! The rows differ along what the rule generalises over: how many of the burst's requests are
//! answered 4xx (so the share the alarm grades differs), beside the untouched control
//! distribution whose alarm has the same configuration and no traffic at all. The sibling
//! [`super::cloudfront_edge_4xx_share_decides_the_alarm`] suite drives the same wiring — out of
//! the helpers below — over shares that do NOT breach the threshold, including the bound the
//! alarm's comparison excludes.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    cw, drain, given_topic_with_subscriber, Grant,
};
use crate::query_wire::element;

/// The whole emulator over one engine, exactly as the binary assembles it, with request
/// authentication as `authenticated` says.
///
/// The VIEWER plane is the authenticating one, which is what the binary serves: a viewer
/// presents no credential (CloudFront verifies none), so the only signature in the chain is
/// the one the edge puts on its own origin request. The setup calls go through a second
/// router over the SAME engine with authentication off, so each one carries a credential
/// scope (which is what names the Region) without a real signature.
pub(crate) fn emulator(
    engine: &Arc<aws_emulator_router::InMemoryEngine>,
    authenticated: bool,
) -> MultiServiceRouter {
    let mut router = aws_emulator_router::build_router(Arc::clone(engine));
    router.set_request_authentication(authenticated);
    router
}

/// The Region the whole wiring lives in. CloudFront reports its per-distribution metrics in
/// one Region, so the alarm, the topic and the queue live there too.
pub(crate) const REGION: &str = "us-east-1";

/// CloudFront's control-plane host and the API version every route is under.
const CLOUDFRONT_HOST: &str = "cloudfront.amazonaws.com";
const API_VERSION: &str = "2020-05-31";

/// The AWS-managed cache policy `Managed-CachingDisabled`, whose TTLs are all zero: no viewer
/// request of the burst is absorbed by a cache hit, so each one is answered — and counted —
/// on its own.
const CACHING_DISABLED: &str = "4135ea2d-6df8-44a3-9df3-4b5a84be39ad";

/// The service principal an origin access control makes CloudFront's origin request as, which
/// the origin bucket's own policy grants.
const CLOUDFRONT_PRINCIPAL: &str = "cloudfront.amazonaws.com";

/// The namespace, metric and dimensions the alarm watches: CloudFront's own per-distribution
/// series.
const CLOUDFRONT_NAMESPACE: &str = "AWS/CloudFront";
const FOUR_XX_ERROR_RATE: &str = "4xxErrorRate";
const DISTRIBUTION_ID_DIMENSION: &str = "DistributionId";
const REGION_DIMENSION: &str = "Region";
const GLOBAL_REGION: &str = "Global";

/// The alarm's period, in seconds, and the share of a period's requests its threshold sits
/// at: a burst in which more than this percentage answered 4xx breaches it.
const ALARM_PERIOD: i64 = 60;
pub(crate) const ALARM_THRESHOLD: f64 = 20.0;

/// How long a row waits for the chain to complete, and how often it looks again. An
/// evaluation grades the period the datapoints fell in once that period has been open for the
/// evaluation cadence, so the transition is seconds away rather than instant; the deadline is
/// generous only so a failure is a failure rather than a hang.
pub(crate) const CHAIN_DEADLINE: Duration = Duration::from_secs(90);
pub(crate) const LOOK_AGAIN: Duration = Duration::from_millis(200);

/// How long a row watches the untouched control for a transition it must NOT make, once the
/// subject has fired: long enough to span several of the alarm clock's own wake-ups.
const CONTROL_WATCH: Duration = Duration::from_secs(2);

/// One REST call against the service whose signing name is `signing_name`, addressed to
/// `host` — the credential scope is what routes it. Answers the status and the body as text.
async fn rest(
    router: &MultiServiceRouter,
    signing_name: &str,
    host: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: Bytes,
) -> (u16, String) {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(path)
        .header("host", host)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 \
                 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder.body(body).expect("the setup request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, text)
}

/// One VIEWER request, addressed the way a client on the machine this emulator runs on can
/// address one: nothing resolves a distribution's own domain name here, so the name travels
/// in the first path segment on the emulator's own endpoint — the addressing the edge reads
/// beside the host-style one. Answers the status and the body the viewer was served.
pub(crate) async fn view(router: &MultiServiceRouter, domain: &str, path: &str) -> (u16, String) {
    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/{domain}{path}"))
        .header("host", EMULATOR_ENDPOINT)
        .body(Bytes::new())
        .expect("the viewer request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let served = String::from_utf8_lossy(&response.into_body()).to_string();
    (status, served)
}

/// One VIEWER request addressed HOST-STYLE, at the distribution's own domain name.
async fn view_at_domain(router: &MultiServiceRouter, domain: &str, path: &str) -> u16 {
    let request = http::Request::builder()
        .method("GET")
        .uri(path)
        .header("host", domain)
        .body(Bytes::new())
        .expect("the viewer request is well formed");
    router.dispatch(request).await.status().as_u16()
}

/// The endpoint a client on the machine this emulator runs on reaches it at: nothing resolves
/// a distribution's own domain name here, so a viewer carries that name in the path instead.
const EMULATOR_ENDPOINT: &str = "127.0.0.1:9297";

/// The PATH-STYLE endpoint of this Region, which carries no bucket in its host.
fn regional_endpoint() -> String {
    format!("s3.{REGION}.amazonaws.com")
}

/// The host a bucket is addressed at virtual-hosted style on the GLOBAL endpoint, which is
/// the form a client's own `<bucket>.s3.amazonaws.com` origin domain takes.
fn origin_domain(bucket: &str) -> String {
    format!("{bucket}.s3.amazonaws.com")
}

/// The bytes the object in the origin bucket carries, which is what a viewer served that
/// object is answered with.
pub(crate) const ORIGIN_OBJECT_BODY: &str = "the object the edge serves";

/// A private bucket holding `key`, with the policy that grants the CloudFront service
/// principal the read its origin access control makes.
pub(crate) async fn given_origin_bucket(
    router: &MultiServiceRouter,
    case: &str,
    bucket: &str,
    key: &str,
) {
    let (status, body) = rest(
        router,
        "s3",
        &regional_endpoint(),
        "PUT",
        &format!("/{bucket}"),
        &[],
        Bytes::new(),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateBucket({bucket:?}) must succeed, got {status} with {body}"
    );
    let (status, body) = rest(
        router,
        "s3",
        &regional_endpoint(),
        "PUT",
        &format!("/{bucket}/{key}"),
        &[],
        Bytes::from(ORIGIN_OBJECT_BODY),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutObject({key:?}) must succeed, got {status} with {body}"
    );
    let policy = json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowCloudFrontServicePrincipal",
            "Effect": "Allow",
            "Principal": {"Service": CLOUDFRONT_PRINCIPAL},
            "Action": "s3:GetObject",
            "Resource": format!("arn:aws:s3:::{bucket}/*"),
        }],
    })
    .to_string();
    let (status, body) = rest(
        router,
        "s3",
        &regional_endpoint(),
        "PUT",
        &format!("/{bucket}?policy"),
        &[("content-type", "application/json")],
        Bytes::from(policy),
    )
    .await;
    assert!(
        (200..300).contains(&status),
        "case {case:?}: PutBucketPolicy must succeed, got {status} with {body}"
    );
}

/// `cloudfront:CreateOriginAccessControl` for an `s3` origin, answering the control's id.
pub(crate) async fn given_origin_access_control(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> String {
    let document = format!(
        "<OriginAccessControlConfig \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <Name>{name}</Name>\
         <SigningProtocol>sigv4</SigningProtocol>\
         <SigningBehavior>always</SigningBehavior>\
         <OriginAccessControlOriginType>s3</OriginAccessControlOriginType>\
         </OriginAccessControlConfig>"
    );
    let (status, body) = rest(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/origin-access-control"),
        &[("content-type", "application/xml")],
        Bytes::from(document),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateOriginAccessControl must succeed, got {status} with {body}"
    );
    element(&body, "Id")
}

/// `cloudfront:CreateDistributionWithTags` over the S3 origin `bucket` under the origin
/// access control `control`, answering the distribution's `(id, DomainName)` — the create a
/// client that tags its distributions makes, which runs the same create as
/// `CreateDistribution`.
pub(crate) async fn given_distribution(
    router: &MultiServiceRouter,
    case: &str,
    caller_reference: &str,
    bucket: &str,
    control: &str,
) -> (String, String) {
    let document = format!(
        "<DistributionConfigWithTags \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <DistributionConfig>\
         <CallerReference>{caller_reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items><Origin>\
         <Id>origin-bucket</Id><DomainName>{}</DomainName>\
         <S3OriginConfig><OriginAccessIdentity></OriginAccessIdentity></S3OriginConfig>\
         <OriginAccessControlId>{control}</OriginAccessControlId>\
         </Origin></Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>origin-bucket</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>\
         <CachePolicyId>{CACHING_DISABLED}</CachePolicyId>\
         </DefaultCacheBehavior>\
         <Comment>edge metrics into an alarm</Comment>\
         <Enabled>true</Enabled>\
         </DistributionConfig>\
         <Tags><Items><Tag><Key>suite</Key><Value>edge-metrics</Value></Tag></Items></Tags>\
         </DistributionConfigWithTags>",
        origin_domain(bucket)
    );
    let (status, body) = rest(
        router,
        "cloudfront",
        CLOUDFRONT_HOST,
        "POST",
        &format!("/{API_VERSION}/distribution?WithTags"),
        &[("content-type", "application/xml")],
        Bytes::from(document),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateDistributionWithTags must succeed, got {status} with {body}"
    );
    (element(&body, "Id"), element(&body, "DomainName"))
}

/// Wait until `GetDistribution` reports `id` as `Deployed`, which is when its edge serves.
pub(crate) async fn deployed(router: &MultiServiceRouter, case: &str, id: &str) {
    let started = Instant::now();
    let mut last = String::new();
    while started.elapsed() < CHAIN_DEADLINE {
        let (status, body) = rest(
            router,
            "cloudfront",
            CLOUDFRONT_HOST,
            "GET",
            &format!("/{API_VERSION}/distribution/{id}"),
            &[],
            Bytes::new(),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: GetDistribution({id}) must answer, got {status} with {body}"
        );
        last = element(&body, "Status");
        if last == "Deployed" {
            return;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
    panic!("case {case:?}: the distribution {id} never reached Deployed (last: {last:?})");
}

/// Create the alarm `name` over `distribution`'s 4xx error rate, with `topic` as its alarm
/// action.
pub(crate) async fn given_alarm(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    distribution: &str,
    topic: &str,
) {
    cw(
        router,
        case,
        REGION,
        "PutMetricAlarm",
        &json!({
            "AlarmName": name,
            "Namespace": CLOUDFRONT_NAMESPACE,
            "MetricName": FOUR_XX_ERROR_RATE,
            "Dimensions": [
                {"Name": DISTRIBUTION_ID_DIMENSION, "Value": distribution},
                {"Name": REGION_DIMENSION, "Value": GLOBAL_REGION},
            ],
            "Statistic": "Average",
            "Period": ALARM_PERIOD,
            "EvaluationPeriods": 1,
            "Threshold": ALARM_THRESHOLD,
            "ComparisonOperator": "GreaterThanThreshold",
            "TreatMissingData": "notBreaching",
            "AlarmActions": [topic],
        }),
    )
    .await;
}

/// What `DescribeAlarms` reports as `name`'s state.
pub(crate) async fn alarm_state(router: &MultiServiceRouter, case: &str, name: &str) -> String {
    let described = cw(
        router,
        case,
        REGION,
        "DescribeAlarms",
        &json!({"AlarmNames": [name]}),
    )
    .await;
    described["MetricAlarms"][0]["StateValue"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Wall-clock now as epoch seconds, the clock the series' windows are measured against.
pub(crate) fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |since| since.as_secs_f64())
}

/// The share of the burst's requests the series reports as answered 4xx, read back through
/// `GetMetricStatistics` over the window the traffic fell in.
pub(crate) async fn reported_rate(
    router: &MultiServiceRouter,
    case: &str,
    distribution: &str,
    at: f64,
) -> f64 {
    let answered = cw(
        router,
        case,
        REGION,
        "GetMetricStatistics",
        &json!({
            "Namespace": CLOUDFRONT_NAMESPACE,
            "MetricName": FOUR_XX_ERROR_RATE,
            "Dimensions": [
                {"Name": DISTRIBUTION_ID_DIMENSION, "Value": distribution},
                {"Name": REGION_DIMENSION, "Value": GLOBAL_REGION},
            ],
            "StartTime": at - 2.0 * ALARM_PERIOD as f64,
            "EndTime": at + 2.0 * ALARM_PERIOD as f64,
            "Period": ALARM_PERIOD,
            "Statistics": ["SampleCount", "Sum"],
        }),
    )
    .await;
    let mut samples = 0.0;
    let mut sum = 0.0;
    for datapoint in answered["Datapoints"].as_array().cloned().unwrap_or_default() {
        samples += datapoint["SampleCount"].as_f64().unwrap_or_default();
        sum += datapoint["Sum"].as_f64().unwrap_or_default();
    }
    assert!(
        samples > 0.0,
        "case {case:?}: the edge must have counted the burst, got {answered}"
    );
    sum / samples
}

/// One row: how many of the burst's requests ask for a key the origin does not hold, and how
/// many ask for the one it does.
struct Row {
    case: &'static str,
    missing: usize,
    present: usize,
}

#[tokio::test]
async fn an_edge_4xx_burst_fires_the_alarm_to_its_queue_while_the_untouched_control_stays_ok() {
    // Rows differ in the share of the burst answered 4xx — both above the alarm's threshold,
    // one of them with no successful request at all.
    let rows: [Row; 2] = [
        Row {
            case: "five of six requests answered 4xx",
            missing: 5,
            present: 1,
        },
        Row {
            case: "every request answered 4xx",
            missing: 2,
            present: 0,
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let engine = aws_emulator_router::new_engine();
        // The setup plane carries a credential scope without a computed signature; the viewer
        // plane below is the authenticating one the binary serves.
        let router = emulator(&engine, false);
        let viewers = emulator(&engine, true);
        let bucket = format!("edge-alarm-origin-{index}");
        let present_key = "present.txt";
        let missing_key = "absent.txt";
        given_origin_bucket(&router, case, &bucket, present_key).await;
        let control = given_origin_access_control(&router, case, &format!("edge-alarm-{index}")).await;
        let (subject_id, subject_domain) = given_distribution(
            &router,
            case,
            &format!("edge-alarm-subject-{index}"),
            &bucket,
            &control,
        )
        .await;
        let (quiet_id, _quiet_domain) = given_distribution(
            &router,
            case,
            &format!("edge-alarm-control-{index}"),
            &bucket,
            &control,
        )
        .await;
        deployed(&router, case, &subject_id).await;
        deployed(&router, case, &quiet_id).await;

        // Each alarm publishes to its OWN topic and queue, so the control's silence is read
        // where a client reads it rather than inferred from a shared queue.
        let (subject_topic, subject_queue) =
            given_topic_with_subscriber(&router, case, REGION, &format!("edge-subject-{index}"), Grant::Unscoped)
                .await;
        let (quiet_topic, quiet_queue) = given_topic_with_subscriber(
            &router,
            case,
            REGION,
            &format!("edge-control-{index}"),
            Grant::ScopedToTopic,
        )
        .await;
        let subject_alarm = format!("edge-4xx-subject-{index}");
        let quiet_alarm = format!("edge-4xx-control-{index}");
        given_alarm(&router, case, &subject_alarm, &subject_id, &subject_topic).await;
        given_alarm(&router, case, &quiet_alarm, &quiet_id, &quiet_topic).await;

        // The burst: viewer requests for a key the origin does not hold, answered 4xx, beside
        // the requests for the key it does hold, answered 200.
        let at = now_secs();
        for request in 0..row.missing {
            let (status, _served) = view(&viewers, &subject_domain, &format!("/{missing_key}")).await;
            assert!(
                (400..500).contains(&status),
                "case {case:?}: request {request} for a missing key must answer 4xx, got {status}"
            );
        }
        for request in 0..row.present {
            let (status, served) = view(&viewers, &subject_domain, &format!("/{present_key}")).await;
            assert_eq!(
                status, 200,
                "case {case:?}: request {request} for the present key must answer 200"
            );
            assert_eq!(
                served, ORIGIN_OBJECT_BODY,
                "case {case:?}: request {request} must be served the origin object's own bytes"
            );
        }
        // The same answer is served HOST-STYLE, at the name the distribution publishes, so
        // the addressing a client uses decides nothing about what it is served.
        let host_style = view_at_domain(&viewers, &subject_domain, &format!("/{present_key}")).await;
        assert_eq!(
            host_style, 200,
            "case {case:?}: the distribution's own domain name must serve the present key too"
        );

        // What the edge counted is the share of the burst that answered 4xx — over every
        // request it served for this distribution, the host-style one included.
        #[allow(clippy::cast_precision_loss)]
        let expected = 100.0 * row.missing as f64 / (row.missing + row.present + 1) as f64;
        let reported = reported_rate(&router, case, &subject_id, at).await;
        assert!(
            (reported - expected).abs() < f64::EPSILON,
            "case {case:?}: expected the edge to report a 4xx rate of {expected}, got {reported}"
        );
        assert!(
            reported > ALARM_THRESHOLD,
            "case {case:?}: the burst must breach the alarm's threshold, got {reported}"
        );

        // The chain: the evaluation the clock owes moves the alarm, its action publishes, and
        // the subscribed queue receives the notification — with no call forcing any of it.
        let started = Instant::now();
        let mut delivered: Vec<Value> = Vec::new();
        while started.elapsed() < CHAIN_DEADLINE && delivered.is_empty() {
            delivered.extend(drain(&router, case, REGION, &subject_queue).await);
        }
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: the alarm's action must reach its queue exactly once, got \
             {delivered:?}"
        );
        assert_eq!(
            delivered[0]["AlarmName"].as_str(),
            Some(subject_alarm.as_str()),
            "case {case:?}: the notification must name the alarm, got {}",
            delivered[0]
        );
        assert_eq!(
            delivered[0]["NewStateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the notification must carry the state moved to, got {}",
            delivered[0]
        );
        let state = alarm_state(&router, case, &subject_alarm).await;
        assert_eq!(
            state, "ALARM",
            "case {case:?}: the alarm the burst breached must report ALARM, got {state:?}"
        );

        // The untouched control: no viewer request reached it, so its series holds nothing,
        // its alarm stays OK and nothing is delivered for it.
        tokio::time::sleep(CONTROL_WATCH).await;
        let quiet_state = alarm_state(&router, case, &quiet_alarm).await;
        assert_eq!(
            quiet_state, "OK",
            "case {case:?}: the untouched distribution's alarm must stay OK, got {quiet_state:?}"
        );
        let withheld = drain(&router, case, REGION, &quiet_queue).await;
        assert!(
            withheld.is_empty(),
            "case {case:?}: nothing may be delivered for an alarm that never fired, got \
             {withheld:?}"
        );
    }
}
