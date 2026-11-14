// SPDX-License-Identifier: CC0-1.0

//! @op ListAutomatedReasoningPolicies
//!
//! Reports the Automated Reasoning policy revisions of [`crate::state`] as summaries:
//! filtered, sorted and paged, each read out of the stored record, so a policy
//! `CreateAutomatedReasoningPolicy` minted is listed here exactly as
//! `GetAutomatedReasoningPolicy` reads it back.
//!
//! WHICH revisions are in scope is the `policyArn` filter's rule:
//!
//! * no filter — the WORKING DRAFT of every policy of the signing region, as `policyArn`
//!   documents ("If not provided, the DRAFT versions for all policies are listed"). An
//!   `AutomatedReasoningPolicyArn` carries a region, so a policy of another region belongs
//!   to another account-region's listing.
//! * an unversioned ARN — the revisions of the one policy it names: its working draft and
//!   every numbered version cut from that draft.
//! * an ARN carrying the `:<version>` suffix the shape admits — that one version.
//!
//! An ARN this account holds no such revision under is refused with
//! `ResourceNotFoundException` rather than answered as an empty listing. WHICH sentence
//! that refusal carries is this operation's own: Amazon Bedrock words the absent policy per
//! operation, so the listing answers
//! [`crate::state::AUTOMATED_REASONING_POLICY_NOT_FOUND_ON_LISTING`] where a read of one
//! policy answers [`crate::state::AUTOMATED_REASONING_POLICY_NOT_FOUND`]. The missing
//! VERSION of a policy that IS there is the one sentence every operation reports it in
//! ([`crate::state::AUTOMATED_REASONING_POLICY_VERSION_NOT_FOUND`]).
//!
//! The listing is ordered by `(createdAt, policyArn)` — a TOTAL order, which is what lets
//! its pages partition the revisions — and `nextToken` is the cursor of the row the next
//! page starts on, so the pages carry neither a gap nor a repeat. This operation only
//! REPORTS (`@readonly`): it writes nothing.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListAutomatedReasoningPolicies`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json")
//! ```
//!
//! Lists all Automated Reasoning policies in your account, with optional filtering by
//! policy ARN. This helps you manage and discover existing policies.
//!
//! ## Input
//!
//! * `maxResults` (MaxResults, range 1..1000, default 25) **required** — The maximum number
//!   of policies to return in a single call.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The pagination token from a
//!   previous request to retrieve the next page of results.
//! * `policyArn` (AutomatedReasoningPolicyArn, len 1..2048, pattern
//!   ^arn:aws(-[^:]+)?:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:automated-reasoning-policy/[a-z0-9]{12}(:([1-9][0-9]{0,11}))?$)
//!   — Optional filter to list only the policy versions with the specified Amazon Resource
//!   Name (ARN). If not provided, the DRAFT versions for all policies are listed.
//!
//! ## Output
//!
//! * `automatedReasoningPolicySummaries` (AutomatedReasoningPolicySummaries, len 0..1000)
//!   **required** — A list of Automated Reasoning policy summaries.
//! * `nextToken` (PaginationToken, len 1..2048, pattern ^\S*$) — The pagination token to use
//!   in a subsequent request to retrieve the next page of results.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The request is denied because of missing access
//!   permissions.
//! * `InternalServerException` (HTTP 500) — An internal server error occurred. Retry your
//!   request.
//! * `ResourceNotFoundException` (HTTP 404) — The specified resource Amazon Resource Name
//!   (ARN) was not found. Check the Amazon Resource Name (ARN) and try your request again.
//! * `ThrottlingException` (HTTP 429) — The number of requests exceeds the limit. Resubmit
//!   your request later.
//! * `ValidationException` (HTTP 400) — Input validation failed. Check your request
//!   parameters and retry the request.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListAutomatedReasoningPoliciesError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListAutomatedReasoningPoliciesError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.bedrock#ListAutomatedReasoningPolicies")]
#[derive(Debug)]
pub enum ListAutomatedReasoningPoliciesErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_automated_reasoning_policies::ListAutomatedReasoningPoliciesError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListAutomatedReasoningPoliciesError = crate::scaffolded::Scaffolded<ListAutomatedReasoningPoliciesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListAutomatedReasoningPoliciesErrorModeled> for ListAutomatedReasoningPoliciesError {
    fn from(modeled: ListAutomatedReasoningPoliciesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListAutomatedReasoningPolicies` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListAutomatedReasoningPolicies<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListAutomatedReasoningPolicies<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The largest page this listing answers with: `AutomatedReasoningPolicySummaries` is
/// modelled `len 0..1000` and `maxResults` `range 1..1000`, so 1000 is the whole span
/// either admits.
const MAX_PAGE_SIZE: usize = 1000;

/// The page size a request that names none takes: the `maxResults` member's own modelled
/// `default` (`smithy_query(query="operation",operation="ListAutomatedReasoningPolicies")`).
const DEFAULT_PAGE_SIZE: usize = 25;

/// How a policy's WORKING DRAFT is summarised: the identity and instants a read of the
/// policy's unversioned ARN reports, minus the members the summary does not model.
fn draft_summary(
    policy: &crate::state::AutomatedReasoningPolicyRecord,
) -> AutomatedReasoningPolicySummary {
    AutomatedReasoningPolicySummary {
        policy_arn: policy.arn.clone(),
        name: policy.name.clone(),
        description: policy.description.clone(),
        // The unversioned ARN names the policy's working version, which the create
        // documents as always DRAFT.
        version: crate::state::DRAFT_VERSION.to_string(),
        policy_id: policy.id.clone(),
        created_at: crate::state::date_time(policy.created_at),
        updated_at: crate::state::date_time(policy.updated_at),
    }
}

/// How one NUMBERED version is summarised: the snapshot it froze, under its own versioned
/// ARN, with the policy's `policyId` — a version is a revision of that one policy. Nothing
/// rewrites a version, so the instant it was cut is also its last update.
fn version_summary(
    policy: &crate::state::AutomatedReasoningPolicyRecord,
    version: &crate::state::AutomatedReasoningPolicyVersionRecord,
) -> AutomatedReasoningPolicySummary {
    AutomatedReasoningPolicySummary {
        policy_arn: version.arn.clone(),
        name: version.name.clone(),
        description: version.description.clone(),
        version: version.version.to_string(),
        policy_id: policy.id.clone(),
        created_at: crate::state::date_time(version.created_at),
        updated_at: crate::state::date_time(version.created_at),
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListAutomatedReasoningPolicies<Eng>
{
    type Input = ListAutomatedReasoningPoliciesInput;
    type Output = ListAutomatedReasoningPoliciesOutput;
    type Error = ListAutomatedReasoningPoliciesError;

    const NAME: &'static str = "ListAutomatedReasoningPolicies";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATED_REASONING_POLICIES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_AUTOMATED_REASONING_POLICIES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(
                ListAutomatedReasoningPoliciesErrorModeled::InternalServerException(
                    InternalServerException {
                        message: Some(
                            "automated reasoning policy state is not available on this engine"
                                .to_string(),
                        ),
                    },
                )
                .into(),
            );
        };
        let region = crate::state::request_region(ctx.parts());
        let store = state.lock();

        let mut rows: Vec<(String, AutomatedReasoningPolicySummary)> = match input
            .policy_arn
            .as_deref()
        {
            // The filter names ONE policy, resolved as the caller spelled it exactly as a
            // read of that policy resolves it: a revision this account holds none of is
            // refused rather than reported as an empty listing.
            Some(policy_arn) => {
                let Some((policy, version)) =
                    store.automated_reasoning_policy_revision(policy_arn)
                else {
                    // Whether the refusal reports the missing version or the missing policy
                    // is the store's own rule — which half of the ARN resolved to nothing —
                    // and the sentence it words the missing POLICY in is THIS operation's,
                    // which is not the sentence a read of one policy answers.
                    return Err(
                        ListAutomatedReasoningPoliciesErrorModeled::ResourceNotFoundException(
                            ResourceNotFoundException {
                                message: Some(
                                    store.automated_reasoning_policy_listing_not_found_message(
                                        policy_arn,
                                    ),
                                ),
                            },
                        )
                        .into(),
                    );
                };
                match version {
                    // A versioned ARN names exactly one revision, so the listing holds
                    // exactly that one.
                    Some(version) => vec![(
                        crate::state::listing_cursor(version.created_at, &version.arn),
                        version_summary(policy, version),
                    )],
                    // An unversioned ARN names the policy itself: its working draft, and
                    // every version cut from that draft.
                    None => ::std::iter::once((
                        crate::state::listing_cursor(policy.created_at, &policy.arn),
                        draft_summary(policy),
                    ))
                    .chain(policy.versions.values().map(|version| {
                        (
                            crate::state::listing_cursor(version.created_at, &version.arn),
                            version_summary(policy, version),
                        )
                    }))
                    .collect(),
                }
            }
            // No filter: the working draft of every policy of this region, and no numbered
            // version of any of them.
            None => store
                .automated_reasoning_policies
                .values()
                .filter(|policy| policy.region == region)
                .map(|policy| {
                    (
                        crate::state::listing_cursor(policy.created_at, &policy.arn),
                        draft_summary(policy),
                    )
                })
                .collect(),
        };
        // The cursor's leading half IS the instant the revision was created and its
        // trailing half is that revision's own ARN, so byte order over cursors is the
        // `(createdAt, policyArn)` order this listing walks — a TOTAL order, which is what
        // lets its pages partition the revisions.
        rows.sort_by(|(left, _), (right, _)| left.cmp(right));

        if let Some(token) = input.next_token.as_deref() {
            if !crate::state::cursor_belongs(token) {
                return Err(
                    ListAutomatedReasoningPoliciesErrorModeled::ValidationException(
                        ValidationException {
                            message: Some(
                                "nextToken is not a pagination token this service issued"
                                    .to_string(),
                            ),
                        },
                    )
                    .into(),
                );
            }
            // A well-formed cursor names a POSITION in that order, so a page resumes at the
            // first revision at or after it whatever happened in between.
            rows.retain(|(cursor, _)| cursor.as_str() >= token);
        }

        // An absent `maxResults` arrives as the member's own modelled default, and anything
        // the constraint walk admitted is already inside the modelled range; the clamp is
        // the shape every list operation of this crate computes a page size with.
        let page_size = usize::try_from(input.max_results)
            .unwrap_or(DEFAULT_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE);
        // The revision the next page starts on IS the next page's cursor, so the pages
        // partition the listing with neither a gap nor a repeat.
        let next_token = rows.get(page_size).map(|(cursor, _)| cursor.clone());
        rows.truncate(page_size);

        Ok(ListAutomatedReasoningPoliciesOutput {
            automated_reasoning_policy_summaries: rows
                .into_iter()
                .map(|(_, summary)| summary)
                .collect(),
            next_token,
        })
    }
}
