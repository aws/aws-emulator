// SPDX-License-Identifier: CC0-1.0

//! The REAL-TIME LOG RECORD a served viewer request delivers to the Kinesis data stream its
//! cache behaviour's real-time log configuration names — end to end over HTTP, with
//! CloudFront, Kinesis and (for the role rules) IAM mounted on one router.
//!
//! The rules under test:
//!
//! * a real-time log configuration holds the submitted field SET in the order CloudFront
//!   SENDS the fields in, which is the order its own reads report:
//!   `RealtimeLogConfig$Fields` is "a list of fields that are included in each real-time log
//!   record. In an API response, the fields are provided in the same order in which they are
//!   sent to the Amazon Kinesis data stream";
//! * a viewer request the edge serves for a cache behaviour that names a configuration
//!   produces ONE record on each data stream that configuration's `EndPoints` name:
//!   tab-delimited, one value per field, in the order the configuration reported — and each
//!   value is the one THIS request and THIS answer carry;
//! * `SamplingRate` is "a whole number between 1 and 100 (inclusive) to determine the
//!   percentage of viewer requests that are represented in the real-time log data", so of the
//!   requests a configuration covers exactly that percentage produce a record;
//! * a behaviour that names no configuration delivers nothing;
//! * the write is made under the role the endpoint names
//!   (`KinesisStreamConfig$RoleARN`, "an IAM role that CloudFront can use to send real-time
//!   log data to your Kinesis data stream"), so an endpoint whose role is not allowed
//!   `kinesis:PutRecord` on its stream receives NOTHING while every other endpoint of the
//!   same configuration still receives its record.
//!
//! Every record read back here is read from KINESIS — `ListShards`, `GetShardIterator` and
//! `GetRecords` on the stream the configuration names — so a record this suite sees is a
//! record a client polling that stream sees.
//!
//! The origin is this service's own control-plane endpoint, whose
//! `GET /2020-05-31/distribution` answers a `DistributionList`: an origin this emulator
//! really serves, so a served request has an origin answer to be a cache miss of.

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::distribution_lifecycle::{call, get_distribution, API_VERSION, HOST};

/// How long a poll loop waits for a resource to reach the state the clock owes it. Generous:
/// what is asserted is that the state arrives, never how fast the host is.
const READY_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The Region every call below is signed for, and so the Region the streams' ARNs carry.
const REGION: &str = "us-east-1";

/// The status a stream that serves reads and writes is in.
const STREAM_ACTIVE: &str = "ACTIVE";

/// The status a distribution whose configuration has reached the edge locations is in.
const DEPLOYED: &str = "Deployed";

/// Kinesis's own endpoint, and the `X-Amz-Target` prefix its `awsJson1_1` operations are
/// named under (that model's `targetPrefix`).
const KINESIS_HOST: &str = "kinesis.amazonaws.com";
const KINESIS_TARGET: &str = "Kinesis_20131202";

/// IAM's own endpoint and API version, for the awsQuery calls the role rules need.
const IAM_HOST: &str = "iam.amazonaws.com";
const IAM_VERSION: &str = "2010-05-08";

/// One router carrying CloudFront and Kinesis over one shared engine: the edge delivers each
/// served request's log record to a data stream, so the two have to be mounted together for
/// the record to have anywhere to land.
fn emulator_with_kinesis() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(engine),
    );
    router
}

/// The same router with IAM beside them, for the rules about what the DELIVERY ROLE allows:
/// that decision is IAM's, so IAM has to be there to answer it.
fn emulator_with_kinesis_and_iam() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 credential scope naming `signing_name`, which is what routes a request to that
/// service.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// Call one Kinesis operation and answer its JSON body.
async fn kinesis(
    router: &MultiServiceRouter,
    operation: &str,
    body: ::serde_json::Value,
) -> ::serde_json::Value {
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", KINESIS_HOST)
        .header("authorization", authorization("kinesis"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("{KINESIS_TARGET}.{operation}"))
        .body(::bytes::Bytes::from(body.to_string()))
        .expect("the Kinesis request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    assert_eq!(
        status, 200,
        "case {operation}: expected HTTP 200 from Kinesis, got {status} with {text}"
    );
    if text.trim().is_empty() {
        return ::serde_json::Value::Null;
    }
    ::serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {operation}: Kinesis answered no JSON ({error}): {text}"))
}

/// Call one IAM operation over awsQuery and answer the XML body.
async fn iam(router: &MultiServiceRouter, action: &str, parameters: &[(&str, String)]) -> String {
    let mut body = format!("Action={action}&Version={IAM_VERSION}");
    for (name, value) in parameters {
        body.push('&');
        body.push_str(name);
        body.push('=');
        body.push_str(
            &::percent_encoding::utf8_percent_encode(value, ::percent_encoding::NON_ALPHANUMERIC)
                .to_string(),
        );
    }
    let request = ::http::Request::builder()
        .method("POST")
        .uri("/")
        .header("host", IAM_HOST)
        .header("authorization", authorization("iam"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(::bytes::Bytes::from(body))
        .expect("the IAM request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let text = String::from_utf8_lossy(&response.into_body()).to_string();
    assert_eq!(
        status, 200,
        "case {action}: expected HTTP 200 from IAM, got {status} with {text}"
    );
    text
}

/// The text of the first `<name>…</name>` element of an XML body.
fn element(body: &str, name: &str) -> Option<String> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = body.find(&open)? + open.len();
    let end = body[start..].find(&close)? + start;
    Some(body[start..end].to_string())
}

/// Create one Kinesis data stream and answer the ARN Kinesis minted for it, WITHOUT waiting
/// for the stream to serve writes ([`awaited_active`] does that).
///
/// Split in two so a suite with several streams starts them all and then waits once: the
/// clock owes each stream its creation from the moment that stream was asked for, so waiting
/// for one at a time would add up those spans for nothing.
///
/// The ARN is read out of Kinesis's own answer, never built here: it is the identifier that
/// service assigned, and it is what a real-time log configuration's endpoint names.
async fn data_stream(router: &MultiServiceRouter, name: &str) -> String {
    kinesis(
        router,
        "CreateStream",
        ::serde_json::json!({"StreamName": name, "ShardCount": 1}),
    )
    .await;
    let described = kinesis(
        router,
        "DescribeStreamSummary",
        ::serde_json::json!({"StreamName": name}),
    )
    .await;
    described["StreamDescriptionSummary"]["StreamARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: Kinesis reported no StreamARN: {described}"))
        .to_string()
}

/// Look again until the stream `name` reports itself `ACTIVE` — a stream that is still being
/// created serves no write — or a generous deadline passes.
async fn awaited_active(router: &MultiServiceRouter, name: &str) {
    let deadline = Instant::now() + READY_DEADLINE;
    loop {
        let described = kinesis(
            router,
            "DescribeStreamSummary",
            ::serde_json::json!({"StreamName": name}),
        )
        .await;
        if described["StreamDescriptionSummary"]["StreamStatus"].as_str() == Some(STREAM_ACTIVE) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {name}: the stream never reached {STREAM_ACTIVE}, last answer {described}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every record a stream holds, from its horizon, as the bytes a consumer decodes.
///
/// Read through Kinesis itself: the shards the stream reports, an iterator at each shard's
/// horizon, and the records that iterator answers — the same walk a client polling the stream
/// makes.
async fn stream_records(router: &MultiServiceRouter, stream_arn: &str) -> Vec<String> {
    let shards = kinesis(
        router,
        "ListShards",
        ::serde_json::json!({"StreamARN": stream_arn}),
    )
    .await;
    let mut records: Vec<String> = Vec::new();
    for shard in shards["Shards"].as_array().into_iter().flatten() {
        let shard_id = shard["ShardId"].as_str().unwrap_or_default();
        let iterator = kinesis(
            router,
            "GetShardIterator",
            ::serde_json::json!({
                "StreamARN": stream_arn,
                "ShardId": shard_id,
                "ShardIteratorType": "TRIM_HORIZON",
            }),
        )
        .await;
        let got = kinesis(
            router,
            "GetRecords",
            ::serde_json::json!({
                "StreamARN": stream_arn,
                "ShardIterator": iterator["ShardIterator"].as_str().unwrap_or_default(),
            }),
        )
        .await;
        for record in got["Records"].as_array().into_iter().flatten() {
            let data = record["Data"].as_str().unwrap_or_default();
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(data)
                .unwrap_or_else(|error| {
                    panic!("a record's Data is not base64 ({error}): {record}")
                });
            records.push(String::from_utf8_lossy(&bytes).to_string());
        }
    }
    records
}

/// One Kinesis endpoint a real-time log configuration delivers through.
struct EndPoint {
    role_arn: String,
    stream_arn: String,
}

/// Create one real-time log configuration and answer its ARN beside the field list the
/// service reported for it — which is the list each record's values are read by.
async fn realtime_log_config(
    router: &MultiServiceRouter,
    name: &str,
    sampling_rate: i64,
    fields: &[&str],
    end_points: &[EndPoint],
) -> (String, Vec<String>) {
    let mut body = format!(
        "<CreateRealtimeLogConfigRequest \
         xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\"><EndPoints>"
    );
    for end_point in end_points {
        body.push_str(&format!(
            "<member><StreamType>Kinesis</StreamType><KinesisStreamConfig>\
             <RoleARN>{}</RoleARN><StreamARN>{}</StreamARN>\
             </KinesisStreamConfig></member>",
            end_point.role_arn, end_point.stream_arn
        ));
    }
    body.push_str("</EndPoints><Fields>");
    for field in fields {
        body.push_str(&format!("<Field>{field}</Field>"));
    }
    body.push_str(&format!(
        "</Fields><Name>{name}</Name><SamplingRate>{sampling_rate}</SamplingRate>\
         </CreateRealtimeLogConfigRequest>"
    ));
    let answer = call(
        router,
        "POST",
        &format!("/{API_VERSION}/realtime-log-config"),
        Some(&body),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {name}: expected HTTP 201 creating the real-time log configuration, got {} with {}",
        answer.status, answer.raw
    );
    let arn = answer
        .root()
        .find_text("ARN")
        .unwrap_or_else(|| panic!("case {name}: the create carries no ARN: {}", answer.raw))
        .to_string();
    let reported: Vec<String> = answer
        .root()
        .descendant("Fields")
        .map(|fields| {
            fields
                .children
                .iter()
                .map(|field| field.text().to_string())
                .collect()
        })
        .unwrap_or_default();
    (arn, reported)
}

/// One `DistributionConfig` over the served origin, whose DEFAULT cache behaviour names
/// `realtime_log_config_arn` when it names one, and which caches for `default_ttl` seconds
/// when it caches at all.
fn config_xml(
    reference: &str,
    realtime_log_config_arn: Option<&str>,
    default_ttl: Option<i64>,
) -> String {
    let logging = realtime_log_config_arn
        .map(|arn| format!("<RealtimeLogConfigArn>{arn}</RealtimeLogConfigArn>"))
        .unwrap_or_default();
    let ttl = default_ttl
        .map(|seconds| format!("<DefaultTTL>{seconds}</DefaultTTL>"))
        .unwrap_or_default();
    format!(
        "<DistributionConfig xmlns=\"http://cloudfront.amazonaws.com/doc/{API_VERSION}/\">\
         <CallerReference>{reference}</CallerReference>\
         <Origins><Quantity>1</Quantity><Items>\
         <Origin><Id>primary</Id><DomainName>{HOST}</DomainName></Origin>\
         </Items></Origins>\
         <DefaultCacheBehavior><TargetOriginId>primary</TargetOriginId>\
         <ViewerProtocolPolicy>allow-all</ViewerProtocolPolicy>{logging}{ttl}\
         </DefaultCacheBehavior>\
         <Comment>{reference}</Comment>\
         <PriceClass>PriceClass_All</PriceClass>\
         <Enabled>true</Enabled>\
         </DistributionConfig>"
    )
}

/// One distribution this suite created: the id its deployment is polled by and the domain
/// name a viewer dials it at.
struct Distribution {
    id: String,
    domain: String,
}

/// Create one distribution, WITHOUT waiting for its configuration to reach the edge
/// locations ([`awaited_deployment`] does that).
///
/// Split in two for the reason [`data_stream`] is: the clock owes each distribution its
/// deployment from the moment it was created, so a suite with several of them creates them
/// all and then waits once.
async fn distribution(
    router: &MultiServiceRouter,
    reference: &str,
    realtime_log_config_arn: Option<&str>,
    default_ttl: Option<i64>,
) -> Distribution {
    let answer = call(
        router,
        "POST",
        &format!("/{API_VERSION}/distribution"),
        Some(&config_xml(reference, realtime_log_config_arn, default_ttl)),
        None,
    )
    .await;
    assert_eq!(
        answer.status, 201,
        "case {reference}: expected HTTP 201 creating the distribution, got {} with {}",
        answer.status, answer.raw
    );
    let id = answer
        .root()
        .find_text("Id")
        .unwrap_or_else(|| panic!("case {reference}: the create carries no Id: {}", answer.raw))
        .to_string();
    let domain = answer
        .root()
        .find_text("DomainName")
        .unwrap_or_else(|| {
            panic!(
                "case {reference}: the create carries no DomainName: {}",
                answer.raw
            )
        })
        .to_string();
    Distribution { id, domain }
}

/// Look again until the distribution reports itself `Deployed` — a configuration that has not
/// reached the edge locations serves no viewer — or a generous deadline passes.
async fn awaited_deployment(router: &MultiServiceRouter, case: &str, distribution: &Distribution) {
    let deadline = Instant::now() + READY_DEADLINE;
    loop {
        let read = get_distribution(router, &distribution.id).await;
        if read.root().find_text("Status").unwrap_or_default() == DEPLOYED {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the distribution never reached {DEPLOYED}, last answer {}",
            read.raw
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// What the edge answered one viewer request with.
struct Viewed {
    status: u16,
    /// `X-Amz-Cf-Id`, the identifier the edge stamped on the request it served.
    request_id: String,
}

/// One VIEWER request, addressed host-style at the name the distribution published and
/// carrying no credential — which is what a viewer is.
async fn view(router: &MultiServiceRouter, domain: &str, method: &str, target: &str) -> Viewed {
    let request = ::http::Request::builder()
        .method(method)
        .uri(target)
        .header("host", domain)
        .body(::bytes::Bytes::new())
        .expect("the viewer request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get("x-amz-cf-id")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    Viewed { status, request_id }
}

/// The request one record is expected to describe, and the answer it got.
struct Probe {
    method: &'static str,
    path: String,
    query: &'static str,
    domain: String,
    status: u16,
    result_type: &'static str,
    request_id: String,
    /// The caller's own clock window the record's `timestamp` has to fall inside.
    window: (u64, u64),
}

/// Assert `record` is the record `probe` owes, read by the field list the configuration
/// reported: one value per field, in that order.
///
/// Each field is checked against what THIS request and THIS answer carry, so a record of
/// another request, or a value from another part of the request, fails the row.
fn assert_record(case: &str, fields: &[String], record: &str, probe: &Probe) {
    let values: Vec<&str> = record.split('\t').collect();
    assert_eq!(
        values.len(),
        fields.len(),
        "case {case}: expected one value per field {fields:?}, got {record:?}"
    );
    for (field, value) in fields.iter().zip(values) {
        match field.as_str() {
            "timestamp" => {
                let seconds: u64 = value.parse().unwrap_or_else(|error| {
                    panic!("case {case}: timestamp {value:?} is no instant ({error})")
                });
                assert!(
                    (probe.window.0..=probe.window.1).contains(&seconds),
                    "case {case}: expected a timestamp inside the run's window {:?}, got \
                     {seconds}",
                    probe.window
                );
            }
            "c-ip" => assert!(
                !value.is_empty() && value != "-",
                "case {case}: expected a non-empty c-ip, got {value:?}"
            ),
            "c-ip-version" => assert_eq!(
                value, "IPv4",
                "case {case}: expected the IP version of the viewer's own address, got {value:?}"
            ),
            "sc-status" => assert_eq!(
                value,
                probe.status.to_string(),
                "case {case}: expected sc-status {}, got {value:?}",
                probe.status
            ),
            "cs-uri-stem" => assert_eq!(
                value, probe.path,
                "case {case}: expected cs-uri-stem {:?}, got {value:?}",
                probe.path
            ),
            "cs-uri-query" => {
                let expected = if probe.query.is_empty() {
                    "-"
                } else {
                    probe.query
                };
                assert_eq!(
                    value, expected,
                    "case {case}: expected cs-uri-query {expected:?}, got {value:?}"
                );
            }
            "cs-method" => assert_eq!(
                value, probe.method,
                "case {case}: expected cs-method {:?}, got {value:?}",
                probe.method
            ),
            "cs-host" | "x-host-header" => assert_eq!(
                value, probe.domain,
                "case {case}: expected {field} {:?}, got {value:?}",
                probe.domain
            ),
            "x-edge-result-type" | "x-edge-response-result-type" => assert_eq!(
                value, probe.result_type,
                "case {case}: expected {field} {:?}, got {value:?}",
                probe.result_type
            ),
            "x-edge-request-id" => assert_eq!(
                value, probe.request_id,
                "case {case}: expected the identifier the edge stamped on the answer ({:?}), \
                 got {value:?}",
                probe.request_id
            ),
            "c-country" => assert_eq!(
                value.len(),
                2,
                "case {case}: expected a two-letter country code, got {value:?}"
            ),
            "headers-count" => {
                let count: usize = value.parse().unwrap_or_else(|error| {
                    panic!("case {case}: headers-count {value:?} is no count ({error})")
                });
                assert!(
                    count > 0,
                    "case {case}: a viewer request carries at least its Host header, got {count}"
                );
            }
            "cs-referer" => assert_eq!(
                value, "-",
                "case {case}: a viewer that sent no Referer has none, got {value:?}"
            ),
            "cache-behavior-path-pattern" => assert_eq!(
                value, "*",
                "case {case}: expected the default behaviour's own pattern, got {value:?}"
            ),
            other => panic!("case {case}: the suite asserts nothing about the field {other:?}"),
        }
    }
}

/// The caller's own clock window, as the record's `timestamp` is checked against: the second
/// the run started in, through a generous span past it.
fn window() -> (u64, u64) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    (now.saturating_sub(1), now + READY_DEADLINE.as_secs() * 4)
}

/// One row of the delivery table.
struct Row {
    case: &'static str,
    /// The fields the configuration is created with, in an order of the REQUEST's.
    submitted: &'static [&'static str],
    /// The fields the service is owed to report for it: the submitted SET in the order
    /// CloudFront sends the fields in.
    reported: &'static [&'static str],
    sampling_rate: i64,
    /// Whether the distribution's cache behaviour names the configuration at all.
    attached: bool,
    /// How many viewer requests the row drives.
    requests: usize,
    /// How many records those requests owe the stream.
    records: usize,
    /// The viewer's query string, without its leading `?`.
    query: &'static str,
}

/// A configuration's field SET comes back in CloudFront's own order, each served viewer
/// request its behaviour covers puts exactly one record carrying those fields on the
/// configuration's stream, the `SamplingRate` decides how many of the covered requests are
/// represented, and a behaviour naming no configuration delivers nothing.
#[tokio::test]
async fn a_served_viewer_request_delivers_one_record_to_the_configured_stream() {
    let router = emulator_with_kinesis();
    // Rows differ along what the rules generalise over: the field set and the order it was
    // submitted in (including fields the probe set does not name), the sampling rate (every
    // request, three in four, every second one, one in four, and the smallest rate of all),
    // how many requests are driven, whether the behaviour names the configuration at all,
    // and whether the viewer sent a query string.
    let rows: [Row; 7] = [
        Row {
            case: "the probe field set, submitted out of CloudFront's order",
            submitted: &[
                "timestamp",
                "c-ip",
                "cs-uri-stem",
                "sc-status",
                "x-edge-response-result-type",
            ],
            reported: &[
                "timestamp",
                "c-ip",
                "sc-status",
                "cs-uri-stem",
                "x-edge-response-result-type",
            ],
            sampling_rate: 100,
            attached: true,
            requests: 1,
            records: 1,
            query: "",
        },
        Row {
            case: "fields the probe set does not name, submitted reversed",
            submitted: &[
                "headers-count",
                "cache-behavior-path-pattern",
                "c-country",
                "x-host-header",
                "x-edge-request-id",
                "cs-host",
                "cs-uri-query",
                "cs-method",
                "c-ip-version",
            ],
            reported: &[
                "cs-method",
                "cs-host",
                "x-edge-request-id",
                "x-host-header",
                "c-ip-version",
                "cs-uri-query",
                "c-country",
                "cache-behavior-path-pattern",
                "headers-count",
            ],
            sampling_rate: 100,
            attached: true,
            requests: 1,
            records: 1,
            query: "one=1&two=2",
        },
        Row {
            case: "every second request represented, over four requests",
            submitted: &["sc-status", "timestamp"],
            reported: &["timestamp", "sc-status"],
            sampling_rate: 50,
            attached: true,
            requests: 4,
            records: 2,
            query: "",
        },
        Row {
            case: "three requests in four represented, over four requests",
            submitted: &["sc-status", "timestamp"],
            reported: &["timestamp", "sc-status"],
            sampling_rate: 75,
            attached: true,
            requests: 4,
            records: 3,
            query: "",
        },
        Row {
            case: "one request in four represented, over four requests",
            submitted: &["sc-status"],
            reported: &["sc-status"],
            sampling_rate: 25,
            attached: true,
            requests: 4,
            records: 1,
            query: "",
        },
        Row {
            case: "the smallest rate, over four requests",
            submitted: &["sc-status"],
            reported: &["sc-status"],
            sampling_rate: 1,
            attached: true,
            requests: 4,
            records: 0,
            query: "",
        },
        Row {
            case: "a cache behaviour that names no configuration",
            submitted: &["timestamp", "sc-status"],
            reported: &["timestamp", "sc-status"],
            sampling_rate: 100,
            attached: false,
            requests: 2,
            records: 0,
            query: "",
        },
    ];

    // Every stream, configuration and distribution is created FIRST, and only then waited
    // for: the clock owes each of them its own readiness from the moment it was asked for,
    // so the whole table waits about as long as one row does.
    struct Staged {
        case: &'static str,
        name: String,
        stream_arn: String,
        reported: Vec<String>,
        distribution: Distribution,
    }
    let mut staged: Vec<Staged> = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("realtime-logs-{index}");
        let stream_arn = data_stream(&router, &name).await;
        let (config_arn, reported) = realtime_log_config(
            &router,
            &format!("probe-{index}"),
            row.sampling_rate,
            row.submitted,
            &[EndPoint {
                role_arn: format!(
                    "arn:aws:iam::{}:role/cf-realtime-logs",
                    engine::emulator_account_id()
                ),
                stream_arn: stream_arn.clone(),
            }],
        )
        .await;
        let expected_fields: Vec<String> =
            row.reported.iter().map(|field| (*field).to_string()).collect();
        assert_eq!(
            reported, expected_fields,
            "case {case}: expected the submitted field set reported in CloudFront's own \
             order {expected_fields:?}, got {reported:?}"
        );
        let attached = row.attached.then_some(config_arn.as_str());
        let distribution = distribution(&router, &name, attached, None).await;
        staged.push(Staged {
            case,
            name,
            stream_arn,
            reported,
            distribution,
        });
    }

    for row in &staged {
        awaited_active(&router, &row.name).await;
        awaited_deployment(&router, row.case, &row.distribution).await;
    }

    for (row, staged) in rows.iter().zip(&staged) {
        let case = row.case;
        let reported = &staged.reported;
        let domain = &staged.distribution.domain;
        let path = format!("/{API_VERSION}/distribution");
        let target = if row.query.is_empty() {
            path.clone()
        } else {
            format!("{path}?{}", row.query)
        };
        let window = window();
        let mut request_ids: Vec<String> = Vec::new();
        for request in 0..row.requests {
            let answer = view(&router, domain, "GET", &target).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected the origin's answer to viewer request {request}, got {}",
                answer.status
            );
            request_ids.push(answer.request_id);
        }
        let records = stream_records(&router, &staged.stream_arn).await;
        assert_eq!(
            records.len(),
            row.records,
            "case {case}: expected {} record(s) on the stream for {} viewer request(s), got \
             {records:?}",
            row.records,
            row.requests
        );
        for record in &records {
            // The record describes ONE of the requests driven, and every value in it is
            // that request's: which of them it is, is what the identifier the edge stamped
            // on each answer says.
            let request_id = request_ids
                .iter()
                .find(|id| record.split('\t').any(|value| value == id.as_str()))
                .cloned()
                .unwrap_or_else(|| request_ids.last().cloned().unwrap_or_default());
            let probe = Probe {
                method: "GET",
                path: path.clone(),
                query: row.query,
                domain: domain.clone(),
                status: 200,
                result_type: "Miss",
                request_id,
                window,
            };
            assert_record(case, reported, record, &probe);
        }
    }
}

/// How the edge CLASSIFIED each answer is what the record reports: the origin's own answer
/// is a miss, the one the edge cache then holds is a hit, and a request CloudFront refuses
/// itself is an error — and the status in each record is that answer's own.
#[tokio::test]
async fn a_record_reports_how_the_edge_classified_the_answer_it_served() {
    let router = emulator_with_kinesis();
    let stream_arn = data_stream(&router, "realtime-logs-classified").await;
    let fields = &[
        "timestamp",
        "sc-status",
        "cs-uri-stem",
        "cs-referer",
        "x-edge-response-result-type",
    ];
    let (config_arn, reported) = realtime_log_config(
        &router,
        "classified",
        100,
        fields,
        &[EndPoint {
            role_arn: format!("arn:aws:iam::{}:role/cf-realtime-logs", engine::emulator_account_id()),
            stream_arn: stream_arn.clone(),
        }],
    )
    .await;
    // A behaviour that caches, so the second request for one path is answered from the edge
    // cache rather than by the origin.
    let distribution = distribution(
        &router,
        "realtime-logs-classified",
        Some(&config_arn),
        Some(86_400),
    )
    .await;
    awaited_active(&router, "realtime-logs-classified").await;
    awaited_deployment(&router, "classified", &distribution).await;
    let domain = &distribution.domain;
    let path = format!("/{API_VERSION}/distribution");
    let window = window();

    // (case, the method the viewer used, the status the edge owes, the classification)
    let rows: [(&str, &str, u16, &str); 3] = [
        ("the origin's own answer", "GET", 200, "Miss"),
        ("the answer the edge cache then held", "GET", 200, "Hit"),
        (
            "a method the behaviour does not allow, refused by CloudFront",
            "DELETE",
            403,
            "Error",
        ),
    ];
    let mut expected: Vec<(&str, Probe)> = Vec::new();
    for (case, method, status, result_type) in rows {
        let answer = view(&router, domain, method, &path).await;
        assert_eq!(
            answer.status, status,
            "case {case}: expected the edge to answer {status}, got {}",
            answer.status
        );
        expected.push((
            case,
            Probe {
                method,
                path: path.clone(),
                query: "",
                domain: domain.clone(),
                status,
                result_type,
                request_id: answer.request_id,
                window,
            },
        ));
    }

    let records = stream_records(&router, &stream_arn).await;
    assert_eq!(
        records.len(),
        expected.len(),
        "expected one record per served viewer request, got {records:?}"
    );
    // The records arrive in the order the requests were served, a stream holding the records
    // of one producer in the order they were written.
    for (record, (case, probe)) in records.iter().zip(&expected) {
        assert_record(case, &reported, record, probe);
    }
}

/// An inline policy document carrying one statement per `(effect, action)` pair, on
/// `resource`.
fn policy(statements: &[(&str, &str)], resource: &str) -> String {
    let rendered: Vec<String> = statements
        .iter()
        .map(|(effect, action)| {
            ::serde_json::json!({"Effect": effect, "Action": action, "Resource": resource})
                .to_string()
        })
        .collect();
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
        rendered.join(",")
    )
}

/// Create one IAM role CloudFront can assume, carrying `document` as its one inline policy,
/// and answer the ARN IAM minted for it.
async fn delivery_role(router: &MultiServiceRouter, name: &str, document: &str) -> String {
    let trust = ::serde_json::json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "cloudfront.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }],
    })
    .to_string();
    let created = iam(
        router,
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    iam(
        router,
        "PutRolePolicy",
        &[
            ("RoleName", name.to_string()),
            ("PolicyName", "deliver".to_string()),
            ("PolicyDocument", document.to_string()),
        ],
    )
    .await;
    element(&created, "Arn")
        .unwrap_or_else(|| panic!("case {name}: CreateRole answered no Arn: {created}"))
}

/// One row of the role table: what it is about, the stream's name, what the role's policy
/// states about that stream, and how many records the stream is owed for one served viewer
/// request.
type RoleRow = (&'static str, &'static str, &'static [(&'static str, &'static str)], usize);

/// The write is the delivery ROLE's to make: an endpoint whose role IAM says may not put a
/// record on its stream receives nothing, while every other endpoint of the same
/// configuration — a control stream on the same distribution — receives every record.
#[tokio::test]
async fn an_endpoint_whose_role_may_not_put_a_record_receives_nothing() {
    let router = emulator_with_kinesis_and_iam();
    // Rows differ along what the role's policy states about the stream: the put allowed on
    // its own, allowed through a wildcard over the service's actions, left out of the
    // policy altogether, and explicitly denied beside a wildcard allow.
    let rows: [RoleRow; 4] = [
        (
            "a role allowed to put a record on the stream",
            "realtime-logs-allowed",
            &[("Allow", "kinesis:PutRecord")],
            1,
        ),
        (
            "a role allowed every Kinesis action on the stream",
            "realtime-logs-wildcard",
            &[("Allow", "kinesis:*")],
            1,
        ),
        (
            "a role whose policy omits the put",
            "realtime-logs-omitted",
            &[("Allow", "kinesis:DescribeStreamSummary")],
            0,
        ),
        (
            "a role explicitly denied the put",
            "realtime-logs-denied",
            &[("Allow", "kinesis:*"), ("Deny", "kinesis:PutRecord")],
            0,
        ),
    ];

    let mut end_points: Vec<EndPoint> = Vec::new();
    let mut streams: Vec<(&str, &str, String, usize)> = Vec::new();
    for (case, name, statements, owed) in rows {
        let stream_arn = data_stream(&router, name).await;
        let role_arn = delivery_role(&router, name, &policy(statements, &stream_arn)).await;
        end_points.push(EndPoint {
            role_arn,
            stream_arn: stream_arn.clone(),
        });
        streams.push((case, name, stream_arn, owed));
    }

    // ONE configuration naming every endpoint, attached to ONE distribution: what decides
    // each stream's outcome is its own role and nothing else.
    let (config_arn, _reported) = realtime_log_config(
        &router,
        "delivery-roles",
        100,
        &["timestamp", "sc-status", "cs-uri-stem"],
        &end_points,
    )
    .await;
    let distribution = distribution(&router, "realtime-logs-roles", Some(&config_arn), None).await;
    // Every stream was created before any of them was waited for, so the table waits about
    // as long as one stream does.
    for (_case, name, _stream_arn, _owed) in &streams {
        awaited_active(&router, name).await;
    }
    awaited_deployment(&router, "delivery-roles", &distribution).await;
    let path = format!("/{API_VERSION}/distribution");
    let answer = view(&router, &distribution.domain, "GET", &path).await;
    assert_eq!(
        answer.status, 200,
        "expected the origin's answer to the viewer request, got {}",
        answer.status
    );

    for (case, _name, stream_arn, owed) in streams {
        let records = stream_records(&router, &stream_arn).await;
        assert_eq!(
            records.len(),
            owed,
            "case {case}: expected {owed} record(s) on this stream, got {records:?}"
        );
    }
}
