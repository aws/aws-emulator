// SPDX-License-Identifier: CC0-1.0

//! `ListEntities`' `componentTypeId` filter, end to end over HTTP.
//!
//! RULE under test: the filter selects every entity that HOLDS a component
//! instantiating that component type, and a composite component is a component
//! of the entity just as the component holding it is — it instantiates a
//! component type and carries properties of its own, and every reader of the
//! entity reports it as a component — so the filter is answered over an
//! entity's top-level components AND the composite components inside them, at
//! whatever depth.
//!
//! The rows differ along what the rule generalises over: WHERE the component of
//! the filtered type sits (a top-level component, a composite component one
//! level down, one two levels down), HOW it came to be stored (`CreateEntity`'s
//! `compositeComponents`, `UpdateEntity`'s `compositeComponentUpdates`), whether
//! the type is one a workspace declares or one the service ships, and HOW MANY
//! entities one component type selects (none, one, several).

use serde_json::{json, Value};

use crate::harness::Emulator;

/// The workspace every case below lives in.
const WORKSPACE: &str = "componentTypeFilter";

/// A component type the service SHIPS and provisions every workspace with, and
/// that an entity instantiates directly: the document type, from the catalogue
/// in this crate's `state::built_in_component_types`. It is the one concrete
/// built-in there, so a composite component can instantiate it.
const BUILT_IN_CONCRETE_TYPE: &str = "com.amazon.iottwinmaker.documents";

/// A component type the service ships that is ABSTRACT, so nothing instantiates
/// it — the same catalogue. Named here to exercise a filter that selects
/// nothing although the workspace holds the type.
const BUILT_IN_ABSTRACT_TYPE: &str = "com.amazon.iottwinmaker.alarm.basic";

/// The component types the cases below instantiate.
///
/// `com.example.holder` declares the composite component types the seeded
/// paths resolve through, one of which (`nested`) instantiates a type that is
/// itself composite, so a composite component two levels below the entity's
/// component exists.
async fn seed_workspace(emulator: &Emulator) {
    let created = emulator.create_workspace(WORKSPACE).await;
    assert_eq!(
        created.status, 200,
        "CreateWorkspace: expected 200, got {created:?}"
    );

    let component_types: Vec<(&str, Value)> = vec![
        (
            "com.example.inner",
            json!({"propertyDefinitions": {"Pressure": {"dataType": {"type": "DOUBLE"}}}}),
        ),
        (
            "com.example.deep",
            json!({"compositeComponentTypes": {"leaf": {"componentTypeId": "com.example.inner"}}}),
        ),
        ("com.example.extra", json!({})),
        ("com.example.plain", json!({})),
        (
            "com.example.holder",
            json!({"compositeComponentTypes": {
                "inner": {"componentTypeId": "com.example.inner"},
                "nested": {"componentTypeId": "com.example.deep"},
                "spare": {"componentTypeId": "com.example.extra"},
                "docs": {"componentTypeId": BUILT_IN_CONCRETE_TYPE},
            }}),
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

/// A component instantiating the type whose `compositeComponentTypes` the
/// seeded paths resolve through.
fn holder_component() -> Value {
    json!({"Holder": {"componentTypeId": "com.example.holder"}})
}

/// One entity a case lists out of: its id, the `components` and
/// `compositeComponents` its create carries, and the `compositeComponentUpdates`
/// a following `UpdateEntity` applies (empty for an entity no update touches).
struct Seed {
    entity_id: &'static str,
    components: Value,
    composite_components: Value,
    composite_component_updates: Value,
}

/// The entities every case below is listed out of.
fn seeded_entities() -> Vec<Seed> {
    vec![
        // A composite component instantiating a type the SERVICE ships.
        Seed {
            entity_id: "compositeBuiltIn",
            components: holder_component(),
            composite_components: json!({"Holder/docs": {}}),
            composite_component_updates: json!({}),
        },
        // A composite component TWO levels below the entity's component,
        // beside the one-level composite its path runs through.
        Seed {
            entity_id: "compositeDeep",
            components: holder_component(),
            composite_components: json!({"Holder/nested": {}, "Holder/nested/leaf": {}}),
            composite_component_updates: json!({}),
        },
        // A composite component ONE level below the entity's component: the
        // depth the issue's own reproduction uses.
        Seed {
            entity_id: "compositeOne",
            components: holder_component(),
            composite_components: json!({"Holder/inner": {}}),
            composite_component_updates: json!({}),
        },
        // A top-level component and nothing inside it.
        Seed {
            entity_id: "holderTop",
            components: holder_component(),
            composite_components: json!({}),
            composite_component_updates: json!({}),
        },
        // A top-level component of a type no composite declaration names.
        Seed {
            entity_id: "plainTop",
            components: json!({"Plain": {"componentTypeId": "com.example.plain"}}),
            composite_components: json!({}),
            composite_component_updates: json!({}),
        },
        // A component instantiating a component type this workspace does not
        // hold, so no filter naming a type the workspace holds selects this
        // entity.
        Seed {
            entity_id: "unheldComponentType",
            components: json!({"Bare": {"componentTypeId": "com.example.unheld"}}),
            composite_components: json!({}),
            composite_component_updates: json!({}),
        },
        // The composite component is created by an UPDATE rather than by the
        // create, so the filter must see what the update persisted.
        Seed {
            entity_id: "updatedComposite",
            components: holder_component(),
            composite_components: json!({}),
            composite_component_updates: json!({"Holder/spare": {"updateType": "CREATE"}}),
        },
    ]
}

/// Create every seeded entity (applying the update its seed carries) and read
/// each one back, so a case that reports nothing cannot be an entity that was
/// never stored.
async fn seed_entities(emulator: &Emulator) -> Vec<String> {
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

        if seed
            .composite_component_updates
            .as_object()
            .is_some_and(|updates| !updates.is_empty())
        {
            let updated = emulator
                .update_entity(
                    WORKSPACE,
                    seed.entity_id,
                    json!({"compositeComponentUpdates": seed.composite_component_updates}),
                )
                .await;
            assert_eq!(
                updated.status, 200,
                "UpdateEntity {:?}: expected 200, got {updated:?}",
                seed.entity_id
            );
        }

        let read = emulator.get_entity(WORKSPACE, seed.entity_id).await;
        assert_eq!(
            read.status, 200,
            "GetEntity {:?}: expected 200, got {read:?}",
            seed.entity_id
        );
        entity_ids.push(seed.entity_id.to_string());
    }
    entity_ids.sort();
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

/// Every component type `GetEntity` reports a component of, at any depth: the
/// `componentTypeId` of each top-level component and of each composite
/// component inside it.
fn reported_component_type_ids(body: &Value) -> Vec<String> {
    let mut reported = Vec::new();
    let components = body["components"].as_object().cloned().unwrap_or_default();
    for component in components.values() {
        if let Some(component_type_id) = component["componentTypeId"].as_str() {
            reported.push(component_type_id.to_string());
        }
        let composites = component["compositeComponents"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for composite in composites.values() {
            if let Some(component_type_id) = composite["componentTypeId"].as_str() {
                reported.push(component_type_id.to_string());
            }
        }
    }
    reported.sort();
    reported.dedup();
    reported
}

/// RULE: the `componentTypeId` filter selects every entity holding a component
/// of that type, whether that component is one of the entity's top-level
/// components or a composite component inside one of them, at whatever depth
/// and however it was stored.
#[tokio::test]
async fn the_component_type_filter_selects_composite_components_too() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;
    seed_entities(&emulator).await;

    let rows: Vec<(&str, &str, Vec<&str>)> = vec![
        (
            "a top-level component's own type, held by one entity",
            "com.example.plain",
            vec!["plainTop"],
        ),
        (
            "a top-level component's own type, held by several entities",
            "com.example.holder",
            vec![
                "compositeBuiltIn",
                "compositeDeep",
                "compositeOne",
                "holderTop",
                "updatedComposite",
            ],
        ),
        (
            "a composite component's type, one level down in one entity and two \
             levels down in another",
            "com.example.inner",
            vec!["compositeDeep", "compositeOne"],
        ),
        (
            "a composite component's type one level down, itself holding a composite",
            "com.example.deep",
            vec!["compositeDeep"],
        ),
        (
            "a composite component created by UpdateEntity",
            "com.example.extra",
            vec!["updatedComposite"],
        ),
        (
            "a component type the service ships, instantiated as a composite component",
            BUILT_IN_CONCRETE_TYPE,
            vec!["compositeBuiltIn"],
        ),
        (
            "a component type the workspace holds that nothing instantiates",
            BUILT_IN_ABSTRACT_TYPE,
            vec![],
        ),
        (
            "a component type the workspace does not hold",
            "com.example.absent",
            vec![],
        ),
    ];

    for (case, component_type_id, expected) in rows {
        let listed = emulator
            .list_entities(
                WORKSPACE,
                json!({"filters": [{"componentTypeId": component_type_id}]}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected 200, got {listed:?}"
        );
        let actual = page_ids(&listed.body);
        assert_eq!(
            actual, expected,
            "case {case:?}: componentTypeId {component_type_id:?} expected {expected:?}, \
             got {actual:?}"
        );
    }
}

/// RULE (the same one, as an invariant): a filter on any component type a
/// READER of an entity reports selects that entity, so the listing cannot
/// disagree with `GetEntity` about which components the entity holds.
#[tokio::test]
async fn every_component_type_a_reader_reports_selects_its_entity() {
    let emulator = Emulator::new();
    seed_workspace(&emulator).await;
    let all_entity_ids = seed_entities(&emulator).await;

    for entity_id in &all_entity_ids {
        let read = emulator.get_entity(WORKSPACE, entity_id).await;
        assert_eq!(
            read.status, 200,
            "case {entity_id:?}: expected 200 from GetEntity, got {read:?}"
        );
        let reported = reported_component_type_ids(&read.body);
        for component_type_id in &reported {
            let listed = emulator
                .list_entities(
                    WORKSPACE,
                    json!({"filters": [{"componentTypeId": component_type_id}]}),
                )
                .await;
            assert_eq!(
                listed.status, 200,
                "case {entity_id:?} / {component_type_id:?}: expected 200, got {listed:?}"
            );
            let actual = page_ids(&listed.body);
            assert!(
                actual.contains(entity_id),
                "case {entity_id:?} / {component_type_id:?}: GetEntity reports a component of \
                 that type, so the filter must select the entity, got {actual:?}"
            );
        }
    }
}

/// RULE: a listing is a REPORT — filtering by a component type changes nothing
/// a later caller sees, and an unfiltered listing still reports every entity of
/// the workspace.
#[tokio::test]
async fn filtering_by_component_type_changes_nothing_a_later_caller_sees() {
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

    for component_type_id in [
        "com.example.holder",
        "com.example.inner",
        BUILT_IN_CONCRETE_TYPE,
        "com.example.absent",
    ] {
        let first = emulator
            .list_entities(
                WORKSPACE,
                json!({"filters": [{"componentTypeId": component_type_id}]}),
            )
            .await;
        let second = emulator
            .list_entities(
                WORKSPACE,
                json!({"filters": [{"componentTypeId": component_type_id}]}),
            )
            .await;
        assert_eq!(
            page_ids(&first.body),
            page_ids(&second.body),
            "case {component_type_id:?}: two identical filtered listings must agree, got \
             {first:?} then {second:?}"
        );
    }

    let after = emulator.list_entities(WORKSPACE, json!({})).await;
    assert_eq!(
        page_ids(&after.body),
        all_entity_ids,
        "case \"after filtering\": the unfiltered listing must be unchanged, got {after:?}"
    );
}
