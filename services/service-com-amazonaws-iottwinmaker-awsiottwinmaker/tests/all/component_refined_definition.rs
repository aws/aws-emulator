// SPDX-License-Identifier: CC0-1.0

//! Where a component's reported property definition COMES FROM, end to end over
//! HTTP.
//!
//! RULE under test: `isInherited` on a definition a reader reports for an
//! entity's component is true exactly when the component type the component
//! instantiates — or a component type that type extends — declares that
//! property, whether or not the component's own record also declares one for it;
//! and the definition reported carries the inherited value of every member the
//! component's declaration left alone. The same rule decides a component's
//! property GROUP flag, and it is the rule `GetComponentType` already applies to
//! a component type that restates a declaration its `extendsFrom` chain makes,
//! so both readers of the flag answer one question one way.
//!
//! Member optionality is the model's:
//! smithy_query(query="shape", shape="PropertyDefinitionRequest,PropertyDefinitionResponse")
//! — every member of the REQUEST but none of `dataType`, `isInherited` and the
//! other flags of the RESPONSE is optional, which is what lets a declaration
//! restate only the members it changes.
//!
//! The rows differ along what the rule generalises over: WHICH members the
//! component restates (none, one flag, a display name, a default, every member),
//! HOW FAR away the declaring type is (the type the component instantiates, a
//! type that type extends), whether the chain declares the property at ALL (a
//! property only the component declares, a component whose type this workspace
//! does not hold), and WHICH kind of component carries the declaration (a
//! component, a composite component).

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below reads from.
const WORKSPACE: &str = "refinement";

/// The component type at the root of the chain: it declares the refined property
/// WHOLE — a display name, a default value and a data type — plus a second
/// property and one property group.
const GAUGE: &str = "com.example.gauge";
/// A component type that extends [`GAUGE`] and RESTATES the same property
/// carrying only `isTimeSeries`, so a component of it reaches the display name
/// two declarations away.
const TUNED: &str = "com.example.tuned";
/// A component type whose composite component type is a [`GAUGE`], so a
/// composite component can carry the same refining declaration a component does.
const HOLDER: &str = "com.example.holder";
/// A `componentTypeId` this workspace holds no component type under, so a
/// component of it inherits nothing: the service stores such a component (a
/// real workspace carries more built-in component types than this emulator
/// models) and resolves no declaration through it.
const ABSENT: &str = "com.example.absent";

/// The property every case resolves.
const PROPERTY: &str = "Reading";
/// A second property of [`GAUGE`], so every case also covers a property the
/// component never mentions.
const OTHER_PROPERTY: &str = "Label";
/// The group every group case resolves.
const GROUP: &str = "Readings";
/// The one component name every case declares, so every assertion addresses the
/// same component whatever the row put in it.
const COMPONENT: &str = "Spec";
/// The composite component the composite row declares under [`COMPONENT`].
const COMPOSITE: &str = "inner";

/// The display name [`GAUGE`] declares for [`PROPERTY`], which a refining
/// declaration that leaves it alone has to keep.
const INHERITED_DISPLAY_NAME: &str = "Reading (bar)";

/// Create the workspace and the component types the cases instantiate.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    // In reference order: `extendsFrom` and `compositeComponentTypes` have to
    // name a component type the workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            GAUGE,
            json!({
                "propertyDefinitions": {
                    PROPERTY: {
                        "dataType": {"type": "DOUBLE"},
                        "displayName": INHERITED_DISPLAY_NAME,
                        "defaultValue": {"doubleValue": 1.0},
                    },
                    OTHER_PROPERTY: {"dataType": {"type": "STRING"}},
                },
                "propertyGroups": {
                    GROUP: {"groupType": "TABULAR", "propertyNames": [PROPERTY, OTHER_PROPERTY]},
                },
            }),
        ),
        (
            TUNED,
            json!({
                "extendsFrom": [GAUGE],
                "propertyDefinitions": {
                    PROPERTY: {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                },
            }),
        ),
        (
            HOLDER,
            json!({"compositeComponentTypes": {COMPOSITE: {"componentTypeId": GAUGE}}}),
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
}

/// One row of the rule: the declaration a component carries for [`PROPERTY`],
/// and what a reader has to report for it.
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The component type the component instantiates — every component names
    /// one, [`ABSENT`] where this workspace holds none under it.
    component_type_id: &'static str,
    /// The component's own declaration for the property the row resolves, or
    /// `None` where it declares nothing.
    declaration: Option<Value>,
    /// The property the row resolves — [`PROPERTY`] for a property the chain
    /// declares, another name for one only the component does.
    property_name: &'static str,
    /// Whether the reported definition has to be marked inherited.
    is_inherited: bool,
    /// The `displayName` the reported definition has to carry.
    display_name: Option<&'static str>,
    /// The `isTimeSeries` the reported definition has to carry.
    is_time_series: bool,
    /// The `dataType.type` the reported definition has to carry.
    data_type: &'static str,
    /// The `defaultValue` the reported definition has to carry.
    default_value: Option<Value>,
}

/// Every row of the rule.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "declaresNothingSoTheWholeDefinitionIsTheTypes",
            entity_id: "declares-nothing",
            component_type_id: GAUGE,
            declaration: None,
            property_name: PROPERTY,
            is_inherited: true,
            display_name: Some(INHERITED_DISPLAY_NAME),
            is_time_series: false,
            data_type: "DOUBLE",
            default_value: Some(json!({"doubleValue": 1.0})),
        },
        Case {
            what: "refinesOneFlagAndKeepsEveryOtherInheritedMember",
            entity_id: "refines-flag",
            component_type_id: GAUGE,
            declaration: Some(json!({"dataType": {"type": "DOUBLE"}, "isTimeSeries": true})),
            property_name: PROPERTY,
            is_inherited: true,
            display_name: Some(INHERITED_DISPLAY_NAME),
            is_time_series: true,
            data_type: "DOUBLE",
            default_value: Some(json!({"doubleValue": 1.0})),
        },
        Case {
            what: "refinesTheDisplayNameAndKeepsTheInheritedDefault",
            entity_id: "refines-display-name",
            component_type_id: GAUGE,
            declaration: Some(json!({
                "dataType": {"type": "DOUBLE"},
                "displayName": "Reading (psi)",
            })),
            property_name: PROPERTY,
            is_inherited: true,
            display_name: Some("Reading (psi)"),
            is_time_series: false,
            data_type: "DOUBLE",
            default_value: Some(json!({"doubleValue": 1.0})),
        },
        Case {
            what: "refinesTheDefaultAndKeepsTheInheritedDisplayName",
            entity_id: "refines-default",
            component_type_id: GAUGE,
            declaration: Some(json!({
                "dataType": {"type": "DOUBLE"},
                "defaultValue": {"doubleValue": 7.5},
            })),
            property_name: PROPERTY,
            is_inherited: true,
            display_name: Some(INHERITED_DISPLAY_NAME),
            is_time_series: false,
            data_type: "DOUBLE",
            default_value: Some(json!({"doubleValue": 7.5})),
        },
        Case {
            what: "restatingEveryMemberStillReachesThePropertyThroughTheType",
            entity_id: "restates-everything",
            component_type_id: GAUGE,
            declaration: Some(json!({
                "dataType": {"type": "LONG"},
                "displayName": "Reading (counts)",
                "defaultValue": {"longValue": 4},
                "isTimeSeries": true,
                "isRequiredInEntity": true,
                "isExternalId": false,
                "isStoredExternally": false,
            })),
            property_name: PROPERTY,
            is_inherited: true,
            display_name: Some("Reading (counts)"),
            is_time_series: true,
            data_type: "LONG",
            default_value: Some(json!({"longValue": 4})),
        },
        Case {
            what: "refinesADeclarationTwoTypesAwayAlongTheChain",
            entity_id: "refines-chain",
            component_type_id: TUNED,
            declaration: Some(json!({
                "dataType": {"type": "DOUBLE"},
                "defaultValue": {"doubleValue": 3.5},
            })),
            property_name: PROPERTY,
            is_inherited: true,
            // `isTimeSeries` comes from the nearer type, `displayName` from the
            // one it extends: the folded definition the component refines spans
            // the whole chain.
            display_name: Some(INHERITED_DISPLAY_NAME),
            is_time_series: true,
            data_type: "DOUBLE",
            default_value: Some(json!({"doubleValue": 3.5})),
        },
        Case {
            what: "aPropertyNoComponentTypeOnTheChainDeclaresIsTheComponentsOwn",
            entity_id: "own-property",
            component_type_id: TUNED,
            declaration: Some(json!({
                "dataType": {"type": "BOOLEAN"},
                "displayName": "Spare",
            })),
            property_name: "Spare",
            is_inherited: false,
            display_name: Some("Spare"),
            is_time_series: false,
            data_type: "BOOLEAN",
            default_value: None,
        },
        Case {
            what: "aComponentOfAnUnheldComponentTypeInheritsNothing",
            entity_id: "absent-type",
            component_type_id: ABSENT,
            declaration: Some(json!({"dataType": {"type": "INTEGER"}})),
            property_name: "Only",
            is_inherited: false,
            display_name: None,
            is_time_series: false,
            data_type: "INTEGER",
            default_value: None,
        },
    ]
}

/// The component the case's create carries, under the name [`COMPONENT`].
fn component_request(case: &Case) -> Value {
    let mut component = serde_json::Map::new();
    component.insert(
        "componentTypeId".to_string(),
        json!(case.component_type_id),
    );
    if let Some(declaration) = &case.declaration {
        component.insert(
            "properties".to_string(),
            json!({case.property_name: {"definition": declaration.clone()}}),
        );
    }
    Value::Object(component)
}

/// Create the case's entity and answer the component `GetEntity` reports for it.
async fn created_component(emulator: &Emulator, case: &Case) -> Value {
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": case.entity_id,
                "entityName": case.entity_id,
                "components": {COMPONENT: component_request(case)},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {:?}: expected 200 from CreateEntity, got {created:?}",
        case.what
    );

    let read = emulator.get_entity(WORKSPACE, case.entity_id).await;
    assert_eq!(
        read.status, 200,
        "case {:?}: expected 200 from GetEntity, got {read:?}",
        case.what
    );
    read.member("components")[COMPONENT].clone()
}

/// Every member of the rule a reported definition has to answer for, as the
/// tuple a row compares against: where the definition comes from
/// (`isInherited`), and the four members a refinement either sets or leaves to
/// the chain.
type ReportedDefinition<'a> = (
    Option<bool>,
    Option<&'a str>,
    Option<bool>,
    Option<&'a str>,
    Option<Value>,
);

/// The reported definition as that tuple.
fn reported(definition: &Value) -> ReportedDefinition<'_> {
    (
        definition["isInherited"].as_bool(),
        definition["displayName"].as_str(),
        definition["isTimeSeries"].as_bool(),
        definition["dataType"]["type"].as_str(),
        definition.get("defaultValue").cloned(),
    )
}

/// The tuple the case expects.
fn wanted(case: &Case) -> ReportedDefinition<'static> {
    (
        Some(case.is_inherited),
        case.display_name,
        Some(case.is_time_series),
        Some(case.data_type),
        case.default_value.clone(),
    )
}

/// RULE: `GetEntity` marks a component's property definition inherited exactly
/// when the component's inheritance chain declares the property — a declaration
/// the component refines included — and reports the inherited value of every
/// member that declaration left alone.
#[tokio::test]
async fn a_refining_component_declaration_is_still_reached_through_the_type() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let component = created_component(&emulator, &case).await;
        let definition = &component["properties"][case.property_name]["definition"];
        assert_eq!(
            reported(definition),
            wanted(&case),
            "case {:?}: expected the folded definition marked by where it comes from, got \
             {definition:?}",
            case.what
        );

        // The property the component never mentions is the boundary of the
        // rule's other side: wherever the chain declares one, it is inherited.
        // A component type this workspace does not hold declares nothing, so
        // that row has no such property.
        if case.component_type_id != ABSENT {
            let untouched = &component["properties"][OTHER_PROPERTY]["definition"];
            assert_eq!(
                untouched["isInherited"].as_bool(),
                Some(true),
                "case {:?}: expected the property the component never mentioned to be inherited, \
                 got {untouched:?}",
                case.what
            );
        }
    }
}

/// RULE (one property, one answer): `ListProperties` reports the same definition
/// `GetEntity` does, flag included, so the two readers of a component's
/// properties cannot disagree about where a definition comes from.
#[tokio::test]
async fn list_properties_reports_the_same_flag_as_get_entity() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let component = created_component(&emulator, &case).await;

        let listed = emulator
            .list_properties(
                WORKSPACE,
                json!({"entityId": case.entity_id, "componentName": COMPONENT}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListProperties, got {listed:?}",
            case.what
        );
        let row = listed
            .member("propertySummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|row| row["propertyName"] == case.property_name)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected a row for {:?}, got {listed:?}",
                    case.what, case.property_name
                )
            });
        assert_eq!(
            reported(&row["definition"]),
            wanted(&case),
            "case {:?}: expected the folded definition from ListProperties, got {row:?}",
            case.what
        );
        assert_eq!(
            row["definition"],
            component["properties"][case.property_name]["definition"],
            "case {:?}: expected ListProperties and GetEntity to report one definition, got \
             {row:?}",
            case.what
        );
    }
}

/// RULE: a COMPOSITE component resolves the flag the same way, since the
/// definition reaches it through the component type its path's declaration
/// names.
#[tokio::test]
async fn a_composite_components_refining_declaration_is_inherited_too() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let entity_id = "composite-refinement";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {"componentTypeId": HOLDER}},
                "compositeComponents": {
                    format!("{COMPONENT}/{COMPOSITE}"): {
                        "properties": {PROPERTY: {"definition": {
                            "dataType": {"type": "DOUBLE"},
                            "isTimeSeries": true,
                        }}},
                        "propertyGroups": {GROUP: {"groupType": "TABULAR"}},
                    },
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "componentPath": format!("{COMPONENT}/{COMPOSITE}"),
            }),
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
        .find(|row| row["propertyName"] == PROPERTY)
        .unwrap_or_else(|| panic!("expected a row for {PROPERTY:?}, got {listed:?}"));
    assert_eq!(
        reported(&row["definition"]),
        (
            Some(true),
            Some(INHERITED_DISPLAY_NAME),
            Some(true),
            Some("DOUBLE"),
            Some(json!({"doubleValue": 1.0})),
        ),
        "expected the composite component's refining declaration to be reported as reached \
         through its component type, got {row:?}"
    );

    let read = emulator.get_entity(WORKSPACE, entity_id).await;
    assert_eq!(read.status, 200, "expected 200 from GetEntity, got {read:?}");
    let group =
        &read.member("components")[COMPONENT]["compositeComponents"][COMPOSITE]["propertyGroups"]
            [GROUP];
    assert_eq!(
        (
            group["isInherited"].as_bool(),
            group["propertyNames"].as_array().cloned().unwrap_or_default(),
        ),
        (
            Some(true),
            vec![json!(PROPERTY), json!(OTHER_PROPERTY)],
        ),
        "expected the composite component's restated group to keep the inherited names and be \
         reported as reached through its component type, got {group:?}"
    );
}

/// RULE (one flag, one rule): the component-side flag and the component-TYPE
/// side flag answer the same question the same way — a declaration the
/// inheritance chain makes is inherited whether or not the nearer reader also
/// restates it — for a property definition and for a property group alike.
#[tokio::test]
async fn a_component_and_a_component_type_answer_the_flag_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The component TYPE that refines the inherited definition.
    let read = emulator.get_component_type(WORKSPACE, TUNED).await;
    assert_eq!(
        read.status, 200,
        "expected 200 from GetComponentType, got {read:?}"
    );
    let by_type = &read.member("propertyDefinitions")[PROPERTY];
    assert_eq!(
        by_type["isInherited"].as_bool(),
        Some(true),
        "expected a component type that refines an inherited definition to report it inherited, \
         got {by_type:?}"
    );

    // The component that refines the same inherited definition, and restates
    // the inherited group carrying only its `groupType`.
    let entity_id = "parity";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {
                    "componentTypeId": GAUGE,
                    "properties": {PROPERTY: {"definition": {
                        "dataType": {"type": "DOUBLE"},
                        "isTimeSeries": true,
                    }}},
                    "propertyGroups": {GROUP: {"groupType": "TABULAR"}},
                    "description": "refines what its type declares",
                }},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );
    let component = emulator
        .get_entity(WORKSPACE, entity_id)
        .await
        .member("components")[COMPONENT]
        .clone();
    let by_component = &component["properties"][PROPERTY]["definition"];
    assert_eq!(
        by_component["isInherited"].as_bool(),
        by_type["isInherited"].as_bool(),
        "expected a component that refines an inherited definition to answer the flag as its \
         component type does, got {by_component:?}"
    );

    let group = &component["propertyGroups"][GROUP];
    assert_eq!(
        (
            group["isInherited"].as_bool(),
            group["propertyNames"].as_array().cloned().unwrap_or_default(),
        ),
        (Some(true), vec![json!(PROPERTY), json!(OTHER_PROPERTY)]),
        "expected a component that restates an inherited group to keep its names and report it \
         inherited, got {group:?}"
    );

    // A group only the component declares reaches it through nothing, so it is
    // its own: the boundary the group rule sits on.
    let own_group_entity = "own-group";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": own_group_entity,
                "entityName": own_group_entity,
                "components": {COMPONENT: {
                    "componentTypeId": GAUGE,
                    "propertyGroups": {"Spares": {"propertyNames": [OTHER_PROPERTY]}},
                }},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );
    let own_group = emulator
        .get_entity(WORKSPACE, own_group_entity)
        .await
        .member("components")[COMPONENT]["propertyGroups"]["Spares"]
        .clone();
    assert_eq!(
        own_group["isInherited"].as_bool(),
        Some(false),
        "expected a group no component type on the chain declares to be the component's own, got \
         {own_group:?}"
    );
}
