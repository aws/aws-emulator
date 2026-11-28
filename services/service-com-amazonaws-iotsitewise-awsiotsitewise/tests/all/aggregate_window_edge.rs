// SPDX-License-Identifier: CC0-1.0

//! The bucket-EDGE rule of the two aggregate reads, driven over HTTP through the
//! same `MultiServiceRouter` the emulator boots with.
//!
//! The rule this suite pins: a resolution's windows are the epoch-aligned
//! intervals of that resolution's length, each EXCLUSIVE at its start and
//! INCLUSIVE at its end, and an aggregate is stamped with the instant its window
//! ENDS — so a reading whose timestamp falls exactly on a grid boundary `t` is
//! summarised in the window ENDING at `t`, not in the one starting there. It holds
//! for every resolution and for both `GetAssetPropertyAggregates` and
//! `BatchGetAssetPropertyAggregates`, which share one aggregation engine.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over:
//! the resolution (hence the window length), where inside its window a reading
//! falls, how many windows the readings spread over, and the time ordering. The
//! expected edges are computed by an independent oracle over the instants this
//! file wrote, never read back from the service.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, create_asset, create_model, property_id, router};

/// Seconds in a day: the length of the longest modelled window.
const DAY: i64 = 86_400;

/// Every `Resolution` an aggregate read may name, with the length in seconds of
/// the window it names.
///
/// Source: `smithy_query(query="shape", shape="Resolution")` — length 2..3 and
/// pattern `^1m|15m|1h|1d$`, i.e. one minute, fifteen minutes, one hour and one
/// day. Each length divides [`DAY`], so a midnight-aligned instant starts a
/// window of all four. "15m" is in the table though the issue cited only "1m".
const RESOLUTIONS: [(&str, i64); 4] = [("1m", 60), ("15m", 900), ("1h", 3_600), ("1d", DAY)];

/// Where `BatchPutAssetPropertyValue` is bound: `POST /properties`.
const BATCH_PUT: &str = "/properties";

/// Where `BatchGetAssetPropertyAggregates` is bound:
/// `POST /properties/batch/aggregates`.
const BATCH_AGGREGATES: &str = "/properties/batch/aggregates";

/// How close two doubles must be to count as the same number.
const EPSILON: f64 = 1e-9;

/// The UTC midnight every reading in this suite is placed after.
///
/// Taken from the shared anchor so the readings stay inside the
/// `[now - 7 days, now + 10 minutes]` range `BatchPutAssetPropertyValue` ingests,
/// and truncated to a whole day so a window of every modelled resolution starts
/// here. Fixed once per test binary.
fn grid_start() -> i64 {
    let anchor = super::support::anchor();
    anchor - anchor.rem_euclid(DAY)
}

/// An epoch second as the ISO-8601 `date-time` an `@httpQuery`-bound timestamp
/// travels as.
fn iso(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// Create an asset model with one DOUBLE measurement property, an asset of it,
/// and answer `(assetId, propertyId)`.
async fn asset_with_property(router: &MultiServiceRouter, name: &str) -> (String, String) {
    let model_id = create_model(
        router,
        &format!("{name}Model"),
        json!([{"name": "Reading", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(router, name, &model_id).await;
    let (status, described) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset({name}): got {status} {described}");
    let property = property_id(&described["assetProperties"], "Reading");
    (asset_id, property)
}

/// Persist one reading per `(offset from grid_start(), value)` pair, ten at a
/// time (the modelled per-entry maximum).
async fn write_readings(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    readings: &[(i64, f64)],
) {
    for (chunk, batch) in readings.chunks(10).enumerate() {
        let values: Vec<Value> = batch
            .iter()
            .map(|(offset, value)| {
                json!({
                    "value": {"doubleValue": value},
                    "timestamp": {"timeInSeconds": grid_start() + offset},
                    "quality": "GOOD",
                })
            })
            .collect();
        let (status, body) = call(
            router,
            "POST",
            BATCH_PUT,
            Some(json!({"entries": [{
                "entryId": format!("w{chunk}"),
                "assetId": asset_id,
                "propertyId": property,
                "propertyValues": values,
            }]})),
        )
        .await;
        assert_eq!(status, 200, "write chunk {chunk}: got {status} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "write chunk {chunk}: every reading must be accepted, got {body}"
        );
    }
}

/// The windows the rule says a read of `readings` at window length `window` must
/// report, as `(window end, the readings of that window)`, oldest first.
///
/// An INDEPENDENT oracle: it groups the instants this file wrote onto the
/// epoch-aligned grid itself and stamps each group with the END of the window it
/// falls in. A window is `(end - window, end]`, so the end of the window holding an
/// instant is the SMALLEST multiple of `window` that is not before it — the instant
/// itself when it lands exactly on a boundary.
fn expected_windows(window: i64, readings: &[(i64, f64)]) -> Vec<(i64, Vec<f64>)> {
    let mut windows: Vec<(i64, Vec<f64>)> = Vec::new();
    for (offset, value) in readings {
        let end = window_end(window, grid_start() + offset);
        match windows.last_mut() {
            Some((last, values)) if *last == end => values.push(*value),
            _ => windows.push((end, vec![*value])),
        }
    }
    windows
}

/// The end of the `window`-long epoch-aligned interval that holds the instant `at`,
/// under the `(start, end]` convention: the smallest multiple of `window` that is
/// not before `at`.
fn window_end(window: i64, at: i64) -> i64 {
    let inside = at.rem_euclid(window);
    if inside == 0 {
        at
    } else {
        at - inside + window
    }
}

/// `GET /properties/aggregates?...` for one property over `(start, end]` at
/// `resolution`, asking for COUNT, SUM and AVERAGE, with `extra` appended.
fn single_uri(
    asset_id: &str,
    property: &str,
    resolution: &str,
    start: i64,
    end: i64,
    extra: &str,
) -> String {
    format!(
        "/properties/aggregates?assetId={asset_id}&propertyId={property}\
         &resolution={resolution}&aggregateTypes=COUNT&aggregateTypes=SUM\
         &aggregateTypes=AVERAGE&startDate={}&endDate={}{extra}",
        iso(start),
        iso(end)
    )
}

/// One `BatchGetAssetPropertyAggregates` entry over the same window.
fn batch_entry(
    asset_id: &str,
    property: &str,
    resolution: &str,
    start: i64,
    end: i64,
    ordering: Option<&str>,
) -> Value {
    let mut entry = json!({
        "entryId": "edge",
        "assetId": asset_id,
        "propertyId": property,
        "aggregateTypes": ["COUNT", "SUM", "AVERAGE"],
        "resolution": resolution,
        "startDate": start,
        "endDate": end,
    });
    if let Some(ordering) = ordering {
        entry["timeOrdering"] = json!(ordering);
    }
    entry
}

/// The `aggregatedValues` of a single-property 200 answer.
fn single_windows(case: &str, body: &Value) -> Vec<Value> {
    body["aggregatedValues"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {body}"))
        .clone()
}

/// The `aggregatedValues` of the one success entry of a batch 200 answer.
fn batch_windows(case: &str, body: &Value) -> Vec<Value> {
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: expected no errorEntries, got {body}"
    );
    let successes = body["successEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no successEntries in {body}"));
    assert_eq!(
        successes.len(),
        1,
        "case {case:?}: expected one success entry, got {body}"
    );
    successes[0]["aggregatedValues"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {body}"))
        .clone()
}

/// The epoch second an aggregate is stamped with.
fn stamped(case: &str, window: &Value) -> i64 {
    window["timestamp"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: an aggregate with no epoch timestamp: {window}"))
        as i64
}

/// Assert `actual` is within [`EPSILON`] of `expected`.
fn assert_close(case: &str, what: &str, actual: Option<f64>, expected: f64) {
    assert!(
        actual.is_some_and(|actual| (actual - expected).abs() < EPSILON),
        "case {case:?}: expected {what} {expected}, got {actual:?}"
    );
}

/// Check one answer's windows against the oracle: the same number of windows, each
/// stamped with its window's END and carrying that window's aggregates.
fn assert_windows(case: &str, window: i64, expected: &[(i64, Vec<f64>)], reported: &[Value]) {
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected {} windows, got {reported:?}",
        expected.len()
    );
    for (at, (end, readings)) in expected.iter().enumerate() {
        let actual = stamped(case, &reported[at]);
        assert_eq!(
            actual, *end,
            "case {case:?}: window {at} covers ({}, {end}], so it is stamped with {end}, got {actual}",
            end - window
        );
        // The edge is the INCLUSIVE end of the interval the aggregate summarises:
        // every reading it counted is at or before it, and the window it closes
        // starts one resolution length earlier, on the epoch-aligned grid.
        assert_eq!(
            actual.rem_euclid(window),
            0,
            "case {case:?}: window {at} must close an epoch-aligned window, got {actual}"
        );
        let sum: f64 = readings.iter().sum();
        let count = readings.len() as f64;
        assert_close(case, "count", reported[at]["value"]["count"].as_f64(), count);
        assert_close(case, "sum", reported[at]["value"]["sum"].as_f64(), sum);
        assert_close(
            case,
            "average",
            reported[at]["value"]["average"].as_f64(),
            sum / count,
        );
    }
}

/// The reading placements the resolution table reads: offsets from [`grid_start`]
/// and the value written at each, chosen so every resolution groups them
/// differently and so that readings land exactly on a grid boundary (which closes
/// the window ENDING there), inside a window, and on the last second before the
/// next boundary.
const READINGS: [(i64, f64); 6] = [
    (0, 1.0),
    (30, 3.0),
    (59, 5.0),
    (60, 7.0),
    (899, 9.0),
    (3_600, 11.0),
];

#[tokio::test]
async fn every_resolution_stamps_an_aggregate_with_the_end_of_its_window() {
    let router = router();
    let (asset_id, property) = asset_with_property(&router, "EdgePress").await;
    write_readings(&router, &asset_id, &property, &READINGS).await;

    // Rows are every modelled `Resolution`, so the rule is checked at each window
    // length rather than only at the one the issue cited.
    for (resolution, window) in RESOLUTIONS {
        let expected = expected_windows(window, &READINGS);
        let case = format!("GetAssetPropertyAggregates/{resolution}");
        let (status, body) = call(
            &router,
            "GET",
            &single_uri(
                &asset_id,
                &property,
                resolution,
                grid_start() - 1,
                grid_start() + 2 * DAY,
                "",
            ),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_windows(&case, window, &expected, &single_windows(&case, &body));

        // The batch sibling shares the aggregation engine, so it must report the
        // same edge for the same series.
        let case = format!("BatchGetAssetPropertyAggregates/{resolution}");
        let (status, body) = call(
            &router,
            "POST",
            BATCH_AGGREGATES,
            Some(json!({"entries": [batch_entry(
                &asset_id,
                &property,
                resolution,
                grid_start() - 1,
                grid_start() + 2 * DAY,
                None,
            )]})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_windows(&case, window, &expected, &batch_windows(&case, &body));
    }
}

#[tokio::test]
async fn a_reading_anywhere_in_a_window_is_reported_at_that_window_s_end() {
    let router = router();

    // Rows walk ONE reading across a `15m` window: its exclusive start (a
    // boundary, which belongs to the PREVIOUS window), a second inside it, its
    // inclusive end, and a second past that end. Each row is a fresh series, so
    // only the placement decides the reported edge.
    let window = 900_i64;
    let placements: [(&str, i64); 4] = [
        ("a grid boundary, closing the window before it", 0),
        ("a second inside the window", 1),
        ("the window's inclusive end", window),
        ("the first second past that end", window + 1),
    ];
    for (case, offset) in placements {
        let (asset_id, property) =
            asset_with_property(&router, &format!("EdgeProbe{offset}")).await;
        write_readings(&router, &asset_id, &property, &[(offset, 2.0)]).await;

        let expected = window_end(window, grid_start() + offset);
        let (status, body) = call(
            &router,
            "GET",
            &single_uri(
                &asset_id,
                &property,
                "15m",
                grid_start() - 1,
                grid_start() + 2 * DAY,
                "",
            ),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let reported = single_windows(case, &body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected one window, got {body}"
        );
        let actual = stamped(case, &reported[0]);
        assert_eq!(
            actual, expected,
            "case {case:?}: a reading at {} closes the window ending {expected}, got {actual}",
            grid_start() + offset
        );
        assert!(
            actual >= grid_start() + offset && actual - window < grid_start() + offset,
            "case {case:?}: the reading at {} falls in ({}, {actual}]",
            grid_start() + offset,
            actual - window
        );
    }
}

#[tokio::test]
async fn the_edge_convention_holds_whatever_the_time_ordering_and_page_size() {
    let router = router();
    let (asset_id, property) = asset_with_property(&router, "EdgeFan").await;
    write_readings(&router, &asset_id, &property, &READINGS).await;

    let ascending: Vec<i64> = expected_windows(60, &READINGS)
        .into_iter()
        .map(|(end, _)| end)
        .collect();
    let mut descending = ascending.clone();
    descending.reverse();

    // Rows are both `TimeOrdering` values plus the request naming none — whose
    // documented default is ASCENDING — each walked at page sizes from one window
    // to more than every window.
    let orderings: [(&str, Option<&str>, &Vec<i64>); 3] = [
        ("default", None, &ascending),
        ("ASCENDING", Some("ASCENDING"), &ascending),
        ("DESCENDING", Some("DESCENDING"), &descending),
    ];
    for (ordering, parameter, expected) in orderings {
        for page_size in 1..=expected.len() + 1 {
            let case = format!("{ordering}/maxResults={page_size}");
            let mut collected: Vec<i64> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut extra = format!("&maxResults={page_size}");
                if let Some(parameter) = parameter {
                    extra.push_str(&format!("&timeOrdering={parameter}"));
                }
                if let Some(token) = token.as_deref() {
                    extra.push_str(&format!("&nextToken={token}"));
                }
                let (status, body) = call(
                    &router,
                    "GET",
                    &single_uri(
                        &asset_id,
                        &property,
                        "1m",
                        grid_start() - 1,
                        grid_start() + 2 * DAY,
                        &extra,
                    ),
                    None,
                )
                .await;
                assert_eq!(status, 200, "case {case:?}: got {status} {body}");
                let page = single_windows(&case, &body);
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page carries at most maxResults windows, got {body}"
                );
                collected.extend(page.iter().map(|window| stamped(&case, window)));
                token = body["nextToken"].as_str().map(str::to_string);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}: the walk must terminate, collected {collected:?}"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                &collected, expected,
                "case {case:?}: expected the window ends {expected:?}, got {collected:?}"
            );
        }
    }

    // The batch sibling reports the same ends in the same orders.
    for (ordering, parameter, expected) in orderings {
        let case = format!("batch/{ordering}");
        let (status, body) = call(
            &router,
            "POST",
            BATCH_AGGREGATES,
            Some(json!({"entries": [batch_entry(
                &asset_id,
                &property,
                "1m",
                grid_start() - 1,
                grid_start() + 2 * DAY,
                parameter,
            )]})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let reported: Vec<i64> = batch_windows(&case, &body)
            .iter()
            .map(|window| stamped(&case, window))
            .collect();
        assert_eq!(
            &reported, expected,
            "case {case:?}: expected the window ends {expected:?}, got {reported:?}"
        );
    }
}
