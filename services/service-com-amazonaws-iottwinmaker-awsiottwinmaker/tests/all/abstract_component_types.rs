// SPDX-License-Identifier: CC0-1.0

//! Abstract component types, end to end over HTTP.
//!
//! Every case drives the real request path — `MultiServiceRouter::dispatch`, the
//! same entry the served emulator uses — so a route, a label, a status code and
//! a serialized body are all exercised, not just a handler body.
//!
//! The RULE under test: a component type that declares an externally stored
//! property and carries no data connector that could read or write one — its
//! own or an inherited one — has no way to serve that property's values, so it
//! is ABSTRACT: `GetComponentType` reports `isAbstract` true for it, and a
//! `CreateEntity` whose component instantiates it is refused with the modelled
//! 400 `ValidationException` before anything is stored. Every other component
//! type is concrete and a component of it is accepted.
//!
//! The rows differ along what the rule generalises over — whether the external
//! property is a time series, which data-connector name backs it, whether the
//! property or the connector is the type's own or inherited, and how many
//! properties the type declares — because those are the dimensions the rule
//! ranges over; the time-series row is the one recorded traffic observed.

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::{
    DATA_CONNECTOR_FUNCTION_NAMES, SCHEMA_INITIALIZER_FUNCTION,
};

use crate::harness::{Emulator, Resp};

/// The workspace every case of a suite is created in.
const WORKSPACE: &str = "abstractWs";

/// A component type that carries a data connector and no property at all, so a
/// type extending it INHERITS the connector.
const CONNECTOR_PARENT: &str = "com.example.connector.parent";
/// A component type that carries an externally stored property and no
/// connector, so a type extending it INHERITS the unservable property — and is
/// itself abstract.
const EXTERNAL_PARENT: &str = "com.example.external.parent";

/// One externally stored property definition: `isStoredExternally` is "a
/// Boolean value that specifies whether the property is stored externally" (the
/// member's own documentation), which is what leaves its values unservable
/// without a connector.
fn external_property(is_time_series: bool) -> Value {
    json!({
        "dataType": {"type": "DOUBLE"},
        "isTimeSeries": is_time_series,
        "isStoredExternally": true,
    })
}

/// One function entry under `function_name`, implemented by a lambda the way
/// `FunctionRequest::implementedBy` (a `DataConnector`) models it.
fn function(function_name: &str) -> Value {
    json!({
        function_name: {
            "scope": "ENTITY",
            "implementedBy": {
                "lambda": {"arn": "arn:aws:lambda:us-west-2:000000000000:function:connector"},
            },
        },
    })
}

/// The message a refused `CreateEntity` carries, read off the error body.
fn message(resp: &Resp) -> String {
    resp.member("message").as_str().unwrap_or_default().to_string()
}

/// The refusal AWS builds from the request for a component of an abstract
/// component type: it names the component type the call asked for.
fn abstract_refusal(component_type_id: &str) -> String {
    format!("Abstract ComponentType {component_type_id} can not be used for creating a component")
}

/// The `entityName`s one `ListEntities` page reports.
fn listed_names(resp: &Resp) -> Vec<String> {
    resp.member("entitySummaries")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|summary| summary["entityName"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// Create the workspace and the two parent component types the inheritance rows
/// extend.
async fn seeded() -> Emulator {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace every case is created in must exist"
    );
    assert_eq!(
        emulator
            .create_component_type(
                WORKSPACE,
                CONNECTOR_PARENT,
                json!({
                    "propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}},
                    "functions": function("dataReader"),
                }),
            )
            .await
            .status,
        200,
        "the connector-carrying parent must be created"
    );
    assert_eq!(
        emulator
            .create_component_type(
                WORKSPACE,
                EXTERNAL_PARENT,
                json!({"propertyDefinitions": {"Inherited": external_property(true)}}),
            )
            .await
            .status,
        200,
        "the external-property parent must be created"
    );
    emulator
}

/// RULE: a component type is abstract exactly when it declares an externally
/// stored property (its own or inherited) and no data connector (its own or
/// inherited) — and an abstract component type cannot be instantiated.
#[tokio::test]
async fn abstractness_is_decided_by_an_unservable_property_and_bars_instantiation() {
    let emulator = seeded().await;

    // (case/componentTypeId, the create body, whether the type is abstract).
    // The connector names come from the service's own catalogue
    // (`state::DATA_CONNECTOR_FUNCTION_NAMES`), so a name added to or dropped
    // from it moves these rows with it — including the ones the issue never
    // cited (`dataWriter`, `dataReaderByComponentType`).
    let mut rows: Vec<(String, Value, bool)> = vec![
        (
            // The recorded shape: one externally stored time-series property
            // and no functions at all.
            "com.example.timeSeriesExternal".to_string(),
            json!({"propertyDefinitions": {"Temperature": external_property(true)}}),
            true,
        ),
        (
            // The same gap with a property that is NOT a time series.
            "com.example.plainExternal".to_string(),
            json!({"propertyDefinitions": {"Serial": external_property(false)}}),
            true,
        ),
        (
            // Several properties, only one of them externally stored: one
            // unservable property is enough.
            "com.example.mixedProperties".to_string(),
            json!({
                "propertyDefinitions": {
                    "Label": {"dataType": {"type": "STRING"}},
                    "Count": {"dataType": {"type": "INTEGER"}},
                    "Temperature": external_property(true),
                },
            }),
            true,
        ),
        (
            // A schema initializer is not a data connector: it initializes the
            // schema and serves no property's values.
            "com.example.schemaInitializerOnly".to_string(),
            json!({
                "propertyDefinitions": {"Temperature": external_property(true)},
                "functions": function(SCHEMA_INITIALIZER_FUNCTION),
            }),
            true,
        ),
        (
            // No externally stored property at all: nothing is unservable.
            "com.example.plainOnly".to_string(),
            json!({"propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}}}),
            false,
        ),
        (
            // No properties at all.
            "com.example.bare".to_string(),
            json!({}),
            false,
        ),
        (
            // The property is the type's own, the connector INHERITED.
            "com.example.inheritedConnector".to_string(),
            json!({
                "extendsFrom": [CONNECTOR_PARENT],
                "propertyDefinitions": {"Temperature": external_property(true)},
            }),
            false,
        ),
        (
            // The property is INHERITED and no connector reaches it.
            "com.example.inheritedExternalProperty".to_string(),
            json!({"extendsFrom": [EXTERNAL_PARENT]}),
            true,
        ),
        (
            // The property is inherited from one parent and the connector from
            // another.
            "com.example.inheritedBoth".to_string(),
            json!({"extendsFrom": [EXTERNAL_PARENT, CONNECTOR_PARENT]}),
            false,
        ),
    ];
    // One row per data-connector name the service publishes: each of them backs
    // an externally stored property, so each leaves the type concrete.
    for connector in DATA_CONNECTOR_FUNCTION_NAMES {
        rows.push((
            format!("com.example.backedBy.{connector}"),
            json!({
                "propertyDefinitions": {"Temperature": external_property(true)},
                "functions": function(connector),
            }),
            false,
        ));
    }

    for (component_type_id, request, is_abstract) in &rows {
        let created = emulator
            .create_component_type(WORKSPACE, component_type_id, request.clone())
            .await;
        assert_eq!(
            created.status, 200,
            "case {component_type_id:?}: expected 200 from CreateComponentType, got {created:?}"
        );

        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            read.member("isAbstract").as_bool(),
            Some(*is_abstract),
            "case {component_type_id:?}: expected isAbstract {is_abstract}, got {read:?}"
        );

        // The same component type, instantiated by an entity's component.
        let entity_name = format!("entity for {component_type_id}");
        let create_entity = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityName": entity_name,
                    "components": {"Sensor": {"componentTypeId": component_type_id}},
                }),
            )
            .await;
        if *is_abstract {
            assert_eq!(
                (create_entity.status, create_entity.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {component_type_id:?}: an abstract component type must be refused, \
                 got {create_entity:?}"
            );
            assert_eq!(
                message(&create_entity),
                abstract_refusal(component_type_id),
                "case {component_type_id:?}: expected the refusal to name the component type, \
                 got {create_entity:?}"
            );
            let listed = emulator.list_entities(WORKSPACE, json!({})).await;
            assert!(
                !listed_names(&listed).contains(&entity_name),
                "case {component_type_id:?}: a refused create must persist no entity, \
                 got {listed:?}"
            );
        } else {
            assert_eq!(
                create_entity.status, 200,
                "case {component_type_id:?}: a concrete component type must be accepted, \
                 got {create_entity:?}"
            );
            // Round trip: the component really was persisted against the type
            // the create named.
            let entity_id = create_entity
                .member("entityId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            let entity = emulator.get_entity(WORKSPACE, &entity_id).await;
            assert_eq!(
                entity.member("components")["Sensor"]["componentTypeId"].as_str(),
                Some(component_type_id.as_str()),
                "case {component_type_id:?}: the stored component must name the created type, \
                 got {entity:?}"
            );
        }
    }

    // Controls: an entity with no components at all, and one whose component
    // names a component type this workspace does not hold, are both accepted —
    // the refusal is about abstractness, not about having components.
    for (case, request) in [
        ("noComponents", json!({"entityName": "no components"})),
        (
            "componentOfAnUnheldType",
            json!({
                "entityName": "component of an unheld type",
                "components": {
                    "Sensor": {"componentTypeId": "com.example.not.in.this.workspace"},
                },
            }),
        ),
    ] {
        let created = emulator.create_entity(WORKSPACE, request).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected 200 from CreateEntity, got {created:?}"
        );
    }
}

/// RULE: a COMPOSITE component instantiates the component type the declaration
/// its path resolved to names, so the same abstractness rule refuses it — and a
/// composite component of a concrete type is still accepted.
#[tokio::test]
async fn a_composite_component_of_an_abstract_component_type_is_refused() {
    let emulator = seeded().await;

    // A holder whose two composite declarations differ only in the abstractness
    // of the type they name, so the path that is refused is decided by that
    // alone.
    assert_eq!(
        emulator
            .create_component_type(
                WORKSPACE,
                "com.example.concrete.leaf",
                json!({"propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}}}),
            )
            .await
            .status,
        200,
        "the concrete leaf type must be created"
    );
    assert_eq!(
        emulator
            .create_component_type(
                WORKSPACE,
                "com.example.holder",
                json!({
                    "compositeComponentTypes": {
                        "abstractLeaf": {"componentTypeId": EXTERNAL_PARENT},
                        "concreteLeaf": {"componentTypeId": "com.example.concrete.leaf"},
                    },
                }),
            )
            .await
            .status,
        200,
        "the holder type must be created"
    );

    // (case, the composite path the create addresses, whether it is refused).
    let rows = [
        ("abstractLeaf", "Holder/abstractLeaf", true),
        ("concreteLeaf", "Holder/concreteLeaf", false),
    ];

    for (case, component_path, refused) in rows {
        let entity_name = format!("composite {case}");
        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityName": entity_name,
                    "components": {"Holder": {"componentTypeId": "com.example.holder"}},
                    "compositeComponents": {component_path: {}},
                }),
            )
            .await;
        if refused {
            assert_eq!(
                (created.status, created.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {case:?}: an abstract composite type must be refused, got {created:?}"
            );
            assert_eq!(
                message(&created),
                abstract_refusal(EXTERNAL_PARENT),
                "case {case:?}: expected the refusal to name the component type, got {created:?}"
            );
            let listed = emulator.list_entities(WORKSPACE, json!({})).await;
            assert!(
                !listed_names(&listed).contains(&entity_name),
                "case {case:?}: a refused create must persist no entity, got {listed:?}"
            );
        } else {
            assert_eq!(
                created.status, 200,
                "case {case:?}: a concrete composite type must be accepted, got {created:?}"
            );
            let entity_id = created.member("entityId").as_str().unwrap_or_default().to_string();
            let entity = emulator.get_entity(WORKSPACE, &entity_id).await;
            assert_eq!(
                entity.member("components")["Holder"]["compositeComponents"][case]
                    ["componentTypeId"]
                    .as_str(),
                Some("com.example.concrete.leaf"),
                "case {case:?}: the composite component must read back, got {entity:?}"
            );
        }
    }
}

/// RULE: abstractness is decided from what the component type holds, so a patch
/// that adds or removes the data connector moves it — in BOTH directions — and
/// what `CreateEntity` accepts moves with it.
#[tokio::test]
async fn a_patch_that_changes_the_connector_changes_abstractness() {
    let emulator = seeded().await;
    let component_type_id = "com.example.patched";
    assert_eq!(
        emulator
            .create_component_type(
                WORKSPACE,
                component_type_id,
                json!({"propertyDefinitions": {"Temperature": external_property(true)}}),
            )
            .await
            .status,
        200,
        "the component type under patch must be created"
    );

    // (case, the patch, whether the component type is abstract afterwards).
    let rows: Vec<(&str, Option<Value>, bool)> = vec![
        ("asCreated", None, true),
        (
            "connectorAdded",
            Some(json!({"functions": function("dataReader")})),
            false,
        ),
        (
            // The functions map is replaced WHOLE, so naming it empty drops the
            // connector and the property is unservable again.
            "connectorRemoved",
            Some(json!({"functions": {}})),
            true,
        ),
        (
            // The property goes instead of the connector: a type with no
            // externally stored property is concrete however few functions it
            // has.
            "externalPropertyDropped",
            Some(json!({"propertyDefinitions": {"Label": {"dataType": {"type": "STRING"}}}})),
            false,
        ),
    ];

    for (case, patch, is_abstract) in &rows {
        if let Some(patch) = patch {
            let updated = emulator
                .update_component_type(WORKSPACE, component_type_id, patch.clone())
                .await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected 200 from UpdateComponentType, got {updated:?}"
            );
        }
        let read = emulator
            .get_component_type(WORKSPACE, component_type_id)
            .await;
        assert_eq!(
            read.member("isAbstract").as_bool(),
            Some(*is_abstract),
            "case {case:?}: expected isAbstract {is_abstract}, got {read:?}"
        );

        let created = emulator
            .create_entity(
                WORKSPACE,
                json!({
                    "entityName": format!("patched {case}"),
                    "components": {"Sensor": {"componentTypeId": component_type_id}},
                }),
            )
            .await;
        let expected = if *is_abstract { 400 } else { 200 };
        assert_eq!(
            created.status, expected,
            "case {case:?}: expected HTTP {expected} from CreateEntity, got {created:?}"
        );
    }
}
