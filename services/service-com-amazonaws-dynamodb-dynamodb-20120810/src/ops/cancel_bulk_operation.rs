// SPDX-License-Identifier: CC0-1.0

//! @op CancelBulkOperation
//! IMPLEMENTED: stops the ONE bulk operation its request addresses by ARN, and answers that
//! operation's ARN with the status it stands in once it has been asked to stop
//! (`smithy_query(query="operation", operation="CancelBulkOperation")`: the output carries
//! those two members and nothing else, so what a caller reads the rest of the record back
//! through is `DescribeBulkOperation`).
//!
//! A MUTATION, and the one bulk-operation call that is: the record the catalogue holds is
//! changed, and every reader of it — `DescribeBulkOperation` and `ListBulkOperations` —
//! reports the change from then on. Stopping work takes a span exactly as doing it does, so
//! the operation reports `CANCELLING` for one transition span
//! ([`crate::state::STATUS_TRANSITION`]) and the `CANCELLED` it settles into from then on;
//! nothing sleeps for it, because the due instant is stored on the record and every reader
//! derives the status from the clock ([`crate::state::BulkOperation::status`]).
//!
//! IDEMPOTENT: a second cancel of an operation already stopping asks for the cancellation
//! that is under way, so it answers that operation and does not push the instant it stops
//! further out — two callers asking for one cancellation observe one.
//!
//! WORK THAT IS OVER CANNOT BE STOPPED: an operation whose span has elapsed has already
//! reached a terminal status, so the request is refused rather than answered with a
//! cancellation that did not happen, and the operation keeps the status it reached
//! ([`crate::state::bulk_operation_not_cancellable`]).
//!
//! THE ARN IS THE WHOLE SELECTOR, read exactly as the sibling reader reads it
//! (`super::describe_bulk_operation`): a value that is no bulk-operation ARN at all is
//! refused as that fault, and a well-formed ARN the account holds no operation under is the
//! modelled `ResourceNotFoundException`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so engine
//! access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

/// The operation's MODELED errors.
///
/// `InternalServerError` is widened in through `extra_errors`: this SERVICE models that
/// shape and this operation's own `errors` list omits it
/// (`smithy_query(query="operation", operation="CancelBulkOperation")`), but a cancel that
/// cannot reach this service's own store has failed on the server's side of the call, which
/// is the one class that fault belongs to.
#[smithy_macros::operation_error(
    "com.amazonaws.dynamodb#CancelBulkOperation",
    extra_errors = ["InternalServerError"]
)]
#[derive(Debug)]
pub enum CancelBulkOperationErrorModeled {
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::cancel_bulk_operation::CancelBulkOperationError`).
pub type CancelBulkOperationError = crate::scaffolded::Scaffolded<CancelBulkOperationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<CancelBulkOperationErrorModeled> for CancelBulkOperationError {
    fn from(modeled: CancelBulkOperationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `CancelBulkOperation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this crate
/// names no concrete engine type.
pub struct CancelBulkOperation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> CancelBulkOperation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for CancelBulkOperation<Eng>
{
    type Input = CancelBulkOperationInput;
    type Output = CancelBulkOperationOutput;
    type Error = CancelBulkOperationError;

    const NAME: &'static str = "CancelBulkOperation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CANCEL_BULK_OPERATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::CANCEL_BULK_OPERATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        let addressed = input.bulk_operation_arn;
        crate::state::resolve_bulk_operation_arn(&addressed).map_err(lift)?;
        // WHO this request speaks for, resolved before this service's store is locked.
        let caller = crate::state::request_caller(&*self.engine, ctx);
        // AUTHORIZATION COMES FIRST, ahead of the lookup and of the change: a caller the
        // policies in force do not let stop this work must be refused before it is told
        // whether the operation it named exists ([`crate::state::authorization`]).
        if let Some(denial) = crate::state::authorization::account_denial(
            &caller,
            ACTION,
            &crate::state::authorization::account_resource(&region, None),
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }

        let mut store = state.lock();
        let operation = store
            .bulk_operation_in_mut(&region, &addressed)
            .ok_or_else(|| lift(crate::state::bulk_operation_not_found(&addressed)))?;
        // Work that is over cannot be stopped: the operation keeps the status it reached.
        if !operation.is_running(now) {
            let status = operation.status(now).to_string();
            return Err(lift(crate::state::bulk_operation_not_cancellable(
                &addressed, &status,
            )));
        }
        operation.cancel(now);
        let outcome = operation.status(now).to_string();
        drop(store);
        audited(Self::NAME, ctx, &addressed, &outcome);
        Ok(CancelBulkOperationOutput {
            bulk_operation_arn: addressed,
            bulk_operation_status: outcome,
        })
    }
}

/// The IAM action this operation performs, which a refusal names.
const ACTION: &str = "dynamodb:CancelBulkOperation";

/// The audit record for a state-changing call: who asked, which operation was stopped and
/// the status it moved to. Never a credential — only the access key id the caller presented.
/// Request-sourced strings are rendered with `Debug`, so a control character in one cannot
/// shape the record.
fn audited(
    operation: &str,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    bulk_operation_arn: &str,
    outcome: &str,
) {
    ::tracing::info!(
        operation,
        akid = %::emulator_server::extract_akid(&ctx.parts().headers),
        bulk_operation_arn = ?bulk_operation_arn,
        outcome = ?outcome,
        "bulk operation cancelled"
    );
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> CancelBulkOperationError {
    CancelBulkOperationErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> CancelBulkOperationError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            CancelBulkOperationErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(message),
            })
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}
