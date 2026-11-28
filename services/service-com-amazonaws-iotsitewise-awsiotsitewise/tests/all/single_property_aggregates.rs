// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetAssetPropertyAggregates` — the single-property form of
//! the aggregate read — driven over HTTP through the same `MultiServiceRouter` the
//! emulator boots with, so every assertion is about the wire behaviour a real client
//! sees.
//!
//! The rule these suites pin: the operation reports, for every aggregation window of
//! the requested `resolution` that holds at least one stored reading of the
//! identified property inside `(startDate, endDate]` and of the filtered quality,
//! exactly the `aggregateTypes` computed over the readings of that window, stamped
//! with the instant that window ENDS, in the requested `timeOrdering`, paginated by
//! `maxResults`/`nextToken`. Every row writes its readings through
//! `BatchPutAssetPropertyValue` first, so the answer is a function of stored state
//! and of nothing else.
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over:
//! the window length, the aggregate function, the range bounds, the quality, the
//! time ordering, the page size, and the ways a request fails to identify a
//! property.

use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// Seconds in a day. [`base`] is an exact multiple of it, so it is a UTC midnight
/// and every modelled resolution's windows align on it.
const DAY: i64 = 24 * 60 * 60;

/// The length in seconds of the window each modelled `Resolution` names.
///
/// Source: `smithy_query(query="shape", shape="Resolution")` — length 2..3 and
/// pattern `^1m|15m|1h|1d$`, i.e. one minute, fifteen minutes, one hour and one
/// day. An aggregate is stamped with the END of its window, so this length is
/// what every expected timestamp below adds to the window's start.
fn window_seconds(resolution: &str) -> i64 {
    match resolution {
        "1m" => 60,
        "15m" => 900,
        "1h" => 3_600,
        "1d" => DAY,
        other => panic!("{other} is not a modelled Resolution"),
    }
}

/// The epoch second every reading in this suite is placed relative to: the UTC
/// midnight two days before the current one, so the `1d` window boundary is this
/// instant itself AND every reading written at it or a little after falls inside the
/// `[-7 days, +10 minutes]` window `BatchPutAssetPropertyValue` accepts. Resolved
/// once per process, so two calls of one case can never straddle a midnight.
fn base() -> i64 {
    static ONCE: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *ONCE.get_or_init(|| {
        let now = DateTime::from(std::time::SystemTime::now()).secs();
        now.div_euclid(DAY) * DAY - 2 * DAY
    })
}

/// A router with just this service mounted on a fresh shared engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pin authentication off so the suite is independent of the process-wide switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// Dispatch one request and return `(status, parsed body)`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .map(|value| Bytes::from(serde_json::to_vec(&value).expect("a serializable body")))
        .unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTEST/20260101/{REGION}/iotsitewise/aws4_request, \
                 SignedHeaders=host, Signature=abc"
            ),
        )
        .body(payload)
        .expect("a valid request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            Value::String(String::from_utf8_lossy(bytes.as_ref()).into_owned())
        })
    };
    (status, parsed)
}

/// The wire error code of an error body (`__type`), or `""`.
fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// An epoch second as the ISO-8601 `date-time` an `@httpQuery`-bound timestamp
/// travels as.
fn iso(seconds: i64) -> String {
    DateTime::from_secs(seconds)
        .fmt(Format::DateTime)
        .expect("a formattable instant")
}

/// Create an asset model with one measurement property of `data_type`, an asset of
/// it, and return `(assetId, propertyId)`.
async fn asset_with_property(
    router: &MultiServiceRouter,
    name: &str,
    property: &str,
    data_type: &str,
) -> (String, String) {
    let (status, model) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": format!("{name}Model"),
            "assetModelProperties": [
                {"name": property, "dataType": data_type, "type": {"measurement": {}}}
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel({name}): got {status} {model}");
    let model_id = model["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAssetModel({name}): no assetModelId in {model}"))
        .to_string();

    let (status, asset) = call(
        router,
        "POST",
        "/assets",
        Some(json!({ "assetName": name, "assetModelId": model_id })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset({name}): got {status} {asset}");
    let asset_id = asset["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateAsset({name}): no assetId in {asset}"))
        .to_string();

    let (status, described) = call(router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset({name}): got {status} {described}");
    let property_id = described["assetProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeAsset({name}): no properties in {described}"))
        .iter()
        .find(|candidate| candidate["name"] == property)
        .unwrap_or_else(|| panic!("DescribeAsset({name}): no property {property}"))["id"]
        .as_str()
        .unwrap_or_else(|| panic!("DescribeAsset({name}): property {property} has no id"))
        .to_string();

    (asset_id, property_id)
}

/// One reading to persist: `(offset from base() in seconds, doubleValue, quality)`.
type Reading = (i64, f64, Option<&'static str>);

/// Persist `readings` on `(asset_id, property_id)` through
/// `BatchPutAssetPropertyValue`, ten at a time (the modelled per-entry maximum), and
/// assert every one was accepted.
async fn write_readings(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    readings: &[Reading],
) {
    for (chunk, batch) in readings.chunks(10).enumerate() {
        let values: Vec<Value> = batch
            .iter()
            .map(|(offset, value, quality)| {
                let mut reading = json!({
                    "value": {"doubleValue": value},
                    "timestamp": {"timeInSeconds": base() + offset},
                });
                if let Some(quality) = quality {
                    reading["quality"] = json!(quality);
                }
                reading
            })
            .collect();
        let (status, body) = call(
            router,
            "POST",
            "/properties",
            Some(json!({"entries": [{
                "entryId": format!("w{chunk}"),
                "assetId": asset_id,
                "propertyId": property_id,
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

/// `GET /properties/aggregates?...` from `(name, value)` pairs. A name repeated once
/// per element is how an `@httpQuery` list travels.
fn aggregates_uri(params: &[(&str, String)]) -> String {
    let query: Vec<String> = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    format!("/properties/aggregates?{}", query.join("&"))
}

/// The query naming one property over `(start, end]` at `resolution`, asking for
/// `types`.
fn query(
    asset_id: &str,
    property_id: &str,
    types: &[&str],
    resolution: &str,
    start: i64,
    end: i64,
) -> Vec<(&'static str, String)> {
    let mut params = vec![
        ("assetId", asset_id.to_string()),
        ("propertyId", property_id.to_string()),
        ("resolution", resolution.to_string()),
        ("startDate", iso(start)),
        ("endDate", iso(end)),
    ];
    params.extend(types.iter().map(|ty| ("aggregateTypes", ty.to_string())));
    params
}

/// The `(timestamp, member value)` of every aggregate in a response body, as epoch
/// seconds and `f64`.
fn reported(body: &Value, member: &str) -> Vec<(i64, f64)> {
    body["aggregatedValues"]
        .as_array()
        .unwrap_or_else(|| panic!("no aggregatedValues in {body}"))
        .iter()
        .map(|aggregate| {
            let at = aggregate["timestamp"]
                .as_f64()
                .unwrap_or_else(|| panic!("aggregate has no epoch timestamp in {aggregate}"))
                as i64;
            let value = aggregate["value"][member]
                .as_f64()
                .unwrap_or_else(|| panic!("aggregate has no {member} in {aggregate}"));
            (at, value)
        })
        .collect()
}

/// The readings the resolution and aggregate tables below aggregate: offsets from
/// [`base`] chosen so each resolution groups them differently.
const READINGS: [Reading; 6] = [
    (10, 2.0, None),
    (20, 4.0, None),
    (70, 6.0, None),
    (1_000, 8.0, None),
    (4_000, 10.0, None),
    (90_000, 20.0, None),
];

/// One row of the resolution table: the `Resolution` value and the windows it groups
/// [`READINGS`] into, as `(offset from base() of the window's INCLUSIVE END, sum,
/// count)`.
type ResolutionCase = (&'static str, Vec<(i64, f64, f64)>);

#[tokio::test]
async fn a_resolution_groups_the_readings_into_its_own_aligned_windows() {
    let router = router();
    let (asset_id, speed) = asset_with_property(&router, "Turbine", "Speed", "DOUBLE").await;
    write_readings(&router, &asset_id, &speed, &READINGS).await;

    // Rows are every `Resolution` the modelled pattern `^1m|15m|1h|1d$` admits —
    // including "15m", which the issue never cited — with the windows each one groups
    // READINGS into, written out by hand as an independent oracle. Windows are
    // epoch-aligned and each is `(start, end]`, so `1d` ends at UTC midnight and
    // `15m` at :00/:15/:30/:45, and a reading exactly on a boundary belongs to the
    // window ENDING there — which is why the reading at +90000 s closes the window
    // ending at +90000 s under `1m`, `15m` and `1h`, each of whose lengths divides
    // it.
    let cases: [ResolutionCase; 4] = [
        (
            "1m",
            vec![
                (60, 6.0, 2.0),
                (120, 6.0, 1.0),
                (1_020, 8.0, 1.0),
                (4_020, 10.0, 1.0),
                (90_000, 20.0, 1.0),
            ],
        ),
        (
            "15m",
            vec![
                (900, 12.0, 3.0),
                (1_800, 8.0, 1.0),
                (4_500, 10.0, 1.0),
                (90_000, 20.0, 1.0),
            ],
        ),
        (
            "1h",
            vec![(3_600, 20.0, 4.0), (7_200, 10.0, 1.0), (90_000, 20.0, 1.0)],
        ),
        ("1d", vec![(DAY, 30.0, 5.0), (2 * DAY, 20.0, 1.0)]),
    ];

    for (resolution, windows) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &aggregates_uri(&query(
                &asset_id,
                &speed,
                &["SUM", "COUNT"],
                resolution,
                base(),
                base() + 2 * DAY,
            )),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {resolution:?}: got {status} {body}");
        // Each window is stamped with the instant it ends.
        let end_of = |end: i64| base() + end;
        let expected_sums: Vec<(i64, f64)> = windows
            .iter()
            .map(|(offset, sum, _)| (end_of(*offset), *sum))
            .collect();
        let expected_counts: Vec<(i64, f64)> = windows
            .iter()
            .map(|(offset, _, count)| (end_of(*offset), *count))
            .collect();
        assert_eq!(
            reported(&body, "sum"),
            expected_sums,
            "case {resolution:?}: expected windows {expected_sums:?}, got {body}"
        );
        assert_eq!(
            reported(&body, "count"),
            expected_counts,
            "case {resolution:?}: expected counts {expected_counts:?}, got {body}"
        );
    }
}

#[tokio::test]
async fn each_aggregate_type_is_that_function_of_the_window_s_readings() {
    let router = router();
    let (asset_id, speed) = asset_with_property(&router, "Turbine", "Speed", "DOUBLE").await;
    write_readings(&router, &asset_id, &speed, &READINGS).await;

    // One `1d` window holds the first five readings: 2, 4, 6, 8, 10.
    let samples = [2.0_f64, 4.0, 6.0, 8.0, 10.0];
    let count = samples.len() as f64;
    let sum: f64 = samples.iter().sum();
    let mean = sum / count;
    // Rows are every `AggregateType` the model declares
    // (`smithy_query(query="shape", shape="AggregateType")`), each paired with the
    // response member it sets and the value computed here independently.
    let cases: [(&str, &str, f64); 6] = [
        ("AVERAGE", "average", mean),
        ("COUNT", "count", count),
        (
            "MAXIMUM",
            "maximum",
            samples.iter().copied().fold(f64::MIN, f64::max),
        ),
        (
            "MINIMUM",
            "minimum",
            samples.iter().copied().fold(f64::MAX, f64::min),
        ),
        ("SUM", "sum", sum),
        (
            "STANDARD_DEVIATION",
            "standardDeviation",
            (samples.iter().map(|s| (s - mean) * (s - mean)).sum::<f64>() / count).sqrt(),
        ),
    ];

    for (aggregate_type, member, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &aggregates_uri(&query(
                &asset_id,
                &speed,
                &[aggregate_type],
                "1d",
                base(),
                base() + DAY - 1,
            )),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {aggregate_type:?}: got {status} {body}");
        let aggregates = &body["aggregatedValues"];
        assert_eq!(
            aggregates.as_array().map(Vec::len),
            Some(1),
            "case {aggregate_type:?}: expected one window, got {body}"
        );
        let actual = aggregates[0]["value"][member].as_f64();
        assert!(
            actual.is_some_and(|actual| (actual - expected).abs() < 1e-9),
            "case {aggregate_type:?}: expected {member} {expected}, got {body}"
        );
        // Only the requested aggregate is reported: a function nobody asked for is
        // absent rather than reported as a zero.
        for (_, other, _) in cases {
            assert!(
                other == member || aggregates[0]["value"].get(other).is_none_or(Value::is_null),
                "case {aggregate_type:?}: {other} was not requested, got {body}"
            );
        }
        assert_eq!(
            aggregates[0]["timestamp"].as_f64().map(|at| at as i64),
            Some(base() + window_seconds("1d")),
            "case {aggregate_type:?}: the aggregate is stamped with its window end, got {body}"
        );
    }

    // A type named twice is the same request as naming it once: the aggregate is
    // computed once and reported once.
    let mut params = query(&asset_id, &speed, &["AVERAGE"], "1d", base(), base() + DAY - 1);
    params.push(("aggregateTypes", "AVERAGE".to_string()));
    params.push(("aggregateTypes", "SUM".to_string()));
    let (status, body) = call(&router, "GET", &aggregates_uri(&params), None).await;
    assert_eq!(status, 200, "a repeated aggregateType: got {status} {body}");
    let ends = base() + window_seconds("1d");
    assert_eq!(
        reported(&body, "average"),
        vec![(ends, mean)],
        "a repeated aggregateType: expected average {mean}, got {body}"
    );
    assert_eq!(
        reported(&body, "sum"),
        vec![(ends, sum)],
        "a repeated aggregateType: expected sum {sum}, got {body}"
    );
}

#[tokio::test]
async fn the_queried_range_excludes_its_start_and_includes_its_end() {
    let router = router();
    let (asset_id, speed) = asset_with_property(&router, "Pump", "Flow", "DOUBLE").await;
    // Two readings in the same `1h` window, so only the range bounds decide which of
    // them is aggregated.
    write_readings(
        &router,
        &asset_id,
        &speed,
        &[(100, 1.0, None), (200, 2.0, None)],
    )
    .await;

    // Rows walk each bound onto, just inside and just outside a reading: `startDate`
    // is "the exclusive start of the range" and `endDate` "the inclusive end". An
    // empty range holds no reading, so it reports no window rather than failing.
    let cases: [(&str, i64, i64, Option<f64>); 7] = [
        ("both readings", 0, 200, Some(3.0)),
        ("start on the first reading", 100, 200, Some(2.0)),
        ("start one second before", 99, 200, Some(3.0)),
        ("end one second before the last", 0, 199, Some(1.0)),
        ("end on the first reading", 0, 100, Some(1.0)),
        ("range holds no reading", 100, 199, None),
        ("an empty range", 100, 100, None),
    ];

    for (case, start, end, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &aggregates_uri(&query(
                &asset_id,
                &speed,
                &["SUM"],
                "1h",
                base() + start,
                base() + end,
            )),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let sums = reported(&body, "sum");
        match expected {
            Some(sum) => assert_eq!(
                sums,
                vec![(base() + window_seconds("1h"), sum)],
                "case {case:?}: expected sum {sum}, got {body}"
            ),
            None => assert!(
                sums.is_empty(),
                "case {case:?}: no reading is in range, so no window is reported, got {body}"
            ),
        }
    }
}

#[tokio::test]
async fn qualities_selects_which_readings_are_aggregated() {
    let router = router();
    let (asset_id, speed) = asset_with_property(&router, "Kiln", "Heat", "DOUBLE").await;
    // One reading per `Quality` the model declares, all in one `1h` window, plus one
    // written with no quality at all.
    write_readings(
        &router,
        &asset_id,
        &speed,
        &[
            (10, 1.0, Some("GOOD")),
            (20, 100.0, Some("BAD")),
            (30, 50.0, Some("UNCERTAIN")),
            (40, 3.0, None),
        ],
    )
    .await;

    // Rows are every `Quality` value (`smithy_query(query="shape", shape="Quality")`)
    // plus the unfiltered request. A reading written with no quality is GOOD data.
    let cases: [(&str, Option<&str>, f64, f64); 4] = [
        ("no filter", None, 4.0, 2.0),
        ("GOOD", Some("GOOD"), 4.0, 2.0),
        ("BAD", Some("BAD"), 100.0, 1.0),
        ("UNCERTAIN", Some("UNCERTAIN"), 50.0, 1.0),
    ];

    for (case, quality, expected_sum, expected_count) in cases {
        let mut params = query(&asset_id, &speed, &["SUM", "COUNT"], "1h", base(), base() + DAY);
        if let Some(quality) = quality {
            params.push(("qualities", quality.to_string()));
        }
        let (status, body) = call(&router, "GET", &aggregates_uri(&params), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let ends = base() + window_seconds("1h");
        assert_eq!(
            reported(&body, "sum"),
            vec![(ends, expected_sum)],
            "case {case:?}: expected sum {expected_sum}, got {body}"
        );
        assert_eq!(
            reported(&body, "count"),
            vec![(ends, expected_count)],
            "case {case:?}: expected count {expected_count}, got {body}"
        );
        assert_eq!(
            body["aggregatedValues"][0]["quality"].as_str(),
            Some(quality.unwrap_or("GOOD")),
            "case {case:?}: the aggregate reports the quality it covers, got {body}"
        );
    }
}

#[tokio::test]
async fn time_ordering_and_max_results_page_the_windows() {
    let router = router();
    let (asset_id, speed) = asset_with_property(&router, "Fan", "Rpm", "DOUBLE").await;
    write_readings(&router, &asset_id, &speed, &READINGS).await;

    // The `1m` windows of READINGS, ascending, each named by the instant it ends:
    // the smallest whole minute at or after the reading it holds, so a reading on a
    // minute boundary (+90000 s) closes the window ending there.
    let ascending: Vec<i64> = [60, 120, 1_020, 4_020, 90_000]
        .iter()
        .map(|end| base() + end)
        .collect();
    let mut descending = ascending.clone();
    descending.reverse();

    // Rows are both `TimeOrdering` values
    // (`smithy_query(query="shape", shape="TimeOrdering")`) plus the request that
    // names none — whose documented default is ASCENDING — each walked at page sizes
    // from one window to more than every window.
    let orderings: [(&str, Option<&str>, &Vec<i64>); 3] = [
        ("default", None, &ascending),
        ("ASCENDING", Some("ASCENDING"), &ascending),
        ("DESCENDING", Some("DESCENDING"), &descending),
    ];
    for (case, ordering, expected) in orderings {
        for page_size in [1_usize, 2, 5, 6] {
            let mut collected: Vec<i64> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let mut params =
                    query(&asset_id, &speed, &["COUNT"], "1m", base(), base() + 2 * DAY);
                params.push(("maxResults", page_size.to_string()));
                if let Some(ordering) = ordering {
                    params.push(("timeOrdering", ordering.to_string()));
                }
                if let Some(token) = token.as_deref() {
                    params.push(("nextToken", token.to_string()));
                }
                let (status, body) = call(&router, "GET", &aggregates_uri(&params), None).await;
                assert_eq!(status, 200, "case {case:?}/{page_size}: got {status} {body}");
                let page = reported(&body, "count");
                assert!(
                    page.len() <= page_size,
                    "case {case:?}/{page_size}: a page carries at most maxResults, got {body}"
                );
                collected.extend(page.iter().map(|(at, _)| *at));
                token = body["nextToken"].as_str().map(str::to_string);
                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {case:?}/{page_size}: pagination must terminate, got {collected:?}"
                );
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                &collected, expected,
                "case {case:?}/{page_size}: expected {expected:?}, got {collected:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_non_numeric_reading_is_counted_but_feeds_no_numeric_aggregate() {
    let router = router();
    let (asset_id, label) = asset_with_property(&router, "Mixer", "Label", "STRING").await;
    let (status, put) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": "s",
            "assetId": asset_id,
            "propertyId": label,
            "propertyValues": [{
                "value": {"stringValue": "running"},
                "timestamp": {"timeInSeconds": base() + 10},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "writing a string reading: got {status} {put}");
    assert_eq!(
        put["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "writing a string reading: expected no errorEntries, got {put}"
    );

    // A non-numeric reading is still a DATA POINT of its window, so COUNT reports it,
    // but it can feed none of the numeric aggregates — so a window that was asked for
    // only those is not reported at all. This is the rule
    // `crate::state::aggregated_values` applies for both aggregate operations.
    let cases: [(&str, &str, Option<f64>); 6] = [
        ("COUNT", "count", Some(1.0)),
        ("AVERAGE", "average", None),
        ("MAXIMUM", "maximum", None),
        ("MINIMUM", "minimum", None),
        ("SUM", "sum", None),
        ("STANDARD_DEVIATION", "standardDeviation", None),
    ];
    for (aggregate_type, member, expected) in cases {
        let (status, body) = call(
            &router,
            "GET",
            &aggregates_uri(&query(
                &asset_id,
                &label,
                &[aggregate_type],
                "1h",
                base(),
                base() + DAY,
            )),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {aggregate_type:?}: got {status} {body}");
        match expected {
            Some(count) => assert_eq!(
                reported(&body, member),
                vec![(base() + window_seconds("1h"), count)],
                "case {aggregate_type:?}: expected {member} {count}, got {body}"
            ),
            None => assert_eq!(
                body["aggregatedValues"].as_array().map(Vec::len),
                Some(0),
                "case {aggregate_type:?}: a string reading feeds no numeric aggregate, \
                 so no window is reported, got {body}"
            ),
        }
    }
}

#[tokio::test]
async fn a_request_that_identifies_no_property_is_a_modelled_refusal() {
    let router = router();
    let absent = "11111111-2222-3333-4444-555555555555";
    let (asset_id, speed) = asset_with_property(&router, "Valve", "Position", "DOUBLE").await;
    write_readings(&router, &asset_id, &speed, &READINGS).await;

    let ok = query(&asset_id, &speed, &["AVERAGE"], "1h", base(), base() + DAY);
    let with = |extra: Vec<(&'static str, String)>| {
        let mut params = ok.clone();
        params.extend(extra);
        aggregates_uri(&params)
    };

    // Rows: an id that resolves to nothing is 404; a request the service cannot read
    // a property or a range out of is 400.
    let cases: [(&str, String, u16, &str); 7] = [
        (
            "unknown asset",
            aggregates_uri(&query(absent, &speed, &["AVERAGE"], "1h", base(), base() + DAY)),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown property",
            aggregates_uri(&query(&asset_id, absent, &["AVERAGE"], "1h", base(), base() + DAY)),
            404,
            "ResourceNotFoundException",
        ),
        (
            "unknown alias",
            aggregates_uri(&[
                ("propertyAlias", "/no/such/stream".to_string()),
                ("resolution", "1h".to_string()),
                ("startDate", iso(base())),
                ("endDate", iso(base() + DAY)),
                ("aggregateTypes", "AVERAGE".to_string()),
            ]),
            404,
            "ResourceNotFoundException",
        ),
        (
            "neither identifying form",
            aggregates_uri(&[
                ("resolution", "1h".to_string()),
                ("startDate", iso(base())),
                ("endDate", iso(base() + DAY)),
                ("aggregateTypes", "AVERAGE".to_string()),
            ]),
            400,
            "InvalidRequestException",
        ),
        (
            "both identifying forms",
            with(vec![("propertyAlias", "/a/b".to_string())]),
            400,
            "InvalidRequestException",
        ),
        (
            "endDate before startDate",
            aggregates_uri(&query(
                &asset_id,
                &speed,
                &["AVERAGE"],
                "1h",
                base() + DAY,
                base(),
            )),
            400,
            "InvalidRequestException",
        ),
        (
            "a nextToken this operation never issued",
            // Hex of "7": a well-formed token whose resume point is in no window.
            with(vec![("nextToken", "37".to_string())]),
            400,
            "InvalidRequestException",
        ),
    ];

    for (case, uri, expected_status, expected_code) in cases {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `maxResults` is bounded at the documented ceiling of 2500, and every enumerated
    // member at the catalogue the model publishes. A bound the model itself carries is
    // refused by the boundary validator rather than by the handler, so only the status
    // is this operation's to promise.
    let bounds: [(&str, String, u16); 9] = [
        (
            "maxResults at the ceiling",
            with(vec![("maxResults", "2500".to_string())]),
            200,
        ),
        (
            "maxResults past the ceiling",
            with(vec![("maxResults", "2501".to_string())]),
            400,
        ),
        (
            "maxResults below the modelled minimum",
            with(vec![("maxResults", "0".to_string())]),
            400,
        ),
        (
            "a resolution the model does not admit",
            aggregates_uri(&query(&asset_id, &speed, &["AVERAGE"], "2m", base(), base() + DAY)),
            400,
        ),
        (
            "a quality the model does not declare",
            with(vec![("qualities", "good".to_string())]),
            400,
        ),
        (
            "a timeOrdering the model does not declare",
            with(vec![("timeOrdering", "SIDEWAYS".to_string())]),
            400,
        ),
        (
            "an aggregateType the model does not declare",
            with(vec![("aggregateTypes", "MEDIAN".to_string())]),
            400,
        ),
        (
            // Six aggregate functions exist, so five more entries beside the base
            // request's AVERAGE is the longest list that can name each one once.
            "aggregateTypes as long as there are aggregate types",
            with(
                ["COUNT", "MAXIMUM", "MINIMUM", "SUM", "STANDARD_DEVIATION"]
                    .iter()
                    .map(|ty| ("aggregateTypes", ty.to_string()))
                    .collect(),
            ),
            200,
        ),
        (
            "aggregateTypes longer than there are aggregate types",
            with(
                [
                    "COUNT",
                    "MAXIMUM",
                    "MINIMUM",
                    "SUM",
                    "STANDARD_DEVIATION",
                    "AVERAGE",
                ]
                .iter()
                .map(|ty| ("aggregateTypes", ty.to_string()))
                .collect(),
            ),
            400,
        ),
    ];
    for (case, uri, expected_status) in bounds {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {status} {body}"
        );
    }
}

#[tokio::test]
async fn reading_the_aggregates_changes_nothing_a_later_caller_sees() {
    let router = router();
    let (asset_id, speed) = asset_with_property(&router, "Press", "Force", "DOUBLE").await;
    write_readings(&router, &asset_id, &speed, &READINGS).await;

    let uri = aggregates_uri(&query(
        &asset_id,
        &speed,
        &["AVERAGE", "COUNT", "SUM"],
        "1h",
        base(),
        base() + 2 * DAY,
    ));
    let (status, first) = call(&router, "GET", &uri, None).await;
    assert_eq!(status, 200, "first read: got {status} {first}");
    let (status, again) = call(&router, "GET", &uri, None).await;
    assert_eq!(status, 200, "second read: got {status} {again}");
    assert_eq!(
        first, again,
        "a repeated read must report the same aggregates: {first} then {again}"
    );

    // And the series itself is untouched: the latest value still reads back.
    let (status, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={speed}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "GetAssetPropertyValue: got {status} {latest}");
    assert_eq!(
        latest["propertyValue"]["value"]["doubleValue"].as_f64(),
        Some(20.0),
        "the aggregate read must not disturb the stored series, got {latest}"
    );
}
