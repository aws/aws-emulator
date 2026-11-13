// SPDX-License-Identifier: CC0-1.0

//! The MODEL CARD slice of this service's state: the card `CreateModelCard` writes,
//! the versions it accumulates, the content document it admits, and the export jobs
//! `CreateModelCardExportJob` runs over it.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit. Every name is re-exported from [`super`], so a handler keeps
//! naming `crate::state::…`.

use super::*;

/// The version number a model card's FIRST version carries. `DescribeModelCard`'s
/// `ModelCardVersion` is modelled as an `Integer` and the API's version history starts
/// at one, so `CreateModelCard` stores version 1 and an `UpdateModelCard` that carries
/// new `Content` appends the next integer ([`ModelCard::next_version`]).
pub const FIRST_MODEL_CARD_VERSION: i32 = 1;

/// One version of a model card, as `CreateModelCard` stores the first one and
/// `DescribeModelCard` replays it.
///
/// A model card is versioned: `Content`, `ModelCardStatus` and `SecurityConfig` belong
/// to a version, while the name, the ARN and the tags belong to the card.
///
/// `created_by` is stored when the version is created rather than rebuilt per read, so
/// every later read reports the identity that actually created it; `last_modified_by`
/// stays absent until a call modifies the version.
#[derive(Debug, Clone)]
pub struct ModelCardVersion {
    pub version: i32,
    pub content: String,
    pub status: String,
    pub security_config: Option<ModelCardSecurityConfig>,
    pub creation_time: SystemTime,
    pub created_by: UserContext,
    pub last_modified_time: SystemTime,
    pub last_modified_by: Option<UserContext>,
}

/// A model card created by `CreateModelCard`, with every version it has had.
///
/// `versions` is ordered by `version` ascending and is never empty: a card exists only
/// once its first version does.
#[derive(Debug, Clone)]
pub struct ModelCard {
    pub name: String,
    pub arn: String,
    pub tags: Vec<Tag>,
    pub versions: Vec<ModelCardVersion>,
}

impl ModelCard {
    /// A card holding exactly `first`, the version `CreateModelCard` minted.
    pub fn new(name: String, arn: String, tags: Vec<Tag>, first: ModelCardVersion) -> Self {
        Self {
            name,
            arn,
            tags,
            versions: vec![first],
        }
    }

    /// The highest-numbered version — what a reader that names no version describes.
    pub fn latest_version(&self) -> Option<&ModelCardVersion> {
        self.versions.last()
    }

    /// The version numbered `version`, or `None` when the card never had it.
    pub fn version(&self, version: i32) -> Option<&ModelCardVersion> {
        self.versions.iter().find(|held| held.version == version)
    }

    /// The highest-numbered version, mutably — the one version an in-place change (a
    /// status update) applies to; the versions before it are history and stay as they
    /// were recorded.
    pub fn latest_version_mut(&mut self) -> Option<&mut ModelCardVersion> {
        self.versions.last_mut()
    }

    /// The number the NEXT version of this card carries: one past the highest it holds,
    /// so a card's version history is the consecutive run starting at
    /// [`FIRST_MODEL_CARD_VERSION`].
    pub fn next_version(&self) -> i32 {
        self.versions
            .last()
            .map_or(FIRST_MODEL_CARD_VERSION, |held| {
                held.version.saturating_add(1)
            })
    }

    /// Whether this card can gain another version. See [`MODEL_CARD_VERSION_QUOTA`].
    pub fn versions_below_quota(&self) -> bool {
        self.versions.len() < MODEL_CARD_VERSION_QUOTA
    }

    /// Whether the card is ARCHIVED, i.e. its latest version carries
    /// [`MODEL_CARD_STATUS_ARCHIVED`] — the status that takes the card out of review and
    /// closes it to further change.
    ///
    /// The status belongs to a version and the latest one is the card's current state, so
    /// that is the version the question is asked of; the archived versions behind it are
    /// history.
    pub fn is_archived(&self) -> bool {
        self.latest_version()
            .is_some_and(|latest| latest.status == MODEL_CARD_STATUS_ARCHIVED)
    }
}

/// The `ModelCardStatus` member that closes a card to further change: `Archived`, which
/// the modelled enum documents as "The model card is archived. No more updates should be
/// made to the model card, but it can still be exported"
/// (`smithy_query(query="shape",shape="ModelCardStatus")`). Named once here so no handler
/// compares a bare string literal.
pub const MODEL_CARD_STATUS_ARCHIVED: &str = "Archived";

/// How SageMaker refuses a change to a card that is already
/// [`MODEL_CARD_STATUS_ARCHIVED`]: it states the card's state and nothing else.
///
/// Entirely FIXED text — the recorded refusal interpolates no name, ARN or status value,
/// and closes with no period — so the sentence is stated once here and every operation
/// that would change an archived card words the one fact the same way.
pub const MODEL_CARD_ARCHIVED: &str = "Model card is in archived state";

/// Whether `document` is a JSON object.
///
/// The one structural claim this service makes about a caller-supplied JSON document
/// whose model bounds only its length — a model card's `Content`
/// ([`is_model_card_content`]), a model package group's `ResourcePolicy` — so the
/// boundary validator passes such a value and the service is what refuses it. Stated
/// once here rather than once per operation.
pub fn is_json_object(document: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(document).is_ok_and(|parsed| parsed.is_object())
}

/// Whether `content` can be a model card's `Content`.
///
/// `Content`'s own documentation says it "must be in model card JSON schema and provided
/// as a string", so a value that is not even a JSON object is not a model card. The
/// model constrains only the member's length, so the boundary validator passes such a
/// value and the service refuses it. The schema's own required members are NOT checked
/// here: this is the one structural claim the model's documentation makes.
pub fn is_model_card_content(content: &str) -> bool {
    is_json_object(content)
}

/// `ModelCardExportJobStatus`'s members this emulator moves an export job through (the
/// modelled enum carries exactly `InProgress`, `Completed` and `Failed`). Named once
/// here so no handler compares a bare string literal.
pub const MODEL_CARD_EXPORT_STATUS_IN_PROGRESS: &str = "InProgress";
/// See [`MODEL_CARD_EXPORT_STATUS_IN_PROGRESS`].
pub const MODEL_CARD_EXPORT_STATUS_FAILED: &str = "Failed";
/// See [`MODEL_CARD_EXPORT_STATUS_IN_PROGRESS`]: the member for an export that produced its
/// document (`crate::state::model_card_export`).
pub const MODEL_CARD_EXPORT_STATUS_COMPLETED: &str = "Completed";

/// How long after `CreateModelCardExportJob` the clock owes the job's terminal status.
///
/// Same contract as [`TRANSFORM_JOB_SETTLES_AFTER`]: a caller that describes the job
/// straight after creating it still observes `InProgress`, one that waits observes the
/// terminal status, so a waiter terminates. Real SageMaker takes as long as rendering
/// and uploading the document takes; the span is compressed, never zero, and the due
/// instant is stored rather than slept on.
pub const MODEL_CARD_EXPORT_JOB_SETTLES_AFTER: Duration = Duration::from_millis(300);

/// The store key one model card export job is held under: the card it exports and the
/// job's own name.
///
/// A job's ARN is `<card ARN>/export-job/<job name>`, so the job's name is unique within
/// the card rather than within the account — two cards may each have an export job of
/// the same name, and they are two jobs.
pub fn model_card_export_job_key(card_name: &str, job_name: &str) -> String {
    format!("{card_name}/{job_name}")
}

/// The store key the export-job ARN `arn` addresses, or `None` when the value does not
/// name an export job of a model card of this service, in this account and in `region`.
///
/// `DescribeModelCardExportJob` resolves a job by ARN alone, and the store is keyed by
/// the card and job names the ARN carries — so the ARN is parsed and its fixed segments
/// checked the way [`entity_name_from_name_or_arn`] checks an entity's, before the
/// resource portion `model-card/<card>/export-job/<job>` is reduced to that key. An ARN
/// naming another partition, service, Region, account or resource shape names no job
/// here, and a name segment carrying a further `/` is refused rather than folded into the
/// key, so two different ARNs can never address one record.
pub fn model_card_export_job_key_from_arn(arn: &str, region: &str) -> Option<String> {
    let (card_name, job_name) = arn_resource(arn, region)?
        .strip_prefix("model-card/")?
        .split_once("/export-job/")?;
    (!card_name.is_empty()
        && !card_name.contains('/')
        && !job_name.is_empty()
        && !job_name.contains('/'))
        .then(|| model_card_export_job_key(card_name, job_name))
}

/// A model card export job created by `CreateModelCardExportJob`, as
/// `DescribeModelCardExportJob` replays it.
///
/// `card_version` is the version the create RESOLVED (the requested one, or the card's
/// latest when the request named none), so a later read reports the version that was
/// exported rather than whichever is latest by then. `ends_at` is the instant the clock
/// owes the move out of `InProgress`; it is stored, never slept on, and materialised by
/// whichever reporting call first runs at or after it ([`ModelCardExportJob::settle_status`]).
///
/// `outcome` is what the job's own export RAN ([`run_model_card_export`]): the `s3://` URI
/// of the document it wrote, or why it wrote none. It is recorded when the export runs and
/// is what the terminal status the clock owes is read off, so the status, the
/// `FailureReason` and the `ExportArtifacts` a read reports always agree with what
/// happened.
#[derive(Debug, Clone)]
pub struct ModelCardExportJob {
    pub name: String,
    pub arn: String,
    pub card_name: String,
    pub card_version: i32,
    pub status: String,
    pub output_config: ModelCardExportOutputConfig,
    pub failure_reason: Option<String>,
    pub outcome: ModelCardExportOutcome,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    pub ends_at: SystemTime,
}

impl ModelCardExportJob {
    /// Materialise the move out of `InProgress` the clock already owes.
    ///
    /// The one write a reporting operation may make: the move was due at `ends_at`
    /// whether or not anybody described the job, so the observer makes no difference to
    /// the result, and `LastModifiedAt` takes the instant the move was due rather than
    /// "now", so the value is stable across later reads.
    ///
    /// The terminal status is the one the job's own [`ModelCardExportJob::outcome`] names:
    /// [`MODEL_CARD_EXPORT_STATUS_COMPLETED`] for an export that wrote its document, and
    /// [`MODEL_CARD_EXPORT_STATUS_FAILED`] carrying the reason for one that did not.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status != MODEL_CARD_EXPORT_STATUS_IN_PROGRESS || now < self.ends_at {
            return false;
        }
        match &self.outcome {
            ModelCardExportOutcome::Exported(_) => {
                self.status = MODEL_CARD_EXPORT_STATUS_COMPLETED.to_string();
            }
            ModelCardExportOutcome::Failed(reason) => {
                self.status = MODEL_CARD_EXPORT_STATUS_FAILED.to_string();
                self.failure_reason = Some(reason.clone());
            }
        }
        self.last_modified_at = self.ends_at;
        true
    }

    /// The document this job's export wrote, once the job has REACHED
    /// [`MODEL_CARD_EXPORT_STATUS_COMPLETED`], and `None` before that.
    ///
    /// A job still `InProgress` names no artifact: the export's own object is reported
    /// beside the status that says the export finished, never ahead of it.
    #[must_use]
    pub fn exported_artifact(&self) -> Option<&str> {
        match &self.outcome {
            ModelCardExportOutcome::Exported(uri)
                if self.status == MODEL_CARD_EXPORT_STATUS_COMPLETED =>
            {
                Some(uri.as_str())
            }
            _ => None,
        }
    }
}
