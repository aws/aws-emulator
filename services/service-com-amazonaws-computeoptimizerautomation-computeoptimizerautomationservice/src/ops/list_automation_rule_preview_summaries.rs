// SPDX-License-Identifier: CC0-1.0

//! @op ListAutomationRulePreviewSummaries
//! Summarises the recommended actions an Automation rule's preview selects: the
//! SAME set `ListAutomationRulePreview` reports one by one (the actions this service
//! holds for the accounts the rule type and `organizationScope` cover, restricted to
//! the requested `recommendedActionTypes` and to the `criteria` conditions), reduced
//! to the ONE account-wide total of that selection — how many actions it holds and
//! the sum of their estimated monthly savings.
//!
//! `PreviewResultSummary.key` is documented only as "The key identifier for this
//! preview result summary" and this shape, unlike `AutomationEventSummary`, carries
//! no `dimensions` member naming a dimension to break the selection down by
//! (`smithy_query(query="shape",shape="PreviewResultSummary,AutomationEventSummary,SummaryDimension")`).
//! The summary is therefore the aggregate of the whole selection, keyed by the fixed
//! text [`crate::state::ALL_RECOMMENDED_ACTION_SUMMARY_KEY`] that every
//! recommended-action summary row of this service carries (see
//! [`crate::state::aggregate_recommended_actions`]) and derived from no request
//! member: exactly one row per request, present with a count of zero for a selection
//! that holds nothing, so a caller can size a rule's impact before activating it and
//! cross-check the total against the actions the unsummarised preview lists.
//!
//! `@readonly`, so this handler writes NOTHING: the inventory it reports over is the
//! analysis output Compute Optimizer derives from observed resources
//! (`crate::state::RecommendedActionStore`), and a page token carries its own cursor.
//!
//! The refusals below are the EMULATOR's choice rather than modelled constraints, each
//! reported with the only client-input error this operation declares
//! (`InvalidParameterValueException`), and each the same reading its unsummarised
//! sibling takes — one request cannot be valid for a preview and invalid for that
//! preview's totals:
//!
//! * an `organizationScope` sent with a rule type other than `OrganizationRule`;
//! * an EMPTY `recommendedActionTypes`, which selects no action type at all;
//! * a `criteria` this service cannot evaluate (a condition with no `comparison`, no
//!   `values`, an operator from the wrong family, or a `resourceTag` condition with no
//!   `key`);
//! * a `nextToken` that does not decode, or that was taken with a different account
//!   scope (what the rule type and `organizationScope` cover),
//!   action-type list or criteria — or from another operation, including
//!   `ListAutomationRulePreview`, whose pages are single actions rather than groups.
//!
//! NOT enforced here, deliberately: `ruleType`'s prose says only the management account
//! or a delegated administrator may ask for `OrganizationRule`. This emulator has one
//! caller account and models no organization membership or delegated administrator, so
//! there is no entitlement to check and the modelled `AccessDeniedException` /
//! `ForbiddenException` are unreachable from this handler, exactly as in
//! `ListAutomationRulePreview`.
//!
//! `maxResults` is an emulator reading, not a refusal: the model bounds it at
//! `range(min: 1, max: 100)` while the clipped prose transcribed below says 1-1000, so
//! this handler follows the MODEL, which is the constraint the protocol boundary
//! enforces.
//!
//! ---
//!
//! # Modeled contract for `ListAutomationRulePreviewSummaries`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json")
//! ```
//!
//! Returns a summary of the recommended actions that match your rule preview configuration
//! and criteria.
//!
//! ## Input
//!
//! * `criteria` (Criteria) — A set of conditions that specify which recommended action
//!   qualify for implementation. When a rule is active and a recommended action matches these
//!   criteria, Compute Optimizer implements the action at the scheduled run time. You can
//!   specify up to 20 conditions per filter criteria and 20 values per condition.
//! * `maxResults` (Integer) — The maximum number of automation rule preview summaries to
//!   return in a single response. Valid range is 1-1000.
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination to
//!   retrieve the next set of results when the response is truncated.
//! * `organizationScope` (OrganizationScope) — The organizational scope for the rule preview.
//! * `recommendedActionTypes` (RecommendedActionTypeList) **required** — The types of
//!   recommended actions to include in the preview.
//! * `ruleType` (RuleType, one of: AccountRule | OrganizationRule) **required** — The type of
//!   rule.
//!
//! ## Output
//!
//! * `nextToken` (NextToken, pattern ^[A-Za-z0-9+/=]+$) — A token used for pagination. If
//!   present, indicates there are more results available and can be used in subsequent
//!   requests.
//! * `previewResultSummaries` (PreviewResultSummaries) — The list of automation rule preview
//!   summaries that match the specified criteria.
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
// `ListAutomationRulePreviewSummariesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAutomationRulePreviewSummariesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.computeoptimizerautomation#ListAutomationRulePreviewSummaries")]
#[derive(Debug)]
pub enum ListAutomationRulePreviewSummariesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    ForbiddenException(ForbiddenException),
    InternalServerException(InternalServerException),
    InvalidParameterValueException(InvalidParameterValueException),
    OptInRequiredException(OptInRequiredException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_automation_rule_preview_summaries::ListAutomationRulePreviewSummariesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAutomationRulePreviewSummariesError = crate::scaffolded::Scaffolded<ListAutomationRulePreviewSummariesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAutomationRulePreviewSummariesErrorModeled> for ListAutomationRulePreviewSummariesError {
    fn from(modeled: ListAutomationRulePreviewSummariesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAutomationRulePreviewSummaries` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAutomationRulePreviewSummaries<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAutomationRulePreviewSummaries<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAutomationRulePreviewSummaries<Eng>
{
    type Input = ListAutomationRulePreviewSummariesInput;
    type Output = ListAutomationRulePreviewSummariesOutput;
    type Error = ListAutomationRulePreviewSummariesError;

    const NAME: &'static str = "ListAutomationRulePreviewSummaries";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_SUMMARIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATION_RULE_PREVIEW_SUMMARIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListAutomationRulePreviewSummariesErrorModeled::InternalServerException(
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
        // every action in scope carrying a requested action type is summarised.
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
        // handed an (empty) summary that tooling would then build on. See
        // `crate::state::…::opt_in_required`.
        if let Some(refusal) = guard.opt_in_required() {
            return Err(
                ListAutomationRulePreviewSummariesErrorModeled::OptInRequiredException(refusal)
                    .into(),
            );
        }

        // The ONE aggregate row the selected actions total up to: the selection
        // is what gets summarised, never the request's action-type list, and the
        // row is reported even for a selection that holds nothing. The
        // aggregation is the one every recommended-action summary list of this
        // service performs, so it lives in
        // [`crate::state::aggregate_recommended_actions`].
        let grouped = crate::state::aggregate_recommended_actions(
            crate::state::previewed_actions(
                &guard,
                &hierarchy,
                &scope,
                &input.recommended_action_types,
                &criteria,
            ),
        );

        // The rows are what gets paged, and the page walk is the one every
        // paginated listing of this service performs, entered the way a cursor
        // enters it: `crate::state::resume_page` answers nothing for a cursor
        // naming a position this sequence of rows does not reach, which is a
        // token this listing never minted. One aggregate row fits any page this
        // operation can be asked for, so the walk mints no token here. Each row
        // is projected into the modelled summary afterwards, so the projection
        // runs only for the page served.
        let served: Option<(Vec<crate::state::ActionGroup>, Option<String>)> =
            crate::state::resume_page(grouped, offset, page_size, selection_fingerprint);
        let Some((groups, next_token)) = served else {
            return Err(unissued_token());
        };
        let page: Vec<PreviewResultSummary> = groups
            .into_iter()
            .map(|group| PreviewResultSummary {
                key: group.key,
                total: RulePreviewTotal {
                    recommended_action_count: group.count,
                    estimated_monthly_savings: crate::state::total_savings(group.estimates.iter()),
                },
            })
            .collect();

        Ok(ListAutomationRulePreviewSummariesOutput {
            preview_result_summaries: Some(page),
            next_token,
        })
    }
}

fn invalid_parameter(message: String) -> ListAutomationRulePreviewSummariesError {
    ListAutomationRulePreviewSummariesErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 400 for a `nextToken` this operation did not issue: one that
/// decodes to no cursor at all, or one naming a position the sequence of groups
/// this listing pages does not reach.
fn unissued_token() -> ListAutomationRulePreviewSummariesError {
    invalid_parameter(crate::state::constraint_report(
        "nextToken",
        "Member must be a token returned by a previous ListAutomationRulePreviewSummaries call",
    ))
}
