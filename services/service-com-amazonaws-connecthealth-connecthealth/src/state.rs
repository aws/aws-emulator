// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-connecthealth-connecthealth service.
//!
//! The Domain -> Subscription lifecycle lives here. Every field a response carries is
//! read back OUT of these records: a handler persists what it was given, derives what
//! the service itself owns (ids, ARNs, timestamps, status), and then renders the stored
//! record. Nothing is echoed straight from a request.
//!
//! Helpers several operations share (id minting, ARN building, the opaque pagination
//! cursor, the one status step a `Subscription` transition takes, and every response
//! projection — `SubscriptionDescription` and the create's own same-member-set
//! `CreateSubscriptionOutput`,
//! `WebAppConfiguration`, `EncryptionContext`, and the recorded `accountId` /
//! `onboardingGeneration` pair) live here rather than being copied
//! into each `src/ops/*.rs`: `src/lib.rs` is generated, so this module is the crate's
//! one shared, editable home.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    CreateSubscriptionOutput, CreateWebAppConfiguration, EncryptionContext, InputDataConfig,
    InsightsContext, InsightsOutput, InternalServerException, MedicalScribeChannelDefinition,
    MedicalScribeListeningSessionDetails, MedicalScribePostStreamActionSettingsResponse,
    MedicalScribePostStreamActionsResult, OutputDataConfig, PatientInsightsEncounterContext,
    PatientInsightsPatientContext, SubscriptionDescription, UserContext, ValidationException,
    WebAppConfiguration,
};

/// The region an emulated ARN carries when the request names none.
const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace / SigV4 signing name (`OPERATIONS.md`, "Wire protocol").
const ARN_NAMESPACE: &str = "health-agent";

/// The ARN namespace of IAM Identity Center, where the application a domain's web app
/// runs as lives — another service's namespace, not this one's.
const IDC_ARN_NAMESPACE: &str = "sso";

/// How long a `Domain` stays in `DELETING` before the deletion the clock owes is
/// materialised — the record is purged, and `GetDomain` then answers
/// `ResourceNotFoundException`.
///
/// AWS documents no duration for this, so the span is CHOSEN, and chosen inside the
/// emulator's transient window (250-750 ms): a caller that reads straight after
/// `DeleteDomain` still observes the `DELETING` state rather than an instant purge.
///
/// Only a domain whose deletion CANNOT finish inside the call waits this span — see
/// [`Domain::deletion_completes_in_call`].
pub const DOMAIN_DELETION_SPAN: Duration = Duration::from_millis(500);

/// The most `Domain` records ONE ACCOUNT holds at a time IN ONE REGION — the cap
/// `CreateDomain` is subject to.
///
/// RECORDED behaviour: `CreateDomain` past the cap is refused with a
/// `ValidationException` whose text names the account and this number
/// ("Account <accountId> has reached the maximum limit of 10 domains."). The cap is a
/// LIVE count of the domains the account currently holds in the Region the create is
/// addressed to, not a monotonic counter and not a cross-Region total, so a
/// `DeleteDomain` frees a slot for the next create and a domain in another Region
/// consumes none of this Region's quota.
///
/// It is a quota, NOT this store's growth bound: the Region a record is filed under is
/// read from the request, so a count per Region is a count per value a caller chose.
/// [`MAX_DOMAINS_HELD`] is what bounds the store.
pub const MAX_DOMAINS_PER_ACCOUNT: usize = 10;

/// How many distinct Regions this store files `Domain` records under.
///
/// The EMULATOR's own ceiling, not a rule of the service: it is set above the number of
/// Regions AWS operates, so no use of the emulator across real Regions reaches it.
pub const MAX_DOMAIN_REGIONS: usize = 64;

/// The most `Domain` records this store holds at once, across every account and every
/// Region: [`MAX_DOMAINS_PER_ACCOUNT`] in each of [`MAX_DOMAIN_REGIONS`] Regions.
///
/// THIS is what bounds the store's growth, and it is an absolute count rather than a
/// count per key read off the request: a caller looping `CreateDomain` cannot grow the
/// shared store past it whatever Region, and however many Regions, it signs for. It
/// bounds everything held UNDER a domain with it, since a domain holds at most
/// [`MAX_SUBSCRIPTIONS_PER_DOMAIN`] subscriptions and
/// [`MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN`] jobs, and a tag entry is keyed by the ARN of
/// one of those records.
///
/// Enforced in `CreateDomain`, the one operation that adds a `Domain`, which refuses
/// with the `ServiceQuotaExceededException` its model declares once the store is full.
pub const MAX_DOMAINS_HELD: usize = MAX_DOMAINS_PER_ACCOUNT * MAX_DOMAIN_REGIONS;

/// How many `domainId`s this service remembers having allocated, beyond the domains it
/// currently holds.
///
/// A delete needs to tell an id this service once allocated — whose domain is simply
/// gone — from one that was never allocated to anybody, because the two answer
/// DIFFERENT errors (see `DeleteDomain`'s handler, which cites the recording for each).
/// The history is therefore kept past the domain's own purge, and BOUNDED here: a caller
/// looping create/delete would otherwise grow the store without limit, which is the one
/// thing every ceiling in this module exists to prevent.
///
/// The bound is the EMULATOR's retention, not a rule of the service: once the history is
/// full the oldest id is dropped, and a delete of that id then answers as it did before
/// the id was ever minted. It is set two orders of magnitude above
/// [`MAX_DOMAINS_PER_ACCOUNT`], so an account has to create and delete this many domains
/// before its first id is forgotten.
pub const MAX_DOMAIN_ALLOCATION_HISTORY: usize = 1024;

/// The most `Subscription` records one `Domain` holds, so the product
/// (domains x subscriptions) is bounded too.
pub const MAX_SUBSCRIPTIONS_PER_DOMAIN: usize = 100;

/// The most `PatientInsightsJob` records one `Domain` holds, so the product
/// (domains x jobs) is bounded as well: a caller looping `StartPatientInsightsJob`
/// cannot grow the shared store without limit. A job record stays reachable for as long
/// as its domain does, so nothing drops one behind a caller's back; past this count a
/// start is refused instead (see `StartPatientInsightsJob`).
pub const MAX_PATIENT_INSIGHTS_JOBS_PER_DOMAIN: usize = 100;

/// The most `ScribeSession` records one `Subscription` holds, so the product
/// (domains x subscriptions x sessions) is bounded as well: a caller looping
/// `StartMedicalScribeListeningSession` with a fresh `sessionId` each time cannot grow
/// the shared store without limit. A session record stays reachable for as long as its
/// subscription does, so nothing drops one behind a caller's back; past this count a
/// start is refused instead (see `StartMedicalScribeListeningSession`).
pub const MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION: usize = 100;

/// The most event-stream frames ONE `StartMedicalScribeListeningSession` request's
/// input stream carries.
///
/// A caller chooses both how many frames a body holds and how small each one is, so
/// decoding them is work a caller sizes: a body the transport accepts still holds
/// hundreds of thousands of tiny frames. The count is refused before the frames are
/// read rather than after.
pub const MAX_SCRIBE_STREAM_EVENTS: usize = 1024;

/// How long a patient insights job sits in ONE non-terminal `JobStatus` before the
/// transition the clock owes it is materialised (`SUBMITTED` -> `IN_PROGRESS`, then
/// `IN_PROGRESS` -> a terminal status).
///
/// AWS documents no duration for a patient insights job, so the span is CHOSEN, and
/// chosen inside the emulator's transient window (250-750 ms): a caller that reads
/// straight after `StartPatientInsightsJob` still observes `SUBMITTED` rather than a job
/// that was already finished by the time the start returned.
pub const PATIENT_INSIGHTS_JOB_STEP_SPAN: Duration = Duration::from_millis(400);

/// The MODELLED length bound of an S3 URI this service reports or accepts: `S3Uri` is
/// `@length max 1024` (`smithy_query(query="shape",shape="S3Uri,InsightsOutput")`), which
/// holds for `InsightsOutput$uri` as much as for `OutputDataConfig$s3OutputPath`.
/// Measured in CHARACTERS, as `@length` on a string is defined.
///
/// Stated here because [`patient_insights_output_uri`] DERIVES one S3 URI from another and
/// so has to stay inside the bound the member it fills declares.
pub const MAX_S3_URI_LEN: usize = 1024;

/// The MODELLED `@pattern` of an S3 URI this service accepts: `S3Uri` is
/// `^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$`
/// (`smithy_query(query="shape",shape="S3Uri,OutputDataConfig")`) — an `s3://` scheme, a
/// bucket name of 3 to 63 characters in the lowercase DNS-ish alphabet the shape names,
/// and an optional key path after the separator.
///
/// Stated here because `OutputDataConfig$s3OutputPath` is judged by the handler rather
/// than by the boundary walk (`skip_validation` in `crate::ops::types::shared_types`), so
/// the rule the boundary would have applied is applied here instead — the pattern SOURCE
/// verbatim, run by the same regex engine the framework's own `@pattern` check uses, so
/// the two cannot judge one value differently.
pub const S3_URI_PATTERN: &str = r"^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$";

/// Whether `s3_output_path` satisfies the rules `OutputDataConfig$s3OutputPath`'s shape
/// declares: within [`MAX_S3_URI_LEN`] characters and matching [`S3_URI_PATTERN`].
///
/// The compiled pattern is built once and shared ([`std::sync::LazyLock`]), as the
/// framework's own `@pattern` check caches its regexes, so one request never pays for a
/// compile. A pattern that does not compile is a bug in the constant above, not in the
/// request, so it is treated as matching — exactly what the framework's check does with
/// an unparseable model pattern, rather than refusing every caller.
pub fn output_location_within_shape(s3_output_path: &str) -> bool {
    static S3_URI: std::sync::LazyLock<Option<::regex::Regex>> =
        std::sync::LazyLock::new(|| ::regex::Regex::new(S3_URI_PATTERN).ok());
    s3_output_path.chars().count() <= MAX_S3_URI_LEN
        && S3_URI
            .as_ref()
            .is_none_or(|pattern| pattern.is_match(s3_output_path))
}

/// The most tag entries one resource ARN carries — AWS's standard tagging limit of 50,
/// which bounds the set a request may leave a resource holding. `TagMap` itself declares
/// no `@length`, so this count is the SERVICE's own rule.
///
/// Stated here, compared in [`crate::service::tag_bounds::merged_within_cap`] — the one
/// place every operation that accepts a `TagMap` bounds a resource's tag set, so a
/// create and a merge cannot bound different counts.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The MODELLED length bounds of one tag entry, both ends of each: `TagKey` is
/// `@length 1..128` and `TagValue` is `@length 0..256`
/// (`smithy_query(query="shape",shape="TagMap,TagKey,TagValue")`), which are also AWS's
/// standard tagging limits. Measured in CHARACTERS, as `@length` on a string is defined.
///
/// Stated here, enforced in [`crate::service::tag_bounds`] — the one check every
/// operation that accepts a `TagMap` runs, because a constraint on a map's key or value
/// is not one the framework's boundary walk refuses.
pub const MIN_TAG_KEY_LEN: usize = 1;
pub const MAX_TAG_KEY_LEN: usize = 128;
pub const MIN_TAG_VALUE_LEN: usize = 0;
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The tag-key prefix AWS keeps for the tags IT sets, which a caller may therefore
/// neither create nor overwrite: a request whose `tags` map carries a key starting with
/// it is refused, so no caller-set entry can pass for an AWS-managed one. `TagKey`
/// carries no `@pattern` (`smithy_query(query="shape",shape="TagKey")`), so this is a
/// SERVICE-side rule, stated here and enforced — like the lengths above — in
/// [`crate::service::tag_bounds`], the one check every operation that accepts a `TagMap`
/// runs.
///
/// Matched without regard to ASCII case, the way the reserved namespace is defined: the
/// prefix is reserved whatever case the caller spells it in.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// A `Domain`'s lifecycle, as the model's `DomainStatus` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DomainStatus {
    Active,
    Deleting,
    /// The TERMINAL state: the deletion has been applied. A domain reaches it only as
    /// the record is purged — either within the deleting call (when nothing outside
    /// this service had to be torn down) or when the `DELETING` span falls due — so a
    /// `DELETED` domain is gone rather than sitting in the store.
    Deleted,
}

impl DomainStatus {
    /// The wire value — one of `types::DOMAIN_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Deleting => "DELETING",
            Self::Deleted => "DELETED",
        }
    }

    /// The status a wire value names, or `None` when it names none.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ACTIVE" => Some(Self::Active),
            "DELETING" => Some(Self::Deleting),
            "DELETED" => Some(Self::Deleted),
            _ => None,
        }
    }
}

/// A `Subscription`'s lifecycle, as the model's `SubscriptionStatus` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubscriptionStatus {
    Active,
    Inactive,
    Deleted,
}

impl SubscriptionStatus {
    /// The wire value — one of `types::SUBSCRIPTION_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
            Self::Deleted => "DELETED",
        }
    }
}

/// Which provisioning generation a `Domain` was onboarded through.
///
/// RECORDED behaviour, not modelled: no shape in this model declares an
/// `onboardingGeneration` member (`smithy_query(query="shape",
/// shape="CreateDomainOutput,GetDomainOutput")` lists neither), while every recorded
/// domain response carries the discriminator a client branches on to decide which
/// provisioning path a domain uses. It is a property of the domain, fixed when the
/// domain is created and stored on the record, never recomputed per read.
///
/// This emulator provisions a domain through exactly one path — the legacy web-app /
/// Identity Center onboarding every `CreateDomain` input in this model describes
/// (`webAppSetupConfiguration` naming an Identity Center instance) — so `LEGACY` is
/// the generation it records. The enum, rather than a bare string, is what keeps the
/// value a closed discriminator with room for a later generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnboardingGeneration {
    Legacy,
}

impl OnboardingGeneration {
    /// The wire value.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Legacy => "LEGACY",
        }
    }
}

/// How a `Domain`'s data is encrypted, as the model's `EncryptionType` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncryptionType {
    AwsOwnedKey,
    CustomerManagedKey,
}

impl EncryptionType {
    /// The wire value — one of `types::ENCRYPTION_TYPE_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::AwsOwnedKey => "AWS_OWNED_KEY",
            Self::CustomerManagedKey => "CUSTOMER_MANAGED_KEY",
        }
    }
}

/// The stored web-application configuration of a `Domain`, derived once from the
/// `CreateWebAppConfiguration` the caller supplied: the Identity Center *instance* the
/// caller named, and the Identity Center *application* this service created in it.
#[derive(Debug, Clone)]
pub struct WebAppConfigurationRecord {
    pub ehr_role: String,
    /// The Identity Center INSTANCE this domain's web application was created in — the
    /// one the create named. An application is identified within its instance, so the
    /// instance is part of the application's identity and is kept on the record rather
    /// than discarded once the application has been created.
    pub idc_instance_id: String,
    /// The application this service created for the domain, as its own id within
    /// [`Self::idc_instance_id`] (`apl-` plus 16 hex). The identifier the responses
    /// report is this id in that instance — see [`Self::projected`].
    pub idc_application_suffix: String,
    pub idc_region: String,
}

impl WebAppConfigurationRecord {
    /// The record as the modelled `WebAppConfiguration` every Domain response reports
    /// (`smithy_query(query="shape", shape="WebAppConfiguration")`: `ehrRole`,
    /// `idcApplicationId`, `idcRegion`, all required).
    ///
    /// The reported `idcApplicationId` is the stored application IN the stored instance:
    /// an Identity Center application is identified within its instance, so the
    /// identifier is built here, in the one place every response reads it from, rather
    /// than in each handler.
    pub fn projected(&self) -> WebAppConfiguration {
        WebAppConfiguration {
            ehr_role: self.ehr_role.clone(),
            idc_application_id: idc_application_id(
                &self.idc_instance_id,
                &self.idc_application_suffix,
            ),
            idc_region: self.idc_region.clone(),
        }
    }
}

/// A persisted `Domain`.
#[derive(Debug, Clone)]
pub struct Domain {
    pub domain_id: String,
    pub arn: String,
    /// The account that OWNS this domain — the account the create was signed for.
    /// Stored on the record, so every response projects the domain's own account
    /// rather than recomputing one per read.
    pub account_id: String,
    /// The REGION this domain lives in: the Region the create was signed for, and the
    /// region segment of [`Domain::arn`].
    ///
    /// This service's endpoint is REGIONAL (`smithy_query(query="service_info")` reports
    /// `endpointPrefix`/`arnNamespace` `health-agent` with standard regional endpoints,
    /// and every modelled ARN pattern carries a region segment), so a domain is part of
    /// exactly one Region's inventory. Every lookup in this module matches on it, so a
    /// resource created in one Region cannot be listed, read, mutated or deleted through
    /// another Region's endpoint, and the per-Region quotas count only that Region's
    /// records.
    pub region: String,
    /// The provisioning generation this domain was onboarded through.
    pub onboarding_generation: OnboardingGeneration,
    pub name: String,
    pub kms_key_arn: Option<String>,
    pub encryption_type: EncryptionType,
    pub status: DomainStatus,
    /// When the owed `DELETING` -> purged transition becomes due; `None` unless the
    /// domain is being deleted.
    pub purge_due_at: Option<SystemTime>,
    /// When this domain's deletion was RECORDED — the instant the call that deleted it
    /// was served; `None` while the domain is live. Written once, so a repeated delete
    /// reports the instant the first one recorded.
    pub deleted_at: Option<SystemTime>,
    pub web_app_url: Option<String>,
    pub web_app_configuration: Option<WebAppConfigurationRecord>,
    /// The identity of the `CreateDomain` request that created this domain
    /// ([`domain_creation_fingerprint`]), so a repeat of that request resolves to THIS
    /// record instead of minting a second domain — what the operation's `@idempotent`
    /// trait requires. Written once, at creation, and never from a later call, so a
    /// mutation of the domain afterwards does not change which request created it.
    pub creation_fingerprint: String,
    pub created_at: SystemTime,
    pub subscriptions: BTreeMap<String, Subscription>,
    /// The patient insights jobs started in this domain, by `jobId`. Held HERE rather
    /// than in a map beside the registry, so a job is scoped to its own domain and goes
    /// away with it.
    pub patient_insights_jobs: BTreeMap<String, PatientInsightsJob>,
    /// The `jobId` each `clientToken` a `StartPatientInsightsJob` carried is bound to, so
    /// a retried start with the same token resolves to the job the first one created
    /// instead of starting a second. Scoped to the domain, as the job it names is, and
    /// bounded with it: an entry is written only when a job is created.
    pub patient_insights_job_ids_by_client_token: BTreeMap<String, String>,
}

impl Domain {
    /// Whether this record is still live: a `DeleteDomain` already recorded against it
    /// has taken it out of `ACTIVE`, and from that instant it is gone as far as every
    /// reader of what it CONTAINS — its subscriptions and the sessions and jobs under
    /// them — and every transition on them is concerned.
    ///
    /// The ONE place this rule is stated, beside [`Subscription::is_live`]: every reader
    /// that needs a live domain goes through it
    /// ([`ComAmazonawsConnecthealthConnecthealthState::live_domain`],
    /// [`ComAmazonawsConnecthealthConnecthealthState::live_domain_mut`],
    /// [`ComAmazonawsConnecthealthConnecthealthState::domain_created_by`]), so two of them
    /// cannot disagree about whether a domain is still live. The domain RECORD itself
    /// outlives its liveness — a reporting operation reads it through
    /// [`ComAmazonawsConnecthealthConnecthealthState::held_domain`] until the purge
    /// [`ComAmazonawsConnecthealthConnecthealthState::materialise_due`] owes it drops it.
    pub fn is_live(&self) -> bool {
        self.status == DomainStatus::Active
    }

    /// Whether deleting this domain finishes inside the deleting call.
    ///
    /// Deleting a domain tears down everything the service provisioned for it. What it
    /// has to wait for is the resource provisioned OUTSIDE this service: a domain with
    /// a web application owns an Identity Center application in another service, whose
    /// teardown outlives the call, so such a domain is reported in the non-terminal
    /// `DELETING` and the purge the clock owes applies the deletion afterwards. A domain
    /// with nothing provisioned outside this service holds only records of this store
    /// (its subscriptions and tags), which this call removes itself, so its deletion
    /// completes within the call and is reported in the terminal `DELETED`.
    pub fn deletion_completes_in_call(&self) -> bool {
        self.web_app_configuration.is_none()
    }

    /// This domain as the modelled `EncryptionContext` every Domain response reports
    /// (`smithy_query(query="shape", shape="EncryptionContext")`: `encryptionType`
    /// required, `kmsKeyArn` "Only present when encryptionType is
    /// CUSTOMER_MANAGED_KEY").
    ///
    /// Defined once, here, and called from each handler that reports a domain, so the
    /// create response and every later read cannot describe one stored record's
    /// encryption differently. The reported key is the one the record holds, which a
    /// domain has exactly when its type is `CUSTOMER_MANAGED_KEY`: the type is derived
    /// from the key when the domain is created, so the model's rule holds by the
    /// record's own invariant rather than by a second test here.
    pub fn encryption_context(&self) -> EncryptionContext {
        EncryptionContext {
            encryption_type: self.encryption_type.as_wire().to_string(),
            kms_key_arn: self.kms_key_arn.clone(),
        }
    }

    /// This domain's recorded identity pair — the account that owns it and the
    /// generation it was onboarded through — as the members a Domain response carries
    /// BESIDE its modelled ones.
    ///
    /// Neither member is in the model (see [`RecordedMember`]), so the member SCHEMAS
    /// stay per-response — each names its own output shape and the member index free in
    /// it — and the answering operation hands its two in. The member LIST is one
    /// computation: which members the pair is, in which order, read out of which fields
    /// of the stored record. Defined once, here, beside the other `Domain` projections,
    /// and called from each handler that reports a domain, so the create response and
    /// every later read cannot describe one stored record's identity differently.
    pub fn recorded_identity_members(
        &self,
        account_id_member: &'static ::aws_smithy_schema::Schema,
        onboarding_generation_member: &'static ::aws_smithy_schema::Schema,
    ) -> Vec<RecordedMember> {
        vec![
            RecordedMember::Text(account_id_member, self.account_id.clone()),
            RecordedMember::Text(
                onboarding_generation_member,
                self.onboarding_generation.as_wire().to_string(),
            ),
        ]
    }
}

/// A patient insights job's lifecycle, as the model's `JobStatus` enum declares it
/// (`smithy_query(query="shape",shape="JobStatus")`: `SUBMITTED`, `IN_PROGRESS`,
/// `FAILED`, `SUCCEEDED`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Submitted,
    InProgress,
    Failed,
    Succeeded,
}

impl JobStatus {
    /// The wire value — one of `types::JOB_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Submitted => "SUBMITTED",
            Self::InProgress => "IN_PROGRESS",
            Self::Failed => "FAILED",
            Self::Succeeded => "SUCCEEDED",
        }
    }

    /// The status a wire value names, or `None` when it names none.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "SUBMITTED" => Some(Self::Submitted),
            "IN_PROGRESS" => Some(Self::InProgress),
            "FAILED" => Some(Self::Failed),
            "SUCCEEDED" => Some(Self::Succeeded),
            _ => None,
        }
    }
}

/// The job parameters a `StartPatientInsightsJob` request carries — every member of that
/// request except the `domainId` it is scoped by and the `clientToken` that identifies
/// the request rather than the job.
///
/// Stored once and rendered back by `GetPatientInsightsJob`, which reports each of them
/// as a required member: the reader projects the STORED parameters, so what a start
/// persisted is what a later get reads.
#[derive(Debug, Clone)]
pub struct PatientInsightsJobSpec {
    pub patient_context: PatientInsightsPatientContext,
    pub insights_context: InsightsContext,
    pub encounter_context: PatientInsightsEncounterContext,
    pub user_context: UserContext,
    pub input_data_config: InputDataConfig,
    pub output_data_config: OutputDataConfig,
}

impl PatientInsightsJobSpec {
    /// A digest of every parameter, so a repeated `clientToken` can be told from a token
    /// reused for DIFFERENT parameters without keeping a second copy of the request
    /// around.
    ///
    /// Every value is length-prefixed before it is folded in, so two different parameter
    /// sets cannot collide by running their text together, and an optional member
    /// contributes a part whether or not it was supplied — an omitted one a part of its
    /// own, so omitting a member and supplying it are different requests even when what
    /// was supplied is empty. A variable-length section folds in its element count ahead
    /// of the elements, so the sections cannot be read into one another.
    pub fn fingerprint(&self) -> String {
        let mut parts: Vec<String> = vec![
            self.patient_context.patient_id.clone(),
            self.patient_context.date_of_birth.clone().unwrap_or_default(),
            self.patient_context.pronouns.clone().unwrap_or_default(),
            self.insights_context.insights_type.clone(),
            self.encounter_context.encounter_reason.clone(),
            self.user_context.role.clone(),
            self.user_context.user_id.clone(),
            self.user_context.specialty.clone().unwrap_or_default(),
            self.output_data_config.s3_output_path.clone(),
        ];
        match &self.input_data_config.fhir_server {
            Some(server) => {
                parts.push("fhirServer".to_string());
                parts.push(server.fhir_endpoint.clone());
                parts.push(server.oauth_token.clone().unwrap_or_default());
            }
            None => parts.push(String::new()),
        }
        match &self.input_data_config.s3_sources {
            Some(sources) => {
                parts.push("s3Sources".to_string());
                // The element count goes in AHEAD of the elements, so a list and a
                // longer list that starts with it are different requests and this
                // section cannot be read into whatever follows it.
                parts.push(sources.len().to_string());
                for source in sources {
                    parts.push(source.uri.clone());
                }
            }
            None => parts.push(String::new()),
        }
        fingerprint_of(parts.iter().map(String::as_str))
    }
}

/// The digest of a sequence of request values — the ONE place this crate turns the
/// parameters a request carried into the identity of that request, so no two operations
/// tell a repeat from a different request by different rules.
///
/// Every value is LENGTH-PREFIXED before it is folded in, so two different sequences
/// cannot collide by running their text together.
pub fn fingerprint_of<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part.as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// The identity of a `CreateDomain` REQUEST: a digest of every parameter that operation
/// accepts — `name`, `kmsKeyArn`, `tags` and `webAppSetupConfiguration` — so a repeat of
/// one request can be told from a different request without keeping a second copy of
/// either around. Stored on the record the request created
/// ([`Domain::creation_fingerprint`]) and compared by
/// [`ComAmazonawsConnecthealthConnecthealthState::domain_created_by`].
///
/// A SUPPLIED `tags` map is digested in KEY order, the order the store holds the set in,
/// so the same entries presented in a different wire order are the same request; a part
/// naming the member and the entry count are folded in ahead of the entries so the
/// sections cannot be read into one another. An absent optional member contributes a part
/// of its own instead — one no supplied value can produce — so omitting a member and
/// supplying it are different requests, EVERY optional member of this operation and
/// whatever was supplied: an absent `tags` and an empty `tags` map are two different
/// requests, exactly as an absent `kmsKeyArn` and a supplied one are.
pub fn domain_creation_fingerprint(
    name: &str,
    kms_key_arn: Option<&str>,
    tags: Option<&BTreeMap<String, String>>,
    web_app_setup: Option<&CreateWebAppConfiguration>,
) -> String {
    let mut parts: Vec<String> = vec![name.to_string()];
    match kms_key_arn {
        Some(arn) => parts.push(format!("kmsKeyArn:{arn}")),
        None => parts.push(String::new()),
    }
    match tags {
        Some(tags) => {
            parts.push("tags".to_string());
            // The entry count goes in AHEAD of the entries, so a set and a larger set
            // that starts with it are different requests and this section cannot be read
            // into whatever follows it.
            parts.push(tags.len().to_string());
            for (key, value) in tags {
                parts.push(key.clone());
                parts.push(value.clone());
            }
        }
        None => parts.push(String::new()),
    }
    match web_app_setup {
        Some(setup) => {
            parts.push(setup.ehr_role.clone());
            parts.push(setup.idc_instance_id.clone());
            parts.push(setup.idc_region.clone());
        }
        None => parts.push(String::new()),
    }
    fingerprint_of(parts.iter().map(String::as_str))
}

/// A persisted patient insights job, stored under the `Domain` it was started in — which
/// is what SCOPES it: a job is reachable only through the `domainId` it belongs to, so
/// one domain's job cannot be read through another's, and a domain's jobs go away with
/// the domain.
#[derive(Debug, Clone)]
pub struct PatientInsightsJob {
    pub job_id: String,
    pub domain_id: String,
    pub arn: String,
    pub status: JobStatus,
    /// The text reported for the CURRENT status; `None` while there is nothing to say
    /// about it.
    pub status_details: Option<String>,
    /// Where this job's insights were written: the location under the output path the
    /// start configured, derived once by [`patient_insights_output_uri`] when the job
    /// reaches its terminal success status. `None` until then, and `None` for a job that
    /// ended without producing output.
    pub insights_output: Option<InsightsOutput>,
    pub created_at: SystemTime,
    /// When the record last changed — the instant of its last status transition, which is
    /// the creation instant for a job that has not moved yet.
    pub updated_at: SystemTime,
    /// When the next status transition the clock owes becomes due; `None` once the job is
    /// in a terminal status and owes none.
    pub next_transition_at: Option<SystemTime>,
    /// The parameters the start persisted, reported back by the reader.
    pub spec: PatientInsightsJobSpec,
}

impl PatientInsightsJob {
    /// Apply every status transition the CLOCK has already made due.
    ///
    /// A non-terminal job owes exactly one transition at a time, one
    /// [`PATIENT_INSIGHTS_JOB_STEP_SPAN`] after the previous one: `SUBMITTED` ->
    /// `IN_PROGRESS`, then `IN_PROGRESS` -> `SUCCEEDED`, the terminal status a job that
    /// ran to completion reaches. Each transition is stamped with the instant it FELL DUE
    /// rather than the instant somebody happened to look, so the record reads the same
    /// however late the first read comes.
    ///
    /// Completing is what fills [`Self::insights_output`]: the location is derived from
    /// the output path the start persisted and this job's own id
    /// ([`patient_insights_output_uri`]), so the member the reader answers with is read
    /// out of the record rather than recomputed per read. Nothing is reported about the
    /// status of a job that did what it was asked, so `statusDetails` stays as it was.
    fn apply_due_transitions(&mut self, now: SystemTime) {
        while let Some(due) = self.next_transition_at.filter(|due| *due <= now) {
            match self.status {
                JobStatus::Submitted => {
                    self.status = JobStatus::InProgress;
                    self.next_transition_at = Some(due + PATIENT_INSIGHTS_JOB_STEP_SPAN);
                }
                JobStatus::InProgress => {
                    self.status = JobStatus::Succeeded;
                    self.insights_output = Some(InsightsOutput {
                        uri: patient_insights_output_uri(
                            &self.spec.output_data_config.s3_output_path,
                            &self.job_id,
                        ),
                    });
                    self.next_transition_at = None;
                }
                // A terminal status owes nothing; a due instant left on one is dropped
                // rather than acted on.
                JobStatus::Failed | JobStatus::Succeeded => self.next_transition_at = None,
            }
            self.updated_at = due;
        }
    }
}

/// A persisted `Subscription`, stored under its own `Domain`.
///
/// `activated_at` / `deactivated_at` are a HISTORY of the explicit
/// `ActivateSubscription` / `DeactivateSubscription` transitions, not a view of
/// `status`: each is stamped when its transition runs and is never cleared, so both
/// boundaries of a billing interval remain readable after the opposite transition. Each
/// holds the MOST RECENT run of its own transition. A freshly created subscription has
/// made neither transition, so it carries neither, whatever `status` it was created in.
#[derive(Debug, Clone)]
pub struct Subscription {
    pub domain_id: String,
    pub subscription_id: String,
    pub arn: String,
    pub status: SubscriptionStatus,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    pub activated_at: Option<SystemTime>,
    pub deactivated_at: Option<SystemTime>,
    /// The Medical Scribe listening sessions started on this subscription, by
    /// `sessionId`. Held HERE rather than in a map beside the registry, so a session is
    /// scoped to its own subscription and goes away with it. Written by
    /// [`Self::start_scribe_session`], the one growth site, which is bounded by
    /// [`MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION`].
    pub scribe_sessions: BTreeMap<String, ScribeSession>,
}

/// A Medical Scribe listening session's stream status, as the model's
/// `MedicalScribeStreamStatus` enum declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScribeStreamStatus {
    InProgress,
    Paused,
    Failed,
    Completed,
}

impl ScribeStreamStatus {
    /// The wire value — one of `types::MEDICAL_SCRIBE_STREAM_STATUS_VALUES`.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Paused => "PAUSED",
            Self::Failed => "FAILED",
            Self::Completed => "COMPLETED",
        }
    }
}

/// A persisted Medical Scribe listening session, stored under the `Subscription` it was
/// started on — which is what SCOPES it: a session is reachable only through the
/// (`domainId`, `subscriptionId`) pair it belongs to, so one subscription's session
/// cannot be read through another's (see [`ComAmazonawsConnecthealthConnecthealthState::live_scribe_session`]).
///
/// A session is written by the one operation that starts one,
/// `StartMedicalScribeListeningSession` (hand-authored, because its input carries a
/// `@streaming` event-stream member the stub generator emits no framing for), and it is
/// written through [`Subscription::start_scribe_session`] — the single growth site,
/// bounded by [`MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION`]. `GetMedicalScribeListeningSession`
/// resolves a session out of this map rather than answering a constant, so a stored
/// session is reported from state and an absent one is the modelled 404.
#[derive(Debug, Clone)]
pub struct ScribeSession {
    pub session_id: String,
    pub domain_id: String,
    pub subscription_id: String,
    pub language_code: String,
    pub media_sample_rate_hertz: i32,
    pub media_encoding: String,
    pub channel_definitions: Vec<MedicalScribeChannelDefinition>,
    pub post_stream_action_settings: Option<MedicalScribePostStreamActionSettingsResponse>,
    pub post_stream_action_result: Option<MedicalScribePostStreamActionsResult>,
    pub encounter_context_provided: bool,
    pub stream_status: ScribeStreamStatus,
    pub stream_creation_time: SystemTime,
    /// When the stream ENDED; `None` while it is still running, so a session in a
    /// non-terminal status carries no end instant.
    pub stream_end_time: Option<SystemTime>,
}

impl ScribeSession {
    /// This record as the modelled `MedicalScribeListeningSessionDetails`.
    pub fn details(&self) -> MedicalScribeListeningSessionDetails {
        MedicalScribeListeningSessionDetails {
            session_id: Some(self.session_id.clone()),
            domain_id: Some(self.domain_id.clone()),
            subscription_id: Some(self.subscription_id.clone()),
            language_code: Some(self.language_code.clone()),
            media_sample_rate_hertz: Some(self.media_sample_rate_hertz),
            media_encoding: Some(self.media_encoding.clone()),
            channel_definitions: Some(self.channel_definitions.clone()),
            post_stream_action_settings: self.post_stream_action_settings.clone(),
            post_stream_action_result: self.post_stream_action_result.clone(),
            encounter_context_provided: Some(self.encounter_context_provided),
            stream_status: Some(self.stream_status.as_wire().to_string()),
            stream_creation_time: Some(to_timestamp(self.stream_creation_time)),
            stream_end_time: self.stream_end_time.map(to_timestamp),
        }
    }
}

/// What starting a listening session under a subscription did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScribeSessionStart {
    /// The `sessionId` named no session of this subscription, and the new record is
    /// stored.
    Started,
    /// The `sessionId` named a session this subscription already holds, so that record
    /// was carried forward rather than duplicated: the new stream's configuration and
    /// status replace what was stored, and the session keeps the creation instant of
    /// the stream that first claimed the id.
    Resumed,
    /// The subscription already holds [`MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION`] sessions
    /// and the id names none of them, so NOTHING was stored: the caller is answered with
    /// the modelled quota failure and the store is left exactly as it was.
    AtCapacity,
}

impl Subscription {
    /// Whether this record is still live: one `DeleteDomain` already marked `DELETED`
    /// is gone as far as every reader and every transition is concerned.
    pub fn is_live(&self) -> bool {
        self.status != SubscriptionStatus::Deleted
    }

    /// Move this record to `target`, as of `now` — the ONE status step every
    /// `Subscription` transition takes, single-homed here beside [`Self::is_live`] and
    /// [`Self::description`] rather than written out in each handler that transitions a
    /// subscription.
    ///
    /// The step writes the status, stamps the instant of the transition on the member
    /// that status owns — `activated_at` for `ACTIVE`, `deactivated_at` for `INACTIVE`,
    /// and no stamp for `DELETED`, which this record keeps no instant of its own for —
    /// and moves `last_updated_at` to that same instant. The OPPOSITE status's stamp is
    /// never cleared: the stamps are a history of the transitions that ran (see this
    /// type's own documentation), so both boundaries of a billing interval stay readable.
    ///
    /// The step itself is UNCONDITIONAL. Whether a call that finds the record already in
    /// `target` takes it is the operation's own rule, not this record's: the transition
    /// the model declares `@idempotent` writes nothing on a repeat, while one it does not
    /// records that this call ran.
    pub fn transition_to(&mut self, target: SubscriptionStatus, now: SystemTime) {
        self.status = target;
        match target {
            SubscriptionStatus::Active => self.activated_at = Some(now),
            SubscriptionStatus::Inactive => self.deactivated_at = Some(now),
            SubscriptionStatus::Deleted => {}
        }
        self.last_updated_at = now;
    }

    /// Start a Medical Scribe listening session on this subscription, under the
    /// `sessionId` the record carries.
    ///
    /// The ONE write that adds a session to this service's store, so the bound on how
    /// many a subscription holds is stated once, here, and a refused start leaves the
    /// store untouched. An id this subscription already holds RESUMES that session
    /// rather than adding another: the model requires the caller to name the
    /// `sessionId`, and a session the model lets a client resume keeps the instant its
    /// stream was first created.
    pub fn start_scribe_session(&mut self, mut session: ScribeSession) -> ScribeSessionStart {
        if let Some(held) = self.scribe_sessions.get(&session.session_id) {
            session.stream_creation_time = held.stream_creation_time;
            self.scribe_sessions
                .insert(session.session_id.clone(), session);
            return ScribeSessionStart::Resumed;
        }
        if self.scribe_sessions.len() >= MAX_SCRIBE_SESSIONS_PER_SUBSCRIPTION {
            return ScribeSessionStart::AtCapacity;
        }
        self.scribe_sessions
            .insert(session.session_id.clone(), session);
        ScribeSessionStart::Started
    }

    /// This record as the modelled `SubscriptionDescription` four operations return.
    ///
    /// The ONE projection of a stored subscription: every operation that reports one
    /// goes through here (see [`Self::create_output`] for the create, whose output shape
    /// is a distinct Rust type carrying the same member set), so no two responders can
    /// disagree about a member of one record.
    pub fn description(&self) -> SubscriptionDescription {
        SubscriptionDescription {
            domain_id: self.domain_id.clone(),
            subscription_id: self.subscription_id.clone(),
            arn: self.arn.clone(),
            status: self.status.as_wire().to_string(),
            created_at: to_timestamp(self.created_at),
            last_updated_at: to_timestamp(self.last_updated_at),
            activated_at: self.activated_at.map(to_timestamp),
            deactivated_at: self.deactivated_at.map(to_timestamp),
        }
    }

    /// This record as the `CreateSubscriptionOutput` `CreateSubscription` answers with.
    ///
    /// `CreateSubscriptionOutput` is a distinct generated type from
    /// `SubscriptionDescription`, but it carries the identical member set, so it is the
    /// SAME projection re-typed rather than a second one: this re-types
    /// [`Self::description`] member for member instead of reading the record again.
    /// Destructuring is deliberate — if either shape gains or loses a member, this stops
    /// compiling instead of letting the create's view of a subscription drift from the
    /// view every other responder reports.
    pub fn create_output(&self) -> CreateSubscriptionOutput {
        let SubscriptionDescription {
            domain_id,
            subscription_id,
            arn,
            status,
            created_at,
            last_updated_at,
            activated_at,
            deactivated_at,
        } = self.description();
        CreateSubscriptionOutput {
            domain_id,
            subscription_id,
            arn,
            status,
            created_at,
            last_updated_at,
            activated_at,
            deactivated_at,
        }
    }
}

/// Per-service state for com-amazonaws-connecthealth-connecthealth. A handler reaches it through [`service_state`].
///
/// `BTreeMap` throughout, so `ListDomains` / `ListSubscriptions` walk a deterministic,
/// key-ordered sequence and a `nextToken` naming the last key returned is a stable
/// cursor into it.
#[derive(Debug, Default)]
pub struct ComAmazonawsConnecthealthConnecthealthState {
    /// Every live `Domain`, keyed by `domainId` (ids are minted unique across the whole
    /// store, so one key never names two domains). The Region each one belongs to is on
    /// the record (`Domain::region`) and every lookup matches it, so this one map holds a
    /// separate inventory per Region.
    pub domains: BTreeMap<String, Domain>,
    /// The ONE tag store, keyed by resource ARN so domain and subscription ARNs both
    /// work. `CreateDomain` seeds it and `TagResource` merges into it, both through
    /// [`Self::set_tags`]; `UntagResource`/`ListTagsForResource` share it.
    pub tags_by_arn: BTreeMap<String, BTreeMap<String, String>>,
    /// Every `domainId` this service has allocated and still remembers, whether or not
    /// the domain itself is still held — the ALLOCATION HISTORY a delete consults to
    /// tell "this domain is gone" from "this id was never valid anywhere".
    pub allocated_domain_ids: BTreeSet<String>,
    /// The same ids in allocation order, so the bound in
    /// [`MAX_DOMAIN_ALLOCATION_HISTORY`] drops the oldest first.
    pub allocation_order: VecDeque<String>,
    /// Monotonic per-process sequence folded into every minted id, so two ids minted
    /// in the same nanosecond still differ.
    pub next_id_seq: u64,
}

impl ComAmazonawsConnecthealthConnecthealthState {
    /// The next value of the id sequence.
    fn next_seq(&mut self) -> u64 {
        let seq = self.next_id_seq;
        self.next_id_seq = self.next_id_seq.wrapping_add(1);
        seq
    }

    /// Mint an unused `domainId`: `dom-` plus 16 lowercase hex characters, which is 20
    /// characters long and matches `^(hai-|dom-)[a-z0-9]+$` (len 20..25).
    ///
    /// Unused means unused by a domain this store holds AND by one it remembers
    /// allocating, so an id never names two domains and the allocation history cannot
    /// be made ambiguous by a re-mint.
    pub fn mint_domain_id(&mut self) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = mint_id("dom-", 16, seq);
            if !self.domains.contains_key(&candidate)
                && !self.allocated_domain_ids.contains(&candidate)
            {
                return candidate;
            }
        }
    }

    /// Every `Domain` record `account_id` OWNS in `region`, whatever its status, in
    /// `domainId` order.
    ///
    /// The ONE place the ACCOUNT scope of this store is expressed, as
    /// [`Self::domains_in`] is the one place its Region scope is: every lookup whose
    /// contract is an account's domains — the account's cap, the repeat an `@idempotent`
    /// create resolves, and the listing the model documents as an account's — goes through
    /// here, so none of them can disagree about which domains an account holds. A record
    /// carries the account that created it (`Domain::account_id`), so a domain of another
    /// account is outside every one of those answers.
    pub fn domains_owned_by<'a>(
        &'a self,
        region: &str,
        account_id: &str,
    ) -> impl Iterator<Item = &'a Domain> + 'a {
        // Captured by VALUE for the same reason [`Self::domains_in`] captures its Region:
        // the iterator borrows only the store.
        let account_id = account_id.to_string();
        self.domains_in(region)
            .filter(move |domain| domain.account_id == account_id)
    }

    /// How many `Domain` records `account_id` currently holds IN `region` — the LIVE
    /// count the account's per-Region cap is measured against, which a purge of a deleted
    /// domain lowers again. Every record carries its owning account and its Region, so
    /// the count is of that account's domains in that Region alone and a domain in
    /// another Region consumes nothing here.
    pub fn domains_held_by(&self, region: &str, account_id: &str) -> usize {
        self.domains_owned_by(region, account_id).count()
    }

    /// The LIVE domain `account_id` already holds in `region` that was created by the
    /// request `fingerprint` identifies ([`domain_creation_fingerprint`]), or `None` when
    /// no such domain exists — what tells a REPEAT of a `CreateDomain` request from a
    /// first one.
    ///
    /// Scoped to the account and Region the create is addressed to, the scope the quota
    /// and every lookup in this module use, so a request repeated against another
    /// Region's endpoint addresses that Region's inventory and creates there. Only a LIVE
    /// domain resolves one: a domain already being deleted is on its way out, so the
    /// earlier request's effect has been undone by an intervening call and the create is
    /// a first one again.
    pub fn domain_created_by(
        &self,
        region: &str,
        account_id: &str,
        fingerprint: &str,
    ) -> Option<&Domain> {
        self.domains_owned_by(region, account_id).find(|domain| {
            domain.is_live() && domain.creation_fingerprint == fingerprint
        })
    }

    /// Every `Domain` record `region` holds, whatever its status, in `domainId` order.
    ///
    /// The ONE place the Region scope of this store is expressed: a reader that
    /// enumerates domains goes through here, so an enumeration and a point lookup
    /// ([`Self::held_domain`]) can never disagree about which Region holds what. A reader
    /// whose contract is one ACCOUNT's domains narrows this through
    /// [`Self::domains_owned_by`] rather than filtering the account itself.
    pub fn domains_in<'a>(&'a self, region: &str) -> impl Iterator<Item = &'a Domain> + 'a {
        // The Region is captured by VALUE, so the iterator borrows only the store: a
        // caller may hand in a Region that outlives nothing and still keep the records
        // it yields.
        let region = region.to_string();
        self.domains
            .values()
            .filter(move |domain| domain.region == region)
    }

    /// Remember that `domain_id` was allocated, dropping the oldest remembered id once
    /// the history is at [`MAX_DOMAIN_ALLOCATION_HISTORY`].
    pub fn record_domain_allocation(&mut self, domain_id: &str) {
        if !self.allocated_domain_ids.insert(domain_id.to_string()) {
            return;
        }
        self.allocation_order.push_back(domain_id.to_string());
        while self.allocation_order.len() > MAX_DOMAIN_ALLOCATION_HISTORY {
            if let Some(evicted) = self.allocation_order.pop_front() {
                self.allocated_domain_ids.remove(&evicted);
            }
        }
    }

    /// Whether `domain_id` was ever allocated to a domain this service created.
    pub fn has_allocated_domain(&self, domain_id: &str) -> bool {
        self.allocated_domain_ids.contains(domain_id)
    }

    /// Mint a `subscriptionId` unused within `domain_id`: `sub-` plus 21 hex
    /// characters, which is exactly 25 characters and matches `^sub-[a-zA-Z0-9]{21}$`.
    pub fn mint_subscription_id(&mut self, domain_id: &str) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = mint_id("sub-", 21, seq);
            let taken = self
                .domains
                .get(domain_id)
                .is_some_and(|domain| domain.subscriptions.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// Mint a `jobId` unused within `domain_id`: 32 hex characters, which is inside the
    /// modelled `JobId` bound of 1..36 and inside the `[-a-zA-Z0-9_/.]+` segment the
    /// `JobArn` pattern allows for a job.
    pub fn mint_patient_insights_job_id(&mut self, domain_id: &str) -> String {
        loop {
            let seq = self.next_seq();
            let candidate = mint_id("", 32, seq);
            let taken = self
                .domains
                .get(domain_id)
                .is_some_and(|domain| domain.patient_insights_jobs.contains_key(&candidate));
            if !taken {
                return candidate;
            }
        }
    }

    /// Mint the `requestId` a response reports for ONE call: 32 hex characters in the
    /// hyphenated 8-4-4-4-12 form AWS request ids take, which is 36 characters and so
    /// inside the modelled `RequestId` bound of 1..256.
    ///
    /// Per CALL, not per resource: it identifies the request, so it is minted fresh on
    /// every call and stored on nothing. The digest is the one every id this service
    /// mints comes from, so two calls of one process cannot share an id.
    pub fn mint_request_id(&mut self) -> String {
        let seq = self.next_seq();
        let body = mint_id("", 32, seq);
        // Hyphenated by walking the digest, so the form never depends on an index being
        // inside it: the separators go where the 8-4-4-4-12 grouping puts them.
        let mut request_id = String::with_capacity(36);
        for (index, digit) in body.chars().enumerate() {
            if matches!(index, 8 | 12 | 16 | 20) {
                request_id.push('-');
            }
            request_id.push(digit);
        }
        request_id
    }

    /// Mint the id of the Identity Center application this service creates for a
    /// domain's web app, WITHIN the Identity Center instance the create named: `apl-`
    /// plus 16 hex, as Identity Center names an application.
    ///
    /// Every domain owns its own application, so a fresh id is minted per call; the
    /// instance is folded into the digest, so the id is unique to the
    /// instance-and-application pair rather than to this store's sequence alone.
    pub fn mint_idc_application_suffix(&mut self, idc_instance_id: &str) -> String {
        let seq = self.next_seq();
        mint_scoped_id("apl-", 16, seq, idc_instance_id)
    }

    /// Materialise EVERYTHING the clock has already made due, whatever class of owed
    /// work it is — the one preamble every handler in this crate runs before it reads or
    /// writes anything else.
    ///
    /// Stated once HERE rather than per handler, so no handler can materialise one class
    /// of owed work and leave another standing: a `Domain` whose `DELETING` span has
    /// elapsed is purged (`purge_due_deletions`), and a patient insights job of a LIVE
    /// domain whose next status transition has fallen due is moved on
    /// (`advance_due_patient_insights_jobs`). The purge runs first, so no transition is
    /// applied to a record this same instant already owed the removal of.
    ///
    /// It writes only what was owed at THIS instant whether or not anybody called, which
    /// is why the reporting operations run it too: a deletion is applied at the instant
    /// its span elapsed and each job transition is stamped with the instant it FELL DUE
    /// (`PatientInsightsJob::apply_due_transitions`) rather than the instant somebody
    /// looked, so running it in one handler cannot change a value another handler
    /// reports.
    pub fn materialise_due(&mut self, now: SystemTime) {
        self.purge_due_deletions(now);
        self.advance_due_patient_insights_jobs(now);
    }

    /// Materialise every deletion the CLOCK has already made due: a `Domain` whose
    /// `DELETING` span has elapsed is purged, with its subscriptions and every tag
    /// entry keyed by one of their ARNs.
    ///
    /// One half of [`Self::materialise_due`], which is the only caller: a handler reaches
    /// the owed work through that one entry point rather than naming the halves itself.
    fn purge_due_deletions(&mut self, now: SystemTime) {
        let due: Vec<String> = self
            .domains
            .values()
            .filter(|domain| domain.purge_due_at.is_some_and(|due| due <= now))
            .map(|domain| domain.domain_id.clone())
            .collect();
        for domain_id in due {
            if let Some(domain) = self.domains.remove(&domain_id) {
                self.tags_by_arn.remove(&domain.arn);
                for subscription in domain.subscriptions.values() {
                    self.tags_by_arn.remove(&subscription.arn);
                }
            }
        }
    }

    /// Materialise every patient-insights-job status transition the CLOCK has already
    /// made due, in every LIVE domain.
    ///
    /// The other half of [`Self::materialise_due`], which is the only caller: it writes
    /// only what was owed at this instant whether or not anybody called, so a job
    /// observed late reads exactly as it would have at the moment its transition fell
    /// due.
    ///
    /// Liveness is the scope because of the rule [`Domain::is_live`] states: from the
    /// instant a delete is recorded against a domain, the records it CONTAINS are gone
    /// for every transition on them, so a job held under a domain whose deletion is
    /// already recorded owes nothing further — the same treatment `DeleteDomain` gives
    /// the other contained record kind, whose lifecycle it ends in the deleting call.
    /// Owed work on such a record is the purge, which the half that runs first
    /// (`purge_due_deletions`) applies.
    fn advance_due_patient_insights_jobs(&mut self, now: SystemTime) {
        for domain in self.domains.values_mut().filter(|domain| domain.is_live()) {
            for job in domain.patient_insights_jobs.values_mut() {
                job.apply_due_transitions(now);
            }
        }
    }

    /// The patient insights job `job_id` names under EXACTLY the domain `domain_id`
    /// names.
    ///
    /// `None` when either link of that chain is missing — the domain is absent or no
    /// longer live, or it holds no job of that id — so a job started in one domain is
    /// never reachable through another, and both misses are the one answer
    /// `GetPatientInsightsJob` gives: `ResourceNotFoundException`.
    pub fn patient_insights_job(
        &self,
        region: &str,
        domain_id: &str,
        job_id: &str,
    ) -> Option<&PatientInsightsJob> {
        self.live_domain(region, domain_id)?
            .patient_insights_jobs
            .get(job_id)
    }

    /// The tags recorded for `arn`, in key order.
    pub fn tags_for(&self, arn: &str) -> BTreeMap<String, String> {
        self.tags_by_arn.get(arn).cloned().unwrap_or_default()
    }

    /// The tags a RESPONSE reports for `arn`: always a map, EMPTY when the resource
    /// carries none.
    ///
    /// A modelled tag map is a map of the resource's tags, and a resource with no tags
    /// has an empty one — an absent member says "unknown" instead, so a client
    /// iterating the member would have to tell the two apart. Every response that
    /// reports a resource's tags projects them through here.
    pub fn reported_tags(&self, arn: &str) -> std::collections::HashMap<String, String> {
        self.tags_for(arn).into_iter().collect()
    }

    /// Whether `arn` names a resource this service currently holds IN `region`: the ARN
    /// of a `Domain` record this Region still holds, or of a live `Subscription` under
    /// one.
    ///
    /// Holding a record is what makes a resource addressable, NOT the lifecycle status
    /// that record reports: a `Domain` whose deletion is in flight is a resource the
    /// service still holds — its own reader answers it ([`Self::held_domain`] is what
    /// `GetDomain` looks it up through, and `ListDomains` lists it under its non-terminal
    /// status) — so its ARN resolves for exactly as long as the record does. The one
    /// place an ARN stops resolving is the purge [`Self::materialise_due`] owes, which
    /// drops the record together with the tag entries keyed by its ARN. An ARN this resolves is
    /// therefore always one some reader resolves too, and the ARN-addressed operations
    /// and the resource's own reader never disagree about whether it exists.
    ///
    /// The Region scope is the readers' as well ([`Self::domains_in`]): an ARN rooted in
    /// another Region resolves nowhere here, exactly as the domain it names resolves
    /// through no other Region's endpoint.
    ///
    /// A `Subscription` is resolved through its own reader's liveness
    /// ([`Self::live_subscription`]): a subscription already `DELETED` is gone for that
    /// reader and gone here, which covers every subscription of a domain being deleted —
    /// `DeleteDomain` marks them all `DELETED` as it records the deletion.
    pub fn holds_resource_arn(&self, region: &str, arn: &str) -> bool {
        self.domains_in(region).any(|domain| {
            domain.arn == arn
                || domain.subscriptions.values().any(|subscription| {
                    subscription.arn == arn
                        && self
                            .live_subscription(
                                region,
                                &domain.domain_id,
                                &subscription.subscription_id,
                            )
                            .is_some()
                })
        })
    }

    /// Record `tags` as the tag set of `arn`.
    ///
    /// The caller passes the already-merged set, bounded by
    /// [`crate::service::tag_bounds::merged_within_cap`]. An EMPTY
    /// set is stored as no entry at all, so "no tags" has one representation in the
    /// store whether the resource was never tagged or had every tag removed.
    pub fn set_tags(&mut self, arn: &str, tags: BTreeMap<String, String>) {
        if tags.is_empty() {
            self.tags_by_arn.remove(arn);
        } else {
            self.tags_by_arn.insert(arn.to_string(), tags);
        }
    }

    /// The live domain `domain_id` names IN `region`, for reading.
    ///
    /// `None` when `region` holds no such domain (it is absent, or it belongs to another
    /// Region) or when the one it holds is no longer `ACTIVE` — the cases every operation
    /// on a domain's subscriptions answers with `ResourceNotFoundException`. ONE liveness
    /// rule and ONE Region scope, so a subscription a caller can enumerate is one it can
    /// also read.
    pub fn live_domain(&self, region: &str, domain_id: &str) -> Option<&Domain> {
        self.held_domain(region, domain_id)
            .filter(|domain| domain.is_live())
    }

    /// The live domain `domain_id` names in `region`, for writing. Liveness is exactly
    /// [`Self::live_domain`]'s.
    pub fn live_domain_mut(&mut self, region: &str, domain_id: &str) -> Option<&mut Domain> {
        self.held_domain_mut(region, domain_id)
            .filter(|domain| domain.is_live())
    }

    /// The domain `region` holds under `domain_id`, WHATEVER its status — the reader a
    /// reporting operation uses when a non-`ACTIVE` domain is still something it reports
    /// (`GetDomain` on a domain that is `DELETING`).
    ///
    /// `None` when this Region holds no such domain, whether because the id names
    /// nothing at all or because the domain it names belongs to ANOTHER Region: a
    /// regional endpoint sees only its own Region's inventory, so both are the same miss
    /// and both answer `ResourceNotFoundException`.
    pub fn held_domain(&self, region: &str, domain_id: &str) -> Option<&Domain> {
        self.domains
            .get(domain_id)
            .filter(|domain| domain.region == region)
    }

    /// The domain `region` holds under `domain_id`, for writing. The scope is exactly
    /// [`Self::held_domain`]'s.
    pub fn held_domain_mut(&mut self, region: &str, domain_id: &str) -> Option<&mut Domain> {
        self.domains
            .get_mut(domain_id)
            .filter(|domain| domain.region == region)
    }

    /// The live subscription `subscription_id` carries under `domain_id`, for reading.
    ///
    /// `None` when the domain is not live, or when the subscription is absent or
    /// already `DELETED` — the cases every operation on a subscription answers with
    /// `ResourceNotFoundException`.
    pub fn live_subscription(
        &self,
        region: &str,
        domain_id: &str,
        subscription_id: &str,
    ) -> Option<&Subscription> {
        self.live_domain(region, domain_id)?
            .subscriptions
            .get(subscription_id)
            .filter(|subscription| subscription.is_live())
    }

    /// The Medical Scribe listening session `session_id` names under EXACTLY this
    /// (`domain_id`, `subscription_id`) scope.
    ///
    /// `None` when any link of that chain is missing — the domain, the subscription, or
    /// the session under that subscription — so a session started on one subscription is
    /// never reachable through another, and every miss is the one answer
    /// `GetMedicalScribeListeningSession` gives: `ResourceNotFoundException`.
    pub fn live_scribe_session(
        &self,
        region: &str,
        domain_id: &str,
        subscription_id: &str,
        session_id: &str,
    ) -> Option<&ScribeSession> {
        self.live_subscription(region, domain_id, subscription_id)?
            .scribe_sessions
            .get(session_id)
    }

    /// The live subscription `subscription_id` carries under `domain_id`, for writing.
    /// Liveness is exactly [`Self::live_subscription`]'s.
    pub fn live_subscription_mut(
        &mut self,
        region: &str,
        domain_id: &str,
        subscription_id: &str,
    ) -> Option<&mut Subscription> {
        self.live_domain_mut(region, domain_id)?
            .subscriptions
            .get_mut(subscription_id)
            .filter(|subscription| subscription.is_live())
    }
}

/// Mint an id: `prefix` followed by `body_len` characters of a SHA-256 digest over the
/// prefix, the caller's sequence number, this process and the current instant.
///
/// Hex digits are lowercase alphanumerics, so one minting rule satisfies every id
/// pattern this service declares, and folding in both the sequence and the clock keeps
/// ids unique within a process and across processes.
fn mint_id(prefix: &str, body_len: usize, seq: u64) -> String {
    mint_scoped_id(prefix, body_len, seq, "")
}

/// [`mint_id`], with `scope` folded into the digest: the id of the resource the minted
/// one is allocated WITHIN, so the result is unique to the pair rather than to the
/// sequence and the clock alone.
fn mint_scoped_id(prefix: &str, body_len: usize, seq: u64, scope: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(prefix.as_bytes());
    hasher.update(scope.as_bytes());
    hasher.update(seq.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    let body = hex::encode(hasher.finalize());
    format!("{prefix}{}", &body[..body_len])
}

/// The most characters of the Identity Center instance id a create named that this
/// service carries into the application identifier it reports.
///
/// `WebAppConfiguration$idcApplicationId` is bounded at 256 characters, while
/// `CreateWebAppConfiguration$idcInstanceId` is bounded at 256 on its own and declares
/// no pattern (`smithy_query(query="shape",
/// shape="WebAppConfiguration,CreateWebAppConfiguration")`), so a value the model
/// accepts can be longer than the identifier that has to carry it wrapped in an ARN.
/// This is what keeps the reported identifier inside its own modelled bound.
///
/// Carried as far as it fits rather than refused: every value up to the modelled 256 is
/// one the model declares this operation accepts, so refusing one would be a refusal the
/// model does not state. An Identity Center instance id (`ssoins-` plus 16 hex) is an
/// order of magnitude shorter than this bound, and the application id minted for the
/// instance folds in the WHOLE value, so two instances that share a prefix still get
/// different application identifiers.
pub const MAX_IDC_INSTANCE_SEGMENT: usize = 160;

/// The identifier of the Identity Center application `suffix` inside the Identity Center
/// instance `idc_instance_id`:
/// `arn:aws:sso::<account>:application/<instanceId>/apl-<suffix>`.
///
/// An Identity Center application is identified WITHIN its instance, so the identifier
/// names both: a web app created against one instance is distinguishable from one
/// created against another, and the instance a domain's web application lives in is
/// observable in every response that reports the configuration. Identity Center is a
/// global namespace, so the ARN carries no region segment.
///
/// The instance is carried as the create NAMED it — the model documents the member as
/// the instance id and constrains nothing else about it, so this service neither
/// reformats the value nor judges its form — bounded by [`MAX_IDC_INSTANCE_SEGMENT`].
pub fn idc_application_id(idc_instance_id: &str, suffix: &str) -> String {
    let instance: String = idc_instance_id
        .chars()
        .take(MAX_IDC_INSTANCE_SEGMENT)
        .collect();
    ::engine::emulator_arn(
        IDC_ARN_NAMESPACE,
        "",
        &format!("application/{instance}/{suffix}"),
    )
}

/// The ARN of a `Domain`:
/// `arn:aws:health-agent:<region>:<account>:domain/<domainId>`.
pub fn domain_arn(region: &str, domain_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("domain/{domain_id}"))
}

/// The account every resource of this emulator belongs to: the canonical emulator
/// account the vendored runtime owns, which is also the account segment of every ARN
/// [`domain_arn`] builds. A domain's owning account is stored on its record at creation
/// (`Domain::account_id`), so the responses project the domain's own account.
pub fn owning_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// One member the RECORDED service carries beside a modelled response, with the member
/// schema it is written under.
///
/// None of these members is in the model (`smithy_query(query="shape",
/// shape="CreateDomainOutput,GetDomainOutput,DeleteDomainOutput")` lists neither
/// `accountId`, `onboardingGeneration` nor `deletedAt`), so no generated field exists
/// to fill: [`RecordedMembers`] writes them after the modelled payload's own members,
/// each under a member schema the answering operation declares. The modelled shapes and
/// the model file stay exactly as shipped, and a client that knows only the modelled
/// shape ignores the extra keys.
#[derive(Debug)]
pub enum RecordedMember {
    /// A string-valued member.
    Text(&'static ::aws_smithy_schema::Schema, String),
    /// A timestamp-valued member, serialized in the protocol's timestamp format.
    Instant(
        &'static ::aws_smithy_schema::Schema,
        ::aws_smithy_types::DateTime,
    ),
}

/// A modelled response plus the members the recorded service carries beside it.
#[derive(Debug)]
pub struct RecordedMembers<T> {
    /// The modelled response — the only part an SDK-generated client reads.
    pub payload: T,
    /// The recorded-only members, written in order after the modelled ones.
    pub members: Vec<RecordedMember>,
}

impl<T: ::aws_smithy_schema::serde::SerializableStruct>
    ::aws_smithy_schema::serde::SerializableStruct for RecordedMembers<T>
{
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.payload.serialize_members(serializer)?;
        for member in &self.members {
            match member {
                RecordedMember::Text(schema, value) => serializer.write_string(schema, value)?,
                RecordedMember::Instant(schema, value) => {
                    serializer.write_timestamp(schema, value)?
                }
            }
        }
        Ok(())
    }
}

/// The ARN of a `Subscription`: its domain's ARN plus `/subscription/<subscriptionId>`.
pub fn subscription_arn(domain_arn: &str, subscription_id: &str) -> String {
    format!("{domain_arn}/subscription/{subscription_id}")
}

/// The ARN of a patient insights job: its domain's ARN plus
/// `/patient-insights-job/<jobId>`, which is the resource form the modelled `JobArn`
/// pattern declares (`smithy_query(query="shape",shape="JobArn")`:
/// `^arn:aws[-a-z]*:health-agent:[-a-z0-9]+:[0-9]{12}:domain/[-a-zA-Z0-9-]+/patient-insights-job/[-a-zA-Z0-9_/.]+$`).
pub fn patient_insights_job_arn(domain_arn: &str, job_id: &str) -> String {
    format!("{domain_arn}/patient-insights-job/{job_id}")
}

/// Where a completed patient insights job's output sits: UNDER the `s3OutputPath` the
/// start configured, in a prefix of the job's own id.
///
/// The configured path is where the caller asked for output, and the job's id is what
/// keeps two jobs configured with the same path from reporting the same location, so the
/// reported URI is the configured path (with any trailing separator collapsed) followed
/// by `<jobId>/`. It is DERIVED from stored values only — the path the request persisted
/// and the id this service minted — so the same job reports the same location on every
/// read.
///
/// `InsightsOutput$uri` is an `S3Uri`, so the result must satisfy that shape's
/// [`MAX_S3_URI_LEN`]: a configured path with no room left for a job prefix is reported
/// unchanged rather than lengthened past the bound its own member declares.
///
/// The fit is measured in CHARACTERS, as `@length` on a string is defined and as every
/// other modelled bound this module keeps counts it, so a path whose characters are
/// multi-byte is reported under its job prefix exactly like an ASCII path of the same
/// length: a value's encoding is not what the bound it fills constrains.
pub fn patient_insights_output_uri(s3_output_path: &str, job_id: &str) -> String {
    let derived = format!("{}/{job_id}/", s3_output_path.trim_end_matches('/'));
    if derived.chars().count() <= MAX_S3_URI_LEN {
        derived
    } else {
        s3_output_path.to_string()
    }
}

/// The web-app URL of a `Domain`, derived from the domain's own id and region.
pub fn web_app_url(region: &str, domain_id: &str) -> String {
    format!("https://{domain_id}.{ARN_NAMESPACE}.{region}.on.aws")
}

/// The longest credential-scope region field this service files a record under.
///
/// Every region code AWS publishes is far shorter; the bound exists because the field is
/// read off the request, so without it one request could file a record under a key of
/// arbitrary size. A longer field names no region this service serves, and is treated
/// exactly like a scope that names none at all.
const MAX_REGION_LEN: usize = 32;

/// The region this request was signed for, or [`DEFAULT_REGION`] when it names none.
///
/// Constrained to the `[a-z0-9-]+` segment every ARN pattern in this model declares and
/// to [`MAX_REGION_LEN`], so a request signed for a nonsense region cannot mint an ARN
/// that fails its own pattern, nor file a record under a key of its own choosing.
pub fn region_of(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= MAX_REGION_LEN
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A stored instant as the modelled timestamp type.
pub fn to_timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// How a page scope spells "no filter was asked for", which is a collection of its own:
/// one no filter value can also spell, since every value a filter takes is a modelled
/// enum's wire value and those are `[A-Z_]`.
const UNFILTERED_PAGE_SCOPE: &str = "*";

/// The collection a `ListDomains` cursor enumerates: ONE account's domains in ONE
/// Region, narrowed by the `status` filter the request states — `None` being the
/// unfiltered listing, a different collection from any one status's.
///
/// EVERY request value that narrows which records a listing enumerates belongs in the
/// scope, because a cursor is a bare "resume strictly after this key" position: a cursor
/// accepted under a different narrowing would skip every record sorting at or before its
/// key, including records the resuming request's own narrowing never served. The account
/// is not part of it because every record of this emulator belongs to the one canonical
/// account ([`owning_account_id`]); the Region and the `status` filter are, because two
/// live collections of this listing differ in exactly those.
pub fn domains_page_scope(region: &str, status: Option<DomainStatus>) -> String {
    format!(
        "domains/{region}/{}",
        status.map_or(UNFILTERED_PAGE_SCOPE, DomainStatus::as_wire)
    )
}

/// The collection a `ListSubscriptions` cursor enumerates: the subscriptions of ONE
/// domain in ONE Region, which is the only collection that listing reports — it narrows
/// by no filter of its own
/// (`smithy_query(query="operation",operation="ListSubscriptions")` reports `domainId`,
/// `maxResults` and `nextToken` as its whole input), so the domain and the Region are the
/// narrowing values the scope carries.
pub fn subscriptions_page_scope(region: &str, domain_id: &str) -> String {
    format!("subscriptions/{region}/{domain_id}")
}

/// The longest `nextToken` this service attempts to decode at all.
///
/// A cursor it mints is two hex characters per plaintext byte of a 32-character tag, a
/// scope built from a Region ([`MAX_REGION_LEN`]) and a modelled id, plus one more
/// modelled id — well inside this bound. The limit exists because the decode allocation
/// would otherwise be sized by what a caller sends.
const MAX_PAGE_TOKEN_LEN: usize = 512;

/// The process-lifetime key that authenticates this service's pagination cursors.
///
/// Drawn from the standard library's randomly seeded hasher, so it is derived from
/// nothing a caller can see or send, never leaves the process and appears in no
/// response: without it the tag over a cursor's plaintext cannot be computed, which is
/// what makes a cursor verifiable as one THIS service issued rather than merely
/// well-formed.
///
/// A cursor therefore does not outlive the process that issued it, which matches the
/// store it is a position into: both live in memory only, so a cursor from an earlier
/// process names a position in a collection that is gone.
fn page_token_key() -> &'static [u8; 32] {
    static KEY: ::std::sync::OnceLock<[u8; 32]> = ::std::sync::OnceLock::new();
    KEY.get_or_init(|| {
        use ::std::hash::BuildHasher as _;

        let seed = ::std::collections::hash_map::RandomState::new();
        let mut key = [0u8; 32];
        for (index, chunk) in key.chunks_mut(8).enumerate() {
            chunk.copy_from_slice(&seed.hash_one(index as u64).to_be_bytes());
        }
        key
    })
}

/// The authentication tag a cursor of `scope` that resumes after `after_key` carries:
/// half a SHA-256 over the process key and the length-delimited pair, so the tag of one
/// collection's cursor is never also another's and a caller can compute neither.
fn page_token_tag(scope: &str, after_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(page_token_key());
    hasher.update((scope.len() as u64).to_be_bytes());
    hasher.update(scope.as_bytes());
    hasher.update(after_key.as_bytes());
    hex::encode(&hasher.finalize()[..16])
}

/// The opaque `nextToken` for a page of `scope` that stopped at `last_key`.
///
/// Hex of `<tag>:<scope>:<key>`, so the token stays opaque to a client yet resumes
/// EXACTLY where the page stopped: the next page starts at the first key strictly after
/// it, whether or not that key still exists. It carries the collection it was issued
/// for and the tag that proves this service issued it — what [`decode_page_token`]
/// checks before any token is used as a resume point.
pub fn encode_page_token(scope: &str, last_key: &str) -> String {
    let tag = page_token_tag(scope, last_key);
    hex::encode(format!("{tag}:{scope}:{last_key}").as_bytes())
}

/// The key a `nextToken` resumes after in `scope`, or `None` when the token is not one
/// this service issued FOR THAT collection — a fabricated token, a token whose tag does
/// not match the key it carries, and a token another listing minted all answer `None`.
///
/// Only the ONE spelling [`encode_page_token`] writes for a position names that position:
/// a token is accepted only when it is byte-identical to the token this service would
/// mint for the collection and key it decodes to, which is the check that covers the
/// scope, the tag and the spelling at once. The hex decode is laxer than the hex encode —
/// it reads `A-F` as well as the `a-f` the minter emits — so without that comparison a
/// re-cased copy of a cursor, a string no response of this service ever carried, would
/// resume a listing.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    if token.len() > MAX_PAGE_TOKEN_LEN {
        return None;
    }
    let plaintext = String::from_utf8(hex::decode(token).ok()?).ok()?;
    // A scope carries no `:`, so the first two fields are the tag and the scope and the
    // remainder is the key, whatever the key itself contains.
    let after_key = plaintext.splitn(3, ':').nth(2)?;
    (encode_page_token(scope, after_key) == token).then(|| after_key.to_string())
}

/// The page-size bound a `@paginated` listing of this service enforces itself, and the
/// page it serves when the caller asks for no `maxResults` (the maximum).
///
/// Treat it as THIS EMULATOR's cap on one page: the generated "Modeled contract" header
/// of each listing lists `maxResults` without a range, so the handler enforces the bound
/// itself (refusing, never clamping) rather than relying on the boundary validator.
/// `smithy_query(query="operation", operation="ListDomains,ListSubscriptions")` does
/// report a `range {min: 1, max: 100}` for the member of BOTH listings, which is where
/// these numbers come from — and why the bound is stated once, here, rather than per
/// handler.
pub const MIN_PAGE_SIZE: i32 = 1;
pub const MAX_PAGE_SIZE: i32 = 100;

/// The input member every listing binds its page size to (`pageSize` of each
/// operation's `@paginated` trait), as a refusal names it.
const PAGE_SIZE_MEMBER: &str = "maxResults";

/// The detail text of the refusal answered for a `nextToken` that is not a cursor this
/// service issued for the collection being listed.
const UNISSUED_PAGE_TOKEN_DETAIL: &str = "nextToken is not a token this service issued";

/// One page of a paginated listing: how many records to serve, and which key to resume
/// strictly after.
///
/// This is the pagination POLICY both listings of this service obey, held once beside the
/// cursor primitives it is built on ([`encode_page_token`] / [`decode_page_token`] and
/// the `*_page_scope` constructors): absent `maxResults` means a full page and a value
/// outside the bound is refused rather than clamped; a `nextToken` is a resume point only
/// when this service issued it FOR THIS collection; and a page issues a cursor only when
/// a matching record is left after it.
///
/// [`Page::resolve`] answers the refusal TEXT rather than a modelled error, because a
/// refusal has to be lifted into the CALLING operation's own error enum, which is a
/// distinct type per operation. The shape that text is built as is the one this service
/// models for a client-input refusal, built once in [`invalid_request`], so both listings
/// refuse a page this service cannot serve with the same shape and the same words.
#[derive(Debug)]
pub struct Page {
    /// The collection this page is a position into, as the `*_page_scope` constructors
    /// spell it: what a cursor is issued for and checked against.
    scope: String,
    size: usize,
    resume_after: Option<String>,
}

impl Page {
    /// The page a request asks for in `scope`, or the detail text of the refusal it
    /// earns.
    ///
    /// The two refusals are reported in the order the request states them: the page size
    /// first, then the cursor.
    pub fn resolve(
        scope: String,
        max_results: Option<i32>,
        next_token: Option<&str>,
    ) -> Result<Self, String> {
        let size = match max_results {
            None => MAX_PAGE_SIZE,
            Some(requested) if (MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&requested) => requested,
            // Worded as AWS words a constraint violation, and in the one place that
            // sentence is written (`crate::service::error_detail`), so this refusal reads
            // identically to the boundary validator's for the same violation.
            Some(requested) => {
                return Err(crate::service::error_detail::range_violation_message(
                    PAGE_SIZE_MEMBER,
                    requested,
                    MIN_PAGE_SIZE,
                    MAX_PAGE_SIZE,
                ))
            }
        };
        let resume_after = match next_token {
            None => None,
            Some(token) => match decode_page_token(&scope, token) {
                Some(key) => Some(key),
                None => return Err(UNISSUED_PAGE_TOKEN_DETAIL.to_string()),
            },
        };
        Ok(Self {
            // The bound above is inside `usize` on every target, so the fallback is
            // unreachable; it is the smallest page rather than an unbounded one.
            size: usize::try_from(size).unwrap_or(1),
            scope,
            resume_after,
        })
    }

    /// How many records this page serves at most.
    pub fn size(&self) -> usize {
        self.size
    }

    /// Whether `key` lies after this page's resume point — the filter a listing applies
    /// to the collection it enumerates.
    ///
    /// A page with no cursor starts at the beginning; a page resuming from one starts
    /// STRICTLY after the key that cursor carries, whether or not that record still
    /// exists.
    pub fn starts_after(&self, key: &str) -> bool {
        self.resume_after
            .as_deref()
            .is_none_or(|after| key > after)
    }

    /// The `nextToken` this page reports: a cursor for `last_key` when a matching record
    /// is left after the page (`more_after_page`), and otherwise none at all — so a
    /// caller is handed a cursor only when there is something to resume to, and an empty
    /// page never carries one.
    pub fn next_token(&self, more_after_page: bool, last_key: Option<&str>) -> Option<String> {
        match (more_after_page, last_key) {
            (true, Some(last)) => Some(encode_page_token(&self.scope, last)),
            _ => None,
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with the modelled server error [`store_unreachable`] builds, never with
/// a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsConnecthealthConnecthealthState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsConnecthealthConnecthealthState>)
}

/// The failure every handler answers an unreachable store with — the one
/// [`service_state`] returning `None` is reported through.
///
/// A store this service cannot reach is a fault on the SERVICE's side: nothing about the
/// request caused it and no member of the request can be changed to avoid it. So it is
/// reported through the shape this service models for a server-side failure, which
/// carries that classification, its status and its retryability on the wire — never
/// through a client-input refusal, which tells the caller to fix a request that was
/// never wrong. Built HERE, once, so every handler words and classifies the condition
/// identically; each lifts it into its own operation's error enum, widening the shape in
/// through `extra_errors` where the operation's own `errors` list omits it.
pub fn store_unreachable(operation: &str) -> InternalServerException {
    InternalServerException {
        message: Some(format!("{operation} could not reach this service's store")),
    }
}

/// The failure every handler answers A REQUEST THIS SERVICE CANNOT SERVE with — a page
/// size outside the bound, a `nextToken` it never issued, a `status` outside the enum, a
/// tag entry or key it does not accept.
///
/// It is the counterpart of [`store_unreachable`] on the CLIENT side: a fault in the
/// request, which the caller fixes by changing a member of it. So it is reported through
/// the shape this service MODELS for invalid client input
/// (`smithy_query(query="shape", shape="ValidationException")`, errorKind client,
/// httpError 400), which six of its operations declare — never through a shape outside
/// the model that merely renders the same status and code, which no client's generated
/// code can deserialise and which cannot carry a member the modelled shape grows. Built
/// HERE, once, so every handler words and classifies the condition identically; each
/// lifts it into its own operation's error enum, widening the shape in through
/// `extra_errors` where the operation's own `errors` list omits it.
pub fn invalid_request(detail: impl Into<String>) -> ValidationException {
    ValidationException {
        message: Some(detail.into()),
    }
}
