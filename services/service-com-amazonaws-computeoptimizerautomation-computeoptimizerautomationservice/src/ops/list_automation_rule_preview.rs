// SPDX-License-Identifier: CC0-1.0

//! @op ListAutomationRulePreview
//! Returns a preview of the recommended actions that match an Automation rule's
//! configuration and criteria: the actions this service holds for the accounts the
//! rule type and `organizationScope` cover, restricted to the requested
//! `recommendedActionTypes` and to the `criteria` conditions, one page at a time.
//!
//! `@readonly`, so this handler writes NOTHING: the inventory it reports over is the
//! analysis output Compute Optimizer derives from observed resources
//! (`crate::state::RecommendedActionStore`), and a page token carries its own cursor.
//!
//! The refusals below are the EMULATOR's choice rather than modelled constraints, each
//! reported with the only client-input error this operation declares
//! (`InvalidParameterValueException`):
//!
//! * an `organizationScope` sent with a rule type other than `OrganizationRule`, because
//!   the scope names the accounts of an organization-wide preview and silently ignoring a
//!   member the caller sent would be worse than reporting it;
//! * an EMPTY `recommendedActionTypes`, because the member is required and names the
//!   action types to include, so an empty list selects nothing: answering an empty page
//!   would read as "your rule matches no resource" rather than "you asked for no action
//!   type";
//! * a `criteria` this service cannot evaluate — a condition with no `comparison`, with no
//!   `values`, with an operator from the wrong family, or a `resourceTag` condition with no
//!   `key` (the model types all of those as optional, so the protocol boundary admits
//!   them). None of them can select resources, so each is reported rather than quietly
//!   matching nothing.
//! * a `nextToken` that does not decode, or that was taken with a different account
//!   scope (what the rule type and `organizationScope` cover), action-type list or
//!   criteria (or from another operation): resuming a walk
//!   with it would answer a page of a different result set, so the token is refused
//!   instead.
//!
//! NOT enforced here, deliberately: the model's prose for `ruleType` says only the
//! management account or a delegated administrator may ask for `OrganizationRule`. This
//! emulator has one caller account and models no organization membership or delegated
//! administrator (nothing in this crate records either, and `CreateAutomationRule` takes
//! the same reading), so there is no entitlement to check and the modelled
//! `AccessDeniedException` / `ForbiddenException` are unreachable from this handler: the
//! single caller stands in for the management account. A caller that must see the refusal
//! cannot get it from this emulator.
//!
//! `maxResults` is another emulator reading, not a refusal: the model bounds it at
//! `range(min: 1, max: 100)` and the clipped prose transcribed below says 1-1000, so this
//! handler follows the MODEL, which is the constraint the protocol boundary enforces.
//!
//! ---
//!
//! # Modeled contract for `ListAutomationRulePreview`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json")
//! ```
//!
//! Returns a preview of the recommended actions that match your Automation rule's
//! configuration and criteria.
//!
//! ## Input
//!
//! * `criteria` (Criteria) — A set of conditions that specify which recommended action
//!   qualify for implementation. When a rule is active and a recommended action matches these
//!   criteria, Compute Optimizer implements the action at the scheduled run time.
//! * `maxResults` (Integer) — The maximum number of automation rule preview results to return
//!   in a single response. Valid range is 1-1000. (The shape itself carries
//!   `range(min: 1, max: 100)`, which is the constraint the protocol boundary enforces; see
//!   the header above.)
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination to
//!   retrieve the next set of results when the response is truncated.
//! * `organizationScope` (OrganizationScope) — The organizational scope for the rule preview.
//! * `recommendedActionTypes` (RecommendedActionTypeList) **required** — The types of
//!   recommended actions to include in the preview.
//! * `ruleType` (RuleType, one of: AccountRule | OrganizationRule) **required** — The type of
//!   rule. Only the management account or a delegated administrator can set the ruleType to
//!   be OrganizationRule.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination. If
//!   present, indicates there are more results available and can be used in subsequent
//!   requests.
//! * `previewResults` (PreviewResults) — The list of actions that would be taken based on the
//!   specified criteria.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — You do not have sufficient permissions to perform
//!   this action.
//! * `ForbiddenException` (HTTP 403) — You are not authorized to perform this action.
//! * `InternalServerException` (HTTP 500) — An internal error occurred while processing the
//!   request.
//! * `InvalidParameterValueException` (HTTP 400) — One or more parameter values are not
//!   valid.
//! * `OptInRequiredException` (HTTP 403) — The account must be opted in to Compute Optimizer
//!   Automation before performing this action.
//! * `ServiceUnavailableException` (HTTP 503) — The service is temporarily unavailable.
//! * `ThrottlingException` (HTTP 429) — The request was denied due to request throttling.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAutomationRulePreviewError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAutomationRulePreviewError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.computeoptimizerautomation#ListAutomationRulePreview")]
#[derive(Debug)]
pub enum ListAutomationRulePreviewErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ForbiddenException(ForbiddenException),
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    OptInRequiredException(OptInRequiredException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_automation_rule_preview::ListAutomationRulePreviewError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAutomationRulePreviewError = crate::scaffolded::Scaffolded<ListAutomationRulePreviewErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAutomationRulePreviewErrorModeled> for ListAutomationRulePreviewError {
    fn from(modeled: ListAutomationRulePreviewErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAutomationRulePreview` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAutomationRulePreview<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAutomationRulePreview<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAutomationRulePreview<Eng>
{
    type Input = ListAutomationRulePreviewInput;
    type Output = ListAutomationRulePreviewOutput;
    type Error = ListAutomationRulePreviewError;

    const NAME: &'static str = "ListAutomationRulePreview";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListAutomationRulePreviewErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "the service is not mounted on a state-bearing engine".into(),
                        ),
                    },
                )
                .into(),
            );
        };

        // Rules the model cannot express, so the protocol boundary let them
        // through, judged by the one check both preview operations share
        // (`crate::state::preview_selection_violation`, beside the
        // `previewed_actions` it guards) so that a request this operation
        // cannot evaluate is refused the same way by its sibling. Reported with
        // the only client-input error this operation declares, worded by the
        // service's one validation reporter, which echoes no request value.
        if let Some(report) = crate::state::preview_selection_violation(
            &input.rule_type,
            input.organization_scope.as_ref(),
            &input.recommended_action_types,
            input.criteria.as_ref(),
        ) {
            return Err(invalid_parameter(report));
        }

        // The selection this request names, derived exactly as
        // `crate::state::previewed_actions` performs it below: the accounts the
        // rule type and `organizationScope` cover, and the criteria after
        // defaulting — a caller that sends no criteria constrains nothing, so
        // every action in scope carrying a requested action type is previewed.
        let scope = crate::state::preview_account_scope(
            &input.rule_type,
            input.organization_scope.as_ref(),
        );
        let criteria = input.criteria.unwrap_or_default();

        // The page token carries the operation and that DERIVED selection, so
        // resuming a walk with a different scope, action-type list or criteria —
        // or with another operation's token — is refused instead of answered
        // with a page of a different result set, while two spellings of one
        // selection continue one walk: `crate::state::preview_selection_fingerprint`.
        let selection_fingerprint = crate::state::preview_selection_fingerprint(
            Self::NAME,
            &scope,
            &input.recommended_action_types,
            &criteria,
        );
        // Where this call resumes, by the one rule every paginated operation of
        // this service enters its walk under: `crate::state::page_entry_offset`.
        // Only the WORDING of each refusal is this operation's own, because it
        // names what the token had to have been taken for.
        let offset =
            crate::state::page_entry_offset(input.next_token.as_deref(), &selection_fingerprint)
                .map_err(|refusal| match refusal {
                    crate::state::PageEntryRefusal::Unissued => unissued_token(),
                    crate::state::PageEntryRefusal::OtherQuery => {
                        invalid_parameter(crate::state::constraint_report(
                            "nextToken",
                            "Member must be a token returned for the same rule type, action types \
                             and criteria",
                        ))
                    }
                })?;

        // The model bounds `maxResults` at 1..=100, so a caller that sends none
        // gets the largest page it could have asked for, and the protocol
        // boundary has already refused anything outside that range. One rule for
        // every paginated listing here, including the floor against a
        // non-advancing zero page: `crate::state::page_size`.
        let page_size = crate::state::page_size(input.max_results);

        // The organization the caller's standing in each account is resolved
        // through, read from the Organizations store — which owns the root, the
        // OUs and the node each member account sits under — before this
        // service's store is locked, so no call holds two stores at once.
        let hierarchy = crate::state::OrganizationHierarchy::read(&*self.engine);

        let guard = state.lock();
        // The service-wide enrollment gate, AHEAD of the inventory walk: an
        // account that is not opted in is refused the whole call rather than
        // handed an (empty) preview that tooling would then build on. See
        // `crate::state::…::opt_in_required`.
        if let Some(refusal) = guard.opt_in_required() {
            return Err(
                ListAutomationRulePreviewErrorModeled::OptInRequiredException(refusal).into(),
            );
        }
        // The selection itself — scope, requested action types, criteria — is
        // `crate::state::previewed_actions`, so this operation and
        // `ListAutomationRulePreviewSummaries` report over exactly the same set
        // of actions for the same request. Paging it is the one page walk every
        // paginated listing of this service performs, entered the way a cursor
        // enters it: `crate::state::resume_page` answers nothing for a cursor
        // naming a position this selection does not reach, which is a token this
        // listing never minted and so the same refusal as one that does not
        // decode at all.
        let served: Option<(Vec<PreviewResult>, Option<String>)> = crate::state::resume_page(
            crate::state::previewed_actions(
                &guard,
                &hierarchy,
                &scope,
                &input.recommended_action_types,
                &criteria,
            )
            .map(crate::state::preview_result_for),
            offset,
            page_size,
            selection_fingerprint,
        );
        let Some((page, next_token)) = served else {
            return Err(unissued_token());
        };

        Ok(ListAutomationRulePreviewOutput {
            preview_results: Some(page),
            next_token,
        })
    }
}

fn invalid_parameter(message: String) -> ListAutomationRulePreviewError {
    ListAutomationRulePreviewErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 400 for a `nextToken` this operation did not issue: one that
/// decodes to no cursor at all, or one naming a position the selection this
/// listing pages does not reach.
fn unissued_token() -> ListAutomationRulePreviewError {
    invalid_parameter(crate::state::constraint_report(
        "nextToken",
        "Member must be a token returned by a previous ListAutomationRulePreview call",
    ))
}
