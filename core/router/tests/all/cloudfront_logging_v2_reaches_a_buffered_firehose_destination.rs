// SPDX-License-Identifier: CC0-1.0

//! STANDARD LOGGING v2 into a destination that BUFFERS: the access-log record a served viewer
//! request leaves in an Amazon S3 delivery destination, and in a Firehose delivery destination
//! whose stream was configured to hold its records for minutes.
//!
//! RULE: `BufferingHints$IntervalInSeconds` is how long a destination's records are held
//! before they are written — "buffer incoming data for the specified period of time, in
//! seconds, before delivering it to the destination" — so the window a destination asks for is
//! the wait its records get, and they are at the destination once that window has passed and
//! not before. A vended-log pipeline is such a producer — `CreateDelivery`
//! pairs "exactly one delivery source and one delivery destination", and "a delivery
//! destination can represent a log group in CloudWatch Logs, an Amazon S3 bucket, a delivery
//! stream in Firehose, or X-Ray" — so each request a CloudFront distribution's edge serves
//! becomes ONE record in EVERY live delivery's destination, whichever kind each is and whatever
//! buffering the stream behind one of them was configured with, and the records of one request
//! carry the same `x-edge-request-id` ("an encrypted string that uniquely identifies a
//! request", the identifier the edge answer handed the viewer as `X-Amz-Cf-Id`) in all of them,
//! so a consumer can join them.
//!
//! Only a cross-service suite can pin this: the sending side is CloudFront's edge over a
//! private Amazon S3 origin, the pipeline is CloudWatch Logs' own, and the Firehose leg lands in
//! Amazon S3 only once the stream behind it flushes, under the role IAM holds for it. The
//! CloudFront crate's `edge_standard_logging_v2` suite pins the fan-out itself over a stream
//! whose hints ask for no wait at all, and the Firehose crate's `buffering_hints` suite pins the
//! window rule on a producer's own puts; neither states that a record travelling a vended-log
//! pipeline reaches a destination whose stream asked to hold it for minutes.
//!
//! The rows span what the rule generalises over: the `IntervalInSeconds` the destination stream
//! asks for — the published minimum and three non-zero windows a case can wait out — beside the
//! `SizeInMBs` next to it, none of which two small records come near (the published ranges are
//! `SizeInMBs` 1..128 and `IntervalInSeconds` 0..900, `smithy_query(query="shape",
//! shape="SizeInMBs,IntervalInSeconds")`), and the ANSWER each viewer request got (the origin's
//! object, then the same read again, which the managed caching policy answers from the edge
//! cache — two result classifications).
//!
//! No case sleeps a window out or reads the wall clock: every destination is polled until it
//! holds the records its pipeline owes, under one deadline generous beside every window a row
//! asks for, so a loaded host never fails a delivery the clock would have made.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::state::invocation_logging as logging;

use super::cloudfront_apigateway_origin_edge::wait_until_deployed;
use super::cloudfront_oac_s3_origin_edge::{
    call, create_bucket, create_distribution, create_origin_access_control, policy,
    put_bucket_policy, put_object, view, CLOUDFRONT_PRINCIPAL, REGION,
};
use super::logs_delivery_source_cloudfront_resource::logs;
use super::query_wire::form_encode;

/// The AWS-managed cache policy `Managed-CachingOptimized`, by the id Amazon publishes for it:
/// the second of two identical viewer requests is answered from the edge cache under it, so the
/// two requests the rows log differ in their result classification.
///
/// `pub(crate)` with the pipeline helpers below, so the sibling suite that drives the same
/// pipeline over a window no consumer could wait out
/// ([`super::cloudfront_access_log_join_survives_a_minutes_long_firehose_window`]) wires it the
/// same way rather than keeping another copy.
pub(crate) const CACHING_OPTIMIZED: &str = "658327ea-f89d-4fab-a63d-7e88639e58f6";

/// The one `logType` a CloudFront delivery source sends ("For CloudFront, the valid value is
/// ACCESS_LOGS", `smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
pub(crate) const LOG_TYPE: &str = "ACCESS_LOGS";

/// `DeliveryDestination$outputFormat` every destination here is created under: the structured
/// one of `OutputFormat`'s own values (`smithy_query(query="shape",shape="OutputFormat")`), so
/// each record is a JSON object of its fields.
pub(crate) const OUTPUT_FORMAT: &str = "json";

/// The `DeliveryDestinationType` of a destination that is an Amazon S3 bucket, and of one that
/// is a Firehose delivery stream (`smithy_query(query="shape",shape="DeliveryDestinationType")`).
pub(crate) const S3: &str = "S3";
pub(crate) const FH: &str = "FH";

/// The record layout every delivery here carries: the fields a consumer joining an S3 record to
/// a Firehose record reads, in this order.
pub(crate) const FIELDS: &[&str] = &[
    "x-edge-request-id",
    "cs-uri-stem",
    "sc-status",
    "x-edge-result-type",
];

/// The origin bucket, the bucket the S3 delivery destination names, the key the origin holds
/// and the bytes it holds there.
const ORIGIN_BUCKET: &str = "logv2-buffered-origin";
const VENDED_BUCKET: &str = "logv2-buffered-vended";
const KEY: &str = "report.txt";
const PAYLOAD: &str = "the object the edge serves and logs";

/// The role every destination stream's own Amazon S3 delivery is made under.
const ROLE: &str = "logv2-buffered-firehose";

/// The `X-Amz-Target` prefix and the content type Firehose's wire carries.
pub(crate) const FIREHOSE_TARGET: &str = "Firehose_20150804";
const JSON_1_1: &str = "application/x-amz-json-1.1";

/// The `DeliveryStreamStatus` a stream takes puts in.
pub(crate) const ACTIVE: &str = "ACTIVE";

/// How long a destination that IS owed its records is polled before a case fails, and how often
/// it is looked at again.
///
/// Many times the longest window any row here asks its stream to hold its records for, so a
/// loaded host never fails a delivery the clock would have made; it bounds nothing a case
/// measures.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(50);

/// One Firehose delivery destination of the pipeline: the buffering its stream asks for.
struct Row {
    case: &'static str,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`; `None` leaves the member out
    /// altogether, which is the documented defaults.
    hints: Option<(i64, i64)>,
}

/// The rows: the window each destination stream asks to hold its records for.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the shortest interval the member admits",
            hints: Some((1, 0)),
        },
        Row {
            case: "a short window beside the documented default size",
            hints: Some((5, 3)),
        },
        Row {
            case: "a longer window under a size two small records never come near",
            hints: Some((64, 6)),
        },
        Row {
            case: "the longest window a case can wait out, under the largest size it admits",
            hints: Some((128, 9)),
        },
    ]
}

/// One wired leg of the pipeline: where its records land and how a consumer reads them.
pub(crate) struct Leg {
    pub(crate) case: &'static str,
    /// The bucket its records land in, and the prefix they land under.
    pub(crate) bucket: String,
    pub(crate) prefix: String,
    /// Whether the objects are the gzipped log files a vended-log delivery writes into a
    /// bucket, or the records a delivery stream put into its own destination uncompressed.
    pub(crate) gzipped: bool,
}

/// The five services this suite exercises, mounted on `engine` exactly as the router mounts
/// them, with request authentication as `authenticated` says.
pub(crate) fn emulator(
    engine: &Arc<engine::InMemoryEngine>,
    authenticated: bool,
) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(authenticated);
    router.register_service(
        service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(Arc::clone(
            engine,
        )),
    );
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(
        Arc::clone(engine),
    ));
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(Arc::clone(engine)),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(Arc::clone(
            engine,
        )),
    );
    router
}

/// One Firehose call, which must be accepted.
pub(crate) async fn firehose(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
) -> Value {
    let answered = call(
        router,
        "firehose",
        &format!("firehose.{REGION}.amazonaws.com"),
        "POST",
        "/",
        &[
            ("content-type", JSON_1_1),
            ("x-amz-target", &format!("{FIREHOSE_TARGET}.{operation}")),
        ],
        bytes::Bytes::from(body.to_string()),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: {operation} must be accepted, got {} with {:?}",
        answered.status,
        answered.text()
    );
    serde_json::from_str(&answered.text()).unwrap_or(Value::Null)
}

/// One IAM action, which must be accepted.
pub(crate) async fn iam(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) {
    let mut body = format!("Action={action}&Version=2010-05-08");
    for (key, value) in params {
        body.push('&');
        body.push_str(&form_encode(key));
        body.push('=');
        body.push_str(&form_encode(value));
    }
    let answered = call(
        router,
        "iam",
        "iam.amazonaws.com",
        "POST",
        "/",
        &[("content-type", "application/x-www-form-urlencoded")],
        bytes::Bytes::from(body),
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "case {case:?}: IAM {action} must be accepted, got {} with {:?}",
        answered.status,
        answered.text()
    );
}

/// Every key `bucket` holds under `prefix`.
pub(crate) async fn keys_under(
    router: &MultiServiceRouter,
    bucket: &str,
    prefix: &str,
) -> Vec<String> {
    let listed = call(
        router,
        "s3",
        &format!("s3.{REGION}.amazonaws.com"),
        "GET",
        &format!("/{bucket}?list-type=2&prefix={prefix}"),
        &[],
        bytes::Bytes::new(),
    )
    .await;
    assert_eq!(
        listed.status,
        200,
        "listing {bucket:?} under {prefix:?} must succeed, got {} with {:?}",
        listed.status,
        listed.text()
    );
    listed
        .text()
        .split("<Key>")
        .skip(1)
        .filter_map(|tail| tail.split("</Key>").next())
        .map(str::to_string)
        .collect()
}

/// The records one leg's destination holds, read the way a consumer of that kind of destination
/// reads them: the gzipped log file a vended-log delivery writes into a bucket, or the
/// newline-delimited records a delivery stream put into its own destination.
pub(crate) async fn records(router: &MultiServiceRouter, leg: &Leg) -> Vec<Value> {
    let mut held = Vec::new();
    for key in keys_under(router, &leg.bucket, &leg.prefix).await {
        let fetched = call(
            router,
            "s3",
            &format!("s3.{REGION}.amazonaws.com"),
            "GET",
            &format!("/{}/{key}", leg.bucket),
            &[],
            bytes::Bytes::new(),
        )
        .await;
        assert_eq!(
            fetched.status, 200,
            "case {:?}: reading {key:?} must succeed, got {}",
            leg.case, fetched.status
        );
        if leg.gzipped {
            let parsed = logging::delivered_records(&fetched.body).unwrap_or_else(|| {
                panic!(
                    "case {:?}: the delivered object {key:?} is not a vended-log object, got {} \
                     byte(s)",
                    leg.case,
                    fetched.body.len()
                )
            });
            held.extend(parsed);
        } else {
            for line in fetched.text().lines() {
                if line.trim().is_empty() {
                    continue;
                }
                held.push(serde_json::from_str(line).unwrap_or_else(|error| {
                    panic!(
                        "case {:?}: a record of {key:?} is not JSON ({error}): {line:?}",
                        leg.case
                    )
                }));
            }
        }
    }
    held
}

/// Look at `leg`'s destination until it holds `wanted` records or [`DELIVERY_DEADLINE`] passes,
/// then answer whatever it holds. Waits on the CONDITION, never on a span.
pub(crate) async fn records_until(
    router: &MultiServiceRouter,
    leg: &Leg,
    wanted: usize,
) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let held = records(router, leg).await;
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// One viewer request of the table: the answer it must get, which is what its record states.
struct Request {
    case: &'static str,
    /// `sc-status` — the status the viewer was answered with.
    status: u16,
    /// `x-edge-result-type` — the first word of the answer's `X-Cache`.
    result_type: &'static str,
}

/// The two requests every destination must then hold a record of: the read the origin answers,
/// and the same read again, which the managed caching policy answers from the edge cache.
fn requests() -> [Request; 2] {
    [
        Request {
            case: "the read the origin answers",
            status: 200,
            result_type: "Miss",
        },
        Request {
            case: "the same read again, which the edge cache answers",
            status: 200,
            result_type: "Hit",
        },
    ]
}

#[tokio::test]
async fn each_served_request_reaches_every_delivery_destination_whatever_its_stream_buffers() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let setup = emulator(&engine, false);
    let viewers = emulator(&engine, true);
    let case = "the buffered pipeline";
    let account = engine::emulator_account_id();

    // The origin, and the bucket the S3 delivery destination names.
    create_bucket(&setup, case, ORIGIN_BUCKET).await;
    create_bucket(&setup, case, VENDED_BUCKET).await;
    put_object(&setup, case, ORIGIN_BUCKET, KEY, PAYLOAD, &[]).await;

    // The role every destination stream's own delivery is made under.
    iam(
        &setup,
        case,
        "CreateRole",
        &[
            ("RoleName", ROLE),
            (
                "AssumeRolePolicyDocument",
                &json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Principal": {"Service": "firehose.amazonaws.com"},
                        "Action": "sts:AssumeRole",
                    }],
                })
                .to_string(),
            ),
        ],
    )
    .await;
    iam(
        &setup,
        case,
        "PutRolePolicy",
        &[
            ("RoleName", ROLE),
            ("PolicyName", "deliver"),
            (
                "PolicyDocument",
                &json!({
                    "Version": "2012-10-17",
                    "Statement": [{
                        "Effect": "Allow",
                        "Action": "s3:PutObject",
                        "Resource": "*",
                    }],
                })
                .to_string(),
            ),
        ],
    )
    .await;

    // The distribution whose edge sends the records, over the private origin.
    let control =
        create_origin_access_control(&setup, case, &format!("{ORIGIN_BUCKET}-control"), "always")
            .await;
    let (id, domain, arn) = create_distribution(
        &setup,
        case,
        ORIGIN_BUCKET,
        ORIGIN_BUCKET,
        &control,
        CACHING_OPTIMIZED,
    )
    .await;
    put_bucket_policy(
        &setup,
        case,
        ORIGIN_BUCKET,
        &policy(
            CLOUDFRONT_PRINCIPAL,
            "s3:GetObject",
            &format!("arn:aws:s3:::{ORIGIN_BUCKET}/*"),
            Some(&format!(
                "{{\"StringEquals\":{{\"AWS:SourceArn\":\"{arn}\"}}}}"
            )),
        ),
    )
    .await;

    // The ONE delivery source every delivery below is paired with.
    let source = format!("{ORIGIN_BUCKET}-source");
    let (status, answered) = logs(
        &setup,
        "PutDeliverySource",
        &json!({"name": &source, "resourceArn": &arn, "logType": LOG_TYPE}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutDeliverySource must be accepted, got {status} {answered}"
    );

    // One delivery per leg: the bucket destination, and one Firehose destination per row.
    let mut legs: Vec<Leg> = vec![Leg {
        case: "the destination bucket",
        bucket: VENDED_BUCKET.to_string(),
        // The published object layout a vended-log delivery writes a bucket's files under.
        prefix: format!("AWSLogs/{account}/cloudfront/"),
        gzipped: true,
    }];
    let mut wiring: Vec<(&'static str, String, &'static str)> = vec![(
        "the destination bucket",
        format!("arn:aws:s3:::{VENDED_BUCKET}"),
        S3,
    )];
    for (ordinal, row) in rows().into_iter().enumerate() {
        let stream = format!("logv2-buffered-{ordinal}");
        let bucket = format!("logv2-buffered-stream-{ordinal}");
        let prefix = format!("from-the-stream-{ordinal}/");
        create_bucket(&setup, row.case, &bucket).await;
        let mut destination = json!({
            "RoleARN": format!("arn:aws:iam::{account}:role/{ROLE}"),
            "BucketARN": format!("arn:aws:s3:::{bucket}"),
            "Prefix": &prefix,
            "CompressionFormat": "UNCOMPRESSED",
        });
        if let Some((size, interval)) = row.hints {
            destination["BufferingHints"] = json!({
                "SizeInMBs": size,
                "IntervalInSeconds": interval,
            });
        }
        firehose(
            &setup,
            row.case,
            "CreateDeliveryStream",
            &json!({
                "DeliveryStreamName": &stream,
                "DeliveryStreamType": "DirectPut",
                "ExtendedS3DestinationConfiguration": destination,
            }),
        )
        .await;
        wiring.push((
            row.case,
            format!("arn:aws:firehose:{REGION}:{account}:deliverystream/{stream}"),
            FH,
        ));
        legs.push(Leg {
            case: row.case,
            bucket,
            prefix,
            gzipped: false,
        });
    }
    for (index, (leg_case, resource, kind)) in wiring.iter().enumerate() {
        let (status, answered) = logs(
            &setup,
            "PutDeliveryDestination",
            &json!({
                "name": format!("logv2-buffered-destination-{index}"),
                "outputFormat": OUTPUT_FORMAT,
                "deliveryDestinationConfiguration": {"destinationResourceArn": resource},
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {leg_case:?}: PutDeliveryDestination must be accepted, got {status} {answered}"
        );
        assert_eq!(
            answered["deliveryDestination"]["deliveryDestinationType"].as_str(),
            Some(*kind),
            "case {leg_case:?}: expected a {kind} destination, got {answered}"
        );
        let destination_arn = answered["deliveryDestination"]["arn"].clone();
        let (status, answered) = logs(
            &setup,
            "CreateDelivery",
            &json!({
                "deliverySourceName": &source,
                "deliveryDestinationArn": destination_arn,
                "recordFields": FIELDS,
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {leg_case:?}: CreateDelivery must be accepted, got {status} {answered}"
        );
    }

    // Every stream takes puts, and the distribution is deployed: the state the edge serves a
    // viewer request in, and the one its records can travel from.
    wait_until_deployed(&setup, case, &id).await;
    for ordinal in 0..rows().len() {
        let stream = format!("logv2-buffered-{ordinal}");
        let deadline = Instant::now() + DELIVERY_DEADLINE;
        loop {
            let described = firehose(
                &setup,
                &stream,
                "DescribeDeliveryStream",
                &json!({"DeliveryStreamName": &stream}),
            )
            .await;
            let status = described["DeliveryStreamDescription"]["DeliveryStreamStatus"].clone();
            if status == json!(ACTIVE) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {stream:?}: the destination stream must reach {ACTIVE}, last saw {status}"
            );
            ::tokio::time::sleep(POLL).await;
        }
    }

    // The traffic: the same object twice, so the two records differ in their result
    // classification.
    let table = requests();
    let mut request_ids: Vec<String> = Vec::new();
    for request in &table {
        let viewed = view(&viewers, &domain, &format!("/{KEY}")).await;
        assert_eq!(
            viewed.status,
            request.status,
            "case {:?}: expected the edge to answer {}, got {} with {:?}",
            request.case,
            request.status,
            viewed.status,
            viewed.text()
        );
        let cache = viewed.header("x-cache");
        assert!(
            cache.starts_with(request.result_type),
            "case {:?}: expected an X-Cache naming {:?}, got {cache:?}",
            request.case,
            request.result_type
        );
        let request_id = viewed.header("x-amz-cf-id");
        assert!(
            !request_id.is_empty(),
            "case {:?}: the edge answered no X-Amz-Cf-Id (status {})",
            request.case,
            viewed.status
        );
        request_ids.push(request_id);
    }
    let distinct: BTreeSet<&String> = request_ids.iter().collect();
    assert_eq!(
        distinct.len(),
        table.len(),
        "case {case:?}: expected one identifier per served request, got {request_ids:?}"
    );

    // Every destination holds one record per served request — the bucket at once, and each
    // stream's own destination once that stream's buffer is flushed, whatever window it asked
    // to hold its records for.
    for leg in &legs {
        let held = records_until(&setup, leg, table.len()).await;
        assert_eq!(
            held.len(),
            table.len(),
            "case {:?}: expected one record per served request ({}), got {held:?}",
            leg.case,
            table.len()
        );
        for (request, request_id) in table.iter().zip(request_ids.iter()) {
            let record = held
                .iter()
                .find(|record| record["x-edge-request-id"].as_str() == Some(request_id.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?} ({}): expected a record of the request the edge identified, \
                         got {held:?}",
                        leg.case, request.case
                    )
                });
            let fields: BTreeSet<&str> = record
                .as_object()
                .map(|fields| fields.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let wanted: BTreeSet<&str> = FIELDS.iter().copied().collect();
            assert_eq!(
                fields, wanted,
                "case {:?} ({}): expected exactly the delivery's record fields, got {record}",
                leg.case, request.case
            );
            for (field, expected) in [
                ("cs-uri-stem", format!("/{KEY}")),
                ("sc-status", request.status.to_string()),
                ("x-edge-result-type", request.result_type.to_string()),
            ] {
                assert_eq!(
                    record[field].as_str(),
                    Some(expected.as_str()),
                    "case {:?} ({}): expected {field} {expected:?}, got {record}",
                    leg.case,
                    request.case
                );
            }
        }
    }
}
