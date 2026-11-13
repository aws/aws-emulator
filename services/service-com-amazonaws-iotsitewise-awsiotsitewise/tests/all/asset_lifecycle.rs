// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the IoT SiteWise asset-model -> asset -> property-value
//! slice, driven over HTTP through the same `MultiServiceRouter` the emulator
//! boots with, so every assertion is about the wire behaviour a real client sees.
//!
//! The rule these suites pin: every one of these operations answers from the one
//! shared engine store, so a resource reads back exactly as it was created, a
//! property reads back the latest value written to it, and an id that resolves to
//! no stored resource is a modelled refusal rather than a fabricated answer.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_iotsitewise_awsiotsitewise::state::{
    service_state, MAX_SAMPLES_PER_SERIES,
};
use service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService;

/// The region the test signs for; every ARN the emulator mints must carry it.
pub(crate) const REGION: &str = "us-west-2";

/// A router with just this service mounted on a fresh shared engine.
///
/// This module is the crate's test harness: the sibling suites under `tests/all/`
/// reach the helpers below rather than growing a second copy of them.
pub(crate) fn router() -> MultiServiceRouter {
    router_with_engine().0
}

/// The same, keeping a handle on the shared engine so a test can read the store
/// directly — the only way to observe a bound the API exposes no reader for.
pub(crate) fn router_with_engine() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Pin authentication off so the suite is independent of the process-wide switch.
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (router, engine)
}

/// Dispatch one request and return `(status, parsed body)`.
pub(crate) async fn call(
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
        // The SigV4 credential scope is what routes the request to this service and
        // what the handlers read the ARN region from.
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
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type").and_then(Value::as_str).unwrap_or("")
}

/// A `nextToken` in the form `ListAssets` issues: the hex of the asset id the next
/// page resumes from.
///
/// Hand-rolled on purpose, as an INDEPENDENT oracle: the handler encodes its token
/// with the `hex` crate, so re-using that crate here would let one encoding bug
/// agree with itself and pass.
pub(crate) fn hex_token(asset_id: &str) -> String {
    asset_id
        .as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// `true` when `id` has the shape every `ID` member models: a lowercase
/// hyphenated UUID that is not the all-zero one.
pub(crate) fn is_modelled_id(id: &str) -> bool {
    id.len() == 36
        && id != "00000000-0000-0000-0000-000000000000"
        && id
            .chars()
            .enumerate()
            .all(|(at, ch)| match at {
                8 | 13 | 18 | 23 => ch == '-',
                _ => ch.is_ascii_digit() || ('a'..='f').contains(&ch),
            })
}

/// Create an asset model with `properties` and return its id.
pub(crate) async fn create_model(
    router: &MultiServiceRouter,
    name: &str,
    properties: Value,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/asset-models",
        Some(json!({ "assetModelName": name, "assetModelProperties": properties })),
    )
    .await;
    assert_eq!(status, 202, "create model {name}: got {status} {body}");
    body["assetModelId"]
        .as_str()
        .unwrap_or_else(|| panic!("create model {name}: no assetModelId in {body}"))
        .to_string()
}

/// Create an asset of `asset_model_id` and return its id.
pub(crate) async fn create_asset(
    router: &MultiServiceRouter,
    name: &str,
    asset_model_id: &str,
) -> String {
    let (status, body) = call(
        router,
        "POST",
        "/assets",
        Some(json!({ "assetName": name, "assetModelId": asset_model_id })),
    )
    .await;
    assert_eq!(status, 202, "create asset {name}: got {status} {body}");
    body["assetId"]
        .as_str()
        .unwrap_or_else(|| panic!("create asset {name}: no assetId in {body}"))
        .to_string()
}

/// The property id of `name` in a `DescribeAssetModel` / `DescribeAsset` body.
pub(crate) fn property_id(properties: &Value, name: &str) -> String {
    properties
        .as_array()
        .unwrap_or_else(|| panic!("expected a property list, got {properties}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("no property named {name} in {properties}"))["id"]
        .as_str()
        .unwrap_or_else(|| panic!("property {name} has no id in {properties}"))
        .to_string()
}

/// One measurement property per `PropertyDataType` a measurement can carry, plus
/// one attribute. Source: `smithy_query(query="shape", shape="PropertyDataType")`
/// — STRING | INTEGER | DOUBLE | BOOLEAN | STRUCT | VIDEO | ANNOTATION | JSON,
/// of which the last four are not measurement types.
pub(crate) fn property_catalog() -> Value {
    json!([
        {"name": "Temperature", "dataType": "DOUBLE", "unit": "Celsius", "type": {"measurement": {}}},
        {"name": "Cycles", "dataType": "INTEGER", "unit": "Count", "type": {"measurement": {}}},
        {"name": "Status", "dataType": "STRING", "type": {"measurement": {}}},
        {"name": "Running", "dataType": "BOOLEAN", "type": {"measurement": {}}},
        {"name": "Serial", "dataType": "STRING", "type": {"attribute": {"defaultValue": "unset"}}},
    ])
}

#[tokio::test]
async fn the_asset_model_asset_and_property_value_slice_round_trips() {
    let router = router();

    // --- CreateAssetModel -------------------------------------------------
    let (status, created_model) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Boiler",
            "assetModelDescription": "A boiler",
            "assetModelProperties": property_catalog(),
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created_model}");
    let model_id = created_model["assetModelId"]
        .as_str()
        .expect("assetModelId")
        .to_string();
    assert!(
        is_modelled_id(&model_id),
        "CreateAssetModel: assetModelId must be a non-zero UUID, got {model_id:?}"
    );
    assert_eq!(
        created_model["assetModelArn"].as_str().unwrap_or(""),
        format!(
            "arn:aws:iotsitewise:{REGION}:{}:asset-model/{model_id}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "CreateAssetModel: unexpected assetModelArn in {created_model}"
    );
    let created_state = created_model["assetModelStatus"]["state"]
        .as_str()
        .unwrap_or("")
        .to_string();
    assert_eq!(
        created_state, "CREATING",
        "CreateAssetModel: a new model is reported CREATING, got {created_model}"
    );

    // --- DescribeAssetModel reports what was created ----------------------
    let (status, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {model}");
    assert_eq!(
        model["assetModelName"], "Boiler",
        "DescribeAssetModel: stored name not reported in {model}"
    );
    assert_eq!(
        model["assetModelDescription"], "A boiler",
        "DescribeAssetModel: stored description not reported in {model}"
    );
    assert_eq!(
        model["assetModelType"], "ASSET_MODEL",
        "DescribeAssetModel: assetModelType defaults to ASSET_MODEL, got {model}"
    );
    assert_eq!(
        model["assetModelArn"], created_model["assetModelArn"],
        "DescribeAssetModel: ARN differs from the created one in {model}"
    );
    assert_eq!(
        model["assetModelStatus"]["state"].as_str().unwrap_or(""),
        created_state,
        "DescribeAssetModel: status disagrees with CreateAssetModel in {model}"
    );

    // Every definition is reported back with a minted id, its name, data type and unit.
    let expected: Vec<(String, String, Option<String>)> = property_catalog()
        .as_array()
        .expect("catalog")
        .iter()
        .map(|definition| {
            (
                definition["name"].as_str().expect("name").to_string(),
                definition["dataType"].as_str().expect("dataType").to_string(),
                definition
                    .get("unit")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect();
    let reported = model["assetModelProperties"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeAssetModel: no assetModelProperties in {model}"));
    assert_eq!(
        reported.len(),
        expected.len(),
        "DescribeAssetModel: expected {} properties, got {reported:?}",
        expected.len()
    );
    for ((name, data_type, unit), property) in expected.iter().zip(reported) {
        assert_eq!(
            property["name"].as_str().unwrap_or(""),
            name,
            "case {name:?}: property order or name changed, got {property}"
        );
        assert_eq!(
            property["dataType"].as_str().unwrap_or(""),
            data_type,
            "case {name:?}: expected dataType {data_type}, got {property}"
        );
        assert_eq!(
            property.get("unit").and_then(Value::as_str).map(str::to_string),
            *unit,
            "case {name:?}: expected unit {unit:?}, got {property}"
        );
        let id = property["id"].as_str().unwrap_or("");
        assert!(
            is_modelled_id(id),
            "case {name:?}: expected a minted UUID id, got {id:?}"
        );
        assert!(
            property.get("type").is_some(),
            "case {name:?}: the property type is not reported in {property}"
        );
    }

    // --- CreateAsset ------------------------------------------------------
    let (status, created_asset) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "Boiler1",
            "assetModelId": model_id,
            "assetDescription": "the first boiler",
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created_asset}");
    let asset_id = created_asset["assetId"].as_str().expect("assetId").to_string();
    assert!(
        is_modelled_id(&asset_id),
        "CreateAsset: assetId must be a non-zero UUID, got {asset_id:?}"
    );
    assert_eq!(
        created_asset["assetArn"].as_str().unwrap_or(""),
        format!(
            "arn:aws:iotsitewise:{REGION}:{}:asset/{asset_id}",
            engine::EMULATOR_ACCOUNT_ID
        ),
        "CreateAsset: unexpected assetArn in {created_asset}"
    );

    // --- DescribeAsset reports the model-derived property list ------------
    let (status, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 200, "DescribeAsset: got {status} {asset}");
    assert_eq!(
        asset["assetName"], "Boiler1",
        "DescribeAsset: stored name not reported in {asset}"
    );
    assert_eq!(
        asset["assetModelId"].as_str().unwrap_or(""),
        model_id,
        "DescribeAsset: assetModelId not reported in {asset}"
    );
    assert_eq!(
        asset["assetDescription"], "the first boiler",
        "DescribeAsset: stored description not reported in {asset}"
    );
    for (name, ..) in &expected {
        assert_eq!(
            property_id(&asset["assetProperties"], name),
            property_id(&model["assetModelProperties"], name),
            "case {name:?}: the asset property id must be inherited from the model property"
        );
    }

    // `excludeProperties=true` leaves the property list out.
    let (status, bare) = call(
        &router,
        "GET",
        &format!("/assets/{asset_id}?excludeProperties=true"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeAsset(excludeProperties): got {status} {bare}");
    assert!(
        bare["assetProperties"]
            .as_array()
            .is_none_or(|properties| properties.is_empty()),
        "DescribeAsset(excludeProperties=true): expected no properties, got {bare}"
    );

    // --- BatchPutAssetPropertyValue, then read each value back ------------
    // One row per data type: the variant member written must be the one read back.
    let temperature = property_id(&asset["assetProperties"], "Temperature");
    let cycles = property_id(&asset["assetProperties"], "Cycles");
    let status_property = property_id(&asset["assetProperties"], "Status");
    let running = property_id(&asset["assetProperties"], "Running");
    // Anchored a minute in the past, inside the window IoT SiteWise ingests.
    let base = super::support::now_seconds() - 60;
    let rows = [
        ("Temperature", &temperature, "doubleValue", json!(42.5), base),
        ("Cycles", &cycles, "integerValue", json!(7), base + 1),
        ("Status", &status_property, "stringValue", json!("NOMINAL"), base + 2),
        ("Running", &running, "booleanValue", json!(true), base + 3),
    ];
    let entries: Vec<Value> = rows
        .iter()
        .enumerate()
        .map(|(at, (_, id, member, value, seconds))| {
            json!({
                "entryId": format!("e{at}"),
                "assetId": asset_id,
                "propertyId": id,
                "propertyValues": [{
                    "value": { *member: value },
                    "timestamp": {"timeInSeconds": seconds, "offsetInNanos": 250},
                    "quality": "GOOD",
                }],
            })
        })
        .collect();
    let (status, put) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({ "entries": entries })),
    )
    .await;
    assert_eq!(status, 200, "BatchPutAssetPropertyValue: got {status} {put}");
    assert_eq!(
        put["errorEntries"].as_array().map(Vec::len),
        Some(0),
        "BatchPutAssetPropertyValue: every entry resolved, so errorEntries must be empty: {put}"
    );

    for (name, id, member, value, seconds) in &rows {
        let (status, latest) = call(
            &router,
            "GET",
            &format!("/properties/latest?assetId={asset_id}&propertyId={id}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {name:?}: got {status} {latest}");
        assert_eq!(
            &latest["propertyValue"]["value"][*member],
            value,
            "case {name:?}: expected {member} {value}, got {latest}"
        );
        assert_eq!(
            latest["propertyValue"]["timestamp"]["timeInSeconds"].as_i64(),
            Some(*seconds),
            "case {name:?}: expected timeInSeconds {seconds}, got {latest}"
        );
        assert_eq!(
            latest["propertyValue"]["timestamp"]["offsetInNanos"].as_i64(),
            Some(250),
            "case {name:?}: expected offsetInNanos 250, got {latest}"
        );
        assert_eq!(
            latest["propertyValue"]["quality"], "GOOD",
            "case {name:?}: expected quality GOOD, got {latest}"
        );
    }

    // An attribute property's model default value IS its current value, so the
    // property nothing was written to reports that default rather than nothing.
    // (The rule across data types is pinned by the `attribute_default_values`
    // suite.)
    let serial = property_id(&asset["assetProperties"], "Serial");
    let (status, defaulted) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={serial}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "attribute property: got {status} {defaulted}");
    assert_eq!(
        defaulted["propertyValue"]["value"]["stringValue"].as_str(),
        Some("unset"),
        "attribute property: expected the model's default value, got {defaulted}"
    );
    assert_eq!(
        defaulted["propertyValue"]["quality"].as_str(),
        Some("GOOD"),
        "attribute property: expected the quality of an ordinary reading, got {defaulted}"
    );

    // --- ListAssets -------------------------------------------------------
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/assets?assetModelId={model_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssets: got {status} {listed}");
    let summaries = listed["assetSummaries"].as_array().expect("assetSummaries");
    assert_eq!(
        summaries.len(),
        1,
        "ListAssets: expected the one created asset, got {listed}"
    );
    assert_eq!(
        summaries[0]["id"].as_str().unwrap_or(""),
        asset_id,
        "ListAssets: unexpected summary id in {listed}"
    );
    assert_eq!(
        summaries[0]["arn"], created_asset["assetArn"],
        "ListAssets: summary ARN differs from the created one in {listed}"
    );
    assert_eq!(
        summaries[0]["name"], "Boiler1",
        "ListAssets: unexpected summary name in {listed}"
    );
    assert_eq!(
        summaries[0]["assetModelId"].as_str().unwrap_or(""),
        model_id,
        "ListAssets: unexpected summary assetModelId in {listed}"
    );
    assert!(
        summaries[0]["status"]["state"].is_string(),
        "ListAssets: a summary carries its status in {listed}"
    );
    assert!(
        summaries[0]["creationDate"].is_number() && summaries[0]["lastUpdateDate"].is_number(),
        "ListAssets: a summary carries its timestamps in {listed}"
    );

    // --- DeleteAssetModel is refused while the asset references it --------
    // A dependant the caller must remove first is a TERMINAL request error, not
    // the retryable operation conflict: 400 InvalidRequestException.
    let (status, conflict) =
        call(&router, "DELETE", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 400,
        "DeleteAssetModel with a live asset: expected 400, got {status} {conflict}"
    );
    assert_eq!(
        error_code(&conflict),
        "InvalidRequestException",
        "DeleteAssetModel with a live asset: unexpected error code in {conflict}"
    );

    // --- DeleteAsset ------------------------------------------------------
    let (status, deleted) = call(&router, "DELETE", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 202, "DeleteAsset: got {status} {deleted}");
    assert_eq!(
        deleted["assetId"].as_str().unwrap_or(""),
        asset_id,
        "DeleteAsset: unexpected assetId in {deleted}"
    );
    assert_eq!(
        deleted["assetStatus"]["state"], "DELETING",
        "DeleteAsset: expected state DELETING, got {deleted}"
    );

    let (status, gone) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(status, 404, "DescribeAsset after delete: got {status} {gone}");
    let (status, listed) = call(
        &router,
        "GET",
        &format!("/assets?assetModelId={model_id}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssets after delete: got {status} {listed}");
    assert_eq!(
        listed["assetSummaries"].as_array().map(Vec::len),
        Some(0),
        "ListAssets after delete: the asset must be gone, got {listed}"
    );
    let (status, gone) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={temperature}"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "GetAssetPropertyValue after delete: got {status} {gone}"
    );

    // --- DeleteAssetModel -------------------------------------------------
    let (status, deleted) =
        call(&router, "DELETE", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 202, "DeleteAssetModel: got {status} {deleted}");
    assert_eq!(
        deleted["assetModelId"].as_str().unwrap_or(""),
        model_id,
        "DeleteAssetModel: unexpected assetModelId in {deleted}"
    );
    assert_eq!(
        deleted["assetModelStatus"]["state"], "DELETING",
        "DeleteAssetModel: expected state DELETING, got {deleted}"
    );
    let (status, gone) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 404,
        "DescribeAssetModel after delete: got {status} {gone}"
    );
    assert_eq!(
        error_code(&gone),
        "ResourceNotFoundException",
        "DescribeAssetModel after delete: unexpected error code in {gone}"
    );
}

#[tokio::test]
async fn the_latest_value_is_the_highest_timestamp_whatever_the_write_order() {
    let router = router();
    let model_id = create_model(
        &router,
        "Turbine",
        json!([{"name": "Speed", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Turbine1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let speed = property_id(&asset["assetProperties"], "Speed");

    // Rows differ in write order, count and nanosecond offset; in every one the
    // latest value is the sample with the highest (timeInSeconds, offsetInNanos).
    /// One sample: `(timeInSeconds, offsetInNanos, doubleValue)`.
    type Sample = (i64, i32, f64);
    // Anchored inside the `[now - 7 days, now + 10 minutes]` ingest window.
    let base = super::support::now_seconds() - 10_000;
    let cases: [(&str, Vec<Sample>, Sample); 4] = [
        ("one sample", vec![(base + 1_000, 0, 1.0)], (base + 1_000, 0, 1.0)),
        (
            "ascending",
            vec![
                (base + 2_000, 0, 1.0),
                (base + 2_001, 0, 2.0),
                (base + 2_002, 0, 3.0),
            ],
            (base + 2_002, 0, 3.0),
        ),
        (
            "descending",
            vec![
                (base + 3_002, 0, 3.0),
                (base + 3_001, 0, 2.0),
                (base + 3_000, 0, 1.0),
            ],
            (base + 3_002, 0, 3.0),
        ),
        (
            "same second, different nanos",
            vec![
                (base + 4_000, 999, 1.0),
                (base + 4_000, 1, 2.0),
                (base + 4_000, 500, 3.0),
            ],
            (base + 4_000, 999, 1.0),
        ),
    ];

    for (case, writes, (expected_seconds, expected_nanos, expected_value)) in cases {
        for (at, (seconds, nanos, value)) in writes.iter().enumerate() {
            let (status, put) = call(
                &router,
                "POST",
                "/properties",
                Some(json!({"entries": [{
                    "entryId": format!("w{at}"),
                    "assetId": asset_id,
                    "propertyId": speed,
                    "propertyValues": [{
                        "value": {"doubleValue": value},
                        "timestamp": {"timeInSeconds": seconds, "offsetInNanos": nanos},
                    }],
                }]})),
            )
            .await;
            assert_eq!(status, 200, "case {case:?}: write {at} got {status} {put}");
            assert_eq!(
                put["errorEntries"].as_array().map(Vec::len),
                Some(0),
                "case {case:?}: write {at} must be accepted, got {put}"
            );
        }

        let (status, latest) = call(
            &router,
            "GET",
            &format!("/properties/latest?assetId={asset_id}&propertyId={speed}"),
            None,
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: got {status} {latest}");
        assert_eq!(
            latest["propertyValue"]["timestamp"]["timeInSeconds"].as_i64(),
            Some(expected_seconds),
            "case {case:?}: expected timeInSeconds {expected_seconds}, got {latest}"
        );
        assert_eq!(
            latest["propertyValue"]["timestamp"]["offsetInNanos"].as_i64(),
            Some(i64::from(expected_nanos)),
            "case {case:?}: expected offsetInNanos {expected_nanos}, got {latest}"
        );
        assert_eq!(
            latest["propertyValue"]["value"]["doubleValue"].as_f64(),
            Some(expected_value),
            "case {case:?}: expected doubleValue {expected_value}, got {latest}"
        );
    }

    // A second write at an already-stored timestamp replaces that sample.
    let (status, put) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({"entries": [{
            "entryId": "replace",
            "assetId": asset_id,
            "propertyId": speed,
            "propertyValues": [{
                "value": {"doubleValue": 99.5},
                "timestamp": {"timeInSeconds": base + 4_000, "offsetInNanos": 999},
            }],
        }]})),
    )
    .await;
    assert_eq!(status, 200, "replacing a sample: got {status} {put}");
    let (_, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={speed}"),
        None,
    )
    .await;
    assert_eq!(
        latest["propertyValue"]["value"]["doubleValue"].as_f64(),
        Some(99.5),
        "replacing a sample: expected the replacement doubleValue, got {latest}"
    );
}

#[tokio::test]
async fn an_id_that_resolves_to_nothing_is_a_modelled_refusal() {
    let router = router();
    let absent = "11111111-2222-3333-4444-555555555555";
    let model_id = create_model(
        &router,
        "Pump",
        json!([{"name": "Flow", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Pump1", &model_id).await;

    // Rows span the operations that take an id and the two label forms a
    // `CustomID` member accepts.
    let reads: [(&str, &str, &str); 6] = [
        ("DescribeAsset by id", "GET", &format!("/assets/{absent}")),
        (
            "DescribeAsset by externalId",
            "GET",
            "/assets/externalId:no_such_asset",
        ),
        (
            "DescribeAssetModel by id",
            "GET",
            &format!("/asset-models/{absent}"),
        ),
        (
            "DescribeAssetModel by externalId",
            "GET",
            "/asset-models/externalId:no_such_model",
        ),
        (
            "DeleteAsset by id",
            "DELETE",
            &format!("/assets/{absent}"),
        ),
        (
            "DeleteAssetModel by id",
            "DELETE",
            &format!("/asset-models/{absent}"),
        ),
    ];
    for (case, method, uri) in reads {
        let (status, body) = call(&router, method, uri, None).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // CreateAsset against a model that does not exist.
    let (status, body) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({"assetName": "Orphan", "assetModelId": absent})),
    )
    .await;
    assert_eq!(
        status, 404,
        "CreateAsset with an unknown model: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "CreateAsset with an unknown model: unexpected error code in {body}"
    );

    // GetAssetPropertyValue for an unknown asset, and for an unknown property of a
    // known asset.
    let property = property_id(
        &call(&router, "GET", &format!("/assets/{asset_id}"), None).await.1["assetProperties"],
        "Flow",
    );
    let latest_cases = [
        (
            "unknown asset",
            format!("/properties/latest?assetId={absent}&propertyId={property}"),
        ),
        (
            "unknown property",
            format!("/properties/latest?assetId={asset_id}&propertyId={absent}"),
        ),
    ];
    for (case, uri) in latest_cases {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // Naming neither form, or both, identifies no property.
    let ambiguous = [
        ("neither", "/properties/latest".to_string()),
        (
            "both",
            format!("/properties/latest?assetId={asset_id}&propertyId={property}&propertyAlias=/a/b"),
        ),
    ];
    for (case, uri) in ambiguous {
        let (status, body) = call(&router, "GET", &uri, None).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // A second create with an already-used id collides.
    let (status, body) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({"assetModelName": "PumpAgain", "assetModelId": model_id})),
    )
    .await;
    assert_eq!(
        status, 409,
        "CreateAssetModel with a used id: expected 409, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceAlreadyExistsException",
        "CreateAssetModel with a used id: unexpected error code in {body}"
    );
}

#[tokio::test]
async fn batch_put_reports_one_error_entry_per_entry_it_cannot_resolve() {
    let router = router();
    let absent = "11111111-2222-3333-4444-555555555555";
    let model_id = create_model(
        &router,
        "Mixer",
        json!([
            {"name": "Rpm", "dataType": "INTEGER", "type": {"measurement": {}}},
            {"name": "Label", "dataType": "STRING", "type": {"measurement": {}}},
        ]),
    )
    .await;
    let asset_id = create_asset(&router, "Mixer1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let rpm = property_id(&asset["assetProperties"], "Rpm");
    let label = property_id(&asset["assetProperties"], "Label");

    let timestamp = json!({"timeInSeconds": super::support::now_seconds() - 60});
    // Rows span the ways an entry fails to resolve, and one that succeeds: the
    // response must carry an error entry for exactly the failing ones.
    let cases: Vec<(&str, Value, Option<&str>)> = vec![
        (
            "accepted",
            json!({"entryId": "ok", "assetId": asset_id, "propertyId": rpm,
                   "propertyValues": [{"value": {"integerValue": 3}, "timestamp": timestamp}]}),
            None,
        ),
        (
            "unknown asset",
            json!({"entryId": "noasset", "assetId": absent, "propertyId": rpm,
                   "propertyValues": [{"value": {"integerValue": 3}, "timestamp": timestamp}]}),
            Some("ResourceNotFoundException"),
        ),
        (
            "unknown property",
            json!({"entryId": "noproperty", "assetId": asset_id, "propertyId": absent,
                   "propertyValues": [{"value": {"integerValue": 3}, "timestamp": timestamp}]}),
            Some("ResourceNotFoundException"),
        ),
        (
            // An alias no asset property carries is NOT a missing resource: IoT
            // SiteWise accepts alias-addressed ingestion and implicitly creates the
            // DISASSOCIATED time series the alias names (the `time_series_ingestion`
            // suite covers what that series then reports).
            "an alias nothing is bound to yet",
            json!({"entryId": "noalias", "propertyAlias": "/no/such/stream",
                   "propertyValues": [{"value": {"integerValue": 3}, "timestamp": timestamp}]}),
            None,
        ),
        (
            // The series a new alias creates takes its data type from the first value
            // ingested, so a first value that names no type establishes nothing.
            "a new alias whose first value names no type",
            json!({"entryId": "notype", "propertyAlias": "/no/such/stream/untyped",
                   "propertyValues": [{"value": {}, "timestamp": timestamp}]}),
            Some("InvalidRequestException"),
        ),
        (
            "no identifier at all",
            json!({"entryId": "noid",
                   "propertyValues": [{"value": {"integerValue": 3}, "timestamp": timestamp}]}),
            Some("InvalidRequestException"),
        ),
        (
            "wrong variant for the data type",
            json!({"entryId": "wrongtype", "assetId": asset_id, "propertyId": label,
                   "propertyValues": [{"value": {"integerValue": 3}, "timestamp": timestamp}]}),
            Some("InvalidRequestException"),
        ),
        (
            "two variant members",
            json!({"entryId": "twovariants", "assetId": asset_id, "propertyId": rpm,
                   "propertyValues": [{"value": {"integerValue": 3, "stringValue": "3"},
                                       "timestamp": timestamp}]}),
            Some("InvalidRequestException"),
        ),
        (
            "no variant member",
            json!({"entryId": "novariant", "assetId": asset_id, "propertyId": rpm,
                   "propertyValues": [{"value": {}, "timestamp": timestamp}]}),
            Some("InvalidRequestException"),
        ),
    ];

    let entries: Vec<Value> = cases.iter().map(|(_, entry, _)| entry.clone()).collect();
    let (status, put) = call(
        &router,
        "POST",
        "/properties",
        Some(json!({"entries": entries})),
    )
    .await;
    assert_eq!(status, 200, "BatchPutAssetPropertyValue: got {status} {put}");
    let error_entries = put["errorEntries"]
        .as_array()
        .unwrap_or_else(|| panic!("no errorEntries in {put}"));

    for (case, entry, expected) in &cases {
        let entry_id = entry["entryId"].as_str().expect("entryId");
        let reported = error_entries
            .iter()
            .find(|reported| reported["entryId"] == entry_id);
        match expected {
            None => assert!(
                reported.is_none(),
                "case {case:?}: expected no error entry, got {reported:?}"
            ),
            Some(code) => {
                let reported = reported.unwrap_or_else(|| {
                    panic!("case {case:?}: expected an error entry, got {error_entries:?}")
                });
                assert_eq!(
                    reported["errors"][0]["errorCode"].as_str().unwrap_or(""),
                    *code,
                    "case {case:?}: expected errorCode {code}, got {reported}"
                );
                assert!(
                    reported["errors"][0]["errorMessage"]
                        .as_str()
                        .is_some_and(|message| !message.is_empty()),
                    "case {case:?}: an error entry carries a message, got {reported}"
                );
            }
        }
    }

    // The one accepted entry really landed; the refused ones wrote nothing.
    let (status, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={rpm}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "accepted entry read back: got {status} {latest}");
    assert_eq!(
        latest["propertyValue"]["value"]["integerValue"].as_i64(),
        Some(3),
        "accepted entry read back: expected integerValue 3, got {latest}"
    );
    let (status, untouched) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={label}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "refused entry read back: got {status} {untouched}");
    assert!(
        untouched.get("propertyValue").is_none_or(Value::is_null),
        "refused entry read back: nothing must have been written, got {untouched}"
    );
}

#[tokio::test]
async fn external_id_labels_resolve_to_the_same_resource_as_the_raw_id() {
    let router = router();
    let (status, created_model) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Chiller",
            "assetModelExternalId": "chiller_model",
            "assetModelProperties": [
                {"name": "Load", "dataType": "DOUBLE", "type": {"measurement": {}}}
            ],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created_model}");
    let model_id = created_model["assetModelId"].as_str().expect("id").to_string();

    // The asset names its model by the external-id alias form.
    let (status, created_asset) = call(
        &router,
        "POST",
        "/assets",
        Some(json!({
            "assetName": "Chiller1",
            "assetModelId": "externalId:chiller_model",
            "assetExternalId": "chiller_one",
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAsset: got {status} {created_asset}");
    let asset_id = created_asset["assetId"].as_str().expect("id").to_string();

    // Each row is a label form; both must name the same stored resource.
    let model_labels = [
        ("raw id", model_id.clone()),
        ("externalId alias", "externalId:chiller_model".to_string()),
    ];
    for (case, label) in model_labels {
        let (status, body) = call(&router, "GET", &format!("/asset-models/{label}"), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["assetModelId"].as_str().unwrap_or(""),
            model_id,
            "case {case:?}: resolved to a different model in {body}"
        );
        assert_eq!(
            body["assetModelExternalId"], "chiller_model",
            "case {case:?}: the stored external id is not reported in {body}"
        );
    }

    let asset_labels = [
        ("raw id", asset_id.clone()),
        ("externalId alias", "externalId:chiller_one".to_string()),
    ];
    for (case, label) in asset_labels {
        let (status, body) = call(&router, "GET", &format!("/assets/{label}"), None).await;
        assert_eq!(status, 200, "case {case:?}: got {status} {body}");
        assert_eq!(
            body["assetId"].as_str().unwrap_or(""),
            asset_id,
            "case {case:?}: resolved to a different asset in {body}"
        );
        assert_eq!(
            body["assetModelId"].as_str().unwrap_or(""),
            model_id,
            "case {case:?}: the externalId model label did not resolve at create time: {body}"
        );
    }

    // ListAssets takes the alias form on `assetModelId` too.
    let (status, listed) = call(
        &router,
        "GET",
        "/assets?assetModelId=externalId:chiller_model",
        None,
    )
    .await;
    assert_eq!(status, 200, "ListAssets by alias: got {status} {listed}");
    assert_eq!(
        listed["assetSummaries"][0]["id"].as_str().unwrap_or(""),
        asset_id,
        "ListAssets by alias: expected the one asset, got {listed}"
    );
}

#[tokio::test]
async fn list_assets_pages_through_the_enumeration_with_next_token() {
    let router = router();
    let model_id = create_model(
        &router,
        "Fan",
        json!([{"name": "Rpm", "dataType": "INTEGER", "type": {"measurement": {}}}]),
    )
    .await;
    let other_model_id = create_model(&router, "Valve", json!([])).await;

    let mut created = Vec::new();
    for name in ["Fan1", "Fan2", "Fan3", "Fan4", "Fan5"] {
        created.push(create_asset(&router, name, &model_id).await);
    }
    // An asset of another model must never appear under this model's filter.
    let _ = create_asset(&router, "Valve1", &other_model_id).await;

    // Rows differ in page size: every one must enumerate all five assets exactly
    // once, in one order, and stop with no `nextToken`.
    for page_size in [1_usize, 2, 3, 5, 250] {
        let mut seen = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let uri = match &token {
                Some(token) => format!(
                    "/assets?assetModelId={model_id}&maxResults={page_size}&nextToken={token}"
                ),
                None => format!("/assets?assetModelId={model_id}&maxResults={page_size}"),
            };
            let (status, body) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case page_size={page_size}: got {status} {body}"
            );
            let summaries = body["assetSummaries"].as_array().expect("assetSummaries");
            assert!(
                summaries.len() <= page_size,
                "case page_size={page_size}: a page must hold at most {page_size}, got {body}"
            );
            for summary in summaries {
                seen.push(summary["id"].as_str().expect("id").to_string());
            }
            pages += 1;
            assert!(
                pages <= 10,
                "case page_size={page_size}: pagination did not terminate after {pages} pages"
            );
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            created.len(),
            "case page_size={page_size}: expected {} distinct assets, saw {seen:?}",
            created.len()
        );
        let mut expected = created.clone();
        expected.sort();
        assert_eq!(
            sorted, expected,
            "case page_size={page_size}: the enumeration must cover exactly the model's assets"
        );
    }

    // A `nextToken` that this operation never issued is refused — whether it is not
    // a token at all, or well-formed but naming a resume point outside this
    // enumeration (an id of another model's asset, or one that no longer exists).
    let elsewhere = create_asset(&router, "Valve2", &other_model_id).await;
    let forged = [
        ("not hex at all", "zzz".to_string()),
        ("hex of an unknown id", hex_token("11111111-2222-3333-4444-555555555555")),
        ("hex of another model's asset", hex_token(&elsewhere)),
    ];
    for (case, token) in forged {
        let (status, body) = call(
            &router,
            "GET",
            &format!("/assets?assetModelId={model_id}&nextToken={token}"),
            None,
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "InvalidRequestException",
            "case {case:?}: unexpected error code in {body}"
        );
    }

    // `filter=ALL` (the default) requires `assetModelId`; `TOP_LEVEL` does not.
    let (status, body) = call(&router, "GET", "/assets", None).await;
    assert_eq!(
        status, 400,
        "ListAssets with no assetModelId: expected 400, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidRequestException",
        "ListAssets with no assetModelId: unexpected error code in {body}"
    );
    let (status, body) = call(&router, "GET", "/assets?filter=TOP_LEVEL", None).await;
    assert_eq!(
        status, 200,
        "ListAssets filter=TOP_LEVEL: got {status} {body}"
    );
    assert_eq!(
        body["assetSummaries"].as_array().map(Vec::len),
        Some(created.len() + 2),
        "ListAssets filter=TOP_LEVEL: no asset is a child yet, so all are top level: {body}"
    );
}

#[tokio::test]
async fn a_created_resource_reports_creating_until_the_owed_transition_falls_due() {
    let router = router();
    let (_, created_model) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({"assetModelName": "Boiler", "assetModelProperties": []})),
    )
    .await;
    let model_id = created_model["assetModelId"].as_str().expect("id").to_string();
    let asset_id = create_asset(&router, "Boiler1", &model_id).await;

    // Straight after the creating call, both resources report the transient state
    // the real service reports.
    let (_, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        model["assetModelStatus"]["state"], "CREATING",
        "DescribeAssetModel straight after create: got {model}"
    );
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(
        asset["assetStatus"]["state"], "CREATING",
        "DescribeAsset straight after create: got {asset}"
    );

    // Once the transition the clock owes is due, every reader sees ACTIVE — and
    // nothing had to call an operation to make it happen.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let (_, model) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        model["assetModelStatus"]["state"], "ACTIVE",
        "DescribeAssetModel after the activation delay: got {model}"
    );
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    assert_eq!(
        asset["assetStatus"]["state"], "ACTIVE",
        "DescribeAsset after the activation delay: got {asset}"
    );
    let (_, listed) = call(
        &router,
        "GET",
        &format!("/assets?assetModelId={model_id}"),
        None,
    )
    .await;
    assert_eq!(
        listed["assetSummaries"][0]["status"]["state"], "ACTIVE",
        "ListAssets after the activation delay: got {listed}"
    );
}

#[tokio::test]
async fn a_property_definition_must_name_exactly_one_type_and_spec_a_struct() {
    let router = router();

    // Rows span the two rules the shapes' own documentation states: "You can only
    // specify one type in a property definition", and dataTypeSpec "is required on
    // properties that have the STRUCT data type".
    let cases: Vec<(&str, Value, bool)> = vec![
        (
            "one type",
            json!([{"name": "A", "dataType": "DOUBLE", "type": {"measurement": {}}}]),
            true,
        ),
        (
            "attribute type",
            json!([{"name": "A", "dataType": "STRING", "type": {"attribute": {}}}]),
            true,
        ),
        (
            "no type at all",
            json!([{"name": "A", "dataType": "DOUBLE", "type": {}}]),
            false,
        ),
        (
            "two types",
            json!([{"name": "A", "dataType": "DOUBLE",
                    "type": {"measurement": {}, "attribute": {}}}]),
            false,
        ),
        (
            "STRUCT without dataTypeSpec",
            json!([{"name": "A", "dataType": "STRUCT", "type": {"measurement": {}}}]),
            false,
        ),
        (
            "STRUCT with dataTypeSpec",
            json!([{"name": "A", "dataType": "STRUCT", "dataTypeSpec": "AWS/ALARM_STATE",
                    "type": {"measurement": {}}}]),
            true,
        ),
    ];

    for (case, properties, accepted) in cases {
        let (status, body) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({"assetModelName": format!("Model {case}"),
                        "assetModelProperties": properties})),
        )
        .await;
        if accepted {
            assert_eq!(status, 202, "case {case:?}: expected 202, got {status} {body}");
        } else {
            assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {case:?}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_hierarchy_definition_must_name_a_stored_child_model() {
    let router = router();
    let child_id = create_model(&router, "Turbine", json!([])).await;

    // Rows: the child model named by raw id, by its external-id alias, and one that
    // does not exist.
    let (status, external_child) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({"assetModelName": "Gearbox", "assetModelExternalId": "gearbox_model"})),
    )
    .await;
    assert_eq!(status, 202, "create the aliased child: got {status} {external_child}");

    let cases: [(&str, String, bool); 3] = [
        ("child by raw id", child_id.clone(), true),
        (
            "child by externalId alias",
            "externalId:gearbox_model".to_string(),
            true,
        ),
        (
            "child that does not exist",
            "11111111-2222-3333-4444-555555555555".to_string(),
            false,
        ),
    ];
    for (case, child, accepted) in cases {
        let (status, body) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({
                "assetModelName": format!("Farm {case}"),
                "assetModelHierarchies": [
                    {"name": "children", "childAssetModelId": child}
                ],
            })),
        )
        .await;
        if accepted {
            assert_eq!(status, 202, "case {case:?}: expected 202, got {status} {body}");
            let parent = body["assetModelId"].as_str().expect("id").to_string();
            let (status, described) =
                call(&router, "GET", &format!("/asset-models/{parent}"), None).await;
            assert_eq!(status, 200, "case {case:?}: describe got {status} {described}");
            let hierarchy = &described["assetModelHierarchies"][0];
            assert_eq!(
                hierarchy["name"], "children",
                "case {case:?}: the stored hierarchy name is not reported in {described}"
            );
            assert!(
                is_modelled_id(hierarchy["id"].as_str().unwrap_or("")),
                "case {case:?}: the hierarchy must carry a minted id, got {described}"
            );
            // The stored child is the RESOLVED id, whichever label named it.
            assert!(
                is_modelled_id(hierarchy["childAssetModelId"].as_str().unwrap_or("")),
                "case {case:?}: childAssetModelId must be the resolved UUID, got {described}"
            );
        } else {
            assert_eq!(status, 404, "case {case:?}: expected 404, got {status} {body}");
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: unexpected error code in {body}"
            );
        }
    }

    // And the child model CAN be deleted while a hierarchy still merely names it:
    // what holds a delete back is the ASSETS created from the model, and a
    // reference another model's definition makes to it is not that.
    let (status, body) = call(&router, "DELETE", &format!("/asset-models/{child_id}"), None).await;
    assert_eq!(
        status, 202,
        "deleting a model a hierarchy merely names: expected 202, got {status} {body}"
    );
    assert_eq!(
        body["assetModelStatus"]["state"].as_str(),
        Some("DELETING"),
        "deleting a model a hierarchy merely names: expected DELETING, got {body}"
    );
}

#[tokio::test]
async fn assets_cannot_be_created_from_a_component_or_interface_model() {
    let router = router();

    // Rows span every AssetModelType value, from
    // `smithy_query(query="shape", shape="AssetModelType")`.
    let cases = [
        ("ASSET_MODEL", true),
        ("COMPONENT_MODEL", false),
        ("INTERFACE", false),
    ];
    for (model_type, instantiable) in cases {
        let (status, created) = call(
            &router,
            "POST",
            "/asset-models",
            Some(json!({
                "assetModelName": format!("Model {model_type}"),
                "assetModelType": model_type,
            })),
        )
        .await;
        assert_eq!(
            status, 202,
            "case {model_type:?}: create got {status} {created}"
        );
        let model_id = created["assetModelId"].as_str().expect("id").to_string();
        let (status, described) =
            call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
        assert_eq!(status, 200, "case {model_type:?}: describe got {status} {described}");
        assert_eq!(
            described["assetModelType"], model_type,
            "case {model_type:?}: the stored type is not reported in {described}"
        );

        let (status, body) = call(
            &router,
            "POST",
            "/assets",
            Some(json!({"assetName": format!("Asset {model_type}"), "assetModelId": model_id})),
        )
        .await;
        if instantiable {
            assert_eq!(status, 202, "case {model_type:?}: expected 202, got {status} {body}");
        } else {
            assert_eq!(status, 400, "case {model_type:?}: expected 400, got {status} {body}");
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case {model_type:?}: unexpected error code in {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_composite_model_is_reported_once_as_a_model_or_as_a_summary() {
    let router = router();
    let (status, created) = call(
        &router,
        "POST",
        "/asset-models",
        Some(json!({
            "assetModelName": "Alarmed",
            "assetModelCompositeModels": [{
                "name": "HighTemp",
                "type": "AWS/ALARM",
                "description": "the high-temperature alarm",
                "properties": [{
                    "name": "State",
                    "dataType": "STRUCT",
                    "dataTypeSpec": "AWS/ALARM_STATE",
                    "type": {"measurement": {}},
                }],
            }, {
                "name": "Setpoints",
                "type": "CUSTOM",
                "description": "the operator's setpoints",
                "properties": [{
                    "name": "Target",
                    "dataType": "DOUBLE",
                    "type": {"attribute": {"defaultValue": "20"}},
                }],
            }],
        })),
    )
    .await;
    assert_eq!(status, 202, "CreateAssetModel: got {status} {created}");
    let model_id = created["assetModelId"].as_str().expect("id").to_string();

    let (status, described) = call(&router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel: got {status} {described}");
    let models = described["assetModelCompositeModels"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelCompositeModels in {described}"));
    let summaries = described["assetModelCompositeModelSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("no assetModelCompositeModelSummaries in {described}"));
    // Each immediate child is reported ONCE: `assetModelCompositeModels` is "the
    // list of built-in composite models for the asset model, such as those with
    // those of type AWS/ALARMS", the summaries "the list of the immediate child
    // custom composite model summaries".
    assert_eq!(
        (models.len(), summaries.len()),
        (1, 1),
        "the built-in composite model belongs to assetModelCompositeModels and the custom \
         one to the summaries: {described}"
    );
    assert_eq!(
        (models[0]["name"].as_str(), summaries[0]["name"].as_str()),
        (Some("HighTemp"), Some("Setpoints")),
        "each composite model must be reported by the member its type belongs to: {described}"
    );
    let id = models[0]["id"].as_str().unwrap_or("");
    assert!(
        is_modelled_id(id),
        "the composite model must carry a minted UUID id, got {id:?}"
    );
    let custom_id = summaries[0]["id"].as_str().unwrap_or("");
    assert!(
        is_modelled_id(custom_id),
        "the custom composite model must carry a minted UUID id, got {custom_id:?}"
    );
    // The summary's path starts at the ROOT of the asset model and ends at the
    // composite model: "the path that includes all the pieces that make up the
    // composite model".
    let path = summaries[0]["path"]
        .as_array()
        .unwrap_or_else(|| panic!("no path on the summary in {described}"));
    assert_eq!(
        path.iter()
            .map(|segment| segment["id"].as_str().unwrap_or(""))
            .collect::<Vec<&str>>(),
        vec![model_id.as_str(), custom_id],
        "the summary's path must run from the asset model root to the composite model: \
         {described}"
    );
    assert_eq!(
        models[0]["properties"][0]["dataTypeSpec"], "AWS/ALARM_STATE",
        "the stored composite property is not reported in {described}"
    );

    // `excludeProperties=true` withholds property definitions by EVERY route: the
    // top-level list and each composite model's own properties.
    let (status, bare) = call(
        &router,
        "GET",
        &format!("/asset-models/{model_id}?excludeProperties=true"),
        None,
    )
    .await;
    assert_eq!(status, 200, "DescribeAssetModel(excludeProperties): got {status} {bare}");
    for route in ["assetModelProperties", "assetModelCompositeModels[0].properties"] {
        let reported = if route == "assetModelProperties" {
            bare["assetModelProperties"].clone()
        } else {
            bare["assetModelCompositeModels"][0]["properties"].clone()
        };
        assert!(
            reported.is_null()
                || reported
                    .as_array()
                    .is_some_and(|properties| properties.is_empty()),
            "case {route:?}: excludeProperties=true must withhold property definitions, got {bare}"
        );
    }
    assert_eq!(
        bare["assetModelCompositeModels"][0]["id"].as_str().unwrap_or(""),
        id,
        "DescribeAssetModel(excludeProperties): the composite model itself is still reported: {bare}"
    );

    // A model with no composite models reports both collections as present and
    // EMPTY, so a client can tell "none" from "not reported".
    let plain_id = create_model(&router, "Plain", json!([])).await;
    let (status, plain) = call(&router, "GET", &format!("/asset-models/{plain_id}"), None).await;
    assert_eq!(status, 200, "DescribeAssetModel(plain): got {status} {plain}");
    for member in [
        "assetModelCompositeModels",
        "assetModelCompositeModelSummaries",
    ] {
        assert_eq!(
            plain[member].as_array().map(Vec::len),
            Some(0),
            "case {member:?}: a model with no composite models must report an empty list, \
             got {plain}"
        );
    }
}

#[tokio::test]
async fn a_batch_put_request_is_bounded_at_ten_entries_of_ten_values() {
    let router = router();
    let model_id = create_model(
        &router,
        "Press",
        json!([{"name": "Force", "dataType": "INTEGER", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Press1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let force = property_id(&asset["assetProperties"], "Force");

    /// Build an entry with `values` samples. Timestamps start at `first_second`.
    fn entry(entry_id: &str, asset_id: &str, property_id: &str, first_second: i64, values: usize) -> Value {
        json!({
            "entryId": entry_id,
            "assetId": asset_id,
            "propertyId": property_id,
            "propertyValues": (0..values)
                .map(|at| json!({
                    "value": {"integerValue": at},
                    "timestamp": {"timeInSeconds": first_second + at as i64},
                }))
                .collect::<Vec<Value>>(),
        })
    }

    // Rows span the published bounds and one past each: "You can specify up to 10
    // entries per request" and "up to 10 propertyValues array elements".
    // Anchored inside the ingest window; see `super::support::now_seconds`.
    let base = super::support::now_seconds() - 10_000;
    let entry_counts = [(1_usize, true), (10, true), (11, false)];
    for (count, accepted) in entry_counts {
        let entries: Vec<Value> = (0..count)
            .map(|at| {
                entry(
                    &format!("e{at}"),
                    &asset_id,
                    &force,
                    base + at as i64 * 100,
                    1,
                )
            })
            .collect();
        let (status, body) = call(
            &router,
            "POST",
            "/properties",
            Some(json!({"entries": entries})),
        )
        .await;
        if accepted {
            assert_eq!(status, 200, "case entries={count}: expected 200, got {status} {body}");
            assert_eq!(
                body["errorEntries"].as_array().map(Vec::len),
                Some(0),
                "case entries={count}: every entry resolves, got {body}"
            );
        } else {
            assert_eq!(status, 400, "case entries={count}: expected 400, got {status} {body}");
            assert_eq!(
                error_code(&body),
                "InvalidRequestException",
                "case entries={count}: unexpected error code in {body}"
            );
        }
    }

    let value_counts = [(1_usize, true), (10, true), (11, false)];
    for (count, accepted) in value_counts {
        let (status, body) = call(
            &router,
            "POST",
            "/properties",
            Some(json!({"entries": [
                entry("v", &asset_id, &force, base + 2_000 + count as i64 * 100, count)
            ]})),
        )
        .await;
        assert_eq!(status, 200, "case values={count}: got {status} {body}");
        let reported = body["errorEntries"]
            .as_array()
            .unwrap_or_else(|| panic!("case values={count}: no errorEntries in {body}"));
        if accepted {
            assert!(
                reported.is_empty(),
                "case values={count}: expected no error entry, got {body}"
            );
        } else {
            assert_eq!(
                reported.len(),
                1,
                "case values={count}: expected one error entry, got {body}"
            );
            assert_eq!(
                reported[0]["errors"][0]["errorCode"].as_str().unwrap_or(""),
                "LimitExceededException",
                "case values={count}: unexpected errorCode in {body}"
            );
            assert!(
                reported[0]["errors"][0]["timestamps"]
                    .as_array()
                    .is_some_and(|timestamps| timestamps.len() <= 10),
                "case values={count}: the error entry must not amplify the request, got {body}"
            );
        }
    }
}

#[tokio::test]
async fn a_property_series_is_bounded_and_keeps_the_newest_samples() {
    let (router, engine) = router_with_engine();
    let model_id = create_model(
        &router,
        "Kiln",
        json!([{"name": "Heat", "dataType": "INTEGER", "type": {"measurement": {}}}]),
    )
    .await;
    let asset_id = create_asset(&router, "Kiln1", &model_id).await;
    let (_, asset) = call(&router, "GET", &format!("/assets/{asset_id}"), None).await;
    let heat = property_id(&asset["assetProperties"], "Heat");

    // Write past the cap, ten samples per request (the modelled per-entry maximum),
    // with ascending timestamps 1..=overflow.
    let overflow = MAX_SAMPLES_PER_SERIES + 20;
    // Anchored so the whole ascending run lands inside the ingest window.
    let base = super::support::now_seconds() - overflow as i64 - 10;
    for chunk in (0..overflow).step_by(10) {
        let values: Vec<Value> = (chunk..(chunk + 10).min(overflow))
            .map(|at| {
                json!({
                    "value": {"integerValue": at as i64},
                    "timestamp": {"timeInSeconds": base + at as i64 + 1},
                })
            })
            .collect();
        let (status, body) = call(
            &router,
            "POST",
            "/properties",
            Some(json!({"entries": [{
                "entryId": format!("c{chunk}"),
                "assetId": asset_id,
                "propertyId": heat,
                "propertyValues": values,
            }]})),
        )
        .await;
        assert_eq!(status, 200, "case chunk={chunk}: got {status} {body}");
        assert_eq!(
            body["errorEntries"].as_array().map(Vec::len),
            Some(0),
            "case chunk={chunk}: every sample must be accepted, got {body}"
        );
    }

    // The series is capped, and what it dropped is the OLDEST sample: the window
    // ends at the newest timestamp written. Read straight from the store (the only
    // way to see a series length: GetAssetPropertyValueHistory is still a stub), in
    // a scope that releases the guard before the next request.
    let (series_len, oldest_second, newest_second) = {
        let state = service_state(&*engine).expect("the service store");
        let store = state.lock();
        let series = store
            .property_values
            .get(&asset_id)
            .and_then(|properties| properties.get(&heat))
            .unwrap_or_else(|| panic!("no series for ({asset_id}, {heat})"));
        (
            series.len(),
            series[0].timestamp.time_in_seconds,
            series[series.len() - 1].timestamp.time_in_seconds,
        )
    };
    assert_eq!(
        series_len, MAX_SAMPLES_PER_SERIES,
        "expected the series to be capped at {MAX_SAMPLES_PER_SERIES}, got {series_len}"
    );
    assert_eq!(
        oldest_second,
        base + (overflow - MAX_SAMPLES_PER_SERIES) as i64 + 1,
        "the oldest kept sample must be the one just past the evicted window, got {oldest_second}"
    );
    assert_eq!(
        newest_second,
        base + overflow as i64,
        "the newest kept sample must be the last one written, got {newest_second}"
    );

    // And the wire still reports that newest sample as the latest value.
    let (status, latest) = call(
        &router,
        "GET",
        &format!("/properties/latest?assetId={asset_id}&propertyId={heat}"),
        None,
    )
    .await;
    assert_eq!(status, 200, "latest after the cap: got {status} {latest}");
    assert_eq!(
        latest["propertyValue"]["timestamp"]["timeInSeconds"].as_i64(),
        Some(base + overflow as i64),
        "latest after the cap: expected timeInSeconds {}, got {latest}",
        base + overflow as i64
    );
    assert_eq!(
        latest["propertyValue"]["value"]["integerValue"].as_i64(),
        Some(overflow as i64 - 1),
        "latest after the cap: expected integerValue {}, got {latest}",
        overflow - 1
    );
}
