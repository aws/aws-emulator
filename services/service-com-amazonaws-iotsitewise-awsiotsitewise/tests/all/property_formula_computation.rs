// SPDX-License-Identifier: CC0-1.0

//! The data points a `transform` and a `metric` property hold, driven over HTTP
//! through the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: a property whose definition carries a formula holds
//! no ingested data of its own — it holds what its expression EVALUATES to over
//! the properties its variables name. A `transform` is "a one-to-one mapping of a
//! property's data points from one form to another ... by applying the
//! transformation expression to each data point", so it holds one data point per
//! instant an input holds one at; a `metric` is computed over the `window`
//! "over which IoT SiteWise computes the metric's aggregation expression. IoT
//! SiteWise computes one data point per `window`", placed "at the end of the
//! interval", so it holds one data point per COMPLETED window. Every reader of a
//! data stream reports those data points: the latest value, the history and the
//! aggregates alike.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over:
//! the formula's operators and functions, how many variables it declares, the data
//! type of each input (`DOUBLE`, `INTEGER`, and an `attribute`, which is a constant
//! of the asset rather than a stream), whether an input is itself a computed
//! property (a metric over a transform), whether the input lies in one window or
//! several, and whether the window has ended. Every expected value is computed by
//! hand in the table, never read back from the service.
//!
//! The wire harness (`router`, `call`, `create_model`, …) lives in
//! `super::asset_lifecycle`; the hierarchy walk this file's roll-up suite needs
//! lives in `super::asset_hierarchies`.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_hierarchies::{associate, hierarchy_id};
use super::asset_lifecycle::{call, create_asset, create_model, property_id, router};

/// Where `BatchPutAssetPropertyValue` is bound: `POST /properties`.
const BATCH_PUT: &str = "/properties";

/// Where `GetAssetPropertyValue` is bound: `GET /properties/latest`.
const LATEST: &str = "/properties/latest";

/// Where `GetAssetPropertyValueHistory` is bound: `GET /properties/history`.
const HISTORY: &str = "/properties/history";

/// Where `GetAssetPropertyAggregates` is bound: `GET /properties/aggregates`.
const AGGREGATES: &str = "/properties/aggregates";

/// Seconds in a minute: the length of the tumbling window these suites compute
/// over, and the shortest `TumblingWindow.interval` names ("the interval time must
/// be between 1 minute and 1 week").
const MINUTE: i64 = 60;

/// How close two doubles must be to count as the same number.
const EPSILON: f64 = 1e-9;

/// How far ahead of the clock the not-yet-ended window's reading sits.
///
/// `BatchPutAssetPropertyValue` accepts a timestamp up to ten minutes ahead of the
/// clock, and the window a reading this far ahead falls in cannot END for at least
/// this long — so the "a window that has not ended holds no data point" case has
/// minutes of room rather than depending on how fast the host runs.
const AHEAD: i64 = 300;

/// The minute boundary every reading in these suites is placed after.
///
/// Taken from the shared anchor, so the readings stay inside the
/// `[now - 7 days, now + 10 minutes]` range `BatchPutAssetPropertyValue` ingests,
/// and truncated to a whole minute, so a tumbling window of [`MINUTE`] starts
/// exactly here and the windows the readings fall in are long past. Fixed once per
/// test binary.
fn grid_start() -> i64 {
    let anchor = super::support::anchor();
    anchor - anchor.rem_euclid(MINUTE)
}

/// An epoch second as the ISO-8601 `date-time` an `@httpQuery`-bound timestamp
/// travels as.
fn iso(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// A `transform` property definition: `expression` over `variables`, each pair
/// being the name the formula calls the variable by and the property it names.
fn transform(name: &str, expression: &str, variables: &[(&str, &str)]) -> Value {
    json!({
        "name": name,
        "dataType": "DOUBLE",
        "type": {"transform": {
            "expression": expression,
            "variables": declared(variables),
        }},
    })
}

/// A `metric` property definition over a [`MINUTE`] tumbling window.
fn metric(name: &str, expression: &str, variables: &[(&str, &str)]) -> Value {
    json!({
        "name": name,
        "dataType": "DOUBLE",
        "type": {"metric": {
            "expression": expression,
            "variables": declared(variables),
            "window": {"tumbling": {"interval": "1m"}},
        }},
    })
}

/// `ExpressionVariable`s naming same-model properties by name — the spelling
/// `VariableValue.propertyId` documents for a property "from the same asset model".
fn declared(variables: &[(&str, &str)]) -> Value {
    Value::Array(
        variables
            .iter()
            .map(|(name, property)| json!({"name": name, "value": {"propertyId": property}}))
            .collect(),
    )
}

/// The model these suites compute over: the ingested properties its formulas read,
/// then one property per formula row of the tables below.
fn formula_model() -> Value {
    json!([
        {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}},
        {"name": "Cycles", "dataType": "INTEGER", "unit": "Count", "type": {"measurement": {}}},
        {"name": "Calibration", "dataType": "DOUBLE", "type": {"attribute": {"defaultValue": "2.5"}}},
        transform("Fahrenheit", "temp * 9 / 5 + 32", &[("temp", "Temperature")]),
        transform("Scaled", "temp * (9 / 5)", &[("temp", "Temperature")]),
        transform("Rooted", "sqrt(temp)", &[("temp", "Temperature")]),
        transform(
            "Powered",
            "pow(temp, 2) + round(temp / 3)",
            &[("temp", "Temperature")],
        ),
        transform(
            "Calibrated",
            "temp + calibration",
            &[("temp", "Temperature"), ("calibration", "Calibration")],
        ),
        transform(
            "Combined",
            "temp + cycles",
            &[("temp", "Temperature"), ("cycles", "Cycles")],
        ),
        metric("AvgTemp", "avg(temp)", &[("temp", "Temperature")]),
        metric("SumTemp", "sum(temp)", &[("temp", "Temperature")]),
        metric("CountTemp", "count(temp)", &[("temp", "Temperature")]),
        metric("MaxTemp", "max(temp)", &[("temp", "Temperature")]),
        metric("MinTemp", "min(temp)", &[("temp", "Temperature")]),
        metric("StdevTemp", "stdev(temp)", &[("temp", "Temperature")]),
        metric("SpreadTemp", "max(temp) - min(temp)", &[("temp", "Temperature")]),
        metric(
            "AvgCalibrated",
            "avg(temp) + calibration",
            &[("temp", "Temperature"), ("calibration", "Calibration")],
        ),
        metric("AvgFahrenheit", "avg(f)", &[("f", "Fahrenheit")]),
    ])
}

/// The `Temperature` readings every table is written against: `(offset from
/// [`grid_start`], value)`. Three fall in the window ending at `+60` — the last of
/// them exactly ON its inclusive end — and two in the window ending at `+120`.
const TEMPERATURES: [(i64, f64); 5] = [
    (1, 10.0),
    (30, 20.0),
    (60, 30.0),
    (61, 100.0),
    (90, 200.0),
];

/// The `Cycles` readings, on a grid of their own: a two-variable transform is
/// computed at the instants of EITHER input, so these instants interleave with
/// [`TEMPERATURES`] rather than lining up with them.
const CYCLES: [(i64, i64); 2] = [(5, 7), (70, 9)];

/// Create the model and one asset of it, answering `(assetId, properties)` — the
/// asset's own property list, which every property id below is read out of.
async fn asset_of_the_formula_model(router: &MultiServiceRouter, name: &str) -> (String, Value) {
    let model_id = create_model(router, &format!("{name}Model"), formula_model()).await;
    let asset_id = create_asset(router, name, &model_id).await;
    let (status, described) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset({name}): got {status} {described}");
    let properties = described["assetProperties"].clone();
    (asset_id, properties)
}

/// Ingest `values` into `(asset_id, property)` through
/// `BatchPutAssetPropertyValue`, ten at a time (the modelled per-entry maximum).
async fn write(
    router: &MultiServiceRouter,
    asset_id: &str,
    property: &str,
    values: &[(i64, Value)],
) {
    for (chunk, batch) in values.chunks(10).enumerate() {
        let property_values: Vec<Value> = batch
            .iter()
            .map(|(at, value)| {
                json!({
                    "value": value,
                    "timestamp": {"timeInSeconds": at, "offsetInNanos": 0},
                    "quality": "GOOD",
                })
            })
            .collect();
        let (status, body) = call(
            router,
            "POST",
            BATCH_PUT,
            Some(json!({"entries": [{
                "entryId": format!("seed{chunk}"),
                "assetId": asset_id,
                "propertyId": property,
                "propertyValues": property_values,
            }]})),
        )
        .await;
        assert_eq!(status, 200, "seeding {property}: got {status} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "seeding {property}: every reading must be accepted, got {body}"
        );
    }
}

/// Ingest [`TEMPERATURES`] and [`CYCLES`] onto `asset_id`.
async fn write_the_inputs(router: &MultiServiceRouter, asset_id: &str, properties: &Value) {
    let temperatures: Vec<(i64, Value)> = TEMPERATURES
        .iter()
        .map(|(at, value)| (grid_start() + at, json!({"doubleValue": value})))
        .collect();
    write(
        router,
        asset_id,
        &property_id(properties, "Temperature"),
        &temperatures,
    )
    .await;
    let cycles: Vec<(i64, Value)> = CYCLES
        .iter()
        .map(|(at, value)| (grid_start() + at, json!({"integerValue": value})))
        .collect();
    write(router, asset_id, &property_id(properties, "Cycles"), &cycles).await;
}

/// The data points `GetAssetPropertyValueHistory` reports for one property, as
/// `(epoch second, doubleValue)` pairs, oldest first.
async fn history(
    router: &MultiServiceRouter,
    case: &str,
    asset_id: &str,
    property: &str,
) -> Vec<(i64, f64)> {
    let (status, body) = call(
        router,
        "GET",
        &format!("{HISTORY}?assetId={asset_id}&propertyId={property}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: history got {status} {body}");
    body["assetPropertyValueHistory"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no assetPropertyValueHistory in {body}"))
        .iter()
        .map(|point| {
            (
                point["timestamp"]["timeInSeconds"].as_i64().unwrap_or_else(|| {
                    panic!("case {case:?}: a data point with no timeInSeconds: {point}")
                }),
                point["value"]["doubleValue"].as_f64().unwrap_or_else(|| {
                    panic!("case {case:?}: a data point with no doubleValue: {point}")
                }),
            )
        })
        .collect()
}

/// The `propertyValue` `GetAssetPropertyValue` reports for one property, as
/// `(epoch second, doubleValue)` — `None` when it reports none at all.
async fn latest(
    router: &MultiServiceRouter,
    case: &str,
    asset_id: &str,
    property: &str,
) -> Option<(i64, f64)> {
    let (status, body) = call(
        router,
        "GET",
        &format!("{LATEST}?assetId={asset_id}&propertyId={property}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: latest got {status} {body}");
    let point = body.get("propertyValue")?;
    if point.is_null() {
        return None;
    }
    Some((
        point["timestamp"]["timeInSeconds"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: a value with no timeInSeconds: {point}")),
        point["value"]["doubleValue"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: a value with no doubleValue: {point}")),
    ))
}

/// One row of the computed-series tables: the case it names, the property it reads
/// and the data points that property must hold, as `(offset from [`grid_start`],
/// value)` pairs, oldest first.
type SeriesRow = (&'static str, &'static str, Vec<(i64, f64)>);

/// One row of the aggregate table: the case it names, the property it aggregates
/// and the windows that read must report, as `(offset of the window's stamp,
/// count, average)`, oldest first.
type WindowRow = (&'static str, &'static str, Vec<(i64, f64, f64)>);

/// Assert the data points `reported` are the `(offset from [`grid_start`], value)`
/// pairs `expected`, in that order.
fn assert_points(case: &str, expected: &[(i64, f64)], reported: &[(i64, f64)]) {
    assert_eq!(
        reported.len(),
        expected.len(),
        "case {case:?}: expected {} data points, got {reported:?}",
        expected.len()
    );
    for (at, (offset, value)) in expected.iter().enumerate() {
        let (second, reported_value) = reported[at];
        assert_eq!(
            second,
            grid_start() + offset,
            "case {case:?}: data point {at} must be stamped {}, got {second}",
            grid_start() + offset
        );
        assert!(
            (reported_value - value).abs() < EPSILON,
            "case {case:?}: data point {at} must be {value}, got {reported_value}"
        );
    }
}

#[tokio::test]
async fn a_transform_holds_one_computed_data_point_per_input_data_point() {
    let router = router();
    let (asset_id, properties) = asset_of_the_formula_model(&router, "TransformBoiler").await;
    write_the_inputs(&router, &asset_id, &properties).await;

    // Rows differ along the formula: its operators, the function it applies, how
    // many variables it declares, and the data type of each input — a `DOUBLE`
    // stream, an `INTEGER` stream and an `attribute`, which is a value of the asset
    // rather than a stream and so brings no instant of its own.
    let rows: Vec<SeriesRow> = vec![
        (
            "arithmetic over one DOUBLE stream",
            "Fahrenheit",
            vec![(1, 50.0), (30, 68.0), (60, 86.0), (61, 212.0), (90, 392.0)],
        ),
        (
            // 9/5 is the RATIO the formula language means, not an integer
            // division: a property computed from it can "only have a data type of
            // DOUBLE".
            "a parenthesised ratio of two integer literals",
            "Scaled",
            vec![(1, 18.0), (30, 36.0), (60, 54.0), (61, 180.0), (90, 360.0)],
        ),
        (
            // A function the issue did not cite.
            "a function applied to the input",
            "Rooted",
            vec![
                (1, 3.1622776601683795),
                (30, 4.47213595499958),
                (60, 5.477225575051661),
                (61, 10.0),
                (90, 14.142135623730951),
            ],
        ),
        (
            // Two more functions the issue did not cite, one of them of two
            // arguments, over a sub-expression each.
            "two functions of a sub-expression",
            "Powered",
            vec![
                (1, 103.0),
                (30, 407.0),
                (60, 910.0),
                (61, 10_033.0),
                (90, 40_067.0),
            ],
        ),
        (
            "two variables, the second an attribute of the asset",
            "Calibrated",
            vec![
                (1, 12.5),
                (30, 22.5),
                (60, 32.5),
                (61, 102.5),
                (90, 202.5),
            ],
        ),
        (
            // The instants of BOTH streams, each computed with the value the other
            // input held then — and no data point at `+1`, where `Cycles` had no
            // value yet, so there was nothing to map.
            "two streams on interleaved grids",
            "Combined",
            vec![
                (5, 17.0),
                (30, 27.0),
                (60, 37.0),
                (61, 107.0),
                (70, 109.0),
                (90, 209.0),
            ],
        ),
    ];

    for (case, property, expected) in rows {
        let property = property_id(&properties, property);
        let reported = history(&router, case, &asset_id, &property).await;
        assert_points(case, &expected, &reported);

        // The latest value is the newest computed data point, as it is for an
        // ingested series.
        let newest = latest(&router, case, &asset_id, &property).await;
        let (offset, value) = expected
            .last()
            .copied()
            .expect("every row expects at least one data point");
        assert_eq!(
            newest.map(|(second, _)| second),
            Some(grid_start() + offset),
            "case {case:?}: the latest value must be stamped {}, got {newest:?}",
            grid_start() + offset
        );
        assert!(
            newest.is_some_and(|(_, newest)| (newest - value).abs() < EPSILON),
            "case {case:?}: the latest value must be {value}, got {newest:?}"
        );

        // A read reports; reading twice reports the same series, so nothing a read
        // did changed what the next caller sees.
        let again = history(&router, case, &asset_id, &property).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report the same data points"
        );
    }
}

#[tokio::test]
async fn a_metric_holds_one_computed_data_point_per_completed_window() {
    let router = router();
    let (asset_id, properties) = asset_of_the_formula_model(&router, "MetricBoiler").await;
    write_the_inputs(&router, &asset_id, &properties).await;

    // Both windows are long past, so each is a data point stamped with the instant
    // it ENDS: the window `(grid_start, +60]` holds 10, 20 and 30 (the last exactly
    // on its inclusive end) and `(+60, +120]` holds 100 and 200. Rows differ along
    // the aggregation the formula applies, whether it combines two of them, whether
    // it reads a constant beside the window, and whether its input is itself a
    // computed property.
    let rows: Vec<SeriesRow> = vec![
        ("the average of the window", "AvgTemp", vec![(60, 20.0), (120, 150.0)]),
        ("the sum of the window", "SumTemp", vec![(60, 60.0), (120, 300.0)]),
        (
            "how many data points the window holds",
            "CountTemp",
            vec![(60, 3.0), (120, 2.0)],
        ),
        ("the largest of the window", "MaxTemp", vec![(60, 30.0), (120, 200.0)]),
        ("the smallest of the window", "MinTemp", vec![(60, 10.0), (120, 100.0)]),
        (
            "the standard deviation of the window",
            "StdevTemp",
            vec![(60, 8.164_965_809_277_26), (120, 50.0)],
        ),
        (
            "two aggregations of one window combined",
            "SpreadTemp",
            vec![(60, 20.0), (120, 100.0)],
        ),
        (
            "an aggregation beside an attribute of the asset",
            "AvgCalibrated",
            vec![(60, 22.5), (120, 152.5)],
        ),
        (
            // The input is the `Fahrenheit` transform, itself computed: 50, 68 and
            // 86 in the first window, 212 and 392 in the second.
            "an aggregation over a computed input",
            "AvgFahrenheit",
            vec![(60, 68.0), (120, 302.0)],
        ),
    ];

    for (case, property, expected) in rows {
        let property = property_id(&properties, property);
        let reported = history(&router, case, &asset_id, &property).await;
        assert_points(case, &expected, &reported);

        let newest = latest(&router, case, &asset_id, &property).await;
        let (offset, value) = expected
            .last()
            .copied()
            .expect("every row expects at least one window");
        assert_eq!(
            newest.map(|(second, _)| second),
            Some(grid_start() + offset),
            "case {case:?}: the newest window must be stamped {}, got {newest:?}",
            grid_start() + offset
        );
        assert!(
            newest.is_some_and(|(_, newest)| (newest - value).abs() < EPSILON),
            "case {case:?}: the newest window must be {value}, got {newest:?}"
        );

        let again = history(&router, case, &asset_id, &property).await;
        assert_eq!(
            again, reported,
            "case {case:?}: a second read must report the same windows"
        );
    }
}

#[tokio::test]
async fn an_aggregate_read_summarises_the_computed_series() {
    let router = router();
    let (asset_id, properties) = asset_of_the_formula_model(&router, "AggregateBoiler").await;
    write_the_inputs(&router, &asset_id, &properties).await;

    // Rows differ along which computed series is aggregated — a transform's, whose
    // data points sit at the instants of its input, and a metric's, whose data
    // points sit at the ends of its windows — and so along how the epoch-aligned
    // `1m` windows of the aggregate read group them. The expected windows are the
    // ones the aggregate reader reports for the computed instants: a data point at
    // an epoch-aligned instant starts the window it is counted in, which is stamped
    // one resolution later.
    let rows: Vec<WindowRow> = vec![
        (
            // The transform's five data points sit at `+1`, `+30`, `+60`, `+61` and
            // `+90`: two of them in the minute stamped `+60` and three in the one
            // stamped `+120`.
            "a transform's computed series",
            "Fahrenheit",
            vec![(60, 2.0, 59.0), (120, 3.0, 230.0)],
        ),
        (
            // The metric's two data points are stamped `+60` and `+120`, each the
            // only one in the minute it starts.
            "a metric's computed series",
            "AvgTemp",
            vec![(120, 1.0, 20.0), (180, 1.0, 150.0)],
        ),
    ];

    for (case, property, expected) in rows {
        let property = property_id(&properties, property);
        let (status, body) = call(
            &router,
            "GET",
            &format!(
                "{AGGREGATES}?assetId={asset_id}&propertyId={property}&resolution=1m\
                 &aggregateTypes=COUNT&aggregateTypes=AVERAGE&startDate={}&endDate={}",
                iso(grid_start()),
                iso(grid_start() + 10 * MINUTE)
            ),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let reported = body["aggregatedValues"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {body}"))
            .clone();
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: expected {} windows, got {reported:?}",
            expected.len()
        );
        for (at, (offset, count, average)) in expected.iter().enumerate() {
            let window = &reported[at];
            let stamped = window["timestamp"].as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: a window with no epoch timestamp: {window}")
            }) as i64;
            assert_eq!(
                stamped,
                grid_start() + offset,
                "case {case:?}: window {at} must be stamped {}, got {stamped}",
                grid_start() + offset
            );
            let reported_count = window["value"]["count"].as_f64();
            assert!(
                reported_count.is_some_and(|actual| (actual - count).abs() < EPSILON),
                "case {case:?}: window {at} must count {count} data points, got {reported_count:?}"
            );
            let reported_average = window["value"]["average"].as_f64();
            assert!(
                reported_average.is_some_and(|actual| (actual - average).abs() < EPSILON),
                "case {case:?}: window {at} must average {average}, got {reported_average:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_window_the_clock_has_not_passed_the_end_of_holds_no_data_point() {
    let router = router();
    let (asset_id, properties) = asset_of_the_formula_model(&router, "AheadBoiler").await;

    // ONE reading, stamped ahead of the clock: the window it falls in cannot have
    // ended, so the metric that aggregates it has no data point at all, while the
    // transform that maps it has exactly one — the difference between "one data
    // point per input data point" and "one data point per completed window".
    let ahead = super::support::now_seconds() + AHEAD;
    write(
        &router,
        &asset_id,
        &property_id(&properties, "Temperature"),
        &[(ahead, json!({"doubleValue": 40.0}))],
    )
    .await;

    let case = "a reading inside a window that has not ended";
    let metric = property_id(&properties, "AvgTemp");
    let windows = history(&router, case, &asset_id, &metric).await;
    assert!(
        windows.is_empty(),
        "case {case:?}: the metric must hold no data point yet, got {windows:?}"
    );
    let newest = latest(&router, case, &asset_id, &metric).await;
    assert!(
        newest.is_none(),
        "case {case:?}: the metric must report no latest value yet, got {newest:?}"
    );

    let transform = property_id(&properties, "Fahrenheit");
    let mapped = history(&router, case, &asset_id, &transform).await;
    assert_eq!(
        mapped.len(),
        1,
        "case {case:?}: the transform must map the one reading, got {mapped:?}"
    );
    assert_eq!(
        mapped[0].0, ahead,
        "case {case:?}: the mapped data point must carry the reading's own instant"
    );
    assert!(
        (mapped[0].1 - 104.0).abs() < EPSILON,
        "case {case:?}: 40 degrees must map to 104, got {}",
        mapped[0].1
    );
}

#[tokio::test]
async fn a_formula_the_evaluator_cannot_reduce_holds_no_data_point() {
    let router = router();

    // A formula whose parentheses nest as deep as the modelled `Expression` length
    // allows: the expression is bounded by the model (length 0..1024), so the depth
    // a caller can ask for is bounded with it, and the evaluation of this one still
    // answers the value its input carries.
    let nested = format!("{}temp{}", "(".repeat(300), ")".repeat(300));
    let model_id = create_model(
        &router,
        "GuardedBoiler",
        json!([
            {"name": "Temperature", "dataType": "DOUBLE", "type": {"measurement": {}}},
            transform("Nested", &nested, &[("temp", "Temperature")]),
            // No function catalogue reaches a write — the formula language's own
            // lives in the IoT SiteWise User Guide and AWS adds to it — so a
            // formula naming one this evaluator does not implement is stored and
            // computes nothing, rather than being answered with an invented value.
            transform("Unknown", "mystery(temp)", &[("temp", "Temperature")]),
            // The same holds for a name the expression ENGINE offers under its own
            // builtins (`len` is one of them): the formula language's functions are
            // the only ones a formula can reach, so this computes nothing too.
            transform("Builtin", "len(temp)", &[("temp", "Temperature")]),
            // Two metrics that read each other: a dependency the stored
            // definitions allow and no series can come out of.
            metric("PingPong", "avg(pong)", &[("pong", "Pong")]),
            metric("Pong", "avg(pingpong)", &[("pingpong", "PingPong")]),
        ]),
    )
    .await;
    let asset_id = create_asset(&router, "GuardedBoiler1", &model_id).await;
    let (status, described) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {described}");
    let properties = described["assetProperties"].clone();
    let temperatures: Vec<(i64, Value)> = TEMPERATURES
        .iter()
        .map(|(at, value)| (grid_start() + at, json!({"doubleValue": value})))
        .collect();
    write(
        &router,
        &asset_id,
        &property_id(&properties, "Temperature"),
        &temperatures,
    )
    .await;

    let rows: Vec<SeriesRow> = vec![
        (
            "parentheses nested as deep as the expression length allows",
            "Nested",
            TEMPERATURES.to_vec(),
        ),
        (
            "a function the evaluator does not implement",
            "Unknown",
            Vec::new(),
        ),
        (
            "a function the expression engine offers as a builtin",
            "Builtin",
            Vec::new(),
        ),
        (
            "a formula that reaches itself through its variables",
            "PingPong",
            Vec::new(),
        ),
        (
            "the other half of the same cycle",
            "Pong",
            Vec::new(),
        ),
    ];

    for (case, property, expected) in rows {
        let property = property_id(&properties, property);
        let reported = history(&router, case, &asset_id, &property).await;
        assert_points(case, &expected, &reported);
    }
}

#[tokio::test]
async fn a_metric_over_a_hierarchy_rolls_up_every_asset_under_it() {
    let router = router();

    // The child model the roll-up reads, and a parent whose metric names that
    // property through the hierarchy it declares: "the ID of the hierarchy to query
    // for the property ID" (`VariableValue.hierarchyId`).
    let child_model = create_model(
        &router,
        "RollUpTurbine",
        json!([{"name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let (status, described) = call(
        &router,
        "GET",
        &format!("/asset-models/{child_model}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");
    let child_power = property_id(&described["assetModelProperties"], "Power");

    let (status, parent_model) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "RollUpFarm",
            "assetModelProperties": [{
                "name": "TotalPower",
                "dataType": "DOUBLE",
                "type": {"metric": {
                    "expression": "sum(p)",
                    "variables": [{"name": "p", "value": {
                        "propertyId": child_power,
                        "hierarchyId": "Turbines",
                    }}],
                    "window": {"tumbling": {"interval": "1m"}},
                }},
            }],
            "assetModelHierarchies": [{"name": "Turbines", "childAssetModelId": child_model}],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {parent_model}");
    let parent_model = parent_model["assetModelId"]
        .as_str()
        .expect("a created model id")
        .to_string();
    let hierarchy = hierarchy_id(&router, &parent_model, "Turbines").await;

    let farm = create_asset(&router, "RollUpFarm1", &parent_model).await;
    // Rows differ along how much each child contributes to the window: one reading
    // from the first child and two from the second, so the roll-up is neither one
    // child's series nor one reading per child.
    let children: [(&str, &[(i64, f64)]); 2] = [
        ("RollUpTurbine1", &[(10, 3.0)]),
        ("RollUpTurbine2", &[(20, 4.0), (40, 5.0)]),
    ];
    for (name, readings) in children {
        let child = create_asset(&router, name, &child_model).await;
        let (status, body) = associate(&router, &farm, &hierarchy, &child).await;
        assert_eq!(status, 200, "associate {name}: got {status} {body}");
        let (status, described) = call(&router, "GET", &format!("/assets/{child}"), None).await;
        assert_eq!(status, 200, "DescribeAsset({name}): got {status} {described}");
        let power = property_id(&described["assetProperties"], "Power");
        let values: Vec<(i64, Value)> = readings
            .iter()
            .map(|(at, value)| (grid_start() + at, json!({"doubleValue": value})))
            .collect();
        write(&router, &child, &power, &values).await;
    }

    let (status, described) = call(&router, "GET", &format!("/assets/{farm}"), None).await;
    assert_eq!(status, 200, "DescribeAsset(farm): got {status} {described}");
    let total = property_id(&described["assetProperties"], "TotalPower");

    let case = "a metric summing one hierarchy's assets";
    let reported = history(&router, case, &farm, &total).await;
    // Every reading falls in the window ending at `+60`, so the roll-up is one data
    // point there carrying the sum over BOTH children.
    assert_points(case, &[(60, 12.0)], &reported);
}
