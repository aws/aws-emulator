// SPDX-License-Identifier: CC0-1.0

//! @op UpdateExtensionAssociation
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateExtensionAssociation`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json")
//! ```
//!
//! Updates an association. For more information about extensions and associations, see
//! Extending workflows in the AppConfig User Guide.
//!
//! ## Input
//!
//! * `ExtensionAssociationId` (Id, pattern ^[a-z0-9]{4,7}$) **required** — The
//!   system-generated ID for the association.
//! * `Parameters` (ParameterValueMap, len 0..10) — The parameter names and values defined in
//!   the extension.
//!
//! ## Output
//!
//! * `Arn` (Arn, len 20..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:[a-z]+:((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1})?:(\d{12})?:[a-zA-Z0-9-_/:.]+$)
//!   — The system-generated Amazon Resource Name (ARN) for the extension.
//! * `ExtensionArn` (Arn, len 20..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:[a-z]+:((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1})?:(\d{12})?:[a-zA-Z0-9-_/:.]+$)
//!   — The ARN of the extension defined in the association.
//! * `ExtensionVersionNumber` (Integer, default 0) **required** — The version number for the
//!   extension defined in the association.
//! * `Id` (Identifier, len 1..2048) — The system-generated ID for the association.
//! * `Parameters` (ParameterValueMap, len 0..10) — The parameter names and values defined in
//!   the association.
//! * `ResourceArn` (Arn, len 20..2048, pattern
//!   ^arn:(aws[a-zA-Z-]*)?:[a-z]+:((eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1})?:(\d{12})?:[a-zA-Z0-9-_/:.]+$)
//!   — The ARNs of applications, configuration profiles, or environments defined in the
//!   association.
//!
//! ## Errors
//!
//! * `BadRequestException` (HTTP 400) — The input fails to satisfy the constraints specified
//!   by an Amazon Web Services service.
//! * `InternalServerException` (HTTP 500) — There was an internal failure in the AppConfig
//!   service.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource could not be found.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateExtensionAssociationError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-appconfig-amazonappconfig.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateExtensionAssociationError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.appconfig#UpdateExtensionAssociation")]
#[derive(Debug)]
pub enum UpdateExtensionAssociationErrorModeled {
    BadRequestException(BadRequestException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_extension_association::UpdateExtensionAssociationError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateExtensionAssociationError = crate::scaffolded::Scaffolded<UpdateExtensionAssociationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateExtensionAssociationErrorModeled> for UpdateExtensionAssociationError {
    fn from(modeled: UpdateExtensionAssociationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateExtensionAssociation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateExtensionAssociation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateExtensionAssociation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateExtensionAssociation<Eng>
{
    type Input = UpdateExtensionAssociationInput;
    type Output = UpdateExtensionAssociationOutput;
    type Error = UpdateExtensionAssociationError;

    const NAME: &'static str = "UpdateExtensionAssociation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_EXTENSION_ASSOCIATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_EXTENSION_ASSOCIATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                UpdateExtensionAssociationErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "AppConfig state is not reachable on this engine".to_string(),
                        ),
                    },
                )
                .into(),
            );
        };

        let region = crate::state::request_region(ctx.parts());
        let mut store = state.lock();

        // An association is addressed by the id its create minted, so an id this account
        // holds no association under is the modelled 404 rather than an update of nothing.
        let Some(association) = store.extension_association(&input.extension_association_id)
        else {
            return Err(not_found(crate::state::extension_association_not_found()));
        };
        let extension_id = association.extension_id.clone();
        let extension_version_number = association.extension_version_number;

        // The parameter values are checked against the declaration of the extension
        // VERSION this association binds — the same declaration, through the same
        // function, the create checked the values it stored against — and before anything
        // is written, so a refused update leaves the association exactly as it was.
        if let Some(parameters) = input.parameters.as_ref() {
            let Some((extension, _)) = store.resolve_extension(&extension_id, &region) else {
                return Err(not_found(crate::state::extension_not_found()));
            };
            let Some(version) = extension.versions.get(&extension_version_number) else {
                return Err(not_found(crate::state::extension_not_found()));
            };
            // The refusal names the extension by the id this association stores, which is
            // the identifier AppConfig puts in the sentence.
            if let Some(reason) = crate::state::parameter_value_rejection(
                &version.parameters,
                parameters,
                &extension.id,
                version.version_number,
            ) {
                return Err(bad_request(reason));
            }
        }

        // The update is IN PLACE: the association keeps the id and ARN its create minted,
        // the extension version it binds and the resource it is bound to, because the
        // request carries no member that names any of them. An update carries only what
        // the caller means to change, so an omitted `Parameters` leaves the values the
        // association already holds; supplying it REPLACES that whole map, the member
        // being the map itself rather than a set of edits to it.
        // An id that addresses no association is the modelled 404 wherever this handler
        // looks the association up, so the write resolves the id through the same refusal
        // the read above answers with rather than a second, different one.
        let Some(association) = store.extension_association_mut(&input.extension_association_id)
        else {
            return Err(not_found(crate::state::extension_association_not_found()));
        };
        if let Some(parameters) = input.parameters {
            association.parameters = parameters;
        }

        // Answered from the stored record, so this response and a later
        // `GetExtensionAssociation` or `ListExtensionAssociations` report the same
        // association.
        Ok(UpdateExtensionAssociationOutput {
            id: Some(association.id.clone()),
            arn: Some(association.arn.clone()),
            extension_arn: Some(association.extension_arn.clone()),
            resource_arn: Some(association.resource_arn.clone()),
            extension_version_number: association.extension_version_number,
            // `ParameterValueMap` is bounded `len 0..10`, so an association that holds no
            // value reports an empty map rather than omitting the member.
            parameters: Some(association.parameters.clone()),
        })
    }
}

/// The `BadRequestException` (HTTP 400) this operation answers an update it will not store
/// with: "The input fails to satisfy the constraints specified by an Amazon Web Services
/// service."
fn bad_request(message: String) -> UpdateExtensionAssociationError {
    UpdateExtensionAssociationErrorModeled::BadRequestException(BadRequestException {
        message: Some(message),
        reason: None,
        details: None,
    })
    .into()
}

/// The `ResourceNotFoundException` (HTTP 404) this operation answers an id that addresses
/// no association this service holds with.
fn not_found(message: String) -> UpdateExtensionAssociationError {
    UpdateExtensionAssociationErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
        resource_name: None,
    })
    .into()
}
