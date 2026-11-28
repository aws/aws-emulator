// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `BatchGetAssetPropertyAggregates`, driven over HTTP through
//! the same `MultiServiceRouter` the emulator boots with.
//!
//! The rule these suites pin: an aggregate entry answers from the samples
//! `BatchPutAssetPropertyValue` persisted for the property it names — the samples of
//! the asked-for quality whose timestamp is after `startDate` and no later than
//! `endDate`, bucketed into epoch-aligned windows of the entry's `resolution`, each
//! window reduced to the requested `aggregateTypes` — so the answer is a function of
//! the stored series and of nothing else.
//!
//! Every suite is a TABLE whose rows differ along what the rule generalises over:
//! the window length, the aggregate function, the range bounds, the quality, the
//! time ordering, the page size and the ways an entry fails to resolve.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

/// The region the test signs for.
const REGION: &str = "us-west-2";

/// The wire path `BatchGetAssetPropertyAggregates` is bound to, from
/// `smithy_query(query="operation", operation="BatchGetAssetPropertyAggregates")`:
/// `POST /properties/batch/aggregates`.
const AGGREGATES: &str = "/properties/batch/aggregates";

/// A Unix instant aligned to a whole day, hence also to every shorter modelled
/// resolution (1m, 15m, 1h): window starts are then `base() + k * resolution`.
///
/// Two to three days in the past, and resolved ONCE for the whole test binary, so
/// that it is both stable across the calls of one case and recent enough for
/// `BatchPutAssetPropertyValue` to accept the samples — that operation refuses a
/// timestamp more than 7 days old or more than 10 minutes in the future.
fn base() -> i64 {
    static BASE: std::sync::OnceLock<i64> = std::sync::OnceLock::new();
    *BASE.get_or_init(|| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("a clock after the Unix epoch")
            .as_secs() as i64;
        let two_days_ago = now - 2 * 86_400;
        two_days_ago - two_days_ago.rem_euclid(86_400)
    })
}

/// How close two doubles must be to count as the same aggregate.
const EPSILON: f64 = 1e-9;

/// The length in seconds of the window each modelled `Resolution` names.
///
/// Source: `smithy_query(query="shape", shape="Resolution")` — length 2..3 and
/// pattern `^1m|15m|1h|1d$`, i.e. one minute, fifteen minutes, one hour and one
/// day. An aggregate is stamped with the END of its window, so this is what every
/// expected timestamp below adds to the window's start.
fn window_seconds(resolution: &str) -> i64 {
    match resolution {
        "1m" => 60,
        "15m" => 900,
        "1h" => 3_600,
        "1d" => 86_400,
        other => panic!("{other} is not a modelled Resolution"),
    }
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

/// Create an asset model carrying one measurement property of `data_type` named
/// `property`, create an asset of it, and return `(assetId, propertyId)`.
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

/// One sample to persist: `(offset from `base()` in seconds, doubleValue, quality)`.
type Sample = (i64, f64, Option<&'static str>);

/// Persist `samples` on `(asset_id, property_id)` through `BatchPutAssetPropertyValue`,
/// ten at a time (the modelled per-entry maximum).
async fn write_samples(
    router: &MultiServiceRouter,
    asset_id: &str,
    property_id: &str,
    samples: &[Sample],
) {
    for (chunk, batch) in samples.chunks(10).enumerate() {
        let values: Vec<Value> = batch
            .iter()
            .map(|(offset, value, quality)| {
                let mut sample = json!({
                    "value": {"doubleValue": value},
                    "timestamp": {"timeInSeconds": base() + offset},
                });
                if let Some(quality) = quality {
                    sample["quality"] = json!(quality);
                }
                sample
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
            "write chunk {chunk}: every sample must be accepted, got {body}"
        );
    }
}

/// One aggregate entry naming a property by id.
fn entry(
    entry_id: &str,
    asset_id: &str,
    property_id: &str,
    resolution: &str,
    aggregate_types: &[&str],
    start: i64,
    end: i64,
) -> Value {
    json!({
        "entryId": entry_id,
        "assetId": asset_id,
        "propertyId": property_id,
        "aggregateTypes": aggregate_types,
        "resolution": resolution,
        "startDate": start,
        "endDate": end,
    })
}

/// The one success entry of `body`, panicking when the response carries anything
/// else.
fn only_success(case: &str, body: &Value) -> Value {
    assert_eq!(
        body["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "case {case:?}: expected no error entries, got {body}"
    );
    let entries = body["successEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no successEntries in {body}"));
    assert_eq!(
        entries.len(),
        1,
        "case {case:?}: expected exactly one success entry, got {body}"
    );
    entries[0].clone()
}

/// The aggregated values of a success entry.
fn aggregated(case: &str, success: &Value) -> Vec<Value> {
    success["aggregatedValues"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no aggregatedValues in {success}"))
        .clone()
}

/// Assert `actual` is within [`EPSILON`] of `expected`.
fn assert_close(case: &str, what: &str, actual: &Value, expected: f64) {
    let actual = actual.as_f64();
    assert!(
        actual.is_some_and(|actual| (actual - expected).abs() < EPSILON),
        "case {case:?}: expected {what} {expected}, got {actual:?}"
    );
}

#[tokio::test]
async fn aggregates_bucket_the_series_on_the_epoch_aligned_resolution_grid() {
    let router = router();
    let (asset_id, property_id) =
        asset_with_property(&router, "Boiler", "Temperature", "DOUBLE").await;
    // Five samples: three in the first hour (two of them in the first minute, one
    // in each of the next two) and one in the second hour.
    let samples: [Sample; 5] = [
        (10, 1.0, Some("GOOD")),
        (20, 3.0, Some("GOOD")),
        (70, 5.0, Some("GOOD")),
        (130, 7.0, Some("GOOD")),
        (3_700, 9.0, Some("GOOD")),
    ];
    write_samples(&router, &asset_id, &property_id, &samples).await;

    /// One expected window: `(offset from base(), count, sum, average, minimum,
    /// maximum, standard deviation)`.
    type Window = (i64, f64, f64, f64, f64, f64, f64);
    // Rows are every `Resolution` the model admits (`^1m|15m|1h|1d$`), including
    // "15m", which the issue never cited. The expected windows are written out by
    // hand rather than recomputed, so they are an independent oracle.
    let cases: [(&str, Vec<Window>); 4] = [
        (
            "1m",
            vec![
                (0, 2.0, 4.0, 2.0, 1.0, 3.0, 1.0),
                (60, 1.0, 5.0, 5.0, 5.0, 5.0, 0.0),
                (120, 1.0, 7.0, 7.0, 7.0, 7.0, 0.0),
                (3_660, 1.0, 9.0, 9.0, 9.0, 9.0, 0.0),
            ],
        ),
        (
            "15m",
            vec![
                (0, 4.0, 16.0, 4.0, 1.0, 7.0, 5.0_f64.sqrt()),
                (3_600, 1.0, 9.0, 9.0, 9.0, 9.0, 0.0),
            ],
        ),
        (
            "1h",
            vec![
                (0, 4.0, 16.0, 4.0, 1.0, 7.0, 5.0_f64.sqrt()),
                (3_600, 1.0, 9.0, 9.0, 9.0, 9.0, 0.0),
            ],
        ),
        ("1d", vec![(0, 5.0, 25.0, 5.0, 1.0, 9.0, 8.0_f64.sqrt())]),
    ];

    for (resolution, expected) in cases {
        let (status, body) = call(
            &router,
            "POST",
            AGGREGATES,
            Some(json!({"entries": [entry(
                "r",
                &asset_id,
                &property_id,
                resolution,
                &["COUNT", "SUM", "AVERAGE", "MINIMUM", "MAXIMUM", "STANDARD_DEVIATION"],
                base(),
                base() + 7_200,
            )]})),
        )
        .await;
        assert_eq!(status, 200, "case {resolution:?}: got {status} {body}");
        let success = only_success(resolution, &body);
        assert_eq!(
            success["entryId"], "r",
            "case {resolution:?}: the success entry must carry the requested entryId, got {success}"
        );
        assert!(
            body["nextToken"].is_null(),
            "case {resolution:?}: a complete answer carries no nextToken, got {body}"
        );
        let values = aggregated(resolution, &success);
        assert_eq!(
            values.len(),
            expected.len(),
            "case {resolution:?}: expected {} windows, got {values:?}",
            expected.len()
        );
        for (at, (offset, count, sum, average, minimum, maximum, deviation)) in
            expected.iter().enumerate()
        {
            let case = format!("{resolution} window {offset}");
            let value = &values[at];
            // A window is stamped with the instant it ENDS: its start plus the
            // length of the resolution's window.
            assert_close(
                &case,
                "timestamp",
                &value["timestamp"],
                (base() + offset + window_seconds(resolution)) as f64,
            );
            assert_eq!(
                value["quality"], "GOOD",
                "case {case:?}: expected quality GOOD, got {value}"
            );
            assert_close(&case, "count", &value["value"]["count"], *count);
            assert_close(&case, "sum", &value["value"]["sum"], *sum);
            assert_close(&case, "average", &value["value"]["average"], *average);
            assert_close(&case, "minimum", &value["value"]["minimum"], *minimum);
            assert_close(&case, "maximum", &value["value"]["maximum"], *maximum);
            assert_close(
                &case,
                "standardDeviation",
                &value["value"]["standardDeviation"],
                *deviation,
            );
        }
    }
}

#[tokio::test]
async fn only_the_requested_aggregate_types_are_reported() {
    let router = router();
    let (asset_id, property_id) = asset_with_property(&router, "Pump", "Flow", "DOUBLE").await;
    write_samples(
        &router,
        &asset_id,
        &property_id,
        &[(10, 2.0, None), (20, 6.0, None)],
    )
    .await;

    // Rows are every `AggregateType` the model declares
    // (`smithy_query(query="shape", shape="AggregateType")`), one per row, plus one
    // row asking for two at once: the response must carry exactly the asked-for
    // members of `Aggregates` and no other.
    /// One row: the case name, the `aggregateTypes` it asks for, and the
    /// `(Aggregates member, value)` pairs the answer must carry.
    type Row = (&'static str, Vec<&'static str>, Vec<(&'static str, f64)>);
    let cases: [Row; 7] = [
        ("AVERAGE", vec!["AVERAGE"], vec![("average", 4.0)]),
        ("COUNT", vec!["COUNT"], vec![("count", 2.0)]),
        ("MAXIMUM", vec!["MAXIMUM"], vec![("maximum", 6.0)]),
        ("MINIMUM", vec!["MINIMUM"], vec![("minimum", 2.0)]),
        ("SUM", vec!["SUM"], vec![("sum", 8.0)]),
        (
            "STANDARD_DEVIATION",
            vec!["STANDARD_DEVIATION"],
            vec![("standardDeviation", 2.0)],
        ),
        (
            "two at once",
            vec!["MINIMUM", "MAXIMUM"],
            vec![("minimum", 2.0), ("maximum", 6.0)],
        ),
    ];
    let every_member = [
        "average",
        "count",
        "maximum",
        "minimum",
        "sum",
        "standardDeviation",
    ];

    for (case, aggregate_types, expected) in cases {
        let (status, body) = call(
            &router,
            "POST",
            AGGREGATES,
            Some(json!({"entries": [entry(
                "a",
                &asset_id,
                &property_id,
                "1h",
                &aggregate_types,
                base(),
                base() + 3_600,
            )]})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let success = only_success(case, &body);
        let values = aggregated(case, &success);
        assert_eq!(
            values.len(),
            1,
            "case {case:?}: expected one window, got {values:?}"
        );
        let aggregates = &values[0]["value"];
        for (member, value) in &expected {
            assert_close(case, member, &aggregates[member], *value);
        }
        for member in every_member {
            if expected.iter().any(|(asked, _)| *asked == member) {
                continue;
            }
            assert!(
                aggregates.get(member).is_none_or(Value::is_null),
                "case {case:?}: {member} was not requested, so it must not be reported: {aggregates}"
            );
        }
    }
}

#[tokio::test]
async fn the_queried_range_excludes_its_start_and_includes_its_end() {
    let router = router();
    let (asset_id, property_id) = asset_with_property(&router, "Mixer", "Rpm", "DOUBLE").await;
    // Three samples one window apart in nothing: a single 1d window holds them all,
    // so only the range bounds can change the count.
    write_samples(
        &router,
        &asset_id,
        &property_id,
        &[(10, 1.0, None), (20, 1.0, None), (70, 1.0, None)],
    )
    .await;

    // The rule: "The exclusive start of the range" / "The inclusive end of the
    // range" (the `startDate` / `endDate` docs). Rows put each bound exactly on a
    // sample, one second either side of one, and on an empty range.
    let cases: [(&str, i64, i64, Option<f64>); 6] = [
        ("start on a sample excludes it", 10, 70, Some(2.0)),
        ("start one second earlier includes it", 9, 70, Some(3.0)),
        ("end on a sample includes it", 10, 20, Some(1.0)),
        ("end one second earlier excludes it", 10, 19, None),
        ("an empty range reports no window", 70, 70, None),
        ("a range before every sample", -100, 9, None),
    ];

    for (case, start, end, expected) in cases {
        let (status, body) = call(
            &router,
            "POST",
            AGGREGATES,
            Some(json!({"entries": [entry(
                "b",
                &asset_id,
                &property_id,
                "1d",
                &["COUNT"],
                base() + start,
                base() + end,
            )]})),
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let success = only_success(case, &body);
        let values = aggregated(case, &success);
        match expected {
            Some(count) => {
                assert_eq!(
                    values.len(),
                    1,
                    "case {case:?}: expected one window, got {values:?}"
                );
                assert_close(case, "count", &values[0]["value"]["count"], count);
            }
            None => assert!(
                values.is_empty(),
                "case {case:?}: expected no window at all, got {values:?}"
            ),
        }
    }
}

#[tokio::test]
async fn an_entry_aggregates_only_the_samples_of_the_quality_it_filters_on() {
    let router = router();
    let (asset_id, property_id) = asset_with_property(&router, "Kiln", "Heat", "DOUBLE").await;
    // One sample per `Quality` the model declares, plus one written without a
    // quality at all — which IoT SiteWise treats as GOOD.
    write_samples(
        &router,
        &asset_id,
        &property_id,
        &[
            (10, 1.0, Some("GOOD")),
            (20, 2.0, Some("BAD")),
            (30, 4.0, Some("UNCERTAIN")),
            (40, 8.0, None),
        ],
    )
    .await;

    // Rows: each declared quality, and the entry that names none.
    let cases: [(&str, Option<&str>, f64, f64, &str); 4] = [
        ("no filter defaults to GOOD", None, 2.0, 9.0, "GOOD"),
        ("GOOD", Some("GOOD"), 2.0, 9.0, "GOOD"),
        ("BAD", Some("BAD"), 1.0, 2.0, "BAD"),
        ("UNCERTAIN", Some("UNCERTAIN"), 1.0, 4.0, "UNCERTAIN"),
    ];

    for (case, quality, count, sum, reported) in cases {
        let mut asked = entry(
            "q",
            &asset_id,
            &property_id,
            "1d",
            &["COUNT", "SUM"],
            base(),
            base() + 3_600,
        );
        if let Some(quality) = quality {
            asked["qualities"] = json!([quality]);
        }
        let (status, body) = call(&router, "POST", AGGREGATES, Some(json!({"entries": [asked]}))).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let success = only_success(case, &body);
        let values = aggregated(case, &success);
        assert_eq!(
            values.len(),
            1,
            "case {case:?}: expected one window, got {values:?}"
        );
        assert_close(case, "count", &values[0]["value"]["count"], count);
        assert_close(case, "sum", &values[0]["value"]["sum"], sum);
        assert_eq!(
            values[0]["quality"], reported,
            "case {case:?}: expected the filtered quality {reported} to be reported, got {}",
            values[0]
        );
    }
}

#[tokio::test]
async fn time_ordering_reverses_the_windows_without_changing_them() {
    let router = router();
    let (asset_id, property_id) = asset_with_property(&router, "Fan", "Speed", "DOUBLE").await;
    write_samples(
        &router,
        &asset_id,
        &property_id,
        &[(10, 1.0, None), (70, 2.0, None), (130, 3.0, None)],
    )
    .await;

    // Rows: the two `TimeOrdering` values, plus the entry that names none — whose
    // documented default is ASCENDING. Each window is stamped with the instant it
    // ends, so the `1m` windows starting at 0, 60 and 120 report 60, 120 and 180.
    let ascending = vec![60_i64, 120, 180];
    let descending = vec![180_i64, 120, 60];
    let cases: [(&str, Option<&str>, Vec<i64>); 3] = [
        ("default", None, ascending.clone()),
        ("ASCENDING", Some("ASCENDING"), ascending),
        ("DESCENDING", Some("DESCENDING"), descending),
    ];

    for (case, ordering, expected) in cases {
        let mut asked = entry(
            "o",
            &asset_id,
            &property_id,
            "1m",
            &["COUNT"],
            base(),
            base() + 3_600,
        );
        if let Some(ordering) = ordering {
            asked["timeOrdering"] = json!(ordering);
        }
        let (status, body) = call(&router, "POST", AGGREGATES, Some(json!({"entries": [asked]}))).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        let success = only_success(case, &body);
        let values = aggregated(case, &success);
        let reported: Vec<i64> = values
            .iter()
            .map(|value| value["timestamp"].as_f64().unwrap_or(0.0) as i64 - base())
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the windows at {expected:?}, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn max_results_pages_the_batch_and_skips_entries_earlier_pages_finished() {
    let router = router();
    let (asset_id, first) = asset_with_property(&router, "Press", "Force", "DOUBLE").await;
    // A second asset gives the batch two entries over two different series while
    // keeping each fixture model to one property.
    let (other_asset, other_property) =
        asset_with_property(&router, "Press2", "Force", "DOUBLE").await;

    write_samples(
        &router,
        &asset_id,
        &first,
        &[(10, 1.0, None), (70, 2.0, None), (130, 3.0, None)],
    )
    .await;
    write_samples(
        &router,
        &other_asset,
        &other_property,
        &[(10, 4.0, None), (70, 5.0, None), (130, 6.0, None)],
    )
    .await;

    let entries = vec![
        entry("one", &asset_id, &first, "1m", &["SUM"], base(), base() + 3_600),
        entry(
            "two",
            &other_asset,
            &other_property,
            "1m",
            &["SUM"],
            base(),
            base() + 3_600,
        ),
    ];

    // The unpaginated answer is the oracle: paging must deliver exactly it, in
    // order, whatever the page size.
    let (status, whole) = call(
        &router,
        "POST",
        AGGREGATES,
        Some(json!({"entries": entries.clone()})),
    )
    .await;
    assert_eq!(status, 200, "unpaginated: got {status} {whole}");
    assert!(
        whole["nextToken"].is_null(),
        "unpaginated: expected no nextToken, got {whole}"
    );
    let expected: Vec<(String, f64)> = sums(&whole);
    assert_eq!(
        expected.len(),
        6,
        "unpaginated: expected six data points, got {expected:?}"
    );

    // Rows: page sizes that split inside an entry, exactly on its boundary, and
    // wider than the whole batch.
    let ids: Vec<String> = entries
        .iter()
        .map(|entry| entry["entryId"].as_str().unwrap_or("").to_string())
        .collect();
    // The oracle, per entry: `maxResults` spreads the page across the entries, so
    // what a walk must reproduce is each entry's OWN ordered run, not the order the
    // unpaginated answer happens to concatenate them in.
    let expected_by_entry: Vec<(String, Vec<f64>)> = ids
        .iter()
        .map(|id| {
            (
                id.clone(),
                expected
                    .iter()
                    .filter(|(entry_id, _)| entry_id == id)
                    .map(|(_, sum)| *sum)
                    .collect(),
            )
        })
        .collect();
    for max_results in [1_usize, 2, 3, 4, 6, 7] {
        let case = format!("maxResults={max_results}");
        let mut collected: Vec<(String, Vec<f64>)> =
            ids.iter().map(|id| (id.clone(), Vec::new())).collect();
        let mut token: Option<String> = None;
        for page in 0..10 {
            let mut request = json!({"entries": entries.clone(), "maxResults": max_results});
            if let Some(token) = &token {
                request["nextToken"] = json!(token);
            }
            let (status, body) = call(&router, "POST", AGGREGATES, Some(request)).await;
            assert_eq!(status, 200, "case {case:?} page {page}: got {status} {body}");
            assert_eq!(
                body["errorEntries"].as_array().map(Vec::len),
                Some(0),
                "case {case:?} page {page}: expected no error entries, got {body}"
            );
            let points = sums(&body);
            assert!(
                points.len() <= max_results,
                "case {case:?} page {page}: expected at most {max_results} data points, got {points:?}"
            );
            // Every entry a previous page completely processed is reported as
            // skipped on this one, and as having completed successfully. An entry
            // finishes when ITS windows run out, which under one budget spread
            // across the entries need not be in the order the request lists them.
            let skipped: Vec<String> = body["skippedEntries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|skipped| {
                    assert_eq!(
                        skipped["completionStatus"], "SUCCESS",
                        "case {case:?} page {page}: a resolvable entry completes successfully, got {skipped}"
                    );
                    assert!(
                        skipped["errorInfo"].is_null(),
                        "case {case:?} page {page}: a successful entry carries no errorInfo, got {skipped}"
                    );
                    skipped["entryId"].as_str().unwrap_or("").to_string()
                })
                .collect();
            let expected_skipped: Vec<String> = expected_by_entry
                .iter()
                .zip(&collected)
                .filter(|((_, want), (_, got))| want == got)
                .map(|((id, _), _)| id.clone())
                .collect();
            assert_eq!(
                skipped, expected_skipped,
                "case {case:?} page {page}: the skipped entries are exactly the ones \
                 earlier pages finished, got {skipped:?}"
            );
            assert!(
                skipped
                    .iter()
                    .all(|id| points.iter().all(|(answered, _)| answered != id)),
                "case {case:?} page {page}: an entry cannot be both skipped and answered, got {body}"
            );
            for (answered, sum) in points {
                let slot = collected
                    .iter_mut()
                    .find(|(id, _)| *id == answered)
                    .unwrap_or_else(|| {
                        panic!("case {case:?} page {page}: {answered} is not an entry of this request")
                    });
                slot.1.push(sum);
            }
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: paging did not terminate within ten pages"
        );
        assert_eq!(
            collected, expected_by_entry,
            "case {case:?}: paging must deliver every entry's own ordered run exactly once"
        );
    }
}

/// Every `(entryId, sum)` data point of a response, in the order it is reported.
fn sums(body: &Value) -> Vec<(String, f64)> {
    body["successEntries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .flat_map(|success| {
            let id = success["entryId"].as_str().unwrap_or("").to_string();
            success["aggregatedValues"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|value| (id.clone(), value["value"]["sum"].as_f64().unwrap_or(f64::NAN)))
                .collect::<Vec<(String, f64)>>()
        })
        .collect()
}

#[tokio::test]
async fn an_entry_that_cannot_be_answered_is_refused_on_its_own() {
    let router = router();
    let absent = "11111111-2222-3333-4444-555555555555";
    let (asset_id, property_id) = asset_with_property(&router, "Valve", "Position", "DOUBLE").await;
    write_samples(&router, &asset_id, &property_id, &[(10, 1.0, None)]).await;

    // Rows span the ways one entry fails while the rest of the batch is answered:
    // the error codes are `BatchGetAssetPropertyAggregatesErrorCode`
    // (`ResourceNotFoundException`, `InvalidRequestException`, `AccessDeniedException`).
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "answered",
            entry("ok", &asset_id, &property_id, "1m", &["COUNT"], base(), base() + 60),
            None,
        ),
        (
            "unknown asset",
            entry("noasset", absent, &property_id, "1m", &["COUNT"], base(), base() + 60),
            Some("ResourceNotFoundException"),
        ),
        (
            "unknown property",
            entry("noproperty", &asset_id, absent, "1m", &["COUNT"], base(), base() + 60),
            Some("ResourceNotFoundException"),
        ),
        (
            "unknown alias",
            json!({
                "entryId": "noalias",
                "propertyAlias": "/no/such/stream",
                "aggregateTypes": ["COUNT"],
                "resolution": "1m",
                "startDate": base(),
                "endDate": base() + 60,
            }),
            Some("ResourceNotFoundException"),
        ),
        (
            "no identifier at all",
            json!({
                "entryId": "noid",
                "aggregateTypes": ["COUNT"],
                "resolution": "1m",
                "startDate": base(),
                "endDate": base() + 60,
            }),
            Some("InvalidRequestException"),
        ),
        (
            "both identifier forms",
            json!({
                "entryId": "bothforms",
                "assetId": asset_id,
                "propertyId": property_id,
                "propertyAlias": "/some/stream",
                "aggregateTypes": ["COUNT"],
                "resolution": "1m",
                "startDate": base(),
                "endDate": base() + 60,
            }),
            Some("InvalidRequestException"),
        ),
        (
            "startDate after endDate",
            entry("backwards", &asset_id, &property_id, "1m", &["COUNT"], base() + 60, base()),
            Some("InvalidRequestException"),
        ),
    ];

    let entries: Vec<Value> = cases.iter().map(|(_, entry, _)| entry.clone()).collect();
    let (status, body) = call(
        &router,
        "POST",
        AGGREGATES,
        Some(json!({"entries": entries})),
    )
    .await;
    assert_eq!(status, 200, "a batch of mixed entries: got {status} {body}");

    for (case, asked, expected) in &cases {
        let id = asked["entryId"].as_str().unwrap_or("");
        let reported = body["errorEntries"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|reported| reported["entryId"] == id);
        match expected {
            Some(code) => {
                let reported = reported.unwrap_or_else(|| {
                    panic!("case {case:?}: expected an error entry for {id}, got {body}")
                });
                assert_eq!(
                    reported["errorCode"], *code,
                    "case {case:?}: expected errorCode {code}, got {reported}"
                );
                assert!(
                    reported["errorMessage"]
                        .as_str()
                        .is_some_and(|message| !message.is_empty()),
                    "case {case:?}: an error entry must say why, got {reported}"
                );
                assert!(
                    body["successEntries"]
                        .as_array()
                        .is_none_or(|entries| entries.iter().all(|entry| entry["entryId"] != id)),
                    "case {case:?}: a refused entry must not also succeed, got {body}"
                );
            }
            None => {
                assert!(
                    reported.is_none(),
                    "case {case:?}: expected no error entry for {id}, got {reported:?}"
                );
                let success = body["successEntries"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                    .into_iter()
                    .find(|success| success["entryId"] == id)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: expected a success entry for {id}, got {body}")
                    });
                assert_close(case, "count", &success["aggregatedValues"][0]["value"]["count"], 1.0);
            }
        }
    }
}

#[tokio::test]
async fn the_request_itself_is_bounded_at_sixteen_entries_and_four_thousand_points() {
    let router = router();
    let (asset_id, property_id) = asset_with_property(&router, "Gearbox", "Torque", "DOUBLE").await;
    write_samples(&router, &asset_id, &property_id, &[(10, 1.0, None)]).await;

    let one = |id: &str| entry(id, &asset_id, &property_id, "1m", &["COUNT"], base(), base() + 60);

    // Rows are the published bound and one past it at each end: "You can specify
    // up to 16 entries per request", and a batch naming no entry asks for nothing.
    for (count, accepted) in [(0_usize, false), (1, true), (16, true), (17, false)] {
        let entries: Vec<Value> = (0..count).map(|at| one(&format!("e{at}"))).collect();
        let (status, body) = call(
            &router,
            "POST",
            AGGREGATES,
            Some(json!({"entries": entries})),
        )
        .await;
        if accepted {
            assert_eq!(status, 200, "case {count} entries: got {status} {body}");
            assert_eq!(
                body["successEntries"].as_array().map(Vec::len),
                Some(count),
                "case {count} entries: expected one success entry each, got {body}"
            );
        } else {
            assert_eq!(status, 400, "case {count} entries: got {status} {body}");
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {count} entries: unexpected error code in {body}"
            );
        }
    }

    // Rows are the published ceiling on `maxResults` and one past it: "The maximum
    // value of maxResults is 4000."
    for (max_results, accepted) in [(1_i64, true), (4_000, true), (4_001, false)] {
        let (status, body) = call(
            &router,
            "POST",
            AGGREGATES,
            Some(json!({"entries": [one("m")], "maxResults": max_results})),
        )
        .await;
        if accepted {
            assert_eq!(status, 200, "case maxResults={max_results}: got {status} {body}");
        } else {
            assert_eq!(status, 400, "case maxResults={max_results}: got {status} {body}");
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case maxResults={max_results}: unexpected error code in {body}"
            );
        }
    }

    // A `nextToken` this operation never issued is refused rather than answered
    // with a page.
    for token in ["zz", &hex_token("9:0"), &hex_token("0:7"), &hex_token("nope")] {
        let (status, body) = call(
            &router,
            "POST",
            AGGREGATES,
            Some(json!({"entries": [one("t")], "nextToken": token})),
        )
        .await;
        assert_eq!(status, 400, "case nextToken={token:?}: got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case nextToken={token:?}: unexpected error code in {body}"
        );
    }
}

/// `text` hex-encoded, the form this operation's `nextToken` takes.
///
/// Hand-rolled on purpose, as an INDEPENDENT oracle: the handler encodes its token
/// with the `hex` crate, so re-using that crate here would let one encoding bug
/// agree with itself and pass.
fn hex_token(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn a_value_outside_the_modelled_catalog_is_refused_never_answered() {
    let router = router();
    let (asset_id, property_id) = asset_with_property(&router, "Blower", "Draft", "DOUBLE").await;
    write_samples(&router, &asset_id, &property_id, &[(10, 1.0, None)]).await;

    // Rows take each enumerated member of an entry one step outside the catalogue
    // the model publishes: `Resolution` (`^1m|15m|1h|1d$`), `AggregateType`,
    // `Quality` and `TimeOrdering`. Whether the refusal lands as a request-level
    // constraint failure or as this entry's own `InvalidRequestException`, what must
    // never happen is an answer: the emulator cannot aggregate on a rule it has no
    // definition for.
    let cases: [(&str, &str, Value); 4] = [
        ("resolution", "resolution", json!("1mm")),
        ("aggregateTypes", "aggregateTypes", json!(["TOTAL"])),
        ("qualities", "qualities", json!(["PERFECT"])),
        ("timeOrdering", "timeOrdering", json!("SIDEWAYS")),
    ];

    for (case, member, value) in cases {
        let mut asked = entry(
            "c",
            &asset_id,
            &property_id,
            "1m",
            &["COUNT"],
            base(),
            base() + 60,
        );
        asked[member] = value.clone();
        let (status, body) =
            call(&router, "POST", AGGREGATES, Some(json!({"entries": [asked]}))).await;
        if status == 200 {
            let refused = body["errorEntries"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|reported| reported["entryId"] == "c")
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {member}={value} was answered rather than refused: {body}")
                });
            assert_eq!(
                refused["errorCode"], "InvalidRequestException",
                "case {case:?}: expected errorCode InvalidRequestException, got {refused}"
            );
            assert_eq!(
                body["successEntries"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: a refused entry must not also succeed, got {body}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected a client refusal, got {status} {body}"
            );
        }
    }
}
