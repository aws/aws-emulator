// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wellarchitected-wellarchitectedapiservicelambda service.
//!
//! The workload lifecycle (`CreateWorkload`, `GetWorkload`, `ListWorkloads`,
//! `UpdateWorkload`, `DeleteWorkload`), the milestones recorded against a workload
//! (`CreateMilestone`, `GetMilestone`, `ListMilestones`), the review templates
//! (`CreateReviewTemplate`, `GetReviewTemplate`, `ListReviewTemplates`,
//! `UpdateReviewTemplate`, `DeleteReviewTemplate`), the lens reviews of a workload (`GetLensReview`,
//! `ListLensReviews`, `UpdateLensReview`), the answers of
//! a workload's lens reviews (`GetAnswer`, `ListAnswers`, `UpdateAnswer`), the
//! improvement plan of one of those lens
//! reviews (`ListLensReviewImprovements`) and the answers of a review template's lens
//! reviews (`ListReviewTemplateAnswers`, `GetReviewTemplateAnswer`,
//! `UpdateReviewTemplateAnswer`) are backed entirely by the maps below, so a
//! create is visible to the next request on the same engine. A lens review — of a
//! workload or of a template — is held against the lens VERSION it was applied at, which
//! `UpgradeLensReview` ([`LensReviewRecord::lens_version`]) and
//! `UpgradeReviewTemplateLensReview` ([`ReviewTemplateLensReviewRecord::lens_version`])
//! each move to the lens's latest, keeping the review as it stood in a milestone of the
//! workload in the first case. The custom lenses
//! `ImportLens`
//! imports, the versions `CreateLensVersion` publishes from them, the content
//! `ExportLens` reads back and the versions `GetLensVersionDifference` compares are backed
//! the same way, and `ListLenses` pages over those
//! lenses beside the AWS-official lens this emulator ships; `DeleteLens` drops the lens
//! content whose publication state the request names, so a lens whose every version and
//! draft is gone is gone from the store itself. The Well-Architected
//! Tool profiles (`CreateProfile`, `GetProfile`, `UpdateProfile`, `ListProfiles`) are backed the same way, each keyed by the
//! `ProfileArn` the create minted, and the association of one with a workload
//! (`AssociateProfiles`, `DisassociateProfiles`) is held on the workload the association is
//! OF, pinned at the profile version the workload is reviewed against — the version
//! `UpgradeProfileVersion` moves to the one that profile is at. The profile notifications
//! `ListProfileNotifications` reports are DERIVED from those associations rather than
//! stored: an association pinned at a version the profile has moved past is the
//! notification, so nothing has to be recorded when a profile is revised and no read has
//! to write. The LENS notifications `ListNotifications` reports are derived the same way,
//! from the lens reviews of the workloads and of the review templates: a review pinned at a
//! version its lens has published past
//! ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::superseded_lens_review`])
//! is the notification an upgrade of that review clears. The shares of a profile
//! (`CreateProfileShare`, `ListProfileShares`, `DeleteProfileShare`) are held like a
//! workload's, keyed by the `ProfileArn` they share then by the share's own `ShareId`, and
//! each raises the invitation `ListShareInvitations` reports to the principal it was
//! offered to. The shares of a REVIEW TEMPLATE (`CreateTemplateShare`,
//! `ListTemplateShares`, `DeleteTemplateShare`) are held the same way, keyed by the
//! `TemplateArn` they share then
//! by the share's own `ShareId`, and raise an invitation of their own kind through that
//! same enumeration. The agent profiles
//! (`CreateAgentProfile`, `GetAgentProfile`, `ListAgentProfiles`) and the optimization
//! goals recorded against one (`CreateAgentGoal`, `GetAgentGoal`, `UpdateAgentGoal`,
//! `DeleteAgentGoal`) are backed the same way,
//! each goal keyed under the profile it belongs to. The contexts recorded against a
//! profile (`CreateAgentContext`, `GetAgentContext`, `ListAgentContexts`,
//! `UpdateAgentContext`, `DeleteAgentContext`) are held
//! exactly like the goals, keyed under the profile they belong to. The recommendation
//! generations of a profile (`StartAgentRecommendationGeneration`,
//! `GetAgentRecommendationGeneration`, `ListAgentRecommendationGenerations`) are held the
//! same way; a generation is a JOB, so
//! its status, progress and transition timestamps are DERIVED from the instants the
//! clock owes its transitions at ([`AgentGenerationRecord`]) rather than stored, and no
//! read has to write. The recommendations a COMPLETED generation produced
//! (`ListAgentRecommendations`, `UpdateAgentRecommendationStatus`) are held under the
//! profile they belong to and keyed by the ARN that names each one, materialised from the
//! generation whose analysis the clock has finished
//! ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::materialise_due_agent_recommendations`])
//! and carrying the finding's content by reference into the catalog in
//! [`recommendation_catalog`], so a recommendation's text is the same in every reading of
//! it and only its status and the feedback submitted on it are a caller's to move
//! (`UpdateAgentRecommendationStatus`, `PutAgentRecommendationFeedback`). The items of one
//! recommendation (`ListAgentRecommendationItems`) are DERIVED from that same finding and
//! from the recommendations its generation produced beside it, so nothing has to be
//! recorded when one is produced and no read has to write. The record types
//! and
//! their projections live here rather than in a handler because several operations reach
//! them.
//!
//! The lens content this emulator ships itself — the AWS-official lens catalog, whose
//! entries carry each official lens's name, version, pillars and question set
//! ([`official_lenses`]) — lives here too, for the same reason: the risk roll-up of a
//! workload and the answer list of a lens review must agree about which questions a lens
//! asks. The aliases of the lenses Amazon Web Services publishes
//! ([`AWS_OFFICIAL_LENS_ALIASES`]) live beside it, because a lens is applied BY NAME and
//! every operation that applies one has to resolve that name against the same catalog
//! ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::resolve_lens`]) —
//! the aliases are every lens AWS publishes, of which the content catalog carries the ones
//! this emulator holds questions for.
//!
//! The Trusted Advisor check content lives here for the same reason
//! ([`trusted_advisor_checks`]): a check is associated to one best practice of one lens, so
//! the checks a best practice reports (`ListCheckDetails`) and the question set that best
//! practice comes from have to agree about which choice is which, and the association is
//! resolved through the question's own choice catalog rather than restated beside it.
//!
//! The modelled `ResourceNotFoundException` / `ConflictException` constructors live
//! here for the same reason: every operation that resolves an identifier or enforces a
//! uniqueness rule must word the failure identically, so there is ONE place each
//! message is built.
//!
//! Every map below holds the resources of ONE Region. The Well-Architected Tool is a
//! regional service, so the engine holds one store per Region ([`RegionalStores`]) and a
//! handler resolves the store of the Region its request was signed for
//! ([`service_state`]): a resource is created, listed, resolved, counted and deleted in
//! its own Region, carries that Region in its ARN, and does not exist in any other. The
//! partitioning, and what bounds it, live in [`regions`].

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

pub mod official_lenses;
pub mod profile_template;
pub mod recommendation_catalog;
/// The Amazon Web Services Regions this API names — a Region as a VALUE.
pub mod regions;
/// How this service's state is PARTITIONED by Region, and what bounds that
/// partitioning. Re-exported below, so a handler names `crate::state::service_state`
/// for its Region's store exactly as it does for a record type.
///
/// It is what the engine holds this state AS rather than another kind of record in it, so
/// its file sits beside this one; the `#[path]` is what keeps it a module OF this one,
/// which is what lets it reach the store's own fields.
#[path = "account_regions.rs"]
pub mod account_regions;
pub mod trusted_advisor_checks;
mod framework_lens;
/// The document ONE lens review is REPORTED as (`GetLensReviewReport`), derived from the
/// stored review exactly as the review's own projection is.
mod lens_review_report;
mod store;
mod refusals;
mod shares;
mod agents;
mod profiles;
mod workload;
pub use workload::*;
pub use profiles::*;
pub use agents::*;
pub use shares::*;
pub use refusals::*;
pub use framework_lens::*;
pub use lens_review_report::*;

pub use regions::{AWS_REGIONS, DEFAULT_REGION};

pub use account_regions::{
    partition_region, region_state, request_region, service_state, RegionalServiceState,
    RegionalStateGuard, RegionalStores, MAX_REGIONS,
};

pub use profile_template::{
    profile_template, profile_template_published_at, profile_template_question,
    ProfileTemplateChoiceEntry, ProfileTemplateQuestionEntry, PROFILE_TEMPLATE_NAME,
    PROFILE_TEMPLATE_QUESTIONS, PROFILE_TEMPLATE_REVISION,
};

pub use recommendation_catalog::{
    recommendation_content, RecommendationContent, RECOMMENDATION_CATALOG,
};

pub use trusted_advisor_checks::{
    check_account_summary, check_result, checks_for_best_practice, CheckAccountSummary,
    CheckResult, TrustedAdvisorCheck, CHECK_PROVIDER_TRUSTED_ADVISOR, CHECK_STATUS_BY_SEVERITY,
    TRUSTED_ADVISOR_CHECK_CATALOG,
};

pub use official_lenses::{
    framework_lens_published_at, lens_metadata, lens_pillars, lens_question, lens_question_count,
    lens_questions, official_lens, LensMetadata, OfficialLens, AUTHORED_CONTENT_REVISION,
    CATALOG_DESCRIPTION_PREFIX, CATALOG_DESCRIPTION_SUFFIX, LENS_NAME_SUFFIX,
    OFFICIAL_LENS_CATALOG, WELL_ARCHITECTED_PILLARS,
};

use crate::ops::types::{
    AccountJiraConfigurationOutput, AgentProfileSummary, AgentRecommendationGenerationSummary,
    AgentRecommendationSummary, AggregationConfiguration, Choice, ChoiceAnswer, ConflictException,
    ContextContent, ContextSummary, GetGlobalSettingsOutput, GoalSummary, JiraConfiguration,
    JiraSelectedQuestionConfiguration, LensReview, LensShareSummary, LensSummary,
    Milestone, MilestoneSummary, PillarReviewSummary, Profile, ProfileQuestion,
    ProfileShareSummary, ProfileSummary, Progress, ResourceNotFoundException, ReviewTemplate,
    ReviewTemplateAnswer, ReviewTemplateLensReview, ReviewTemplatePillarReviewSummary,
    ReviewTemplateSummary, Scope,
    ShareInvitation, ShareInvitationSummary, Tag, TemplateShareSummary, Workload,
    WorkloadDiscoveryConfig,
    WorkloadJiraConfigurationInput, WorkloadJiraConfigurationOutput, WorkloadShare,
    WorkloadShareSummary, WorkloadSummary, APPLICATION_TYPE_VALUES, CONTEXT_TYPE_VALUES,
    CRITICALITY_VALUES, QUESTION_VALUES, RISK_VALUES,
};

/// `Workload$ImprovementStatus` before any lens review has been answered. The
/// `WorkloadImprovementStatus` enum's `NOT_APPLICABLE` member — see
/// `smithy_query(query="shape", shape="WorkloadImprovementStatus")`.
pub const DEFAULT_IMPROVEMENT_STATUS: &str = "NOT_APPLICABLE";

/// The `Risk` member every unanswered question of an attached lens counts under —
/// `smithy_query(query="shape", shape="Risk")` lists `UNANSWERED` beside the four
/// answered risks. A question is unanswered while nothing has answered it AND while its
/// stored answer says nothing about any of its choices ([`AnswerRecord::risk`]), so the
/// roll-up below fills this bucket from the lens question set and from such an answer
/// alike.
pub const RISK_UNANSWERED: &str = "UNANSWERED";

/// The four `Risk` members a question an answer SPEAKS ABOUT can carry
/// (`smithy_query(query="shape", shape="Risk")`), named here because
/// [`AnswerRecord::risk`] derives one of them from such a stored answer.
pub const RISK_HIGH: &str = "HIGH";
/// See [`RISK_HIGH`].
pub const RISK_MEDIUM: &str = "MEDIUM";
/// See [`RISK_HIGH`].
pub const RISK_NONE: &str = "NONE";
/// See [`RISK_HIGH`].
pub const RISK_NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The `ChoiceStatus` member that says a best practice is NOT in place
/// (`smithy_query(query="shape", shape="ChoiceStatus")`): what keeps a question's risk
/// off `NONE`.
pub const CHOICE_STATUS_UNSELECTED: &str = "UNSELECTED";

/// The `ChoiceStatus` member that says a best practice IS in place
/// (`smithy_query(query="shape", shape="ChoiceStatus")`): the status every `ChoiceId` a
/// `SelectedChoices` names carries, and the only status a selected choice carries — which
/// is what keeps an answer's `SelectedChoices` and its `ChoiceAnswers` in agreement
/// ([`AnswerRecord::selected_choices_of`], which derives the one from the other).
pub const CHOICE_STATUS_SELECTED: &str = "SELECTED";

/// `TrustedAdvisorIntegrationStatus$DISABLED`: what a workload's discovery
/// configuration reports while no Trusted Advisor integration has been configured for
/// it. The enum publishes exactly `ENABLED` and `DISABLED`
/// (`smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`), and a
/// workload nobody enabled the integration for is not integrated.
pub const DEFAULT_TRUSTED_ADVISOR_INTEGRATION_STATUS: &str = "DISABLED";

/// `TrustedAdvisorIntegrationStatus$ENABLED`: what a workload's discovery configuration
/// reports once a caller has activated the Trusted Advisor integration for it
/// (`smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`) — the status
/// under which that workload's Trusted Advisor checks are fetched
/// ([`WorkloadRecord::trusted_advisor_integration_enabled`]).
pub const TRUSTED_ADVISOR_INTEGRATION_ENABLED: &str = "ENABLED";

/// The `DISABLED` member every account-level opt-in status of this service publishes:
/// `OrganizationSharingStatus`, `DiscoveryIntegrationStatus` and
/// `AccountJiraIssueManagementStatus` each publish exactly `ENABLED` and `DISABLED`
/// (`smithy_query(query="shape", shape="OrganizationSharingStatus")`). It is what an
/// account that has turned none of them on reports: `UpdateGlobalSettings` is what
/// "update[s] whether the Amazon Web Services account is opted into organization sharing
/// and discovery integration features", so an account no write has opted in is not opted
/// in.
pub const GLOBAL_SETTING_DISABLED: &str = "DISABLED";

/// `IssueManagementType$MANUAL`: the issue management type under which the questions that
/// carry an issue in the integrated service are the ones the CALLER selected, where `AUTO`
/// — the other member the enum publishes (`smithy_query(query="shape",
/// shape="IssueManagementType")`) — leaves every question of a review covered. It is what
/// gives `LensReview$JiraConfiguration`'s selected pillars and questions their effect: a
/// sync of a workload whose issue management is manual covers exactly the questions that
/// selection names ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::sync_workload_integration`]).
pub const ISSUE_MANAGEMENT_TYPE_MANUAL: &str = "MANUAL";

/// `IntegrationStatus$NOT_CONFIGURED`: what an account's Jira integration reports until
/// it is connected. The reported status publishes `CONFIGURED` and `NOT_CONFIGURED`,
/// while the member a REQUEST carries (`IntegrationStatusInput`) publishes
/// `NOT_CONFIGURED` alone (`smithy_query(query="shape",
/// shape="IntegrationStatus,IntegrationStatusInput")`) — so no request can ask for
/// `CONFIGURED`, which is reached by completing the Jira connection outside this API.
pub const JIRA_INTEGRATION_NOT_CONFIGURED: &str = "NOT_CONFIGURED";

/// The lens alias of the AWS-official Well-Architected Framework lens — the first entry
/// of the AWS-official lens catalog this emulator publishes
/// ([`official_lenses::OFFICIAL_LENS_CATALOG`]). A lens is named on the wire either by its
/// alias or by its ARN, whose resource part ends in `lens/<alias>`
/// (`smithy_query(query="shape", shape="LensAlias")`).
pub const FRAMEWORK_LENS_ALIAS: &str = "wellarchitected";

/// The `LensSummary$Description` of the AWS-official Well-Architected Framework lens: the
/// description AWS's lens catalog publishes for it, within `LensDescription`'s modelled
/// 1..1024 characters (`smithy_query(query="shape", shape="LensSummary")`).
///
/// The framework lens is the one lens AWS describes in prose of its own; every other
/// published lens is described in the catalog's short form
/// ([`official_lenses::CATALOG_DESCRIPTION_PREFIX`]). Catalog content rather than a stored
/// value, so it lives beside the question set it describes, next to that lens's name and
/// version ([`FRAMEWORK_LENS_NAME`], [`FRAMEWORK_LENS_VERSION`]).
pub const FRAMEWORK_LENS_DESCRIPTION: &str = "The AWS Well-Architected Framework Lens \
     provides a set of foundational questions for you to consider for all of your cloud \
     architectures.";

/// `LensSummary$LensStatus` for a lens whose current content no version has been
/// published from.
///
/// "The new or updated lens will have a status of DRAFT. The lens cannot be applied to
/// workloads or shared with other Amazon Web Services accounts until it's published with
/// CreateLensVersion" (`ImportLens` doc) — so this is the status of a lens that has only
/// been imported, and of one imported OVER after a version was published from it.
///
/// It is NOT a member of the modelled `LensStatus` enum, which publishes `CURRENT`,
/// `NOT_CURRENT`, `DEPRECATED`, `DELETED` and `UNSHARED` (`smithy_query(query="shape",
/// shape="LensStatus")`) — the version-currency vocabulary a workload's lens REVIEW
/// reports, which is what [`LENS_STATUS_CURRENT`] is for. What a lens SUMMARY reports
/// there is the lens's publication state, the vocabulary its own filter is defined over:
/// `LensStatusType` is exactly `ALL`, `DRAFT` and `PUBLISHED`
/// (`smithy_query(query="shape", shape="LensStatusType")`), so `ListLenses` filters on
/// the member it is named after.
pub const LENS_STATUS_DRAFT: &str = "DRAFT";

/// `LensSummary$LensVersion` for a lens whose current content no version has been
/// published from.
///
/// A custom lens's versions are named by the caller (`CreateLensVersion`'s `LensVersion`
/// is an input), so an unpublished lens has no version NAME of its own; what it has is its
/// draft, and that is what the service reports as the version of the content a caller
/// would read — the same word `LensSummary$LensStatus` reports ([`LENS_STATUS_DRAFT`]),
/// because both members speak about that one content.
pub const LENS_VERSION_DRAFT: &str = "DRAFT";

/// `LensSummary$LensStatus` for a lens a version HAS been published from — including the
/// lenses Amazon Web Services publishes itself, whose content is the version they are
/// published at. See [`LENS_STATUS_DRAFT`] for why these two and not the modelled
/// `LensStatus` members.
pub const LENS_STATUS_PUBLISHED: &str = "PUBLISHED";

/// `LensType$AWS_OFFICIAL`: a lens Amazon Web Services publishes
/// (`smithy_query(query="shape", shape="LensType")`).
pub const LENS_TYPE_AWS_OFFICIAL: &str = "AWS_OFFICIAL";

/// `LensType$CUSTOM_SELF`: a custom lens the CALLING account owns.
pub const LENS_TYPE_CUSTOM_SELF: &str = "CUSTOM_SELF";

/// `LensType$CUSTOM_SHARED`: a custom lens another account owns and shared with the
/// caller.
pub const LENS_TYPE_CUSTOM_SHARED: &str = "CUSTOM_SHARED";

/// `LensStatusType$ALL`: every lens, whatever its publication state
/// (`smithy_query(query="shape", shape="LensStatusType")`, which publishes `ALL`,
/// `DRAFT` and `PUBLISHED`).
///
/// This is also what a `ListLenses` request that names NO `LensStatus` selects: the
/// member filters the list, and a request that asks for no filtering gets none. An
/// unpublished lens is still a lens the account has — `ImportLens` answers its ARN, and
/// `ListLenses` is how a caller finds it again — so it is listed too, with the publication
/// state of each reported lens being what `DRAFT` and `PUBLISHED` then narrow to.
pub const LENS_STATUS_TYPE_ALL: &str = "ALL";

/// `LensStatusType$DRAFT`: only lenses carrying content no version has been published
/// from.
pub const LENS_STATUS_TYPE_DRAFT: &str = "DRAFT";

/// `LensStatusType$PUBLISHED`: only lenses a published version exists of — the lenses
/// that can actually be applied to a workload, since a lens that has never been published
/// "cannot be applied to workloads or shared with other Amazon Web Services accounts"
/// (`ImportLens` doc).
pub const LENS_STATUS_TYPE_PUBLISHED: &str = "PUBLISHED";

/// The `LensName` of the AWS-official Well-Architected Framework lens: the name AWS
/// publishes for the lens [`FRAMEWORK_LENS_ALIAS`] names.
pub const FRAMEWORK_LENS_NAME: &str = "AWS Well-Architected Framework";

/// The `LensVersion` of the Well-Architected Framework lens: the revision AWS publishes
/// the framework lens at, whose question numbering [`FRAMEWORK_LENS_QUESTIONS`] carries
/// (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6). AWS names a lens
/// version by the day that content was published, and `LensVersion` is
/// `@length(min: 1, max: 32)` (`smithy_query(query="shape", shape="LensReview")`).
pub const FRAMEWORK_LENS_VERSION: &str = "2025-02-25";

/// `LensStatus$CURRENT`: the lens review is against the lens's latest version
/// (`smithy_query(query="shape", shape="LensStatus")` publishes `CURRENT`,
/// `NOT_CURRENT`, `DEPRECATED`, `DELETED` and `UNSHARED`). Each AWS-official lens in the
/// catalog carries exactly one version (its [`OfficialLens::version`]), so a review of one
/// is on that lens's current version; a custom lens's versions are what
/// `CreateLensVersion` publishes, and a review stays on the version it was applied at
/// until it is upgraded.
pub const LENS_STATUS_CURRENT: &str = "CURRENT";

/// `LensStatus$NOT_CURRENT`: the lens review is against a version of the lens a later one
/// has superseded — what a review reports between the publish that supersedes its version
/// and the upgrade that moves it to the latest one.
pub const LENS_STATUS_NOT_CURRENT: &str = "NOT_CURRENT";

/// One question of an AWS-official lens, as this emulator's lens content: the pillar it
/// belongs to, the id it is named by on the wire, its title and the best practices it
/// asks about.
///
/// This is the STATIC catalog's entry, distinct from [`LensQuestion`] — the question of
/// a custom lens DOCUMENT, which carries the choice and risk-rule content that document
/// supplies.
#[derive(Debug, Clone, Copy)]
pub struct FrameworkQuestion {
    /// `AnswerSummary$PillarId` — the pillar this question belongs to, one of
    /// [`WELL_ARCHITECTED_PILLARS`].
    pub pillar_id: &'static str,
    /// `AnswerSummary$QuestionId` (`@length(1, 128)`), stable for the life of the lens.
    pub question_id: &'static str,
    /// `AnswerSummary$QuestionTitle` (`@length(1, 512)`).
    pub title: &'static str,
    /// The best practices this question asks about, in the lens's own order: the `Choice`
    /// the question offers for each of them.
    pub best_practices: &'static [FrameworkChoice],
}

/// One best practice an AWS-official lens asks about, as the `Choice` the question that
/// asks about it offers.
///
/// A lens that PUBLISHES its content publishes each choice's own id and prose, so those
/// are carried verbatim ([`c`]); where the content of a lens is this emulator's own
/// ([`bp`]) the choice is named by its position in the question and carries no prose,
/// because there is none to report.
#[derive(Debug, Clone, Copy)]
pub struct FrameworkChoice {
    /// `Choice$ChoiceId` (`@length(1, 64)`) as the lens publishes it, or `None` where
    /// this emulator's own content names the choice by its position in the question
    /// ([`FrameworkQuestion::choice_id`]).
    pub choice_id: Option<&'static str>,
    /// `Choice$Title` (`@length(1, 512)`) — the best practice itself.
    pub title: &'static str,
    /// `Choice$Description` (`@length(1, 1024)`) — "the description of a choice"
    /// (`smithy_query(query="shape", shape="Choice")`), the prose the lens publishes for
    /// this best practice, and `None` where the lens publishes none.
    pub description: Option<&'static str>,
}

/// One [`FrameworkChoice`] of a lens that PUBLISHES its content: the choice id and the
/// prose are the lens's own.
pub const fn c(
    choice_id: &'static str,
    title: &'static str,
    description: &'static str,
) -> FrameworkChoice {
    FrameworkChoice {
        choice_id: Some(choice_id),
        title,
        description: Some(description),
    }
}

/// One [`FrameworkChoice`] of a lens whose content is this emulator's own: the best
/// practice's title alone, named on the wire by its position in the question
/// ([`FrameworkQuestion::choice_id`]) and carrying no prose.
pub const fn bp(title: &'static str) -> FrameworkChoice {
    FrameworkChoice {
        choice_id: None,
        title,
        description: None,
    }
}

impl FrameworkQuestion {
    /// The `ChoiceId` of the best practice at `index` in [`Self::best_practices`]: the id
    /// the lens publishes for it, or — where this emulator authored the content —
    /// `<question id>-bp-<two-digit position>`.
    ///
    /// A derived id is unique within the question, stable for the life of the catalog (an
    /// answer stored under one keeps resolving), and inside the modelled `ChoiceId`
    /// `@length(min: 1, max: 64)`.
    pub fn choice_id(&self, index: usize) -> String {
        match self.best_practices.get(index).and_then(|choice| choice.choice_id) {
            Some(published) => published.to_string(),
            None => format!("{}-bp-{:02}", self.question_id, index.saturating_add(1)),
        }
    }

    /// This catalog row as the question a LENS REVIEW asks
    /// ([`ReviewQuestion`]): the one projection of a static catalog entry into the
    /// question set every answer operation reads and pages over, so an official lens's
    /// questions and a custom lens's are reported through the same shape.
    ///
    /// Each best practice becomes a choice carrying the `ChoiceId` and the prose the lens
    /// publishes for it, or — where this emulator authored the content — an id derived
    /// from the question and the practice's position ([`Self::choice_id`]) and no prose; a
    /// custom lens's choices carry the ids ITS document names them by.
    pub fn to_review_question(&self) -> ReviewQuestion {
        ReviewQuestion {
            pillar_id: self.pillar_id.to_string(),
            question_id: self.question_id.to_string(),
            title: self.title.to_string(),
            best_practices: self
                .best_practices
                .iter()
                .enumerate()
                .map(|(index, best_practice)| LensChoice {
                    id: self.choice_id(index),
                    title: best_practice.title.to_string(),
                    description: best_practice.description.map(str::to_string),
                })
                .collect(),
        }
    }
}

/// ONE question of ONE lens review, whatever lens the review is of: the pillar it belongs
/// to, the id it is named by on the wire, its title and the best practices it asks about,
/// each with the `ChoiceId` the lens names it by.
///
/// A lens review's question set is the LENS's own content, and this emulator holds that
/// content two ways: an AWS-official lens's questions are the static catalog's rows
/// ([`FrameworkQuestion`], projected by [`FrameworkQuestion::to_review_question`]), and a
/// custom lens's are the pillars and questions its imported document defines
/// ([`LensDefinition::review_questions`]). Every operation that reports or resolves a
/// question of a review speaks THIS type, so neither half of the catalog is reported
/// differently from the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewQuestion {
    /// `AnswerSummary$PillarId` — the pillar of the lens this question belongs to.
    pub pillar_id: String,
    /// `AnswerSummary$QuestionId` (`@length(1, 128)`), stable for the life of the lens's
    /// content.
    pub question_id: String,
    /// `AnswerSummary$QuestionTitle` (`@length(1, 512)`).
    pub title: String,
    /// The best practices this question asks about, in the lens's own order: each
    /// choice's `ChoiceId` and `Choice$Title`.
    pub best_practices: Vec<LensChoice>,
}

impl ReviewQuestion {
    /// This question's CHOICE catalog — `AnswerSummary$Choices` /
    /// `ReviewTemplateAnswerSummary$Choices` / `Answer$Choices`: one `Choice` per best
    /// practice the question asks about, in the lens's order.
    ///
    /// It is the only place a client can discover the `ChoiceId`s that
    /// `UpdateAnswer$SelectedChoices` and `ChoiceUpdates` name, so every projection of a
    /// question reports it from here and they cannot disagree. `HelpfulResource`,
    /// `ImprovementPlan` and `AdditionalResources` are left off: they "only appl[y] to
    /// custom lenses" (`smithy_query(query="shape", shape="Choice")`) and the lens document
    /// format's per-choice prose is not recorded ([`parse_lens_json`]), so there is nothing
    /// to report in them, and every one is optional.
    pub fn choices(&self) -> Vec<Choice> {
        self.best_practices
            .iter()
            .map(|best_practice| Choice {
                choice_id: Some(best_practice.id.clone()),
                title: Some(best_practice.title.clone()),
                // "The description of a choice" — the prose the lens publishes for this
                // best practice, and absent for a lens that publishes none.
                description: best_practice.description.clone(),
                helpful_resource: None,
                improvement_plan: None,
                additional_resources: None,
            })
            .collect()
    }

    /// Whether `choice_id` names one of this question's choices. A choice belongs to the
    /// question that offers it, so an answer can only speak about the choices named here.
    pub fn defines_choice(&self, choice_id: &str) -> bool {
        self.each_choice_id().any(|defined| defined == choice_id)
    }

    /// The `ChoiceId` of every best practice this question asks about, in the lens's own
    /// order — the key space an answer's per-choice statuses live in, and what a
    /// request's `SelectedChoices` values and `ChoiceUpdates` keys are judged against.
    ///
    /// Lazy, so a caller that only has to RECOGNISE an id, or that consumes the ids one at
    /// a time, does not build the whole list to do it.
    fn each_choice_id(&self) -> impl Iterator<Item = &str> + '_ {
        self.best_practices
            .iter()
            .map(|best_practice| best_practice.id.as_str())
    }
}

/// One [`FrameworkQuestion`], so the catalog above reads as a table.
const fn q(
    pillar_id: &'static str,
    question_id: &'static str,
    title: &'static str,
    best_practices: &'static [FrameworkChoice],
) -> FrameworkQuestion {
    FrameworkQuestion {
        pillar_id,
        question_id,
        title,
        best_practices,
    }
}

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// workload.
pub const WORKLOAD_RESOURCE_TYPE: &str = "WORKLOAD";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// milestone.
pub const MILESTONE_RESOURCE_TYPE: &str = "MILESTONE";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// review template.
pub const REVIEW_TEMPLATE_RESOURCE_TYPE: &str = "REVIEW_TEMPLATE";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// profile.
pub const PROFILE_RESOURCE_TYPE: &str = "PROFILE";

/// `ConflictException$ResourceType` for a share of a workload. The share is a resource
/// of its own — it has its own `ShareId` — and `ShareResourceType`
/// (`smithy_query(query="shape", shape="ShareResourceType")`) names the shared resource
/// kind `WORKLOAD`, so a share of one is named here by what it is.
pub const WORKLOAD_SHARE_RESOURCE_TYPE: &str = "WORKLOAD_SHARE";

/// `ConflictException$ResourceType` for a share of a LENS — the counterpart of
/// [`WORKLOAD_SHARE_RESOURCE_TYPE`] for the other resource kind this emulator shares.
/// The share is a resource of its own (its own `ShareId`), while
/// [`LENS_RESOURCE_TYPE`] is the `ShareResourceType` member naming the shared resource
/// KIND, so the two are spelled apart for the same reason.
pub const LENS_SHARE_RESOURCE_TYPE: &str = "LENS_SHARE";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// share of a PROFILE — the counterpart of
/// [`WORKLOAD_SHARE_RESOURCE_TYPE`] for the Well-Architected Tool profile. The share is
/// a resource of its own (its own `ShareId`), while [`PROFILE_RESOURCE_TYPE`] is the
/// `ShareResourceType` member naming the shared resource KIND, so the two are spelled
/// apart for the same reason.
pub const PROFILE_SHARE_RESOURCE_TYPE: &str = "PROFILE_SHARE";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// share invitation. The invitation is a resource of its own — it has its own
/// `ShareInvitationId`, which `UpdateShareInvitation` resolves it by — so it is named
/// here by what it is, distinctly from the share it mediates
/// ([`WORKLOAD_SHARE_RESOURCE_TYPE`]).
pub const SHARE_INVITATION_RESOURCE_TYPE: &str = "SHARE_INVITATION";

/// `ShareStatus` a newly created share carries, whatever resource it shares.
///
/// A share is mediated by an invitation — "Shared access to a workload is not removed
/// until the workload invitation is deleted" (`CreateWorkloadShare` doc), and the same
/// sentence about the lens invitation in `CreateLensShare`'s — so a share the sharer has
/// just created is waiting on the principal it was offered to, and `PENDING` is the
/// `ShareStatus` member for exactly that
/// (`smithy_query(query="shape", shape="ShareStatus")`). Nothing the CLOCK does moves it:
/// it leaves `PENDING` only when the recipient answers the invitation
/// (`UpdateShareInvitation`).
pub const DEFAULT_SHARE_STATUS: &str = "PENDING";

/// The `ShareInvitationAction` members the model publishes
/// (`smithy_query(query="shape", shape="ShareInvitationAction")`): the two answers the
/// principal a resource was shared with can give its invitation.
pub const SHARE_INVITATION_ACTION_ACCEPT: &str = "ACCEPT";
/// See [`SHARE_INVITATION_ACTION_ACCEPT`].
pub const SHARE_INVITATION_ACTION_REJECT: &str = "REJECT";

/// The `ShareStatus` a share carries once the principal it was offered to has accepted
/// its invitation, and the one it carries once that principal has rejected it — both
/// published members of the enum (`smithy_query(query="shape", shape="ShareStatus")`).
/// Each is TERMINAL: the answer is the last thing that happens to the share's status, so
/// nothing the clock does moves it afterwards.
pub const SHARE_STATUS_ACCEPTED: &str = "ACCEPTED";
/// See [`SHARE_STATUS_ACCEPTED`].
pub const SHARE_STATUS_REJECTED: &str = "REJECTED";

/// The `ShareStatus` the share mediated by an invitation moves to when that invitation
/// is answered with `action`, or `None` when `action` is not a published
/// `ShareInvitationAction` member.
///
/// One mapping, here, so every resource kind whose share invitation can be answered
/// records the same status for the same answer.
pub fn share_status_for_invitation_action(action: &str) -> Option<&'static str> {
    match action {
        SHARE_INVITATION_ACTION_ACCEPT => Some(SHARE_STATUS_ACCEPTED),
        SHARE_INVITATION_ACTION_REJECT => Some(SHARE_STATUS_REJECTED),
        _ => None,
    }
}

/// The `ShareResourceType` member that names a REVIEW TEMPLATE share
/// (`smithy_query(query="shape", shape="ShareResourceType")` publishes `WORKLOAD`,
/// `LENS`, `PROFILE` and `TEMPLATE`). Distinct from
/// [`REVIEW_TEMPLATE_RESOURCE_TYPE`], which is how an exception names the template
/// resource itself; the other three share resource types are spelled the same as the
/// exception resource types, so [`WORKLOAD_RESOURCE_TYPE`], [`LENS_RESOURCE_TYPE`] and
/// [`PROFILE_RESOURCE_TYPE`] serve for those.
pub const TEMPLATE_SHARE_RESOURCE_TYPE: &str = "TEMPLATE";

/// The most shares one workload holds in this emulator.
///
/// The Well-Architected Tool meters workload shares per workload and
/// `CreateWorkloadShare` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") for the case; the real quota is adjustable through
/// Service Quotas, so the number here is this emulator's own ceiling rather than a value
/// read off a doc page. It is what bounds the share map's growth — one share per
/// principal per workload, and every create token is 1:1 with a live share — and it sits
/// far above any realistic client workflow, so a caller never meets it by accident.
pub const MAX_SHARES_PER_WORKLOAD: usize = 1_000;

/// The most shares one lens holds in this emulator.
///
/// The same reasoning as [`MAX_SHARES_PER_WORKLOAD`], for the other resource kind that
/// is shared here: `CreateLensShare` models `ServiceQuotaExceededException` (HTTP 402,
/// "The user has reached their resource quota") for the metered case and the real quota
/// is adjustable through Service Quotas, so the number is this emulator's own ceiling
/// rather than a value read off a doc page. It bounds the lens-share map's growth — one
/// share per principal per lens, and every create token is 1:1 with a live share.
pub const MAX_SHARES_PER_LENS: usize = 1_000;

/// The most shares one Well-Architected Tool profile holds in this emulator.
///
/// The same reasoning as [`MAX_SHARES_PER_WORKLOAD`], for the profile:
/// `CreateProfileShare` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") for the metered case and the real quota is adjustable
/// through Service Quotas, so the number is this emulator's own ceiling rather than a
/// value read off a doc page. It bounds the profile-share map's growth — one share per
/// principal per profile, and every create token is 1:1 with a live share.
pub const MAX_SHARES_PER_PROFILE: usize = 1_000;

/// The most shares one REVIEW TEMPLATE holds in this emulator.
///
/// The same reasoning as [`MAX_SHARES_PER_WORKLOAD`], for the review template:
/// `CreateTemplateShare` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") for the metered case and the real quota is adjustable
/// through Service Quotas, so the number is this emulator's own ceiling rather than a
/// value read off a doc page. It bounds the template-share map's growth — one share per
/// principal per template, and every create token is 1:1 with a live share.
pub const MAX_SHARES_PER_REVIEW_TEMPLATE: usize = 1_000;

/// `ResourceNotFoundException$ResourceType` for the review of one lens in one workload
/// — the resource `ListAnswers` and the other per-lens reads resolve. The modelled
/// member is a free-form string (`smithy_query(query="shape",
/// shape="ExceptionResourceType")`), so the type names the resource that was missing:
/// the workload's review OF that lens, which exists only while the lens is associated
/// with the workload.
pub const LENS_REVIEW_RESOURCE_TYPE: &str = "LENS_REVIEW";

/// `AnswerSummary$QuestionType` for every question of a workload's lens review here.
///
/// The enum publishes exactly `PRIORITIZED` and `NON_PRIORITIZED`
/// (`smithy_query(query="shape", shape="QuestionType")`), and a question is prioritized
/// because a PROFILE associated with the workload prioritises it. WHICH lens question a
/// profile's answer prioritises is a MAPPING from the published profile template's choices
/// onto a lens's questions, and neither the Smithy model nor the service docs publish one,
/// so a profile associated with a workload prioritises no question of it and every question
/// of every workload reports this type.
pub const DEFAULT_QUESTION_TYPE: &str = "NON_PRIORITIZED";

/// The `QuestionType` a question is prioritized with, the counterpart of
/// [`DEFAULT_QUESTION_TYPE`] (`smithy_query(query="shape", shape="QuestionType")`).
pub const PRIORITIZED_QUESTION_TYPE: &str = "PRIORITIZED";

/// The `QuestionPriority` that selects a question of the given `QuestionType`:
/// `PRIORITIZED` selects the prioritized questions and `NONE` the rest. The two enums
/// publish `PRIORITIZED | NONE` and `PRIORITIZED | NON_PRIORITIZED` respectively
/// (`smithy_query(query="shape", shape="QuestionPriority,QuestionType")`). One mapping,
/// here, so every operation that filters or reports a question's priority agrees.
pub fn question_priority_of(question_type: &str) -> &'static str {
    if question_type == PRIORITIZED_QUESTION_TYPE {
        "PRIORITIZED"
    } else {
        "NONE"
    }
}

/// The ARN service namespace (`OPERATIONS.md`: "ARN namespace: `wellarchitected`").
pub const ARN_NAMESPACE: &str = "wellarchitected";

/// The Region an ARN names, or [`DEFAULT_REGION`] when the text carries no Region field.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>`, so the Region is
/// its fourth field. Used where a resource is minted FROM another resource's ARN rather
/// than from a request: the new ARN then names the same Region as the resource it belongs
/// to.
fn arn_region(arn: &str) -> &str {
    match arn.split(':').nth(3) {
        Some(region) if !region.is_empty() => region,
        _ => DEFAULT_REGION,
    }
}

/// The most workloads this emulator holds for its one account and Region.
///
/// The Well-Architected Tool meters workloads per account per Region and models
/// `ServiceQuotaExceededException` (HTTP 402, "The user has reached their resource
/// quota") on `CreateWorkload` for exactly that case; the real quota is adjustable
/// through Service Quotas, so the number here is this emulator's own ceiling rather
/// than a value read off a doc page. It is what bounds the store's growth: the name
/// index holds one entry per workload, and `remove_workload` drops the create tokens
/// that resolve to a deleted workload, so no token outlives its workload and all three
/// maps are bounded by this one count. It sits far above any realistic client workflow,
/// so a caller never meets it by accident.
pub const MAX_WORKLOADS_PER_ACCOUNT_REGION: usize = 10_000;

/// The most review templates this emulator's store holds.
///
/// `CreateReviewTemplate` models `ServiceQuotaExceededException` (HTTP 402, "The user
/// has reached their resource quota") for the metered case, and the real quota is
/// adjustable through Service Quotas, so this is this emulator's own ceiling rather than
/// a value read off a doc page — the same reasoning as
/// [`MAX_WORKLOADS_PER_ACCOUNT_REGION`], and what bounds the template map and the create
/// tokens keyed to it. The count is per Region, because the store it bounds is one
/// Region's ([`RegionalStores`]), and the emulator serves ONE account — so it is a
/// ceiling per account and Region, enforced without scanning the stored ARNs.
pub const MAX_REVIEW_TEMPLATES: usize = 10_000;

/// The most lenses one review template carries — a quota this emulator sets.
///
/// `CreateReviewTemplateInput$Lenses` targets `ReviewTemplateLenses`, which publishes no
/// `@length`, so nothing in the model bounds the list a create applies. The number here
/// is the one the service publishes for a review-template lens list elsewhere:
/// `UpdateReviewTemplate`'s `LensesToAssociate` / `LensesToDisassociate` are
/// `ReviewTemplateLensAliases`, `@length(min: 1, max: 10)`
/// (`smithy_query(query="shape", shape="ReviewTemplateLensAliases")`). Because it is an
/// emulator quota rather than a modelled constraint, `CreateReviewTemplate` answers a
/// longer list with the modelled `ServiceQuotaExceededException`, not a
/// `ValidationException`. What is counted against it is the DISTINCT lenses the template
/// would end up applying ([`distinct_lenses`]), which is the list it stores: it is what
/// bounds a STORED template's lens list, and with it the work of applying that template to
/// a workload.
pub const MAX_LENSES_PER_REVIEW_TEMPLATE: usize = 10;

/// The most lenses one `CreateWorkload` request attaches — the same kind of quota as
/// [`MAX_LENSES_PER_REVIEW_TEMPLATE`], for the lens list of a WORKLOAD.
///
/// `CreateWorkloadInput$Lenses` targets `WorkloadLenses`, which publishes no `@length`, so
/// nothing in the model bounds the list a create attaches. The number here is the one the
/// service publishes for a workload lens list elsewhere: `AssociateLenses` /
/// `DisassociateLenses` take `LensAliases`, whose doc is "List of lens aliases to associate
/// or disassociate with a workload. Up to 10 lenses can be specified."
/// (`smithy_query(query="shape", shape="LensAliases")`). Being an emulator quota rather
/// than a modelled constraint, `CreateWorkload` answers a longer list with the modelled
/// `ServiceQuotaExceededException` — as `CreateReviewTemplate` does for a template's — and
/// what is counted against it is the DISTINCT lenses the workload would end up reviewed
/// against ([`distinct_lenses`]), which is the list it stores. It is what bounds the lens
/// list ONE create attaches, the lens resolution that
/// create runs ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::unapplicable_lenses_refusal`])
/// and the per-lens work of every read that reports its lens reviews. What bounds the
/// list a STORED workload carries once further lenses are associated with it is the
/// service's own documented ceiling, [`MAX_LENSES_ASSOCIATED_PER_WORKLOAD`].
pub const MAX_LENSES_PER_WORKLOAD: usize = 10;

/// The most lenses one workload is reviewed against, however many requests attached them:
/// "A maximum of 20 lenses can be associated with a workload"
/// (`smithy_query(query="operation", operation="AssociateLenses")`).
///
/// A ceiling the service publishes rather than one this emulator chose, and the one bound
/// on a STORED workload's lens list: the per-request quotas above bound only what a single
/// create or association names. `AssociateLenses` models no `ServiceQuotaExceededException`,
/// so a request that would leave a workload reviewed against more lenses than this is
/// answered with the modelled `ValidationException`
/// ([`too_many_lenses_associated_exception`]). It is also what keeps the per-lens work of
/// every read that reports a workload's lens reviews bounded.
pub const MAX_LENSES_ASSOCIATED_PER_WORKLOAD: usize = 20;

/// The most lens aliases one `AssociateLenses` / `DisassociateLenses` request may name —
/// an emulator quota, like [`MAX_PROFILE_ARNS_PER_REQUEST`], because the member those two
/// operations carry the aliases in publishes only `@length(min: 1)`
/// (`smithy_query(query="shape", shape="LensAliases")`), so nothing in the model bounds
/// the list a caller sends. The number is the one that member's own doc states: "Up to 10
/// lenses can be specified".
///
/// It is what makes the per-request work of an association constant rather than
/// caller-chosen: the length is checked before any per-entry work and before the shared
/// store lock is taken, so an oversized list costs one comparison. Being a request-shape
/// rule rather than a resource quota — and neither operation models
/// `ServiceQuotaExceededException` — it is answered with the modelled `ValidationException`
/// naming [`LENS_ALIASES_MEMBER`] ([`too_many_lens_aliases_exception`]).
pub const MAX_LENS_ALIASES_PER_REQUEST: usize = 10;

/// The most profiles one workload is reviewed against — a MODELLED bound, unlike the lens
/// quota above: `WorkloadProfiles`, the list every read of a workload reports its
/// associations by, is `@length(max: 1)` (`smithy_query(query="shape",
/// shape="WorkloadProfiles")`), as is `CreateWorkloadInput$ProfileArns`. A workload holding
/// more than one could not be serialized, so an association request that would leave it
/// holding more is refused instead ([`PROFILE_ARNS_MEMBER`]).
pub const MAX_PROFILES_PER_WORKLOAD: usize = 1;

/// The modelled input member a profile-association finding names
/// (`AssociateProfilesInput$ProfileArns` / `DisassociateProfilesInput$ProfileArns`).
pub const PROFILE_ARNS_MEMBER: &str = "ProfileArns";

/// The most `ProfileArn`s one `AssociateProfiles` / `DisassociateProfiles` request may
/// name — an emulator quota, like [`MAX_LENSES_PER_WORKLOAD`], because the member those
/// two operations carry the ARNs in (`ProfileArns`) publishes only `@length(min: 1)`
/// (`smithy_query(query="shape", shape="ProfileArns")`), so nothing in the model bounds
/// the list a caller sends. The number is the one the service publishes for the list of
/// the SIBLING associate/disassociate pair: "Up to 10 lenses can be specified"
/// (`smithy_query(query="shape", shape="LensAliases")`).
///
/// It is what makes the per-request work of those operations constant rather than
/// caller-chosen: the length is checked before any per-entry work and before the shared
/// store lock is taken, so an oversized list costs one comparison. Being a request-shape
/// rule rather than a resource quota — and neither operation models
/// `ServiceQuotaExceededException` — it is answered with the modelled
/// `ValidationException` naming [`PROFILE_ARNS_MEMBER`]
/// ([`too_many_profile_arns_exception`]).
pub const MAX_PROFILE_ARNS_PER_REQUEST: usize = 10;

/// `ReviewTemplateUpdateStatus$CURRENT`: what a review template reports while every lens
/// applied to it is at its current version. The enum publishes exactly `CURRENT` and
/// `LENS_NOT_CURRENT` (`smithy_query(query="shape",
/// shape="ReviewTemplateUpdateStatus")`), and a template whose lenses have not been
/// superseded is current.
pub const REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT: &str = "CURRENT";

/// `ReviewTemplateUpdateStatus$LENS_NOT_CURRENT`: what a review template reports once a
/// lens applied to it has a version later than the one its lens review is against
/// ([`ReviewTemplateLensReviewRecord::lens_version`]) — the template's own view of the
/// `LensStatus` that review reports ([`LENS_STATUS_NOT_CURRENT`]), which
/// `UpgradeReviewTemplateLensReview` is what clears.
pub const REVIEW_TEMPLATE_UPDATE_STATUS_LENS_NOT_CURRENT: &str = "LENS_NOT_CURRENT";

/// The `Question` bucket a question with no answer counts under —
/// `smithy_query(query="shape", shape="Question")` publishes exactly `UNANSWERED` and
/// `ANSWERED`. Named here because the `QuestionCounts` roll-up below fills it from the
/// lens question set rather than from an answer.
pub const QUESTION_UNANSWERED: &str = "UNANSWERED";

/// The `Question` bucket a question that CARRIES an answer counts under — the other
/// member `smithy_query(query="shape", shape="Question")` publishes. Named here
/// because the `QuestionCounts` roll-up below fills it from the stored answers.
pub const QUESTION_ANSWERED: &str = "ANSWERED";

/// `ReviewTemplateAnswerStatus$ANSWERED`: a review-template question the template
/// records an answer for. The enum publishes exactly `ANSWERED` and `UNANSWERED`
/// (`smithy_query(query="shape", shape="ReviewTemplateAnswerStatus")`), and it is the
/// stored answer — not a separate flag — that decides which one a question reports.
pub const REVIEW_TEMPLATE_ANSWER_STATUS_ANSWERED: &str = "ANSWERED";

/// `ReviewTemplateAnswerStatus$UNANSWERED`: a review-template question the template
/// records no answer for. See [`REVIEW_TEMPLATE_ANSWER_STATUS_ANSWERED`].
pub const REVIEW_TEMPLATE_ANSWER_STATUS_UNANSWERED: &str = "UNANSWERED";

/// The most milestones one workload holds: "A workload can have a maximum of 100
/// milestones" (`MilestoneNumber` doc), which is also why `MilestoneNumber` is
/// `@range(min: 1, max: 100)`. Numbers are handed out from 1 upwards and never reused,
/// so this is what bounds the milestone map per workload.
pub const MAX_MILESTONES_PER_WORKLOAD: i32 = 100;

/// The largest page most of this service's paginated operations serve, and the page size
/// a request that omits `MaxResults` gets.
///
/// Every paginated operation here targets the one modelled `MaxResults` shape, but its
/// `@range` is a MEMBER trait: all but one of them bound it to
/// `@range(min: 1, max: 50)`, so one ceiling covers them, and a request that omits the
/// member asks for a full page. The exception carries its own ceiling
/// ([`IMPROVEMENTS_MAX_RESULTS_CEILING`]).
pub const MAX_RESULTS_CEILING: i32 = 50;

/// The ceiling `ListLensReviewImprovements` serves: its `MaxResults` member is
/// `@range(min: 1, max: 100)`
/// (`smithy_query(query="operation", operation="ListLensReviewImprovements")`), twice
/// what every other paginated member of this service allows.
pub const IMPROVEMENTS_MAX_RESULTS_CEILING: i32 = 100;

/// [`MAX_RESULTS_CEILING`] applied to the `MaxResults` a request carried: the modelled
/// range restated in the handler, so the page size is in `1..=MAX_RESULTS_CEILING`
/// whatever reaches it.
pub fn page_size(max_results: Option<i32>) -> usize {
    page_size_up_to(max_results, MAX_RESULTS_CEILING)
}

/// [`page_size`] against the ceiling the CALLING operation's `MaxResults` member is
/// bounded by, for the one whose bound is not [`MAX_RESULTS_CEILING`]: the page size is
/// in `1..=ceiling` whatever reaches it, and a request that omits the member gets a full
/// page of that operation's own size.
pub fn page_size_up_to(max_results: Option<i32>, ceiling: i32) -> usize {
    let clamped = max_results.unwrap_or(ceiling).clamp(1, ceiling);
    usize::try_from(clamped)
        .expect("MaxResults is clamped into 1..=ceiling, which always fits in a usize")
}

/// One stored answer to one question of one lens review, as the operations that answer
/// a question record it.
///
/// Only the members a projection reports are held: `AnswerSummary` carries the per-choice
/// answers, whether the question applies and the reason it does not, and `Answer` /
/// `ReviewTemplateAnswer` carry the answer's notes too. Neither the question's RISK nor
/// its `SelectedChoices` is stored — both are derived from what IS stored, against the
/// question the answer answers ([`AnswerRecord::risk`],
/// [`AnswerRecord::selected_choices_of`]), on every read, so they move when the answer or
/// the question's choice set does.
#[derive(Debug, Clone)]
pub struct AnswerRecord {
    /// The answer recorded for each choice the answer speaks about, keyed by `ChoiceId`.
    ///
    /// How this answer REMEMBERS the per-choice statuses, which is not the same thing as
    /// the choices it reports them for: the key space they live in is the question's own
    /// choice set, so a read resolves this map against that question
    /// ([`Self::reported_choice_answers`]) rather than reporting or rating it as stored.
    pub choice_answers: BTreeMap<String, ChoiceAnswerRecord>,
    /// `IsApplicable`: false when the question does not apply to this workload.
    pub is_applicable: bool,
    /// `Reason` (an `AnswerReason`): why the question does not apply, when it does not.
    pub reason: Option<String>,
    /// `Notes`: the notes recorded against this answer, absent until one is written.
    pub notes: Option<String>,
}

/// One per-choice answer as a read of an answer REPORTS it: the choice of the question,
/// the status it carries, and the reason and notes recorded with it.
///
/// Distinct from [`ChoiceAnswerRecord`], which is what the store HOLDS: an answer is read
/// against the QUESTION it answers ([`AnswerRecord::reported_choice_answers`]), so a
/// choice the question asks about that the record holds no status for is reported too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReportedChoiceAnswer<'answer> {
    /// `ChoiceAnswer$ChoiceId` / `ChoiceAnswerSummary$ChoiceId` — a choice of the question
    /// the answer was read against.
    pub choice_id: &'answer str,
    /// `Status`: a `ChoiceStatus` (`SELECTED` | `UNSELECTED` | `NOT_APPLICABLE`).
    pub status: &'answer str,
    /// `Reason`: a `ChoiceReason`, when one was given.
    pub reason: Option<&'answer str>,
    /// `ChoiceAnswer$Notes`, when any were given. `ChoiceAnswerSummary` models no notes
    /// member, so only the full answer reports it.
    pub notes: Option<&'answer str>,
}

/// One stored per-choice answer: the choice's status, the reason for it and its notes.
#[derive(Debug, Clone)]
pub struct ChoiceAnswerRecord {
    /// A `ChoiceStatus` (`SELECTED` | `UNSELECTED` | `NOT_APPLICABLE`).
    pub status: String,
    /// A `ChoiceReason`, when one was given.
    pub reason: Option<String>,
    /// `ChoiceAnswer$Notes` — "the notes associated with a choice", when any were given.
    /// `ChoiceAnswerSummary` models no notes member, so only the full answer reports it.
    pub notes: Option<String>,
}

impl AnswerRecord {
    /// The answer a question carries the moment it is first answered: every choice the
    /// question asks about recorded [`CHOICE_STATUS_UNSELECTED`] — a best practice is not
    /// in place until an answer says it is — with nothing selected, the question applying
    /// and no reason or notes.
    ///
    /// The whole choice set is recorded rather than only the choices a request speaks
    /// about, so an answered question reports one `ChoiceAnswers` entry per choice of the
    /// question however it was answered.
    pub fn for_question(question: &ReviewQuestion) -> Self {
        Self {
            choice_answers: question
                .each_choice_id()
                .map(|choice_id| {
                    (
                        choice_id.to_string(),
                        ChoiceAnswerRecord {
                            status: CHOICE_STATUS_UNSELECTED.to_string(),
                            reason: None,
                            notes: None,
                        },
                    )
                })
                .collect(),
            is_applicable: true,
            reason: None,
            notes: None,
        }
    }

    /// Record `selected` as exactly the choices of `question` that are in place: each
    /// `ChoiceId` it names becomes [`CHOICE_STATUS_SELECTED`] and every OTHER choice of
    /// the question becomes [`CHOICE_STATUS_UNSELECTED`] — "the values entered replace the
    /// previously selected choices" (`SelectedChoices` doc).
    ///
    /// Only each choice's STATUS is written: a choice's reason and notes are what a
    /// `ChoiceUpdate` carries, so they are left as stored.
    pub fn select_only(&mut self, question: &ReviewQuestion, selected: &[String]) {
        let selected: BTreeSet<&str> = selected.iter().map(String::as_str).collect();
        for choice_id in question.each_choice_id() {
            let status = if selected.contains(choice_id) {
                CHOICE_STATUS_SELECTED
            } else {
                CHOICE_STATUS_UNSELECTED
            };
            self.choice_answers
                .entry(choice_id.to_string())
                .and_modify(|choice| choice.status = status.to_string())
                .or_insert_with(|| ChoiceAnswerRecord {
                    status: status.to_string(),
                    reason: None,
                    notes: None,
                });
        }
    }

    /// This answer read against the question it answers: one entry per choice `question`
    /// asks about, in the lens's own choice order.
    ///
    /// The key space an answer's per-choice statuses live in is the QUESTION's choice set
    /// (the `ChoiceId` of each best practice it asks about), and the stored map is only how
    /// this answer remembers them, so the two are reconciled HERE on every read: a choice
    /// the question asks about that the map holds no status for is
    /// [`CHOICE_STATUS_UNSELECTED`] — a best
    /// practice is not in place until an answer says it is — and a `ChoiceId` the question
    /// does not ask about is not part of this answer at all and is left out. That matters
    /// because a review can be moved to another version of its lens
    /// (`UpgradeLensReview`, `UpgradeReviewTemplateLensReview`) and a template's answers
    /// can be carried into a workload's review of another version, either of which changes
    /// the choice set the same `QuestionId` asks about.
    ///
    /// The one place a stored answer is resolved against a question, so the choices an
    /// answer reports, the ones it reports as selected ([`Self::selected_choices_of`]) and
    /// the risk derived from it ([`Self::risk`]) cannot disagree with each other or with
    /// the question's own choice catalog ([`ReviewQuestion::choices`]).
    pub fn reported_choice_answers<'answer>(
        &'answer self,
        question: &'answer ReviewQuestion,
    ) -> impl Iterator<Item = ReportedChoiceAnswer<'answer>> + 'answer {
        question.each_choice_id().map(move |choice_id| {
            let stored = self.choice_answers.get(choice_id);
            ReportedChoiceAnswer {
                choice_id,
                status: stored.map_or(CHOICE_STATUS_UNSELECTED, |choice| choice.status.as_str()),
                reason: stored.and_then(|choice| choice.reason.as_deref()),
                notes: stored.and_then(|choice| choice.notes.as_deref()),
            }
        })
    }

    /// The `SelectedChoices` this answer reports for `question`: the choices of the
    /// question it records [`CHOICE_STATUS_SELECTED`] for, in the lens's own choice order.
    ///
    /// Derived rather than stored, so the selected choices and the per-choice answers a
    /// client reads back cannot contradict each other whichever of them the request wrote
    /// through — and so neither of them can name a choice the question does not ask about.
    pub fn selected_choices_of(&self, question: &ReviewQuestion) -> Vec<String> {
        self.reported_choice_answers(question)
            .filter(|choice| choice.status == CHOICE_STATUS_SELECTED)
            .map(|choice| choice.choice_id.to_string())
            .collect()
    }

    /// The `Risk` this answer carries for the question it answers, derived from what it
    /// records for that question's own choices.
    ///
    /// The Well-Architected Tool rates a question by what its answer says about that
    /// question's best practices, and a question is answered only while its answer says
    /// something about one of them: an answer that records NO choice of the question —
    /// because it selected none, or because a later update cleared the selection it had —
    /// leaves the question unanswered, so it rates [`RISK_UNANSWERED`], the same rating a
    /// question with no stored answer at all carries. Beyond that: a question that does not
    /// apply is `NOT_APPLICABLE`, an answered question with no best practice in place is
    /// `HIGH`, one where some choice is still unselected is `MEDIUM`, and one with nothing
    /// left unselected is `NONE`.
    ///
    /// The best practices rated are the ones `question` asks about — the rating is of that
    /// question, so it moves with the choice set the review's lens version defines for it
    /// ([`Self::reported_choice_answers`]), not with whatever the stored map happens to
    /// hold.
    pub fn risk(&self, question: &ReviewQuestion) -> &'static str {
        if !self.is_applicable {
            return RISK_NOT_APPLICABLE;
        }
        let mut any_in_place = false;
        let mut any_left_out = false;
        // Whether the answer says anything at all about one of the question's choices: a
        // choice it records as in place, or one it records as not applying to this review.
        // A choice left UNSELECTED is what every choice of a question starts out as, so it
        // is not something an answer says.
        let mut answers_a_choice = false;
        for choice in self.reported_choice_answers(question) {
            if choice.status == CHOICE_STATUS_SELECTED {
                any_in_place = true;
                answers_a_choice = true;
            } else if choice.status == CHOICE_STATUS_UNSELECTED {
                any_left_out = true;
            } else {
                answers_a_choice = true;
            }
        }
        if !answers_a_choice {
            return RISK_UNANSWERED;
        }
        if !any_in_place {
            return RISK_HIGH;
        }
        if any_left_out {
            return RISK_MEDIUM;
        }
        RISK_NONE
    }
}

/// The `Answer` projection of one question of a lens review: the question as the LENS
/// defines it, plus the answer the reviewed resource records for it.
///
/// Lives here rather than in a handler because both the read of a single answer and the
/// write that records one report it, so the two cannot disagree about what an answer
/// looks like. A question with NO stored answer is `UNANSWERED` and applicable, with
/// nothing selected and no choice answered — the same reading of an unanswered question
/// the risk roll-up counts ([`WorkloadRecord::risk_counts`]).
///
/// `QuestionDescription`, `ImprovementPlanUrl`, `HelpfulResourceUrl` and
/// `HelpfulResourceDisplayText` are omitted: each is per-question or per-choice prose
/// this emulator's lens content does not carry, and the members are optional, so they
/// are left off rather than filled with something else.
pub fn answer_projection(
    question: &ReviewQuestion,
    answer: Option<&AnswerRecord>,
    jira_configuration: Option<JiraConfiguration>,
) -> crate::ops::types::Answer {
    crate::ops::types::Answer {
        question_id: Some(question.question_id.clone()),
        pillar_id: Some(question.pillar_id.clone()),
        question_title: Some(question.title.clone()),
        question_description: None,
        improvement_plan_url: None,
        helpful_resource_url: None,
        helpful_resource_display_text: None,
        // The question's CHOICE CATALOG: the best practices it asks about, reported
        // whether or not the question has been answered, because it describes the
        // QUESTION. It is also the only place a client can discover the `ChoiceId`s an
        // update names.
        choices: Some(question.choices()),
        selected_choices: Some(
            answer
                .map(|answer| answer.selected_choices_of(question))
                .unwrap_or_default(),
        ),
        choice_answers: Some(
            answer
                .map(|answer| {
                    answer
                        .reported_choice_answers(question)
                        .map(|choice| crate::ops::types::ChoiceAnswer {
                            choice_id: Some(choice.choice_id.to_string()),
                            status: Some(choice.status.to_string()),
                            reason: choice.reason.map(str::to_string),
                            notes: choice.notes.map(str::to_string),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        // A question applies unless an answer says it does not.
        is_applicable: Some(answer.is_none_or(|answer| answer.is_applicable)),
        risk: Some(
            answer
                .map_or(RISK_UNANSWERED, |answer| answer.risk(question))
                .to_string(),
        ),
        // `Notes` and `Reason` exist only once an answer records them.
        notes: answer.and_then(|answer| answer.notes.clone()),
        reason: answer.and_then(|answer| answer.reason.clone()),
        // The integration state of this question's issue in the integrated service, which
        // exists once a sync has recorded one ([`WorkloadRecord::question_jira_configuration`]).
        jira_configuration,
    }
}

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// lens — the resource type `ShareResourceType` names a lens by
/// (`smithy_query(query="shape", shape="ShareResourceType")` publishes `LENS`).
pub const LENS_RESOURCE_TYPE: &str = "LENS";

/// `ImportLensStatus$IN_PROGRESS`: an import that has been accepted and whose processing
/// has not finished (`smithy_query(query="shape", shape="ImportLensStatus")`).
pub const IMPORT_LENS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `ImportLensStatus$COMPLETE`: the import finished.
pub const IMPORT_LENS_COMPLETE: &str = "COMPLETE";

/// How long after `ImportLens` accepts a lens document its import reports `COMPLETE`.
///
/// The operation answers with an `ImportLensStatus`, so the import is a job rather than an
/// edit, and AWS publishes no duration for it: this is the emulator's own span, non-zero so
/// that a caller acting straight after the call still observes `IN_PROGRESS`. It gates the
/// STATUS only — "Import a new custom lens or update an existing custom lens" is what the
/// accepting call itself does, so the document it validated is the lens's content from that
/// moment on and every read and publish of the lens resolves it at once.
pub const LENS_IMPORT_DURATION: Duration = Duration::from_millis(500);

/// The `schemaVersion` a custom lens document must declare — the version of the lens
/// JSON format specification the Well-Architected Tool User Guide publishes, which
/// `ImportLens`'s own doc points at ("Lenses are defined in JSON. For more information,
/// see JSON format specification in the Well-Architected Tool User Guide").
pub const LENS_SCHEMA_VERSION: &str = "2021-11-01";

/// The risks a lens document's `riskRules` may assign, from that same format
/// specification. They are the LENS DOCUMENT's own vocabulary, not the API's `Risk`
/// enum: a rule names `HIGH_RISK` where the API reports `HIGH`.
pub const LENS_JSON_RISK_VALUES: &[&str] = &["HIGH_RISK", "MEDIUM_RISK", "NO_RISK"];

/// The condition the LAST risk rule of a question carries: the rule that applies when
/// no earlier rule matched.
pub const LENS_JSON_DEFAULT_CONDITION: &str = "default";

/// The member of `ImportLensInput` a lens-document refusal names.
pub const LENS_JSON_MEMBER: &str = "JSONString";

/// The most custom lenses this emulator holds.
///
/// Custom lenses are metered per account per Region, and `ImportLens` models
/// `ServiceQuotaExceededException` (HTTP 402, "The user has reached their resource
/// quota") for the case; the real quota is adjustable through Service Quotas, so — as
/// with [`MAX_WORKLOADS_PER_ACCOUNT_REGION`] — the number here is this emulator's own
/// ceiling rather than a value read off a doc page. This emulator serves one account
/// ([`::engine::emulator_account_id`]) and holds one store per Region
/// ([`RegionalStores`]), so the lens count of a store IS that account's count in that
/// Region, and this ceiling is what bounds the lens map's growth. It sits far above any
/// realistic client workflow.
pub const MAX_CUSTOM_LENSES_PER_ACCOUNT_REGION: usize = 1_000;

/// The most versions one custom lens carries: "A lens can have up to 100 versions" (the
/// `CreateLensVersion` doc). `CreateLensVersion` models `ServiceQuotaExceededException`
/// (HTTP 402, "The user has reached their resource quota"), which is what a publish past
/// this ceiling answers, so this is what bounds a lens record's version map.
pub const MAX_LENS_VERSIONS_PER_LENS: usize = 100;

/// The most idempotency tokens ONE ledger remembers.
///
/// A `ClientRequestToken` is caller-chosen and a replay of one creates no resource, so no
/// resource quota bounds a ledger keyed by it: each carries its own ceiling, evicting the
/// token remembered longest ago ([`TokenLedger::remember`]), the convention this workspace
/// uses for an idempotency ledger. A caller whose token has been forgotten gets a fresh
/// create rather than a replay, which is also what AWS's own "the result of the original
/// request is returned" window does once it closes. It sits far above any realistic client
/// workflow.
pub const MAX_REMEMBERED_REQUEST_TOKENS: usize = 10_000;

/// A BOUNDED ledger of what each `ClientRequestToken` a caller supplied resolved to.
///
/// Every idempotent operation of this service needs one, and caller input keys it — so it
/// is this type, not a bare `HashMap`, that the state struct holds: there is no `insert`
/// to reach, only [`Self::remember`], which holds the ledger at
/// [`MAX_REMEMBERED_REQUEST_TOKENS`] by forgetting the token remembered longest ago. That
/// is what keeps a stream of fresh tokens from growing the process without bound.
#[derive(Debug)]
pub struct TokenLedger<Token> {
    remembered: HashMap<String, Token>,
    /// The remembered tokens in insertion order, so the ledger evicts the one remembered
    /// longest ago first. A `HashMap` alone cannot say which that is.
    order: VecDeque<String>,
}

/// An empty ledger, whatever the token type: `#[derive(Default)]` would demand
/// `Token: Default`, which a token record has no reason to be.
impl<Token> Default for TokenLedger<Token> {
    fn default() -> Self {
        Self {
            remembered: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<Token> TokenLedger<Token> {
    /// What `client_request_token` resolved to, if the ledger still remembers it.
    pub fn get(&self, client_request_token: &str) -> Option<&Token> {
        self.remembered.get(client_request_token)
    }

    /// Whether the ledger still remembers `client_request_token`.
    pub fn contains_key(&self, client_request_token: &str) -> bool {
        self.remembered.contains_key(client_request_token)
    }

    /// How many tokens the ledger holds — at most [`MAX_REMEMBERED_REQUEST_TOKENS`].
    pub fn len(&self) -> usize {
        self.remembered.len()
    }

    /// Whether the ledger remembers nothing.
    pub fn is_empty(&self) -> bool {
        self.remembered.is_empty()
    }

    /// Remember what `client_request_token` resolved to, within
    /// [`MAX_REMEMBERED_REQUEST_TOKENS`].
    ///
    /// Re-recording a token already held replaces its entry and evicts nothing: it names
    /// the same request. A token the ledger has forgotten is not a replay, so the next
    /// request carrying it acts afresh — which is what bounds a stream of fresh tokens,
    /// caller-chosen as they are.
    pub fn remember(&mut self, client_request_token: String, token: Token) {
        if let Some(known) = self.remembered.get_mut(&client_request_token) {
            *known = token;
            return;
        }
        while self.order.len() >= MAX_REMEMBERED_REQUEST_TOKENS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.remembered.remove(&oldest);
                }
                None => break,
            }
        }
        self.remembered
            .insert(client_request_token.clone(), token);
        self.order.push_back(client_request_token);
    }

    /// Forget every token whose record `keep` rejects — what a delete of the resource a
    /// token resolved to does to that token.
    pub fn retain<Keep: Fn(&Token) -> bool>(&mut self, keep: Keep) {
        self.remembered.retain(|_, token| keep(token));
        self.order
            .retain(|client_request_token| self.remembered.contains_key(client_request_token));
    }

    /// Forget ONE token: what a token resolving to a resource that is no longer stored
    /// does to it, so the next request carrying it acts afresh.
    pub fn forget(&mut self, client_request_token: &str) {
        self.remembered.remove(client_request_token);
        self.order.retain(|known| known != client_request_token);
    }
}

/// What a COMPLETED delete's `ClientRequestToken` resolves to: the resource the original
/// request removed, plus a fingerprint of that request's parameters so a token spent on a
/// DIFFERENT request can be told apart from a genuine retry.
///
/// A delete is the one idempotent shape whose resource is GONE by the time a retry
/// arrives, so the ledger entry — not the resource — is what the replay is served from:
/// there is nothing left to project, only the record that this exact request has already
/// executed once. That is why ONE record type serves every delete this service models,
/// where each create carries its own (its replay has to answer the created resource's
/// identifiers).
#[derive(Debug, Clone)]
pub struct CompletedDeleteToken {
    /// The identifier the original request named — the id or ARN of the resource it
    /// deleted — so a token spent on another request can report what it already resolved
    /// to.
    pub resource_id: String,
    /// [`parameter_fingerprint`] of the original request's parameters.
    pub fingerprint: String,
}

/// What a delete carrying a `ClientRequestToken` must do, before it resolves its target.
#[derive(Debug)]
pub enum DeleteReplay {
    /// The ledger does not remember the token: this is a new request, so run the delete.
    Fresh,
    /// The token names THIS request, which already completed successfully: answer its
    /// result again rather than resolving a resource the original request removed.
    Replay,
    /// The token was already spent on a request with different parameters, so this one
    /// cannot be the same request: refuse with the modelled conflict.
    Conflict(ConflictException),
}

/// How a delete carrying `client_request_token` must proceed, read off `ledger` — the
/// ledger of deletes of this kind that have already completed.
///
/// This is the `@idempotencyToken` rule the `ClientRequestToken` shape documents, applied
/// to a delete: a retry with the same token and the same parameters, after the original
/// request completed successfully, is answered with the original request's result. A
/// delete's result is its empty success, so a caller that resolves this to
/// [`DeleteReplay::Replay`] answers that success without touching the store — the retry
/// must not be a not-found just because the original request did its job.
///
/// `resource_type` and the ledger entry's [`CompletedDeleteToken::resource_id`] are what
/// the conflict names, so a token reused for another target says which resource it was
/// already spent on.
pub fn delete_replay(
    ledger: &TokenLedger<CompletedDeleteToken>,
    client_request_token: &str,
    resource_type: &str,
    fingerprint: &str,
) -> DeleteReplay {
    match ledger.get(client_request_token) {
        None => DeleteReplay::Fresh,
        Some(spent) if spent.fingerprint == fingerprint => DeleteReplay::Replay,
        Some(spent) => DeleteReplay::Conflict(idempotency_token_conflict(
            resource_type,
            spent.resource_id.clone(),
            client_request_token,
        )),
    }
}

/// What a COMPLETED upgrade's `ClientRequestToken` resolves to: the resource the original
/// request upgraded, plus a fingerprint of that request's parameters so a token spent on a
/// DIFFERENT request can be told apart from a genuine retry.
///
/// Held for the same reason [`CompletedDeleteToken`] is: an upgrade's result is its empty
/// success, so there is nothing to project and only the record that this exact request has
/// already executed once. Replaying it from the record rather than re-running it is what
/// keeps a retry from moving a lens review onto a version published since the original
/// request answered.
#[derive(Debug, Clone)]
pub struct CompletedUpgradeToken {
    /// The identifier the original request named — the ARN of the resource whose lens
    /// review it upgraded — so a token spent on another request can report what it
    /// already resolved to.
    pub resource_id: String,
    /// [`parameter_fingerprint`] of the original request's parameters.
    pub fingerprint: String,
}

/// The most tags one resource carries: `TagMap` is `@length(min: 1, max: 50)` wherever a
/// request supplies one (`smithy_query(query="operation", operation="ImportLens")`), so
/// 50 is the most a stored resource can hold too.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The modelled member a tag-count refusal reports (`TagResourceInput$Tags`,
/// `ImportLensInput$Tags` — every tag-writing operation that takes a `TagMap` names it
/// `Tags`).
pub const TAGS_MEMBER: &str = "Tags";

/// The same member on an operation that takes a `Tags` LIST instead, which this service's
/// agent surface spells in lower camel case (`CreateAgentProfileRequest$tags`) — a field
/// finding names the member the request actually carried, as [`PILLARS_MEMBER`] does.
pub const AGENT_TAGS_MEMBER: &str = "tags";

/// How many tags a resource would hold once `requested` is merged into `stored` — the
/// count [`MAX_TAGS_PER_RESOURCE`] bounds.
///
/// The UNION of the two key sets, because a request that names a key the resource
/// already carries overwrites that tag rather than adding one: the stored count plus
/// the requested keys the resource does not carry yet. Shared by every operation that
/// writes tags to a resource, so two tag writers cannot drift on what they count.
pub fn merged_tag_count(
    stored: &BTreeMap<String, String>,
    requested: &BTreeMap<String, String>,
) -> usize {
    stored.len()
        + requested
            .keys()
            .filter(|key| !stored.contains_key(*key))
            .count()
}

/// The constraint sentence a tag write is refused with when it would raise `subject`'s
/// stored tag count above [`MAX_TAGS_PER_RESOURCE`] (`subject` is the resource as the
/// operation names it, e.g. `the lens`). One wording for every tag writer, for the
/// reason [`merged_tag_count`] is shared.
pub fn tag_ceiling_constraint(subject: &str) -> String {
    format!("Member must not raise {subject}'s tag count above {MAX_TAGS_PER_RESOURCE}")
}

/// How a tag write is refused when a pair it would store is one the modelled `Tag`
/// STRUCTURE cannot carry, or `None` when every pair can be carried.
///
/// The two tag shapes of this service are NOT interchangeable: a `TagMap` value
/// (`TagValue`) is `@length(max: 256)` with no minimum and no pattern, while `Tag$value` is
/// `@required` with `@length(min: 1, max: 256)` over a character class
/// (`smithy_query(query="shape", shape="Tag,TagMap")`). A resource whose own reads project
/// its tags as `Tag` structures
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::tags_reported_as_tag_structures`])
/// therefore cannot be given a pair only a `TagMap` carries — the empty value, or a value
/// outside that class — because a later read of it would report a member value the model
/// says that member cannot hold.
///
/// The rule is READ OFF THE MODEL rather than restated here: each pair is walked against
/// `Tag`'s own schema by the framework's constraint walk — the same code, in the same
/// words, that refuses the pair when a create carries it as a `Tags` list entry — and each
/// finding is re-pathed onto the map entry the request sent the pair in, which is the path
/// that walk gives a map value that itself failed and where the caller has to fix it.
/// `member` is the `TagMap` member that carried the pairs. They are judged in key order and
/// the FIRST pair that cannot be carried is reported, so the refusal neither depends on the
/// request map's iteration order nor grows with the number of pairs.
///
/// The KEY needs no rule of its own: `TagMap`'s `TagKey` and `Tag$key` declare the same
/// `@length(1, 128)` over the same class, so a key the request boundary admitted is a key
/// the structure carries — the walk below covers it anyway, because it judges the whole
/// pair.
pub fn tag_structure_refusal(
    member: &str,
    requested: &BTreeMap<String, String>,
) -> Option<String> {
    use ::aws_smithy_schema_server::constraints::{envelope, path, validate, Violation};

    let member_path = path::member(member);
    requested.iter().find_map(|(key, value)| {
        let findings: Vec<Violation> = validate(
            &Tag {
                key: key.clone(),
                value: value.clone(),
            },
            &Tag::SCHEMA,
        )
        .into_iter()
        .map(|finding| Violation {
            path: format!("{member_path}.{key}"),
            ..finding
        })
        .collect();
        envelope(&findings)
    })
}

/// One profile associated with a workload: "The profile associated with a workload"
/// (`smithy_query(query="shape", shape="WorkloadProfile")`).
///
/// A workload is reviewed against ONE VERSION of the profile — `WorkloadProfile` carries
/// `ProfileVersion` beside `ProfileArn`, and `UpgradeProfileVersion` is what moves a
/// workload to a later one — so the association records the version it is pinned at
/// rather than resolving the profile's current one on every read.
#[derive(Debug, Clone)]
pub struct WorkloadProfileRecord {
    pub profile_arn: String,
    pub profile_version: String,
}

impl WorkloadProfileRecord {
    /// The `WorkloadProfile` projection a workload read reports this association by.
    pub fn to_workload_profile(&self) -> crate::ops::types::WorkloadProfile {
        crate::ops::types::WorkloadProfile {
            profile_arn: Some(self.profile_arn.clone()),
            profile_version: Some(self.profile_version.clone()),
        }
    }
}

/// Where the tags of the resource a tag ARN names LIVE in this store: which map holds the
/// record the tags sit on, plus — for a workload, which is keyed by its `WorkloadId`
/// rather than by its ARN — the key it is held under.
///
/// Resolved in ONE place
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::tagged_resource`]),
/// so reading a resource's tags and writing them cannot disagree about what an ARN names.
enum TaggedResource {
    /// The workload held under this `WorkloadId`.
    Workload(String),
    /// The review template held under the ARN as sent.
    ReviewTemplate,
    /// The Well-Architected Tool profile held under the ARN as sent.
    Profile,
    /// The agent profile held under the ARN as sent.
    AgentProfile,
    /// The custom lens the ARN resolves to
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_by_alias`]).
    Lens,
}

/// ONE lens review of a workload as the store holds it: the version of the lens the
/// review is against, the review's own notes, the notes of each pillar of the lens under
/// review, and the Jira question selection recorded against it.
///
/// A lens review of a workload is not a resource of its own — it exists exactly while
/// that lens is associated with the workload ([`WorkloadRecord::reviews_lens`]) — so what
/// this record holds is the version that association pinned and what a caller has written
/// to the review. Everything else a lens review reports — the lens's name and currency,
/// its pillars and its risk roll-up — is DERIVED from the lens content and the workload's
/// stored answers by [`WorkloadRecord::to_lens_review`].
///
/// The mirror of [`ReviewTemplateLensReviewRecord`], which holds the same notes for a review
/// TEMPLATE's lens review, with two differences the two shapes make: a workload's review
/// also carries a Jira question selection (`LensReview$JiraConfiguration`), and it carries
/// no `UpdatedAt` of its own, because a workload's lens review is part of the workload
/// record — so the instant the workload last changed is the instant its reviews last
/// changed ([`WorkloadRecord::updated_at`]), which is also what a milestone freezes.
#[derive(Debug, Clone, Default)]
pub struct LensReviewRecord {
    /// `LensReview$Notes`, absent until a caller writes it.
    pub notes: Option<String>,
    /// `PillarReviewSummary$Notes` per `PillarId`, holding only the pillars a caller has
    /// written notes for.
    pub pillar_notes: BTreeMap<String, String>,
    /// `LensReview$JiraConfiguration`: the pillars and questions of this review a caller
    /// has selected, absent until one is written.
    pub jira_configuration: Option<JiraSelectedQuestionConfiguration>,
    /// When each question of this review was last synced with the integrated service,
    /// keyed by `QuestionId` — what `UpdateIntegration` records and what every read of an
    /// answer reports as `JiraConfiguration$LastSyncedTime`
    /// ([`WorkloadRecord::question_jira_configuration`]). A question nothing has synced
    /// carries no entry, and one a later sync no longer covers keeps the instant it was
    /// last synced at.
    pub jira_question_syncs: BTreeMap<String, SystemTime>,
    /// The `LensVersion` this review is against: the version the lens's content was
    /// published as when the lens was associated with the workload
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::applied_lens_version`]),
    /// which `UpgradeLensReview` moves to the lens's latest version. `None` for a lens
    /// this emulator holds no content for, which has no version to be at, and for a
    /// review whose record was first written to before any version was pinned — which
    /// reads as the lens's latest, exactly as a review template's unpinned review does
    /// ([`ReviewTemplateLensReviewRecord::lens_version`]).
    pub lens_version: Option<String>,
}

/// The CONTENT of ONE lens, as every read of a review of that lens reports and counts it:
/// the pillars the lens is organized into — each with the `PillarName` its content gives
/// it, in the lens's own order — and the questions it asks, in the lens's own order.
///
/// A lens review reports and counts what the LENS defines, and this emulator holds lens
/// content in two halves that never overlap: an AWS-official lens's content is its catalog
/// row ([`official_lens`]) and a custom lens's is the document the version the review is
/// against carries ([`LensRecord::content_at`]). Every derivation over "what does this lens
/// ask?" — a question listing, a risk or question roll-up, a pillar summary, a `PillarId`
/// or `QuestionId` a request supplies — resolves one of them through
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_content_at`] and
/// then reads THIS view, so neither half of the catalog is reported differently from the
/// other and a lens this emulator holds no content for asks nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LensContentView {
    /// `PillarReviewSummary$PillarId` and `$PillarName` of each pillar, in the lens's own
    /// pillar order.
    pillars: Vec<(String, String)>,
    /// The questions the lens asks, in the lens's own order, each carrying the pillar it
    /// belongs to and the choices the lens defines for it.
    questions: Vec<ReviewQuestion>,
}

impl LensContentView {
    /// This lens's content from its two halves: the pillars it is organized into and the
    /// questions it asks, each in the lens's own order.
    pub fn new(pillars: Vec<(String, String)>, questions: Vec<ReviewQuestion>) -> Self {
        Self { pillars, questions }
    }

    /// The lens content this emulator holds for a lens it holds NONE for: no pillar and no
    /// question, so every roll-up over it is the empty roll-up and every identifier a
    /// request supplies for it is taken as given.
    pub fn nothing() -> &'static Self {
        static NOTHING: LensContentView = LensContentView {
            pillars: Vec::new(),
            questions: Vec::new(),
        };
        &NOTHING
    }

    /// The questions this lens asks, in the lens's own order.
    pub fn questions(&self) -> &[ReviewQuestion] {
        &self.questions
    }

    /// The questions this lens asks, taken by value for a caller that reports them.
    pub fn into_questions(self) -> Vec<ReviewQuestion> {
        self.questions
    }

    /// The pillars this lens is organized into, in the lens's own order.
    pub fn pillars(&self) -> &[(String, String)] {
        &self.pillars
    }

    /// The questions ONE pillar of this lens asks, in the lens's own order: what a pillar
    /// review summary of a review of this lens is counted over.
    pub fn pillar_questions<'content>(
        &'content self,
        pillar_id: &'content str,
    ) -> impl Iterator<Item = &'content ReviewQuestion> + 'content {
        self.questions
            .iter()
            .filter(move |question| question.pillar_id == pillar_id)
    }

    /// The `PillarId`s this lens's content organizes it into — the pillar set a `PillarId`
    /// a request supplies is judged against.
    pub fn pillar_ids(&self) -> BTreeSet<&str> {
        self.pillars
            .iter()
            .map(|(pillar_id, _)| pillar_id.as_str())
            .collect()
    }

    /// Whether this lens asks `question_id` in `pillar_id` — a question belongs to the
    /// pillar the lens content puts it in, so a question of another pillar is not this
    /// pillar's.
    pub fn asks_in_pillar(&self, pillar_id: &str, question_id: &str) -> bool {
        self.questions.iter().any(|question| {
            question.pillar_id == pillar_id && question.question_id == question_id
        })
    }

    /// Whether this emulator holds NO content for the lens: it names no pillar and asks no
    /// question, so there is nothing to judge an identifier against.
    pub fn is_empty(&self) -> bool {
        self.pillars.is_empty() && self.questions.is_empty()
    }
}

/// The content of every lens ONE reviewed resource is reviewed against — a workload, the
/// workload a milestone recorded, or a review template — by the lens ALIAS that identifies
/// the lens ([`lens_alias`]), so a lens named two ways is one entry.
///
/// Resolved from the store in one pass
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_content`])
/// and then read by every projection of that resource, so a roll-up over all of its lens
/// reviews and a read of one of them cannot disagree about what a lens asks.
#[derive(Debug, Clone, Default)]
pub struct ReviewedLensContent {
    lenses: BTreeMap<String, LensContentView>,
}

impl ReviewedLensContent {
    /// The content of ONE of those lenses, named as a request names it (an alias or an
    /// ARN). A lens the resource does not review, and one this emulator holds no content
    /// for, asks nothing ([`LensContentView::nothing`]).
    pub fn of(&self, lens: &str) -> &LensContentView {
        self.lenses
            .get(lens_alias(lens))
            .unwrap_or_else(|| LensContentView::nothing())
    }

    /// Every `PillarId` the lenses of this resource's reviews are organized into: the
    /// union of their pillar sets ([`LensContentView::pillar_ids`]), which is the set of
    /// pillars those reviews report a `PillarReviewSummary` for.
    ///
    /// Empty for a resource whose lenses this emulator holds no content for, which is
    /// organized into no pillars HERE — so a caller judging a `PillarId` against this set
    /// has nothing of that lens to judge it by.
    pub fn pillar_ids(&self) -> BTreeSet<&str> {
        self.lenses
            .values()
            .flat_map(LensContentView::pillar_ids)
            .collect()
    }
}

/// The pillars ONE lens review reports a summary for, each with the `PillarName` the lens
/// content gives it where this emulator holds that content.
///
/// The pillars the lens is organized into come first, in the lens's own order
/// ([`LensContentView::pillars`]); then any pillar the review records notes for that the
/// lens content does not name, in `PillarId` order and with no `PillarName`, so notes a
/// caller wrote are always readable back. For a lens this emulator holds content for the
/// second group is empty — an update refuses a `PillarNotes` key that lens does not
/// define — and for a lens it holds none for the first group is, because such a lens is
/// organized into no pillars HERE while the real lens is.
///
/// Shared by a workload's lens review and a review template's: both are reviews of the
/// same lens content and report their pillars the same way, so the rule lives once.
pub fn lens_review_pillars(
    content: &LensContentView,
    pillar_notes: Option<&BTreeMap<String, String>>,
) -> Vec<(String, Option<String>)> {
    let mut pillars: Vec<(String, Option<String>)> = content
        .pillars()
        .iter()
        .map(|(pillar_id, pillar_name)| (pillar_id.clone(), Some(pillar_name.clone())))
        .collect();
    if let Some(written) = pillar_notes {
        let defined = content.pillar_ids();
        // The notes are held in a `BTreeMap`, so the extra pillars come out in `PillarId`
        // order.
        pillars.extend(
            written
                .keys()
                .filter(|pillar_id| !defined.contains(pillar_id.as_str()))
                .map(|pillar_id| (pillar_id.clone(), None)),
        );
    }
    pillars
}

/// A risk roll-up with one entry for every `Risk` the enum publishes, all zero: what
/// every roll-up here starts from, so a caller that reads a bucket to report review
/// progress always finds it.
fn empty_risk_counts() -> BTreeMap<String, i32> {
    RISK_VALUES
        .iter()
        .map(|risk| ((*risk).to_string(), 0))
        .collect()
}

/// A question roll-up with one entry for every `Question` the enum publishes, all zero:
/// what every `QuestionCounts` roll-up here starts from, so a caller that reads a bucket
/// to report review progress always finds it — including for a lens this emulator holds
/// no question set for, whose every bucket stays 0.
fn empty_question_counts() -> BTreeMap<String, i32> {
    QUESTION_VALUES
        .iter()
        .map(|bucket| ((*bucket).to_string(), 0))
        .collect()
}

/// A risk roll-up as the modelled `RiskCounts` map a response carries.
pub fn risk_counts_to_map(counts: &BTreeMap<String, i32>) -> HashMap<String, i32> {
    counts.iter().map(|(k, v)| (k.clone(), *v)).collect()
}

/// ONE best practice of ONE lens review of ONE workload, as a read of the Trusted Advisor
/// checks associated to it resolves it
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::best_practice_checks`]).
///
/// The workload is borrowed from the store the resolution read, so a caller still reads the
/// accounts it covers and the account that owns it off the one record the resolution
/// resolved, rather than looking it up a second time.
#[derive(Debug)]
pub struct BestPracticeChecks<'store> {
    /// The workload the request named, which the checks are reported for.
    pub workload: &'store WorkloadRecord,
    /// The `LensArn` the lens under review is reported by, in the Region of the request
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_arn_of`]).
    pub lens_arn: String,
    /// The checks associated to the best practice, in the catalog's order
    /// ([`checks_for_best_practice`]) — empty while the workload's Trusted Advisor
    /// integration is not activated.
    pub checks: Vec<&'static TrustedAdvisorCheck>,
}

/// How a read of the Trusted Advisor checks of one best practice is refused: the modelled
/// exception for the case, for the calling operation to lift into its own error enum.
///
/// Shared by every operation that reports those checks — the per-account view and the
/// summarized one — so both word the same refusal identically
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::best_practice_checks`]).
#[derive(Debug, Clone)]
pub enum BestPracticeRefusal {
    /// No workload is stored under the id the request named, or that workload does not
    /// review the lens it named — in either case there is no lens review whose best
    /// practices could be checked: a `ResourceNotFoundException` (HTTP 404).
    NotFound(ResourceNotFoundException),
    /// The lens's own content defines no such pillar, asks no such question of that pillar
    /// or offers no such choice of that question, so the request names no best practice of
    /// that lens: a `ValidationException` (HTTP 400).
    Undefined(crate::ops::types::ValidationException),
}

/// One stored milestone: a workload recorded as it stood at one instant.
///
/// The workload is held by VALUE, not by id: "A milestone records the state of a
/// workload at a particular point in time" (`CreateMilestone` doc), so a later
/// `UpdateWorkload` must not change what a recorded milestone reports.
#[derive(Debug, Clone)]
pub struct MilestoneRecord {
    pub milestone_number: i32,
    pub milestone_name: String,
    pub recorded_at: SystemTime,
    pub workload: WorkloadRecord,
}

impl MilestoneRecord {
    /// The `Milestone` shape `GetMilestone` returns. `content` is the content of the
    /// lenses the RECORDED workload reviews, which its risk roll-up is counted over.
    pub fn to_milestone(&self, content: &ReviewedLensContent) -> Milestone {
        Milestone {
            milestone_number: Some(self.milestone_number),
            milestone_name: Some(self.milestone_name.clone()),
            recorded_at: Some(::aws_smithy_types::DateTime::from(self.recorded_at)),
            workload: Some(self.workload.to_workload(content)),
        }
    }

    /// The `MilestoneSummary` projection `ListMilestones` pages over: the same milestone,
    /// with the recorded workload projected to its `WorkloadSummary`
    /// (`smithy_query(query="shape", shape="MilestoneSummary")`).
    pub fn to_summary(&self, content: &ReviewedLensContent) -> MilestoneSummary {
        MilestoneSummary {
            milestone_number: Some(self.milestone_number),
            milestone_name: Some(self.milestone_name.clone()),
            recorded_at: Some(::aws_smithy_types::DateTime::from(self.recorded_at)),
            workload_summary: Some(self.workload.to_summary(content)),
        }
    }
}

/// How recording a milestone of a workload is refused: the modelled exception for the
/// case, for the calling operation to lift into its own error enum.
///
/// Shared by every operation that records one — `CreateMilestone` and the upgrades that
/// keep the review as it stood before they moved it — so all of them word the same
/// refusal identically ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::record_milestone`]).
#[derive(Debug, Clone)]
pub enum MilestoneRefusal {
    /// No workload is stored under the id the milestone would be recorded against, so
    /// there is no workload to record: a `ResourceNotFoundException` (HTTP 404).
    NotFound(ResourceNotFoundException),
    /// A milestone of that workload already holds the name: "Milestone names must be
    /// unique within a workload" (`smithy_query(query="shape", shape="MilestoneName")`),
    /// so the name is taken — a `ConflictException` (HTTP 409).
    NameTaken(ConflictException),
    /// The workload already holds [`MAX_MILESTONES_PER_WORKLOAD`] milestones, which is
    /// "the user has reached their resource quota" — a `ServiceQuotaExceededException`
    /// (HTTP 402).
    QuotaExceeded(crate::ops::types::ServiceQuotaExceededException),
}

/// One lens review of a review template, as the store holds it.
///
/// A lens review of a template is not a resource of its own — it exists exactly while
/// the lens is applied to the template ([`ReviewTemplateRecord::applies_lens`]) — so what
/// this record holds is the review's own state and nothing the lens or the template's
/// answers already say: the notes a caller has written ("the notes that will be
/// associated with the workload when the template is applied",
/// `UpdateReviewTemplateLensReviewInput$LensNotes` / `$PillarNotes` docs), when the review
/// was last written (`ReviewTemplateLensReview$UpdatedAt`, "the date and time when the
/// review template lens review was last updated") and the VERSION of the lens the review
/// is against — pinned when the lens was applied, exactly as a workload's profile
/// association pins the profile version it is reviewed against
/// ([`WorkloadProfileRecord::profile_version`]).
///
/// Everything else a lens review reports — the lens's name and currency, its pillars and
/// its question roll-up — is DERIVED from the lens catalog and the template's stored
/// answers by [`ReviewTemplateRecord::to_lens_review`].
#[derive(Debug, Clone)]
pub struct ReviewTemplateLensReviewRecord {
    /// `ReviewTemplateLensReview$Notes`, absent until a caller writes it.
    pub notes: Option<String>,
    /// `ReviewTemplatePillarReviewSummary$Notes` per `PillarId`, holding only the
    /// pillars a caller has written notes for.
    pub pillar_notes: BTreeMap<String, String>,
    /// `ReviewTemplateLensReview$UpdatedAt`.
    pub updated_at: SystemTime,
    /// The `LensVersion` this review is against: the version the lens's content was
    /// published as when the lens was applied ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::applied_lens_version`]),
    /// which `UpgradeReviewTemplateLensReview` moves to the lens's latest version.
    /// `None` for a lens this emulator holds no content for, which has no version to be
    /// at.
    pub lens_version: Option<String>,
}

/// What the lens catalog says about the lens ONE lens review is a review of, as that
/// review reports it: the ARN the lens is reported by, the `LensVersion` the review is
/// against, the lens's name as of that version and the currency of that version.
///
/// It is resolved from the catalog in ONE step
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens`])
/// and owns every value it carries, so a handler can resolve it BEFORE it takes the
/// resource carrying the review for writing and still project the review afterwards —
/// the same reason [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_arn_of`]
/// is called before the write.
///
/// Every member but the ARN is absent for a lens this emulator holds no content for: such
/// a lens has no version here to be at, and inventing one would mean reporting a review
/// against a version nothing published.
#[derive(Debug, Clone)]
pub struct ReviewedLens {
    /// `ReviewTemplateLensReview$LensArn` / `LensReview$LensArn`.
    pub lens_arn: String,
    /// `ReviewTemplateLensReview$LensVersion` / `LensReview$LensVersion` — the version
    /// the review is against, which is the lens's latest for a review that pins none.
    pub lens_version: Option<String>,
    /// `ReviewTemplateLensReview$LensName` / `LensReview$LensName`, as the content of
    /// that version names the lens.
    pub lens_name: Option<String>,
    /// `ReviewTemplateLensReview$LensStatus` / `LensReview$LensStatus`:
    /// [`LENS_STATUS_CURRENT`] while the review is against the lens's latest version and
    /// [`LENS_STATUS_NOT_CURRENT`] once a later one has been published.
    pub lens_status: Option<String>,
}

/// One stored review template: "A review template" (`ReviewTemplate` shape doc).
///
/// Every member of the modelled `ReviewTemplate` a request can set is persisted here;
/// the members the SERVICE computes — the question roll-up, the owning account and the
/// update status — are derived by [`ReviewTemplateRecord::to_review_template`], the only
/// place a response is built from this record.
#[derive(Debug, Clone)]
pub struct ReviewTemplateRecord {
    pub template_arn: String,
    pub template_name: String,
    pub description: String,
    pub notes: Option<String>,
    pub lenses: Vec<String>,
    pub owner: String,
    pub updated_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// The answers recorded against this template's lens reviews: lens ALIAS (as
    /// [`lens_alias`] resolves it, so one entry however the request named the lens),
    /// then `QuestionId`. Held exactly as a workload holds its own
    /// ([`WorkloadRecord::answers`]) — a review template is reviewed against the same
    /// lens question set, so the two cannot disagree about what an answer records.
    pub answers: BTreeMap<String, BTreeMap<String, AnswerRecord>>,
    /// This template's lens reviews, by lens ALIAS (as [`lens_alias`] resolves it, so one
    /// entry however the request named the lens). An entry exists for every lens applied
    /// to the template, because the review exists as soon as the lens is applied
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::applied_lens_reviews`]).
    pub lens_reviews: BTreeMap<String, ReviewTemplateLensReviewRecord>,
}

impl ReviewTemplateRecord {
    /// The answer this template records for one question of one lens, if any.
    ///
    /// `lens` is a lens as a request names it — an alias or an ARN — and resolves
    /// through [`lens_alias`], so the same answer is found by either name.
    pub fn answer(&self, lens: &str, question_id: &str) -> Option<&AnswerRecord> {
        self.answers.get(lens_alias(lens))?.get(question_id)
    }

    /// [`Self::answer`] for a caller about to WRITE it, created from the question's own
    /// choice set on the first write ([`AnswerRecord::for_question`]).
    ///
    /// Recording an answer is what makes the question `ANSWERED`
    /// ([`Self::question_counts`]), so this is the one place a template's answer comes
    /// into existence. `lens` is a lens as a request names it, so an answer written by
    /// either spelling is the same answer ([`lens_alias`]).
    pub fn answer_mut(&mut self, lens: &str, question: &ReviewQuestion) -> &mut AnswerRecord {
        self.answers
            .entry(lens_alias(lens).to_string())
            .or_default()
            .entry(question.question_id.clone())
            .or_insert_with(|| AnswerRecord::for_question(question))
    }

    /// Whether this template applies the lens a `LensAlias` names, however the template
    /// and the request spell it: a lens review of a template exists only while that
    /// lens is applied to it.
    pub fn applies_lens(&self, lens: &str) -> bool {
        self.lenses
            .iter()
            .any(|applied| lens_alias(applied) == lens_alias(lens))
    }

    /// Drop the review of one lens from this template: the lens itself, the answers
    /// recorded against its review and whatever a caller wrote to that review.
    ///
    /// The mirror of [`WorkloadRecord::disassociate_lens`], and for the same reason: a lens
    /// review is not a resource of its own — it exists exactly while that lens is applied
    /// to the template ([`Self::applies_lens`]) — so dropping the lens drops the review,
    /// and the answers and notes held under it go with it rather than surviving to reappear
    /// under a later apply. A workload created from this template copied its lenses, notes
    /// and answers at create time (`CreateWorkload`), so it keeps them: the workload is not
    /// a child of the template.
    ///
    /// Idempotent in the lens and in the SPELLING of it: a lens is one lens however it is
    /// named ([`lens_alias`]), so every entry naming the same lens goes and the order of
    /// the rest is left alone. It answers nothing, because whether the template applied the
    /// lens is what [`Self::applies_lens`] is for — an operation that has to refuse a lens
    /// the template does not apply resolves it BEFORE removing any.
    pub fn disassociate_lens(&mut self, lens: &str) {
        let alias = lens_alias(lens);
        self.lenses.retain(|applied| lens_alias(applied) != alias);
        self.answers.remove(alias);
        self.lens_reviews.remove(alias);
    }

    /// This template's question roll-up: one entry for every `Question` the enum
    /// publishes, so a caller reading a bucket to report review progress always finds
    /// it.
    ///
    /// `ANSWERED` counts the questions of the applied lenses this template records an
    /// answer for and `UNANSWERED` the rest; a lens is applied once however many ways
    /// the request named it. The map is DERIVED from the applied lenses and the stored
    /// answers on every read and never stored, so it moves when either does — exactly
    /// as a workload's risk roll-up does ([`WorkloadRecord::risk_counts`]).
    ///
    /// `content` is the content of the lenses this template applies, resolved from the
    /// whole lens catalog
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_content`]).
    pub fn question_counts(&self, content: &ReviewedLensContent) -> BTreeMap<String, i32> {
        let mut counts = empty_question_counts();
        let applied: std::collections::BTreeSet<&str> =
            self.lenses.iter().map(|lens| lens_alias(lens)).collect();
        for lens in applied {
            for (bucket, count) in self.lens_question_counts(lens, content) {
                counts
                    .entry(bucket)
                    .and_modify(|total| *total = total.saturating_add(count))
                    .or_insert(count);
            }
        }
        counts
    }

    /// The question roll-up of ONE lens review of this template: the same buckets as
    /// [`Self::question_counts`], counted over the questions that ONE lens asks rather
    /// than over every applied lens's — which is what a lens review reports.
    ///
    /// `lens` is a lens as a request names it (an alias or an ARN), and `content` is as
    /// [`Self::question_counts`] takes it. A lens this emulator holds no content for asks
    /// nothing, so every bucket is 0.
    pub fn lens_question_counts(
        &self,
        lens: &str,
        content: &ReviewedLensContent,
    ) -> BTreeMap<String, i32> {
        self.question_counts_over(lens, content.of(lens).questions().iter())
    }

    /// [`Self::lens_question_counts`] narrowed to the questions ONE pillar of that lens
    /// asks, which is what a pillar review summary of a lens review reports.
    pub fn pillar_question_counts(
        &self,
        lens: &str,
        pillar_id: &str,
        content: &ReviewedLensContent,
    ) -> BTreeMap<String, i32> {
        self.question_counts_over(lens, content.of(lens).pillar_questions(pillar_id))
    }

    /// The question roll-up over an arbitrary set of ONE lens's questions — the one
    /// place a template question is bucketed, so the template's roll-up, a lens
    /// review's and a pillar's cannot disagree about what a question counts as.
    fn question_counts_over<'question>(
        &self,
        lens: &str,
        questions: impl Iterator<Item = &'question ReviewQuestion>,
    ) -> BTreeMap<String, i32> {
        let mut counts = empty_question_counts();
        for question in questions {
            let bucket = if self.answer(lens, &question.question_id).is_some() {
                QUESTION_ANSWERED
            } else {
                QUESTION_UNANSWERED
            };
            counts
                .entry(bucket.to_string())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
        counts
    }

    /// ONE of this template's lens reviews as the store holds it, if the store holds one.
    ///
    /// `lens` is a lens as a request names it, so the same review is found by either
    /// spelling ([`lens_alias`]).
    pub fn lens_review_record(&self, lens: &str) -> Option<&ReviewTemplateLensReviewRecord> {
        self.lens_reviews.get(lens_alias(lens))
    }

    /// [`Self::lens_review_record`] for a caller about to WRITE to it, created empty at
    /// `now` if the store holds none: the lens review itself exists as soon as the lens is
    /// applied, and this record is only how the store remembers that review's own state.
    pub fn lens_review_record_mut(
        &mut self,
        lens: &str,
        now: SystemTime,
    ) -> &mut ReviewTemplateLensReviewRecord {
        self.lens_reviews
            .entry(lens_alias(lens).to_string())
            .or_insert_with(|| ReviewTemplateLensReviewRecord {
                notes: None,
                pillar_notes: BTreeMap::new(),
                updated_at: now,
                lens_version: None,
            })
    }

    /// The `LensVersion` ONE lens review of this template is against, as it was pinned
    /// when the lens was applied or last moved by an upgrade.
    pub fn lens_review_version(&self, lens: &str) -> Option<&str> {
        self.lens_review_record(lens)?.lens_version.as_deref()
    }

    /// The lens reviews this template has, through the one place that rule lives
    /// ([`lens_reviews_of`]) — the same rule a WORKLOAD's lens reviews follow
    /// ([`WorkloadRecord::lens_reviews`]).
    pub fn lens_reviews(&self) -> Vec<(&str, &str)> {
        lens_reviews_of(&self.lenses)
    }

    /// Every lens this template applies, as the apply recorded it, beside the
    /// `LensVersion` its review of that lens is against — the mirror of
    /// [`WorkloadRecord::reviewed_lens_versions`], because a review template's lens review
    /// is a review against ONE version of the lens exactly as a workload's is.
    pub fn reviewed_lens_versions(&self) -> Vec<(&str, Option<&str>)> {
        self.lenses
            .iter()
            .map(|lens| (lens.as_str(), self.lens_review_version(lens)))
            .collect()
    }

    /// The pillars ONE lens review of this template reports a summary for, through the
    /// one place that rule lives ([`lens_review_pillars`]) — the same rule a WORKLOAD's
    /// lens review follows ([`WorkloadRecord::lens_review_pillars`]).
    pub fn lens_review_pillars(
        &self,
        lens: &str,
        content: &ReviewedLensContent,
    ) -> Vec<(String, Option<String>)> {
        lens_review_pillars(
            content.of(lens),
            self.lens_review_record(lens)
                .map(|written| &written.pillar_notes),
        )
    }

    /// The `ReviewTemplateLensReview` shape `GetReviewTemplateLensReview` and
    /// `UpdateReviewTemplateLensReview` return for ONE lens applied to this template.
    ///
    /// `lens` is the lens AS THE REQUEST NAMED IT — `LensAlias` is "either the lens
    /// alias … or the lens ARN" (`LensAlias` doc) and this shape models no member to
    /// resolve one spelling into the other — and `reviewed` is what the lens catalog says
    /// about the lens this review is against, resolved for the version the review is at
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens`]).
    ///
    /// Everything but the review's own state is DERIVED on every read: the lens's name,
    /// version and currency from `reviewed`, one pillar summary per pillar of the lens
    /// ([`Self::lens_review_pillars`]) and the answered/unanswered roll-ups from the
    /// template's stored answers.
    pub fn to_lens_review(
        &self,
        lens: &str,
        reviewed: &ReviewedLens,
        content: &ReviewedLensContent,
    ) -> ReviewTemplateLensReview {
        let written = self.lens_review_record(lens);
        ReviewTemplateLensReview {
            lens_alias: Some(lens.to_string()),
            lens_arn: Some(reviewed.lens_arn.clone()),
            lens_version: reviewed.lens_version.clone(),
            lens_name: reviewed.lens_name.clone(),
            lens_status: reviewed.lens_status.clone(),
            pillar_review_summaries: Some(
                self.lens_review_pillars(lens, content)
                    .into_iter()
                    .map(|(pillar_id, pillar_name)| ReviewTemplatePillarReviewSummary {
                        notes: written
                            .and_then(|written| written.pillar_notes.get(&pillar_id).cloned()),
                        question_counts: Some(
                            self.pillar_question_counts(lens, &pillar_id, content)
                                .into_iter()
                                .collect::<HashMap<String, i32>>(),
                        ),
                        pillar_id: Some(pillar_id),
                        pillar_name,
                    })
                    .collect(),
            ),
            // "The date and time when the review template lens review was last updated":
            // the review's own record carries that instant, and one the store holds no
            // record for was last changed when the template carrying it was.
            updated_at: Some(::aws_smithy_types::DateTime::from(
                written.map_or(self.updated_at, |written| written.updated_at),
            )),
            notes: written.and_then(|written| written.notes.clone()),
            question_counts: Some(
                self.lens_question_counts(lens, content)
                    .into_iter()
                    .collect::<HashMap<String, i32>>(),
            ),
            // `ReviewTemplateLensReview$NextToken` continues a paged read, and neither
            // operation that returns this shape models a paging member to continue, so
            // there is nothing to hand back.
            next_token: None,
        }
    }

    /// The `ReviewTemplateSummary` shape `ListReviewTemplates` reports for this template:
    /// "Summary of a review template" — the members of the stored template a LIST reports,
    /// projected from the same state [`Self::to_review_template`] projects the full shape
    /// from, so a summary and a `GetReviewTemplate` of the same template cannot disagree.
    ///
    /// `ReviewTemplateSummary` models no `Notes`, `Tags`, `QuestionCounts` or
    /// `ShareInvitationId` member (`smithy_query(query="shape",
    /// shape="ReviewTemplateSummary")`), so a summary carries only what it declares.
    ///
    /// `update_status` is this template's `ReviewTemplateUpdateStatus`, which depends on
    /// the lens catalog rather than on the record
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::review_template_update_status`]).
    pub fn to_summary(&self, update_status: &str) -> ReviewTemplateSummary {
        ReviewTemplateSummary {
            description: Some(self.description.clone()),
            lenses: Some(self.lenses.clone()),
            owner: Some(self.owner.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            template_arn: Some(self.template_arn.clone()),
            template_name: Some(self.template_name.clone()),
            update_status: Some(update_status.to_string()),
        }
    }

    /// The `ReviewTemplate` shape `GetReviewTemplate` returns. `update_status` is as
    /// [`Self::to_summary`] takes it, so a summary and a `GetReviewTemplate` of the same
    /// template cannot disagree about it; `content` is the content of the lenses this
    /// template applies, which its question roll-up is counted over
    /// ([`Self::question_counts`]).
    pub fn to_review_template(
        &self,
        update_status: &str,
        content: &ReviewedLensContent,
    ) -> ReviewTemplate {
        ReviewTemplate {
            description: Some(self.description.clone()),
            lenses: Some(self.lenses.clone()),
            notes: self.notes.clone(),
            question_counts: Some(
                self.question_counts(content)
                    .into_iter()
                    .collect::<HashMap<String, i32>>(),
            ),
            owner: Some(self.owner.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            template_arn: Some(self.template_arn.clone()),
            template_name: Some(self.template_name.clone()),
            // The tag map the template carries, empty when it carries none: the
            // `@length(min: 1)` on `TagMap` constrains the tags a REQUEST supplies, not
            // what a projection of the stored template reports.
            tags: Some(self.tags.clone().into_iter().collect()),
            update_status: Some(update_status.to_string()),
            // "The ID assigned to the template share invitation": only a template reached
            // THROUGH a share invitation carries one, which is the recipient's reading of
            // a shared template; the owner reading a template it shared out carries none.
            share_invitation_id: None,
        }
    }

    /// Carry the review content this template holds for each lens it applies onto
    /// `carried`, keyed by lens ALIAS: the answers it records for that lens's questions
    /// and the notes written against that lens review and its pillars.
    ///
    /// A review template is applied to a workload to PRE-POPULATE that workload's review:
    /// "for a review template, these are the notes that will be associated with the
    /// workload when the template is applied" (`smithy_query(query="doc", shape="Notes")`
    /// and `smithy_query(query="doc", shape="PillarNotes")`), and the recorded answers are
    /// what creating a workload from a template needs
    /// `wellarchitected:GetReviewTemplateAnswer` and
    /// `wellarchitected:ListReviewTemplateAnswers` for
    /// (`smithy_query(query="operation", operation="CreateWorkload")`). Every half of that
    /// content is projected from this one place, so an applier cannot carry one half and
    /// drop the other.
    ///
    /// Content already in `carried` is LEFT as it stands, so where two applied templates
    /// speak about the same lens, question or pillar, the first one applied is what the
    /// workload is created with.
    pub fn carry_review_content_into(
        &self,
        carried: &mut BTreeMap<String, CarriedLensReviewContent>,
    ) {
        for (alias, review) in &self.lens_reviews {
            let lens = carried.entry(alias.clone()).or_default();
            if lens.notes.is_none() {
                lens.notes = review.notes.clone();
            }
            for (pillar_id, notes) in &review.pillar_notes {
                lens.pillar_notes
                    .entry(pillar_id.clone())
                    .or_insert_with(|| notes.clone());
            }
        }
        for (alias, answers) in &self.answers {
            let lens = carried.entry(alias.clone()).or_default();
            for (question_id, answer) in answers {
                lens.answers
                    .entry(question_id.clone())
                    .or_insert_with(|| answer.clone());
            }
        }
    }
}

/// The review content ONE lens review of an applied review template contributes to the
/// workload it is applied to ([`ReviewTemplateRecord::carry_review_content_into`]): what
/// the template recorded about that lens, in the form the workload's own review holds it.
#[derive(Debug, Clone, Default)]
pub struct CarriedLensReviewContent {
    /// The answers the template records for this lens's questions, by `QuestionId`.
    pub answers: BTreeMap<String, AnswerRecord>,
    /// The notes written against the template's review of this lens.
    pub notes: Option<String>,
    /// The notes written against the pillars of that review, by `PillarId`.
    pub pillar_notes: BTreeMap<String, String>,
}

/// Write the review content of the applied review templates (`carried`) onto the lens
/// reviews of the workload being created, and answer the workload's `answers` map.
///
/// Only a lens the workload actually reviews takes content: every lens an applied
/// template applies is associated with the workload, so this drops nothing in practice —
/// it is what keeps the store from holding a review or an answer for a lens the workload
/// has no review of.
///
/// A workload being created has nothing of its own in either place — no operation writes a
/// per-lens note or an answer before the workload exists — so the carried content is
/// written as it stands rather than merged.
pub fn apply_carried_review_content(
    carried: BTreeMap<String, CarriedLensReviewContent>,
    lens_reviews: &mut BTreeMap<String, LensReviewRecord>,
) -> BTreeMap<String, BTreeMap<String, AnswerRecord>> {
    let mut answers: BTreeMap<String, BTreeMap<String, AnswerRecord>> = BTreeMap::new();
    for (alias, content) in carried {
        let Some(review) = lens_reviews.get_mut(&alias) else {
            continue;
        };
        review.notes = content.notes;
        review.pillar_notes = content.pillar_notes;
        if !content.answers.is_empty() {
            answers.insert(alias, content.answers);
        }
    }
    answers
}

/// The `ReviewTemplateAnswer` shape `UpdateReviewTemplateAnswer` and
/// `GetReviewTemplateAnswer` report for ONE question of ONE lens review of a review
/// template: "an answer of the question".
///
/// `question` is the question as the LENS defines it and `answer` the answer the template
/// records for it, absent for a question nothing has answered — which is what makes the
/// difference between the two states a `ReviewTemplateAnswerStatus` publishes. A question
/// with no stored answer applies, has nothing selected and speaks about no choice, exactly
/// as the template's question roll-up counts it ([`ReviewTemplateRecord::question_counts`])
/// and as a `ReviewTemplateAnswerSummary` of the same question reports it.
///
/// `QuestionDescription`, `ImprovementPlanUrl`, `HelpfulResourceUrl` and
/// `HelpfulResourceDisplayText` are lens CONTENT this emulator's official-lens catalog
/// does not carry, and every one of them is optional, so they are omitted rather than
/// guessed at.
pub fn review_template_answer(
    question: &ReviewQuestion,
    answer: Option<&AnswerRecord>,
) -> ReviewTemplateAnswer {
    ReviewTemplateAnswer {
        question_id: Some(question.question_id.clone()),
        pillar_id: Some(question.pillar_id.clone()),
        question_title: Some(question.title.clone()),
        question_description: None,
        improvement_plan_url: None,
        helpful_resource_url: None,
        helpful_resource_display_text: None,
        // The per-question CHOICE CATALOG the lens defines: the best practices the
        // question asks about, each with the `ChoiceId` a client passes back in
        // `UpdateReviewTemplateAnswer$SelectedChoices` and `$ChoiceUpdates`. It describes
        // the QUESTION, so it is reported whether or not the template has answered it.
        choices: Some(question.choices()),
        selected_choices: Some(
            answer
                .map(|answer| answer.selected_choices_of(question))
                .unwrap_or_default(),
        ),
        choice_answers: Some(
            answer
                .map(|answer| {
                    answer
                        .reported_choice_answers(question)
                        .map(|choice| ChoiceAnswer {
                            choice_id: Some(choice.choice_id.to_string()),
                            status: Some(choice.status.to_string()),
                            reason: choice.reason.map(str::to_string),
                            notes: choice.notes.map(str::to_string),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        // A question applies unless an answer says it does not.
        is_applicable: Some(answer.is_none_or(|answer| answer.is_applicable)),
        // "The status of whether or not this question has been answered": the stored
        // answer IS that status, so an answer and the template's roll-up cannot disagree.
        answer_status: Some(
            if answer.is_some() {
                REVIEW_TEMPLATE_ANSWER_STATUS_ANSWERED
            } else {
                REVIEW_TEMPLATE_ANSWER_STATUS_UNANSWERED
            }
            .to_string(),
        ),
        notes: answer.and_then(|answer| answer.notes.clone()),
        reason: answer.and_then(|answer| answer.reason.clone()),
    }
}

/// One choice a lens question offers: "choices" in the lens JSON format specification —
/// the `ChoiceId` a client passes back when it answers the question, and the
/// `Choice$Title` the question reports for it.
///
/// It is also how a [`ReviewQuestion`] carries a best practice, whichever half of the
/// catalog the lens comes from: an AWS-official lens's choice id and prose are its
/// catalog row's ([`FrameworkChoice`]), a custom lens's id is the one its document names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensChoice {
    pub id: String,
    pub title: String,
    /// `Choice$Description` — the prose the lens publishes for this best practice, and
    /// `None` for a lens that publishes none.
    pub description: Option<String>,
}

/// One rule that maps the choices selected for a question to the risk the answer
/// carries. `condition` is the format specification's own expression over the
/// question's choice ids, kept VERBATIM: this store records the rule rather than
/// interpreting it, and an answer's `Risk` is derived from the best practices it leaves
/// out ([`AnswerRecord::risk`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensRiskRule {
    pub condition: String,
    pub risk: String,
}

/// One question a lens pillar asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensQuestion {
    pub id: String,
    pub title: String,
    pub choices: Vec<LensChoice>,
    pub risk_rules: Vec<LensRiskRule>,
}

/// One pillar of a lens, with the questions it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensPillar {
    pub id: String,
    pub name: String,
    pub questions: Vec<LensQuestion>,
}

/// A custom lens as its JSON document defines it, after [`parse_lens_json`] has checked
/// it against the lens JSON format specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensDefinition {
    pub schema_version: String,
    pub name: String,
    pub description: Option<String>,
    pub pillars: Vec<LensPillar>,
}

impl LensDefinition {
    /// How many questions this lens asks in all — the roll-up denominator a lens review
    /// of this lens counts against.
    pub fn question_count(&self) -> usize {
        self.pillars
            .iter()
            .map(|pillar| pillar.questions.len())
            .sum()
    }

    /// This lens's pillars, in the order the document lists them.
    pub fn pillar_ids(&self) -> Vec<String> {
        self.pillars
            .iter()
            .map(|pillar| pillar.id.clone())
            .collect()
    }

    /// The questions a review of this lens asks, in the document's own order: pillar by
    /// pillar, each pillar's questions in the order it lists them, each carrying the
    /// pillar it belongs to and the choices the document defines for it.
    ///
    /// This is the custom half of a lens review's question set — the counterpart of
    /// [`FrameworkQuestion::to_review_question`] for the AWS-official catalog — so a
    /// review of an imported lens reports the content its document supplies rather than
    /// nothing.
    pub fn review_questions(&self) -> Vec<ReviewQuestion> {
        self.pillars
            .iter()
            .flat_map(|pillar| {
                pillar.questions.iter().map(|question| ReviewQuestion {
                    pillar_id: pillar.id.clone(),
                    question_id: question.id.clone(),
                    title: question.title.clone(),
                    best_practices: question.choices.clone(),
                })
            })
            .collect()
    }
}

/// One version of a lens's content: the document as it was imported, and the definition
/// parsed out of it. The raw document is kept because it is the resource `ExportLens`
/// returns, and the parsed definition because it is what the service reads the lens's
/// name, pillars and questions from.
#[derive(Debug, Clone)]
pub struct LensContent {
    pub json: String,
    pub definition: LensDefinition,
    pub imported_at: SystemTime,
}

/// One PUBLISHED version of a custom lens: the lens's content as it stood when
/// `CreateLensVersion` published that version, plus what the publishing request said
/// about it.
///
/// A version is immutable — "Create a new lens version… after you have imported a lens"
/// (`CreateLensVersion` doc) — so the content is held BY VALUE here: a later import that
/// changes the lens's draft must not change what a published version carries.
#[derive(Debug, Clone)]
pub struct PublishedLensVersion {
    /// The content published under this version.
    pub content: LensContent,
    /// `CreateLensVersionInput$IsMajorVersion` — "Set to true if this new major lens
    /// version". A request that omits the member is not publishing a major version.
    pub is_major_version: bool,
    pub published_at: SystemTime,
}

/// One stored custom lens.
///
/// A lens imported by `ImportLens` "will have a status of DRAFT. The lens cannot be
/// applied to workloads or shared with other Amazon Web Services accounts until it's
/// published with CreateLensVersion" (`ImportLens` doc) — so a record carries its draft
/// content plus the versions `CreateLensVersion` has published from it, and the latter
/// stays empty until it publishes one.
#[derive(Debug, Clone)]
pub struct LensRecord {
    /// The 32-lowercase-hex resource id the lens ARN ends in.
    pub lens_id: String,
    pub lens_arn: String,
    /// The account that imported the lens — its owner.
    pub owner: String,
    pub created_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// The lens's content: the document of the last import `ImportLens` accepted, which
    /// is a DRAFT until a version is published from it. `None` only for a lens whose
    /// content a delete took away.
    pub draft: Option<LensContent>,
    /// The instant the latest import's `ImportLensStatus` turns `COMPLETE`, if one has
    /// been accepted ([`LENS_IMPORT_DURATION`]). The import's STATUS is all it gates.
    pub import_completes_at: Option<SystemTime>,
    /// The published versions of this lens, by `LensVersion`.
    pub versions: BTreeMap<String, PublishedLensVersion>,
}

impl LensRecord {
    /// The lens's content — the document of the last import of it.
    ///
    /// Readable from the instant `ImportLens` ACCEPTS the document: "Import a new custom
    /// lens or update an existing custom lens" is what that call does, so the document it
    /// validated and recorded is the lens's content from then on, whatever its import job
    /// still reports as its [`Self::import_status`]. `None` only for a lens a delete left
    /// no content on.
    pub fn content(&self) -> Option<&LensContent> {
        self.draft.as_ref()
    }

    /// The `ImportLensStatus` of this lens's latest import as of `now`.
    pub fn import_status(&self, now: SystemTime) -> &'static str {
        match self.import_completes_at {
            Some(completes_at) if completes_at > now => IMPORT_LENS_IN_PROGRESS,
            _ => IMPORT_LENS_COMPLETE,
        }
    }

    /// The version this lens's CURRENT content is published as, as `(LensVersion, that
    /// published version)`, or `None` while that content is a draft nothing has been
    /// published from — which is the lens being in DRAFT.
    ///
    /// "The new or updated lens will have a status of DRAFT. The lens cannot be applied to
    /// workloads or shared with other Amazon Web Services accounts until it's published
    /// with CreateLensVersion" (`ImportLens` doc), so the question is whether a version was
    /// published FROM the content the lens now carries: a version qualifies when it was
    /// published no earlier than that content was imported (publishing copies the content
    /// current at that instant) AND carries that same document. The instant is what makes
    /// an UPDATE a draft again even when the document it imports is byte-identical to a
    /// published one.
    pub fn published_version(&self) -> Option<(&str, &PublishedLensVersion)> {
        let content = self.content()?;
        self.versions
            .iter()
            .filter(|(_, published)| {
                published.published_at >= content.imported_at
                    && published.content.json == content.json
            })
            // `LensVersion` is caller-chosen, so the version NAMES do not order the
            // versions; when more than one carries this content, the one published last is
            // the version the lens reports.
            .max_by(|(left_name, left), (right_name, right)| {
                left.published_at
                    .cmp(&right.published_at)
                    .then_with(|| left_name.cmp(right_name))
            })
            .map(|(version, published)| (version.as_str(), published))
    }

    /// This lens's LATEST published version, as `(LensVersion, that published version)`,
    /// or `None` while no version has been published from it.
    ///
    /// Distinct from [`Self::published_version`], which answers the version the lens's
    /// CURRENT content is published as (and nothing while that content is a draft): the
    /// latest version is the one published most recently, whatever the lens's draft now
    /// holds. `LensVersion` is caller-chosen (`CreateLensVersion`'s `LensVersion` is an
    /// input), so the version NAMES do not order the versions — the publication instant
    /// does, with the name breaking a tie between two publishes the clock cannot separate.
    pub fn latest_version(&self) -> Option<(&str, &PublishedLensVersion)> {
        self.versions
            .iter()
            .max_by(|(left_name, left), (right_name, right)| {
                left.published_at
                    .cmp(&right.published_at)
                    .then_with(|| left_name.cmp(right_name))
            })
            .map(|(version, published)| (version.as_str(), published))
    }

    /// The content a REVIEW of this lens asks: the content of its latest published
    /// version, and nothing while no version has been published from it.
    ///
    /// A custom lens "cannot be applied to workloads … until it's published with
    /// CreateLensVersion" (`ImportLens` doc) and a published version is immutable, so what
    /// a review of this lens asks is PUBLISHED content and never the draft an import left
    /// behind: re-importing a lens must not move the questions a review of it already asks.
    /// [`Self::latest_version`] is which published version that is, for the reason stated
    /// there.
    ///
    /// This is the content a review that pins NO version is against; a review that pins one
    /// reads [`Self::content_at`].
    pub fn reviewed_content(&self) -> Option<&LensContent> {
        self.latest_version()
            .map(|(_, published)| &published.content)
    }

    /// The content ONE version of this lens carries — the content a review PINNED at
    /// `version` asks.
    ///
    /// A lens review is a review against ONE version of the lens, and a published version
    /// is immutable, so the pillars and questions a review asks are the ones the version it
    /// is against defines: publishing a later version does not move them, which is what
    /// `UpgradeLensReview` and `UpgradeReviewTemplateLensReview` are for. A review that
    /// pins no version, and one pinned at a version this lens does not carry, is against
    /// the lens's latest ([`Self::reviewed_content`]) — the same fallback
    /// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_name_at`]
    /// reads the lens's NAME through, so a review's name and its question set always come
    /// from one version.
    pub fn content_at(&self, version: Option<&str>) -> Option<&LensContent> {
        version
            .and_then(|version| self.versions.get(version))
            .map(|published| &published.content)
            .or_else(|| self.reviewed_content())
    }

    /// Drop the content of this lens that no version has been published from, leaving it
    /// at its latest published version: the content the lens reports becomes that
    /// version's.
    ///
    /// `false` when no version has been published from this lens, so there is nothing to
    /// fall back to and no content of the lens would be left at all — the caller removes
    /// the record itself then ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remove_lens`]).
    ///
    /// The content is taken from [`Self::latest_version`] rather than left empty so that
    /// every read of the lens keeps agreeing: a lens with a published version is one
    /// `ExportLens` answers content for and `ListLenses` describes, and the version that
    /// content was published as is what [`Self::published_version`] then reports it at.
    pub fn discard_draft(&mut self) -> bool {
        let Some(published) = self
            .latest_version()
            .map(|(_, published)| published.content.clone())
        else {
            return false;
        };
        // The import that carried the discarded document is over as far as this lens is
        // concerned, so its job has no status left to report.
        self.import_completes_at = None;
        self.draft = Some(published);
        true
    }

    /// The `LensType` of this lens, as the account that reaches it sees it.
    ///
    /// A custom lens is `CUSTOM_SELF` to the account that owns it and `CUSTOM_SHARED` to
    /// an account it was shared with (`smithy_query(query="shape", shape="LensType")`);
    /// every request this emulator serves is the one emulator account's
    /// ([`::engine::emulator_account_id`]), so the owner recorded on the lens is what
    /// tells the two apart.
    pub fn lens_type(&self) -> &'static str {
        if self.owner == ::engine::emulator_account_id() {
            LENS_TYPE_CUSTOM_SELF
        } else {
            LENS_TYPE_CUSTOM_SHARED
        }
    }

    /// The `LensSummary` projection of this lens, or `None` for a record that carries no
    /// content at all — which no import leaves behind.
    pub fn to_summary(&self) -> Option<LensSummary> {
        let content = self.content()?;
        // The version the content this summary describes is published as, if it is: both
        // `LensVersion` and `LensStatus` speak about THAT content, so they cannot disagree.
        let published = self.published_version();
        Some(LensSummary {
            lens_arn: Some(self.lens_arn.clone()),
            // "For custom lenses, this is the lens ARN" (`LensAlias` doc), so the alias a
            // summary reports is the ARN every other operation resolves the lens by.
            lens_alias: Some(self.lens_arn.clone()),
            lens_name: Some(content.definition.name.clone()),
            lens_type: Some(self.lens_type().to_string()),
            description: content.definition.description.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // "when the lens was last updated": the instant the content it is described
            // by was imported.
            updated_at: Some(::aws_smithy_types::DateTime::from(content.imported_at)),
            // "The version of the lens": the version its content was published as, and
            // [`LENS_VERSION_DRAFT`] while that content is a draft.
            lens_version: Some(
                published
                    .map_or(LENS_VERSION_DRAFT, |(version, _)| version)
                    .to_string(),
            ),
            owner: Some(self.owner.clone()),
            lens_status: Some(
                published
                    .map_or(LENS_STATUS_DRAFT, |_| LENS_STATUS_PUBLISHED)
                    .to_string(),
            ),
        })
    }
}

/// The stable enumeration key of one lens in the lens catalog `ListLenses` pages over:
/// the AWS-official lenses this emulator ships first — the Well-Architected Framework
/// lens at their head — then the custom lenses, each group in its own identifier order.
///
/// The model publishes no order for `LensSummaries`, so this is the emulator's own. The
/// Framework lens leads the official group because it is the lens the service applies to
/// every workload by default, which is where a client looking for it expects to find it;
/// the rest follow in alias order. What the key has to be is TOTAL and stable — the
/// `NextToken` carries the key of the last lens a page served, so a key that moved or
/// repeated would serve a lens twice or skip it.
pub fn lens_catalog_key(lens_type: &str, identifier: &str) -> String {
    let group = u8::from(lens_type != LENS_TYPE_AWS_OFFICIAL);
    let within = u8::from(!(group == 0 && identifier == FRAMEWORK_LENS_ALIAS));
    format!("{group}/{within}/{identifier}")
}

/// What an `ImportLens` idempotency token resolves to: the lens the original request
/// imported into, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct ImportLensToken {
    pub lens_id: String,
    pub fingerprint: String,
}

/// What a `CreateLensVersion` idempotency token resolves to: the lens and the version
/// the original request published, plus that request's parameter fingerprint so a reuse
/// of the token with DIFFERENT parameters can be told from a genuine replay.
#[derive(Debug, Clone)]
pub struct CreateLensVersionToken {
    pub lens_id: String,
    pub lens_version: String,
    pub fingerprint: String,
}

/// Why a lens document was refused.
///
/// The lens JSON format specification the `ImportLens` doc points at is a SCHEMA, so a
/// document is reported the way a schema layer reports what it read: a document the service
/// cannot read as JSON at all is [`LensJsonRejection::Unparsable`] — there is nothing to
/// report about a document that was never read — and one it did read carries the FINDING its
/// content produced, in the same voice this service states a finding about the request's own
/// content in ([`service_common::validation_message`]). [`lens_json_exception`] turns either
/// into the modelled `ValidationException`.
#[derive(Debug, Clone)]
pub enum LensJsonRejection {
    /// The document is not parsable JSON, so none of the format's rules were reached.
    Unparsable,
    /// What the format's schema found in the document the service read.
    Finding(String),
}

/// The modelled `ValidationException` (HTTP 400) for a refused lens document.
///
/// Either way the request carried a document and the document is not a lens, so the request
/// never became what it had to be: the envelope is the one this service answers a request it
/// could not READ with ([`unstated_reason_exception`]) — `Reason` carried unclassified, and
/// no field finding. What they state differs with what the service knows: a document it
/// could not read at all is answered by naming the member
/// ([`unparsable_document_exception`]), and one it read states what the format's schema
/// found in it, listed the way every finding of this service is listed. Built through the
/// same constructors every other refusal in this service uses, so the envelope cannot drift.
pub fn lens_json_exception(
    rejection: &LensJsonRejection,
) -> crate::ops::types::ValidationException {
    match rejection {
        LensJsonRejection::Unparsable => unparsable_document_exception(LENS_JSON_MEMBER),
        LensJsonRejection::Finding(finding) => unstated_reason_exception(
            service_common::validation_message::bracketed_findings(std::slice::from_ref(finding)),
        ),
    }
}

/// `LensName`'s modelled length bounds (`smithy_query(query="shape", shape="Lens")`:
/// `Name` targets `LensName`, length 1..128), which the lens document's `name` becomes.
const LENS_NAME_LENGTH: (usize, usize) = (1, 128);

/// `LensDescription`'s modelled length bounds (length 1..1024), which the lens
/// document's `description` becomes.
const LENS_DESCRIPTION_LENGTH: (usize, usize) = (1, 1024);

/// The properties the lens JSON format requires of the document itself, in the order a
/// finding about the object lists them — `description` is optional, so it is not one of
/// them.
const LENS_JSON_REQUIRED_PROPERTIES: [&str; 3] = ["name", "pillars", "schemaVersion"];

/// The type an instance HAS, in the vocabulary a schema declares a type in — what a finding
/// about a type names the instance by.
fn lens_json_type(value: &::serde_json::Value) -> &'static str {
    match value {
        ::serde_json::Value::Null => "null",
        ::serde_json::Value::Bool(_) => "boolean",
        ::serde_json::Value::Number(number) => {
            if number.is_f64() {
                "number"
            } else {
                "integer"
            }
        }
        ::serde_json::Value::String(_) => "string",
        ::serde_json::Value::Array(_) => "array",
        ::serde_json::Value::Object(_) => "object",
    }
}

/// The member `member` of `object`, or the finding that reports the document does not carry
/// it: the property as the format SPELLS it, which is what the caller would have had to send.
fn lens_json_member<'doc>(
    object: &'doc ::serde_json::Value,
    member: &str,
) -> Result<&'doc ::serde_json::Value, LensJsonRejection> {
    match object.get(member) {
        Some(::serde_json::Value::Null) | None => Err(LensJsonRejection::Finding(
            service_common::validation_message::missing_required_properties(&[member]),
        )),
        Some(value) => Ok(value),
    }
}

/// The finding for an instance of another type than the format admits there.
fn lens_json_type_finding(value: &::serde_json::Value, allowed: &str) -> LensJsonRejection {
    LensJsonRejection::Finding(service_common::validation_message::instance_type_mismatch(
        lens_json_type(value),
        &[allowed],
    ))
}

/// A required string member of a lens document, within `bounds` characters.
fn lens_json_string(
    object: &::serde_json::Value,
    member: &str,
    bounds: (usize, usize),
) -> Result<String, LensJsonRejection> {
    let value = lens_json_member(object, member)?;
    let text = value
        .as_str()
        .ok_or_else(|| lens_json_type_finding(value, "string"))?;
    let (min, max) = bounds;
    // The format bounds a string in CHARACTERS, as every modelled `@length` does, and the
    // finding states the count that was checked.
    let length = text.chars().count();
    if length < min {
        return Err(LensJsonRejection::Finding(
            service_common::validation_message::string_too_short(text, length, min as i64),
        ));
    }
    if length > max {
        return Err(LensJsonRejection::Finding(
            service_common::validation_message::string_too_long(text, length, max as i64),
        ));
    }
    Ok(text.to_string())
}

/// A required, non-empty array member of a lens document.
fn lens_json_array<'doc>(
    object: &'doc ::serde_json::Value,
    member: &str,
) -> Result<&'doc Vec<::serde_json::Value>, LensJsonRejection> {
    let value = lens_json_member(object, member)?;
    let items = value
        .as_array()
        .ok_or_else(|| lens_json_type_finding(value, "array"))?;
    if items.is_empty() {
        return Err(LensJsonRejection::Finding(
            service_common::validation_message::array_too_short(items.len(), 1),
        ));
    }
    Ok(items)
}

/// Record `id` as taken within `scope`, or refuse it as a duplicate.
///
/// A schema cannot state that the ids of an array's objects differ, so this finding is the
/// lens FORMAT's own rule rather than one the schema layer words: a rule, an answer and a
/// report all name a question or a choice by its id, so two of them cannot share one.
fn lens_json_unique(
    seen: &mut std::collections::BTreeSet<String>,
    id: &str,
    scope: &str,
) -> Result<(), LensJsonRejection> {
    if !seen.insert(id.to_string()) {
        return Err(LensJsonRejection::Finding(format!(
            "array must not carry a duplicate id (\"{id}\") within one {scope}"
        )));
    }
    Ok(())
}

/// The custom lens a JSON document defines, or why it is not a lens.
///
/// The document is checked against the lens JSON format specification the `ImportLens`
/// doc points at ("Lenses are defined in JSON. For more information, see JSON format
/// specification in the Well-Architected Tool User Guide"): a `schemaVersion` of
/// [`LENS_SCHEMA_VERSION`], a `name` and optional `description` within the modelled
/// `LensName` / `LensDescription` bounds, and at least one pillar, each with at least
/// one question, each question with at least one choice and a `riskRules` list whose
/// last rule is the [`LENS_JSON_DEFAULT_CONDITION`] one and whose risks are
/// [`LENS_JSON_RISK_VALUES`]. Ids are unique within their scope, because a rule, an
/// answer and a report all name a question or a choice by its id.
///
/// The `@length(2..500000)` on `LensJSON` ("A custom lens cannot exceed 500 KB in size")
/// is a modelled constraint, so the boundary validator refuses an oversized document
/// before any of this runs.
pub fn parse_lens_json(document: &str) -> Result<LensDefinition, LensJsonRejection> {
    // A document that is not JSON is refused by naming the member alone: the decoder's own
    // diagnostic is not part of the API contract, so it is not built into a message.
    let value: ::serde_json::Value =
        ::serde_json::from_str(document).map_err(|_| LensJsonRejection::Unparsable)?;
    if !value.is_object() {
        return Err(lens_json_type_finding(&value, "object"));
    }
    // Every property the document is missing, in ONE finding: the sentence that layer
    // states is about the object and lists each property it did not find, so a caller is
    // told everything it has to add rather than one property per attempt.
    let missing: Vec<&str> = LENS_JSON_REQUIRED_PROPERTIES
        .iter()
        .copied()
        .filter(|property| {
            matches!(
                value.get(property),
                None | Some(::serde_json::Value::Null)
            )
        })
        .collect();
    if !missing.is_empty() {
        return Err(LensJsonRejection::Finding(
            service_common::validation_message::missing_required_properties(&missing),
        ));
    }

    let schema_version = lens_json_string(&value, "schemaVersion", (1, 32))?;
    if schema_version != LENS_SCHEMA_VERSION {
        return Err(LensJsonRejection::Finding(
            service_common::validation_message::enum_value_not_found(
                &schema_version,
                &[LENS_SCHEMA_VERSION],
            ),
        ));
    }
    let name = lens_json_string(&value, "name", LENS_NAME_LENGTH)?;
    let description = match value.get("description") {
        None | Some(::serde_json::Value::Null) => None,
        Some(_) => Some(lens_json_string(
            &value,
            "description",
            LENS_DESCRIPTION_LENGTH,
        )?),
    };

    let mut pillar_ids = std::collections::BTreeSet::new();
    let mut question_ids = std::collections::BTreeSet::new();
    let mut pillars = Vec::new();
    for pillar_value in lens_json_array(&value, "pillars")? {
        let pillar_id = lens_json_string(pillar_value, "id", (1, 128))?;
        lens_json_unique(&mut pillar_ids, &pillar_id, "lens")?;
        let pillar_name = lens_json_string(pillar_value, "name", LENS_NAME_LENGTH)?;

        let mut questions = Vec::new();
        for question_value in lens_json_array(pillar_value, "questions")? {
            let question_id = lens_json_string(question_value, "id", (1, 128))?;
            lens_json_unique(&mut question_ids, &question_id, "lens")?;
            let title = lens_json_string(question_value, "title", (1, 1024))?;

            let mut choice_ids = std::collections::BTreeSet::new();
            let mut choices = Vec::new();
            for choice_value in lens_json_array(question_value, "choices")? {
                let choice_id = lens_json_string(choice_value, "id", (1, 128))?;
                lens_json_unique(&mut choice_ids, &choice_id, "question")?;
                choices.push(LensChoice {
                    id: choice_id,
                    title: lens_json_string(choice_value, "title", (1, 1024))?,
                    // The lens JSON format specification carries no per-choice prose, so
                    // an imported lens publishes none.
                    description: None,
                });
            }

            let rule_values = lens_json_array(question_value, "riskRules")?;
            let mut risk_rules = Vec::new();
            for (rule_index, rule_value) in rule_values.iter().enumerate() {
                let condition = lens_json_string(rule_value, "condition", (1, 1024))?;
                let risk = lens_json_string(rule_value, "risk", (1, 32))?;
                if !LENS_JSON_RISK_VALUES.contains(&risk.as_str()) {
                    return Err(LensJsonRejection::Finding(
                        service_common::validation_message::enum_value_not_found(
                            &risk,
                            LENS_JSON_RISK_VALUES,
                        ),
                    ));
                }
                // The last rule is the one that applies when no other matched, so it
                // carries the default condition and no other rule may. A schema cannot
                // state that, so this finding is the lens FORMAT's own rule.
                let is_last = rule_index + 1 == rule_values.len();
                let is_default = condition == LENS_JSON_DEFAULT_CONDITION;
                if is_last != is_default {
                    return Err(LensJsonRejection::Finding(format!(
                        "array must carry the \"{LENS_JSON_DEFAULT_CONDITION}\" condition on \
                         its last risk rule and on no other"
                    )));
                }
                risk_rules.push(LensRiskRule { condition, risk });
            }

            questions.push(LensQuestion {
                id: question_id,
                title,
                choices,
                risk_rules,
            });
        }

        pillars.push(LensPillar {
            id: pillar_id,
            name: pillar_name,
            questions,
        });
    }

    Ok(LensDefinition {
        schema_version,
        name,
        description,
        pillars,
    })
}

/// The lens reviews a resource applying `lenses` has: every applied lens's [`lens_alias`]
/// — the identity the lens is ONE lens under, so a lens applied under two spellings has
/// exactly one review — beside the name the apply recorded it under, which is how a
/// `LensAlias` of a custom lens is its ARN.
///
/// The order is the resource's OWN lens order: the order its `Lenses` list records, which
/// is the order the lenses were applied to it ([`distinct_lenses`] keeps a lens where the
/// list first named it) — so a resource reports the reviews of its lenses in the order it
/// reports the lenses themselves, the default lens its creation applied first. A page over
/// these reviews follows that order, and a cursor carrying the last alias served resumes
/// just past that alias in it.
///
/// One place, because a WORKLOAD's lens reviews ([`WorkloadRecord::lens_reviews`]) and a
/// REVIEW TEMPLATE's ([`ReviewTemplateRecord::lens_reviews`]) are both the reviews of the
/// lenses their resource applies, and the two cannot disagree about which lens is one
/// review or about the order they come in.
pub fn lens_reviews_of(lenses: &[String]) -> Vec<(&str, &str)> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut reviews: Vec<(&str, &str)> = Vec::with_capacity(lenses.len());
    for lens in lenses {
        let alias = lens_alias(lens);
        if seen.insert(alias) {
            reviews.push((alias, lens.as_str()));
        }
    }
    reviews
}

/// The lenses a list names, each kept ONCE and in the order the list first named it, AS
/// SENT.
///
/// A lens is one lens however it is spelled — [`lens_alias`] resolves a bare alias, an
/// official lens ARN and a version-pinned ARN to the same lens — so a list naming one lens
/// twice names ONE lens, asks for ONE lens review ([`lens_reviews_of`]) and counts ONCE
/// against a lens quota. The name as sent is what is kept, because that is the `LensAlias`
/// every read of the resource reports.
///
/// One place, because every operation that applies lenses to a resource — a create that
/// mints it with lenses and an association that adds lenses to the stored one — stores the
/// list this returns, and the lens list a resource reports and the lens reviews it holds
/// cannot disagree about which lens is one lens. Linear in the list it is given.
pub fn distinct_lenses(lenses: &[String]) -> Vec<String> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut distinct: Vec<String> = Vec::new();
    for lens in lenses {
        if seen.insert(lens_alias(lens)) {
            distinct.push(lens.clone());
        }
    }
    distinct
}

/// The lens a `LensAlias` names, as its bare alias.
///
/// "For Amazon Web Services official lenses, this is either the lens alias, such as
/// `serverless`, or the lens ARN, such as
/// `arn:aws:wellarchitected:us-east-1::lens/serverless`" (`LensAlias` doc), so an ARN's
/// trailing `lens/<alias>` resolves to the same lens as the bare alias. A lens ARN may
/// also pin a version after the alias, which names the same lens.
pub fn lens_alias(lens: &str) -> &str {
    let alias = lens.rsplit_once("lens/").map_or(lens, |(_, alias)| alias);
    alias.split('/').next().unwrap_or(alias)
}

/// The lenses a list names, each as the LENS it names rather than as the spelling it was
/// named by, in the list's own order.
///
/// This is the form a lens list takes inside a [`parameter_fingerprint`]: "if you retry a
/// request with the same client request token and the same parameters after the original
/// request has completed successfully, the result of the original request is returned"
/// (`ClientRequestToken` doc), and the parameter a lens list carries is the set of lenses
/// the request applies — resolved through [`lens_alias`], the identity every read of those
/// lenses is resolved through — so a retry that spells one of them the other way carries
/// the SAME parameters and replays. Order is kept, because the order a request names its
/// lenses in is the order the resource stores and reports them in, so a reordered list asks
/// for a different resource.
///
/// One place, because every `@idempotencyToken` operation that applies a list of lenses
/// fingerprints that list, and two of them cannot disagree about which lens is one lens.
pub fn lens_identities(lenses: &[String]) -> Vec<&str> {
    lenses.iter().map(|lens| lens_alias(lens)).collect()
}

/// Whether `alias` names `record`: an ARN must agree with the lens's own ARN (so the
/// account and Region it carries have to match), optionally pinning a version after it.
///
/// A name that is NOT an ARN never names a stored lens, whatever id it happens to spell:
/// "For custom lenses, this is the lens ARN" (`LensAlias` doc) is the only spelling a
/// custom lens answers to, so a bare name is an AWS-official lens's alias
/// ([`is_aws_official_lens`]) — the same classification every operation that refuses a
/// bare name as official applies before it reaches this store.
fn alias_names_lens(alias: &str, record: &LensRecord) -> bool {
    if !alias.starts_with("arn:") {
        return false;
    }
    alias == record.lens_arn
        || alias
            .strip_prefix(record.lens_arn.as_str())
            .is_some_and(|version| version.starts_with('/'))
}

/// Whether a `LensAlias` names an AWS-official lens rather than a custom one.
///
/// "For Amazon Web Services official lenses, this is either the lens alias, such as
/// `serverless`, or the lens ARN, such as `arn:aws:wellarchitected:us-east-1::lens/
/// serverless`. … For custom lenses, this is the lens ARN, such as
/// `arn:aws:wellarchitected:us-west-2:123456789012:lens/0123…`" (`LensAlias` doc): an
/// official lens ARN names NO owner ACCOUNT, because AWS publishes it rather than an
/// account owning it, while a custom lens ARN carries its owner's account id. So the
/// account position of an official lens ARN is either empty (the spelling the doc shows)
/// or the publisher ([`AWS_OFFICIAL_LENS_OWNER_TOKEN`], the spelling the service reports —
/// [`official_lens_arn`]), and both name the same lens. A bare alias is official too — a
/// custom lens is only ever named by its ARN.
pub fn is_aws_official_lens(alias: &str) -> bool {
    let Some(arn) = alias.strip_prefix("arn:") else {
        return true;
    };
    // partition : service : region : account : resource — read by lookup rather than by
    // index, so a value with fewer fields cannot reach one that is not there.
    match arn.split(':').nth(3) {
        None => true,
        Some(account) => account.is_empty() || account == AWS_OFFICIAL_LENS_OWNER_TOKEN,
    }
}

/// The aliases Amazon Web Services publishes its own lenses under — the AWS-OFFICIAL
/// half of the catalog a `LensAlias` resolves against.
///
/// SOURCE: the Lens Catalog of the AWS Well-Architected Tool user guide
/// (<https://docs.aws.amazon.com/wellarchitected/latest/userguide/lenses.html>), whose
/// lenses are the ones AWS's own `ListLenses` reports under `LensType$AWS_OFFICIAL`. The
/// Smithy model cannot supply the set: it bounds a `LensAlias` only by length and carries
/// `serverless` as an example of one (`smithy_query(query="shape", shape="LensAlias")`),
/// so the catalog is written out here for the same reason [`AWS_REGIONS`] is — a value
/// outside the published set names no lens at all, and an operation that APPLIES a lens
/// refuses it rather than recording it.
///
/// This emulator ships the QUESTION CONTENT of the lenses in
/// [`official_lenses::OFFICIAL_LENS_CATALOG`], whose entries are drawn from the aliases
/// below, and `ListLenses` reports a `LensSummary` for each of them: a `LensSummary` is
/// lens CONTENT — its `LensName`, `Description`, `LensVersion` and publication instants —
/// so a published alias the content catalog does not carry is a lens this emulator knows
/// exists and holds no questions for ([`lens_questions`]), which is applicable but not
/// listed rather than listed with content this emulator invented. Applying one is a
/// different question: a lens is applied BY NAME, and the name is all this catalog has to
/// resolve.
pub const AWS_OFFICIAL_LENS_ALIASES: &[&str] = &[
    "connectedmobility",
    "dataanalytics",
    "devops",
    "financialservices",
    "games",
    "genai",
    "healthcareindustry",
    "iot",
    "machinelearning",
    "mergersandacquisitions",
    "migration",
    "sap",
    "serverless",
    "softwareasaservice",
    FRAMEWORK_LENS_ALIAS,
];

/// Whether a `LensAlias` names a lens Amazon Web Services publishes: a bare alias of
/// [`AWS_OFFICIAL_LENS_ALIASES`], or an official lens ARN whose resource part ends in
/// `lens/<one of those aliases>` ([`lens_alias`], [`is_aws_official_lens`]).
pub fn is_published_official_lens(lens: &str) -> bool {
    is_aws_official_lens(lens) && AWS_OFFICIAL_LENS_ALIASES.contains(&lens_alias(lens))
}

/// What the lens a `LensAlias` names is, as of the instant it is resolved — the answer the
/// operations that APPLY a lens act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensResolution {
    /// A lens Amazon Web Services publishes ([`AWS_OFFICIAL_LENS_ALIASES`]). AWS's own
    /// lenses are published content, so one is applicable as it stands.
    Official,
    /// A stored custom lens whose current content a version has been published from
    /// ([`LensRecord::published_version`]) — applicable.
    PublishedCustom,
    /// A stored custom lens whose current content no version has been published from: the
    /// lens exists, and "cannot be applied to workloads … until it's published with
    /// CreateLensVersion" (`ImportLens` doc).
    DraftCustom,
    /// No lens: neither an alias of the official catalog nor a custom lens this store
    /// holds under the ARN the request named.
    Unknown,
}

/// How a `Lenses` list that names a lens which cannot be applied is refused: the modelled
/// exception for the case, for the calling operation to lift into its own error enum.
#[derive(Debug, Clone)]
pub enum LensesRefusal {
    /// An entry the caller cannot reach a lens through: one that names no lens at all, and
    /// one that names a custom lens no version has been published from — the two are
    /// refused identically, because the service does not tell a caller which of them it
    /// is. The exception carries the entry AS SENT as its `ResourceId`, which is what the
    /// refusal the calling operation words names.
    NotFound(ResourceNotFoundException),
}

/// The member a lens-list refusal of an ASSOCIATION names
/// (`AssociateLensesInput$LensAliases` / `DisassociateLensesInput$LensAliases`). A refusal
/// names the member the request carried the lenses in, so the two lens-list members are
/// named apart.
pub const LENS_ALIASES_MEMBER: &str = "LensAliases";

/// The ARN the service reports an AWS-OFFICIAL lens by:
/// `arn:aws:wellarchitected::aws:lens/<alias>`.
///
/// A lens Amazon Web Services publishes is PARTITION-GLOBAL and owned by no customer
/// account: the one lens is the same lens in every Region, so its ARN carries no Region
/// segment, and the account position carries the literal `aws` — the publisher — rather
/// than an account id. The Region a request is signed for therefore does not reach this
/// ARN; a Region-qualified spelling of it is still accepted on input, because a lens is
/// named by "either the lens alias … or the lens ARN" (`LensAlias` doc) and [`lens_alias`]
/// reduces either to the alias.
///
/// Built from [`::engine::emulator_arn`], so the partition and the namespace still come
/// from the one place, with the Region left empty (the form that crate documents for a
/// resource that is not Region-scoped) and the account placeholder replaced by the
/// publisher. A CUSTOM lens's ARN is not built here: it IS account- and Region-scoped,
/// and the lens that import minted owns it ([`LensRecord::lens_arn`]).
pub fn official_lens_arn(lens: &str) -> String {
    let arn = ::engine::emulator_arn(ARN_NAMESPACE, "", &format!("lens/{}", lens_alias(lens)));
    arn.replacen(
        &format!(":{}:", ::engine::emulator_account_id()),
        &format!(":{AWS_OFFICIAL_LENS_OWNER_TOKEN}:"),
        1,
    )
}

/// What stands in the ACCOUNT position of an AWS-official lens's ARN, and what
/// `LensSummary$Owner` reports for such a lens: the publisher of the lens rather than an
/// account that owns it.
///
/// AWS publishes its own lenses, so no customer account owns one — which is why an
/// official lens ARN is `arn:aws:wellarchitected::aws:lens/<alias>` and why `Owner`,
/// modelled an `AwsAccountId` for the CUSTOM lenses an account really does own, reports
/// the publisher here instead of a 12-digit id that would name an account that does not
/// own the lens.
pub const AWS_OFFICIAL_LENS_OWNER_TOKEN: &str = "aws";

/// `LensSummary$Owner` of a lens Amazon Web Services publishes: the publisher, as the
/// service reports it — the same owner the lens's ARN names
/// ([`AWS_OFFICIAL_LENS_OWNER_TOKEN`]), spelled the way the summary reports it.
pub const AWS_OFFICIAL_LENS_OWNER: &str = "AWS";

/// The pillar priority order a workload is created with when the request names none:
/// the framework's own pillar order, which is what orders the items in a review's
/// improvement plan (`WorkloadPillarPriorities` doc: "The priorities of the pillars,
/// which are used to order items in the improvement plan") — the order
/// [`order_by_pillar_priority`] puts that plan's questions in.
pub fn default_pillar_priorities() -> Vec<String> {
    WELL_ARCHITECTED_PILLARS
        .iter()
        .map(|(pillar_id, _)| (*pillar_id).to_string())
        .collect()
}

/// The pillar order a request leaves a workload reviewed in: the priorities the request
/// names, or — when it names NONE, whether by omitting the member or by carrying an empty
/// list — the order the workload is already reviewed in, falling back to the framework's
/// own pillar order when it carries none yet.
///
/// A workload is always reviewed in some pillar order ("The priorities of the pillars,
/// which are used to order items in the improvement plan", `WorkloadPillarPriorities`
/// doc), and `WorkloadPillarPriorities` publishes no `@length`, so the boundary validator
/// passes an empty list through to the handler. The rule therefore lives HERE, in the one
/// place every path that writes the member resolves it through — a create against no
/// current order, an update against the order the stored workload carries — so no path
/// can leave a workload reviewed in no order at all.
pub fn resolved_pillar_priorities(
    requested: Option<Vec<String>>,
    current: &[String],
) -> Vec<String> {
    match requested {
        Some(priorities) if !priorities.is_empty() => priorities,
        _ if !current.is_empty() => current.to_vec(),
        _ => default_pillar_priorities(),
    }
}

/// The modelled input member a pillar-priority finding names
/// (`CreateWorkloadInput$PillarPriorities`, `UpdateWorkloadInput$PillarPriorities`), as
/// `ValidationExceptionField$Name` reports it.
pub const PILLAR_PRIORITIES_MEMBER: &str = "PillarPriorities";

/// The refusal for a pillar priority order that names a pillar the workload has none of,
/// or `None` when every entry of it resolves to a pillar.
///
/// "Each pillar is represented by its `PillarReviewSummary$PillarId`"
/// (`WorkloadPillarPriorities` doc) and "a pillar is identified by its
/// `PillarReviewSummary$PillarId`" (`PillarId` doc): an entry of the order a workload is
/// reviewed in is a pillar IDENTIFIER, so it has to name a pillar that exists — one of the
/// framework's own pillars ([`WELL_ARCHITECTED_PILLARS`], the order a workload that names
/// none is reviewed in), or a pillar a lens the workload reviews defines, which is what
/// that lens review reports a `PillarReviewSummary` for. An identifier outside both names
/// no pillar of the workload and is refused rather than stored, exactly as
/// [`undefined_pillar_refusal`] refuses one that names no pillar of the lens under review.
///
/// Unlike that sibling, this rule needs no carve-out for a lens this emulator holds no
/// content for: every lens AWS publishes is organized into the framework's own pillars
/// ([`WELL_ARCHITECTED_PILLARS`]), which the judged set always carries, and a CUSTOM lens
/// whose document names pillars of its own cannot be reviewed by a workload until a version
/// of it has been published ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::unapplicable_lenses_refusal`]) —
/// at which point its content IS held. So a reviewed lens with no content here defines no
/// pillar the set is missing, and nothing legitimate is refused for want of its content.
///
/// The offending ids are sorted and deduplicated, so the refusal does not depend on the
/// order the request listed them in. Shared by both operations that write the member, so
/// they cannot word it differently or hold the member to different sets.
pub fn undefined_pillar_priority_refusal(
    priorities: &[String],
    reviewed: &ReviewedLensContent,
) -> Option<crate::ops::types::ValidationException> {
    let mut defined: BTreeSet<&str> = WELL_ARCHITECTED_PILLARS
        .iter()
        .map(|(pillar_id, _)| *pillar_id)
        .collect();
    defined.extend(reviewed.pillar_ids());
    let mut undefined: Vec<&str> = priorities
        .iter()
        .map(String::as_str)
        .filter(|pillar_id| !defined.contains(pillar_id))
        .collect();
    if undefined.is_empty() {
        return None;
    }
    undefined.sort_unstable();
    undefined.dedup();
    Some(member_constraint_exception(
        PILLAR_PRIORITIES_MEMBER,
        &format!(
            "every pillar must be a pillar the workload is reviewed in, which {} is not",
            undefined.join(", ")
        ),
    ))
}

/// What a `CreateWorkload` idempotency token resolves to: the workload the original
/// request created, plus a fingerprint of that request's parameters so a reuse of the
/// token with DIFFERENT parameters can be told apart from a genuine replay.
#[derive(Debug, Clone)]
pub struct CreateWorkloadToken {
    pub workload_id: String,
    pub fingerprint: String,
}

/// What a `CreateMilestone` idempotency token resolves to: the milestone the original
/// request recorded, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateMilestoneToken {
    pub workload_id: String,
    pub milestone_number: i32,
    pub fingerprint: String,
}

/// What a `CreateProfile` idempotency token resolves to: the profile the original request
/// created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateProfileToken {
    pub profile_arn: String,
    pub fingerprint: String,
}

/// What an `UpgradeProfileVersion` idempotency token resolves to: the profile whose
/// association the original request upgraded, plus that request's parameter fingerprint.
///
/// The operation answers an EMPTY body, so a replay has no value to return — what the
/// token is for here is keeping the replay from acting a second time, which matters
/// because the upgrade can also record a milestone and a second recording of the same
/// milestone name would collide ([`milestone_name_conflict`]). The profile ARN is kept so
/// a token reused with different parameters can name the resource it already upgraded, and
/// the workload id so the token goes with the workload whose association it upgraded
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remove_workload`]).
#[derive(Debug, Clone)]
pub struct UpgradeProfileVersionToken {
    pub workload_id: String,
    pub profile_arn: String,
    pub fingerprint: String,
}

/// What an `UpdateIntegration` idempotency token resolves to: the workload the original
/// request synced, plus that request's parameter fingerprint.
///
/// The operation answers an EMPTY body, so a replay has no value to return — what the
/// token is for here is keeping the replay from syncing a second time, which matters
/// because a sync records the instant it was served at against every question it covers,
/// so acting twice would move an instant the original request already reported. The
/// workload id is kept so a token reused with different parameters can name the resource
/// it already synced, and so the token goes with the workload it synced
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remove_workload`]).
#[derive(Debug, Clone)]
pub struct UpdateIntegrationToken {
    pub workload_id: String,
    pub fingerprint: String,
}

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for an
/// AGENT profile — the resource `CreateAgentProfile` mints and every
/// `/api/v1/agent-profiles/{profileArn}` route resolves.
///
/// Deliberately NOT [`PROFILE_RESOURCE_TYPE`]: the Well-Architected Tool profile of
/// `CreateProfile` is a different resource, named by a different ARN (`ProfileArn`'s
/// `profile/…` against `AgentProfileArn`'s `agent-profile/…` —
/// `smithy_query(query="shape", shape="AgentProfileArn")`), so a failure about one must
/// not claim to be about the other.
pub const AGENT_PROFILE_RESOURCE_TYPE: &str = "AGENT_PROFILE";

/// `ResourceNotFoundException$ResourceType` for an optimization goal of an agent
/// profile. `ExceptionResourceType` is a free-form string
/// (`smithy_query(query="shape", shape="ExceptionResourceType")`), so it names the kind
/// of resource that was missing: a goal.
pub const AGENT_GOAL_RESOURCE_TYPE: &str = "GOAL";

/// `ResourceNotFoundException$ResourceType` / `ConflictException$ResourceType` for a
/// context of an agent profile — "application and environment information used during
/// recommendation generation" (`CreateAgentContext` doc). `ExceptionResourceType` is a
/// free-form string, so it names the kind of resource the failure is about: a context.
pub const AGENT_CONTEXT_RESOURCE_TYPE: &str = "CONTEXT";

/// The most agent profiles this emulator holds.
///
/// `CreateAgentProfile` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") and the real quota is adjustable through Service
/// Quotas, so the number here is this emulator's own ceiling rather than a value read
/// off a doc page — it is what bounds [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::agent_profiles`]
/// (and, with it, the create ledger that is 1:1 with a live profile), and it sits far
/// above any realistic client workflow.
pub const MAX_AGENT_PROFILES: usize = 1_000;

/// The most optimization goals one agent profile holds here. `CreateAgentGoal` models
/// `ServiceQuotaExceededException` for the case; the ceiling is this emulator's own, for
/// the reasons [`MAX_AGENT_PROFILES`] gives, and it bounds the goal map per profile.
pub const MAX_GOALS_PER_AGENT_PROFILE: usize = 100;

/// The most contexts one agent profile holds here. `CreateAgentContext` models
/// `ServiceQuotaExceededException` for the case; the ceiling is this emulator's own, for
/// the reasons [`MAX_AGENT_PROFILES`] gives, and it bounds the context map per profile.
pub const MAX_CONTEXTS_PER_AGENT_PROFILE: usize = 100;

/// The largest page the agent-profile list operations serve, and the ceiling their
/// `maxResults` is held to.
///
/// Unlike the workload-era lists ([`MAX_RESULTS_CEILING`]), every agent list member is
/// modelled `@range(min: 1, max: 1000)` with `@default(100)` —
/// `smithy_query(query="operation", operation="ListAgentContexts")` shows
/// `maxResults` that way — so the agent lists get their own ceiling rather than the
/// workload one.
pub const MAX_AGENT_LIST_RESULTS: i32 = 1_000;

/// The page size an agent list request asks for: the modelled
/// `@range(min: 1, max: 1000)` restated in the handler, so the page size is in
/// `1..=MAX_AGENT_LIST_RESULTS` whatever reaches it. The member carries
/// `@default(100)`, so a request that omits it arrives here already carrying 100.
pub fn agent_list_page_size(max_results: i32) -> usize {
    let clamped = max_results.clamp(1, MAX_AGENT_LIST_RESULTS);
    usize::try_from(clamped).expect(
        "maxResults is clamped into 1..=MAX_AGENT_LIST_RESULTS, which always fits in a usize",
    )
}

/// The modelled input member a pillar-set finding names
/// (`CreateAgentProfileRequest$pillars`, `CreateAgentGoalRequest$pillars`), as
/// `ValidationExceptionField$Name` reports it.
pub const PILLARS_MEMBER: &str = "pillars";

/// The refusal for a `Pillars` list that names a value the `Pillar` enum does not
/// publish, or `None` when every entry is published.
///
/// `Pillar` is an enum, so the boundary walk in `crate::service` refuses such a value with
/// this same 400 before a handler runs; every operation that STORES a pillar list keeps
/// the check as defence in depth for a caller that builds the input directly, so the
/// sentence they all refuse it with is built here rather than once per operation.
pub fn unpublished_pillar_exception(
    pillars: &[String],
) -> Option<crate::ops::types::ValidationException> {
    let published = crate::ops::types::PILLAR_VALUES;
    let unknown = pillars
        .iter()
        .find(|pillar| !published.contains(&pillar.as_str()))?;
    Some(member_constraint_exception(
        PILLARS_MEMBER,
        &format!(
            "Member must satisfy enum value set: [{}], got {unknown}",
            published.join(", ")
        ),
    ))
}

/// The modelled input member a context-type finding names
/// (`CreateAgentContextRequest$contextType`), as `ValidationExceptionField$Name` reports
/// it.
pub const CONTEXT_TYPE_MEMBER: &str = "contextType";

/// The modelled input member a finding about the agent profile a request names names
/// (`…Request$profileArn`), as `ValidationExceptionField$Name` reports it.
pub const PROFILE_ARN_MEMBER: &str = "profileArn";

/// The modelled input member a finding about one recommendation type names
/// (`ListAgentRecommendationGenerationsRequest$recommendationType`, the optional filter,
/// which travels as the `RecommendationType` query parameter). Named apart from the
/// `types` LIST member a start request carries, because a field finding must name the
/// member the request actually carried.
pub const RECOMMENDATION_TYPE_MEMBER: &str = "recommendationType";

/// The member an application-type finding of a context's content names
/// (`ContextContent$applicationType`, reached through `…Request$content`), as the path
/// `ValidationExceptionField$Name` reports for a member inside a structure.
pub const CONTENT_APPLICATION_TYPE_MEMBER: &str = "content.applicationType";

/// The member a criticality finding of a context's content names
/// (`ContextContent$criticality`). See [`CONTENT_APPLICATION_TYPE_MEMBER`].
pub const CONTENT_CRITICALITY_MEMBER: &str = "content.criticality";

/// The refusal for a context whose enum-typed members carry a value their enum does not
/// publish, or `None` when every one of them does.
///
/// Defence in depth for a caller that builds the input directly: `ContextType`,
/// `ApplicationType` and `Criticality` are enums, so the boundary walk in
/// `crate::service` already refuses a value outside their published sets with this same
/// 400 before a handler runs. It is kept so a context is never STORED naming a value the
/// enums do not publish, and shared by every operation that writes one so they cannot
/// word the refusal differently.
pub fn unknown_context_enum_exception(
    context_type: Option<&str>,
    content: &ContextContent,
) -> Option<crate::ops::types::ValidationException> {
    let candidates: [(&str, Option<&str>, &[&str]); 3] = [
        (CONTEXT_TYPE_MEMBER, context_type, CONTEXT_TYPE_VALUES),
        (
            CONTENT_APPLICATION_TYPE_MEMBER,
            content.application_type.as_deref(),
            APPLICATION_TYPE_VALUES,
        ),
        (
            CONTENT_CRITICALITY_MEMBER,
            content.criticality.as_deref(),
            CRITICALITY_VALUES,
        ),
    ];
    let findings: Vec<(String, String)> = candidates
        .iter()
        .filter_map(|(member, value, values)| {
            let value = (*value)?;
            (!values.contains(&value)).then(|| {
                (
                    (*member).to_string(),
                    constraint_sentence(
                        member,
                        &format!(
                            "Member must satisfy enum value set: [{}], got {value}",
                            values.join(", ")
                        ),
                    ),
                )
            })
        })
        .collect();
    (!findings.is_empty()).then(|| field_validation_exception(&findings))
}

/// The modelled input member an agent profile's generation-eligibility finding names
/// (`CreateAgentProfileRequest$aggregationConfiguration`), as
/// `CreateAgentProfileResponse$fieldErrors` keys it — "a map of field paths to error
/// messages for invalid or missing input fields".
pub const AGGREGATION_CONFIGURATION_MEMBER: &str = "aggregationConfiguration";

/// What `fieldErrors` says about a profile that names no scope to analyse, and so is
/// not eligible for recommendation generation (see
/// [`AgentProfileRecord::eligible_for_generation`]).
pub const NO_AGGREGATION_CONFIGURATION_MESSAGE: &str =
    "At least one aggregation configuration is required to generate recommendations.";

/// `ResourceNotFoundException$ResourceType` for an optimization recommendation — the
/// resource a completed generation produces, `ListAgentRecommendations` reports and
/// `UpdateAgentRecommendationStatus` moves the status of. `ExceptionResourceType` is a
/// free-form string, so it names the kind of resource that was missing: a recommendation.
pub const AGENT_RECOMMENDATION_RESOURCE_TYPE: &str = "RECOMMENDATION";

/// `RecommendationStatus$ACTIVE`: the status a recommendation a generation has just
/// produced carries — nobody has acted on it yet (`smithy_query(query="shape",
/// shape="RecommendationStatus")`).
pub const RECOMMENDATION_ACTIVE: &str = "ACTIVE";

/// `RecommendationStatus$COMPLETED`: the caller has implemented the recommendation.
pub const RECOMMENDATION_COMPLETED: &str = "COMPLETED";

/// `RecommendationStatus$SUPPRESSED`: the caller has decided not to act on the
/// recommendation.
pub const RECOMMENDATION_SUPPRESSED: &str = "SUPPRESSED";

/// `RecommendationState$OPEN`: a recommendation that is still outstanding work.
pub const RECOMMENDATION_STATE_OPEN: &str = "OPEN";

/// `RecommendationState$CLOSED`: a recommendation that is no longer outstanding work.
pub const RECOMMENDATION_STATE_CLOSED: &str = "CLOSED";

/// One optimization recommendation: the resource a COMPLETED recommendation generation
/// produced, `ListAgentRecommendations` reports and `UpdateAgentRecommendationStatus`
/// moves "through the implementation lifecycle", carrying the feedback
/// `PutAgentRecommendationFeedback` has submitted on it.
///
/// The record holds what the SERVICE owns — the ARN it minted, the generation the
/// recommendation came out of, the finding's content and the identity and instant it is
/// attributed to — plus the status a caller has since moved it to. `state` is DERIVED from
/// that status ([`Self::state`]) rather than stored beside it, so the two cannot disagree,
/// and the finding's text is a reference INTO
/// [`recommendation_catalog::RECOMMENDATION_CATALOG`] rather than a copy of it, so every
/// reading of one recommendation reports the same words.
#[derive(Debug, Clone)]
pub struct AgentRecommendationRecord {
    /// The minted `RecommendationArn` this recommendation is named by, everywhere.
    pub arn: String,
    /// The agent profile the recommendation belongs to: the profile whose generation
    /// produced it, and the only profile that lists it.
    pub profile_arn: String,
    /// `generationId` — "the identifier of the generation process that produced this
    /// recommendation".
    pub generation_id: String,
    /// The finding this recommendation reports, by reference into the catalog. Its
    /// `pillar` is the pillar of the generation's scope this recommendation came from.
    pub content: &'static RecommendationContent,
    /// `type` — the `RecommendationType` the generation was started for
    /// (`StartAgentRecommendationGenerationRequest$types`, modelled `@length(1, 1)`), which
    /// is what kind of recommendation it asked for.
    pub recommendation_type: String,
    /// `status` — a `RecommendationStatus`: [`RECOMMENDATION_ACTIVE`] as produced, then
    /// whatever `UpdateAgentRecommendationStatus` last moved it to.
    pub status: String,
    /// `updateReason` — "the free-text reason associated with the recommendation's most
    /// recent status update", so it is whatever the last update carried and absent when
    /// that update carried none.
    pub update_reason: Option<String>,
    pub created_by: String,
    pub created_at: SystemTime,
    pub last_modified_by: Option<String>,
    pub last_modified_at: Option<SystemTime>,
    /// The feedback a caller has submitted on this recommendation
    /// ([`AgentRecommendationFeedback`]), or nothing while none has been submitted.
    pub feedback: Option<AgentRecommendationFeedback>,
}

/// The feedback submitted on one recommendation: what `PutAgentRecommendationFeedback`
/// "submits … to help improve future optimization suggestions and track implementation
/// outcomes".
///
/// A SUB-RESOURCE of the recommendation it judges — the operation addresses it as
/// `…/{recommendationArn}/feedback` — so it is held on
/// [`AgentRecommendationRecord::feedback`] and can neither be reached without the
/// recommendation nor outlive it. The operation is `@idempotent` and puts the whole
/// sub-resource, so a recommendation holds the feedback last submitted for it rather than
/// a log of submissions: resubmitting the same feedback leaves it where it already was,
/// and submitting different feedback replaces what stood, including by carrying none of
/// an optional member the previous submission carried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecommendationFeedback {
    /// `type` — "the type of feedback being provided", a `RecommendationFeedbackType`
    /// (`@required`, so every submission carries one).
    pub feedback_type: String,
    /// `feedbackCategory` — "optional category classifying the nature of the feedback", a
    /// `FeedbackCategory`; absent when the submission classified none.
    pub category: Option<String>,
    /// `comments` — "optional comments providing additional context about the feedback";
    /// absent when the submission carried none.
    pub comments: Option<String>,
    /// Who the submission is attributed to: every request this emulator serves is its one
    /// account's, exactly as the recommendation's own `createdBy` is.
    pub submitted_by: String,
    /// The instant the submission was recorded.
    pub submitted_at: SystemTime,
}

impl AgentRecommendationRecord {
    /// `state` — the `RecommendationState` this recommendation is in, DERIVED from its
    /// status.
    ///
    /// The status tracks the recommendation "through the implementation lifecycle"
    /// (`UpdateAgentRecommendationStatus` doc) and the state is the OPEN/CLOSED reading of
    /// that lifecycle a listing filters on ("return only recommendations with the specified
    /// state (OPEN or CLOSED)" — `ListAgentRecommendationsRequest$state`): a recommendation
    /// still `ACTIVE` is outstanding work, and one the caller has implemented
    /// (`COMPLETED`) or decided against (`SUPPRESSED`) is not. Derived rather than stored,
    /// so no update can leave the two disagreeing.
    pub fn state(&self) -> &'static str {
        match self.status.as_str() {
            RECOMMENDATION_COMPLETED | RECOMMENDATION_SUPPRESSED => RECOMMENDATION_STATE_CLOSED,
            // [`RECOMMENDATION_ACTIVE`], and — defensively — any status only a later
            // revision of the enum could publish: a recommendation that has neither been
            // implemented nor set aside is outstanding work.
            _ => RECOMMENDATION_STATE_OPEN,
        }
    }

    /// The `AgentRecommendationSummary` projection a listing reports this recommendation as
    /// (`ListAgentRecommendationsResponse$items`), so one place decides how a stored
    /// recommendation is summarised.
    ///
    /// `numberOfResources`, `businessUnits` and `applications` are left absent: they
    /// describe the customer's own resources, business units and applications, and this
    /// emulator analyses none — all three are modelled optional, so reporting nothing is
    /// what it has to say about them.
    pub fn to_summary(&self) -> AgentRecommendationSummary {
        AgentRecommendationSummary {
            recommendation_arn: self.arn.clone(),
            profile_arn: self.profile_arn.clone(),
            generation_id: Some(self.generation_id.clone()),
            title: self.content.title.to_string(),
            description: self.content.description.to_string(),
            r#type: self.recommendation_type.clone(),
            pillar: self.content.pillar.to_string(),
            priority: self.content.priority.to_string(),
            effort: self.content.effort.to_string(),
            status: self.status.clone(),
            state: self.state().to_string(),
            update_reason: self.update_reason.clone(),
            impact: self.content.impact.to_string(),
            roi: self.content.roi(),
            number_of_resources: None,
            aws_services: Some(self.content.aws_service_list()),
            business_units: None,
            applications: None,
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: self.last_modified_by.clone(),
            last_modified_at: self
                .last_modified_at
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// What a `CreateAgentProfile` idempotency token resolves to: the profile the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateAgentProfileToken {
    pub profile_arn: String,
    pub fingerprint: String,
}

/// What an `UpdateAgentProfile` idempotency token resolves to: the profile the original
/// request updated, plus that request's parameter fingerprint, so a retry carrying the
/// token can be told from a later request that happens to reuse it for other values.
#[derive(Debug, Clone)]
pub struct UpdateAgentProfileToken {
    pub profile_arn: String,
    pub fingerprint: String,
}

/// What a `CreateAgentGoal` idempotency token resolves to: the goal the original request
/// created, the profile it belongs to, and that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateAgentGoalToken {
    pub profile_arn: String,
    pub goal_id: String,
    pub fingerprint: String,
}

/// What an `UpdateAgentGoal` idempotency token resolves to: the goal the original request
/// updated, the profile it belongs to, and that request's parameter fingerprint.
///
/// The update itself has no value to mint, so what the token is for here is keeping a
/// retry from acting a SECOND time: the replay answers the goal as the original request
/// left it instead of stamping `lastModifiedAt` again.
#[derive(Debug, Clone)]
pub struct UpdateAgentGoalToken {
    pub profile_arn: String,
    pub goal_id: String,
    pub fingerprint: String,
}

/// What a `CreateAgentContext` idempotency token resolves to: the context the original
/// request created, the profile it belongs to, and that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateAgentContextToken {
    pub profile_arn: String,
    pub context_id: String,
    pub fingerprint: String,
}

/// What an `UpdateAgentContext` idempotency token resolves to: the context the original
/// request updated, the profile it belongs to, and that request's parameter fingerprint.
///
/// Held exactly as [`UpdateAgentGoalToken`] is, and for the same reason: the update mints
/// no value, so what the token is for is keeping a retry from acting a SECOND time — the
/// replay answers the context as the original request left it instead of stamping
/// `lastModifiedAt` again.
#[derive(Debug, Clone)]
pub struct UpdateAgentContextToken {
    pub profile_arn: String,
    pub context_id: String,
    pub fingerprint: String,
}

/// The account-level settings `UpdateGlobalSettings` records and `GetGlobalSettings`
/// reports — "Global settings for all workloads"
/// (`smithy_query(query="doc", shape="GetGlobalSettings")`). There is one set per
/// account, so the store holds a singleton rather than a map.
///
/// Every member is what the last write RECORDED: a member no write has named is `None`,
/// and the projection ([`GlobalSettingsRecord::to_global_settings`]) is what answers such
/// a member with the value an account that has configured nothing is running under.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalSettingsRecord {
    /// `OrganizationSharingStatus` as the last write recorded it.
    pub organization_sharing_status: Option<String>,
    /// `DiscoveryIntegrationStatus` as the last write recorded it.
    pub discovery_integration_status: Option<String>,
    /// `AccountJiraConfigurationInput$IntegrationStatus` as the last write recorded it.
    pub jira_integration_status: Option<String>,
    /// `AccountJiraConfigurationInput$IssueManagementStatus` as the last write recorded
    /// it.
    pub jira_issue_management_status: Option<String>,
    /// `AccountJiraConfigurationInput$IssueManagementType` as the last write recorded it.
    pub jira_issue_management_type: Option<String>,
    /// `AccountJiraConfigurationInput$JiraProjectKey` as the last write recorded it — the
    /// "Jira project key to sync workloads to".
    pub jira_project_key: Option<String>,
}

impl GlobalSettingsRecord {
    /// The `GetGlobalSettingsOutput` this account's settings project to.
    ///
    /// A status a write recorded is reported as recorded. A status no write has set is
    /// reported as the one an account that has configured nothing is running under
    /// ([`GLOBAL_SETTING_DISABLED`], and [`JIRA_INTEGRATION_NOT_CONFIGURED`] for the Jira
    /// connection), because such an account is not unconfigured — it is running with the
    /// feature off.
    ///
    /// The Jira members that are NOT a status are reported only once a write recorded
    /// them: `IssueManagementType` publishes `AUTO` and `MANUAL`
    /// (`smithy_query(query="shape", shape="IssueManagementType")`) and so has no member
    /// standing for "nobody chose one", and a project key is a value, not a default.
    /// `Subdomain` and `StatusMessage` are reported by the Jira CONNECTION and no request
    /// of this API carries either (`smithy_query(query="shape",
    /// shape="AccountJiraConfigurationInput")`), so this emulator holds nothing to report
    /// them from.
    pub fn to_global_settings(&self) -> GetGlobalSettingsOutput {
        GetGlobalSettingsOutput {
            organization_sharing_status: Some(
                self.organization_sharing_status
                    .clone()
                    .unwrap_or_else(|| GLOBAL_SETTING_DISABLED.to_string()),
            ),
            discovery_integration_status: Some(
                self.discovery_integration_status
                    .clone()
                    .unwrap_or_else(|| GLOBAL_SETTING_DISABLED.to_string()),
            ),
            jira_configuration: Some(AccountJiraConfigurationOutput {
                integration_status: Some(
                    self.jira_integration_status
                        .clone()
                        .unwrap_or_else(|| JIRA_INTEGRATION_NOT_CONFIGURED.to_string()),
                ),
                issue_management_status: Some(
                    self.jira_issue_management_status
                        .clone()
                        .unwrap_or_else(|| GLOBAL_SETTING_DISABLED.to_string()),
                ),
                issue_management_type: self.jira_issue_management_type.clone(),
                subdomain: None,
                jira_project_key: self.jira_project_key.clone(),
                status_message: None,
            }),
        }
    }
}

/// ONE Region's store of this service's resources — every resource the Well-Architected
/// Tool holds for the account in a single Region. A handler reaches the store of the
/// Region its request was signed for through [`service_state`], and
/// [`RegionalStores`] is what holds one of these per Region.
#[derive(Debug, Default)]
pub struct ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
    /// Every stored workload, keyed by its 32-lowercase-hex `WorkloadId`.
    pub workloads: HashMap<String, WorkloadRecord>,
    /// Every recorded milestone, keyed by `WorkloadId` then by `MilestoneNumber`. A
    /// `BTreeMap` because the next number is one past the highest already handed out.
    pub milestones: HashMap<String, BTreeMap<i32, MilestoneRecord>>,
    /// `CreateMilestone`'s `ClientRequestToken` -> what the original request recorded.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_milestone_tokens: TokenLedger<CreateMilestoneToken>,
    /// `UpgradeLensReview`'s `ClientRequestToken` -> the upgrade the original request
    /// completed, held exactly as [`Self::upgrade_review_template_lens_review_tokens`] is
    /// and for the same reason: an upgrade answers an empty success, so the completed
    /// upgrade is what a retry is served from ([`CompletedUpgradeToken`]) rather than the
    /// request being run a second time — which would move the review again and collide
    /// with the milestone the original request recorded.
    pub upgrade_lens_review_tokens: TokenLedger<CompletedUpgradeToken>,
    /// Every share recorded against a workload, keyed by `WorkloadId` then by `ShareId`.
    /// A `BTreeMap` because `ShareId` order is the stable order `ListWorkloadShares`
    /// pages over.
    pub workload_shares: HashMap<String, BTreeMap<String, WorkloadShareRecord>>,
    /// `CreateWorkloadShare`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_workload_share_tokens: TokenLedger<CreateWorkloadShareToken>,
    /// `DeleteWorkloadShare`'s `ClientRequestToken` -> the delete the original request
    /// completed ([`delete_replay`]), held exactly as [`Self::delete_profile_share_tokens`]
    /// is and for the same reason: the revoked share cannot serve the replay, so the
    /// completed delete does, and a retry carrying the token answers that success rather
    /// than the not-found the original request's own work produced.
    pub delete_workload_share_tokens: TokenLedger<CompletedDeleteToken>,
    /// Every share recorded against a custom lens, keyed by the lens id then by
    /// `ShareId`. A `BTreeMap` because `ShareId` order is the stable order a listing of
    /// a lens's shares pages over, exactly as [`Self::workload_shares`] is ordered.
    pub lens_shares: HashMap<String, BTreeMap<String, LensShareRecord>>,
    /// `CreateLensShare`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_lens_share_tokens: TokenLedger<CreateLensShareToken>,
    /// `DeleteLensShare`'s `ClientRequestToken` -> the delete the original request
    /// completed ([`delete_replay`]), held exactly as [`Self::delete_workload_tokens`] is
    /// and for the same reason: the revoked share cannot serve the replay, so the
    /// completed delete does, and a retry carrying the token answers that success rather
    /// than the not-found the original request's own work produced.
    pub delete_lens_share_tokens: TokenLedger<CompletedDeleteToken>,
    /// Normalized workload name -> `WorkloadId`. Backs the documented "name must be
    /// unique within an account within a Region; spaces and capitalization are
    /// ignored" rule.
    pub workload_name_index: HashMap<String, String>,
    /// `CreateWorkload`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_workload_tokens: TokenLedger<CreateWorkloadToken>,
    /// `DeleteWorkload`'s `ClientRequestToken` -> the delete the original request
    /// completed ([`delete_replay`]), so a retry carrying it replays that success instead
    /// of resolving an id the original request removed.
    ///
    /// It deliberately OUTLIVES the workload it names — that is the whole point of it,
    /// and why [`Self::remove_workload`] (which drops the ledgers that resolve to a live
    /// workload) leaves it alone. Nothing a live resource bounds it, so the
    /// [`TokenLedger`]'s own ceiling ([`MAX_REMEMBERED_REQUEST_TOKENS`]) is what does:
    /// write it through [`TokenLedger::remember`].
    pub delete_workload_tokens: TokenLedger<CompletedDeleteToken>,
    /// Every stored review template, keyed by its `TemplateArn` — the only identifier
    /// the modelled operations take for one (`GetReviewTemplate`,
    /// `UpdateReviewTemplate` and `DeleteReviewTemplate` all bind `TemplateArn` as the
    /// URI label).
    pub review_templates: HashMap<String, ReviewTemplateRecord>,
    /// `CreateReviewTemplate`'s `ClientRequestToken` -> what the original request
    /// created. Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_review_template_tokens: TokenLedger<CreateReviewTemplateToken>,
    /// `DeleteReviewTemplate`'s `ClientRequestToken` -> the delete the original request
    /// completed, held exactly as [`Self::delete_workload_tokens`] is and for the same
    /// reason: the deleted template cannot serve the replay, so the completed delete does.
    pub delete_review_template_tokens: TokenLedger<CompletedDeleteToken>,
    /// `UpgradeReviewTemplateLensReview`'s `ClientRequestToken` -> the upgrade the
    /// original request completed, held exactly as [`Self::delete_review_template_tokens`]
    /// is and for the same reason: an upgrade answers an empty success, so the completed
    /// upgrade is what a retry is served from ([`CompletedUpgradeToken`]).
    pub upgrade_review_template_lens_review_tokens: TokenLedger<CompletedUpgradeToken>,
    /// Every stored Well-Architected Tool profile, keyed by its `ProfileArn` — the only
    /// identifier the modelled profile operations take for one (`GetProfile`,
    /// `UpdateProfile`, `DeleteProfile` and every `/profiles/{ProfileArn}` route bind it
    /// as the URI label).
    pub profiles: HashMap<String, ProfileRecord>,
    /// `CreateProfile`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_profile_tokens: TokenLedger<CreateProfileToken>,
    /// `DeleteProfile`'s `ClientRequestToken` -> the delete the original request
    /// completed, held exactly as [`Self::delete_workload_tokens`] is and for the same
    /// reason: the deleted profile cannot serve the replay, so the completed delete does.
    pub delete_profile_tokens: TokenLedger<CompletedDeleteToken>,
    /// `UpgradeProfileVersion`'s `ClientRequestToken` -> the upgrade the original request
    /// performed. Caller input keys it, so it is a BOUNDED [`TokenLedger`]: write it
    /// through [`TokenLedger::remember`].
    pub upgrade_profile_version_tokens: TokenLedger<UpgradeProfileVersionToken>,
    /// `UpdateIntegration`'s `ClientRequestToken` -> the sync the original request
    /// performed. Caller input keys it, so it is a BOUNDED [`TokenLedger`]: write it
    /// through [`TokenLedger::remember`].
    pub update_integration_tokens: TokenLedger<UpdateIntegrationToken>,
    /// Every share recorded against a Well-Architected Tool profile, keyed by that
    /// profile's `ProfileArn` then by `ShareId`. A `BTreeMap` because `ShareId` order is
    /// the stable order `ListProfileShares` pages over, exactly as
    /// [`Self::workload_shares`] is ordered.
    pub profile_shares: HashMap<String, BTreeMap<String, ProfileShareRecord>>,
    /// `CreateProfileShare`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_profile_share_tokens: TokenLedger<CreateProfileShareToken>,
    /// `DeleteProfileShare`'s `ClientRequestToken` -> the delete the original request
    /// completed ([`delete_replay`]), held exactly as [`Self::delete_lens_share_tokens`]
    /// is and for the same reason: the revoked share cannot serve the replay, so the
    /// completed delete does, and a retry carrying the token answers that success rather
    /// than the not-found the original request's own work produced.
    pub delete_profile_share_tokens: TokenLedger<CompletedDeleteToken>,
    /// Every share recorded against a review template, keyed by that template's
    /// `TemplateArn` then by `ShareId`. A `BTreeMap` because `ShareId` order is the
    /// stable order `ListTemplateShares` pages over, exactly as
    /// [`Self::workload_shares`] is ordered.
    pub template_shares: HashMap<String, BTreeMap<String, TemplateShareRecord>>,
    /// `CreateTemplateShare`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_template_share_tokens: TokenLedger<CreateTemplateShareToken>,
    /// `DeleteTemplateShare`'s `ClientRequestToken` -> the delete the original request
    /// completed ([`delete_replay`]), held exactly as [`Self::delete_profile_share_tokens`]
    /// is and for the same reason: the revoked share cannot serve the replay, so the
    /// completed delete does, and a retry carrying the token answers that success rather
    /// than the not-found the original request's own work produced.
    pub delete_template_share_tokens: TokenLedger<CompletedDeleteToken>,
    /// Every stored custom lens, keyed by its 32-lowercase-hex lens id — the resource
    /// part of its ARN, which is how a `LensAlias` names it.
    pub lenses: HashMap<String, LensRecord>,
    /// `ImportLens`'s `ClientRequestToken` -> what the original request imported.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`]: write it through
    /// [`TokenLedger::remember`], which holds it at
    /// [`MAX_REMEMBERED_REQUEST_TOKENS`].
    pub import_lens_tokens: TokenLedger<ImportLensToken>,
    /// `CreateLensVersion`'s `ClientRequestToken` -> the version the original request
    /// published. Caller input keys it, so it is a BOUNDED [`TokenLedger`] like every
    /// other ledger here — the lens and version quotas
    /// ([`MAX_CUSTOM_LENSES_PER_ACCOUNT_REGION`] times [`MAX_LENS_VERSIONS_PER_LENS`])
    /// bound what a publish can RECORD, not the stream of tokens a caller can spend
    /// against it.
    pub create_lens_version_tokens: TokenLedger<CreateLensVersionToken>,
    /// `DeleteLens`'s `ClientRequestToken` -> the delete the original request completed
    /// ([`delete_replay`]), held exactly as [`Self::delete_lens_share_tokens`] is and for
    /// the same reason: the content the delete removed cannot serve the replay, so the
    /// completed delete does, and a retry carrying the token answers that success rather
    /// than the not-found the original request's own work produced.
    pub delete_lens_tokens: TokenLedger<CompletedDeleteToken>,
    /// Every stored agent profile, keyed by its `AgentProfileArn` — the only identifier
    /// the modelled agent-profile operations take for one (`GetAgentProfile`,
    /// `UpdateAgentProfile`, `DeleteAgentProfile` and every `/goals` route bind
    /// `profileArn` as the URI label).
    pub agent_profiles: HashMap<String, AgentProfileRecord>,
    /// `CreateAgentProfile`'s `clientToken` -> what the original request created. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_agent_profile_tokens: TokenLedger<CreateAgentProfileToken>,
    /// `UpdateAgentProfile`'s `clientToken` -> the profile and parameters the original
    /// request applied, so a retry carrying it records no second modification. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`] like every ledger here.
    pub update_agent_profile_tokens: TokenLedger<UpdateAgentProfileToken>,
    /// Every optimization goal recorded against an agent profile, keyed by that
    /// profile's ARN then by the goal's `UUID`. A `BTreeMap` because goal-id order is the
    /// stable order `ListAgentGoals` pages over.
    pub agent_goals: HashMap<String, BTreeMap<String, AgentGoalRecord>>,
    /// `CreateAgentGoal`'s `clientToken` -> what the original request created. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_agent_goal_tokens: TokenLedger<CreateAgentGoalToken>,
    /// `UpdateAgentGoal`'s `clientToken` -> the goal the original request updated. Held
    /// exactly as the create ledger above is, and bounded for the same reason: caller
    /// input keys it, so write it through [`TokenLedger::remember`].
    pub update_agent_goal_tokens: TokenLedger<UpdateAgentGoalToken>,
    /// Every context recorded against an agent profile, keyed by that profile's ARN then
    /// by the context's `UUID`. A `BTreeMap` because context-id order is the stable order
    /// `ListAgentContexts` pages over.
    pub agent_contexts: HashMap<String, BTreeMap<String, AgentContextRecord>>,
    /// `CreateAgentContext`'s `clientToken` -> what the original request created. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_agent_context_tokens: TokenLedger<CreateAgentContextToken>,
    /// `UpdateAgentContext`'s `clientToken` -> the context the original request updated.
    /// Held exactly as the create ledger above is, and bounded for the same reason:
    /// caller input keys it, so write it through [`TokenLedger::remember`].
    pub update_agent_context_tokens: TokenLedger<UpdateAgentContextToken>,
    /// Every recommendation generation `StartAgentRecommendationGeneration` has accepted,
    /// keyed by the agent profile's ARN then by the generation's `UUID`. A `BTreeMap`
    /// because generation-id order is the stable order a listing pages over, exactly as
    /// the goals and contexts above are held.
    pub agent_generations: HashMap<String, BTreeMap<String, AgentGenerationRecord>>,
    /// Every optimization recommendation a COMPLETED generation has produced, keyed by the
    /// agent profile's ARN then by the recommendation's own `RecommendationArn`. A
    /// `BTreeMap` because ARN order is the stable order `ListAgentRecommendations` pages
    /// over, exactly as the goals, contexts and generations above are held.
    ///
    /// Bounded by what it is produced FROM: a generation produces one recommendation per
    /// pillar its scope names (at most the `Pillar` enum's whole set), and a profile holds
    /// at most [`MAX_GENERATIONS_PER_AGENT_PROFILE`] generations, so no caller can grow
    /// this past that product — and [`Self::remove_agent_profile`] drops a deleted
    /// profile's recommendations with its generations, which is what keeps that bound real.
    pub agent_recommendations: HashMap<String, BTreeMap<String, AgentRecommendationRecord>>,
    /// The account's global settings — what `UpdateGlobalSettings` records and
    /// `GetGlobalSettings` reports. One set per account, so a singleton rather than a
    /// map, and `Default` (every member unrecorded) is the account before any write.
    pub global_settings: GlobalSettingsRecord,
    /// Monotonic input to [`Self::next_hex_id`]; never returned to a caller.
    id_counter: u64,
}
