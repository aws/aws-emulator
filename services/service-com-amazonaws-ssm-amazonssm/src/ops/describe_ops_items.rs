// SPDX-License-Identifier: CC0-1.0

//! @op DescribeOpsItems
//! Queries the OpsItems of the request's Region: it narrows them by the request's own
//! filters, orders them, serves one page and hands back the id the next page resumes at.
//! WHICH OpsItems a set of filters selects, and the order they are reported in, live beside
//! the store ([`crate::state::ops_item_queries`]); each one is projected from the record its
//! writes persisted ([`crate::state::ops_items::OpsItemRecord::to_summary`]).
//!
//! ---
//!
//! # Modeled contract for `DescribeOpsItems`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-ssm-amazonssm/smithy-model/com-amazonaws-ssm-amazonssm.json")
//! ```
//!
//! Query a set of OpsItems. You must have permission in Identity and Access Management
//! (IAM) to query a list of OpsItems. For more information, see Set up OpsCenter in the
//! Amazon Web Services Systems Manager User Guide. Operations engineers and IT
//! professionals use Amazon Web Services Systems Manager OpsCenter to view, investigate,
//! and remediate operational issues impacting the performance and health of their Amazon
//! Web Services resources. For more information, see Amazon Web Services Systems Manager
//! OpsCenter in the Amazon Web Services Systems Manager User Guide.
//!
//! ## Input
//!
//! * `MaxResults` (OpsItemMaxResults, range 1..50) — The maximum number of items to return
//!   for this call. The call also returns a token that you can specify in a subsequent call
//!   to get the next set of results.
//! * `NextToken` (String) — A token to start the list. Use this token to get the next set of
//!   results.
//! * `OpsItemFilters` (OpsItemFilters) — One or more filters to limit the response. Key:
//!   CreatedTime Operations: GreaterThan, LessThan Key: LastModifiedBy Operations: Contains,
//!   Equals Key: LastModifiedTime Operations: GreaterThan, LessThan Key: Priority Operations:
//!   Equals Key: Source Operations: Contains, Equals Key: Status Operations: Equals Key:
//!   Title* Operations: Equals,Contains Key: OperationalData** Operations: Equals Key:
//!   OperationalDataKey Operations: Equals Key: OperationalDataValue Operations: Equals,
//!   Contains Key: OpsItemId Operations: Equals Key: ResourceId Operations: Contains Key:
//!   AutomationId Operations: Equals Key: AccountId Operations: Equals *The Equals operator
//!   for Title matches the first 100 characters. If you …
//!
//! ## Output
//!
//! * `NextToken` (String) — The token for the next set of items to return. Use this token to
//!   get the next set of results.
//! * `OpsItemSummaries` (OpsItemSummaries) — A list of OpsItems.
//!
//! ## Errors
//!
//! * `InternalServerError` (HTTP 500) — An error occurred on the server side.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `DescribeOpsItemsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssm-amazonssm.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `DescribeOpsItemsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.ssm#DescribeOpsItems")]
#[derive(Debug)]
pub enum DescribeOpsItemsErrorModeled {
    InternalServerError(InternalServerError),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_ops_items::DescribeOpsItemsError`). Wrapping the modeled enum is what
/// lets this operation refuse a request the model declares no error for with a 400
/// `ValidationException` — the answer a `NextToken` it never minted earns.
pub type DescribeOpsItemsError = crate::scaffolded::Scaffolded<DescribeOpsItemsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so the handler
/// writes `Err(SomeModeledError.into())` rather than naming the wrapper variant.
impl From<DescribeOpsItemsErrorModeled> for DescribeOpsItemsError {
    fn from(modeled: DescribeOpsItemsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeOpsItems` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct DescribeOpsItems<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeOpsItems<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeOpsItems<Eng>
{
    type Input = DescribeOpsItemsInput;
    type Output = DescribeOpsItemsOutput;
    type Error = DescribeOpsItemsError;

    const NAME: &'static str = "DescribeOpsItems";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_OPS_ITEMS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::DESCRIBE_OPS_ITEMS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Each filter's `OpsItemFilterKey` and `OpsItemFilterOperator`, and `MaxResults`'s
        // 1..50 range, are MODELLED constraints, so the centralized walk has already refused
        // anything outside them. What is read here is which of the OpsItems this Region holds
        // the request's criteria select.
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(DescribeOpsItemsErrorModeled::InternalServerError(
                InternalServerError {
                    message: Some("the OpsItem store is unavailable".to_string()),
                },
            )
            .into());
        };

        let page_size = crate::state::ops_item_queries::ops_item_page_size(input.max_results)
            .map_err(crate::scaffolded::Scaffolded::validation)?;

        // The token this operation hands out is the id of the OpsItem the next page starts
        // at, so a token that is not shaped like an OpsItem id was never one of ours.
        let resume_from = match input.next_token.as_deref() {
            None | Some("") => None,
            Some(token) if crate::state::ops_items::is_ops_item_id(token) => {
                Some(token.to_string())
            }
            Some(_) => {
                return Err(crate::scaffolded::Scaffolded::validation(
                    crate::state::INVALID_NEXT_TOKEN,
                ))
            }
        };

        // OpsCenter is regional: the OpsItems "in the current Amazon Web Services Region and
        // Amazon Web Services account" are the ones a query reads, so the Region this request
        // was signed for is the one whose OpsItems it reports.
        let region = crate::state::request_region(ctx.parts());
        let now = ::std::time::SystemTime::now();
        let mut store = state.lock();
        // A delete that had settled by now was owed whether or not anybody called, so an
        // OpsItem it removed is gone before this query resolves anything.
        store.materialize_due(now);

        let filters = input.ops_item_filters.as_deref().unwrap_or_default();
        let selected = store.queried_ops_items(&region, filters);

        let start = match resume_from.as_deref() {
            None => 0,
            Some(token) => {
                let Some(at) = selected
                    .iter()
                    .position(|record| record.ops_item_id == token)
                else {
                    // The token is shaped like an OpsItem id but names no OpsItem this result
                    // holds, so it is not a page of this result to resume.
                    return Err(crate::scaffolded::Scaffolded::validation(
                        crate::state::INVALID_NEXT_TOKEN,
                    ));
                };
                at
            }
        };
        // `start` is a position in `selected`, so the page is a suffix of it; `get` keeps that
        // in-bounds without indexing on a value the request steers.
        let page = selected.get(start..).unwrap_or_default();
        let summaries = page
            .iter()
            .take(page_size)
            .map(|record| record.to_summary())
            .collect();
        // An OpsItem is still waiting past this page: hand the caller the id it resumes at, so
        // the next request continues where this one stopped.
        let next_token = page.get(page_size).map(|record| record.ops_item_id.clone());

        Ok(DescribeOpsItemsOutput {
            ops_item_summaries: Some(summaries),
            next_token,
        })
    }
}
