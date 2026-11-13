// SPDX-License-Identifier: CC0-1.0

//! The METADATA DOCUMENT a transfer reads and writes: the one spelling of what
//! an IoT TwinMaker workspace's metadata looks like outside a workspace.
//!
//! A metadata transfer job moves component types and entities between a
//! workspace and a document: an Amazon S3 source names a document to import, an
//! Amazon S3 destination is where a workspace is exported to, and a job with an
//! IoT TwinMaker end at each side moves the same entries from one workspace to
//! another. So there is ONE intermediate form here — [`MetadataDocument`] — with
//! a reader ([`parse`]) and a writer ([`render`]) over it, and the two are
//! inverses: a workspace exported to a document and imported back describes the
//! same component types and entities.
//!
//! The document's members are the API's own, spelled exactly as the modelled
//! request shapes spell them on the wire (`componentTypeId`,
//! `propertyDefinitions`, `dataType`, `isSingleton`, …), because an entry of the
//! document IS the request that would have created the resource. That is why the
//! entries below carry the modelled request types
//! (`PropertyDefinitionRequest`, `FunctionRequest`, `PropertyRequest`) rather
//! than a second description of them: the writer that lands an entry builds the
//! same records `CreateComponentType` and `CreateEntity` build.
//!
//! The document is read with the maintained JSON parser this crate already
//! carries; nothing here is hand-lexed. What IS stated here is the member
//! layout, which no crate can know, and the bounds a caller-supplied document is
//! read within: at most [`MAX_DOCUMENT_BYTES`] of it is fetched, it may describe
//! no more entries than a workspace can hold
//! ([`MAX_ENTITIES_PER_WORKSPACE`], [`MAX_COMPONENT_TYPES_PER_WORKSPACE`]), and
//! a value's nesting is read to [`MAX_DOCUMENT_VALUE_DEPTH`] levels and no
//! deeper.

use super::*;

use ::serde_json::{Map, Value};

/// The most bytes of one metadata document a transfer reads.
///
/// Emulator resource guard, NOT a quota AWS publishes: the document is buffered
/// in this process, so without a ceiling one job could ask the emulator to hold
/// an unbounded amount of it. Set well above any realistic use of a local
/// emulator — a workspace at this service's own entity ceiling renders far
/// smaller than this.
pub const MAX_DOCUMENT_BYTES: usize = 4 * 1024 * 1024;

/// How deeply a `DataValue`'s `listValue`/`mapValue` nesting is read and
/// written.
///
/// A `DataValue` nests through those two members with no modelled bound on the
/// depth, so a document could describe a value nested arbitrarily deep; this is
/// the depth the codec reads, which bounds the recursion a caller-supplied
/// document drives — the same reason this service bounds the depth it renders a
/// filter value into a pagination token to. Past the bound the nested value is
/// read and written as nothing.
pub const MAX_DOCUMENT_VALUE_DEPTH: usize = 8;

/// The entries one metadata document describes.
///
/// Component types come first in every document this renders and are written
/// before the entities in every document it reads: an entity's component
/// instantiates a component type, so the types have to be in hand before the
/// entities that name them.
#[derive(Debug, Clone, Default)]
pub struct MetadataDocument {
    pub component_types: Vec<ComponentTypeEntry>,
    pub entities: Vec<EntityEntry>,
}

impl MetadataDocument {
    /// How many resources this document describes: one entry, component type or
    /// entity, is one unit of the transfer's progress.
    pub fn entry_count(&self) -> usize {
        self.component_types.len() + self.entities.len()
    }

    /// Whether this document describes nothing at all.
    pub fn is_empty(&self) -> bool {
        self.entry_count() == 0
    }
}

/// One component type the document describes — the members
/// `CreateComponentType` takes, in the shapes it takes them in.
#[derive(Debug, Clone, Default)]
pub struct ComponentTypeEntry {
    pub component_type_id: String,
    pub component_type_name: Option<String>,
    pub description: Option<String>,
    /// The parent component types this one extends, in document order.
    pub extends_from: Vec<String>,
    pub is_singleton: bool,
    pub property_definitions: BTreeMap<String, crate::ops::types::PropertyDefinitionRequest>,
    pub functions: BTreeMap<String, crate::ops::types::FunctionRequest>,
    pub property_groups: BTreeMap<String, crate::ops::types::PropertyGroupRequest>,
    pub composite_component_types:
        BTreeMap<String, crate::ops::types::CompositeComponentTypeRequest>,
    pub tags: BTreeMap<String, String>,
}

/// One entity the document describes — the members `CreateEntity` takes.
///
/// A component's own composite components are NOT part of the document: a
/// composite component is addressed by a `ComponentPath` resolved against the
/// workspace that holds it, which is a workspace-local address rather than
/// something a document outside any workspace can name.
#[derive(Debug, Clone, Default)]
pub struct EntityEntry {
    pub entity_id: String,
    pub entity_name: String,
    pub description: Option<String>,
    /// The parent the entity hangs off, absent for one at the root of the
    /// workspace's entity tree.
    pub parent_entity_id: Option<String>,
    pub components: BTreeMap<String, ComponentEntry>,
    pub tags: BTreeMap<String, String>,
}

/// One component of an entity the document describes — the members
/// `ComponentRequest` carries.
#[derive(Debug, Clone, Default)]
pub struct ComponentEntry {
    /// The component type the component instantiates, as the document spells
    /// it: optional on the wire like the request member it mirrors, and
    /// required of every component the writer lands
    /// ([`crate::state::component_type_id_required_message`]), which fails the
    /// entry that names none.
    pub component_type_id: Option<String>,
    pub description: Option<String>,
    pub properties: BTreeMap<String, crate::ops::types::PropertyRequest>,
    pub property_groups: BTreeMap<String, crate::ops::types::PropertyGroupRequest>,
}

/// Read `bytes` as a metadata document, or say why they describe none.
///
/// The reason is the job's own `error.message`, so it names what about the
/// document could not be read rather than quoting the document back.
pub fn parse(bytes: &[u8]) -> Result<MetadataDocument, String> {
    let document: Value = ::serde_json::from_slice(bytes)
        .map_err(|_| "The metadata document is not a JSON document.".to_string())?;
    let Some(members) = document.as_object() else {
        return Err("The metadata document is not a JSON object.".to_string());
    };
    if !members.contains_key(COMPONENT_TYPES) && !members.contains_key(ENTITIES) {
        return Err(format!(
            "The metadata document names neither {COMPONENT_TYPES} nor {ENTITIES}."
        ));
    }
    let component_types = entries(members, COMPONENT_TYPES)?;
    let entities = entries(members, ENTITIES)?;
    if component_types.len() > MAX_COMPONENT_TYPES_PER_WORKSPACE {
        return Err(format!(
            "The metadata document describes more than the {MAX_COMPONENT_TYPES_PER_WORKSPACE} \
             component types a workspace holds."
        ));
    }
    if entities.len() > MAX_ENTITIES_PER_WORKSPACE {
        return Err(format!(
            "The metadata document describes more than the {MAX_ENTITIES_PER_WORKSPACE} entities \
             a workspace holds."
        ));
    }
    Ok(MetadataDocument {
        component_types: component_types
            .into_iter()
            .map(component_type_entry)
            .collect::<Result<Vec<_>, String>>()?,
        entities: entities
            .into_iter()
            .map(entity_entry)
            .collect::<Result<Vec<_>, String>>()?,
    })
}

/// Write a metadata document describing `document`'s entries.
///
/// The inverse of [`parse`]: every member an entry carries is written under the
/// name [`parse`] reads it by, and a member the entry does not carry is left out
/// rather than written as a null, so a document this renders reads back as the
/// entries it was rendered from.
pub fn render(document: &MetadataDocument) -> Value {
    let component_types: Vec<Value> = document
        .component_types
        .iter()
        .map(component_type_json)
        .collect();
    let entities: Vec<Value> = document.entities.iter().map(entity_json).collect();
    Value::Object(Map::from_iter([
        (COMPONENT_TYPES.to_string(), Value::Array(component_types)),
        (ENTITIES.to_string(), Value::Array(entities)),
    ]))
}

/// The member a document lists its component types under.
const COMPONENT_TYPES: &str = "componentTypes";
/// The member a document lists its entities under.
const ENTITIES: &str = "entities";

/// The objects `member` lists, or the reason it is not a list of them. An absent
/// member lists nothing: a document may describe component types only, or
/// entities only.
fn entries<'a>(document: &'a Map<String, Value>, member: &str) -> Result<Vec<&'a Value>, String> {
    let Some(listed) = document.get(member) else {
        return Ok(Vec::new());
    };
    let Some(listed) = listed.as_array() else {
        return Err(format!("The metadata document's {member} is not a list."));
    };
    if let Some(index) = listed.iter().position(|entry| !entry.is_object()) {
        return Err(format!(
            "The metadata document's {member}[{index}] is not a JSON object."
        ));
    }
    Ok(listed.iter().collect())
}

/// One `componentTypes` entry as the component type it describes.
fn component_type_entry(entry: &Value) -> Result<ComponentTypeEntry, String> {
    let component_type_id = text(entry, "componentTypeId")
        .ok_or_else(|| format!("A {COMPONENT_TYPES} entry names no componentTypeId."))?;
    Ok(ComponentTypeEntry {
        component_type_id,
        component_type_name: text(entry, "componentTypeName"),
        description: text(entry, "description"),
        extends_from: names(entry, "extendsFrom"),
        is_singleton: flag(entry, "isSingleton").unwrap_or(false),
        property_definitions: keyed(entry, "propertyDefinitions", property_definition),
        functions: keyed(entry, "functions", function),
        property_groups: keyed(entry, "propertyGroups", property_group),
        composite_component_types: keyed(entry, "compositeComponentTypes", |declaration| {
            crate::ops::types::CompositeComponentTypeRequest {
                component_type_id: text(declaration, "componentTypeId"),
            }
        }),
        tags: string_map(entry, "tags"),
    })
}

/// One `entities` entry as the entity it describes.
fn entity_entry(entry: &Value) -> Result<EntityEntry, String> {
    let entity_id = text(entry, "entityId")
        .ok_or_else(|| format!("An {ENTITIES} entry names no entityId."))?;
    // `entityName` is `@required` on `CreateEntityRequest`, so an entry that
    // names none describes no entity any writer could create.
    let entity_name = text(entry, "entityName")
        .ok_or_else(|| format!("The {ENTITIES} entry {entity_id} names no entityName."))?;
    Ok(EntityEntry {
        entity_id,
        entity_name,
        description: text(entry, "description"),
        parent_entity_id: text(entry, "parentEntityId"),
        components: keyed(entry, "components", component),
        tags: string_map(entry, "tags"),
    })
}

/// One `components` entry as the component it describes.
fn component(entry: &Value) -> ComponentEntry {
    ComponentEntry {
        component_type_id: text(entry, "componentTypeId"),
        description: text(entry, "description"),
        properties: keyed(entry, "properties", property),
        property_groups: keyed(entry, "propertyGroups", property_group),
    }
}

/// One `properties` entry as the modelled `PropertyRequest` it describes.
///
/// `updateType` is not read: it is a directive to an UPDATE about what to do
/// with the property, and a document describes the property itself.
fn property(entry: &Value) -> crate::ops::types::PropertyRequest {
    crate::ops::types::PropertyRequest {
        definition: entry.get("definition").map(property_definition),
        value: entry.get("value").map(|value| data_value(value, 0)),
        update_type: None,
    }
}

/// One `propertyDefinitions` entry as the modelled `PropertyDefinitionRequest`.
fn property_definition(entry: &Value) -> crate::ops::types::PropertyDefinitionRequest {
    crate::ops::types::PropertyDefinitionRequest {
        data_type: entry.get("dataType").map(|value| data_type(value, 0)),
        is_required_in_entity: flag(entry, "isRequiredInEntity"),
        is_external_id: flag(entry, "isExternalId"),
        is_stored_externally: flag(entry, "isStoredExternally"),
        is_time_series: flag(entry, "isTimeSeries"),
        default_value: entry.get("defaultValue").map(|value| data_value(value, 0)),
        configuration: present_map(string_map(entry, "configuration"))
            .map(|configuration| configuration.into_iter().collect()),
        display_name: text(entry, "displayName"),
    }
}

/// One `dataType` as the modelled `DataType`.
fn data_type(entry: &Value, depth: usize) -> crate::ops::types::DataType {
    crate::ops::types::DataType {
        // `type` is `@required` on `DataType`, and a definition whose type is
        // unreadable is one the writer refuses by the rule every writer of a
        // definition meets (`dataType` is what a read has to report), so the
        // empty string stands for "the entry named none" rather than inventing
        // a type.
        r#type: text(entry, "type").unwrap_or_default(),
        nested_type: match depth < MAX_DOCUMENT_VALUE_DEPTH {
            true => entry
                .get("nestedType")
                .map(|nested| Box::new(data_type(nested, depth + 1))),
            false => None,
        },
        allowed_values: present(
            entry
                .get("allowedValues")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| data_value(value, depth))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        ),
        unit_of_measure: text(entry, "unitOfMeasure"),
        relationship: entry
            .get("relationship")
            .map(|relationship| crate::ops::types::Relationship {
                target_component_type_id: text(relationship, "targetComponentTypeId"),
                relationship_type: text(relationship, "relationshipType"),
            }),
    }
}

/// One value as the modelled `DataValue`, read to
/// [`MAX_DOCUMENT_VALUE_DEPTH`] levels of nesting.
fn data_value(entry: &Value, depth: usize) -> crate::ops::types::DataValue {
    let nested = depth < MAX_DOCUMENT_VALUE_DEPTH;
    crate::ops::types::DataValue {
        boolean_value: flag(entry, "booleanValue"),
        double_value: entry.get("doubleValue").and_then(Value::as_f64),
        integer_value: entry
            .get("integerValue")
            .and_then(Value::as_i64)
            .and_then(|number| i32::try_from(number).ok()),
        long_value: entry.get("longValue").and_then(Value::as_i64),
        string_value: text(entry, "stringValue"),
        list_value: match nested {
            true => entry
                .get("listValue")
                .and_then(Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| data_value(value, depth + 1))
                        .collect()
                }),
            false => None,
        },
        map_value: match nested {
            true => entry
                .get("mapValue")
                .and_then(Value::as_object)
                .map(|values| {
                    values
                        .iter()
                        .map(|(key, value)| (key.clone(), data_value(value, depth + 1)))
                        .collect()
                }),
            false => None,
        },
        relationship_value: entry.get("relationshipValue").map(|relationship| {
            crate::ops::types::RelationshipValue {
                target_entity_id: text(relationship, "targetEntityId"),
                target_component_name: text(relationship, "targetComponentName"),
            }
        }),
        expression: text(entry, "expression"),
    }
}

/// One `functions` entry as the modelled `FunctionRequest`.
fn function(entry: &Value) -> crate::ops::types::FunctionRequest {
    crate::ops::types::FunctionRequest {
        required_properties: present(names(entry, "requiredProperties")),
        scope: text(entry, "scope"),
        implemented_by: entry
            .get("implementedBy")
            .map(|connector| crate::ops::types::DataConnector {
                lambda: connector
                    .get("lambda")
                    .and_then(|lambda| text(lambda, "arn"))
                    .map(|arn| crate::ops::types::LambdaFunction { arn }),
                is_native: flag(connector, "isNative"),
            }),
    }
}

/// One `propertyGroups` entry as the modelled `PropertyGroupRequest`.
fn property_group(entry: &Value) -> crate::ops::types::PropertyGroupRequest {
    crate::ops::types::PropertyGroupRequest {
        group_type: text(entry, "groupType"),
        property_names: present(names(entry, "propertyNames")),
    }
}

/// One component type entry as the document writes it.
fn component_type_json(entry: &ComponentTypeEntry) -> Value {
    let mut written = Map::new();
    written.insert(
        "componentTypeId".to_string(),
        Value::from(entry.component_type_id.as_str()),
    );
    put_text(&mut written, "componentTypeName", &entry.component_type_name);
    put_text(&mut written, "description", &entry.description);
    put_names(&mut written, "extendsFrom", &entry.extends_from);
    if entry.is_singleton {
        written.insert("isSingleton".to_string(), Value::from(true));
    }
    put_keyed(
        &mut written,
        "propertyDefinitions",
        &entry.property_definitions,
        property_definition_json,
    );
    put_keyed(&mut written, "functions", &entry.functions, function_json);
    put_keyed(
        &mut written,
        "propertyGroups",
        &entry.property_groups,
        property_group_json,
    );
    put_keyed(
        &mut written,
        "compositeComponentTypes",
        &entry.composite_component_types,
        |declaration| {
            let mut written = Map::new();
            put_text(
                &mut written,
                "componentTypeId",
                &declaration.component_type_id,
            );
            Value::Object(written)
        },
    );
    put_string_map(&mut written, "tags", &entry.tags);
    Value::Object(written)
}

/// One entity entry as the document writes it.
fn entity_json(entry: &EntityEntry) -> Value {
    let mut written = Map::new();
    written.insert(
        "entityId".to_string(),
        Value::from(entry.entity_id.as_str()),
    );
    written.insert(
        "entityName".to_string(),
        Value::from(entry.entity_name.as_str()),
    );
    put_text(&mut written, "description", &entry.description);
    put_text(&mut written, "parentEntityId", &entry.parent_entity_id);
    put_keyed(&mut written, "components", &entry.components, component_json);
    put_string_map(&mut written, "tags", &entry.tags);
    Value::Object(written)
}

/// One component of an entity as the document writes it.
fn component_json(entry: &ComponentEntry) -> Value {
    let mut written = Map::new();
    put_text(&mut written, "componentTypeId", &entry.component_type_id);
    put_text(&mut written, "description", &entry.description);
    put_keyed(&mut written, "properties", &entry.properties, property_json);
    put_keyed(
        &mut written,
        "propertyGroups",
        &entry.property_groups,
        property_group_json,
    );
    Value::Object(written)
}

/// One property of a component as the document writes it.
fn property_json(entry: &crate::ops::types::PropertyRequest) -> Value {
    let mut written = Map::new();
    if let Some(definition) = &entry.definition {
        written.insert(
            "definition".to_string(),
            property_definition_json(definition),
        );
    }
    if let Some(value) = &entry.value {
        written.insert("value".to_string(), data_value_json(value, 0));
    }
    Value::Object(written)
}

/// One property definition as the document writes it.
fn property_definition_json(entry: &crate::ops::types::PropertyDefinitionRequest) -> Value {
    let mut written = Map::new();
    if let Some(declared) = &entry.data_type {
        written.insert("dataType".to_string(), data_type_json(declared, 0));
    }
    put_flag(&mut written, "isRequiredInEntity", entry.is_required_in_entity);
    put_flag(&mut written, "isExternalId", entry.is_external_id);
    put_flag(&mut written, "isStoredExternally", entry.is_stored_externally);
    put_flag(&mut written, "isTimeSeries", entry.is_time_series);
    if let Some(value) = &entry.default_value {
        written.insert("defaultValue".to_string(), data_value_json(value, 0));
    }
    if let Some(configuration) = &entry.configuration {
        let configuration: BTreeMap<String, String> = configuration
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        put_string_map(&mut written, "configuration", &configuration);
    }
    put_text(&mut written, "displayName", &entry.display_name);
    Value::Object(written)
}

/// One `dataType` as the document writes it.
fn data_type_json(entry: &crate::ops::types::DataType, depth: usize) -> Value {
    let mut written = Map::new();
    written.insert("type".to_string(), Value::from(entry.r#type.as_str()));
    if let Some(nested) = &entry.nested_type {
        if depth < MAX_DOCUMENT_VALUE_DEPTH {
            written.insert("nestedType".to_string(), data_type_json(nested, depth + 1));
        }
    }
    if let Some(allowed) = &entry.allowed_values {
        written.insert(
            "allowedValues".to_string(),
            Value::Array(
                allowed
                    .iter()
                    .map(|value| data_value_json(value, depth))
                    .collect(),
            ),
        );
    }
    put_text(&mut written, "unitOfMeasure", &entry.unit_of_measure);
    if let Some(relationship) = &entry.relationship {
        let mut target = Map::new();
        put_text(
            &mut target,
            "targetComponentTypeId",
            &relationship.target_component_type_id,
        );
        put_text(
            &mut target,
            "relationshipType",
            &relationship.relationship_type,
        );
        written.insert("relationship".to_string(), Value::Object(target));
    }
    Value::Object(written)
}

/// One `DataValue` as the document writes it, to
/// [`MAX_DOCUMENT_VALUE_DEPTH`] levels of nesting.
fn data_value_json(entry: &crate::ops::types::DataValue, depth: usize) -> Value {
    let mut written = Map::new();
    put_flag(&mut written, "booleanValue", entry.boolean_value);
    if let Some(number) = entry.double_value {
        written.insert("doubleValue".to_string(), Value::from(number));
    }
    if let Some(number) = entry.integer_value {
        written.insert("integerValue".to_string(), Value::from(number));
    }
    if let Some(number) = entry.long_value {
        written.insert("longValue".to_string(), Value::from(number));
    }
    put_text(&mut written, "stringValue", &entry.string_value);
    if depth < MAX_DOCUMENT_VALUE_DEPTH {
        if let Some(values) = &entry.list_value {
            written.insert(
                "listValue".to_string(),
                Value::Array(
                    values
                        .iter()
                        .map(|value| data_value_json(value, depth + 1))
                        .collect(),
                ),
            );
        }
        if let Some(values) = &entry.map_value {
            // In key order, so one value renders the same way every time it is
            // written: the modelled map is a `HashMap`, whose iteration order
            // is not the document's.
            let values: BTreeMap<&String, &crate::ops::types::DataValue> =
                values.iter().collect();
            written.insert(
                "mapValue".to_string(),
                Value::Object(
                    values
                        .into_iter()
                        .map(|(key, value)| (key.clone(), data_value_json(value, depth + 1)))
                        .collect(),
                ),
            );
        }
    }
    if let Some(relationship) = &entry.relationship_value {
        let mut target = Map::new();
        put_text(&mut target, "targetEntityId", &relationship.target_entity_id);
        put_text(
            &mut target,
            "targetComponentName",
            &relationship.target_component_name,
        );
        written.insert("relationshipValue".to_string(), Value::Object(target));
    }
    put_text(&mut written, "expression", &entry.expression);
    Value::Object(written)
}

/// One `functions` entry as the document writes it.
fn function_json(entry: &crate::ops::types::FunctionRequest) -> Value {
    let mut written = Map::new();
    if let Some(required) = &entry.required_properties {
        put_names(&mut written, "requiredProperties", required);
    }
    put_text(&mut written, "scope", &entry.scope);
    if let Some(connector) = &entry.implemented_by {
        let mut implemented_by = Map::new();
        if let Some(lambda) = &connector.lambda {
            implemented_by.insert(
                "lambda".to_string(),
                Value::Object(Map::from_iter([(
                    "arn".to_string(),
                    Value::from(lambda.arn.as_str()),
                )])),
            );
        }
        put_flag(&mut implemented_by, "isNative", connector.is_native);
        written.insert("implementedBy".to_string(), Value::Object(implemented_by));
    }
    Value::Object(written)
}

/// One `propertyGroups` entry as the document writes it.
fn property_group_json(entry: &crate::ops::types::PropertyGroupRequest) -> Value {
    let mut written = Map::new();
    put_text(&mut written, "groupType", &entry.group_type);
    if let Some(property_names) = &entry.property_names {
        put_names(&mut written, "propertyNames", property_names);
    }
    Value::Object(written)
}

/// The string `member` carries, or `None` when it carries no string.
fn text(entry: &Value, member: &str) -> Option<String> {
    entry
        .get(member)?
        .as_str()
        .map(::std::string::ToString::to_string)
}

/// The boolean `member` carries, or `None` when it carries no boolean.
fn flag(entry: &Value, member: &str) -> Option<bool> {
    entry.get(member)?.as_bool()
}

/// The strings `member` lists, in list order; empty when it lists none.
fn names(entry: &Value, member: &str) -> Vec<String> {
    entry
        .get(member)
        .and_then(Value::as_array)
        .map(|listed| {
            listed
                .iter()
                .filter_map(Value::as_str)
                .map(::std::string::ToString::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// The string-to-string map `member` carries, in KEY order; empty when it
/// carries none.
fn string_map(entry: &Value, member: &str) -> BTreeMap<String, String> {
    entry
        .get(member)
        .and_then(Value::as_object)
        .map(|carried| {
            carried
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The entries the object `member` carries, each read by `read`, in KEY order.
fn keyed<T>(entry: &Value, member: &str, read: impl Fn(&Value) -> T) -> BTreeMap<String, T> {
    entry
        .get(member)
        .and_then(Value::as_object)
        .map(|carried| {
            carried
                .iter()
                .map(|(key, value)| (key.clone(), read(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// `collection` when it holds something, and `None` when it is empty — so an
/// absent member reads back absent rather than as an empty collection.
fn present<T>(collection: Vec<T>) -> Option<Vec<T>> {
    (!collection.is_empty()).then_some(collection)
}

/// Write `member` when `value` names one.
fn put_text(written: &mut Map<String, Value>, member: &str, value: &Option<String>) {
    if let Some(value) = value {
        written.insert(member.to_string(), Value::from(value.as_str()));
    }
}

/// Write `member` when `value` is set either way.
fn put_flag(written: &mut Map<String, Value>, member: &str, value: Option<bool>) {
    if let Some(value) = value {
        written.insert(member.to_string(), Value::from(value));
    }
}

/// Write the list `member` when it holds something.
fn put_names(written: &mut Map<String, Value>, member: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    written.insert(
        member.to_string(),
        Value::Array(names.iter().map(|name| Value::from(name.as_str())).collect()),
    );
}

/// Write the string map `member` when it holds something.
fn put_string_map(
    written: &mut Map<String, Value>,
    member: &str,
    entries: &BTreeMap<String, String>,
) {
    if entries.is_empty() {
        return;
    }
    written.insert(
        member.to_string(),
        Value::Object(
            entries
                .iter()
                .map(|(key, value)| (key.clone(), Value::from(value.as_str())))
                .collect(),
        ),
    );
}

/// Write the keyed `member` when it holds something, each entry by `write`.
fn put_keyed<T>(
    written: &mut Map<String, Value>,
    member: &str,
    entries: &BTreeMap<String, T>,
    write: impl Fn(&T) -> Value,
) {
    if entries.is_empty() {
        return;
    }
    written.insert(
        member.to_string(),
        Value::Object(
            entries
                .iter()
                .map(|(key, entry)| (key.clone(), write(entry)))
                .collect(),
        ),
    );
}

/// `present`, for a map: the empty map reads back as no member at all.
fn present_map<T>(entries: BTreeMap<String, T>) -> Option<BTreeMap<String, T>> {
    (!entries.is_empty()).then_some(entries)
}

/// The stored component type as the document describes it: the inverse of what
/// the writer builds from a document entry, so a workspace exported and
/// imported back describes the same component type.
///
/// The service's own BUILT-IN component types are not described by a document:
/// every workspace is provisioned with them, so a document that carried them
/// would describe resources no import could create
/// ([`built_in_component_type_id`]). The caller's own are what a workspace
/// holds beyond them ([`Workspace::created_component_types`]).
pub fn component_type_entry_of(record: &ComponentType) -> ComponentTypeEntry {
    ComponentTypeEntry {
        component_type_id: record.component_type_id.clone(),
        component_type_name: record.component_type_name.clone(),
        description: record.description.clone(),
        extends_from: record.extends_from.clone(),
        is_singleton: record.is_singleton,
        property_definitions: record.property_definitions.clone(),
        functions: record.functions.clone(),
        property_groups: record
            .property_groups
            .iter()
            .map(|(name, group)| (name.clone(), property_group_request_of(group)))
            .collect(),
        composite_component_types: record.composite_component_types.clone(),
        tags: record.tags.clone(),
    }
}

/// The stored entity as the document describes it, the mirror of
/// [`component_type_entry_of`].
///
/// A description the create resolved to the empty string is written as the
/// empty string it is, and the root parent ([`ROOT_PARENT_ENTITY_ID`]) is not
/// written at all: it is what an entity with no parent carries, and a document
/// entry that named it would describe a parent no workspace holds.
pub fn entity_entry_of(record: &Entity) -> EntityEntry {
    EntityEntry {
        entity_id: record.entity_id.clone(),
        entity_name: record.entity_name.clone(),
        description: Some(record.description.clone()),
        parent_entity_id: (record.parent_entity_id != ROOT_PARENT_ENTITY_ID)
            .then(|| record.parent_entity_id.clone()),
        components: record
            .components
            .iter()
            .map(|(name, component)| (name.clone(), component_entry_of(component)))
            .collect(),
        tags: record.tags.clone(),
    }
}

/// One stored component as the document describes it.
fn component_entry_of(record: &ComponentRecord) -> ComponentEntry {
    ComponentEntry {
        // Always written: a stored component instantiates a component type
        // (`crate::state::component_type_id_required_message`), and the entry a
        // document carries is what another workspace re-creates it from.
        component_type_id: Some(record.component_type_id.clone()),
        description: record.description.clone(),
        properties: record
            .properties
            .iter()
            .map(|(name, property)| {
                (
                    name.clone(),
                    crate::ops::types::PropertyRequest {
                        definition: property.definition.clone(),
                        // The value the CONTROL plane set, which is the value a
                        // create or an update of the component wrote. The
                        // time-series values a property carries are data-plane
                        // writes rather than metadata, so they are no part of a
                        // metadata document.
                        value: property.value.clone(),
                        update_type: None,
                    },
                )
            })
            .collect(),
        property_groups: record
            .property_groups
            .iter()
            .map(|(name, group)| (name.clone(), property_group_request_of(group)))
            .collect(),
    }
}

/// One stored property group as the modelled request shape a document carries —
/// the inverse of [`PropertyGroupRecord::from_request`].
fn property_group_request_of(record: &PropertyGroupRecord) -> crate::ops::types::PropertyGroupRequest {
    crate::ops::types::PropertyGroupRequest {
        group_type: record.group_type.clone(),
        property_names: present(record.property_names.clone()),
    }
}

/// The records a document entry describes, for a writer that lands it in a
/// workspace: the properties and property groups of one component.
///
/// `component_type_id` is the component type the entry named: a component IS an
/// instance of one, so the writer resolves it (and fails the entry that names
/// none) before building the record.
pub fn component_record_of(
    component_name: &str,
    component_type_id: &str,
    entry: &ComponentEntry,
) -> ComponentRecord {
    ComponentRecord {
        component_name: component_name.to_string(),
        component_type_id: component_type_id.to_string(),
        description: entry.description.clone(),
        properties: entry
            .properties
            .iter()
            .map(|(property_name, property)| {
                let mut record = PropertyRecord {
                    definition: property.definition.clone(),
                    ..PropertyRecord::default()
                };
                if let Some(value) = property.value.clone() {
                    // Through the writer, so the control plane is recorded as
                    // the plane that wrote this property's value, exactly as a
                    // `CreateEntity` of the same component records it.
                    record.set_value(value);
                }
                (property_name.clone(), record)
            })
            .collect(),
        property_groups: entry
            .property_groups
            .iter()
            .map(|(group_name, group)| {
                (
                    group_name.clone(),
                    PropertyGroupRecord::from_request(group.clone()),
                )
            })
            .collect(),
        // A document describes no composite component: see [`EntityEntry`].
        composite_components: BTreeMap::new(),
    }
}

/// The property GROUPS a component type entry declares, as the records the
/// store holds — one conversion
/// ([`PropertyGroupRecord::from_request`]) for every writer of a component
/// type's groups, a transferred one included.
pub fn property_group_records_of(
    entry: &ComponentTypeEntry,
) -> BTreeMap<String, PropertyGroupRecord> {
    entry
        .property_groups
        .iter()
        .map(|(group_name, group)| {
            (
                group_name.clone(),
                PropertyGroupRecord::from_request(group.clone()),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a document reads back as the entries it was rendered from — the
    /// codec's two halves are inverses — over entries that differ along what a
    /// document describes: a component type's declarations, an entity's
    /// components, the values a property carries and the nesting of one.
    #[test]
    fn a_rendered_document_reads_back_as_the_same_entries() {
        let rows: Vec<(&str, MetadataDocument)> = vec![
            ("an empty document", MetadataDocument::default()),
            (
                "a component type with one definition",
                MetadataDocument {
                    component_types: vec![ComponentTypeEntry {
                        component_type_id: "com.example.pump".to_string(),
                        component_type_name: Some("Pump".to_string()),
                        description: Some("a pump".to_string()),
                        is_singleton: true,
                        property_definitions: BTreeMap::from([(
                            "pressure".to_string(),
                            crate::ops::types::PropertyDefinitionRequest {
                                data_type: Some(crate::ops::types::DataType {
                                    r#type: "DOUBLE".to_string(),
                                    unit_of_measure: Some("bar".to_string()),
                                    ..crate::ops::types::DataType::default()
                                }),
                                is_time_series: Some(true),
                                ..crate::ops::types::PropertyDefinitionRequest::default()
                            },
                        )]),
                        ..ComponentTypeEntry::default()
                    }],
                    entities: Vec::new(),
                },
            ),
            (
                "an entity whose component carries a nested value",
                MetadataDocument {
                    component_types: Vec::new(),
                    entities: vec![EntityEntry {
                        entity_id: "pump1".to_string(),
                        entity_name: "Pump 1".to_string(),
                        description: Some(String::new()),
                        parent_entity_id: Some("site1".to_string()),
                        components: BTreeMap::from([(
                            "pump".to_string(),
                            ComponentEntry {
                                component_type_id: Some("com.example.pump".to_string()),
                                properties: BTreeMap::from([(
                                    "readings".to_string(),
                                    crate::ops::types::PropertyRequest {
                                        value: Some(crate::ops::types::DataValue {
                                            list_value: Some(vec![
                                                crate::ops::types::DataValue {
                                                    integer_value: Some(7),
                                                    ..crate::ops::types::DataValue::default()
                                                },
                                            ]),
                                            ..crate::ops::types::DataValue::default()
                                        }),
                                        ..crate::ops::types::PropertyRequest::default()
                                    },
                                )]),
                                ..ComponentEntry::default()
                            },
                        )]),
                        tags: BTreeMap::from([("line".to_string(), "a".to_string())]),
                    }],
                },
            ),
        ];
        for (case, document) in rows {
            let rendered = render(&document);
            let bytes = ::serde_json::to_vec(&rendered).expect("the document serializes");
            // Compared as the DOCUMENT the codec writes rather than through the
            // entries themselves: the entries carry the modelled request
            // shapes, which the model does not make comparable, and what has to
            // round trip is exactly what the document says.
            let read = parse(&bytes).unwrap_or_else(|reason| {
                panic!("case {case:?}: the rendered document must read back, got {reason:?}")
            });
            let again = render(&read);
            assert_eq!(
                again, rendered,
                "case {case:?}: expected the same document back, got {again}"
            );
        }
    }

    /// RULE: bytes that are no metadata document are refused with the reason
    /// they are not, and nothing is read out of them.
    #[test]
    fn bytes_that_describe_no_document_are_refused() {
        let rows: Vec<(&str, Vec<u8>)> = vec![
            ("empty bytes", Vec::new()),
            ("not JSON at all", b"componentTypes".to_vec()),
            ("a JSON list", b"[]".to_vec()),
            ("an object naming neither member", b"{\"other\": 1}".to_vec()),
            (
                "componentTypes that is not a list",
                b"{\"componentTypes\": {}}".to_vec(),
            ),
            (
                "an entities entry that is not an object",
                b"{\"entities\": [1]}".to_vec(),
            ),
            (
                "a component type naming no id",
                b"{\"componentTypes\": [{}]}".to_vec(),
            ),
            (
                "an entity naming no name",
                b"{\"entities\": [{\"entityId\": \"pump1\"}]}".to_vec(),
            ),
        ];
        for (case, bytes) in rows {
            let read = parse(&bytes);
            assert!(
                read.is_err(),
                "case {case:?}: expected a refusal, got {read:?}"
            );
        }
    }
}
