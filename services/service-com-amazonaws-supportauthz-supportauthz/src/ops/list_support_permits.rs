// SPDX-License-Identifier: CC0-1.0

//! @op ListSupportPermits
//!
//! Hand-authored: the scaffold op-stub generator declined this operation
//! (`src/ops/SKIPPED_STUBS.md`) because its `Permit` shape reaches two unions
//! whose "all" member targets the Smithy prelude shape `smithy.api#Unit`. This
//! file is patterned on the rendered siblings in this directory and is the ONLY
//! file to touch for this operation.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! ---
//!
//! # Modeled contract for `ListSupportPermits`
//!
//! From this service's Smithy model. For the FULL text of any one item:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-supportauthz-supportauthz/smithy-model/com-amazonaws-supportauthz-supportauthz.json")
//! ```
//!
//! Lists all support permits in the caller's account. Use pagination to ensure that the
//! operation returns quickly and successfully.
//!
//! **Route:** `GET /support-permits` -> 200, `@readonly`,
//! `@paginated(inputToken=nextToken, outputToken=nextToken, items=supportPermits)`
//!
//! ## Input
//!
//! * `nextToken` (NextToken, len 1..2048, pattern ^[a-zA-Z0-9/+=%_-]+$,
//!   @httpQuery(nextToken))
//! * `maxResults` (MaxResults, range 1..100, @httpQuery(maxResults))
//! * `supportPermitStatuses` (list<SupportPermitStatus>, len 1..5,
//!   @httpQuery(supportPermitStatuses))
//!
//! ## Output
//!
//! * `supportPermits` (list<SupportPermitSummary>, len 0..100) **required**
//! * `nextToken` (NextToken, len 1..2048, pattern ^[a-zA-Z0-9/+=%_-]+$)
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403), `InternalServerException` (HTTP 500),
//!   `ThrottlingException` (HTTP 429), `ValidationException` (HTTP 400).

#[allow(unused_imports)]
use super::types::*;

// The shapes of this operation the scaffold declines to render, declared from the
// model in this crate's own source; `crate::service` is where that module is
// declared, because the `@generated` `types/mod.rs` cannot keep a `mod` line.
#[allow(unused_imports)]
use crate::service::types_local::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-supportauthz-supportauthz.json");

/// The operation's MODELED errors.
#[smithy_macros::operation_error("com.amazonaws.supportauthz#ListSupportPermits")]
#[derive(Debug)]
pub enum ListSupportPermitsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it.
pub type ListSupportPermitsError = crate::scaffolded::Scaffolded<ListSupportPermitsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<ListSupportPermitsErrorModeled> for ListSupportPermitsError {
    fn from(modeled: ListSupportPermitsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListSupportPermits` operation.
pub struct ListSupportPermits<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListSupportPermits<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListSupportPermits<Eng>
{
    type Input = ListSupportPermitsInput;
    type Output = ListSupportPermitsOutput;
    type Error = ListSupportPermitsError;

    const NAME: &'static str = "ListSupportPermits";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::LIST_SUPPORT_PERMITS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        crate::service::types_local::LIST_SUPPORT_PERMITS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // Request metadata only; the store is reached through `self.engine`.
        let _ = ctx;

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            ListSupportPermitsErrorModeled::InternalServerException(InternalServerException {
                message: "SupportAuthZ permit state is unavailable on this engine.".to_string(),
                retry_after_seconds: None,
            })
        })?;
        let mut state = state.lock();

        // ONE instant decides both the clock's work and this read: a permit
        // whose `DeleteSupportPermit` removal was already due is gone whether or
        // not anybody called. Materialising that, and minting the page cursor
        // handed back below, are the only writes this reporting operation makes.
        state.purge_due(::std::time::SystemTime::now());

        // `supportPermitStatuses` targets the `SupportPermitStatus` ENUM shape.
        // The scaffold types an enum member as a plain string, so nothing
        // between the wire and here enforces its value set — this does, against
        // the model's own value list.
        if let Some(statuses) = input.support_permit_statuses.as_deref() {
            if let Some(unknown) = statuses
                .iter()
                .find(|status| !SUPPORT_PERMIT_STATUS_VALUES.contains(&status.as_str()))
            {
                return Err(ListSupportPermitsErrorModeled::ValidationException(
                    ValidationException {
                        message: format!(
                            "supportPermitStatuses must satisfy enum value set: [{}]",
                            SUPPORT_PERMIT_STATUS_VALUES.join(", ")
                        ),
                        field_list: Some(vec![ValidationExceptionField {
                            path: "supportPermitStatuses".to_string(),
                            message: format!("'{unknown}' is not a SupportPermitStatus."),
                        }]),
                    },
                )
                .into());
            }
        }

        // A `nextToken` is RESOLVED against the cursors this service minted,
        // never parsed: a token it did not hand out names no page here.
        let after = match input.next_token.as_deref() {
            None => None,
            Some(token) => match state.cursor_for(token) {
                Some(after) => Some(after.to_string()),
                None => {
                    return Err(ListSupportPermitsErrorModeled::ValidationException(
                        ValidationException {
                            message: "The nextToken provided is not a valid pagination token for \
                                      ListSupportPermits."
                                .to_string(),
                            field_list: Some(vec![ValidationExceptionField {
                                path: "nextToken".to_string(),
                                message: "No page is outstanding for this token.".to_string(),
                            }]),
                        },
                    )
                    .into());
                }
            },
        };

        // `maxResults` caps the page; absent, the page is the modeled maximum.
        // The clamp is belt-and-braces — the modeled 1..100 range is refused
        // before `handle` runs.
        let limit = input
            .max_results
            .unwrap_or(crate::state::MAX_RESULTS)
            .clamp(1, crate::state::MAX_RESULTS) as usize;

        let (support_permits, resume) = state.page(
            after.as_deref(),
            input.support_permit_statuses.as_deref(),
            limit,
        );
        // A token is handed back only when a matching permit remains beyond this
        // page, so the last page carries none.
        let next_token = resume.map(|arn| state.token_for(&arn));

        Ok(ListSupportPermitsOutput {
            support_permits,
            next_token,
        })
    }
}
