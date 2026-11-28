// SPDX-License-Identifier: CC0-1.0

//! The destination-enablement wait, and then `GetTimeSeriesServiceStatistics`
//! period bucketing under a service selector, end to end over the wire.
//!
//! Every case drives the SAME front door the production router does (the shared
//! harness in [`crate::xray_wire`]), so the assertions are about HTTP responses
//! rather than internal state. The two rules under test are the two halves of
//! the client flow an instrumentation automation runs:
//!
//! * `GetTraceSegmentDestination` reports the settled status (`ACTIVE`) for an
//!   account whose destination is in effect — the account default before
//!   anything configures one, and the configured destination once the switch has
//!   been applied — so a caller polling for the settled status always gets there
//!   in a BOUNDED number of reads. That poll is the only completion signal the
//!   model publishes for the switch (`TraceSegmentDestinationStatus` is
//!   `[PENDING, ACTIVE]`, `smithy_query(query="shape",
//!   shape="TraceSegmentDestinationStatus")`), so a status stuck at the
//!   transitional value is a caller that waits forever.
//! * with the destination settled, requests submitted through
//!   `PutTraceSegments` are counted into the `Period`-wide aggregation period
//!   that contains each request's `start_time`: requests in different periods
//!   occupy DISTINCT datapoints whose `Timestamp`s are one period grid apart,
//!   requests inside one period share its datapoint, and a selector naming a
//!   service that recorded nothing yields no datapoint carrying
//!   `ServiceSummaryStatistics`.
//!
//! `time_series_service_statistics.rs` owns the per-class counting rules (how a
//! throttle, an error and a fault are classified) and the unselected, edge and
//! group surfaces; this suite is about the periods a service's clean requests
//! land in, which is what the enablement flow blocks on reaching.

use std::time::Duration;

use serde_json::{json, Value};

use service_com_amazonaws_xray_awsxray::ops::get_time_series_service_statistics::TIME_SERIES_PAGE_SIZE;
use service_com_amazonaws_xray_awsxray::ops::types::ADMITTED_TRACE_SEGMENT_DESTINATIONS;
use service_com_amazonaws_xray_awsxray::state::{
    CLOUD_WATCH_LOGS_DESTINATION, DEFAULT_TRACE_SEGMENT_DESTINATION, DESTINATION_STATUS_ACTIVE,
};

use crate::xray_wire::{array, close, root_document, trace_id, Answer, Xray};

/// The `/TimeSeriesServiceStatistics` URI this operation is bound to.
const PATH: &str = "/TimeSeriesServiceStatistics";

/// An hour-aligned epoch instant (`1_699_999_200 == 3600 * 472_222`), so every
/// period width a case names divides it and a case's expected `Timestamp`s are
/// the offsets it lists.
const BASE: f64 = 1_699_999_200.0;

/// How long each submitted request took, in seconds.
const DURATION: f64 = 1.0;

/// How long a caller waits between two reads while polling for the settled
/// status, and how many reads it is allowed. The product is far past any switch
/// this emulator leaves pending, so a poll that does not terminate inside it is
/// one that never would.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The number of reads the settle poll is allowed; a bound, so a status stuck at
/// the transitional value FAILS the case instead of hanging the suite.
const POLL_READS: usize = 64;

/// The number of pages the exhaustion walk is allowed, so a token that never
/// clears fails the case instead of looping.
const MAX_PAGES: usize = 8;

/// One clean completed request's segment document: a root segment of `service`
/// that recorded a 2xx status and declared no error or fault.
fn clean_document(n: usize, service: &str, start: f64) -> String {
    let mut segment = root_document(
        &format!("{n:016x}"),
        &trace_id(u32::try_from(n).expect("the submission index fits")),
        service,
        start,
        Some(start + DURATION),
    );
    segment["http"] = json!({
        "request": {"method": "GET", "url": "http://localhost/"},
        "response": {"status": 200},
    });
    segment.to_string()
}

/// `GetTraceSegmentDestination`.
async fn read_destination(xray: &Xray) -> Answer {
    xray.call("/GetTraceSegmentDestination", &json!({})).await
}

/// The wait half of the documented enablement flow: poll
/// `GetTraceSegmentDestination` until it reports the settled status, and answer
/// how many reads that took.
async fn poll_settled(case: &str, xray: &Xray, expected: &str) -> usize {
    for read in 1..=POLL_READS {
        let answered = read_destination(xray).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: read #{read} of the destination must answer 200, got {answered:?}"
        );
        if answered.get("Status").as_str() == Some(DESTINATION_STATUS_ACTIVE) {
            assert_eq!(
                answered.get("Destination").as_str(),
                Some(expected),
                "case {case:?}: the settled destination must be {expected:?}, got {answered:?}"
            );
            return read;
        }
        tokio::time::sleep(POLL_INTERVAL).await;
    }
    panic!(
        "case {case:?}: the destination never reported {DESTINATION_STATUS_ACTIVE} in \
         {POLL_READS} reads, so a caller polling for the settled status waits forever"
    );
}

/// The whole documented enablement flow: configure `destination`, then wait for
/// the switch to be in effect, and answer how many reads the wait took.
///
/// A `CloudWatchLogs` destination is one this service DELIVERS to, so the grant
/// that admits the account's spans into the log group they go to is put first —
/// the step AWS's own Transaction Search enablement flow starts with, and
/// without which the switch is refused.
async fn settle(case: &str, xray: &Xray, destination: &str) -> usize {
    if destination == CLOUD_WATCH_LOGS_DESTINATION {
        xray.grant_spans_delivery(case).await;
    }
    let updated = xray
        .call(
            "/UpdateTraceSegmentDestination",
            &json!({"Destination": destination}),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "case {case:?}: configuring {destination:?} must be accepted, got {updated:?}"
    );
    poll_settled(case, xray, destination).await
}

/// The selector naming one service's own requests, written the way the
/// enablement flow writes it — trailing space included, which narrows nothing.
fn service_selector(service: &str) -> String {
    format!("service(id(name: \"{service}\")) ")
}

/// Every `TimeSeriesServiceStatistics` entry the selector's aggregation holds,
/// paged to exhaustion.
async fn all_periods(
    case: &str,
    xray: &Xray,
    service: &str,
    window: (f64, f64),
    period: i64,
) -> Vec<Value> {
    let mut collected = Vec::new();
    let mut token: Option<String> = None;
    for page in 1..=MAX_PAGES {
        let mut body = json!({
            "StartTime": window.0,
            "EndTime": window.1,
            "Period": period,
            "EntitySelectorExpression": service_selector(service),
        });
        if let Some(token) = &token {
            body["NextToken"] = json!(token);
        }
        let answered = xray.call(PATH, &body).await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: page {page} must answer 200, got {answered:?}"
        );
        collected.extend(
            array(&answered.body, "TimeSeriesServiceStatistics")
                .iter()
                .cloned(),
        );
        match answered.get("NextToken").as_str() {
            Some(next) => token = Some(next.to_string()),
            None => return collected,
        }
    }
    panic!("case {case:?}: the aggregation still issued a NextToken after {MAX_PAGES} pages");
}

/// A member read as an `i64`, or `-1` so a missing count fails loudly against an
/// expected zero rather than matching it.
fn count(value: &Value, member: &str) -> i64 {
    value.get(member).and_then(Value::as_i64).unwrap_or(-1)
}

/// Assert one datapoint reports `requests` clean requests of the selected
/// service, at `timestamp`.
fn assert_datapoint(case: &str, datapoint: &Value, timestamp: f64, requests: i64) {
    let actual_timestamp = datapoint.get("Timestamp").and_then(Value::as_f64);
    assert!(
        actual_timestamp.is_some_and(|actual| close(actual, timestamp)),
        "case {case:?}: expected Timestamp {timestamp}, got {datapoint:?}"
    );
    // A selector naming a SERVICE asks about that service's own requests, which
    // `ServiceSummaryStatistics` reports.
    let Some(summary) = datapoint.get("ServiceSummaryStatistics") else {
        panic!("case {case:?}: a service selector's datapoint must carry ServiceSummaryStatistics, got {datapoint:?}");
    };
    let errors = summary.get("ErrorStatistics").cloned().unwrap_or(Value::Null);
    let faults = summary.get("FaultStatistics").cloned().unwrap_or(Value::Null);
    let actual = (
        count(summary, "TotalCount"),
        count(summary, "OkCount"),
        count(&errors, "TotalCount"),
        count(&errors, "ThrottleCount"),
        count(&errors, "OtherCount"),
        count(&faults, "TotalCount"),
        count(&faults, "OtherCount"),
    );
    assert_eq!(
        actual,
        (requests, requests, 0, 0, 0, 0, 0),
        "case {case:?}: expected (Total, Ok, ErrorTotal, Throttle, ErrorOther, FaultTotal, \
         FaultOther) ({requests}, {requests}, 0, 0, 0, 0, 0), got {datapoint:?}"
    );
    let response_time = summary.get("TotalResponseTime").and_then(Value::as_f64);
    #[allow(clippy::cast_precision_loss)]
    let expected_response_time = requests as f64 * DURATION;
    assert!(
        response_time.is_some_and(|actual| close(actual, expected_response_time)),
        "case {case:?}: expected TotalResponseTime {expected_response_time}, got {datapoint:?}"
    );
}

// =============================================================================
// 1. The enablement wait terminates, from the default and from every switch
// =============================================================================

#[tokio::test]
async fn polling_for_the_settled_destination_terminates() {
    // Rows: the account nothing configured, and a switch to each value the
    // destination type admits that is not the one already in effect — naming
    // that one applies no switch at all and is refused, which
    // `trace_segment_destination.rs` owns.
    let xray = Xray::new();
    let reads = poll_settled(
        "a never-configured account",
        &xray,
        DEFAULT_TRACE_SEGMENT_DESTINATION,
    )
    .await;
    assert_eq!(
        reads, 1,
        "case \"a never-configured account\": the steady-state default is already settled, so \
         the first read must report it, took {reads} reads"
    );

    for destination in ADMITTED_TRACE_SEGMENT_DESTINATIONS {
        if *destination == DEFAULT_TRACE_SEGMENT_DESTINATION {
            continue;
        }
        let case = format!("a switch to {destination}");
        let xray = Xray::with_cloud_watch_logs();
        settle(&case, &xray, destination).await;
        // And it stays settled: the next read of a settled account reports the
        // settled status straight away.
        let reads = poll_settled(&case, &xray, destination).await;
        assert_eq!(
            reads, 1,
            "case {case:?}: a settled account must report the settled status on the first \
             read, took {reads} reads"
        );
    }
}

// =============================================================================
// 2. A service's requests land in the period that contains each request's start
// =============================================================================

/// One bucketing case: the requests one service submits, and the datapoints the
/// aggregation must then report.
struct Case {
    case: &'static str,
    /// `Period`, in seconds.
    period: i64,
    /// When each request started, as an offset from [`BASE`] in seconds.
    starts: Vec<f64>,
    /// `(the datapoint's Timestamp as an offset from BASE, the requests it
    /// counts)`, in the order the aggregation reports them.
    expected: Vec<(f64, i64)>,
}

fn bucketing_cases() -> Vec<Case> {
    // The paging row: one request per period, over more periods than one page
    // carries, so the walk only sees them all by following the token to
    // exhaustion.
    let paged = TIME_SERIES_PAGE_SIZE + 2;
    #[allow(clippy::cast_precision_loss)]
    let paged_starts: Vec<f64> = (0..paged).map(|n| n as f64 * 60.0).collect();
    let paged_expected: Vec<(f64, i64)> = paged_starts.iter().map(|start| (*start, 1)).collect();

    vec![
        Case {
            case: "two requests two periods apart occupy two datapoints",
            period: 60,
            starts: vec![0.0, 120.0],
            expected: vec![(0.0, 1), (120.0, 1)],
        },
        Case {
            case: "two requests a second either side of a period boundary occupy adjacent datapoints",
            period: 60,
            starts: vec![30.0, 91.0],
            expected: vec![(0.0, 1), (60.0, 1)],
        },
        Case {
            case: "two requests inside one period share its datapoint",
            period: 60,
            starts: vec![10.0, 59.5],
            expected: vec![(0.0, 2)],
        },
        Case {
            case: "a period wider than the gap merges what a narrower one separated",
            period: 300,
            starts: vec![0.0, 120.0],
            expected: vec![(0.0, 2)],
        },
        Case {
            // The periods a statistics read serves are the one-minute and
            // five-minute aggregations, so separation is shown at the wider of
            // the two as well as at the narrower one above.
            case: "the five-minute period separates requests five minutes apart",
            period: 300,
            starts: vec![0.0, 300.0],
            expected: vec![(0.0, 1), (300.0, 1)],
        },
        Case {
            case: "three requests in three periods, two of them in the same one",
            period: 60,
            starts: vec![5.0, 65.0, 95.0, 185.0],
            expected: vec![(0.0, 1), (60.0, 2), (180.0, 1)],
        },
        Case {
            case: "more periods than one page carries, paged to exhaustion",
            period: 60,
            starts: paged_starts,
            expected: paged_expected,
        },
    ]
}

#[tokio::test]
async fn buckets_a_service_s_requests_into_period_wide_datapoints() {
    for (row, case) in bucketing_cases().into_iter().enumerate() {
        let label = case.case;
        let xray = Xray::with_cloud_watch_logs();
        // The flow a client runs: grant the delivery, wait for the destination
        // to settle, then instrument. A service name per row, so one row's
        // traffic is never another's.
        settle(label, &xray, CLOUD_WATCH_LOGS_DESTINATION).await;
        let service = format!("checkout-{row}");

        for (n, start) in case.starts.iter().enumerate() {
            // One call per request, as an instrumented client submits them.
            let put = xray
                .put_segments(&[clean_document(row * 100 + n, &service, BASE + start)])
                .await;
            assert_eq!(
                (put.status, array(&put.body, "UnprocessedTraceSegments").len()),
                (200, 0),
                "case {label:?}: submission #{n} must be accepted whole, got {put:?}"
            );
        }

        let last = case
            .starts
            .iter()
            .copied()
            .fold(f64::NEG_INFINITY, f64::max);
        let window = (BASE - 1.0, BASE + last + 10.0);
        let periods = all_periods(label, &xray, &service, window, case.period).await;
        assert_eq!(
            periods.len(),
            case.expected.len(),
            "case {label:?}: expected {} datapoints, got {periods:?}",
            case.expected.len()
        );
        for (datapoint, (offset, requests)) in periods.iter().zip(&case.expected) {
            assert_datapoint(label, datapoint, BASE + offset, *requests);
            let timestamp = datapoint
                .get("Timestamp")
                .and_then(Value::as_f64)
                .unwrap_or(f64::NAN);
            assert!(
                timestamp >= window.0 && timestamp < window.1,
                "case {label:?}: every datapoint's Timestamp must fall inside the requested \
                 window {window:?}, got {timestamp}"
            );
        }
        // Consecutive datapoints sit on the period grid, so the gap between two
        // of them is a whole number of periods — and the two submissions a
        // period apart are a period apart in the answer.
        for pair in periods.windows(2) {
            let first = pair[0].get("Timestamp").and_then(Value::as_f64);
            let second = pair[1].get("Timestamp").and_then(Value::as_f64);
            #[allow(clippy::cast_precision_loss)]
            let period = case.period as f64;
            assert!(
                first.zip(second).is_some_and(|(first, second)| {
                    let gap = second - first;
                    gap >= period && close(gap % period, 0.0)
                }),
                "case {label:?}: consecutive Timestamps must be a whole number of \
                 {period}-second periods apart, got {first:?} and {second:?}"
            );
        }

        // Reporting, not mutating: the same question answers the same thing.
        let again = all_periods(label, &xray, &service, window, case.period).await;
        assert_eq!(
            again, periods,
            "case {label:?}: the aggregation is recomputed from the stored segments, so a \
             second identical call must answer identically, got {again:?}"
        );

        // A selector naming a service that submitted nothing counts nothing, so
        // no datapoint of it carries ServiceSummaryStatistics.
        let absent = format!("{service}-never-submitted");
        let none = all_periods(label, &xray, &absent, window, case.period).await;
        let carrying: Vec<&Value> = none
            .iter()
            .filter(|datapoint| datapoint.get("ServiceSummaryStatistics").is_some())
            .collect();
        assert!(
            carrying.is_empty(),
            "case {label:?}: the selector {absent:?} names no recorded service, so no \
             datapoint may carry ServiceSummaryStatistics, got {carrying:?}"
        );
    }
}

// =============================================================================
// 3. The same rule on instants the live clock produces
// =============================================================================

/// The rule the aggregation buckets by, stated here from `Period`'s own
/// documentation ("Aggregation period in seconds"): the period containing an
/// instant starts at the multiple of `period` at or below it, so the same
/// instant falls in the same period whatever window asks about it.
fn period_containing(seconds: f64, period: f64) -> f64 {
    (seconds / period).floor() * period
}

#[tokio::test]
async fn the_rule_holds_for_times_the_live_clock_produces() {
    // An instrumented client stamps its segments from the wall clock, which no
    // period boundary is aligned to, and the window it asks about is not
    // aligned either — so this case differs from the table above in exactly
    // that: the times are arbitrary rather than chosen.
    const PERIOD: f64 = 60.0;
    /// More than one period, so the two requests cannot share a period, and a
    /// whole number of them, so their datapoints are this far apart.
    const GAP: f64 = 120.0;

    let case = "two live-clock submissions a whole number of periods apart";
    let xray = Xray::with_cloud_watch_logs();
    settle(case, &xray, CLOUD_WATCH_LOGS_DESTINATION).await;
    let service = "live-clock-checkout";

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs_f64();
    let starts = [now - GAP, now];
    for (n, start) in starts.iter().enumerate() {
        let put = xray
            .put_segments(&[clean_document(900 + n, service, *start)])
            .await;
        assert_eq!(
            (put.status, array(&put.body, "UnprocessedTraceSegments").len()),
            (200, 0),
            "case {case:?}: submission #{n} must be accepted whole, got {put:?}"
        );
    }

    // A window with a period of margin at each end, as an automation asking
    // about what it just submitted writes it.
    let window = (starts[0] - PERIOD, starts[1] + PERIOD);
    #[allow(clippy::cast_possible_truncation)]
    let period = PERIOD as i64;
    let periods = all_periods(case, &xray, service, window, period).await;
    assert_eq!(
        periods.len(),
        2,
        "case {case:?}: two requests {GAP} seconds apart cannot share a {PERIOD}-second \
         period, so expected 2 datapoints, got {periods:?}"
    );
    for (datapoint, start) in periods.iter().zip(starts) {
        assert_datapoint(case, datapoint, period_containing(start, PERIOD), 1);
        let timestamp = datapoint
            .get("Timestamp")
            .and_then(Value::as_f64)
            .unwrap_or(f64::NAN);
        assert!(
            timestamp >= window.0 && timestamp < window.1,
            "case {case:?}: every datapoint's Timestamp must fall inside the requested \
             window {window:?}, got {timestamp}"
        );
    }
    let gap = periods[1].get("Timestamp").and_then(Value::as_f64).zip(
        periods[0]
            .get("Timestamp")
            .and_then(Value::as_f64),
    );
    assert!(
        gap.is_some_and(|(second, first)| close(second - first, GAP)),
        "case {case:?}: the datapoints must be as far apart as the submissions ({GAP} \
         seconds), got {periods:?}"
    );
}
