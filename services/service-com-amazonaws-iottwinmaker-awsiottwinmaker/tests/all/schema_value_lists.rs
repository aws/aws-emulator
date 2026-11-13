// SPDX-License-Identifier: CC0-1.0

//! What a writer of a component's or a component type's SCHEMA accepts in the
//! two members that target a modelled enumeration, and in a property group's
//! `propertyNames`.
//!
//! RULE under test, in two parts:
//!
//! 1. a request member whose model target is an ENUMERATION carries one of the
//!    values that enumeration defines, and a value outside the list is refused
//!    with a `ValidationException` before anything is stored. The two members a
//!    schema carries are `PropertyDefinitionRequest.dataType.type` (target
//!    `Type`, and `@required` on the `DataType` a read reports) and a property
//!    group's `groupType` (target `GroupType`, `@required` on the group
//!    response), the second being optional on the request, where an absent one
//!    reads back as the only value the model admits.
//! 2. a property GROUP groups properties of the schema it is declared on, so
//!    each name in its `propertyNames` is one that schema HAS: for a component,
//!    one of its own properties or one the component type it instantiates — or
//!    a type that type extends — defines.
//!
//! Both parts hold for every writer of those members, so the rows below drive
//! them through `CreateEntity`, `UpdateEntity`, `CreateComponentType` and
//! `UpdateComponentType`, at the top level of an entity and inside a composite
//! component.
//!
//! The value lists are the model's, read from the crate's generated catalogs
//! (`ops::types::TYPE_VALUES`, `ops::types::GROUP_TYPE_VALUES`), so a value the
//! model adds is a row here without this suite changing:
//! smithy_query(query="shape", shape="Type,GroupType,DataType,PropertyGroupRequest,ComponentPropertyGroupRequest")
//!
//! The rows differ along what the rule generalises over: WHICH writer carries
//! the member, WHERE in the request it sits (a component, a composite
//! component, a component type), and HOW the supplied value misses the list
//! (unknown word, wrong case, empty, padded, a nested `dataType`'s own type) —
//! and, for the names, where a name the group carries could come from (its own
//! property, the component type's, an inherited one, one the same request
//! declares, none at all).

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::ops::types::{
    GROUP_TYPE_VALUES, TYPE_VALUES,
};

use crate::harness::Emulator;

/// The workspace every case below is created in.
const WORKSPACE: &str = "schemavalues";

/// The component type the cases instantiate: it defines `heat` and `label`, and
/// declares one composite component type.
const SENSOR: &str = "com.example.sensor";
/// A component type that EXTENDS [`SENSOR`], so an instance of it has `heat`
/// and `label` without either being declared on it.
const DERIVED: &str = "com.example.derived";
/// The component type [`SENSOR`] declares as a composite one, which defines
/// `depth`.
const INNER: &str = "com.example.inner";
/// A component type nothing below instantiates, so a property only IT defines
/// is a property the cases' components do not have.
const OTHER: &str = "com.example.other";
/// An id in the namespace the service reserves for the component types it
/// SHIPS, and one this emulator does not model: a component naming it is
/// accepted, and its schema is not one this emulator can enumerate.
const UNMODELLED_BUILT_IN: &str = "com.amazon.iotsitewise.unmodelled";

/// The one component name the cases declare.
const COMPONENT: &str = "C";
/// The composite component path the cases address, through [`SENSOR`]'s own
/// `compositeComponentTypes` declaration.
const COMPOSITE_PATH: &str = "C/inner";
/// The group name the cases declare.
const GROUP: &str = "G";

/// A `dataType` for one value of the model's `Type` enum: `LIST` and `MAP` take
/// a `nestedType` and `RELATIONSHIP` a `relationship`, which is what makes each
/// of those a complete data type; the scalar values need neither.
fn data_type(type_value: &str) -> Value {
    match type_value {
        "LIST" | "MAP" => json!({"type": type_value, "nestedType": {"type": "STRING"}}),
        "RELATIONSHIP" => json!({
            "type": type_value,
            "relationship": {"targetComponentTypeId": SENSOR, "relationshipType": "belongsTo"},
        }),
        _ => json!({"type": type_value}),
    }
}

/// Create the workspace and the component types the cases instantiate.
async fn seed(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    // In reference order: a declared id has to name a component type the
    // workspace already holds.
    let component_types: Vec<(&str, Value)> = vec![
        (
            INNER,
            json!({"propertyDefinitions": {"depth": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            OTHER,
            json!({"propertyDefinitions": {"foreign": {"dataType": {"type": "STRING"}}}}),
        ),
        (
            SENSOR,
            json!({
                "propertyDefinitions": {
                    "heat": {"dataType": {"type": "DOUBLE"}},
                    "label": {"dataType": {"type": "STRING"}},
                },
                "compositeComponentTypes": {"inner": {"componentTypeId": INNER}},
            }),
        ),
        (DERIVED, json!({"extendsFrom": [SENSOR]})),
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

/// The values a `dataType.type` can miss the model's `Type` list by, each with
/// the `dataType` that carries it.
///
/// Every row differs in HOW it misses: an unknown word, a real value in the
/// wrong case, the empty string, a padded value, a list of values, and a NESTED
/// `dataType` whose own type misses the list (`nestedType` targets `DataType`
/// itself, so the rule reaches down it).
fn data_types_outside_the_list() -> Vec<(&'static str, Value)> {
    vec![
        ("unknownWord", json!({"type": "FOO"})),
        ("wrongCase", json!({"type": "string"})),
        ("empty", json!({"type": ""})),
        ("padded", json!({"type": "STRING "})),
        ("severalValues", json!({"type": "DOUBLE,INTEGER"})),
        (
            "nestedTypeOutsideTheList",
            json!({"type": "LIST", "nestedType": {"type": "NOPE"}}),
        ),
        (
            "twiceNestedTypeOutsideTheList",
            json!({"type": "LIST", "nestedType": {"type": "LIST", "nestedType": {"type": "NOPE"}}}),
        ),
    ]
}

/// The `type` a row's bad `dataType` carries, at whatever depth it sits: the
/// value the refusal has to name.
fn bad_type(data_type: &Value) -> &str {
    let mut data_type = data_type;
    while let Some(nested) = data_type.get("nestedType") {
        data_type = nested;
    }
    data_type["type"].as_str().unwrap_or_default()
}

/// The values a `groupType` can miss the model's `GroupType` list by.
fn group_types_outside_the_list() -> Vec<(&'static str, &'static str)> {
    vec![
        ("unknownWord", "NOT_A_GROUP_TYPE"),
        ("wrongCase", "tabular"),
        ("empty", ""),
        ("padded", "TABULAR "),
    ]
}

/// RULE 1: every value the model's `Type` enum defines is accepted on a
/// COMPONENT's property definition and read back verbatim, nested types
/// included — the catalog is the model's own, so a value it adds is a row here
/// without this suite changing.
#[tokio::test]
async fn every_modelled_data_type_is_accepted_on_a_component_property() {
    let emulator = Emulator::new();
    seed(&emulator).await;
    assert!(
        TYPE_VALUES.len() >= 8,
        "the model's Type enum must still carry its values, got {TYPE_VALUES:?}"
    );

    let mut properties = serde_json::Map::new();
    for type_value in TYPE_VALUES {
        properties.insert(
            format!("prop_{type_value}"),
            json!({"definition": {"dataType": data_type(type_value)}}),
        );
    }
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "every-type",
                "entityName": "every-type",
                "components": {COMPONENT: {
                    "componentTypeId": SENSOR,
                    "properties": Value::Object(properties),
                }},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case \"everyModelledType\": expected 200 from CreateEntity, got {created:?}"
    );

    let read = emulator.get_entity(WORKSPACE, "every-type").await;
    assert_eq!(
        read.status, 200,
        "case \"everyModelledType\": expected 200 from GetEntity, got {read:?}"
    );
    for type_value in TYPE_VALUES {
        let reported = &read.member("components")[COMPONENT]["properties"]
            [format!("prop_{type_value}")]["definition"]["dataType"];
        assert_eq!(
            reported, &data_type(type_value),
            "case {type_value:?}: expected the created dataType back, got {reported:?}"
        );
    }
}

/// RULE 1: a `dataType.type` outside the model's `Type` value list is refused
/// by every writer of a property definition — an entity's component, a
/// composite component, and a component type — and nothing is stored.
#[tokio::test]
async fn a_data_type_outside_the_modelled_list_is_refused_by_every_writer() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    // The entity the UPDATE cases patch, created with a definition the rule
    // accepts so each refusal below is the one the row is about.
    let baseline = json!({
        "entityId": "patched",
        "entityName": "patched",
        "components": {COMPONENT: {
            "componentTypeId": SENSOR,
            "properties": {"own": {"definition": {"dataType": {"type": "STRING"}}}},
        }},
        "compositeComponents": {COMPOSITE_PATH: {}},
    });
    let created = emulator.create_entity(WORKSPACE, baseline).await;
    assert_eq!(
        created.status, 200,
        "the entity the update cases patch must be created, got {created:?}"
    );

    for (what, bad) in data_types_outside_the_list() {
        let property = json!({"definition": {"dataType": bad.clone()}});

        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": "created",
                    "entityName": "created",
                    "components": {COMPONENT: {
                        "componentTypeId": SENSOR,
                        "properties": {"p": property.clone()},
                    }},
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected CreateEntity to refuse {bad}, got {refused:?}"
        );
        // The refusal names the member the caller has to fix and the value it
        // carried, whether the boundary's own value-list check or this crate's
        // rule for the writers no boundary stands in front of answers it.
        let message = refused.member("message").as_str().unwrap_or_default();
        assert!(
            message.contains("dataType") && message.contains(bad_type(&bad)),
            "case {what:?}: expected the refusal to name the member and the value, got \
             {message:?}"
        );
        let absent = emulator.get_entity(WORKSPACE, "created").await;
        assert_eq!(
            absent.status, 404,
            "case {what:?}: a refused create stores no entity, got {absent:?}"
        );

        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": "created-composite",
                    "entityName": "created-composite",
                    "components": {COMPONENT: {"componentTypeId": SENSOR}},
                    "compositeComponents": {COMPOSITE_PATH: {"properties": {"p": property.clone()}}},
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected CreateEntity to refuse {bad} on a composite component, got \
             {refused:?}"
        );

        let refused = emulator
            .update_entity(
                WORKSPACE,
                "patched",
                json!({"componentUpdates": {COMPONENT: {
                    "propertyUpdates": {"own": property.clone()},
                }}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected UpdateEntity to refuse {bad}, got {refused:?}"
        );
        let unchanged = emulator.get_entity(WORKSPACE, "patched").await;
        assert_eq!(
            unchanged.member("components")[COMPONENT]["properties"]["own"]["definition"]
                ["dataType"],
            json!({"type": "STRING"}),
            "case {what:?}: a refused patch leaves the stored definition alone, got {unchanged:?}"
        );

        let refused = emulator
            .update_entity(
                WORKSPACE,
                "patched",
                json!({"compositeComponentUpdates": {COMPOSITE_PATH: {
                    "propertyUpdates": {"p": property.clone()},
                }}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected UpdateEntity to refuse {bad} on a composite component, got \
             {refused:?}"
        );

        let refused = emulator
            .create_component_type(
                WORKSPACE,
                "com.example.created",
                json!({"propertyDefinitions": {"p": {"dataType": bad.clone()}}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected CreateComponentType to refuse {bad}, got {refused:?}"
        );
        let absent = emulator
            .get_component_type(WORKSPACE, "com.example.created")
            .await;
        assert_eq!(
            absent.status, 404,
            "case {what:?}: a refused create stores no component type, got {absent:?}"
        );

        let refused = emulator
            .update_component_type(
                WORKSPACE,
                OTHER,
                json!({"propertyDefinitions": {"foreign": {"dataType": bad.clone()}}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected UpdateComponentType to refuse {bad}, got {refused:?}"
        );
        let unchanged = emulator.get_component_type(WORKSPACE, OTHER).await;
        assert_eq!(
            unchanged.member("propertyDefinitions")["foreign"]["dataType"],
            json!({"type": "STRING"}),
            "case {what:?}: a refused patch leaves the stored definition alone, got {unchanged:?}"
        );
    }
}

/// RULE 1: a `groupType` is accepted for every value the model's `GroupType`
/// enum defines and for none outside it, and a group that names none reads back
/// as the only value the model admits — on a component, a composite component
/// and a component type alike.
#[tokio::test]
async fn a_group_type_outside_the_modelled_list_is_refused_by_every_writer() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "grouped",
                "entityName": "grouped",
                "components": {COMPONENT: {
                    "componentTypeId": SENSOR,
                    "propertyGroups": {GROUP: {"propertyNames": ["heat"]}},
                }},
                "compositeComponents": {COMPOSITE_PATH: {}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "a group naming no groupType is accepted, got {created:?}"
    );
    let read = emulator.get_entity(WORKSPACE, "grouped").await;
    assert_eq!(
        read.member("components")[COMPONENT]["propertyGroups"][GROUP]["groupType"].as_str(),
        GROUP_TYPE_VALUES.first().copied(),
        "a group stored with no groupType reads back as the only value the model admits, got \
         {read:?}"
    );

    // The accepted side: every value the model publishes, which is one row
    // today and every row the model grows to.
    for group_type in GROUP_TYPE_VALUES {
        let entity_id = format!("group-{group_type}");
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {COMPONENT: {
                        "componentTypeId": SENSOR,
                        "propertyGroups": {GROUP: {"groupType": group_type, "propertyNames": ["heat"]}},
                    }},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {group_type:?}: expected a modelled groupType to be accepted, got {created:?}"
        );
        let read = emulator.get_entity(WORKSPACE, &entity_id).await;
        assert_eq!(
            read.member("components")[COMPONENT]["propertyGroups"][GROUP]["groupType"].as_str(),
            Some(*group_type),
            "case {group_type:?}: expected the created groupType back, got {read:?}"
        );
    }

    for (what, bad) in group_types_outside_the_list() {
        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": "group-created",
                    "entityName": "group-created",
                    "components": {COMPONENT: {
                        "componentTypeId": SENSOR,
                        "propertyGroups": {GROUP: {"groupType": bad, "propertyNames": ["heat"]}},
                    }},
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected CreateEntity to refuse groupType {bad:?}, got {refused:?}"
        );
        let message = refused.member("message").as_str().unwrap_or_default();
        assert!(
            message.contains("groupType") && message.contains(bad),
            "case {what:?}: expected the refusal to name the member and the value, got \
             {message:?}"
        );
        let absent = emulator.get_entity(WORKSPACE, "group-created").await;
        assert_eq!(
            absent.status, 404,
            "case {what:?}: a refused create stores no entity, got {absent:?}"
        );

        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": "group-created-composite",
                    "entityName": "group-created-composite",
                    "components": {COMPONENT: {"componentTypeId": SENSOR}},
                    "compositeComponents": {COMPOSITE_PATH: {
                        "propertyGroups": {GROUP: {"groupType": bad, "propertyNames": ["depth"]}},
                    }},
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected CreateEntity to refuse groupType {bad:?} on a composite \
             component, got {refused:?}"
        );

        let refused = emulator
            .update_entity(
                WORKSPACE,
                "grouped",
                json!({"componentUpdates": {COMPONENT: {
                    "propertyGroupUpdates": {GROUP: {"groupType": bad}},
                }}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected UpdateEntity to refuse groupType {bad:?}, got {refused:?}"
        );
        let unchanged = emulator.get_entity(WORKSPACE, "grouped").await;
        assert_eq!(
            unchanged.member("components")[COMPONENT]["propertyGroups"][GROUP]["groupType"]
                .as_str(),
            GROUP_TYPE_VALUES.first().copied(),
            "case {what:?}: a refused patch leaves the stored group alone, got {unchanged:?}"
        );

        let refused = emulator
            .update_entity(
                WORKSPACE,
                "grouped",
                json!({"compositeComponentUpdates": {COMPOSITE_PATH: {
                    "propertyGroupUpdates": {GROUP: {"groupType": bad, "propertyNames": ["depth"]}},
                }}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected UpdateEntity to refuse groupType {bad:?} on a composite \
             component, got {refused:?}"
        );

        let refused = emulator
            .create_component_type(
                WORKSPACE,
                "com.example.grouped",
                json!({
                    "propertyDefinitions": {"p": {"dataType": {"type": "STRING"}}},
                    "propertyGroups": {GROUP: {"groupType": bad, "propertyNames": ["p"]}},
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected CreateComponentType to refuse groupType {bad:?}, got \
             {refused:?}"
        );

        let refused = emulator
            .update_component_type(
                WORKSPACE,
                OTHER,
                json!({"propertyGroups": {GROUP: {"groupType": bad, "propertyNames": ["foreign"]}}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected UpdateComponentType to refuse groupType {bad:?}, got \
             {refused:?}"
        );
    }
}

/// One row of rule 2: the component a create carries, and whether the group it
/// declares names properties the component has.
struct NameCase {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The single component the create declares, under [`COMPONENT`].
    component: Value,
    /// Whether the create is accepted.
    accepted: bool,
}

/// Every row of the group-membership rule, differing along where a name the
/// group carries could come from.
fn name_cases() -> Vec<NameCase> {
    vec![
        NameCase {
            what: "namesAPropertyTheComponentDeclaresItself",
            entity_id: "names-own",
            component: json!({
                "componentTypeId": SENSOR,
                "properties": {"own": {"definition": {"dataType": {"type": "STRING"}}}},
                "propertyGroups": {GROUP: {"propertyNames": ["own"]}},
            }),
            accepted: true,
        },
        NameCase {
            what: "namesAPropertyTheComponentTypeDefines",
            entity_id: "names-type",
            component: json!({
                "componentTypeId": SENSOR,
                "propertyGroups": {GROUP: {"propertyNames": ["heat", "label"]}},
            }),
            accepted: true,
        },
        NameCase {
            what: "namesAPropertyInheritedThroughExtendsFrom",
            entity_id: "names-inherited",
            component: json!({
                "componentTypeId": DERIVED,
                "propertyGroups": {GROUP: {"propertyNames": ["heat"]}},
            }),
            accepted: true,
        },
        NameCase {
            what: "namesNothingAtAll",
            entity_id: "names-none",
            component: json!({
                "componentTypeId": SENSOR,
                "propertyGroups": {GROUP: {"groupType": "TABULAR"}},
            }),
            accepted: true,
        },
        NameCase {
            what: "underAComponentTypeThisEmulatorDoesNotModel",
            entity_id: "names-built-in",
            component: json!({
                "componentTypeId": UNMODELLED_BUILT_IN,
                "propertyGroups": {GROUP: {"propertyNames": ["whateverThatTypeDefines"]}},
            }),
            accepted: true,
        },
        NameCase {
            what: "namesAPropertyNothingDefines",
            entity_id: "names-unknown",
            component: json!({
                "componentTypeId": SENSOR,
                "propertyGroups": {GROUP: {"propertyNames": ["notAProperty"]}},
            }),
            accepted: false,
        },
        NameCase {
            what: "namesAPropertyOnlyAnotherComponentTypeDefines",
            entity_id: "names-foreign",
            component: json!({
                "componentTypeId": SENSOR,
                "propertyGroups": {GROUP: {"propertyNames": ["foreign"]}},
            }),
            accepted: false,
        },
        NameCase {
            what: "theLastNameOfTheListIsUndefined",
            entity_id: "names-last",
            component: json!({
                "componentTypeId": SENSOR,
                "propertyGroups": {GROUP: {"propertyNames": ["heat", "notAProperty"]}},
            }),
            accepted: false,
        },
        NameCase {
            // A name only the component's COMPOSITE component type defines: a
            // composite component is a component of its own rather than part of
            // the holding component's schema, so the name is not one this
            // component has.
            what: "namesAPropertyOnlyACompositeComponentTypeDefines",
            entity_id: "names-composite",
            component: json!({
                "componentTypeId": SENSOR,
                "propertyGroups": {GROUP: {"propertyNames": ["depth"]}},
            }),
            accepted: false,
        },
    ]
}

/// RULE 2: a component's property group names only properties the component
/// has, so what `CreateEntity` accepts is exactly what `ListProperties` on the
/// same component lists.
#[tokio::test]
async fn a_component_group_names_only_properties_the_component_has() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in name_cases() {
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
        if !case.accepted {
            assert_eq!(
                (created.status, created.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {:?}: expected CreateEntity to refuse the group, got {created:?}",
                case.what
            );
            let message = created.member("message").as_str().unwrap_or_default();
            assert!(
                message.contains("components.C.propertyGroups.G.propertyNames"),
                "case {:?}: expected the refusal to name the member that has to be fixed, got \
                 {message:?}",
                case.what
            );
            let absent = emulator.get_entity(WORKSPACE, case.entity_id).await;
            assert_eq!(
                absent.status, 404,
                "case {:?}: a refused create stores no entity, got {absent:?}",
                case.what
            );
            continue;
        }
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
        let reported: Vec<String> = read.member("components")[COMPONENT]["propertyGroups"][GROUP]
            ["propertyNames"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|name| name.as_str().unwrap_or_default().to_string())
            .collect();

        // The consequence the rule exists for: a name the group reports is a
        // property the component's own listing carries — except where the
        // component's schema is one this emulator cannot enumerate, which is
        // exactly the case the rule stands down on.
        if case.entity_id == "names-built-in" {
            continue;
        }
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
        let listed_names: Vec<String> = listed
            .member("propertySummaries")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|summary| {
                summary["propertyName"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        for name in &reported {
            assert!(
                listed_names.contains(name),
                "case {:?}: the group names {name:?}, which ListProperties does not list: got \
                 {listed_names:?}",
                case.what
            );
        }
    }
}

/// RULE 2 through the other writer: a `propertyGroupUpdates` entry is held to
/// the same rule, against the properties the component holds AFTER the same
/// request's `propertyUpdates` — so a group may name a property the same call
/// declares, and may not name one nothing does.
#[tokio::test]
async fn an_updated_component_group_names_only_properties_the_component_has() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": "patched",
                "entityName": "patched",
                "components": {COMPONENT: {
                    "componentTypeId": SENSOR,
                    "propertyGroups": {GROUP: {"groupType": "TABULAR", "propertyNames": ["heat"]}},
                }},
                "compositeComponents": {COMPOSITE_PATH: {}},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "the entity the cases patch must be created, got {created:?}"
    );

    // (what, the patch, whether it is accepted)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "patchNamesAPropertyTheComponentTypeDefines",
            json!({"componentUpdates": {COMPONENT: {
                "propertyGroupUpdates": {GROUP: {"propertyNames": ["label"]}},
            }}}),
            true,
        ),
        (
            "patchNamesAPropertyTheSameRequestDeclares",
            json!({"componentUpdates": {COMPONENT: {
                "propertyUpdates": {"fresh": {"definition": {"dataType": {"type": "STRING"}}}},
                "propertyGroupUpdates": {"H": {
                    "updateType": "CREATE",
                    "propertyNames": ["fresh"],
                }},
            }}}),
            true,
        ),
        (
            "patchNamesAPropertyNothingDefines",
            json!({"componentUpdates": {COMPONENT: {
                "propertyGroupUpdates": {GROUP: {"propertyNames": ["notAProperty"]}},
            }}}),
            false,
        ),
        (
            "aCreatedGroupNamesAPropertyNothingDefines",
            json!({"componentUpdates": {COMPONENT: {
                "propertyGroupUpdates": {"I": {
                    "updateType": "CREATE",
                    "propertyNames": ["notAProperty"],
                }},
            }}}),
            false,
        ),
        (
            "aCompositeComponentsGroupNamesAPropertyNothingDefines",
            json!({"compositeComponentUpdates": {COMPOSITE_PATH: {
                "propertyGroupUpdates": {GROUP: {
                    "updateType": "CREATE",
                    "propertyNames": ["notAProperty"],
                }},
            }}}),
            false,
        ),
        (
            "aCompositeComponentsGroupNamesItsTypesProperty",
            json!({"compositeComponentUpdates": {COMPOSITE_PATH: {
                "propertyGroupUpdates": {GROUP: {
                    "updateType": "CREATE",
                    "propertyNames": ["depth"],
                }},
            }}}),
            true,
        ),
    ];

    for (what, patch, accepted) in rows {
        let patched = emulator
            .update_entity(WORKSPACE, "patched", patch.clone())
            .await;
        if accepted {
            assert_eq!(
                patched.status, 200,
                "case {what:?}: expected 200 from UpdateEntity, got {patched:?}"
            );
            continue;
        }
        assert_eq!(
            (patched.status, patched.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {what:?}: expected UpdateEntity to refuse the group, got {patched:?}"
        );
        // A refused patch changes nothing: the group the entity holds is still
        // the one the create stored.
        let read = emulator.get_entity(WORKSPACE, "patched").await;
        assert_eq!(
            read.member("components")[COMPONENT]["propertyGroups"][GROUP]["propertyNames"],
            json!(["label"]),
            "case {what:?}: a refused patch leaves the stored group alone, got {read:?}"
        );
    }
}
