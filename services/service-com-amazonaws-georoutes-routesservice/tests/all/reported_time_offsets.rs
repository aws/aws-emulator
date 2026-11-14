// SPDX-License-Identifier: CC0-1.0

//! How this service STATES the times it reports.
//!
//! `TimestampWithTimezoneOffset` carries either a UTC instant or a local time with an
//! explicit UTC offset ("Time format: `YYYY-MM-DDThh:mm:ss.sssZ |
//! YYYY-MM-DDThh:mm:ss.sss+hh:mm`" on every timestamp member of this model), and the times
//! these operations report are the times the REQUEST pinned the calculation to. The rule
//! under test is therefore not "this recorded request gives this recorded timestamp" but:
//!
//! * a reported time is the same instant the request pinned, stated the way the request
//!   stated it — the offset it supplied, not that instant re-expressed in UTC;
//! * every time of one answer is stated at that same offset, including the ones the service
//!   derived by moving the clock forward;
//! * a time taken from this service's own clock was stated nowhere, so it is reported in
//!   UTC, in the form the model's pattern admits.
//!
//! The table spans what the rule generalises over: the operation reporting the time, which
//! member the request pinned it with, and the offset itself (UTC as `Z` and as an explicit
//! zero, east and west of UTC, a half-hour offset, both published hour bounds, and local
//! times whose UTC instant falls on another day or in another year).

use serde_json::{json, Value};

use crate::routing::{post, routes_service, OPTIMIZE, ROUTES};

/// The path `CalculateIsolines` is bound to.
const ISOLINES: &str = "/v2/isolines";

/// A position in mapped territory, where the road network has coverage.
const ORIGIN: [f64; 2] = [-123.1, 49.28];
/// A second one, near enough to be routed to over the same network.
const DESTINATION: [f64; 2] = [-123.12, 49.26];

/// How `timestamp` states its own UTC offset: the `[+-]hh:mm` the format's offset form ends
/// with, or `Z` for the UTC form — the two alternatives of the `TimestampWithTimezoneOffset`
/// pattern `(Z|[+-]([01][0-9]|2[0-3]):[0-5][0-9])$`.
fn designator_of(timestamp: &str) -> &str {
    let tail = &timestamp[timestamp.len() - 6..];
    if tail.starts_with('+') || tail.starts_with('-') {
        tail
    } else {
        "Z"
    }
}

/// Whether `value` is a timestamp of the form `TimestampWithTimezoneOffset` admits, with
/// whole seconds and no fractional part: `YYYY-MM-DDThh:mm:ss` followed by `Z` or by
/// `[+-]hh:mm`, as the model's pattern lays it out.
fn is_timestamp(value: &str) -> bool {
    let shape: &[u8] = match designator_of(value) {
        "Z" => b"dddd-dd-ddTdd:dd:ddZ",
        _ => b"dddd-dd-ddTdd:dd:ddsdd:dd",
    };
    value.len() == shape.len()
        && value.bytes().zip(shape).all(|(byte, expected)| match expected {
            b'd' => byte.is_ascii_digit(),
            b's' => byte == b'+' || byte == b'-',
            other => byte == *other,
        })
}

/// Every reported timestamp under `value`, found by walking the answer: the members a
/// response of this service carries a time in all end in `Time`.
fn reported_times(value: &Value, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(members) => {
            for (name, member) in members {
                match member.as_str() {
                    Some(text) if name.ends_with("Time") => out.push((name.clone(), text.to_string())),
                    _ => reported_times(member, out),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                reported_times(item, out);
            }
        }
        _ => {}
    }
}

/// The timestamps the table is driven with: the same wall-clock form stated at the offsets
/// the model's pattern admits. The two UTC forms and `+02:00` are the examples the
/// timestamp members' own documentation carries; `+00:00`, `+23:00` and `-00:30` sit at the
/// bounds that pattern allows (`([01][0-9]|2[0-3]):[0-5][0-9]`), and the rest are offsets
/// whose local time falls on a different UTC day or year.
const STATED: [&str; 8] = [
    "2024-04-22T17:57:24Z",
    "2024-04-22T17:57:24+00:00",
    "2024-04-22T17:57:24+02:00",
    "2026-09-18T11:23:08-07:00",
    "2025-01-01T00:00:00+05:30",
    "2025-12-31T23:59:59-11:30",
    "2024-02-29T12:00:00+23:00",
    "2024-02-29T12:00:00-00:30",
];

/// A reported time is the instant the request pinned, stated the way the request stated it:
/// the offset the request supplied is the offset the answer carries, for every operation of
/// this service that reports a time.
#[tokio::test]
async fn a_reported_time_is_stated_the_way_the_request_stated_it() {
    let router = routes_service();

    for stated in STATED {
        // An isoline from an origin reports the departure it was computed for.
        let case = &format!("an isoline departing at {stated}");
        let (status, answer, _) = post(
            &router,
            ISOLINES,
            &json!({
                "Origin": ORIGIN,
                "Thresholds": { "Time": [600] },
                "IsolineGranularity": { "MaxPoints": 31 },
                "DepartureTime": stated,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            answer["DepartureTime"].as_str(),
            Some(stated),
            "case {case:?}: the departure is reported as it was stated, got {:?}",
            answer["DepartureTime"]
        );

        // An isoline towards a destination reports the arrival it was computed for.
        let case = &format!("an isoline arriving at {stated}");
        let (status, answer, _) = post(
            &router,
            ISOLINES,
            &json!({
                "Destination": DESTINATION,
                "Thresholds": { "Distance": [2_000] },
                "IsolineGranularity": { "MaxPoints": 31 },
                "ArrivalTime": stated,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            answer["ArrivalTime"].as_str(),
            Some(stated),
            "case {case:?}: the arrival is reported as it was stated, got {:?}",
            answer["ArrivalTime"]
        );

        // A route's first leg departs at the instant the request pinned, and every other
        // time of the answer is the clock moved on from it.
        let case = &format!("a route departing at {stated}");
        let (status, answer, _) = post(
            &router,
            ROUTES,
            &json!({
                "Origin": ORIGIN,
                "Destination": DESTINATION,
                "Waypoints": [{ "Position": [-123.11, 49.27] }],
                "DepartureTime": stated,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            answer["Routes"][0]["Legs"][0]["VehicleLegDetails"]["Departure"]["Time"].as_str(),
            Some(stated),
            "case {case:?}: the first leg departs at the instant pinned, as it was stated, got {:?}",
            answer["Routes"][0]["Legs"][0]["VehicleLegDetails"]["Departure"]["Time"]
        );
        assert_same_offset(&answer, stated, case);

        // An optimized sequence leaves its first stop at the instant the request pinned.
        let case = &format!("an optimized sequence departing at {stated}");
        let (status, answer, _) = post(
            &router,
            OPTIMIZE,
            &json!({
                "Origin": ORIGIN,
                "Waypoints": [
                    { "Id": "a", "Position": [-123.11, 49.27], "ServiceDuration": 600 },
                    { "Id": "b", "Position": [-123.05, 49.25] }
                ],
                "DepartureTime": stated,
            }),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        assert_eq!(
            answer["OptimizedWaypoints"][0]["DepartureTime"].as_str(),
            Some(stated),
            "case {case:?}: the sequence leaves its first stop at the instant pinned, as it was \
             stated, got {:?}",
            answer["OptimizedWaypoints"][0]
        );
        assert_same_offset(&answer, stated, case);
    }
}

/// Every time of `answer` is a timestamp of the modelled form, stated at the offset
/// `stated` carries — including the ones the service derived by moving the clock forward
/// from it.
fn assert_same_offset(answer: &Value, stated: &str, case: &str) {
    let mut times = Vec::new();
    reported_times(answer, &mut times);
    assert!(
        !times.is_empty(),
        "case {case:?}: the answer must report the times of its journey, got {answer:?}"
    );
    for (member, time) in times {
        assert!(
            is_timestamp(&time),
            "case {case:?}: {member} must be a modelled timestamp, got {time:?}"
        );
        assert_eq!(
            designator_of(&time),
            designator_of(stated),
            "case {case:?}: {member} must be stated at the offset the request stated, got {time:?}"
        );
    }
}

/// A time this service read off its own clock was stated nowhere, so it is reported in UTC,
/// in the form the modelled pattern admits — for every operation that can be asked for one
/// without pinning an instant.
#[tokio::test]
async fn a_time_taken_from_the_clock_is_reported_in_utc() {
    let router = routes_service();

    // case, path, the request that pins no instant.
    let cases: [(&str, &str, Value); 4] = [
        (
            "an isoline that names no time",
            ISOLINES,
            json!({
                "Origin": ORIGIN,
                "Thresholds": { "Distance": [2_000] },
                "IsolineGranularity": { "MaxPoints": 31 },
            }),
        ),
        (
            "an isoline departing now",
            ISOLINES,
            json!({
                "Origin": ORIGIN,
                "Thresholds": { "Time": [600] },
                "IsolineGranularity": { "MaxPoints": 31 },
                "DepartNow": true,
            }),
        ),
        (
            "a route that names no time",
            ROUTES,
            json!({ "Origin": ORIGIN, "Destination": DESTINATION }),
        ),
        (
            "an optimized sequence that names no time",
            OPTIMIZE,
            json!({
                "Origin": ORIGIN,
                "Waypoints": [{ "Id": "a", "Position": [-123.11, 49.27] }],
            }),
        ),
    ];

    for (case, path, body) in cases {
        let (status, answer, _) = post(&router, path, &body).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} with {answer:?}");
        let mut times = Vec::new();
        reported_times(&answer, &mut times);
        assert!(
            !times.is_empty(),
            "case {case:?}: the answer must report the instant it was computed for, got {answer:?}"
        );
        for (member, time) in times {
            assert!(
                is_timestamp(&time) && designator_of(&time) == "Z",
                "case {case:?}: {member} came off the clock, so it is reported in UTC, got {time:?}"
            );
        }
    }
}
