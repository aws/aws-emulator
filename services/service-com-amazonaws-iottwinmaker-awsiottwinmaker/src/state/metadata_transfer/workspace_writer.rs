// SPDX-License-Identifier: CC0-1.0

//! Landing a metadata document's entries in a workspace: the DESTINATION side
//! of an IoT TwinMaker end of a transfer.
//!
//! One entry is one resource, and an entry lands only if the workspace could
//! have been asked to create that resource directly: every rule this writer
//! applies is the rule `CreateComponentType` and `CreateEntity` apply to a
//! caller's own request — the same reserved namespace, the same reference
//! resolution, the same `dataType` and value rules, the same ceilings — reached
//! through the shared functions on [`crate::state`] rather than restated here.
//! So a transfer cannot put into a workspace something the control plane would
//! have refused, and what it reports as written is what the workspace holds
//! afterwards.
//!
//! What an entry comes to is one of three things, which are exactly the counts
//! `MetadataTransferJobProgress` reports: it was WRITTEN, it was SKIPPED
//! because the workspace already holds that id (a transfer run twice is not a
//! transfer that fails), or it FAILED, carrying the reason the rule that refused
//! it produced. A failed entry fails the entry, not the run: the run walks every
//! entry and the job reports the counts.
//!
//! ORDER: component types first, then entities, because an entity's component
//! instantiates a component type the document may itself describe. Within the
//! entities, a parent lands before its children — the document lists them in no
//! particular order, so the writer takes the entries whose parent is already in
//! the workspace and repeats until nothing more can land, which leaves an entry
//! whose parent nothing could write failing for the parent it names.

use super::*;

/// Write every entry of `document` into the workspace `workspace_id` of
/// `state`, and report what became of each.
///
/// `now` is the instant the run is performed at: a component type lands in the
/// non-terminal `CREATING` state its own create answers with and carries the
/// instant its transition to `ACTIVE` falls due, exactly as one created
/// directly does, and an entity lands `ACTIVE` as its own create leaves it.
pub fn write_document_into_workspace(
    state: &mut RegionalState,
    region: &str,
    workspace_id: &str,
    document: &MetadataDocument,
    now: SystemTime,
) -> TransferReport {
    let mut report = TransferReport::default();
    for entry in &document.component_types {
        write_component_type(state, region, workspace_id, entry, now, &mut report);
    }
    // The entities whose parent is not in the workspace yet are kept back and
    // tried again, so a document that lists a child before its parent still
    // lands both; the sweep stops as soon as a pass writes nothing, which is
    // when the parents that are left will never arrive.
    let mut pending: Vec<&EntityEntry> = document.entities.iter().collect();
    while !pending.is_empty() {
        let mut deferred: Vec<&EntityEntry> = Vec::new();
        let before = report.entries.len();
        for entry in pending {
            if parent_is_absent(state, workspace_id, entry) {
                deferred.push(entry);
                continue;
            }
            write_entity(state, region, workspace_id, entry, now, &mut report);
        }
        if report.entries.len() == before {
            // Nothing landed in this pass, so no parent any of these entries
            // waits for is ever going to be written.
            for entry in deferred {
                report.failed(
                    ENTRY_KIND_ENTITY,
                    &entry.entity_id,
                    parent_not_in_workspace_message(entry, workspace_id),
                );
            }
            return report;
        }
        pending = deferred;
    }
    report
}

/// Whether `entry` names a parent entity the workspace does not hold yet — the
/// one reason this writer keeps an entry back rather than deciding it.
///
/// An entry that names no parent, or names the root
/// ([`ROOT_PARENT_ENTITY_ID`]), hangs off nothing and is never kept back.
fn parent_is_absent(state: &RegionalState, workspace_id: &str, entry: &EntityEntry) -> bool {
    let Some(parent_entity_id) = entry.parent_entity_id.as_deref() else {
        return false;
    };
    if parent_entity_id == ROOT_PARENT_ENTITY_ID || parent_entity_id == entry.entity_id {
        return false;
    }
    state
        .workspaces
        .get(workspace_id)
        .is_none_or(|workspace| !workspace.entities.contains_key(parent_entity_id))
}

/// The reason an entry whose parent never arrived could not be written. Built
/// from the entry and the workspace, so it names what would have to change.
fn parent_not_in_workspace_message(entry: &EntityEntry, workspace_id: &str) -> String {
    format!(
        "parentEntityId {} does not name an entity in workspace {workspace_id}",
        entry.parent_entity_id.as_deref().unwrap_or_default()
    )
}

/// Write one component type entry, or report why it was skipped or failed.
fn write_component_type(
    state: &mut RegionalState,
    region: &str,
    workspace_id: &str,
    entry: &ComponentTypeEntry,
    now: SystemTime,
    report: &mut TransferReport,
) {
    let component_type_id = entry.component_type_id.as_str();
    let Some(workspace) = state.workspaces.get(workspace_id) else {
        report.failed(
            ENTRY_KIND_COMPONENT_TYPE,
            component_type_id,
            workspace_not_found_message(workspace_id),
        );
        return;
    };
    // An id the destination already holds is one this transfer has nothing to
    // do for: the resource is there, so the entry is SKIPPED rather than failed
    // as the conflict a direct create would answer.
    if workspace.component_types.contains_key(component_type_id) {
        report.skipped(
            ENTRY_KIND_COMPONENT_TYPE,
            component_type_id,
            already_exists_message("Component type", component_type_id, Some(workspace_id)),
        );
        return;
    }
    if built_in_component_type_id(component_type_id) {
        report.failed(
            ENTRY_KIND_COMPONENT_TYPE,
            component_type_id,
            reserved_component_type_namespace_message(component_type_id),
        );
        return;
    }
    if workspace.created_component_types().count() >= MAX_COMPONENT_TYPES_PER_WORKSPACE {
        report.failed(
            ENTRY_KIND_COMPONENT_TYPE,
            component_type_id,
            format!(
                "workspace {workspace_id} has reached the limit of \
                 {MAX_COMPONENT_TYPES_PER_WORKSPACE} component types"
            ),
        );
        return;
    }
    if let Some(message) = reserved_tag_keys_violation("tags", entry.tags.keys().map(String::as_str))
    {
        report.failed(ENTRY_KIND_COMPONENT_TYPE, component_type_id, message);
        return;
    }
    let refusal = extends_from_violation(workspace, component_type_id, &entry.extends_from)
        .or_else(|| {
            composite_component_types_violation(
                workspace,
                component_type_id,
                &entry.composite_component_types,
            )
        })
        .or_else(|| {
            property_definitions_violation(
                "propertyDefinitions",
                entry
                    .property_definitions
                    .iter()
                    .map(|(property_name, definition)| (property_name.as_str(), definition)),
            )
        })
        .or_else(|| {
            workspace
                .stranded_property_definition(
                    &entry.extends_from,
                    entry
                        .property_definitions
                        .iter()
                        .map(|(property_name, definition)| (property_name.as_str(), definition)),
                )
                .map(property_definition_needs_data_type_message)
        })
        .or_else(|| {
            workspace.property_definition_default_violation(
                &entry.extends_from,
                entry
                    .property_definitions
                    .iter()
                    .map(|(property_name, definition)| (property_name.as_str(), definition)),
            )
        });
    if let Some(message) = refusal {
        report.failed(ENTRY_KIND_COMPONENT_TYPE, component_type_id, message);
        return;
    }

    let arn = component_type_arn(region, workspace_id, component_type_id);
    let mut record = ComponentType {
        component_type_id: component_type_id.to_string(),
        arn: arn.clone(),
        workspace_id: workspace_id.to_string(),
        component_type_name: entry.component_type_name.clone(),
        description: entry.description.clone(),
        is_singleton: entry.is_singleton,
        // Decided from the record below, never taken from the document: a
        // component type's abstractness is not a member any writer declares.
        is_abstract: false,
        extends_from: entry.extends_from.clone(),
        property_definitions: entry.property_definitions.clone(),
        functions: entry.functions.clone(),
        property_groups: property_group_records_of(entry),
        composite_component_types: entry.composite_component_types.clone(),
        tags: entry.tags.clone(),
        creation_date_time: now,
        update_date_time: now,
        // The transfer wrote the component type; its provisioning window is the
        // one its own create opens, and the clock closes it.
        state: STATE_CREATING.to_string(),
        settles_at: now + COMPONENT_TYPE_SETTLE_SPAN,
    };
    if let Some(message) = property_groups_violation(workspace, &record) {
        report.failed(ENTRY_KIND_COMPONENT_TYPE, component_type_id, message);
        return;
    }
    record.is_abstract = component_type_is_abstract(workspace, &record);

    let Some(workspace) = state.workspaces.get_mut(workspace_id) else {
        report.failed(
            ENTRY_KIND_COMPONENT_TYPE,
            component_type_id,
            workspace_not_found_message(workspace_id),
        );
        return;
    };
    workspace
        .component_types
        .insert(component_type_id.to_string(), record);
    state.arn_index.insert(
        arn,
        ResourceRef::ComponentType {
            workspace_id: workspace_id.to_string(),
            component_type_id: component_type_id.to_string(),
        },
    );
    report.succeeded(ENTRY_KIND_COMPONENT_TYPE, component_type_id);
}

/// Write one entity entry, or report why it was skipped or failed.
fn write_entity(
    state: &mut RegionalState,
    region: &str,
    workspace_id: &str,
    entry: &EntityEntry,
    now: SystemTime,
    report: &mut TransferReport,
) {
    let entity_id = entry.entity_id.as_str();
    let Some(workspace) = state.workspaces.get(workspace_id) else {
        report.failed(
            ENTRY_KIND_ENTITY,
            entity_id,
            workspace_not_found_message(workspace_id),
        );
        return;
    };
    if workspace.entities.contains_key(entity_id) {
        report.skipped(
            ENTRY_KIND_ENTITY,
            entity_id,
            already_exists_message("Entity", entity_id, Some(workspace_id)),
        );
        return;
    }
    if workspace.entities.len() >= MAX_ENTITIES_PER_WORKSPACE {
        report.failed(
            ENTRY_KIND_ENTITY,
            entity_id,
            format!(
                "workspace {workspace_id} has reached the limit of \
                 {MAX_ENTITIES_PER_WORKSPACE} entities"
            ),
        );
        return;
    }
    let parent_entity_id = entry
        .parent_entity_id
        .clone()
        .unwrap_or_else(|| ROOT_PARENT_ENTITY_ID.to_string());
    // An entity cannot be its own parent: that would make the stored parent
    // graph cyclic, which the tree walks over it must never be asked to handle.
    if parent_entity_id == entity_id {
        report.failed(
            ENTRY_KIND_ENTITY,
            entity_id,
            format!("parentEntityId {parent_entity_id} must not name the entity being created"),
        );
        return;
    }
    if let Some(message) = reserved_tag_keys_violation("tags", entry.tags.keys().map(String::as_str))
    {
        report.failed(ENTRY_KIND_ENTITY, entity_id, message);
        return;
    }
    // Every component the entry describes has to be one the workspace could
    // hold: it instantiates a component type that is not abstract, every
    // definition it declares leaves the property with a `dataType` the model
    // defines, every value it carries is a value of that type, and every
    // property group it declares carries a `groupType` the model admits and
    // names properties the component has. The rules are the crate's own,
    // shared with `CreateEntity`, and they are asked of the RECORDS this write
    // would store, which is what the control plane asks them of.
    let components: BTreeMap<String, ComponentRecord> = entry
        .components
        .iter()
        .map(|(component_name, component)| {
            (
                component_name.clone(),
                component_record_of(component_name, component),
            )
        })
        .collect();
    for (component_name, record) in &components {
        // The entry this record was built from, under the same key: the
        // property rule is asked of the REQUEST shapes the entry carries, the
        // group rule of the records the write would store.
        let requested = entry.components.get(component_name);
        let refusal = abstract_component_type_violation(
            workspace,
            record.component_type_id.as_deref(),
        )
        .or_else(|| {
            workspace.component_property_violation(
                record.component_type_id.as_deref(),
                &format!("components.{component_name}"),
                requested.into_iter().flat_map(|component| {
                    component
                        .properties
                        .iter()
                        .map(|(property_name, property)| (property_name.as_str(), property))
                }),
            )
        })
        .or_else(|| {
            component_property_groups_violation(
                workspace,
                &format!("components.{component_name}"),
                record.component_type_id.as_deref(),
                &record.properties,
                &record.property_groups,
            )
        });
        if let Some(message) = refusal {
            report.failed(ENTRY_KIND_ENTITY, entity_id, message);
            return;
        }
    }

    let arn = entity_arn(region, workspace_id, entity_id);
    let record = Entity {
        entity_id: entity_id.to_string(),
        entity_name: entry.entity_name.clone(),
        arn: arn.clone(),
        workspace_id: workspace_id.to_string(),
        // Stored resolved rather than as an absent member, exactly as a create
        // stores it: an entity carries a description, which is the empty string
        // when nothing named one.
        description: entry.description.clone().unwrap_or_default(),
        parent_entity_id,
        components,
        tags: entry.tags.clone(),
        creation_date_time: now,
        update_date_time: now,
        // An entity's create completes inside the call that asks for it, and so
        // does the write of a transferred one: there is no provisioning window
        // to emulate, so the record lands in the terminal ACTIVE state and owes
        // no transition.
        state: STATE_ACTIVE.to_string(),
        settles_at: now,
    };
    if let Some(message) = entity_contents_over_ceiling(&record) {
        report.failed(ENTRY_KIND_ENTITY, entity_id, message);
        return;
    }

    let Some(workspace) = state.workspaces.get_mut(workspace_id) else {
        report.failed(
            ENTRY_KIND_ENTITY,
            entity_id,
            workspace_not_found_message(workspace_id),
        );
        return;
    };
    workspace.entities.insert(entity_id.to_string(), record);
    state.arn_index.insert(
        arn,
        ResourceRef::Entity {
            workspace_id: workspace_id.to_string(),
            entity_id: entity_id.to_string(),
        },
    );
    report.succeeded(ENTRY_KIND_ENTITY, entity_id);
}
