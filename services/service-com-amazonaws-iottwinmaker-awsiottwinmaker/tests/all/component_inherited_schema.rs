// SPDX-License-Identifier: CC0-1.0

//! A component's EFFECTIVE schema, end to end over HTTP.
//!
//! RULE under test: a reader of an entity's component reports the schema the
//! component effectively has — every property and property group the component
//! type it instantiates, or a type that type extends, defines, merged under the
//! ones the entity declared inline — with each one marked `isInherited` exactly
//! when the component type it instantiates, or a type that type extends,
//! declares it (whether or not the entity also restated it), and `definedIn`
//! naming the component type the
//! inherited definition set came from (absent when nothing was resolved). Every
//! reader of a component answers alike: `GetEntity`, `ListComponents` and
//! `ListProperties`.
//!
//! The rows differ along what the rule generalises over: WHERE a member of the
//! schema comes from (the entity's own declaration, the component type, a type
//! that type extends), how MANY the type contributes (two, three, one, none),
//! whether the entity RESTATES one the type also defines, and whether the
//! component type resolves at all (one this workspace holds, one it does not,
//! none named).

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::harness::{Emulator, Resp};

/// The workspace every case below reads from.
const WORKSPACE: &str = "inheritance";

/// The component type at the root of the chain: two property definitions (one
/// carrying a default, one carrying a display name) and one property group.
const BASE: &str = "com.example.base";
/// A component type that extends [`BASE`], so a component of it carries one own
/// definition and two reached through `extendsFrom`.
const DERIVED: &str = "com.example.derived";
/// A component type that defines no property and no group: the boundary the rule
/// sits on, a resolvable definition set that contributes nothing.
const EMPTY: &str = "com.example.empty";
/// A component type id this workspace does NOT hold, so a component naming it
/// resolves no definition set at all.
const MISSING: &str = "com.example.missing";

/// Create the workspace and the three component types the cases instantiate.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    // In reference order: `extendsFrom` has to name a component type the
    // workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            BASE,
            json!({
                "propertyDefinitions": {
                    "Serial": {"dataType": {"type": "STRING"}, "displayName": "Serial number"},
                    "Tag": {
                        "dataType": {"type": "STRING"},
                        "defaultValue": {"stringValue": "unset"},
                    },
                },
                "propertyGroups": {
                    "Readings": {"groupType": "TABULAR", "propertyNames": ["Serial", "Tag"]},
                },
            }),
        ),
        (
            DERIVED,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {
                    "Pressure": {"dataType": {"type": "DOUBLE"}, "isTimeSeries": true},
                },
            }),
        ),
        (EMPTY, json!({})),
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

/// One property the component must report: its name, the `dataType.type` the
/// reported definition has to carry, and whether the definition is INHERITED —
/// i.e. whether the component's inheritance chain declares the property.
type ExpectedProperty = (&'static str, &'static str, bool);

/// One case of the rule: the component the entity's create carries, and the
/// whole schema a reader has to report for it.
struct Case {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The single component the create declares, under the name [`COMPONENT`].
    component: Value,
    /// Every property the component has, in any order.
    properties: Vec<ExpectedProperty>,
    /// Every property group the component has, with whether it is inherited.
    groups: Vec<(&'static str, bool)>,
    /// The component type the inherited definition set came from, or `None`
    /// where nothing resolved.
    defined_in: Option<&'static str>,
}

/// The one component name every case declares, so every assertion addresses the
/// same component whatever the row put in it.
const COMPONENT: &str = "Spec";

/// Every row of the rule.
fn cases() -> Vec<Case> {
    vec![
        Case {
            what: "inheritsEveryDefinitionOfItsType",
            entity_id: "inherits-all",
            component: json!({"componentTypeId": BASE}),
            properties: vec![("Serial", "STRING", true), ("Tag", "STRING", true)],
            groups: vec![("Readings", true)],
            defined_in: Some(BASE),
        },
        Case {
            what: "inheritsAlongTheExtendsFromChain",
            entity_id: "inherits-chain",
            component: json!({"componentTypeId": DERIVED}),
            properties: vec![
                ("Pressure", "DOUBLE", true),
                ("Serial", "STRING", true),
                ("Tag", "STRING", true),
            ],
            groups: vec![("Readings", true)],
            defined_in: Some(DERIVED),
        },
        Case {
            what: "theEntitysOwnDeclarationWinsOverTheInheritedOne",
            entity_id: "overrides-one",
            component: json!({
                "componentTypeId": BASE,
                "properties": {"Serial": {
                    "definition": {"dataType": {"type": "LONG"}},
                    "value": {"longValue": 7},
                }},
                "propertyGroups": {
                    "Readings": {"groupType": "TABULAR", "propertyNames": ["Serial"]},
                },
            }),
            properties: vec![("Serial", "LONG", true), ("Tag", "STRING", true)],
            groups: vec![("Readings", true)],
            defined_in: Some(BASE),
        },
        Case {
            what: "anEntityLocalPropertyIsReportedBesideTheInheritedOnes",
            entity_id: "declares-extra",
            component: json!({
                "componentTypeId": BASE,
                "properties": {"Extra": {"definition": {"dataType": {"type": "BOOLEAN"}}}},
                "propertyGroups": {
                    "Spares": {"groupType": "TABULAR", "propertyNames": ["Extra"]},
                },
            }),
            properties: vec![
                ("Extra", "BOOLEAN", false),
                ("Serial", "STRING", true),
                ("Tag", "STRING", true),
            ],
            groups: vec![("Readings", true), ("Spares", false)],
            defined_in: Some(BASE),
        },
        Case {
            what: "aResolvedTypeThatDefinesNothingContributesNothing",
            entity_id: "empty-type",
            component: json!({"componentTypeId": EMPTY}),
            properties: vec![],
            groups: vec![],
            defined_in: Some(EMPTY),
        },
        Case {
            what: "anUnresolvedComponentTypeNamesNoDefinitionSet",
            entity_id: "missing-type",
            component: json!({"componentTypeId": MISSING}),
            properties: vec![],
            groups: vec![],
            defined_in: None,
        },
        Case {
            what: "aComponentOfAnUnresolvedTypeKeepsItsOwnDefinitions",
            entity_id: "no-inheritance",
            component: json!({
                "componentTypeId": MISSING,
                "properties": {"Only": {"definition": {"dataType": {"type": "INTEGER"}}}},
            }),
            properties: vec![("Only", "INTEGER", false)],
            groups: vec![],
            defined_in: None,
        },
    ]
}

/// Create the case's entity and answer the component `GetEntity` reports for it.
async fn created_component(emulator: &Emulator, case: &Case) -> Value {
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": case.entity_id,
                "entityName": case.entity_id,
                "components": {COMPONENT: case.component.clone()},
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

/// The `(name, isInherited)` pairs of a reported `propertyGroups` map.
fn reported_groups(component: &Value) -> BTreeMap<String, Option<bool>> {
    component["propertyGroups"]
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(name, group)| (name, group["isInherited"].as_bool()))
        .collect()
}

/// RULE: `GetEntity` reports every property and property group the component
/// effectively has — the entity's own declarations merged over the ones its
/// component type (or a type that type extends) defines — each marked
/// `isInherited` exactly when that chain declares it, and `definedIn` names the
/// component type the inherited definition set came from.
#[tokio::test]
async fn a_component_reports_the_whole_schema_of_the_type_it_instantiates() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let component = created_component(&emulator, &case).await;

        // Exactly these properties, and nothing else: a component HAS the
        // properties its type gives it as much as the ones its entity declared.
        let mut reported: Vec<String> = component["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default()
            .keys()
            .cloned()
            .collect();
        reported.sort();
        let mut wanted: Vec<String> = case
            .properties
            .iter()
            .map(|(name, _, _)| (*name).to_string())
            .collect();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {:?}: expected exactly these properties, got {component:?}",
            case.what
        );

        for (property_name, data_type, is_inherited) in &case.properties {
            let definition = &component["properties"][property_name]["definition"];
            assert_eq!(
                definition["dataType"]["type"].as_str(),
                Some(*data_type),
                "case {:?} property {property_name:?}: wrong dataType, got {definition:?}",
                case.what
            );
            assert_eq!(
                definition["isInherited"].as_bool(),
                Some(*is_inherited),
                "case {:?} property {property_name:?}: expected isInherited {is_inherited}, got \
                 {definition:?}",
                case.what
            );
        }

        assert_eq!(
            reported_groups(&component),
            case.groups
                .iter()
                .map(|(name, is_inherited)| ((*name).to_string(), Some(*is_inherited)))
                .collect::<BTreeMap<String, Option<bool>>>(),
            "case {:?}: expected exactly these property groups, each marked as resolved, got \
             {component:?}",
            case.what
        );

        assert_eq!(
            component["definedIn"].as_str(),
            case.defined_in,
            "case {:?}: wrong definedIn, got {component:?}",
            case.what
        );

        // Nothing was truncated: no row here comes near the documented ceiling,
        // so the flag has to say every property was returned.
        assert_eq!(
            component["areAllPropertiesReturned"].as_bool(),
            Some(true),
            "case {:?}: expected every property to be reported, got {component:?}",
            case.what
        );
    }
}

/// RULE: the members an inherited definition carries are the component TYPE's —
/// the display name, the default value and the flags it declared — so a property
/// the entity never restated still reads with its type's meaning and its type's
/// default value.
#[tokio::test]
async fn an_inherited_definition_carries_the_types_own_members() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let case = Case {
        what: "inheritedMembers",
        entity_id: "inherited-members",
        component: json!({"componentTypeId": DERIVED}),
        properties: vec![],
        groups: vec![],
        defined_in: Some(DERIVED),
    };
    let component = created_component(&emulator, &case).await;

    // The members each definition the chain contributes has to carry:
    // `(property, displayName, isTimeSeries, the value the property reads as)`.
    let rows: Vec<(&str, Option<&str>, bool, Option<Value>)> = vec![
        ("Serial", Some("Serial number"), false, None),
        ("Tag", None, false, Some(json!({"stringValue": "unset"}))),
        ("Pressure", None, true, None),
    ];
    for (property_name, display_name, is_time_series, value) in rows {
        let property = &component["properties"][property_name];
        let definition = &property["definition"];
        assert_eq!(
            definition["displayName"].as_str(),
            display_name,
            "case {property_name:?}: expected the type's displayName, got {definition:?}"
        );
        assert_eq!(
            definition["isTimeSeries"].as_bool(),
            Some(is_time_series),
            "case {property_name:?}: expected the type's isTimeSeries, got {definition:?}"
        );
        // A property nothing has written to reads as its definition's default,
        // which for an inherited definition is the component TYPE's default.
        assert_eq!(
            property.get("value").cloned(),
            value,
            "case {property_name:?}: expected the value the definition gives it, got {property:?}"
        );
    }

    // The group the type declares is reported with the type's own contents.
    let group = &component["propertyGroups"]["Readings"];
    assert_eq!(
        (
            group["groupType"].as_str(),
            group["propertyNames"].as_array().cloned().unwrap_or_default(),
        ),
        (Some("TABULAR"), vec![json!("Serial"), json!("Tag")]),
        "expected the type's own group contents, got {group:?}"
    );
}

/// RULE (one component, one answer): `ListComponents` and `ListProperties`
/// resolve a component's schema the same way `GetEntity` does, so the three
/// readers cannot disagree about what a component has or about what is
/// inherited.
#[tokio::test]
async fn every_reader_of_a_component_resolves_its_schema_alike() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in cases() {
        let component = created_component(&emulator, &case).await;

        let listed = emulator
            .list_components(WORKSPACE, case.entity_id, json!({}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListComponents, got {listed:?}",
            case.what
        );
        let summary = summaries(&listed)
            .into_iter()
            .find(|summary| summary["componentName"] == COMPONENT)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the {COMPONENT} summary, got {listed:?}",
                    case.what
                )
            });
        assert_eq!(
            summary["definedIn"].as_str(),
            component["definedIn"].as_str(),
            "case {:?}: expected ListComponents and GetEntity to name one definition set, got \
             {summary:?}",
            case.what
        );
        assert_eq!(
            reported_groups(&summary),
            reported_groups(&component),
            "case {:?}: expected ListComponents and GetEntity to report one group set, got \
             {summary:?}",
            case.what
        );

        let rows = emulator
            .list_properties(
                WORKSPACE,
                json!({"entityId": case.entity_id, "componentName": COMPONENT}),
            )
            .await;
        assert_eq!(
            rows.status, 200,
            "case {:?}: expected 200 from ListProperties, got {rows:?}",
            case.what
        );
        for (property_name, _, _) in &case.properties {
            let row = rows
                .member("propertySummaries")
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .find(|row| row["propertyName"] == *property_name)
                .unwrap_or_else(|| {
                    panic!(
                        "case {:?}: expected a row for {property_name:?}, got {rows:?}",
                        case.what
                    )
                });
            assert_eq!(
                row["definition"], component["properties"][property_name]["definition"],
                "case {:?} property {property_name:?}: expected ListProperties and GetEntity to \
                 report one definition, got {row:?}",
                case.what
            );
        }
    }
}

/// The `componentSummaries` of a `ListComponents` answer.
fn summaries(listed: &Resp) -> Vec<Value> {
    listed
        .member("componentSummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
}
