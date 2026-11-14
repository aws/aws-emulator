// SPDX-License-Identifier: CC0-1.0

//! @op ListLensReviewImprovements
//! This operation reports the IMPROVEMENT PLAN of ONE lens review of ONE workload: the
//! questions of that review the workload's answers leave a risk in, each with the risk
//! it is rated at, in the workload's own pillar priority order.
//!
//! It is a REPORTING operation: it resolves the workload, the milestone (when one is
//! named) and the lens review, reads the lens's question catalog and the workload's
//! stored answers, and writes nothing.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine, ctx.parts())
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `ListLensReviewImprovements`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-wellarchitected-wellarchitectedapiservicelambda/smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json")
//! ```
//!
//! List the improvements of a particular lens review.
//!
//! ## Input
//!
//! * `LensAlias` (LensAlias, len 1..128) **required** — The alias of the lens. For Amazon Web
//!   Services official lenses, this is either the lens alias, such as serverless, or the lens
//!   ARN, such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some
//!   operations (such as ExportLens and CreateLensShare) are not permitted on Amazon Web
//!   Services official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `MaxResults` (MaxResults) — The maximum number of results to return for this request.
//! * `MilestoneNumber` (MilestoneNumber, range 1..100) — The milestone number. A workload can
//!   have a maximum of 100 milestones.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `PillarId` (PillarId, len 1..64) — The ID used to identify a pillar, for example,
//!   security. A pillar is identified by its PillarReviewSummary$PillarId.
//! * `QuestionPriority` (QuestionPriority, one of: NONE | PRIORITIZED) — The priority of the
//!   question.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) **required** — The ID
//!   assigned to the workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Output
//!
//! * `ImprovementSummaries` (ImprovementSummaries) — List of improvement summaries of lens
//!   review in a workload.
//! * `LensAlias` (LensAlias, len 1..128) — The alias of the lens. For Amazon Web Services
//!   official lenses, this is either the lens alias, such as serverless, or the lens ARN,
//!   such as arn:aws:wellarchitected:us-east-1::lens/serverless. Note that some operations
//!   (such as ExportLens and CreateLensShare) are not permitted on Amazon Web Services
//!   official lenses. For custom lenses, this is the lens ARN, such as
//!   arn:aws:wellarchitected:us-west-2:123456789012:lens/0123456789abcdef01234567890abcdef.
//!   Each lens is identified by its LensSummary$LensAlias.
//! * `LensArn` (LensArn) — The ARN for the lens.
//! * `MilestoneNumber` (MilestoneNumber, range 1..100) — The milestone number. A workload can
//!   have a maximum of 100 milestones.
//! * `NextToken` (NextToken, pattern ^[A-Za-z0-9+\/=_-]+$) — The token to use to retrieve the
//!   next set of results.
//! * `WorkloadId` (WorkloadId, len 32..32, pattern ^[0-9a-f]{32}$) — The ID assigned to the
//!   workload. This ID is unique within an Amazon Web Services Region.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — User does not have sufficient access to perform
//!   this action.
//! * `InternalServerException` (HTTP 500) — There is a problem with the Well-Architected Tool
//!   API service.
//! * `ResourceNotFoundException` (HTTP 404) — The requested resource was not found.
//! * `ThrottlingException` (HTTP 429) — Request was denied due to request throttling.
//! * `ValidationException` (HTTP 400) — The user input is not valid.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `ListLensReviewImprovementsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `ListLensReviewImprovementsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.wellarchitected#ListLensReviewImprovements")]
#[derive(Debug)]
pub enum ListLensReviewImprovementsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::list_lens_review_improvements::ListLensReviewImprovementsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type ListLensReviewImprovementsError = crate::scaffolded::Scaffolded<ListLensReviewImprovementsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<ListLensReviewImprovementsErrorModeled> for ListLensReviewImprovementsError {
    fn from(modeled: ListLensReviewImprovementsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `ListLensReviewImprovements` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct ListLensReviewImprovements<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ListLensReviewImprovements<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for ListLensReviewImprovements<Eng>
{
    type Input = ListLensReviewImprovementsInput;
    type Output = ListLensReviewImprovementsOutput;
    type Error = ListLensReviewImprovementsError;

    const NAME: &'static str = "ListLensReviewImprovements";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_LENS_REVIEW_IMPROVEMENTS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::LIST_LENS_REVIEW_IMPROVEMENTS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let state = crate::state::service_state(&*self.engine, ctx.parts()).ok_or_else(|| {
            ListLensReviewImprovementsErrorModeled::InternalServerException(
                InternalServerException {
                    message: "the Well-Architected Tool workload store is unavailable".to_string(),
                },
            )
        })?;

        // This operation's `MaxResults` member is `@range(min: 1, max: 100)`
        // (`smithy_query(query="operation", operation="ListLensReviewImprovements")`):
        // the range is a MEMBER trait, so this read's ceiling is its own rather than the
        // one most of this service's paginated reads carry. A request that omits the
        // member asks for a full page.
        let page_size = crate::state::page_size_up_to(
            input.max_results,
            crate::state::IMPROVEMENTS_MAX_RESULTS_CEILING,
        );

        // An opaque token this service did not issue is a bad request, not an empty page.
        // A token is this service's own cursor, not a value a caller composes: it cannot
        // be corrected, only obtained again, so the refusal is classified `CANNOT_PARSE`
        // ("restart the pagination") rather than as a field rule to fix — and it still
        // names the member whose value could not be decoded.
        let resume_after = match input.next_token.as_deref() {
            None => None,
            Some(token) => match crate::state::decode_question_token(token) {
                Some(question_id) => Some(question_id),
                None => return Err(invalid_next_token()),
            },
        };

        let store = state.lock();
        let live = store.workloads.get(&input.workload_id).ok_or_else(|| {
            ListLensReviewImprovementsErrorModeled::ResourceNotFoundException(
                crate::state::workload_not_found(&input.workload_id),
            )
        })?;

        // "A milestone records the state of a workload at a particular point in time"
        // (`CreateMilestone` doc): a request that names one reports the improvements of
        // the RECORDED workload's review, not of the live one.
        let workload = match input.milestone_number {
            None => live,
            Some(number) => {
                &store
                    .milestone(&input.workload_id, number)
                    .ok_or_else(|| {
                        ListLensReviewImprovementsErrorModeled::ResourceNotFoundException(
                            crate::state::milestone_not_found(&input.workload_id, number),
                        )
                    })?
                    .workload
            }
        };

        // A lens review exists only while that lens is associated with the workload, so a
        // lens the workload does not carry names no review to report improvements of — a
        // value of `LensAlias` this service refuses rather than a resource it could not
        // find (`crate::service::lens_content_validation`).
        if !workload.reviews_lens(&input.lens_alias) {
            return Err(ListLensReviewImprovementsErrorModeled::ValidationException(
                crate::service::lens_content_validation::no_lens_review(
                    &input.workload_id,
                    &input.lens_alias,
                ),
            )
            .into());
        }

        // `PillarId` names a pillar OF THE LENS under review, and the pillars of a lens are
        // what that lens's content defines, so a value naming none of them belongs to no
        // pillar of this review: it is refused through the one place this service words that
        // refusal (`crate::service::lens_content_validation`) rather than silently selecting
        // nothing.
        if let Some(exception) = crate::service::lens_content_validation::undefined_pillar(
            &store.lens_content_at(
                &input.lens_alias,
                workload.lens_review_version(&input.lens_alias),
            ),
            input.pillar_id.as_deref(),
        ) {
            return Err(ListLensReviewImprovementsErrorModeled::ValidationException(exception).into());
        }

        // The improvement plan is a SELECTION over the lens's questions, in the ORDER the
        // workload is reviewed in: the questions whose stored answer leaves a risk — the
        // ones the review IDENTIFIED a risk in, through the one place that rule lives
        // (`crate::state::identified_risk`) — narrowed further by the request's filters and
        // paged through the service's shared question cursor. The question set is the LENS's own content — an AWS-official
        // lens's catalog entry or a custom lens's imported document — which is what
        // `crate::state::...::lens_review_questions` resolves, for the lens VERSION this
        // review is against. `PillarId` was resolved against that content above, so what it
        // does here is SELECT the pillar's questions; `QuestionPriority` filters rather than
        // resolving anything, so a priority no question carries is an empty page.
        //
        // Every question of a workload lens review here carries
        // `crate::state::DEFAULT_QUESTION_TYPE`, so the priority filter selects all of
        // them or none.
        let priority_selects = input.question_priority.as_deref().is_none_or(|priority| {
            priority == crate::state::question_priority_of(crate::state::DEFAULT_QUESTION_TYPE)
        });
        let mut questions = store.lens_review_questions(
            &input.lens_alias,
            workload.lens_review_version(&input.lens_alias),
        );
        // "The priorities of the pillars, which are used to order items in the improvement
        // plan" (`WorkloadPillarPriorities` doc): the plan's order is the WORKLOAD's, so
        // the question set is put in the reviewed workload's pillar priority order before
        // anything is selected or paged over it
        // (`crate::state::order_by_pillar_priority`, which also fixes where a pillar those
        // priorities do not name sits). The workload here is the one whose plan is being
        // read — the live one, or the one the named milestone recorded — so a milestone
        // reports the priorities it froze.
        crate::state::order_by_pillar_priority(&mut questions, &workload.pillar_priorities);
        let page = crate::state::question_page(
            &questions,
            |question| {
                priority_selects
                    && input
                        .pillar_id
                        .as_deref()
                        .is_none_or(|pillar| pillar == question.pillar_id)
                    && crate::state::identified_risk(
                        question,
                        workload.answer(&input.lens_alias, &question.question_id),
                    )
                    .is_some()
            },
            resume_after.as_deref(),
            page_size,
        )
        .ok_or_else(invalid_next_token)?;

        let improvement_summaries = page
            .questions
            .iter()
            .filter_map(|question| {
                let risk = crate::state::identified_risk(
                    question,
                    workload.answer(&input.lens_alias, &question.question_id),
                )?;
                Some(improvement_summary(
                    question,
                    risk,
                    workload
                        .question_jira_configuration(&input.lens_alias, &question.question_id),
                ))
            })
            .collect();

        Ok(ListLensReviewImprovementsOutput {
            workload_id: Some(workload.workload_id.clone()),
            // Echoed only when the request scoped the read to a milestone: a read of the
            // live review is not a read of any milestone.
            milestone_number: input.milestone_number,
            // The lens as the request named it, beside the ARN it resolves to
            // (`crate::state::...::lens_arn_of`, the one place this service resolves a
            // lens's ARN).
            lens_alias: Some(input.lens_alias.clone()),
            lens_arn: Some(store.lens_arn_of(&input.lens_alias)),
            improvement_summaries: Some(improvement_summaries),
            next_token: page.next_token,
        })
    }
}

/// The `ImprovementSummary` for one question of a lens review: the question as the lens
/// defines it, plus the risk the workload's stored answer is rated at.
fn improvement_summary(
    question: &crate::state::ReviewQuestion,
    risk: &str,
    jira_configuration: Option<JiraConfiguration>,
) -> ImprovementSummary {
    ImprovementSummary {
        question_id: Some(question.question_id.clone()),
        pillar_id: Some(question.pillar_id.clone()),
        question_title: Some(question.title.clone()),
        risk: Some(risk.to_string()),
        // `ImprovementPlanUrl` is the per-question improvement-plan DOCUMENT a lens
        // publishes, and `ImprovementPlans` the per-choice one, which "is only applicable
        // to custom lenses" (`smithy_query(query="shape", shape="ImprovementSummary")`).
        // Neither is content this emulator's lens carries — it ships a question's pillar,
        // title and best practices and nothing else, the same reason
        // `Choice$ImprovementPlan` is left off that question's choice catalog — and both
        // members are optional, so they are omitted rather than filled with a URL that
        // documents nothing.
        improvement_plan_url: None,
        improvement_plans: None,
        // The integration state of this question's issue in the integrated service,
        // which exists once a sync has recorded one
        // (`crate::state::WorkloadRecord::question_jira_configuration`, the one place every
        // read of an answer reads it from).
        jira_configuration,
    }
}

/// What a `NextToken` this service did not issue is answered with. The token is this
/// service's own opaque cursor, so the only thing a caller can do is start the
/// pagination again — which is what `CANNOT_PARSE` tells it.
fn invalid_next_token() -> ListLensReviewImprovementsError {
    ListLensReviewImprovementsErrorModeled::ValidationException(
        crate::state::cannot_parse_exception(
            crate::state::NEXT_TOKEN_MEMBER,
            crate::state::INVALID_NEXT_TOKEN_MESSAGE,
        ),
    )
    .into()
}
