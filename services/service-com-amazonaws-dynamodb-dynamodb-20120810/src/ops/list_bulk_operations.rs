// SPDX-License-Identifier: CC0-1.0

//! @op ListBulkOperations
//! IMPLEMENTED: reports one `BulkOperationSummary` per bulk operation of the region the
//! request reached — the operation's own ARN, the status it is in and the instants it began
//! and (once it has) ended — paged by `MaxResults` with a `NextToken` that stands for the
//! entry the page stopped on.
//!
//! THE WHOLE REGION'S OPERATIONS: the request carries no member that narrows the listing
//! (`smithy_query(query="operation", operation="ListBulkOperations")` gives `MaxResults` and
//! `NextToken` and nothing else), so every bulk operation the account holds in the region
//! the endpoint serves is reported, and the table each one acted on is read back through
//! `DescribeBulkOperation`.
//!
//! A REPORTING operation: every summary is read from the bulk-operation catalogue
//! `StartBulkPutItems` writes ([`crate::state::BulkOperation`]), and the status each one
//! reports is derived from the clock at every read, so the only write it makes is what the
//! clock had already made due and two callers reading it see the same thing.
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
/// bulk-operation readers widen it in (`super::describe_bulk_operation`): this SERVICE models
/// the shape, this operation's own `errors` list omits it
/// (`smithy_query(query="operation", operation="ListBulkOperations")`), and a listing that
/// cannot reach this service's store has failed on the server's side of the call.
#[smithy_macros::operation_error(
    "com.amazonaws.dynamodb#ListBulkOperations",
    extra_errors = ["InternalServerError"]
)]
#[derive(Debug)]
pub enum ListBulkOperationsErrorModeled {
    InternalServerError(InternalServerError),
    InvalidEndpointException(InvalidEndpointException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_bulk_operations::ListBulkOperationsError`).
pub type ListBulkOperationsError = crate::scaffolded::Scaffolded<ListBulkOperationsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListBulkOperationsErrorModeled> for ListBulkOperationsError {
    fn from(modeled: ListBulkOperationsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListBulkOperations` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this crate
/// names no concrete engine type.
pub struct ListBulkOperations<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListBulkOperations<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListBulkOperations<Eng>
{
    type Input = ListBulkOperationsInput;
    type Output = ListBulkOperationsOutput;
    type Error = ListBulkOperationsError;

    const NAME: &'static str = "ListBulkOperations";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::LIST_BULK_OPERATIONS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::LIST_BULK_OPERATIONS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        // A `NextToken` must be one this operation minted, so a token it did not stands for
        // no page and is refused rather than quietly taken for the first one.
        let cursor = input.next_token.as_deref().map(decode_token).transpose()?;
        // WHO this request speaks for, resolved before this service's store is locked: the
        // listing is authorized against the caller's own identity-based policies, over the
        // account's whole table namespace, since the request narrows itself to no table
        // ([`crate::state::authorization`]).
        let caller = crate::state::request_caller(&*self.engine, ctx);
        if let Some(denial) = crate::state::authorization::account_denial(
            &caller,
            ACTION,
            &crate::state::authorization::account_resource(&region, None),
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }

        let mut store = state.lock();
        // Finish what the clock has already made due before reporting. This is a read, so
        // that is the only write it makes.
        store.apply_due_transitions(now);
        // A reporting operation, and one that reports nothing a WRITE could settle: a
        // bulk-operation record is fixed once written and the status it reports is derived
        // from the clock on each read. So this reads only.
        let mut summaries: Vec<BulkOperationSummary> = store
            .bulk_operations_in(&region)
            // The cursor: this page starts strictly after the entry the previous one
            // stopped on, which that page's `NextToken` carried.
            .filter(|operation| {
                cursor
                    .as_deref()
                    .is_none_or(|start| operation.arn.as_str() > start)
            })
            .map(|operation| operation.summary(now))
            .collect();
        drop(store);

        // The page size the request asked for, or as many as one page may carry when it
        // asked for none. The model's `@range` on that member admits only a positive count,
        // so a page size this reads is one the validating walk ahead of it has already
        // admitted; a value outside `usize` could therefore only be one of those, and it is
        // read as the empty page rather than as an unbounded one.
        let limit = usize::try_from(input.max_results.unwrap_or(DEFAULT_MAX_RESULTS)).unwrap_or(0);
        let truncated = summaries.len() > limit;
        summaries.truncate(limit);
        // Reported only where entries remain beyond this page, and standing for the entry
        // the page stopped on.
        let next_token = truncated
            .then(|| {
                summaries
                    .last()
                    .map(|summary| encode_token(&summary.bulk_operation_arn))
            })
            .flatten();

        Ok(ListBulkOperationsOutput {
            bulk_operations_summary_list: Some(summaries),
            next_token,
        })
    }
}

/// The page size this operation reports when the request names no `MaxResults`: the ceiling
/// the model declares for that member
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#ListBulkOperationsMaxLimit")`
/// gives `@range` 1..25), so a request that asks for no bound is answered with as many
/// operations as one page may carry.
const DEFAULT_MAX_RESULTS: i32 = 25;

/// The IAM action this operation performs, which a refusal names.
const ACTION: &str = "dynamodb:ListBulkOperations";

/// The bytes one group of the token's pattern carries: `^([0-9a-f]{16})+$`
/// (`smithy_query(query="shape", shape="com.amazonaws.dynamodb#BulkOperationNextToken")`) is
/// a whole number of 16-hex-digit groups, and 16 hex digits are 8 bytes.
const TOKEN_GROUP_BYTES: usize = 8;

/// The cursor a truncated page reports, in the form the `BulkOperationNextToken` member
/// models: the entry the page stopped on, as its bytes in hex, padded with NULs to the
/// 8-byte boundary that member's 16-hex-digit groups require.
///
/// A token is therefore opaque to a client and carries nothing but that entry, and it
/// satisfies the member's own constraint — so the value this operation reports is one the
/// next request may send straight back. The one padding rule a token of this service is
/// padded by, which its sibling listing states too (`super::list_imports`).
fn encode_token(cursor: &str) -> String {
    let mut bytes = cursor.as_bytes().to_vec();
    while !bytes.len().is_multiple_of(TOKEN_GROUP_BYTES) {
        bytes.push(0);
    }
    ::hex::encode(bytes)
}

/// The cursor a `NextToken` stands for, or the refusal for a token this operation did not
/// mint.
fn decode_token(token: &str) -> Result<String, ListBulkOperationsError> {
    let mut bytes = ::hex::decode(token).map_err(|_| invalid_next_token())?;
    // Drop the NUL padding [`encode_token`] added, without indexing: the position is
    // derived from the vector itself, so there is no request-derived index at all.
    let unpadded = bytes
        .iter()
        .rposition(|byte| *byte != 0)
        .map_or(0, |last| last + 1);
    bytes.truncate(unpadded);
    let cursor = String::from_utf8(bytes).map_err(|_| invalid_next_token())?;
    if cursor.is_empty() {
        return Err(invalid_next_token());
    }
    Ok(cursor)
}

/// How a `NextToken` that is not one of this operation's is refused: the wording this
/// service states for that case wherever a reader pages by a token.
fn invalid_next_token() -> ListBulkOperationsError {
    crate::scaffolded::Scaffolded::validation("Invalid NextToken")
}

/// The modelled server error for a request that cannot reach this service's store.
fn store_unavailable() -> ListBulkOperationsError {
    ListBulkOperationsErrorModeled::InternalServerError(InternalServerError {
        message: Some("the DynamoDB table store is unavailable".to_string()),
    })
    .into()
}
