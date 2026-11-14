// SPDX-License-Identifier: CC0-1.0

//! Re-pointing the component type a STORED component instantiates, judged
//! against the property VALUES that component already holds, end to end over
//! HTTP.
//!
//! RULE under test: a value a component's property holds is a value of the
//! `dataType` a read reports for that property — the property's own declaration
//! refined over the ones it inherits from the component type the component
//! instantiates — so a writer that re-points that component type answers the
//! rule over every value the patched component would hold, not only over the
//! ones the request itself carried, and refuses the patch rather than leaving a
//! read reporting a stored value beside a `dataType` it contradicts. Where the
//! request names no `componentTypeId` it changes none of the rule's inputs, and
//! where the `dataType` a value is judged against does not move (the property
//! declares it itself, or the type the patch names declares the same one) there
//! is nothing to refuse.
//!
//! Member optionality and the typed value members are the model's:
//! smithy_query(query="shape", shape="PropertyDefinitionRequest,PropertyRequest,DataValue,ComponentUpdateRequest")
//! — every member of `PropertyDefinitionRequest` is optional (which is what lets
//! a component's property inherit its `dataType` from the component type it
//! instantiates), `DataValue` carries one typed member per `Type`, and
//! `ComponentUpdateRequest.componentTypeId` is the member an update re-points the
//! inheritance with.
//!
//! The rows differ along what the rule generalises over: the DATA TYPE the
//! stored value travels in and the one the re-pointed type declares (`STRING`
//! against `DOUBLE`, `LONG`, `BOOLEAN`), WHERE the `dataType` after the patch
//! comes from (the type's own declaration, one it inherits along `extendsFrom`,
//! the property's own declaration, the type the component already had), WHICH
//! plane wrote the stored value (the control plane, and the time-series data
//! plane in its own test), HOW MANY stored values the patch would contradict (so
//! the refusal names the first in property-name order), and whether the same
//! request also writes, resets or deletes the value in question.

use serde_json::{json, Value};

use crate::harness::{data_writer_functions, Emulator};

/// The workspace every case works in. Each test builds its own emulator, so the
/// id is shared without the cases sharing state.
const WORKSPACE: &str = "repointValsWs";

/// The component type every component is created instantiating: it declares both
/// properties the cases write values to as `STRING`, so the create's values are
/// values of the `dataType` they inherit.
const STRINGY: &str = "com.example.stringy";
/// Another component type declaring [`PROPERTY`] with the SAME `dataType`
/// [`STRINGY`] does, so re-pointing a component at it moves no stored value's
/// answer.
const ALIAS: &str = "com.example.alias";
/// A component type declaring both properties with ANOTHER `dataType` than
/// [`STRINGY`] does.
const DOUBLY: &str = "com.example.doubly";
/// A component type declaring [`PROPERTY`] as a `LONG`.
const LONGY: &str = "com.example.longy";
/// A component type declaring [`PROPERTY`] as a `BOOLEAN`.
const BOOLEANY: &str = "com.example.boolish";
/// A component type that declares NEITHER property, so a stored value reaches a
/// `dataType` only through a declaration of the component's own.
const PLAIN: &str = "com.example.plain";
/// A component type that declares nothing of its own and extends [`DOUBLY`], so
/// a stored value reaches the contradicting `dataType` one type further along the
/// chain.
const DERIVED: &str = "com.example.derived";
/// A component type whose composite component type is a [`STRINGY`], so a
/// composite component can hold the same stored value a component does.
const HOLDER: &str = "com.example.holder";
/// An id no case creates: a component type the workspace does not hold, which a
/// component may still name and which supplies no inheritance.
const ABSENT: &str = "com.example.absent";

/// A time series component type declaring [`PROPERTY`] as a `STRING`, with the
/// write connector the data plane writes through.
const SERIES_STRING: &str = "com.example.series.string";
/// The same as [`SERIES_STRING`] with another `dataType`, so re-pointing at it
/// contradicts a value the data plane wrote.
const SERIES_DOUBLE: &str = "com.example.series.double";
/// The same as [`SERIES_STRING`] under another id, so re-pointing at it
/// contradicts nothing.
const SERIES_ALIAS: &str = "com.example.series.alias";

/// The property every case writes a value to.
const PROPERTY: &str = "Reading";
/// A second property, written by the row that contradicts two stored values at
/// once. It sorts BEFORE [`PROPERTY`] in byte order, so a walk in property-name
/// order names it first.
const OTHER_PROPERTY: &str = "Label";
/// The composite component [`HOLDER`] declares.
const COMPOSITE: &str = "inner";
/// The one component name every case addresses.
const COMPONENT: &str = "Spec";

/// The instant the time-series case writes its value for, as the ISO 8601 `Time`
/// a write carries.
const T0: &str = "2026-04-01T00:00:00Z";

/// An emulator holding the workspace and the component types the cases point
/// their components at.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case works in must exist"
    );
    // In reference order: `extendsFrom` and `compositeComponentTypes` have to
    // name a component type the workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            STRINGY,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "STRING"}},
                    OTHER_PROPERTY: {"dataType": {"type": "STRING"}},
                },
            }),
        ),
        (
            ALIAS,
            json!({"propertyDefinitions": {PROPERTY: {"dataType": {"type": "STRING"}}}}),
        ),
        (
            DOUBLY,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "DOUBLE"}},
                    OTHER_PROPERTY: {"dataType": {"type": "DOUBLE"}},
                },
            }),
        ),
        (
            LONGY,
            json!({"propertyDefinitions": {PROPERTY: {"dataType": {"type": "LONG"}}}}),
        ),
        (
            BOOLEANY,
            json!({"propertyDefinitions": {PROPERTY: {"dataType": {"type": "BOOLEAN"}}}}),
        ),
        (
            PLAIN,
            json!({"propertyDefinitions": {"Other": {"dataType": {"type": "STRING"}}}}),
        ),
        (DERIVED, json!({"extendsFrom": [DOUBLY]})),
        (
            HOLDER,
            json!({"compositeComponentTypes": {COMPOSITE: {"componentTypeId": STRINGY}}}),
        ),
        (
            SERIES_STRING,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "STRING"}, "isTimeSeries": true},
                },
                "functions": data_writer_functions(),
            }),
        ),
        (
            SERIES_DOUBLE,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                },
                "functions": data_writer_functions(),
            }),
        ),
        (
            SERIES_ALIAS,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "STRING"}, "isTimeSeries": true},
                },
                "functions": data_writer_functions(),
            }),
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
    emulator
}

/// One row of the rule: the properties a create stores on the component, the
/// `componentUpdates` entry that patches it, and what the service has to answer.
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The `properties` the component carries when it is created, keyed by
    /// property name.
    properties: Value,
    /// The `componentUpdates.<COMPONENT>` entry the patch carries.
    patch: Value,
    /// The property the refusal has to name, with the data type the stored value
    /// carries and the one the patched component declares for it — `None` where
    /// the patch is accepted.
    refusal: Option<(&'static str, &'static str, &'static str)>,
    /// The `dataType.type` a read has to report for [`PROPERTY`] after the call:
    /// the one the create left in place where the patch is refused.
    reported_data_type: Option<&'static str>,
    /// The `value` a read has to report for [`PROPERTY`] after the call, or
    /// `null` where the property reads as nothing.
    reported_value: Value,
}

/// Every row of the rule.
fn cases() -> Vec<Case> {
    // The stored value the cases carry: a value of the `dataType` the component
    // INHERITS, which is exactly the value a re-point can contradict.
    let stored = || json!({PROPERTY: {"value": {"stringValue": "warm"}}});
    let warm = || json!({"stringValue": "warm"});
    vec![
        Case {
            what: "repointToATypeDeclaringThePropertyAsADouble",
            entity_id: "repoint-double",
            properties: stored(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": DOUBLY}),
            refusal: Some((PROPERTY, "STRING", "DOUBLE")),
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "repointToATypeDeclaringThePropertyAsALong",
            entity_id: "repoint-long",
            properties: stored(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": LONGY}),
            refusal: Some((PROPERTY, "STRING", "LONG")),
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "repointToATypeDeclaringThePropertyAsABoolean",
            entity_id: "repoint-boolean",
            properties: stored(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": BOOLEANY}),
            refusal: Some((PROPERTY, "STRING", "BOOLEAN")),
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "repointToATypeThatInheritsTheContradictingDeclaration",
            entity_id: "repoint-inherited",
            properties: stored(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": DERIVED}),
            refusal: Some((PROPERTY, "STRING", "DOUBLE")),
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "repointToATypeDeclaringTheSameDataType",
            entity_id: "repoint-same-type",
            properties: stored(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": ALIAS}),
            refusal: None,
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "repointToTheTypeTheComponentAlreadyInstantiates",
            entity_id: "repoint-restated",
            properties: stored(),
            patch: json!({"updateType": "UPDATE", "componentTypeId": STRINGY}),
            refusal: None,
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "aPropertyCarryingItsOwnDataTypeInheritsNothingSoNothingMoves",
            entity_id: "repoint-self-typed",
            properties: json!({PROPERTY: {
                "definition": {"dataType": {"type": "STRING"}},
                "value": {"stringValue": "warm"},
            }}),
            patch: json!({"updateType": "UPDATE", "componentTypeId": DOUBLY}),
            refusal: None,
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "repointToATypeThatDeclaresThePropertyNowhere",
            entity_id: "repoint-undeclared",
            properties: json!({PROPERTY: {
                "definition": {"dataType": {"type": "STRING"}},
                "value": {"stringValue": "warm"},
            }}),
            patch: json!({"updateType": "UPDATE", "componentTypeId": PLAIN}),
            refusal: None,
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "repointToAComponentTypeTheWorkspaceDoesNotHold",
            entity_id: "repoint-absent",
            properties: json!({PROPERTY: {
                "definition": {"dataType": {"type": "STRING"}},
                "value": {"stringValue": "warm"},
            }}),
            patch: json!({"updateType": "UPDATE", "componentTypeId": ABSENT}),
            refusal: None,
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "twoContradictedValuesAreRefusedOnTheFirstInPropertyNameOrder",
            entity_id: "repoint-two-values",
            properties: json!({
                PROPERTY: {"value": {"stringValue": "warm"}},
                OTHER_PROPERTY: {"value": {"stringValue": "tag"}},
            }),
            patch: json!({"updateType": "UPDATE", "componentTypeId": DOUBLY}),
            refusal: Some((OTHER_PROPERTY, "STRING", "DOUBLE")),
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
        Case {
            what: "theSameRequestMayWriteAValueOfTheDataTypeItRepointsTo",
            entity_id: "repoint-rewritten",
            properties: stored(),
            patch: json!({
                "updateType": "UPDATE",
                "componentTypeId": DOUBLY,
                "propertyUpdates": {PROPERTY: {"value": {"doubleValue": 1.5}}},
            }),
            refusal: None,
            reported_data_type: Some("DOUBLE"),
            reported_value: json!({"doubleValue": 1.5}),
        },
        Case {
            what: "theSameRequestMayResetTheValueTheRepointWouldContradict",
            entity_id: "repoint-reset",
            properties: stored(),
            patch: json!({
                "updateType": "UPDATE",
                "componentTypeId": DOUBLY,
                "propertyUpdates": {PROPERTY: {"updateType": "RESET_VALUE"}},
            }),
            refusal: None,
            reported_data_type: Some("DOUBLE"),
            reported_value: json!(null),
        },
        Case {
            what: "theSameRequestMayDeleteThePropertyTheRepointWouldContradict",
            entity_id: "repoint-deleted",
            properties: stored(),
            patch: json!({
                "updateType": "UPDATE",
                "componentTypeId": DOUBLY,
                "propertyUpdates": {PROPERTY: {"updateType": "DELETE"}},
            }),
            refusal: None,
            reported_data_type: Some("DOUBLE"),
            reported_value: json!(null),
        },
        Case {
            what: "anEntryThatNamesNoComponentTypeIdChangesNoneOfTheRulesInputs",
            entity_id: "repoint-untouched",
            properties: stored(),
            patch: json!({"updateType": "UPDATE", "description": "patched without a re-point"}),
            refusal: None,
            reported_data_type: Some("STRING"),
            reported_value: warm(),
        },
    ]
}

/// The refusal message the shared value rule earns for one stored value.
fn message(property_name: &str, carried: &str, declared: &str) -> String {
    format!(
        "the value written to property {property_name} is a {carried} where that property's \
         dataType is {declared}"
    )
}

/// RULE: `UpdateEntity` answers the component property VALUE rule over every
/// value the patched component would hold — the stored ones the request never
/// mentions included — so a re-pointed `componentTypeId` is refused rather than
/// leaving a read reporting a value beside a `dataType` it contradicts, and is
/// accepted wherever every value the component holds is still a value of the
/// `dataType` the patched component resolves for it.
#[tokio::test]
async fn repointing_a_components_type_is_refused_where_a_stored_value_contradicts_it() {
    let emulator = seeded().await;

    for case in cases() {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": case.entity_id,
                    "entityName": case.entity_id,
                    "components": {COMPONENT: {
                        "componentTypeId": STRINGY,
                        "properties": case.properties.clone(),
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: the component the patch addresses must exist, got {created:?}",
            case.what
        );

        let patched = emulator
            .update_entity(
                WORKSPACE,
                case.entity_id,
                json!({"componentUpdates": {COMPONENT: case.patch.clone()}}),
            )
            .await;
        match case.refusal {
            Some((property_name, carried, declared)) => {
                assert_eq!(
                    (patched.status, patched.error_type.as_deref()),
                    (400, Some("ValidationException")),
                    "case {:?}: expected the modelled refusal, got {patched:?}",
                    case.what
                );
                let expected = message(property_name, carried, declared);
                let reported = patched.member("message").as_str().unwrap_or_default();
                assert!(
                    reported.contains(expected.as_str()),
                    "case {:?}: expected the refusal to read {expected:?}, got {reported:?}",
                    case.what
                );
            }
            None => assert_eq!(
                patched.status, 200,
                "case {:?}: expected 200 from UpdateEntity, got {patched:?}",
                case.what
            ),
        }

        // What a read reports afterwards is the whole point of the rule: an
        // accepted patch leaves every value a value of the `dataType` reported
        // beside it, and a refused one leaves the entity exactly as the create
        // wrote it.
        let read = emulator.get_entity(WORKSPACE, case.entity_id).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected 200 from GetEntity, got {read:?}",
            case.what
        );
        let component = read.member("components")[COMPONENT].clone();
        let property = &component["properties"][PROPERTY];
        assert_eq!(
            property["definition"]["dataType"]["type"].as_str(),
            case.reported_data_type,
            "case {:?}: expected the reported definition's dataType, got {property:?}",
            case.what
        );
        assert_eq!(
            property["value"], case.reported_value,
            "case {:?}: expected the reported value, got {property:?}",
            case.what
        );
        if case.refusal.is_some() {
            assert_eq!(
                component["componentTypeId"].as_str(),
                Some(STRINGY),
                "case {:?}: a refused update changes nothing, got {component:?}",
                case.what
            );
        }

        // One property, one answer: `GetPropertyValue` reports the same value
        // `GetEntity` does, so the two readers cannot disagree about what the
        // patched component holds.
        let valued = emulator
            .get_property_value(
                WORKSPACE,
                json!({
                    "entityId": case.entity_id,
                    "componentName": COMPONENT,
                    "selectedProperties": [PROPERTY],
                }),
            )
            .await;
        assert_eq!(
            valued.status, 200,
            "case {:?}: expected 200 from GetPropertyValue, got {valued:?}",
            case.what
        );
        assert_eq!(
            valued.member("propertyValues")[PROPERTY]["propertyValue"], case.reported_value,
            "case {:?}: expected GetPropertyValue to report the value GetEntity does, got \
             {valued:?}",
            case.what
        );
    }
}

/// RULE (the same rule over the other plane's values): a value the TIME SERIES
/// data plane wrote is a value of the property's `dataType` too, so a re-point
/// that would contradict one is refused and the stored series is left whole.
#[tokio::test]
async fn repointing_a_components_type_is_refused_where_a_written_series_value_contradicts_it() {
    let emulator = seeded().await;

    let entity_id = "series-holder";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {"componentTypeId": SERIES_STRING}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the component the patches address must exist, got {created:?}"
    );

    let written = emulator
        .batch_put_property_values(
            WORKSPACE,
            json!([{
                "entityPropertyReference": {
                    "entityId": entity_id,
                    "componentName": COMPONENT,
                    "propertyName": PROPERTY,
                },
                "propertyValues": [{"time": T0, "value": {"stringValue": "warm"}}],
            }]),
        )
        .await;
    assert_eq!(
        written.status, 200,
        "expected 200 from BatchPutPropertyValues, got {written:?}"
    );
    let errors = written
        .member("errorEntries")
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        errors.iter().all(|entry| entry["errors"]
            .as_array()
            .is_none_or(|errors| errors.is_empty())),
        "expected BatchPutPropertyValues to serve the write in full, got {written:?}"
    );

    let refused = emulator
        .update_entity(
            WORKSPACE,
            entity_id,
            json!({"componentUpdates": {COMPONENT: {
                "updateType": "UPDATE",
                "componentTypeId": SERIES_DOUBLE,
            }}}),
        )
        .await;
    assert_eq!(
        (refused.status, refused.error_type.as_deref()),
        (400, Some("ValidationException")),
        "expected the modelled refusal for a re-point contradicting a written series value, got \
         {refused:?}"
    );
    let expected = message(PROPERTY, "STRING", "DOUBLE");
    let reported = refused.member("message").as_str().unwrap_or_default();
    assert!(
        reported.contains(expected.as_str()),
        "expected the refusal to read {expected:?}, got {reported:?}"
    );

    // A re-point that contradicts nothing is accepted, and the series the data
    // plane wrote is still there to read afterwards.
    let patched = emulator
        .update_entity(
            WORKSPACE,
            entity_id,
            json!({"componentUpdates": {COMPONENT: {
                "updateType": "UPDATE",
                "componentTypeId": SERIES_ALIAS,
            }}}),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "expected 200 from UpdateEntity for a re-point that contradicts no stored value, got \
         {patched:?}"
    );
    let history = emulator
        .get_property_value_history(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "componentName": COMPONENT,
                "selectedProperties": [PROPERTY],
            }),
        )
        .await;
    assert_eq!(
        history.status, 200,
        "expected 200 from GetPropertyValueHistory, got {history:?}"
    );
    let values: Vec<Value> = history
        .member("propertyValues")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .flat_map(|series| {
            series["values"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
        })
        .collect();
    assert_eq!(
        values
            .iter()
            .map(|value| (value["time"].clone(), value["value"].clone()))
            .collect::<Vec<_>>(),
        vec![(json!(T0), json!({"stringValue": "warm"}))],
        "expected the written series to survive an accepted re-point, got {history:?}"
    );
}

/// RULE (the boundary the rule's inputs draw): a COMPOSITE component's values are
/// judged against the component type its OWN record names, so re-pointing the
/// holding component's `componentTypeId` contradicts none of them and is
/// accepted.
#[tokio::test]
async fn repointing_the_holding_component_leaves_a_composite_components_value_alone() {
    let emulator = seeded().await;

    let entity_id = "composite-holder";
    let composite_path = format!("{COMPONENT}/{COMPOSITE}");
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {"componentTypeId": HOLDER}},
                "compositeComponents": {
                    composite_path.clone(): {
                        "properties": {PROPERTY: {"value": {"stringValue": "warm"}}},
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the composite component the patch leaves alone must exist, got {created:?}"
    );

    let patched = emulator
        .update_entity(
            WORKSPACE,
            entity_id,
            json!({"componentUpdates": {COMPONENT: {
                "updateType": "UPDATE",
                "componentTypeId": DOUBLY,
            }}}),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "expected 200 from UpdateEntity: the holding component holds no value of its own, got \
         {patched:?}"
    );

    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": entity_id, "componentPath": composite_path.clone()}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListProperties, got {listed:?}"
    );
    let row = listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|row| row["propertyName"] == PROPERTY);
    assert_eq!(
        row.as_ref().map(|row| (
            row["definition"]["dataType"]["type"].clone(),
            row["value"].clone()
        )),
        Some((json!("STRING"), json!({"stringValue": "warm"}))),
        "expected the composite component's stored value to keep the dataType its own component \
         type declares, got {row:?}"
    );
}
