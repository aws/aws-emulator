// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-s3control-awss3controlservicev20180820 service.
//!
//! Besides the state struct itself this module holds the derivations every
//! handler shares — the access point ARN, alias, network origin and
//! `ListAccessPoints` page token, the Multi-Region Access Point alias and the
//! asynchronous request token ARN, the Storage Lens configuration and Storage
//! Lens group ARNs, and the refusals every resource lifecycle answers — so the
//! handlers under `src/ops/` compute each one way instead of once per handler.

use crate::ops::types::{
    AccessGrantsLocationConfiguration, AccessPoint, AsyncErrorDetails, AsyncOperation,
    AsyncRequestParameters, AsyncResponseDetails, CreateJobInput,
    CreateMultiRegionAccessPointInput2, DeleteMultiRegionAccessPointInput2,
    EstablishedMultiRegionAccessPointPolicy, Grantee, JobDescriptor, JobFailure, JobListDescriptor,
    JobManifest, JobManifestGenerator, JobOperation, JobProgressSummary, JobReport, JobTimers,
    ListStorageLensConfigurationEntry, MultiRegionAccessPointPolicyDocument,
    MultiRegionAccessPointRegionalResponse, MultiRegionAccessPointReport,
    MultiRegionAccessPointRoute, MultiRegionAccessPointsAsyncResponse, PolicyStatus,
    ProposedMultiRegionAccessPointPolicy, PublicAccessBlockConfiguration,
    PutMultiRegionAccessPointPolicyInput2, RegionReport, S3GeneratedManifestDescriptor, S3Tag,
    StorageLensConfiguration, StorageLensGroup, StorageLensTag, Tag, VpcConfiguration,
    ASYNC_OPERATION_NAME_VALUES, JOB_STATUS_VALUES, MULTI_REGION_ACCESS_POINT_STATUS_VALUES,
    NETWORK_ORIGIN_VALUES, OPERATION_NAME_VALUES,
};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Publishing an access point's DATA PATH into the object store that serves it.
pub mod access_point_routes;
/// The ACCOUNT-level `PublicAccessBlock` configuration, held in the object store
/// that enforces it.
pub mod account_public_access_block;
/// The paging every S3 Access Grants listing reads: the page size a request asked
/// for, and where a continuation token resumes the scan.
pub mod access_grants_listing;
/// Reading a job's manifest and rendering what its run writes back — the
/// derivations the worker below runs over, held apart from it because they are
/// pure functions of a manifest, a filter and a set of task outcomes.
pub mod batch_job_plan;
/// Whether S3 Batch Operations can assume the role a job's tasks run as, read
/// from the account's own IAM records.
pub mod batch_job_role;
/// Running a submitted S3 Batch Operations job's tasks, off the response path.
pub mod batch_job_worker;
/// Where a bucket this service associates actually lives, read from the object
/// store that owns the bucket.
pub mod bucket_regions;

/// One stored access point: everything `GetAccessPoint`, `ListAccessPoints` and
/// `GetAccessPointPolicy` report, recorded by `CreateAccessPoint`.
#[derive(Debug, Clone)]
pub struct AccessPointRecord {
    pub name: String,
    /// The account that owns the access point — the first half of the key, kept
    /// on the record so a record passed on its own still names its owner.
    pub account_id: String,
    pub bucket: String,
    /// The account that owns the BUCKET this access point fronts — the account
    /// the create request named in `BucketAccountId`, or the account that owns
    /// the access point when it named none ([`effective_bucket_account_id`]).
    /// Not an `Option`: a bucket always has an owning account, so every read
    /// reports it.
    pub bucket_account_id: String,
    pub arn: String,
    /// The Region the access point lives in — the one the create request was
    /// signed for. Recorded because the access point's data-plane endpoints are
    /// Regional ([`access_point_endpoints`]) and its ARN names the same Region.
    pub region: String,
    pub alias: String,
    /// What this access point fronts, as `AccessPointType` reports it: one of
    /// [`ACCESS_POINT_TYPE_REGULAR`]. Recorded by the create, which is what
    /// decides it, so every read projects the stored value rather than deciding
    /// again.
    pub access_point_type: String,
    /// One of [`NETWORK_ORIGIN_VALUES`] — `VPC` when the access point is
    /// VPC-restricted, otherwise `Internet`.
    pub network_origin: String,
    pub vpc_configuration: Option<VpcConfiguration>,
    /// The access point's EFFECTIVE public-access block settings — the
    /// configuration the create request supplied, or
    /// [`default_public_access_block`] when it supplied none. Not an `Option`:
    /// an access point always has these settings, so every read reports them.
    pub public_access_block_configuration: PublicAccessBlockConfiguration,
    pub creation_date: ::aws_smithy_types::DateTime,
    /// The tags `CreateAccessPoint` was given. Recorded here because they are
    /// part of the resource; the operations that report them
    /// (`ListTagsForResource`, `TagResource`, `UntagResource`) are still stubs
    /// with their own gap issues, and will read this field rather than a second
    /// copy.
    pub tags: Vec<Tag>,
    /// The document the last `PutAccessPointPolicy` stored, byte for byte;
    /// `None` until one is put, and `None` again once
    /// `DeleteAccessPointPolicy` removes it, which is what
    /// `GetAccessPointPolicy` reports as `NoSuchAccessPointPolicy`.
    pub policy: Option<String>,
}

impl AccessPointRecord {
    /// The record as the `AccessPoint` summary shape `ListAccessPoints` returns.
    pub fn summary(&self) -> AccessPoint {
        AccessPoint {
            name: self.name.clone(),
            network_origin: self.network_origin.clone(),
            vpc_configuration: self.vpc_configuration.clone(),
            bucket: self.bucket.clone(),
            access_point_arn: Some(self.arn.clone()),
            alias: Some(self.alias.clone()),
            bucket_account_id: Some(self.bucket_account_id.clone()),
            // S3 on Outposts / FSx data sources are not emulated, so a record
            // created here is attached to no data source.
            data_source_id: None,
            data_source_type: None,
            access_point_type: Some(self.access_point_type.clone()),
        }
    }
}

/// The type an access point over a general-purpose bucket reports, which is
/// the one kind `CreateAccessPoint` makes here: a create that would make
/// another — one scoped to a directory bucket, one on an Outpost, one attached
/// to a data source — is refused rather than served, so no record carries
/// another type.
pub const ACCESS_POINT_TYPE_REGULAR: &str = "Regular";

/// The account that owns the bucket an access point fronts: the one the create
/// request named, or — when it named none — the account that owns the access
/// point.
///
/// `CreateAccessPoint$BucketAccountId` states the rule: "For same account
/// access point when your bucket and access point belong to the same account
/// owner, the BucketAccountId is not required. For cross-account access point
/// when your bucket and access point are not in the same account, the
/// BucketAccountId is required." So an absent member means the two accounts
/// are the SAME one, not that the bucket has no owner — which is why every
/// read reports the owning account whether the create named it or not.
pub fn effective_bucket_account_id(requested: Option<&str>, account_id: &str) -> String {
    requested.unwrap_or(account_id).to_string()
}

/// The public-access block settings an access point resource carries when the
/// request that created it supplied none: all four blocked.
///
/// Amazon S3 blocks public access to a newly created access point by default —
/// every one of the four settings of `PublicAccessBlockConfiguration` is
/// enabled unless the create request asks otherwise — so the settings are part
/// of the resource from the moment it exists and a read never reports it
/// without them.
pub fn default_public_access_block() -> PublicAccessBlockConfiguration {
    PublicAccessBlockConfiguration {
        block_public_acls: true,
        ignore_public_acls: true,
        block_public_policy: true,
        restrict_public_buckets: true,
    }
}

/// The public-access block settings a create request leaves on the resource it
/// creates: exactly the configuration it supplied, or
/// [`default_public_access_block`] when it supplied none.
pub fn effective_public_access_block(
    requested: Option<&PublicAccessBlockConfiguration>,
) -> PublicAccessBlockConfiguration {
    requested
        .cloned()
        .unwrap_or_else(default_public_access_block)
}

/// The condition keys that, named in a statement's `Condition`, fix WHO the
/// statement lets in and so keep a wildcard principal from being public.
///
/// Amazon S3 reads a resource policy as public when it grants access to a
/// wildcard principal, except where the statement's condition pins the
/// requester — its identity, its account, its organization, or the source the
/// call comes through. Compared case-insensitively, as IAM compares condition
/// keys.
const REQUESTER_NARROWING_CONDITION_KEYS: [&str; 10] = [
    "aws:principalarn",
    "aws:principalaccount",
    "aws:principalorgid",
    "aws:principalorgpaths",
    "aws:sourcearn",
    "aws:sourceaccount",
    "aws:sourceowner",
    "aws:sourcevpc",
    "aws:sourcevpce",
    "s3:dataaccesspointarn",
];

/// Whether a statement's `Principal` (or `NotPrincipal`) lets ANY principal in:
/// the `"*"` wildcard, bare or under the `AWS` key, alone or among named
/// principals.
fn principal_is_wildcard(principal: &::serde_json::Value) -> bool {
    match principal {
        ::serde_json::Value::String(named) => named == "*",
        ::serde_json::Value::Array(named) => named.iter().any(principal_is_wildcard),
        ::serde_json::Value::Object(by_kind) => by_kind.values().any(principal_is_wildcard),
        _ => false,
    }
}

/// Whether a statement's `Condition` pins the requester, which is what keeps a
/// wildcard principal from granting public access.
fn condition_narrows_requester(condition: &::serde_json::Value) -> bool {
    let Some(operators) = condition.as_object() else {
        return false;
    };
    operators.values().any(|keys| {
        keys.as_object().is_some_and(|keys| {
            keys.keys().any(|key| {
                REQUESTER_NARROWING_CONDITION_KEYS.contains(&key.to_lowercase().as_str())
            })
        })
    })
}

/// Whether one statement of a resource policy grants public access.
fn statement_grants_public_access(statement: &::serde_json::Value) -> bool {
    let Some(statement) = statement.as_object() else {
        return false;
    };
    // Only an `Allow` grants anything; a `Deny` never makes a policy public.
    if statement.get("Effect").and_then(|effect| effect.as_str()) != Some("Allow") {
        return false;
    }
    // `NotPrincipal` on an `Allow` grants everyone it does not name, so it is
    // as public as a wildcard `Principal`.
    let open_to_anyone = statement
        .get("Principal")
        .is_some_and(principal_is_wildcard)
        || statement.contains_key("NotPrincipal");
    if !open_to_anyone {
        return false;
    }
    !statement
        .get("Condition")
        .is_some_and(condition_narrows_requester)
}

/// Whether a resource policy document grants public access — the derivation
/// `PolicyStatus$IsPublic` reports, and the one a `BlockPublicPolicy` setting
/// refuses a put of.
///
/// A document that is not a policy object, or whose statements grant nothing to
/// anyone, is not public; a document that is not JSON at all grants nothing
/// either (`PutAccessPointPolicy` refuses it as malformed before it is ever
/// stored).
pub fn policy_grants_public_access(document: &str) -> bool {
    let Ok(policy) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    let Some(statements) = policy.get("Statement") else {
        return false;
    };
    match statements {
        ::serde_json::Value::Array(statements) => {
            statements.iter().any(statement_grants_public_access)
        }
        // A single statement may be written as the object itself.
        one => statement_grants_public_access(one),
    }
}

/// How many access points the store holds before `CreateAccessPoint` refuses
/// with `TooManyAccessPoints`.
///
/// S3 Control publishes an adjustable quota on access points per account per
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops the state growing without end: every record it
/// holds is released by `DeleteAccessPoint`, which drops that access point's
/// policy with it, so together with [`MAX_POLICY_BYTES`] the bytes this store
/// can hold are bounded.
pub const ACCESS_POINT_QUOTA: usize = 1_000;

/// The largest policy document `PutAccessPointPolicy` accepts and retains, in
/// bytes.
///
/// Amazon S3 documents a 20 KB maximum for a bucket or access point policy, so a
/// document past it is one the real service would not store either. It is also
/// what bounds the bytes a single record can hold.
pub const MAX_POLICY_BYTES: usize = 20 * 1024;

/// How a stored access point is keyed: the owning account id and the access
/// point name, which is the pair a name is unique within.
pub type AccessPointKey = (String, String);

/// How many accounts' `PublicAccessBlock` configurations the store holds before
/// `PutPublicAccessBlock` refuses to record one for a FURTHER account.
///
/// This is the emulator's OWN bound on its store, not a claim about any limit
/// AWS publishes: every account has exactly one such configuration, and
/// `AccountId` is constrained only to `^\d{12}$`, which bounds the shape of a
/// key and not how many of them a caller can name. Without this bound a caller
/// could grow this process's state without end by varying `AccountId`. It is
/// checked only when a request would ADD an account's configuration, so every
/// account already in the store stays writable, and `DeletePublicAccessBlock`
/// releases a slot.
pub const PUBLIC_ACCESS_BLOCK_QUOTA: usize = 1_000;

/// One stored S3 Storage Lens configuration: the document the last
/// `PutStorageLensConfiguration` stored, and the tag set that configuration
/// carries.
#[derive(Debug, Clone)]
pub struct StorageLensConfigurationRecord {
    /// The configuration document as the last put left it — stored WHOLE, never
    /// merged with the document before it, with `StorageLensArn` set to the ARN
    /// this service assigns (that member is documented read-only).
    pub configuration: StorageLensConfiguration,
    /// The configuration's tag set.
    ///
    /// Held BESIDE the document rather than inside it, because the two are
    /// separate resources on the wire: `GetStorageLensConfiguration` reports no
    /// tags at all, and `GetStorageLensConfigurationTagging` /
    /// `PutStorageLensConfigurationTagging` are what report and replace them.
    pub tags: Vec<StorageLensTag>,
    /// The configuration's S3 Storage Lens HOME Region: the Region the put that
    /// stored this document was signed for, which is also the Region segment of
    /// the ARN this service assigned it
    /// ([`storage_lens_configuration_arn`]).
    ///
    /// Recorded on the record because the configuration document carries no
    /// member for it while `ListStorageLensConfigurationEntry$HomeRegion` is
    /// required: "Your metrics data is stored and retained in your designated S3
    /// Storage Lens home Region."
    pub home_region: String,
}

impl StorageLensConfigurationRecord {
    /// The record as the `ListStorageLensConfigurationEntry` summary
    /// `ListStorageLensConfigurations` returns: the configuration's id, the ARN
    /// this service assigned it, its home Region, and whether it is enabled.
    ///
    /// The summary carries no part of the document's metric selections and no
    /// tag: those are reported by `GetStorageLensConfiguration` and the tagging
    /// operations, which is what the entry shape declares.
    pub fn entry(&self) -> ListStorageLensConfigurationEntry {
        ListStorageLensConfigurationEntry {
            id: self.configuration.id.clone(),
            // `StorageLensArn` is required on the entry, and every stored
            // configuration carries the ARN its put assigned; it is recomputed
            // from the record's own identity if a record ever lacks one, so the
            // entry is never reported with an empty ARN.
            storage_lens_arn: self
                .configuration
                .storage_lens_arn
                .clone()
                .unwrap_or_else(|| {
                    storage_lens_configuration_arn(&self.home_region, &self.configuration.id)
                }),
            home_region: self.home_region.clone(),
            is_enabled: self.configuration.is_enabled,
        }
    }
}

/// How a stored S3 Storage Lens configuration is keyed: the owning account id
/// and the configuration id, which is the pair an id is unique within.
pub type StorageLensConfigurationKey = (String, String);

/// How many S3 Storage Lens configurations the store holds before
/// `PutStorageLensConfiguration` refuses to add another.
///
/// S3 Storage Lens limits how many configurations an account may hold in a
/// Region, and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an
/// emulator serves, and together with [`MAX_STORAGE_LENS_TAGS`] it is what
/// stops this part of the state growing without end. (The record a put adds is
/// released by `DeleteStorageLensConfiguration`, so a caller that deletes what
/// it no longer needs never reaches the bound.)
pub const STORAGE_LENS_CONFIGURATION_QUOTA: usize = 1_000;

/// The largest tag set an S3 Storage Lens configuration carries.
///
/// The model documents it on both members that carry a tag set
/// (`PutStorageLensConfigurationRequest$Tags` and
/// `PutStorageLensConfigurationTaggingRequest$Tags`): "You can set up to a
/// maximum of 50 tags."
pub const MAX_STORAGE_LENS_TAGS: usize = 50;

/// The status an operation of this service whose response carries NO document
/// answers a success with: `204 No Content`.
///
/// This holds for every member whose modelled output is `smithy.api#Unit`: the
/// model declares no response shape at all for it, so a success has nothing to
/// put in a body, and the wire answers No Content. The model's `http.code` for
/// most of them is the Smithy default of 200, which is stale for a member that
/// carries no body — and the model itself codes the same empty-body answer 204
/// where it states a code (`CreateStorageLensGroup`,
/// `DeleteBucketTagging`, `DeleteStorageLensGroup`, `UpdateStorageLensGroup`,
/// `TagResource`, `UntagResource`). A 200 there promises a document the
/// response does not carry, so a client that distinguishes the two — one that
/// parses a body only when there is one, or that asserts the status it was
/// told — takes the wrong branch.
///
/// The operations that DO report a document keep the modelled 200, because they
/// answer one.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

/// How many entries one page of `ListStorageLensConfigurations` carries.
///
/// The operation is `@paginated` and its request models NO page-size member, so
/// the service alone decides how many entries a page holds: its
/// `NextToken` is documented as returned "if the request produced more than the
/// maximum number of S3 Storage Lens configuration results". AWS publishes no
/// number for that maximum, so this is the emulator's OWN page size — what it
/// fixes is that a caller holding more configurations than one page carries is
/// handed a continuation token rather than a truncated answer.
pub const STORAGE_LENS_CONFIGURATIONS_PAGE_SIZE: usize = 100;

/// One stored S3 Storage Lens group: the grouping document the group was
/// created with, and the Amazon Web Services resource tags it carries.
#[derive(Debug, Clone)]
pub struct StorageLensGroupRecord {
    /// The group document as the create call left it, with
    /// `StorageLensGroupArn` set to the ARN this service assigns (that member
    /// is documented read-only).
    pub group: StorageLensGroup,
    /// The resource tags the group carries.
    ///
    /// Held BESIDE the document rather than inside it, because the two are
    /// separate resources on the wire: `GetStorageLensGroup` reports the
    /// document and no tags at all, and the generic resource tagging operations
    /// (`TagResource`, `ListTagsForResource`, `UntagResource`) are what report
    /// and replace them. Those three are still scaffolds with their own gap
    /// issues, and will read this field rather than a second copy.
    pub tags: Vec<Tag>,
}

/// How a stored S3 Storage Lens group is keyed: the owning account id and the
/// group name, which is the pair a name is unique within.
pub type StorageLensGroupKey = (String, String);

/// How many S3 Storage Lens groups the store holds before
/// `CreateStorageLensGroup` refuses to add another.
///
/// S3 Storage Lens limits how many groups an account may hold in a Region, and
/// this emulator models neither Service Quotas nor an account's adjustments —
/// so this is the emulator's OWN bound on the store rather than a claim about
/// any account's limit, and it counts every record the store holds rather than
/// one account's: `AccountId` is a request member constrained only to
/// `^\d{12}$`, which bounds the shape of a key and not how many of them a
/// caller can name. It is set far above any workflow an emulator serves, and
/// together with the model's own length bounds on every member of the group
/// document and on its tag set (`TagList`, len 0..50) it is what stops this
/// part of the state growing without end. (The record a create adds is released
/// by `DeleteStorageLensGroup`, which is still a scaffold with its own gap
/// issue; until it is written the bound is the only limit.)
pub const STORAGE_LENS_GROUP_QUOTA: usize = 1_000;

// ---------------------------------------------------------------------------
// S3 Batch Operations jobs
//
// `CreateJob` records a job here and every job operation reads the one record
// it stored: `DescribeJob` reports it, `UpdateJobPriority` replaces its
// priority, the job tagging operations read and replace its tag set, and
// `UpdateJobStatus` moves its status.
// ---------------------------------------------------------------------------

/// `JobStatus` of a job the service has accepted and not yet prepared — the
/// status a `CreateJob` that needs no confirmation leaves the job in.
pub const JOB_STATUS_NEW: &str = "New";

/// `JobStatus` of a job that has been prepared and is waiting for its tasks to
/// be run.
pub const JOB_STATUS_READY: &str = "Ready";

/// `JobStatus` of a job that will not run because its cancellation has been
/// carried out — a terminal status, reached only through `UpdateJobStatus`, and
/// the status whose `TerminationDate` is when the cancellation was asked for.
pub const JOB_STATUS_CANCELLED: &str = "Cancelled";

/// `JobStatus` of a job that is waiting for confirmation before it runs, which
/// is what `JobDescriptor$SuspendedCause` documents a created job enters when
/// confirmation is required: "When you create the job, it enters the Suspended
/// state to await confirmation before running."
pub const JOB_STATUS_SUSPENDED: &str = "Suspended";

/// The `SuspendedCause` of a job held in [`JOB_STATUS_SUSPENDED`] by its own
/// `ConfirmationRequired`, from the sentence above: it is awaiting
/// confirmation. `SuspendedCause` is a plain string in the model (len 1..1024),
/// so the model names no catalog of causes.
pub const SUSPENDED_AWAITING_CONFIRMATION: &str = "AwaitingConfirmation";

/// The `JobStatus` values a job can still be cancelled from: those of a job
/// that has neither ended nor begun ending.
///
/// The model's `JobStatus` enum names thirteen statuses; the six left out here
/// — `Cancelling` and `Cancelled`, `Completing` and `Complete`, `Failing` and
/// `Failed` — are a job whose run is already ending or over, so there is
/// nothing left for a cancellation to stop. Spelled as the model spells them,
/// and read through [`job_cancellable`].
pub const CANCELLABLE_JOB_STATUSES: &[&str] = &[
    "Active",
    "New",
    "Paused",
    "Pausing",
    "Preparing",
    "Ready",
    "Suspended",
];

/// Whether a job in `status` can still be cancelled — see
/// [`CANCELLABLE_JOB_STATUSES`].
pub fn job_cancellable(status: &str) -> bool {
    CANCELLABLE_JOB_STATUSES.contains(&status)
}

/// How long a created job takes to leave [`JOB_STATUS_NEW`].
///
/// A span chosen by this emulator (AWS documents no duration for it), inside
/// the 250–750 ms band this project fixes for a transition the clock owes. It
/// is never zero, so a caller that describes the job straight after creating it
/// still sees `New`.
pub const JOB_PREPARATION: Duration = Duration::from_millis(500);

/// `JobStatus` of a job whose tasks are being run.
pub const JOB_STATUS_ACTIVE: &str = "Active";

/// `JobStatus` of a job a caller has paused — no run is owed to it until it is
/// resumed.
pub const JOB_STATUS_PAUSED: &str = "Paused";

/// `JobStatus` of a job whose tasks have all been run — the terminal status a
/// job reaches however many of its individual tasks failed, because the job
/// itself did what it was created to do.
pub const JOB_STATUS_COMPLETE: &str = "Complete";

/// `JobStatus` of a job that could not be run at all: its role could not be
/// assumed, the manifest could not be read, or the action is one this emulator
/// does not carry out. What stopped it is reported in
/// `JobDescriptor$FailureReasons`.
pub const JOB_STATUS_FAILED: &str = "Failed";

/// The `JobStatus` values a job has ENDED in: nothing moves a job out of one of
/// them, so a caller polling for the outcome stops here.
///
/// The model's `JobStatus` enum also names the three transient statuses on the
/// way into these (`Cancelling`, `Completing`, `Failing`); a job of this store
/// reaches the ended status itself, so they are not among them.
pub const TERMINAL_JOB_STATUSES: &[&str] =
    &[JOB_STATUS_COMPLETE, JOB_STATUS_FAILED, JOB_STATUS_CANCELLED];

/// Whether a job in `status` has ended — see [`TERMINAL_JOB_STATUSES`].
pub fn job_terminal(status: &str) -> bool {
    TERMINAL_JOB_STATUSES.contains(&status)
}

/// How long after a created job's submission its RUN begins — the instant the
/// job leaves the statuses it waits in for [`JOB_STATUS_ACTIVE`], after which
/// its tasks are carried out and it settles.
///
/// A span chosen by this emulator: AWS documents no duration for it, and on the
/// real service a job takes minutes. It is never zero, so a caller that
/// describes the job straight after creating it still sees the job waiting, and
/// it is long enough that the statuses before the run are observable to a
/// caller polling at a client's pace.
pub const JOB_RUN_AFTER: Duration = Duration::from_secs(3);

/// How many tasks one job of this store runs.
///
/// S3 Batch Operations itself bounds a job by the size of its manifest rather
/// than by a task count, so this is the emulator's OWN bound on the work one
/// job can ask this process for — what stops a caller spending this process's
/// time on a single manifest. A job whose object list is longer is FAILED with
/// that reason rather than run over part of it.
pub const MAX_JOB_TASKS: usize = 1_000;

/// How long after a job has ended it is still reported by a job listing.
///
/// `ListJobs` "lists current S3 Batch Operations jobs as well as the jobs that
/// have ended within the last 90 days for the Amazon Web Services account
/// making the request" — so a job that ended longer ago than this is past what
/// a listing reports, while a job that has not ended is reported however old it
/// is.
pub const JOB_LISTING_WINDOW: Duration = Duration::from_secs(90 * 24 * 60 * 60);

/// How many jobs the store holds before `CreateJob` refuses another.
///
/// S3 Batch Operations publishes an adjustable quota on jobs, and this emulator
/// models neither Service Quotas nor an account's adjustments — so this is the
/// emulator's OWN bound on the store rather than a claim about any account's
/// limit. It is set far above any workflow an emulator serves, and it is what
/// stops the job store growing without end: every member of a job record is
/// length-bounded by the model, and a repeated `CreateJob` carrying one
/// `ClientRequestToken` adds no record at all.
pub const JOB_QUOTA: usize = 1_000;

/// How many tags a job's tag set holds: "Amazon S3 limits the maximum number
/// of tags to 50 tags per job" (`PutJobTagging`), so a tag set past it is one
/// the real service would not store either.
///
/// It is also what bounds the bytes one job record's tag set can hold: at most
/// this many tags, each a modelled key (len 1..128) and value (len 0..256).
pub const MAX_JOB_TAGS: usize = 50;

/// How a stored job is keyed: the owning account id and the job id, which is
/// the pair a job id is unique within.
pub type JobKey = (String, String);

/// One stored S3 Batch Operations job: every member `DescribeJob` reports,
/// recorded by `CreateJob`.
#[derive(Debug, Clone)]
pub struct JobRecord {
    /// The service-assigned job id, which is also the second half of the key.
    pub job_id: String,
    /// The account that created the job — the first half of the key, kept on
    /// the record so a record passed on its own still names its owner.
    pub account_id: String,
    /// The job's ARN, assigned by this service (see [`job_arn`]).
    pub arn: String,
    /// The job's current status: one of [`JOB_STATUS_VALUES`].
    pub status: String,
    pub confirmation_required: Option<bool>,
    pub description: Option<String>,
    /// The manifest the create named, if it named one rather than a generator.
    pub manifest: Option<JobManifest>,
    /// The manifest generator the create named, if it named one rather than a
    /// manifest.
    pub manifest_generator: Option<JobManifestGenerator>,
    pub operation: JobOperation,
    pub priority: i32,
    pub report: JobReport,
    pub role_arn: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub suspended_date: Option<::aws_smithy_types::DateTime>,
    pub suspended_cause: Option<String>,
    /// The reason carried by the last `UpdateJobStatus`; `None` until one
    /// arrives, which is what `DescribeJob` reports as an absent member.
    pub status_update_reason: Option<String>,
    /// When the job reached a terminal status; `None` while it has not.
    pub termination_date: Option<::aws_smithy_types::DateTime>,
    /// The job's tag set, as `CreateJob` was given it and as the job tagging
    /// operations last left it: `PutJobTagging` replaces it whole,
    /// `DeleteJobTagging` empties it and `GetJobTagging` reports it. Bounded
    /// by [`MAX_JOB_TAGS`], and the one copy all four operations share.
    pub tags: Vec<S3Tag>,
    /// The `ClientRequestToken` the create carried, which is what makes a
    /// repeated create idempotent rather than a second job.
    pub client_request_token: String,
    /// A digest of the create parameters BESIDE the token (see
    /// [`create_job_fingerprint`]), so a repeat of the same token can be told
    /// from a different request reusing it.
    pub request_fingerprint: String,
    /// When the transition out of [`JOB_STATUS_NEW`] falls due; `None` once it
    /// has been materialised (or when the job was never in `New`). Stored as an
    /// instant rather than slept for.
    pub status_due_at: Option<SystemTime>,
    /// How many tasks the job's object list came to — `None` until the run has
    /// read the manifest (or generated one), which is what
    /// `ProgressSummary$TotalNumberOfTasks` reports ("the total number of tasks
    /// that the specified job has run"). A job reports no progress summary
    /// until there is an object list to account for.
    pub total_tasks: Option<i64>,
    /// How many of those tasks the run carried out, and how many it could not:
    /// `ProgressSummary$NumberOfTasksSucceeded` and `NumberOfTasksFailed`,
    /// counted as the tasks run rather than derived from the total.
    pub tasks_succeeded: i64,
    /// See [`Self::tasks_succeeded`].
    pub tasks_failed: i64,
    /// How long the job was `Active`, in seconds — `JobTimers$
    /// ElapsedTimeInActiveSeconds` — recorded when the run ends, so the value a
    /// read reports is the one the run measured rather than one computed per
    /// read. `None` until then.
    pub elapsed_active_seconds: Option<i64>,
    /// Why the job could not be run at all, if it could not: what
    /// `JobDescriptor$FailureReasons` reports. Empty for a job whose run was
    /// carried out, however many of its individual tasks failed — a task
    /// failure is accounted in [`Self::tasks_failed`], not here.
    pub failures: Vec<JobFailure>,
    /// The manifest the job's own generator WROTE, once it has written one —
    /// `JobDescriptor$GeneratedManifestDescriptor`. `None` for a job whose
    /// object list came from a manifest the request named, and for a generator
    /// that was not asked to write its manifest out.
    pub generated_manifest: Option<S3GeneratedManifestDescriptor>,
}

impl JobRecord {
    /// The record as the `JobDescriptor` shape `DescribeJob` returns.
    ///
    /// `ProgressSummary` and `FailureReasons` report what the job's RUN
    /// accounted for ([`crate::state::batch_job_worker`]): the object list's
    /// task count and the outcome of each task once the run has them, and
    /// nothing at all before, so neither is a claim about work nothing did.
    pub fn descriptor(&self) -> JobDescriptor {
        JobDescriptor {
            job_id: Some(self.job_id.clone()),
            confirmation_required: self.confirmation_required,
            description: self.description.clone(),
            job_arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            manifest: self.manifest.clone(),
            operation: Some(self.operation.clone()),
            priority: self.priority,
            progress_summary: self.progress_summary(),
            status_update_reason: self.status_update_reason.clone(),
            failure_reasons: (!self.failures.is_empty()).then(|| self.failures.clone()),
            report: Some(self.report.clone()),
            creation_time: Some(self.creation_time),
            termination_date: self.termination_date,
            role_arn: Some(self.role_arn.clone()),
            suspended_date: self.suspended_date,
            suspended_cause: self.suspended_cause.clone(),
            manifest_generator: self.manifest_generator.clone(),
            // A generated manifest is described only once the job's generator
            // has actually written one out.
            generated_manifest_descriptor: self.generated_manifest.clone(),
        }
    }

    /// What the job's run has accounted for, or `None` while it has no object
    /// list to account for yet.
    ///
    /// `TotalNumberOfTasks` is the length of that list, and the two counters
    /// are the outcomes the run recorded as it went, so a job read mid-run
    /// reports the tasks already carried out rather than the whole list as
    /// done. `Timers` carries the measured active span once the run has ended.
    pub fn progress_summary(&self) -> Option<JobProgressSummary> {
        let total = self.total_tasks?;
        Some(JobProgressSummary {
            total_number_of_tasks: Some(total),
            number_of_tasks_succeeded: Some(self.tasks_succeeded),
            number_of_tasks_failed: Some(self.tasks_failed),
            timers: self
                .elapsed_active_seconds
                .map(|elapsed| JobTimers {
                    elapsed_time_in_active_seconds: Some(elapsed),
                }),
        })
    }

    /// The record as the `JobListDescriptor` summary shape a job listing
    /// returns.
    ///
    /// A SUMMARY of the same record [`Self::descriptor`] reports in full, so the
    /// two cannot disagree about one job: every member `JobListDescriptor`
    /// declares is the stored one, and `ProgressSummary` is the same derivation
    /// — what the job's run accounted for, and nothing before it has an object
    /// list to account for.
    pub fn list_descriptor(&self) -> JobListDescriptor {
        JobListDescriptor {
            job_id: Some(self.job_id.clone()),
            description: self.description.clone(),
            operation: job_operation_name(&self.operation),
            priority: self.priority,
            status: Some(self.status.clone()),
            creation_time: Some(self.creation_time),
            termination_date: self.termination_date,
            progress_summary: self.progress_summary(),
        }
    }

    /// Whether a job listing reports this job at `now`: a job that has not ended
    /// is always reported, and one that has ended is reported until
    /// [`JOB_LISTING_WINDOW`] has passed since it ended.
    pub fn within_listing_window(&self, now: SystemTime) -> bool {
        let Some(terminated) = self.termination_date else {
            return true;
        };
        let Ok(now_secs) = i64::try_from(
            now.duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
        ) else {
            return true;
        };
        let window = i64::try_from(JOB_LISTING_WINDOW.as_secs()).unwrap_or(i64::MAX);
        now_secs.saturating_sub(terminated.secs()) <= window
    }

    /// Apply the transition the clock already owed this job at `now`: a job
    /// leaves [`JOB_STATUS_NEW`] for [`JOB_STATUS_READY`] once its preparation
    /// span has passed.
    ///
    /// This materialises a change that was due at the same instant whether or
    /// not anybody called — it is not a move made because the job was read.
    fn materialize_due(&mut self, now: SystemTime) {
        if self.status != JOB_STATUS_NEW {
            return;
        }
        if self.status_due_at.is_some_and(|due| due <= now) {
            self.status = job_status(JOB_STATUS_READY);
            self.status_due_at = None;
        }
    }
}

/// The spelling of a job status that goes on the wire: the one the generated
/// [`JOB_STATUS_VALUES`] carries (from the model's `JobStatus` enum), found by
/// VALUE rather than by position, so neither a reordering of the generated list
/// nor a typo in a constant above can change what is returned.
pub fn job_status(wanted: &'static str) -> String {
    modelled_value(JOB_STATUS_VALUES, wanted)
}

/// The actions `operation` names, by the member names `JobOperation` declares.
///
/// `JobOperation` is "the action that you want this job to perform on every
/// object listed in the manifest" — ONE action, carried in one of the
/// structure's otherwise-optional members, so this is what lets `CreateJob`
/// refuse a request naming none or several, and what lets a job listing report
/// the one action a stored job runs.
///
/// Destructured rather than field-accessed, so an action the model adds to
/// `JobOperation` is a compile error here rather than an action silently
/// accepted beside another.
pub fn job_operation_names(operation: &JobOperation) -> Vec<&'static str> {
    let JobOperation {
        lambda_invoke,
        s3_put_object_copy,
        s3_put_object_acl,
        s3_put_object_tagging,
        s3_delete_object_tagging,
        s3_initiate_restore_object,
        s3_put_object_legal_hold,
        s3_put_object_retention,
        s3_replicate_object,
        s3_compute_object_checksum,
        s3_update_object_encryption,
    } = operation;
    [
        ("LambdaInvoke", lambda_invoke.is_some()),
        ("S3PutObjectCopy", s3_put_object_copy.is_some()),
        ("S3PutObjectAcl", s3_put_object_acl.is_some()),
        ("S3PutObjectTagging", s3_put_object_tagging.is_some()),
        ("S3DeleteObjectTagging", s3_delete_object_tagging.is_some()),
        (
            "S3InitiateRestoreObject",
            s3_initiate_restore_object.is_some(),
        ),
        ("S3PutObjectLegalHold", s3_put_object_legal_hold.is_some()),
        ("S3PutObjectRetention", s3_put_object_retention.is_some()),
        ("S3ReplicateObject", s3_replicate_object.is_some()),
        (
            "S3ComputeObjectChecksum",
            s3_compute_object_checksum.is_some(),
        ),
        (
            "S3UpdateObjectEncryption",
            s3_update_object_encryption.is_some(),
        ),
    ]
    .into_iter()
    .filter(|(_, named)| *named)
    .map(|(name, _)| name)
    .collect()
}

/// The `OperationName` of the one action a job runs, spelled as the model's own
/// enum carries it — the member name of the single [`job_operation_names`] entry
/// the job's `JobOperation` names.
///
/// `None` when the structure names no single action, which `CreateJob` refuses
/// to record: a listing then reports no operation rather than one of several.
pub fn job_operation_name(operation: &JobOperation) -> Option<String> {
    let mut named = job_operation_names(operation).into_iter();
    let only = named.next()?;
    named
        .next()
        .is_none()
        .then(|| modelled_value(OPERATION_NAME_VALUES, only))
}

// ---------------------------------------------------------------------------
// Multi-Region Access Points, and the asynchronous requests that manage them.
// ---------------------------------------------------------------------------

/// How long a Multi-Region Access Point stays in the transient status an
/// asynchronous request put it in before the clock owes it the settled outcome:
/// `CREATING` becomes `READY`, and `DELETING` releases the record.
///
/// `CreateMultiRegionAccessPoint` and `DeleteMultiRegionAccessPoint` are
/// documented as asynchronous ("you might receive a response before the command
/// has completed"), and neither the model nor the documentation publishes how
/// long the propagation takes — on the real service it is minutes. This span is
/// therefore the emulator's OWN: long enough that a caller which reads straight
/// after the call still observes the transient status, short enough that a
/// request settles within one test, and owed by the clock alone, so it completes
/// whether or not anybody calls again.
pub const MULTI_REGION_ACCESS_POINT_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The `MultiRegionAccessPointStatus` a create request leaves behind until
/// [`MULTI_REGION_ACCESS_POINT_SETTLE_AFTER`] has elapsed — "a temporary state
/// that exists while the request is propagating and being completed", per the
/// member's documentation.
pub const STATUS_CREATING: &str = "CREATING";

/// The `MultiRegionAccessPointStatus` a created Multi-Region Access Point
/// reaches once the clock owes it.
pub const STATUS_READY: &str = "READY";

/// The `MultiRegionAccessPointStatus` a delete request leaves behind until
/// [`MULTI_REGION_ACCESS_POINT_SETTLE_AFTER`] has elapsed, after which the
/// record is released.
pub const STATUS_DELETING: &str = "DELETING";

/// The `AsyncRequestStatus` an asynchronous request carries until the clock owes
/// its work.
///
/// `AsyncRequestStatus` is an unconstrained string in the model, so these two
/// values are this service's vocabulary for the two states a Multi-Region Access
/// Point request can be in rather than a modelled enumeration.
pub const ASYNC_REQUEST_IN_PROGRESS: &str = "INPROGRESS";

/// The `AsyncRequestStatus` an asynchronous request carries once its work is
/// due. See [`ASYNC_REQUEST_IN_PROGRESS`].
pub const ASYNC_REQUEST_SUCCEEDED: &str = "SUCCEEDED";

/// The `AsyncRequestStatus` of a request whose span has run out and whose work
/// this service will not carry out — the status the LOSER of a create race
/// settles in, with the conflict in its `ErrorDetails`. See
/// [`ASYNC_REQUEST_IN_PROGRESS`].
pub const ASYNC_REQUEST_FAILED: &str = "FAILED";

/// `wanted` as the generated enum list `values` spells it.
///
/// The spelling that goes on the wire is the model's, found by VALUE rather than
/// by position: neither a reordering of the generated list nor a typo in the
/// constant above can change what is returned.
fn modelled_value(values: &[&'static str], wanted: &'static str) -> String {
    values
        .iter()
        .copied()
        .find(|value| *value == wanted)
        .unwrap_or(wanted)
        .to_string()
}

/// The ARN of a job: the `job/<job-id>` resource in this service's ARN
/// namespace (`s3`, per the model's `awsService.arnNamespace`), which is the
/// form `JobDescriptor$JobArn` states
/// (`^arn:[^:]+:s3:[a-zA-Z0-9\-]+:\d{12}:job\/`).
///
/// The account segment comes from [`::emulator_server::emulator_arn`], the
/// single source of truth for the canonical emulator account.
pub fn job_arn(region: &str, job_id: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("job/{job_id}"))
}

/// A digest of everything a `CreateJob` asked for EXCEPT its
/// `ClientRequestToken` and the account it names, which is what lets a repeat
/// of one token be answered with the job it already created while a DIFFERENT
/// request reusing that token is refused.
///
/// The members are rendered with `Debug` — the one total, structural rendering
/// every modelled shape in this service derives — and hashed with SHA-256, so
/// two creates agree here exactly when they asked for the same job. The input
/// is destructured rather than field-accessed, so a member the model adds to
/// `CreateJobRequest` is a compile error here rather than a difference two
/// creates could disagree on unnoticed.
pub fn create_job_fingerprint(input: &CreateJobInput) -> String {
    use sha2::Digest;

    let CreateJobInput {
        // Not part of the fingerprint: the token is what this digest is
        // compared ACROSS, and the account id is already the scope the token is
        // looked up in.
        account_id: _,
        client_request_token: _,
        confirmation_required,
        operation,
        report,
        manifest,
        description,
        priority,
        role_arn,
        tags,
        manifest_generator,
    } = input;
    let rendered = format!(
        "{operation:?}\u{1f}{report:?}\u{1f}{manifest:?}\u{1f}{manifest_generator:?}\u{1f}\
         {description:?}\u{1f}{priority:?}\u{1f}{role_arn:?}\u{1f}{confirmation_required:?}\u{1f}\
         {tags:?}"
    );
    hex::encode(sha2::Sha256::digest(rendered.as_bytes()))
}

/// A fresh service-assigned resource id, in the shape S3 Control gives one: a
/// version-4 UUID, `8-4-4-4-12` lowercase hex with the version and variant bits
/// RFC 4122 §4.4 fixes — 36 characters, which is the maximum the modelled
/// `JobId` (len 5..36, pattern `^[a-zA-Z0-9\-\_]+$`) admits and which the
/// modelled `AccessGrantsLocationId` / `AccessGrantId` (len 1..64, pattern
/// `^[a-zA-Z0-9\-]+$`) admit too.
///
/// The 128 bits come from three sources hashed together with SHA-256: the
/// OS-seeded randomness a `RandomState` draws, the system clock in nanoseconds,
/// and `sequence` — a counter the store hands out, so two creates in the same
/// nanosecond cannot collide. This names a resource; a credential would come
/// from the credential store instead, never from here.
fn new_resource_uuid(sequence: u64) -> String {
    use sha2::Digest;
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = sha2::Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    uuid_from_digest(&hasher.finalize())
}

/// The first 16 bytes of `digest` as the text of a version-4 UUID: `8-4-4-4-12`
/// lowercase hex with the version and variant bits RFC 4122 §4.4 fixes.
///
/// The one place that shape is rendered, so every service-assigned identifier
/// this crate mints in it — a resource id ([`new_resource_uuid`]) and the id of
/// an asynchronous request ([`async_request_token_arn`]) alike — carries the
/// same 36 characters. `digest` is a SHA-256 digest, so it is never shorter.
fn uuid_from_digest(digest: &[u8]) -> String {
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits
    // are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let rendered = hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &rendered[0..8],
        &rendered[8..12],
        &rendered[12..16],
        &rendered[16..20],
        &rendered[20..32]
    )
}

/// One of [`STATUS_CREATING`], [`STATUS_READY`] or [`STATUS_DELETING`], spelled
/// as the model's `MultiRegionAccessPointStatus` enum carries it.
pub fn multi_region_access_point_status(wanted: &'static str) -> String {
    modelled_value(MULTI_REGION_ACCESS_POINT_STATUS_VALUES, wanted)
}

/// An `AsyncOperationName`, spelled as the model's enum carries it.
pub fn async_operation_name(wanted: &'static str) -> String {
    modelled_value(ASYNC_OPERATION_NAME_VALUES, wanted)
}

/// One stored Multi-Region Access Point: everything `GetMultiRegionAccessPoint`
/// reports, recorded by `CreateMultiRegionAccessPoint`.
#[derive(Debug, Clone)]
pub struct MultiRegionAccessPointRecord {
    pub name: String,
    /// The account that owns it — the first half of the key, kept on the record
    /// so a record passed on its own still names its owner.
    pub account_id: String,
    pub alias: String,
    pub created_at: ::aws_smithy_types::DateTime,
    /// The access point's EFFECTIVE public-access block settings, defaulted by
    /// [`effective_public_access_block`] when the create request supplied none;
    /// reported on every read of the record.
    pub public_access_block: PublicAccessBlockConfiguration,
    /// The Regions and buckets the create request associated, in the order it
    /// listed them.
    pub regions: Vec<RegionReport>,
    /// Where this access point's traffic may be routed: one entry per member of
    /// [`Self::regions`], in the same order, each carrying that bucket's
    /// traffic dial. Recorded at create ([`default_routes`]) so a read reports
    /// the routes of the buckets this record actually associates.
    pub routes: Vec<MultiRegionAccessPointRoute>,
    /// The policy document the last COMPLETED policy put left on the access
    /// point — what `GetMultiRegionAccessPointPolicy` reports as `Established`
    /// and what its policy status is derived from. `None` until a put
    /// completes.
    pub established_policy: Option<String>,
    /// The document a policy put submitted whose span the clock has not yet
    /// run out — what `GetMultiRegionAccessPointPolicy` reports as `Proposed`,
    /// "first listed as the proposed policy" until every Region has it.
    pub proposed_policy: Option<String>,
    /// When the clock owes [`Self::proposed_policy`] its promotion to
    /// [`Self::established_policy`]; `None` once nothing is owed.
    pub policy_due_at: Option<SystemTime>,
    /// One of [`MULTI_REGION_ACCESS_POINT_STATUS_VALUES`].
    pub status: String,
    /// When the clock owes `status` its settled outcome, for the transient
    /// statuses an asynchronous request leaves behind; `None` once nothing is
    /// owed.
    pub status_due_at: Option<SystemTime>,
}

impl MultiRegionAccessPointRecord {
    /// The record as the `MultiRegionAccessPointReport` shape
    /// `GetMultiRegionAccessPoint` returns.
    pub fn report(&self) -> MultiRegionAccessPointReport {
        MultiRegionAccessPointReport {
            name: Some(self.name.clone()),
            alias: Some(self.alias.clone()),
            created_at: Some(self.created_at),
            public_access_block: Some(self.public_access_block.clone()),
            status: Some(self.status.clone()),
            regions: Some(self.regions.clone()),
        }
    }

    /// The record's access control policy as
    /// `GetMultiRegionAccessPointPolicy` reports it: the document the last
    /// completed put established, and — while a put's span has not run out —
    /// the one it proposed. A member with no document is left unset rather
    /// than reported as an empty policy.
    pub fn policy_document(&self) -> MultiRegionAccessPointPolicyDocument {
        MultiRegionAccessPointPolicyDocument {
            established: self.established_policy.clone().map(|policy| {
                EstablishedMultiRegionAccessPointPolicy {
                    policy: Some(policy),
                }
            }),
            proposed: self
                .proposed_policy
                .clone()
                .map(|policy| ProposedMultiRegionAccessPointPolicy {
                    policy: Some(policy),
                }),
        }
    }

    /// Whether the policy in force on this access point grants public access —
    /// the derivation `PolicyStatus$IsPublic` reports, run over the
    /// ESTABLISHED document on every read, so a policy a later put replaced is
    /// reported as it now reads. An access point carrying no established
    /// document grants nobody anything, so it is not public.
    pub fn policy_status(&self) -> PolicyStatus {
        PolicyStatus {
            is_public: self
                .established_policy
                .as_deref()
                .is_some_and(policy_grants_public_access),
        }
    }

    /// Whether the clock has already released this record: a delete request's
    /// span has elapsed, so the Multi-Region Access Point is gone whether or not
    /// anybody called.
    fn removal_due(&self, now: SystemTime) -> bool {
        self.status == STATUS_DELETING && self.status_due_at.is_some_and(|due| now >= due)
    }

    /// Move `status` on if the clock already owes it: a creating access point
    /// reaches `READY`, and a proposed policy whose span has run out becomes
    /// the established one. A record whose removal is due is dropped by
    /// [`ComAmazonawsS3controlAwss3controlservicev20180820State::settle_multi_region_access_points`]
    /// rather than here.
    fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_CREATING && self.status_due_at.is_some_and(|due| now >= due) {
            self.status = multi_region_access_point_status(STATUS_READY);
            self.status_due_at = None;
        }
        if self.policy_due_at.is_some_and(|due| now >= due) {
            if let Some(proposed) = self.proposed_policy.take() {
                self.established_policy = Some(proposed);
            }
            self.policy_due_at = None;
        }
    }
}

/// The traffic dial of an ACTIVE route, which is what every bucket a create
/// request associates starts out as: "a value of `100` indicates an active
/// state, which means that traffic will be routed to the specified Region"
/// (`MultiRegionAccessPointRoute$TrafficDialPercentage`).
pub const TRAFFIC_DIAL_ACTIVE: i32 = 100;

/// The traffic dial of a PASSIVE route: "a value of `0` indicates a passive
/// state, which means that no new traffic will be routed to the Region"
/// (`MultiRegionAccessPointRoute$TrafficDialPercentage`). It is the ONE dial
/// the member documents as passive, so every other value its range (0..100)
/// admits leaves the Region taking traffic — which is what
/// [`route_is_active`] reads.
pub const TRAFFIC_DIAL_PASSIVE: i32 = 0;

/// Whether `route` is ACTIVE: its traffic dial is anything other than
/// [`TRAFFIC_DIAL_PASSIVE`], the one value the member documents as passive.
///
/// This is what "at least one Region must be active at all times"
/// (`SubmitMultiRegionAccessPointRoutes`) is decided by, so a routing submit
/// and a routing read cannot disagree about which Regions take traffic.
pub fn route_is_active(route: &MultiRegionAccessPointRoute) -> bool {
    route.traffic_dial_percentage != TRAFFIC_DIAL_PASSIVE
}

/// How many route updates one `SubmitMultiRegionAccessPointRoutes` call may
/// carry.
///
/// `RouteList` carries no modelled length, so this is the emulator's OWN bound
/// on the work one request can ask for, and it is what stops a caller spending
/// this process's time on a single call. It is set far above any routing
/// configuration a Multi-Region Access Point can have — one route per
/// associated bucket — so no workflow an emulator serves reaches it.
pub const MAX_ROUTE_UPDATES: usize = 1_000;

/// The routes a Multi-Region Access Point carries when it is created: one per
/// associated bucket, in the order the create request listed them, each active.
pub fn default_routes(regions: &[RegionReport]) -> Vec<MultiRegionAccessPointRoute> {
    regions
        .iter()
        .map(|report| MultiRegionAccessPointRoute {
            bucket: report.bucket.clone(),
            region: report.region.clone(),
            traffic_dial_percentage: TRAFFIC_DIAL_ACTIVE,
        })
        .collect()
}

/// One asynchronous Multi-Region Access Point request, as
/// `DescribeMultiRegionAccessPointOperation` reports it.
#[derive(Debug, Clone)]
pub struct AsyncRequestRecord {
    /// The token the create or delete call handed its caller — this record's
    /// identity, and the second half of its key.
    pub request_token_arn: String,
    /// The account the request was made for.
    pub account_id: String,
    /// One of [`ASYNC_OPERATION_NAME_VALUES`].
    pub operation: String,
    pub creation_time: ::aws_smithy_types::DateTime,
    /// The parameters the call submitted, which the model reports back verbatim.
    pub request_parameters: AsyncRequestParameters,
    /// The Regions the request touches, in request order; each is reported with
    /// its own status.
    pub regions: Vec<String>,
    /// When the clock owes this request's work — the same instant the Multi-Region
    /// Access Point's transient status settles at.
    pub due_at: SystemTime,
    /// Why this request's work will not be carried out, for a request this
    /// service accepted but cannot complete — what
    /// `AsyncResponseDetails$ErrorDetails` reports once the clock owes the
    /// outcome, and what makes the request's status [`ASYNC_REQUEST_FAILED`]
    /// rather than [`ASYNC_REQUEST_SUCCEEDED`]. `None` for a request whose work
    /// is done.
    pub failure: Option<AsyncErrorDetails>,
}

impl AsyncRequestRecord {
    /// The record as the `AsyncOperation` shape, with the status the clock owes
    /// at `now`.
    ///
    /// A request whose span has not run out is still `INPROGRESS` whatever its
    /// outcome will be: the outcome — including the error details of a request
    /// whose work this service will not carry out — is what the clock makes
    /// due, which is why a caller polling the token observes the conflict here
    /// rather than at submission.
    pub fn operation_report(&self, now: SystemTime) -> AsyncOperation {
        let settled = now >= self.due_at;
        let status = match (settled, self.failure.is_some()) {
            (false, _) => ASYNC_REQUEST_IN_PROGRESS,
            (true, false) => ASYNC_REQUEST_SUCCEEDED,
            (true, true) => ASYNC_REQUEST_FAILED,
        };
        // A request whose work FAILED carried none of it out, so it reports no
        // per-Region result at all: what became of each Region is not a
        // question a request that was never run has an answer to, and the
        // error details beside this list are what says why. A request still
        // in progress, and one whose work was done, report one entry per
        // Region the request touches, carrying that same status.
        let regional = match status {
            ASYNC_REQUEST_FAILED => Vec::new(),
            _ => self
                .regions
                .iter()
                .map(|region| MultiRegionAccessPointRegionalResponse {
                    name: Some(region.clone()),
                    request_status: Some(status.to_string()),
                })
                .collect(),
        };
        AsyncOperation {
            creation_time: Some(self.creation_time),
            operation: Some(self.operation.clone()),
            request_token_arn: Some(self.request_token_arn.clone()),
            request_parameters: Some(self.request_parameters.clone()),
            request_status: Some(status.to_string()),
            response_details: Some(AsyncResponseDetails {
                multi_region_access_point_details: Some(MultiRegionAccessPointsAsyncResponse {
                    regions: Some(regional),
                }),
                // Only a request that FAILED carries error details, and only
                // once its span has run out: before that there is no outcome to
                // report, and a request whose work was done has no error.
                error_details: settled.then(|| self.failure.clone()).flatten(),
            }),
        }
    }

    /// The pair this record is ordered by when the store evicts its oldest
    /// retained request: when it was made, to the nanosecond.
    fn made_at(&self) -> (i64, u32) {
        (self.creation_time.secs(), self.creation_time.subsec_nanos())
    }

    /// Whether this record has been settled for longer than
    /// [`ASYNC_REQUEST_RETENTION`], so the store no longer holds it — see that
    /// constant.
    fn retention_over(&self, now: SystemTime) -> bool {
        self.due_at
            .checked_add(ASYNC_REQUEST_RETENTION)
            .is_some_and(|released_at| now >= released_at)
    }
}

/// How a stored Multi-Region Access Point is keyed: the owning account id and
/// the name, which is the pair a name is unique within.
pub type MultiRegionAccessPointKey = (String, String);

/// How a stored asynchronous request is keyed: the account it was made for and
/// its request token ARN.
pub type AsyncRequestKey = (String, String);

/// How many Multi-Region Access Points the store holds before
/// `CreateMultiRegionAccessPoint` refuses to add another.
///
/// S3 publishes an adjustable quota on Multi-Region Access Points per account,
/// and this emulator models neither Service Quotas nor an account's
/// adjustments — so this is the emulator's OWN bound on the store rather than a
/// claim about any account's limit. It is set far above any workflow an emulator
/// serves, and it is what stops this part of the state growing without end:
/// every record it holds is released by `DeleteMultiRegionAccessPoint`.
pub const MULTI_REGION_ACCESS_POINT_QUOTA: usize = 1_000;

/// How many asynchronous request records the store retains.
///
/// Unlike a Multi-Region Access Point, an asynchronous request is never deleted
/// by a caller, so retention is what bounds it: recording the
/// [`ASYNC_REQUESTS_RETAINED`]-th + 1 request releases the oldest one, exactly
/// as the real service forgets a request long after it completed. A caller that
/// then describes a forgotten token is answered the same not-found refusal as
/// one that invents a token.
pub const ASYNC_REQUESTS_RETAINED: usize = 1_000;

/// How long after its work settled an asynchronous request record is still
/// retained, after which it is released whether or not the store is near
/// [`ASYNC_REQUESTS_RETAINED`].
///
/// The count above bounds how many records the store can hold at once; this
/// bounds how LONG one is held, which is the other half of "the real service
/// forgets a request long after it completed": a caller that keeps submitting
/// requests — the same create for a name it already holds, with a fresh
/// `ClientToken` each time — accumulates nothing beyond this window, and a
/// token described after it is answered the same not-found refusal as an
/// invented one. The span is this emulator's own: AWS publishes no retention
/// for these records, and it is set far beyond any workflow an emulator serves,
/// so no caller observes a request being forgotten mid-test.
pub const ASYNC_REQUEST_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// One stored S3 Access Grants instance: everything
/// `CreateAccessGrantsInstance` recorded and `GetAccessGrantsInstance` reports.
#[derive(Debug, Clone)]
pub struct AccessGrantsInstanceRecord {
    /// The account that owns the instance — the first half of the key, kept on
    /// the record so a record passed on its own still names its owner.
    pub account_id: String,
    /// The Region the instance lives in, which is the Region the create call
    /// was signed for: `CreateAccessGrantsInstance` documents one instance per
    /// Region per account, so the Region is half this resource's identity and
    /// the ARN it reports is in that Region.
    pub region: String,
    /// The instance id. `AccessGrantsInstanceId` documents it as `default` for
    /// every instance (see [`ACCESS_GRANTS_INSTANCE_ID`]), so it is on the
    /// record as the value both the ARN and the reads are built from rather
    /// than re-derived per handler.
    pub id: String,
    pub arn: String,
    pub created_at: ::aws_smithy_types::DateTime,
    /// The IAM Identity Center instance the create call associated, if any —
    /// `None` for an instance created without one, which is what the reads
    /// report as an absent `IdentityCenterInstanceArn`.
    pub identity_center_instance_arn: Option<String>,
    /// The Identity Center application S3 Access Grants creates for the
    /// instance when one is associated (see
    /// [`identity_center_application_arn`]); `None` exactly when
    /// `identity_center_instance_arn` is.
    pub identity_center_application_arn: Option<String>,
    /// The tags `CreateAccessGrantsInstance` was given. Recorded here because
    /// they are part of the resource; the operations that report them
    /// (`ListTagsForResource`, `TagResource`, `UntagResource`) are still stubs
    /// with their own gap issues, and will read this field rather than a second
    /// copy.
    pub tags: Vec<Tag>,
    /// The resource policy the last `PutAccessGrantsInstanceResourcePolicy`
    /// stored; `None` until one is put, and `None` again once
    /// `DeleteAccessGrantsInstanceResourcePolicy` removes it, which is what
    /// `GetAccessGrantsInstanceResourcePolicy` refuses as a missing policy.
    ///
    /// On the instance record rather than in a map of its own: the policy is a
    /// subresource of ONE instance, so it is read and written through the
    /// instance it belongs to and cannot outlive it.
    pub resource_policy: Option<AccessGrantsResourcePolicyRecord>,
}

/// The resource policy of one S3 Access Grants instance: the document
/// `PutAccessGrantsInstanceResourcePolicy` stored, the organization it named,
/// and when it was stored — everything that call and
/// `GetAccessGrantsInstanceResourcePolicy` report.
#[derive(Debug, Clone)]
pub struct AccessGrantsResourcePolicyRecord {
    /// The document the put call sent, byte for byte: it is what that call
    /// reports back, and what [`canonical_resource_policy`] renders for a read.
    pub policy: String,
    /// "The Organization of the resource policy of the S3 Access Grants
    /// instance" (`PutAccessGrantsInstanceResourcePolicyRequest$Organization`) —
    /// `None` when the put named none, which is what the reads report as an
    /// absent `Organization`.
    pub organization: Option<String>,
    /// When the policy was stored. A further put replaces the policy, so this
    /// is the instant of the put the read reports.
    pub created_at: ::aws_smithy_types::DateTime,
}

/// The top-level elements of an IAM JSON policy document, in the order the
/// policy language presents them — the order a document held in that language
/// is written back in. See [`canonical_resource_policy`].
const POLICY_ELEMENT_ORDER: [&str; 3] = ["Version", "Id", STATEMENT_ELEMENT];

/// The element that carries a policy's statements, which is the one top-level
/// element whose value is itself written in the policy language rather than
/// being a plain value.
const STATEMENT_ELEMENT: &str = "Statement";

/// The elements of ONE statement, in the policy language's own order. See
/// [`canonical_resource_policy`].
const STATEMENT_ELEMENT_ORDER: [&str; 9] = [
    "Sid",
    "Effect",
    "Principal",
    "NotPrincipal",
    "Action",
    "NotAction",
    "Resource",
    "NotResource",
    "Condition",
];

/// The statement elements the policy language writes as ONE value or as a list
/// of values — the principals, the actions, the resources and a condition's
/// values. A list naming a single value says exactly what that value says on
/// its own, so it is the value that is written back. See
/// [`canonical_resource_policy`].
const STATEMENT_VALUE_ELEMENTS: [&str; 7] = [
    "Principal",
    "NotPrincipal",
    "Action",
    "NotAction",
    "Resource",
    "NotResource",
    "Condition",
];

/// The resource policy `document` as the Access Grants instance HOLDS it: the
/// canonical rendering of the policy language it is written in.
///
/// S3 Access Grants takes the document the put call submitted — which is what
/// that call reports back — and holds the POLICY it expresses, so a read
/// answers that policy written back in the language's canonical form rather
/// than the caller's bytes: the elements of the document and of each statement
/// in the order the policy language presents them
/// ([`POLICY_ELEMENT_ORDER`], [`STATEMENT_ELEMENT_ORDER`]), no insignificant
/// whitespace, and an element that names ONE value written as that value rather
/// than as a one-entry list ([`STATEMENT_VALUE_ELEMENTS`]).
///
/// Everything the language does not define the form of — a member it places
/// nothing about, and the inside of any value it states no form for — is written
/// back as `::serde_json` renders it, which orders an object's members by name;
/// a `document` that is not JSON at all expresses no policy, so it is answered
/// unchanged.
///
/// The order the CALLER wrote the members in is not available to write back:
/// `::serde_json`'s object is ordered by member name, and the insertion-ordered
/// form of it is a crate-wide feature every service in this workspace would
/// then render its JSON through. So the one order this rendering can state is
/// the policy language's own, which is also the order that makes the rendering
/// canonical rather than caller-shaped.
///
/// The walk below recurses over the PARSED document, so its depth is bounded by
/// the nesting `::serde_json` admits when it reads the text, and the text itself
/// by the modelled `PolicyDocument` length.
pub fn canonical_resource_policy(document: &str) -> String {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return document.to_string();
    };
    let mut rendered = String::with_capacity(document.len());
    write_policy_document(&mut rendered, &parsed);
    rendered
}

/// Write the whole document: its elements in the language's order, with the
/// statements written as statements.
///
/// Reading the text is `::serde_json`'s — nothing here parses JSON — and so is
/// every VALUE written back out ([`write_parsed`]), quoting, escaping and number
/// forms included. What these walkers add is the ORDER the members are written
/// in and the one-value form of an element, neither of which `::serde_json`'s
/// own object can express: its `Map` is ordered by member name.
fn write_policy_document(rendered: &mut String, document: &::serde_json::Value) {
    let Some(elements) = document.as_object() else {
        write_parsed(rendered, document);
        return;
    };
    rendered.push('{');
    for (index, (name, value)) in ordered_members(elements, &POLICY_ELEMENT_ORDER)
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            rendered.push(',');
        }
        write_member_name(rendered, name);
        if name == STATEMENT_ELEMENT {
            write_statements(rendered, value);
        } else {
            write_parsed(rendered, value);
        }
    }
    rendered.push('}');
}

/// Write the `Statement` element, which the language writes as one statement or
/// as a list of them.
fn write_statements(rendered: &mut String, element: &::serde_json::Value) {
    let ::serde_json::Value::Array(entries) = element else {
        write_statement(rendered, element);
        return;
    };
    rendered.push('[');
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            rendered.push(',');
        }
        write_statement(rendered, entry);
    }
    rendered.push(']');
}

/// Write ONE statement: its elements in the language's order, with each element
/// that names values written in the form the language gives it.
fn write_statement(rendered: &mut String, statement: &::serde_json::Value) {
    let Some(elements) = statement.as_object() else {
        write_parsed(rendered, statement);
        return;
    };
    rendered.push('{');
    for (index, (name, value)) in ordered_members(elements, &STATEMENT_ELEMENT_ORDER)
        .into_iter()
        .enumerate()
    {
        if index > 0 {
            rendered.push(',');
        }
        write_member_name(rendered, name);
        if STATEMENT_VALUE_ELEMENTS.contains(&name) {
            write_element_values(rendered, value);
        } else {
            write_parsed(rendered, value);
        }
    }
    rendered.push('}');
}

/// Write an element that names values: a list naming a single value is written
/// as that value, and the values a principal type or a condition key names are
/// written the same way.
///
/// The single value may be of any scalar form the language writes — a string
/// names an action, a resource or a principal, and a condition key's value may
/// be a number or a boolean — so what decides the collapse is the list naming
/// exactly ONE value, not which scalar that value is. A one-entry list whose
/// entry is itself a list or an object names no value, so it is left as the list
/// it is.
fn write_element_values(rendered: &mut String, element: &::serde_json::Value) {
    match element {
        ::serde_json::Value::Array(entries) => {
            if let [single] = entries.as_slice() {
                if !single.is_array() && !single.is_object() {
                    write_parsed(rendered, single);
                    return;
                }
            }
            rendered.push('[');
            for (index, entry) in entries.iter().enumerate() {
                if index > 0 {
                    rendered.push(',');
                }
                write_element_values(rendered, entry);
            }
            rendered.push(']');
        }
        ::serde_json::Value::Object(members) => {
            rendered.push('{');
            for (index, (name, member)) in members.iter().enumerate() {
                if index > 0 {
                    rendered.push(',');
                }
                write_member_name(rendered, name.as_str());
                write_element_values(rendered, member);
            }
            rendered.push('}');
        }
        value => write_parsed(rendered, value),
    }
}

/// Write `value` as `::serde_json` renders it: JSON's own compact form, which is
/// the rendering for everything the policy language states no form for.
fn write_parsed(rendered: &mut String, value: &::serde_json::Value) {
    rendered.push_str(&value.to_string());
}

/// Write one member's name and the colon after it, quoted and escaped by
/// `::serde_json` as a JSON string.
fn write_member_name(rendered: &mut String, name: &str) {
    write_parsed(rendered, &::serde_json::Value::String(name.to_string()));
    rendered.push(':');
}

/// `members`, with the ones `order` names first and in that order, and every
/// other one after them in the order `::serde_json` holds them, which is by
/// member name — so a member the policy language places nothing about is
/// written back rather than dropped.
fn ordered_members<'a>(
    members: &'a ::serde_json::Map<String, ::serde_json::Value>,
    order: &[&'a str],
) -> Vec<(&'a str, &'a ::serde_json::Value)> {
    let mut ordered: Vec<(&str, &::serde_json::Value)> = order
        .iter()
        .filter_map(|name| members.get(*name).map(|value| (*name, value)))
        .collect();
    ordered.extend(
        members
            .iter()
            .filter(|(name, _)| !order.contains(&name.as_str()))
            .map(|(name, value)| (name.as_str(), value)),
    );
    ordered
}

/// How a stored S3 Access Grants instance is keyed: the owning account id and
/// the Region, which is the pair an instance is unique within — one instance
/// per Region per account, as `CreateAccessGrantsInstance` documents.
pub type AccessGrantsInstanceKey = (String, String);

/// The id every S3 Access Grants instance carries.
///
/// `AccessGrantsInstanceId` documents it: "The ID of the S3 Access Grants
/// instance. The ID is `default`. You can have one S3 Access Grants instance
/// per Region per account." — so it is a constant and not a minted id.
pub const ACCESS_GRANTS_INSTANCE_ID: &str = "default";

/// How many S3 Access Grants instances the store holds before
/// `CreateAccessGrantsInstance` refuses to record one for a FURTHER
/// (account, Region) pair.
///
/// This is the emulator's OWN bound on its store, not a claim about a limit AWS
/// publishes: AWS allows one instance per Region per account, which bounds how
/// many instances ONE account can have in one Region, but `AccountId` is
/// constrained only to `^\d{12}$` and the Region comes from the request's
/// credential scope, so the number of KEYS is caller-chosen. Without this bound
/// a caller could grow this process's state without end by varying either half
/// of the key.
pub const ACCESS_GRANTS_INSTANCE_QUOTA: usize = 1_000;

/// One registered S3 Access Grants location: everything
/// `CreateAccessGrantsLocation` recorded and `GetAccessGrantsLocation` reports.
#[derive(Debug, Clone)]
pub struct AccessGrantsLocationRecord {
    /// The account whose S3 Access Grants instance holds the location.
    pub account_id: String,
    /// The Region the location was registered in — "Your S3 data must be in the
    /// same Region as your S3 Access Grants instance"
    /// (`CreateAccessGrantsLocation`), and an instance is Region-scoped, so the
    /// Region is part of this resource's identity too.
    pub region: String,
    /// The id S3 Access Grants assigned: [`ACCESS_GRANTS_LOCATION_DEFAULT_ID`]
    /// for the default location, otherwise a service-assigned one.
    pub id: String,
    pub arn: String,
    /// The S3 URI path the location spans, as registered.
    pub location_scope: String,
    /// The IAM role S3 Access Grants assumes to manage the location.
    pub iam_role_arn: String,
    pub created_at: ::aws_smithy_types::DateTime,
    /// The tags the register call was given. Recorded here because they are part
    /// of the resource; the operations that report them (`ListTagsForResource`,
    /// `TagResource`, `UntagResource`) are still stubs with their own gap
    /// issues, and will read this field rather than a second copy.
    pub tags: Vec<Tag>,
}

/// How a registered location is keyed: the owning account, the Region, and the
/// location id the service assigned — the triple a location id is unique
/// within, because the instance it belongs to is itself (account, Region).
pub type AccessGrantsLocationKey = (String, String, String);

/// The default S3 Access Grants location scope.
///
/// `AccessGrantsLocationId` documents it: "S3 Access Grants assigns the ID
/// `default` to the default location `s3://`" — so the scope and the id it is
/// assigned are both fixed rather than minted.
pub const DEFAULT_LOCATION_SCOPE: &str = "s3://";

/// The id the default location carries. See [`DEFAULT_LOCATION_SCOPE`].
pub const ACCESS_GRANTS_LOCATION_DEFAULT_ID: &str = "default";

/// How many registered locations the store holds before
/// `CreateAccessGrantsLocation` refuses to record a FURTHER one.
///
/// This is the emulator's OWN bound on its store, not a claim about a limit AWS
/// publishes: `LocationScope` is an arbitrary S3 URI path, so the number of
/// locations a caller can register is caller-chosen, and without this bound a
/// caller could grow this process's state without end.
pub const ACCESS_GRANTS_LOCATION_QUOTA: usize = 1_000;

/// One access grant: everything `CreateAccessGrant` recorded, what
/// `GetAccessGrant` reports, and the scope `GetDataAccess` evaluates a target
/// against.
#[derive(Debug, Clone)]
pub struct AccessGrantRecord {
    /// The account whose S3 Access Grants instance holds the grant.
    pub account_id: String,
    /// The Region the grant was created in — the Region of the instance and of
    /// the registered location it narrows.
    pub region: String,
    /// The id S3 Access Grants assigned the grant.
    pub id: String,
    pub arn: String,
    /// The registered location the grant narrows.
    pub access_grants_location_id: String,
    /// The location configuration the create call submitted, reported back
    /// verbatim; `None` when the grant covers the whole location scope.
    pub location_configuration: Option<AccessGrantsLocationConfiguration>,
    /// The S3 URI path the grant covers: see [`grant_scope`].
    pub grant_scope: String,
    pub grantee: Grantee,
    /// One of the modelled `Permission` values.
    pub permission: String,
    /// The Identity Center application the grantee must come through, if the
    /// create call named one.
    pub application_arn: Option<String>,
    /// The `S3PrefixType` the create call submitted (`Object` when the grant
    /// scope is a single object). Part of the resource, though no modelled
    /// output member carries it back.
    pub s3_prefix_type: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    /// The tags the create call was given, recorded for the same reason as
    /// [`AccessGrantsLocationRecord::tags`].
    pub tags: Vec<Tag>,
    /// The grant's usage limit, which `GetAccessGrant` reports back:
    /// [`ACCESS_GRANT_UNCAPPED_USAGE_LIMIT`] for a grant created without one.
    ///
    /// No modelled member of `CreateAccessGrant` asks for a limit
    /// (`smithy_query(query="operation", operation="CreateAccessGrant")`), so
    /// every grant this service records is created without one; the field is the
    /// record's own so the read reports what the record holds rather than a
    /// number the serializer invents.
    pub usage_limit: i32,
}

/// The usage limit of a grant created without one — the only kind this service
/// records. See [`AccessGrantRecord::usage_limit`].
pub const ACCESS_GRANT_UNCAPPED_USAGE_LIMIT: i32 = 0;

/// How a grant is keyed: the owning account, the Region, and the grant id — the
/// triple a grant id is unique within, for the same reason as
/// [`AccessGrantsLocationKey`].
pub type AccessGrantKey = (String, String, String);

/// How many grants the store holds before `CreateAccessGrant` refuses to record
/// a FURTHER one. The emulator's OWN bound on its store, for the same reason as
/// [`ACCESS_GRANTS_LOCATION_QUOTA`].
pub const ACCESS_GRANT_QUOTA: usize = 1_000;

/// How many live credentials `GetDataAccess` has vended before it refuses to
/// vend another.
///
/// The emulator's OWN bound: a vend adds a credential to the emulator's
/// credential store, and nothing but its own expiry removes it, so without this
/// bound a caller could grow the store without end. A credential whose
/// expiration has passed is released first (see
/// [`ComAmazonawsS3controlAwss3controlservicev20180820State::expire_data_access_credentials`]),
/// so the bound is on credentials that are still live.
pub const DATA_ACCESS_CREDENTIAL_QUOTA: usize = 1_000;

/// The session duration `GetDataAccess` vends a credential for when the request
/// names none.
///
/// `DurationSeconds` documents it: "The default value is 1 hour, but the grantee
/// can specify a range from 900 seconds (15 minutes) up to 43200 seconds (12
/// hours)."
pub const DATA_ACCESS_DEFAULT_DURATION_SECONDS: i32 = 3_600;

/// The shortest session `GetDataAccess` vends a credential for, which is the
/// floor of `DurationSeconds`' modelled `range 900..43200` — "the grantee can
/// specify a range from 900 seconds (15 minutes) up to 43200 seconds (12
/// hours)".
pub const DATA_ACCESS_MIN_DURATION_SECONDS: i32 = 900;

/// The longest session `GetDataAccess` vends a credential for, the ceiling of
/// the same modelled range. See [`DATA_ACCESS_MIN_DURATION_SECONDS`].
pub const DATA_ACCESS_MAX_DURATION_SECONDS: i32 = 43_200;

/// The `Privilege` whose token carries the scope of the matched grant.
///
/// `Privilege` documents the two values: "Default – The scope of the returned
/// temporary access token is the scope of the grant that is closest to the
/// target scope. Minimal – The scope of the returned temporary access token is
/// the same as the requested target scope as long as the requested scope is the
/// same as or a subset of the grant scope." `Privilege` is also the member's
/// default when a request omits it.
pub const PRIVILEGE_DEFAULT: &str = "Default";

/// The `Privilege` whose token carries the requested target scope. See
/// [`PRIVILEGE_DEFAULT`].
pub const PRIVILEGE_MINIMAL: &str = "Minimal";

/// The `Permission` that grants read-only access.
pub const PERMISSION_READ: &str = "READ";

/// The `Permission` that grants write-only access.
pub const PERMISSION_WRITE: &str = "WRITE";

/// The `Permission` that grants both read and write access, so it satisfies a
/// request for either one — "READWRITE – Grant both read and write access to
/// the S3 data" (`Permission`).
pub const PERMISSION_READWRITE: &str = "READWRITE";

/// The grant scope of a grant on `location_scope` narrowed by `sub_prefix`.
///
/// `AccessGrantsLocationConfiguration$S3SubPrefix` states the rule: "The
/// `S3SubPrefix` is appended to the location scope creating the grant scope",
/// and its example appends `<bucket-name>/*` to the default location `s3://` to
/// reach `s3://<bucket-name>/*`. A grant that narrows nothing has the location
/// scope itself for its grant scope. The separator is normalised to exactly one
/// `/` between the two halves, whichever of them carries it, so a location
/// registered as a bucket path with or without its trailing slash yields the
/// same grant scope.
pub fn grant_scope(location_scope: &str, sub_prefix: Option<&str>) -> String {
    let Some(sub_prefix) = sub_prefix.filter(|prefix| !prefix.is_empty()) else {
        return location_scope.to_string();
    };
    let tail = sub_prefix.strip_prefix('/').unwrap_or(sub_prefix);
    if location_scope.ends_with('/') {
        format!("{location_scope}{tail}")
    } else {
        format!("{location_scope}/{tail}")
    }
}

/// How much of `target` the S3 URI path `scope` covers, or `None` when it covers
/// none of it.
///
/// An S3 Access Grants scope is an S3 URI path, and `S3SubPrefix` states how one
/// is read: "If the `S3SubPrefix` includes a prefix, append the wildcard
/// character `*` after the prefix to indicate that you want to include all
/// object key names in the bucket that start with that prefix." So a scope
/// ending in `*` covers every target whose path starts with the text before the
/// `*`; a scope ending in `/` is a bucket-or-prefix boundary and covers every
/// target beneath it; and any other scope names one object, covering the target
/// equal to it and the targets under it as a prefix boundary.
///
/// The answer is the number of characters of `scope` that did the covering, so a
/// caller can pick the MOST SPECIFIC of several covering scopes — the grant
/// "closest to the target scope" `Privilege` speaks of — by taking the largest.
pub fn scope_coverage(scope: &str, target: &str) -> Option<usize> {
    if let Some(prefix) = scope.strip_suffix('*') {
        return target.starts_with(prefix).then_some(prefix.len());
    }
    if scope.ends_with('/') {
        return target.starts_with(scope).then_some(scope.len());
    }
    let covers = target == scope || target.starts_with(&format!("{scope}/"));
    covers.then_some(scope.len())
}

/// Whether a grant carrying `granted` satisfies a request for `requested`.
///
/// `Permission` documents the three values, of which only `READWRITE` grants
/// more than itself: it "grant[s] both read and write access to the S3 data", so
/// it satisfies a request for `READ` or for `WRITE` as well as for itself.
pub fn permission_satisfies(granted: &str, requested: &str) -> bool {
    granted == requested
        || (granted == PERMISSION_READWRITE
            && (requested == PERMISSION_READ || requested == PERMISSION_WRITE))
}

/// Per-service state for com-amazonaws-s3control-awss3controlservicev20180820. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsS3controlAwss3controlservicev20180820State {
    /// Access points by (account id, name).
    ///
    /// A `BTreeMap` rather than a `HashMap` so the one scan order every reader
    /// sees is sorted by that key: `ListAccessPoints` is `@paginated`, and a
    /// page token can only resume a scan whose order does not change between
    /// calls.
    ///
    /// BOUNDED: `CreateAccessPoint` refuses past [`ACCESS_POINT_QUOTA`] records
    /// and `PutAccessPointPolicy` refuses a document past
    /// [`MAX_POLICY_BYTES`], so neither the number of records nor the size of
    /// one is caller-chosen without limit. `DeleteAccessPoint` releases a
    /// record and its policy.
    pub access_points: BTreeMap<AccessPointKey, AccessPointRecord>,

    // The account-level `PublicAccessBlock` configuration is NOT held here: it
    // is what Amazon S3 judges a public bucket policy, a public ACL and a public
    // read by, so it lives once in the store that serves those requests, which
    // this service writes and reads it in (`account_public_access_block`) —
    // exactly as a bucket's tag set and an access point's data path do.

    /// S3 Storage Lens configurations by (account id, configuration id).
    ///
    /// A `BTreeMap` for the same reason as `access_points`:
    /// `ListStorageLensConfigurations` is `@paginated`, and a page token can
    /// only resume a scan whose order does not change between calls.
    ///
    /// BOUNDED: `PutStorageLensConfiguration` refuses a NEW configuration past
    /// [`STORAGE_LENS_CONFIGURATION_QUOTA`] records, and both puts that carry a
    /// tag set refuse one past [`MAX_STORAGE_LENS_TAGS`] tags; every member of
    /// the document itself is length-bounded by the model.
    pub storage_lens_configurations:
        BTreeMap<StorageLensConfigurationKey, StorageLensConfigurationRecord>,

    /// S3 Storage Lens groups by (account id, group name).
    ///
    /// A `BTreeMap` for the same reason as the two maps above:
    /// `ListStorageLensGroups` is `@paginated`, and a page token can only
    /// resume a scan whose order does not change between calls.
    ///
    /// BOUNDED: `CreateStorageLensGroup` refuses a new group once the map holds
    /// [`STORAGE_LENS_GROUP_QUOTA`] records in total, whatever account ids they
    /// are keyed under, and every member of the group document and of its tag
    /// set is length-bounded by the model.
    pub storage_lens_groups: BTreeMap<StorageLensGroupKey, StorageLensGroupRecord>,

    /// S3 Batch Operations jobs by (account id, job id).
    ///
    /// A `BTreeMap` for the same reason as `access_points`: `ListJobs` is
    /// `@paginated`, and a page token can only resume a scan whose order does
    /// not change between calls.
    ///
    /// BOUNDED: `CreateJob` refuses past [`JOB_QUOTA`] records, and a repeated
    /// create carrying one `ClientRequestToken` adds none; every member of a
    /// record is length-bounded by the model.
    pub jobs: BTreeMap<JobKey, JobRecord>,

    /// How many resource ids this store has minted, so two ids are distinct even
    /// when the clock reports the same nanosecond (see [`new_resource_uuid`]).
    minted_resource_ids: u64,

    /// Multi-Region Access Points by (account id, name).
    ///
    /// A `BTreeMap` for the same reason as `access_points`:
    /// `ListMultiRegionAccessPoints` is `@paginated`, and a page token can only
    /// resume a scan whose order does not change between calls.
    ///
    /// BOUNDED: `CreateMultiRegionAccessPoint` refuses past
    /// [`MULTI_REGION_ACCESS_POINT_QUOTA`] records, and every member of a record
    /// is length-bounded by the model. `DeleteMultiRegionAccessPoint` releases a
    /// record once the clock owes its removal.
    pub multi_region_access_points:
        BTreeMap<MultiRegionAccessPointKey, MultiRegionAccessPointRecord>,

    /// The asynchronous Multi-Region Access Point requests
    /// `DescribeMultiRegionAccessPointOperation` reports, by (account id,
    /// request token ARN).
    ///
    /// BOUNDED by retention rather than by a caller's delete: see
    /// [`ASYNC_REQUESTS_RETAINED`] and [`Self::record_async_request`].
    pub async_requests: BTreeMap<AsyncRequestKey, AsyncRequestRecord>,

    /// S3 Access Grants instances by (account id, Region) — what
    /// `CreateAccessGrantsInstance` records and `GetAccessGrantsInstance`
    /// reports.
    ///
    /// A `BTreeMap` for the same reason as `access_points`:
    /// `ListAccessGrantsInstances` is `@paginated`, and a page token can only
    /// resume a scan whose order does not change between calls.
    ///
    /// An (account, Region) pair is a KEY here only once an instance has been
    /// created for it: an ABSENT entry is what `GetAccessGrantsInstance`
    /// refuses as a missing instance, and `CreateAccessGrantsInstance` refuses
    /// a PRESENT one rather than replacing it, because an account has at most
    /// one instance per Region.
    ///
    /// BOUNDED: `CreateAccessGrantsInstance` refuses a NEW instance past
    /// [`ACCESS_GRANTS_INSTANCE_QUOTA`] records, and every member of a record
    /// is length-bounded by the model.
    pub access_grants_instances: BTreeMap<AccessGrantsInstanceKey, AccessGrantsInstanceRecord>,

    /// Registered S3 Access Grants locations by (account id, Region, location
    /// id) — what `CreateAccessGrantsLocation` records and
    /// `GetAccessGrantsLocation` reports.
    ///
    /// A `BTreeMap` for the same reason as `access_points`:
    /// `ListAccessGrantsLocations` is `@paginated`, and a page token can only
    /// resume a scan whose order does not change between calls.
    ///
    /// BOUNDED: `CreateAccessGrantsLocation` refuses a NEW location past
    /// [`ACCESS_GRANTS_LOCATION_QUOTA`] records, and every member of a record is
    /// length-bounded by the model.
    pub access_grants_locations: BTreeMap<AccessGrantsLocationKey, AccessGrantsLocationRecord>,

    /// Access grants by (account id, Region, grant id) — what
    /// `CreateAccessGrant` records, `GetAccessGrant` reports and `GetDataAccess`
    /// evaluates a target against.
    ///
    /// A `BTreeMap` for the same reason as `access_points`: `ListAccessGrants`
    /// is `@paginated`, and a page token can only resume a scan whose order does
    /// not change between calls. The sorted order also decides which of two
    /// equally specific grants a target matches, so that answer does not move
    /// between calls.
    ///
    /// BOUNDED: `CreateAccessGrant` refuses a NEW grant past
    /// [`ACCESS_GRANT_QUOTA`] records, and every member of a record is
    /// length-bounded by the model.
    pub access_grants: BTreeMap<AccessGrantKey, AccessGrantRecord>,

    /// When each credential `GetDataAccess` has vended expires, by the access
    /// key id it was minted under.
    ///
    /// The credential material itself lives in the emulator's credential store,
    /// which is the only thing that mints or forgets it; this map is the record
    /// of WHEN each one is owed to be forgotten, which only a handler holding
    /// that store can act on.
    ///
    /// BOUNDED: `GetDataAccess` releases every expired credential before it
    /// vends, and refuses to vend past [`DATA_ACCESS_CREDENTIAL_QUOTA`] live
    /// ones.
    pub data_access_credentials: BTreeMap<String, SystemTime>,
}

impl ComAmazonawsS3controlAwss3controlservicev20180820State {
    /// Whether `BlockPublicPolicy` is in force for the access point `key` names:
    /// the access point's OWN setting or the owning account's configuration,
    /// either of which blocks on its own — the most restrictive of the settings
    /// that apply is the one in force. False when the store holds no such access
    /// point.
    ///
    /// `account_blocks_public_policy` is the ACCOUNT-level half, read where that
    /// configuration lives ([`account_public_access_block::blocks_public_policy`])
    /// before this store's guard is taken, since that record belongs to the store
    /// Amazon S3 enforces it from.
    pub fn block_public_policy_in_force(
        &self,
        key: &AccessPointKey,
        account_blocks_public_policy: bool,
    ) -> bool {
        let own = self
            .access_points
            .get(key)
            .is_some_and(|record| record.public_access_block_configuration.block_public_policy);
        own || account_blocks_public_policy
    }

    /// A fresh service-assigned id for a resource this store is about to hold,
    /// over this store's own counter (see [`new_resource_uuid`]) — the one place
    /// the counter is bumped, so two ids are distinct whatever kind of resource
    /// they name.
    fn new_resource_id(&mut self) -> String {
        self.minted_resource_ids = self.minted_resource_ids.wrapping_add(1);
        new_resource_uuid(self.minted_resource_ids)
    }

    /// A fresh job id for a job this store is about to hold.
    pub fn new_job_id(&mut self) -> String {
        self.new_resource_id()
    }

    /// A fresh location id — the id S3 Access Grants assigns a location other
    /// than the default one.
    pub fn new_access_grants_location_id(&mut self) -> String {
        self.new_resource_id()
    }

    /// A fresh grant id — the id `AccessGrantId` documents S3 Access Grants
    /// auto-generates when the grant is created.
    pub fn new_access_grant_id(&mut self) -> String {
        self.new_resource_id()
    }

    /// The location `location_id` names in `account_id`'s instance in `region`,
    /// or `None` when that instance has no such location.
    pub fn access_grants_location(
        &self,
        account_id: &str,
        region: &str,
        location_id: &str,
    ) -> Option<&AccessGrantsLocationRecord> {
        self.access_grants_locations.get(&(
            account_id.to_string(),
            region.to_string(),
            location_id.to_string(),
        ))
    }

    /// The location in `account_id`'s instance in `region` registered for
    /// `location_scope`, if any: the record a repeat registration conflicts
    /// with, because one scope is registered once.
    pub fn access_grants_location_by_scope(
        &self,
        account_id: &str,
        region: &str,
        location_scope: &str,
    ) -> Option<&AccessGrantsLocationRecord> {
        self.access_grants_locations.values().find(|location| {
            location.account_id == account_id
                && location.region == region
                && location.location_scope == location_scope
        })
    }

    /// The grant `grant_id` names in `account_id`'s instance in `region`, or
    /// `None` when that instance has no such grant.
    pub fn access_grant(
        &self,
        account_id: &str,
        region: &str,
        grant_id: &str,
    ) -> Option<&AccessGrantRecord> {
        self.access_grants.get(&(
            account_id.to_string(),
            region.to_string(),
            grant_id.to_string(),
        ))
    }

    /// Remove the grant `grant_id` names from `account_id`'s instance in
    /// `region` and answer it, or `None` when that instance has no such grant —
    /// what `DeleteAccessGrant` releases, keyed exactly as [`Self::access_grant`]
    /// reads it.
    pub fn remove_access_grant(
        &mut self,
        account_id: &str,
        region: &str,
        grant_id: &str,
    ) -> Option<AccessGrantRecord> {
        self.access_grants.remove(&(
            account_id.to_string(),
            region.to_string(),
            grant_id.to_string(),
        ))
    }

    /// Whether any grant of `account_id`'s instance in `region` references the
    /// location `location_id` names.
    ///
    /// A registered location is referenced by every grant whose scope was
    /// computed from it, which is what makes deregistering it a refusal while
    /// one of them is still there.
    pub fn access_grants_location_is_referenced(
        &self,
        account_id: &str,
        region: &str,
        location_id: &str,
    ) -> bool {
        self.access_grants.values().any(|grant| {
            grant.account_id == account_id
                && grant.region == region
                && grant.access_grants_location_id == location_id
        })
    }

    /// Whether `account_id`'s instance in `region` still holds a registered
    /// location or a grant.
    ///
    /// The instance is what both of those belong to, which is what makes
    /// deleting it a refusal while one of them is still there — the same
    /// referential integrity [`Self::access_grants_location_is_referenced`]
    /// states one resource down.
    pub fn access_grants_instance_is_referenced(&self, account_id: &str, region: &str) -> bool {
        let locations = self
            .access_grants_locations
            .values()
            .any(|location| location.account_id == account_id && location.region == region);
        let grants = self
            .access_grants
            .values()
            .any(|grant| grant.account_id == account_id && grant.region == region);
        locations || grants
    }

    /// Remove the S3 Access Grants instance `account_id` owns in `region` and
    /// answer it, or `None` when that account has none there — what
    /// `DeleteAccessGrantsInstance` releases, keyed exactly as the reads resolve
    /// it, so a later read of that pair is the missing-instance refusal.
    pub fn remove_access_grants_instance(
        &mut self,
        account_id: &str,
        region: &str,
    ) -> Option<AccessGrantsInstanceRecord> {
        self.access_grants_instances
            .remove(&(account_id.to_string(), region.to_string()))
    }

    /// The grant in `account_id`'s instance in `region` whose scope covers
    /// `target` for `permission` and is CLOSEST to it — the authorization
    /// decision `GetDataAccess` makes.
    ///
    /// Specificity is how much of the target the grant's scope covers (see
    /// [`scope_coverage`]), so a grant on a narrower prefix wins over one on the
    /// bucket that contains it. Two equally specific grants are ordered by their
    /// key, so the answer does not move between calls.
    pub fn grant_covering(
        &self,
        account_id: &str,
        region: &str,
        target: &str,
        permission: &str,
    ) -> Option<&AccessGrantRecord> {
        self.access_grants
            .values()
            .filter(|grant| grant.account_id == account_id && grant.region == region)
            .filter(|grant| permission_satisfies(&grant.permission, permission))
            .filter_map(|grant| {
                scope_coverage(&grant.grant_scope, target).map(|covered| (covered, grant))
            })
            .max_by(|(left, left_grant), (right, right_grant)| {
                left.cmp(right)
                    .then_with(|| right_grant.id.cmp(&left_grant.id))
            })
            .map(|(_, grant)| grant)
    }

    /// Release every credential `GetDataAccess` vended whose expiration has
    /// passed at `now`, and answer their access key ids so the caller can forget
    /// the credential material too.
    ///
    /// Each was over at `now` whether or not anybody called, so the operation
    /// that vends runs this before vending: the credentials this process holds
    /// are the ones the clock says are still live.
    pub fn expire_data_access_credentials(&mut self, now: SystemTime) -> Vec<String> {
        let expired: Vec<String> = self
            .data_access_credentials
            .iter()
            .filter(|(_, expires_at)| **expires_at <= now)
            .map(|(access_key_id, _)| access_key_id.clone())
            .collect();
        for access_key_id in &expired {
            self.data_access_credentials.remove(access_key_id);
        }
        expired
    }

    /// Apply every job transition the clock already owed at `now`.
    ///
    /// Called by the operations that report or move a job, so what they answer
    /// is what the clock had already made due — never a move caused by the read
    /// itself. The scan is over the whole job store, which [`JOB_QUOTA`]
    /// bounds.
    pub fn materialize_due_jobs(&mut self, now: SystemTime) {
        for job in self.jobs.values_mut() {
            job.materialize_due(now);
        }
    }

    /// The job `job_id` names under `account_id`, or `None` when that account
    /// owns no such job.
    pub fn job(&self, account_id: &str, job_id: &str) -> Option<&JobRecord> {
        self.jobs
            .get(&(account_id.to_string(), job_id.to_string()))
    }

    /// The job `job_id` names under `account_id`, for an operation that CHANGES
    /// it (`UpdateJobPriority` moves its priority), or `None` when that account
    /// owns no such job — the same lookup [`Self::job`] reports from, so a
    /// writer and a reader cannot disagree about which record a job id names.
    pub fn job_mut(&mut self, account_id: &str, job_id: &str) -> Option<&mut JobRecord> {
        self.jobs
            .get_mut(&(account_id.to_string(), job_id.to_string()))
    }

    /// The job this account already created with `client_request_token`, if
    /// any: the record a repeated create is answered from.
    pub fn job_by_client_request_token(
        &self,
        account_id: &str,
        client_request_token: &str,
    ) -> Option<&JobRecord> {
        self.jobs.values().find(|job| {
            job.account_id == account_id && job.client_request_token == client_request_token
        })
    }

    /// Materialise every Multi-Region Access Point transition the clock already
    /// owes at `now`: a creating access point reaches `READY`, and a deleting
    /// one is released.
    ///
    /// Each of these was owed at `now` whether or not anybody called, so a
    /// reporting operation runs it before reading: that is what makes a status a
    /// caller observes the status the clock says it is, rather than the one the
    /// last writer left behind.
    pub fn settle_multi_region_access_points(&mut self, now: SystemTime) {
        self.multi_region_access_points
            .retain(|_, record| !record.removal_due(now));
        for record in self.multi_region_access_points.values_mut() {
            record.settle(now);
        }
        // An asynchronous request record is released once it has been settled
        // for longer than [`ASYNC_REQUEST_RETENTION`] — owed at that instant
        // whether or not anybody called, exactly like the transitions above, and
        // what keeps a caller that submits request after request from
        // accumulating records beyond that window.
        self.async_requests
            .retain(|_, request| !request.retention_over(now));
    }

    /// The Multi-Region Access Point of `account_id` that `alias` names, if the
    /// account holds one.
    ///
    /// An alias is the service-assigned identity of an access point
    /// (`multi_region_access_point_alias`) and is what a
    /// `MultiRegionAccessPointId` carries, so this is the resolution the
    /// routing operations read their `Mrap` member through — a different
    /// lookup from the one by NAME that `multi_region_access_points` is keyed
    /// by.
    pub fn multi_region_access_point_by_alias(
        &self,
        account_id: &str,
        alias: &str,
    ) -> Option<&MultiRegionAccessPointRecord> {
        self.multi_region_access_points
            .values()
            .find(|record| record.account_id == account_id && record.alias == alias)
    }

    /// The Multi-Region Access Point of `account_id` that `alias` names, for a
    /// call that WRITES it.
    ///
    /// The same resolution [`Self::multi_region_access_point_by_alias`]
    /// performs — an `Mrap` member is an alias, scoped to the account that owns
    /// it — so a routing submit reaches exactly the record a routing read
    /// reports, and neither can resolve a value the other would not.
    pub fn multi_region_access_point_by_alias_mut(
        &mut self,
        account_id: &str,
        alias: &str,
    ) -> Option<&mut MultiRegionAccessPointRecord> {
        self.multi_region_access_points
            .values_mut()
            .find(|record| record.account_id == account_id && record.alias == alias)
    }

    /// Retain `record`, releasing the oldest retained request first when the
    /// store is already holding [`ASYNC_REQUESTS_RETAINED`] of them, so the
    /// number of async request records a caller can accumulate is bounded.
    pub fn record_async_request(&mut self, record: AsyncRequestRecord) {
        while self.async_requests.len() >= ASYNC_REQUESTS_RETAINED {
            let oldest = self
                .async_requests
                .iter()
                .min_by_key(|(key, retained)| (retained.made_at(), (*key).clone()))
                .map(|(key, _)| key.clone());
            let Some(oldest) = oldest else { break };
            self.async_requests.remove(&oldest);
        }
        let key = (record.account_id.clone(), record.request_token_arn.clone());
        self.async_requests.insert(key, record);
    }

    /// The tag set of the resource `resource_arn` names among the ones THIS
    /// store holds, or `None` when it holds no such resource.
    ///
    /// Addressed by the ARN the store RECORDED for the resource, so a tagging
    /// call reaches exactly the resource whose own reads report that ARN and
    /// nothing is derived twice from the ARN's text. The account is the one the
    /// request names — a resource is tagged through the account that owns it —
    /// which is the first field of every key here.
    ///
    /// The ONE seam the generic tagging surface writes a held resource's tags
    /// through, so `TagResource`, `UntagResource` and `ListTagsForResource`
    /// cannot disagree about where a resource's tags live, and each resource's
    /// own create call keeps writing the same field.
    pub fn resource_tags_mut(
        &mut self,
        account_id: &str,
        resource_arn: &str,
    ) -> Option<&mut Vec<Tag>> {
        if let Some(record) = self
            .access_points
            .iter_mut()
            .find(|((owner, _), record)| owner == account_id && record.arn == resource_arn)
            .map(|(_, record)| record)
        {
            return Some(&mut record.tags);
        }
        if let Some(record) = self
            .storage_lens_groups
            .iter_mut()
            .find(|((owner, _), record)| {
                owner == account_id
                    && record.group.storage_lens_group_arn.as_deref() == Some(resource_arn)
            })
            .map(|(_, record)| record)
        {
            return Some(&mut record.tags);
        }
        if let Some(record) = self
            .access_grants_instances
            .iter_mut()
            .find(|((owner, _), record)| owner == account_id && record.arn == resource_arn)
            .map(|(_, record)| record)
        {
            return Some(&mut record.tags);
        }
        if let Some(record) = self
            .access_grants_locations
            .iter_mut()
            .find(|((owner, _, _), record)| owner == account_id && record.arn == resource_arn)
            .map(|(_, record)| record)
        {
            return Some(&mut record.tags);
        }
        self.access_grants
            .iter_mut()
            .find(|((owner, _, _), record)| owner == account_id && record.arn == resource_arn)
            .map(|(_, record)| &mut record.tags)
    }
}

// ---------------------------------------------------------------------------
// The generic resource tagging surface
//
// `TagResource`, `UntagResource` and `ListTagsForResource` each address ONE S3
// resource by its ARN, whatever kind of resource that is, so the resolution
// below and the tag-set change beside it are shared by the three rather than
// restated in each handler.
// ---------------------------------------------------------------------------

/// The ARN namespace of this service's own resources and of Amazon S3's:
/// `awsService.arnNamespace` of both models, which is what the third field of a
/// `ResourceArn` has to name for the ARN to address an S3 resource at all.
pub const S3_ARN_NAMESPACE: &str = "s3";

/// The ARN namespace of the S3 Express One Zone resources — a directory bucket
/// and an access point for one — which `S3ResourceArn`'s own pattern
/// (`^arn:[^:]+:s3(express)?:[^:]`) names beside [`S3_ARN_NAMESPACE`].
pub const S3_EXPRESS_ARN_NAMESPACE: &str = "s3express";

/// The resource type of a bucket in an S3 Express One Zone bucket ARN
/// (`arn:<partition>:s3express:<region>:<account>:bucket/<name>`).
const BUCKET_RESOURCE_TYPE: &str = "bucket";

/// The resource type of an access point ARN, as [`access_point_arn`] builds it.
const ACCESS_POINT_RESOURCE_TYPE: &str = "accesspoint";

/// The resource type of a Storage Lens group ARN, as
/// [`storage_lens_group_arn`] builds it.
const STORAGE_LENS_GROUP_RESOURCE_TYPE: &str = "storage-lens-group";

/// The resource type shared by the three S3 Access Grants ARNs, as
/// [`access_grants_instance_arn`], [`access_grants_location_arn`] and
/// [`access_grant_arn`] build them: an instance's resource field is this type
/// and the instance id, and a location's or a grant's carries the subresource
/// type and id after that pair
/// ([`access_grants_subresource_arn`]).
const ACCESS_GRANTS_RESOURCE_TYPE: &str = "access-grants";

/// The largest tag set an S3 resource carries.
///
/// "You can add up to 50 Amazon Web Services resource tags for each S3
/// resource" (`TagResource`'s own documentation). The modelled `TagList` bound
/// (len 0..50) says that of ONE request's list, which the boundary already
/// enforces; this bounds the set the RESOURCE ends up with, which a merge onto
/// tags it already carries can take past the length of any one request.
pub const MAX_RESOURCE_TAGS: usize = 50;

/// Which S3 resource the `ResourceArn` of a generic tagging call names, read
/// from the ARN alone.
///
/// "This operation is only supported for the following Amazon S3 resource:
/// General purpose buckets[,] Access Points for directory buckets[,] Access
/// Points for general purpose buckets[,] Directory buckets[,] S3 Storage Lens
/// groups[,] S3 Access Grants instances, registered locations, or grants"
/// (`TagResource`), so the ARN's own resource type is what says which resource
/// — and so which store's tag set — a call addresses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    /// A bucket, named by the ARN's resource: a general purpose bucket
    /// (`arn:<partition>:s3:::<name>`, whose Region and account fields are
    /// empty "because a bucket name is globally unique") or a directory bucket
    /// (`arn:<partition>:s3express:<region>:<account>:bucket/<name>`). A
    /// bucket's tag set is the one Amazon S3's own
    /// `GetBucketTagging`/`PutBucketTagging` read and write, so it lives in the
    /// bucket store THAT service serves rather than in a second copy here.
    Bucket(String),
    /// A resource THIS service holds — an access point, a Storage Lens group, or
    /// an S3 Access Grants instance, registered location or grant. Which one it
    /// is does not have to be carried: the resource is addressed by the ARN it
    /// reports for itself
    /// ([`ComAmazonawsS3controlAwss3controlservicev20180820State::resource_tags_mut`]),
    /// and this surface answers ONE missing-resource refusal for every kind of
    /// resource it tags ([`no_such_tagged_resource`]).
    Held,
}

/// The resource `resource_arn` names, or `None` when it names no S3 resource
/// this surface tags.
///
/// Read through the workspace's ONE caller-supplied-ARN parse
/// ([`::service_common::arn`]), so this service does not split an ARN its own
/// way: the vendor field has to be [`S3_ARN_NAMESPACE`] or
/// [`S3_EXPRESS_ARN_NAMESPACE`] and the resource type is what the form after it
/// is read against.
///
/// Deliberately independent of `S3ResourceArn`'s modelled `@pattern`, which
/// requires a non-colon character straight after `s3:` and so admits no general
/// purpose bucket ARN: the service accepts that form, which is why the pattern
/// is off the boundary for these three operations (`skip_validation` in
/// `src/ops/types/`) and the ARN is read here instead.
pub fn tagged_resource(resource_arn: &str) -> Option<TaggedResource> {
    let arn = ::service_common::arn::parse(resource_arn)?;
    if arn.vendor != S3_ARN_NAMESPACE && arn.vendor != S3_EXPRESS_ARN_NAMESPACE {
        return None;
    }
    let resource_type = arn.resource_type();
    // A general purpose bucket ARN carries the bucket name as its WHOLE
    // resource field — there is no type token — beside empty Region and account
    // fields.
    if resource_type == arn.resource {
        return (arn.vendor == S3_ARN_NAMESPACE && arn.region.is_empty() && arn.account.is_empty())
            .then(|| TaggedResource::Bucket(arn.resource.to_string()));
    }
    let id = ::service_common::arn::resource_id_of_type(resource_arn, resource_type)?;
    match resource_type {
        BUCKET_RESOURCE_TYPE => Some(TaggedResource::Bucket(id.to_string())),
        ACCESS_POINT_RESOURCE_TYPE
        | STORAGE_LENS_GROUP_RESOURCE_TYPE
        | ACCESS_GRANTS_RESOURCE_TYPE => Some(TaggedResource::Held),
        _ => None,
    }
}

/// What a generic tagging call does to the one tag set it addresses.
///
/// The three operations differ only in this, so the rule each follows is stated
/// once here and applied to whichever store holds the addressed resource's
/// tags.
#[derive(Debug, Clone, Copy)]
pub enum TagSetChange<'a> {
    /// `TagResource`: add each pair, re-valuing a key the resource already
    /// carries — "Creates a new user-defined tag or updates an existing tag"
    /// (`TagResource`), so a tagging call MERGES rather than replacing, and the
    /// tags an earlier call set are kept.
    Merge(&'a [Tag]),
    /// `UntagResource`: remove the keys it names and nothing else — "This
    /// operation ... removes the specified Amazon Web Services resource tags"
    /// (`UntagResource`). A key the resource does not carry removes nothing and
    /// is not an error, so repeating one call answers what making it once did.
    Remove(&'a [String]),
    /// `ListTagsForResource`: report the set, changing nothing.
    Report,
}

impl TagSetChange<'_> {
    /// The tag set `tags` becomes under this change, or `None` when the change
    /// writes nothing (a report).
    ///
    /// `Err(count)` when a merge would take the resource past
    /// [`MAX_RESOURCE_TAGS`] tags, carrying how many it would have ended up
    /// with; nothing is written then, so a refused call leaves the tag set
    /// exactly as it was.
    pub fn applied(&self, tags: &[Tag]) -> Result<Option<Vec<Tag>>, usize> {
        match self {
            Self::Report => Ok(None),
            Self::Merge(pairs) => {
                let mut merged = tags.to_vec();
                for pair in *pairs {
                    match merged.iter_mut().find(|held| held.key == pair.key) {
                        Some(held) => held.value = pair.value.clone(),
                        None => merged.push(pair.clone()),
                    }
                }
                if merged.len() > MAX_RESOURCE_TAGS {
                    return Err(merged.len());
                }
                Ok(Some(merged))
            }
            Self::Remove(keys) => {
                let mut kept = tags.to_vec();
                kept.retain(|held| !keys.iter().any(|removed| removed == &held.key));
                Ok(Some(kept))
            }
        }
    }
}

/// Apply `change` to the ONE tag set `resource_arn` addresses, wherever that
/// tag set lives, and answer the set afterwards.
///
/// The whole of what the three generic tagging operations do, so none of them
/// resolves an ARN or merges a tag set its own way: a bucket's tags are reached
/// in the bucket store Amazon S3 serves ([`change_bucket_tags`]) and every other
/// resource's in this service's own ([`
/// ComAmazonawsS3controlAwss3controlservicev20180820State::resource_tags_mut`]),
/// and an ARN naming a resource no store holds is refused as that resource being
/// missing ([`no_such_tagged_resource`]) rather than as a bad request.
pub fn change_resource_tags<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    account_id: &str,
    resource_arn: &str,
    change: TagSetChange<'_>,
) -> Result<Vec<Tag>, crate::scaffolded::Scaffolded<E>> {
    let Some(resource) = tagged_resource(resource_arn) else {
        return Err(untaggable_resource_arn(resource_arn));
    };
    if let TaggedResource::Bucket(bucket) = &resource {
        return change_bucket_tags(engine, bucket, change);
    }
    let Some(state) = service_state(engine) else {
        return Err(state_unavailable());
    };
    let mut store = state.lock();
    let Some(held) = store.resource_tags_mut(account_id, resource_arn) else {
        return Err(no_such_tagged_resource());
    };
    match change.applied(held).map_err(too_many_resource_tags)? {
        Some(updated) => {
            *held = updated.clone();
            Ok(updated)
        }
        // A report writes nothing, so a later caller sees what this one saw.
        None => Ok(held.clone()),
    }
}

/// [`change_resource_tags`] for a BUCKET, whose tag set is the one Amazon S3's
/// own bucket tagging operations read and write.
///
/// A bucket has ONE tag set however it is written: a set this surface records is
/// what S3's `GetBucketTagging` reports, and a set `PutBucketTagging` recorded is
/// what this surface reports. So the set is reached in the bucket store that
/// service serves rather than copied into this one, where each API would hold a
/// tag set of its own and neither would see the other's.
///
/// LOCK ORDER: this takes the bucket store's lock and holds no other while it
/// does, and hands back an owned value rather than a guard — so it must be
/// called with NO guard of this service's store held. Keeping the cross-service
/// access one-directional and guard-free is what makes a lock cycle between the
/// two stores unreachable.
///
/// Takes no account id, where a resource THIS store holds is reached under the
/// account that owns it: a bucket name is globally unique and the bucket store
/// keys a bucket by that name alone, which is also why a bucket ARN carries no
/// account field for a request to be checked against.
fn change_bucket_tags<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    bucket: &str,
    change: TagSetChange<'_>,
) -> Result<Vec<Tag>, crate::scaffolded::Scaffolded<E>> {
    use ::service_com_amazonaws_s3_amazons3::state::ResourceTag;

    let Some(state) = ::service_com_amazonaws_s3_amazons3::state::service_state(engine) else {
        return Err(state_unavailable());
    };
    let mut store = state.lock();
    let Some(held) = store.bucket_mut(bucket) else {
        return Err(no_such_tagged_resource());
    };
    let current: Vec<Tag> = held
        .tags()
        .iter()
        .map(|tag| Tag {
            key: tag.key.clone(),
            value: tag.value.clone(),
        })
        .collect();
    match change.applied(&current).map_err(too_many_resource_tags)? {
        Some(updated) => {
            held.replace_tags(
                updated
                    .iter()
                    .map(|tag| ResourceTag {
                        key: tag.key.clone(),
                        value: tag.value.clone(),
                    })
                    .collect(),
            );
            Ok(updated)
        }
        None => Ok(current),
    }
}

/// `InvalidRequest` (HTTP 400): `ResourceArn` names no S3 resource the generic
/// tagging surface tags.
///
/// The code and status are the ones this service answers a well-formed request
/// it cannot carry out with (see [`job_not_found`]); a refusal of this service
/// travels in its own S3-style XML `<Code>`/`<Message>` envelope, as every
/// refusal above does.
pub fn untaggable_resource_arn<E>(resource_arn: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequest",
        400,
        format!("The specified ARN names no S3 resource that can be tagged: {resource_arn}"),
    )
}

/// `NoSuchResource` (HTTP 404): no resource the generic tagging surface holds
/// tags for answers to that ARN — the resource was never created, or has been
/// deleted, or belongs to another account.
///
/// ONE refusal for every kind of resource this surface tags, rather than each
/// resource's own `NoSuch<Resource>` code: a recorded `ListTagsForResource` on
/// the ARN of a Storage Lens group that had just been deleted is answered
/// `NoSuchResource`, and the resource kind is not something a tagging call
/// names — it reads the ARN it was handed. The fixed message is the sentence S3
/// publishes for that code, naming no resource: it is what the surface answers
/// for every ARN it holds nothing under.
pub fn no_such_tagged_resource<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchResource",
        404,
        "The specified resource doesn't exist.".to_string(),
    )
}

/// `TooManyTagsException` (HTTP 400): the tag set the request would leave on
/// the resource carries more than [`MAX_RESOURCE_TAGS`] tags.
///
/// The code and status are this service's own modelled `TooManyTagsException`
/// ("Amazon S3 throws this exception if you have too many tags in your tag
/// set", a 400 client error), which these operations' models do not list — the
/// same pairing [`too_many_storage_lens_tags`] answers one tag set over.
pub fn too_many_resource_tags<E>(submitted: usize) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyTagsException",
        400,
        format!(
            "An S3 resource carries at most {MAX_RESOURCE_TAGS} tags; this request would \
             leave it with {submitted}."
        ),
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsS3controlAwss3controlservicev20180820State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsS3controlAwss3controlservicev20180820State>)
}

/// The region this request was signed for, which is the region the resources it
/// touches live in. Falls back to the AWS SDK default region when the request
/// carries no SigV4 credential scope (authentication may be off).
pub fn signing_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an unsigned request is answered in — the region the emulator's own
/// generated credentials file names (`vendor/server/src/lib.rs`).
const DEFAULT_REGION: &str = "us-east-1";

/// The ARN of an access point: the `accesspoint/<name>` resource in this
/// service's ARN namespace (`s3`, per the model's `awsService.arnNamespace`).
///
/// The account segment comes from [`::emulator_server::emulator_arn`], the single
/// source of truth for the canonical emulator account — a generated emulator
/// never puts another account id in an ARN it emits.
pub fn access_point_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("accesspoint/{name}"))
}

/// The host an access point's data plane is reached at, in `region`, over
/// `variant` — the Regional S3 access point endpoints, whose host is built from
/// the `s3-accesspoint` service name, the FIPS and dual-stack markers the
/// variant carries, the Region and the partition's DNS suffix.
fn access_point_endpoint(region: &str, fips: bool, dualstack: bool) -> String {
    let service = if fips {
        "s3-accesspoint-fips"
    } else {
        "s3-accesspoint"
    };
    let stack = if dualstack { "dualstack." } else { "" };
    format!("{service}.{stack}{region}.{DNS_SUFFIX}")
}

/// The DNS suffix of the partition this emulator answers for.
const DNS_SUFFIX: &str = "amazonaws.com";

/// The endpoints an access point is reachable at, keyed by the IP stack and
/// FIPS mode each one serves — what `GetAccessPoint$Endpoints` reports.
///
/// A client resolves the access point's DATA-PLANE host from this map rather
/// than building one, so all four Regional variants are reported: plain IPv4,
/// FIPS, dual-stack, and FIPS over dual-stack. Every host is DERIVED from the
/// Region the access point lives in, so an access point in another Region
/// reports another four.
pub fn access_point_endpoints(region: &str) -> ::std::collections::HashMap<String, String> {
    [
        ("ipv4", false, false),
        ("fips", true, false),
        ("fips_dualstack", true, true),
        ("dualstack", false, true),
    ]
    .into_iter()
    .map(|(variant, fips, dualstack)| {
        (
            variant.to_string(),
            access_point_endpoint(region, fips, dualstack),
        )
    })
    .collect()
}

/// The bounds Amazon S3 states for the LENGTH of an access point name: "Must be
/// between 3 and 50 characters long".
///
/// The modelled `AccessPointName` maximum is far higher (255) because the same
/// member also carries the URL-encoded ARN of an S3 on Outposts access point,
/// which this service does not serve, and the modelled minimum (3) is the only
/// one of S3's rules the model states at all.
pub const ACCESS_POINT_NAME_LEN: ::std::ops::RangeInclusive<usize> = 3..=50;

/// The characters the HOST an access point is reached at is written in: the
/// lowercase letters, digits and hyphens a label carries, and the dots that
/// separate labels. A character outside this set cannot be part of a host at
/// all, so a path naming an access point with one cannot be parsed — see
/// [`addressable_access_point_name`].
fn host_character(character: char) -> bool {
    character.is_ascii_lowercase() || character.is_ascii_digit() || matches!(character, '-' | '.')
}

/// Whether `name` is one the wire can ADDRESS.
///
/// An access point name is one segment of the request path
/// (`/v20180820/accesspoint/{Name}`) and the leading label of the access
/// point's own alias host, and the rules S3 states for a name split in two
/// across the stages that read it. PARSING the path comes first, and it reads
/// the name as part of a HOST: at most [`ACCESS_POINT_NAME_LEN`]'s maximum
/// characters ("Must be between 3 and 50 characters long"), all of them
/// [`host_character`]s. A segment that is not that cannot be parsed, and is
/// answered [`INVALID_URI_CODE`]. The rest of the rules are the name's own and
/// are held against the parsed name by [`valid_access_point_name`].
pub fn addressable_access_point_name(name: &str) -> bool {
    access_point_name_within_addressable_length(name) && name.chars().all(host_character)
}

/// Whether `name` is short enough for one path segment to CARRY as an access
/// point name: at most [`ACCESS_POINT_NAME_LEN`]'s maximum characters.
///
/// Half of [`addressable_access_point_name`], named on its own because a segment
/// LONGER than this is not something the service can read as a name at all —
/// there is no resource for the path to name — while a segment of a length it can
/// read names a resource whether or not the name is one it will accept. That is
/// what decides which of the two a refusal states in its `URI` member (see
/// `crate::service::envelope`).
pub fn access_point_name_within_addressable_length(name: &str) -> bool {
    name.chars().count() <= *ACCESS_POINT_NAME_LEN.end()
}

/// Whether `name` is a name an access point may be GIVEN, for a path segment
/// this service could already parse ([`addressable_access_point_name`]).
///
/// The rest of the rules S3 states for an access point name: it must be at
/// least [`ACCESS_POINT_NAME_LEN`]'s minimum, it "can consist only of lowercase
/// letters, numbers, and hyphens", it "must begin and end with a letter or
/// number", and it "must not end with the suffix -s3alias" ([`ALIAS_TAIL`],
/// which is what an ALIAS ends in, so a name ending in it could not be told
/// from one). A name that breaks one of these sits in a path this service
/// parses perfectly well, so it is refused as a request this service will not
/// carry out ([`INVALID_ACCESS_POINT_NAME_MESSAGE`]) rather than as an
/// unparseable URI.
pub fn valid_access_point_name(name: &str) -> bool {
    let alphanumeric =
        |character: char| character.is_ascii_lowercase() || character.is_ascii_digit();
    ACCESS_POINT_NAME_LEN.contains(&name.chars().count())
        && name
            .chars()
            .all(|character| alphanumeric(character) || character == '-')
        && name.starts_with(alphanumeric)
        && name.ends_with(alphanumeric)
        && !name.ends_with(ALIAS_TAIL)
}

/// The ARN namespace of the buckets this service's BUCKET routes address: S3 on
/// Outposts.
///
/// Every `/v20180820/bucket/{Bucket}` route documents its `Bucket` member as "the
/// S3 on Outposts bucket", addressed "in the format
/// `arn:aws:s3-outposts:<region>:<account>:outpost/<outpost>/bucket/<name>`"
/// whose "value must be URL encoded"
/// (`smithy_query(query="doc",shape="PutBucketReplicationRequest",member="Bucket")`),
/// so the vendor field of a parsed bucket address is this one.
pub const OUTPOSTS_VENDOR: &str = "s3-outposts";

/// The resource type an S3 on Outposts BUCKET ARN names under its outpost:
/// `outpost/<outpost>/bucket/<name>`, so the resource field of a bucket address
/// carries this segment.
pub const OUTPOSTS_BUCKET_SEGMENT: &str = "/bucket/";

/// Whether `bucket` — the address span of a bucket route's request path, already
/// percent-decoded — is an S3 on Outposts bucket ARN
/// ([`OUTPOSTS_VENDOR`]).
///
/// One of the two forms those routes document a bucket is addressed in: the ARN
/// the SDK and CLI put in the label. The other is the bucket's NAME beside the
/// `x-amz-outpost-id` header the REST API requires, which is read where the
/// request's headers are (`crate::service::envelope`), so this reads the label
/// alone. A request in neither form names no Outpost, no account and no Region,
/// so there is no resource for the path to address and the URI cannot be parsed
/// at all ([`INVALID_URI_CODE`]) — a different answer from a bucket that is
/// addressed properly and merely does not exist.
pub fn addressable_bucket(bucket: &str) -> bool {
    ::service_common::arn::parse(bucket).is_some_and(|arn| {
        arn.vendor == OUTPOSTS_VENDOR && arn.resource.contains(OUTPOSTS_BUCKET_SEGMENT)
    })
}

/// The characters a Storage Lens configuration id can be ADDRESSED with: the
/// ASCII letters and digits, the hyphen and the underscore.
///
/// The modelled `ConfigId` pattern (`^[a-zA-Z0-9\-\_\.]+$`) also admits the
/// dot, but the id is read out of the request path before any operation is
/// named and a segment carrying a dot is not one the wire reads as a
/// configuration id at all — so the ids the model admits and the ids a URI can
/// carry are two different sets, and this is the narrower one.
fn storage_lens_config_id_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_')
}

/// Whether `config_id` is one the wire can ADDRESS a Storage Lens
/// configuration by.
///
/// Read off the request path by `crate::service::envelope` before an operation
/// is named — exactly as [`addressable_access_point_name`] is one resource over
/// — so an id the path cannot carry is answered [`INVALID_URI_CODE`] before any
/// configuration is read, written or deleted. The id's remaining rule, its
/// length, is the one the model states (`ConfigId` is 1..64) and is held
/// against the parsed member by the request boundary.
pub fn addressable_storage_lens_config_id(config_id: &str) -> bool {
    !config_id.is_empty() && config_id.chars().all(storage_lens_config_id_character)
}

/// The ARN of an S3 Storage Lens configuration.
///
/// `StorageLensConfiguration$StorageLensArn` documents the member as read-only
/// and states the format it follows:
/// `arn:aws:s3:<region>:<account-id>:storage-lens/<config-id>` — so the service
/// assigns it from the configuration's own identity rather than taking whatever
/// a request submitted. The account segment comes from
/// [`::emulator_server::emulator_arn`], the single source of truth for the
/// canonical emulator account.
pub fn storage_lens_configuration_arn(region: &str, config_id: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("storage-lens/{config_id}"))
}

/// The ARN of an S3 Storage Lens group.
///
/// `StorageLensGroup$StorageLensGroupArn` documents the member as read-only,
/// and the `StorageLensGroupArn` shape constrains it to
/// `^arn:[a-z\-]+:s3:[a-z0-9\-]+:\d{12}:storage\-lens\-group\/` — so the
/// service assigns it from the group's own identity (its region and name)
/// rather than taking whatever a request submitted. The account segment comes
/// from [`::emulator_server::emulator_arn`], the single source of truth for the
/// canonical emulator account.
pub fn storage_lens_group_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("storage-lens-group/{name}"))
}

/// The ARN of an S3 Access Grants instance: the `access-grants/<instance-id>`
/// resource in this service's ARN namespace (`s3`, per the model's
/// `awsService.arnNamespace`), in the Region the instance lives in — the form
/// `AccessGrantsInstanceArn`'s pattern
/// (`^arn:[a-z\-]+:s3:[a-z0-9\-]+:\d{12}:access\-grants\/[a-zA-Z0-9\-]+$`)
/// admits.
///
/// The account segment comes from [`::emulator_server::emulator_arn`], the
/// single source of truth for the canonical emulator account.
pub fn access_grants_instance_arn(region: &str, instance_id: &str) -> String {
    ::emulator_server::emulator_arn("s3", region, &format!("access-grants/{instance_id}"))
}

/// The resource-type token of a registered location in the ARN of an S3 Access
/// Grants location, as [`access_grants_location_arn`] builds it.
const ACCESS_GRANTS_LOCATION_SUBRESOURCE: &str = "location";

/// The resource-type token of a grant in the ARN of an access grant, as
/// [`access_grant_arn`] builds it.
const ACCESS_GRANT_SUBRESOURCE: &str = "grant";

/// The ARN of a resource held INSIDE one S3 Access Grants instance — a
/// registered location or a grant.
///
/// Both are subresources of the instance that holds them, so the ARN each
/// reports is the instance's own resource path
/// (`access-grants/<instance-id>`, see [`access_grants_instance_arn`])
/// followed by the subresource's type and the id S3 Access Grants assigned it:
/// `arn:<partition>:s3:<region>:<account>:access-grants/<instance-id>/<type>/<id>`.
/// The instance segment is what says WHICH instance the location or grant
/// belongs to, so an ARN without it addresses no resource; an instance carries
/// the one id [`ACCESS_GRANTS_INSTANCE_ID`], which is the id this path names.
///
/// DELIBERATELY not the form `AccessGrantsLocationArn`'s and `AccessGrantArn`'s
/// modelled patterns state (`…:access\-grants\/location/<id>`,
/// `…:access\-grants\/grant/<id>`): those patterns omit the instance segment,
/// and the recorded wire — which is the authority for the ARN a client is
/// handed — carries it. The patterns bind an INPUT member's value and the
/// framework validates them there; nothing validates an output member's value
/// against them, so the longer resource field is what every projection reports.
/// Do not "repair" this back to the pattern.
///
/// The account segment comes from [`::emulator_server::emulator_arn`], the
/// single source of truth for the canonical emulator account.
fn access_grants_subresource_arn(region: &str, subresource_type: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        "s3",
        region,
        &format!(
            "{ACCESS_GRANTS_RESOURCE_TYPE}/{ACCESS_GRANTS_INSTANCE_ID}/{subresource_type}/{id}"
        ),
    )
}

/// The ARN of a registered S3 Access Grants location, in the Region the
/// location was registered in: the location subresource of the instance that
/// holds it ([`access_grants_subresource_arn`]).
pub fn access_grants_location_arn(region: &str, location_id: &str) -> String {
    access_grants_subresource_arn(region, ACCESS_GRANTS_LOCATION_SUBRESOURCE, location_id)
}

/// The ARN of an access grant, in the Region the grant was created in: the
/// grant subresource of the instance that holds it
/// ([`access_grants_subresource_arn`]).
pub fn access_grant_arn(region: &str, grant_id: &str) -> String {
    access_grants_subresource_arn(region, ACCESS_GRANT_SUBRESOURCE, grant_id)
}

/// How many characters of the Identity Center application segment an
/// application ARN this emulator mints carries after `apl-`.
const IDENTITY_CENTER_APPLICATION_SUFFIX_LEN: usize = 16;

/// The ARN of the IAM Identity Center application S3 Access Grants creates for
/// an instance that is associated with an Identity Center instance.
///
/// `CreateAccessGrantsInstance` documents that S3 Access Grants creates this
/// application itself ("S3 Access Grants creates this Identity Center
/// application for the specific S3 Access Grants instance"), so the ARN is
/// assigned by the service and never taken from the request. It follows
/// `IdentityCenterApplicationArn`'s pattern
/// (`^arn:[^:]+:sso::\d{12}:application/.*$`): an `sso` ARN carries no Region,
/// and its resource is `application/<identity-center-instance-id>/apl-<suffix>`.
///
/// Both segments are DERIVED from the instance's identity — the Identity Center
/// instance it is a subresource of, and the account and Region of the S3 Access
/// Grants instance it was created for — rather than drawn at random, so the ARN
/// is one value per instance: stable across every later read, and different for
/// a different instance.
pub fn identity_center_application_arn(
    identity_center_instance_arn: &str,
    account_id: &str,
    region: &str,
) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(
        format!("{identity_center_instance_arn}/{account_id}/{region}").as_bytes(),
    );
    let suffix: String = hex::encode(digest)
        .chars()
        .take(IDENTITY_CENTER_APPLICATION_SUFFIX_LEN)
        .collect();
    // The Identity Center instance id is the tail of the instance ARN, whose
    // resource the pattern `^arn:[^:]+:sso::(\d{12}){0,1}:instance/.*$` spells
    // `instance/<id>`. An ARN whose tail is empty leaves the application a
    // subresource of the digest alone, which still names one instance.
    let instance_id = identity_center_instance_arn
        .rsplit("instance/")
        .next()
        .unwrap_or_default();
    let resource = if instance_id.is_empty() {
        format!("application/apl-{suffix}")
    } else {
        format!("application/{instance_id}/apl-{suffix}")
    };
    ::emulator_server::emulator_arn("sso", "", &resource)
}

/// The suffix length AWS access point aliases carry between the name and the
/// `-s3alias` tail: a 26-character, service-assigned string.
const ALIAS_SUFFIX_LEN: usize = 26;

/// The tail every S3 access point alias ends in.
const ALIAS_TAIL: &str = "-s3alias";

/// `Alias`'s modelled maximum length (`com.amazonaws.s3control#Alias`, len 0..63).
const ALIAS_MAX_LEN: usize = 63;

/// The alias of an access point: `<name>-<26 service-assigned characters>-s3alias`,
/// the form S3 reports for an access point alias.
///
/// The middle segment is derived from the access point's identity (its owning
/// account and name) rather than drawn at random, so it is one value per access
/// point: stable across every later read, and different for a different access
/// point. The name segment is truncated when the whole alias would pass the
/// modelled 63-character maximum.
pub fn access_point_alias(account_id: &str, name: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(format!("{account_id}/{name}").as_bytes());
    let suffix: String = hex::encode(digest).chars().take(ALIAS_SUFFIX_LEN).collect();

    let room_for_name = ALIAS_MAX_LEN - ALIAS_TAIL.len() - ALIAS_SUFFIX_LEN - 1;
    let lowered = name.to_lowercase();
    let head: String = lowered.chars().take(room_for_name).collect();
    format!("{head}-{suffix}{ALIAS_TAIL}")
}

/// The `NetworkOrigin` an access point restricted to a VPC reports, as
/// [`network_origin`] decides it: the one spelling both that projection and the
/// confinement the data path enforces ([`confining_vpc`]) are read from.
const VPC_NETWORK_ORIGIN: &str = "VPC";

/// The network origin of an access point: `VPC` when the create call restricted
/// it to a VPC, otherwise `Internet` — the rule the `AccessPoint$NetworkOrigin`
/// documentation states.
pub fn network_origin(vpc_configuration: Option<&VpcConfiguration>) -> String {
    let vpc_restricted = vpc_configuration.is_some_and(|vpc| !vpc.vpc_id.is_empty());
    let wanted = if vpc_restricted {
        VPC_NETWORK_ORIGIN
    } else {
        "Internet"
    };
    // The spelling that goes on the wire is the one the generated
    // NETWORK_ORIGIN_VALUES carries (from the model's `NetworkOrigin` enum),
    // found by VALUE rather than by position: neither a reordering of the
    // generated list nor a typo in the name above can change what is returned.
    NETWORK_ORIGIN_VALUES
        .iter()
        .copied()
        .find(|value| *value == wanted)
        .unwrap_or(wanted)
        .to_string()
}

/// The VPC an access point is CONFINED to: the id its `VpcConfiguration` names
/// when that configuration makes the access point's network origin `VPC`
/// ([`network_origin`], the one reading of that rule), and `None` for an access
/// point of `Internet` origin.
///
/// "If this field is specified, this access point will only allow connections
/// from the specified VPC ID" (`VpcConfiguration$VpcId`) — a restriction on the
/// DATA PATH, which is published with that path
/// ([`crate::state::access_point_routes::publish`]). Derived from the same
/// reading the reported origin is, so what this service REPORTS about an access
/// point and what its data path ENFORCES cannot drift.
pub fn confining_vpc(vpc_configuration: Option<&VpcConfiguration>) -> Option<String> {
    (network_origin(vpc_configuration) == VPC_NETWORK_ORIGIN)
        .then(|| vpc_configuration.map(|configuration| configuration.vpc_id.clone()))
        .flatten()
}

/// Encode a `ListAccessPoints` page token: the opaque form of the last access
/// point name the previous page returned.
///
/// Encoded rather than passed through so a caller does not come to depend on a
/// token's contents, the way an AWS page token's contents are not part of its
/// contract. The encoding is reversible and is NOT a secret or an access check:
/// a token only names a position in the account's own scan.
pub fn encode_page_token(last_name: &str) -> String {
    hex::encode(last_name.as_bytes())
}

/// Decode a page token produced by [`encode_page_token`], or `None` when it was
/// not produced by it.
pub fn decode_page_token(token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    String::from_utf8(bytes).ok()
}

/// Where a job listing stopped: the position of the last job the previous page
/// returned, in the order a listing scans ([`JobRecord::creation_time`] newest
/// first, ties broken by job id), so the next page resumes strictly after it.
///
/// A position and not a cursor: the same token names the same place however many
/// times it is presented. Carried through [`encode_page_token`], so a job
/// listing's tokens are opaque in exactly the same way the other listings' are.
pub fn encode_job_page_token(creation_time: ::aws_smithy_types::DateTime, job_id: &str) -> String {
    encode_page_token(&format!(
        "{}:{}:{job_id}",
        creation_time.secs(),
        creation_time.subsec_nanos()
    ))
}

/// The position a token produced by [`encode_job_page_token`] names — the
/// creation instant (seconds and nanoseconds since the epoch) and the job id —
/// or `None` when it was not produced by it.
pub fn decode_job_page_token(token: &str) -> Option<(i64, u32, String)> {
    let decoded = decode_page_token(token)?;
    let (secs, rest) = decoded.split_once(':')?;
    let (nanos, job_id) = rest.split_once(':')?;
    if job_id.is_empty() {
        return None;
    }
    Some((secs.parse().ok()?, nanos.parse().ok()?, job_id.to_string()))
}

// ---------------------------------------------------------------------------
// The derivations the Multi-Region Access Point lifecycle shares.
// ---------------------------------------------------------------------------

/// The tail every Multi-Region Access Point alias ends in, per the modelled
/// `MultiRegionAccessPointAlias` pattern `^[a-z][a-z0-9]*[.]mrap$`.
const MRAP_ALIAS_TAIL: &str = ".mrap";

/// How many characters of a Multi-Region Access Point alias precede
/// [`MRAP_ALIAS_TAIL`]. Well inside the modelled 63-character maximum, so an
/// alias never has to be truncated.
const MRAP_ALIAS_BODY_LEN: usize = 16;

/// The characters an alias body is drawn from: the first from the letters alone,
/// because the modelled pattern requires an alias to START with `[a-z]`, the
/// rest from letters and digits.
const MRAP_ALIAS_LETTERS: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const MRAP_ALIAS_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";

/// The alias of a Multi-Region Access Point: a service-assigned string of the
/// form the modelled `MultiRegionAccessPointAlias` pattern admits, ending in
/// `.mrap`.
///
/// Unlike an access point's alias, a Multi-Region Access Point's carries no part
/// of the name — the model documents the two as distinct, and the pattern admits
/// no hyphen. It is derived from the access point's identity (its owning account
/// and name) rather than drawn at random, so it is one value per access point:
/// stable across every later read, and different for a different access point.
pub fn multi_region_access_point_alias(account_id: &str, name: &str) -> String {
    use sha2::Digest;

    let digest = sha2::Sha256::digest(format!("mrap/{account_id}/{name}").as_bytes());
    let mut alias = String::with_capacity(MRAP_ALIAS_BODY_LEN + MRAP_ALIAS_TAIL.len());
    for (position, byte) in digest.iter().take(MRAP_ALIAS_BODY_LEN).enumerate() {
        let alphabet = if position == 0 {
            MRAP_ALIAS_LETTERS
        } else {
            MRAP_ALIAS_ALPHABET
        };
        alias.push(alphabet[usize::from(*byte) % alphabet.len()] as char);
    }
    alias.push_str(MRAP_ALIAS_TAIL);
    alias
}

/// The `async-request/mrap/create` path segment, and the `create` half of a
/// request token ARN.
pub const ASYNC_VERB_CREATE: &str = "create";

/// The `delete` half of a request token ARN. See [`ASYNC_VERB_CREATE`].
pub const ASYNC_VERB_DELETE: &str = "delete";

/// The `put-policy` half of a request token ARN, spelled as the operation's own
/// route spells it (`POST /v20180820/async-requests/mrap/put-policy`). See
/// [`ASYNC_VERB_CREATE`].
pub const ASYNC_VERB_PUT_POLICY: &str = "put-policy";

/// The ARN of an asynchronous Multi-Region Access Point request: the
/// `async-request/mrap/<verb>/<id>` resource in this service's ARN namespace
/// (`s3`, per the model's `awsService.arnNamespace`), in the Region the request
/// was signed for — which satisfies the modelled `AsyncRequestTokenARN` pattern
/// `^arn:.+$`.
///
/// The id is the UUID every service-assigned identifier of this service is
/// shaped as ([`uuid_from_digest`]), carrying a digest of the request's own
/// identity — the verb, the account, the caller's `ClientToken` and the details
/// it submitted — so one request has ONE token: a byte-identical retry is
/// handed the token the first call returned, which is what an
/// `@idempotencyToken` asks for ("guarantee that requests are unique"), and two
/// different requests never share a token. The account segment comes from
/// [`::emulator_server::emulator_arn`], the single source of truth for the
/// canonical emulator account.
///
/// The VALUE is this service's own to mint and no caller may depend on a
/// particular one: what a token must do is name ONE request, which is what the
/// `DescribeMultiRegionAccessPointOperation` route resolves it as.
pub fn async_request_token_arn(
    region: &str,
    verb: &str,
    account_id: &str,
    client_token: &str,
    details: &str,
) -> String {
    use sha2::Digest;

    let digest =
        sha2::Sha256::digest(format!("{verb}\n{account_id}\n{client_token}\n{details}").as_bytes());
    let id = uuid_from_digest(&digest);
    ::emulator_server::emulator_arn("s3", region, &format!("async-request/mrap/{verb}/{id}"))
}

/// A `CreateMultiRegionAccessPoint` request's details as one string, for
/// [`async_request_token_arn`]: every member the caller submitted, so two
/// requests that differ in any of them differ in their token.
pub fn create_details_fingerprint(details: &CreateMultiRegionAccessPointInput2) -> String {
    let mut fingerprint = format!("name={}", details.name);
    for region in &details.regions {
        fingerprint.push_str(&format!(
            "|bucket={}@{}",
            region.bucket,
            region.bucket_account_id.as_deref().unwrap_or_default()
        ));
    }
    if let Some(block) = &details.public_access_block {
        fingerprint.push_str(&format!(
            "|block={},{},{},{}",
            block.block_public_acls,
            block.ignore_public_acls,
            block.block_public_policy,
            block.restrict_public_buckets
        ));
    }
    fingerprint
}

/// A `DeleteMultiRegionAccessPoint` request's details as one string, for
/// [`async_request_token_arn`].
pub fn delete_details_fingerprint(details: &DeleteMultiRegionAccessPointInput2) -> String {
    format!("name={}", details.name)
}

/// A `PutMultiRegionAccessPointPolicy` request's details as one string, for
/// [`async_request_token_arn`]: the access point it names and the document it
/// submitted, so two puts of DIFFERENT documents under one client token are two
/// requests while a byte-identical retry is one.
pub fn put_policy_details_fingerprint(details: &PutMultiRegionAccessPointPolicyInput2) -> String {
    format!("name={}|policy={}", details.name, details.policy)
}

// ---------------------------------------------------------------------------
// The refusals the access point lifecycle shares.
//
// S3 Control's model declares no errors for these operations, so each is built
// through `Scaffolded::service_failure`, which carries the wire `<Code>` and the
// HTTP status a client branches on. They live here, once, so four handlers
// cannot drift on a code.
// ---------------------------------------------------------------------------

/// `NoSuchAccessPoint` (HTTP 404): no access point of that name is owned by the
/// account the request names.
///
/// The message is FIXED text, naming no access point: it is what the service
/// answers for every access point that is not there. WHICH name was looked up
/// travels in the member beside it ([`ACCESS_POINT_NAME_MEMBER`]), which is
/// where a client reads the offending identity from instead of re-parsing the
/// request it sent.
pub fn no_such_access_point<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure_naming(
        "NoSuchAccessPoint",
        404,
        "The specified accesspoint does not exist",
        vec![crate::scaffolded::ErrorDetail::new(
            &ACCESS_POINT_NAME_MEMBER,
            name,
        )],
    )
}

/// `InvalidURI` (HTTP 400): the request path is not one this service can parse,
/// because the access point name it carries is longer than the wire can address
/// ([`ADDRESSABLE_ACCESS_POINT_NAME_LEN`]).
///
/// A refusal of the URI rather than of a member: it is answered by
/// `crate::service::envelope`, which reads the path before any operation is
/// named, so no access point is looked up, created or deleted. The message is
/// fixed text and the path the caller sent travels in the `URI` member beside
/// it — a path this service could not parse resolves to no resource, so there is
/// nothing to name but the request target.
///
/// The wire `<Code>` a client branches on.
pub const INVALID_URI_CODE: &str = "InvalidURI";

/// The HTTP status of an [`INVALID_URI_CODE`] refusal.
pub const INVALID_URI_STATUS: u16 = 400;

/// The message of an [`INVALID_URI_CODE`] refusal: fixed text, naming no path.
pub const INVALID_URI_MESSAGE: &str = "Couldn't parse the specified URI.";

/// `InvalidRequest` (HTTP 400): a request this service will not carry out,
/// including one whose body it could not READ at all.
///
/// The message is the fixed sentence S3 Control answers such a request with. It
/// states the CLASS and nothing about the value that caused it: a message built
/// from request text would make a client branch on prose, and the condition is
/// the same one whatever the value.
pub fn request_invalid<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        REQUEST_INVALID_MESSAGE,
    )
}

/// The wire `<Code>` of [`request_invalid`], which a client branches on.
///
/// Named separately because the refusal is answered from two places: a handler
/// that will not carry out the request it was given, and
/// `crate::service::envelope`, which answers it for a request body the codec
/// could not read — the same refusal, so it carries the same code, status and
/// message.
pub const REQUEST_INVALID_CODE: &str = "InvalidRequest";

/// The HTTP status of [`request_invalid`].
pub const REQUEST_INVALID_STATUS: u16 = 400;

/// The message of [`request_invalid`]: fixed text, naming no value.
pub const REQUEST_INVALID_MESSAGE: &str = "Request invalid";

/// The message S3 Control answers a request naming an access point whose name
/// it will not accept with, under [`REQUEST_INVALID_CODE`] and
/// [`REQUEST_INVALID_STATUS`]: fixed text, naming no name.
///
/// A name the wire can ADDRESS but an access point may not be GIVEN
/// ([`valid_access_point_name`]) is a request this service will not carry out,
/// which is a different answer from a path it could not parse at all
/// ([`INVALID_URI_CODE`]) — the two rules the name is held to are what the
/// refusals tell apart.
pub const INVALID_ACCESS_POINT_NAME_MESSAGE: &str = "Your Amazon S3 AccessPoint name is invalid";

/// `InvalidRequest` (HTTP 400): the create named a bucket the object store does
/// not hold, so there is no bucket for the access point to be attached to.
///
/// An access point is a resource OF a bucket — "Creates an access point and
/// associates it to a specified bucket" (`CreateAccessPoint`) — so the bucket is
/// resolved in the store that owns it ([`bucket_regions::bucket_exists`]) before
/// the access point is recorded, and a create that named no existing bucket
/// records nothing.
pub fn access_point_bucket_absent<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        ACCESS_POINT_BUCKET_ABSENT_MESSAGE,
    )
}

/// The message of [`access_point_bucket_absent`]: fixed text, naming no bucket —
/// the caller named one bucket and the answer is about that same one.
pub const ACCESS_POINT_BUCKET_ABSENT_MESSAGE: &str =
    "Amazon S3 AccessPoint can only be created for existing bucket";

/// The message S3 Control answers a create that named NO bucket at all with,
/// under [`REQUEST_INVALID_CODE`] and [`REQUEST_INVALID_STATUS`]: fixed text,
/// naming neither the member, the value nor a limit.
///
/// `CreateAccessPointRequest$Bucket` is `@required`, so the absence is found by
/// the request boundary before the handler runs; the refusal is this service's
/// own all the same, and is stated by `crate::service::envelope` where that
/// boundary refusal is restated.
pub const MISSING_ACCESS_POINT_BUCKET_MESSAGE: &str =
    "Must specify bucket to create Amazon S3 AccessPoint";

/// The `<Error>` member a refusal names the access point it was asked about in.
///
/// S3 Control's model declares no error shape for these operations, so this
/// member exists only on the wire: it carries the name the REQUEST named, which
/// is what a client reads the offending identity out of instead of re-parsing
/// the request it sent.
static ACCESS_POINT_NAME_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3control#ErrorDetails$AccessPointName",
        "com.amazonaws.s3control",
        "AccessPointName",
    ),
    ShapeType::String,
    "AccessPointName",
    1,
)
.with_xml_name("AccessPointName");

/// `NoSuchAccessPointPolicy` (HTTP 404): the access point exists but carries no
/// policy, because none was ever put on it (or it was deleted).
///
/// The message is fixed text, naming no access point: it is what the service
/// answers for every access point that carries no policy. WHICH access point
/// was asked about travels in the member beside it
/// ([`ACCESS_POINT_NAME_MEMBER`]) — the same member [`no_such_access_point`]
/// carries, because both refusals are about the access point the request
/// addressed.
pub fn no_such_access_point_policy<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure_naming(
        "NoSuchAccessPointPolicy",
        404,
        "The specified accesspoint policy does not exist",
        vec![crate::scaffolded::ErrorDetail::new(
            &ACCESS_POINT_NAME_MEMBER,
            name,
        )],
    )
}

/// `InvalidRequest` (HTTP 400): a listing scoped to a bucket that has no access
/// point attached to it.
///
/// An access point is a resource OF a bucket, so scoping the listing to one asks
/// for that bucket's access points: a bucket with none attached carries no such
/// collection, which is a request the service will not carry out rather than an
/// empty page. The message is fixed text, naming no bucket: the caller asked
/// about one bucket and the answer is about that same one.
pub fn no_access_point_attached_to_bucket<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        NO_ACCESS_POINT_ATTACHED_MESSAGE,
    )
}

/// The message of [`no_access_point_attached_to_bucket`]: fixed text, naming no
/// bucket.
pub const NO_ACCESS_POINT_ATTACHED_MESSAGE: &str = "No access point attached to this bucket";

/// `AccessDenied` (HTTP 403): a `BlockPublicPolicy` setting in force for the
/// access point refuses a policy document that grants public access, so the
/// document is not stored.
pub fn public_policy_access_denied<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        ACCESS_DENIED_CODE,
        ACCESS_DENIED_STATUS,
        ACCESS_DENIED_MESSAGE.to_string(),
    )
}

/// The wire `<Code>` of the refusal S3 Control answers a request the caller is
/// not allowed to have carried out with, which a client branches on.
///
/// Named separately because the refusal is answered from two places: a handler
/// that will not carry out the request it was given, and
/// `crate::service::authorization`, which answers it for a request naming an
/// account the caller holds no credentials for — the same refusal, so it carries
/// the same code, status and message.
pub const ACCESS_DENIED_CODE: &str = "AccessDenied";

/// The HTTP status of an [`ACCESS_DENIED_CODE`] refusal.
pub const ACCESS_DENIED_STATUS: u16 = 403;

/// The message of an [`ACCESS_DENIED_CODE`] refusal: fixed text, naming neither
/// the caller, the account nor the resource.
pub const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The element an [`ACCESS_DENIED_CODE`] refusal of a request addressed in
/// another account names that account in: the request's own `AccountId`,
/// reported back so a caller sees WHICH account it was refused for.
pub const ACCOUNT_ID_ELEMENT: &str = "AccountId";

/// `AccessPointAlreadyOwnedByYou` (HTTP 409): the account already owns an access
/// point of that name, so creating it again would be a second resource under one
/// name.
///
/// The message is the fixed sentence the S3 family answers a create of a
/// resource the caller already owns with, naming no access point: the caller
/// asked for one by name and the answer is about that same one.
pub fn access_point_already_owned_by_you<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessPointAlreadyOwnedByYou",
        409,
        "Your previous request to create the named accesspoint succeeded and you already own it.",
    )
}

/// `TooManyAccessPoints` (HTTP 400): the store already holds
/// [`ACCESS_POINT_QUOTA`] access points, so another record would grow it without
/// end. Released by `DeleteAccessPoint`.
pub fn too_many_access_points<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyAccessPoints",
        400,
        format!("This emulator holds the maximum of {ACCESS_POINT_QUOTA} access points."),
    )
}

/// `MalformedPolicy` (HTTP 400): the policy document is not one this service
/// will store — it is not JSON, or it is larger than [`MAX_POLICY_BYTES`].
pub fn malformed_policy<E>(why: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        MALFORMED_POLICY_CODE,
        MALFORMED_POLICY_STATUS,
        format!("The specified policy is not a policy this service can store: {why}"),
    )
}

/// The wire `<Code>` of a refusal of a policy document, which a client branches
/// on. Named separately because the same code carries every clause a document
/// can break ([`policy_grammar_refusal`]).
pub const MALFORMED_POLICY_CODE: &str = "MalformedPolicy";

/// The HTTP status of a [`MALFORMED_POLICY_CODE`] refusal.
pub const MALFORMED_POLICY_STATUS: u16 = 400;

/// [`MALFORMED_POLICY_CODE`] stating `message`: the refusal of a document whose
/// clause of the IAM policy language AWS words itself, so the message is handed
/// through exactly as that language's reading states it.
pub fn malformed_policy_stating<E>(message: impl Into<String>) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        MALFORMED_POLICY_CODE,
        MALFORMED_POLICY_STATUS,
        message,
    )
}

/// The refusal a text that is not a policy DOCUMENT at all is answered with, or
/// `None` for one that is.
///
/// Every operation of this service that takes a policy document as a string
/// reads at least this clause, so two of them handed the same text cannot
/// disagree about whether it is a policy: a member documented as "a policy
/// document, formatted in JSON" carries a document in the IAM JSON policy
/// language, whose top-level elements are the members of a JSON OBJECT — and the
/// language is read once for the whole workspace
/// ([`::service_common::iam_policy`]).
///
/// The document's SIZE is not read here: the bound is the one the carrying
/// member states, which differs per operation (see [`MAX_POLICY_BYTES`] and the
/// modelled length of the Access Grants `PolicyDocument`), so each caller
/// applies its own before this.
pub fn policy_document_refusal<E>(document: &str) -> Option<crate::scaffolded::Scaffolded<E>> {
    matches!(
        ::service_common::iam_policy::attached_policy_defect(document, |_| true),
        Some(::service_common::iam_policy::AttachedPolicyDefect::NotAPolicyObject)
    )
    .then(|| malformed_policy_stating(::service_common::iam_policy::NOT_A_JSON_POLICY_MESSAGE))
}

/// The refusal a document the ACCESS POINT policy surface will not attach is
/// answered with, or `None` for one it stores.
///
/// The whole reading the validator that surface runs applies
/// ([`::service_common::iam_policy::attached_policy_defect`]), each clause
/// answered with AWS's own sentence for it: the text is a policy object, every
/// member name it carries is one the policy language defines, it carries the
/// element the language requires, it declares a version string the language
/// defines, and every action its statements name is one the language can read
/// and this surface can grant ([`grantable_action`]).
///
/// A document breaking any clause describes nothing an access point's policy
/// could enforce, so it is refused rather than attached. The whole reading is
/// this surface's and not every policy surface's: the Access Grants instance
/// resource policy accepts a member the language does not define and reports it
/// back in the document it renders, so that operation reads only
/// [`policy_document_refusal`].
pub fn access_point_policy_refusal<E>(document: &str) -> Option<crate::scaffolded::Scaffolded<E>> {
    ::service_common::iam_policy::attached_policy_defect(document, |action| {
        grantable_action(action)
    })
    .map(|defect| malformed_policy_stating(defect.message()))
}

/// The IAM service namespaces an action an access point policy grants is named
/// in: the prefixes of the S3 family.
///
/// An access point fronts S3 storage, so a statement of its policy grants an
/// action of S3's own API; an action named in another service's namespace names
/// nothing the access point could allow. The prefixes are the ones the S3 family
/// publishes for its APIs — general-purpose and directory buckets (`s3`,
/// `s3express`), Object Lambda access points (`s3-object-lambda`) and S3 on
/// Outposts (`s3-outposts`).
pub const S3_ACTION_NAMESPACES: [&str; 4] = ["s3", "s3-object-lambda", "s3-outposts", "s3express"];

/// Whether `action` names an action an access point policy can grant: the policy
/// language's wildcard, or a `<namespace>:<action>` pair whose namespace is one
/// of [`S3_ACTION_NAMESPACES`].
pub fn grantable_action(action: &str) -> bool {
    action == ::service_common::iam_policy::ANY_ACTION
        || action
            .split_once(':')
            .is_some_and(|(namespace, _)| S3_ACTION_NAMESPACES.contains(&namespace))
}

/// `AccessDenied` (HTTP 403): the store already holds
/// [`PUBLIC_ACCESS_BLOCK_QUOTA`] accounts' configurations, so recording one for
/// a further account would grow this process's state without end.
/// `PutPublicAccessBlock` documents `AccessDenied` (403) as the refusal a
/// caller gets when the service will not let the account-level configuration be
/// written, and the message says which bound was reached.
pub fn public_access_block_store_full<E>(account_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessDenied",
        403,
        format!(
            "This emulator already holds the maximum of {PUBLIC_ACCESS_BLOCK_QUOTA} \
             PublicAccessBlock configurations and cannot add one for account {account_id}."
        ),
    )
}

/// `NoSuchConfiguration` (HTTP 404): no S3 Storage Lens configuration of that
/// id is owned by the account the request names.
///
/// S3 Control declares no error for the Storage Lens operations, so — exactly
/// as for the access point refusals above — the refusal is carried in the
/// S3-style XML `<Code>`/`<Message>` envelope with the code and status the real
/// service answers a missing configuration with.
///
/// The message is fixed text, naming no configuration id: it is what the
/// service answers for every id the account holds no configuration under.
pub fn no_such_storage_lens_configuration<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "NoSuchConfiguration",
        404,
        "The specified configuration does not exist.".to_string(),
    )
}

/// `InvalidIncludeExcludeContainers` (HTTP 400): the submitted S3 Storage Lens
/// document carries an `Include` container and an `Exclude` container at once.
///
/// The model documents the rule on the member itself
/// (`StorageLensConfiguration$Include`: "This container can only be valid if
/// there is no `Exclude` container submitted, and it's not empty"), and the two
/// containers scope the same configuration in opposite directions — so a
/// document submitting both scopes it two ways and names no one scope.
///
/// The message is fixed text, naming neither container's contents: it is what
/// the service answers for every document that carries the pair.
pub fn invalid_include_exclude_containers<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidIncludeExcludeContainers",
        400,
        "Only one of Include or Exclude should be present.".to_string(),
    )
}

/// A family of S3 Storage Lens metrics that `AccountLevel` and `BucketLevel`
/// BOTH declare a container for.
///
/// The two containers declare the same five metric members
/// (`smithy_query(query="shape",shape="AccountLevel,BucketLevel")`), and
/// `AccountLevel` documents the rule that spans them: "You must enable Storage
/// Lens metrics consistently at both the account level and bucket level, or
/// your request will fail." This is the one place that set and the words each
/// member is refused in are written down, so the rule is stated once for every
/// family rather than once per family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageLensMetrics {
    Activity,
    AdvancedCostOptimization,
    AdvancedDataProtection,
    DetailedStatusCodes,
    AdvancedPerformance,
}

impl StorageLensMetrics {
    /// Every family the rule spans, in the order `AccountLevel` declares them.
    pub const ALL: [Self; 5] = [
        Self::Activity,
        Self::AdvancedCostOptimization,
        Self::AdvancedDataProtection,
        Self::DetailedStatusCodes,
        Self::AdvancedPerformance,
    ];

    /// The member both containers declare this family under.
    pub fn member(self) -> &'static str {
        match self {
            Self::Activity => "ActivityMetrics",
            Self::AdvancedCostOptimization => "AdvancedCostOptimizationMetrics",
            Self::AdvancedDataProtection => "AdvancedDataProtectionMetrics",
            Self::DetailedStatusCodes => "DetailedStatusCodesMetrics",
            Self::AdvancedPerformance => "AdvancedPerformanceMetrics",
        }
    }

    /// The wire `<Code>` of [`missing_bucket_level_metrics`] for this family:
    /// what is missing, and the member it is missing from.
    pub fn missing_bucket_level_code(self) -> &'static str {
        match self {
            Self::Activity => "MissingBucketLevelActivityMetrics",
            Self::AdvancedCostOptimization => "MissingBucketLevelAdvancedCostOptimizationMetrics",
            Self::AdvancedDataProtection => "MissingBucketLevelAdvancedDataProtectionMetrics",
            Self::DetailedStatusCodes => "MissingBucketLevelDetailedStatusCodesMetrics",
            Self::AdvancedPerformance => "MissingBucketLevelAdvancedPerformanceMetrics",
        }
    }

    /// The words a refusal names this family in: the member read as prose, as
    /// a sentence opens.
    pub fn words(self) -> &'static str {
        match self {
            Self::Activity => "Activity metrics",
            Self::AdvancedCostOptimization => "Advanced cost optimization metrics",
            Self::AdvancedDataProtection => "Advanced data protection metrics",
            Self::DetailedStatusCodes => "Detailed status codes metrics",
            Self::AdvancedPerformance => "Advanced performance metrics",
        }
    }
}

/// `MissingBucketLevel<Metrics>` (HTTP 400): the submitted document enables a
/// metric family for the account and does not enable it for the buckets under
/// it.
///
/// The rule is the one `AccountLevel` documents — metrics are enabled
/// "consistently at both the account level and bucket level, or your request
/// will fail" — so the same refusal is answered for every family both
/// containers declare ([`StorageLensMetrics`]), each naming its own family in
/// the code and in the sentence: what must be enabled where, opening with the
/// family and closing with the level it was enabled at.
pub fn missing_bucket_level_metrics<E>(
    family: StorageLensMetrics,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        family.missing_bucket_level_code(),
        400,
        format!(
            "{words} must be enabled at bucket level when {spoken} is enabled at account level",
            words = family.words(),
            spoken = family.words().to_lowercase(),
        ),
    )
}

/// `MalformedXML` (HTTP 400): the document a Storage Lens configuration
/// request carried is not one this service can take — it is not a document the
/// protocol can read, or it breaks a rule the shape it is read into states.
///
/// Answered by `crate::service::envelope` for a request that surface's
/// boundary refused over the document it carried, so a caller sees one refusal
/// for a document the service will not take whatever part of it was wrong.
///
/// The wire `<Code>` a client branches on.
pub const MALFORMED_XML_CODE: &str = "MalformedXML";

/// The HTTP status of a [`MALFORMED_XML_CODE`] refusal.
pub const MALFORMED_XML_STATUS: u16 = 400;

/// The message of a [`MALFORMED_XML_CODE`] refusal: fixed text, naming neither
/// the member nor the value that caused it.
pub const MALFORMED_XML_MESSAGE: &str =
    "The XML you provided was not well-formed or did not validate against our published schema";

/// A [`MALFORMED_XML_CODE`] refusal, for a handler that READ the document and
/// will not take it.
///
/// The same refusal the request boundary's is restated as
/// (`crate::service::envelope`): a document the Storage Lens configuration
/// surface will not take is one refusal whatever part of it the service could
/// not take — a body it could not parse, a value breaking a rule its shape
/// states, or a document whose own `Id` is not the configuration the URI
/// addresses (`PutStorageLensConfigurationRequest$ConfigId` and
/// `StorageLensConfiguration$Id` are both documented as the id OF the
/// configuration, so a document naming another one does not describe the
/// configuration the request addresses).
///
/// A document the service DID take and whose values break a rule it documents
/// elsewhere is a different answer, under that rule's own code — see
/// [`invalid_include_exclude_containers`] and
/// [`missing_bucket_level_metrics`].
pub fn malformed_storage_lens_document<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        MALFORMED_XML_CODE,
        MALFORMED_XML_STATUS,
        MALFORMED_XML_MESSAGE,
    )
}

/// `TooManyTagsException` (HTTP 400): the request carries more than
/// [`MAX_STORAGE_LENS_TAGS`] tags, the maximum the model documents for a
/// Storage Lens configuration's tag set.
///
/// The code and status are this service's own modelled `TooManyTagsException`
/// ("Amazon S3 throws this exception if you have too many tags in your tag
/// set", a 400 client error), which these operations' models do not list.
pub fn too_many_storage_lens_tags<E>(submitted: usize) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyTagsException",
        400,
        format!(
            "A Storage Lens configuration carries at most {MAX_STORAGE_LENS_TAGS} tags; \
             the request carries {submitted}."
        ),
    )
}

/// `TooManyConfigurations` (HTTP 400): the store already holds
/// [`STORAGE_LENS_CONFIGURATION_QUOTA`] Storage Lens configurations, so another
/// record would grow it without end. Replacing an existing configuration is
/// never refused by this bound: it adds no record.
pub fn too_many_storage_lens_configurations<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyConfigurations",
        400,
        format!(
            "This emulator holds the maximum of {STORAGE_LENS_CONFIGURATION_QUOTA} \
             Storage Lens configurations."
        ),
    )
}

/// `NoSuchConfiguration` (HTTP 404): no S3 Storage Lens group of that name is
/// owned by the account the request names.
///
/// S3 Control declares no error for the Storage Lens group operations, so — as
/// for every refusal above — it is carried in the S3-style XML
/// `<Code>`/`<Message>` envelope. The code and the fixed sentence are the ones
/// the Storage Lens surface answers for a resource that is not there, the SAME
/// pair [`no_such_storage_lens_configuration`] answers one resource over: a
/// recorded read of a group that had just been deleted is answered
/// `NoSuchConfiguration`, not a code naming the group kind, so the two reads do
/// not disagree and a client matching on the code sees one for both.
pub fn no_such_storage_lens_group<E>() -> crate::scaffolded::Scaffolded<E> {
    no_such_storage_lens_configuration()
}

/// `InvalidRequestException` (HTTP 400): the account already owns an S3 Storage
/// Lens group of that name, so creating it again would be a second resource
/// under one name — which is what `UpdateStorageLensGroup` is for.
///
/// The code and status are this service's own modelled `InvalidRequestException`
/// (`com.amazonaws.s3control#InvalidRequestException`, a 400 client error),
/// which this operation's model does not list; the group operations model no
/// error of their own.
pub fn storage_lens_group_already_exists<E>(name: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequestException",
        400,
        format!(
            "A Storage Lens group named {name} already exists in this account; \
             change it with UpdateStorageLensGroup."
        ),
    )
}

/// `InvalidRequestException` (HTTP 400): an `UpdateStorageLensGroup` whose URI
/// names one Storage Lens group and whose document names another.
///
/// Both members are documented as the name OF the Storage Lens group
/// (`UpdateStorageLensGroupRequest$Name`, `StorageLensGroup$Name`), and the name
/// is the group's identity — it is what the store keys a record by and what the
/// read-only `StorageLensGroupArn` is built from — so a request carrying two
/// different names names no one group, and an update is not a rename. The code
/// and status are this service's own modelled `InvalidRequestException`
/// (`com.amazonaws.s3control#InvalidRequestException`, a 400 client error),
/// which this operation's model does not list.
pub fn storage_lens_group_name_mismatch<E>(
    name: &str,
    document_name: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequestException",
        400,
        format!(
            "Name is {name} but the submitted StorageLensGroup's Name is {document_name}; \
             they must name the same Storage Lens group."
        ),
    )
}

/// `InvalidRequest` (HTTP 400): the account the request names owns no job of
/// that id.
///
/// The model binds `NotFoundException` to `DescribeJob`, but the recorded
/// behaviour of this service for a well-formed id that names no job is the
/// S3-style XML envelope with code `InvalidRequest` and HTTP 400, so that is
/// what the job-id surface answers — uniformly, so `DescribeJob`, the job
/// priority and status updates and the job tagging operations cannot drift on
/// it.
///
/// The message is FIXED text, [`JOB_NOT_FOUND_MESSAGE`]: the id that was looked
/// up is the caller's own, carried by the request it sent, so it is not
/// interpolated into the sentence — which is why this takes no job id at all
/// and no caller can put one back. What names THIS answer is the correlation
/// pair the envelope carries beside the message
/// ([`new_error_correlation`]).
pub fn job_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure("InvalidRequest", 400, JOB_NOT_FOUND_MESSAGE)
}

/// The fixed sentence the Batch Operations job surface answers for a job id the
/// account owns no job for, as this service words it.
pub const JOB_NOT_FOUND_MESSAGE: &str = "Job not found";

/// `InvalidRequest` (HTTP 400): the job is in a status that admits no move to
/// the one `UpdateJobStatus` asks for — a confirmation of a job that awaits
/// none, or a cancellation of a job that has ended or begun ending.
///
/// The model binds `JobStatusException` to `UpdateJobStatus`, but the recorded
/// behaviour of this service for a status it will not move is the same S3-style
/// XML envelope the rest of the job surface answers a well-formed request it
/// cannot carry out with (see [`job_not_found`]): code `InvalidRequest` with
/// HTTP 400 and [`JOB_STATUS_FORBIDDEN_MESSAGE`] — fixed text, so neither the
/// status the job is in nor the one the request asked for is interpolated into
/// it. Which job was addressed and which status it asked for are the caller's
/// own, carried by the request it sent, and the status the job IS in is what
/// `DescribeJob` reports.
pub fn job_status_not_updatable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        JOB_STATUS_FORBIDDEN_MESSAGE,
    )
}

/// The fixed sentence the Batch Operations job surface answers a status move it
/// will not make with, as this service words it.
pub const JOB_STATUS_FORBIDDEN_MESSAGE: &str = "Requested job status forbidden";

/// `InvalidRequest` (HTTP 400): a `CreateJob` names no source for the object
/// list its action runs over — "jobs must be created with either a manifest
/// file or a ManifestGenerator, but not both"
/// (`CreateJobRequest$ManifestGenerator`), and neither member is modelled
/// `@required`, so the boundary validator cannot refuse this.
///
/// The message is [`JOB_SOURCE_REQUIRED_MESSAGE`], the fixed sentence this
/// service answers that class with: it names the two members the model
/// declares, and nothing the request carried.
pub fn job_source_required<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        JOB_SOURCE_REQUIRED_MESSAGE,
    )
}

/// The fixed sentence the Batch Operations job surface answers a create that
/// names neither of the two members a job's object list can come from with, as
/// this service words it.
pub const JOB_SOURCE_REQUIRED_MESSAGE: &str =
    "Either a Manifest or a ManifestGenerator must be provided.";

/// `InvalidRequest` (HTTP 400): a `CreateJob` names BOTH members a job's object
/// list can come from, so it names two object lists for one job — "but not
/// both" (`CreateJobRequest$ManifestGenerator`).
///
/// The other half of the one rule [`job_source_required`] answers, so it is
/// answered in the same words, which this service closes with the clause that
/// states the half that was broken: [`JOB_SOURCE_AMBIGUOUS_MESSAGE`]. Fixed
/// text — neither the manifest nor the generator the request carried appears in
/// it.
pub fn job_source_ambiguous<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        JOB_SOURCE_AMBIGUOUS_MESSAGE,
    )
}

/// The fixed sentence the Batch Operations job surface answers a create that
/// names both of the two members a job's object list can come from with, as
/// this service words it.
pub const JOB_SOURCE_AMBIGUOUS_MESSAGE: &str =
    "Either a Manifest or a ManifestGenerator must be provided, but not both";

/// `InvalidRequest` (HTTP 400): a `CreateJob` reuses a `ClientRequestToken`
/// that already created a job, for a DIFFERENT job — "an idempotency token to
/// ensure that you don't accidentally submit the same request twice"
/// (`CreateJobRequest$ClientRequestToken`) names one job, so a second set of
/// parameters under it asks for a job the token cannot name.
///
/// The model binds `IdempotencyException` to `CreateJob`, but the recorded
/// behaviour of this service for a token collision is the envelope the rest of
/// the job surface answers a request it will not carry out with (see
/// [`job_not_found`]): code `InvalidRequest`, HTTP 400 and
/// [`DUPLICATE_CLIENT_REQUEST_TOKEN_MESSAGE`] — fixed text, so neither the
/// token nor the id of the job it already created is interpolated into it.
pub fn duplicate_client_request_token<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        DUPLICATE_CLIENT_REQUEST_TOKEN_MESSAGE,
    )
}

/// The fixed sentence the Batch Operations job surface answers a reused
/// `ClientRequestToken` with, as this service words it.
pub const DUPLICATE_CLIENT_REQUEST_TOKEN_MESSAGE: &str = "Duplicate ClientRequestToken";

/// `NoSuchMultiRegionAccessPoint` (HTTP 404): no Multi-Region Access Point of
/// that name is owned by the account the request names.
///
/// S3 Control declares no error for these operations, so — exactly as for the
/// access point refusals above — the refusal is carried in the S3-style XML
/// `<Code>`/`<Message>` envelope, with the `NoSuch<Resource>` code and the 404
/// this service answers a missing named resource with.
///
/// The message is FIXED text: what was looked up travels in the members BESIDE
/// it — the name the request named and the account it named it under — which is
/// where a client reads the offending identity from
/// ([`MULTI_REGION_ACCESS_POINT_NAME_MEMBER`], [`ACCOUNT_ID_MEMBER`]).
pub fn no_such_multi_region_access_point<E>(
    name: &str,
    account_id: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure_naming(
        "NoSuchMultiRegionAccessPoint",
        404,
        "The specified Multi-Region Access Point does not exist.",
        vec![
            crate::scaffolded::ErrorDetail::new(&MULTI_REGION_ACCESS_POINT_NAME_MEMBER, name),
            crate::scaffolded::ErrorDetail::new(&ACCOUNT_ID_MEMBER, account_id),
        ],
    )
}

/// `InvalidRequest` (HTTP 400): the request names a Multi-Region Access Point
/// this service cannot resolve, so there is nothing for it to act on.
///
/// The routing operations take a `MultiRegionAccessPointId` — the
/// service-assigned ALIAS of an access point, not its name — and a value that
/// names none is a request this service will not carry out rather than a
/// resource that is missing — which is [`request_invalid`], the one refusal
/// this service answers a request it will not carry out with.
pub fn invalid_multi_region_access_point_request<E>() -> crate::scaffolded::Scaffolded<E> {
    request_invalid()
}

/// `InvalidRequest` (HTTP 400): a submitted route update names no route of the
/// Multi-Region Access Point's routing configuration — because it identifies
/// neither a bucket nor a Region ("either the `Bucket` or the `Region` value
/// must be provided"), because the access point associates no such bucket or
/// Region, or because it names a bucket together with a Region the bucket is
/// not in ("if both are provided, the bucket must be in the specified
/// Region" — `MultiRegionAccessPointRoute`).
///
/// An update that resolves to no route is a request this service will not carry
/// out, which is [`request_invalid`].
pub fn invalid_route_update<E>() -> crate::scaffolded::Scaffolded<E> {
    request_invalid()
}

/// `InvalidRequest` (HTTP 400): the submitted routing configuration would leave
/// every associated Region passive — "at least one Region must be active at all
/// times", and "if all Regions in the routing configuration are designated as
/// passive, you'll receive an `InvalidRequest` error"
/// (`SubmitMultiRegionAccessPointRoutes`,
/// `MultiRegionAccessPointRoute$TrafficDialPercentage`).
pub fn no_active_route<E>() -> crate::scaffolded::Scaffolded<E> {
    request_invalid()
}

/// `InvalidRequest` (HTTP 400): the submit carries more route updates than this
/// emulator accepts in one call ([`MAX_ROUTE_UPDATES`]), so carrying it out
/// would spend this process's time on one request.
///
/// The code and status are the ones this service answers a submit it will not
/// carry out with ([`request_invalid`]); the message states the bound, which is
/// this emulator's own rather than anything the model declares.
pub fn too_many_route_updates<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        format!(
            "This emulator accepts at most {MAX_ROUTE_UPDATES} route updates in one \
             request."
        ),
    )
}

/// The `ErrorDetails` the LOSER of a Multi-Region Access Point create race
/// settles with: the account already owns a Multi-Region Access Point of that
/// name — including one still being created or deleted — so creating it again
/// would be a second resource under one name.
///
/// `CreateMultiRegionAccessPoint` is asynchronous and binds no errors at all:
/// it answers a request token, and uniqueness is decided by the request the
/// token names rather than by the submission. So this conflict is an OUTCOME of
/// that request ([`AsyncRequestRecord::failure`]), reported to the caller that
/// describes the token, and the code follows the one this service answers for
/// the same conflict one resource down (`AccessPointAlreadyOwnedByYou`).
pub fn multi_region_access_point_already_owned_by_you(name: &str) -> AsyncErrorDetails {
    AsyncErrorDetails {
        code: Some("MultiRegionAccessPointAlreadyOwnedByYou".to_string()),
        message: Some(format!(
            "A Multi-Region Access Point with the same name already exists: {name}"
        )),
        resource: Some(name.to_string()),
        request_id: Some(async_failure_request_id()),
    }
}

/// The `RequestId` the `ErrorDetails` of an asynchronous outcome names the
/// request that failed by.
///
/// The same identifier an error RESPONSE of this service names its request with
/// ([`new_error_correlation`]), minted once — when the outcome is decided —
/// rather than per read, so every `DescribeMultiRegionAccessPointOperation` of
/// one token reports the one id. The VALUE is per-request and nothing may
/// depend on a particular one; what it must do is name ONE request.
fn async_failure_request_id() -> String {
    new_error_correlation().0
}

/// `InvalidRequest` (HTTP 400): the create named no bucket at all, so there is
/// nothing for the Multi-Region Access Point to associate.
///
/// `CreateMultiRegionAccessPointInput$Regions` is `@required`, so the request
/// boundary finds the member's ABSENCE; a member present and EMPTY is a value
/// the model admits and this service will not carry out, which is this one
/// refusal. The message is the fixed sentence S3 Control answers it with.
pub fn empty_region_list<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        REQUEST_INVALID_CODE,
        REQUEST_INVALID_STATUS,
        EMPTY_REGION_LIST_MESSAGE,
    )
}

/// The message of [`empty_region_list`]: fixed text, naming neither the member
/// nor a bound.
pub const EMPTY_REGION_LIST_MESSAGE: &str = "List of regions cannot be empty";

/// The message S3 Control refuses a Multi-Region Access Point NAME it will not
/// accept with, under [`REQUEST_INVALID_CODE`] and [`REQUEST_INVALID_STATUS`].
///
/// Fixed text, naming neither the name, the member nor the rule: the name a
/// Multi-Region Access Point may be given is constrained by the modelled
/// `MultiRegionAccessPointName` (len 0..50, pattern
/// `^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$`), and every value outside it is the same
/// refusal of the same class whatever the value — which is why the value the
/// caller sent is not restated back to it.
pub const INVALID_MULTI_REGION_ACCESS_POINT_NAME_MESSAGE: &str =
    "The provided Amazon S3 MultiRegionAccessPoint name is invalid";

/// The `ErrorDetails` a Multi-Region Access Point create settles with when its
/// `Regions` list names one bucket more than once.
///
/// A Multi-Region Access Point associates ONE bucket per Region, so a list that
/// names a bucket twice asks for a configuration no access point can have. The
/// create binds no errors at all, so this is an OUTCOME of the request the
/// token names ([`AsyncRequestRecord::failure`]), under the code this service
/// answers a request it will not carry out with ([`REQUEST_INVALID_CODE`]).
///
/// `Resource` names the Multi-Region Access Point the request was about, as
/// every outcome of these requests does — the message states the class, naming
/// no bucket.
pub fn duplicate_associated_bucket(name: &str) -> AsyncErrorDetails {
    AsyncErrorDetails {
        code: Some(REQUEST_INVALID_CODE.to_string()),
        message: Some(DUPLICATE_ASSOCIATED_BUCKET_MESSAGE.to_string()),
        resource: Some(name.to_string()),
        request_id: Some(async_failure_request_id()),
    }
}

/// The message of [`duplicate_associated_bucket`]: fixed text, naming no
/// bucket — it states what the caller has to change about the list it sent.
pub const DUPLICATE_ASSOCIATED_BUCKET_MESSAGE: &str =
    "Your request includes duplicate buckets. Select a new bucket in a different Region \
     for your Multi-Region Access Point.";

/// The `ErrorDetails` a Multi-Region Access Point create settles with when a
/// bucket it named is not held by the object store.
///
/// A Multi-Region Access Point IS its set of associated buckets, so a create
/// naming a bucket that does not exist associates nothing and cannot be carried
/// out — the same referential read `CreateAccessPoint` makes before it attaches
/// an access point to a bucket ([`bucket_regions::bucket_exists`]), reported
/// where this asynchronous call's one contract puts an outcome. The code and
/// the fixed message are the ones the S3 family answers for a bucket that is
/// not there, and `Resource` names the Multi-Region Access Point the request
/// was about.
pub fn associated_bucket_absent(name: &str) -> AsyncErrorDetails {
    AsyncErrorDetails {
        code: Some(NO_SUCH_BUCKET_CODE.to_string()),
        message: Some(NO_SUCH_BUCKET_MESSAGE.to_string()),
        resource: Some(name.to_string()),
        request_id: Some(async_failure_request_id()),
    }
}

/// The code the S3 family answers for a bucket the store does not hold.
pub const NO_SUCH_BUCKET_CODE: &str = "NoSuchBucket";

/// The message of [`associated_bucket_absent`]: the fixed sentence the S3
/// family answers a missing bucket with, naming no bucket.
pub const NO_SUCH_BUCKET_MESSAGE: &str = "The specified bucket does not exist.";

/// `TooManyMultiRegionAccessPoints` (HTTP 400): the store already holds
/// [`MULTI_REGION_ACCESS_POINT_QUOTA`] Multi-Region Access Points, so another
/// record would grow it without end. Released by
/// `DeleteMultiRegionAccessPoint`.
pub fn too_many_multi_region_access_points<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyMultiRegionAccessPoints",
        400,
        format!(
            "This emulator holds the maximum of {MULTI_REGION_ACCESS_POINT_QUOTA} \
             Multi-Region Access Points."
        ),
    )
}

/// `TooManyStorageLensGroups` (HTTP 400): the store already holds
/// [`STORAGE_LENS_GROUP_QUOTA`] Storage Lens groups across every account it
/// holds records for, so another record would grow it without end. Released by
/// `DeleteStorageLensGroup`.
///
/// The bound is the emulator's OWN and is store-wide — `AccountId` is a request
/// member constrained only to `^\d{12}$`, which bounds the shape of a key and
/// not how many of them a caller can name — so the code names the resource it
/// bounds, the `TooMany<Resource>` form this service already answers for access
/// points and Multi-Region Access Points.
pub fn too_many_storage_lens_groups<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "TooManyStorageLensGroups",
        400,
        format!(
            "This emulator holds the maximum of {STORAGE_LENS_GROUP_QUOTA} \
             Storage Lens groups."
        ),
    )
}

/// `NoSuchAsyncRequest` (HTTP 404): the token names no asynchronous request
/// this account made — because none was ever made under it, because it is not a
/// token this service mints at all, or because the request was made so long ago
/// that the store no longer retains it (see [`ASYNC_REQUESTS_RETAINED`]).
///
/// A request token is an OPAQUE identifier the service minted and handed its
/// caller, so a token is RESOLVED against the requests this service holds
/// rather than checked against a shape: whatever its text, a token that matches
/// no record names no request, which is what this one refusal says. The code
/// and the fixed message are the ones S3 Control answers it with.
pub fn no_such_async_request<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        NO_SUCH_ASYNC_REQUEST_CODE,
        NO_SUCH_ASYNC_REQUEST_STATUS,
        NO_SUCH_ASYNC_REQUEST_MESSAGE,
    )
}

/// The wire `<Code>` of [`no_such_async_request`], which a client branches on.
///
/// Named separately because the refusal is answered from two places: the
/// handler, which resolved the token against the store and found no request,
/// and `crate::service::envelope`, which answers it for a token the request
/// boundary would otherwise refuse on the modelled shape of the label — the
/// same refusal, so it carries the same code, status and message.
pub const NO_SUCH_ASYNC_REQUEST_CODE: &str = "NoSuchAsyncRequest";

/// The HTTP status of [`no_such_async_request`].
pub const NO_SUCH_ASYNC_REQUEST_STATUS: u16 = 404;

/// The message of [`no_such_async_request`]: fixed text, naming no token.
pub const NO_SUCH_ASYNC_REQUEST_MESSAGE: &str = "The specified request was not found.";

/// The `<Error>` member a refusal names the Multi-Region Access Point it could
/// not find in.
///
/// S3 Control's model declares no error shape for these operations, so this
/// member exists only on the wire: it carries the name the REQUEST named, which
/// is what a client reads the offending identity out of instead of re-parsing
/// the request it sent.
static MULTI_REGION_ACCESS_POINT_NAME_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3control#ErrorDetails$MultiRegionAccessPointName",
        "com.amazonaws.s3control",
        "MultiRegionAccessPointName",
    ),
    ShapeType::String,
    "MultiRegionAccessPointName",
    1,
)
.with_xml_name("MultiRegionAccessPointName");

/// The `<Error>` member a refusal names the account it was made for in. See
/// [`MULTI_REGION_ACCESS_POINT_NAME_MEMBER`].
static ACCOUNT_ID_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3control#ErrorDetails$AccountId",
        "com.amazonaws.s3control",
        "AccountId",
    ),
    ShapeType::String,
    "AccountId",
    2,
)
.with_xml_name("AccountId");

/// The `<Error>` member a refusal names the access grant it was asked about in.
/// See [`MULTI_REGION_ACCESS_POINT_NAME_MEMBER`] for why a member of a refusal
/// this service's model declares no error shape for exists only on the wire.
static ACCESS_GRANT_ID_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3control#ErrorDetails$AccessGrantId",
        "com.amazonaws.s3control",
        "AccessGrantId",
    ),
    ShapeType::String,
    "AccessGrantId",
    1,
)
.with_xml_name("AccessGrantId");

/// The `<Error>` member a refusal names the registered location it was asked
/// about in. See [`ACCESS_GRANT_ID_MEMBER`].
static ACCESS_GRANTS_LOCATION_ID_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3control#ErrorDetails$AccessGrantsLocationId",
        "com.amazonaws.s3control",
        "AccessGrantsLocationId",
    ),
    ShapeType::String,
    "AccessGrantsLocationId",
    1,
)
.with_xml_name("AccessGrantsLocationId");

/// The `<Error>` member the referential-integrity refusal
/// ([`access_grants_location_not_empty`]) names the registered location in.
///
/// The SAME identity as [`ACCESS_GRANTS_LOCATION_ID_MEMBER`] under a different
/// element name: S3 Access Grants spells the element `AccessGrantsLocationID`
/// for this refusal and `AccessGrantsLocationId` for the missing-location one,
/// so the two spellings are what the wire carries and a client reads the id out
/// of whichever its refusal names.
static ACCESS_GRANTS_LOCATION_ID_UPPERCASE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.s3control#ErrorDetails$AccessGrantsLocationID",
        "com.amazonaws.s3control",
        "AccessGrantsLocationID",
    ),
    ShapeType::String,
    "AccessGrantsLocationID",
    1,
)
.with_xml_name("AccessGrantsLocationID");

/// How many hex characters the `RequestId` of an error envelope carries — the
/// length of the `x-amz-request-id` an S3-family service answers with.
const ERROR_REQUEST_ID_LEN: usize = 16;

/// How many hex characters the `HostId` of an error envelope carries, matching
/// the extended request id (`x-amz-id-2`) this emulator's framework renders for
/// an unwrapped S3 error: 32 bytes as lowercase hex.
const ERROR_HOST_ID_LEN: usize = 64;

/// The correlation pair ONE error response names the request it answers with:
/// the `RequestId` and `HostId` siblings of `<Error>`, which the matching
/// `x-amz-request-id` / `x-amz-id-2` headers mirror.
///
/// Both VALUES are per-response and neither is a requirement: nothing can
/// predict one and no caller may depend on a particular one. What they must do
/// is name ONE response — so each is drawn afresh, and two responses carry two
/// different pairs — in the shape an S3-family service renders them: the
/// request id as [`ERROR_REQUEST_ID_LEN`] upper-case hex characters, the host id
/// as [`ERROR_HOST_ID_LEN`] lower-case ones. They are identifiers for
/// correlation, never secrets: nothing authenticates with them.
///
/// A free function rather than a mint on the store: a refusal the request
/// boundary made never reaches a handler, so the envelope that answers it has
/// no state to reach.
pub fn new_error_correlation() -> (String, String) {
    use std::sync::atomic::{AtomicU64, Ordering};

    static ANSWERED: AtomicU64 = AtomicU64::new(0);

    let sequence = ANSWERED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let request_id: String = correlation_hex("request", sequence, nanos)
        .chars()
        .take(ERROR_REQUEST_ID_LEN)
        .flat_map(char::to_uppercase)
        .collect();
    let host_id: String = correlation_hex("host", sequence, nanos)
        .chars()
        .take(ERROR_HOST_ID_LEN)
        .collect();
    (request_id, host_id)
}

/// The hex digest one correlation id of [`new_error_correlation`] is rendered
/// from: the OS-seeded randomness a `RandomState` draws, the system clock in
/// nanoseconds, a counter so two responses in the same nanosecond cannot
/// collide, and `label` so the two ids of one response are independent of each
/// other.
fn correlation_hex(label: &str, sequence: u64, nanos: u128) -> String {
    use sha2::Digest;
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let mut hasher = sha2::Sha256::new();
    hasher.update(label.as_bytes());
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    hex::encode(hasher.finalize())
}

// The refusals the S3 Access Grants instance lifecycle shares.
//
// S3 Control's model declares no errors for these operations either, so each is
// built through `Scaffolded::service_failure`, which carries the wire `<Code>`
// and the HTTP status a client branches on.
// ---------------------------------------------------------------------------

/// `AccessGrantsInstanceNotExistsError` (HTTP 404): the account the request
/// names has no S3 Access Grants instance in the Region the request was signed
/// for, because none was created there (or it was deleted).
///
/// The message is FIXED text, with no variable part at all: an instance is a
/// Region-scoped resource of which an account has at most one and every instance
/// carries the same id ([`ACCESS_GRANTS_INSTANCE_ID`]), so there is no instance
/// identity to name — the account and the Region the request named are the
/// request's own, and a refusal that interpolated them would make a client match
/// on prose built from what it had just sent.
pub fn no_such_access_grants_instance<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessGrantsInstanceNotExistsError",
        404,
        NO_SUCH_ACCESS_GRANTS_INSTANCE_MESSAGE,
    )
}

/// The message of [`no_such_access_grants_instance`], which is also what the
/// operations that need an instance before they can act answer.
pub const NO_SUCH_ACCESS_GRANTS_INSTANCE_MESSAGE: &str = "Access Grants Instance does not exist";

/// `InvalidRequest` (HTTP 400): the account named has no S3 Access Grants
/// instance in this Region, so there is no instance to take an Identity Center
/// association off.
///
/// The SAME precondition as [`no_such_access_grants_instance`], answered the way
/// `DissociateAccessGrantsIdentityCenter` answers it: the code and status this
/// service gives a well-formed request it cannot carry out (see
/// [`untaggable_resource_arn`]), with the fixed sentence naming the instance the
/// request asked about — the request's own account and Region are not
/// interpolated, for the same reason they are not in the read's refusal.
pub fn no_access_grants_instance_to_dissociate<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequest",
        400,
        NO_ACCESS_GRANTS_INSTANCE_TO_DISSOCIATE_MESSAGE,
    )
}

/// The message of [`no_access_grants_instance_to_dissociate`].
pub const NO_ACCESS_GRANTS_INSTANCE_TO_DISSOCIATE_MESSAGE: &str =
    "The specified Access Grants Instance does not exist";

/// `AccessGrantsInstanceAlreadyExists` (HTTP 409): the account already has an S3
/// Access Grants instance in this Region, and `CreateAccessGrantsInstance`
/// documents one instance per Region per account — so creating it again would be
/// a second resource under one identity, not a replacement.
///
/// The code carries NO `Error` suffix, where the missing-instance refusal's does
/// ([`no_such_access_grants_instance`]): recorded behaviour gives the two
/// conditions those two codes, and a client branches on the code.
///
/// The message is FIXED text, for the same reason that refusal's is: an instance
/// is a Region-scoped singleton of which an account has at most one, so there is
/// no instance identity to name, and the account and the Region a sentence could
/// name are the request's own.
pub fn access_grants_instance_already_exists<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessGrantsInstanceAlreadyExists",
        409,
        ACCESS_GRANTS_INSTANCE_ALREADY_EXISTS_MESSAGE,
    )
}

/// The message of [`access_grants_instance_already_exists`].
pub const ACCESS_GRANTS_INSTANCE_ALREADY_EXISTS_MESSAGE: &str =
    "Access Grants Instance already exists";

/// `AccessDenied` (HTTP 403): the store already holds
/// [`ACCESS_GRANTS_INSTANCE_QUOTA`] instances, so recording one for a further
/// (account, Region) pair would grow this process's state without end. As with
/// [`public_access_block_store_full`], `AccessDenied` is the refusal a caller
/// gets when the service will not let the resource be created, and the message
/// says which bound was reached.
pub fn access_grants_instance_store_full<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessDenied",
        403,
        format!(
            "This emulator holds the maximum of {ACCESS_GRANTS_INSTANCE_QUOTA} S3 Access \
             Grants instances."
        ),
    )
}

/// `NoSuchAccessGrantsLocationError` (HTTP 404): the S3 Access Grants instance
/// of the account the request names, in the Region it was signed for, has no
/// location registered under that id — because none was ever registered under
/// it, or because the registration was deleted.
///
/// The message is the fixed sentence S3 Access Grants answers for a location id
/// it holds no registration for, whichever id was asked about: the message
/// states the CLASS of failure and the `AccessGrantsLocationId` member BESIDE it
/// states the instance — the id the request named — which is where a client
/// reads the offending identity from (see
/// [`no_such_multi_region_access_point`]). Carried in the S3-style XML envelope
/// because the model declares no error here.
pub fn no_such_access_grants_location<E>(location_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure_naming(
        "NoSuchAccessGrantsLocationError",
        404,
        "The specified access grants location does not exist",
        vec![crate::scaffolded::ErrorDetail::new(
            &ACCESS_GRANTS_LOCATION_ID_MEMBER,
            location_id,
        )],
    )
}

/// `AccessGrantsLocationNotEmptyError` (HTTP 400): grants of this instance still
/// reference the location, and `DeleteAccessGrantsLocation` documents the rule —
/// "You can only delete a location registration from an S3 Access Grants
/// instance if there are no grants associated with this location."
///
/// A REFERENTIAL-INTEGRITY refusal: nothing about the service's state will clear
/// it, so the caller has to send a different sequence of requests — delete the
/// grants first — which is the 400 this service answers a request it will not
/// carry out with, and not the 409 an identity conflict carries (see
/// [`access_grants_location_already_exists`], where retrying the create under
/// another identity is what resolves it). The message is fixed text, as the
/// missing-location refusal's is: what the caller must do about it is the same
/// whichever location and however many grants, and the id it is about travels in
/// the member beside it.
pub fn access_grants_location_not_empty<E>(location_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure_naming(
        "AccessGrantsLocationNotEmptyError",
        400,
        "Please delete access grants before deleting access grants location",
        vec![crate::scaffolded::ErrorDetail::new(
            &ACCESS_GRANTS_LOCATION_ID_UPPERCASE_MEMBER,
            location_id,
        )],
    )
}

/// `AccessGrantsInstanceNotEmptyError` (HTTP 400): locations are still
/// registered in the instance, or grants still held by it, and
/// `DeleteAccessGrantsInstance` documents the rule — "You must first delete the
/// access grants and locations before S3 Access Grants can delete the instance."
///
/// The referential-integrity refusal one resource up from
/// [`access_grants_location_not_empty`], so it carries that refusal's code shape
/// and status; it names no id because every instance carries the same one.
pub fn access_grants_instance_not_empty<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessGrantsInstanceNotEmptyError",
        400,
        "Please delete access grants and locations before deleting access grants instance",
    )
}

/// `AccessGrantsLocationAlreadyExistsError` (HTTP 409): the instance already has
/// a location registered for that scope, so registering it again would be two
/// locations over one S3 path rather than a replacement — the same conflict, and
/// so the same code shape and status, as
/// [`access_grants_instance_already_exists`].
pub fn access_grants_location_already_exists<E>(
    location_scope: &str,
) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessGrantsLocationAlreadyExistsError",
        409,
        format!("Access Grants Location already exists for this scope: {location_scope}"),
    )
}

/// `AccessDenied` (HTTP 403): the store already holds
/// [`ACCESS_GRANTS_LOCATION_QUOTA`] registered locations, so recording another
/// would grow this process's state without end. The code and status are the ones
/// [`access_grants_instance_store_full`] answers for the same kind of bound.
pub fn access_grants_location_store_full<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessDenied",
        403,
        format!(
            "This emulator holds the maximum of {ACCESS_GRANTS_LOCATION_QUOTA} S3 Access \
             Grants locations."
        ),
    )
}

/// `NoSuchAccessGrantError` (HTTP 404): the instance has no grant under that id,
/// because none was created under it or the grant was deleted. The code, status,
/// fixed message and the member naming the id the request asked about follow the
/// Access Grants surface's missing-resource refusal one resource up; see
/// [`no_such_access_grants_location`].
pub fn no_such_access_grant<E>(grant_id: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure_naming(
        "NoSuchAccessGrantError",
        404,
        "The specified access grant does not exist",
        vec![crate::scaffolded::ErrorDetail::new(
            &ACCESS_GRANT_ID_MEMBER,
            grant_id,
        )],
    )
}

/// `AccessGrantsInstanceResourcePolicyNotExists` (HTTP 404): the instance of the
/// account the request names, in the Region it was signed for, carries no
/// resource policy — none was put, or the one that was put has been deleted.
///
/// The policy is a subresource of one instance and has no id of its own, so the
/// message is the fixed sentence the service answers for its absence; the status
/// is the missing-resource 404 the rest of this surface answers (see
/// [`no_such_access_grants_location`]).
pub fn no_access_grants_instance_resource_policy<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessGrantsInstanceResourcePolicyNotExists",
        404,
        "Access Grants Instance Resource Policy does not exist",
    )
}

/// `AccessDenied` (HTTP 403): the store already holds [`ACCESS_GRANT_QUOTA`]
/// grants, so recording another would grow this process's state without end.
/// The code and status are the ones [`access_grants_instance_store_full`]
/// answers for the same kind of bound.
pub fn access_grant_store_full<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessDenied",
        403,
        format!("This emulator holds the maximum of {ACCESS_GRANT_QUOTA} S3 access grants."),
    )
}

/// `InvalidRequest` (HTTP 400): a grant was asked for over the whole default
/// location.
///
/// `CreateAccessGrant$AccessGrantsLocationId` states the rule: "If you are
/// passing the default location, you cannot create an access grant for the
/// entire default location. You must also specify a bucket or a bucket and
/// prefix in the Subprefix field." The code and status are the ones this
/// service answers a well-formed request it cannot carry out with (see
/// [`job_not_found`]).
pub fn grant_sub_prefix_required<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InvalidRequest",
        400,
        format!(
            "An access grant on the default location {DEFAULT_LOCATION_SCOPE} must narrow it: \
             specify a bucket, or a bucket and prefix, in S3SubPrefix."
        ),
    )
}

/// `AccessDenied` (HTTP 403): no grant of this instance covers the requested
/// target for the requested permission, so `GetDataAccess` vends nothing.
///
/// The message is built from the request — the permission asked for and the S3
/// path asked about — because those are what the caller has to change for the
/// call to succeed.
pub fn data_access_denied<E>(permission: &str, target: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessDenied",
        403,
        format!("You do not have {permission} permissions to the requested S3 Prefix: {target}"),
    )
}

/// `AccessDenied` (HTTP 403): `GetDataAccess` has already vended
/// [`DATA_ACCESS_CREDENTIAL_QUOTA`] credentials that are still live, so vending
/// another would grow this process's credential store without end. The code and
/// status are the ones [`access_grants_instance_store_full`] answers for the
/// same kind of bound.
pub fn data_access_credential_store_full<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "AccessDenied",
        403,
        format!(
            "This emulator has vended the maximum of {DATA_ACCESS_CREDENTIAL_QUOTA} live S3 \
             Access Grants credentials."
        ),
    )
}

/// `InternalError` (HTTP 500): [`service_state`] answered `None`, so this
/// process has no shared store to read. Never answered with a fresh engine — a
/// private store would accept writes nobody else can see.
pub fn state_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InternalError",
        500,
        "The emulator's shared state is not reachable from this request.",
    )
}

/// `InternalError` (HTTP 500): the credential store minted a session credential
/// without the secrets a session always carries, so there is no credential to
/// answer with. Never answered with material chosen here.
pub fn credential_without_secrets<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::service_failure(
        "InternalError",
        500,
        "A temporary credential was created without its secrets.",
    )
}
