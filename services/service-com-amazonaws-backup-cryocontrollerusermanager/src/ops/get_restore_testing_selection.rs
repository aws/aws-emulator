// SPDX-License-Identifier: CC0-1.0

//! @op GetRestoreTestingSelection
//!
//! Reports the restore testing selection the store holds under the plan and
//! selection names the request names, from the ONE read projection of that
//! stored record ([`crate::state::RestoreTestingSelectionRecord::for_get`]), so
//! this read reports nothing the store does not hold — the resource scope, the
//! conditions filter and the overrides map every selection carries included,
//! which are reported as the collections they are, empty ones as the empty
//! collection rather than as absent.
//!
//! A name this account holds no plan under is the plan's not-found condition
//! ([`crate::state::restore_testing_plan_not_found`]) and a selection name the
//! named plan does not hold is this resource's
//! ([`crate::state::restore_testing_selection_not_found`]), both resolved by the
//! one step every call that addresses a selection shares.
//!
//! REPORTING ONLY: "Returns RestoreTestingSelection, which displays resources
//! and elements of the restore testing plan" (the model's doc below), so this
//! handler writes nothing — a later caller sees exactly what it saw.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `GetRestoreTestingSelection`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
//! ```
//!
//! Returns RestoreTestingSelection, which displays resources and elements of the restore
//! testing plan.
//!
//! ## Input
//!
//! * `RestoreTestingPlanName` (String) **required** — Required unique name of the restore
//!   testing plan.
//! * `RestoreTestingSelectionName` (String) **required** — Required unique name of the
//!   restore testing selection.
//!
//! ## Output
//!
//! * `RestoreTestingSelection` (RestoreTestingSelectionForGet) **required** — Unique name of
//!   the restore testing selection.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 400) — A resource that is required for the action
//!   doesn't exist.
//! * `ServiceUnavailableException` (HTTP 500) — The request failed due to a temporary failure
//!   of the server.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `GetRestoreTestingSelectionError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `GetRestoreTestingSelectionError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.backup#GetRestoreTestingSelection")]
#[derive(Debug)]
pub enum GetRestoreTestingSelectionErrorModeled {
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::get_restore_testing_selection::GetRestoreTestingSelectionError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type GetRestoreTestingSelectionError = crate::scaffolded::Scaffolded<GetRestoreTestingSelectionErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<GetRestoreTestingSelectionErrorModeled> for GetRestoreTestingSelectionError {
    fn from(modeled: GetRestoreTestingSelectionErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `GetRestoreTestingSelection` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct GetRestoreTestingSelection<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> GetRestoreTestingSelection<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for GetRestoreTestingSelection<Eng>
{
    type Input = GetRestoreTestingSelectionInput;
    type Output = GetRestoreTestingSelectionOutput;
    type Error = GetRestoreTestingSelectionError;

    const NAME: &'static str = "GetRestoreTestingSelection";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_RESTORE_TESTING_SELECTION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::GET_RESTORE_TESTING_SELECTION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        _ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            GetRestoreTestingSelectionErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException {
                    message: Some("the backup vault store is unavailable".to_string()),
                    ..Default::default()
                },
            )
        })?;
        let store = state.lock();
        let selection = store
            .stored_restore_testing_selection(
                &input.restore_testing_plan_name,
                &input.restore_testing_selection_name,
            )
            .map_err(GetRestoreTestingSelectionErrorModeled::ResourceNotFoundException)?;
        Ok(GetRestoreTestingSelectionOutput {
            restore_testing_selection: selection.for_get(),
        })
    }
}
