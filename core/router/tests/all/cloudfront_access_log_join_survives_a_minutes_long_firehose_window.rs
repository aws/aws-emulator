// SPDX-License-Identifier: CC0-1.0

//! STANDARD LOGGING v2 into a Firehose delivery destination whose stream asks to hold its
//! records for MINUTES: the access-log record of each viewer request still reaches both
//! destinations of the pipeline, so a consumer can join the two on the edge request id.
//!
//! RULE: a Firehose destination's `BufferingHints` are hints rather than a contract — "these
//! options are treated as hints, and therefore Firehose might choose to use different values
//! when it is optimal" (`smithy_query(query="shape",shape="BufferingHints")`) — so the window
//! `IntervalInSeconds` asks for is an UPPER bound on the wait its records get, and a window
//! past the span this emulator holds a buffer for is improved on rather than waited out.
//! Everything downstream of a stream is therefore observable: a producer that writes through a
//! stream and then reads the destination back finds its records there, whichever window the
//! stream was configured with, and so does the consumer of a pipeline whose records pass
//! through one. A vended-log delivery is such a pipeline — `CreateDelivery` pairs "exactly one
//! delivery source and one delivery destination", and a delivery destination "can represent a
//! log group in CloudWatch Logs, an Amazon S3 bucket, a delivery stream in Firehose, or X-Ray"
//! — so every request a CloudFront distribution's edge serves becomes ONE record carrying
//! exactly the delivery's own `recordFields` in EVERY destination of its source, and the
//! records of one request carry the same `x-edge-request-id` ("an encrypted string that
//! uniquely identifies a request", the value the edge handed the viewer as `X-Amz-Cf-Id`) in
//! all of them, so the copy an S3 destination holds and the copy a Firehose destination's own
//! stream wrote are the same record.
//!
//! Only a cross-service suite can pin this: the sending side is CloudFront's edge over a
//! private Amazon S3 origin, the pipeline is CloudWatch Logs' own, and the Firehose leg lands
//! in Amazon S3 only once the stream behind it flushes, under the role IAM holds for it. The
//! sibling suite [`super::cloudfront_logging_v2_reaches_a_buffered_firehose_destination`] drives
//! the same pipeline over streams whose windows a case can wait out, and the firehose crate's
//! `buffering_hints` suite pins the window rule on a producer's own puts; neither states that a
//! record travelling a vended-log pipeline reaches a destination whose stream asked to hold it
//! for a minute or more. The emulator, the Firehose and IAM calls, the record reader and the
//! poll-until-held helper are that sibling's own rather than built again here.
//!
//! The rows span what the rule generalises over: the window the destination stream asks for —
//! a minute, the documented default stated explicitly, the member left out so that default
//! stands unstated, and the published maximum, all past the span this emulator holds a buffer
//! for — beside the `SizeInMBs` next to it, none of which three small records come near (the
//! published ranges are `SizeInMBs` 1..128 and `IntervalInSeconds` 0..900,
//! `smithy_query(query="shape", shape="SizeInMBs,IntervalInSeconds")`) — crossed with the three
//! ANSWERS a viewer request can get at this distribution's edge (the origin's object, the same
//! read again which the managed caching policy answers from the edge cache, and a key the
//! bucket does not hold, which the origin's grant does not let the edge list).
//!
//! No case sleeps a window out or reads the wall clock. The wait for the records is the one
//! limit these rows test, so it is bounded where the rule puts it: many times the span this
//! emulator holds a buffer for, and far inside the shortest window any row asks its stream for,
//! so a loaded host never fails a delivery the clock would have made and a destination that
//! delivers only at the end of the window it asked for fails its row. Every other wait here —
//! the stream reaching `ACTIVE`, the distribution reaching `Deployed` — is open-handed and
//! waits on the condition.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::cloudfront_apigateway_origin_edge::wait_until_deployed;
use super::cloudfront_logging_v2_reaches_a_buffered_firehose_destination::{
    emulator, firehose, iam, records, Leg, ACTIVE, CACHING_OPTIMIZED, FH, FIELDS, LOG_TYPE,
    OUTPUT_FORMAT, S3,
};
use super::cloudfront_oac_s3_origin_edge::{
    create_bucket, create_distribution, create_origin_access_control, policy, put_bucket_policy,
    put_object, view, CLOUDFRONT_PRINCIPAL, REGION,
};
use super::logs_delivery_source_cloudfront_resource::logs;

/// The origin bucket, the bucket the S3 delivery destination names, the key the origin holds
/// and the bytes it holds there.
const ORIGIN_BUCKET: &str = "logv2-held-origin";
const VENDED_BUCKET: &str = "logv2-held-vended";
const KEY: &str = "report.txt";
const PAYLOAD: &str = "the object the edge serves and logs";

/// How long a stream is polled for `ACTIVE` before the case fails, and how often it is looked
/// at again: far past the span the clock owes a created stream, so a loaded host never fails a
/// transition the clock would have made.
const ACTIVE_DEADLINE: Duration = Duration::from_secs(90);
const POLL: Duration = Duration::from_millis(50);

/// How long a destination is polled for the records its pipeline owes before the case fails.
///
/// This is the LIMIT the rows test, so it is the one wait here that is not open-handed: many
/// times the span this emulator holds a buffer for — so a loaded host never fails a delivery
/// the clock would have made — and far inside the shortest window any row asks its stream for,
/// so a destination that only delivers at the end of the window it asked for fails its row
/// instead of passing it slowly.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(20);

/// The role every destination stream's own Amazon S3 delivery is made under.
const ROLE: &str = "logv2-held-firehose";

/// One Firehose delivery destination of the pipeline: the window its stream asks to hold its
/// records for.
struct Row {
    case: &'static str,
    /// `BufferingHints` as `(SizeInMBs, IntervalInSeconds)`; `None` leaves the member out
    /// altogether, which is the documented defaults ("The default value is 5", "The default
    /// value is 300").
    hints: Option<(i64, i64)>,
}

/// The rows: every window a client can ask for that no consumer of the pipeline could wait out.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a window of a minute under the smallest size the member admits",
            hints: Some((1, 60)),
        },
        Row {
            case: "the documented defaults stated explicitly",
            hints: Some((5, 300)),
        },
        Row {
            case: "no BufferingHints member at all, so the documented default stands unstated",
            hints: None,
        },
        Row {
            case: "the longest window the member admits, under the largest size it admits",
            hints: Some((128, 900)),
        },
    ]
}

/// One viewer request of the table: the answer it must get, which is what its record states.
struct Request {
    case: &'static str,
    /// The path the viewer asks for, which is what `cs-uri-stem` states.
    path: &'static str,
    /// `sc-status` — the status the viewer was answered with.
    status: u16,
    /// `x-edge-result-type` — the first word of the answer's `X-Cache`.
    result_type: &'static str,
}

/// The three requests every destination must then hold a record of: the read the origin
/// answers, the same read again which the managed caching policy answers from the edge cache,
/// and a read of a key the bucket does not hold.
fn requests() -> [Request; 3] {
    [
        Request {
            case: "the read the origin answers",
            path: "/report.txt",
            status: 200,
            result_type: "Miss",
        },
        Request {
            case: "the same read again, which the edge cache answers",
            path: "/report.txt",
            status: 200,
            result_type: "Hit",
        },
        Request {
            case: "a key the bucket does not hold, which the grant does not let the edge list",
            path: "/absent.txt",
            status: 403,
            result_type: "Error",
        },
    ]
}

#[tokio::test]
async fn every_served_request_reaches_both_destinations_whatever_window_its_stream_asked_for() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let setup = emulator(&engine, false);
    let viewers = emulator(&engine, true);
    let case = "the held pipeline";
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

    // The distribution whose edge sends the records, over the private origin. The grant is
    // `s3:GetObject` alone, so a key the bucket does not hold is refused rather than disclosed.
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

    // The ONE delivery source every delivery below is paired with, which reports the
    // distribution it represents and the log type CloudFront sends.
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
    assert_eq!(
        (
            answered["deliverySource"]["service"].as_str(),
            answered["deliverySource"]["logType"].as_str(),
            answered["deliverySource"]["resourceArns"].as_array(),
        ),
        (
            Some("cloudfront"),
            Some(LOG_TYPE),
            Some(&vec![Value::String(arn.clone())]),
        ),
        "case {case:?}: expected a cloudfront {LOG_TYPE} source over {arn:?}, got {answered}"
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
    let table = rows();
    for (ordinal, row) in table.iter().enumerate() {
        let stream = format!("logv2-held-{ordinal}");
        let bucket = format!("logv2-held-stream-{ordinal}");
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
                "name": format!("logv2-held-destination-{index}"),
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
        assert_eq!(
            answered["delivery"]["deliveryDestinationType"].as_str(),
            Some(*kind),
            "case {leg_case:?}: expected the delivery to report a {kind} destination, got \
             {answered}"
        );
    }

    // Every stream takes puts, and the distribution is deployed: the state the edge serves a
    // viewer request in, and the one its records can travel from.
    wait_until_deployed(&setup, case, &id).await;
    for ordinal in 0..table.len() {
        await_active(&setup, &format!("logv2-held-{ordinal}")).await;
    }

    // The traffic: the object, the same read again, and a key the bucket does not hold.
    let served = requests();
    let mut request_ids: Vec<String> = Vec::new();
    for request in &served {
        let viewed = view(&viewers, &domain, request.path).await;
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
        served.len(),
        "case {case:?}: expected one identifier per served request, got {request_ids:?}"
    );

    // Every destination holds one record per served request — the bucket at once, and each
    // stream's own destination once that stream's buffer is flushed, whatever window it asked
    // to hold its records for.
    let mut joined: BTreeMap<&str, Vec<BTreeMap<String, String>>> = BTreeMap::new();
    for leg in &legs {
        let held = records_until(&setup, leg, served.len()).await;        assert_eq!(
            held.len(),
            served.len(),
            "case {:?}: expected one record per served request ({}), got {held:?}",
            leg.case,
            served.len()
        );
        for (request, request_id) in served.iter().zip(request_ids.iter()) {
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
                ("cs-uri-stem", request.path.to_string()),
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
            joined
                .entry(request.case)
                .or_default()
                .push(flattened(record));
        }
    }

    // And the copy each destination holds of one request is the SAME record, which is what a
    // consumer joining the two destinations on the edge request id reads.
    for request in &served {
        let copies = joined.remove(request.case).unwrap_or_default();
        assert_eq!(
            copies.len(),
            legs.len(),
            "case {:?}: expected the record of this request in every destination ({}), got \
             {copies:?}",
            request.case,
            legs.len()
        );
        let first = copies.first().cloned().unwrap_or_default();
        for (leg, copy) in legs.iter().zip(copies.iter()) {
            assert_eq!(
                *copy, first,
                "case {:?} ({}): every destination's copy of one request is the same record, \
                 got {copy:?} beside {first:?}",
                request.case, leg.case
            );
        }
    }
}

/// Look at `leg`'s destination until it holds `wanted` records or [`DELIVERY_DEADLINE`] passes,
/// then answer whatever it holds. Waits on the CONDITION, never on a span.
async fn records_until(router: &MultiServiceRouter, leg: &Leg, wanted: usize) -> Vec<Value> {
    let deadline = Instant::now() + DELIVERY_DEADLINE;
    loop {
        let held = records(router, leg).await;
        if held.len() >= wanted || Instant::now() >= deadline {
            return held;
        }
        ::tokio::time::sleep(POLL).await;
    }
}

/// One record as a plain map of its members, so the copies two destinations hold of one request
/// are compared as records rather than as the text each destination stores them in.
fn flattened(record: &Value) -> BTreeMap<String, String> {
    record
        .as_object()
        .map(|members| {
            members
                .iter()
                .map(|(field, value)| {
                    let held = value
                        .as_str()
                        .map_or_else(|| value.to_string(), str::to_string);
                    (field.clone(), held)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Poll `stream` until Firehose reports it `ACTIVE`, which is the state a put is taken in.
///
/// The wait is on the CONDITION, never on a span of the clock.
async fn await_active(router: &MultiServiceRouter, stream: &str) {
    let deadline = Instant::now() + ACTIVE_DEADLINE;
    loop {
        let described = firehose(
            router,
            stream,
            "DescribeDeliveryStream",
            &json!({"DeliveryStreamName": stream}),
        )
        .await;
        let status = described["DeliveryStreamDescription"]["DeliveryStreamStatus"].clone();
        if status == json!(ACTIVE) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {stream:?}: the destination stream must reach {ACTIVE}, last saw {status}"
        );
        ::tokio::time::sleep(POLL).await;
    }
}
