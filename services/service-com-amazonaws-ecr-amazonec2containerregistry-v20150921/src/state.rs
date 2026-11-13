// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ecr-amazonec2containerregistry-v20150921 service.
//!
//! One registry (this emulator's account) holding repositories by name, each holding
//! its images by digest plus a tag index, beside the registry-wide settings a caller can
//! configure (its replication rules, its signing configuration) and the registry-level
//! resources ECR keys by prefix or principal (pull through cache rules, repository
//! creation templates and pull time update exclusions). Everything an operation answers
//! with is derived from here; nothing is re-synthesised from a request.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    EncryptionConfiguration, EncryptionConfigurationForRepositoryCreationTemplate, ImageDetail,
    ImageIdentifier, ImageReplicationStatus, ImageScanFinding, ImageScanFindings, ImageScanStatus,
    ImageScanningConfiguration, ImageTagMutabilityExclusionFilter,
    LifecyclePolicyPreviewResult, LifecyclePolicyPreviewSummary, LifecyclePolicyRuleAction,
    PullThroughCacheRule, ReplicationConfiguration, ReplicationRule, Repository,
    RepositoryCreationTemplate, RepositoryScanningConfiguration, SigningConfiguration, Tag,
    TransitioningImageTotalCount, IMAGE_ACTION_TYPE_VALUES,
    LIFECYCLE_POLICY_TARGET_STORAGE_CLASS_VALUES, TAG_STATUS_VALUES,
};

/// `CreateRepository`'s documented default: "If this parameter is omitted, the default
/// setting of MUTABLE will be used which will allow image tags to be overwritten."
pub const DEFAULT_IMAGE_TAG_MUTABILITY: &str = "MUTABLE";

/// The default `encryptionConfiguration.encryptionType`: ECR encrypts repository
/// contents with `AES256` unless the caller asks for a KMS key.
pub const DEFAULT_ENCRYPTION_TYPE: &str = S3_MANAGED_ENCRYPTION_TYPE;

/// The encryption type that encrypts with an Amazon S3-managed key, and therefore the
/// one that takes no `kmsKey` of the caller's: the `KMS` types are the ones a key
/// belongs to.
pub const S3_MANAGED_ENCRYPTION_TYPE: &str = "AES256";

/// The ceiling on how many repositories this store holds, how many images one
/// repository holds, and how many tags point into one repository: the three caps that
/// bound caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process and a
/// create or a push is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// `DescribeRepositories` and `DescribeImages` must still answer what `CreateRepository`
/// and `PutImage` stored. The two operations that grow the store refuse past these caps
/// with the `LimitExceededException` both of them model ("the operation did not succeed
/// because it would have exceeded a service limit for your account") and no read can
/// return. The image cap is the one that matters most: `imageManifest` is modelled
/// `@length(1..4194304)`, so each image carries up to 4 MiB of caller-supplied text.
/// The numbers are the emulator's own bound, not a published AWS default — AWS
/// publishes the repository and image quotas as adjustable, so no single value is the
/// contract — and they are set high enough that no ordinary workflow meets one.
pub const MAX_REPOSITORIES: usize = 1_000;

/// See [`MAX_REPOSITORIES`].
pub const MAX_IMAGES_PER_REPOSITORY: usize = 1_000;

/// See [`MAX_REPOSITORIES`]. This one bounds `tag_index`, which grows by one entry per
/// push that introduces a tag the repository did not carry.
pub const MAX_TAGS_PER_REPOSITORY: usize = 10_000;

/// See [`MAX_REPOSITORIES`]: the same reasoning for the pull through cache rule table,
/// which `CreatePullThroughCacheRule` grows by one entry per rule. The number is this
/// emulator's own bound rather than a published AWS default (AWS publishes the pull
/// through cache rule quota as adjustable), and a create past it is refused with the
/// `LimitExceededException` the operation models.
pub const MAX_PULL_THROUGH_CACHE_RULES: usize = 1_000;

/// See [`MAX_REPOSITORIES`]. This one bounds `creation_templates`, which grows by one
/// entry per `CreateRepositoryCreationTemplate` that introduces a new namespace prefix;
/// past it the create is refused with the `LimitExceededException` that operation models.
pub const MAX_REPOSITORY_CREATION_TEMPLATES: usize = 1_000;

/// How many pull time update exclusions one registry holds. Unlike the caps above this
/// is NOT the emulator's own number: the model states it, because
/// `ListPullTimeUpdateExclusions`'s `pullTimeUpdateExclusions` member targets
/// `PullTimeUpdateExclusionList`, a list declared `@length(min: 0, max: 100)` — read with
/// `smithy_query(query="shape",shape="PullTimeUpdateExclusionList")`. A collection whose
/// own read side cannot report more than that many entries cannot hold more than that
/// many, so `pull_time_update_exclusions` is bounded here at the modelled maximum and a
/// `RegisterPullTimeUpdateExclusion` that would push it past the bound is refused with
/// the `LimitExceededException` that operation models, storing nothing.
pub const MAX_PULL_TIME_UPDATE_EXCLUSIONS: usize = 100;

/// The `ImageStatus` of an image in Amazon ECR standard storage: what a push stores,
/// and what an image restored out of archive is back in. Which status an image has is
/// per-image state ([`ImageRecord::status`]) that `UpdateImageStorageClass` moves, so a
/// read reports the stored one rather than this constant.
pub const ACTIVE_IMAGE_STATUS: &str = "ACTIVE";

/// The `ImageStatus` of an image whose storage class is Amazon ECR archive:
/// `UpdateImageStorageClass` with `targetStorageClass` `ARCHIVE` is the only thing that
/// puts an image in it, and it is the signal that a pull needs a restore first.
pub const ARCHIVED_IMAGE_STATUS: &str = "ARCHIVED";

/// The `ImageStatus` of an image being restored out of Amazon ECR archive: the status a
/// `targetStorageClass` `STANDARD` transition puts it in until the restore the clock owes
/// falls due ([`IMAGE_RESTORE_DELAY`]).
pub const ACTIVATING_IMAGE_STATUS: &str = "ACTIVATING";

/// How long after a restore is requested the `ACTIVATING` -> `ACTIVE` transition falls
/// due.
///
/// Restoring an archived image is not instantaneous — the status enum carries
/// `ACTIVATING` for exactly that reason — and AWS publishes no duration for it, so the
/// span is this emulator's own: short enough that no workflow waits on it, and never
/// zero, so a caller reading the image straight after the call still sees `ACTIVATING`.
/// The instant the transition falls due is STORED ([`ImageRecord::activate_due_at`]) and
/// materialised by whoever next looks at the image; nothing sleeps for it.
pub const IMAGE_RESTORE_DELAY: Duration = Duration::from_millis(400);

/// The `LifecyclePolicyStorageClass` of an archived image — the storage class that goes
/// with [`ARCHIVED_IMAGE_STATUS`], as a lifecycle policy preview spells it.
pub const ARCHIVE_STORAGE_CLASS: &str = "ARCHIVE";

/// The `ReplicationStatus` of a copy to one destination that is still being made.
pub const REPLICATION_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `ReplicationStatus` of a copy to one destination the clock has made due.
pub const REPLICATION_STATUS_COMPLETE: &str = "COMPLETE";

/// How long after the push that starts it the clock owes one destination's copy of an
/// image its `IN_PROGRESS` -> `COMPLETE` transition.
///
/// AWS publishes no duration for a replication, so this is a CHOSEN value inside the
/// 250–750 ms band this project fixes for a clock-owed transition. Like
/// [`IMAGE_RESTORE_DELAY`] it is never zero, so a caller reading the status straight after
/// the push still meets `IN_PROGRESS`, and it is stored as a due instant and never slept
/// for: the completion is owed whether or not anybody calls again.
pub const IMAGE_REPLICATION_DELAY: Duration = Duration::from_millis(400);

/// The only `RepositoryFilterType` the API defines: "The repository filter type. The only
/// supported value is `PREFIX_MATCH`, which is a repository name prefix specified with the
/// `filter` parameter." (`RepositoryFilter$filterType`).
pub const PREFIX_MATCH_FILTER_TYPE: &str = "PREFIX_MATCH";

/// See [`MAX_REPOSITORIES`]: the same reasoning for the replication records ONE stored
/// image carries.
///
/// A replication configuration may hold 25 rules of 100 destinations each (`@length` on
/// `ReplicationRuleList` and `ReplicationDestinationList`), and a destination's registry id
/// is a caller-chosen account number, so without a bound one push could write thousands of
/// records and the registry's image cap would multiply them. A push records at most this
/// many destinations for an image; the number is the emulator's own bound, set at the
/// per-rule destination maximum the model declares, which is far past the handful of
/// Regions a real registry replicates to.
pub const MAX_IMAGE_REPLICATIONS: usize = 100;

/// The `ScanFrequency` of a repository whose scan on push is switched on: a push is what
/// triggers its scan.
pub const SCAN_ON_PUSH_FREQUENCY: &str = "SCAN_ON_PUSH";

/// The `ScanFrequency` of a repository that is scanned only when asked: "If scan on push
/// is not specified, then the MANUAL scan frequency is set by default"
/// (`RegistryScanningRule.scanFrequency`).
pub const MANUAL_SCAN_FREQUENCY: &str = "MANUAL";

/// Page size used when a paginated operation's `maxResults` is absent, and the ceiling
/// on the page a request may ask for: the ECR API reference documents 100 as the default
/// and as the maximum number of results per page for `DescribeRepositories`,
/// `ListImages` and `DescribeImages`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// "The maximum number of tags that can be applied to a repository is 50."
/// (`TooManyTagsException`'s own documentation).
pub const MAX_TAGS: usize = 50;

/// "Tag keys can have a maximum character length of 128 characters, and tag values can
/// have a maximum length of 256 characters." (`InvalidTagParameterException`).
pub const MAX_TAG_KEY_LEN: usize = 128;
/// See [`MAX_TAG_KEY_LEN`].
pub const MAX_TAG_VALUE_LEN: usize = 256;

/// The tag-key namespace AWS keeps for itself: "the `aws:` prefix is reserved for AWS
/// use [...] you can't edit or delete a tag key or value with this prefix" (AWS General
/// Reference, "Tagging AWS resources"). A request that tries to create or remove a key
/// in it is refused rather than stored, so no caller-supplied tag can ever impersonate
/// one AWS set.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The region an unsigned request is served in: a request that carries no SigV4
/// credential scope names no region, and ECR's ARNs and repository URIs are regional.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after the `StartLifecyclePolicyPreview` call that starts it the clock owes a
/// lifecycle policy preview its `IN_PROGRESS` -> `COMPLETE` transition.
///
/// AWS publishes no duration for a preview run, so this is a CHOSEN value inside the
/// 250–750 ms band this project fixes for a clock-owed transition. It is never zero, so a
/// caller that starts a second preview straight after the first still meets the
/// `IN_PROGRESS` state and the `LifecyclePolicyPreviewInProgressException` it answers
/// with. It is stored as a due instant and never slept for: the completion is owed
/// whether or not anybody calls again.
pub const LIFECYCLE_PREVIEW_DELAY: Duration = Duration::from_millis(400);

/// The `LifecyclePolicyPreviewStatus` a preview carries while it is still running.
///
/// The other two values the enum carries are not reachable here: a policy document that
/// does not parse is refused at the start call rather than stored as a `FAILED` preview,
/// and no published lifetime after which a stored preview goes `EXPIRED` exists to drive
/// that one.
pub const PREVIEW_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `LifecyclePolicyPreviewStatus` a preview carries once the clock has made its
/// completion due. See [`PREVIEW_STATUS_IN_PROGRESS`].
pub const PREVIEW_STATUS_COMPLETE: &str = "COMPLETE";

/// The `LifecyclePolicyStorageClass` of an image in standard storage — the storage class
/// that goes with [`ACTIVE_IMAGE_STATUS`], as a lifecycle policy preview spells it.
pub const STANDARD_STORAGE_CLASS: &str = "STANDARD";

/// How long after the `StartImageScan` call that starts it the clock owes a basic image
/// scan its `IN_PROGRESS` -> `COMPLETE` transition.
///
/// AWS publishes no duration for a basic scan run, so this is a CHOSEN value inside the
/// 250–750 ms band this project fixes for a clock-owed transition. It is never zero, so a
/// caller that reads the scan straight after starting it still meets the `IN_PROGRESS`
/// state. Like [`LIFECYCLE_PREVIEW_DELAY`] it is stored as a due instant and never slept
/// for: the completion is owed whether or not anybody calls again.
pub const IMAGE_SCAN_DELAY: Duration = Duration::from_millis(400);

/// The `ScanStatus` a basic scan carries while it is still running.
pub const SCAN_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `ScanStatus` a basic scan carries once the clock has made its completion due AND
/// the scanned image was one basic scanning could examine: the state in which its
/// findings are readable.
pub const SCAN_STATUS_COMPLETE: &str = "COMPLETE";

/// The terminal `ScanStatus` of a run that could not examine the image it was started
/// on. It is a TERMINAL status like [`SCAN_STATUS_COMPLETE`] — the scan ran and is over
/// — so a caller polling for an outcome stops here; what it reports is that no findings
/// were produced. See [`basic_scan_outcome`].
pub const SCAN_STATUS_FAILED: &str = "FAILED";

/// The `imageScanStatus.description` of a run that examined the image and finished:
/// stated in words beside [`SCAN_STATUS_COMPLETE`].
pub const SCAN_COMPLETE_DESCRIPTION: &str = "The scan was completed successfully.";

/// The `imageScanStatus.description` of a run whose image carries no operating system
/// basic scanning can examine — the wording Amazon ECR reports for that outcome, which
/// names the error and what about the image was not supported.
pub const UNSUPPORTED_IMAGE_SCAN_DESCRIPTION: &str =
    "UnsupportedImageError: The operating system and/or package manager are not supported.";

/// How often one image may be scanned: "A basic image scan can only be started once per
/// 24 hours on an individual image. This limit includes if an image was scanned on
/// initial push." (`StartImageScan`), so a start inside this span of the last scan of
/// that image is refused with the `LimitExceededException` the operation models.
pub const BASIC_SCAN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How many basic scans one registry may start per [`BASIC_SCAN_INTERVAL`]: "You can
/// start up to 100,000 basic scans per 24 hours." (`StartImageScan`). Unlike the caps
/// this store sets for itself, this one is AWS's own published number.
pub const MAX_BASIC_SCANS_PER_INTERVAL: usize = 100_000;

/// The `LifecyclePolicyStorageClass` of an image whose `ImageStatus` is `status`.
///
/// The two vocabularies describe one piece of state: an image is either in Amazon ECR
/// standard storage (`ACTIVE`) or in Amazon ECR archive, which is what `storageClass`
/// reports. An image being restored (`ACTIVATING`) is still in archive until the restore
/// is due.
pub fn storage_class_of_status(status: &str) -> &'static str {
    if status == ACTIVE_IMAGE_STATUS {
        STANDARD_STORAGE_CLASS
    } else {
        ARCHIVE_STORAGE_CLASS
    }
}

/// The `ImageActionType` an `expire` rule applies — the modelled spelling (from
/// `IMAGE_ACTION_TYPE_VALUES`) of what a policy document writes in lower case.
pub const EXPIRE_ACTION_TYPE: &str = "EXPIRE";

/// The `ImageActionType` a `transition` rule applies. See [`EXPIRE_ACTION_TYPE`].
pub const TRANSITION_ACTION_TYPE: &str = "TRANSITION";

/// The `TagStatus` a selection with no tag list asks for.
pub const ANY_TAG_STATUS: &str = "ANY";

/// The `TagStatus` a selection that names tags asks for — the one status the format's
/// tag-filter rules are stated about.
pub const TAGGED_TAG_STATUS: &str = "TAGGED";

/// The only `selection.countUnit` a lifecycle policy rule carries: a `sinceImagePushed`
/// count is a number of days.
pub const COUNT_UNIT_DAYS: &str = "days";

/// The `selection.countType` that counts IMAGES, keeping the newest `countNumber` of
/// them. The policy format names its count types in lower camel case and the Smithy
/// model has no enum for them, so the two live here, as the one source both the parser
/// and the refusal it words read.
pub const IMAGE_COUNT_MORE_THAN_COUNT_TYPE: &str = "imageCountMoreThan";

/// The `selection.countType` that counts TIME since an image was pushed.
pub const SINCE_IMAGE_PUSHED_COUNT_TYPE: &str = "sinceImagePushed";

/// Every `selection.countType` the lifecycle policy format defines.
pub const LIFECYCLE_COUNT_TYPES: [&str; 2] = [
    IMAGE_COUNT_MORE_THAN_COUNT_TYPE,
    SINCE_IMAGE_PUSHED_COUNT_TYPE,
];

/// The unit a `sinceImagePushed` rule counts in, in seconds.
const SECONDS_PER_DAY: u64 = 86_400;

/// One account setting of this registry: the setting's name, the values
/// `PutAccountSetting` accepts for it, and the value the registry reports for it before
/// anybody sets one.
#[derive(Debug, Clone, Copy)]
pub struct AccountSettingSpec {
    /// The name `GetAccountSetting`/`PutAccountSetting` address this setting by.
    pub name: &'static str,
    /// The values `PutAccountSetting` may set this setting to.
    pub settable_values: &'static [&'static str],
    /// The value this setting has until a `PutAccountSetting` changes it.
    pub default_value: &'static str,
}

/// Every account setting this registry serves.
///
/// The names come from the model's own documentation of
/// `PutAccountSettingRequest$name` ("The name of the account setting, such as
/// `BASIC_SCAN_TYPE_VERSION`, `REGISTRY_POLICY_SCOPE`, or `BLOB_MOUNTING`") and the
/// settable values from `PutAccountSettingRequest$value` ("Valid value for basic scan
/// type: `AWS_NATIVE`. Valid values for registry policy scope: `V2`. Valid values for
/// blob mounting: `ENABLED` or `DISABLED`"), read with
/// `smithy_query(query="doc",shape="PutAccountSettingRequest",member="name"|"value")`.
///
/// Each default is the value of a registry nobody has configured. None of the three is
/// stated by the model, which documents only what a caller may SET, so each is an
/// inference with its reasoning below — and this constant is the single place to correct
/// one if a recorded response ever contradicts it:
///
/// * `BASIC_SCAN_TYPE_VERSION` — `AWS_NATIVE` is the only version the model lets a
///   caller set, so it is also the one in force before any call.
/// * `REGISTRY_POLICY_SCOPE` — `V2` is the only scope the model lets a caller set, so
///   the move to it is one-way and the scope a registry holds beforehand is the other
///   one, `V1`.
/// * `BLOB_MOUNTING` — settable either way and with no documented default, so this one
///   is the weakest inference of the three: a feature nobody has asked for is taken to
///   be `DISABLED`.
pub const ACCOUNT_SETTINGS: &[AccountSettingSpec] = &[
    AccountSettingSpec {
        name: "BASIC_SCAN_TYPE_VERSION",
        settable_values: &["AWS_NATIVE"],
        default_value: "AWS_NATIVE",
    },
    AccountSettingSpec {
        name: "BLOB_MOUNTING",
        settable_values: &["ENABLED", "DISABLED"],
        default_value: "DISABLED",
    },
    AccountSettingSpec {
        name: "REGISTRY_POLICY_SCOPE",
        settable_values: &["V2"],
        default_value: "V1",
    },
];

/// The API parameter both account-setting operations address a setting by
/// (`smithy_query(query="operation",operation="GetAccountSetting,PutAccountSetting")`:
/// each input's `name` member).
const ACCOUNT_SETTING_NAME_PARAMETER: &str = "name";

/// The constraint a setting name this registry does not serve fails: a STABLE phrase
/// about the parameter, naming no value set.
///
/// `name` targets `AccountSettingName`, which
/// `smithy_query(query="shape",shape="AccountSettingName")` gives as a plain string of
/// length 1..64 with NO `@enum` trait. So the set of settings a registry serves is not a
/// constraint of the API, and a name outside it is not an enum-membership failure:
/// quoting [`ACCOUNT_SETTINGS`] back at the caller would assert a constraint the model
/// does not declare, publish this emulator's own allow-list, and drift from the real
/// service as settings are added.
pub const UNKNOWN_ACCOUNT_SETTING_CONSTRAINT: &str = "Invalid account setting name provided";

/// The setting `name` addresses, or [`Fault::InvalidParameter`] naming the parameter and
/// the constraint it failed.
pub fn account_setting_spec(name: &str) -> Result<&'static AccountSettingSpec, Fault> {
    ACCOUNT_SETTINGS
        .iter()
        .find(|spec| spec.name == name)
        .ok_or_else(|| {
            invalid_parameter_at(
                ACCOUNT_SETTING_NAME_PARAMETER,
                UNKNOWN_ACCOUNT_SETTING_CONSTRAINT,
            )
        })
}

/// The `LayerAvailability` of a blob the repository holds and can serve.
///
/// The third value the enum carries is not reachable from this store: `ARCHIVED` is a
/// statement about a LAYER's storage, and what this registry archives is an IMAGE
/// (`UpdateImageStorageClass` moves an image's storage class; the repository's blob
/// record, which is what availability is read from, is untouched by it).
pub const AVAILABLE_LAYER_AVAILABILITY: &str = "AVAILABLE";

/// The `LayerAvailability` of a layer the repository does not hold: the digest is a
/// content digest, so it addresses a layer, and that layer is simply not here — which is
/// what a push client asks about before uploading a blob.
pub const UNAVAILABLE_LAYER_AVAILABILITY: &str = "UNAVAILABLE";

/// The `LayerFailureCode` for a request entry that carries no digest at all, so it
/// addresses no layer to report on.
pub const MISSING_LAYER_DIGEST_FAILURE: &str = "MissingLayerDigest";

/// The `LayerFailureCode` for a requested digest that is not a content digest at all.
pub const INVALID_LAYER_DIGEST_FAILURE: &str = "InvalidLayerDigest";

/// A failure a shared state helper can report, mapped by each operation onto the
/// modelled error its own model declares.
#[derive(Debug, Clone)]
pub enum Fault {
    /// No repository of that name in this registry — `RepositoryNotFoundException`.
    RepositoryNotFound(String),
    /// The request is self-inconsistent or names another registry —
    /// `InvalidParameterException`.
    InvalidParameter(String),
    /// An `imageIds` entry matches no stored image — `ImageNotFoundException`.
    ImageNotFound(String),
    /// The repository exists but carries no lifecycle policy —
    /// `LifecyclePolicyNotFoundException`.
    LifecyclePolicyNotFound(String),
    /// A tag key or value is longer than a tag may be — `InvalidTagParameterException`.
    InvalidTagParameter(String),
    /// The resource would end up carrying more tags than one may hold —
    /// `TooManyTagsException`.
    TooManyTags(String),
}

impl Fault {
    /// The message this failure renders with.
    pub fn message(&self) -> &str {
        match self {
            Self::RepositoryNotFound(m)
            | Self::InvalidParameter(m)
            | Self::ImageNotFound(m)
            | Self::LifecyclePolicyNotFound(m)
            | Self::InvalidTagParameter(m)
            | Self::TooManyTags(m) => m,
        }
    }
}

/// One blob a repository holds: the digest it is addressed by, plus the size and media
/// type the manifest that references it declares for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerBlob {
    pub digest: String,
    pub size: Option<i64>,
    pub media_type: Option<String>,
    /// What a basic image scan identifies inside this blob's bytes, read ONCE when
    /// `CompleteLayerUpload` assembled and registered them ([`scan_layer_filesystem`]),
    /// so a later scan of an image built on this layer reads this record rather than
    /// walking the archive again.
    ///
    /// Empty ([`LayerFilesystem::default`]) for a blob whose bytes carry no filesystem
    /// this scan could read, and for a DESCRIPTOR read out of a manifest document: a
    /// descriptor states a digest, a size and a media type, and carries none of the
    /// blob's content.
    pub filesystem: LayerFilesystem,
    /// The blob's own bytes, exactly as `CompleteLayerUpload` assembled them from the
    /// parts the upload accepted: what a download of this layer serves, byte for byte,
    /// and what [`Self::digest`] addresses it by.
    ///
    /// A layer is content-addressed, so the bytes ARE the layer: a registry that reports
    /// a layer available and hands out a download URL for it has to still hold them.
    /// They count against [`MAX_REGISTRY_CALLER_BYTES`]
    /// ([`caller_bytes_held`](ComAmazonawsEcrAmazonec2containerregistryV20150921State::caller_bytes_held)),
    /// where they are the same bytes the completed upload held, so closing an upload
    /// moves a layer's bytes between two records rather than freeing the budget they sit
    /// under.
    ///
    /// Empty for a DESCRIPTOR read out of a manifest document, which carries a digest, a
    /// size and a media type and none of the content they describe.
    pub content: Vec<u8>,
}

/// How many levels of manifest list [`Repo::refresh_image_sizes`] resolves when it
/// recomputes the size of every stored image.
///
/// A list entry may name another list, so a list's size can rest on a chain of them; the
/// documents and the chain are both caller-supplied, so the resolution is bounded. Four
/// is well past anything a registry client builds (an index over image manifests is one
/// level).
pub const MAX_MANIFEST_LIST_DEPTH: usize = 4;

/// The most `manifests[*]` entries this service reads out of one manifest list.
///
/// A list's entries are caller-supplied and `imageManifest` is modelled
/// `@length(1..4194304)`, so one document can name tens of thousands of children; this
/// is the bound that, with [`MAX_MANIFEST_LIST_DEPTH`] and
/// [`MAX_IMAGES_PER_REPOSITORY`], caps the work of recomputing image sizes. AWS
/// publishes no limit on the entries of an index, and a real index carries one entry per
/// platform (a handful), so a document naming more children than this states no size
/// this service reports rather than being read in part.
pub const MAX_MANIFEST_LIST_ENTRIES: usize = 100;

/// The most blob descriptors this service reads out of ONE image manifest: its `layers`
/// (or `fsLayers`) entries plus its `config` descriptor.
///
/// The same reasoning as [`MAX_MANIFEST_LIST_ENTRIES`], for the other arm of the same
/// document: the descriptors are caller-supplied and `imageManifest` is modelled
/// `@length(1..4194304)`, so one document can name tens of thousands of blobs, and
/// reading them all would let a single `PutImage` body decide how much work this registry
/// does. AWS publishes no limit on the layers of an image (the Docker and OCI formats put
/// a real image at tens of layers), so a document naming more blobs than this states no
/// size this service reports rather than being read in part.
pub const MAX_MANIFEST_BLOB_ENTRIES: usize = 1_000;

/// What content a stored manifest document references.
///
/// A registry manifest is one of two things: an image manifest, which references the blobs
/// of one image, or a manifest list / OCI image index, which lists other manifests by
/// digest. Anything else references nothing this registry can size.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ManifestContent {
    /// An image manifest: the DISTINCT blobs it references — its layers and its image
    /// configuration — each carrying the size its descriptor states (`None` for a
    /// descriptor that states no usable size, which the repository's own record of the
    /// blob then supplies).
    Blobs(Vec<LayerBlob>),
    /// A manifest list or OCI image index: the distinct digests of the manifests it
    /// references.
    List(BTreeSet<String>),
    /// A document that references neither blobs nor manifests.
    #[default]
    Opaque,
}

/// Read a manifest document for the content it references.
///
/// An image manifest carries OCI content descriptors — a `mediaType`, a `digest` and a
/// `size` — under `config` (the image configuration blob) and `layers` (the filesystem
/// layers), and a V2 schema 1 manifest lists its layers as `fsLayers[].blobSum`; a
/// manifest list or index carries `manifests` instead, which are other images rather than
/// this one's content. What a manifest references is therefore exactly what
/// [`manifest_blobs`] reads, DEDUPLICATED by digest: one blob a manifest names twice (as
/// its configuration and as its only layer, which is what the recorded probe images do)
/// is one blob in the repository, and is counted once.
///
/// A descriptor's `size` is the byte count of the blob it points at, so a value that is
/// absent, not an integer, or below zero states no size at all: the document is
/// caller-supplied, and nothing it says may be read as a byte count it did not give. The
/// digest is kept beside it, because what the manifest does not state about a blob the
/// repository's own record of that blob does (see [`Repo::refresh_image_sizes`]).
///
/// A list's digests are collected as a SET, so naming one child a thousand times costs
/// what naming it once costs, and a document naming more than
/// [`MAX_MANIFEST_LIST_ENTRIES`] children is [`ManifestContent::Opaque`] rather than
/// read in part.
pub fn manifest_content(manifest: &str) -> ManifestContent {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return ManifestContent::Opaque;
    };
    if let Some(manifests) = document.get("manifests").and_then(|m| m.as_array()) {
        if manifests.len() > MAX_MANIFEST_LIST_ENTRIES {
            return ManifestContent::Opaque;
        }
        return ManifestContent::List(
            manifests
                .iter()
                .filter_map(|descriptor| descriptor.get("digest"))
                .filter_map(serde_json::Value::as_str)
                .map(ToString::to_string)
                .collect(),
        );
    }
    // A document that names none of the three arrays is not a manifest of anything: it
    // references no blob, which is different from an image manifest whose `layers` array
    // is empty (that one references no layer content, and says so).
    let declares_content = ["config", "layers", "fsLayers"]
        .iter()
        .any(|member| document.get(member).is_some());
    if !declares_content {
        return ManifestContent::Opaque;
    }
    let mut blobs = manifest_blobs(manifest);
    if blobs.len() > MAX_MANIFEST_BLOB_ENTRIES {
        return ManifestContent::Opaque;
    }
    // De-duplicated through a SET of the digests already kept, so one document naming a
    // blob many times costs its length rather than its length squared.
    let mut kept: BTreeSet<String> = BTreeSet::new();
    blobs.retain(|blob| kept.insert(blob.digest.clone()));
    ManifestContent::Blobs(blobs)
}

/// The `artifactMediaType` of an image, read out of its manifest.
///
/// "The artifact media type of the image" is the media type of the artifact the manifest
/// describes, which a manifest states in its `config` descriptor: the Docker image
/// manifest V2 schema 2 and the OCI image manifest both carry `config.mediaType`, and
/// for a container image that is the image-configuration type. A manifest that carries
/// no config descriptor (a manifest list, a V2 schema 1 manifest, a document that is not
/// a manifest) describes no artifact type, and the member is then absent rather than
/// invented.
pub fn manifest_artifact_media_type(manifest: &str) -> Option<String> {
    let document: serde_json::Value = serde_json::from_str(manifest).ok()?;
    document
        .get("config")?
        .get("mediaType")?
        .as_str()
        .map(ToString::to_string)
}

/// One replication of a stored image to ONE destination registry: the copy the push
/// started under the registry's replication configuration.
///
/// A replication is the record of a copy that was started, not a projection of the
/// registry's rules: it exists because a push happened while a rule matched the
/// repository, which is why a rule stored afterwards adds none ("replication is not
/// retroactive"). Its status follows from the instant the clock owes the copy its
/// completion, so every reader derives the same answer and no read has to write.
#[derive(Debug, Clone)]
pub struct ImageReplication {
    /// The destination Region the image is copied to.
    pub region: String,
    /// The destination registry's account id.
    pub registry_id: String,
    /// When the push that started this copy was made.
    pub started_at: SystemTime,
    /// When the clock owes this copy its `IN_PROGRESS` -> `COMPLETE` transition:
    /// `started_at` + [`IMAGE_REPLICATION_DELAY`].
    pub complete_due_at: SystemTime,
}

impl ImageReplication {
    /// The `ImageReplicationStatus` `DescribeImageReplicationStatus` reports for this
    /// copy.
    ///
    /// `failureCode` is "the failure code for a replication that has failed", so it is
    /// absent: nothing failed, and a code would state a failure that did not happen.
    pub fn to_status(&self, now: SystemTime) -> ImageReplicationStatus {
        let status = if self.complete_due_at <= now {
            REPLICATION_STATUS_COMPLETE
        } else {
            REPLICATION_STATUS_IN_PROGRESS
        };
        ImageReplicationStatus {
            region: Some(self.region.clone()),
            registry_id: Some(self.registry_id.clone()),
            status: Some(status.to_string()),
            failure_code: None,
        }
    }
}

/// One stored image: the manifest as pushed, its digest, and every tag pointing at it.
#[derive(Debug, Clone)]
pub struct ImageRecord {
    pub digest: String,
    pub tags: BTreeSet<String>,
    pub manifest: String,
    pub manifest_media_type: Option<String>,
    pub artifact_media_type: Option<String>,
    /// What [`Self::manifest`] references, read ONCE when the image was stored (see
    /// [`manifest_content`]): the layer sizes of an image manifest, or the digests a
    /// manifest list names. Sizing an image never re-parses a manifest document.
    pub references: ManifestContent,
    /// `imageSizeInBytes`: the size of the IMAGE in the repository, derived from
    /// [`Self::references`] and, for a manifest list, from the images stored beside it.
    /// Maintained by [`Repo::refresh_image_sizes`] whenever the repository's image set
    /// changes, so a read projects it in constant time; `None` when nothing stored states
    /// a size for this image.
    pub size_in_bytes: Option<i64>,
    pub pushed_at: SystemTime,
    /// The one basic scan this image carries, or `None` while none has been started on
    /// it — which is what `DescribeImageScanFindings` reports `ScanNotFoundException`
    /// for. One image carries at most one scan ([`BASIC_SCAN_INTERVAL`]), and it lives on
    /// the image record, so deleting the image drops its scan with it.
    pub scan: Option<ImageScan>,
    /// This image's `ImageStatus`: [`ACTIVE_IMAGE_STATUS`] while it is in Amazon ECR
    /// standard storage and [`ARCHIVED_IMAGE_STATUS`] once `UpdateImageStorageClass` has
    /// moved it to Amazon ECR archive. Per image, so archiving one image in a repository
    /// leaves its neighbours where they are.
    pub status: String,
    /// `lastArchivedAt`: when this image was last transitioned to Amazon ECR archive, or
    /// `None` while it never has been.
    pub last_archived_at: Option<SystemTime>,
    /// `lastActivatedAt`: when this image was last restored from Amazon ECR archive to
    /// Amazon ECR standard, or `None` while it never has been. A push does not set it —
    /// an image that was never archived was never restored either.
    pub last_activated_at: Option<SystemTime>,
    /// When the `ACTIVATING` -> `ACTIVE` transition a restore started falls due, or `None`
    /// when no restore is in flight. See [`IMAGE_RESTORE_DELAY`].
    pub activate_due_at: Option<SystemTime>,
    /// `lastRecordedPullTime`: when this registry last recorded a pull of this image over
    /// its Docker Registry v2 data plane, or `None` while it has recorded none — an image
    /// nobody has pulled, and an image pulled only by a principal the registry's pull time
    /// update exclusion list names. See `state/registry_v2.rs`.
    pub last_recorded_pull_time: Option<SystemTime>,
    /// The copies of this image the push that stored it started, one per destination the
    /// registry's replication configuration sent this repository's images to at that
    /// instant — what `DescribeImageReplicationStatus` reports. Empty for an image pushed
    /// while no rule matched its repository.
    pub replications: Vec<ImageReplication>,
    /// The signing jobs the push that stored it created, one per signing profile the
    /// registry's signing configuration pointed at this repository at that instant — what
    /// `DescribeImageSigningStatus` reports. Empty for an image pushed while no signing
    /// rule matched its repository, which is the empty status list ECR answers with for an
    /// image nothing signed. See [`signing`].
    pub signings: Vec<signing::ImageSigning>,
}

impl ImageRecord {
    /// A freshly pushed image: in standard storage, never archived, and sized by
    /// [`Repo::refresh_image_sizes`] once it is in the repository.
    pub fn pushed(
        digest: String,
        manifest: String,
        manifest_media_type: Option<String>,
        pushed_at: SystemTime,
    ) -> Self {
        Self {
            digest,
            tags: BTreeSet::new(),
            artifact_media_type: manifest_artifact_media_type(&manifest),
            references: manifest_content(&manifest),
            manifest,
            manifest_media_type,
            size_in_bytes: None,
            pushed_at,
            status: ACTIVE_IMAGE_STATUS.to_string(),
            last_archived_at: None,
            last_activated_at: None,
            activate_due_at: None,
            // A pull is what records a pull time; a freshly pushed image has had none.
            last_recorded_pull_time: None,
            // The copies a push starts are decided by the registry's replication
            // configuration, which this record knows nothing about: `PutImage` reads the
            // rules and sets them (see
            // [`ComAmazonawsEcrAmazonec2containerregistryV20150921State::planned_replications`]).
            replications: Vec::new(),
            // The signing jobs a push starts are decided by the registry's signing
            // configuration and by AWS Signer's own profiles, neither of which this record
            // knows anything about: `PutImage` reads them and sets the jobs (see
            // [`signing::planned_signings`]).
            signings: Vec::new(),
            // A scan is started by `StartImageScan`, never by a push: a freshly stored
            // image carries none, which is what `DescribeImageScanFindings` reports
            // `ScanNotFoundException` for.
            scan: None,
        }
    }

    /// The `ImageDetail` `DescribeImages` reports for this image.
    ///
    /// `imageScanStatus` and `imageScanFindingsSummary` are deliberately NOT reported:
    /// "The new version of Amazon ECR Basic Scanning doesn't use the
    /// ImageDetail$imageScanFindingsSummary and ImageDetail$imageScanStatus attributes
    /// from the API response to return scan results. Use the DescribeImageScanFindings
    /// API instead." (`DescribeImages`), and this registry is on that new version —
    /// `BASIC_SCAN_TYPE_VERSION`'s only settable value and its default are `AWS_NATIVE`
    /// (see [`ACCOUNT_SETTINGS`]). A scan is therefore read through
    /// `DescribeImageScanFindings`, which reports the record [`ImageScan`] holds.
    pub fn to_detail(&self, registry_id: &str, repository_name: &str) -> ImageDetail {
        ImageDetail {
            registry_id: Some(registry_id.to_string()),
            repository_name: Some(repository_name.to_string()),
            image_digest: Some(self.digest.clone()),
            // ECR omits `imageTags` for an untagged image rather than reporting [].
            image_tags: if self.tags.is_empty() {
                None
            } else {
                Some(self.tags.iter().cloned().collect())
            },
            image_size_in_bytes: self.size_in_bytes,
            image_pushed_at: Some(reported_instant(self.pushed_at)),
            image_manifest_media_type: self.manifest_media_type.clone(),
            artifact_media_type: self.artifact_media_type.clone(),
            image_status: Some(self.status.clone()),
            last_archived_at: self.last_archived_at.map(reported_instant),
            last_activated_at: self.last_activated_at.map(reported_instant),
            last_recorded_pull_time: self.last_recorded_pull_time.map(reported_instant),
            ..ImageDetail::default()
        }
    }

    /// This image's place in the order a paginated read of a repository's images reports
    /// them in: ascending push instant, and the digest where two images were pushed in
    /// the same instant, so the order is total and the same on every call.
    ///
    /// Rendered as a key [`paginate`] can compare: the push instant as a fixed-width
    /// count of nanoseconds since the epoch, so comparing the rendered keys as text
    /// orders them the same way comparing the instants does, followed by the digest.
    pub fn push_order_key(&self) -> String {
        let since_epoch = self
            .pushed_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{since_epoch:039}\u{1}{}", self.digest)
    }

    /// The byte length of the stored manifest document — `imageManifestSize`, the size of
    /// the manifest a read returns, as distinct from [`Self::size_in_bytes`], the size of
    /// the image content that manifest references. An answer that serves the manifest in
    /// another schema reports ITS length through the same function
    /// ([`manifest_document_size`]).
    pub fn manifest_size(&self) -> i64 {
        manifest_document_size(&self.manifest)
    }

    /// Whether this image's storage class is Amazon ECR archive — including while a
    /// restore of it is in flight, which is a transition out of archive that has not
    /// completed.
    pub fn is_archived(&self) -> bool {
        self.status != ACTIVE_IMAGE_STATUS
    }

    /// Whether a restore of this image is in flight.
    pub fn is_activating(&self) -> bool {
        self.status == ACTIVATING_IMAGE_STATUS
    }

    /// Transition this image to Amazon ECR archive at `now`, recording the instant of the
    /// transition as its `lastArchivedAt`. A restore in flight is abandoned: the image is
    /// in archive again, so there is no activation left to fall due.
    pub fn archive(&mut self, now: SystemTime) {
        self.status = ARCHIVED_IMAGE_STATUS.to_string();
        self.last_archived_at = Some(now);
        self.activate_due_at = None;
    }

    /// Start restoring this image from Amazon ECR archive to Amazon ECR standard at
    /// `now`.
    ///
    /// The image enters `ACTIVATING` and the completion the clock owes falls due
    /// [`IMAGE_RESTORE_DELAY`] later ([`Self::settle_storage_class`] materialises it).
    /// `lastActivatedAt` is the instant the image was restored, which is what a caller
    /// reading it while it activates sees.
    pub fn activate(&mut self, now: SystemTime) {
        self.status = ACTIVATING_IMAGE_STATUS.to_string();
        self.last_activated_at = Some(now);
        self.activate_due_at = Some(now + IMAGE_RESTORE_DELAY);
    }

    /// Materialise the `ACTIVATING` -> `ACTIVE` transition the clock has already made due
    /// at `now`.
    ///
    /// The transition is owed by the clock, not by a call: it falls due whether or not
    /// anybody asks, so every operation that looks at an image writes down what was
    /// already due before reporting it.
    pub fn settle_storage_class(&mut self, now: SystemTime) {
        let Some(due) = self.activate_due_at else {
            return;
        };
        if due <= now {
            self.status = ACTIVE_IMAGE_STATUS.to_string();
            self.activate_due_at = None;
        }
    }

    /// The identifier that addresses this image for a request that named `tag`.
    ///
    /// The digest is this image's own — the stored one, never the one a request guessed
    /// — and the tag is reported only when it really points at this image.
    pub fn identifier(&self, tag: Option<&str>) -> ImageIdentifier {
        ImageIdentifier {
            image_digest: Some(self.digest.clone()),
            image_tag: tag
                .filter(|tag| self.tags.contains(*tag))
                .map(ToString::to_string),
        }
    }

    /// The manifest as a JSON document, for the members a registry reads out of it
    /// (an OCI artifact's `subject`, `artifactType` and `annotations`).
    ///
    /// `None` when the stored manifest is not JSON at all, which is what a caller who
    /// pushed something else gets: no member can be read out of it.
    pub fn manifest_document(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&self.manifest).ok()
    }

    /// The `(digest, tag)` pairs this image is addressable by: one per tag, or a single
    /// tagless identifier when nothing points at it.
    pub fn identifiers(&self) -> Vec<ImageIdentifier> {
        if self.tags.is_empty() {
            return vec![ImageIdentifier {
                image_digest: Some(self.digest.clone()),
                image_tag: None,
            }];
        }
        self.tags
            .iter()
            .map(|tag| ImageIdentifier {
                image_digest: Some(self.digest.clone()),
                image_tag: Some(tag.clone()),
            })
            .collect()
    }
}

/// See [`MAX_REPOSITORIES`]. This one bounds `uploads`, which grows by one entry per
/// `InitiateLayerUpload` and shrinks when an upload is completed. An upload a client
/// opened and then abandoned is reclaimed only when the registry needs its slot — see
/// [`ABANDONED_UPLOAD_AGE`] and
/// [`reclaim_abandoned_uploads`](ComAmazonawsEcrAmazonec2containerregistryV20150921State::reclaim_abandoned_uploads)
/// — so the registry needs a ceiling on how many in-flight layer uploads it holds at
/// once. Past it, with nothing reclaimable, the open is refused with the
/// `ServerException` `InitiateLayerUpload` models: reaching the ceiling is this
/// registry's own capacity, not a fault in the request.
pub const MAX_LAYER_UPLOADS: usize = 1_000;

/// How long an in-flight upload nothing has touched may sit in the store before the
/// registry may reclaim its slot to open a new one.
///
/// AWS publishes no age at which an in-flight layer upload expires, so this span is the
/// EMULATOR's own capacity policy rather than a claim about the service: an upload is
/// never dropped while the store has room, so no ordinary push can lose its session, and
/// a client that opens uploads and abandons them cannot pin [`MAX_LAYER_UPLOADS`] slots
/// (and their bytes) for the life of the process. It is deliberately far longer than any
/// push takes.
pub const ABANDONED_UPLOAD_AGE: Duration = Duration::from_secs(60 * 60);

/// The registry-wide ceiling on the caller-supplied BYTES this store retains at once:
/// every in-flight upload's received bytes, every registered layer blob's bytes, and
/// every stored image manifest.
///
/// The per-record caps above each bound ONE dimension and their product is far larger
/// than any host — [`MAX_LAYER_UPLOADS`] uploads of [`MAX_LAYER_UPLOAD_BYTES`] alone
/// would be 128 GiB — so this is the aggregate the two growing paths are actually held
/// to: `UploadLayerPart` and `PutImage` refuse, with the `LimitExceededException` each
/// models, the call that would take the registry past it, and no byte of the refused
/// payload is kept. It is a bound on THIS PROCESS's memory rather than an AWS quota (AWS
/// publishes none), set well above the bytes any ordinary push or test sends.
pub const MAX_REGISTRY_CALLER_BYTES: usize = 512 * 1024 * 1024;

/// The `partSize` `InitiateLayerUpload` reports: the size ECR tells the client to send
/// future layer parts in.
///
/// AWS publishes no single number for it — it is the registry's own answer, which is
/// why the client is told it rather than assuming one — so this is the emulator's
/// choice, bounded by the only published constraint on a part: the model caps
/// `layerPartBlob` at 20971520 bytes, and a part of `partSize` bytes must fit under
/// that cap.
pub const REPORTED_LAYER_PART_SIZE: i64 = 10 * 1024 * 1024;

/// See [`MAX_REPOSITORIES`]: the ceiling on how many bytes ONE in-flight upload
/// accumulates. `UploadLayerPart` is the only thing that grows it — by up to 20971520
/// bytes per call, the model's cap on `layerPartBlob` — and the bytes stay in memory
/// until the upload is completed or the process ends, so the registry needs a ceiling
/// on the layer a client can stream into it. Past it the part is refused with the
/// `LimitExceededException` `UploadLayerPart` models ("the operation did not succeed
/// because it would have exceeded a service limit for your account") and no byte of it
/// is kept. AWS publishes no fixed maximum layer size, so the number is this
/// emulator's own bound, set well above any layer an ordinary push sends.
pub const MAX_LAYER_UPLOAD_BYTES: usize = 128 * 1024 * 1024;

/// See [`MAX_REPOSITORIES`]. This one bounds [`Repo::layer_blobs`], which grows by one
/// entry per `CompleteLayerUpload` that registers a blob the repository did not already
/// hold, and nothing shrinks: a registered blob stays for the life of the process, so
/// the registry needs a ceiling on how many blobs one repository holds a record of.
/// Past it the completion is refused with the `ServerException`
/// `CompleteLayerUpload` models — reaching the ceiling is this registry's own capacity,
/// not a fault in the request, and that operation models no limit error. AWS publishes
/// no fixed maximum layer count per repository, so the number is this emulator's own
/// bound, set well above the layer count of any ordinary image set.
pub const MAX_LAYERS_PER_REPOSITORY: usize = 10_000;

/// One in-flight layer upload: what `InitiateLayerUpload` opened, addressed by the
/// upload id it handed the client.
///
/// `UploadLayerPart` streams the layer into it one part at a time and
/// `CompleteLayerUpload` is what closes it, registering the assembled blob on the
/// repository ([`Repo::layer_blobs`]) and dropping the upload, so what is kept here is
/// what opening the upload decided — which repository it is for, the part size the
/// client was told to send, when it was opened — plus the layer bytes received so far.
#[derive(Debug, Clone)]
pub struct LayerUpload {
    /// The repository the upload was opened against. An upload id is only valid for
    /// the repository it was issued for, so every later call names both.
    pub repository_name: String,
    /// The `partSize` reported to the client when the upload was opened.
    pub part_size: i64,
    /// When `InitiateLayerUpload` opened it. A client closes an upload with
    /// `CompleteLayerUpload`, and AWS publishes no age at which an in-flight upload
    /// expires, so this registry expires none while it has room; it is read only when a
    /// new upload needs a slot the store does not have, to reclaim an upload nothing has
    /// touched for [`ABANDONED_UPLOAD_AGE`]
    /// ([`reclaim_abandoned_uploads`](ComAmazonawsEcrAmazonec2containerregistryV20150921State::reclaim_abandoned_uploads)).
    pub started_at: SystemTime,
    /// The layer bytes accepted so far, in the order they arrived. Each part starts
    /// where the previous one ended, so this is the prefix of the layer the registry
    /// holds and its length is both the byte count received and the offset the next
    /// part must start at — see [`LayerUpload::receive_part`].
    pub received_bytes: Vec<u8>,
}

/// Why a layer part is not accepted into an upload. The caller-facing error each one
/// answers with belongs to the operation (`UploadLayerPart`'s
/// `InvalidLayerPartException` and `LimitExceededException`); what is decided here is
/// whether the part may join the upload at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerPartRejection {
    /// `partFirstByte` and `partLastByte` do not describe the blob sent with them: the
    /// range spans a different number of bytes, or spans none at all.
    PartSizeMismatch,
    /// The part does not begin where the upload's received bytes end, so accepting it
    /// would leave a gap in the layer or overwrite bytes already received.
    NonConsecutivePart,
    /// Accepting the part would take the upload past [`MAX_LAYER_UPLOAD_BYTES`].
    UploadTooLarge,
}

impl LayerUpload {
    /// The offset the next part must start at: the number of bytes already accepted, so
    /// the first part of an upload starts at byte 0.
    pub fn next_part_first_byte(&self) -> i64 {
        i64::try_from(self.received_bytes.len()).unwrap_or(i64::MAX)
    }

    /// The last byte of the layer this upload has accepted, or `None` while it has
    /// accepted nothing — there is no last byte before the first part arrives.
    pub fn last_byte_received(&self) -> Option<i64> {
        self.next_part_first_byte()
            .checked_sub(1)
            .filter(|last| *last >= 0)
    }

    /// Append one layer part to this upload, answering the last byte the upload has now
    /// received.
    ///
    /// A part is accepted only when its declared `[partFirstByte, partLastByte]` range
    /// spans exactly as many bytes as the blob carries AND starts where the upload's
    /// received bytes end; a rejected part changes nothing, so the upload stays exactly
    /// as the last accepted part left it.
    pub fn receive_part(
        &mut self,
        part_first_byte: i64,
        part_last_byte: i64,
        blob: &[u8],
    ) -> Result<i64, LayerPartRejection> {
        let declared_len = part_last_byte
            .checked_sub(part_first_byte)
            .and_then(|span| span.checked_add(1))
            .filter(|len| *len > 0);
        let carried_len = i64::try_from(blob.len()).unwrap_or(i64::MAX);
        if declared_len != Some(carried_len) {
            return Err(LayerPartRejection::PartSizeMismatch);
        }
        if part_first_byte != self.next_part_first_byte() {
            return Err(LayerPartRejection::NonConsecutivePart);
        }
        if self.received_bytes.len().saturating_add(blob.len()) > MAX_LAYER_UPLOAD_BYTES {
            return Err(LayerPartRejection::UploadTooLarge);
        }
        self.received_bytes.extend_from_slice(blob);
        Ok(self.last_byte_received().unwrap_or(part_last_byte))
    }
}

/// How long a layer download URL `GetDownloadUrlForLayer` hands out stays valid.
///
/// AWS publishes no validity window for the pre-signed URL that operation answers with,
/// so this span is the EMULATOR's own choice rather than a claim about the service. It is
/// long enough that a client which asked for a URL per layer of an image can pull every
/// one of them, and far inside the longest validity a pre-signed URL may carry at all
/// (seven days), which is the only published bound on one.
pub const LAYER_DOWNLOAD_URL_TTL: Duration = Duration::from_secs(60 * 60);

/// The path space this registry publishes layer downloads under, beneath the endpoint the
/// caller reached this emulator on.
///
/// `GetDownloadUrlForLayer`'s `downloadUrl` is a promise that whoever holds it can FETCH
/// the layer, so the URL has to name a host this emulator serves: a literal Amazon S3
/// hostname would send the next client out of the emulator entirely. The prefix names
/// this service and this resource, so it cannot collide with another service's URI space,
/// and no modelled operation of this service is bound to a path at all — every one of
/// them is `POST /` on awsJson1_1 — so claiming a read beneath it cannot take a request
/// any operation would have served.
pub const LAYER_DOWNLOAD_PATH_PREFIX: &str = "/ecr-layer-download";

/// The media type a layer download is served under.
///
/// A layer is opaque content to the registry: what it IS is stated by the descriptor of
/// whichever manifest references it, not by the blob, so a download carries the generic
/// binary type — as the pre-signed object store a real download is served from does.
pub const LAYER_DOWNLOAD_CONTENT_TYPE: &str = "application/octet-stream";

/// See [`MAX_REPOSITORIES`]. This one bounds `layer_download_grants`, which grows by one
/// entry per `GetDownloadUrlForLayer` call: each call hands out its own URL, exactly as
/// each pre-signed URL AWS answers with is its own, so the only thing that shrinks the
/// table is a grant's window closing
/// ([`reclaim_expired_layer_download_grants`](ComAmazonawsEcrAmazonec2containerregistryV20150921State::reclaim_expired_layer_download_grants)).
/// Past the ceiling, with nothing expired to reclaim, the call is refused with the
/// `ServerException` that operation models: reaching it is this registry's own capacity,
/// not a fault in the request. A grant is a few hundred bytes and holds no layer content,
/// so the number is set well above the URLs any ordinary pull sequence asks for.
pub const MAX_LAYER_DOWNLOAD_GRANTS: usize = 10_000;

/// One layer download URL this registry handed out: which layer of which repository it
/// serves, and the window it serves it in.
///
/// A pre-signed URL is a CAPABILITY, so what makes a fetch resolve is a grant this
/// registry minted and still holds — addressed by the opaque token the URL carries — and
/// never the repository or digest spelled in the URL's path: a path a client assembled
/// for itself names no grant and serves nothing.
///
/// The layer's bytes are NOT copied here. A download reads the repository's own record of
/// the blob ([`LayerBlob::content`]) when it is fetched, so a grant for a layer this
/// registry no longer holds has nothing to serve, and two grants for one layer are two
/// capabilities over one copy of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LayerDownloadGrant {
    /// The repository whose blob this grant serves. A blob is per-repository, so the
    /// grant names the repository as well as the digest.
    pub repository_name: String,
    /// The digest of the layer this grant serves.
    pub layer_digest: String,
    /// When `GetDownloadUrlForLayer` minted it.
    pub issued_at: SystemTime,
    /// When its window closes: [`LAYER_DOWNLOAD_URL_TTL`] after it was minted. Stored as
    /// the instant rather than recomputed per fetch, so a grant's window is decided once,
    /// by the call that handed the URL out.
    pub expires_at: SystemTime,
}

impl LayerDownloadGrant {
    /// The grant for one layer of one repository, minted at `now` and valid from then for
    /// [`LAYER_DOWNLOAD_URL_TTL`].
    pub fn issued(repository_name: &str, layer_digest: &str, now: SystemTime) -> Self {
        Self {
            repository_name: repository_name.to_string(),
            layer_digest: layer_digest.to_string(),
            issued_at: now,
            expires_at: now + LAYER_DOWNLOAD_URL_TTL,
        }
    }

    /// Whether this grant's window has closed as of `now`, which is what makes a fetch of
    /// its URL an expired request.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.expires_at <= now
    }

    /// How long the URL stays valid from the instant it was issued, in whole seconds:
    /// the validity the URL states to whoever holds it.
    pub fn validity_seconds(&self) -> u64 {
        self.expires_at
            .duration_since(self.issued_at)
            .unwrap_or_default()
            .as_secs()
    }
}

/// Why an `imageIds` entry resolves to no stored image.
///
/// The variants are named after the `ImageFailureCode` values ECR reports for an
/// identifier it could not resolve (`IMAGE_FAILURE_CODE_VALUES` in
/// `ops::types::shared_types`), so a batch operation reports one directly and an
/// operation that refuses the whole call maps it onto its own modelled error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageLookupFailure {
    /// The entry carries neither an `imageDigest` nor an `imageTag`, so it names
    /// nothing at all.
    MissingDigestAndTag,
    /// The entry's `imageDigest` is not a content digest at all. The model gives
    /// `ImageDigest` no pattern, so a malformed digest reaches the handler and is
    /// reported against its own entry rather than refusing the whole batch.
    InvalidImageDigest,
    /// Nothing in the repository answers to the entry.
    ImageNotFound,
    /// The entry's digest and its tag each name a stored image, but not the same one.
    ImageTagDoesNotMatchDigest,
}

impl ImageLookupFailure {
    /// The `ImageFailureCode` this failure is reported as.
    pub fn code(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => "MissingDigestAndTag",
            Self::InvalidImageDigest => "InvalidImageDigest",
            Self::ImageNotFound => "ImageNotFound",
            Self::ImageTagDoesNotMatchDigest => "ImageTagDoesNotMatchDigest",
        }
    }

    /// The `failureReason` reported beside [`Self::code`]: what about the request made
    /// the identifier unresolvable.
    pub fn reason(self) -> &'static str {
        match self {
            Self::MissingDigestAndTag => {
                "Invalid request parameters: at least one of [imageDigest, imageTag] \
                 must be specified"
            }
            Self::InvalidImageDigest => {
                "Invalid request parameters: imageDigest must be of the form \
                 <algorithm>:<hex digest>"
            }
            Self::ImageNotFound => "Requested image not found",
            Self::ImageTagDoesNotMatchDigest => {
                "Requested image tag does not match the requested image digest"
            }
        }
    }
}

/// One parsed `rules` entry of a lifecycle policy document.
///
/// ECR's lifecycle policy document is the service's own format, so it is read here
/// rather than with a crates.io parser: the JSON itself comes from `serde_json`, and
/// what this adds is the meaning of `selection` and `action`. Every modelled value it
/// checks against comes from the model's own enums (`TAG_STATUS_VALUES`,
/// `IMAGE_ACTION_TYPE_VALUES`, `LIFECYCLE_POLICY_TARGET_STORAGE_CLASS_VALUES`).
#[derive(Debug, Clone)]
pub struct LifecycleRule {
    /// `rulePriority`: unique within a policy, and the order rules are applied in.
    pub priority: i32,
    /// `selection.tagStatus`, as the modelled `TagStatus` spells it.
    pub tag_status: String,
    /// `selection.tagPrefixList`: a tag the rule selects begins with one of these.
    pub tag_prefixes: Vec<String>,
    /// `selection.tagPatternList`: a tag the rule selects matches one of these, where
    /// `*` stands for any run of characters.
    pub tag_patterns: Vec<String>,
    /// `selection.countType` and the numbers that go with it.
    pub count: LifecycleCount,
    /// `action.type`, as the modelled `ImageActionType` spells it.
    pub action_type: String,
    /// `action.targetStorageClass`, present only on a `TRANSITION` action.
    pub target_storage_class: Option<String>,
}

/// What a rule's `selection.countType` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleCount {
    /// `imageCountMoreThan`: the rule acts on every image its selection matches beyond
    /// the newest `keep` of them.
    ImageCountMoreThan { keep: usize },
    /// `sinceImagePushed`: the rule acts on every image its selection matches that was
    /// pushed more than `days` days ago.
    SinceImagePushed { days: u64 },
}

impl LifecycleRule {
    /// Whether this rule's `selection` admits `image`, before its count is applied.
    fn selects(&self, image: &ImageRecord) -> bool {
        match self.tag_status.as_str() {
            "UNTAGGED" => image.tags.is_empty(),
            "TAGGED" => image.tags.iter().any(|tag| self.tag_matches(tag)),
            // ANY, and anything the model widens `TagStatus` with later.
            _ => true,
        }
    }

    /// Whether `tag` begins with one of the rule's `tagPrefixList` entries or matches one
    /// of its `tagPatternList` wildcards.
    fn tag_matches(&self, tag: &str) -> bool {
        self.tag_prefixes
            .iter()
            .any(|prefix| tag.starts_with(prefix))
            || self
                .tag_patterns
                .iter()
                .any(|pattern| wildcard_matches(pattern, tag))
    }
}

/// One image a preview acts on: the image as it stood when the preview was started, and
/// the rule that claimed it.
#[derive(Debug, Clone)]
pub struct PreviewedImage {
    pub digest: String,
    pub tags: BTreeSet<String>,
    pub pushed_at: SystemTime,
    pub applied_rule_priority: i32,
    pub action_type: String,
    pub target_storage_class: Option<String>,
    /// `storageClass`: "the storage class of the image" as it stood when the preview was
    /// started, read from the image's own storage class rather than assumed (see
    /// [`storage_class_of_status`]).
    pub storage_class: String,
}

impl PreviewedImage {
    /// The `LifecyclePolicyPreviewResult` a preview read reports for this image.
    pub fn to_result(&self) -> LifecyclePolicyPreviewResult {
        LifecyclePolicyPreviewResult {
            // As everywhere else in this service, an untagged image omits `imageTags`
            // rather than reporting [].
            image_tags: if self.tags.is_empty() {
                None
            } else {
                Some(self.tags.iter().cloned().collect())
            },
            image_digest: Some(self.digest.clone()),
            image_pushed_at: Some(reported_instant(self.pushed_at)),
            action: Some(LifecyclePolicyRuleAction {
                r#type: Some(self.action_type.clone()),
                target_storage_class: self.target_storage_class.clone(),
            }),
            applied_rule_priority: Some(self.applied_rule_priority),
            storage_class: Some(self.storage_class.clone()),
        }
    }
}

/// How an `imageIds` entry reads in a message: ECR renders the requested
/// `ImageIdentifier` with both members spelled out and an absent one as `null`, so a
/// caller sees the entry exactly as it sent it.
///
/// The public-registry service renders it identically
/// (`service-com-amazonaws-ecrpublic-spencerfrontendservice`'s
/// `state::identifier_description`); the two are separate functions only because
/// `ImageIdentifier` is generated per service crate. Keep them in step.
pub fn identifier_description(id: &ImageIdentifier) -> String {
    format!("imageId {}", identifier_members(id))
}

/// The BRACE EXPRESSION alone of [`identifier_description`]: both members of the
/// requested `ImageIdentifier` spelled out, an absent one as `null`, with no `imageId`
/// label in front of it.
///
/// The label belongs to the SENTENCE rather than to the identifier, and ECR words the
/// two sentences differently: one names the identifier (`The image with imageId
/// {…}`), the other quotes the expression itself (`… for the image with '{…}'`). Both
/// render the identifier from this one function, so the two cannot drift apart in how a
/// digest or a tag is spelled.
pub fn identifier_members(id: &ImageIdentifier) -> String {
    format!(
        "{{imageDigest:'{}', imageTag:'{}'}}",
        id.image_digest.as_deref().unwrap_or("null"),
        id.image_tag.as_deref().unwrap_or("null")
    )
}

/// The lifecycle policy a repository carries: the document [`Repo`] was given, and the
/// instant that policy was last evaluated.
///
/// The document is held as TEXT and never as a parsed value: `lifecyclePolicyText` is an
/// opaque document (`ecr#LifecyclePolicyText`, a string), so what is stored is what every
/// operation reporting this policy reports, down to the member order and the spelling of
/// each token. The one thing ECR normalises is the whitespace between those tokens, which
/// [`normalize_lifecycle_policy`] removes once, on the way in.
/// [`parse_lifecycle_policy`] reads the document to validate it and to evaluate it, and
/// throws the parse away.
#[derive(Debug, Clone)]
pub struct StoredLifecyclePolicy {
    /// The document as ECR reports it: the document the `PutLifecyclePolicy` that stored
    /// it supplied, with the whitespace between its JSON tokens removed
    /// ([`normalize_lifecycle_policy`]). Every operation that reports this policy reports
    /// these same bytes, so a read never differs from another read.
    pub text: String,
    /// The instant this policy was last evaluated — the `lastEvaluatedAt`
    /// `GetLifecyclePolicy` and `DeleteLifecyclePolicy` report (`ecr#EvaluationTimestamp`,
    /// "The time stamp of the last time that the lifecycle policy was run").
    ///
    /// Setting a policy is what evaluates it, so this is the instant of the
    /// `PutLifecyclePolicy` that stored this document: ECR documents that the images an
    /// evaluation claims are then acted on "within 24 hours" of it, which is longer than
    /// any run of this emulator, so nothing here removes an image behind a caller's back
    /// and `StartLifecyclePolicyPreview` stays the way to see what a policy would act on.
    /// No read writes this instant, so two reads of one policy report the same one.
    pub last_evaluated_at: SystemTime,
}

/// The lifecycle policy preview one `StartLifecyclePolicyPreview` call started.
///
/// The evaluation is computed once, when the preview is started, from the images the
/// repository held at that instant: a preview is a snapshot, so an image pushed after it
/// started does not appear in it, and reading the preview back computes nothing new.
#[derive(Debug, Clone)]
pub struct LifecyclePreview {
    /// The policy the preview was started with — the text the start call named, or the
    /// repository's own policy when it named none.
    pub policy_text: String,
    /// A [`PREVIEW_STATUS_IN_PROGRESS`] / [`PREVIEW_STATUS_COMPLETE`] value.
    pub status: String,
    /// When the `IN_PROGRESS` -> `COMPLETE` transition the clock owes falls due; `None`
    /// once it has been materialised.
    pub complete_due_at: Option<SystemTime>,
    /// The images the policy acts on, keyed by digest so every page — and therefore
    /// every pagination token — has one deterministic order.
    pub results: BTreeMap<String, PreviewedImage>,
}

impl LifecyclePreview {
    /// The `summary` a preview read reports: how many images the preview expires, and how
    /// many it transitions to each storage class.
    ///
    /// It summarises the whole preview rather than one page of it, so it does not change
    /// as a caller pages through the results.
    pub fn summary(&self) -> LifecyclePolicyPreviewSummary {
        let expiring = self
            .results
            .values()
            .filter(|result| result.action_type == EXPIRE_ACTION_TYPE)
            .count();
        let mut transitioning: BTreeMap<String, i32> = BTreeMap::new();
        for result in self
            .results
            .values()
            .filter(|result| result.action_type == TRANSITION_ACTION_TYPE)
        {
            let class = result.target_storage_class.clone().unwrap_or_default();
            *transitioning.entry(class).or_default() += 1;
        }
        LifecyclePolicyPreviewSummary {
            expiring_image_total_count: Some(i32::try_from(expiring).unwrap_or(i32::MAX)),
            // "This field is only present if at least one image will be transitioned in
            // the summary."
            transitioning_image_total_counts: if transitioning.is_empty() {
                None
            } else {
                Some(
                    transitioning
                        .into_iter()
                        .map(
                            |(target_storage_class, image_total_count)| {
                                TransitioningImageTotalCount {
                                    target_storage_class: Some(target_storage_class),
                                    image_total_count: Some(image_total_count),
                                }
                            },
                        )
                        .collect(),
                )
            },
        }
    }
}

/// The lifecycle policy document ECR reports back for `text`: the same JSON document with
/// the insignificant whitespace between its tokens removed.
///
/// ECR does not echo the document a caller supplied byte for byte. It reports a MINIFIED
/// rendering of it: replaying the recorded `PutLifecyclePolicy` while this service stored
/// the submitted text verbatim diverges on `lifecyclePolicyText` on the put and on every
/// later read of that policy, and reporting it minified is what agrees with the recording
/// — so the document the recorded request carried was not the spelling ECR answered with.
/// Nothing else about the document moves: the members keep the order they were given in,
/// and every token — a string's escapes, a number's spelling — is reported as it arrived.
///
/// The scan this delegates to is TOTAL, so a text that is not JSON comes back with the
/// whitespace outside its quotes dropped rather than refused here: every caller validates
/// the document with [`parse_lifecycle_policy`] first, which is what refuses one.
///
/// The scan itself belongs to no one service — a document reported back keeps its bytes
/// wherever it is reported — so it lives in `service_common::json_document` and is named
/// here for the member it is the rendering of.
pub fn normalize_lifecycle_policy(text: &str) -> String {
    service_common::json_document::minified(text)
}

/// The instant this service REPORTS for a stored `SystemTime`: the same instant, truncated
/// to a whole millisecond.
///
/// ECR states the instants it reports to a resolution of one millisecond — "the date and
/// time, expressed in standard JavaScript date format" (`ImageDetail$imagePushedAt`),
/// whose resolution is the millisecond, and "the Unix time in seconds and milliseconds"
/// (`AuthorizationData$expiresAt`). A `SystemTime` read from the host clock carries
/// nanoseconds, so the sub-millisecond part is dropped here, in the ONE place a stored
/// instant becomes a reported one — every member of every response this service serves
/// goes through it, so no answer states an instant more precisely than ECR does.
pub fn reported_instant(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    ::aws_smithy_types::DateTime::from_millis(i64::try_from(since_epoch).unwrap_or(i64::MAX))
}

/// ECR's refusal of a parameter it will not accept, worded to the template it states
/// every such refusal with: `Invalid parameter at '<Parameter>' failed to satisfy
/// constraint: '<constraint>'`.
///
/// The ONE place that template is written for a refusal this service's own validation
/// raises (the boundary validator words the constraints the MODEL declares; see
/// `crate::service`). `parameter` is the API label of the request parameter and
/// `constraint` a phrase naming what it failed; neither half ever carries a detail of how
/// this emulator read the value.
pub fn invalid_parameter_at(parameter: &str, constraint: &str) -> Fault {
    Fault::InvalidParameter(parameter_constraint(parameter, constraint))
}

/// The clause [`invalid_parameter_at`] states, as TEXT: the one place the template is
/// written, for a caller that carries it on a modelled error of its own rather than on a
/// [`Fault`].
pub fn parameter_constraint(parameter: &str, constraint: &str) -> String {
    format!("Invalid parameter at '{parameter}' failed to satisfy constraint: '{constraint}'")
}

/// [`invalid_parameter_at`] for a DOCUMENT-valued parameter: `parameter` is the
/// API-DOCUMENTED label of the request parameter carrying the document
/// ([`POLICY_TEXT_PARAMETER`], [`LIFECYCLE_POLICY_TEXT_PARAMETER`]), which the API
/// reference capitalises — not the lower-camel wire member the request spells — and
/// `constraint` is a stable phrase naming the constraint the document failed, never a
/// parser position.
pub fn invalid_document(parameter: &str, constraint: &str) -> Fault {
    invalid_parameter_at(parameter, constraint)
}

/// The API-documented label of the `policyText` parameter, as a resource-policy refusal
/// names it.
pub const POLICY_TEXT_PARAMETER: &str = "PolicyText";

/// The API-documented label of the `lifecyclePolicyText` parameter, as a lifecycle
/// policy refusal names it.
pub const LIFECYCLE_POLICY_TEXT_PARAMETER: &str = "LifecyclePolicyText";

/// The API-documented label of the `nextToken` parameter, as a pagination refusal names
/// it: `NextToken` is how the API reference spells the parameter, which is the label a
/// refusal states rather than the lower-camel wire member.
pub const NEXT_TOKEN_PARAMETER: &str = "NextToken";

/// The constraint a continuation token this service never minted fails. One phrase for
/// the class — a token is opaque, so a caller learns only that the one it sent is not a
/// token of this listing, never how this service reads it or what it contained.
pub const NEXT_TOKEN_CONSTRAINT: &str = "Invalid next token provided";

/// The API-documented label of the `maxResults` parameter, as a pagination refusal names
/// it.
pub const MAX_RESULTS_PARAMETER: &str = "MaxResults";

/// The API-documented label of the `repositoryNames` parameter, as a refusal naming the
/// repositories a read was scoped to names it.
pub const REPOSITORY_NAMES_PARAMETER: &str = "RepositoryNames";

/// The API-documented label of the `imageIds` parameter, as a refusal naming the images a
/// read was scoped to names it.
pub const IMAGE_IDS_PARAMETER: &str = "ImageIds";

/// Refuse a read that asks for a PAGE of a set it has already enumerated.
///
/// A read whose request carries the identifiers to report (`identified`) reports exactly
/// those, so paging it means nothing: of `maxResults` and `nextToken`, "This option cannot
/// be used when you specify ..." (`DescribeRepositoriesRequest$maxResults`,
/// `DescribeImagesRequest$maxResults`, `GetLifecyclePolicyPreviewRequest$maxResults`, and
/// each of those requests' `nextToken`). The combination is refused as an invalid
/// parameter, in the template ECR states a refused parameter in
/// ([`invalid_parameter_at`]), naming the pagination parameter the request may not carry
/// and the identifier parameter that excludes it — `identifiers` being the API-documented
/// label of that list ([`REPOSITORY_NAMES_PARAMETER`], [`IMAGE_IDS_PARAMETER`]). Nothing
/// of the request's values is echoed: the clause is the same for every such refusal.
///
/// `maxResults` is reported ahead of `nextToken` for a request carrying both, so one
/// request is refused for one parameter.
pub fn check_pagination_excluded(
    identifiers: &str,
    identified: bool,
    max_results: Option<i32>,
    next_token: Option<&String>,
) -> Result<(), Fault> {
    if !identified {
        return Ok(());
    }
    let excluded = max_results
        .map(|_| MAX_RESULTS_PARAMETER)
        .or_else(|| next_token.map(|_| NEXT_TOKEN_PARAMETER));
    match excluded {
        Some(parameter) => Err(invalid_parameter_at(
            parameter,
            &format!("Cannot provide {parameter} if providing {identifiers}"),
        )),
        None => Ok(()),
    }
}

/// The constraint a lifecycle document that cannot be MAPPED onto the lifecycle policy
/// shape fails: JSON that does not parse, or a member whose JSON type the shape cannot
/// hold. One phrase covers the class, because nothing of the document was read.
pub const LIFECYCLE_POLICY_PARSING_CONSTRAINT: &str =
    "Lifecycle policy parsing failure: Could not map policyString into LifecyclePolicy.";

/// A lifecycle document that maps onto the shape but breaks one of the policy format's
/// own constraints, which `detail` states. A `detail` that is the schema validator's
/// output carries that validator's line terminator (see [`validator_report`]); one ECR
/// words itself does not.
pub const LIFECYCLE_POLICY_VALIDATION_CONSTRAINT_PREFIX: &str =
    "Lifecycle policy validation failure: ";

/// [`invalid_document`] for a lifecycle document that cannot be mapped onto the shape.
fn lifecycle_unmappable() -> Fault {
    invalid_document(
        LIFECYCLE_POLICY_TEXT_PARAMETER,
        LIFECYCLE_POLICY_PARSING_CONSTRAINT,
    )
}

/// [`invalid_document`] for a lifecycle document that breaks the constraint `detail`
/// states.
fn lifecycle_invalid(detail: String) -> Fault {
    invalid_document(
        LIFECYCLE_POLICY_TEXT_PARAMETER,
        &format!("{LIFECYCLE_POLICY_VALIDATION_CONSTRAINT_PREFIX}{detail}"),
    )
}

/// The schema check of a lifecycle document is a JSON-schema validator, and the
/// constraint a refusal quotes for a schema failure is that validator's OWN output —
/// which it writes as a LINE, so the text ends with the newline that terminates it and
/// the constraint's closing quote follows that newline rather than the last word.
///
/// Every phrase below goes through here, because every one of them is a report the
/// validator produced. The policy format's own cross-member rules (a tag filter against
/// a `tagStatus`, a `countUnit` against a `countType`, a unique rule priority, the
/// evaluation order) are checked after the schema and worded by ECR itself, so they are
/// stated as plain sentences with no terminator.
fn validator_report(line: String) -> String {
    format!("{line}\n")
}

/// The constraint phrase for a member the document leaves out.
fn missing_required(member: &str) -> String {
    validator_report(format!(
        "object has missing required properties ([\"{member}\"])"
    ))
}

/// The constraint phrase for a number below the minimum its member declares, naming
/// both the bound and the value the request carried.
fn below_minimum(minimum: i64, found: i64) -> String {
    validator_report(format!(
        "numeric instance is lower than the required minimum (minimum: {minimum}, \
         found: {found})"
    ))
}

/// The constraint phrase for a value outside the set its member admits, naming the
/// value the request carried and every value the format defines for it.
fn not_in_enum<S: AsRef<str>>(value: &str, allowed: &[S]) -> String {
    let values: Vec<String> = allowed
        .iter()
        .map(|value| format!("\"{}\"", value.as_ref()))
        .collect();
    validator_report(format!(
        "instance value (\"{value}\") not found in enum (possible values: [{}])",
        values.join(",")
    ))
}

/// The constraint phrase for an array holding fewer entries than its member requires.
fn array_too_short(minimum: usize, found: usize) -> String {
    validator_report(format!(
        "array is too short: must have at least {minimum} elements but instance has \
         {found} elements"
    ))
}

/// How many alternative forms the lifecycle policy schema admits a RULE in — the forms it
/// CHOOSES between, so a rule has to be in exactly one of them.
///
/// A rule's form is fixed by the two values the choice is made on, its count type and its
/// action type, because each pair also fixes which of the rule's conditional members it
/// carries: a `countUnit` with an age count and none with a count of images, a
/// `targetStorageClass` with a transition and none with an expiry. Neither of the two
/// alone admits as many forms as the count ECR reports, and their cross does, so the
/// alternatives are the cross and the number is derived from the two value sets rather
/// than written out.
fn rule_alternatives() -> usize {
    LIFECYCLE_COUNT_TYPES.len() * IMAGE_ACTION_TYPE_VALUES.len()
}

/// The report the schema produces for a rule that is in NONE of those forms: a choice
/// states its OWN outcome — how many of the alternatives the instance matched — and never
/// the sub-constraint one alternative failed, so this report carries neither the value the
/// rule gave nor the set an alternative admits.
///
/// Only the members the choice is MADE on reach it: the two values it is made on
/// (`selection.countType`, `action.type`) and the conditional members each pair fixes
/// (`selection.countUnit`, `action.targetStorageClass`). Every member the alternatives
/// SHARE — a rule's `rulePriority`, a selection's `tagStatus` and `countNumber`, and the
/// presence of each of them — is checked by the schema common to all the alternatives,
/// which states its own constraint ([`missing_required`], [`below_minimum`],
/// [`not_in_enum`]), and the format's rules ACROSS members are checked after the schema
/// and worded by ECR itself.
fn outside_every_rule_alternative() -> Fault {
    lifecycle_invalid(validator_report(format!(
        "instance failed to match exactly one schema (matched 0 out of {})",
        rule_alternatives()
    )))
}

/// `values` as a lifecycle policy DOCUMENT spells them: the modelled enums
/// (`TagStatus`, `ImageActionType`) are upper case, and a policy document writes them in
/// the lower case ECR's published lifecycle policy examples use — so a refusal that
/// lists the values a member admits lists them the way the document would have had to
/// write one.
fn document_spellings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| value.to_lowercase()).collect()
}

/// The member `name` of a lifecycle document object, or `None` when the document leaves
/// it out — an absent member and an explicit `null` are the same absence to a shape the
/// document is mapped onto.
fn document_member<'a>(object: &'a serde_json::Value, name: &str) -> Option<&'a serde_json::Value> {
    object.get(name).filter(|value| !value.is_null())
}

/// `name` read as a string. A member present with another JSON type cannot be mapped
/// onto the shape, which is a PARSING failure rather than a constraint one.
fn lifecycle_str<'a>(
    object: &'a serde_json::Value,
    name: &str,
) -> Result<Option<&'a str>, Fault> {
    match document_member(object, name) {
        None => Ok(None),
        Some(value) => value.as_str().map(Some).ok_or_else(lifecycle_unmappable),
    }
}

/// `name` read as an integer. See [`lifecycle_str`] for the type-mismatch rule; a
/// fractional number is not an integer the shape holds either.
fn lifecycle_i64(object: &serde_json::Value, name: &str) -> Result<Option<i64>, Fault> {
    match document_member(object, name) {
        None => Ok(None),
        Some(value) => value.as_i64().map(Some).ok_or_else(lifecycle_unmappable),
    }
}

/// `name` read as an array. See [`lifecycle_str`] for the type-mismatch rule.
fn lifecycle_array<'a>(
    object: &'a serde_json::Value,
    name: &str,
) -> Result<Option<&'a Vec<serde_json::Value>>, Fault> {
    match document_member(object, name) {
        None => Ok(None),
        Some(value) => value.as_array().map(Some).ok_or_else(lifecycle_unmappable),
    }
}

/// `name` read as a nested object. See [`lifecycle_str`] for the type-mismatch rule.
fn lifecycle_object<'a>(
    object: &'a serde_json::Value,
    name: &str,
) -> Result<Option<&'a serde_json::Value>, Fault> {
    match document_member(object, name) {
        None => Ok(None),
        Some(value) if value.is_object() => Ok(Some(value)),
        Some(_) => Err(lifecycle_unmappable()),
    }
}

/// A member the policy format requires, or the refusal for a document that leaves it
/// out.
fn lifecycle_required<T>(member: Option<T>, name: &str) -> Result<T, Fault> {
    member.ok_or_else(|| lifecycle_invalid(missing_required(name)))
}

/// One rule of a lifecycle policy document, checked against the lifecycle policy SCHEMA.
///
/// The schema states a rule in two parts, and which part a value belongs to is what
/// decides how its refusal is worded:
///
/// * the members every alternative form of a rule SHARES — `rulePriority`, and a
///   `selection`'s `tagStatus`, `countType`, `countNumber` — are checked against the
///   schema common to the alternatives, which states the constraint that failed
///   ([`missing_required`], [`below_minimum`], [`not_in_enum`]);
/// * the rule's FORM is then chosen among the alternatives
///   ([`rule_alternatives`]), and a rule in none of them is refused with the choice's own
///   outcome ([`outside_every_rule_alternative`]) — so the value it gave for a member the
///   choice is made on is not stated back to the caller.
///
/// A policy document writes `tagStatus`, `countType` and `action.type` in lower camel
/// case; the modelled enums are what they mean, so each is matched case-insensitively and
/// stored in the modelled spelling.
fn parse_lifecycle_rule(rule: &serde_json::Value) -> Result<LifecycleRule, Fault> {
    if !rule.is_object() {
        return Err(lifecycle_unmappable());
    }

    // --- the members the alternatives share ---
    let priority_number = lifecycle_required(lifecycle_i64(rule, "rulePriority")?, "rulePriority")?;
    let priority = match i32::try_from(priority_number) {
        // "rulePriority … The priority must be a positive value".
        Ok(priority) if priority >= 1 => priority,
        Ok(priority) => return Err(lifecycle_invalid(below_minimum(1, i64::from(priority)))),
        // Outside the range the shape's integer member holds at all, so the document
        // does not map onto it.
        Err(_) => return Err(lifecycle_unmappable()),
    };

    let selection = lifecycle_required(lifecycle_object(rule, "selection")?, "selection")?;
    let tag_status_text = lifecycle_required(lifecycle_str(selection, "tagStatus")?, "tagStatus")?;
    let tag_status = TAG_STATUS_VALUES
        .iter()
        .find(|value| value.eq_ignore_ascii_case(tag_status_text))
        .ok_or_else(|| {
            lifecycle_invalid(not_in_enum(
                tag_status_text,
                &document_spellings(TAG_STATUS_VALUES),
            ))
        })?;

    let string_list = |member: &str| -> Result<Vec<String>, Fault> {
        match lifecycle_array(selection, member)? {
            None => Ok(Vec::new()),
            Some(entries) => entries
                .iter()
                .map(|entry| {
                    entry
                        .as_str()
                        .map(ToString::to_string)
                        .ok_or_else(lifecycle_unmappable)
                })
                .collect(),
        }
    };
    let tag_prefixes = string_list("tagPrefixList")?;
    let tag_patterns = string_list("tagPatternList")?;

    let count_type_text = lifecycle_required(lifecycle_str(selection, "countType")?, "countType")?;
    let count_number = lifecycle_required(lifecycle_i64(selection, "countNumber")?, "countNumber")?;
    // The format's own minimum: an `imageCountMoreThan` keeps at least one image, and a
    // `sinceImagePushed` counts at least one day.
    if count_number < 1 {
        return Err(lifecycle_invalid(below_minimum(1, count_number)));
    }
    let count_unit = lifecycle_str(selection, "countUnit")?;

    let action = lifecycle_required(lifecycle_object(rule, "action")?, "action")?;
    let action_text = lifecycle_required(lifecycle_str(action, "type")?, "type")?;
    let target_text = lifecycle_str(action, "targetStorageClass")?;

    // --- the choice among the rule's alternative forms ---
    let count_type = LIFECYCLE_COUNT_TYPES
        .iter()
        .find(|value| value.eq_ignore_ascii_case(count_type_text))
        .ok_or_else(outside_every_rule_alternative)?;
    // Each count type fixes whether the rule carries a `countUnit`, and an age count
    // counts in the one unit the format defines for it.
    let count = if *count_type == IMAGE_COUNT_MORE_THAN_COUNT_TYPE {
        if count_unit.is_some() {
            return Err(outside_every_rule_alternative());
        }
        LifecycleCount::ImageCountMoreThan {
            keep: usize::try_from(count_number).unwrap_or(usize::MAX),
        }
    } else {
        let unit = count_unit.ok_or_else(outside_every_rule_alternative)?;
        if !unit.eq_ignore_ascii_case(COUNT_UNIT_DAYS) {
            return Err(outside_every_rule_alternative());
        }
        LifecycleCount::SinceImagePushed {
            days: u64::try_from(count_number).unwrap_or(u64::MAX),
        }
    };

    let action_type: &str = IMAGE_ACTION_TYPE_VALUES
        .iter()
        .find(|value| value.eq_ignore_ascii_case(action_text))
        .ok_or_else(outside_every_rule_alternative)?;
    // And each action type fixes whether the rule carries a target storage class.
    let target_storage_class = match (action_type, target_text) {
        (TRANSITION_ACTION_TYPE, Some(target)) => Some(
            LIFECYCLE_POLICY_TARGET_STORAGE_CLASS_VALUES
                .iter()
                .find(|value| value.eq_ignore_ascii_case(target))
                .ok_or_else(outside_every_rule_alternative)?
                .to_string(),
        ),
        (TRANSITION_ACTION_TYPE, None) | (_, Some(_)) => {
            return Err(outside_every_rule_alternative())
        }
        (_, None) => None,
    };

    Ok(LifecycleRule {
        priority,
        tag_status: (*tag_status).to_string(),
        tag_prefixes,
        tag_patterns,
        count,
        action_type: action_type.to_string(),
        target_storage_class,
    })
}

/// The format's rule ACROSS one rule's members about its tag filters: a rule selecting
/// TAGGED images names exactly one of the two tag lists, and a rule selecting anything
/// else names neither.
///
/// Checked once `rule` is through the schema and worded by ECR itself, so the sentence
/// names the `tagStatus` the rule asked for and carries no validator terminator.
fn tag_filters_stated(rule: &LifecycleRule) -> Result<(), Fault> {
    let tag_status = &rule.tag_status;
    let tagged = tag_status == TAGGED_TAG_STATUS;
    let names_tags = !rule.tag_prefixes.is_empty() || !rule.tag_patterns.is_empty();
    if tagged && !names_tags {
        return Err(lifecycle_invalid(format!(
            "Must specify tagPrefixList or tagPatternList when tagStatus={tag_status}."
        )));
    }
    if !rule.tag_prefixes.is_empty() && !rule.tag_patterns.is_empty() {
        return Err(lifecycle_invalid(format!(
            "Must specify only one of tagPrefixList or tagPatternList when \
             tagStatus={tag_status}."
        )));
    }
    if !tagged && names_tags {
        return Err(lifecycle_invalid(format!(
            "Must not specify tagPrefixList or tagPatternList when tagStatus={tag_status}."
        )));
    }
    Ok(())
}

/// The rules a lifecycle policy document declares, in the order they are applied, or
/// [`Fault::InvalidParameter`] stating the constraint the document failed.
///
/// Every check below is a documented requirement of the policy format: a `rules` array;
/// a unique `rulePriority` of 1 or more per rule; a `selection` naming a `TagStatus`, a
/// count type and a `countNumber` of 1 or more (the format's own minimum — an
/// `imageCountMoreThan` keeps at least one image, and a `sinceImagePushed` counts at
/// least one day); exactly one of the two tag lists on a rule selecting tagged images,
/// and neither anywhere else; `countUnit` on `sinceImagePushed` and only there; and an
/// `action` naming an `ImageActionType`, with a target storage class on a transition and
/// only there.
///
/// Those checks run in the order the SERVICE makes them, which is what decides which
/// constraint a document breaking several of them is refused with: the document's own
/// schema first (each rule against the alternatives the schema chooses between, see
/// [`parse_lifecycle_rule`]), then the format's rules ACROSS one rule's members
/// ([`tag_filters_stated`]), then the rules across the whole policy — a unique rule
/// priority and the evaluation order of a catch-all rule.
///
/// A refusal is worded by [`lifecycle_unmappable`] when the document cannot be mapped
/// onto the policy shape at all, and by [`lifecycle_invalid`] when it maps but breaks
/// one of those constraints — so no refusal carries a position in this emulator's parse
/// of the document.
pub fn parse_lifecycle_policy(text: &str) -> Result<Vec<LifecycleRule>, Fault> {
    let document: serde_json::Value =
        serde_json::from_str(text).map_err(|_| lifecycle_unmappable())?;
    // The document maps onto the LifecyclePolicy shape, which is an object; anything
    // else cannot be mapped at all.
    if !document.is_object() {
        return Err(lifecycle_unmappable());
    }
    let rules = lifecycle_required(lifecycle_array(&document, "rules")?, "rules")?;
    if rules.is_empty() {
        return Err(lifecycle_invalid(array_too_short(1, rules.len())));
    }

    // The schema check, rule by rule.
    let mut parsed: Vec<LifecycleRule> = Vec::new();
    for rule in rules {
        parsed.push(parse_lifecycle_rule(rule)?);
    }

    // Then the format's own rules across one rule's members.
    for rule in &parsed {
        tag_filters_stated(rule)?;
    }

    // A rule priority is unique within a policy, so each one is recorded as it is read.
    let mut seen_priorities: BTreeSet<i32> = BTreeSet::new();
    for rule in &parsed {
        if !seen_priorities.insert(rule.priority) {
            return Err(lifecycle_invalid(format!(
                "Duplicate rulePriority {}; a rulePriority is unique within a policy.",
                rule.priority
            )));
        }
    }

    // Rules are applied in ascending rule priority, whatever order the document lists
    // them in.
    parsed.sort_by_key(|rule| rule.priority);

    // "A lifecycle policy rule with a tagStatus value of any must have the highest value
    // for rulePriority and be evaluated last", so a catch-all rule is refused exactly
    // when another rule sorts after it. Checked once the rules are in applied order: the
    // last one is the one with the highest priority.
    if let Some(last) = parsed.last() {
        if let Some(catch_all) = parsed
            .iter()
            .find(|rule| rule.tag_status == ANY_TAG_STATUS && rule.priority < last.priority)
        {
            return Err(lifecycle_invalid(format!(
                "Must specify the highest rulePriority for the rule with \
                 tagStatus={ANY_TAG_STATUS}; rulePriority {} is evaluated before \
                 rulePriority {}.",
                catch_all.priority, last.priority
            )));
        }
    }

    Ok(parsed)
}

/// The images `rules` acts on, evaluated against `images` as they stand at `now` and
/// keyed by digest.
///
/// Rules are applied in ascending `rulePriority`, and an image the first matching rule
/// claims is not offered to a later one — so every previewed image carries exactly one
/// `appliedRulePriority`.
pub fn evaluate_lifecycle_policy(
    rules: &[LifecycleRule],
    images: &BTreeMap<String, ImageRecord>,
    now: SystemTime,
) -> BTreeMap<String, PreviewedImage> {
    let mut acted: BTreeMap<String, PreviewedImage> = BTreeMap::new();
    for rule in rules {
        let mut candidates: Vec<&ImageRecord> = images
            .values()
            .filter(|image| !acted.contains_key(&image.digest))
            .filter(|image| rule.selects(image))
            .collect();
        let claimed: Vec<&ImageRecord> = match rule.count {
            LifecycleCount::SinceImagePushed { days } => {
                let age = Duration::from_secs(days.saturating_mul(SECONDS_PER_DAY));
                candidates
                    .into_iter()
                    .filter(|image| {
                        now.duration_since(image.pushed_at).unwrap_or_default() > age
                    })
                    .collect()
            }
            LifecycleCount::ImageCountMoreThan { keep } => {
                // The newest `keep` images the selection matched stay; everything older
                // is what the rule acts on. The digest breaks a tie between two images
                // pushed at the same instant, so the choice is deterministic.
                candidates.sort_by(|left, right| {
                    right
                        .pushed_at
                        .cmp(&left.pushed_at)
                        .then_with(|| left.digest.cmp(&right.digest))
                });
                candidates.into_iter().skip(keep).collect()
            }
        };
        for image in claimed {
            acted.insert(
                image.digest.clone(),
                PreviewedImage {
                    digest: image.digest.clone(),
                    tags: image.tags.clone(),
                    pushed_at: image.pushed_at,
                    applied_rule_priority: rule.priority,
                    action_type: rule.action_type.clone(),
                    target_storage_class: rule.target_storage_class.clone(),
                    storage_class: storage_class_of_status(&image.status).to_string(),
                },
            );
        }
    }
    acted
}

/// One stored repository, with the identity fields computed once at creation so every
/// later read reports exactly what the create returned. The one field NOT stored is the
/// repository's URI: that names the endpoint this registry answers at rather than anything
/// about the repository, so every read composes it for the client it is answering
/// ([`Repo::to_repository`]).
#[derive(Debug, Clone)]
pub struct Repo {
    pub name: String,
    pub registry_id: String,
    pub arn: String,
    pub created_at: SystemTime,
    pub image_tag_mutability: String,
    pub image_tag_mutability_exclusion_filters: Vec<ImageTagMutabilityExclusionFilter>,
    /// Always present: `ImageScanningConfiguration$scanOnPush` is modelled
    /// `@default(false)` ("If this parameter is not specified, it will default to false"),
    /// so a repository created without one scans nothing on push rather than carrying no
    /// scanning configuration at all. Every repository therefore reports the member.
    pub image_scanning_config: ImageScanningConfiguration,
    pub encryption_config: EncryptionConfiguration,
    pub tags: Vec<Tag>,
    /// The repository policy, as the last `SetRepositoryPolicy` stored it — the
    /// submitted document in the canonical IAM-policy rendering ECR reports a policy in
    /// (see [`canonical_policy_document`]). `None`
    /// until a policy is set and again once `DeleteRepositoryPolicy` removes it, which
    /// is what `GetRepositoryPolicy` reports `RepositoryPolicyNotFoundException` for.
    /// It lives on the repository record, so deleting the repository drops its policy
    /// with it.
    pub policy_text: Option<String>,
    pub images: BTreeMap<String, ImageRecord>,
    pub tag_index: BTreeMap<String, String>,
    /// The blobs this repository holds, by digest: one entry per layer upload
    /// `CompleteLayerUpload` closed against it, carrying the digest the registry
    /// computed over the assembled bytes and how many bytes those were. A manifest
    /// cannot be pushed before every blob it references is in here, which is what
    /// `PutImage`'s `LayersNotFoundException` reports, and it is where
    /// `BatchCheckLayerAvailability` reads a layer's availability from. Blobs are
    /// per-repository: a layer uploaded to one repository is not a layer of another.
    /// The bytes themselves are not kept — nothing serves them yet — only the
    /// registry's record of the blob, which carries what reading those bytes identified
    /// in them ([`LayerBlob::filesystem`]).
    pub layer_blobs: BTreeMap<String, LayerBlob>,
    /// The repository's lifecycle policy, which `PutLifecyclePolicy` sets: the policy
    /// `StartLifecyclePolicyPreview` evaluates when the start call names none, and the
    /// document and evaluation instant `GetLifecyclePolicy` reports. `None` until a
    /// policy is put and again once `DeleteLifecyclePolicy` removes it, which is what the
    /// two of them report `LifecyclePolicyNotFoundException` for.
    pub lifecycle_policy: Option<StoredLifecyclePolicy>,
    /// The one lifecycle policy preview this repository carries; a later start replaces
    /// it.
    pub lifecycle_preview: Option<LifecyclePreview>,
}

impl Repo {
    /// The `Repository` every operation that reports this repository returns, announced at
    /// `registry_endpoint` — the endpoint this emulator serves the registry at for the
    /// calling client ([`announced_registry_endpoint`]).
    ///
    /// `repositoryUri` is composed here rather than stored on the record, because it is
    /// not a fact about the repository: it is the address this registry answers at, so
    /// every read announces the address the reader itself can reach.
    pub fn to_repository(&self, registry_endpoint: &str) -> Repository {
        Repository {
            repository_arn: Some(self.arn.clone()),
            registry_id: Some(self.registry_id.clone()),
            repository_name: Some(self.name.clone()),
            repository_uri: Some(repository_uri(registry_endpoint, &self.name)),
            created_at: Some(reported_instant(self.created_at)),
            image_tag_mutability: Some(self.image_tag_mutability.clone()),
            image_tag_mutability_exclusion_filters: if self
                .image_tag_mutability_exclusion_filters
                .is_empty()
            {
                None
            } else {
                Some(self.image_tag_mutability_exclusion_filters.clone())
            },
            image_scanning_configuration: Some(self.image_scanning_config.clone()),
            encryption_configuration: Some(self.encryption_config.clone()),
        }
    }

    /// The scanning configuration reported for this repository.
    ///
    /// Derived from what is stored, never from the request: `scanOnPush` is the flag the
    /// repository's own `imageScanningConfiguration` carries (modelled
    /// `@default(false)`, so a repository created without one is not scanned on push),
    /// and the frequency follows from it — a repository scanned on push has the
    /// `SCAN_ON_PUSH` frequency, and one that is not has the `MANUAL` one.
    /// `appliedScanFilters` lists the registry scanning rules' filters that match this
    /// repository; deriving that match from the registry configuration
    /// `PutRegistryScanningConfiguration` now stores belongs to this projection's own
    /// operation, so the list stays empty here.
    pub fn to_scanning_configuration(&self) -> RepositoryScanningConfiguration {
        let scan_on_push = self.image_scanning_config.scan_on_push;
        RepositoryScanningConfiguration {
            repository_arn: Some(self.arn.clone()),
            repository_name: Some(self.name.clone()),
            scan_on_push,
            scan_frequency: Some(
                if scan_on_push {
                    SCAN_ON_PUSH_FREQUENCY
                } else {
                    MANUAL_SCAN_FREQUENCY
                }
                .to_string(),
            ),
            applied_scan_filters: Some(Vec::new()),
        }
    }

    /// Whether `tag` may be moved from the image it points at to another one.
    ///
    /// The repository's `imageTagMutability` decides it, and the two
    /// `*_WITH_EXCLUSION` settings invert that decision for a tag matching any
    /// `imageTagMutabilityExclusionFilters` entry — the filters name the tags
    /// "excluded from the repository's image tag mutability setting".
    pub fn tag_is_mutable(&self, tag: &str) -> bool {
        let excluded = self
            .image_tag_mutability_exclusion_filters
            .iter()
            .any(|f| wildcard_matches(&f.filter, tag));
        match self.image_tag_mutability.as_str() {
            "IMMUTABLE" => false,
            "MUTABLE_WITH_EXCLUSION" => !excluded,
            "IMMUTABLE_WITH_EXCLUSION" => excluded,
            // MUTABLE, and anything the model widens the enum with later.
            _ => true,
        }
    }

    /// The digest `tag` currently points at.
    pub fn digest_of_tag(&self, tag: &str) -> Option<&String> {
        self.tag_index.get(tag)
    }

    /// The stored image an `imageIds` entry names, or why it names none.
    ///
    /// An entry carries a digest, a tag, or both, and it names an image only when this
    /// repository holds one that answers to every part of it: a digest addresses the
    /// manifest of that content, a tag addresses whatever the tag index points at, and
    /// an entry carrying both resolves only when they agree. A digest that is not a
    /// content digest at all addresses nothing, and is reported as that rather than as
    /// an absent image.
    ///
    /// This is the one place an `imageId` is resolved: a batch operation reports the
    /// failure per entry, and [`Self::resolve_image`] lifts it onto a [`Fault`] for an
    /// operation that refuses the whole call.
    pub fn lookup_image(&self, id: &ImageIdentifier) -> Result<&ImageRecord, ImageLookupFailure> {
        if id
            .image_digest
            .as_deref()
            // The same `<algorithm>:<hex>` form a layer digest has: a content digest is
            // one shape, whatever it addresses.
            .is_some_and(|digest| !is_layer_digest(digest))
        {
            return Err(ImageLookupFailure::InvalidImageDigest);
        }
        match (id.image_digest.as_deref(), id.image_tag.as_deref()) {
            (None, None) => Err(ImageLookupFailure::MissingDigestAndTag),
            (Some(digest), None) => self
                .images
                .get(digest)
                .ok_or(ImageLookupFailure::ImageNotFound),
            (None, Some(tag)) => self
                .digest_of_tag(tag)
                .and_then(|digest| self.images.get(digest))
                .ok_or(ImageLookupFailure::ImageNotFound),
            (Some(digest), Some(tag)) => {
                let image = self
                    .images
                    .get(digest)
                    .ok_or(ImageLookupFailure::ImageNotFound)?;
                match self.digest_of_tag(tag) {
                    // The tag is on this very image, so both parts agree.
                    Some(tagged) if tagged == digest => Ok(image),
                    // The tag names another stored image: the two parts disagree.
                    Some(_) => Err(ImageLookupFailure::ImageTagDoesNotMatchDigest),
                    // Nothing in the repository carries the tag at all.
                    None => Err(ImageLookupFailure::ImageNotFound),
                }
            }
        }
    }

    /// The stored image an [`ImageIdentifier`] names, or the [`Fault`] it resolves to.
    ///
    /// The resolution is [`Self::lookup_image`]'s; what this adds is the mapping onto
    /// the faults an operation that refuses the WHOLE call reports, so every such
    /// operation refuses the same identifier the same way. An identifier that names no
    /// stored image is an image fault; one that is malformed, or that names neither a
    /// digest nor a tag, is a parameter fault.
    pub fn resolve_image(&self, id: &ImageIdentifier) -> Result<&ImageRecord, Fault> {
        // Matched exhaustively, so a lookup failure added later is a compile error here
        // rather than silently reported as an absent image.
        match self.lookup_image(id) {
            Ok(stored) => Ok(stored),
            Err(ImageLookupFailure::MissingDigestAndTag) => Err(Fault::InvalidParameter(
                "an image identifier must carry an imageDigest, an imageTag, or both".to_string(),
            )),
            Err(ImageLookupFailure::InvalidImageDigest) => Err(Fault::InvalidParameter(format!(
                "Invalid parameter at 'imageIds' failed to satisfy constraint: imageDigest \
                 '{}' must be of the form <algorithm>:<hex digest>",
                id.image_digest.as_deref().unwrap_or_default()
            ))),
            Err(
                ImageLookupFailure::ImageNotFound
                | ImageLookupFailure::ImageTagDoesNotMatchDigest,
            ) => Err(Fault::ImageNotFound(not_within_repository(
                &format!("image with {}", identifier_description(id)),
                &self.name,
                &self.registry_id,
            ))),
        }
    }

    /// The stored image an [`ImageIdentifier`] names, FOR MUTATION, or the [`Fault`] it
    /// resolves to.
    ///
    /// The resolution is [`Self::resolve_image`]'s, so an operation that WRITES to an
    /// image names it exactly as a read does; what this adds is the record to write to,
    /// so no caller has to look the digest up a second time (and no caller has to invent
    /// a failure for a second lookup that cannot miss).
    pub fn resolve_image_mut(
        &mut self,
        id: &ImageIdentifier,
    ) -> Result<&mut ImageRecord, Fault> {
        // Resolved first, then taken mutably: the digest is the stored record's own, so
        // the second lookup is the same image and cannot miss. It is still written as a
        // lookup rather than an unwrap, so a store that changes under this call answers
        // the caller's own not-found rather than panicking; nothing of the repository is
        // cloned to word it, because that wording is never reached.
        let digest = self.resolve_image(id)?.digest.clone();
        self.images.get_mut(&digest).ok_or_else(|| {
            Fault::ImageNotFound(format!(
                "The image with {} does not exist within the repository",
                identifier_description(id)
            ))
        })
    }

    /// Every blob this repository holds, by digest, WITHOUT the blobs' bytes.
    ///
    /// A blob is in a repository because a layer upload against that repository was
    /// COMPLETED — `CompleteLayerUpload` is what registers one, under the digest the
    /// registry computed over the bytes it assembled — so [`Self::layer_blobs`] is the
    /// record of what the repository holds, and the byte count it carries is the
    /// registry's own count rather than anything a manifest declares. What a manifest
    /// adds is the blob's MEDIA TYPE, which only a descriptor referencing it states (a
    /// V2 schema 1 `fsLayers` entry states not even that), so it is filled in here from
    /// whichever stored manifest knows it. Blobs are per-repository: a layer uploaded to
    /// one repository is not a layer of another, which is why this is a method on the
    /// repository.
    ///
    /// [`LayerBlob::content`] is left EMPTY in the copies this answers with: every reader
    /// of it reports ON a layer — its digest, its size, its media type, its availability
    /// — rather than serving it, and one layer runs to [`MAX_LAYER_UPLOAD_BYTES`], so
    /// copying the content per read would let one small request allocate every byte the
    /// repository holds. The download plane reads [`Self::layer_blobs`] itself.
    pub fn layers(&self) -> BTreeMap<String, LayerBlob> {
        let mut layers: BTreeMap<String, LayerBlob> = self
            .layer_blobs
            .iter()
            .map(|(digest, blob)| {
                (
                    digest.clone(),
                    LayerBlob {
                        digest: blob.digest.clone(),
                        size: blob.size,
                        media_type: blob.media_type.clone(),
                        filesystem: blob.filesystem.clone(),
                        content: Vec::new(),
                    },
                )
            })
            .collect();
        for image in self.images.values() {
            for blob in manifest_blobs(&image.manifest) {
                // A descriptor for a blob the repository does not hold describes
                // nothing here: only a held blob is a layer of this repository.
                if let Entry::Occupied(mut held) = layers.entry(blob.digest.clone()) {
                    let record = held.get_mut();
                    if record.media_type.is_none() {
                        record.media_type = blob.media_type;
                    }
                }
            }
        }
        layers
    }

    /// The digests `manifest` references that this repository does not hold, each once
    /// and in the order [`manifest_blobs`] reads them: the image's layers, then its
    /// configuration blob.
    ///
    /// Empty means every blob the manifest references is in the repository, so the
    /// manifest may be stored; anything else is what `PutImage`'s
    /// `LayersNotFoundException` names. The same blob referenced twice is one missing
    /// blob, so it is named once. A document that is not a manifest at all references no
    /// blob ([`manifest_blobs`]), and so names no missing one.
    ///
    /// The digests already named are tracked in a SET beside the ordered list rather than
    /// by scanning the list: `imageManifest` is modelled `@length(1..4194304)`, so one
    /// request can reference on the order of a hundred thousand descriptors, and a linear
    /// scan per descriptor would make the work quadratic in a number the caller chooses.
    pub fn missing_layers(&self, manifest: &str) -> Vec<String> {
        let mut missing: Vec<String> = Vec::new();
        // The digests already named, as a SET: a document naming one blob many times
        // costs its length rather than its length squared.
        let mut named: BTreeSet<String> = BTreeSet::new();
        for blob in manifest_blobs(manifest) {
            if !self.layer_blobs.contains_key(&blob.digest) && named.insert(blob.digest.clone()) {
                missing.push(blob.digest);
            }
        }
        missing
    }

    /// Record the basic image scan a `StartImageScan` at `now` starts on the image `id`
    /// names, answering the `ImageScanStatus` that start reports.
    ///
    /// Where the run ENDS is decided here, once, from the image it was started on and
    /// this repository's own records of the blobs that image is built from
    /// ([`basic_scan_outcome`]), and stored on the image together with what the run found:
    /// reading the scan back afterwards reports THIS run rather than evaluating a fresh
    /// one, so every reader derives the same answer and no read has to write. The status
    /// answered is the stored record's own projection, so the start and a read straight
    /// after it report the same scan.
    pub fn start_basic_scan(
        &mut self,
        id: &ImageIdentifier,
        now: SystemTime,
    ) -> Result<ImageScanStatus, Fault> {
        let image = self.resolve_image(id)?;
        let terminal = basic_scan_outcome(image, &self.layer_blobs);
        let findings = basic_scan_findings(image, &self.layer_blobs);
        let scan = ImageScan {
            started_at: now,
            complete_due_at: now + IMAGE_SCAN_DELAY,
            terminal,
            findings,
        };
        let status = scan.to_status(now);
        self.resolve_image_mut(id)?.scan = Some(scan);
        Ok(status)
    }

    /// Materialise every image's clock-owed storage-class transition at `now` (see
    /// [`ImageRecord::settle_storage_class`]), so a read reports what was already due
    /// before it looked.
    pub fn settle_image_storage_classes(&mut self, now: SystemTime) {
        for image in self.images.values_mut() {
            image.settle_storage_class(now);
        }
    }

    /// Recompute the `imageSizeInBytes` every image in this repository reports.
    ///
    /// `ImageDetail$imageSizeInBytes` is "the size, in bytes, of the image in the
    /// repository": the total of the sizes of the DISTINCT blobs the image's manifest
    /// references — its filesystem layers and its image configuration — which is the
    /// content this repository stores for that image, never the size of the manifest
    /// document itself. A blob a manifest names twice counts once. "If the image is a
    /// manifest list, this will be the max size of all manifests in the list", so a list
    /// reports the largest of the images it references, each child's size being its own
    /// blob total.
    ///
    /// A list's size is a statement about other images in the repository, so it is
    /// recomputed whenever the image set changes — a push or a delete — rather than
    /// frozen when the list itself arrived, and every read then projects
    /// [`ImageRecord::size_in_bytes`] in constant time. The recomputation reads no
    /// manifest document (each one was parsed once, into [`ImageRecord::references`]) and
    /// its work is the product of three documented bounds:
    /// [`MAX_IMAGES_PER_REPOSITORY`] entries, each resolving at most
    /// [`MAX_MANIFEST_LIST_ENTRIES`] children (or [`MAX_MANIFEST_BLOB_ENTRIES`] blobs),
    /// over at most [`MAX_MANIFEST_LIST_DEPTH`] levels of nesting.
    ///
    /// A size is `None` for an image nothing stored states one for: the member is neither
    /// required nor defaulted, so an unknown size is absent rather than zero. That covers
    /// a document that references no blob and no manifest at all, a blob whose size
    /// neither its descriptor nor this repository's record of it states, a total no `long`
    /// can represent — the addends are caller-supplied, so they are added with checked
    /// arithmetic and never wrapped — and a list none of whose children the repository
    /// holds. An image manifest with an EMPTY `layers` array and no configuration blob is
    /// a statement, not a gap: it references no content, so its total is `0`.
    ///
    /// What a blob's size is read from: the size its descriptor declares, else the byte
    /// count this repository recorded when the layer upload was completed
    /// ([`Self::layer_blobs`]) — a manifest form that states no size (a V2 schema 1
    /// `fsLayers` entry, a descriptor without `size`) still references content the
    /// registry has counted.
    pub fn refresh_image_sizes(&mut self) {
        // An image manifest's total rests on its own descriptors and on the blobs this
        // repository holds for them.
        let mut sizes: BTreeMap<String, Option<i64>> = self
            .images
            .iter()
            .map(|(digest, image)| {
                let own = match &image.references {
                    ManifestContent::Blobs(blobs) => blobs.iter().try_fold(0i64, |total, blob| {
                        let size = blob.size.or_else(|| {
                            self.layer_blobs.get(&blob.digest).and_then(|held| held.size)
                        })?;
                        total.checked_add(size)
                    }),
                    ManifestContent::List(_) | ManifestContent::Opaque => None,
                };
                (digest.clone(), own)
            })
            .collect();

        // A manifest list is the max over the images it names, so a list of lists settles
        // one level per pass; a pass that changes nothing has settled.
        for _ in 0..MAX_MANIFEST_LIST_DEPTH {
            let mut settled = true;
            let mut next = sizes.clone();
            for (digest, image) in &self.images {
                if let ManifestContent::List(children) = &image.references {
                    let resolved = children
                        .iter()
                        .filter_map(|child| sizes.get(child).copied().flatten())
                        .max();
                    if next.insert(digest.clone(), resolved) != Some(resolved) {
                        settled = false;
                    }
                }
            }
            sizes = next;
            if settled {
                break;
            }
        }

        for (digest, size) in sizes {
            if let Some(image) = self.images.get_mut(&digest) {
                image.size_in_bytes = size;
            }
        }
    }

    /// Apply `incoming` to this repository's metadata tags: a key the repository already
    /// carries takes the new value where it stands, a key it does not carry is appended.
    /// "Existing tags on a resource are not changed if they are not specified in the
    /// request parameters" (`TagResource`), so nothing else is touched.
    pub fn merge_tags(&mut self, incoming: Vec<Tag>) {
        for tag in incoming {
            match self.tags.iter_mut().find(|held| held.key == tag.key) {
                Some(held) => held.value = tag.value,
                None => self.tags.push(tag),
            }
        }
    }

    /// Remove every metadata tag whose key `keys` names. A key the repository does not
    /// carry removes nothing: `UntagResource` names the keys to be removed, not an
    /// assertion that they are held.
    pub fn remove_tag_keys(&mut self, keys: &[String]) {
        self.tags.retain(|held| !keys.contains(&held.key));
    }

    /// Attach `tag` to `digest`, detaching it from whatever held it before, so the tag
    /// index and the images' own tag sets never disagree.
    pub fn move_tag(&mut self, tag: &str, digest: &str) {
        if let Some(previous) = self.tag_index.get(tag).cloned() {
            if let Some(image) = self.images.get_mut(&previous) {
                image.tags.remove(tag);
            }
        }
        self.tag_index.insert(tag.to_string(), digest.to_string());
        if let Some(image) = self.images.get_mut(digest) {
            image.tags.insert(tag.to_string());
        }
    }

    /// Materialise the lifecycle policy preview completion the clock has already made
    /// due at `now`.
    ///
    /// The transition is owed by the clock, not by a call: it falls due
    /// [`LIFECYCLE_PREVIEW_DELAY`] after the start whether or not anybody asks, so every
    /// operation that looks at the preview writes down what was already due before
    /// reading it.
    pub fn settle_lifecycle_preview(&mut self, now: SystemTime) {
        let Some(preview) = self.lifecycle_preview.as_mut() else {
            return;
        };
        let Some(due) = preview.complete_due_at else {
            return;
        };
        if due <= now {
            preview.status = PREVIEW_STATUS_COMPLETE.to_string();
            preview.complete_due_at = None;
        }
    }

    /// Whether a preview started on this repository is still running.
    pub fn lifecycle_preview_in_progress(&self) -> bool {
        self.lifecycle_preview
            .as_ref()
            .is_some_and(|preview| preview.status == PREVIEW_STATUS_IN_PROGRESS)
    }
}

/// One stored repository creation template: the settings a later repository creation in
/// this namespace prefix is to be given, exactly as the create or the update left them.
#[derive(Debug, Clone)]
pub struct CreationTemplate {
    pub prefix: String,
    pub description: Option<String>,
    /// Always present, the way a repository's own [`Repo::encryption_config`] is:
    /// `EncryptionConfigurationForRepositoryCreationTemplate$encryptionType` is a
    /// REQUIRED member, so a template carries a resolved encryption posture from the
    /// moment it is created — the configuration the create named, or
    /// [`default_template_encryption`] when it named none. Every operation that reports
    /// the template therefore reports the member.
    pub encryption_config: EncryptionConfigurationForRepositoryCreationTemplate,
    pub resource_tags: Vec<Tag>,
    pub image_tag_mutability: String,
    pub image_tag_mutability_exclusion_filters: Vec<ImageTagMutabilityExclusionFilter>,
    pub repository_policy: Option<String>,
    pub lifecycle_policy: Option<String>,
    pub applied_for: Vec<String>,
    pub custom_role_arn: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

/// The encryption configuration a repository creation template created without one
/// resolves to: the service default encryption type, with no caller key beside it (the
/// S3-managed type takes none).
pub fn default_template_encryption() -> EncryptionConfigurationForRepositoryCreationTemplate {
    EncryptionConfigurationForRepositoryCreationTemplate {
        encryption_type: DEFAULT_ENCRYPTION_TYPE.to_string(),
        kms_key: None,
    }
}

impl CreationTemplate {
    /// The `RepositoryCreationTemplate` every operation that reports this template
    /// returns, derived from the stored record alone.
    pub fn to_template(&self) -> RepositoryCreationTemplate {
        RepositoryCreationTemplate {
            prefix: Some(self.prefix.clone()),
            description: self.description.clone(),
            encryption_configuration: Some(self.encryption_config.clone()),
            // An empty list is reported as an absent member: nothing was applied.
            resource_tags: if self.resource_tags.is_empty() {
                None
            } else {
                Some(self.resource_tags.clone())
            },
            image_tag_mutability: Some(self.image_tag_mutability.clone()),
            image_tag_mutability_exclusion_filters: if self
                .image_tag_mutability_exclusion_filters
                .is_empty()
            {
                None
            } else {
                Some(self.image_tag_mutability_exclusion_filters.clone())
            },
            repository_policy: self.repository_policy.clone(),
            lifecycle_policy: self.lifecycle_policy.clone(),
            applied_for: Some(self.applied_for.clone()),
            custom_role_arn: self.custom_role_arn.clone(),
            created_at: Some(reported_instant(self.created_at)),
            updated_at: Some(reported_instant(self.updated_at)),
        }
    }
}

/// `upstreamRepositoryPrefix`'s documented default: "When this field isn't specified,
/// Amazon ECR will use the ROOT."
pub const DEFAULT_UPSTREAM_REPOSITORY_PREFIX: &str = "ROOT";

/// The upstream registries whose URL the `upstreamRegistryUrl` documentation gives as
/// one fixed host, as `(registry, host)` pairs. The two it gives with a caller-chosen
/// label — Amazon ECR (`<accountId>.dkr.ecr.<region>.amazonaws.com`) and Microsoft Azure
/// Container Registry (`<custom>.azurecr.io`) — are matched by shape in
/// [`upstream_registry_for_url`] instead, and `chainguard`, whose URL that
/// documentation does not give, is named only by the caller.
pub const DOCUMENTED_UPSTREAM_REGISTRY_URLS: [(&str, &str); 6] = [
    ("ecr-public", "public.ecr.aws"),
    ("docker-hub", "registry-1.docker.io"),
    ("github-container-registry", "ghcr.io"),
    ("gitlab-container-registry", "registry.gitlab.com"),
    ("k8s", "registry.k8s.io"),
    ("quay", "quay.io"),
];

/// The `UpstreamRegistry` a URL names, from the per-registry URL syntax the
/// `upstreamRegistryUrl` member documents, or `None` when the URL matches none of them.
pub fn upstream_registry_for_url(url: &str) -> Option<&'static str> {
    let host = url.trim().trim_end_matches('/');
    if let Some((registry, _)) = DOCUMENTED_UPSTREAM_REGISTRY_URLS
        .iter()
        .find(|(_, documented)| host.eq_ignore_ascii_case(documented))
    {
        return Some(registry);
    }
    let lower = host.to_ascii_lowercase();
    // `<custom>.azurecr.io` and `<accountId>.dkr.ecr.<region>.amazonaws.com`: the label
    // is the caller's, the rest of the host is the registry's, so each is matched by its
    // documented label arrangement rather than by a substring.
    if lower
        .strip_suffix(".azurecr.io")
        .is_some_and(|custom| !custom.is_empty())
    {
        return Some("azure-container-registry");
    }
    if is_ecr_registry_host(&lower) {
        return Some("ecr");
    }
    None
}

/// The account id and the Region an `<accountId>.dkr.ecr.<region>.amazonaws.com` host
/// names, as the `upstreamRegistryUrl` doc spells Amazon ECR's: an account id, the two
/// fixed labels, one Region label, and nothing else. `host` is already lowercased, and
/// `None` for every host of another shape.
///
/// The ONE reading of that host layout: the `upstreamRegistryUrl` resolution above asks
/// whether a URL names Amazon ECR at all ([`is_ecr_registry_host`]), and a PULL asks which
/// Region and account the reference it carries addresses
/// ([`registry_pull::parse_reference`]), so neither walks the labels itself.
pub fn ecr_registry_host_parts(host: &str) -> Option<(&str, &str)> {
    let rest = host.strip_suffix(".amazonaws.com")?;
    let mut labels = rest.split('.');
    let account = labels.next().filter(|account| {
        account.len() == ACCOUNT_ID_LEN && account.bytes().all(|byte| byte.is_ascii_digit())
    })?;
    if labels.next() != Some("dkr") || labels.next() != Some("ecr") {
        return None;
    }
    let region = labels.next().filter(|region| !region.is_empty())?;
    labels.next().is_none().then_some((account, region))
}

/// Whether `host` is an `<accountId>.dkr.ecr.<region>.amazonaws.com` registry host of ANY
/// account, which is what makes an `upstreamRegistryUrl` name Amazon ECR.
fn is_ecr_registry_host(host: &str) -> bool {
    ecr_registry_host_parts(host).is_some()
}

/// The length of an Amazon Web Services account id, which the `registryId` member's own
/// pattern gives as `^[0-9]{12}$`.
const ACCOUNT_ID_LEN: usize = 12;

/// Per-service state for com-amazonaws-ecr-amazonec2containerregistry-v20150921. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// Repositories by name. A `BTreeMap` so every listing — and therefore every
    /// pagination token — has one deterministic order.
    pub repositories: BTreeMap<String, Repo>,
    /// This registry's pull through cache rules, by `ecrRepositoryPrefix` — the key AWS
    /// addresses a rule by, as [`normalize_prefix`] spells it. `CreatePullThroughCacheRule`
    /// is the only operation that adds one; `UpdatePullThroughCacheRule`,
    /// `ValidatePullThroughCacheRule` and `DeletePullThroughCacheRule` resolve a prefix
    /// here rather than each keeping their own copy.
    pub pull_through_cache_rules: BTreeMap<String, PullThroughCacheRule>,
    /// The account settings a `PutAccountSetting` has changed, by setting name. A name
    /// absent from here is still readable — it reports its spec's
    /// [`AccountSettingSpec::default_value`] — so a read never has to invent a value
    /// and never has to write one.
    pub account_settings: BTreeMap<String, String>,
    /// Repository creation templates by namespace prefix, keyed the way
    /// [`normalize_prefix`] spells that prefix. A `BTreeMap` for the same reason as
    /// `repositories`: one deterministic listing order, so a pagination token means the
    /// same thing on the next call.
    pub creation_templates: BTreeMap<String, CreationTemplate>,
    /// The registry's replication rules, exactly as the last
    /// `PutReplicationConfiguration` persisted them, in the order that call listed
    /// them. Empty until one is made: a registry nobody has configured replication for
    /// reports an empty rule list, which is what `DescribeRegistry` answers with.
    ///
    /// Registry-wide, like the single registry this store models: a put made against
    /// one Region's endpoint is the configuration every later read reports, while the
    /// "a registry cannot replicate to itself" refusal is evaluated per call, against
    /// the Region that call was made against.
    pub replication_rules: Vec<ReplicationRule>,
    /// The registry permissions policy, as `PutRegistryPolicy` stored it: the submitted
    /// document in the canonical IAM-policy rendering ECR reports a policy in (see
    /// [`canonical_policy_document`]), so the read and the delete report the same
    /// document the put answered with. `None` is "this registry has no associated
    /// registry policy", which the registry-policy read and delete answer with
    /// `RegistryPolicyNotFoundException`. One registry is served here, so there is at
    /// most one such policy.
    pub registry_policy: Option<String>,
    /// The registry's signing configuration, which `PutSigningConfiguration` creates or
    /// replaces and `GetSigningConfiguration` reports. There is at most one per registry
    /// and this emulator serves one registry, so it is a single optional record rather
    /// than a map: `None` is a registry that has none, which is what
    /// `SigningConfigurationNotFoundException` reports.
    pub signing_configuration: Option<SigningConfiguration>,
    /// The registry's pull time update exclusion list: the IAM principal ARN of every
    /// principal whose image pulls ECR does not record a pull time for, mapped to when
    /// its exclusion was registered. Keyed by the ARN — one exclusion per principal is
    /// what makes a second register of the same principal a duplicate — and a
    /// `BTreeMap` so listing and its pagination tokens have one deterministic order.
    pub pull_time_update_exclusions: BTreeMap<String, SystemTime>,
    /// In-flight layer uploads by upload id. Registry-wide, because an upload id is
    /// what `UploadLayerPart` and `CompleteLayerUpload` address an upload by; each
    /// record names the repository it belongs to, and a call naming a repository that
    /// no longer exists is refused by the repository lookup before the upload is ever
    /// reached. An upload opened against a repository later deleted is therefore inert,
    /// but still counts against [`MAX_LAYER_UPLOADS`] until the process ends.
    pub uploads: BTreeMap<String, LayerUpload>,
    /// The layer download URLs this registry has handed out, by the opaque token each
    /// URL carries. Registry-wide, because a token is what a fetch addresses a grant by;
    /// each record names the repository and the layer it serves. A `BTreeMap` for the
    /// same deterministic order as the maps above, and bounded by
    /// [`MAX_LAYER_DOWNLOAD_GRANTS`].
    pub layer_download_grants: BTreeMap<String, LayerDownloadGrant>,
    /// The registry's scanning configuration, exactly as the last
    /// `PutRegistryScanningConfiguration` stored it. `None` until one is stored, which is
    /// the state the documented default describes — see
    /// [`registry_scanning_configuration`](ComAmazonawsEcrAmazonec2containerregistryV20150921State::registry_scanning_configuration).
    pub registry_scanning: Option<crate::ops::types::RegistryScanningConfiguration>,
    /// The `docker login` credentials `GetAuthorizationToken` has issued, by the content
    /// digest of each password: what the Docker Registry v2 data plane authenticates a
    /// pull against. Registry-wide, because such a credential reaches every repository
    /// the registry holds, and a `BTreeMap` for the same deterministic order as the maps
    /// above; bounded by [`MAX_REGISTRY_CREDENTIALS`]. See `state/registry_v2.rs` for why
    /// the password itself is not here.
    pub registry_credentials: BTreeMap<String, RegistryCredential>,
}

impl ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// The repository `name`, or [`Fault::RepositoryNotFound`].
    pub fn repository(&self, name: &str) -> Result<&Repo, Fault> {
        self.repositories
            .get(name)
            .ok_or_else(|| Fault::RepositoryNotFound(repository_not_found_message(name)))
    }

    /// The repository `name` for mutation, or [`Fault::RepositoryNotFound`].
    pub fn repository_mut(&mut self, name: &str) -> Result<&mut Repo, Fault> {
        self.repositories
            .get_mut(name)
            .ok_or_else(|| Fault::RepositoryNotFound(repository_not_found_message(name)))
    }

    /// The in-flight upload `upload_id` addresses, for mutation, or `None` when this
    /// registry holds no such upload OR holds one that was opened against another
    /// repository: an upload id is only valid for the repository it was issued for,
    /// which is exactly what `UploadNotFoundException` reports ("the upload could not
    /// be found, or the specified upload ID is not valid for this repository").
    pub fn upload_mut(
        &mut self,
        repository_name: &str,
        upload_id: &str,
    ) -> Option<&mut LayerUpload> {
        self.uploads
            .get_mut(upload_id)
            .filter(|upload| upload.repository_name == repository_name)
    }

    /// Drop every layer download grant whose window has closed as of `now`.
    ///
    /// Expiry is the clock's, not a caller's: a grant past [`LAYER_DOWNLOAD_URL_TTL`]
    /// serves nothing whether or not anything has looked at it, so dropping it changes
    /// nothing a later caller can observe and keeps the table from growing for the life
    /// of the process. Called by `GetDownloadUrlForLayer` before it mints another.
    pub fn reclaim_expired_layer_download_grants(&mut self, now: SystemTime) {
        self.layer_download_grants
            .retain(|_, grant| !grant.has_expired(now));
    }

    /// Mint one layer download grant for `layer_digest` of `repository_name` at `now`,
    /// keep it, and answer the token addressing it with the grant itself.
    ///
    /// `None` when the registry already holds [`MAX_LAYER_DOWNLOAD_GRANTS`] live grants
    /// and none has expired, which is the one case the caller refuses rather than
    /// answering a URL. Expired grants are reclaimed first, so a client that keeps
    /// pulling is bounded by how many URLs it asks for within one window rather than by
    /// how many it has ever asked for.
    pub fn issue_layer_download_grant(
        &mut self,
        repository_name: &str,
        layer_digest: &str,
        now: SystemTime,
    ) -> Option<(String, LayerDownloadGrant)> {
        self.reclaim_expired_layer_download_grants(now);
        if self.layer_download_grants.len() >= MAX_LAYER_DOWNLOAD_GRANTS {
            return None;
        }
        let grant = LayerDownloadGrant::issued(repository_name, layer_digest, now);
        let token = mint_hex(LAYER_DOWNLOAD_TOKEN_BYTES);
        self.layer_download_grants
            .insert(token.clone(), grant.clone());
        Some((token, grant))
    }

    /// The grant `token` addresses, whether or not its window is still open: whether it
    /// has expired is the FETCH's question, because an expired URL and a URL this
    /// registry never handed out are not the same answer.
    pub fn layer_download_grant(&self, token: &str) -> Option<&LayerDownloadGrant> {
        self.layer_download_grants.get(token)
    }

    /// The rule registered for `prefix`, whichever way its trailing slash is written, or
    /// `None` when the registry holds no rule for it.
    pub fn pull_through_cache_rule(&self, prefix: &str) -> Option<&PullThroughCacheRule> {
        self.pull_through_cache_rules
            .get(&normalize_prefix(prefix))
    }

    /// The caller-supplied BYTES this registry currently holds: every in-flight upload's
    /// received bytes, every registered layer blob's bytes, and every stored image
    /// manifest.
    ///
    /// The one accounting behind [`MAX_REGISTRY_CALLER_BYTES`], so the two paths that
    /// grow the store — `UploadLayerPart` and `PutImage` — are held to one aggregate
    /// rather than to their own per-record caps alone. A registered blob's bytes are
    /// counted here as well as an in-flight upload's ([`LayerBlob::content`]): closing an
    /// upload moves the same bytes from one record to the other, so a client cannot free
    /// the aggregate by completing uploads and keep streaming new ones.
    ///
    /// Summed on demand: every map is bounded by its entry cap, so this is a walk of at
    /// most a few thousand lengths and no second copy of the figure can drift from the
    /// bytes actually held.
    pub fn caller_bytes_held(&self) -> usize {
        let upload_bytes = self
            .uploads
            .values()
            .fold(0usize, |held, upload| {
                held.saturating_add(upload.received_bytes.len())
            });
        self.repositories.values().fold(upload_bytes, |held, repo| {
            let with_blobs = repo.layer_blobs.values().fold(held, |held, blob| {
                held.saturating_add(blob.content.len())
            });
            repo.images.values().fold(with_blobs, |held, image| {
                held.saturating_add(image.manifest.len())
            })
        })
    }

    /// Whether `additional` more caller-supplied bytes fit under
    /// [`MAX_REGISTRY_CALLER_BYTES`]. A caller that asks for more is refused before any
    /// byte of its payload is stored.
    pub fn has_byte_budget_for(&self, additional: usize) -> bool {
        self.caller_bytes_held().saturating_add(additional) <= MAX_REGISTRY_CALLER_BYTES
    }

    /// How many basic image scans this registry has started within `interval` of `now`:
    /// the figure [`MAX_BASIC_SCANS_PER_INTERVAL`] bounds.
    ///
    /// Counted from the scans the registry holds rather than from a counter beside them,
    /// so the budget can never disagree with the scans actually recorded: an image
    /// deleted with its scan stops counting, exactly as its scan stops being readable.
    pub fn basic_scans_started_within(&self, interval: Duration, now: SystemTime) -> usize {
        self.repositories.values().fold(0usize, |started, repo| {
            repo.images.values().fold(started, |started, image| {
                let counted = image
                    .scan
                    .as_ref()
                    .is_some_and(|scan| scan.started_within(interval, now));
                started.saturating_add(usize::from(counted))
            })
        })
    }

    /// The copies a push of an image into the repository named `repository_name` starts:
    /// one per destination of every stored replication rule whose repository filters match
    /// that repository, as of `now`.
    ///
    /// Which images are copied is decided by the registry's replication configuration:
    /// "Specifying a repository filter for a replication rule provides a method for
    /// controlling which repositories in a private registry are replicated. If no filters
    /// are added, the contents of all repositories are replicated."
    /// ([`RepositoryFilter`]'s own documentation). The set is decided at the PUSH and kept
    /// on the image, because replication copies an image as it arrives: a rule stored after
    /// a push does not reach back for an image the registry already holds.
    ///
    /// Two rules naming one destination are one copy of the image, so the destinations are
    /// de-duplicated by the (Region, registry) pair they name, and no more than
    /// [`MAX_IMAGE_REPLICATIONS`] of them are recorded for one image.
    pub fn planned_replications(
        &self,
        repository_name: &str,
        now: SystemTime,
    ) -> Vec<ImageReplication> {
        let mut planned: BTreeMap<(String, String), ImageReplication> = BTreeMap::new();
        for rule in &self.replication_rules {
            if !replication_rule_covers(rule, repository_name) {
                continue;
            }
            for destination in &rule.destinations {
                if planned.len() >= MAX_IMAGE_REPLICATIONS {
                    break;
                }
                planned.insert(
                    (destination.region.clone(), destination.registry_id.clone()),
                    ImageReplication {
                        region: destination.region.clone(),
                        registry_id: destination.registry_id.clone(),
                        started_at: now,
                        complete_due_at: now + IMAGE_REPLICATION_DELAY,
                    },
                );
            }
        }
        planned.into_values().collect()
    }

    /// Drop every in-flight upload nothing has touched for [`ABANDONED_UPLOAD_AGE`] as of
    /// `now`.
    ///
    /// Called by `InitiateLayerUpload` when it needs a slot: an upload a client opened and
    /// abandoned otherwise holds its slot and its received bytes for the life of the
    /// process, so without this the entry cap and the byte budget could both be pinned by
    /// sessions nobody will ever complete. Nothing is dropped while the registry has room,
    /// and an upload younger than that age is never dropped, so a push in progress keeps
    /// its session.
    pub fn reclaim_abandoned_uploads(&mut self, now: SystemTime) {
        self.uploads.retain(|_, upload| {
            let abandoned = now
                .duration_since(upload.started_at)
                .is_ok_and(|age| age >= ABANDONED_UPLOAD_AGE);
            !abandoned
        });
    }

    /// The value of the account setting `name`: what was last set for it, or its
    /// documented default while nothing has been. Reading one changes nothing.
    pub fn account_setting(&self, name: &str) -> Result<String, Fault> {
        let spec = account_setting_spec(name)?;
        Ok(self
            .account_settings
            .get(spec.name)
            .cloned()
            .unwrap_or_else(|| spec.default_value.to_string()))
    }

    /// Set the account setting `name` to `value`, refusing a name this registry does
    /// not serve and a value that setting does not take. Answers with the value the
    /// store now holds for that setting, read back out of the map rather than echoed,
    /// so a caller cannot report a value the store does not carry.
    pub fn set_account_setting(&mut self, name: &str, value: &str) -> Result<String, Fault> {
        let spec = account_setting_spec(name)?;
        if !spec.settable_values.contains(&value) {
            // The setting and the value come from the request, and "in this region" is
            // the reason: a value this registry's region does not offer for that
            // setting.
            return Err(Fault::InvalidParameter(format!(
                "You can't set '{}' to '{value}' in this region.",
                spec.name
            )));
        }
        self.account_settings
            .insert(spec.name.to_string(), value.to_string());
        Ok(self
            .account_settings
            .get(spec.name)
            .cloned()
            .unwrap_or_else(|| spec.default_value.to_string()))
    }

    /// The registry's replication configuration: the stored rules, which is the empty
    /// list before any `PutReplicationConfiguration` call. ECR reports the
    /// configuration either way — an unconfigured registry has no rules rather than no
    /// configuration.
    pub fn replication_configuration(&self) -> ReplicationConfiguration {
        ReplicationConfiguration {
            rules: self.replication_rules.clone(),
        }
    }
}

/// The wording ECR uses when no pull through cache rule answers to a prefix: the kind of
/// thing that is missing, the identifier the request named it by, and the registry it was
/// looked for in — `A pull-through cache rule with repository prefix '<prefix>' does not
/// exist in the registry with id '<registry id>'`.
///
/// `PullThroughCacheRuleNotFoundException`'s own documentation ("The pull through cache
/// rule was not found. Specify a valid pull through cache rule and try again.") describes
/// the SHAPE; the sentence the API answers with is built from the request, the same
/// construction [`repository_not_found_message`] follows.
pub fn pull_through_cache_rule_not_found_message(prefix: &str) -> String {
    format!(
        "A pull-through cache rule with repository prefix '{prefix}' does not exist in \
         the registry with id '{}'",
        registry_id()
    )
}

/// Whether `rule` covers the repository named `repository_name`, so that a push into it
/// starts a copy to each of the rule's destinations.
///
/// A rule carrying no `repositoryFilters` covers every repository of the registry ("If no
/// filters are added, the contents of all repositories are replicated"), and a
/// `PREFIX_MATCH` filter covers the repositories whose name starts with the filter's value
/// ("the repository name prefix to configure replication for"). The filters of one rule
/// are alternatives: a repository one of them names is replicated by that rule. A
/// `filterType` the model widens the enum with later covers nothing until it is read here,
/// so a filter this code does not understand never silently replicates a repository.
pub fn replication_rule_covers(rule: &ReplicationRule, repository_name: &str) -> bool {
    match rule.repository_filters.as_deref() {
        None => true,
        Some([]) => true,
        Some(filters) => filters.iter().any(|filter| {
            filter.filter_type == PREFIX_MATCH_FILTER_TYPE
                && repository_name.starts_with(&filter.filter)
        }),
    }
}

/// The wording ECR uses when no repository creation template answers to a prefix: the
/// kind of thing that is missing, the prefix the request named it by, and the registry it
/// was looked for in. `TemplateNotFoundException`'s own documentation describes the shape,
/// not the sentence the API answers with — see
/// [`pull_through_cache_rule_not_found_message`].
pub fn template_not_found_message(prefix: &str) -> String {
    template_message(prefix, "is not found in the registry with id")
}

/// The wording ECR uses when the registry ALREADY holds a repository creation template
/// for a prefix — `TemplateAlreadyExistsException`. The same construction as
/// [`template_not_found_message`], with this verdict: the runtime message names the KIND
/// of record with the prefix the request gave and the registry holding it, which the
/// error shape's own documentation ("The repository creation template already exists.
/// Specify a unique prefix and try again.") describes but is not.
pub fn template_already_exists_message(prefix: &str) -> String {
    template_message(prefix, "already exists in the registry with id")
}

/// The frame ECR states a refusal ABOUT ONE repository creation template in: the kind of
/// record, the prefix the request addressed it by, then the verdict and the registry. The
/// one place that frame is written, so both halves of a template's lifecycle are worded
/// the same way and only the verdict differs.
fn template_message(prefix: &str, verdict: &str) -> String {
    format!(
        "A repository creation template with prefix '{prefix}' {verdict} '{}'",
        registry_id()
    )
}

/// The wording ECR uses when the registry holds no pull time update exclusion for a
/// principal — the state `ExclusionNotFoundException` documents ("The specified pull time
/// update exclusion was not found"), phrased from the request: the principal ARN that was
/// asked for and the registry it was looked for in.
pub fn pull_time_update_exclusion_not_found_message(principal_arn: &str) -> String {
    format!(
        "The pull time update exclusion for principal arn '{principal_arn}' was not \
         found in registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when the registry ALREADY holds a pull time update exclusion for
/// a principal — `ExclusionAlreadyExistsException`. The same construction as
/// [`pull_time_update_exclusion_not_found_message`], so the two halves of one record's
/// lifecycle name the principal and the registry the same way.
pub fn pull_time_update_exclusion_already_exists_message(principal_arn: &str) -> String {
    format!(
        "The pull time update exclusion for principal arn '{principal_arn}' already \
         exists in registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when a register would carry the registry past the number of pull
/// time update exclusions it may hold — `LimitExceededException`. Built the same way as
/// the two messages above, from the registry the request addressed and the bound it met:
/// [`MAX_PULL_TIME_UPDATE_EXCLUSIONS`], which is the maximum the model declares for the
/// list the read side reports.
pub fn pull_time_update_exclusion_limit_message() -> String {
    format!(
        "The registry with id '{}' has the maximum number of pull time update exclusions \
         allowed, which is {MAX_PULL_TIME_UPDATE_EXCLUSIONS}.",
        registry_id()
    )
}

/// The upstream registries ECR documents as serving anonymously, each with the host it
/// is reached at: Amazon ECR Public and Kubernetes, from the ECR user guide's
/// supported-upstream table. One table, so the `UpstreamRegistry` value and the host it
/// corresponds to cannot drift apart.
pub const ANONYMOUS_UPSTREAM_REGISTRIES: [(&str, &str); 2] = [
    ("ecr-public", "public.ecr.aws"),
    ("k8s", "registry.k8s.io"),
];

/// Whether an upstream registry a pull through cache rule points at authenticates its
/// callers, and therefore needs a credential stored on the rule.
///
/// Every upstream registry ECR supports except the two in
/// [`ANONYMOUS_UPSTREAM_REGISTRIES`] — the rest of the model's `UpstreamRegistry` enum,
/// listed in `UPSTREAM_REGISTRY_VALUES` — requires credentials. A rule that names no
/// upstream registry is judged by the host it points at instead.
pub fn upstream_needs_credential(upstream_registry: Option<&str>, upstream_url: &str) -> bool {
    match upstream_registry {
        Some(registry) => !ANONYMOUS_UPSTREAM_REGISTRIES
            .iter()
            .any(|(anonymous, _)| *anonymous == registry),
        None => {
            // A registry URL's authority: what is left after an optional scheme, before
            // the path, without the userinfo or the port, compared case-insensitively —
            // a host is not case-sensitive, and `public.ecr.aws:443` is the same host as
            // `public.ecr.aws`.
            let host = upstream_url
                .split_once("://")
                .map_or(upstream_url, |(_, rest)| rest)
                .split('/')
                .next()
                .unwrap_or_default();
            let host = host.rsplit_once('@').map_or(host, |(_, after)| after);
            let host = host.split_once(':').map_or(host, |(before, _)| before);
            let host = host.to_ascii_lowercase();
            !ANONYMOUS_UPSTREAM_REGISTRIES
                .iter()
                .any(|(_, anonymous_host)| *anonymous_host == host)
        }
    }
}

/// The Secrets Manager secret name prefix a pull through cache rule's credential is
/// stored under. `CredentialArn`'s own modelled pattern states it —
/// `^arn:aws(-\w+)*:secretsmanager:[a-zA-Z0-9-:]+:secret:ecr\-pullthroughcache\/[a-zA-Z0-9\/_+=.@-]+$`
/// — so a value naming anything else names no secret ECR reads a rule's upstream
/// credential from.
pub const CREDENTIAL_SECRET_NAME_PREFIX: &str = "ecr-pullthroughcache/";

/// The ARN vendor (service namespace) field of a `credentialArn`: the secret a rule's
/// credential lives in is an Amazon Web Services Secrets Manager secret.
const SECRETS_MANAGER_VENDOR: &str = "secretsmanager";

/// The resource field prefix of a Secrets Manager secret ARN (`…:secret:<name>`).
const SECRET_RESOURCE_PREFIX: &str = "secret:";

/// Whether `value` is the ARN of the Secrets Manager secret a pull through cache rule's
/// `credentialArn` names, as that member's own modelled pattern spells one:
/// `^arn:aws(-\w+)*:secretsmanager:[a-zA-Z0-9-:]+:secret:ecr\-pullthroughcache\/[a-zA-Z0-9\/_+=.@-]+$`.
///
/// The six fields come from [`::service_common::arn::parse_of`], the tree's ONE ARN parse,
/// which also settles that the value is an ARN of the `secretsmanager` vendor at all. What
/// is checked here is the rest of that pattern, field by field: the partition is `aws`
/// optionally followed by `-`-separated word labels, the region-and-account span is the
/// `[a-zA-Z0-9-:]+` the pattern states, and the secret name after
/// [`CREDENTIAL_SECRET_NAME_PREFIX`] is non-empty and drawn from the pattern's own
/// character class — so a value this accepts is one the member accepts.
pub fn is_pull_through_cache_credential_arn(value: &str) -> bool {
    let Some(arn) = ::service_common::arn::parse_of(value, SECRETS_MANAGER_VENDOR) else {
        return false;
    };
    let Some(name) = arn
        .resource
        .strip_prefix(SECRET_RESOURCE_PREFIX)
        .and_then(|name| name.strip_prefix(CREDENTIAL_SECRET_NAME_PREFIX))
    else {
        return false;
    };
    is_aws_partition(arn.partition)
        // `[a-zA-Z0-9-:]+` spans the region and account fields together, so each is read
        // under that class and the pair must carry something.
        && (!arn.region.is_empty() || !arn.account.is_empty())
        && [arn.region, arn.account]
            .iter()
            .all(|field| field.bytes().all(is_region_or_account_byte))
        && !name.is_empty()
        && name.bytes().all(is_secret_name_byte)
}

/// Whether `partition` is the `aws(-\w+)*` partition label `CredentialArn`'s pattern
/// states: `aws`, optionally followed by `-`-separated word labels (`aws-cn`,
/// `aws-us-gov`).
fn is_aws_partition(partition: &str) -> bool {
    let Some(labels) = partition.strip_prefix("aws") else {
        return false;
    };
    if labels.is_empty() {
        return true;
    }
    // `(-\w+)*`: every label after the first is introduced by a hyphen and is a non-empty
    // run of word characters.
    labels.starts_with('-')
        && labels.split('-').skip(1).all(|label| {
            !label.is_empty()
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
}

/// A byte of the `[a-zA-Z0-9-:]+` span `CredentialArn`'s pattern allows between the
/// service namespace and the `secret:` resource.
///
/// The `:` of that class is what lets the span cover the Region and the account TOGETHER
/// (so a secret ARN carrying no Region still matches); the two fields are read one at a
/// time here, each already split on `:`, so only the rest of the class applies to a field.
fn is_region_or_account_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-'
}

/// A byte of the `[a-zA-Z0-9\/_+=.@-]+` secret name `CredentialArn`'s pattern allows
/// after the `ecr-pullthroughcache/` namespace.
fn is_secret_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"/_+=.@-".contains(&byte)
}

/// Whether `value` names an IAM role of this account, which is what a rule's
/// `customRoleArn` carries ("The ARN of the IAM role associated with the pull through
/// cache rule").
///
/// The reading is [`::service_common::iam_role::role_name`], the tree's one IAM role-ARN
/// reader — the `iam` vendor, the empty Region a partition-global service's ARN carries,
/// this account, and a `role/…` resource with a name — so this service and every other
/// service that resolves a role ARN agree on what one is.
pub fn is_iam_role_arn(value: &str) -> bool {
    ::service_common::iam_role::role_name(value).is_some()
}

/// The reason the stored pull through cache `rule` does not validate, or `None` when it
/// does.
///
/// `ValidatePullThroughCacheRule` is a verdict on the rule the registry HOLDS:
/// "validates an existing pull through cache rule for an upstream registry that requires
/// authentication … retrieve the contents of the Secrets Manager secret, verify the
/// syntax, and then validate that authentication to the upstream registry is
/// successful". So the verdict is read off the stored record and nothing else:
///
/// * a rule has to name the upstream it caches from;
/// * an upstream that authenticates its callers needs a credential on the rule — a
///   `credentialArn` or a `customRoleArn` — because without one authentication cannot
///   even be attempted, while an upstream ECR documents as serving anonymously
///   ([`ANONYMOUS_UPSTREAM_REGISTRIES`]) needs none;
/// * a credential the rule does carry has to be the kind of ARN its member documents,
///   which is the "verify the syntax" step: a `credentialArn` outside the
///   [`CREDENTIAL_SECRET_NAME_PREFIX`] secret namespace, or a `customRoleArn` that is no
///   IAM role, names nothing the upstream could be authenticated with.
///
/// An empty `customRoleArn` is no credential: the member is modelled `@length(0..2048)`,
/// so the empty string is a legal value that stores no role.
///
/// Nothing here depends on reaching the upstream registry. This emulator opens no
/// outbound connection, and that is a property of the emulator rather than of the rule,
/// so it is never reported as the rule's verdict: a complete rule validates.
pub fn pull_through_cache_rule_failure(rule: &PullThroughCacheRule) -> Option<String> {
    let prefix = rule.ecr_repository_prefix.as_deref().unwrap_or_default();
    let url = rule.upstream_registry_url.as_deref().unwrap_or_default();
    if url.trim().is_empty() {
        return Some(format!(
            "the pull through cache rule for '{prefix}' stores no upstreamRegistryUrl, \
             so it names no upstream registry to cache from"
        ));
    }

    let credential = rule.credential_arn.as_deref().filter(|arn| !arn.is_empty());
    if let Some(credential) = credential {
        if !is_pull_through_cache_credential_arn(credential) {
            return Some(format!(
                "the credentialArn '{credential}' stored on the pull through cache rule \
                 for '{prefix}' is not the ARN of an Amazon Web Services Secrets Manager \
                 secret named '{CREDENTIAL_SECRET_NAME_PREFIX}…', so no upstream \
                 credential can be retrieved from it"
            ));
        }
    }

    let role = rule.custom_role_arn.as_deref().filter(|arn| !arn.is_empty());
    if let Some(role) = role {
        if !is_iam_role_arn(role) {
            return Some(format!(
                "the customRoleArn '{role}' stored on the pull through cache rule for \
                 '{prefix}' is not the ARN of an IAM role, so no role can be assumed to \
                 read the upstream credential"
            ));
        }
    }

    if credential.is_none()
        && role.is_none()
        && upstream_needs_credential(rule.upstream_registry.as_deref(), url)
    {
        return Some(format!(
            "the upstream registry '{}' requires authentication and the pull through \
             cache rule for '{prefix}' stores neither a credentialArn nor a customRoleArn",
            rule.upstream_registry.as_deref().unwrap_or(url)
        ));
    }

    None
}

/// Whether `setting` is one of the two `ImageTagMutability` values that honour
/// exclusion filters: a filter names tags "excluded from the repository's image tag
/// mutability setting", and only the `*_WITH_EXCLUSION` settings have an exclusion to
/// apply. The four modelled values are matched exactly, so a value the model widens the
/// enum with later is not read as an exclusion setting by its name alone.
pub fn honours_exclusion_filters(setting: &str) -> bool {
    matches!(setting, "IMMUTABLE_WITH_EXCLUSION" | "MUTABLE_WITH_EXCLUSION")
}

/// Refuse a tag-mutability configuration that pairs exclusion filters with a setting
/// that has no exclusion to apply: `MUTABLE` lets every tag be overwritten and
/// `IMMUTABLE` lets none, so a filter beside either could never be consulted. The model
/// cannot express the pairing, so both the operations that write this configuration
/// check it here, through one rule.
///
/// The converse is NOT refused: a `*_WITH_EXCLUSION` setting carrying no filter excludes
/// nothing, which is exactly its plain counterpart's behaviour rather than a
/// contradiction.
pub fn check_exclusion_filters(
    setting: &str,
    filters: &[ImageTagMutabilityExclusionFilter],
) -> Result<(), Fault> {
    if !filters.is_empty() && !honours_exclusion_filters(setting) {
        return Err(Fault::InvalidParameter(format!(
            "imageTagMutabilityExclusionFilters cannot be specified with the {setting} \
             image tag mutability setting"
        )));
    }
    Ok(())
}

/// The identity of a namespace prefix: a repository creation template's `prefix`, and a
/// pull through cache rule's `ecrRepositoryPrefix`, which documents the same rule.
///
/// `Prefix`'s own documentation: "There is always an assumed / applied to the end of the
/// prefix. If you specify ecr-public as the prefix, Amazon ECR treats that as
/// ecr-public/." That assumed separator is a single one, so exactly one trailing slash
/// is not part of the namespace's identity: `prod` and `prod/` name the same template —
/// or the same cache rule — and the stored record spells it one way.
pub fn normalize_prefix(prefix: &str) -> String {
    prefix.strip_suffix('/').unwrap_or(prefix).to_string()
}

/// The wording ECR uses when a repository does not exist.
pub fn repository_not_found_message(name: &str) -> String {
    format!(
        "The repository with name '{name}' does not exist in the registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses for an upload id this registry cannot resolve against the
/// repository the request named — the state `UploadNotFoundException` documents: "the
/// upload could not be found, or the specified upload ID is not valid for this
/// repository". One wording for every operation that addresses an upload, naming the
/// three things the request gave: the upload id, the repository and the registry.
pub fn upload_not_found_message(upload_id: &str, repository_name: &str) -> String {
    upload_message(upload_id, repository_name, "does not exist")
}

/// The wording ECR uses when an upload a request asks to close accepted no part — the
/// state `EmptyUploadException` documents: "the specified layer upload does not contain
/// any layer parts". The same frame every upload refusal is stated in
/// ([`upload_message`]), with this verdict.
pub fn empty_upload_message(upload_id: &str, repository_name: &str) -> String {
    upload_message(
        upload_id,
        repository_name,
        "cannot be completed because it does not have any parts",
    )
}

/// The frame ECR states a refusal ABOUT ONE UPLOAD in: the three things the request
/// addressed it by — the upload id, the repository it was opened against and the registry
/// holding it — and then the verdict. The one place that frame is written, so every
/// operation that refuses an upload words it the same way and only the verdict differs.
fn upload_message(upload_id: &str, repository_name: &str, verdict: &str) -> String {
    format!(
        "The upload with id '{upload_id}' in the repository with name \
         '{repository_name}' in registry with id '{}' {verdict}",
        registry_id()
    )
}

/// The wording ECR uses when the repository already holds the layer the upload a request
/// asks to close carries — `LayerAlreadyExistsException`, whose own documentation ("The
/// image layer already exists in the associated repository.") describes the state rather
/// than the sentence.
///
/// A refusal names the resource by the identifier the REQUEST addressed it by, and a
/// completion addresses an upload by its id: the digest the registry computed over the
/// bytes it received is its own value, not something the caller asked by, so the message
/// states the upload id — in the clause every refusal about one upload states it in
/// ([`upload_message`]).
pub fn layer_already_exists_message(upload_id: &str, repository_name: &str) -> String {
    format!(
        "Layer with upload id '{upload_id}' already exists in the repository with name \
         '{repository_name}' in registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when a repository holds no layer of the digest a request named
/// — the state `LayersNotFoundException` documents: "the specified layers could not be
/// found, or the specified layer is not valid for this repository". The frame every
/// resource held INSIDE a repository is reported missing in ([`not_within_repository`]),
/// naming the three things the request gave: the digest, the repository and the registry.
pub fn layer_not_found_message(layer_digest: &str, repository_name: &str) -> String {
    not_within_repository(
        &format!("layer with digest '{layer_digest}'"),
        repository_name,
        registry_id(),
    )
}

/// The frame ECR states a resource HELD INSIDE a repository is missing in: what the
/// request addressed the resource by (`subject`, the kind of record and the identifier),
/// then the repository and the registry it was looked for in. The one place that frame is
/// written, so a layer and an image are reported missing by the same sentence.
pub(crate) fn not_within_repository(
    subject: &str,
    repository_name: &str,
    registry_id: &str,
) -> String {
    format!(
        "The {subject} does not exist within the repository with name \
         '{repository_name}' in the registry with id '{registry_id}'"
    )
}

/// The wording ECR uses when a repository carries no policy: the error's own
/// documentation is "the specified repository and registry combination does not have an
/// associated repository policy".
pub fn repository_policy_not_found_message(name: &str) -> String {
    format!(
        "Repository policy does not exist for the repository with name '{name}' in the \
         registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when the registry carries no signing configuration — the state
/// `SigningConfigurationNotFoundException` documents: "attempting to retrieve or delete a
/// signing configuration that does not exist".
///
/// One wording for both readers of the record (`GetSigningConfiguration` and
/// `DeleteSigningConfiguration`), shaped like [`repository_not_found_message`]'s "… does
/// not exist in the registry with id '<registry id>'", which is also the text the
/// recorded ECR refusal carries.
pub fn signing_configuration_not_found_message() -> String {
    format!(
        "The signing configuration does not exist in the registry with id '{}'",
        registry_id()
    )
}

/// This emulator's registry: its account id is the registry id of every repository.
pub fn registry_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// The region this request was signed for, which the ARN and repository URI carry.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:ecr:<region>:<registry id>:repository/<name>`.
pub fn repository_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("ecr", region, &format!("repository/{name}"))
}


/// How many bytes of a minted id a layer download token carries: 32, the full width of
/// the digest [`mint_hex`] takes it from, so the token a URL carries is as hard to guess
/// as the mint allows.
const LAYER_DOWNLOAD_TOKEN_BYTES: usize = 32;

/// The host a layer download URL is announced under when the request carried no endpoint
/// this emulator could read: a loopback literal, because this emulator is what serves the
/// download.
const LOOPBACK_HOST: &str = "127.0.0.1";

/// Refuse a request that names a registry other than this emulator's: there is exactly
/// one registry here, and a repository in another account's registry is not ours to serve.
pub fn check_registry(registry_id_input: Option<&String>) -> Result<(), Fault> {
    match registry_id_input {
        Some(id) if id != registry_id() => Err(Fault::InvalidParameter(format!(
            "registryId '{id}' is not this registry; the only registry served here is '{}'",
            registry_id()
        ))),
        _ => Ok(()),
    }
}

/// The policy-language versions an IAM JSON policy document may declare. The IAM JSON
/// policy reference documents exactly two `Version` values: `2012-10-17`, the current
/// one, and `2008-10-17`, the legacy one that no new policy should use.
pub const POLICY_VERSIONS: [&str; 2] = ["2012-10-17", "2008-10-17"];

/// The values an IAM statement's `Effect` may take — "the Effect element is required
/// and specifies whether the statement results in an allow or an explicit deny".
pub const POLICY_EFFECTS: [&str; 2] = ["Allow", "Deny"];

/// The wording ECR uses when the registry carries no registry policy.
pub fn registry_policy_not_found_message() -> String {
    format!(
        "Registry policy does not exist in the registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when a repository carries no lifecycle policy.
pub fn lifecycle_policy_not_found_message(name: &str) -> String {
    format!(
        "Lifecycle policy does not exist for the repository with name '{name}' in the \
         registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when a repository carries no lifecycle policy PREVIEW: the same
/// sentence as [`lifecycle_policy_not_found_message`], naming the preview as the thing
/// that does not exist for the repository the request asked about.
pub fn lifecycle_policy_preview_not_found_message(name: &str) -> String {
    format!(
        "Lifecycle policy preview does not exist for the repository with name '{name}' in \
         the registry with id '{}'",
        registry_id()
    )
}

/// The wording ECR uses when the image an identifier names carries no scan: the state
/// `ScanNotFoundException` reports ("the specified image scan could not be found"). It
/// names the identifier the request sent, the repository and the registry, the way every
/// other not-found in this service does.
pub fn scan_not_found_message(id: &ImageIdentifier, repository_name: &str) -> String {
    format!(
        "Image scan does not exist for the image with '{}' in the repository with name \
         '{repository_name}' in the registry with id '{}'",
        identifier_members(id),
        registry_id()
    )
}

/// The wording ECR uses when the image an identifier names is in Amazon ECR archive and
/// the call needs it in standard storage — the state `ImageArchivedException` reports
/// ("The specified image is archived and cannot be scanned"). Built from the request: the
/// identifier it sent, the repository it named and the registry that holds it.
pub fn image_archived_message(id: &ImageIdentifier, repository_name: &str) -> String {
    format!(
        "The image with {} is archived within the repository with name \
         '{repository_name}' in the registry with id '{}'",
        identifier_description(id),
        registry_id()
    )
}

/// The wording ECR uses when a repository's lifecycle policy preview is still running —
/// `LifecyclePolicyPreviewInProgressException`. Built from the request, so a caller
/// previewing policies across many repositories at once can tell WHICH repository
/// refused and target its retry: the repository the preview is running in, the registry
/// that holds it, and what the caller has to wait for.
pub fn lifecycle_preview_in_progress_message(name: &str) -> String {
    format!(
        "Lifecycle policy preview is already in progress in the repository with name \
         '{name}' in the registry with id '{}', please wait until it completes to start \
         a new one",
        registry_id()
    )
}

/// The constraint a REGISTRY permissions policy document ECR will not accept fails.
/// Whatever the document got wrong, the refusal states the one phrase: the registry
/// policy as a whole is what was rejected, and nothing of how it was read is reported.
pub const REGISTRY_POLICY_CONSTRAINT: &str = "Invalid registry policy provided";

/// The same phrase for a REPOSITORY policy document. See [`REGISTRY_POLICY_CONSTRAINT`].
pub const REPOSITORY_POLICY_CONSTRAINT: &str = "Invalid repository policy provided";

/// Validate `text` as a resource-based IAM policy document, or report
/// [`Fault::InvalidParameter`] worded by [`invalid_document`] with `constraint` — the
/// stable phrase the service naming this document uses for a document it will not
/// accept ([`REGISTRY_POLICY_CONSTRAINT`], [`REPOSITORY_POLICY_CONSTRAINT`]).
///
/// The grammar enforced here is the IAM JSON policy language's own: the document is a
/// JSON object; a `Version` it declares is one of [`POLICY_VERSIONS`]; `Statement` is
/// required and holds either one statement object or a non-empty array of them; and
/// every statement carries an `Effect` of [`POLICY_EFFECTS`], at least one action
/// (`Action` or `NotAction`) and — because this is a RESOURCE-based policy, which
/// states who the permission is granted to — a `Principal`.
///
/// `check_statement` is called with each statement object once it satisfies that
/// grammar, so a service that adds its own rule about what a policy may say (see
/// [`validate_registry_policy`], and the repository-policy lock-out rule in
/// `ops::set_repository_policy`) does not have to parse the document a second time; a
/// service with no such rule passes `|_| Ok(())`.
///
/// Parsing is `serde_json`'s; only the policy grammar on top of it is ours — and neither
/// the parser's own diagnostics nor which clause of the grammar failed reaches the
/// refusal, which states the parameter and the one constraint phrase.
pub fn validate_resource_policy(
    constraint: &str,
    text: &str,
    mut check_statement: impl FnMut(&serde_json::Map<String, serde_json::Value>) -> Result<(), Fault>,
) -> Result<(), Fault> {
    let refuse = || invalid_document(POLICY_TEXT_PARAMETER, constraint);

    let document: serde_json::Value = serde_json::from_str(text).map_err(|_| refuse())?;
    let document = document.as_object().ok_or_else(refuse)?;

    if let Some(version) = document.get("Version") {
        let declared = version.as_str().ok_or_else(refuse)?;
        if !POLICY_VERSIONS.contains(&declared) {
            return Err(refuse());
        }
    }

    let statement = document.get("Statement").ok_or_else(refuse)?;
    let statements: Vec<&serde_json::Value> = match statement {
        serde_json::Value::Array(entries) => entries.iter().collect(),
        single => vec![single],
    };
    if statements.is_empty() {
        return Err(refuse());
    }

    for entry in statements {
        let entry = entry.as_object().ok_or_else(refuse)?;
        let effect = entry
            .get("Effect")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(refuse)?;
        if !POLICY_EFFECTS.contains(&effect) {
            return Err(refuse());
        }
        // At least one action, named by `Action` or by `NotAction`.
        if !["Action", "NotAction"]
            .iter()
            .any(|key| entry.get(*key).is_some_and(names_values))
        {
            return Err(refuse());
        }
        entry
            .get("Principal")
            .filter(|principal| names_values(principal))
            .ok_or_else(refuse)?;
        check_statement(entry)?;
    }
    Ok(())
}

/// The constraint a resource policy document fails when one of the identities it grants to
/// cannot be resolved: the principal was looked up and this account's IAM holds no such
/// identity.
pub const POLICY_PRINCIPAL_NOT_FOUND_CONSTRAINT: &str = "Principal not found";

/// Every identity the statements of the resource policy `text` grant to: the values of
/// their `Principal` elements — the element a resource-based policy states its grantees in,
/// and the one this service's grammar requires of every statement — flattened out of the
/// string, the list or the
/// `{"AWS": …}`-style object each may be written as ([`policy_values`]), in the order the
/// document names them and without repeats — the same identity named twice is one identity
/// to resolve.
///
/// Reported for a document that is a resource policy ([`validate_resource_policy`]), so a
/// text this cannot read names no principal rather than failing: what makes a document
/// unusable is stated by the grammar, once, and not a second time here.
pub fn policy_principals(text: &str) -> Vec<String> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(text) else {
        return Vec::new();
    };
    let statements: Vec<&serde_json::Value> = match document.get("Statement") {
        Some(serde_json::Value::Array(entries)) => entries.iter().collect(),
        // One statement may be written as the object itself rather than as a list.
        Some(single) => vec![single],
        None => Vec::new(),
    };
    let mut named: Vec<String> = Vec::new();
    for principal in statements
        .iter()
        .filter_map(|statement| statement.get("Principal"))
        .flat_map(policy_values)
    {
        if !named.iter().any(|already| already == principal) {
            named.push(principal.to_string());
        }
    }
    named
}

/// The refusal a resource policy document earns when one of the identities it grants to is
/// not there, or `None` when every identity it names resolves.
///
/// A resource-based policy states WHO a permission is granted to, so ECR resolves the
/// identities a document names against this account's IAM before it stores the document —
/// a grant to an identity that does not exist is a grant to nobody — and refuses the
/// `policyText` parameter with [`POLICY_PRINCIPAL_NOT_FOUND_CONSTRAINT`] when IAM answers
/// that one of them is absent. Both of the documents this service stores go through here:
/// a repository policy (`ops::set_repository_policy`) and the registry policy
/// (`ops::put_registry_policy`).
///
/// The resolution itself is [`::service_common::iam_principal::identity_is_absent`], the
/// tree's one reading of a policy principal against IAM, so a refusal is grounded in what
/// IAM ANSWERED: a principal that names no IAM role or user of this account — the `*`
/// wildcard, an account or its root, a service principal, another account's identity — is
/// resolved by nothing here and refuses nothing.
pub async fn unresolved_policy_principal(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    text: &str,
) -> Option<Fault> {
    for principal in policy_principals(text) {
        if ::service_common::iam_principal::identity_is_absent(ctx, region, &principal).await {
            return Some(invalid_document(
                POLICY_TEXT_PARAMETER,
                POLICY_PRINCIPAL_NOT_FOUND_CONSTRAINT,
            ));
        }
    }
    None
}

/// An attached IAM policy document as ECR REPORTS one: the canonical IAM-policy rendering
/// ([`::service_common::iam_policy::canonical_document`]), which is the pretty-printed
/// layout every policy ECR echoes back comes in, whatever layout the caller submitted.
///
/// The layout is the tree's ONE policy rendering, shared with every other service that
/// re-serializes an attached document, so two services handed the same policy report it
/// identically; `text` that is not well-formed JSON comes back as it came, because this is a
/// rendering and the policy grammar ([`validate_resource_policy`]) is what refuses an
/// unusable document.
pub fn canonical_policy_document(text: &str) -> String {
    ::service_common::iam_policy::canonical_document(text)
}

/// Whether a policy element names at least one value: a non-empty string, a non-empty
/// array of strings, or — as `Principal` is written — an object mapping a principal
/// type to either of those.
fn names_values(element: &serde_json::Value) -> bool {
    match element {
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(entries) => !entries.is_empty() && entries.iter().all(names_values),
        serde_json::Value::Object(members) => {
            !members.is_empty() && members.values().all(names_values)
        }
        _ => false,
    }
}

/// Every value a policy element names, flattened out of the string, the array or the
/// `{"AWS": …}`-style object it may be written as. `Principal`, `Action` and `NotAction`
/// are all written in those three forms, so one flattener reads them all.
pub fn policy_values(element: &serde_json::Value) -> Vec<&str> {
    match element {
        serde_json::Value::String(value) => vec![value.as_str()],
        serde_json::Value::Array(entries) => entries.iter().flat_map(policy_values).collect(),
        serde_json::Value::Object(members) => members.values().flat_map(policy_values).collect(),
        _ => Vec::new(),
    }
}

/// Validate a registry permissions policy document: the resource-policy grammar of
/// [`validate_resource_policy`], plus the one rule a REGISTRY policy adds — it grants to
/// other accounts, so every statement must name the identities it grants to and may not
/// open the registry to every principal with the `*` wildcard. Either way the refusal
/// states [`REGISTRY_POLICY_CONSTRAINT`]: both are documents ECR will not accept as this
/// registry's policy.
pub fn validate_registry_policy(text: &str) -> Result<(), Fault> {
    validate_resource_policy(REGISTRY_POLICY_CONSTRAINT, text, |statement| {
        // The grammar above guarantees the statement carries a `Principal`.
        let grants_to_everyone = statement
            .get("Principal")
            .is_some_and(|principal| policy_values(principal).contains(&"*"));
        if grants_to_everyone {
            return Err(invalid_document(
                POLICY_TEXT_PARAMETER,
                REGISTRY_POLICY_CONSTRAINT,
            ));
        }
        Ok(())
    })
}

/// The resource-type prefix of an ECR repository ARN's resource field.
pub const REPOSITORY_RESOURCE_PREFIX: &str = "repository/";

/// The repository a tagging operation's `resourceArn` names.
///
/// ECR's tagging operations address their resource by ARN, and "currently, the only
/// supported resource is an Amazon ECR repository" (`resourceArn`'s own documentation),
/// so a usable value is an `arn:<partition>:ecr:<region>:<account>:repository/<name>`
/// of THIS registry: anything else — a malformed ARN, another service, another resource
/// type — is [`Fault::InvalidParameter`], and another account's registry is refused the
/// way [`check_registry`] refuses it. Whether the named repository exists is the
/// caller's lookup, so a well-formed ARN for an absent repository is reported as
/// [`Fault::RepositoryNotFound`] by the store rather than as an invalid parameter.
pub fn repository_name_from_arn(arn: &str) -> Result<String, Fault> {
    let invalid = || {
        Fault::InvalidParameter(format!(
            "resourceArn '{arn}' is not a valid Amazon ECR repository ARN; the only \
             supported resource is an Amazon ECR repository"
        ))
    };
    // Read through [`::service_common::arn::parse_of`], the tree's ONE resource-ARN parse,
    // which also settles that the value is an ARN of the `ecr` vendor at all.
    let Some(parsed) = ::service_common::arn::parse_of(arn, "ecr") else {
        return Err(invalid());
    };
    let (region, account, resource) = (parsed.region, parsed.account, parsed.resource);
    if region.is_empty() {
        return Err(invalid());
    }
    let name = resource
        .strip_prefix(REPOSITORY_RESOURCE_PREFIX)
        .filter(|name| !name.is_empty())
        .ok_or_else(invalid)?;
    if account != registry_id() {
        return Err(Fault::InvalidParameter(format!(
            "resourceArn '{arn}' names the registry '{account}'; the only registry \
             served here is '{}'",
            registry_id()
        )));
    }
    Ok(name.to_string())
}

/// Refuse a tag key no caller may create or remove: one in AWS's reserved namespace, or
/// one longer than a tag key may be.
fn check_tag_key(key: &str) -> Result<(), Fault> {
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Err(Fault::InvalidParameter(format!(
            "tag key '{key}' is reserved: keys beginning with \
             '{RESERVED_TAG_KEY_PREFIX}' are for Amazon Web Services use and cannot be \
             created or removed by a caller"
        )));
    }
    let key_len = key.chars().count();
    if key_len > MAX_TAG_KEY_LEN {
        return Err(Fault::InvalidTagParameter(format!(
            "tag key '{key}' is {key_len} characters; a tag key can have a maximum \
             character length of {MAX_TAG_KEY_LEN}"
        )));
    }
    Ok(())
}

/// Refuse a `tags` list a resource may not be given, whatever it already carries: an
/// unusable key (see [`check_tag_key`]) or an over-long value.
pub fn check_tags(incoming: &[Tag]) -> Result<(), Fault> {
    for tag in incoming {
        check_tag_key(&tag.key)?;
        let value_len = tag.value.chars().count();
        if value_len > MAX_TAG_VALUE_LEN {
            return Err(Fault::InvalidTagParameter(format!(
                "the value of tag key '{}' is {value_len} characters; a tag value can \
                 have a maximum length of {MAX_TAG_VALUE_LEN}",
                tag.key
            )));
        }
    }
    Ok(())
}

/// Refuse a `tagKeys` list a caller may not name (see [`check_tag_key`]).
pub fn check_tag_keys(keys: &[String]) -> Result<(), Fault> {
    for key in keys {
        check_tag_key(key)?;
    }
    Ok(())
}

/// Refuse an `incoming` list that would leave a resource already carrying `existing`
/// over the tag limit. A tag is identified by its key, so a key already held is
/// replaced rather than added and does not count twice.
pub fn check_tag_count(existing: &[Tag], incoming: &[Tag]) -> Result<(), Fault> {
    let total = existing
        .iter()
        .chain(incoming)
        .map(|tag| tag.key.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    if total > MAX_TAGS {
        return Err(Fault::TooManyTags(format!(
            "the request would leave {total} tags on the resource; the maximum number \
             of tags that can be applied to a repository is {MAX_TAGS}"
        )));
    }
    Ok(())
}

/// The content digest of a blob: `sha256:<hex>` over its bytes, which is how a registry
/// addresses content it was handed — a layer assembled from uploaded parts as much as a
/// manifest.
pub fn content_digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("sha256:{}", hex::encode(Sha256::digest(bytes)))
}

/// The content digest of an image manifest: `sha256:<hex>` over the manifest bytes,
/// which is how a registry addresses a manifest it was handed.
pub fn manifest_digest(manifest: &str) -> String {
    content_digest(manifest.as_bytes())
}

/// The byte length an answer advertises for the manifest document it carries —
/// `imageManifestSize`, whether that document is the stored one
/// ([`ImageRecord::manifest_size`]) or the same manifest served in another schema.
pub fn manifest_document_size(manifest: &str) -> i64 {
    // `imageManifest` is modelled `@length(1..4194304)`, far inside `i64`, and a served
    // document is a re-spelling of one, so the conversion cannot be reached — it saturates
    // rather than wrapping or panicking if it ever is.
    i64::try_from(manifest.len()).unwrap_or(i64::MAX)
}

/// The media type a manifest declares about itself, for a push that named none.
///
/// Both the Docker image manifest V2 schema 2 and the OCI image manifest carry their own
/// `mediaType`, so the type is read out of the pushed document rather than guessed.
pub fn manifest_media_type(manifest: &str) -> Option<String> {
    let document: serde_json::Value = serde_json::from_str(manifest).ok()?;
    document
        .get("mediaType")?
        .as_str()
        .map(ToString::to_string)
}

/// The media type of a Docker image manifest V2 schema 2 \u2014 one of the two interchangeable
/// image-manifest schemas, and one of the values `BatchGetImage`'s `acceptedMediaTypes`
/// documents.
pub const DOCKER_V2_MANIFEST_MEDIA_TYPE: &str =
    "application/vnd.docker.distribution.manifest.v2+json";

/// The media type of an OCI image manifest \u2014 the other of the two. See
/// [`DOCKER_V2_MANIFEST_MEDIA_TYPE`].
pub const OCI_IMAGE_MANIFEST_MEDIA_TYPE: &str = "application/vnd.oci.image.manifest.v1+json";

/// The DESCRIPTOR media types the two interchangeable schemas spell the same content
/// with, as `(Docker, OCI)` pairs: the image configuration blob, the gzipped filesystem
/// layer, and the layer a registry does not redistribute.
///
/// The Docker image manifest V2 schema 2 and the OCI image manifest describe an image the
/// same way \u2014 a `config` descriptor and an ordered `layers` list, each descriptor
/// carrying `digest`, `size` and `mediaType` \u2014 and differ only in the media types they
/// spell those parts with (the Docker Registry HTTP API V2 manifest specification and the
/// OCI image-spec manifest/layer media types). Rendering a manifest in the other schema is
/// therefore a re-spelling of these names and of the document's own `mediaType`: no
/// digest, size or order changes, so the two documents reference exactly the same blobs.
const INTERCHANGEABLE_DESCRIPTOR_MEDIA_TYPES: [(&str, &str); 3] = [
    (
        "application/vnd.docker.container.image.v1+json",
        "application/vnd.oci.image.config.v1+json",
    ),
    (
        "application/vnd.docker.image.rootfs.diff.tar.gzip",
        "application/vnd.oci.image.layer.v1.tar+gzip",
    ),
    (
        "application/vnd.docker.image.rootfs.foreign.diff.tar.gzip",
        "application/vnd.oci.image.layer.nondistributable.v1.tar+gzip",
    ),
];

/// The manifest document to serve for `image`, and the media type it is served as, for a
/// request whose `acceptedMediaTypes` is `accepted`.
///
/// `acceptedMediaTypes` names the manifest schemas the client understands, so a registry
/// serves the image in one of them: the stored manifest when the client accepts its media
/// type, and otherwise the same manifest rendered in an accepted schema. The two
/// interchangeable image-manifest schemas are the Docker image manifest V2 schema 2 and
/// the OCI image manifest (see [`INTERCHANGEABLE_DESCRIPTOR_MEDIA_TYPES`]); a schema
/// nothing can render the stored document in \u2014 a manifest list, a V2 schema 1, a document
/// stored under no media type at all \u2014 leaves the stored manifest as the only thing the
/// registry holds to serve, and it is reported under the media type it really has.
pub fn negotiated_manifest(
    image: &ImageRecord,
    accepted: Option<&[String]>,
) -> (String, Option<String>) {
    let as_stored = || (image.manifest.clone(), image.manifest_media_type.clone());
    // A request that names no accepted media type accepts whatever the registry holds.
    let Some(accepted) = accepted.filter(|types| !types.is_empty()) else {
        return as_stored();
    };
    let Some(media_type) = image.manifest_media_type.as_deref() else {
        return as_stored();
    };
    if accepted.iter().any(|wanted| wanted == media_type) {
        return as_stored();
    }
    let Some(target) = counterpart_manifest_schema(media_type)
        .filter(|schema| accepted.iter().any(|wanted| wanted == schema))
    else {
        return as_stored();
    };
    match rendered_in_schema(&image.manifest, target) {
        Some(rendered) => (rendered, Some(target.to_string())),
        None => as_stored(),
    }
}

/// The other of the two interchangeable image-manifest schemas, or `None` for a media type
/// that is neither of them.
fn counterpart_manifest_schema(media_type: &str) -> Option<&'static str> {
    match media_type {
        DOCKER_V2_MANIFEST_MEDIA_TYPE => Some(OCI_IMAGE_MANIFEST_MEDIA_TYPE),
        OCI_IMAGE_MANIFEST_MEDIA_TYPE => Some(DOCKER_V2_MANIFEST_MEDIA_TYPE),
        _ => None,
    }
}

/// `manifest` rendered in the image-manifest schema `target` names: its own `mediaType`
/// and the media type of every descriptor it carries re-spelled in that schema's
/// vocabulary, and everything else \u2014 every digest, every size, the order of the layers \u2014
/// left exactly as stored.
///
/// `None` when the document cannot be rendered in it: anything that is not a JSON object,
/// or one naming more blob descriptors than [`MAX_MANIFEST_BLOB_ENTRIES`], which is read
/// no further here than anywhere else.
fn rendered_in_schema(manifest: &str, target: &str) -> Option<String> {
    let mut document: serde_json::Value = serde_json::from_str(manifest).ok()?;
    let object = document.as_object_mut()?;
    object.insert(
        "mediaType".to_string(),
        serde_json::Value::String(target.to_string()),
    );
    let to_oci = target == OCI_IMAGE_MANIFEST_MEDIA_TYPE;
    if let Some(config) = object.get_mut("config") {
        respell_descriptor(config, to_oci);
    }
    if let Some(layers) = object
        .get_mut("layers")
        .and_then(serde_json::Value::as_array_mut)
    {
        if layers.len() > MAX_MANIFEST_BLOB_ENTRIES {
            return None;
        }
        for layer in layers.iter_mut() {
            respell_descriptor(layer, to_oci);
        }
    }
    serde_json::to_string(&document).ok()
}

/// Re-spell one content descriptor's `mediaType` in the other schema's vocabulary. A media
/// type that schema does not define \u2014 including one already spelled in the target
/// vocabulary \u2014 is left as it is: only the names the two schemas share a meaning for are
/// translated.
fn respell_descriptor(descriptor: &mut serde_json::Value, to_oci: bool) {
    let Some(media_type) = descriptor.get("mediaType").and_then(|value| value.as_str()) else {
        return;
    };
    let translated = INTERCHANGEABLE_DESCRIPTOR_MEDIA_TYPES
        .iter()
        .find_map(|(docker, oci)| {
            let (source, rendered) = if to_oci { (docker, oci) } else { (oci, docker) };
            (*source == media_type).then_some(*rendered)
        });
    if let Some(translated) = translated {
        if let Some(object) = descriptor.as_object_mut() {
            object.insert(
                "mediaType".to_string(),
                serde_json::Value::String(translated.to_string()),
            );
        }
    }
}

/// Every blob an image manifest references: its filesystem layers in the order the
/// manifest lists them, and then its configuration blob.
///
/// A manifest names its blobs as descriptors, and both container-image manifest
/// formats in use spell them the same way: the Docker image manifest V2 schema 2 and
/// the OCI image manifest carry `config` (the image configuration blob, which a
/// registry stores as a blob like any other) and `layers` (the filesystem layers), each
/// descriptor carrying `digest`, `size` and `mediaType`; the older V2 schema 1 lists
/// them as `fsLayers[].blobSum`, a digest and nothing else. A manifest list or OCI
/// image index references *manifests* rather than blobs, so its `manifests` entries are
/// deliberately not read here: they are other images, not this one's layers.
///
/// The layers come before the configuration blob because that is the order a refusal
/// built from this list reports them in — `PutImage`'s `LayersNotFoundException` names
/// the image's layers first — and it is the only place the order is observable: every
/// other reader keys the blobs by digest.
pub fn manifest_blobs(manifest: &str) -> Vec<LayerBlob> {
    let mut blobs = manifest_layer_descriptors(manifest);
    let Ok(document) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return blobs;
    };
    if let Some(blob) = document.get("config").and_then(blob_descriptor) {
        blobs.push(blob);
    }
    blobs
}

/// The FILESYSTEM LAYER descriptors an image manifest declares, in the order it lists
/// them — the `layers` entries of a Docker image manifest V2 schema 2 or an OCI image
/// manifest, or the `fsLayers[].blobSum` digests of the older V2 schema 1.
///
/// The one reader of those two members: [`manifest_blobs`] adds the image configuration
/// blob to them, and [`is_scannable_image_manifest`] asks whether there are any, so
/// neither has its own copy of the extraction. A manifest list or OCI image index
/// declares none — its `manifests` entries are other images, not this one's layers — and
/// neither does a document that is not an image manifest at all.
pub fn manifest_layer_descriptors(manifest: &str) -> Vec<LayerBlob> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(manifest) else {
        return Vec::new();
    };
    let mut layers = Vec::new();
    if let Some(declared) = document.get("layers").and_then(serde_json::Value::as_array) {
        layers.extend(declared.iter().filter_map(blob_descriptor));
    }
    if let Some(fs_layers) = document
        .get("fsLayers")
        .and_then(serde_json::Value::as_array)
    {
        layers.extend(fs_layers.iter().filter_map(|entry| {
            Some(LayerBlob {
                digest: entry.get("blobSum")?.as_str()?.to_string(),
                size: None,
                media_type: None,
                filesystem: LayerFilesystem::default(),
                content: Vec::new(),
            })
        }));
    }
    layers
}

/// One OCI/Docker content descriptor, or `None` when it names no blob.
fn blob_descriptor(descriptor: &serde_json::Value) -> Option<LayerBlob> {
    Some(LayerBlob {
        digest: descriptor.get("digest")?.as_str()?.to_string(),
        size: descriptor.get("size").and_then(serde_json::Value::as_i64),
        media_type: descriptor
            .get("mediaType")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        // A descriptor carries no blob content: the bytes, and what reading them
        // identified, live on the repository's own record of the blob.
        filesystem: LayerFilesystem::default(),
        content: Vec::new(),
    })
}

/// Whether `digest` is a content digest at all: `<algorithm>:<hex>`, the form the model
/// constrains `LayerDigest` to (`^[a-zA-Z0-9-_+.]+:[a-fA-F0-9]+$`). A batched request
/// carries `BatchedOperationLayerDigest`, which the model gives no pattern, so a
/// malformed digest reaches a handler and is reported against its own entry instead of
/// refusing the whole batch.
pub fn is_layer_digest(digest: &str) -> bool {
    let Some((algorithm, encoded)) = digest.split_once(':') else {
        return false;
    };
    !algorithm.is_empty()
        && algorithm
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '+' | '.'))
        && !encoded.is_empty()
        && encoded.chars().all(|c| c.is_ascii_hexdigit())
}

/// Match `tag` against a `*`-glob pattern, where `*` stands for any run of characters.
///
/// Two callers share it. An image-tag exclusion filter: the model constrains
/// `ImageTagMutabilityExclusionFilterValue` to `^[0-9a-zA-Z._*-]{1,128}$`, so `*` is the
/// only metacharacter there is to honour — a general glob crate would match more than
/// ECR does. And an IAM policy's `Action`/`NotAction` pattern (see
/// `ops::set_repository_policy`), where `*` means the same thing; IAM's single-character
/// `?` is deliberately NOT honoured, which leaves that caller's lock-out check
/// conservative — a pattern written with `?` reads as not covering the action, so the
/// policy is accepted rather than refused. (The guardduty service carries its own
/// private matcher for its own filter alphabet; neither is reachable from the other
/// today.)
pub fn wildcard_matches(filter: &str, tag: &str) -> bool {
    let segments: Vec<&str> = filter.split('*').collect();
    let (Some(first), Some(last)) = (segments.first(), segments.last()) else {
        return false;
    };
    if segments.len() == 1 {
        return *first == tag;
    }
    let Some(mut rest) = tag.strip_prefix(first) else {
        return false;
    };
    for middle in &segments[1..segments.len() - 1] {
        match rest.find(middle) {
            Some(at) => rest = &rest[at + middle.len()..],
            None => return false,
        }
    }
    rest.ends_with(last)
}

/// Encode a pagination token: an opaque rendering of the last key returned, resolved
/// against live state on the next call rather than against a snapshot.
pub fn encode_token(key: &str) -> String {
    hex::encode(key.as_bytes())
}

/// The key a pagination token was built from, or [`Fault::InvalidParameter`].
///
/// A token this service did not mint is refused in the parameter-and-constraint form ECR
/// states a refused parameter in ([`invalid_parameter_at`]), naming the API label of the
/// parameter that carried it and the constraint it failed. The token itself is NOT
/// echoed: a continuation token is opaque, so the refusal states what failed rather than
/// the value a caller already holds.
pub fn decode_token(token: &str) -> Result<String, Fault> {
    let bad = || invalid_parameter_at(NEXT_TOKEN_PARAMETER, NEXT_TOKEN_CONSTRAINT);
    let bytes = hex::decode(token).map_err(|_| bad())?;
    String::from_utf8(bytes).map_err(|_| bad())
}

/// One page of `items`, which must be ordered by their keys: everything after the key
/// `next_token` names, capped at `max_results` and never at more than
/// [`DEFAULT_PAGE_SIZE`], plus the token for the page after it.
///
/// The token names the last entry the page carried, so the page after it resumes strictly
/// after that entry: paging a stable listing visits every entry exactly once, in the one
/// order its keys put them in.
pub fn paginate<T>(
    items: Vec<(String, T)>,
    next_token: Option<&String>,
    max_results: Option<i32>,
) -> Result<(Vec<T>, Option<String>), Fault> {
    let after = match next_token {
        Some(token) => Some(decode_token(token)?),
        None => None,
    };
    // The modelled `@range(min: 1, max: 1000)` is refused at the boundary, so a
    // non-positive value cannot reach here; it is refused rather than silently read as
    // "an empty page". The page is then clamped to the documented maximum, so one call
    // can never ask for the whole store.
    let page_size = match max_results {
        Some(max) => usize::try_from(max)
            .ok()
            .filter(|size| *size > 0)
            .ok_or_else(|| {
                Fault::InvalidParameter(format!(
                    "maxResults '{max}' must be a positive integer"
                ))
            })?
            .min(DEFAULT_PAGE_SIZE),
        None => DEFAULT_PAGE_SIZE,
    };

    let mut page = Vec::new();
    let mut last_key: Option<String> = None;
    let mut truncated = false;
    for (key, item) in items {
        if after.as_ref().is_some_and(|a| key.as_str() <= a.as_str()) {
            continue;
        }
        if page.len() == page_size {
            truncated = true;
            break;
        }
        last_key = Some(key);
        page.push(item);
    }
    let token = if truncated {
        last_key.map(|key| encode_token(&key))
    } else {
        None
    };
    Ok((page, token))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsEcrAmazonec2containerregistryV20150921State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsEcrAmazonec2containerregistryV20150921State>)
}

/// The registry's scanning configuration — the record `PutRegistryScanningConfiguration`
/// stores and `GetRegistryScanningConfiguration` reports — and everything derived from
/// it, in `state/scanning.rs`. Declared and re-exported here, so a handler reaches it as
/// `crate::state::…` like every other part of this store.
pub mod scanning;
/// The EventBridge events this registry publishes when an image arrives in or leaves a
/// repository, in `state/image_events.rs`. Declared and re-exported here, so a handler
/// reaches it as `crate::state::…` like every other part of this store.
pub mod image_events;
/// Whether a policy in force lets the calling principal reach the images and layers of the
/// repository a request names, in `state/authorization.rs`. Declared (and not glob
/// re-exported, so a handler names the gate it consults) here, so a handler reaches it as
/// `crate::state::authorization::…` like every other part of this store.
pub mod authorization;
/// Serving an image PULL made in-process by a compute service on a role's behalf, and the
/// upstream import a pull through cache rule makes on the first one, in
/// `state/registry_pull.rs`. Declared (and not glob re-exported, so a caller names the
/// registry plane it reaches) here, so it is reached as `crate::state::registry_pull::…`
/// like every other part of this store.
pub mod registry_pull;
/// The `AWS/ECR` metric this registry publishes about the pulls its data plane serves, in
/// `state/pull_metrics.rs`. Declared (and not glob re-exported, so a caller names the
/// publication it makes) here, so it is reached as `crate::state::pull_metrics::…` like
/// every other part of this store.
pub mod pull_metrics;
/// The signing jobs a push starts under the registry's signing configuration, and the
/// status each one reports, in `state/signing.rs`. Declared (and not glob re-exported, so a
/// handler names the part of the store it reads) here, so a handler reaches it as
/// `crate::state::signing::…` like every other part of this store.
pub mod signing;
mod image_scan;
mod layer_download;
/// The endpoint a registry address this service announces names, and the repository URI
/// and `docker login` URL built on it, in `state/registry_address.rs`. Declared and
/// re-exported here, so a handler reaches it as `crate::state::…` like every other part
/// of this store.
mod registry_address;
pub use registry_address::*;
/// The `docker login` credentials this registry has issued, and the pull bookkeeping a
/// registry pull leaves behind, in `state/registry_v2.rs`.
mod registry_v2;
pub use registry_v2::*;
pub use layer_download::*;
pub use image_scan::*;
pub use scanning::*;
