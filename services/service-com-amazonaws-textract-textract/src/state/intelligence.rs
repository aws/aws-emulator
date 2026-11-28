// SPDX-License-Identifier: CC0-1.0

//! Amazon Textract's document-intelligence control plane: the PROJECTS that hold the
//! configuration a document insights job runs under, the BLUEPRINTS that say which fields
//! to extract, and the jobs themselves.
//!
//! Every answer the eleven control-plane operations give is derived from the records here,
//! so a create, an update and a delete are observable through the reads and the listings
//! and can never disagree with them:
//!
//! * a project or a blueprint is addressed by its own ARN, and the identifier inside that
//!   ARN is the key it is stored under ([`resource_id_of`]). One identifier space serves
//!   all three kinds ([`super::ComAmazonawsTextractTextractState::mint_intelligence_id`]),
//!   so no ARN of one kind can ever resolve to a resource of another;
//! * a blueprint carries its LIVE schema plus the immutable numbered versions
//!   `CreateBlueprintVersion` snapshotted from it, so a version keeps the schema it was
//!   cut from however often the blueprint is updated afterwards;
//! * a project's configuration is replaced member by member by `UpdateProject`: a
//!   configuration the request supplies replaces the stored one whole, and one it omits is
//!   left as it was, which is exactly what AWS documents for that operation;
//! * a listing is cut from the stored records in ONE total order (oldest first, the
//!   identifier breaking ties), so an offset cursor cannot skip or repeat a resource
//!   between pages.
//!
//! Nothing here is evicted except an insights job: a project and a blueprint are
//! control-plane resources a later call must still read, so a create past the store's
//! ceiling is refused rather than made room for.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    BlueprintFilter, BlueprintItem, BlueprintSummary, CustomConfiguration,
    EncryptionConfiguration, ExtractionConfiguration, GenerativeConfiguration, ProjectFilter,
    ProjectSummary, ResourceNotFoundException, SensitiveDataConfiguration, SplitterConfiguration,
    ValidationException,
};

use super::{ComAmazonawsTextractTextractState, ARN_SERVICE};

// ---------------------------------------------------------------------------
// ARNs and identifiers
// ---------------------------------------------------------------------------

/// The resource-type segment of a project's ARN.
///
/// `ProjectArn`'s modelled pattern ends `:project/[a-zA-Z0-9-]{12,36}$`
/// (`smithy_query(query="shape", shape="ProjectArn")`), so the resource field is this type
/// segment and the project's own identifier after it.
pub const PROJECT_RESOURCE_TYPE: &str = "project";

/// The resource-type segment of a blueprint's ARN, from `BlueprintArn`'s modelled pattern
/// `:blueprint/(public-[a-zA-Z0-9-_]{1,30}|[a-zA-Z0-9-]{12,36})$`.
pub const BLUEPRINT_RESOURCE_TYPE: &str = "blueprint";

/// The resource-type segment of a document insights job's ARN, from `JobArn`'s modelled
/// pattern `:job/[a-zA-Z0-9-_]+$`.
pub const INSIGHTS_JOB_RESOURCE_TYPE: &str = "job";

/// Characters in a minted project, blueprint or job identifier.
///
/// Inside the `[a-zA-Z0-9-]{12,36}` the `ProjectArn` and `BlueprintArn` patterns allow,
/// and hex, so every character is in that class.
const RESOURCE_ID_LEN: usize = 32;

/// How many projects this service keeps.
///
/// NOT an AWS value: an emulator-side ceiling so a caller cannot grow the heap without
/// end. Nothing is evicted — a project is a control-plane resource a later call must still
/// read — so a create that would exceed it is refused with the modelled
/// `LimitExceededException`.
pub const MAX_STORED_PROJECTS: usize = 1000;

/// How many blueprints this service keeps. Same rule as [`MAX_STORED_PROJECTS`].
pub const MAX_STORED_BLUEPRINTS: usize = 1000;

/// How many immutable versions one blueprint may hold. Same rule again: a version is never
/// evicted, so a `CreateBlueprintVersion` past the ceiling is refused.
pub const MAX_BLUEPRINT_VERSIONS: usize = 100;

/// How many document insights jobs this service keeps. A job is a record of work that has
/// already been done, so the oldest is evicted to make room rather than refusing the
/// submit.
pub const MAX_STORED_INSIGHTS_JOBS: usize = 256;

/// How many `ClientRequestToken` slots this service keeps.
///
/// NOT an AWS value: a slot's key is the caller's own token, so without a ceiling a caller
/// that created and deleted under a fresh token each time would grow the heap without end
/// while staying inside every resource ceiling. A slot is dropped as soon as the resource
/// it named is, and past this ceiling the least recently remembered one is evicted — which
/// only makes that token usable again. Generous beside the resource ceilings, so every live
/// resource's own token keeps resolving.
pub const MAX_STORED_IDEMPOTENCY_SLOTS: usize = 2048;

/// How many fields one document insights job extracts across every blueprint it runs
/// under.
///
/// NOT an AWS value: a blueprint schema is caller-supplied text up to `BlueprintSchema`'s
/// modelled 100,000 characters and `BlueprintItems` admits a thousand blueprints, and each
/// field costs one pass over the document's own fields — so this bounds the PRODUCT one
/// submit can choose rather than either factor alone.
pub const MAX_BLUEPRINT_FIELDS: usize = 256;

/// The `ProjectStatus` a stored project reports.
///
/// `ProjectStatus` is `COMPLETED | FAILED` and AWS describes it as "whether creation
/// completed successfully or failed". `CreateProject` here either stores the project or
/// answers a modelled error, so a project that EXISTS is one whose creation completed.
pub const PROJECT_STATUS_COMPLETED: &str = "COMPLETED";

/// The `ResourceOwner` of a blueprint this account created, from the modelled
/// `ResourceOwner` enum (`ACCOUNT | SERVICE`). `SERVICE` is a blueprint Amazon Textract
/// publishes; this emulator mints none, so it holds only `ACCOUNT` blueprints.
pub const RESOURCE_OWNER_ACCOUNT: &str = "ACCOUNT";

/// The `ResourceOwner` of a blueprint Amazon Textract itself publishes.
pub const RESOURCE_OWNER_SERVICE: &str = "SERVICE";

/// The `JobStatus` a document insights job reports while its completion is still owed by
/// the clock, from the modelled `JobStatus` enum.
pub const JOB_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `JobStatus` of a job whose output was written.
pub const JOB_STATUS_SUCCEEDED: &str = "SUCCEEDED";

/// The `JobStatus` of a job that could not be run to completion.
pub const JOB_STATUS_FAILED: &str = "FAILED";

/// The `JobStatus` of a job whose output was written while part of what the project asked
/// for was not carried out, from the modelled `JobStatus` enum.
pub const JOB_STATUS_PARTIAL_SUCCESS: &str = "PARTIAL_SUCCESS";

/// The `ResourceNotFoundException` message a project-addressed operation answers for a
/// `ProjectArn` that names no project here — one never created, or one already deleted.
///
/// Fixed text for that one failure, whichever operation looked the project up: the ARN the
/// caller sent is already in its own request, so nothing of the request reaches the error
/// surface.
pub const PROJECT_NOT_FOUND_MESSAGE: &str = "Project not found.";

/// The `ResourceNotFoundException` message for a `BlueprintArn` that names no blueprint
/// here. Same rule as [`PROJECT_NOT_FOUND_MESSAGE`].
pub const BLUEPRINT_NOT_FOUND_MESSAGE: &str = "Blueprint not found.";

/// The `ResourceNotFoundException` message for a `BlueprintVersion` the blueprint it was
/// asked of never had.
pub const BLUEPRINT_VERSION_NOT_FOUND_MESSAGE: &str = "Blueprint version not found.";

/// The `ConflictException` message a create answers when the name it asks for is already
/// another project's.
///
/// AWS, on `CreateProjectRequest$ProjectName`: "A unique name for the project." — so a
/// second project cannot take a name this account already uses. Fixed text: the name is in
/// the caller's own request and the project already holding it is not disclosed.
pub const PROJECT_NAME_IN_USE_MESSAGE: &str = "A project with that name already exists.";

/// The `ConflictException` message for a blueprint name already in use. AWS, on
/// `CreateBlueprintRequest$BlueprintName`: "A unique name for the blueprint."
pub const BLUEPRINT_NAME_IN_USE_MESSAGE: &str = "A blueprint with that name already exists.";

/// The `ValidationException` message for a list member the request repeated.
///
/// AWS states this of `ExtractionGranularity$Types` ("Each type can appear only once"),
/// `OutputTextFormat$Types` and `PIIEntitiesConfiguration$PiiEntityTypes`, so it is one
/// sentence for all three.
pub const DUPLICATE_TYPE_MESSAGE: &str = "Each type can appear only once.";

/// The `ValidationException` message for an output text format that asks for `CSV` alone.
///
/// AWS, on `OutputTextFormat$Types`: "CSV can't be the only type."
pub const CSV_ONLY_TEXT_FORMAT_MESSAGE: &str = "CSV can't be the only type.";

/// The `ValidationException` message for a `CustomConfiguration` that supplies a fallback
/// blueprint and no primary one.
///
/// AWS, on `CustomConfiguration$FallbackBlueprints`: "Requires at least one blueprint in
/// `Blueprints`."
pub const FALLBACK_WITHOUT_BLUEPRINT_MESSAGE: &str =
    "FallbackBlueprints requires at least one blueprint in Blueprints.";

/// The `ValidationException` message for a `StartDocumentInsights` that names neither a
/// project nor a blueprint.
///
/// AWS, on `StartDocumentInsights`: it "analyzes a document using the configuration
/// defined by a project or a set of blueprints" — with neither, there is no configuration
/// to analyze it under.
pub const NO_CONFIGURATION_SOURCE_MESSAGE: &str =
    "Either ProjectArn or Blueprints must be specified.";

/// The `ValidationException` message for a blueprint schema that is not a JSON object.
///
/// A blueprint "defines the schema Amazon Textract uses to extract structured data"
/// (`CreateBlueprint`) and its `Schema` "defines the fields to extract and their expected
/// structure" (`CreateBlueprintRequest$Schema`): a schema that is not an object naming
/// fields describes no extraction, so it is refused rather than stored as one that would
/// extract nothing.
pub const SCHEMA_NOT_AN_OBJECT_MESSAGE: &str =
    "Schema must be a JSON object describing the fields to extract.";

/// The ARN of the project `project_id` names, built through the one ARN helper so the
/// account segment is the emulator's canonical one.
///
/// The resource field is `project/{identifier}`, as `ProjectArn`'s modelled pattern spells
/// it.
#[must_use]
pub fn project_arn(region: &str, project_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{PROJECT_RESOURCE_TYPE}/{project_id}"),
    )
}

/// The ARN of the blueprint `blueprint_id` names; resource field `blueprint/{identifier}`.
#[must_use]
pub fn blueprint_arn(region: &str, blueprint_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{BLUEPRINT_RESOURCE_TYPE}/{blueprint_id}"),
    )
}

/// The ARN of the document insights job `job_id` names; resource field `job/{identifier}`.
#[must_use]
pub fn insights_job_arn(region: &str, job_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("{INSIGHTS_JOB_RESOURCE_TYPE}/{job_id}"),
    )
}

/// The identifier `arn` carries, when it is an Amazon Textract ARN of `resource_type`.
///
/// The ONE reading of an ARN into the key a stored resource lives under, so a create, a
/// read, an update and a delete of one resource all resolve the same value. `None` for an
/// ARN that is not this service's, names another resource type, or carries further path
/// segments after the identifier — none of which names a resource stored here.
#[must_use]
pub fn resource_id_of(arn: &str, resource_type: &str) -> Option<String> {
    let resource = super::parse_resource_arn(arn)?;
    (resource.resource_type == resource_type && !resource.resource_id.is_empty())
        .then_some(resource.resource_id)
}

/// The modelled refusal for a `ProjectArn` that names no project here.
#[must_use]
pub fn project_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(PROJECT_NOT_FOUND_MESSAGE.to_string()),
        code: None,
    }
}

/// The modelled refusal for a `BlueprintArn` that names no blueprint here.
#[must_use]
pub fn blueprint_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(BLUEPRINT_NOT_FOUND_MESSAGE.to_string()),
        code: None,
    }
}

/// The modelled refusal for a `BlueprintVersion` its blueprint never had.
#[must_use]
pub fn blueprint_version_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(BLUEPRINT_VERSION_NOT_FOUND_MESSAGE.to_string()),
        code: None,
    }
}

/// A modelled `ValidationException` carrying `message`.
#[must_use]
pub fn invalid(message: &str) -> ValidationException {
    ValidationException {
        message: Some(message.to_string()),
        code: None,
    }
}

// ---------------------------------------------------------------------------
// Projects
// ---------------------------------------------------------------------------

/// The configuration a project holds: every member `CreateProject` takes beyond its name,
/// and every member `UpdateProject` can replace.
///
/// One struct for both, so the create and the update cannot store a project differently.
#[derive(Debug, Clone, Default)]
pub struct ProjectConfiguration {
    pub project_description: Option<String>,
    pub extraction: Option<ExtractionConfiguration>,
    pub generative: Option<GenerativeConfiguration>,
    pub custom: Option<CustomConfiguration>,
    pub splitter: Option<SplitterConfiguration>,
    pub sensitive_data: Option<SensitiveDataConfiguration>,
    pub encryption: Option<EncryptionConfiguration>,
}

impl ProjectConfiguration {
    /// Apply an `UpdateProject` request to this configuration.
    ///
    /// AWS, on `UpdateProject`: "If you omit a configuration, UpdateProject leaves it
    /// unchanged. A configuration that you supply replaces the stored one, so include
    /// every setting that you want to keep." So every `Some` replaces, and every `None`
    /// leaves the stored value alone.
    ///
    /// `EncryptionConfiguration$KmsEncryptionContext` is the ONE exception AWS states:
    /// "On `UpdateProject` and `UpdateBlueprint`, this member is the one exception to
    /// replacing a supplied configuration whole. If you omit it, the stored encryption
    /// context is kept. To clear a custom encryption context, specify an empty map." —
    /// which is what [`merge_encryption`] implements.
    pub fn apply_update(&mut self, update: ProjectConfiguration) {
        if update.project_description.is_some() {
            self.project_description = update.project_description;
        }
        if update.extraction.is_some() {
            self.extraction = update.extraction;
        }
        if update.generative.is_some() {
            self.generative = update.generative;
        }
        if update.custom.is_some() {
            self.custom = update.custom;
        }
        if update.splitter.is_some() {
            self.splitter = update.splitter;
        }
        if update.sensitive_data.is_some() {
            self.sensitive_data = update.sensitive_data;
        }
        if let Some(supplied) = update.encryption {
            self.encryption = Some(merge_encryption(self.encryption.as_ref(), supplied));
        }
    }
}

/// A supplied `EncryptionConfiguration` as it REPLACES a stored one.
///
/// Whole replacement, except for the one member AWS exempts: a request that names no
/// `KmsEncryptionContext` keeps the stored one, and one that names an empty map clears it.
#[must_use]
pub fn merge_encryption(
    stored: Option<&EncryptionConfiguration>,
    supplied: EncryptionConfiguration,
) -> EncryptionConfiguration {
    let kms_encryption_context = supplied.kms_encryption_context.or_else(|| {
        stored
            .and_then(|stored| stored.kms_encryption_context.clone())
    });
    EncryptionConfiguration {
        kms_key_id: supplied.kms_key_id,
        kms_encryption_context,
    }
}

/// A project created by `CreateProject`, as `GetProject` reports it back.
#[derive(Debug, Clone)]
pub struct ProjectRecord {
    /// The identifier this service minted for the project — the one its `ProjectArn`
    /// carries, and the key it is stored under.
    pub project_id: String,
    /// The project's own ARN, built when it was created so every read reports the same
    /// one.
    pub project_arn: String,
    pub project_name: String,
    /// The project's LIVE configuration: the create request's, with every later update
    /// applied.
    pub configuration: ProjectConfiguration,
    pub tags: HashMap<String, String>,
    /// When this service created the project, reported as `CreationTime`. An update does
    /// not move it.
    pub created_at: SystemTime,
    /// When the project was last changed, reported as `LastModifiedTime`. Equal to
    /// [`Self::created_at`] until the first update.
    pub last_modified_at: SystemTime,
}

impl ProjectRecord {
    /// This project as the modelled `ProjectSummary`, which is how `ListProjects` reports
    /// it. ONE projection, from the same record `GetProject` answers from.
    #[must_use]
    pub fn summary(&self) -> ProjectSummary {
        ProjectSummary {
            project_arn: self.project_arn.clone(),
            project_name: Some(self.project_name.clone()),
            creation_time: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_at)),
        }
    }

    /// Every blueprint this project's custom configuration associates with it, primary and
    /// fallback alike.
    fn associated_blueprints(&self) -> impl Iterator<Item = &BlueprintItem> {
        self.configuration
            .custom
            .iter()
            .flat_map(|custom| custom.blueprints.iter().chain(custom.fallback_blueprints.iter().flatten()))
    }

    /// Whether this project is in scope for a `ListProjects` `BlueprintFilter`.
    ///
    /// The filter names a blueprint and optionally one of its versions, so a project is in
    /// scope when its custom configuration associates that blueprint with it. An
    /// association that pins no version uses whichever version is current
    /// (`BlueprintItem$BlueprintVersion`: "If omitted, the latest version is used"), so it
    /// is in scope for a filter naming any version of that blueprint.
    #[must_use]
    pub fn matches_blueprint_filter(&self, filter: &BlueprintFilter) -> bool {
        self.associated_blueprints().any(|item| {
            item.blueprint_arn == filter.blueprint_arn
                && filter.blueprint_version.as_deref().is_none_or(|wanted| {
                    item.blueprint_version
                        .as_deref()
                        .is_none_or(|pinned| pinned == wanted)
                })
        })
    }

    /// Whether `blueprint_arn` is associated with this project at all — what a
    /// `ListBlueprints` `ProjectFilter` selects on.
    #[must_use]
    pub fn associates(&self, blueprint_arn: &str) -> bool {
        self.associated_blueprints()
            .any(|item| item.blueprint_arn == blueprint_arn)
    }
}

// ---------------------------------------------------------------------------
// Blueprints
// ---------------------------------------------------------------------------

/// One immutable version of a blueprint, as `CreateBlueprintVersion` cut it.
///
/// "Creates a new, immutable version of an existing blueprint so that projects can pin to
/// a specific version" (`CreateBlueprintVersion`) — so a version keeps the schema and the
/// encryption configuration the blueprint carried when it was cut, whatever a later
/// `UpdateBlueprint` does to the blueprint itself.
#[derive(Debug, Clone)]
pub struct BlueprintVersionRecord {
    /// This version's number. `BlueprintVersion`'s modelled pattern is `^[0-9]*$`, so the
    /// wire form is this number in decimal.
    pub blueprint_version: u32,
    pub schema: String,
    pub encryption: Option<EncryptionConfiguration>,
    /// When the version was cut.
    pub created_at: SystemTime,
}

impl BlueprintVersionRecord {
    /// This version's `BlueprintVersion`, as the wire spells it.
    #[must_use]
    pub fn wire_version(&self) -> String {
        self.blueprint_version.to_string()
    }
}

/// A blueprint created by `CreateBlueprint`, as `GetBlueprint` reports it back.
#[derive(Debug, Clone)]
pub struct BlueprintRecord {
    /// The identifier this service minted — the one its `BlueprintArn` carries.
    pub blueprint_id: String,
    pub blueprint_arn: String,
    pub blueprint_name: String,
    /// The blueprint's LIVE schema: the create request's, with every later update applied.
    pub schema: String,
    /// The blueprint's LIVE encryption configuration, or `None` while nothing has set one
    /// — in which case Amazon Textract encrypts it with an AWS owned key and there is no
    /// `KmsKeyId` to report.
    pub encryption: Option<EncryptionConfiguration>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_modified_at: SystemTime,
    /// Every immutable version cut from this blueprint, keyed by its number. A `BTreeMap`
    /// so the latest version is the last key however the versions were cut.
    pub versions: BTreeMap<u32, BlueprintVersionRecord>,
    /// The number the next version cut from this blueprint takes. Starts at 1 and never
    /// goes back, so deleting a version does not let a later one reuse its number.
    pub next_version: u32,
}

impl BlueprintRecord {
    /// The highest-numbered version this blueprint holds, if it has been versioned.
    #[must_use]
    pub fn latest_version(&self) -> Option<&BlueprintVersionRecord> {
        self.versions.values().next_back()
    }

    /// The version a wire `BlueprintVersion` names.
    ///
    /// TOTAL over any `&str`: the value is caller-controlled, so a version that is not a
    /// number this service ever minted simply names no version.
    #[must_use]
    pub fn version(&self, wire: &str) -> Option<&BlueprintVersionRecord> {
        self.versions.get(&wire.parse::<u32>().ok()?)
    }

    /// This blueprint as the modelled `BlueprintSummary`, which is how `ListBlueprints`
    /// reports it.
    ///
    /// The summary describes the BLUEPRINT, so its timestamps are the blueprint's own and
    /// its `BlueprintVersion` is the latest version cut from it — the one a project pins
    /// when it names no version — or absent while it has never been versioned.
    #[must_use]
    pub fn summary(&self) -> BlueprintSummary {
        BlueprintSummary {
            blueprint_arn: self.blueprint_arn.clone(),
            blueprint_name: Some(self.blueprint_name.clone()),
            blueprint_version: self.latest_version().map(BlueprintVersionRecord::wire_version),
            resource_owner: Some(RESOURCE_OWNER_ACCOUNT.to_string()),
            creation_time: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_at)),
        }
    }

    /// The schema a document insights job extracts with when it pins `version` of this
    /// blueprint.
    ///
    /// `BlueprintItem$BlueprintVersion`: "If omitted, the latest version is used" — and a
    /// blueprint that has never been versioned has only its live schema.
    #[must_use]
    pub fn schema_for(&self, version: Option<&str>) -> Option<&str> {
        match version {
            Some(wire) => self.version(wire).map(|version| version.schema.as_str()),
            None => Some(
                self.latest_version()
                    .map_or(self.schema.as_str(), |version| version.schema.as_str()),
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Document insights jobs
// ---------------------------------------------------------------------------

/// What running a document insights job established.
#[derive(Debug, Clone)]
pub enum InsightsOutcome {
    /// The document was read, the job's output written to the location the request named,
    /// and everything the project asked for was carried out.
    Succeeded,
    /// The output was written and part of what the project asked for is a capability this
    /// emulator does not have, which `error_type` and `error_message` name: the modelled
    /// `PARTIAL_SUCCESS`, so nothing claims the rest was done.
    PartiallySucceeded {
        error_type: String,
        error_message: String,
    },
    /// The job could not be run to completion. `error_type` is the modelled Amazon
    /// Textract error that names the reason and `error_message` the sentence that service
    /// states it with, which is what `GetDocumentInsights` reports as `ErrorType` and
    /// `ErrorMessage`.
    Failed {
        error_type: String,
        error_message: String,
    },
}

impl InsightsOutcome {
    /// The `JobStatus` this outcome reports once the job has settled.
    #[must_use]
    pub fn as_job_status(&self) -> &'static str {
        match self {
            Self::Succeeded => JOB_STATUS_SUCCEEDED,
            Self::PartiallySucceeded { .. } => JOB_STATUS_PARTIAL_SUCCESS,
            Self::Failed { .. } => JOB_STATUS_FAILED,
        }
    }

    /// The `ErrorType` and `ErrorMessage` this outcome reports, for an outcome that carries
    /// a reason at all.
    #[must_use]
    pub fn reason(&self) -> Option<(&str, &str)> {
        match self {
            Self::Succeeded => None,
            Self::PartiallySucceeded {
                error_type,
                error_message,
            }
            | Self::Failed {
                error_type,
                error_message,
            } => Some((error_type, error_message)),
        }
    }
}

/// A document insights job started by `StartDocumentInsights`.
///
/// The reported `Status` is NOT stored: it is derived from [`Self::settles_at`] the way
/// every other asynchronous job of this service derives it, so a reporting call never has
/// to write state to move the job along.
#[derive(Debug, Clone)]
pub struct InsightsJob {
    /// The identifier this service minted — the one its `JobArn` carries.
    pub job_id: String,
    pub job_arn: String,
    /// The location the job's output was written to, reported back by
    /// `GetDocumentInsights` so a caller that kept only the `JobArn` can still find it.
    pub output_s3_uri: String,
    /// What running the job established.
    pub outcome: InsightsOutcome,
    pub tags: HashMap<String, String>,
    /// When the job was started. Read by the job store's eviction policy.
    pub created_at: SystemTime,
    /// The instant the `IN_PROGRESS` -> terminal transition became due.
    pub settles_at: SystemTime,
}

impl InsightsJob {
    /// The job's status at `now`.
    #[must_use]
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.settles_at {
            self.outcome.as_job_status()
        } else {
            JOB_STATUS_IN_PROGRESS
        }
    }

    /// The job's `ErrorType` and `ErrorMessage` at `now`.
    ///
    /// Reported only once the job has settled into an outcome that carries a reason: both
    /// members describe the error that stopped part of the job, and a job still running has
    /// not been stopped by anything.
    #[must_use]
    pub fn failure(&self, now: SystemTime) -> Option<(String, String)> {
        if now < self.settles_at {
            return None;
        }
        self.outcome
            .reason()
            .map(|(error_type, message)| (error_type.to_string(), message.to_string()))
    }
}

// ---------------------------------------------------------------------------
// The store
// ---------------------------------------------------------------------------

/// What one `ClientRequestToken` slot holds: the answer a replay of that token is given,
/// and when the slot was remembered.
#[derive(Debug, Clone)]
pub struct IdempotencySlot {
    /// A resource's ARN, or a blueprint's ARN and a version number joined by
    /// [`VERSION_SLOT_SEPARATOR`].
    pub answer: String,
    /// When the slot was remembered. Read by the slot store's eviction policy.
    pub remembered_at: SystemTime,
}

impl ComAmazonawsTextractTextractState {
    /// Mint an identifier for a project, a blueprint or a job: [`RESOURCE_ID_LEN`] hex
    /// characters, inside the `[a-zA-Z0-9-]{12,36}` the ARN patterns allow.
    ///
    /// ONE identifier space for the three kinds, so a minted identifier collides with
    /// nothing this service holds and an ARN of one kind can never resolve to another's
    /// resource. Re-mints on the (practically impossible) collision.
    pub fn mint_intelligence_id(&mut self, seed: &str) -> String {
        loop {
            self.next_intelligence_seq = self.next_intelligence_seq.wrapping_add(1);
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            let digest = hex::encode(Sha256::digest(
                format!("{seed}|{}|{nanos}", self.next_intelligence_seq).as_bytes(),
            ));
            let id: String = digest.chars().take(RESOURCE_ID_LEN).collect();
            if !self.projects.contains_key(&id)
                && !self.blueprints.contains_key(&id)
                && !self.insights_jobs.contains_key(&id)
            {
                return id;
            }
        }
    }

    /// The ARN a `ClientRequestToken` already answered for `operation`, if that token has
    /// been used and the resource it named still exists.
    ///
    /// AWS, on every `@idempotencyToken` of this control plane: "If you use the same token
    /// for a request that matches a previous request, the service ignores the request, but
    /// does not return an error." So a replayed token answers with what the first call
    /// created. A token whose resource is gone resolves to `None`, so it can be used again
    /// rather than answering with an ARN that no longer reads back.
    #[must_use]
    pub fn intelligence_token_answer(&self, operation: &str, token: &str) -> Option<String> {
        let arn = &self
            .intelligence_idempotency
            .get(&token_slot(operation, token))?
            .answer;
        let holds = resource_id_of(arn, PROJECT_RESOURCE_TYPE)
            .is_some_and(|id| self.projects.contains_key(&id))
            || resource_id_of(arn, BLUEPRINT_RESOURCE_TYPE)
                .is_some_and(|id| self.blueprints.contains_key(&id));
        holds.then(|| arn.clone())
    }

    /// Remember that `operation`'s `token` answered with `arn`.
    pub fn remember_intelligence_token(&mut self, operation: &str, token: &str, arn: &str) {
        self.remember_slot(token_slot(operation, token), arn.to_string());
    }

    /// Store one `ClientRequestToken` slot, making room first when the store is full.
    ///
    /// The slot key is the caller's own token, so the store is BOUNDED by
    /// [`MAX_STORED_IDEMPOTENCY_SLOTS`] and the least recently remembered slot is evicted
    /// to make room. Evicting a slot whose resource is still live only makes that token
    /// usable again — the resource itself is untouched — which is the trade a bounded store
    /// makes.
    fn remember_slot(&mut self, slot: String, answer: String) {
        while self.intelligence_idempotency.len() >= MAX_STORED_IDEMPOTENCY_SLOTS {
            let oldest = self
                .intelligence_idempotency
                .iter()
                .min_by_key(|(_, held)| held.remembered_at)
                .map(|(key, _)| key.clone());
            match oldest {
                Some(key) => {
                    self.intelligence_idempotency.remove(&key);
                }
                None => break,
            }
        }
        self.intelligence_idempotency.insert(
            slot,
            IdempotencySlot {
                answer,
                remembered_at: SystemTime::now(),
            },
        );
    }

    /// Forget every `ClientRequestToken` slot that answered with `arn`, or with a version
    /// of it.
    ///
    /// Called wherever a resource is removed, so a slot never outlives the resource it
    /// named: one that did would answer with an ARN that no longer reads back, and would
    /// let the slot store grow past the resources it describes however often a caller
    /// created and deleted under fresh tokens.
    fn forget_slots_naming(&mut self, arn: &str) {
        self.intelligence_idempotency.retain(|_, slot| {
            slot.answer != arn
                && slot
                    .answer
                    .rsplit_once(VERSION_SLOT_SEPARATOR)
                    .is_none_or(|(named, _)| named != arn)
        });
    }

    /// The blueprint and version a `CreateBlueprintVersion` `ClientRequestToken` already
    /// answered with, if that version still exists.
    ///
    /// A version is not addressed by an ARN of its own, so the slot holds the blueprint's
    /// ARN and the version number together; a token whose version is gone resolves to
    /// `None`, so it can be used again rather than answering with a version that no longer
    /// reads back.
    #[must_use]
    pub fn blueprint_version_token_answer(
        &self,
        operation: &str,
        token: &str,
    ) -> Option<(String, String)> {
        let stored = &self
            .intelligence_idempotency
            .get(&token_slot(operation, token))?
            .answer;
        let (arn, version) = stored.rsplit_once(VERSION_SLOT_SEPARATOR)?;
        self.blueprint_by_arn(arn)?.version(version)?;
        Some((arn.to_string(), version.to_string()))
    }

    /// Remember that `operation`'s `token` answered with `version` of `blueprint_arn`.
    pub fn remember_blueprint_version_token(
        &mut self,
        operation: &str,
        token: &str,
        blueprint_arn: &str,
        version: &str,
    ) {
        self.remember_slot(
            token_slot(operation, token),
            format!("{blueprint_arn}{VERSION_SLOT_SEPARATOR}{version}"),
        );
    }

    /// The project `arn` names, if this service holds it.
    #[must_use]
    pub fn project_by_arn(&self, arn: &str) -> Option<&ProjectRecord> {
        self.projects.get(&resource_id_of(arn, PROJECT_RESOURCE_TYPE)?)
    }

    /// [`Self::project_by_arn`] for a caller that writes it — the same resolution, so a
    /// write and a read never disagree about which ARN names which project.
    pub fn project_by_arn_mut(&mut self, arn: &str) -> Option<&mut ProjectRecord> {
        self.projects
            .get_mut(&resource_id_of(arn, PROJECT_RESOURCE_TYPE)?)
    }

    /// Remove the project `arn` names, with every `ClientRequestToken` slot that answered
    /// with it, and hand the removed record back.
    pub fn remove_project(&mut self, arn: &str) -> Option<ProjectRecord> {
        let removed = self
            .projects
            .remove(&resource_id_of(arn, PROJECT_RESOURCE_TYPE)?)?;
        self.forget_slots_naming(&removed.project_arn.clone());
        Some(removed)
    }

    /// Whether this account already holds a project called `project_name`.
    #[must_use]
    pub fn project_name_taken(&self, project_name: &str) -> bool {
        self.projects
            .values()
            .any(|project| project.project_name == project_name)
    }

    /// The blueprint `arn` names, if this service holds it.
    #[must_use]
    pub fn blueprint_by_arn(&self, arn: &str) -> Option<&BlueprintRecord> {
        self.blueprints
            .get(&resource_id_of(arn, BLUEPRINT_RESOURCE_TYPE)?)
    }

    /// [`Self::blueprint_by_arn`] for a caller that writes it.
    pub fn blueprint_by_arn_mut(&mut self, arn: &str) -> Option<&mut BlueprintRecord> {
        self.blueprints
            .get_mut(&resource_id_of(arn, BLUEPRINT_RESOURCE_TYPE)?)
    }

    /// Remove the blueprint `arn` names, with every version it holds and every
    /// `ClientRequestToken` slot that answered with it or with one of those versions.
    ///
    /// AWS, on `DeleteBlueprintRequest$BlueprintVersion`: "If omitted, the entire
    /// blueprint and all of its versions are deleted." — a version is a child of its
    /// blueprint, so it goes away with it.
    pub fn remove_blueprint(&mut self, arn: &str) -> Option<BlueprintRecord> {
        let removed = self
            .blueprints
            .remove(&resource_id_of(arn, BLUEPRINT_RESOURCE_TYPE)?)?;
        self.forget_slots_naming(&removed.blueprint_arn.clone());
        Some(removed)
    }

    /// Remove ONE immutable version of the blueprint `arn` names, with the
    /// `ClientRequestToken` slot that answered with it, leaving the blueprint and its other
    /// versions.
    ///
    /// The two ways this cannot be done are kept apart, because they are different
    /// refusals: an ARN that names no blueprint here, and a version its blueprint never
    /// had.
    pub fn remove_blueprint_version(
        &mut self,
        arn: &str,
        version: &str,
    ) -> Result<BlueprintVersionRecord, ResourceNotFoundException> {
        let Some(blueprint) = self.blueprint_by_arn_mut(arn) else {
            return Err(blueprint_not_found());
        };
        // The slot holds the STORED ARN, which is the one the blueprint was created under.
        let blueprint_arn = blueprint.blueprint_arn.clone();
        let removed = version
            .parse::<u32>()
            .ok()
            .and_then(|number| blueprint.versions.remove(&number));
        let Some(removed) = removed else {
            return Err(blueprint_version_not_found());
        };
        let answered = format!("{blueprint_arn}{VERSION_SLOT_SEPARATOR}{version}");
        self.intelligence_idempotency
            .retain(|_, slot| slot.answer != answered);
        Ok(removed)
    }

    /// Whether this account already holds a blueprint called `blueprint_name`.
    #[must_use]
    pub fn blueprint_name_taken(&self, blueprint_name: &str) -> bool {
        self.blueprints
            .values()
            .any(|blueprint| blueprint.blueprint_name == blueprint_name)
    }

    /// The document insights job `arn` names, if this service holds it.
    #[must_use]
    pub fn insights_job_by_arn(&self, arn: &str) -> Option<&InsightsJob> {
        self.insights_jobs
            .get(&resource_id_of(arn, INSIGHTS_JOB_RESOURCE_TYPE)?)
    }

    /// Store `job` under its own identifier, evicting the oldest job when the store is
    /// full.
    pub fn insert_insights_job(&mut self, job: InsightsJob) {
        while self.insights_jobs.len() >= MAX_STORED_INSIGHTS_JOBS {
            let oldest = self
                .insights_jobs
                .iter()
                .min_by_key(|(_, job)| job.created_at)
                .map(|(id, _)| id.clone());
            match oldest {
                Some(id) => {
                    self.insights_jobs.remove(&id);
                }
                None => break,
            }
        }
        self.insights_jobs.insert(job.job_id.clone(), job);
    }

    /// Every stored project a `ListProjects` request admits, in the ONE total order its
    /// pages are cut from.
    ///
    /// A project is listed exactly when the request's `BlueprintFilter` admits it (a
    /// request naming none filtering nothing out). AWS documents no order for the listing
    /// and an offset cursor is only coherent over a stable one, so projects are reported
    /// oldest first, with the project's identifier breaking ties between projects created
    /// in the same instant.
    #[must_use]
    pub fn listed_projects(&self, filter: Option<&BlueprintFilter>) -> Vec<ProjectSummary> {
        let mut selected: Vec<&ProjectRecord> = self
            .projects
            .values()
            .filter(|project| {
                filter.is_none_or(|filter| project.matches_blueprint_filter(filter))
            })
            .collect();
        selected.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.project_id.cmp(&right.project_id))
        });
        selected.into_iter().map(ProjectRecord::summary).collect()
    }

    /// Check that every blueprint `items` associates exists here, with the version each
    /// one pins.
    ///
    /// The ONE resolution of a `BlueprintItems` list, so a project's custom configuration
    /// and a job's `Blueprints` cannot disagree about which blueprints exist: a reference
    /// to a blueprint this account does not hold is the modelled
    /// `ResourceNotFoundException`, and so is one pinning a version its blueprint never
    /// had.
    pub fn blueprints_resolve(
        &self,
        items: &[BlueprintItem],
    ) -> Result<(), ResourceNotFoundException> {
        self.blueprint_schemas(items).map(|_| ())
    }

    /// The schema each of `items` extracts with, in the order they were given.
    pub fn blueprint_schemas(
        &self,
        items: &[BlueprintItem],
    ) -> Result<Vec<String>, ResourceNotFoundException> {
        items
            .iter()
            .map(|item| {
                let blueprint = self
                    .blueprint_by_arn(&item.blueprint_arn)
                    .ok_or_else(blueprint_not_found)?;
                blueprint
                    .schema_for(item.blueprint_version.as_deref())
                    .map(str::to_string)
                    .ok_or_else(blueprint_version_not_found)
            })
            .collect()
    }

    /// [`Self::blueprints_resolve`] over every blueprint a project's configuration
    /// associates with it, primary and fallback alike.
    pub fn project_blueprints_resolve(
        &self,
        configuration: &ProjectConfiguration,
    ) -> Result<(), ResourceNotFoundException> {
        let Some(custom) = &configuration.custom else {
            return Ok(());
        };
        self.blueprints_resolve(&custom.blueprints)?;
        if let Some(fallbacks) = &custom.fallback_blueprints {
            self.blueprints_resolve(fallbacks)?;
        }
        Ok(())
    }

    /// Every stored blueprint a `ListBlueprints` request admits, in the ONE total order
    /// its pages are cut from.
    ///
    /// The three filters narrow independently, each absent one filtering nothing out:
    /// `BlueprintArn` selects the one blueprint that ARN names, `ProjectFilter` the
    /// blueprints a project associates with itself, and `ResourceOwner` the blueprints
    /// owned by this account or published by Amazon Textract — and this emulator mints
    /// none of the latter, so `SERVICE` selects nothing. Order as in
    /// [`Self::listed_projects`].
    #[must_use]
    pub fn listed_blueprints(
        &self,
        blueprint_arn: Option<&str>,
        project_filter: Option<&ProjectFilter>,
        resource_owner: Option<&str>,
    ) -> Vec<BlueprintSummary> {
        if resource_owner.is_some_and(|owner| owner != RESOURCE_OWNER_ACCOUNT) {
            return Vec::new();
        }
        let associated = project_filter.and_then(|filter| self.project_by_arn(&filter.project_arn));
        let mut selected: Vec<&BlueprintRecord> = self
            .blueprints
            .values()
            .filter(|blueprint| {
                blueprint_arn.is_none_or(|arn| blueprint.blueprint_arn == arn)
                    && associated.is_none_or(|project| project.associates(&blueprint.blueprint_arn))
            })
            .collect();
        selected.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.blueprint_id.cmp(&right.blueprint_id))
        });
        selected
            .into_iter()
            .map(BlueprintRecord::summary)
            .collect()
    }
}

/// What separates a blueprint's ARN from a version number inside one idempotency slot.
///
/// An ARN never carries it, and a `BlueprintVersion` is digits only, so the pair is read
/// back unambiguously from its last occurrence.
const VERSION_SLOT_SEPARATOR: char = '#';

/// The slot a `ClientRequestToken` occupies: the operation it was sent to, then the token.
///
/// The operation is length-prefixed rather than interpolated bare, so a token carrying the
/// separator cannot be made to name another operation's slot.
fn token_slot(operation: &str, token: &str) -> String {
    format!("{}:{operation}|{token}", operation.len())
}

// ---------------------------------------------------------------------------
// Pagination scopes
// ---------------------------------------------------------------------------

/// One caller-supplied value as a cursor scope names it: length-prefixed, so a value
/// carrying the separator cannot forge another listing's scope. `-` for an absent one.
fn scoped(value: Option<&str>) -> String {
    value.map_or_else(
        || "-".to_string(),
        |value| format!("{}:{value}", value.len()),
    )
}

/// The cursor scope a `ListProjects` listing's pages are bound to: the filtration criteria
/// the listing was computed under.
///
/// A `NextToken` is an offset into ONE list, so a cursor cut under one filter names nothing
/// in another: changing the filter invalidates the cursor instead of silently indexing a
/// different list. `MaxResults` is deliberately absent — a caller may change the page size
/// between pages.
#[must_use]
pub fn project_page_scope(filter: Option<&BlueprintFilter>) -> String {
    format!(
        "ListProjects|{}|{}",
        scoped(filter.map(|filter| filter.blueprint_arn.as_str())),
        scoped(filter.and_then(|filter| filter.blueprint_version.as_deref())),
    )
}

/// The cursor scope a `ListBlueprints` listing's pages are bound to. Same rule as
/// [`project_page_scope`], over that listing's three filters.
#[must_use]
pub fn blueprint_page_scope(
    blueprint_arn: Option<&str>,
    project_filter: Option<&ProjectFilter>,
    resource_owner: Option<&str>,
) -> String {
    format!(
        "ListBlueprints|{}|{}|{}",
        scoped(blueprint_arn),
        scoped(project_filter.map(|filter| filter.project_arn.as_str())),
        scoped(resource_owner),
    )
}

// ---------------------------------------------------------------------------
// Validation the model cannot express
// ---------------------------------------------------------------------------

/// Whether `values` names each of its entries only once.
fn each_once(values: &[String]) -> bool {
    let distinct: BTreeSet<&str> = values.iter().map(String::as_str).collect();
    distinct.len() == values.len()
}

/// Check an `ExtractionConfiguration` against the rules AWS states in prose, which the
/// model's own constraints cannot express.
///
/// `ExtractionGranularity$Types` and `OutputTextFormat$Types`: "Each type can appear only
/// once." `OutputTextFormat$Types` again: "CSV can't be the only type."
pub fn validate_extraction(extraction: &ExtractionConfiguration) -> Result<(), &'static str> {
    if let Some(types) = &extraction.granularity.types {
        if !each_once(types) {
            return Err(DUPLICATE_TYPE_MESSAGE);
        }
    }
    if let Some(types) = &extraction.output_format.text_format.types {
        if !each_once(types) {
            return Err(DUPLICATE_TYPE_MESSAGE);
        }
        if types.iter().all(|value| value == super::insights::TEXT_FORMAT_CSV) {
            return Err(CSV_ONLY_TEXT_FORMAT_MESSAGE);
        }
    }
    Ok(())
}

/// Check a `SensitiveDataConfiguration`: `PIIEntitiesConfiguration$PiiEntityTypes` is
/// "[t]he list of PII entity types to detect. Each type can appear only once."
pub fn validate_sensitive_data(
    sensitive_data: &SensitiveDataConfiguration,
) -> Result<(), &'static str> {
    match &sensitive_data.pii_entities_configuration {
        Some(entities) if !each_once(&entities.pii_entity_types) => Err(DUPLICATE_TYPE_MESSAGE),
        _ => Ok(()),
    }
}

/// Check a `CustomConfiguration`: a fallback blueprint "[r]equires at least one blueprint
/// in `Blueprints`" (`CustomConfiguration$FallbackBlueprints`).
pub fn validate_custom(custom: &CustomConfiguration) -> Result<(), &'static str> {
    let fallbacks = custom
        .fallback_blueprints
        .as_ref()
        .map_or(0, |fallbacks| fallbacks.len());
    if fallbacks > 0 && custom.blueprints.is_empty() {
        return Err(FALLBACK_WITHOUT_BLUEPRINT_MESSAGE);
    }
    Ok(())
}

/// Whether `schema` is the JSON object a blueprint schema must be.
///
/// A blueprint "defines the schema Amazon Textract uses to extract structured data"
/// (`CreateBlueprint`) and its `Schema` "defines the fields to extract and their expected
/// structure", so the value is read with `serde_json` — nothing here hand-reads it — and a
/// value that is not an object describes no extraction at all.
#[must_use]
pub fn schema_is_an_object(schema: &str) -> bool {
    matches!(
        ::serde_json::from_str::<::serde_json::Value>(schema),
        Ok(::serde_json::Value::Object(_))
    )
}

/// Check every configuration a project can carry, in one place, so `CreateProject` and
/// `UpdateProject` cannot enforce different rules.
pub fn validate_project_configuration(
    configuration: &ProjectConfiguration,
) -> Result<(), &'static str> {
    if let Some(extraction) = &configuration.extraction {
        validate_extraction(extraction)?;
    }
    if let Some(sensitive_data) = &configuration.sensitive_data {
        validate_sensitive_data(sensitive_data)?;
    }
    if let Some(custom) = &configuration.custom {
        validate_custom(custom)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the `ClientRequestToken` slot store is bounded, so a caller that spends a
    /// fresh token on every request cannot grow this service's state without end — the
    /// least recently remembered slot is evicted to make room.
    #[test]
    fn the_token_slot_store_is_bounded() {
        let mut store = ComAmazonawsTextractTextractState::default();
        let arn = project_arn("us-east-1", &"a".repeat(32));
        // Past the ceiling by enough rounds that eviction has to have happened more than
        // once, and the slot the LAST round remembered must still resolve.
        for index in 0..(MAX_STORED_IDEMPOTENCY_SLOTS + 8) {
            store.remember_intelligence_token("CreateProject", &format!("token-{index}"), &arn);
            let held = store.intelligence_idempotency.len();
            assert!(
                held <= MAX_STORED_IDEMPOTENCY_SLOTS,
                "case {index}: the slot store must hold at most {MAX_STORED_IDEMPOTENCY_SLOTS} \
                 slots, holding {held}"
            );
        }
        let last = format!("token-{}", MAX_STORED_IDEMPOTENCY_SLOTS + 7);
        assert!(
            store
                .intelligence_idempotency
                .contains_key(&token_slot("CreateProject", &last)),
            "case {last:?}: the slot just remembered must be the one that is kept"
        );
    }
}
