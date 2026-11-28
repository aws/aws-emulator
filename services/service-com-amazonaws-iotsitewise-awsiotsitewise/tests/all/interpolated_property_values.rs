// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetInterpolatedAssetPropertyValues`, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: the operation reports one value per instant of the
//! interval grid anchored at the request's start — `start + k * interval` for
//! `k >= 0`, so the start instant is itself on the grid — up to the last instant at
//! or before the inclusive end, each computed from the stored readings of the named
//! property that carry the requested quality and fall in that instant's query
//! window: linearly between the readings around the instant, or by carrying the
//! last reading at or before it forward. An instant whose window holds no such
//! readings gets NO value.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The wire harness this crate shares: `super::asset_lifecycle` is its harness
// module and `super::support` holds the clock helpers every property-value suite
// uses, so this suite adds no third copy of either.
use super::asset_lifecycle::{
    call, create_asset, create_model, error_code, property_id, router, router_with_engine,
};
use super::support::{anchored, relative};

/// The two `InterpolationType` values the model documents, from
/// `smithy_query(query="doc", shape="GetInterpolatedAssetPropertyValuesRequest",
/// member="type")`: "Valid values: LINEAR_INTERPOLATION | LOCF_INTERPOLATION".
const LINEAR: &str = "LINEAR_INTERPOLATION";
const LOCF: &str = "LOCF_INTERPOLATION";

/// How close a computed double must be to the oracle's value. Linear interpolation
/// is floating-point arithmetic, so an exact comparison would pin the rounding of
/// one expression rather than the rule.
const TOLERANCE: f64 = 1e-9;

/// The second this suite's tables measure their instants from.
///
/// `BatchPutAssetPropertyValue` ingests only timestamps inside
/// `[now - 7 days, now + 10 minutes]`, so the tables below keep the readable
/// instants they were written against and convert at the wire boundary with
/// [`anchored`] (on the way out) and [`relative`] (on the way back), exactly as
/// `super::support` documents.
const ROW_EPOCH: i64 = 1_000;

/// One asset carrying a measurement property per data type an interpolation can
/// meet, created and read back so every property id comes from the store.
/// Returns `(assetId, |name| propertyId)`.
async fn asset_with_properties(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let model_id = create_model(
        router,
        &format!("{name}Model"),
        json!([
            {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {"name": "Cycles", "dataType": "INTEGER", "type": {"measurement": {}}},
            {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
            {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(router, name, &model_id).await;
    let (status, asset) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset {name}: got {status} {asset}");
    (asset_id, asset["assetProperties"].clone())
}

/// One stored reading: `(timeInSeconds, offsetInNanos, value, quality)`.
type Reading<'a> = (i64, i32, Value, Option<&'a str>);

/// Write `readings` to a property through `BatchPutAssetPropertyValue`, so every
/// sample the interpolation reads got there through the service's own write path.
async fn put_readings(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    readings: &[Reading<'_>],
) {
    for (at, (seconds, nanos, value, quality)) in readings.iter().enumerate() {
        let mut reading = json!({
            "value": value,
            "timestamp": {"timeInSeconds": anchored(ROW_EPOCH, *seconds), "offsetInNanos": nanos},
        });
        if let Some(quality) = quality {
            reading["quality"] = json!(quality);
        }
        let (status, put) = call(
            router,
            "POST",
            "/properties",
            Some(json!({"entries": [{
                "entryId": format!("e{at}"),
                "assetId": asset_id,
                "propertyId": property,
                "propertyValues": [reading],
            }]})),
        )
        .await;
        assert_eq!(status, 200, "write {at}: got {status} {put}");
        assert_eq!(
            put["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "write {at} must be accepted, got {put}"
        );
    }
}

/// The query members of one `GetInterpolatedAssetPropertyValues` request, as the
/// model binds them (every one is an `httpQuery` member of
/// `GET /properties/interpolated`).
///
/// `Clone` so one request can be sent twice — once as it stands and once carrying a
/// `nextToken` — without spelling its members out again.
#[derive(Clone)]
struct Request<'a> {
    asset_id: &'a str,
    property_id: &'a str,
    /// `(startTimeInSeconds, startTimeOffsetInNanos)`.
    start: (i64, i32),
    /// `(endTimeInSeconds, endTimeOffsetInNanos)`.
    end: (i64, i32),
    interval: i64,
    kind: &'a str,
    quality: &'a str,
    window: Option<i64>,
    max_results: Option<i32>,
    next_token: Option<String>,
}

/// A linear-interpolation request over 1000..1100 at 10-second intervals, which
/// each row below varies along the one member it is about.
fn base<'a>(asset_id: &'a str, property_id: &'a str) -> Request<'a> {
    Request {
        asset_id,
        property_id,
        start: (1_000, 0),
        end: (1_100, 0),
        interval: 10,
        kind: LINEAR,
        quality: "GOOD",
        window: None,
        max_results: None,
        next_token: None,
    }
}

impl Request<'_> {
    /// This request as a URI.
    fn uri(&self) -> String {
        let mut query = vec![
            format!("assetId={}", self.asset_id),
            format!("propertyId={}", self.property_id),
            format!("startTimeInSeconds={}", anchored(ROW_EPOCH, self.start.0)),
            format!("startTimeOffsetInNanos={}", self.start.1),
            format!("endTimeInSeconds={}", anchored(ROW_EPOCH, self.end.0)),
            format!("endTimeOffsetInNanos={}", self.end.1),
            format!("intervalInSeconds={}", self.interval),
            format!("quality={}", self.quality),
            format!("type={}", self.kind),
        ];
        if let Some(window) = self.window {
            query.push(format!("intervalWindowInSeconds={window}"));
        }
        if let Some(max_results) = self.max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(token) = self.next_token.as_deref() {
            query.push(format!("nextToken={token}"));
        }
        format!("/properties/interpolated?{}", query.join("&"))
    }
}

/// The instant of one reported point: `(timeInSeconds, offsetInNanos)`.
type At = (i64, i64);

/// One row of a table of double-valued expectations: the case name, the request it
/// sends and the points that request must report.
type DoubleRows<'a> = Vec<(&'a str, Request<'a>, Vec<(At, f64)>)>;

/// The same for a table of integer-valued expectations.
type IntegerRows<'a> = Vec<(&'a str, Request<'a>, Vec<(At, i64)>)>;

/// The `((timeInSeconds, offsetInNanos), value)` of every point in a response.
fn points(body: &Value) -> Vec<(At, Value)> {
    body["interpolatedAssetPropertyValues"]
        .as_array()
        .unwrap_or_else(|| panic!("no interpolatedAssetPropertyValues in {body}"))
        .iter()
        .map(|point| {
            (
                (
                    point["timestamp"]["timeInSeconds"]
                        .as_i64()
                        .map_or(-1, |seconds| relative(ROW_EPOCH, seconds)),
                    point["timestamp"]["offsetInNanos"].as_i64().unwrap_or(0),
                ),
                point["value"].clone(),
            )
        })
        .collect()
}

/// The points of a response whose values are doubles, as
/// `((timeInSeconds, offsetInNanos), doubleValue)`.
fn doubles(body: &Value) -> Vec<(At, f64)> {
    points(body)
        .into_iter()
        .map(|(timestamp, value)| {
            let double = value["doubleValue"]
                .as_f64()
                .unwrap_or_else(|| panic!("point at {timestamp:?} has no doubleValue: {value}"));
            (timestamp, double)
        })
        .collect()
}

/// Assert that `body` carries exactly `expected`, naming `case` and the actual
/// points on every failure.
fn assert_doubles(case: &str, body: &Value, expected: &[(At, f64)]) {
    let actual = doubles(body);
    assert_eq!(
        actual.len(),
        expected.len(),
        "case {case:?}: expected {} points {expected:?}, got {actual:?}",
        expected.len()
    );
    for (at, ((timestamp, value), (want_timestamp, want_value))) in
        actual.iter().zip(expected.iter()).enumerate()
    {
        assert_eq!(
            timestamp, want_timestamp,
            "case {case:?}: point {at} expected timestamp {want_timestamp:?}, got {actual:?}"
        );
        assert!(
            (value - want_value).abs() < TOLERANCE,
            "case {case:?}: point {at} expected {want_value}, got {value} (all: {actual:?})"
        );
    }
}

#[tokio::test]
async fn linear_interpolation_reports_a_value_at_every_interval_instant() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine1").await;
    let temperature = property_id(&properties, "Temperature");
    // Two readings 100 seconds apart, rising by exactly 1.0 per second, so the
    // oracle for any instant between them is "seconds past 1000".
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 0.0}), Some("GOOD")),
            (1_100, 0, json!({"doubleValue": 100.0}), Some("GOOD")),
        ],
    )
    .await;

    // Rows differ in interval size, in the start's nanosecond offset and in how the
    // range sits around the readings. In every one the values land on the interval
    // grid anchored at the start — `start + k * interval` for k >= 0 — up to the last
    // instant at or before the inclusive end.
    let rows: DoubleRows = vec![
        (
            "half-range interval",
            Request {
                interval: 50,
                ..base(&asset_id, &temperature)
            },
            vec![((1_000, 0), 0.0), ((1_050, 0), 50.0), ((1_100, 0), 100.0)],
        ),
        (
            "quarter-range interval",
            Request {
                interval: 25,
                ..base(&asset_id, &temperature)
            },
            vec![
                ((1_000, 0), 0.0),
                ((1_025, 0), 25.0),
                ((1_050, 0), 50.0),
                ((1_075, 0), 75.0),
                ((1_100, 0), 100.0),
            ],
        ),
        (
            "interval that does not divide the range",
            Request {
                interval: 30,
                end: (1_090, 0),
                ..base(&asset_id, &temperature)
            },
            vec![
                ((1_000, 0), 0.0),
                ((1_030, 0), 30.0),
                ((1_060, 0), 60.0),
                ((1_090, 0), 90.0),
            ],
        ),
        (
            // The offset carries through to every interval instant, and the one after
            // the last (1100.5) is past the inclusive end (1100.0).
            "start offset by half a second",
            Request {
                start: (1_000, 500_000_000),
                interval: 25,
                ..base(&asset_id, &temperature)
            },
            vec![
                ((1_000, 500_000_000), 0.5),
                ((1_025, 500_000_000), 25.5),
                ((1_050, 500_000_000), 50.5),
                ((1_075, 500_000_000), 75.5),
            ],
        ),
        (
            // "If not specified, the window ranges between the start time minus the
            // interval and the end time plus the interval" — here [1040, 1110], which
            // excludes the reading at 1000, so only the instant that lands exactly on
            // the reading at 1100 has anything to interpolate from.
            "default window excludes the earlier reading",
            Request {
                start: (1_050, 0),
                interval: 10,
                ..base(&asset_id, &temperature)
            },
            vec![((1_100, 0), 100.0)],
        ),
        (
            // A range of zero length still holds one instant: its start.
            "end equal to the start",
            Request {
                end: (1_000, 0),
                ..base(&asset_id, &temperature)
            },
            vec![((1_000, 0), 0.0)],
        ),
        (
            "interval wider than the range",
            Request {
                end: (1_005, 0),
                interval: 10,
                ..base(&asset_id, &temperature)
            },
            vec![((1_000, 0), 0.0)],
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: got {status} {body} for {}",
            request.uri()
        );
        assert_doubles(case, &body, &expected);
        assert!(
            body["nextToken"].is_null(),
            "case {case:?}: a complete page carries no nextToken, got {body}"
        );
    }
}

#[tokio::test]
async fn locf_interpolation_carries_the_last_reading_forward() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine2").await;
    let temperature = property_id(&properties, "Temperature");
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 10.0}), Some("GOOD")),
            (1_010, 0, json!({"doubleValue": 20.0}), Some("GOOD")),
        ],
    )
    .await;

    // Rows differ in interval size and in where the range starts relative to the
    // first reading. In every one each interval instant reports the last reading at
    // or before it, and an instant with no reading before it reports nothing.
    let rows: DoubleRows = vec![
        (
            "carried forward past the last reading",
            Request {
                kind: LOCF,
                interval: 5,
                end: (1_030, 0),
                ..base(&asset_id, &temperature)
            },
            vec![
                ((1_000, 0), 10.0),
                ((1_005, 0), 10.0),
                ((1_010, 0), 20.0),
                ((1_015, 0), 20.0),
                ((1_020, 0), 20.0),
                ((1_025, 0), 20.0),
                ((1_030, 0), 20.0),
            ],
        ),
        (
            "instants before the first reading report nothing",
            Request {
                kind: LOCF,
                interval: 5,
                start: (980, 0),
                end: (1_005, 0),
                ..base(&asset_id, &temperature)
            },
            vec![((1_000, 0), 10.0), ((1_005, 0), 10.0)],
        ),
        (
            "an interval wider than the gap between the readings",
            Request {
                kind: LOCF,
                interval: 20,
                end: (1_040, 0),
                ..base(&asset_id, &temperature)
            },
            vec![
                ((1_000, 0), 10.0),
                ((1_020, 0), 20.0),
                ((1_040, 0), 20.0),
            ],
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_doubles(case, &body, &expected);
    }

    // Carrying a reading forward copies it, so it is defined for a data type no
    // arithmetic applies to — the doc's own example is an on-off valve.
    let running = property_id(&properties, "Running");
    put_readings(
        &router,
        &asset_id,
        &running,
        &[
            (1_000, 0, json!({"booleanValue": true}), Some("GOOD")),
            (1_020, 0, json!({"booleanValue": false}), Some("GOOD")),
        ],
    )
    .await;
    let request = Request {
        kind: LOCF,
        interval: 10,
        end: (1_030, 0),
        ..base(&asset_id, &running)
    };
    let (status, body) = call(&router, "GET", &request.uri(), None).await;
    assert_eq!(status, 200, "boolean LOCF: got {status} {body}");
    let expected = [
        ((1_000, 0), true),
        ((1_010, 0), true),
        ((1_020, 0), false),
        ((1_030, 0), false),
    ];
    let actual = points(&body);
    assert_eq!(
        actual.len(),
        expected.len(),
        "boolean LOCF: expected {expected:?}, got {actual:?}"
    );
    for (at, ((timestamp, value), (want_timestamp, want_value))) in
        actual.iter().zip(expected.iter()).enumerate()
    {
        assert_eq!(
            (timestamp, value["booleanValue"].as_bool()),
            (want_timestamp, Some(*want_value)),
            "boolean LOCF: point {at} expected {want_timestamp:?} {want_value}, got {actual:?}"
        );
    }

    // And a string reading is carried forward unchanged too.
    let status_property = property_id(&properties, "Status");
    put_readings(
        &router,
        &asset_id,
        &status_property,
        &[
            (1_000, 0, json!({"stringValue": "idle"}), Some("GOOD")),
            (1_015, 0, json!({"stringValue": "spinning"}), Some("GOOD")),
        ],
    )
    .await;
    let request = Request {
        kind: LOCF,
        interval: 10,
        end: (1_030, 0),
        ..base(&asset_id, &status_property)
    };
    let (status, body) = call(&router, "GET", &request.uri(), None).await;
    assert_eq!(status, 200, "string LOCF: got {status} {body}");
    let actual: Vec<((i64, i64), Option<String>)> = points(&body)
        .into_iter()
        .map(|(timestamp, value)| {
            (
                timestamp,
                value["stringValue"].as_str().map(str::to_string),
            )
        })
        .collect();
    assert_eq!(
        actual,
        vec![
            ((1_000, 0), Some("idle".to_string())),
            ((1_010, 0), Some("idle".to_string())),
            ((1_020, 0), Some("spinning".to_string())),
            ((1_030, 0), Some("spinning".to_string())),
        ],
        "string LOCF: expected idle then spinning carried forward, got {actual:?}"
    );
}

#[tokio::test]
async fn an_interval_window_bounds_the_readings_each_interval_may_use() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine3").await;
    let temperature = property_id(&properties, "Temperature");
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 10.0}), Some("GOOD")),
            (1_010, 0, json!({"doubleValue": 20.0}), Some("GOOD")),
            (1_200, 0, json!({"doubleValue": 100.0}), Some("GOOD")),
        ],
    )
    .await;

    // "IoT SiteWise computes each interpolated value by using data points from the
    // timestamp of each interval, minus the window to the timestamp of each interval
    // plus the window", and "if a data point isn't found during the specified query
    // window, IoT SiteWise won't return an interpolated value for the interval".
    // Rows differ in window width relative to the gaps in the series.
    let rows: DoubleRows = vec![
        (
            "window narrower than the gap after the readings",
            Request {
                interval: 10,
                end: (1_030, 0),
                window: Some(2),
                ..base(&asset_id, &temperature)
            },
            vec![((1_000, 0), 10.0), ((1_010, 0), 20.0)],
        ),
        (
            "window reaching one reading on each side",
            Request {
                start: (995, 0),
                interval: 10,
                end: (1_015, 0),
                window: Some(5),
                ..base(&asset_id, &temperature)
            },
            vec![((1_005, 0), 15.0)],
        ),
        (
            "window spanning the whole series",
            Request {
                interval: 100,
                end: (1_200, 0),
                window: Some(200),
                ..base(&asset_id, &temperature)
            },
            vec![
                ((1_000, 0), 10.0),
                ((1_100, 0), 20.0 + 80.0 * (90.0 / 190.0)),
                ((1_200, 0), 100.0),
            ],
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_doubles(case, &body, &expected);
    }
}

#[tokio::test]
async fn only_readings_of_the_requested_quality_are_interpolated() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine4").await;
    let temperature = property_id(&properties, "Temperature");
    // One reading per `Quality` value, from
    // `smithy_query(query="shape", shape="Quality")`: GOOD | BAD | UNCERTAIN.
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 10.0}), Some("GOOD")),
            (1_005, 0, json!({"doubleValue": 99.0}), Some("BAD")),
            (1_007, 0, json!({"doubleValue": 50.0}), Some("UNCERTAIN")),
            (1_010, 0, json!({"doubleValue": 20.0}), Some("GOOD")),
        ],
    )
    .await;

    let rows: DoubleRows = vec![
        (
            // The BAD reading at 1005 is ignored: 1005 is interpolated between the
            // two GOOD readings instead.
            "GOOD",
            Request {
                quality: "GOOD",
                interval: 5,
                end: (1_010, 0),
                ..base(&asset_id, &temperature)
            },
            vec![((1_000, 0), 10.0), ((1_005, 0), 15.0), ((1_010, 0), 20.0)],
        ),
        (
            // Only the BAD reading is in play, so the interval that lands on it
            // reports it and the one past it has nothing to interpolate towards.
            "BAD",
            Request {
                quality: "BAD",
                interval: 5,
                end: (1_010, 0),
                ..base(&asset_id, &temperature)
            },
            vec![((1_005, 0), 99.0)],
        ),
        (
            "UNCERTAIN",
            Request {
                quality: "UNCERTAIN",
                interval: 7,
                end: (1_014, 0),
                ..base(&asset_id, &temperature)
            },
            vec![((1_007, 0), 50.0)],
        ),
        (
            "UNCERTAIN, no interval landing on the reading",
            Request {
                quality: "UNCERTAIN",
                interval: 5,
                end: (1_010, 0),
                ..base(&asset_id, &temperature)
            },
            vec![],
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_doubles(case, &body, &expected);
    }
}

#[tokio::test]
async fn an_integer_property_interpolates_to_an_integer() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine5").await;
    let cycles = property_id(&properties, "Cycles");
    put_readings(
        &router,
        &asset_id,
        &cycles,
        &[
            (1_000, 0, json!({"integerValue": 10}), Some("GOOD")),
            (1_010, 0, json!({"integerValue": 11}), Some("GOOD")),
        ],
    )
    .await;

    // Rows differ in where the interval timestamps fall between the two readings,
    // so they span rounding down, rounding up and landing exactly on a reading.
    let rows: IntegerRows = vec![
        (
            "fifths of the gap",
            Request {
                interval: 2,
                end: (1_010, 0),
                ..base(&asset_id, &cycles)
            },
            vec![
                ((1_000, 0), 10),
                ((1_002, 0), 10),
                ((1_004, 0), 10),
                ((1_006, 0), 11),
                ((1_008, 0), 11),
                ((1_010, 0), 11),
            ],
        ),
        (
            "exactly half the gap",
            Request {
                interval: 5,
                end: (1_010, 0),
                ..base(&asset_id, &cycles)
            },
            vec![((1_000, 0), 10), ((1_005, 0), 11), ((1_010, 0), 11)],
        ),
    ];

    for (case, request, expected) in rows {
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let actual: Vec<((i64, i64), Option<i64>)> = points(&body)
            .into_iter()
            .map(|(timestamp, value)| {
                assert!(
                    value["doubleValue"].is_null(),
                    "case {case:?}: an INTEGER property reports integerValue, got {value}"
                );
                (timestamp, value["integerValue"].as_i64())
            })
            .collect();
        let want: Vec<((i64, i64), Option<i64>)> = expected
            .into_iter()
            .map(|(timestamp, value)| (timestamp, Some(value)))
            .collect();
        assert_eq!(actual, want, "case {case:?}: expected {want:?}, got {actual:?}");
    }
}

#[tokio::test]
async fn the_page_size_defaults_to_ten_and_next_token_resumes_the_intervals() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine6").await;
    let temperature = property_id(&properties, "Temperature");
    // Rising by 1.0 per second again, so the instant `1000 + 10 * k` of a 10-second
    // walk from 1000 holds exactly 10 * k — 13 instants over the 120-second range.
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 0.0}), Some("GOOD")),
            (1_120, 0, json!({"doubleValue": 120.0}), Some("GOOD")),
        ],
    )
    .await;
    let whole: Vec<(At, f64)> = (0..=12)
        .map(|k| ((1_000 + k * 10, 0), (k * 10) as f64))
        .collect();

    // "If not specified, the default value is 10."
    let request = Request {
        interval: 10,
        end: (1_120, 0),
        ..base(&asset_id, &temperature)
    };
    let (status, body) = call(&router, "GET", &request.uri(), None).await;
    assert_eq!(status, 200, "default page: got {status} {body}");
    assert_doubles("default page", &body, &whole[..10]);
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("default page: expected a nextToken in {body}"))
        .to_string();
    let (status, rest) = call(
        &router,
        "GET",
        &Request {
            next_token: Some(token),
            ..Request {
                interval: 10,
                end: (1_120, 0),
                ..base(&asset_id, &temperature)
            }
        }
        .uri(),
        None,
    )
    .await;
    assert_eq!(status, 200, "default page 2: got {status} {rest}");
    assert_doubles("default page 2", &rest, &whole[10..]);
    assert!(
        rest["nextToken"].is_null(),
        "default page 2: the last page carries no nextToken, got {rest}"
    );

    // Rows differ in page size; in every one the pages concatenate to the whole
    // enumeration, in order, and the last page carries no token.
    for max_results in [1, 5, 11, 13, 20] {
        let case = format!("maxResults {max_results}");
        let mut collected: Vec<(At, f64)> = Vec::new();
        let mut next_token = None;
        let mut pages = 0;
        loop {
            let request = Request {
                interval: 10,
                end: (1_120, 0),
                max_results: Some(max_results),
                next_token: next_token.clone(),
                ..base(&asset_id, &temperature)
            };
            let (status, body) = call(&router, "GET", &request.uri(), None).await;
            assert_eq!(status, 200, "case {case:?}: got {status} {body}");
            let page = doubles(&body);
            assert!(
                page.len() <= max_results as usize,
                "case {case:?}: a page holds at most {max_results} points, got {page:?}"
            );
            assert!(
                !page.is_empty(),
                "case {case:?}: a page the token led to must not be empty, got {body}"
            );
            collected.extend(page);
            pages += 1;
            assert!(pages <= 20, "case {case:?}: paging did not terminate");
            match body["nextToken"].as_str() {
                Some(token) => next_token = Some(token.to_string()),
                None => break,
            }
        }
        assert_eq!(
            collected.len(),
            whole.len(),
            "case {case:?}: expected {} points over all pages, got {collected:?}",
            whole.len()
        );
        for (at, ((timestamp, value), (want_timestamp, want_value))) in
            collected.iter().zip(whole.iter()).enumerate()
        {
            assert_eq!(
                timestamp, want_timestamp,
                "case {case:?}: point {at} expected timestamp {want_timestamp:?}, \
                 got {collected:?}"
            );
            assert!(
                (value - want_value).abs() < TOLERANCE,
                "case {case:?}: point {at} expected {want_value}, got {value} \
                 (all: {collected:?})"
            );
        }
    }

    // A token this operation never issued is refused rather than answered with an
    // empty page: it is not hex, or it names no interval of this range.
    for token in ["zz", &::hex::encode("0"), &::hex::encode("9999")] {
        let request = Request {
            interval: 10,
            end: (1_120, 0),
            next_token: Some(token.to_string()),
            ..base(&asset_id, &temperature)
        };
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        assert_eq!(status, 400, "token {token:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "token {token:?}: unexpected error code in {body}"
        );
    }
}

#[tokio::test]
async fn a_next_token_is_honoured_only_against_the_request_that_minted_it() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine6b").await;
    let temperature = property_id(&properties, "Temperature");
    let cycles = property_id(&properties, "Cycles");
    let (other_asset, other_properties) = asset_with_properties(&router, "Turbine6c").await;
    let other_temperature = property_id(&other_properties, "Temperature");

    // One rising series per stream this suite's rows name, so each row below is an
    // enumeration that really holds values — what a foreign token would otherwise
    // reposition the caller inside.
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 0.0}), Some("GOOD")),
            (1_120, 0, json!({"doubleValue": 120.0}), Some("GOOD")),
            // A second series of BAD readings over the same window, so the row that
            // varies `quality` alone names a served enumeration too.
            (1_005, 0, json!({"doubleValue": 5.0}), Some("BAD")),
            (1_115, 0, json!({"doubleValue": 115.0}), Some("BAD")),
        ],
    )
    .await;
    put_readings(
        &router,
        &asset_id,
        &cycles,
        &[
            (1_000, 0, json!({"integerValue": 0}), Some("GOOD")),
            (1_120, 0, json!({"integerValue": 120}), Some("GOOD")),
        ],
    )
    .await;
    put_readings(
        &router,
        &other_asset,
        &other_temperature,
        &[
            (1_000, 0, json!({"doubleValue": 0.0}), Some("GOOD")),
            (1_120, 0, json!({"doubleValue": 120.0}), Some("GOOD")),
        ],
    )
    .await;

    // 13 instants over the 120-second range, the value at each being "seconds past
    // 1000".
    let whole: Vec<(At, f64)> = (0..=12)
        .map(|k| ((1_000 + k * 10, 0), (k * 10) as f64))
        .collect();
    // The request that MINTS the token every row below replays.
    let minting = || Request {
        interval: 10,
        end: (1_120, 0),
        max_results: Some(4),
        ..base(&asset_id, &temperature)
    };
    let (status, body) = call(&router, "GET", &minting().uri(), None).await;
    assert_eq!(status, 200, "minting page: got {status} {body}");
    assert_doubles("minting page", &body, &whole[..4]);
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("minting page: expected a nextToken in {body}"))
        .to_string();

    // Each row varies the minting request along ONE member that decides which
    // instants are enumerated or what each of them reports: the stream, the grid, the
    // range, the interpolation type, the quality, the query window.
    let rows: Vec<(&str, Request)> = vec![
        (
            "another property of the same asset",
            Request {
                property_id: &cycles,
                ..minting()
            },
        ),
        (
            "another asset's property",
            Request {
                asset_id: &other_asset,
                property_id: &other_temperature,
                ..minting()
            },
        ),
        (
            "a wider interval",
            Request {
                interval: 20,
                ..minting()
            },
        ),
        (
            "the other interpolation type",
            Request {
                kind: LOCF,
                ..minting()
            },
        ),
        (
            "an earlier start",
            Request {
                start: (990, 0),
                ..minting()
            },
        ),
        (
            "a start differing only in its nanosecond offset",
            Request {
                start: (1_000, 1),
                ..minting()
            },
        ),
        (
            "a later inclusive end",
            Request {
                end: (1_200, 0),
                ..minting()
            },
        ),
        (
            "another quality",
            Request {
                quality: "BAD",
                ..minting()
            },
        ),
        (
            "an interval window the minting request did not carry",
            Request {
                window: Some(30),
                ..minting()
            },
        ),
    ];
    for (case, foreign) in rows {
        let (status, body) = call(&router, "GET", &foreign.uri(), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: without a token this request is served: got {status} {body}"
        );
        let served = points(&body);
        assert!(
            !served.is_empty(),
            "case {case:?}: the row must enumerate values of its own, got {body}"
        );
        let (status, body) = call(
            &router,
            "GET",
            &Request {
                next_token: Some(token.clone()),
                ..foreign
            }
            .uri(),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a token minted for another request names no page of this \
             one, got {status} {body} (its own first page is {served:?})"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("The provided next token does not match query filtering parameters"),
            "case {case:?}: a token this operation DID issue, replayed with other query \
             filtering parameters, is refused in those words; got {body}"
        );
    }

    // A bare position is not one of this operation's tokens: every in-range ordinal
    // of the minting grid, hex-encoded on its own, names no page.
    for ordinal in 1..=12 {
        let forged = ::hex::encode(ordinal.to_string());
        let (status, body) = call(
            &router,
            "GET",
            &Request {
                next_token: Some(forged.clone()),
                ..minting()
            }
            .uri(),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "ordinal {ordinal}: a bare position is no token, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "ordinal {ordinal}: unexpected error code in {body}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some("Invalid next token provided"),
            "ordinal {ordinal}: text this operation never issued is refused in those \
             fixed words, which carry no part of the request; got {body}"
        );
    }

    // Nor is a respelling of one: the minting request's own token is taken apart here
    // only to re-write its position in a form this operation never issues.
    let decoded = String::from_utf8(::hex::decode(&token).expect("the token is hexadecimal"))
        .expect("the token is text");
    let (position, rest) = decoded
        .split_once('|')
        .unwrap_or_else(|| panic!("a token carries more than a position: {decoded:?}"));
    for respelling in [format!("0{position}|{rest}"), format!("+{position}|{rest}")] {
        let (status, body) = call(
            &router,
            "GET",
            &Request {
                next_token: Some(::hex::encode(&respelling)),
                ..minting()
            }
            .uri(),
            None,
        )
        .await;
        assert_eq!(
            status, 400,
            "respelling {respelling:?}: got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "respelling {respelling:?}: unexpected error code in {body}"
        );
    }

    // The PAGE SIZE is not part of what a token is bound to — it cuts an enumeration
    // into pages rather than deciding what the enumeration holds — so the minting
    // request honours its own token whatever number of values the next page asks for,
    // resuming exactly where the first page stopped.
    for (case, max_results, expected) in [
        ("the minting page size", Some(4), &whole[4..8]),
        ("a larger page size", Some(7), &whole[4..11]),
        ("the default page size", None, &whole[4..]),
    ] {
        let (status, body) = call(
            &router,
            "GET",
            &Request {
                max_results,
                next_token: Some(token.clone()),
                ..minting()
            }
            .uri(),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_doubles(case, &body, expected);
    }
}

#[tokio::test]
async fn a_request_nothing_can_be_interpolated_for_is_a_modelled_refusal() {
    let router = router();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine7").await;
    let temperature = property_id(&properties, "Temperature");
    let status_property = property_id(&properties, "Status");
    let running = property_id(&properties, "Running");
    let absent = "11111111-2222-3333-4444-555555555555";

    // Rows span the refusals the model's own documentation states: an
    // interpolation type outside the two valid values, a window with a type other
    // than LINEAR_INTERPOLATION, linear interpolation of a property no arithmetic
    // applies to, a range whose end precedes its start, and the two forms that
    // identify a property.
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            "unknown interpolation type",
            Request {
                kind: "CUBIC_INTERPOLATION",
                ..base(&asset_id, &temperature)
            }
            .uri(),
            400,
            "InvalidRequestException",
        ),
        (
            "a window with LOCF_INTERPOLATION",
            Request {
                kind: LOCF,
                window: Some(5),
                ..base(&asset_id, &temperature)
            }
            .uri(),
            400,
            "InvalidRequestException",
        ),
        (
            "linear interpolation of a STRING property",
            Request {
                ..base(&asset_id, &status_property)
            }
            .uri(),
            400,
            "InvalidRequestException",
        ),
        (
            "linear interpolation of a BOOLEAN property",
            Request {
                ..base(&asset_id, &running)
            }
            .uri(),
            400,
            "InvalidRequestException",
        ),
        (
            "an end before the start",
            Request {
                start: (1_100, 0),
                end: (1_000, 0),
                ..base(&asset_id, &temperature)
            }
            .uri(),
            400,
            "InvalidRequestException",
        ),
        (
            "an asset that resolves to nothing",
            Request {
                ..base(absent, &temperature)
            }
            .uri(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a property the asset does not have",
            Request {
                ..base(&asset_id, absent)
            }
            .uri(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an alias that resolves to nothing",
            format!(
                "/properties/interpolated?propertyAlias=/company/windfarm/3/turbine/7/temperature\
                 &startTimeInSeconds=1000&endTimeInSeconds=1100&intervalInSeconds=10\
                 &quality=GOOD&type={LINEAR}"
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an asset id without a property id",
            format!(
                "/properties/interpolated?assetId={asset_id}\
                 &startTimeInSeconds=1000&endTimeInSeconds=1100&intervalInSeconds=10\
                 &quality=GOOD&type={LINEAR}"
            ),
            400,
            "InvalidRequestException",
        ),
        (
            "both identification forms at once",
            format!(
                "/properties/interpolated?assetId={asset_id}&propertyId={temperature}\
                 &propertyAlias=/company/windfarm/3/turbine/7/temperature\
                 &startTimeInSeconds=1000&endTimeInSeconds=1100&intervalInSeconds=10\
                 &quality=GOOD&type={LINEAR}"
            ),
            400,
            "InvalidRequestException",
        ),
        (
            "neither identification form",
            format!(
                "/properties/interpolated\
                 ?startTimeInSeconds=1000&endTimeInSeconds=1100&intervalInSeconds=10\
                 &quality=GOOD&type={LINEAR}"
            ),
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, uri, expected_status, expected_code) in rows {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // A `quality` outside the modelled enum is refused rather than filtering nothing
    // and reporting an empty page. The wire code is whichever layer refuses it
    // first — the runtime's own constraint check or this handler's — so only the
    // status is pinned here.
    for quality in ["good", "ANY", "GOOD1", "UNKNOWN"] {
        let request = Request {
            quality,
            ..base(&asset_id, &temperature)
        };
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        assert_eq!(
            status, 400,
            "quality {quality:?}: expected 400, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn interpolating_changes_nothing_a_later_caller_sees() {
    let (router, engine) = router_with_engine();
    let (asset_id, properties) = asset_with_properties(&router, "Turbine8").await;
    let temperature = property_id(&properties, "Temperature");
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 10.0}), Some("GOOD")),
            (1_100, 0, json!({"doubleValue": 110.0}), Some("GOOD")),
        ],
    )
    .await;

    /// The stored series of this property, read straight from the shared engine
    /// store: the only way to see whether a read wrote anything.
    fn stored(
        engine: &engine::InMemoryEngine,
        asset_id: &str,
        property: &str,
    ) -> Vec<(i64, Option<i32>, Option<f64>)> {
        service_com_amazonaws_iotsitewise_awsiotsitewise::state::service_state(engine)
            .expect("the service store")
            .lock()
            .property_values
            .get(asset_id)
            .and_then(|properties| properties.get(property))
            .map(|series| {
                series
                    .iter()
                    .map(|value| {
                        (
                            value.timestamp.time_in_seconds,
                            value.timestamp.offset_in_nanos,
                            value.value.double_value,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    let before = stored(&engine, &asset_id, &temperature);
    let request = Request {
        interval: 10,
        ..base(&asset_id, &temperature)
    };
    let (status, first) = call(&router, "GET", &request.uri(), None).await;
    assert_eq!(status, 200, "first read: got {status} {first}");
    let (status, second) = call(&router, "GET", &request.uri(), None).await;
    assert_eq!(status, 200, "second read: got {status} {second}");
    // Both reads must report the same NON-EMPTY series: two empty answers would
    // agree with each other and prove nothing. The default page holds ten values,
    // so the eleventh instant of this range is behind the token.
    assert_eq!(
        doubles(&first),
        (0..10)
            .map(|k| ((1_000 + k * 10, 0), 10.0 + (k * 10) as f64))
            .collect::<Vec<(At, f64)>>(),
        "first read: expected the first page of the ramp, got {first}"
    );
    assert_eq!(
        doubles(&first),
        doubles(&second),
        "two identical reads must report the same values"
    );
    assert_eq!(
        stored(&engine, &asset_id, &temperature),
        before,
        "interpolating must not change the stored series"
    );

    // And the property still reports the latest value that was written to it.
    let (status, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={temperature}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetAssetPropertyValue: got {status} {latest}");
    assert_eq!(
        latest["propertyValue"]["timestamp"]["timeInSeconds"].as_i64(),
        Some(anchored(ROW_EPOCH, 1_100)),
        "GetAssetPropertyValue after interpolating: got {latest}"
    );
}

/// How the ONE data stream a row addresses by its alias came to exist.
#[derive(Debug, Clone, Copy)]
enum Origin {
    /// Created by alias-addressed ingestion: no asset property has ever carried it.
    AliasIngest,
    /// Bound to an asset property, then detached again by
    /// `DisassociateTimeSeriesFromAssetProperty`, which leaves the stream standing
    /// under its alias with the samples it held.
    Disassociated,
    /// Bound to an asset property and still associated with it.
    Associated,
    /// No stream at all: the alias names nothing.
    Absent,
}

/// What an alias-addressed interpolation must answer for one row.
#[derive(Debug)]
enum Expected {
    /// 200 with these `doubleValue` points.
    Doubles(Vec<(At, f64)>),
    /// 200 with these `integerValue` points.
    Integers(Vec<(At, i64)>),
    /// 200 with these `stringValue` points.
    Strings(Vec<(At, &'static str)>),
    /// This status and error code.
    Refused(u16, &'static str),
}

/// One row of the alias table: how the stream came to exist, the alias that names
/// it, which fixture property a bound row binds that alias to (and whose data type
/// the stream therefore carries), the readings it holds as
/// `(table second, value)`, the interpolation asked for and what must come back.
struct AliasRow {
    case: &'static str,
    alias: &'static str,
    origin: Origin,
    property: &'static str,
    readings: Vec<(i64, Value)>,
    kind: &'static str,
    interval: i64,
    expected: Expected,
}

/// Write `readings` to the data stream `alias` names through
/// `BatchPutAssetPropertyValue` addressed by `propertyAlias` alone — the write that
/// creates the stream when no asset property carries that alias.
async fn put_readings_by_alias(
    router: &MultiServiceRouter,
    alias: &str,
    readings: &[(i64, Value)],
) {
    for (at, (seconds, value)) in readings.iter().enumerate() {
        let (status, put) = call(
            router,
            "POST",
            "/properties",
            Some(json!({"entries": [{
                "entryId": format!("a{at}"),
                "propertyAlias": alias,
                "propertyValues": [{
                    "value": value,
                    "timestamp": {
                        "timeInSeconds": anchored(ROW_EPOCH, *seconds),
                        "offsetInNanos": 0,
                    },
                    "quality": "GOOD",
                }],
            }]})),
        )
        .await;
        assert_eq!(status, 200, "alias write {at}: got {status} {put}");
        assert_eq!(
            put["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "alias write {at} must be accepted, got {put}"
        );
    }
}

/// An alias-addressed `GetInterpolatedAssetPropertyValues` URI over the same
/// 1000..1100 range the tables above use, so an alias-addressed row differs from a
/// pair-addressed one only in how it names the stream.
fn alias_uri(alias: &str, kind: &str, interval: i64) -> String {
    format!(
        "/properties/interpolated?propertyAlias={alias}\
         &startTimeInSeconds={start}&startTimeOffsetInNanos=0\
         &endTimeInSeconds={end}&endTimeOffsetInNanos=0\
         &intervalInSeconds={interval}&quality=GOOD&type={kind}",
        start = anchored(ROW_EPOCH, 1_000),
        end = anchored(ROW_EPOCH, 1_100),
    )
}

#[tokio::test]
async fn an_alias_names_one_stream_whether_or_not_a_property_carries_it() {
    let router = router();

    // Rows differ along how the stream the alias names came to exist, the data type
    // it carries (which is the property's for a bound stream and the ingested
    // value's for one no property owns), and which interpolation is asked for. In
    // every row the alias names a stream that HOLDS the readings, so the
    // interpolation is computed from them; only the row whose alias names nothing is
    // an absent resource.
    let ramp = || {
        vec![
            (1_000, json!({"doubleValue": 0.0})),
            (1_100, json!({"doubleValue": 100.0})),
        ]
    };
    let ramp_points = || vec![((1_000, 0), 0.0), ((1_050, 0), 50.0), ((1_100, 0), 100.0)];
    let counts = || {
        vec![
            (1_000, json!({"integerValue": 10})),
            (1_100, json!({"integerValue": 20})),
        ]
    };
    let rows: Vec<AliasRow> = vec![
        AliasRow {
            case: "a stream created by alias-addressed ingestion",
            alias: "/company/windfarm/3/turbine/7/temperature",
            origin: Origin::AliasIngest,
            property: "Temperature",
            readings: ramp(),
            kind: LINEAR,
            interval: 50,
            expected: Expected::Doubles(ramp_points()),
        },
        AliasRow {
            case: "a stream left behind by disassociating it",
            alias: "/company/windfarm/3/turbine/8/temperature",
            origin: Origin::Disassociated,
            property: "Temperature",
            readings: ramp(),
            kind: LINEAR,
            interval: 50,
            expected: Expected::Doubles(ramp_points()),
        },
        AliasRow {
            case: "a stream still associated with its property",
            alias: "/company/windfarm/3/turbine/9/temperature",
            origin: Origin::Associated,
            property: "Temperature",
            readings: ramp(),
            kind: LINEAR,
            interval: 50,
            expected: Expected::Doubles(ramp_points()),
        },
        AliasRow {
            // Carrying the last observation forward reads the stream's samples just
            // as linear interpolation does, so it must reach a disassociated one too.
            case: "LOCF over a disassociated stream",
            alias: "/company/windfarm/4/turbine/1/temperature",
            origin: Origin::Disassociated,
            property: "Temperature",
            readings: ramp(),
            kind: LOCF,
            interval: 50,
            expected: Expected::Doubles(vec![
                ((1_000, 0), 0.0),
                ((1_050, 0), 0.0),
                ((1_100, 0), 100.0),
            ]),
        },
        AliasRow {
            // A stream no property owns carries the data type of the value that
            // created it, so an INTEGER one reports integers.
            case: "an INTEGER stream created by alias-addressed ingestion",
            alias: "/company/windfarm/4/turbine/2/cycles",
            origin: Origin::AliasIngest,
            property: "Cycles",
            readings: counts(),
            kind: LINEAR,
            interval: 50,
            expected: Expected::Integers(vec![
                ((1_000, 0), 10),
                ((1_050, 0), 15),
                ((1_100, 0), 20),
            ]),
        },
        AliasRow {
            case: "an INTEGER stream left behind by disassociating it",
            alias: "/company/windfarm/4/turbine/3/cycles",
            origin: Origin::Disassociated,
            property: "Cycles",
            readings: counts(),
            kind: LINEAR,
            interval: 50,
            expected: Expected::Integers(vec![
                ((1_000, 0), 10),
                ((1_050, 0), 15),
                ((1_100, 0), 20),
            ]),
        },
        AliasRow {
            // Carrying a reading forward copies it, so it is defined for a stream
            // whose readings are not numbers.
            case: "LOCF over a STRING stream no property owns",
            alias: "/company/windfarm/4/turbine/4/status",
            origin: Origin::AliasIngest,
            property: "Status",
            readings: vec![(1_000, json!({"stringValue": "RUNNING"}))],
            kind: LOCF,
            interval: 50,
            expected: Expected::Strings(vec![
                ((1_000, 0), "RUNNING"),
                ((1_050, 0), "RUNNING"),
                ((1_100, 0), "RUNNING"),
            ]),
        },
        AliasRow {
            // The arithmetic has no meaning for a STRING stream, and the stream's
            // own data type is what says so whether a property owns it or not.
            case: "linear interpolation of a STRING stream no property owns",
            alias: "/company/windfarm/4/turbine/5/status",
            origin: Origin::AliasIngest,
            property: "Status",
            readings: vec![(1_000, json!({"stringValue": "RUNNING"}))],
            kind: LINEAR,
            interval: 50,
            expected: Expected::Refused(400, "InvalidRequestException"),
        },
        AliasRow {
            case: "an alias no stream answers to",
            alias: "/company/windfarm/9/turbine/9/temperature",
            origin: Origin::Absent,
            property: "Temperature",
            readings: Vec::new(),
            kind: LINEAR,
            interval: 50,
            expected: Expected::Refused(404, "ResourceNotFoundException"),
        },
    ];

    for (index, row) in rows.into_iter().enumerate() {
        let case = row.case;
        // Seed the stream the way this row's origin creates it. A bound row gets its
        // own asset, so no row depends on what another one left behind.
        match row.origin {
            Origin::AliasIngest => put_readings_by_alias(&router, row.alias, &row.readings).await,
            Origin::Absent => {}
            Origin::Associated | Origin::Disassociated => {
                let (asset_id, properties) =
                    asset_with_properties(&router, &format!("AliasTurbine{index}")).await;
                let property = property_id(&properties, row.property);
                let (status, bound) = call(
                    &router,
                    "PUT",
                    &format!("/assets/{asset_id}/properties/{property}"),
                    Some(json!({"propertyAlias": row.alias})),
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: binding the alias got {status} {bound}"
                );
                let readings: Vec<Reading> = row
                    .readings
                    .iter()
                    .map(|(seconds, value)| (*seconds, 0, value.clone(), Some("GOOD")))
                    .collect();
                put_readings(&router, &asset_id, &property, &readings).await;
                if matches!(row.origin, Origin::Disassociated) {
                    let (status, detached) = call(
                        &router,
                        "POST",
                        &format!(
                            "/timeseries/disassociate?alias={}&assetId={asset_id}\
                             &propertyId={property}",
                            row.alias
                        ),
                        None,
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: disassociating got {status} {detached}"
                    );
                }
            }
        }

        // The premise of every seeded row: the alias names a stream this service
        // reports, so the interpolation below is not asked about an absent resource.
        let (status, described) = call(
            &router,
            "GET",
            &format!("/timeseries/describe?alias={}", row.alias),
            None,
        )
        .await;
        let expected_describe = if matches!(row.origin, Origin::Absent) {
            404
        } else {
            200
        };
        assert_eq!(
            status, expected_describe,
            "case {case:?}: DescribeTimeSeries expected {expected_describe}, \
             got {status} {described}"
        );

        let (status, body) = call(
            &router,
            "GET",
            &alias_uri(row.alias, row.kind, row.interval),
            None,
        )
        .await;
        match row.expected {
            Expected::Doubles(expected) => {
                assert_eq!(status, 200, "case {case:?}: got {status} {body}");
                assert_doubles(case, &body, &expected);
            }
            Expected::Integers(expected) => {
                assert_eq!(status, 200, "case {case:?}: got {status} {body}");
                let actual: Vec<(At, Option<i64>)> = points(&body)
                    .into_iter()
                    .map(|(timestamp, value)| (timestamp, value["integerValue"].as_i64()))
                    .collect();
                let want: Vec<(At, Option<i64>)> = expected
                    .into_iter()
                    .map(|(timestamp, value)| (timestamp, Some(value)))
                    .collect();
                assert_eq!(
                    actual, want,
                    "case {case:?}: expected {want:?}, got {actual:?} ({body})"
                );
            }
            Expected::Strings(expected) => {
                assert_eq!(status, 200, "case {case:?}: got {status} {body}");
                let actual: Vec<(At, Option<String>)> = points(&body)
                    .into_iter()
                    .map(|(timestamp, value)| {
                        (
                            timestamp,
                            value["stringValue"].as_str().map(str::to_string),
                        )
                    })
                    .collect();
                let want: Vec<(At, Option<String>)> = expected
                    .into_iter()
                    .map(|(timestamp, value)| (timestamp, Some(value.to_string())))
                    .collect();
                assert_eq!(
                    actual, want,
                    "case {case:?}: expected {want:?}, got {actual:?} ({body})"
                );
            }
            Expected::Refused(expected_status, expected_code) => {
                assert_eq!(
                    status, expected_status,
                    "case {case:?}: expected {expected_status}, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    expected_code,
                    "case {case:?}: expected {expected_code}, got {body}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_pair_form_names_the_property_by_the_modelled_id_form() {
    let router = router();
    // This operation models `propertyId` as `ID` — len 36..36 and the UUID pattern,
    // from `smithy_query(query="operation", operation="GetInterpolatedAssetPropertyValues")`
    // — and NOT as `CustomID`, so the minted id is the only form that reaches the
    // handler and the `externalId:<externalId>` form a `CustomID` member accepts is
    // refused by that constraint. The property below carries an external id so the
    // refused form names a property that really exists.
    const EXTERNAL: &str = "turbine-temperature";
    let model_id = create_model(
        &router,
        "ExternalIdTurbineModel",
        json!([{
            "name": "Temperature",
            "externalId": EXTERNAL,
            "dataType": "DOUBLE",
            "type": {"measurement": {}},
        }]),
    )
    .await;
    let asset_id = create_asset(&router, "ExternalIdTurbine", &model_id).await;
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    put_readings(
        &router,
        &asset_id,
        &temperature,
        &[
            (1_000, 0, json!({"doubleValue": 0.0}), Some("GOOD")),
            (1_100, 0, json!({"doubleValue": 100.0}), Some("GOOD")),
        ],
    )
    .await;

    let external_label = format!("externalId:{EXTERNAL}");
    let expected = [((1_000, 0), 0.0), ((1_050, 0), 50.0), ((1_100, 0), 100.0)];
    for (case, property, accepted) in [
        ("the minted id", temperature.as_str(), true),
        ("the external id form", external_label.as_str(), false),
    ] {
        let request = Request {
            interval: 50,
            ..base(&asset_id, property)
        };
        let (status, body) = call(&router, "GET", &request.uri(), None).await;
        if accepted {
            assert_eq!(status, 200, "case {case:?}: got {status} {body}");
            assert_doubles(case, &body, &expected);
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: a propertyId outside the modelled ID form is refused, \
                 got {status} {body}"
            );
        }
    }
}
