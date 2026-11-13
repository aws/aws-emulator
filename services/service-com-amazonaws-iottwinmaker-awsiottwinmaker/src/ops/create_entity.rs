// SPDX-License-Identifier: CC0-1.0

//! @op CreateEntity
//! Creates an entity in a workspace and persists it in [`crate::state`] under
//! that workspace, so `GetEntity` / `ListEntities` read back what was created.
//! The create COMPLETES here: the entity is stored, and reported, in the
//! terminal `ACTIVE` state, which is the state recorded traffic reads a
//! just-created entity back in.
//!
//! Eleven refusals this handler owns, each a `ValidationException` (the model
//! declares no `ResourceNotFoundException` for `CreateEntity`):
//!
//! * `tags` names a key in the reserved `aws:` namespace
//!   ([`crate::state::RESERVED_TAG_KEY_PREFIX`], matched however it is cased):
//!   that namespace is Amazon Web Services' own, so the whole request is
//!   refused and no entity is created;
//! * the workspace does not exist;
//! * `parentEntityId` names an entity that does not exist in the workspace;
//! * a `components` entry names no `componentTypeId`
//!   ([`crate::state::component_type_id_required_message`]): a component IS an
//!   instance of a component type — that type is where the part of its schema
//!   the entity did not declare comes from, and every read reports the
//!   component as instantiating it — so a component that names none is refused
//!   rather than stored with nothing to report there;
//! * a requested property's definition carries a `dataType` whose `type` is
//!   outside the modelled `Type` value list
//!   ([`crate::state::property_definition_violation`]): the member is
//!   `@required` on the `DataType` a read reports, and a type the model does
//!   not define is also one no `DataValue` could ever satisfy, so such a
//!   definition would leave the property unwritable for good. The boundary's
//!   own value-list check answers a request off the WIRE before this handler
//!   sees it (the reason the sibling handlers of this service restate their
//!   own enumerations' lists too); the rule is stated here because every
//!   writer of a definition shares it, the one that lands a transferred
//!   metadata document included;
//! * a requested property's definition would leave the property with no
//!   `dataType` at all — neither its own nor one inherited from the component
//!   type its holder instantiates
//!   ([`crate::state::Workspace::component_property_definition_needs_data_type`]),
//!   which is a definition no read could report;
//! * a requested property's `value` is not a value of the `dataType` the
//!   property has, its own or the inherited one
//!   ([`crate::state::Workspace::component_property_value_violation`]): the
//!   value of a property is a value of THAT type, so a mistyped one is refused
//!   BEFORE anything is stored rather than left for a read to report a value
//!   against a `dataType` it contradicts — the rule the time-series data plane
//!   holds every value it writes to;
//! * a requested property's definition declares a `defaultValue` that is not a
//!   value of the `dataType` the property has, its own or the inherited one
//!   ([`crate::state::Workspace::component_property_default_violation`]): a
//!   read reports that default AS the property's value
//!   ([`crate::state::latest_property_value`]) for as long as nothing has been
//!   written to the property, so it is a value of the property and is held to
//!   the same rule the `value` above is;
//! * a component — top-level or composite — instantiates a component type the
//!   workspace holds and that type is ABSTRACT
//!   ([`crate::state::ComponentType::is_abstract`]): an abstract component type
//!   has no way to serve the values of a property it declares, so it is
//!   extended rather than instantiated, and a component of one is refused
//!   BEFORE anything is stored;
//! * a requested property GROUP of a component — top-level or composite —
//!   carries a `groupType` outside the modelled `GroupType` value list (the
//!   boundary answers that one on the wire too), or names in `propertyNames` a
//!   property the component does not have
//!   ([`crate::state::component_property_groups_violation`]): a group's
//!   `groupType` is `@required` on the response a read reports, and a group
//!   GROUPS properties of the component, so each name it carries is one of the
//!   component's own properties or one the component type it instantiates
//!   gives it — the rule every writer of a component TYPE's `propertyGroups`
//!   already met ([`crate::state::property_groups_violation`]);
//! * a `compositeComponents` key is not the path of a composite component this
//!   entity has: the path's first segment must name one of the request's
//!   `components`, and each further segment an entry in the
//!   `compositeComponentTypes` of the component type the segment before it
//!   reached (`crate::state::resolve_composite_component_path`). A path that
//!   resolves is ACCEPTED and stored under the component it hangs off, so
//!   `GetEntity` reports it as that component's
//!   `compositeComponents.<name>` with the `componentPath` it was created by.
//!
//! An `entityId` that is already taken in the workspace is a `ConflictException`.
//!
//! A create that would store more components, composite components, properties
//! or property groups on one entity than this emulator's declared ceilings
//! allow is refused with the modelled `ServiceQuotaExceededException` and
//! nothing is stored (`crate::state::entity_contents_over_ceiling`, the
//! ceiling rule `UpdateEntity` applies to the record it patches), beside the
//! per-workspace entity ceiling below.
//!
//! Two deliberate gaps: a `componentTypeId` the workspace does not hold at all
//! is still stored without being refused (the real service refuses an unknown
//! one, but a real workspace carries more built-in component types than this
//! emulator models, so refusing would reject creates the real service accepts —
//! which is also why a composite component path THROUGH a component type this
//! workspace does not hold cannot be resolved and is refused), and
//! authorization is not emulated, so the modelled `AccessDeniedException` is
//! never returned. A component type the workspace DOES hold is resolved: see
//! the abstractness refusal above.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateEntity::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! @generated by the emulator core scaffold op-stub generator.
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `CreateEntity`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json")
//! ```
//!
//! Creates an entity.
//!
//! ## Input
//!
//! * `components` (ComponentsMapRequest) — An object that maps strings to the components in
//!   the entity. Each string in the mapping must be unique to this object.
//! * `compositeComponents` (CompositeComponentsMapRequest) — This is an object that maps
//!   strings to compositeComponent updates in the request. Each key of the map represents the
//!   componentPath of the compositeComponent.
//! * `description` (Description, len 0..2048, pattern .*) — The description of the entity.
//! * `entityId` (EntityId, len 1..128, pattern
//!   ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|^[a-zA-Z0-9][a-zA-Z_\-0-9.:]*[a-zA-Z0-9]+$)
//!   — The ID of the entity.
//! * `entityName` (EntityName, len 1..256, pattern ^[^\u0000-\u001F\u007F]+$) **required** —
//!   The name of the entity.
//! * `parentEntityId` (ParentEntityId, len 1..128, pattern
//!   ^\$ROOT|^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|^[a-zA-Z0-9][a-zA-Z_\-0-9.:]*[a-zA-Z0-9]+$)
//!   — The ID of the entity's parent entity.
//! * `tags` (TagMap, len 0..50) — Metadata that you can use to manage the entity.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace that contains the entity.
//!
//! ## Output
//!
//! * `arn` (TwinMakerArn, len 20..2048, pattern
//!   ^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\/a-zA-Z0-9_\-\.:]+$)
//!   **required** — The ARN of the entity.
//! * `creationDateTime` (Timestamp) **required** — The date and time when the entity was
//!   created.
//! * `entityId` (EntityId, len 1..128, pattern
//!   ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|^[a-zA-Z0-9][a-zA-Z_\-0-9.:]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the entity.
//! * `state` (State, one of: ACTIVE | CREATING | DELETING | ERROR | UPDATING) **required** —
//!   The current state of the entity.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Access is denied.
//! * `ConflictException` (HTTP 409) — A conflict occurred.
//! * `InternalServerException` (HTTP 500) — An unexpected error has occurred.
//! * `ServiceQuotaExceededException` (HTTP 402) — The service quota was exceeded.
//! * `ThrottlingException` (HTTP 429) — The rate exceeds the limit.
//! * `ValidationException` (HTTP 400) — Failed

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `CreateEntityError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateEntityError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iottwinmaker#CreateEntity")]
#[derive(Debug)]
pub enum CreateEntityErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_entity::CreateEntityError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateEntityError = crate::scaffolded::Scaffolded<CreateEntityErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateEntityErrorModeled> for CreateEntityError {
    fn from(modeled: CreateEntityErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateEntity` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateEntity<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateEntity<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateEntity<Eng>
{
    type Input = CreateEntityInput;
    type Output = CreateEntityOutput;
    type Error = CreateEntityError;

    const NAME: &'static str = "CreateEntity";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENTITY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_ENTITY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every call is recorded, successful or refused: the real service logs
        // control-plane access, and this one mutates tenant state.
        ::tracing::info!(
            operation = "CreateEntity",
            workspace_id = %input.workspace_id,
            entity_id = input.entity_id.as_deref().unwrap_or("<generated>"),
            "serving a request"
        );
        // The reserved tag-key namespace is AWS's own: a create may no more put a
        // key under it on the new resource than `TagResource` may add one later,
        // so a request naming one is refused whole and nothing is created.
        if let Some(message) = crate::state::reserved_tag_keys_violation(
            "tags",
            input
                .tags
                .iter()
                .flat_map(|tags| tags.keys().map(String::as_str)),
        ) {
            return Err(validation(message).into());
        }

        // The Region this request is signed for picks the store it writes to,
        // and the Region the entity's ARN carries: the workspace it is created
        // in is the one THIS Region holds under that id.
        let region = crate::state::request_region(ctx.parts());
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateEntityErrorModeled::InternalServerException(InternalServerException {
                message: Some("the iottwinmaker store is unavailable".to_string()),
            })
        })?;
        let mut state = state.lock();
        let state = state.region_mut(&region);

        let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
            return Err(validation(crate::state::workspace_not_found_message(
                &input.workspace_id,
            ))
            .into());
        };

        // An entityId the caller did not supply is minted here, as the uuid the
        // `EntityId` pattern's first alternative describes.
        let entity_id = input
            .entity_id
            .clone()
            .unwrap_or_else(|| ::uuid::Uuid::new_v4().to_string());
        if workspace.entities.contains_key(&entity_id) {
            return Err(CreateEntityErrorModeled::ConflictException(
                ConflictException {
                    message: Some(crate::state::already_exists_message(
                        "Entity",
                        &entity_id,
                        Some(&input.workspace_id),
                    )),
                },
            )
            .into());
        }

        // The emulator's declared ceiling on the store: see
        // `crate::state::MAX_ENTITIES_PER_WORKSPACE`. Checked AFTER the
        // identity conflict above, so re-creating a taken id in a full
        // workspace is still the ConflictException the id deserves.
        if workspace.entities.len() >= crate::state::MAX_ENTITIES_PER_WORKSPACE {
            return Err(CreateEntityErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(format!(
                        "workspace {} has reached the limit of {} entities",
                        input.workspace_id,
                        crate::state::MAX_ENTITIES_PER_WORKSPACE
                    )),
                },
            )
            .into());
        }

        let parent_entity_id = input
            .parent_entity_id
            .clone()
            .unwrap_or_else(|| crate::state::ROOT_PARENT_ENTITY_ID.to_string());
        // An entity cannot be its own parent: that would make the stored parent
        // graph cyclic, which the tree walks over it must never be asked to
        // handle.
        if parent_entity_id == entity_id {
            return Err(validation(format!(
                "parentEntityId {parent_entity_id} must not name the entity being created"
            ))
            .into());
        }
        if parent_entity_id != crate::state::ROOT_PARENT_ENTITY_ID
            && !workspace.entities.contains_key(&parent_entity_id)
        {
            return Err(validation(format!(
                "parentEntityId {parent_entity_id} does not name an entity in workspace {}",
                input.workspace_id
            ))
            .into());
        }

        // Both requested maps are walked by the rules below, so both are taken
        // in KEY order (`crate::state::requested_in_key_order`): which entry a
        // refusal names is decided by the request, not by the iteration order
        // of the hash map it was deserialized into.
        let components = crate::state::requested_in_key_order(input.components);
        let composite_components = crate::state::requested_in_key_order(input.composite_components);
        // Each requested component, in component-name order: the component type
        // it INSTANTIATES, which it has to name
        // ([`crate::state::component_type_id_required_message`]), and then the
        // definitions it declares, which inherit through that type.
        //
        // Every declared property definition must leave the property with the
        // `dataType` the modelled `PropertyDefinitionResponse` requires back —
        // its own, or the one it INHERITS from the component type the component
        // instantiates — and every property VALUE the request carries must be a
        // value of that `dataType` ([`require_property_types`], the rules
        // `crate::state::Workspace::component_property_definition_needs_data_type`
        // and `crate::state::Workspace::component_property_value_violation`
        // hold for every writer of a component's properties). Walked in
        // component-name then component-path order, each component's properties
        // in property-name order.
        let mut component_records: ::std::collections::BTreeMap<
            String,
            crate::state::ComponentRecord,
        > = ::std::collections::BTreeMap::new();
        for (component_name, component) in components {
            let member = format!("components.{component_name}");
            // A component IS an instance of a component type, so a create that
            // names none has no component to store: the id is what gives the
            // component the schema a read reports it with, and what every read
            // reports it as instantiating.
            let Some(component_type_id) = component.component_type_id.clone() else {
                return Err(validation(crate::state::component_type_id_required_message(
                    &member,
                ))
                .into());
            };
            require_property_types(
                workspace,
                Some(&component_type_id),
                &member,
                component.properties.as_ref(),
            )?;
            let record = component_record(component_name.clone(), component_type_id, component);
            component_records.insert(component_name, record);
        }

        // A property GROUP of a component carries a `groupType` the model's
        // `GroupType` enumeration admits and GROUPS properties the component
        // has — its own or the ones the component type it instantiates gives it
        // (`crate::state::component_property_groups_violation`, the
        // component-level counterpart of the rule every writer of a component
        // TYPE's `propertyGroups` meets). Asked of the records as this create
        // would store them, so a group may name a property the same request
        // declares, and walked in component-name order so which component a
        // refusal names does not depend on the request map's iteration order.
        for (component_name, component) in &component_records {
            if let Some(message) = crate::state::component_property_groups_violation(
                workspace,
                &format!("components.{component_name}"),
                Some(component.component_type_id.as_str()),
                &component.properties,
                &component.property_groups,
            ) {
                return Err(validation(message).into());
            }
        }

        // A composite component inherits from the component type the
        // declaration its path resolves to names, so the rule above is applied
        // to the path's own component type. The resolution is the one the loop
        // below REFUSES an unresolvable path with
        // (`crate::state::resolve_composite_component_path`); taken leniently
        // here, so this pass decides nothing but the `dataType` rule and a path
        // that resolves to no component type simply inherits nothing.
        for (component_path, composite) in &composite_components {
            let resolved = crate::state::resolve_composite_component_path(
                &component_records,
                &workspace.component_types,
                "compositeComponents",
                component_path,
            )
            .ok();
            require_property_types(
                workspace,
                resolved
                    .as_ref()
                    .map(|resolved| resolved.component_type_id.as_str()),
                &format!("compositeComponents.{component_path}"),
                composite.properties.as_ref(),
            )?;
        }

        // A component INSTANTIATES the component type its `componentTypeId`
        // names, so that type has to be one that can be instantiated at all.
        // An ABSTRACT component type cannot be: it has no way to serve the
        // values of some property it declares, which is what makes it a base
        // type to extend rather than one to hold a component of. Walked in
        // component-name order (the records are keyed by it), so which
        // component a refusal names does not depend on the request map's
        // iteration order, and walked BEFORE anything is stored.
        for component in component_records.values() {
            if let Some(refusal) = abstract_component_type_refusal(
                workspace,
                Some(component.component_type_id.as_str()),
            ) {
                return Err(refusal.into());
            }
        }

        // A `compositeComponents` entry is accepted exactly when its key
        // RESOLVES as a composite component path: one of this entity's
        // components, then one `compositeComponentTypes` declaration of the
        // component type reached so far per further segment
        // (`crate::state::resolve_composite_component_path`). Walked in path
        // order, so which unresolvable path a refusal names does not depend on
        // the request map's iteration order.
        for (component_path, composite) in composite_components {
            let resolved = crate::state::resolve_composite_component_path(
                &component_records,
                &workspace.component_types,
                "compositeComponents",
                &component_path,
            )
            .map_err(validation)?;
            // A composite component instantiates the component type the
            // declaration its path resolved to names, so the same abstractness
            // rule holds for it as for a top-level component.
            if let Some(refusal) =
                abstract_component_type_refusal(workspace, Some(&resolved.component_type_id))
            {
                return Err(refusal.into());
            }
            let record = crate::state::CompositeComponentRecord {
                component_path: component_path.clone(),
                component_name: resolved.composite_name,
                component_type_id: resolved.component_type_id,
                description: composite.description,
                properties: property_records(composite.properties.unwrap_or_default()),
                property_groups: property_group_records(
                    composite.property_groups.unwrap_or_default(),
                ),
            };
            // A composite component's groups are held to the same rule the
            // top-level components' are, against the component type the
            // declaration its path resolved to names.
            if let Some(message) = crate::state::component_property_groups_violation(
                workspace,
                &format!("compositeComponents.{component_path}"),
                Some(&record.component_type_id),
                &record.properties,
                &record.property_groups,
            ) {
                return Err(validation(message).into());
            }
            // The holding component is the one the path's first segment was
            // resolved against, so it is present.
            if let Some(component) = component_records.get_mut(&resolved.component_name) {
                component
                    .composite_components
                    .insert(resolved.composite_path, record);
            }
        }

        let arn = crate::state::entity_arn(&region, &input.workspace_id, &entity_id);
        let now = ::std::time::SystemTime::now();
        let entity = crate::state::Entity {
            entity_id: entity_id.clone(),
            entity_name: input.entity_name,
            arn: arn.clone(),
            workspace_id: input.workspace_id.clone(),
            // Stored resolved rather than as an absent member: an entity
            // carries a description, which is the empty string when the create
            // names none, and every read reports it.
            description: input.description.unwrap_or_default(),
            parent_entity_id,
            components: component_records,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            creation_date_time: now,
            update_date_time: now,
            // The create completes here: the entity is stored in the terminal
            // ACTIVE state and every read reports that stored state. There is
            // no provisioning window to emulate — recorded traffic reads a
            // just-created entity back as ACTIVE.
            state: crate::state::STATE_ACTIVE.to_string(),
            // No transition is owed: the state above is terminal, so the
            // instant one would fall due is this call's own. An UPDATE is what
            // opens a window (`crate::state::Entity::begin_update`).
            settles_at: now,
        };
        let created_state = entity.state.clone();

        // The emulator's declared ceilings on ONE entity's contents
        // (`crate::state::entity_contents_over_ceiling`), applied to the record
        // this create is about to store, so the components and properties a
        // single call carries are bounded exactly as `UpdateEntity`'s are.
        if let Some(message) = crate::state::entity_contents_over_ceiling(&entity) {
            return Err(CreateEntityErrorModeled::ServiceQuotaExceededException(
                ServiceQuotaExceededException {
                    message: Some(message),
                },
            )
            .into());
        }

        // Store the entity BEFORE indexing its ARN, so the index can never name
        // an entity that was not stored: the workspace is re-borrowed mutably
        // here (the lookup that validated the request held it immutably) and
        // that re-borrow is the last fallible step.
        let Some(workspace) = state.workspaces.get_mut(&input.workspace_id) else {
            return Err(CreateEntityErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(format!("workspace {} vanished", input.workspace_id)),
                },
            )
            .into());
        };
        workspace.entities.insert(entity_id.clone(), entity);
        state.arn_index.insert(
            arn.clone(),
            crate::state::ResourceRef::Entity {
                workspace_id: input.workspace_id.clone(),
                entity_id: entity_id.clone(),
            },
        );

        Ok(CreateEntityOutput {
            entity_id,
            arn,
            creation_date_time: crate::state::timestamp(now),
            state: created_state,
        })
    }
}

/// Turn one `ComponentsMapRequest` entry into the stored component record.
///
/// The map key is the component name (`ComponentsMapRequest`'s key is `Name`),
/// so it is carried onto the record rather than left only in the map, and
/// `GetEntity` reports it as `ComponentResponse::componentName`.
/// `component_type_id` is the component type the entry named — required of
/// every component a write brings into being, so it is resolved by the caller
/// before anything is built.
///
/// The record comes back with no composite components: those are addressed by a
/// path that has to be resolved against the workspace's component types, which
/// the handler does once the components are in hand.
fn component_record(
    component_name: String,
    component_type_id: String,
    request: ComponentRequest,
) -> crate::state::ComponentRecord {
    crate::state::ComponentRecord {
        component_name,
        component_type_id,
        description: request.description,
        properties: property_records(request.properties.unwrap_or_default()),
        property_groups: property_group_records(request.property_groups.unwrap_or_default()),
        composite_components: ::std::collections::BTreeMap::new(),
    }
}

/// A requested property map as the stored property records.
///
/// Shared by a component and a composite component, which carry the same
/// modelled `PropertyRequests` map, so one shape has one conversion.
fn property_records(
    properties: ::std::collections::HashMap<String, PropertyRequest>,
) -> ::std::collections::BTreeMap<String, crate::state::PropertyRecord> {
    properties
        .into_iter()
        .map(|(property_name, property)| {
            let mut record = crate::state::PropertyRecord {
                definition: property.definition,
                // A create carries no time-series value: those arrive through
                // the time-series data plane (`BatchPutPropertyValues`), which
                // writes them into the property record this one stores.
                ..crate::state::PropertyRecord::default()
            };
            if let Some(value) = property.value {
                // Through the writer, so the control plane is recorded as the
                // plane that wrote this property's value and every read reports
                // what the create set.
                record.set_value(value);
            }
            (property_name, record)
        })
        .collect()
}

/// A requested `ComponentPropertyGroupRequests` map as the stored records.
///
/// Shared by a component and a composite component for the same reason
/// [`property_records`] is. `propertyNames` is stored RESOLVED, so an absent
/// list is an empty one rather than a missing collection every reader has to
/// guard against.
fn property_group_records(
    property_groups: ::std::collections::HashMap<String, ComponentPropertyGroupRequest>,
) -> ::std::collections::BTreeMap<String, crate::state::PropertyGroupRecord> {
    property_groups
        .into_iter()
        .map(|(group_name, group)| {
            (
                group_name,
                crate::state::PropertyGroupRecord {
                    group_type: group.group_type,
                    property_names: group.property_names.unwrap_or_default(),
                },
            )
        })
        .collect()
}

/// Why a component naming `component_type_id` cannot be created — `None` when
/// it can.
///
/// The rule itself is [`crate::state::abstract_component_type_violation`],
/// which every writer of an entity's components shares; this lifts the message
/// it produces into the refusal THIS operation answers with.
///
/// Shared by the top-level components and the composite ones, so one rule is
/// applied once to every component a create writes.
fn abstract_component_type_refusal(
    workspace: &crate::state::Workspace,
    component_type_id: Option<&str>,
) -> Option<CreateEntityErrorModeled> {
    crate::state::abstract_component_type_violation(workspace, component_type_id).map(validation)
}

/// This operation's `ValidationException`, carrying the message the rule that
/// refused the request produced.
///
/// The message is clamped to the modelled maximum of the member it is reported
/// on ([`crate::state::clamped_message`]): a refusal names what the request
/// named, and this operation's `compositeComponents` is keyed by a
/// `ComponentPath` the model bounds as widely as that member itself, so the
/// rendered reason is brought within the bound instead of overrunning it.
fn validation(message: String) -> CreateEntityErrorModeled {
    CreateEntityErrorModeled::ValidationException(ValidationException {
        message: Some(crate::state::clamped_message(
            message,
            crate::state::ERROR_MESSAGE_MAX_CHARS,
        )),
    })
}

/// Refuse a requested property whose definition would leave the property with
/// no `dataType` at all, and one whose VALUE is not a value of the `dataType`
/// the property has.
///
/// `component_type_id` is the component type the properties' holder — a
/// component or a composite component — instantiates, which is where a
/// definition that carries no `dataType` of its own inherits one from, and
/// where a property the request declares nothing about takes its `dataType`
/// from. Both rules are the crate's own, shared with every other writer of a
/// component's properties: a definition no read could report a `dataType` for
/// is `crate::state::Workspace::component_property_definition_needs_data_type`
/// (so a definition that REFINES an inherited one — every member of
/// `PropertyDefinitionRequest` is optional — is accepted here exactly where a
/// read can report it), and a value of the wrong type is
/// `crate::state::Workspace::component_property_value_violation`, the rule the
/// time-series data plane holds every value it writes to
/// (`crate::ops::batch_put_property_values`). A value is judged against the
/// `dataType` a read REPORTS for the property, so one supplied against a
/// definition the component INHERITS is judged too — and the `defaultValue` a
/// declaration carries is judged by the same rule
/// (`crate::state::Workspace::component_property_default_violation`), since a
/// read reports it as the property's value while nothing has been written to
/// the property.
///
/// `member_path` is the request member the map hangs off (`components.<name>`,
/// `compositeComponents.<path>`), so the refusal names the exact member the
/// caller has to fix.
///
/// The requested properties are walked in property-NAME order, so a request
/// carrying more than one property the rules refuse is always refused for
/// the first of them in key order rather than for whichever one the hash map's
/// iteration order reached first.
fn require_property_types(
    workspace: &crate::state::Workspace,
    component_type_id: Option<&str>,
    member_path: &str,
    properties: Option<&::std::collections::HashMap<String, PropertyRequest>>,
) -> Result<(), CreateEntityErrorModeled> {
    let properties: ::std::collections::BTreeMap<&String, &PropertyRequest> =
        properties.into_iter().flatten().collect();
    match workspace.component_property_violation(
        component_type_id,
        member_path,
        properties
            .into_iter()
            .map(|(property_name, property)| (property_name.as_str(), property)),
    ) {
        Some(violation) => Err(validation(violation)),
        None => Ok(()),
    }
}
