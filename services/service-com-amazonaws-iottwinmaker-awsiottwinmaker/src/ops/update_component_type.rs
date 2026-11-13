// SPDX-License-Identifier: CC0-1.0

//! @op UpdateComponentType
//! Updates information in a component type stored in [`crate::state`], so
//! `GetComponentType` reads back what the update persisted.
//!
//! The update is a PATCH over the request's top-level members: every member it
//! NAMES replaces the stored value for that member, and every member it does
//! not name keeps the value already persisted. A named member is replaced
//! WHOLE whatever its shape — a map member (`propertyDefinitions`,
//! `functions`, `propertyGroups`, `compositeComponentTypes`) is a value, not a
//! collection of separately addressable entries, so the entries the request
//! carries are the entries the component type ends up with and one it leaves
//! out is DROPPED. The API models no per-entry "remove this key", so replacing
//! the map is the only way a caller can shorten one, and the recorded traffic
//! observes exactly that: a property definition left out of an update's
//! `propertyDefinitions` is gone from the component type afterwards.
//!
//! Like `CreateComponentType`, this update does NOT complete within the call:
//! the `state` it answers is the non-terminal `UPDATING`, and the component
//! type settles back in `ACTIVE` once
//! [`crate::state::COMPONENT_TYPE_SETTLE_SPAN`] has elapsed — an instant stored
//! on the record and materialised by the next reader, never slept for.
//!
//! The refusals this handler owns:
//!
//! * the workspace or the component type the labels name does not exist — the
//!   modelled `ResourceNotFoundException` (HTTP 404), the same reading
//!   `GetComponentType` gives the same two labels, since both address a
//!   resource that must already exist;
//! * the patched component type would hold a property definition whose
//!   `dataType` carries a `type` outside the modelled `Type` value list, or a
//!   property group whose `groupType` is outside the modelled `GroupType` list
//!   — a `ValidationException`, the rules
//!   [`crate::state::property_definitions_violation`] and
//!   [`crate::state::property_groups_violation`] hold for every writer of those
//!   members, because the model admits only the values its enumerations define
//!   on members a read reports as required. Checked over the whole patched
//!   record, as the two rules below are;
//! * the patched component type would hold a property definition with no
//!   `dataType` that inherits none — it extends nothing, or no component type
//!   on its `extendsFrom` chain declares that property with a `dataType` — a
//!   `ValidationException`, the rule
//!   [`crate::state::Workspace::stranded_property_definition`] holds for every
//!   writer of a definition. Checked over the WHOLE patched record, because an
//!   update that clears or re-points `extendsFrom` takes away the inheritance
//!   an older definition was relying on, and asked whenever the request touches
//!   either input of the rule;
//! * the patched component type would hold a property definition whose
//!   `defaultValue` carries a value of a type other than the `dataType` the
//!   definition resolves to — a `ValidationException`, the rule
//!   [`crate::state::Workspace::property_definition_default_violation`] holds
//!   for every writer of a definition, because a read reports that default AS
//!   the property's value ([`crate::state::latest_property_value`]) beside the
//!   `dataType` it contradicts. Checked over the whole patched record, and
//!   whenever the request touches either input of the rule, exactly as the
//!   `dataType` rule above is;
//! * a reference the update DECLARES does not resolve — an `extendsFrom` or
//!   `compositeComponentTypes[*].componentTypeId` id naming a component type
//!   the service does not hold, inheritance that reaches back to the component
//!   type being updated, or a `propertyGroups[*].propertyNames` entry naming a
//!   property the patched component type neither defines nor inherits — a
//!   `ValidationException` (HTTP 400), the rules
//!   [`crate::state::extends_from_violation`],
//!   [`crate::state::composite_component_types_violation`] and
//!   [`crate::state::property_groups_violation`] hold for every writer of
//!   those members. A declared id resolves to a component type stored in the
//!   workspace or to one of the service's built-in types (see
//!   [`crate::state::BUILT_IN_COMPONENT_TYPE_ID_PREFIX`]). The two ID members
//!   are resolved only where this request NAMES them, since a reference an
//!   earlier write left dangling is not this call's to refuse; the property
//!   GROUPS are checked over the whole patched record whenever the request
//!   touches what their names resolve against — the groups, the own
//!   definitions, or the inheritance the inherited ones come through — because
//!   a patch that drops a property a stored group names is the call that
//!   strands that group.
//!
//! A component type the SERVICE ships is not a caller's to change: a workspace
//! is provisioned with the built-ins
//! ([`crate::state::built_in_component_types`]), so a patch addressed to one is
//! refused with the modelled `ValidationException` (HTTP 400).
//!
//! Authorization is not emulated, so the modelled `AccessDeniedException` is
//! never returned. No quota applies to a call that creates nothing, so the
//! modelled `ServiceQuotaExceededException` is not returned either.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `UpdateComponentType::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `UpdateComponentType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json")
//! ```
//!
//! Updates information in a component type.
//!
//! ## Input
//!
//! * `componentTypeId` (ComponentTypeId, len 1..256, pattern ^[a-zA-Z_\.\-0-9:]+$)
//!   **required** — The ID of the component type.
//! * `componentTypeName` (ComponentTypeName, len 0..256, pattern [^\u0000-\u001F\u007F]*) —
//!   The component type name.
//! * `compositeComponentTypes` (CompositeComponentTypesRequest) — This is an object that maps
//!   strings to compositeComponentTypes of the componentType. CompositeComponentType is
//!   referenced by componentTypeId.
//! * `description` (Description, len 0..2048, pattern .*) — The description of the component
//!   type.
//! * `extendsFrom` (ExtendsFrom) — Specifies the component type that this component type
//!   extends.
//! * `functions` (FunctionsRequest) — An object that maps strings to the functions in the
//!   component type. Each string in the mapping must be unique to this object.
//! * `isSingleton` (Boolean) — A Boolean value that specifies whether an entity can have more
//!   than one component of this type.
//! * `propertyDefinitions` (PropertyDefinitionsRequest) — An object that maps strings to the
//!   property definitions in the component type. Each string in the mapping must be unique to
//!   this object.
//! * `propertyGroups` (PropertyGroupsRequest) — The property groups.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace.
//!
//! ## Output
//!
//! * `arn` (TwinMakerArn, len 20..2048, pattern
//!   ^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\/a-zA-Z0-9_\-\.:]+$)
//!   **required** — The ARN of the component type.
//! * `componentTypeId` (ComponentTypeId, len 1..256, pattern ^[a-zA-Z_\.\-0-9:]+$)
//!   **required** — The ID of the component type.
//! * `state` (State, one of: ACTIVE | CREATING | DELETING | ERROR | UPDATING) **required** —
//!   The current state of the component type.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace that contains the component type.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — Access is denied.
//! * `InternalServerException` (HTTP 500) — An unexpected error has occurred.
//! * `ResourceNotFoundException` (HTTP 404) — The resource wasn't found.
//! * `ServiceQuotaExceededException` (HTTP 402) — The service quota was exceeded.
//! * `ThrottlingException` (HTTP 429) — The rate exceeds the limit.
//! * `ValidationException` (HTTP 400) — Failed

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateComponentTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateComponentTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iottwinmaker#UpdateComponentType")]
#[derive(Debug)]
pub enum UpdateComponentTypeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_component_type::UpdateComponentTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateComponentTypeError = crate::scaffolded::Scaffolded<UpdateComponentTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateComponentTypeErrorModeled> for UpdateComponentTypeError {
    fn from(modeled: UpdateComponentTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateComponentType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateComponentType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateComponentType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateComponentType<Eng>
{
    type Input = UpdateComponentTypeInput;
    type Output = UpdateComponentTypeOutput;
    type Error = UpdateComponentTypeError;

    const NAME: &'static str = "UpdateComponentType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_COMPONENT_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_COMPONENT_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every call is recorded, successful or refused: the real service logs
        // control-plane access, and this one mutates tenant state.
        ::tracing::info!(
            operation = "UpdateComponentType",
            workspace_id = %input.workspace_id,
            component_type_id = %input.component_type_id,
            "serving a request"
        );
        // The Region this request is signed for picks the store it acts on: a
        // component type created through another Region's endpoint is not in
        // it, so this call cannot update it.
        let region = crate::state::request_region(ctx.parts());
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateComponentTypeErrorModeled::InternalServerException(InternalServerException {
                message: Some("the iottwinmaker store is unavailable".to_string()),
            })
        })?;
        let mut state = state.lock();
        let state = state.region_mut(&region);

        // Both labels address an EXISTING resource, so neither naming something
        // the store does not hold is anything but a 404 — the same reading
        // `GetComponentType` gives the same two labels.
        let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
            return Err(UpdateComponentTypeErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::workspace_not_found_message(
                        &input.workspace_id,
                    )),
                },
            )
            .into());
        };
        let Some(stored) = workspace.component_types.get(&input.component_type_id) else {
            return Err(UpdateComponentTypeErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::component_type_not_found_message(
                        &input.workspace_id,
                        &input.component_type_id,
                    )),
                },
            )
            .into());
        };

        let now = ::std::time::SystemTime::now();

        // A built-in component type is the service's own, provisioned with the
        // workspace: no caller's patch applies to it. Refused AFTER the two
        // lookups above, so an id in the reserved namespace the store does not
        // hold is still the 404 it deserves.
        if crate::state::built_in_component_type_id(&input.component_type_id) {
            return Err(validation_exception(
                crate::state::built_in_component_type_not_writable_message(
                    &input.workspace_id,
                    &input.component_type_id,
                ),
            ));
        }

        // The patch is built on a COPY, so a request refused below leaves the
        // stored record exactly as it was. The clock's transition is applied to
        // that copy first: the update applies to the record as it stands at
        // this instant, not as the previous call left it. The service models no
        // conflict for an update that arrives while a create or an earlier
        // update is still in flight, so one is applied rather than refused; it
        // simply reopens the window.
        let mut updated = stored.clone();
        updated.settle(now);
        // Which reference-carrying members this request DECLARES, captured
        // before the patch moves them onto the copy: the ids and names a call
        // declares are the ones it has to be able to resolve.
        let declares_extends_from = input.extends_from.is_some();
        let declares_property_definitions = input.property_definitions.is_some();
        let declares_composite_component_types = input.composite_component_types.is_some();
        let declares_property_groups = input.property_groups.is_some();
        if let Some(component_type_name) = input.component_type_name {
            updated.component_type_name = Some(component_type_name);
        }
        if let Some(description) = input.description {
            updated.description = Some(description);
        }
        if let Some(is_singleton) = input.is_singleton {
            updated.is_singleton = is_singleton;
        }
        // A member the request NAMES is replaced WHOLE, whatever its shape: a
        // map member is a value like a scalar or a list, not a collection of
        // separately addressable entries, so the entries the request carries
        // are the entries the component type ends up with and one it leaves out
        // is dropped. That is the only way the API can shorten a map at all —
        // it models no per-entry "remove this key" — and dropping a property
        // definition by leaving it out is exactly what the recorded traffic
        // observes.
        if let Some(extends_from) = input.extends_from {
            updated.extends_from = extends_from;
        }
        if let Some(property_definitions) = input.property_definitions {
            updated.property_definitions = property_definitions.into_iter().collect();
        }
        if let Some(functions) = input.functions {
            updated.functions = functions.into_iter().collect();
        }
        if let Some(property_groups) = input.property_groups {
            updated.property_groups = property_groups
                .into_iter()
                .map(|(group_name, group)| {
                    (
                        group_name,
                        crate::state::PropertyGroupRecord::from_request(group),
                    )
                })
                .collect();
        }
        if let Some(composite_component_types) = input.composite_component_types {
            updated.composite_component_types = composite_component_types.into_iter().collect();
        }

        // A reference the update DECLARES has to resolve: the ids it puts in
        // `extendsFrom` and `compositeComponentTypes` must name component types
        // the service holds, the inheritance they declare must not reach back
        // to this component type, and a property group must name properties the
        // patched component type defines. Checked BEFORE the `dataType` rule
        // below, because a dangling parent is what would otherwise license a
        // definition with no `dataType` to be stored.
        //
        // Only the members this request NAMES are resolved for the two id
        // members: those are the references this call declares, and one some
        // earlier write left dangling is not this call's to refuse — refusing
        // it would make an unrelated patch (a description, say) of such a
        // record impossible.
        if declares_extends_from {
            if let Some(message) = crate::state::extends_from_violation(
                workspace,
                &input.component_type_id,
                &updated.extends_from,
            ) {
                return Err(validation_exception(message));
            }
        }
        if declares_composite_component_types {
            if let Some(message) = crate::state::composite_component_types_violation(
                workspace,
                &input.component_type_id,
                &updated.composite_component_types,
            ) {
                return Err(validation_exception(message));
            }
        }
        // A declaration this patch DROPS is not checked against the entities
        // standing on it, and needs no check: a composite component an entity
        // already stores is a record of its own, addressed through the store by
        // every reader and by its writer alike
        // (`crate::state::stored_composite_component_path`), so it stays
        // readable, patchable and removable after the declaration that produced
        // it is gone. The declarations are what a composite component is brought
        // INTO BEING through, which is why only the ones this request declares
        // are resolved above.
        // The property groups, by contrast, are checked over the WHOLE patched
        // record whenever this request touches the set of properties they can
        // resolve against — the groups themselves, the own definitions, or the
        // inheritance the inherited ones come from — because a patch that drops
        // a definition a stored group still names is the call that strands that
        // group, exactly as a patch that clears `extendsFrom` is the call that
        // strands an inherited definition (the `dataType` rule below).
        if declares_property_groups || declares_property_definitions || declares_extends_from {
            if let Some(message) = crate::state::property_groups_violation(workspace, &updated) {
                return Err(validation_exception(message));
            }
        }

        // Every definition the patched record would hold has to be reportable,
        // not just the ones this request carried: an update that clears or
        // re-points `extendsFrom` takes the inheritance an older definition was
        // relying on away with it. Asked whenever the request touches either
        // input of the rule — the definitions themselves or the inheritance
        // they resolve a `dataType` through — and skipped otherwise, exactly as
        // the property-group rule above is, because a patch that leaves both
        // alone leaves the answer the stored record was already held to.
        if declares_property_definitions || declares_extends_from {
            // A definition's `dataType` carries a `type` the model's `Type`
            // enumeration admits — the rule
            // `crate::state::property_definitions_violation` every writer of a
            // definition meets. Asked over the WHOLE patched record, for the
            // same reason the two rules below are, and before them: a type the
            // model does not define has nothing to resolve or compare against.
            if let Some(message) = crate::state::property_definitions_violation(
                "propertyDefinitions",
                updated
                    .property_definitions
                    .iter()
                    .map(|(property_name, definition)| (property_name.as_str(), definition)),
            ) {
                return Err(validation_exception(message));
            }
            if let Some(property_name) = workspace.stranded_property_definition(
                &updated.extends_from,
                updated
                    .property_definitions
                    .iter()
                    .map(|(property_name, definition)| (property_name.as_str(), definition)),
            ) {
                return Err(validation_exception(
                    crate::state::property_definition_needs_data_type_message(property_name),
                ));
            }
            // A definition's `defaultValue` is a value OF the property it
            // defines — a read reports it as the property's value for as long
            // as nothing has been written to the property — so it carries a
            // value of the `dataType` the definition reports beside it: the
            // rule `crate::state::Workspace::property_definition_default_violation`
            // this update shares with `CreateComponentType`. Asked over the
            // WHOLE patched record, and whenever the request touches either
            // input of the rule, for the same reason the `dataType` rule above
            // is: a patch that re-points `extendsFrom`, or that restates a
            // `dataType`, is the call that leaves an older default
            // contradicting the type a read now reports beside it.
            if let Some(message) = workspace.property_definition_default_violation(
                &updated.extends_from,
                updated
                    .property_definitions
                    .iter()
                    .map(|(property_name, definition)| (property_name.as_str(), definition)),
            ) {
                return Err(validation_exception(message));
            }
        }

        // The change has landed, so the record's `updateDateTime` moves and the
        // component type reports the non-terminal state an update in flight is
        // in; the clock, not a caller, settles it back in `ACTIVE`.
        updated.begin_update(now);
        let output = UpdateComponentTypeOutput {
            workspace_id: updated.workspace_id.clone(),
            component_type_id: updated.component_type_id.clone(),
            arn: updated.arn.clone(),
            state: updated.state.clone(),
        };

        // Commit LAST: the record is re-borrowed mutably here — the lookups
        // that validated the request held the workspace immutably, so the
        // references above could be resolved against its other component types
        // — and nothing past this point can fail.
        let Some(workspace) = state.workspaces.get_mut(&input.workspace_id) else {
            return Err(vanished(&input.workspace_id, &input.component_type_id));
        };
        let Some(stored) = workspace.component_types.get_mut(&input.component_type_id) else {
            return Err(vanished(&input.workspace_id, &input.component_type_id));
        };
        *stored = updated;
        // Abstractness is decided from the definitions, functions and
        // inheritance a record holds, so this patch changes the answer for the
        // record it landed on AND for every component type whose inheritance
        // reaches it — a connector this patch added or dropped serves, or stops
        // serving, an externally stored property of each of them. Re-decided
        // over the committed records for that reason, rather than over the
        // patched copy alone. See
        // `crate::state::redecide_component_type_abstractness`.
        crate::state::redecide_component_type_abstractness(workspace, &input.component_type_id);

        Ok(output)
    }
}

/// The server error for a record the validated patch resolved against but the
/// commit below could no longer find.
///
/// Unreachable in practice — the store is held under one lock for the whole call
/// — so it exists to keep the commit from having to unwrap, and names the
/// component type and workspace the call addressed.
fn vanished(workspace_id: &str, component_type_id: &str) -> UpdateComponentTypeError {
    UpdateComponentTypeErrorModeled::InternalServerException(InternalServerException {
        message: Some(format!(
            "component type {component_type_id} of workspace {workspace_id} vanished"
        )),
    })
    .into()
}

/// One refusal of this operation's `ValidationException`, with the message the
/// rule that refused the request produced.
///
/// The message is built from the REQUEST — the member and the id or name the
/// service could not resolve — and is stated within the maximum of the member it
/// travels on ([`crate::state::clamped_message`]), whatever text of the request
/// it names. A value outside a `dataType`'s modelled value list is resolved by
/// the boundary validator before this handler runs, so the branch naming one is
/// this handler's own defence rather than what answers such a request on the
/// wire.
fn validation_exception(message: String) -> UpdateComponentTypeError {
    UpdateComponentTypeErrorModeled::ValidationException(ValidationException {
        message: Some(crate::state::clamped_message(
            message,
            crate::state::ERROR_MESSAGE_MAX_CHARS,
        )),
    })
    .into()
}
