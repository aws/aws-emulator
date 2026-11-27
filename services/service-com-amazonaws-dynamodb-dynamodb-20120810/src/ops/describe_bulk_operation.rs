// SPDX-License-Identifier: CC0-1.0

//! @op DescribeBulkOperation
//! IMPLEMENTED: reports the ONE bulk operation its request addresses by ARN — the table it
//! acts on, the status it is in, the source it reads, the location it reports into, the
//! instants it began and (once it has) ended, and the figures its task counted.
//!
//! A REPORTING operation: every member it answers is read from the bulk-operation catalogue
//! `StartBulkPutItems` writes ([`crate::state::BulkOperation`]), and the status it reports is
//! derived from the clock at every read, so this call writes nothing and two callers reading
//! it see the same thing.
//!
//! THE ARN IS THE WHOLE SELECTOR. A value that is no bulk-operation ARN at all addresses no
//! operation, and is refused as that fault rather than as a resource that is not there
//! ([`crate::state::INVALID_BULK_OPERATION_ARN`], the one sentence this service states for
//! it); a well-formed ARN the account holds no operation under IS a resource that is not
//! there, and is the modelled `ResourceNotFoundException`. An endpoint serves one Region, so
//! the lookup is scoped to the Region the request reached and an ARN naming another names
//! nothing this endpoint can reach.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. Reach the
//! store through [`crate::state::service_state`] and read it through the guard it hands
//! back.

#[allow(unused_imports)]
use super::types::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

/// The operation's MODELED errors.
///
/// `InternalServerError` is widened in through `extra_errors`, for the reason the sibling
/// cancel widens it in (`super::cancel_bulk_operation`): this SERVICE models the shape, this
/// operation's own `errors` list omits it, and a read that cannot reach this service's store
/// has failed on the server's side of the call.
#[smithy_macros::operation_error(
    "com.amazonaws.dynamodb#DescribeBulkOperation",
    extra_errors = ["InternalServerError"]
)]
#[derive(Debug)]
pub enum DescribeBulkOperationErrorModeled {
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_bulk_operation::DescribeBulkOperationError`).
pub type DescribeBulkOperationError =
    crate::scaffolded::Scaffolded<DescribeBulkOperationErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DescribeBulkOperationErrorModeled> for DescribeBulkOperationError {
    fn from(modeled: DescribeBulkOperationErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeBulkOperation` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this crate
/// names no concrete engine type.
pub struct DescribeBulkOperation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeBulkOperation<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeBulkOperation<Eng>
{
    type Input = DescribeBulkOperationInput;
    type Output = DescribeBulkOperationOutput;
    type Error = DescribeBulkOperationError;

    const NAME: &'static str = "DescribeBulkOperation";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_BULK_OPERATION_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_BULK_OPERATION_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        // The selector, held to the form a bulk operation's ARN takes before anything is
        // looked up: a value that addresses no bulk operation is a fault in the request
        // rather than a resource that is not there.
        crate::state::resolve_bulk_operation_arn(&input.bulk_operation_arn).map_err(lift)?;
        // WHO this request speaks for, resolved before this service's store is locked: a
        // bulk operation carries no policy of its own, so the call is authorized against the
        // caller's own identity-based documents over the account's table namespace
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        if let Some(denial) = crate::state::authorization::account_denial(
            &caller,
            ACTION,
            &crate::state::authorization::account_resource(&region, None),
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }

        let store = state.lock();
        // A reporting operation, and one that reports nothing a WRITE could settle: a
        // bulk-operation record is fixed once written and the status it reports is derived
        // from the clock on each read. So this reads only.
        let operation = store
            .bulk_operation_in(&region, &input.bulk_operation_arn)
            .ok_or_else(|| {
                lift(crate::state::bulk_operation_not_found(
                    &input.bulk_operation_arn,
                ))
            })?;
        Ok(DescribeBulkOperationOutput {
            bulk_operation_description: operation.describe(now),
        })
    }
}

/// The IAM action this operation performs, which a refusal names.
const ACTION: &str = "dynamodb:DescribeBulkOperation";

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> DescribeBulkOperationError {
    DescribeBulkOperationErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}

/// Lift a store refusal into this operation's error channel.
fn lift(error: crate::state::RequestError) -> DescribeBulkOperationError {
    match error {
        crate::state::RequestError::NotFound(message) => {
            DescribeBulkOperationErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(message),
                },
            )
            .into()
        }
        crate::state::RequestError::Validation(message) => {
            crate::scaffolded::Scaffolded::validation(message)
        }
    }
}
