// SPDX-License-Identifier: CC0-1.0

//! @op UpdateRestoreTestingSelection
//!
//! Rewrites the restore testing selection stored under the plan and selection
//! names the request addresses, so what this update accepted is what every
//! later `GetRestoreTestingSelection` of that pair reports.
//!
//! "Most elements except the RestoreTestingSelectionName can be updated with
//! this request" (the model doc below), so the members
//! `RestoreTestingSelectionForUpdate` declares are merged into the stored record
//! ([`crate::state::RestoreTestingSelectionRecord::apply_update`]) — a member
//! the body states nothing for keeps the value the selection already held — and
//! the name, the plan the selection belongs to, its protected resource type and
//! its creation instant survive the rewrite.
//!
//! A name this account holds no plan under is the plan's not-found condition
//! ([`crate::state::restore_testing_plan_not_found`]) and a selection name the
//! named plan does not hold is this resource's
//! ([`crate::state::restore_testing_selection_not_found`]), both resolved — in
//! that order, as every call that addresses a selection resolves them — before
//! the body is read, so a call that addresses nothing rewrites nothing.
//!
//! "You can use either protected resource ARNs or conditions, but not both" (the
//! model doc below): the documented rules are read over the selection this
//! update would leave behind, through the same reading that admitted the
//! create's body
//! ([`crate::state::rejected_restore_testing_selection_update`]), so a refused
//! update rewrites nothing and a stored selection is always one a create could
//! have stored.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! The pair is resolved, the rules are read and the record is rewritten under
//! ONE hold of the store, so the updates of one selection are serialized by the
//! store itself and none of them ever sees another half-applied: the modelled
//! `ConflictException` ("Backup can't perform the action that you requested
//! until it finishes performing a previous action") therefore has no condition
//! to state here, which is why this operation never answers it.
//!
//! ---
//!
//! # Modeled contract for `UpdateRestoreTestingSelection`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Updates the specified restore testing selection. Most elements except the
//! RestoreTestingSelectionName can be updated with this request. You can use either
//! protected resource ARNs or conditions, but not both.
//!
//! ## Input
//!
//! * `RestoreTestingPlanName` (String) **required** — The restore testing plan name is
//!   required to update the indicated testing plan.
//! * `RestoreTestingSelection` (RestoreTestingSelectionForUpdate) **required** — To update
//!   your restore testing selection, you can use either protected resource ARNs or
//!   conditions, but not both. That is, if your selection has ProtectedResourceArns,
//!   requesting an update with the parameter ProtectedResourceConditions will be
//!   unsuccessful.
//! * `RestoreTestingSelectionName` (String) **required** — The required restore testing
//!   selection name of the restore testing selection you wish to update.
//!
//! ## Output
//!
//! * `CreationTime` (Timestamp) **required** — The time the resource testing selection was
//!   updated successfully.
//! * `RestoreTestingPlanArn` (String) **required** — Unique string that is the name of the
//!   restore testing plan.
//! * `RestoreTestingPlanName` (String) **required** — The restore testing plan with which the
//!   updated restore testing selection is associated.
//! * `RestoreTestingSelectionName` (String) **required** — The returned restore testing
//!   selection name.
//! * `UpdateTime` (Timestamp) **required** — The time the update completed for the restore
//!   testing selection.
//!
//! ## Errors
//!
//! * `ConflictException` (HTTP 409) — Backup can't perform the action that you requested
//!   until it finishes performing a previous action. Try again later.
//! * `InvalidParameterValueException` (HTTP 400) — Indicates that something is wrong with a
//!   parameter's value. For example, the value is out of range.
//! * `MissingParameterValueException` (HTTP 400) — Indicates that a required parameter is
//!   missing.
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `UpdateRestoreTestingSelectionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateRestoreTestingSelectionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#UpdateRestoreTestingSelection")]
#[derive(Debug)]
pub enum UpdateRestoreTestingSelectionErrorModeled {
    ConflictException(ConflictException),
    InvalidParameterValueException(InvalidParameterValueException),
    MissingParameterValueException(MissingParameterValueException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_restore_testing_selection::UpdateRestoreTestingSelectionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateRestoreTestingSelectionError = crate::scaffolded::Scaffolded<UpdateRestoreTestingSelectionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateRestoreTestingSelectionErrorModeled> for UpdateRestoreTestingSelectionError {
    fn from(modeled: UpdateRestoreTestingSelectionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateRestoreTestingSelection` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateRestoreTestingSelection<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> UpdateRestoreTestingSelection<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateRestoreTestingSelection<Eng>
{
    type Input = UpdateRestoreTestingSelectionInput;
    type Output = UpdateRestoreTestingSelectionOutput;
    type Error = UpdateRestoreTestingSelectionError;

    const NAME: &'static str = "UpdateRestoreTestingSelection";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_RESTORE_TESTING_SELECTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_RESTORE_TESTING_SELECTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The plan's ARN was minted by its create and is stored with it, so this
        // write reports identifiers rather than recomputing any: nothing here is
        // read off the request's region.
        let _ = ctx;
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            UpdateRestoreTestingSelectionErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let plan_name = input.restore_testing_plan_name;
        let selection_name = input.restore_testing_selection_name;
        let body = input.restore_testing_selection;

        // Resolving the pair, reading the body's rules over it and rewriting it
        // are ONE critical section, so the selection a later read reports is the
        // one this update merged its body into rather than a record another
        // update rewrote in between.
        let mut store = state.lock();

        // Resolved in the order every call that addresses a selection resolves
        // them, so a name this account holds no plan under is the PLAN's
        // not-found condition and a selection name the named plan does not hold
        // is this resource's. The plan's stored ARN is what this answer reports,
        // so an update and a read can never report different plan identifiers.
        let plan_arn = store
            .stored_restore_testing_plan(&plan_name)
            .map_err(UpdateRestoreTestingSelectionErrorModeled::ResourceNotFoundException)?
            .arn
            .clone();
        let stored = store
            .stored_restore_testing_selection(&plan_name, &selection_name)
            .map_err(UpdateRestoreTestingSelectionErrorModeled::ResourceNotFoundException)?;

        // The documented rules are read over the selection this update would
        // leave behind — the body's members where it states them, the stored
        // ones otherwise — by the same reading that admitted the create's body,
        // so a refused update leaves the stored selection exactly as it was.
        if let Some(refusal) =
            crate::state::rejected_restore_testing_selection_update(stored, &body)
        {
            return Err(match refusal {
                crate::state::RejectedRestoreTestingSelectionBody::Value(refusal) => {
                    UpdateRestoreTestingSelectionErrorModeled::InvalidParameterValueException(
                        refusal,
                    )
                }
                crate::state::RejectedRestoreTestingSelectionBody::SelectsNothing(refusal) => {
                    UpdateRestoreTestingSelectionErrorModeled::MissingParameterValueException(
                        refusal,
                    )
                }
                // This operation models no `LimitExceededException`, so a body
                // naming more specific ARNs than one selection may is stated as
                // what it is for this call: the member's value is out of range,
                // which is the condition the modelled
                // `InvalidParameterValueException` is documented for
                // ("something is wrong with a parameter's value. For example,
                // the value is out of range").
                crate::state::RejectedRestoreTestingSelectionBody::TooManyProtectedResourceArns(
                    _,
                ) => UpdateRestoreTestingSelectionErrorModeled::InvalidParameterValueException(
                    crate::state::rejected_parameter("ProtectedResourceArns"),
                ),
            }
            .into());
        }

        // The rewrite, with the clock read here: the selection carries the
        // instant its merged body landed rather than one read before the rules
        // above were applied.
        let now = ::std::time::SystemTime::now();
        let record = store
            .stored_restore_testing_selection_mut(&plan_name, &selection_name)
            .map_err(UpdateRestoreTestingSelectionErrorModeled::ResourceNotFoundException)?;
        record.apply_update(body, now);
        Ok(UpdateRestoreTestingSelectionOutput {
            restore_testing_plan_arn: plan_arn,
            restore_testing_plan_name: record.plan_name.clone(),
            restore_testing_selection_name: record.name.clone(),
            // The create's instant, which an update leaves alone, beside the
            // instant this update stored the merged body.
            creation_time: crate::state::modelled_timestamp(record.creation_time),
            update_time: crate::state::modelled_timestamp(record.last_update_time),
        })
    }
}
