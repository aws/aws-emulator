// SPDX-License-Identifier: CC0-1.0

//! How a publish carrying a unit outside the units a measurement may be expressed in is
//! refused.
//!
//! The rule under test: **an operation whose model declares the parameter-value fault refuses a
//! value outside the enumerated type a member targets as THAT fault** — the body names
//! `InvalidParameterValueException` in the service's own package, a query-compatible client
//! reads its legacy code in the `x-amzn-query-error` header, and the sentence names the request
//! member path the value arrived on and the whole set the type declares
//! (`smithy_query(query="operation",operation="PutMetricData")` binds
//! `InvalidParameterValueException` and no request-validation class; `MetricDatum$Unit` targets
//! `StandardUnit`).
//!
//! The rows differ along what the rule generalises over: HOW the submitted value misses the set
//! (a word that is no unit, a declared value in another case, a declared value with its divisor
//! spelled another way, a declared value with a stray space, no characters at all), and WHERE
//! in the request the datum carrying it sits — the first of a batch, a later one, and one that
//! rode in on `EntityMetricData` rather than on `MetricData`, at either depth of that nesting —
//! since the member path the sentence names is read off the request. The value and the position
//! a recorded publish happened to carry are one row among them.
//!
//! The set the sentence enumerates is asserted as a SET against the values the type declares,
//! because membership is what the type fixes; the order it is announced in is this service's own
//! and is asserted by `crate::announced_value_sets`.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::announced_value_sets::unit_in_set_message;
use crate::harness::{
    build_router, call_in_query_mode, call_with_headers, error_message, now_secs, ok,
};
use crate::unit_read_identity::STANDARD_UNITS;

/// The namespace every publish in this suite names.
const NAMESPACE: &str = "Probe/UnitValues";

/// The header the legacy query error code and the fault side are reported in.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The qualified shape id the body names the parameter-value fault under: the service's own
/// modelled fault, in the package this service's faults are named in.
const PARAMETER_VALUE_SHAPE_ID: &str = "com.amazon.coral.service#InvalidParameterValueException";

/// The report the header carries for that fault: its legacy `awsQueryError` code and the fault
/// side a client branches on.
const PARAMETER_VALUE_REPORT: &str = "InvalidParameterValue;Sender";

/// The `__type` of a failure body, verbatim: the qualified id is part of what this suite
/// asserts, so it is read unreduced rather than through the harness's bare-name reader.
fn error_type(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A `MetricDatum` for `metric` carrying one value, with `unit` when one is given.
fn datum(metric: &str, unit: Option<&str>) -> Value {
    let mut datum = json!({
        "MetricName": metric,
        "Value": 1.0,
        "Timestamp": now_secs() - 60.0,
    });
    if let Some(unit) = unit {
        datum["Unit"] = json!(unit);
    }
    datum
}

/// `count` data, the `position`th (1-based) carrying `unit` and the others a unit the type
/// declares — so the member path a refusal names comes from the request rather than from a
/// fixed place.
fn data(metric: &str, count: usize, position: usize, unit: &str) -> Value {
    Value::Array(
        (1..=count)
            .map(|index| {
                let carried = if index == position { unit } else { "Count" };
                datum(&format!("{metric}{index}"), Some(carried))
            })
            .collect(),
    )
}

/// A publish of `data` under [`NAMESPACE`].
fn publish(data: Value) -> Value {
    json!({"Namespace": NAMESPACE, "MetricData": data})
}

/// The same publish carried on `EntityMetricData`: `entities` entities, the `entity`th (1-based)
/// holding the offending datum.
fn publish_as_entity_data(metric: &str, entities: usize, entity: usize, data: Value) -> Value {
    let entries: Vec<Value> = (1..=entities)
        .map(|index| {
            json!({
                "Entity": {"KeyAttributes": {"Type": "Service", "Name": format!("probe{index}")}},
                "MetricData": if index == entity {
                    data.clone()
                } else {
                    json!([datum(&format!("{metric}Held{index}"), Some("Count"))])
                },
            })
        })
        .collect();
    json!({
        "Namespace": NAMESPACE,
        "StrictEntityValidation": false,
        "EntityMetricData": entries,
    })
}

/// One refused publish: what it varies, the request, and the request member path the refusal
/// has to name.
struct Row {
    /// What this row varies, named in every assertion it makes.
    case: &'static str,
    /// The whole publish the row sends.
    request: Value,
    /// The member path the sentence names.
    parameter: String,
}

/// Every row of the table: a unit outside the set, in each way it can miss the set and at each
/// place a datum can sit.
fn rows() -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::new();

    // The ways a submitted value misses the set, each on the first datum of a one-datum batch.
    for (case, unit) in [
        ("a word that names no unit", "Furlongs"),
        ("a declared value in another case", "count"),
        ("a declared value with its divisor spelled another way", "Bytes/sec"),
        ("a declared value with a stray space", "Count "),
        ("a unit of no characters at all", ""),
    ] {
        rows.push(Row {
            case,
            request: publish(data("Missed", 1, 1, unit)),
            parameter: "MetricData.member.1.Unit".to_string(),
        });
    }

    // The same miss further along the batch: the position the sentence names is the datum's own.
    for (case, count, position) in [
        ("the offending datum second in its batch", 3, 2),
        ("the offending datum last in its batch", 4, 4),
    ] {
        rows.push(Row {
            case,
            request: publish(data("Placed", count, position, "Furlongs")),
            parameter: format!("MetricData.member.{position}.Unit"),
        });
    }

    // The same value carried on the other list a publish may use, at either depth of its
    // nesting: which entity, and which datum of that entity's data.
    rows.push(Row {
        case: "the offending datum carried as entity metric data",
        request: publish_as_entity_data("Entity", 1, 1, data("EntityMissed", 1, 1, "Furlongs")),
        parameter: "EntityMetricData.member.1.MetricData.member.1.Unit".to_string(),
    });
    rows.push(Row {
        case: "the offending datum second in an entity's data",
        request: publish_as_entity_data("Entity", 1, 1, data("EntityPlaced", 2, 2, "Furlongs")),
        parameter: "EntityMetricData.member.1.MetricData.member.2.Unit".to_string(),
    });
    rows.push(Row {
        case: "the offending datum under the second entity of a batch",
        request: publish_as_entity_data("Entity", 2, 2, data("EntitySecond", 1, 1, "Furlongs")),
        parameter: "EntityMetricData.member.2.MetricData.member.1.Unit".to_string(),
    });

    rows
}

/// How many datapoints the window around now holds for `metric` — 0 for a series no accepted
/// publish ever created.
async fn stored_samples(router: &smithy_server_runtime::MultiServiceRouter, metric: &str) -> f64 {
    let now = now_secs();
    let stats = ok(
        router,
        "GetMetricStatistics",
        &json!({
            "Namespace": NAMESPACE,
            "MetricName": metric,
            "StartTime": now - 600.0,
            "EndTime": now + 60.0,
            "Period": 60,
            "Statistics": ["SampleCount"],
        }),
    )
    .await;
    stats["Datapoints"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|point| point["SampleCount"].as_f64().unwrap_or_default())
        .sum()
}

#[tokio::test]
async fn a_unit_outside_the_declared_set_is_refused_as_the_modelled_parameter_value_fault() {
    for Row {
        case,
        request,
        parameter,
    } in rows()
    {
        // A fresh emulator per row, so what the row's own publish left behind is the whole
        // store the ingest assertion reads.
        let router = build_router();

        // 1. A query-compatible client reads the modelled class in both places.
        let (status, headers, body) = call_in_query_mode(&router, "PutMetricData", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?} (request {request}): expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_type(&body),
            PARAMETER_VALUE_SHAPE_ID,
            "case {case:?}: expected the body to name {PARAMETER_VALUE_SHAPE_ID}, got {body}"
        );
        let reported = headers
            .get(QUERY_ERROR_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            reported, PARAMETER_VALUE_REPORT,
            "case {case:?}: expected {QUERY_ERROR_HEADER} to read {PARAMETER_VALUE_REPORT:?}, \
             got {reported:?}"
        );

        // 2. The sentence names the member path the value arrived on and the whole set.
        let expected = unit_in_set_message(&parameter);
        let stated = error_message(&body);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );

        // 3. A native JSON client gets the same answer and no legacy report.
        let (plain_status, plain_headers, plain_body) =
            call_with_headers(&router, "PutMetricData", &request).await;
        assert_eq!(
            (plain_status, error_type(&plain_body)),
            (400, PARAMETER_VALUE_SHAPE_ID.to_string()),
            "case {case:?}: a native JSON client must get the same answer, got {plain_status} \
             with {plain_body}"
        );
        assert!(
            plain_headers.get(QUERY_ERROR_HEADER).is_none(),
            "case {case:?}: a client that did not opt in must get no report, got {:?}",
            plain_headers.get(QUERY_ERROR_HEADER)
        );

        // 4. "Any validation error (for entity or metric data) will fail the entire request,
        // and no data will be ingested." (`PutMetricDataInput$StrictEntityValidation`) — not
        // even the data of the batch whose own unit the type declares.
        for metric in ["Missed1", "Placed1", "EntityMissed1", "EntityHeld1"] {
            let stored = stored_samples(&router, metric).await;
            assert_eq!(
                stored, 0.0,
                "case {case:?}: a refused publish must ingest nothing, got {stored} samples of \
                 {metric}"
            );
        }
    }
}

#[tokio::test]
async fn the_refusal_enumerates_exactly_the_units_the_type_declares() {
    let router = build_router();
    let (status, _headers, body) = call_in_query_mode(
        &router,
        "PutMetricData",
        &publish(data("Enumerated", 1, 1, "Furlongs")),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400, got {status} with {body}"
    );

    let stated = error_message(&body);
    let listed: BTreeSet<&str> = stated
        .split_once('[')
        .and_then(|(_, tail)| tail.split_once(']'))
        .map(|(set, _)| set.split(',').map(str::trim).collect())
        .unwrap_or_default();
    let declared: BTreeSet<&str> = STANDARD_UNITS.iter().copied().collect();
    assert_eq!(
        listed, declared,
        "the set the refusal enumerates must be exactly the units the type declares: got \
         {listed:?} against {declared:?} in {stated:?}"
    );
    // The submitted value is not among them, and is not echoed anywhere in the sentence: the
    // set it had to come from is what locates the repair.
    assert!(
        !stated.contains("Furlongs"),
        "the submitted value must not be echoed, got {stated:?}"
    );
}
