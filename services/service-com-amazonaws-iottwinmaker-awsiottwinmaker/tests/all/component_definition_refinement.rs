// SPDX-License-Identifier: CC0-1.0

//! A component's property definition that REFINES an inherited one, end to end
//! over HTTP.
//!
//! RULE under test: a writer of a component's property definition refuses it for
//! want of a `dataType` only where NO declaration reaching that property
//! supplies one — a definition that omits `dataType` where the component type
//! the holder instantiates, or a type that type extends, declares it is accepted
//! and read back with the inherited `dataType` plus the members the refinement
//! changed. Both writers of a component's definitions (`CreateEntity` and
//! `UpdateEntity`) hold one definition to that one rule, for a component and for
//! a composite component alike.
//!
//! Member optionality is the model's:
//! smithy_query(query="shape", shape="PropertyDefinitionRequest,PropertyDefinitionResponse")
//! — every member of the REQUEST is optional while `dataType` is required on the
//! RESPONSE, which is both why a declaration may restate only the members it
//! changes and why a definition nothing supplies a `dataType` for is refused.
//!
//! The accepted rows differ along what the rule generalises over: WHICH member
//! the refinement restates (a flag, two flags, a display name, a default, no
//! member at all), HOW FAR away the declaring component type is (the type the
//! component instantiates, a type that type extends), the `dataType` the
//! declaration being refined carries, WHICH kind of holder carries the
//! refinement (a component, a composite component) and WHICH writer stores it (a
//! create, an update). The refused rows differ along WHY nothing supplies a
//! `dataType`: the component instantiates no type, it instantiates one the
//! workspace does not hold, the property is one no type on the chain declares,
//! and the property is one only a type OFF the chain declares.

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below works in.
const WORKSPACE: &str = "definitionRefinement";

/// The component type at the root of the chain: it declares every property the
/// cases refine, each carrying a `dataType` and some further member a refinement
/// has to keep.
const BASE: &str = "com.example.base";
/// A component type that extends [`BASE`] and declares one property of its own,
/// so a refinement can reach a declaration either one hop or two hops away.
const DERIVED: &str = "com.example.derived";
/// A component type whose composite component type is a [`BASE`], so a COMPOSITE
/// component can carry the same refinement a component does.
const HOLDER: &str = "com.example.holder";
/// A component type id the workspace does not hold, so a component naming it
/// reaches no declaration at all.
const ABSENT: &str = "com.example.absent";

/// The component name every case declares, so every assertion addresses the same
/// component whatever the row put in it.
const COMPONENT: &str = "Spec";
/// The composite component [`HOLDER`] declares, under [`COMPONENT`].
const COMPOSITE: &str = "inner";

/// The `displayName` [`BASE`] declares for `Serial`, which a refinement that
/// leaves it alone has to keep.
const INHERITED_DISPLAY_NAME: &str = "Serial (base)";

/// Create the workspace and the component types the cases instantiate.
///
/// The `dataType.type` values spread over the modelled `Type` enum
/// (smithy_query(query="shape", shape="Type") lists `RELATIONSHIP`, `STRING`,
/// `LONG`, `BOOLEAN`, `INTEGER`, `DOUBLE`, `LIST` and `MAP`); the five scalar
/// ones are declared here, since the other three need a `relationship` or a
/// `nestedType` beside the `type` and the rule under test does not read either.
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
            BASE,
            json!({
                "propertyDefinitions": {
                    "Serial": {
                        "dataType": {"type": "STRING"},
                        "displayName": INHERITED_DISPLAY_NAME,
                    },
                    "Reading": {
                        "dataType": {"type": "DOUBLE"},
                        "defaultValue": {"doubleValue": 1.5},
                    },
                    "Count": {"dataType": {"type": "LONG"}},
                    "Flag": {"dataType": {"type": "BOOLEAN"}},
                },
            }),
        ),
        (
            DERIVED,
            json!({
                "extendsFrom": [BASE],
                "propertyDefinitions": {"Mode": {"dataType": {"type": "INTEGER"}}},
            }),
        ),
        (
            HOLDER,
            json!({"compositeComponentTypes": {COMPOSITE: {"componentTypeId": BASE}}}),
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

/// One accepted row: the refinement a component carries for a property, and the
/// definition a read has to report for it.
#[derive(Debug)]
struct Accepted {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The component type the component instantiates.
    component_type_id: &'static str,
    /// The property the row refines.
    property: &'static str,
    /// The definition the create sends: never carrying a `dataType` of its own,
    /// which is the whole point of the rule.
    refinement: Value,
    /// The `dataType.type` the read must report, inherited from the chain.
    data_type: &'static str,
    /// The `displayName` the read must report, refined or inherited.
    display_name: Option<&'static str>,
    /// The `isRequiredInEntity` the read must report.
    is_required_in_entity: bool,
    /// The `isTimeSeries` the read must report.
    is_time_series: bool,
    /// The `defaultValue` the read must report, refined or inherited.
    default_value: Option<Value>,
}

/// Every accepted row of the rule.
fn accepted() -> Vec<Accepted> {
    vec![
        Accepted {
            // The shape the issue reproduces: one flag restated over the
            // declaration the component's own type makes.
            what: "refinesOneFlagOnTheTypesOwnDeclaration",
            entity_id: "refines-one-flag",
            component_type_id: BASE,
            property: "Serial",
            refinement: json!({"isRequiredInEntity": true}),
            data_type: "STRING",
            display_name: Some(INHERITED_DISPLAY_NAME),
            is_required_in_entity: true,
            is_time_series: false,
            default_value: None,
        },
        Accepted {
            what: "refinesTwoFlagsAtOnce",
            entity_id: "refines-two-flags",
            component_type_id: BASE,
            property: "Flag",
            refinement: json!({"isRequiredInEntity": true, "isTimeSeries": true}),
            data_type: "BOOLEAN",
            display_name: None,
            is_required_in_entity: true,
            is_time_series: true,
            default_value: None,
        },
        Accepted {
            what: "refinesTheDisplayNameAndKeepsTheInheritedDefault",
            entity_id: "refines-display-name",
            component_type_id: BASE,
            property: "Reading",
            refinement: json!({"displayName": "Reading (psi)"}),
            data_type: "DOUBLE",
            display_name: Some("Reading (psi)"),
            is_required_in_entity: false,
            is_time_series: false,
            default_value: Some(json!({"doubleValue": 1.5})),
        },
        Accepted {
            what: "refinesTheDefaultValue",
            entity_id: "refines-default",
            component_type_id: BASE,
            property: "Count",
            refinement: json!({"defaultValue": {"longValue": 7}}),
            data_type: "LONG",
            display_name: None,
            is_required_in_entity: false,
            is_time_series: false,
            default_value: Some(json!({"longValue": 7})),
        },
        Accepted {
            // A declaration that restates NOTHING is still a declaration, and
            // every member it leaves alone is the inherited one.
            what: "restatesNoMemberAtAll",
            entity_id: "restates-nothing",
            component_type_id: BASE,
            property: "Serial",
            refinement: json!({}),
            data_type: "STRING",
            display_name: Some(INHERITED_DISPLAY_NAME),
            is_required_in_entity: false,
            is_time_series: false,
            default_value: None,
        },
        Accepted {
            // The declaration being refined is TWO hops away: the component's
            // type declares nothing for the property and the type it extends
            // does.
            what: "inheritsThroughTheTypeItsTypeExtends",
            entity_id: "inherits-two-hops",
            component_type_id: DERIVED,
            property: "Serial",
            refinement: json!({"isRequiredInEntity": true}),
            data_type: "STRING",
            display_name: Some(INHERITED_DISPLAY_NAME),
            is_required_in_entity: true,
            is_time_series: false,
            default_value: None,
        },
        Accepted {
            what: "refinesTheNearerTypesOwnDeclaration",
            entity_id: "refines-nearer-type",
            component_type_id: DERIVED,
            property: "Mode",
            refinement: json!({"displayName": "Mode (tuned)"}),
            data_type: "INTEGER",
            display_name: Some("Mode (tuned)"),
            is_required_in_entity: false,
            is_time_series: false,
            default_value: None,
        },
    ]
}

/// The definition a read reports, as the tuple a row compares against.
type Reported<'a> = (
    Option<&'a str>,
    Option<&'a str>,
    Option<bool>,
    Option<bool>,
    Option<Value>,
);

/// The reported definition as that tuple.
fn reported(definition: &Value) -> Reported<'_> {
    (
        definition["dataType"]["type"].as_str(),
        definition["displayName"].as_str(),
        definition["isRequiredInEntity"].as_bool(),
        definition["isTimeSeries"].as_bool(),
        definition.get("defaultValue").cloned(),
    )
}

/// The tuple the accepted row expects.
fn wanted(case: &Accepted) -> Reported<'static> {
    (
        Some(case.data_type),
        case.display_name,
        Some(case.is_required_in_entity),
        Some(case.is_time_series),
        case.default_value.clone(),
    )
}

/// RULE (accepted half, `CreateEntity`): a component declaration that omits
/// `dataType` where the component's inheritance chain declares one is created
/// and read back carrying the inherited `dataType` and the members it refined.
#[tokio::test]
async fn a_create_accepts_a_definition_that_inherits_its_data_type() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in accepted() {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": case.entity_id,
                    "entityName": case.entity_id,
                    "components": {COMPONENT: {
                        "componentTypeId": case.component_type_id,
                        "properties": {
                            case.property: {"definition": case.refinement.clone()},
                        },
                    }},
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
        let definition =
            &read.member("components")[COMPONENT]["properties"][case.property]["definition"];
        assert_eq!(
            reported(definition),
            wanted(&case),
            "case {:?}: expected the folded definition read back, got {definition:?}",
            case.what
        );

        // The definition reaches the component through the type it
        // instantiates, so a read reports it as inherited however little the
        // component restated.
        assert_eq!(
            definition["isInherited"].as_bool(),
            Some(true),
            "case {:?}: expected the refined definition reported as inherited, got {definition:?}",
            case.what
        );
    }
}

/// RULE (accepted half, `UpdateEntity`): the SAME refinement is accepted by the
/// other writer of a component's definitions, and read back the same way.
#[tokio::test]
async fn an_update_accepts_a_definition_that_inherits_its_data_type() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in accepted() {
        let entity_id = format!("patched-{}", case.entity_id);
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {COMPONENT: {"componentTypeId": case.component_type_id}},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateEntity, got {created:?}",
            case.what
        );

        let patched = emulator
            .update_entity(
                WORKSPACE,
                &entity_id,
                json!({"componentUpdates": {COMPONENT: {"propertyUpdates": {
                    case.property: {"definition": case.refinement.clone()},
                }}}}),
            )
            .await;
        assert_eq!(
            patched.status, 200,
            "case {:?}: expected 200 from UpdateEntity, got {patched:?}",
            case.what
        );

        let read = emulator.get_entity(WORKSPACE, &entity_id).await;
        let definition =
            &read.member("components")[COMPONENT]["properties"][case.property]["definition"];
        assert_eq!(
            reported(definition),
            wanted(&case),
            "case {:?}: expected the patched definition folded over the inherited one, got \
             {definition:?}",
            case.what
        );
    }
}

/// The definition `ListProperties` reports for one property of the composite
/// component at `component_path`.
async fn composite_definition(
    emulator: &Emulator,
    entity_id: &str,
    component_path: &str,
    property: &str,
) -> Value {
    let listed = emulator
        .list_properties(
            WORKSPACE,
            json!({"entityId": entity_id, "componentPath": component_path}),
        )
        .await;
    assert_eq!(
        listed.status, 200,
        "expected 200 from ListProperties, got {listed:?}"
    );
    listed
        .member("propertySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|row| row["propertyName"] == property)
        .unwrap_or_else(|| panic!("expected a row for {property:?}, got {listed:?}"))["definition"]
        .clone()
}

/// RULE (accepted half, a COMPOSITE component): a composite component inherits
/// from the component type the declaration its path resolves to names, so the
/// same refinement is accepted there — by the create that declares it and by the
/// update that patches it.
#[tokio::test]
async fn a_composite_component_inherits_the_data_type_too() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let path = format!("{COMPONENT}/{COMPOSITE}");
    let entity_id = "composite-refinement";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {"componentTypeId": HOLDER}},
                "compositeComponents": {
                    path.clone(): {"properties": {
                        "Serial": {"definition": {"isRequiredInEntity": true}},
                    }},
                },
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    // A composite component is reported as a `ComponentSummary`, which carries
    // no properties (smithy_query(query="shape",
    // shape="CompositeComponentResponse")), so its properties are read through
    // `ListProperties` addressed by `componentPath`.
    let definition = composite_definition(&emulator, entity_id, &path, "Serial").await;
    assert_eq!(
        reported(&definition),
        (
            Some("STRING"),
            Some(INHERITED_DISPLAY_NAME),
            Some(true),
            Some(false),
            None,
        ),
        "expected the composite component's refinement folded over its component type's \
         declaration, got {definition:?}"
    );

    // The update writer, on the composite component the create left standing.
    let patched = emulator
        .update_entity(
            WORKSPACE,
            entity_id,
            json!({"compositeComponentUpdates": {path.clone(): {"propertyUpdates": {
                "Reading": {"definition": {"displayName": "Reading (bar)"}},
            }}}}),
        )
        .await;
    assert_eq!(
        patched.status, 200,
        "expected 200 from UpdateEntity, got {patched:?}"
    );
    let definition = composite_definition(&emulator, entity_id, &path, "Reading").await;
    assert_eq!(
        reported(&definition),
        (
            Some("DOUBLE"),
            Some("Reading (bar)"),
            Some(false),
            Some(false),
            Some(json!({"doubleValue": 1.5})),
        ),
        "expected the patched composite definition folded over its component type's declaration, \
         got {definition:?}"
    );
}

/// One refused row: the component the create declares and why nothing supplies
/// the `dataType` its definition omits.
#[derive(Debug)]
struct Refused {
    /// What this row is about, quoted by every assertion it makes.
    what: &'static str,
    /// The entity id, also the `entityName`.
    entity_id: &'static str,
    /// The component type the component instantiates. A component always names
    /// one — every writer requires it — so a row that supplies no `dataType`
    /// from a component type names one whose declarations do not reach the
    /// property.
    component_type_id: &'static str,
    /// The property the definition is declared for.
    property: &'static str,
}

/// Every refused row: the other side of the rule, where no declaration reaching
/// the property supplies a `dataType`.
fn refused() -> Vec<Refused> {
    vec![
        Refused {
            what: "aComponentTypeTheWorkspaceDoesNotHold",
            entity_id: "absent-type",
            component_type_id: ABSENT,
            property: "Serial",
        },
        Refused {
            what: "aPropertyNoTypeOnTheChainDeclares",
            entity_id: "undeclared-property",
            component_type_id: BASE,
            property: "Spare",
        },
        Refused {
            // `Mode` is declared by a type that EXTENDS this one, which is not
            // on the chain a component of it inherits through.
            what: "aPropertyOnlyATypeOffTheChainDeclares",
            entity_id: "off-chain-property",
            component_type_id: BASE,
            property: "Mode",
        },
    ]
}

/// A component instantiating `component_type_id`.
fn component_of(component_type_id: &str) -> Value {
    json!({"componentTypeId": component_type_id})
}

/// The component a refused row declares.
fn refused_component(case: &Refused) -> Value {
    let mut component = component_of(case.component_type_id);
    component["properties"] = json!({case.property: {"definition": {"isRequiredInEntity": true}}});
    component
}

/// RULE (refused half): a definition that omits `dataType` where NOTHING
/// reaching the property supplies one is still a 400 `ValidationException` from
/// both writers, and nothing is persisted.
#[tokio::test]
async fn a_definition_no_data_type_reaches_is_still_refused() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    for case in refused() {
        let refused = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": case.entity_id,
                    "entityName": case.entity_id,
                    "components": {COMPONENT: refused_component(&case)},
                }),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: expected the modelled refusal from CreateEntity, got {refused:?}",
            case.what
        );
        let message = refused.member("message").as_str().unwrap_or_default();
        assert!(
            message.contains(case.property),
            "case {:?}: expected the refusal to name the property, got {message:?}",
            case.what
        );

        let read = emulator.get_entity(WORKSPACE, case.entity_id).await;
        assert_eq!(
            (read.status, read.error_type.as_deref()),
            (404, Some("ResourceNotFoundException")),
            "case {:?}: a refused create persists nothing, got {read:?}",
            case.what
        );

        // The same definition through the UPDATE writer, on a component the
        // entity already carries: one rule, both writers.
        let entity_id = format!("patched-{}", case.entity_id);
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": entity_id,
                    "entityName": entity_id,
                    "components": {COMPONENT: component_of(case.component_type_id)},
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: expected 200 from CreateEntity, got {created:?}",
            case.what
        );
        let refused = emulator
            .update_entity(
                WORKSPACE,
                &entity_id,
                json!({"componentUpdates": {COMPONENT: {"propertyUpdates": {
                    case.property: {"definition": {"isRequiredInEntity": true}},
                }}}}),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {:?}: expected the modelled refusal from UpdateEntity, got {refused:?}",
            case.what
        );
        let read = emulator.get_entity(WORKSPACE, &entity_id).await;
        assert!(
            read.member("components")[COMPONENT]["properties"][case.property].is_null(),
            "case {:?}: a refused update persists nothing, got {read:?}",
            case.what
        );
    }
}

/// RULE (the boundary the refusal sits on): what the rule refuses is a property
/// with NO `dataType` anywhere, not a property the chain never declared — the
/// very same property is accepted the moment the definition carries a `dataType`
/// of its own.
#[tokio::test]
async fn a_property_the_chain_never_declares_is_accepted_with_its_own_data_type() {
    let emulator = Emulator::new();
    seed(&emulator).await;

    let entity_id = "own-data-type";
    let created = emulator
        .create_entity(
            WORKSPACE,
            json!({
                "entityId": entity_id,
                "entityName": entity_id,
                "components": {COMPONENT: {
                    "componentTypeId": BASE,
                    "properties": {"Spare": {"definition": {
                        "dataType": {"type": "STRING"},
                        "isRequiredInEntity": true,
                    }}},
                }},
            }),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected 200 from CreateEntity, got {created:?}"
    );

    let read = emulator.get_entity(WORKSPACE, entity_id).await;
    let definition = &read.member("components")[COMPONENT]["properties"]["Spare"]["definition"];
    assert_eq!(
        reported(definition),
        (Some("STRING"), None, Some(true), Some(false), None),
        "expected the component's own definition read back whole, got {definition:?}"
    );
    assert_eq!(
        definition["isInherited"].as_bool(),
        Some(false),
        "expected a property no component type on the chain declares to be the component's own, \
         got {definition:?}"
    );
}
