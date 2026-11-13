// SPDX-License-Identifier: CC0-1.0

//! Which component a `componentPath` names, asserted over EVERY property
//! operation that accepts one, end to end over HTTP.
//!
//! RULE under test: one `componentPath` names one component of an entity —
//! a path of a SINGLE segment names the entity's top-level component of that
//! name (the very component `componentName` addresses), and a longer path names
//! the composite component held under the REST of the path inside that
//! component, whatever depth it sits at. Every operation that reads or writes a
//! property through a path resolves it that way, so the four of them
//! (`ListProperties`, `GetPropertyValue`, `BatchPutPropertyValues`,
//! `GetPropertyValueHistory`) cannot disagree about which component one path
//! names, and a path that resolves to nothing is refused by all of them.
//!
//! The rows below differ along what the rule generalises over: the DEPTH of the
//! path (one segment, two, three), whether the component type the first
//! segment's component instantiates declares a write connector, and — for the
//! paths that resolve to nothing — the segment that fails to resolve (the first
//! one, a deeper one, one past the deepest created, one differing only in case,
//! and the empty segment a trailing separator leaves).

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator, Resp};

/// The workspace every case below reads and writes.
const WORKSPACE: &str = "pathrule";

/// The entity carrying every component the paths below address.
const ENTITY: &str = "unit-1";

/// The component type at the bottom of the chain: properties only.
const LEAF: &str = "com.example.pathleaf";
/// The middle component type: the same properties, plus a composite component
/// type of [`LEAF`] under `deeper`.
const MID: &str = "com.example.pathmid";
/// The top component type an entity component instantiates: the same
/// properties, plus a composite component type of [`MID`] under `inner`.
const TOP: &str = "com.example.pathtop";
/// A component type carrying the same properties and NO `functions`, so a
/// component of it declares no write connector.
const NO_WRITER: &str = "com.example.pathplain";

/// The property whose stored value says WHICH component answered: every
/// component below carries a different one.
const MARKER: &str = "Marker";
/// The property the time-series data plane writes and reads, marked
/// `isTimeSeries` on every component below.
const READING: &str = "Reading";

/// The instant every time-series write below carries.
const T0: &str = "2026-03-01T00:00:00Z";

/// The property definitions every component in the chain carries: the marker
/// and one time series property.
fn property_definitions() -> Value {
    json!({
        MARKER: {"dataType": {"type": "STRING"}},
        READING: {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
    })
}

/// The one path of [`ADDRESSED`] whose component instantiates a component type
/// declaring no write connector, so the time-series data plane sets no value of
/// its properties.
const PATH_WITHOUT_A_WRITE_CONNECTOR: &str = "Plain";

/// The workspace, the component-type chain and the one entity every case reads.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            LEAF,
            json!({
                "propertyDefinitions": property_definitions(),
                "functions": data_writer_functions(),
            }),
        ),
        (
            MID,
            json!({
                "propertyDefinitions": property_definitions(),
                "compositeComponentTypes": {"deeper": {"componentTypeId": LEAF}},
                "functions": data_writer_functions(),
            }),
        ),
        (
            TOP,
            json!({
                "propertyDefinitions": property_definitions(),
                "compositeComponentTypes": {"inner": {"componentTypeId": MID}},
                // The write connector a value is set THROUGH, declared by every
                // component type of the chain.
                "functions": data_writer_functions(),
            }),
        ),
        (
            NO_WRITER,
            json!({"propertyDefinitions": property_definitions()}),
        ),
    ];
    for (component_type_id, body) in component_types {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, body)
            .await;
        assert_eq!(
            created.status, 200,
            "CreateComponentType {component_type_id:?}: expected 200, got {created:?}"
        );
    }

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": ENTITY,
                "entityName": ENTITY,
                "components": {
                    "Assembly": {
                        "componentTypeId": TOP,
                        "properties": {MARKER: {"value": {"stringValue": "Assembly"}}},
                    },
                    // A component of a component type that declares no write
                    // connector, so the definitions a one-segment path reads
                    // are the ones the component itself declares.
                    "Plain": {
                        "componentTypeId": NO_WRITER,
                        "properties": {
                            MARKER: {
                                "definition": {"dataType": {"type": "STRING"}},
                                "value": {"stringValue": "Plain"},
                            },
                            READING: {
                                "definition": {
                                    "dataType": {"type": "DOUBLE"},
                                    "isTimeSeries": true,
                                },
                            },
                        },
                    },
                },
                "compositeComponents": {
                    "Assembly/inner": {
                        "properties": {MARKER: {"value": {"stringValue": "Assembly/inner"}}},
                    },
                    "Assembly/inner/deeper": {
                        "properties": {
                            MARKER: {"value": {"stringValue": "Assembly/inner/deeper"}},
                        },
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "CreateEntity {ENTITY:?}: expected 200, got {created:?}"
    );
}

/// One `componentPath` that resolves, and the component it names.
#[derive(Debug)]
struct Addressed {
    case: &'static str,
    path: &'static str,
    /// The `componentName` that addresses the SAME component, for a path of a
    /// single segment; `None` for a path that names a composite component,
    /// which no name addresses.
    component_name: Option<&'static str>,
    /// The value the addressed component's marker property reads as: what says
    /// the path resolved to that component and no other.
    marker: &'static str,
}

/// The paths the rule is asserted over, one per depth the chain created.
const ADDRESSED: [Addressed; 4] = [
    Addressed {
        case: "a one-segment path naming a top-level component that instantiates a type",
        path: "Assembly",
        component_name: Some("Assembly"),
        marker: "Assembly",
    },
    Addressed {
        case: "a one-segment path naming a top-level component whose type declares no writer",
        path: "Plain",
        component_name: Some("Plain"),
        marker: "Plain",
    },
    Addressed {
        case: "a two-segment path naming a composite component inside a component",
        path: "Assembly/inner",
        component_name: None,
        marker: "Assembly/inner",
    },
    Addressed {
        case: "a three-segment path naming a composite component one level deeper",
        path: "Assembly/inner/deeper",
        component_name: None,
        marker: "Assembly/inner/deeper",
    },
];

/// Every `componentPath` the entity holds no component at. The first segment,
/// a deeper segment, a segment past the deepest created, a segment differing
/// only in case and the empty segment a trailing separator leaves — none of
/// them resolves.
const UNRESOLVED: [(&str, &str); 6] = [
    ("a first segment no component of the entity is named by", "Missing"),
    (
        "a second segment no composite component of that component sits at",
        "Assembly/absent",
    ),
    (
        "a path into a component that holds no composite component at all",
        "Plain/inner",
    ),
    (
        "one segment past the deepest composite component created",
        "Assembly/inner/deeper/extra",
    ),
    (
        "a path differing from a component's name only in case",
        "ASSEMBLY",
    ),
    (
        "a trailing separator, which leaves an empty last segment",
        "Assembly/inner/",
    ),
];

/// The `propertySummaries` of a `ListProperties` response, keyed by property
/// name so a row is read by name rather than by position.
fn summaries(response: &Resp) -> Vec<(String, Value)> {
    response
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|summary| {
            (
                summary["propertyName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
                summary,
            )
        })
        .collect()
}

/// RULE: a `componentPath` names the same one component in `ListProperties` and
/// in `GetPropertyValue`, at every depth, and a path of one segment names
/// exactly what `componentName` names.
#[tokio::test]
async fn one_path_names_one_component_in_every_property_reader() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for addressed in &ADDRESSED {
        let case = addressed.case;

        let listed = emulator
            .list_properties(
                WORKSPACE,
                json!({"entityId": ENTITY, "componentPath": addressed.path}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200 from ListProperties, got {listed:?}"
        );
        let rows = summaries(&listed);
        let names: Vec<&str> = rows.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(
            names,
            vec![MARKER, READING],
            "case {case:?}: expected the addressed component's two properties, got {listed:?}"
        );
        let marker = rows
            .iter()
            .find(|(name, _)| name == MARKER)
            .map(|(_, summary)| summary["value"]["stringValue"].clone())
            .unwrap_or(Value::Null);
        assert_eq!(
            marker,
            json!(addressed.marker),
            "case {case:?}: expected ListProperties to report the marker {:?} of the component \
             the path names, got {marker:?}",
            addressed.marker
        );

        let read = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "entityId": ENTITY,
                    "componentPath": addressed.path,
                    "selectedProperties": [MARKER],
                }),
            )
            .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected 200 from GetPropertyValue, got {read:?}"
        );
        let value = read.member("propertyValues")[MARKER]["propertyValue"]["stringValue"].clone();
        assert_eq!(
            value,
            json!(addressed.marker),
            "case {case:?}: expected GetPropertyValue to report the marker {:?} of the component \
             the path names, got {read:?}",
            addressed.marker
        );

        // A path of one segment is another way of addressing the component
        // `componentName` names, so both forms answer the same.
        if let Some(component_name) = addressed.component_name {
            let by_name = emulator
                .list_properties(
                    WORKSPACE,
                    json!({"entityId": ENTITY, "componentName": component_name}),
                )
                .await;
            assert_eq!(
                by_name.status, 200,
                "case {case:?}: expected 200 from ListProperties by componentName, got {by_name:?}"
            );
            assert_eq!(
                by_name.body, listed.body,
                "case {case:?}: expected the one-segment path to answer exactly as \
                 componentName {component_name:?} does, got {by_name:?} against {listed:?}"
            );
        }
    }
}

/// RULE (the write side of the same resolution): a time-series write through a
/// `componentPath` lands on the component that path names, so reading the same
/// path back answers that write and no other path's. A path whose component
/// instantiates a component type declaring no write connector accepts no write
/// at all.
#[tokio::test]
async fn a_write_through_a_path_lands_on_the_component_the_path_names() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // One distinct value per path, so a value read back identifies the
    // component it was written to.
    let written: Vec<(&Addressed, f64)> = ADDRESSED
        .iter()
        .filter(|addressed| addressed.path != PATH_WITHOUT_A_WRITE_CONNECTOR)
        .enumerate()
        .map(|(index, addressed)| (addressed, 10.0 + index as f64))
        .collect();

    for (addressed, value) in &written {
        let put = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([{
                    "entityPropertyReference": {
                        "entityId": ENTITY,
                        "componentPath": addressed.path,
                        "propertyName": READING,
                    },
                    "propertyValues": [{"time": T0, "value": {"doubleValue": value}}],
                }]),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: expected 200 from BatchPutPropertyValues, got {put:?}",
            addressed.case
        );
        assert_eq!(
            put.member("errorEntries"),
            &json!([]),
            "case {:?}: expected the write to be accepted, got {put:?}",
            addressed.case
        );
    }

    // The component whose component type declares no write connector: nothing
    // can set a value of its properties, so the whole request is refused.
    let refused = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": ENTITY,
                    "componentPath": PATH_WITHOUT_A_WRITE_CONNECTOR,
                    "propertyName": READING,
                },
                "propertyValues": [{"time": T0, "value": {"doubleValue": 99.0}}],
            }]),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "case {PATH_WITHOUT_A_WRITE_CONNECTOR:?}: a component whose component type declares no \
         write connector, so the write must be refused, got {refused:?}"
    );

    for (addressed, value) in &written {
        let history = emulator
            .get_property_value_history(
                WORKSPACE,
                json!({
                    "entityId": ENTITY,
                    "componentPath": addressed.path,
                    "selectedProperties": [READING],
                }),
            )
            .await;
        assert_eq!(
            history.status, 200,
            "case {:?}: expected 200 from GetPropertyValueHistory, got {history:?}",
            addressed.case
        );
        let values: Vec<Value> = history
            .member("propertyValues")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .flat_map(|series| series["values"].as_array().cloned().unwrap_or_default())
            .collect();
        assert_eq!(
            values,
            vec![json!({"time": T0, "value": {"doubleValue": value}})],
            "case {:?}: expected the history of the component the path names to be the one \
             value written through it, got {history:?}",
            addressed.case
        );
    }
}

/// RULE (the boundary): a `componentPath` the entity holds no component at
/// addresses nothing, so every reader refuses it — `ListComponents` and
/// `ListProperties` as a 404 `ResourceNotFoundException`, while the property data
/// plane (`GetPropertyValue`, `GetPropertyValueHistory` and
/// `BatchPutPropertyValues`) reads it as the bad REFERENCE it is and refuses the
/// whole call with a 400 `ValidationException`.
#[tokio::test]
async fn a_path_that_names_no_component_is_refused_by_every_reader() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for (case, path) in UNRESOLVED {
        let listed = emulator
            .list_properties(WORKSPACE, json!({"entityId": ENTITY, "componentPath": path}))
            .await;
        assert_eq!(
            (listed.status, listed.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {case:?} ({path:?}): expected 404 ResourceNotFoundException from \
             ListProperties, got {listed:?}"
        );

        let read = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "entityId": ENTITY,
                    "componentPath": path,
                    "selectedProperties": [MARKER],
                }),
            )
            .await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} ({path:?}): expected 400 ValidationException from \
             GetPropertyValue, got {read:?}"
        );

        let history = emulator
            .get_property_value_history(
                WORKSPACE,
                json!({
                    "entityId": ENTITY,
                    "componentPath": path,
                    "selectedProperties": [READING],
                }),
            )
            .await;
        assert_eq!(
            (history.status, history.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} ({path:?}): expected 400 ValidationException from \
             GetPropertyValueHistory, got {history:?}"
        );

        let put = emulator
            .batch_put_property_values(
                WORKSPACE,
                json!([{
                    "entityPropertyReference": {
                        "entityId": ENTITY,
                        "componentPath": path,
                        "propertyName": READING,
                    },
                    "propertyValues": [{"time": T0, "value": {"doubleValue": 1.0}}],
                }]),
            )
            .await;
        assert_eq!(
            (put.status, put.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?} ({path:?}): expected 400 ValidationException from \
             BatchPutPropertyValues, got {put:?}"
        );
    }
}
