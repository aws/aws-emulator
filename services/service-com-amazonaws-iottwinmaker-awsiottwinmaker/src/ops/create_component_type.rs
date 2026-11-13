// SPDX-License-Identifier: CC0-1.0

//! @op CreateComponentType
//! Creates a component type in a workspace and persists it in [`crate::state`]
//! under that workspace, so `GetComponentType` reads back what was created.
//!
//! Unlike `CreateEntity`, this create does NOT complete within the call: the
//! `state` it answers is the non-terminal `CREATING`, and the component type
//! settles in `ACTIVE` once [`crate::state::COMPONENT_TYPE_SETTLE_SPAN`] has
//! elapsed — an instant stored on the record and materialised by the next
//! reader, never slept for.
//!
//! The refusals this handler owns:
//!
//! * `tags` names a key in the reserved `aws:` namespace
//!   ([`crate::state::RESERVED_TAG_KEY_PREFIX`], matched however it is cased) —
//!   a `ValidationException`, since that namespace is Amazon Web Services' own;
//! * the workspace does not exist — a `ValidationException`, because the model
//!   declares no `ResourceNotFoundException` for this operation (the same
//!   choice `CreateEntity` makes);
//! * `componentTypeId` is already taken in the workspace — a
//!   `ConflictException`;
//! * `componentTypeId` lies in the namespace the service reserves for the
//!   component types it SHIPS
//!   ([`crate::state::BUILT_IN_COMPONENT_TYPE_ID_PREFIX`]) — a
//!   `ValidationException`: those are provisioned with the workspace and are
//!   not a caller's to mint;
//! * the workspace already holds
//!   [`crate::state::MAX_COMPONENT_TYPES_PER_WORKSPACE`] component types the
//!   caller created — the modelled `ServiceQuotaExceededException`;
//! * a property definition carries a `dataType` whose `type` is outside the
//!   modelled `Type` value list, or a property group carries a `groupType`
//!   outside the modelled `GroupType` list — a `ValidationException`, because
//!   both members are `@required` on the response a read reports and the model
//!   admits only the values its enumeration defines (the rules
//!   [`crate::state::property_definitions_violation`] and
//!   [`crate::state::property_groups_violation`], which every writer of those
//!   members meets);
//! * a property definition carries no `dataType` and inherits none — the
//!   component type extends nothing, or no component type on its `extendsFrom`
//!   chain declares that property with a `dataType` — a `ValidationException`,
//!   because `dataType` is required on the `PropertyDefinitionResponse` a read
//!   reports, so there would be nothing to resolve it from (the rule
//!   [`crate::state::Workspace::stranded_property_definition`]);
//! * a property definition's `defaultValue` carries a value of a type other
//!   than the `dataType` the definition resolves to — a `ValidationException`,
//!   because a read reports that default AS the property's value
//!   ([`crate::state::latest_property_value`]) beside the `dataType` it
//!   contradicts (the rule
//!   [`crate::state::Workspace::property_definition_default_violation`], the
//!   same rule every writer of a property's value is held to);
//! * a reference the request DECLARES does not resolve — an `extendsFrom` or
//!   `compositeComponentTypes[*].componentTypeId` id naming a component type
//!   the service does not hold, inheritance that names the component type being
//!   created, or a `propertyGroups[*].propertyNames` entry naming a property
//!   the new component type neither defines nor inherits — a
//!   `ValidationException` (HTTP 400): the rules
//!   [`crate::state::extends_from_violation`],
//!   [`crate::state::composite_component_types_violation`] and
//!   [`crate::state::property_groups_violation`] hold for every writer of
//!   those members, this create as much as `UpdateComponentType`, so the store
//!   cannot come to hold a reference that does not resolve. A declared id
//!   resolves to a component type stored in the workspace or to one in the
//!   service's reserved namespace (see
//!   [`crate::state::BUILT_IN_COMPONENT_TYPE_ID_PREFIX`]): the real service
//!   ships more built-in component types than this emulator models, so an id
//!   there resolves rather than being refused, which keeps the rule from
//!   rejecting inheritance the real service accepts. A composite entry naming
//!   the component type being created resolves too — this very call is what
//!   leaves the workspace holding it — while `extendsFrom` naming it is the
//!   inheritance cycle no chain can be walked up, which is why the two members
//!   differ on a self-reference.
//!
//! The declared references are resolved BEFORE the `dataType` rule, because the
//! `extendsFrom` chain is what a `PropertyDefinitionRequest` carrying no
//! `dataType` inherits one through: a parent that does not resolve would
//! otherwise be asked for a definition it could never contribute.
//!
//! Authorization is not emulated, so the modelled `AccessDeniedException` is
//! never returned.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `CreateComponentType::new(engine)` and drives it through the centralized request walk:
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
//! # Modeled contract for `CreateComponentType`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json")
//! ```
//!
//! Creates a component type.
//!
//! ## Input
//!
//! * `componentTypeId` (ComponentTypeId, len 1..256, pattern ^[a-zA-Z_\.\-0-9:]+$)
//!   **required** — The ID of the component type.
//! * `componentTypeName` (ComponentTypeName, len 0..256, pattern [^\u0000-\u001F\u007F]*) — A
//!   friendly name for the component type.
//! * `compositeComponentTypes` (CompositeComponentTypesRequest) — This is an object that maps
//!   strings to compositeComponentTypes of the componentType. CompositeComponentType is
//!   referenced by componentTypeId.
//! * `description` (Description, len 0..2048, pattern .*) — The description of the component
//!   type.
//! * `extendsFrom` (ExtendsFrom) — Specifies the parent component type to extend.
//! * `functions` (FunctionsRequest) — An object that maps strings to the functions in the
//!   component type. Each string in the mapping must be unique to this object.
//! * `isSingleton` (Boolean) — A Boolean value that specifies whether an entity can have more
//!   than one component of this type.
//! * `propertyDefinitions` (PropertyDefinitionsRequest) — An object that maps strings to the
//!   property definitions in the component type. Each string in the mapping must be unique to
//!   this object.
//! * `propertyGroups` (PropertyGroupsRequest)
//! * `tags` (TagMap, len 0..50) — Metadata that you can use to manage the component type.
//! * `workspaceId` (Id, len 1..128, pattern ^[a-zA-Z_0-9][a-zA-Z_\-0-9]*[a-zA-Z0-9]+$)
//!   **required** — The ID of the workspace that contains the component type.
//!
//! ## Output
//!
//! * `arn` (TwinMakerArn, len 20..2048, pattern
//!   ^arn:((aws)|(aws-cn)|(aws-us-gov)):iottwinmaker:[a-z0-9-]+:[0-9]{12}:[\/a-zA-Z0-9_\-\.:]+$)
//!   **required** — The ARN of the component type.
//! * `creationDateTime` (Timestamp) **required** — The date and time when the entity was
//!   created.
//! * `state` (State, one of: ACTIVE | CREATING | DELETING | ERROR | UPDATING) **required** —
//!   The current state of the component type.
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
// `CreateComponentTypeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `CreateComponentTypeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.iottwinmaker#CreateComponentType")]
#[derive(Debug)]
pub enum CreateComponentTypeErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ConflictException(ConflictException),
    InternalServerException(InternalServerException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::create_component_type::CreateComponentTypeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type CreateComponentTypeError = crate::scaffolded::Scaffolded<CreateComponentTypeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<CreateComponentTypeErrorModeled> for CreateComponentTypeError {
    fn from(modeled: CreateComponentTypeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CreateComponentType` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct CreateComponentType<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CreateComponentType<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CreateComponentType<Eng>
{
    type Input = CreateComponentTypeInput;
    type Output = CreateComponentTypeOutput;
    type Error = CreateComponentTypeError;

    const NAME: &'static str = "CreateComponentType";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_COMPONENT_TYPE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::CREATE_COMPONENT_TYPE_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Every call is recorded, successful or refused: the real service logs
        // control-plane access, and this one mutates tenant state.
        ::tracing::info!(
            operation = "CreateComponentType",
            workspace_id = %input.workspace_id,
            component_type_id = %input.component_type_id,
            "serving a request"
        );
        // The reserved tag-key namespace is AWS's own: a create may no more put a
        // key under it on the new resource than `TagResource` may add one later,
        // so a request naming one is refused whole and nothing is created. The
        // reason names the keys at fault, as many as the request carried, and the
        // construction that builds it is what states it within the bound of the
        // member it travels on.
        if let Some(message) = crate::state::reserved_tag_keys_violation(
            "tags",
            input
                .tags
                .iter()
                .flat_map(|tags| tags.keys().map(String::as_str)),
        ) {
            return Err(validation_exception(message));
        }

        // The Region this request is signed for picks the store it writes to,
        // and the Region the component type's ARN carries: the workspace it is
        // created in is the one THIS Region holds under that id.
        let region = crate::state::request_region(ctx.parts());
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            CreateComponentTypeErrorModeled::InternalServerException(InternalServerException {
                message: Some("the iottwinmaker store is unavailable".to_string()),
            })
        })?;
        let mut state = state.lock();
        let state = state.region_mut(&region);

        let Some(workspace) = state.workspaces.get(&input.workspace_id) else {
            return Err(CreateComponentTypeErrorModeled::ValidationException(
                ValidationException {
                    message: Some(crate::state::workspace_not_found_message(
                        &input.workspace_id,
                    )),
                },
            )
            .into());
        };

        // The id is the caller's: `componentTypeId` is required and carried in
        // the URI, so there is nothing to mint and a taken id is a conflict.
        if workspace.component_types.contains_key(&input.component_type_id) {
            return Err(CreateComponentTypeErrorModeled::ConflictException(
                ConflictException {
                    message: Some(crate::state::already_exists_message(
                        "Component type",
                        &input.component_type_id,
                        Some(&input.workspace_id),
                    )),
                },
            )
            .into());
        }

        // The `com.amazon.` namespace is the SERVICE's: the component types in
        // it are the built-ins every workspace is provisioned with, so a caller
        // cannot mint one there. Checked AFTER the conflict above, so asking for
        // a built-in the workspace already holds is still the conflict that id
        // deserves.
        if crate::state::built_in_component_type_id(&input.component_type_id) {
            return Err(CreateComponentTypeErrorModeled::ValidationException(
                ValidationException {
                    message: Some(crate::state::reserved_component_type_namespace_message(
                        &input.component_type_id,
                    )),
                },
            )
            .into());
        }

        // The emulator's declared ceiling on the store, over the component
        // types the CALLER created: the workspace's built-ins are the service's
        // own, so they do not spend a caller's quota. Checked AFTER the identity
        // conflict above so re-creating a taken id in a full workspace is still
        // the ConflictException the id deserves.
        if workspace.created_component_types().count()
            >= crate::state::MAX_COMPONENT_TYPES_PER_WORKSPACE
        {
            return Err(
                CreateComponentTypeErrorModeled::ServiceQuotaExceededException(
                    ServiceQuotaExceededException {
                        message: Some(format!(
                            "workspace {} has reached the limit of {} component types",
                            input.workspace_id,
                            crate::state::MAX_COMPONENT_TYPES_PER_WORKSPACE
                        )),
                    },
                )
                .into(),
            );
        }

        let extends_from = input.extends_from.clone().unwrap_or_default();
        // Both requested maps are walked by a rule below, so both are taken in
        // KEY order (`crate::state::requested_in_key_order`): which entry a
        // refusal names is decided by the request, not by the iteration order
        // of the hash map it was deserialized into.
        let property_definitions =
            crate::state::requested_in_key_order(input.property_definitions.clone());
        let composite_component_types =
            crate::state::requested_in_key_order(input.composite_component_types.clone());
        // A reference this create DECLARES has to resolve: the ids it puts in
        // `extendsFrom` and in `compositeComponentTypes` must name component
        // types the service holds, and the inheritance it declares must not
        // name the component type being created. The same rules every writer
        // of those members meets, so a create cannot store what an update
        // would be refused, and the store cannot come to hold a dangling
        // reference — the integrity `DeleteComponentType` already relies on in
        // the other direction.
        //
        // Resolved BEFORE the `dataType` rule below, because the `extendsFrom`
        // chain is what a definition carrying no `dataType` inherits one
        // through: a parent that does not resolve would otherwise be asked for
        // a definition it could never contribute.
        if let Some(message) =
            crate::state::extends_from_violation(workspace, &input.component_type_id, &extends_from)
        {
            return Err(validation_exception(message));
        }
        if let Some(message) = crate::state::composite_component_types_violation(
            workspace,
            &input.component_type_id,
            &composite_component_types,
        ) {
            return Err(validation_exception(message));
        }
        // A definition's `dataType` carries a `type` the model's `Type`
        // enumeration admits: the member is `@required` on the `DataType` a
        // read reports, and a value outside the list would leave the property
        // unwritable for good, since no `DataValue` can carry a type the model
        // does not define. The rule
        // `crate::state::property_definitions_violation` every writer of a
        // definition shares, asked of the whole requested map in property-NAME
        // order and BEFORE the resolution below, which has nothing to resolve
        // for a type the model does not define.
        if let Some(message) = crate::state::property_definitions_violation(
            "propertyDefinitions",
            property_definitions
                .iter()
                .map(|(property_name, definition)| (property_name.as_str(), definition)),
        ) {
            return Err(validation_exception(message));
        }
        // `dataType` is optional on `PropertyDefinitionRequest` but REQUIRED on
        // the `PropertyDefinitionResponse` a read reports, so a definition may
        // omit it only where it REFINES one the declared inheritance brings for
        // the same property — the rule
        // `crate::state::Workspace::stranded_property_definition` holds for
        // every writer of a definition, asked of the whole requested map in ONE
        // walk of the declared chain. Walked in property-NAME order, so a
        // request carrying more than one definition without a `dataType` is
        // always refused for the first of them in key order, exactly as the
        // sibling `UpdateComponentType` refuses it.
        if let Some(property_name) = workspace.stranded_property_definition(
            &extends_from,
            property_definitions
                .iter()
                .map(|(property_name, definition)| (property_name.as_str(), definition)),
        ) {
            return Err(validation_exception(
                crate::state::property_definition_needs_data_type_message(property_name),
            ));
        }
        // A definition's `defaultValue` is a value OF the property it defines —
        // a read reports it as the property's value for as long as nothing has
        // been written to the property — so it carries a value of the
        // `dataType` the definition reports beside it, the rule
        // `crate::state::Workspace::property_definition_default_violation`
        // every writer of a definition shares with every writer of a property's
        // VALUE. Asked AFTER the `dataType` rule above, over the same resolved
        // chain, so a default is judged against the `dataType` the definition
        // ends up with whether it declares one itself or inherits it.
        if let Some(message) = workspace.property_definition_default_violation(
            &extends_from,
            property_definitions
                .iter()
                .map(|(property_name, definition)| (property_name.as_str(), definition)),
        ) {
            return Err(validation_exception(message));
        }

        let arn = crate::state::component_type_arn(
            &region,
            &input.workspace_id,
            &input.component_type_id,
        );
        let now = ::std::time::SystemTime::now();
        let mut component_type = crate::state::ComponentType {
            component_type_id: input.component_type_id.clone(),
            arn: arn.clone(),
            workspace_id: input.workspace_id.clone(),
            component_type_name: input.component_type_name,
            description: input.description,
            // Resolved rather than carried as an absent member: the flag reads
            // "whether an entity can have more than one component of this
            // type", which is a question with an answer for every component
            // type, and `false` is the answer for one created without it.
            is_singleton: input.is_singleton.unwrap_or(false),
            // Not the caller's to declare — `isAbstract` is not a member of
            // this request — so it is DECIDED from what the request declares,
            // once the record is in hand, just below.
            is_abstract: false,
            extends_from,
            property_definitions: property_definitions.into_iter().collect(),
            functions: input.functions.unwrap_or_default().into_iter().collect(),
            property_groups: input
                .property_groups
                .unwrap_or_default()
                .into_iter()
                .map(|(group_name, group)| {
                    (
                        group_name,
                        crate::state::PropertyGroupRecord::from_request(group),
                    )
                })
                .collect(),
            composite_component_types,
            tags: input.tags.unwrap_or_default().into_iter().collect(),
            creation_date_time: now,
            update_date_time: now,
            // The create does not complete here: the component type is stored,
            // and reported, in the non-terminal CREATING state, and the instant
            // its transition to ACTIVE falls due is stored beside it.
            state: crate::state::STATE_CREATING.to_string(),
            settles_at: now + crate::state::COMPONENT_TYPE_SETTLE_SPAN,
        };
        // A property GROUP groups properties of the component type, so each
        // name it carries has to be one the new component type defines — its
        // own definitions or the ones its (now resolved) inheritance brings.
        // Checked over the record as this create leaves it, so a group may name
        // a property the same request defines: the rule
        // `crate::state::property_groups_violation` every writer of the member
        // meets.
        if let Some(message) = crate::state::property_groups_violation(workspace, &component_type) {
            return Err(validation_exception(message));
        }
        // Whether the component type is ABSTRACT is decided here, from the
        // definitions and functions the request declared plus the ones its
        // declared inheritance brings, and STORED on the record: every reader
        // of `isAbstract` and every writer that refuses to instantiate an
        // abstract type then reads one answer. See
        // `crate::state::component_type_is_abstract`.
        component_type.is_abstract =
            crate::state::component_type_is_abstract(workspace, &component_type);
        let created_state = component_type.state.clone();

        // Store the record BEFORE indexing its ARN, so the index can never name
        // a component type that was not stored: the workspace is re-borrowed
        // mutably here (the lookup that validated the request held it
        // immutably) and that re-borrow is the last fallible step.
        let Some(workspace) = state.workspaces.get_mut(&input.workspace_id) else {
            return Err(CreateComponentTypeErrorModeled::InternalServerException(
                InternalServerException {
                    message: Some(format!("workspace {} vanished", input.workspace_id)),
                },
            )
            .into());
        };
        workspace
            .component_types
            .insert(input.component_type_id.clone(), component_type);
        state.arn_index.insert(
            arn.clone(),
            crate::state::ResourceRef::ComponentType {
                workspace_id: input.workspace_id.clone(),
                component_type_id: input.component_type_id.clone(),
            },
        );

        Ok(CreateComponentTypeOutput {
            arn,
            creation_date_time: crate::state::timestamp(now),
            state: created_state,
        })
    }
}

/// One refusal of this operation's `ValidationException`, with the message the
/// rule that refused the request produced.
///
/// The message is built from the REQUEST — the member and the id or name the
/// service could not resolve — and is stated within the maximum of the member it
/// travels on ([`crate::state::clamped_message`]), whatever text of the request
/// it names: a `tags` map carries up to fifty keys of its own, and a value
/// outside a `dataType`'s modelled value list is resolved by the boundary
/// validator before this handler runs, so the branch naming one is this
/// handler's own defence rather than what answers such a request on the wire.
fn validation_exception(message: String) -> CreateComponentTypeError {
    CreateComponentTypeErrorModeled::ValidationException(ValidationException {
        message: Some(crate::state::clamped_message(
            message,
            crate::state::ERROR_MESSAGE_MAX_CHARS,
        )),
    })
    .into()
}
