// SPDX-License-Identifier: CC0-1.0

//! How a tumbling window's `interval` is SPELLED, and what the stored formula
//! reports its variables as pointing at once the write has accepted it.
//!
//! Two rules are pinned here, over the same table of writes:
//!
//! * the span an interval names is read from its COUNT and its UNIT, and the unit
//!   letter names its unit whatever its case. `TumblingWindow` documents the same
//!   units in both spellings it accepts — "You can use `m`, `h`, `d`, and `w` when
//!   you specify an interval or offset… The `interval` and `offset` parameters
//!   support the ISO 8601 format. For example… `PT5M` represents 5 minutes"
//!   (`smithy_query(query="shape", shape="TumblingWindow")`) — and the ISO 8601
//!   spelling writes them uppercase, so the case of the letter is not what picks
//!   the unit. Whether the write is accepted therefore depends on the SPAN alone,
//!   against the published limits ("The interval time must be between 1 minute and
//!   1 week"); and
//! * every read of the stored model reports a formula variable's `propertyId` as
//!   the ID of the property it resolves to, not the spelling the request used
//!   (`smithy_query(query="doc", shape="VariableValue", member="propertyId")`: "The
//!   ID of the property to use as the variable. You can use the property `name` if
//!   it's from the same asset model"), and a property's `path` from the ROOT of the
//!   asset model (`AssetModelProperty$path`), for a `metric` and a `transform`
//!   alike.
//!
//! The rows differ along the unit, the case of its letter, the spelling family
//! (shorthand or ISO 8601) and where the span sits relative to each published
//! bound — each bound, and one step past it, in both cases of its letter. The
//! refusal rows sit beside the accepted ones, so the table cannot pass by
//! accepting or refusing everything.
//!
//! The wire harness lives in `super::asset_lifecycle`.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::asset_lifecycle::{call, error_code, is_modelled_id, property_id, router};

/// The error class IoT SiteWise refuses a definition it will not store with.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The metric expression every row writes, which the refusal of a bad interval
/// quotes back.
const EXPRESSION: &str = "avg(power)";

/// What one row of the table expects of the write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// The span is inside the published limits: the write is accepted and the
    /// stored model reports the interval as it was sent.
    Accepted,
    /// The span is outside them, or the value names no span at all: the write is
    /// refused, naming the expression and the interval as the request spelled it.
    Refused,
}

/// The request body one row sends: a measurement, a `metric` over it whose window
/// carries `interval`, and a `transform` over it — both variables spelling the
/// measurement by its NAME, so an accepted row also shows what the read resolves
/// them to.
fn definition(name: &str, interval: &str) -> Value {
    json!({
        "assetModelName": name,
        "assetModelProperties": [
            {"name": "Power", "dataType": "DOUBLE", "type": {"measurement": {}}},
            {
                "name": "Average",
                "dataType": "DOUBLE",
                "type": {"metric": {
                    "expression": EXPRESSION,
                    "variables": [{"name": "power", "value": {"propertyId": "Power"}}],
                    "window": {"tumbling": {"interval": interval}},
                }},
            },
            {
                "name": "Doubled",
                "dataType": "DOUBLE",
                "type": {"transform": {
                    "expression": "power * 2",
                    "variables": [{"name": "power", "value": {"propertyId": "Power"}}],
                }},
            },
        ],
    })
}

/// The property definitions `DescribeAssetModel` reports for `model_id`.
async fn described_properties(router: &MultiServiceRouter, model_id: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/asset-models/{model_id}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeAssetModel {model_id}: got {status} {body}"
    );
    body["assetModelProperties"].clone()
}

/// The property named `name` in a reported property list.
fn property<'p>(properties: &'p Value, name: &str) -> &'p Value {
    properties
        .as_array()
        .unwrap_or_else(|| panic!("expected a property list, got {properties}"))
        .iter()
        .find(|property| property["name"] == name)
        .unwrap_or_else(|| panic!("no property named {name} in {properties}"))
}

/// The `propertyId` the single variable of the `metric` or `transform` of property
/// `name` reads back as.
fn reference_of(properties: &Value, name: &str) -> Option<String> {
    let formula = &property(properties, name)["type"];
    let variables = if formula["metric"].is_object() {
        &formula["metric"]["variables"]
    } else {
        &formula["transform"]["variables"]
    };
    variables[0]["value"]["propertyId"]
        .as_str()
        .map(str::to_string)
}

/// The `(id, name)` pairs of the structured path property `name` reports.
fn path_of(properties: &Value, name: &str) -> Vec<(Option<String>, Option<String>)> {
    property(properties, name)["path"]
        .as_array()
        .unwrap_or_else(|| panic!("property {name} reports no path in {properties}"))
        .iter()
        .map(|segment| {
            (
                segment["id"].as_str().map(str::to_string),
                segment["name"].as_str().map(str::to_string),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_window_interval_names_its_span_whatever_the_case_of_its_unit_letter() {
    let router = router();

    // Each unit the shape documents, at a span inside the limits, in both cases of
    // its letter; each published bound and one step past it, in both cases; the ISO
    // 8601 spelling of a span inside and one below the floor; and a letter the
    // documented catalog does not hold, which names no span in either case.
    let rows: [(&str, &str, Expect); 20] = [
        ("minutes, lowercase", "5m", Expect::Accepted),
        ("minutes, uppercase", "5M", Expect::Accepted),
        ("hours, lowercase", "2h", Expect::Accepted),
        ("hours, uppercase", "2H", Expect::Accepted),
        ("days, lowercase", "3d", Expect::Accepted),
        ("days, uppercase", "3D", Expect::Accepted),
        ("the published floor, lowercase", "1m", Expect::Accepted),
        ("the published floor, uppercase", "1M", Expect::Accepted),
        ("the published ceiling, lowercase", "1w", Expect::Accepted),
        ("the published ceiling, uppercase", "1W", Expect::Accepted),
        ("the ceiling spelled in days, uppercase", "7D", Expect::Accepted),
        ("one second below the floor, lowercase", "59s", Expect::Refused),
        ("one second below the floor, uppercase", "59S", Expect::Refused),
        ("one day past the ceiling, lowercase", "8d", Expect::Refused),
        ("one day past the ceiling, uppercase", "8D", Expect::Refused),
        ("the floor in ISO 8601", "PT1M", Expect::Accepted),
        ("the ceiling in ISO 8601 hours", "PT168H", Expect::Accepted),
        ("below the floor in ISO 8601", "PT30S", Expect::Refused),
        ("a unit letter outside the catalog, lowercase", "5y", Expect::Refused),
        ("a unit letter outside the catalog, uppercase", "5Y", Expect::Refused),
    ];

    for (at, (case, interval, expected)) in rows.into_iter().enumerate() {
        let name = format!("WindowSpelling-{at}");
        let (status, body) = call(
            &router,
            "POST",
            "/asset-models",
            Some(definition(&name, interval)),
        )
        .await;
        match expected {
            Expect::Refused => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: the interval {interval:?} names no span inside the \
                     limits, expected 400, got {status} {body}"
                );
                assert_eq!(
                    error_code(&body),
                    INVALID_REQUEST,
                    "case {case:?}: expected {INVALID_REQUEST}, got {body}"
                );
                let expected_message =
                    format!("`{EXPRESSION}`: Window time interval '{interval}' is not within limits");
                assert_eq!(
                    body["message"].as_str(),
                    Some(expected_message.as_str()),
                    "case {case:?}: expected {expected_message:?}, got {body}"
                );
            }
            Expect::Accepted => {
                assert_eq!(
                    status, 202,
                    "case {case:?}: the interval {interval:?} names a span inside the \
                     limits, expected 202, got {status} {body}"
                );
                let model_id = body["assetModelId"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: no assetModelId in {body}"))
                    .to_string();
                let properties = described_properties(&router, &model_id).await;

                // The interval is stored as the request spelled it: reading a span
                // from it is not rewriting it.
                let stored = property(&properties, "Average")["type"]["metric"]["window"]
                    ["tumbling"]["interval"]
                    .as_str()
                    .map(str::to_string);
                assert_eq!(
                    stored.as_deref(),
                    Some(interval),
                    "case {case:?}: expected the stored interval {interval:?}, got {stored:?}"
                );

                // Both formula members report the variable as the ID of the
                // property it names, and every property reports its path from the
                // root of the asset model.
                let power = property_id(&properties, "Power");
                let root = (Some(model_id.clone()), Some(name.clone()));
                for formula in ["Average", "Doubled"] {
                    let reference = reference_of(&properties, formula);
                    assert_eq!(
                        reference.as_deref(),
                        Some(power.as_str()),
                        "case {case:?}: the variable of {formula} must read back as the \
                         resolved id {power:?}, got {reference:?}"
                    );
                    assert!(
                        reference.as_deref().is_some_and(is_modelled_id),
                        "case {case:?}: the variable of {formula} must read back as an id, \
                         got {reference:?}"
                    );
                    let path = path_of(&properties, formula);
                    let property_segment = (
                        Some(property_id(&properties, formula)),
                        Some(formula.to_string()),
                    );
                    assert_eq!(
                        path,
                        vec![root.clone(), property_segment.clone()],
                        "case {case:?}: {formula} must report the path from the model root, \
                         expected {:?}, got {path:?}",
                        vec![root.clone(), property_segment]
                    );
                }
            }
        }
    }
}
