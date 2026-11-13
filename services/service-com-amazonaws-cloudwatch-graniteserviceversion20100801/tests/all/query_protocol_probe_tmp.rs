// SPDX-License-Identifier: CC0-1.0

//! What this service owes a call is the same whichever of its wire protocols the call arrives
//! in, and this suite probes the two it is reached by side by side on ONE emulator.
//!
//! The rule under test: **the protocol a request is served in is decided per REQUEST, from that
//! request's content type, and changes nothing else this service owes the call** — so a
//! form-encoded `awsQuery` call and an `application/x-amz-json-1.0` call interleaved on one
//! router are each answered in their own protocol (neither latches the other's), each is
//! counted into the `AWS/Usage` `CallCount` series this service publishes about its own API
//! ([`crate::usage_call_count`], which owns that rule), and each reads the same stored state.
//!
//! The rows differ along what the rule generalises over: the protocol of each call in the
//! sequence, the order they arrive in, whether the call is a read, a write or a listing, and
//! whether it succeeds or is refused.

use serde_json::{json, Value};

use crate::harness::{build_router, call, ok, query_call};
use crate::usage_call_count::counted_calls;

/// The media type a query client sends its form body under.
const FORM: &str = "application/x-www-form-urlencoded; charset=utf-8";

/// Which wire a call in the sequence below arrives on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    /// `awsJson1_0` over `POST /` with the RPC target header.
    Json,
    /// `awsQuery`: a form body whose `Action` names the operation.
    Query,
}

#[tokio::test]
async fn interleaved_calls_are_each_served_in_their_own_protocol_and_each_counted() {
    let router = build_router();

    // The sequence alternates and repeats the two wires so that neither the first call nor a
    // run of calls on one wire can decide how a later call on the other is served. Every entry
    // reads the same metric stream, created on the JSON wire below.
    let sequence = [
        Wire::Json,
        Wire::Query,
        Wire::Query,
        Wire::Json,
        Wire::Query,
        Wire::Json,
        Wire::Json,
        Wire::Query,
    ];

    ok(
        &router,
        "PutMetricStream",
        &crate::metric_streams::put_request("interleaved-stream", "json"),
    )
    .await;

    let mut json_calls = 0usize;
    let mut query_calls = 0usize;
    for (index, wire) in sequence.iter().enumerate() {
        let case = format!("call {index} on {wire:?}");
        match wire {
            Wire::Json => {
                json_calls += 1;
                let (status, body) = call(
                    &router,
                    "GetMetricStream",
                    &json!({"Name": "interleaved-stream"}),
                )
                .await;
                assert_eq!(
                    (status, body.get("Name").and_then(Value::as_str)),
                    (200, Some("interleaved-stream")),
                    "case {case:?}: expected the JSON wire to report the stored stream, got \
                     {status} with {body}"
                );
            }
            Wire::Query => {
                query_calls += 1;
                let (status, media_type, body) = query_call(
                    &router,
                    FORM,
                    &[],
                    "GetMetricStream",
                    &[("Name", "interleaved-stream")],
                )
                .await;
                assert_eq!(
                    (status, media_type.as_str()),
                    (200, "text/xml"),
                    "case {case:?}: expected the query wire to answer in its own protocol, got \
                     {status} {media_type:?} with {body}"
                );
                assert!(
                    body.contains("<GetMetricStreamResult>")
                        && body.contains("<Name>interleaved-stream</Name>"),
                    "case {case:?}: expected the query wire to report the stored stream, got \
                     {body}"
                );
            }
        }
    }

    // Every one of them was counted, on either wire: the call count is a property of the call
    // this service SERVED, not of the protocol it arrived in. The reads this assertion itself
    // makes are counted against their own operations, never against this one.
    #[allow(clippy::cast_precision_loss)]
    let expected = (json_calls + query_calls) as f64;
    let (counted, datapoints) = counted_calls(&router, "GetMetricStream").await;
    assert!(
        (counted - expected).abs() < f64::EPSILON,
        "expected the usage telemetry to hold {expected} calls of GetMetricStream \
         ({json_calls} on the JSON wire, {query_calls} on the query wire), got {counted}"
    );
    assert!(
        datapoints >= 1,
        "the counted calls must be reported in at least one datapoint, got {datapoints}"
    );
}

#[tokio::test]
async fn a_write_on_one_wire_is_read_back_on_the_other_and_both_are_counted() {
    let router = build_router();

    // (case, the wire the write arrives on, the stream it configures)
    let rows: [(&str, Wire, &str); 2] = [
        ("a write on the query wire", Wire::Query, "query-written"),
        ("a write on the JSON wire", Wire::Json, "json-written"),
    ];

    for (case, wire, name) in rows {
        match wire {
            Wire::Json => {
                ok(
                    &router,
                    "PutMetricStream",
                    &crate::metric_streams::put_request(name, "json"),
                )
                .await;
            }
            Wire::Query => {
                let (status, media_type, body) = query_call(
                    &router,
                    FORM,
                    &[],
                    "PutMetricStream",
                    &[
                        ("Name", name),
                        (
                            "FirehoseArn",
                            "arn:aws:firehose:us-east-1:000000000000:deliverystream/metrics",
                        ),
                        ("RoleArn", "arn:aws:iam::000000000000:role/MetricStreamRole"),
                        ("OutputFormat", "json"),
                    ],
                )
                .await;
                assert_eq!(
                    (status, media_type.as_str()),
                    (200, "text/xml"),
                    "case {case:?}: expected the write to be accepted on the query wire, got \
                     {status} {media_type:?} with {body}"
                );
            }
        }

        // Whichever wire configured it, the OTHER wire reports it: one store, two protocols.
        let read = ok(&router, "GetMetricStream", &json!({"Name": name})).await;
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case:?}: expected the JSON wire to report the stream, got {read}"
        );
        let (status, media_type, body) =
            query_call(&router, FORM, &[], "GetMetricStream", &[("Name", name)]).await;
        assert_eq!(
            (status, media_type.as_str()),
            (200, "text/xml"),
            "case {case:?}: expected the query wire to report the stream, got {status} with \
             {body}"
        );
        assert!(
            body.contains(&format!("<Name>{name}</Name>")),
            "case {case:?}: expected the query read to name the stream, got {body}"
        );

        // A listing on the query wire reports it too, so the write is in the store rather than
        // in the answer of the call that made it.
        let (status, _media_type, listed) =
            query_call(&router, FORM, &[], "ListMetricStreams", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the query listing to be answered, got {listed}"
        );
        assert!(
            listed.contains(&format!("<Name>{name}</Name>")),
            "case {case:?}: expected the query listing to report the stream, got {listed}"
        );
    }

    // Both writes were counted into the one series for the operation, whichever wire carried
    // them.
    let (counted, _datapoints) = counted_calls(&router, "PutMetricStream").await;
    assert!(
        (counted - 2.0).abs() < f64::EPSILON,
        "expected the usage telemetry to hold the two PutMetricStream calls (one per wire), \
         got {counted}"
    );
}
