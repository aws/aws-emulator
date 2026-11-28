// SPDX-License-Identifier: CC0-1.0

//! The code review CONFIGURATIONS an agent space holds: "the parameters for automated
//! security-focused code analysis" (source: the `CreateCodeReview` operation doc).
//!
//! One concern, one module: the record `CreateCodeReview` writes, the projection every
//! code review response is answered from, the form the id takes, how many an agent
//! space may hold, and the one rule the model states about the budget cap. Every
//! operation of a code review — the create, the batch read, and the update, listing and
//! delete paths implemented after them — reads and writes THIS record, so none of them
//! keeps a second copy of a code review.
//!
//! A code review is a configuration rather than a job: it carries no status member in
//! the model and nothing about it is owed by the clock, so a read of one reports exactly
//! what the last write left.

use std::time::SystemTime;

use crate::ops::types::{Assets, CloudWatchLog, CodeReview, ReportDestination, ReportFilters};

use super::{assets_entry_violation, timestamp};

/// The prefix every `codeReviewId` this service hands out carries.
///
/// CHOSEN, not recorded: `CreateCodeReviewOutput$codeReviewId` targets the prelude
/// `String` and carries neither a `@pattern` nor a `@length`, so nothing pins a form.
/// What is chosen is the FORM every resource class of this service already hands its
/// identifiers out in (see [`super::AGENT_SPACE_ID_PREFIX`],
/// [`super::TARGET_DOMAIN_ID_PREFIX`], [`super::ARTIFACT_ID_PREFIX`]) — a short
/// lowercase class abbreviation, a hyphen, then a body freshly generated for that one
/// resource — so a client can tell a code review's id from the id of the space that
/// holds it.
pub const CODE_REVIEW_ID_PREFIX: &str = "cr-";

/// The most code reviews ONE agent space holds before `CreateCodeReview` answers the
/// service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited, for the same reason as [`super::MAX_ARTIFACTS_PER_AGENT_SPACE`]:
/// the real service's code review quota is not in the model. Every code review is a
/// stored record holding the assets and filters it was configured with, so this bounds
/// what one agent space can accumulate; it is set far above any plausible client's code
/// review count, so normal use never meets it.
pub const MAX_CODE_REVIEWS_PER_AGENT_SPACE: usize = 100;

/// The violation a `maxTaskHours` the service cannot accept fails, or `None` when the
/// value is one it can.
///
/// MODELLED: `maxTaskHours` "must be a positive number" (source:
/// `smithy_query(query="doc",shape="CreateCodeReviewInput",member="maxTaskHours")`),
/// and the member targets `Double`, which carries no `@range`, so the boundary validator
/// lets zero, a negative value and a non-finite one through to the handler. The value is
/// the budget cap jobs started from the stored review run under, so a value that is not a
/// positive number is refused naming the member rather than stored. Stated here once, so
/// every operation that writes the cap refuses the same values.
pub fn max_task_hours_violation(max_task_hours: f64) -> Option<String> {
    (!(max_task_hours.is_finite() && max_task_hours > 0.0))
        .then(|| "member must be a positive number".to_string())
}

/// The most entries ONE of the asset lists of a code review may name.
///
/// CHOSEN, not cited: the asset lists (`Assets$endpoints`, `$actors`, `$documents`,
/// `$sourceCode`, `$integratedRepositories`, `$trustedCaCertificates`) carry no
/// `@length` in the model, so the boundary validator passes a list of any size, and every
/// entry is kept on the stored record and reported on each later read of the review. This
/// bounds what ONE request can add to the store and to a response; it is set far above
/// any plausible client's asset count, so normal use never meets it. (The filter lists
/// beside them need no such bound: `ReportFilters`' members are `@length(max: 50)` in the
/// model, so the boundary validator already refuses an oversized one.)
pub const MAX_CODE_REVIEW_ASSET_ENTRIES: usize = 100;

/// The member path and the violation the `assets` of a code review fail, or `None` when
/// every list it names is one the service can store.
///
/// Stated here once, so every operation that writes the assets of a code review refuses
/// the same lists, naming the member that was too long.
pub fn code_review_assets_violation(assets: &Assets) -> Option<(String, String)> {
    assets_entry_violation(assets, MAX_CODE_REVIEW_ASSET_ENTRIES)
}

/// One stored code review: "a new code review configuration in an agent space" (source:
/// the `CreateCodeReview` operation doc), carrying the members the caller supplied plus
/// the identity and the timestamps the service mints.
///
/// A code review belongs to exactly ONE agent space — every operation that names a code
/// review names the space beside it — so the owning id is part of the record and a read
/// resolves the PAIR rather than the code review id alone.
///
/// The modelled shapes are stored as they arrived rather than flattened into local
/// mirrors, so a read projects the stored record and never reconstructs one from request
/// text.
#[derive(Debug, Clone)]
pub struct CodeReviewRecord {
    pub code_review_id: String,
    /// The agent space the code review was created in.
    pub agent_space_id: String,
    pub title: String,
    pub assets: Assets,
    pub service_role: Option<String>,
    pub log_config: Option<CloudWatchLog>,
    pub code_remediation_strategy: Option<String>,
    pub validation_mode: Option<String>,
    /// The budget cap jobs started from this review run under; `None` is the state the
    /// model describes as "no budget cap".
    pub max_task_hours: Option<f64>,
    pub report_destination: Option<ReportDestination>,
    pub report_filters: Option<ReportFilters>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl CodeReviewRecord {
    /// This code review projected onto the [`CodeReview`] shape every code review
    /// response carries: the stored record's own fields, never request input.
    ///
    /// The create echo and the batch read are answered through THIS one projection, so
    /// what a create tells its caller is what a later read of the store reports, member
    /// for member.
    pub fn modelled(&self) -> CodeReview {
        CodeReview {
            code_review_id: self.code_review_id.clone(),
            agent_space_id: self.agent_space_id.clone(),
            title: self.title.clone(),
            assets: self.assets.clone(),
            service_role: self.service_role.clone(),
            log_config: self.log_config.clone(),
            code_remediation_strategy: self.code_remediation_strategy.clone(),
            validation_mode: self.validation_mode.clone(),
            max_task_hours: self.max_task_hours,
            report_destination: self.report_destination.clone(),
            report_filters: self.report_filters.clone(),
            created_at: Some(timestamp(self.created_at)),
            updated_at: Some(timestamp(self.updated_at)),
        }
    }
}
