// SPDX-License-Identifier: CC0-1.0

//! `ListEntities`' `externalId` filter, end to end over HTTP.
//!
//! RULE under test: `isExternalId` is a member of a property DEFINITION, and a
//! component normally inherits its definitions from the component type it
//! instantiates rather than restating them, so which property of a component is
//! its external id is resolved from the EFFECTIVE definition — the component's
//! own inline definition where it declares one, else the one it inherits from
//! its component type or from a type that type extends — and the value matched
//! is the one a read of the property reports.
//!
//! The rows differ along what the rule generalises over: WHERE the
//! `isExternalId` definition is declared (on the component itself, on the type
//! it names, on a type further up the `extendsFrom` chain, on one of the
//! service's own built-in component types, on a composite component's type),
//! where the VALUE comes from (the component's own property, the definition's
//! `defaultValue`), the DATA TYPE of the external-id property, and how many
//! entities one external id selects.

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below lives in.
const WORKSPACE: &str = "externalIds";

/// The component types the cases resolve their external ids against.
///
/// `com.example.sitewise` extends `com.amazon.iotsitewise.connector`, one of
/// the component types the service ships and provisions every workspace with
/// (the catalogue in this crate's `state::built_in_component_types`, from the
/// user guide's "built-in component types"); its `sitewiseAssetId` and
/// `sitewiseAssetModelId` property definitions are the ones that carry
/// `isExternalId`, so an entity extending it is addressed by either of them.
async fn seed_workspace(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        // The external id declared on the type the component names directly,
        // beside an ordinary property that is NOT one.
        (
            "com.example.sensor",
            json!({"propertyDefinitions": {
                "serial": {"dataType": {"type": "STRING"}, "isExternalId": true},
                "label": {"dataType": {"type": "STRING"}},
            }}),
        ),
        // The external id reached one level further up the `extendsFrom` chain.
        (
            "com.example.pump",
            json!({
                "extendsFrom": ["com.example.sensor"],
                "propertyDefinitions": {"model": {"dataType": {"type": "STRING"}}},
            }),
        ),
        // The external id whose value comes from the definition itself.
        (
            "com.example.defaulted",
            json!({"propertyDefinitions": {"serial": {
                "dataType": {"type": "STRING"},
                "isExternalId": true,
                "defaultValue": {"stringValue": "EXT-DEFAULT"},
            }}}),
        ),
        // An external-id property that is not a string, so it carries no value
        // the filter can match.
        (
            "com.example.numeric",
            json!({"propertyDefinitions": {"code": {
                "dataType": {"type": "INTEGER"},
                "isExternalId": true,
            }}}),
        ),
        // A component type that declares no property at all, so a component of
        // it inherits no definition and its own declarations are all it has.
        ("com.example.plain", json!({})),
        // The external id declared on a COMPOSITE component's type.
        (
            "com.example.holder",
            json!({"compositeComponentTypes": {"inner": {
                "componentTypeId": "com.example.sensor",
            }}}),
        ),
        // The external id declared on one of the service's own built-in types.
        (
            "com.example.sitewise",
            json!({"extendsFrom": ["com.amazon.iotsitewise.connector"]}),
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

/// One entity a case creates: its id, its `components`, and the
/// `compositeComponents` the create carries.
struct Seed {
    entity_id: &'static str,
    components: Value,
    composite_components: Value,
}

/// The entities every case below is listed out of.
fn seeded_entities() -> Vec<Seed> {
    vec![
        // The definition lives on the type the component names; the entity
        // supplies the VALUE only, as a client does.
        Seed {
            entity_id: "typeDeclared",
            components: json!({"Sensor": {
                "componentTypeId": "com.example.sensor",
                "properties": {"serial": {"value": {"stringValue": "EXT-TYPE"}}},
            }}),
            composite_components: json!({}),
        },
        // The definition lives on a type the component's type extends.
        Seed {
            entity_id: "inheritedChain",
            components: json!({"Pump": {
                "componentTypeId": "com.example.pump",
                "properties": {"serial": {"value": {"stringValue": "EXT-CHAIN"}}},
            }}),
            composite_components: json!({}),
        },
        // The definition is declared INLINE on the entity's own property, by a
        // component of a type that declares nothing.
        Seed {
            entity_id: "inlineOnly",
            components: json!({"Tagged": {
                "componentTypeId": "com.example.plain",
                "properties": {"tag": {
                    "definition": {"dataType": {"type": "STRING"}, "isExternalId": true},
                    "value": {"stringValue": "EXT-INLINE"},
                }},
            }}),
            composite_components: json!({}),
        },
        // The component's OWN definition wins over the type's: it says this
        // property is not the external id, so its value selects nothing.
        Seed {
            entity_id: "ownDefinitionWins",
            components: json!({"Sensor": {
                "componentTypeId": "com.example.sensor",
                "properties": {"serial": {
                    "definition": {"dataType": {"type": "STRING"}, "isExternalId": false},
                    "value": {"stringValue": "EXT-SUPPRESSED"},
                }},
            }}),
            composite_components: json!({}),
        },
        // Nothing was written to the external-id property, so its value is the
        // one its definition gave it.
        Seed {
            entity_id: "defaultedValue",
            components: json!({"Sensor": {"componentTypeId": "com.example.defaulted"}}),
            composite_components: json!({}),
        },
        // Both inherited built-in external ids, each of which addresses it.
        Seed {
            entity_id: "sitewiseSynced",
            components: json!({"SiteWise": {
                "componentTypeId": "com.example.sitewise",
                "properties": {
                    "sitewiseAssetId": {"value": {"stringValue": "EXT-ASSET"}},
                    "sitewiseAssetModelId": {"value": {"stringValue": "EXT-MODEL"}},
                },
            }}),
            composite_components: json!({}),
        },
        // An INTEGER external-id property: no string value to match.
        Seed {
            entity_id: "numericCoded",
            components: json!({"Coded": {
                "componentTypeId": "com.example.numeric",
                "properties": {"code": {"value": {"integerValue": 7}}},
            }}),
            composite_components: json!({}),
        },
        // The external id is carried by a COMPOSITE component, whose own type
        // declares the definition.
        Seed {
            entity_id: "compositeCarrier",
            components: json!({"Holder": {"componentTypeId": "com.example.holder"}}),
            composite_components: json!({"Holder/inner": {
                "properties": {"serial": {"value": {"stringValue": "EXT-COMPOSITE"}}},
            }}),
        },
        // A property the type declares WITHOUT `isExternalId`: its value is not
        // an external id.
        Seed {
            entity_id: "plainLabel",
            components: json!({"Sensor": {
                "componentTypeId": "com.example.sensor",
                "properties": {"label": {"value": {"stringValue": "EXT-LABEL"}}},
            }}),
            composite_components: json!({}),
        },
        // Two entities carrying the SAME external id: a filter selects both.
        Seed {
            entity_id: "sharedOne",
            components: json!({"Sensor": {
                "componentTypeId": "com.example.sensor",
                "properties": {"serial": {"value": {"stringValue": "EXT-SHARED"}}},
            }}),
            composite_components: json!({}),
        },
        Seed {
            entity_id: "sharedTwo",
            components: json!({"Pump": {
                "componentTypeId": "com.example.pump",
                "properties": {"serial": {"value": {"stringValue": "EXT-SHARED"}}},
            }}),
            composite_components: json!({}),
        },
    ]
}

/// Create every seeded entity and read each one back, so a case that reports
/// nothing cannot be an entity that was never stored.
async fn seed_entities(emulator: &Emulator) -> Vec<&'static str> {
    let mut entity_ids = Vec::new();
    for seed in seeded_entities() {
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityId": seed.entity_id,
                    "entityName": seed.entity_id,
                    "components": seed.components,
                    "compositeComponents": seed.composite_components,
                }),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "CreateEntity {:?}: expected 200, got {created:?}",
            seed.entity_id
        );
        let read = emulator.get_entity(WORKSPACE, seed.entity_id).await;
        assert_eq!(
            read.status, 200,
            "GetEntity {:?}: expected 200, got {read:?}",
            seed.entity_id
        );
        assert_eq!(
            read.member("entityId").as_str(),
            Some(seed.entity_id),
            "GetEntity {:?}: expected the created entity back, got {read:?}",
            seed.entity_id
        );
        entity_ids.push(seed.entity_id);
    }
    entity_ids.sort_unstable();
    entity_ids
}

/// The entity ids one `ListEntities` page reports, in the order it gave them.
fn page_ids(body: &Value) -> Vec<String> {
    body["entitySummaries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["entityId"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// RULE: the `externalId` filter selects the entities one of whose components
/// carries that external id, where the external-id property is the one whose
/// EFFECTIVE definition — the component's own, else the one it inherits from
/// the component type it instantiates or a type that type extends — carries
/// `isExternalId`, and the value matched is the one a read of that property
/// reports.
#[tokio::test]
async fn the_external_id_filter_resolves_the_definition_through_the_component_type() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;
    seed_entities(&emulator).await;

    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "declared on the component type the component names",
            "EXT-TYPE",
            vec!["typeDeclared"],
        ),
        (
            "declared on a type the component's type extends",
            "EXT-CHAIN",
            vec!["inheritedChain"],
        ),
        (
            "declared inline on the entity's own property",
            "EXT-INLINE",
            vec!["inlineOnly"],
        ),
        (
            "the component's own definition says it is not an external id",
            "EXT-SUPPRESSED",
            vec![],
        ),
        (
            "the value comes from the definition's defaultValue",
            "EXT-DEFAULT",
            vec!["defaultedValue"],
        ),
        (
            "declared on a built-in component type: the asset id",
            "EXT-ASSET",
            vec!["sitewiseSynced"],
        ),
        (
            "declared on a built-in component type: the asset model id",
            "EXT-MODEL",
            vec!["sitewiseSynced"],
        ),
        (
            "the external-id property is not a string",
            "7",
            vec![],
        ),
        (
            "declared on a composite component's own type",
            "EXT-COMPOSITE",
            vec!["compositeCarrier"],
        ),
        (
            "a property the type declares without isExternalId",
            "EXT-LABEL",
            vec![],
        ),
        (
            "one external id carried by two entities",
            "EXT-SHARED",
            vec!["sharedOne", "sharedTwo"],
        ),
        ("an external id nothing carries", "EXT-ABSENT", vec![]),
    ];

    for (case, external_id, expected) in rows {
        let listed = emulator
            .list_entities(WORKSPACE, json!({"filters": [{"externalId": external_id}]}))
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {listed:?}"
        );
        let actual = page_ids(&listed.body);
        assert_eq!(
            actual, expected,
            "case {case:?}: externalId {external_id:?} expected {expected:?}, got {actual:?}"
        );
    }
}

/// RULE: a listing is a REPORT — filtering by an external id changes nothing a
/// later caller sees, and an unfiltered listing still reports every entity of
/// the workspace.
#[tokio::test]
async fn filtering_by_external_id_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;
    let all_entity_ids = seed_entities(&emulator).await;

    let before = emulator.list_entities(WORKSPACE, json!({})).await;
    assert_eq!(
        before.status, 200,
        "the unfiltered listing: expected 200, got {before:?}"
    );
    assert_eq!(
        page_ids(&before.body),
        all_entity_ids,
        "the unfiltered listing must report every entity, got {before:?}"
    );

    for external_id in ["EXT-TYPE", "EXT-SHARED", "EXT-ABSENT"] {
        let first = emulator
            .list_entities(WORKSPACE, json!({"filters": [{"externalId": external_id}]}))
            .await;
        let second = emulator
            .list_entities(WORKSPACE, json!({"filters": [{"externalId": external_id}]}))
            .await;
        assert_eq!(
            page_ids(&first.body),
            page_ids(&second.body),
            "case {external_id:?}: two identical filtered listings must agree, got {first:?} \
             then {second:?}"
        );
    }

    let after = emulator.list_entities(WORKSPACE, json!({})).await;
    assert_eq!(
        page_ids(&after.body),
        all_entity_ids,
        "case \"after filtering\": the unfiltered listing must be unchanged, got {after:?}"
    );
}
