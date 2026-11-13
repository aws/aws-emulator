// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-lambdamicrovms-lambdamicrovms service.
//!
//! Every operation computes its response from the records held here; nothing is
//! canned and nothing is echoed back from the request. The handlers reach this
//! store through [`service_state`] and mutate it through the guard it hands back.
//!
//! ## The clock-owed transitions
//!
//! Several of the service's lifecycles are asynchronous:
//!
//! * `CreateMicrovmImage` — "The build is asynchronous — the image transitions
//!   from CREATING to CREATED on success", or to CREATE_FAILED on failure, which is
//!   decided by resolving the version's code artifact against the emulator's own object
//!   store ([`code_artifact`]);
//! * `RunMicrovm` — "The MicroVM starts in PENDING state and transitions to
//!   RUNNING once provisioning completes."; and
//! * `DeleteMicrovmImage` — the image is reported `DELETING` and the removal that
//!   makes every later read miss is owed by the clock; and
//! * `CreateMicrovmAuthToken` — "the duration in minutes before the authentication
//!   token expires", after which the MicroVM endpoint accepts it no longer.
//!
//! Each record therefore stores the INSTANT its next transition becomes due and
//! [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::materialize_due`] applies
//! every transition the clock has already made due. No handler sleeps, and a
//! caller that reads straight after the create still observes the transient
//! state.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    CloudWatchLogging, CodeArtifact, CpuConfiguration, Hooks, IdlePolicy, Logging, RecordedMember,
    Resources, SnapshotBuild, ARCHITECTURE_VALUES,
};

/// The MicroVM endpoint: the address a MicroVM is published on, the tokens that
/// address accepts, and what this emulator answers there.
pub mod microvm_endpoint;

/// Where a version's `codeArtifact` comes from: the object the emulator's own Amazon S3
/// holds at the address the request named, which is what says whether its build can
/// produce an image at all.
pub mod code_artifact;

/// The region a record belongs to: every resource of this service is regional, so the
/// store is partitioned by region and every lookup is scoped to the region the request
/// was signed for.
pub mod regions;

/// The guest a MicroVM runs: the program held in the image version's code artifact.
pub mod guest_program;

/// RUNNING that guest — the engine the image's own code executes in, and the one request
/// a published endpoint hands it.
pub mod guest_runtime;

/// The network a MicroVM's guest reaches: which outbound connection its
/// `egressNetworkConnectors` carry, and what the guest observes of the one they do.
pub mod microvm_network;

/// The region an ARN carries when the request is unsigned, matching the region
/// the emulator's generated client config defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateMicrovmImage` the asynchronous image build becomes due.
///
/// The model documents the build as asynchronous but gives it no duration, so
/// this span is chosen (not derived from AWS): long enough that a caller acting
/// straight after the create observes `CREATING`, short enough to poll through.
pub const IMAGE_BUILD_SPAN: Duration = Duration::from_millis(500);

/// How long after `RunMicrovm` the `PENDING` -> `RUNNING` provisioning becomes
/// due. Chosen for the same reason as [`IMAGE_BUILD_SPAN`].
pub const MICROVM_PROVISION_SPAN: Duration = Duration::from_millis(500);

/// How long after `TerminateMicrovm` the `TERMINATING` -> `TERMINATED`
/// transition becomes due. Chosen for the same reason as [`IMAGE_BUILD_SPAN`].
pub const MICROVM_TERMINATE_SPAN: Duration = Duration::from_millis(500);

/// How long after `SuspendMicrovm` the `SUSPENDING` -> `SUSPENDED` transition becomes
/// due. Chosen for the same reason as [`IMAGE_BUILD_SPAN`]: the model documents the
/// MicroVM as transitioning "through SUSPENDING to SUSPENDED" but publishes no
/// duration for it.
pub const MICROVM_SUSPEND_SPAN: Duration = Duration::from_millis(500);

/// How long after `DeleteMicrovmImage` the image's removal becomes due.
///
/// The deletion is asynchronous — the call reports the image `DELETING` and a client
/// polls `GetMicrovmImage` until it answers 404 — but AWS publishes no duration for
/// it, so this span is chosen for the same reason as [`IMAGE_BUILD_SPAN`]: a caller
/// acting straight after the delete still observes `DELETING`, and a poll gets
/// through it quickly.
pub const IMAGE_DELETE_SPAN: Duration = Duration::from_millis(500);

/// The AWS-managed network connector a MicroVM image gets when the request names no
/// `egressNetworkConnectors`: outbound internet access.
///
/// This service names its managed connectors (`CreateMicrovmShellAuthToken` documents
/// "the SHELL_INGRESS network connector"); which one is the egress default is recorded
/// AWS behaviour rather than modelled.
pub const DEFAULT_EGRESS_NETWORK_CONNECTOR: &str = "INTERNET_EGRESS";

/// The `maximumDurationInSeconds` a MicroVM gets when the caller names none.
///
/// `RunMicrovmRequest$maximumDurationInSeconds` is documented as "The maximum
/// duration in seconds that the MicroVM can exist before being terminated by the
/// platform. Valid range: 1–28,800 (8 hours)" and declares no `@default`, so a
/// caller that asks for no limit gets the platform's own maximum.
pub const DEFAULT_MAXIMUM_DURATION_IN_SECONDS: i32 = MAX_MAXIMUM_DURATION_IN_SECONDS;

/// The smallest `maximumDurationInSeconds` the member's own `@range` allows.
///
/// `smithy_query(query="operation",operation="RunMicrovm")` reports
/// `maximumDurationInSeconds` as `{"target": "Integer", "range": {"min": 1,
/// "max": 28800}}`, matching the member's documented "Valid range: 1–28,800 (8 hours)".
pub const MIN_MAXIMUM_DURATION_IN_SECONDS: i32 = 1;

/// The largest `maximumDurationInSeconds` the member's own `@range` allows — see
/// [`MIN_MAXIMUM_DURATION_IN_SECONDS`] for the `smithy_query` output it comes from.
pub const MAX_MAXIMUM_DURATION_IN_SECONDS: i32 = 28_800;

/// The largest page a paginated listing of this service serves, and the page size a
/// caller that names no `maxResults` gets.
///
/// This is the maximum the member's own `@range` allows, and every listing of this
/// service declares the same one:
/// `smithy_query(query="operation",operation="ListMicrovms,ListManagedMicrovmImages")`
/// reports `maxResults` as `{"target": "Integer", "httpQuery": "maxResults",
/// "range": {"min": 1, "max": 50}}` for both.
pub const MAX_LIST_PAGE: usize = 50;

/// How many MicroVM images one account may hold.
///
/// The model declares `ServiceQuotaExceededException` — "You have exceeded a service
/// quota for Lambda MicroVMs" — on `CreateMicrovmImage`, so a per-account ceiling is
/// the service's own modelled behaviour. AWS publishes no figure, so the figure is
/// chosen: high enough that no realistic use meets it, finite so the store cannot grow
/// without bound.
pub const MAX_MICROVM_IMAGES: usize = 500;

/// How many MicroVMs one account may have in a non-terminal state at once. Modelled
/// and chosen for the same reasons as [`MAX_MICROVM_IMAGES`], on `RunMicrovm`.
pub const MAX_ACTIVE_MICROVMS: usize = 500;

/// How many REGIONS this store holds records for.
///
/// The store is partitioned by the region a request was signed for ([`request_region`]),
/// and that region comes from the request's own credential scope — so the partition key
/// is a value the CALLER chooses, and without a ceiling a caller signing for endlessly
/// many region names would grow the store without bound. This caps that one dimension,
/// so the whole store is bounded by [`MAX_REGIONS`] partitions, each by
/// [`MAX_MICROVM_IMAGES`], [`MAX_ACTIVE_MICROVMS`] and [`MAX_IDEMPOTENCY_RECORDS`]. The
/// figure is chosen, like [`MAX_MICROVM_IMAGES`]: generous against the regions AWS
/// publishes, so no realistic use meets it, and finite.
pub const MAX_REGIONS: usize = 64;

/// The `message` of the 402 a create answers when it would open a record in a region
/// past [`MAX_REGIONS`]. AWS publishes no wording for it, so — like the message the
/// per-account image quota answers with — the sentence states the rule and the limit it
/// is built from.
pub fn too_many_regions_message() -> String {
    format!("this account already holds MicroVM resources in its limit of {MAX_REGIONS} regions")
}

/// How many versions one MicroVM image may hold.
///
/// The model declares `ServiceQuotaExceededException` on `UpdateMicrovmImage` — the
/// operation that "triggers a new version build" and so mints a further version of an
/// image — so a per-image ceiling is the service's own modelled behaviour. Modelled and
/// chosen for the same reasons as [`MAX_MICROVM_IMAGES`]: high enough that no realistic
/// use meets it, finite so one image's version history cannot grow without bound.
pub const MAX_MICROVM_IMAGE_VERSIONS: usize = 500;

/// The `message` of the 402 a configuration update answers for an image already holding
/// [`MAX_MICROVM_IMAGE_VERSIONS`] versions. AWS publishes no wording for it, so — like
/// the message the per-account image quota answers with — the sentence states the rule
/// and the limit it is built from.
pub fn too_many_image_versions_message() -> String {
    format!("this MicroVM image already holds its limit of {MAX_MICROVM_IMAGE_VERSIONS} versions")
}

/// How long a `TERMINATED` MicroVM stays readable before its record is reclaimed, so
/// terminated MicroVMs do not accumulate for the process's lifetime. AWS publishes no
/// figure, so this span is chosen.
pub const TERMINATED_MICROVM_RETENTION: Duration = Duration::from_secs(60 * 60);

/// How long a `clientToken` keeps replaying the resource it minted. AWS publishes no
/// figure for this service, so the span is chosen; it bounds the idempotency store by
/// time as [`MAX_IDEMPOTENCY_RECORDS`] bounds it by count.
pub const IDEMPOTENCY_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// The most `clientToken` records the store keeps. Reaching it evicts the record
/// closest to expiry, so a caller sending unbounded distinct tokens cannot grow the
/// store without bound.
pub const MAX_IDEMPOTENCY_RECORDS: usize = 10_000;

/// The most tags one resource may carry.
///
/// 50 tags per resource is the maximum AWS publishes for a taggable resource, and the
/// model declares `InvalidParameterValueException` — "One of the parameters in the
/// request is not valid" — on `TagResource`, so a request that would take a resource
/// past it is refused rather than growing the stored tag set without bound.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The `message` of the 400 a tagging call answers when the tag set it would leave on
/// the resource is larger than [`MAX_TAGS_PER_RESOURCE`].
///
/// AWS publishes no wording for this condition on this service, so — like the message
/// `CreateMicrovmImage` answers for the per-account image quota — the sentence states
/// the rule and the limit it is built from, rather than echoing another service's.
pub fn too_many_tags_message() -> String {
    format!("a resource may carry at most {MAX_TAGS_PER_RESOURCE} tags")
}

/// `stateReason` the service records for a caller-requested termination.
///
/// `NonBlankString` has pattern `^[^\s]+$`, so the reason is a single token.
pub const TERMINATION_REASON: &str = "Client.UserInitiatedTermination";

/// `stateReason` the service records when a MicroVM outlives its
/// `maximumDurationInSeconds` — documented as "The maximum duration in seconds that
/// the MicroVM can exist before being terminated by the platform."
pub const EXPIRY_REASON: &str = "Platform.MaximumDurationExceeded";

/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`).
pub const IMAGE_STATE_CREATING: &str = "CREATING";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`).
pub const IMAGE_STATE_CREATED: &str = "CREATED";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`): the state an image
/// carries while `DeleteMicrovmImage`'s asynchronous deletion is in flight.
pub const IMAGE_STATE_DELETING: &str = "DELETING";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`): the state an image
/// carries while the build `UpdateMicrovmImage` triggered is in flight.
pub const IMAGE_STATE_UPDATING: &str = "UPDATING";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`): the state an image
/// reaches once the build `UpdateMicrovmImage` triggered has succeeded.
pub const IMAGE_STATE_UPDATED: &str = "UPDATED";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`): the state an image
/// reaches when the build `CreateMicrovmImage` started FAILED — "the image transitions
/// from CREATING to CREATED on success, or CREATE_FAILED on failure".
pub const IMAGE_STATE_CREATE_FAILED: &str = "CREATE_FAILED";
/// A `MicrovmImageState` value (`MICROVM_IMAGE_STATE_VALUES`): the settled value of the
/// UPDATE lifecycle whose build failed, the same move one level on from
/// [`IMAGE_STATE_CREATE_FAILED`] — each lifecycle of an image is a pair of a progressive
/// value and the settled value the work it started reaches.
pub const IMAGE_STATE_UPDATE_FAILED: &str = "UPDATE_FAILED";

/// Whether an image in `state` still has an asynchronous lifecycle operation of this
/// service's own in flight.
///
/// `MicrovmImageState` names each lifecycle of an image by a pair: the progressive
/// value the service reports while the work it started runs, and the settled value it
/// reaches when that work ends. A record in a progressive value is one this service is
/// still writing, so the next call that would start further work on it has nothing
/// stable to start from.
pub fn is_image_lifecycle_in_flight(state: &str) -> bool {
    matches!(
        state,
        IMAGE_STATE_CREATING | IMAGE_STATE_UPDATING | IMAGE_STATE_DELETING
    )
}

/// Whether an image in `state` is one a configuration update can build the next version
/// of.
///
/// `UpdateMicrovmImage` "updates the configuration of a MicroVM image and triggers a new
/// version build", so it needs a record whose own lifecycle work has SETTLED and that has
/// a built configuration to carry the change forward from. Each lifecycle of an image is
/// named by a pair of `MicrovmImageState` values — the progressive value this service
/// reports while the work runs and the settled value that work reaches — and only the
/// lifecycles that produced a version leave a record an update may start from: the
/// settled values of the CREATE and UPDATE lifecycles that built one. Every other value
/// is a record nothing can be built from — one this service is still writing
/// ([`is_image_lifecycle_in_flight`]), one whose creation never produced a version
/// ([`IMAGE_STATE_CREATE_FAILED`]), and the values of a deletion, which is one-way —
/// so the update is refused with the modelled 400 ([`image_not_updatable_message`]).
pub fn image_state_admits_update(state: &str) -> bool {
    matches!(
        state,
        IMAGE_STATE_CREATED | IMAGE_STATE_UPDATED | IMAGE_STATE_UPDATE_FAILED
    )
}

/// The `message` of the 400 a configuration update answers for an image whose current
/// state admits no update ([`image_state_admits_update`]).
///
/// The refusal names the image by the ARN the service minted for it — the one identity
/// of the record, whichever form `MicrovmImageIdentifier` the request addressed it by —
/// and states the condition rather than the state it happened to be in (recorded AWS
/// behaviour).
pub fn image_not_updatable_message(image_arn: &str) -> String {
    format!("Cannot update MicroVM Image in its current state: {image_arn}")
}

/// The version the FIRST build of a MicroVM image is minted as.
///
/// Versions are named in this service's `major.minor` form, and a client feeds the
/// string straight back into the version-addressed calls
/// (`GetMicrovmImageVersion`, `RunMicrovm$imageVersion`,
/// `DeleteMicrovmImageVersion`), so the form matters as much as the ordering.
pub const FIRST_IMAGE_VERSION: &str = "1.0";

/// The `message` of the 404 a MicroVM-image lookup answers, built from the
/// `imageIdentifier` the request carried.
pub fn image_not_found_message(identifier: &str) -> String {
    format!("MicroVMImage not found for MicroVMImageID: {identifier}")
}

/// The `message` of the 404 the DELETE path answers for an `imageIdentifier` that
/// resolves to no record, built from the identifier the request carried.
///
/// The delete of a resource that is already gone names the resource it was asked to
/// delete in its own words rather than reusing the read path's wording
/// ([`image_not_found_message`]): the two are distinct messages of the same error
/// class.
pub fn delete_image_not_found_message(identifier: &str) -> String {
    format!("MicroVM image not found: {identifier}")
}

/// The `message` of the 400 a MicroVM-image lookup answers for an `imageIdentifier`
/// that is neither an id this account holds nor a well-formed ARN, built from the
/// identifier the request carried.
///
/// `MicrovmImageIdentifier` is "The ARN or ID of the MicroVmImage", so a value that
/// resolves as neither names nothing this API can ever address — a bad REQUEST, not a
/// resource that happens to be missing.
pub fn invalid_arn_format_message(identifier: &str) -> String {
    format!("Invalid ARN format: {identifier}")
}

/// The `message` of the 400 `CreateMicrovmImage` answers when the `name` the request
/// asks for is already taken, built from that name.
///
/// `name` "Must be unique within the AWS account", and the refusal is a VALIDATION of
/// the request's own `name` member — recorded AWS behaviour: the service answers a
/// duplicate name with `400 ValidationException` carrying this template rather than
/// with the `ConflictException` its model also declares.
pub fn image_name_conflict_message(name: &str) -> String {
    format!("A MicroVM image with the name '{name}' already exists in this account")
}

/// The `message` the service answers when a reference to an AWS-MANAGED MicroVM image
/// cannot be resolved to a published one, built from the reference the request carried.
///
/// `baseImageArn` is "The ARN of the Lambda-managed base MicroVM image to build upon",
/// so the one refusal for an unresolvable base-image reference is worded for a managed
/// image whatever namespace the ARN names: `CreateMicrovmImage`'s 400 on its own
/// `baseImageArn` member is where a managed reference is resolved and can therefore be
/// refused.
pub fn base_image_not_found_message(base_image_arn: &str) -> String {
    format!("Managed VMI not found: {base_image_arn}")
}

/// The `message` of the 400 `CreateMicrovmImage` answers when `codeArtifact` carries a
/// `uri` that is not an S3 object URI: it names the form the member requires.
///
/// `CodeArtifact$uri` is "The URI of the code artifact in Amazon S3", and the build
/// resolves that location before anything is created, so a URI that addresses no S3
/// object is refused rather than stored.
pub const CODE_ARTIFACT_URI_REQUIREMENT: &str =
    "CodeArtifact URI must be a valid S3 URI in the format: s3://<bucket>/<key>";

/// The `(bucket, key)` the S3 object URI `uri` addresses, or `None` when `uri` is not
/// one.
///
/// An S3 object URI is `s3://<bucket>/<key>`: the scheme is `s3`, and both the bucket
/// and the key are non-empty — `s3://bucket` names a bucket and no object, and
/// `s3:///key` names no bucket.
pub fn s3_object_uri(uri: &str) -> Option<(&str, &str)> {
    let (bucket, key) = uri.strip_prefix("s3://")?.split_once('/')?;
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key))
}

/// The ARN partition this emulator mints its own ARNs in, read off an ARN
/// [`microvm_image_arn`] builds rather than written here as a literal, so the partition
/// an identifier is held to has one source of truth. A reference built in another
/// partition names a resource no call to this emulator can reach.
///
/// The emulator's account contract does not change while the process runs, so the
/// partition is derived once and every later identifier is judged against that one
/// value.
pub fn emulator_partition() -> &'static str {
    static PARTITION: ::std::sync::OnceLock<String> = ::std::sync::OnceLock::new();
    PARTITION.get_or_init(|| {
        let minted = microvm_image_arn("", "partition-probe");
        ::service_common::arn::parse(&minted)
            .map(|arn| arn.partition.to_string())
            .unwrap_or_default()
    })
}

/// The image NAME an ARN of THIS account's own `microvm-image` namespace carries, or
/// `None` when `arn` names no image of it.
///
/// Read through the one shared ARN parse (`service_common::arn`): the partition is the
/// one this emulator serves ([`emulator_partition`]), the vendor is this service's
/// `arnNamespace` (`lambda`), the account is the emulator's own, and the resource is in
/// the form `MicrovmImageArn`'s own pattern defines
/// (`^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`) — the type
/// `microvm-image`, a colon, and a non-empty name. A value spelling that resource any
/// other way (`microvm-image/<name>`) is outside the pattern, so it names no image of
/// this service whatever account it carries; an ARN of any other account — an
/// AWS-MANAGED base image carries `aws` there — belongs to another namespace, see
/// [`managed_microvm_image_id`].
pub fn own_microvm_image_name(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.partition != emulator_partition()
        || parsed.vendor != "lambda"
        || parsed.account != ::engine::emulator_account_id()
    {
        return None;
    }
    parsed
        .resource
        .strip_prefix("microvm-image:")
        .filter(|name| !name.is_empty())
}

/// Whether `arn` names a MicroVM image in THIS account's own `microvm-image`
/// namespace — the one namespace this emulator holds, and can therefore resolve a
/// reference against. Says NOTHING about whether such an image exists, or about which
/// region holds it; see [`own_microvm_image_name`].
pub fn is_own_microvm_image_arn(arn: &str) -> bool {
    own_microvm_image_name(arn).is_some()
}

/// The ids of the AWS-MANAGED MicroVM base images this service publishes — the
/// catalogue `baseImageArn` is resolved against.
///
/// `CreateMicrovmImage$baseImageArn` is "The ARN of the Lambda-managed base MicroVM
/// image to build upon. Use ListManagedMicrovmImages to discover available base
/// images", so the set of base images is AWS's own published catalogue rather than
/// anything this account creates: `al2023-1` is the Amazon Linux 2023 base image the
/// service publishes (recorded as
/// `arn:aws:lambda:<region>:aws:microvm-image:al2023-1`). `ListManagedMicrovmImages`
/// is the operation that serves this catalogue — through
/// [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::managed_base_images`], the same
/// set the base-image resolution below reads, so the two cannot disagree.
pub const MANAGED_BASE_IMAGE_IDS: &[&str] = &["al2023-1"];

/// The id an AWS-MANAGED MicroVM image ARN names, or `None` when `arn` names no
/// managed MicroVM image.
///
/// An AWS-owned resource of this service carries `aws` in the ARN's account field
/// (the same form [`managed_network_connector_arn`] builds), so that field is what
/// says a reference is into AWS's catalogue rather than into an account's own images.
pub fn managed_microvm_image_id(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != "lambda" || parsed.account != MANAGED_RESOURCE_ACCOUNT {
        return None;
    }
    parsed.resource.strip_prefix("microvm-image:")
}

/// Whether `arn` names a MicroVM image in the AWS-OWNED namespace — the one
/// `ListManagedMicrovmImages` publishes, whose ARNs carry [`MANAGED_RESOURCE_ACCOUNT`]
/// in the account field.
///
/// Such a resource belongs to AWS rather than to the calling account, so a
/// customer-facing operation addressed at one is reaching OUTSIDE the caller's own
/// namespace — whether or not the catalogue publishes it, which is why this says
/// nothing about resolution; see [`managed_microvm_image_id`].
pub fn is_managed_microvm_image_arn(arn: &str) -> bool {
    managed_microvm_image_id(arn).is_some()
}

/// Whether the AWS-managed catalogue publishes a base image with id `image_id`.
pub fn is_published_managed_base_image(image_id: &str) -> bool {
    MANAGED_BASE_IMAGE_IDS.contains(&image_id)
}

/// The ARN of the AWS-managed MicroVM base image `image_id` in `region`.
pub fn managed_base_image_arn(region: &str, image_id: &str) -> String {
    format!("arn:aws:lambda:{region}:{MANAGED_RESOURCE_ACCOUNT}:microvm-image:{image_id}")
}

/// The generations the AWS-managed catalogue publishes, as `(base image id,
/// generation, the instant AWS published that generation as epoch seconds, the CPU
/// architectures that generation supports)`.
///
/// An AWS-PUBLISHED base image is versioned by a plain integer GENERATION counter —
/// not the `major.minor` form this service mints for an account's own images
/// ([`FIRST_IMAGE_VERSION`]) — and AWS publishes several generations of one base image
/// at once, so a client can pin an older generation for a reproducible build.
/// `ListManagedMicrovmImageVersions` serves this table, so what a client DISCOVERS to
/// pin as `CreateMicrovmImage$baseImageVersion` ("The specific version of the base
/// MicroVM image to use") is what the table publishes. A create's own version is not
/// checked against it — the service records whatever version the request pinned — and
/// what a create that pins NONE records is not a generation of this table either; see
/// [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::resolve_base_image_version`].
///
/// Each generation also publishes the architectures a MicroVM built on it can target,
/// which is how a caller selects a base image by architecture; the catalogue's images
/// support every architecture this API models (`ARCHITECTURE_VALUES`, the values
/// `smithy_query(query="shape",shape="Architecture")` lists).
///
/// A generation's publication INSTANT is published metadata of AWS's own catalogue, not
/// anything this emulator or an account does, so it is a constant of this table: the
/// same value on every read, for the life of the entry. The instants below are chosen
/// (this emulator does not hold AWS's own publication dates), anchored on the API
/// version this service serves the catalogue under, and ordered as the generations are:
/// a later generation is published later.
pub const MANAGED_BASE_IMAGE_VERSIONS: &[(&str, &str, i64, &[&str])] = &[
    // Generation 1, published 2025-10-14T00:00:00Z.
    ("al2023-1", "1", 1_760_400_000, ARCHITECTURE_VALUES),
    // Generation 0, published 2025-09-09T00:00:00Z — the service's own API version date,
    // the first instant it could have published a base image under.
    ("al2023-1", "0", 1_757_376_000, ARCHITECTURE_VALUES),
];

/// A `ManagedMicrovmImageVersionStatus` value
/// (`MANAGED_MICROVM_IMAGE_VERSION_STATUS_VALUES`): "the version is available for
/// use".
///
/// The other value, `DEPRECATED`, is for a version AWS has retired ("Do not use this
/// version for new MicroVM images"); the catalogue publishes nothing retired, so
/// every entry it serves is available for use.
pub const MANAGED_VERSION_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The generations the catalogue publishes for the base image `image_id`, in the order
/// a listing pages them: NEWEST generation first, which is the order AWS publishes a
/// base image's generations in (see [`version_order`] for the order itself). Empty for
/// an id the catalogue does not publish, which holds no generations either.
pub fn published_managed_base_image_versions(image_id: &str) -> Vec<&'static str> {
    let mut versions: Vec<&'static str> = match is_published_managed_base_image(image_id) {
        true => MANAGED_BASE_IMAGE_VERSIONS
            .iter()
            .filter(|(id, ..)| *id == image_id)
            .map(|(_, version, ..)| *version)
            .collect(),
        false => Vec::new(),
    };
    versions.sort_unstable_by_key(|version| version_order(version));
    versions.reverse();
    versions
}

/// The architectures the catalogue publishes `image_version` of the base image
/// `image_id` as supporting, as the modelled `CpuConfiguration` structures a listing
/// reports them in. Empty for a generation the catalogue does not publish.
pub fn managed_base_image_supported_architectures(
    image_id: &str,
    image_version: &str,
) -> Vec<CpuConfiguration> {
    MANAGED_BASE_IMAGE_VERSIONS
        .iter()
        .filter(|(id, version, ..)| *id == image_id && *version == image_version)
        .flat_map(|(.., architectures)| architectures.iter())
        .map(|architecture| CpuConfiguration {
            architecture: (*architecture).to_string(),
        })
        .collect()
}

/// When the catalogue published `image_version` of the base image `image_id`, as the
/// wire timestamp type a listing reports it with. `None` for a generation the catalogue
/// does not publish, which was never published at all.
///
/// The instant is the table's own ([`MANAGED_BASE_IMAGE_VERSIONS`]) rather than any
/// clock this emulator reads, so every read of the catalogue reports the same value for
/// a given entry and a client can cache it, diff it and key its own records on it.
pub fn managed_base_image_version_published_at(
    image_id: &str,
    image_version: &str,
) -> Option<DateTime> {
    match is_published_managed_base_image(image_id) {
        true => MANAGED_BASE_IMAGE_VERSIONS
            .iter()
            .find(|(id, version, ..)| *id == image_id && *version == image_version)
            .map(|(_, _, published_at, _)| DateTime::from_secs(*published_at)),
        false => None,
    }
}

/// When the catalogue entry for the base image `image_id` was published and when it was
/// last republished, as `(createdAt, updatedAt)`. `None` for an id the catalogue does
/// not publish.
///
/// A catalogue ENTRY is published with its first generation and written again every time
/// AWS publishes another generation of it, so its `createdAt` is the publication instant
/// of its OLDEST generation and its `updatedAt` that of its NEWEST — both read from the
/// one generation table, which is why a listing of the entry and a listing of its
/// versions cannot disagree, and why the two instants differ for every image the
/// catalogue publishes more than one generation of.
pub fn managed_base_image_publication(image_id: &str) -> Option<(DateTime, DateTime)> {
    let mut published: Vec<i64> = match is_published_managed_base_image(image_id) {
        true => MANAGED_BASE_IMAGE_VERSIONS
            .iter()
            .filter(|(id, ..)| *id == image_id)
            .map(|(_, _, published_at, _)| *published_at)
            .collect(),
        false => Vec::new(),
    };
    published.sort_unstable();
    let created_at = *published.first()?;
    let updated_at = *published.last()?;
    Some((
        DateTime::from_secs(created_at),
        DateTime::from_secs(updated_at),
    ))
}

/// The newest generation the catalogue publishes for the base image `image_id`: the
/// published order is newest first ([`published_managed_base_image_versions`]), so this
/// is the first row `ListManagedMicrovmImageVersions` serves for it.
///
/// A published-order ACCESSOR, not a resolution rule: nothing resolves a request
/// against it (see
/// [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::resolve_base_image_version`] for
/// what an unpinned build records), and the listing's own order check reads it.
pub fn latest_managed_base_image_version(image_id: &str) -> Option<&'static str> {
    published_managed_base_image_versions(image_id)
        .into_iter()
        .next()
}

/// The order versions of one image sort in, oldest first: as the `major.minor` pair the
/// version names, so `1.10` follows `1.9` rather than preceding it (see
/// [`FIRST_IMAGE_VERSION`]), and as its own integer for a plain generation counter, so
/// generation `10` follows generation `9` (see [`MANAGED_BASE_IMAGE_VERSIONS`]). A
/// version in neither form (nothing this service publishes is) sorts after every
/// version that is, by its own text, so the order is total whatever is stored.
pub fn version_order(version: &str) -> (u64, u64, String) {
    let (major, minor) = parse_image_version(version)
        .or_else(|| version.parse().ok().map(|generation| (generation, 0)))
        .unwrap_or((u64::MAX, u64::MAX));
    (major, minor, version.to_string())
}

/// The ARN account field an AWS-OWNED resource of this service carries, as against an
/// account's own resources, which carry that account's id.
pub const MANAGED_RESOURCE_ACCOUNT: &str = "aws";

/// What a `MicrovmImageIdentifier` resolves to.
///
/// The three outcomes are client-visible and mean different things: a resolved
/// identifier is served, a well-formed ARN this account holds nothing under is a
/// not-found (which a poll may legitimately retry), and anything else is a malformed
/// request that can never succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageReference {
    /// The `name` key of the image the identifier names.
    Resolved(String),
    /// A well-formed ARN (or a bare id) that this account holds no image under.
    Absent,
    /// Neither an id this account holds nor a well-formed ARN.
    Malformed,
}

/// What a customer-facing call addressed at a `MicrovmImageIdentifier` is answered
/// with once the AUTHORIZATION verdict is taken — see
/// [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::resolve_image_access`].
///
/// The four outcomes are client-visible and mean different things: a denial says the
/// caller may not read the identifier at all (and says NOTHING about whether it names
/// anything), an absence says this account's own record is not there — the condition a
/// poll may legitimately retry — and a malformed identifier is a request that can never
/// succeed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageAccess {
    /// The `name` key of the image to serve.
    Granted(String),
    /// An identifier of this account's own that names no record of it any more.
    Absent,
    /// An identifier this account holds no record of, now or before.
    Denied,
    /// Neither an id this account holds nor a well-formed ARN.
    Malformed,
}

/// The catalogue id of the AWS-MANAGED base image a `MicrovmImageIdentifier` names.
///
/// `MicrovmImageIdentifier` is "The ARN or ID of the MicroVmImage" wherever it
/// appears, so a managed image is addressed by the SAME two forms an account's own
/// image is ([`ComAmazonawsLambdamicrovmsLambdamicrovmsState::resolve_image_reference`]),
/// resolved against the published catalogue rather than against this account's store:
/// the catalogue id first, then the AWS-owned ARN that carries it. A well-formed ARN
/// the catalogue does not publish is [`ImageReference::Absent`] — a resource that is
/// missing — while a value that is neither a published id nor an ARN addresses nothing
/// this API can name and is [`ImageReference::Malformed`].
pub fn resolve_managed_base_image(identifier: &str) -> ImageReference {
    if is_published_managed_base_image(identifier) {
        return ImageReference::Resolved(identifier.to_string());
    }
    if let Some(image_id) = managed_microvm_image_id(identifier) {
        return match is_published_managed_base_image(image_id) {
            true => ImageReference::Resolved(image_id.to_string()),
            false => ImageReference::Absent,
        };
    }
    match ::service_common::arn::parse(identifier).is_some() {
        true => ImageReference::Absent,
        false => ImageReference::Malformed,
    }
}

/// The `DEFAULT_CPU_ARCHITECTURE` a MicroVM image is built for when the request names
/// no `cpuConfigurations`.
///
/// `Architecture` is an enum whose only value is `ARM_64`
/// (`smithy_query(query="shape",shape="Architecture")`), so the configuration the
/// service normalizes an unspecified request to is the only one it can name.
pub const DEFAULT_CPU_ARCHITECTURE: &str = "ARM_64";

/// The `minimumMemoryInMiB` a MicroVM image gets when the request names no
/// `resources`.
///
/// `Resources$minimumMemoryInMiB` is `@required` within the structure and
/// `ResourcesList` is `@length(max: 1)`, so an accepted image carries exactly one
/// resource floor; AWS publishes no figure for the default, so this one is recorded
/// AWS behaviour rather than modelled.
pub const DEFAULT_MINIMUM_MEMORY_IN_MIB: i32 = 2048;

/// The prefix of the CloudWatch log group a MicroVM image's logs go to when the
/// request names no `logging`.
///
/// `Logging` is a union of `cloudWatch` and `disabled` and declares no `@default`, so
/// a request that names neither gets the service's own target: a log group under this
/// service's own namespace, named for the image. The prefix is recorded AWS behaviour.
pub const DEFAULT_LOG_GROUP_PREFIX: &str = "/aws/lambda-microvms";

/// The `egressNetworkConnectors` a build request's own list resolves to: the
/// connectors it asked for, or the AWS-managed egress connector in `region` when it
/// asked for none.
///
/// `egressNetworkConnectors` are "the list of egress network connectors available to
/// the MicroVM at runtime", so an accepted version records the connectors the MicroVM
/// will actually have rather than nothing — see
/// [`DEFAULT_EGRESS_NETWORK_CONNECTOR`]. The resolution is the one every operation
/// that mints a version performs, so the versions a create and an update persist
/// cannot disagree about it.
pub fn resolve_egress_network_connectors(
    region: &str,
    requested: Option<Vec<String>>,
) -> Vec<String> {
    match requested {
        Some(connectors) if !connectors.is_empty() => connectors,
        _ => vec![managed_network_connector_arn(
            region,
            DEFAULT_EGRESS_NETWORK_CONNECTOR,
        )],
    }
}

/// The `cpuConfigurations` a build request's own list resolves to: the configurations
/// it asked for, or the service's own ([`default_cpu_configurations`]) when it asked
/// for none. Resolved for every minted version, as
/// [`resolve_egress_network_connectors`] is.
pub fn resolve_cpu_configurations(
    requested: Option<Vec<CpuConfiguration>>,
) -> Vec<CpuConfiguration> {
    match requested {
        Some(configurations) if !configurations.is_empty() => configurations,
        _ => default_cpu_configurations(),
    }
}

/// The `resources` a build request's own list resolves to: the requirement it asked
/// for, or the service's own floor ([`default_resources`]) when it asked for none.
/// Resolved for every minted version, as [`resolve_egress_network_connectors`] is.
pub fn resolve_resources(requested: Option<Vec<Resources>>) -> Vec<Resources> {
    match requested {
        Some(resources) if !resources.is_empty() => resources,
        _ => default_resources(),
    }
}

/// The `logging` a build request resolves to for the image named `name`: the
/// configuration it asked for, or this service's own ([`default_logging`]) when it
/// named neither arm of the union. Resolved for every minted version, as
/// [`resolve_egress_network_connectors`] is.
pub fn resolve_logging(name: &str, requested: Option<Logging>) -> Logging {
    requested.unwrap_or_else(|| default_logging(name))
}

/// The `cpuConfigurations` an image gets when the request names none.
pub fn default_cpu_configurations() -> Vec<CpuConfiguration> {
    vec![CpuConfiguration {
        architecture: DEFAULT_CPU_ARCHITECTURE.to_string(),
    }]
}

/// The `resources` an image gets when the request names none.
pub fn default_resources() -> Vec<Resources> {
    vec![Resources {
        minimum_memory_in_mi_b: DEFAULT_MINIMUM_MEMORY_IN_MIB,
    }]
}

/// The `logging` an image named `name` gets when the request names none: its logs go
/// to this service's own CloudWatch log group for that image.
pub fn default_logging(name: &str) -> Logging {
    Logging::cloudWatch(CloudWatchLogging {
        log_group: Some(default_log_group(name)),
        log_stream: None,
    })
}

/// The CloudWatch log group an image named `name` logs to by default.
///
/// `CloudWatchLogging$logGroup` is `@length(1..512)` with pattern
/// `^[a-zA-Z0-9_\-/.#]+$`, which `ImageName` (`^[a-zA-Z0-9-_]+$`, 1..64 characters)
/// under this prefix always satisfies.
pub fn default_log_group(name: &str) -> String {
    format!("{DEFAULT_LOG_GROUP_PREFIX}/{name}")
}

/// The `message` of the 404 a MicroVM lookup answers, built from the
/// `microvmIdentifier` the request carried.
pub fn microvm_not_found_message(identifier: &str) -> String {
    format!("MicroVM not found: {identifier}")
}

/// The `message` of the 400 a caller-requested run-state transition answers for a
/// MicroVM whose current state has no such transition
/// ([`MicrovmTransitionVerdict::Refused`]).
///
/// The sentence states the condition — the transition cannot be made from the state the
/// record is in — and names the MicroVM by the ARN this service minted for it, the one
/// identity of the record whichever form `MicrovmIdentifier` the request addressed it
/// by. That is the same template, in the same error class, as the image path's own state
/// refusal ([`image_not_updatable_message`]), with the transition's own verb
/// ([`MicrovmTransition::verb`]).
pub fn microvm_not_transitionable_message(verb: &str, microvm_arn: &str) -> String {
    format!("Cannot {verb} MicroVM in its current state: {microvm_arn}")
}

/// The `message` of the 400 a caller-requested run-state transition answers for a
/// MicroVM that has entered its terminal lifecycle
/// ([`MicrovmTransitionVerdict::Terminated`]).
///
/// Terminating is the one move no other leads out of, so the refusal states that rather
/// than the transition that was asked for, and names the MicroVM by the `microvmId` of
/// the record (recorded AWS behaviour).
///
/// A MicroVM whose termination is still in flight is answered the same sentence as one
/// that has reached `TERMINATED`: the termination is what took the record out of the
/// run-state machine, and it is one-way, so the state cannot be changed whether or not
/// that move has settled yet.
pub fn microvm_terminated_message(microvm_id: &str) -> String {
    format!("The MicroVM {microvm_id} has been terminated and its state cannot be changed.")
}

/// The `message` of the 404 `RunMicrovm` answers when it can resolve no ACTIVE
/// version for the image the request names — a different condition from a missing
/// image, and the one whose remediation is to publish or wait for a version.
pub fn no_active_version_message(image: &str) -> String {
    format!("No active version found for MicroVM image {image}")
}

/// The `message` of the 404 `RunMicrovm` answers when the version it resolved holds no
/// build that produced an image.
///
/// A version's STATUS says whether it may be launched from and its STATE says what its
/// build produced, so a version published ACTIVE whose builds all failed resolves
/// normally and then has nothing to launch: the refusal names the missing BUILD, for the
/// image and version the launch resolved, rather than reporting the version absent.
pub fn no_image_build_message(image_arn: &str, image_version: &str) -> String {
    format!("No image build found for MicroVM image {image_arn} and version {image_version}")
}

/// The `message` of the 404 a version-addressed call answers when the image it resolved
/// carries no such `imageVersion`, built from the image the request named and that
/// version.
pub fn image_version_not_found_message(image: &str, image_version: &str) -> String {
    format!("MicroVM image {image} has no version {image_version}")
}

/// The `message` of the 404 a build-addressed call answers when the image version it
/// resolved holds no build with the `buildId` the request named.
///
/// The sentence is AWS's own for this error class: the fixed text `Build not found for
/// MicroVMImageID: `, `, VersionID: ` and `, BuildID: `, with the three variable parts
/// taken from the path labels the request carried — the image identifier, the version and
/// the build id — so the refusal names the whole address the build was looked for at
/// (recorded AWS behaviour).
pub fn image_build_not_found_message(image: &str, image_version: &str, build_id: &str) -> String {
    format!("Build not found for MicroVMImageID: {image}, VersionID: {image_version}, BuildID: {build_id}")
}

/// The `message` of the 403 a read answers for a resource the caller may not read.
///
/// `AccessDeniedException`'s documented sentence ("You do not have sufficient access to
/// perform this action.") describes the error CLASS; what AWS puts on the wire for this
/// refusal names the CALLING PRINCIPAL — "User: &lt;caller&gt; is not authorized to perform
/// this operation", whose one variable part is the identity that signed the request
/// (recorded AWS behaviour). That identity is this emulator's own published principal,
/// the same one `sts:GetCallerIdentity` answers with
/// (`service_common::caller_principal`), so the sentence carries nothing of the request
/// and a denial says nothing about what the identifier it refused names.
pub fn access_denied_message() -> String {
    format!(
        "User: {} is not authorized to perform this operation",
        ::service_common::caller_principal::ambient_user_arn()
    )
}

/// The version the NEXT build of an image is minted as, given the versions it
/// already holds.
///
/// Versions are `major.minor` (see [`FIRST_IMAGE_VERSION`]) and a build of an image
/// mints the NEXT MAJOR of the versions it holds, with a minor of `0`: the first build
/// is `1.0`, the next is `2.0`, and so on — a build replaces the whole configuration of
/// the image (`UpdateMicrovmImage` "uses PUT semantics"), so the version it mints is a
/// new generation of the image rather than a point revision of the generation before
/// it. A stored version that is not in the `major.minor` form (nothing this service
/// mints is) is ignored rather than crashing the mint.
pub fn next_image_version<'a>(existing: impl IntoIterator<Item = &'a str>) -> String {
    let highest = existing
        .into_iter()
        .filter_map(parse_image_version)
        .map(|(major, _)| major)
        .max();
    match highest {
        Some(major) => format!("{}.0", major + 1),
        None => FIRST_IMAGE_VERSION.to_string(),
    }
}

/// `version` as its `(major, minor)` pair, or `None` when it is not in that form.
fn parse_image_version(version: &str) -> Option<(u64, u64)> {
    let (major, minor) = version.split_once('.')?;
    Some((major.parse().ok()?, minor.parse().ok()?))
}

/// Whether a MicroVM in `state` can still be connected to, which is what an
/// authentication token is for (`CreateMicrovmAuthToken`: "an authentication token
/// for accessing a running MicroVM").
///
/// A MicroVM on its way out, or already gone, never accepts another connection, so a
/// token for it would grant access to nothing.
pub fn is_connectable_microvm_state(state: &str) -> bool {
    state != MICROVM_STATE_TERMINATING && state != MICROVM_STATE_TERMINATED
}

/// A `MicrovmImageVersionState` value (`MICROVM_IMAGE_VERSION_STATE_VALUES`).
pub const VERSION_STATE_IN_PROGRESS: &str = "IN_PROGRESS";
/// A `MicrovmImageVersionState` value (`MICROVM_IMAGE_VERSION_STATE_VALUES`).
pub const VERSION_STATE_SUCCESSFUL: &str = "SUCCESSFUL";
/// A `MicrovmImageVersionState` value (`MICROVM_IMAGE_VERSION_STATE_VALUES`): the state a
/// version settles in when the build of it did not produce an image.
pub const VERSION_STATE_FAILED: &str = "FAILED";

/// A `MicrovmImageVersionStatus` value (`MICROVM_IMAGE_VERSION_STATUS_VALUES`).
pub const VERSION_STATUS_ACTIVE: &str = "ACTIVE";
/// A `MicrovmImageVersionStatus` value (`MICROVM_IMAGE_VERSION_STATUS_VALUES`).
pub const VERSION_STATUS_INACTIVE: &str = "INACTIVE";

/// A `BuildState` value (`BUILD_STATE_VALUES`): the build is running.
pub const BUILD_STATE_IN_PROGRESS: &str = "IN_PROGRESS";
/// A `BuildState` value (`BUILD_STATE_VALUES`): the build produced a snapshot.
pub const BUILD_STATE_SUCCESSFUL: &str = "SUCCESSFUL";
/// A `BuildState` value (`BUILD_STATE_VALUES`): the build produced nothing, so it has a
/// `stateReason` ("the reason for the build state, if applicable") and no snapshot.
pub const BUILD_STATE_FAILED: &str = "FAILED";

/// One target an image version's asynchronous build produces a build record for: a CPU
/// architecture, the chipset that architecture runs on, and the generation of that
/// chipset the build targets.
#[derive(Debug, Clone, Copy)]
pub struct BuildTarget {
    /// An `Architecture` (`ARCHITECTURE_VALUES`).
    pub architecture: &'static str,
    /// A `Chipset` (`CHIPSET_VALUES`).
    pub chipset: &'static str,
    /// The chipset generation, as `MicrovmImageBuildSummary$chipsetGeneration` reports
    /// it: the generation NUMBER of [`Self::chipset`], as a decimal string.
    pub chipset_generation: &'static str,
}

/// The targets this service builds a MicroVM image version for — one build record each.
///
/// `Architecture` enumerates `ARM_64` only and `Chipset` enumerates `GRAVITON` only
/// (`smithy_query(query="shape",shape="Architecture,Chipset")`), so an `ARM_64` image is
/// built for `GRAVITON`; a build is per chipset GENERATION, so one scheduled build fans
/// out across the generations the service builds that chipset for.
/// `chipsetGeneration` is "The target chipset generation for the build." and is modelled
/// SEPARATELY from `chipset` ("The target chipset for the build."), so the chipset FAMILY
/// lives in `chipset` and this member carries only the generation of it, as a bare
/// decimal string — the same form `ListMicrovmImageBuilds` filters on through its
/// `chipsetGeneration` httpQuery parameter, so a client can feed a generation it was
/// told straight back as a filter. It is a free-form `NonBlankString` the model
/// publishes no values for, so WHICH generations are built is chosen (like
/// [`IMAGE_BUILD_SPAN`]) rather than modelled.
pub const BUILD_TARGETS: &[BuildTarget] = &[
    BuildTarget {
        architecture: DEFAULT_CPU_ARCHITECTURE,
        chipset: "GRAVITON",
        chipset_generation: "2",
    },
    BuildTarget {
        architecture: DEFAULT_CPU_ARCHITECTURE,
        chipset: "GRAVITON",
        chipset_generation: "3",
    },
];

/// Bytes in one MiB: `Resources$minimumMemoryInMiB` is in MiB while
/// `SnapshotBuild$memorySnapshotSizeInBytes` is in bytes.
const BYTES_PER_MIB: i64 = 1024 * 1024;

/// The snapshot a SUCCESSFUL build of a version configured with `resources` produces,
/// or `None` when the version carries no resource requirement to size it from.
///
/// `SnapshotBuild` is "size information about a MicroVM image snapshot build": the
/// memory snapshot of a MicroVM that is configured with `minimumMemoryInMiB` of memory
/// is that much memory, which is the one size this emulator holds the input for. It
/// holds no artifact bytes and no disk image, so `codeInstallSizeInBytes` and
/// `diskSnapshotSizeInBytes` — both optional — are left unset rather than invented.
pub fn snapshot_build_of(resources: &[Resources]) -> Option<SnapshotBuild> {
    let memory_in_mib = resources
        .iter()
        .map(|resource| i64::from(resource.minimum_memory_in_mi_b))
        .max()?;
    Some(SnapshotBuild {
        memory_snapshot_size_in_bytes: Some(memory_in_mib * BYTES_PER_MIB),
        code_install_size_in_bytes: None,
        disk_snapshot_size_in_bytes: None,
    })
}

/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_PENDING: &str = "PENDING";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_RUNNING: &str = "RUNNING";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`): the state a MicroVM carries while
/// the suspend it was asked for is in flight.
pub const MICROVM_STATE_SUSPENDING: &str = "SUSPENDING";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`): the state a suspended MicroVM
/// rests in, with its memory and disk state preserved.
pub const MICROVM_STATE_SUSPENDED: &str = "SUSPENDED";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_TERMINATING: &str = "TERMINATING";
/// A `MicrovmState` value (`MICROVM_STATE_VALUES`).
pub const MICROVM_STATE_TERMINATED: &str = "TERMINATED";

/// A run-state transition a caller can ask one MicroVM to make.
///
/// The two are each other's inverse — `SuspendMicrovm` "Suspends a running MicroVM,
/// preserving its full memory and disk state" and `ResumeMicrovm` "Resumes a suspended
/// MicroVM, restoring it to RUNNING state" — so both are decided by the one rule in
/// [`microvm_transition_verdict`] and applied by [`start_microvm_transition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrovmTransition {
    /// `SuspendMicrovm`: the MicroVM comes to rest in [`MICROVM_STATE_SUSPENDED`].
    Suspend,
    /// `ResumeMicrovm`: the MicroVM runs again, in [`MICROVM_STATE_RUNNING`].
    Resume,
}

impl MicrovmTransition {
    /// The verb a refusal of this transition names it by — see
    /// [`microvm_not_transitionable_message`].
    pub fn verb(self) -> &'static str {
        match self {
            Self::Suspend => "suspend",
            Self::Resume => "resume",
        }
    }
}

/// What the service does with a caller-requested [`MicrovmTransition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MicrovmTransitionVerdict {
    /// The MicroVM moves: [`start_microvm_transition`] applies it.
    Start,
    /// The MicroVM already has the run state the transition asks for, so the
    /// `@idempotent` call succeeds and leaves the record exactly as it is.
    NoOp,
    /// The MicroVM has entered its terminal lifecycle, so its run state can be changed
    /// no further at all: the call is refused with [`microvm_terminated_message`].
    Terminated,
    /// The MicroVM's current state has no such transition, so the call is refused —
    /// like every other refusal this service states from the current state of a record,
    /// with the 400 `ValidationException`
    /// ([`microvm_not_transitionable_message`]) rather than a 409.
    Refused,
}

/// What a caller-requested `transition` does to a MicroVM currently in `state`.
///
/// A MicroVM's run state is reached from the run state the transition starts in, so a
/// transition is served only where that start state holds: `SuspendMicrovm` from a
/// running MicroVM and `ResumeMicrovm` from a suspended one. Both operations are
/// `@idempotent`, so a MicroVM that already has — or is already on its way to — the run
/// state asked for is left alone and the call still succeeds; `ResumeMicrovm` therefore
/// takes a MicroVM whose suspend has not finished settling, because the state it is
/// settling into is the one resume starts from. A MicroVM that has entered its terminal
/// lifecycle has no run state left to change — terminating is the one move no other
/// leads out of — and every remaining state is one no run-state transition starts from:
/// a MicroVM still provisioning has no preserved state to suspend and none to restore.
pub fn microvm_transition_verdict(
    transition: MicrovmTransition,
    state: &str,
) -> MicrovmTransitionVerdict {
    match (transition, state) {
        (_, MICROVM_STATE_TERMINATING | MICROVM_STATE_TERMINATED) => {
            MicrovmTransitionVerdict::Terminated
        }
        (MicrovmTransition::Suspend, MICROVM_STATE_RUNNING) => MicrovmTransitionVerdict::Start,
        (MicrovmTransition::Suspend, MICROVM_STATE_SUSPENDING | MICROVM_STATE_SUSPENDED) => {
            MicrovmTransitionVerdict::NoOp
        }
        (MicrovmTransition::Resume, MICROVM_STATE_SUSPENDING | MICROVM_STATE_SUSPENDED) => {
            MicrovmTransitionVerdict::Start
        }
        (MicrovmTransition::Resume, MICROVM_STATE_RUNNING) => MicrovmTransitionVerdict::NoOp,
        _ => MicrovmTransitionVerdict::Refused,
    }
}

/// Apply the `transition` [`microvm_transition_verdict`] said STARTS to `vm`, at `now`.
///
/// A suspend is reported in flight and the rest it settles into is owed by the clock
/// ([`MICROVM_SUSPEND_SPAN`]), so a caller reading straight after the call still
/// observes `SUSPENDING`. `MicrovmState` names no transient state on the way back, and
/// resume "restor[es] it to RUNNING state with all state intact", so a resume is
/// complete when it answers and it drops the suspend the clock still owed.
///
/// Neither direction touches [`Microvm::expiry_due_at`]: `maximumDurationInSeconds` is
/// "the maximum duration in seconds that the MicroVM can EXIST before being terminated by
/// the platform", and a suspended MicroVM still exists, so that clock runs through a
/// suspend and can terminate the MicroVM while it rests.
pub fn start_microvm_transition(vm: &mut Microvm, transition: MicrovmTransition, now: Instant) {
    match transition {
        MicrovmTransition::Suspend => {
            vm.state = MICROVM_STATE_SUSPENDING.to_string();
            vm.suspend_due_at = Some(now + MICROVM_SUSPEND_SPAN);
        }
        MicrovmTransition::Resume => {
            vm.state = MICROVM_STATE_RUNNING.to_string();
            vm.suspend_due_at = None;
        }
    }
}

/// One build of one MicroVM image version: the target it was built for and the state
/// of that build. Held in [`MicrovmImageVersion::builds`], keyed by its `buildId`.
#[derive(Debug, Clone)]
pub struct MicrovmImageBuild {
    pub build_id: String,
    /// A `BuildState`.
    pub state: String,
    /// An `Architecture`.
    pub architecture: String,
    /// A `Chipset`.
    pub chipset: String,
    pub chipset_generation: String,
    pub state_reason: Option<String>,
    pub created_at: DateTime,
    /// The snapshot the build produced; absent until the build succeeds.
    pub snapshot_build: Option<SnapshotBuild>,
}

/// One version of a MicroVM image: the build inputs the create (or an update)
/// carried, plus the state of that build.
#[derive(Debug, Clone)]
pub struct MicrovmImageVersion {
    pub image_version: String,
    /// A `MicrovmImageVersionState`.
    pub state: String,
    /// A `MicrovmImageVersionStatus`.
    pub status: String,
    pub base_image_arn: String,
    pub base_image_version: Option<String>,
    pub build_role_arn: String,
    pub description: Option<String>,
    pub code_artifact: CodeArtifact,
    pub logging: Option<Logging>,
    pub egress_network_connectors: Vec<String>,
    pub cpu_configurations: Vec<CpuConfiguration>,
    pub resources: Vec<Resources>,
    pub additional_os_capabilities: Vec<String>,
    pub hooks: Option<Hooks>,
    pub environment_variables: BTreeMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: Option<DateTime>,
    pub tags: BTreeMap<String, String>,
    /// When the asynchronous build completes; `None` once it has.
    pub build_due_at: Option<Instant>,
    /// Why the build of this version cannot produce an image, as the object store
    /// ANSWERED when the build was scheduled ([`code_artifact::CodeArtifactVerdict`]);
    /// `None` for a build whose code artifact the store served. The verdict is settled
    /// on the record by [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::materialize_due`]
    /// when the clock makes the build due, so a caller reading before then still
    /// observes the build in flight.
    pub build_failure_reason: Option<String>,
    /// When this version's state last SETTLED — the instant the build it was waiting on
    /// ended, whichever branch it ended on. `None` while the build is still in flight,
    /// which is why a version reports no state-time bucket until then
    /// ([`Self::reported_state_time_bucket`]).
    pub state_settled_at: Option<DateTime>,
    /// The per-target builds this version's asynchronous build fans out into, keyed by
    /// `buildId` — see [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::mint_builds`].
    pub builds: BTreeMap<String, MicrovmImageBuild>,
    /// The GUEST this version's build produced: the program held in the code artifact the
    /// object store served ([`guest_program`]), which is what a MicroVM run from this
    /// version executes behind its published endpoint. `None` when the artifact was not
    /// served at all, or holds no program this emulator's engine can run.
    pub guest: Option<::std::sync::Arc<guest_program::GuestProgram>>,
}

impl MicrovmImageVersion {
    /// The `stateReason` every response reporting this version answers with.
    ///
    /// "The reason for the current state. For example, one or more builds failed": a
    /// version's state is the settlement of its per-target builds, so the reason it
    /// reports is the reason one of those builds recorded, and a version none of whose
    /// builds recorded one has nothing to report. The ONE projection of the stored
    /// reason, so the read of a version and a listing row for it cannot disagree.
    pub fn reported_state_reason(&self) -> Option<String> {
        self.builds
            .values()
            .find_map(|build| build.state_reason.clone())
    }

    /// The `versionStateTimeBucket` a response reporting this version answers with: the
    /// time bucket the version's state settled in ([`state_time_bucket`]), and nothing
    /// at all while the build it is waiting on is still in flight.
    ///
    /// The member is recorded AWS behaviour rather than modelled (the shapes reporting a
    /// version declare no such member), so it is written beside the modelled payload —
    /// see [`crate::ops::types::RecordedMembers`]. The ONE projection of the stored
    /// settlement instant, so a read of a version and a listing row for it cannot
    /// disagree.
    pub fn reported_state_time_bucket(&self) -> Option<String> {
        state_time_bucket(self.state_settled_at?)
    }

    /// This version's state-time bucket, as the recorded-only member a response writes
    /// beside its modelled payload under the member schema `member` — empty for a
    /// version whose state has not settled yet.
    pub fn recorded_state_time_bucket_member(
        &self,
        member: &'static ::aws_smithy_schema::Schema,
    ) -> Vec<RecordedMember> {
        self.reported_state_time_bucket()
            .map(|bucket| RecordedMember::Text(member, bucket))
            .into_iter()
            .collect()
    }
}

/// How long one `versionStateTimeBucket` spans.
///
/// A BUCKET is an interval rather than an instant: the member is recorded AWS behaviour
/// and AWS publishes neither its width nor its form, so the width is chosen — the hour
/// the state settled in, the coarsest bucket a client polling a build can still tell two
/// builds of one day apart by.
pub const VERSION_STATE_TIME_BUCKET: Duration = Duration::from_secs(60 * 60);

/// The time bucket the instant `at` falls in: the start of its
/// [`VERSION_STATE_TIME_BUCKET`]-wide interval, as the ISO-8601 instant every timestamp
/// of this service's JSON bodies is written as. `None` when the instant cannot be
/// rendered, which leaves the member unwritten rather than reported as something else.
pub fn state_time_bucket(at: DateTime) -> Option<String> {
    let width = i64::try_from(VERSION_STATE_TIME_BUCKET.as_secs()).ok()?;
    let start = at.secs() - at.secs().rem_euclid(width);
    DateTime::from_secs(start)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .ok()
}

/// A MicroVM image, keyed in [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::images`] by its name.
#[derive(Debug)]
pub struct MicrovmImage {
    /// The image's own identity, minted with the record and never written again — see
    /// [`mint_microvm_image_id`]. Every response that reports this record reports this
    /// one value ([`Self::recorded_id_member`]), so a client can key its own records on
    /// it and join a later read or listing back to the create.
    pub id: String,
    pub image_arn: String,
    pub name: String,
    /// A `MicrovmImageState`.
    pub state: String,
    pub created_at: DateTime,
    pub updated_at: Option<DateTime>,
    pub latest_active_image_version: Option<String>,
    pub latest_failed_image_version: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub versions: BTreeMap<String, MicrovmImageVersion>,
    /// When this image's asynchronous deletion removes it; `None` while no delete is
    /// in flight.
    pub delete_due_at: Option<Instant>,
}

impl MicrovmImage {
    /// This image's `id`, as the recorded-only member a response writes beside its
    /// modelled payload, under the member schema `member` of the shape answering.
    ///
    /// The ONE projection of the stored id, so the create that minted the image, a
    /// later read of it and a listing row for it cannot report different identities —
    /// and so a client that keyed its own records on the create's id finds the same
    /// value in every later answer. See [`crate::ops::types::RecordedMembers`] for why
    /// the member is written beside the payload rather than on it.
    pub fn recorded_id_member(
        &self,
        member: &'static ::aws_smithy_schema::Schema,
    ) -> Vec<RecordedMember> {
        vec![RecordedMember::Text(member, self.id.clone())]
    }

    /// Publish `status` for this image's version `image_version`, at `at`.
    ///
    /// `MicrovmImageVersionStatus` is "the availability status of the version: ACTIVE
    /// (can be used by `RunMicrovm`) or INACTIVE (blocked from launching new MicroVMs)",
    /// and `latestActiveImageVersion` is "the latest active version of the MicroVM
    /// image" — so publishing a status is what moves that pointer, and exactly one
    /// version of an image is ACTIVE: the one just published takes the pointer and the
    /// one that held it steps down. Taking the status off the version the image was
    /// serving leaves the image with no active version, which is the state a launch that
    /// pins none is then refused from.
    ///
    /// The STATE of a version is what its build settled on and is NOT touched here: the
    /// status says whether the version may be launched from, which a caller sets
    /// whatever that build produced. The image's own state settles to
    /// [`IMAGE_STATE_UPDATED`] because publishing a status is an update of the image —
    /// unless a lifecycle of the image is still in flight, which owns that state until
    /// the work it started ends ([`is_image_lifecycle_in_flight`]).
    pub fn publish_version_status(&mut self, image_version: &str, status: &str, at: DateTime) {
        let Some(version) = self.versions.get_mut(image_version) else {
            return;
        };
        version.status = status.to_string();
        version.updated_at = Some(at);

        if status == VERSION_STATUS_ACTIVE {
            if let Some(previous) = self
                .latest_active_image_version
                .replace(image_version.to_string())
            {
                if previous != image_version {
                    if let Some(stepped_down) = self.versions.get_mut(&previous) {
                        stepped_down.status = VERSION_STATUS_INACTIVE.to_string();
                        stepped_down.updated_at = Some(at);
                    }
                }
            }
        } else if self.latest_active_image_version.as_deref() == Some(image_version) {
            self.latest_active_image_version = None;
        }

        self.updated_at = Some(at);
        if !is_image_lifecycle_in_flight(&self.state) {
            self.state = IMAGE_STATE_UPDATED.to_string();
        }
    }
}

/// Mint a MicroVM image's `id`: the identity the service hands back for the image it
/// just created, and answers for that image from then on.
///
/// A lowercase hyphenated 8-4-4-4-12 version-4 UUID, minted once per image and stored
/// on the record, so it is unique across the images this store holds and stable across
/// every read of one. The TEXT form is the workspace's one rendering of 16 opaque bytes
/// as such a UUID (`service_common::uuid_from_random_bytes`, which computes the version
/// and variant bits and the group boundaries with the maintained `uuid` crate), so
/// neither the bits nor the boundaries are spelled out here.
///
/// The bytes are this store's own ([`minted_bytes`]): an identity is not a credential,
/// so what it needs is to be unique, not unpredictable.
pub fn mint_microvm_image_id() -> String {
    let mut id = [0u8; 16];
    id.copy_from_slice(&minted_bytes()[..16]);
    ::service_common::uuid_from_random_bytes(id)
}

/// 32 opaque bytes for one minted identifier.
///
/// A digest over a process-wide counter (what makes two mints of the same instant
/// differ), the wall clock (what makes two runs differ) and one keyed hash of both from
/// the standard library's own randomly seeded hasher. The digest is the `sha2` this
/// crate already carries for the MicroVM endpoint's token digests, so nothing here
/// hand-writes a hash.
fn minted_bytes() -> [u8; 32] {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use ::sha2::{Digest, Sha256};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    let mut keyed = std::collections::hash_map::RandomState::new().build_hasher();
    keyed.write_u64(seq);
    keyed.write_u128(nanos);

    let mut digest = Sha256::new();
    digest.update(seq.to_be_bytes());
    digest.update(nanos.to_be_bytes());
    digest.update(keyed.finish().to_be_bytes());
    digest.finalize().into()
}

/// A running (or terminated) MicroVM, keyed in
/// [`ComAmazonawsLambdamicrovmsLambdamicrovmsState::microvms`] by its `microvmId`.
#[derive(Debug, Clone)]
pub struct Microvm {
    pub microvm_id: String,
    /// This MicroVM's own ARN, so a `microvmIdentifier` given as an ARN resolves
    /// against the ARN this service minted rather than against any ARN whose last
    /// segment happens to match a local id.
    pub microvm_arn: String,
    /// A `MicrovmState`.
    pub state: String,
    pub endpoint: String,
    pub image_arn: String,
    pub image_version: String,
    pub execution_role_arn: Option<String>,
    pub idle_policy: Option<IdlePolicy>,
    pub logging: Option<Logging>,
    pub maximum_duration_in_seconds: i32,
    pub started_at: DateTime,
    pub terminated_at: Option<DateTime>,
    pub state_reason: Option<String>,
    pub ingress_network_connectors: Vec<String>,
    pub egress_network_connectors: Vec<String>,
    /// The address of the MicroVM's OWN network interface, assigned when it starts: a
    /// link-local address ([`microvm_network::link_local_address`]), which is what the
    /// guest observes as the local end of a connection one of its egress connectors
    /// carries.
    pub link_local_address: String,
    /// When `PENDING` -> `RUNNING` becomes due; `None` once it has.
    pub provision_due_at: Option<Instant>,
    /// When `SUSPENDING` -> `SUSPENDED` becomes due; `None` once it has, and once a
    /// resume or a termination has overtaken the suspend.
    pub suspend_due_at: Option<Instant>,
    /// When `TERMINATING` -> `TERMINATED` becomes due; `None` once it has.
    pub terminate_due_at: Option<Instant>,
    /// When `maximumDurationInSeconds` runs out and the platform terminates this
    /// MicroVM; `None` once it has, or once the MicroVM is already ending.
    pub expiry_due_at: Option<Instant>,
    /// When this `TERMINATED` record is reclaimed, so terminated MicroVMs do not
    /// accumulate for the process's lifetime; set from the termination instant, and
    /// `None` while the MicroVM has not terminated.
    pub reclaim_due_at: Option<Instant>,
}

/// What an `@idempotencyToken` replay must return instead of minting a second
/// resource.
#[derive(Debug, Clone)]
pub enum MintedResource {
    /// `CreateMicrovmImage`: the image ARN and the version that call minted.
    Image {
        image_arn: String,
        image_version: String,
    },
    /// `RunMicrovm`: the MicroVM that call minted.
    Microvm { microvm_id: String },
}

/// A `clientToken` record: what that token minted, and when it stops replaying.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub minted: MintedResource,
    pub expires_at: Instant,
}

/// Per-service state for com-amazonaws-lambdamicrovms-lambdamicrovms. A handler reaches it through [`service_state`].
///
/// Every resource this service mints is REGIONAL, so the records live in one
/// [`regions::RegionalStore`] per region and every lookup is scoped to the region the
/// request was signed for ([`request_region`]) — see the [`regions`] module.
///
/// Every map here is bounded: each region's `images` by [`MAX_MICROVM_IMAGES`], its
/// `microvms` by [`MAX_ACTIVE_MICROVMS`] plus the [`TERMINATED_MICROVM_RETENTION`]
/// reclaim, and its `idempotency` by [`MAX_IDEMPOTENCY_RECORDS`] and
/// [`IDEMPOTENCY_RETENTION`].
#[derive(Debug, Default)]
pub struct ComAmazonawsLambdamicrovmsLambdamicrovmsState {
    /// The records of each region, keyed by the region they were created in. A region
    /// nothing was ever created in holds no entry at all.
    pub regions: BTreeMap<String, regions::RegionalStore>,
    /// The live authentication tokens a published MicroVM endpoint accepts, keyed by
    /// the DIGEST of the token rather than by the token itself
    /// ([`microvm_endpoint::auth_token_digest`]) — the caller keeps the only copy of
    /// the secret. Bounded by [`microvm_endpoint::MAX_LIVE_AUTH_TOKENS`] and by the
    /// expiry the clock applies.
    pub auth_tokens: BTreeMap<String, microvm_endpoint::MicrovmAuthToken>,
    /// The one address every MicroVM of this store is published on, opened on first
    /// use. `None` until a `RunMicrovm` needs an endpoint.
    pub microvm_proxy: Option<microvm_endpoint::MicrovmProxy>,
    /// The sequence the next minted `microvmId` is formatted from.
    pub next_microvm_seq: u64,
    /// The sequence the next minted `buildId` is formatted from.
    pub next_build_seq: u64,
}

/// One entry of the AWS-managed base-image catalogue, as a reporting operation
/// projects it for one region.
#[derive(Debug, Clone)]
pub struct ManagedBaseImage {
    /// The catalogue id — one of [`MANAGED_BASE_IMAGE_IDS`], and the last segment of
    /// [`Self::image_arn`].
    pub image_id: String,
    /// The ARN this entry carries in the region the request was signed for.
    pub image_arn: String,
    /// When the catalogue entry was published: the publication instant of the oldest
    /// generation the catalogue holds of it ([`managed_base_image_publication`]).
    pub created_at: DateTime,
    /// When the entry was last written: the publication instant of the NEWEST generation
    /// the catalogue holds of it, which is the last time AWS republished the entry. An
    /// image the catalogue publishes a single generation of was written once, when it was
    /// published.
    pub updated_at: Option<DateTime>,
}

/// One published VERSION of an AWS-managed base image, as a reporting operation
/// projects it for one region.
#[derive(Debug, Clone)]
pub struct ManagedBaseImageVersion {
    /// The ARN of the managed image this version belongs to, in the region the request
    /// was signed for.
    pub image_arn: String,
    /// The version itself — one of [`published_managed_base_image_versions`].
    pub image_version: String,
    /// The version's lifecycle status; see [`MANAGED_VERSION_STATUS_AVAILABLE`].
    pub status: String,
    /// When the version was published — the generation's own publication instant
    /// ([`managed_base_image_version_published_at`]).
    pub created_at: DateTime,
    /// When the version was last written. A published generation is never rewritten —
    /// AWS publishes another generation instead — so this is the instant it was
    /// published.
    pub updated_at: Option<DateTime>,
    /// The CPU architectures this published generation supports — see
    /// [`managed_base_image_supported_architectures`].
    pub supported_cpu_configurations: Vec<CpuConfiguration>,
}

impl ManagedBaseImageVersion {
    /// This entry's `supportedCpuConfigurations`, as the recorded-only member a listing
    /// row writes beside its modelled payload, under the member schema `member`.
    ///
    /// The ONE projection of the published architectures, read from the catalogue table
    /// rather than from the request, so what the catalogue publishes and what a listing
    /// reports cannot disagree. See [`crate::ops::types::RecordedMembers`] for why the
    /// member is written beside the payload rather than on it.
    pub fn recorded_supported_cpu_configurations_member(
        &self,
        member: &'static ::aws_smithy_schema::Schema,
    ) -> Vec<RecordedMember> {
        vec![RecordedMember::CpuConfigurations(
            member,
            self.supported_cpu_configurations.clone(),
        )]
    }
}

impl ComAmazonawsLambdamicrovmsLambdamicrovmsState {
    /// Apply every lifecycle transition the clock has already made due.
    ///
    /// This is the only write a reporting operation performs: each transition was
    /// owed at this same instant whether or not anybody called.
    ///
    /// The records live per region ([`regions`]), so each region's are settled on their
    /// own; the authentication tokens are held once for the whole store, because the
    /// address they authenticate at is a MicroVM's own rather than a regional endpoint
    /// of this service.
    pub fn materialize_due(&mut self, now: Instant) {
        for regional in self.regions.values_mut() {
            regional.materialize_due(now);
        }
        self.expire_auth_tokens(now);
    }
}

impl regions::RegionalStore {
    /// Apply every lifecycle transition the clock has already made due to the records of
    /// ONE region — the clock-owed transitions this module's header describes, applied
    /// where the records they move live.
    pub(crate) fn materialize_due(&mut self, now: Instant) {
        for image in self.images.values_mut() {
            let due: Vec<String> = image
                .versions
                .iter()
                .filter(|(_, v)| v.build_due_at.is_some_and(|at| at <= now))
                .map(|(k, _)| k.clone())
                .collect();
            for version in due {
                // The build completing CHANGES the version and the image, so both
                // carry a new `updatedAt` — "the timestamp when the MicroVM image
                // was last updated" is what a client orders change by.
                let updated_at = now_datetime();
                let Some(record) = image.versions.get_mut(&version) else {
                    continue;
                };
                record.build_due_at = None;
                record.updated_at = Some(updated_at);
                // The build is TERMINAL whichever way it ends, so the instant it ends is
                // the instant this version's state settled.
                record.state_settled_at = Some(updated_at);
                // The verdict the build was scheduled with decides which branch of the
                // state machine the version, its builds and the image settle on: "the
                // image transitions from CREATING to CREATED on success, or CREATE_FAILED
                // on failure".
                let failure = record.build_failure_reason.clone();
                match failure {
                    // The FAILURE branch. The version is the settlement of its builds, so
                    // each per-target build carries the verdict and its reason, and none
                    // of them produced a snapshot to report.
                    Some(reason) => {
                        record.state = VERSION_STATE_FAILED.to_string();
                        record.status = VERSION_STATUS_INACTIVE.to_string();
                        for build in record.builds.values_mut() {
                            build.state = BUILD_STATE_FAILED.to_string();
                            build.state_reason = Some(reason.clone());
                            build.snapshot_build = None;
                        }
                        image.updated_at = Some(updated_at);
                        // The failed version is the image's latest FAILED one; the
                        // version it was serving, if any, stays its active one, because
                        // a build that produced nothing cannot replace it.
                        image.latest_failed_image_version = Some(version.clone());
                        if image.state == IMAGE_STATE_CREATING {
                            image.state = IMAGE_STATE_CREATE_FAILED.to_string();
                        } else if image.state == IMAGE_STATE_UPDATING {
                            image.state = IMAGE_STATE_UPDATE_FAILED.to_string();
                        }
                    }
                    // The SUCCESS branch.
                    None => {
                        record.state = VERSION_STATE_SUCCESSFUL.to_string();
                        record.status = VERSION_STATUS_ACTIVE.to_string();
                        // The version's build is the fan-out of its per-target builds, so
                        // the builds settle with it: each produced the snapshot the
                        // version's own resource requirement sizes.
                        let snapshot = snapshot_build_of(&record.resources);
                        for build in record.builds.values_mut() {
                            build.state = BUILD_STATE_SUCCESSFUL.to_string();
                            build.snapshot_build = snapshot.clone();
                        }
                        image.updated_at = Some(updated_at);
                        // A newly successful build becomes the image's active version,
                        // and the previous one steps down: `MicrovmImageVersionStatus`
                        // marks exactly one version ACTIVE.
                        if let Some(previous) =
                            image.latest_active_image_version.replace(version.clone())
                        {
                            if previous != version {
                                if let Some(old) = image.versions.get_mut(&previous) {
                                    old.status = VERSION_STATUS_INACTIVE.to_string();
                                }
                            }
                        }
                        // The build settling ends the lifecycle that started it: each
                        // progressive `MicrovmImageState` reaches its own settled value,
                        // and a record already in a settled one is left as it is.
                        if image.state == IMAGE_STATE_CREATING {
                            image.state = IMAGE_STATE_CREATED.to_string();
                        } else if image.state == IMAGE_STATE_UPDATING {
                            image.state = IMAGE_STATE_UPDATED.to_string();
                        }
                    }
                }
            }
        }

        // `DeleteMicrovmImage` is asynchronous: an image whose removal the clock has
        // made due goes, with every version it holds, and both indexes go with it.
        let removed: Vec<String> = self
            .images
            .values()
            .filter(|image| image.delete_due_at.is_some_and(|at| at <= now))
            .map(|image| image.name.clone())
            .collect();
        for name in removed {
            if let Some(image) = self.images.remove(&name) {
                self.arn_index.remove(&image.image_arn);
            }
        }

        for vm in self.microvms.values_mut() {
            if vm.provision_due_at.is_some_and(|at| at <= now) {
                vm.provision_due_at = None;
                if vm.state == MICROVM_STATE_PENDING {
                    vm.state = MICROVM_STATE_RUNNING.to_string();
                }
            }
            // "The MicroVM transitions through SUSPENDING to SUSPENDED."
            if vm.suspend_due_at.is_some_and(|at| at <= now) {
                vm.suspend_due_at = None;
                if vm.state == MICROVM_STATE_SUSPENDING {
                    vm.state = MICROVM_STATE_SUSPENDED.to_string();
                }
            }
            if let Some(due) = vm.terminate_due_at.filter(|at| *at <= now) {
                vm.terminate_due_at = None;
                if vm.state == MICROVM_STATE_TERMINATING {
                    vm.state = MICROVM_STATE_TERMINATED.to_string();
                    // `terminatedAt` is "the timestamp when the MicroVM was
                    // terminated", so it is stamped when the MicroVM REACHES
                    // TERMINATED, not when it started terminating.
                    vm.terminated_at = Some(now_datetime());
                    // The retention runs from the TERMINATION instant, not from the
                    // request that happened to observe it.
                    vm.reclaim_due_at = Some(due + TERMINATED_MICROVM_RETENTION);
                }
            }
            // "The maximum duration in seconds that the MicroVM can exist before
            // being terminated by the platform."
            if let Some(due) = vm.expiry_due_at.filter(|at| *at <= now) {
                vm.expiry_due_at = None;
                if vm.state != MICROVM_STATE_TERMINATING && vm.state != MICROVM_STATE_TERMINATED {
                    vm.state = MICROVM_STATE_TERMINATED.to_string();
                    vm.provision_due_at = None;
                    vm.suspend_due_at = None;
                    vm.terminated_at = Some(now_datetime());
                    vm.state_reason = Some(EXPIRY_REASON.to_string());
                    vm.reclaim_due_at = Some(due + TERMINATED_MICROVM_RETENTION);
                }
            }
        }
        let mut reclaimed: Vec<String> = Vec::new();
        self.microvms.retain(|_, vm| {
            let keep = vm.reclaim_due_at.is_none_or(|at| at > now);
            if !keep {
                reclaimed.push(vm.microvm_arn.clone());
            }
            keep
        });
        for arn in reclaimed {
            self.microvm_arn_index.remove(&arn);
        }
        self.idempotency.retain(|_, record| record.expires_at > now);
    }
}

impl ComAmazonawsLambdamicrovmsLambdamicrovmsState {
    /// The AWS-managed base images this service publishes, in catalogue-id order and
    /// projected into `region`.
    ///
    /// This is the ONE catalogue of the service, the same set
    /// `CreateMicrovmImage$baseImageArn` is resolved against
    /// ([`MANAGED_BASE_IMAGE_IDS`], [`is_published_managed_base_image`]), so a listing
    /// of it and a create that builds on it cannot disagree about which base images
    /// exist. The entries are AWS-owned, so each ARN carries `aws` in its account field
    /// and the region the request named; the instants an entry reports are the
    /// catalogue's own published metadata ([`managed_base_image_publication`]) rather
    /// than any clock this store reads, so every read of the catalogue reports the same
    /// pair and a client can cache and diff it.
    pub fn managed_base_images(&self, region: &str) -> Vec<ManagedBaseImage> {
        let mut ids: Vec<&str> = MANAGED_BASE_IMAGE_IDS.to_vec();
        ids.sort_unstable();
        ids.into_iter()
            .filter_map(|image_id| {
                let (created_at, updated_at) = managed_base_image_publication(image_id)?;
                Some(ManagedBaseImage {
                    image_id: image_id.to_string(),
                    image_arn: managed_base_image_arn(region, image_id),
                    created_at,
                    updated_at: Some(updated_at),
                })
            })
            .collect()
    }

    /// The versions the AWS-managed catalogue publishes for the base image `image_id`,
    /// in the order a listing pages them (newest generation first) and projected into
    /// `region`.
    ///
    /// The same catalogue [`Self::managed_base_images`] serves, read one image deep
    /// ([`MANAGED_BASE_IMAGE_VERSIONS`]), so a listing of an image's versions, the
    /// version a build that pins none resolves to, and the images the catalogue
    /// publishes cannot disagree. Each version reports the instant the catalogue
    /// published that generation ([`managed_base_image_version_published_at`]), a
    /// constant of the catalogue rather than a clock this store reads.
    pub fn managed_base_image_versions(
        &self,
        image_id: &str,
        region: &str,
    ) -> Vec<ManagedBaseImageVersion> {
        published_managed_base_image_versions(image_id)
            .into_iter()
            .filter_map(|image_version| {
                let published_at =
                    managed_base_image_version_published_at(image_id, image_version)?;
                Some(ManagedBaseImageVersion {
                    image_arn: managed_base_image_arn(region, image_id),
                    image_version: image_version.to_string(),
                    status: MANAGED_VERSION_STATUS_AVAILABLE.to_string(),
                    created_at: published_at,
                    updated_at: Some(published_at),
                    supported_cpu_configurations: managed_base_image_supported_architectures(
                        image_id,
                        image_version,
                    ),
                })
            })
            .collect()
    }

    /// Mint the next `microvmId`. Zero-padded so the ids sort in creation
    /// order, which is the order `ListMicrovms` must page through.
    pub fn next_microvm_id(&mut self) -> String {
        let seq = self.next_microvm_seq;
        self.next_microvm_seq += 1;
        format!("mvm-{seq:016x}")
    }

    /// The address of the OWN network interface the next MicroVM is started with: one
    /// link-local address per MicroVM of the store
    /// ([`microvm_network::link_local_address`]), assigned from the same sequence its id
    /// is, so no two MicroVMs of one store hold the same one.
    pub fn next_link_local_address(&self) -> String {
        microvm_network::link_local_address(self.next_microvm_seq)
    }

    /// Mint the next `buildId`. Zero-padded for the same reason as
    /// [`Self::next_microvm_id`]: the ids sort in creation order, which is the order
    /// `ListMicrovmImageBuilds` pages through.
    pub fn next_build_id(&mut self) -> String {
        let seq = self.next_build_seq;
        self.next_build_seq += 1;
        format!("build-{seq:016x}")
    }

    /// The builds a version's asynchronous build fans out into: one per target of
    /// [`BUILD_TARGETS`] the version's `cpuConfigurations` ask for, each `IN_PROGRESS`
    /// until the clock settles the version's build ([`Self::materialize_due`]).
    ///
    /// The fan-out is driven by the TARGETS, so a request that names one architecture
    /// twice still gets one build per target rather than a duplicate of each.
    pub fn mint_builds(
        &mut self,
        cpu_configurations: &[CpuConfiguration],
        created_at: DateTime,
    ) -> BTreeMap<String, MicrovmImageBuild> {
        let mut builds = BTreeMap::new();
        let requested: Vec<BuildTarget> = BUILD_TARGETS
            .iter()
            .filter(|target| {
                cpu_configurations
                    .iter()
                    .any(|configuration| configuration.architecture == target.architecture)
            })
            .copied()
            .collect();
        for target in requested {
            let build_id = self.next_build_id();
            builds.insert(
                build_id.clone(),
                MicrovmImageBuild {
                    build_id,
                    state: BUILD_STATE_IN_PROGRESS.to_string(),
                    architecture: target.architecture.to_string(),
                    chipset: target.chipset.to_string(),
                    chipset_generation: target.chipset_generation.to_string(),
                    state_reason: None,
                    created_at,
                    snapshot_build: None,
                },
            );
        }
        builds
    }
}

/// The key a `clientToken` is recorded under. An `@idempotencyToken` is scoped to the
/// operation that minted with it, so the same token on two operations is two keys.
pub fn idempotency_key(operation: &str, client_token: &str) -> String {
    format!("{operation}\u{1f}{client_token}")
}

/// `now`, as the wire timestamp type every output member uses.
pub fn now_datetime() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// The region this request was signed for, for the ARNs it mints.
pub fn request_region(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of the MicroVM image named `name` in `region`.
///
/// `MicrovmImageArn` has pattern
/// `^arn:[a-z0-9-]+:lambda:[a-z0-9-]*:[a-z0-9]*:microvm-image:.+$`, and the
/// service's `arnNamespace` is `lambda`.
pub fn microvm_image_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("microvm-image:{name}"))
}

/// The ARN of an AWS-MANAGED network connector named `name`, in `region`.
///
/// The connector is an AWS-owned resource, so its ARN carries `aws` in the account
/// field rather than this account's id — which is why it is not built through
/// `engine::emulator_arn` (that mints an ARN owned by the emulator's account).
/// The form is recorded AWS behaviour:
/// `arn:aws:lambda:<region>:aws:network-connector:aws-network-connector:<NAME>`.
pub fn managed_network_connector_arn(region: &str, name: &str) -> String {
    format!("arn:aws:lambda:{region}:aws:network-connector:aws-network-connector:{name}")
}

/// The ARN of the MicroVM `microvm_id` in `region`.
///
/// `MicrovmIdentifier` is documented as "The ARN or ID of the MicroVm", so each
/// MicroVM carries an ARN an identifier can be resolved against; the service's
/// `arnNamespace` is `lambda`.
pub fn microvm_arn(region: &str, microvm_id: &str) -> String {
    ::engine::emulator_arn("lambda", region, &format!("microvm:{microvm_id}"))
}

/// A fresh authentication token for connecting a client to one MicroVM's endpoint,
/// for `CreateMicrovmAuthToken`.
///
/// The token authenticates to the MICROVM endpoint, not to this emulator, so it must
/// never be accepted as a SigV4 credential — but it is still a secret handed to a
/// caller, and nothing in a service may construct a secret itself. It is therefore
/// minted by the one component that generates secrets, the emulator's credential
/// store, and then removed from that store again: the caller gets the secret, the
/// emulator keeps no credential it would accept, and nothing accumulates per call.
/// What the MicroVM endpoint keeps to recognise the token again is its DIGEST
/// ([`microvm_endpoint::auth_token_digest`]), never the token.
///
/// `None` when the store minted no token secret, which leaves the operation to refuse
/// rather than report a token it made up.
pub fn mint_microvm_auth_token(store: &::emulator_server::CredentialStore) -> Option<String> {
    let minted = store.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let token = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The token is not a credential of this emulator; see above.
    let _ = store.delete(&minted.id);
    token
}

/// A sorted copy of a wire map, so a stored record has one deterministic order.
pub fn sorted_map(map: Option<HashMap<String, String>>) -> BTreeMap<String, String> {
    map.unwrap_or_default().into_iter().collect()
}

/// A stored map as the wire shape, omitted when empty rather than serialized as
/// an empty object.
pub fn wire_map(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    (!map.is_empty()).then(|| {
        map.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>()
    })
}

/// A stored map as the wire shape, PRESENT even when empty.
///
/// `GetMicrovmImage` projects the image's whole stored record, and the `tags` of an
/// image carrying none is an empty map rather than an absent member (recorded AWS
/// behaviour): a client reading `tags` gets a map to iterate either way.
pub fn wire_map_present(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    Some(
        map.iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<HashMap<_, _>>(),
    )
}

/// A stored list as the wire shape, omitted when empty.
pub fn wire_list<T: Clone>(list: &[T]) -> Option<Vec<T>> {
    (!list.is_empty()).then(|| list.to_vec())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLambdamicrovmsLambdamicrovmsState>> {
    shared_engine(engine)
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLambdamicrovmsLambdamicrovmsState>)
}

/// The ONE shared engine behind a handler's `Eng`, which is both what holds this
/// service's store ([`service_state`]) and what vends the listener a MicroVM endpoint
/// is served from ([`microvm_endpoint::ensure_base_url`]) — so the crate downcasts in
/// exactly one place.
///
/// `None` when the engine is not the shared in-memory one, which leaves the caller to
/// answer with a modelled server error rather than with a fresh engine.
pub(crate) fn shared_engine<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<&::engine::InMemoryEngine> {
    (engine as &dyn ::std::any::Any).downcast_ref::<::engine::InMemoryEngine>()
}
