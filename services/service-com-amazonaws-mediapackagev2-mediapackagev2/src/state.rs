// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-mediapackagev2-mediapackagev2 service.
//!
//! MediaPackage v2 is a three-level containment hierarchy — channel group -> channel
//! -> origin endpoint — so the store nests the same way: a channel cannot exist
//! without its group, and deleting a parent that still has children is refused wherever
//! the model declares `ConflictException` for that delete — the group's and the channel's.
//! The origin endpoint's delete declares none, so it is unconditional: it takes the
//! records persisted ON the endpoint (its policy, its harvest jobs) with it. Every
//! derived value a response carries (ARN, egress domain, ingest URL, manifest URL,
//! ETag, timestamps) is computed by the call that WRITES the record - a create, or an
//! update - and persisted, so every later read reproduces it byte for byte and no
//! response computes a value of its own while being serialized. A resource's `ETag` is a
//! function of its content and of the commit count the store holds it at (see
//! [`ChannelGroup::version`]), so a write that changed the resource rotates it.
//!
//! MediaPackage v2 is a REGIONAL control plane, so the Region a request is served in is
//! part of the identity of every resource it touches: a channel group is stored under
//! its Region AND its name ([`ChannelGroupKey`]), the same name in two Regions is two
//! independent groups with their own channels, endpoints and harvest jobs, and a request
//! served in one Region resolves, lists, mutates and deletes only what was created in
//! it. Every accessor on the store therefore takes the request's Region
//! ([`request_region`]) beside the name.
//!
//! The maps are [`BTreeMap`]s: `ListOriginEndpoints` pages in name order, and an
//! ordered map makes that order a property of the store rather than of each read.
//!
//! The same holds for the members the SERVICE applies rather than the caller: a channel's
//! input type and output locking mode, its switching and CMSD-header blocks, an origin
//! endpoint's segment settings, startover window and URI/stream-name modes, and every
//! manifest configuration's window, URI path type, child-manifest URL encoding and (on
//! the HLS families) child manifest name are materialised ONCE, at create time, from the
//! submitted value or the service's default, and stored — so the create response, a later
//! read and the list projection all report the same concrete value. That reaches INSIDE a
//! configured block too: the members the model documents the service as supplying within
//! an endpoint's `Segment.Encryption` and its SCTE blocks are materialised with the rest
//! of the segment ([`materialized_segment`]).

use std::collections::{BTreeMap, BTreeSet, HashMap};

use aws_smithy_schema_server::{Violation, ViolationKind};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    CdnAuthConfiguration, ConflictException, CreateDashManifestConfiguration,
    CreateHlsManifestConfiguration, CreateLowLatencyHlsManifestConfiguration,
    CreateMssManifestConfiguration, Destination, Encryption, EncryptionMethod,
    ForceEndpointErrorConfiguration,
    GetDashManifestConfiguration, GetHlsManifestConfiguration,
    GetLowLatencyHlsManifestConfiguration, GetMssManifestConfiguration, HarvestedManifests,
    HarvesterScheduleConfiguration, IngestEndpoint, InputSwitchConfiguration,
    MultiviewConfiguration, OutputHeaderConfiguration, ResourceNotFoundException, Scte, ScteDash,
    ScteHls, Segment, ServiceQuotaExceededException, ValidationException,
};

/// The CDN-authorization, segment-encryption and key-provider-URL rules of an origin
/// endpoint, grouped in their own file. Re-exported here so every caller keeps reaching
/// them at `crate::state::…`.
mod endpoint_security;
pub use endpoint_security::*;

/// Pre-flighting a harvest job's destination against the object store that would have to
/// take its output. Reached as `crate::state::harvest_destination::…`, like the rest of
/// this module's parts.
pub mod harvest_destination;

/// Carrying a due harvest OUT: the objects it writes to that destination and the terminal
/// status the outcome of that write decides. Reached as
/// `crate::state::harvest_export::…`.
pub mod harvest_export;

/// The signing name (and ARN service namespace) of this service, from the model's
/// `awsService.sigv4Name`.
pub const SIGNING_NAME: &str = "mediapackagev2";

/// The region used when a request carries no SigV4 credential scope to read one from.
///
/// The emulator's own shared config writes `region = us-east-1`, so an unsigned request
/// lands in the same region a signed default-profile request does.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The `InputType` a channel is created with when the request omits it: the model's
/// `InputType` doc says "If unprovided, the value defaults to HLS".
pub const DEFAULT_INPUT_TYPE: &str = "HLS";

/// The `OutputLockingMode` a channel is created with when the request omits it and the
/// channel's input type may carry the member at all: the model's `OutputLockingMode` doc
/// says "If you don't specify a value, the default is EPOCH_LOCKED".
pub const DEFAULT_OUTPUT_LOCKING_MODE: &str = "EPOCH_LOCKED";

/// The `OutputLockingMode` a channel has to carry for the origin endpoints under it to
/// carry a segment output-timestamp mode at all: the model's `Segment$OutputTimestampMode`
/// doc says the setting "is only configurable on channels with OutputLockingMode set to
/// NON_EPOCH_LOCKED". One of the two values the `OutputLockingMode` enum publishes.
pub const OUTPUT_TIMESTAMP_MODE_LOCKING_MODE: &str = "NON_EPOCH_LOCKED";

/// The `OutputTimestampMode` an origin endpoint whose channel may carry one is created
/// with when the request omits it: the same member's doc says "If you don't specify a
/// value, the default is PASSTHROUGH".
pub const DEFAULT_OUTPUT_TIMESTAMP_MODE: &str = "PASSTHROUGH";

/// How many ingest endpoints a channel that receives ingest of its own is given.
/// MediaPackage v2 hands such a channel a redundant pair, which is why `IngestEndpoints`
/// is a list rather than a single URL.
const INGEST_ENDPOINTS_PER_CHANNEL: u8 = 2;

/// The most channel groups this emulator holds at once.
///
/// `channel_groups` grows by one record per `CreateChannelGroup` request and shrinks only
/// on an explicit delete, so without a ceiling its entry COUNT is set by how many
/// requests a caller makes. AWS bounds the same thing with a per-account, per-Region
/// service quota and refuses a create past it, so this ceiling is applied per Region too
/// ([`ComAmazonawsMediapackagev2Mediapackagev2State::channel_group_count`]); that quota's value is adjustable and is
/// not in the model, so this number is the EMULATOR's own bound rather than a claim about
/// AWS's, while the refusal it produces is the error the model declares for exactly this
/// case: `ServiceQuotaExceededException` (HTTP 402, "The request would cause a service
/// quota to be exceeded.").
pub const MAX_CHANNEL_GROUPS: usize = 100;

/// The most channel groups this emulator holds at once across EVERY Region together.
///
/// [`MAX_CHANNEL_GROUPS`] bounds one Region's share, and the Region a create is served in
/// is read from that request's own credential scope — so a per-Region ceiling alone would
/// leave the store growing by one record per request again, for a caller that names a
/// fresh Region each time. This is the bound on the whole map, so what the emulator holds
/// is set by this number rather than by how many requests arrive, and it is the same kind
/// of store-wide ceiling [`MAX_INGESTED_BYTES`] is for ingested content. It is a small
/// multiple of the per-Region bound, so a handful of Regions each still reach their own
/// ceiling, and a create past it is refused with the SAME error the per-Region ceiling
/// produces (`ServiceQuotaExceededException`): both are the service's quota refusal.
pub const MAX_CHANNEL_GROUPS_TOTAL: usize = 500;

/// The most channels one channel group holds at once — the same bound, one level down:
/// `channels` grows per `CreateChannel` and shrinks only on an explicit delete.
pub const MAX_CHANNELS_PER_GROUP: usize = 100;

/// The most origin endpoints one channel holds at once — the same bound, one level
/// further down: `origin_endpoints` grows per `CreateOriginEndpoint` and shrinks only on
/// an explicit delete.
pub const MAX_ORIGIN_ENDPOINTS_PER_CHANNEL: usize = 100;

/// The startover window MediaPackage applies to an origin endpoint created without one.
///
/// The model declares no `@default` and documents none, so this is RECORDED behaviour:
/// the service reports a fifteen-minute startover window on an endpoint whose create
/// request named none, and reports the window the request named when it named one. The
/// value is inside the member's modelled `@range 0..1209600`.
pub const DEFAULT_STARTOVER_WINDOW_SECONDS: i32 = 900;

/// The segment duration MediaPackage applies when a create request sends no
/// `SegmentDurationSeconds` — six seconds, within the member's modelled `@range 1..30`
/// and the value the recorded service returns for an endpoint created without a segment
/// configuration.
pub const DEFAULT_SEGMENT_DURATION_SECONDS: i32 = 6;

/// The segment base name MediaPackage applies when a create request sends no
/// `SegmentName` — the value the recorded service returns, and one the member's
/// `^[a-zA-Z0-9_-]+$` pattern and `@length 1..256` admit.
pub const DEFAULT_SEGMENT_NAME: &str = "segment";

/// The manifest window MediaPackage applies when a manifest configuration sends no
/// `ManifestWindowSeconds` — sixty seconds, the value the recorded service returns, and
/// at or above the smallest window the create shapes accept (`@range` min 30).
pub const DEFAULT_MANIFEST_WINDOW_SECONDS: i32 = 60;

/// The URI path type MediaPackage applies when a manifest configuration sends no
/// `UriPathType`. The model states the default on every create manifest shape that
/// carries the member: "If you don't specify a value, the default is `LEAF`."
pub const DEFAULT_URI_PATH_TYPE: &str = "LEAF";

/// The bandwidth reporting mode MediaPackage applies to a DASH manifest configuration
/// that sends no `BandwidthReportingMode`. The model states the default inside the
/// member's own value list, on both the create and the reported shape: "`AVERAGE` –
/// MediaPackage reports the average bitrate of the source video track. This is the
/// default", from the modelled `DashBandwidthReportingMode` enum (`AVERAGE` | `MAX`).
pub const DEFAULT_DASH_BANDWIDTH_REPORTING_MODE: &str = "AVERAGE";

/// The URI separator MediaPackage applies to an origin endpoint created without one —
/// the value the recorded service reports for such an endpoint, from the modelled
/// `UriSeparator` enum (`UNDERSCORE` | `HYPHEN`).
pub const DEFAULT_URI_SEPARATOR: &str = "UNDERSCORE";

/// The stream-name output mode MediaPackage applies to an origin endpoint created
/// without one — the value the recorded service reports, from the modelled
/// `StreamNameOutputMode` enum (`INDEX` | `PASSTHROUGH_NAME`).
pub const DEFAULT_STREAM_NAME_OUTPUT_MODE: &str = "INDEX";

/// Whether MediaPackage bundles the audio tracks of an endpoint created without a
/// segment configuration into a rendition group — recorded behaviour, like
/// [`DEFAULT_TS_INCLUDE_DVB_SUBTITLES`] and [`DEFAULT_INCLUDE_IFRAME_ONLY_STREAMS`]: the
/// model declares no `@default` for any of the three.
pub const DEFAULT_TS_USE_AUDIO_RENDITION_GROUP: bool = true;

/// Whether MediaPackage passes DVB subtitles through for an endpoint created without a
/// segment configuration — recorded behaviour.
pub const DEFAULT_TS_INCLUDE_DVB_SUBTITLES: bool = true;

/// Whether the stream set of an endpoint created without a segment configuration carries
/// the extra I-frame-only stream — recorded behaviour: the service reports it off.
pub const DEFAULT_INCLUDE_IFRAME_ONLY_STREAMS: bool = false;

/// Whether MediaPackage URL-encodes the child-manifest query string of a manifest
/// configuration that does not ask for it — recorded behaviour: the service reports the
/// encoding off.
pub const DEFAULT_URL_ENCODE_CHILD_MANIFEST: bool = false;

/// Whether SCTE-35 messages are embedded in segment files when a configured SCTE block
/// names no choice. The model states the default inside the member's own value list:
/// "None – SCTE-35 messages are not included in segments (default)", from the modelled
/// `ScteInSegments` enum (`NONE` | `ALL` | `MATCHES_FILTER`).
pub const DEFAULT_SCTE_IN_SEGMENTS: &str = "NONE";

/// Which SCTE-35 events appear in a manifest whose configured SCTE block names no choice.
/// The model states the same default on both manifest-side blocks — `ScteHls` and
/// `ScteDash` — "If you don't specify a value, the default is `ALL`", from the modelled
/// `ScteInManifests` enum (`ALL` | `MATCHES_FILTER`).
pub const DEFAULT_SCTE_IN_MANIFESTS: &str = "ALL";

/// The child manifest name MediaPackage applies to an HLS or low-latency HLS manifest
/// configuration submitted without one.
///
/// The model states the rule on both sides of the wire: the create shapes say "If you
/// don't enter a value, MediaPackage uses the default manifest name, index, with an added
/// suffix to distinguish it from the manifest name", and the `Get*` shapes name the
/// applied value outright — "MediaPackage uses the default child manifest name, index_1".
/// The suffixed form is why the reported member's `ResourceName` pattern admits `_` while
/// the submitted member's `ManifestName` pattern does not.
pub const DEFAULT_CHILD_MANIFEST_NAME: &str = "index_1";

/// The most manifest configurations one origin endpoint stores, counted across the HLS,
/// low-latency HLS, DASH and MSS families together.
///
/// Each submitted manifest is persisted with its computed URL, so a single create's
/// record size is caller-chosen unless the count is bounded; the model puts no `@length`
/// on the manifest lists, so the NUMBER here is the emulator's own choice. The REFUSAL
/// is not: the model's `ValidationExceptionType` declares `NUM_MANIFESTS_HIGH` beside
/// `NUM_MANIFESTS_LOW`, i.e. a manifest count outside the service's bounds is an input
/// failure (`ValidationException`), not a quota error.
pub const MAX_MANIFESTS_PER_ORIGIN_ENDPOINT: usize = 100;

/// The fewest manifest configurations an origin endpoint may carry, counted across the
/// four families together.
///
/// Like the ceiling above, this bound is reported as a `ValidationException` carrying the
/// discriminator for the side of the bound that was crossed (`NUM_MANIFESTS_LOW` here),
/// and an endpoint with no manifest can serve nothing at all.
pub const MIN_MANIFESTS_PER_ORIGIN_ENDPOINT: usize = 1;

/// The most harvest jobs one origin endpoint holds at once.
///
/// `harvest_jobs` grows by one record per `CreateHarvestJob` and never shrinks (a
/// finished job stays readable), so without a ceiling its entry COUNT is set by how many
/// requests a caller makes. AWS bounds the same thing with an adjustable service quota
/// that is not in the model, so this number is the EMULATOR's own bound, refused with the
/// `ServiceQuotaExceededException` the model declares for exactly this case.
pub const MAX_HARVEST_JOBS_PER_ORIGIN_ENDPOINT: usize = 100;

/// The most ingested objects one channel's data plane retains, and the most bytes one of
/// them may carry.
///
/// An ingest `PUT` is caller-driven, so both the object COUNT and each object's SIZE are
/// bounded here rather than by how much an encoder pushes. Past either bound the write is
/// refused rather than recorded.
pub const MAX_INGESTED_OBJECTS_PER_CHANNEL: usize = 64;
/// See [`MAX_INGESTED_OBJECTS_PER_CHANNEL`].
pub const MAX_INGESTED_OBJECT_BYTES: usize = 8 * 1024 * 1024;

/// The most bytes the ingest data plane retains across EVERY channel together.
///
/// The two bounds above are per channel, and the channel count is itself caller-driven
/// ([`MAX_CHANNEL_GROUPS`] x [`MAX_CHANNELS_PER_GROUP`]), so their product is not a bound
/// on the store: this is. It is sized for a developer machine rather than derived from
/// the per-channel ceilings, and a `PUT` that would push the retained total past it is
/// refused rather than recorded — nothing is evicted, because an encoder's content is not
/// this emulator's to drop silently.
pub const MAX_INGESTED_BYTES: usize = 64 * 1024 * 1024;

/// What [`ComAmazonawsMediapackagev2Mediapackagev2State::record_ingested_object`] did, so
/// the data plane answers each outcome without re-deriving it — and cannot reach the
/// stored case without having gone through both bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IngestOutcome {
    /// The object was recorded against the channel.
    Recorded,
    /// The group or the channel the path names does not resolve, or the channel it names
    /// publishes no ingest endpoint the path could belong to; nothing was written.
    NoSuchChannel,
    /// The channel already holds [`MAX_INGESTED_OBJECTS_PER_CHANNEL`] distinct objects.
    ObjectCountReached,
    /// Recording this object would push the whole store past [`MAX_INGESTED_BYTES`].
    StoreFull,
}

// ---------------------------------------------------------------------------
// Rejection bodies this service's shared refusal layer does not already own.
//
// The not-found, duplicate-create, children-attached and policy-not-found sentences all
// come from `ResourceKind::not_found`, `children_attached`, `resource_already_exists` and
// `policy_not_found` further down this file, so no handler words one of its own. What is
// declared here is only what those do not cover: the conditional-update conflict, the
// `HarvestJobStatus` values and the `ValidationExceptionType` values the harvest-job and
// pagination rules answer with.
// ---------------------------------------------------------------------------

/// The one sentence for a conditional update whose `x-amzn-update-if-match` entity tag is
/// not the committed one — the loser of a concurrent update.
///
/// This is the text the service itself answers with: fixed per rejection class, naming no
/// resource and carrying nothing from the request, so the sentence is part of the contract
/// rather than a per-run value (the recorded behavioral corpus asserts it for the channel
/// path, and every operation of this class answers with the same one). Note the
/// typographic apostrophes.
pub const CONFLICTING_OPERATION_MESSAGE: &str = "You\u{2019}ve specified an entity tag with \
    your request that does not match the resource\u{2019}s current entity tag. This can \
    occur if the resource has been updated by another request between the time you \
    retrieved the ETag and when you made your update request";

/// `HarvestJobStatus` values, from the model's `HarvestJobStatus` enum
/// (`HARVEST_JOB_STATUS_VALUES` in `ops::types`).
pub const HARVEST_JOB_QUEUED: &str = "QUEUED";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_CANCELLED: &str = "CANCELLED";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_FAILED: &str = "FAILED";
/// See [`HARVEST_JOB_QUEUED`].
pub const HARVEST_JOB_COMPLETED: &str = "COMPLETED";

/// `ValidationExceptionType` values this crate answers with, from the model's
/// `ValidationExceptionType` enum (`VALIDATION_EXCEPTION_TYPE_VALUES` in `ops::types`).
pub const VALIDATION_END_TIME_EARLIER_THAN_START_TIME: &str = "END_TIME_EARLIER_THAN_START_TIME";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION: &str =
    "HARVEST_JOB_INELIGIBLE_FOR_CANCELLATION";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_INVALID_HARVEST_JOB_DURATION: &str = "INVALID_HARVEST_JOB_DURATION";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT: &str =
    "HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB: &str =
    "MULTIVIEW_INPUT_TYPE_WITH_HARVEST_JOB";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_INVALID_PAGINATION_TOKEN: &str = "INVALID_PAGINATION_TOKEN";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE: &str =
    "ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED: &str =
    "MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED";
/// See [`VALIDATION_END_TIME_EARLIER_THAN_START_TIME`].
pub const VALIDATION_MULTIVIEW_SOURCE_NON_EPOCH_LOCKED: &str =
    "MULTIVIEW_SOURCE_NON_EPOCH_LOCKED";

/// The 409 for a conditional update that lost the race: the entity tag the caller
/// presented is no longer the committed one.
pub fn conflicting_operation() -> ConflictException {
    ConflictException {
        message: Some(CONFLICTING_OPERATION_MESSAGE.to_string()),
        conflict_exception_type: Some("CONFLICTING_OPERATION".to_string()),
    }
}

/// The whole conditional-write precondition, for every operation that binds
/// `x-amzn-update-if-match`: an absent entity tag makes the write unconditional, a
/// presented tag that IS the resource's committed one lets it proceed, and a presented tag
/// that is not means somebody else committed first — the refusal
/// [`conflicting_operation`] words, with nothing written.
///
/// `presented` is the request's optional `ETag` member and `committed` the tag the store
/// holds the addressed resource at. Returns the refusal to answer with, or `None` when the
/// write may proceed, which is the shape this crate's other cross-handler rules
/// ([`channel_input_type_violation`], [`speke_role_arn_rejection`]) hand back — so the
/// rule that SELECTS the refusal lives beside the sentence it selects and every
/// conditional writer resolves through this one copy of it.
pub fn stale_entity_tag_rejection(
    presented: Option<&str>,
    committed: &str,
) -> Option<ConflictException> {
    presented
        .is_some_and(|tag| tag != committed)
        .then(conflicting_operation)
}

// ---------------------------------------------------------------------------
// The tagging front end's own rejections.
//
// `TagResource` / `UntagResource` / `ListTagsForResource` model only
// `ValidationException`, but MediaPackage answers them from a tagging front end that
// refuses a malformed `ResourceArn` as `BadRequestException` and an absent resource as
// `NotFoundException` — the codes the recorded behaviour shows, and the codes a client
// branches on. Both are 4xx client errors: a caller that passed a bad ARN must be told
// to fix it, never handed a retryable server fault.
// ---------------------------------------------------------------------------

/// The wire error code for a `ResourceArn` that is not a syntactically valid ARN.
pub const INVALID_ARN_CODE: &str = "BadRequestException";

/// The sentence that accompanies [`INVALID_ARN_CODE`] — a property of the ARN's SYNTAX,
/// so it names nothing from the request.
pub const INVALID_ARN_MESSAGE: &str = "Invalid input resource arn: bad syntax";

/// The wire error code for a well-formed `ResourceArn` naming a resource that is absent.
pub const RESOURCE_NOT_FOUND_CODE: &str = "NotFoundException";

/// The sentence that accompanies [`RESOURCE_NOT_FOUND_CODE`] — the whole fixed text of
/// this failure, which says that the resource was not found and nothing else: the tagging
/// front end does not echo the ARN it could not resolve, so two callers naming two
/// different absent resources read the same sentence.
pub const TAGGED_RESOURCE_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The most tags one resource carries, and so the most entries a resource's tag map can
/// ever hold.
///
/// A tag map grows by one entry per distinct caller-chosen KEY and shrinks only on an
/// explicit `UntagResource`, so without a ceiling its size is set by how many requests a
/// caller makes. AWS bounds it with the tag quota it publishes for every taggable
/// resource — 50 tags per resource — and refuses a request past it, which is what this
/// constant is. The model puts no `@length` on `TagMap`, so the boundary validator does
/// not enforce it and the handlers do.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most characters a tag KEY may carry — AWS's published tag-key limit of 128
/// characters, which the model's `TagMap` does not express either.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The most characters a tag VALUE may carry — AWS's published tag-value limit of 256
/// characters.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// Refuse `tags` when it is past the tag bounds above, reporting the modelled rejection.
///
/// `tags` is the tag map the resource would be left HOLDING, not just the one the request
/// carried, so a `TagResource` that merges into an existing map is judged on the merged
/// result. Called before anything is written, so a refused request leaves the resource's
/// tags exactly as they were.
///
/// `member_path` is the wire name of the member the tags arrived in, which differs per
/// operation exactly as it does for [`reserved_tag_key_rejection`], so the sentence
/// points the caller at the member it sent.
///
/// `ValidationExceptionType` enumerates no tag-quota member
/// (`VALIDATION_EXCEPTION_TYPE_VALUES` in `ops::types`), so the discriminator is left
/// unset rather than claiming a reason the service does not name for this case; the
/// message states which bound was exceeded and the bound's value, in the one envelope
/// [`validation_message`] gives every refusal this service makes.
pub fn tag_bounds_rejection(
    member_path: &str,
    tags: &HashMap<String, String>,
) -> Option<ValidationException> {
    tag_bounds_rejection_counted(member_path, tags, |count| {
        format!(
            "A resource carries at most {MAX_TAGS_PER_RESOURCE} tags; the request would \
             leave it with {count}"
        )
    })
}

/// The same bounds, judged on the map the REQUEST carries rather than on the one the
/// resource is left holding.
///
/// What a merging writer ([`merged_tags`]) can be refused for without reading the stored
/// map at all: merging only ever ADDS keys and never shortens one, so a submitted map
/// already past a bound leaves the resource past it whatever it holds. The count clause
/// states what the REQUEST carried, because how many the resource would then hold is not
/// known at this point — the map the resource is left with is judged by
/// [`tag_bounds_rejection`] once that map is in hand.
pub fn submitted_tag_bounds_rejection(
    member_path: &str,
    tags: &HashMap<String, String>,
) -> Option<ValidationException> {
    tag_bounds_rejection_counted(member_path, tags, |count| {
        format!(
            "A resource carries at most {MAX_TAGS_PER_RESOURCE} tags; the request carries \
             {count}"
        )
    })
}

/// Refuse a request whose tag KEYS are past the bound a key carries, judged on the keys
/// ALONE — the whole of the tag bounds for a path that names keys without values.
///
/// A tag key is as long as the request made it whatever the request asks to be done with
/// that key, so the character bound on a key is one rule across the whole tagging surface:
/// every path that takes caller-supplied tag keys judges it, whether the keys arrive in a
/// map to be written or in a list to be removed. The other two clauses of
/// [`tag_bounds_rejection`] have nothing to judge where no values arrive and nothing is
/// added: the value bound is about a value the request did not send, and the entry-count
/// bound is about the map a resource is left HOLDING, which removing keys only shrinks.
///
/// `member_path` is the wire name of the member the keys arrived in, which differs per
/// operation exactly as it does for [`reserved_tag_key_rejection`], so the sentence points
/// the caller at the member it sent. Called before anything is written, so a refused
/// request leaves the resource's tags exactly as they were.
pub fn tag_key_bounds_rejection<'a, I>(member_path: &str, keys: I) -> Option<ValidationException>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter()
        .find_map(|key| over_long_tag_key_rejection(member_path, key))
}

/// The tag-key character bound as ONE key's rejection: the single place this service
/// judges how long a tag key may be, so a key of a given length earns the same refusal
/// wherever it arrives.
fn over_long_tag_key_rejection(member_path: &str, key: &str) -> Option<ValidationException> {
    let characters = key.chars().count();
    (characters > MAX_TAG_KEY_CHARS).then(|| ValidationException {
        message: Some(validation_message(
            Some(member_path),
            &format!(
                "A tag key carries at most {MAX_TAG_KEY_CHARS} characters; one in the \
                 request carries {characters}"
            ),
        )),
        validation_exception_type: None,
    })
}

/// The bounds themselves, shared by the two readings above: `count_detail` words the
/// entry-count clause for the map being judged, while the per-entry clauses are the same
/// either way, because a key and a value are as long as the request made them.
fn tag_bounds_rejection_counted(
    member_path: &str,
    tags: &HashMap<String, String>,
    count_detail: impl Fn(usize) -> String,
) -> Option<ValidationException> {
    let refuse = |detail: String| {
        Some(ValidationException {
            message: Some(validation_message(Some(member_path), &detail)),
            validation_exception_type: None,
        })
    };
    if tags.len() > MAX_TAGS_PER_RESOURCE {
        return refuse(count_detail(tags.len()));
    }
    for (key, value) in tags {
        // The key half is the shared builder above, so this path and the key-only path
        // cannot drift in either the bound or the sentence.
        if let Some(rejection) = over_long_tag_key_rejection(member_path, key) {
            return Some(rejection);
        }
        if value.chars().count() > MAX_TAG_VALUE_CHARS {
            return refuse(format!(
                "A tag value carries at most {MAX_TAG_VALUE_CHARS} characters; one in the \
                 request carries {}",
                value.chars().count()
            ));
        }
    }
    None
}

/// The tag-key prefix AWS reserves for its own system tags.
///
/// AWS's tagging rules, applied uniformly by every taggable service: "Do not use the
/// `aws:` prefix in your tag names or values because it is reserved for AWS use. You
/// can't edit or delete tag names or values with this prefix." A caller may therefore
/// neither CREATE, REPLACE nor DELETE a tag whose key is in this namespace — the service
/// owns it — so every request that names such a key is refused before anything is
/// written. The model expresses nothing about it (`TagKey` is a bare string with no
/// `@pattern`), so the handlers enforce it.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` is in the reserved [`RESERVED_TAG_KEY_PREFIX`] namespace.
///
/// Compared WITHOUT regard to ASCII case: the namespace belongs to the service, and a
/// key spelled `AWS:` or `Aws:` names the same system namespace as `aws:`, so admitting
/// one spelling would leave the rule bypassable. A key shorter than the prefix, or one
/// whose first bytes are not a character boundary, is not in the namespace.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The requirement a reserved-key refusal states, worded as this service words every
/// other member requirement ([`constraint_detail`]): the rule, then the sentence telling
/// the caller to check what it sent.
///
/// It states the RULE and names no key from the request: the refusal is about the
/// namespace, not about which key happened to land in it, and a tag map carries no
/// ordering a message could pick a representative key from.
pub fn reserved_tag_key_requirement() -> String {
    format!(
        "Member must not contain a tag key beginning with {RESERVED_TAG_KEY_PREFIX}, a \
         prefix reserved for Amazon Web Services use. {CHECK_THE_VALUE}"
    )
}

/// Refuse a request whose tag keys reach into the reserved
/// [`RESERVED_TAG_KEY_PREFIX`] namespace, reporting the modelled rejection.
///
/// `keys` are the keys the REQUEST named — the keys it would create, replace or delete —
/// rather than the keys the resource ends up holding: a stored map can never hold a
/// reserved key (this gate is what keeps it out), and it is the request that is refused.
/// `member_path` is the wire name of the member they arrived in, which differs per
/// operation (`tags` on `TagResource`, `CreateChannelGroup` and `CreateChannel` through
/// `@jsonName`, `Tags` on `CreateOriginEndpoint` and `CreateHarvestJob`, `tagKeys` on
/// `UntagResource`), so the sentence points the caller at the member it sent.
///
/// Called before anything is written, so a refused request leaves the resource exactly
/// as it was. The discriminator is `MEMBER_INVALID`: `ValidationExceptionType` names no
/// reserved-tag reason, and `MEMBER_INVALID` is the value the enum carries for a member
/// that is wrong in a way none of its other values names.
pub fn reserved_tag_key_rejection<'a, I>(
    member_path: &str,
    keys: I,
) -> Option<ValidationException>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().any(is_reserved_tag_key).then(|| {
        validation_failed(
            member_path,
            &reserved_tag_key_requirement(),
            "MEMBER_INVALID",
        )
    })
}

/// The tag map a resource holding `current` would be left with after `submitted` is
/// merged into it, which is what [`tag_bounds_rejection`] has to judge.
///
/// Assigning a tag whose key is already there REPLACES that tag's value rather than
/// adding an entry, so the merged map is the union of the keys.
pub fn merged_tags(
    current: &HashMap<String, String>,
    submitted: &HashMap<String, String>,
) -> HashMap<String, String> {
    let mut merged = current.clone();
    merged.extend(
        submitted
            .iter()
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    merged
}

/// The key a channel group is stored under: the AWS Region the group was created in,
/// then the group's own name.
///
/// The name is the group's primary identifier, and the model says it "must be unique for
/// your account in the AWS Region" — so the Region is part of the identity, not a
/// decoration on the record: two Regions holding the same name hold two independent
/// groups, each with its own channels, origin endpoints and harvest jobs. A
/// [`BTreeMap`] keyed this way orders by Region first, so one Region's groups are a
/// contiguous range and a listing pages them in name order (see [`region_slice`]).
pub type ChannelGroupKey = (String, String);

/// The key the group `name` is stored under when the request that created it was served
/// in `region`.
pub fn channel_group_key(region: &str, name: &str) -> ChannelGroupKey {
    (region.to_string(), name.to_string())
}

/// The records of ONE Region, as `(name, record)` pairs in name order.
///
/// The scan starts at the Region's smallest possible key and stops at the first key
/// naming another Region, so nothing another Region holds is ever reported.
fn region_slice<'map, V>(
    map: &'map BTreeMap<ChannelGroupKey, V>,
    region: &str,
) -> impl Iterator<Item = (&'map String, &'map V)> {
    let region = region.to_string();
    map.range((region.clone(), String::new())..)
        .take_while(move |((stored, _), _)| *stored == region)
        .map(|((_, name), record)| (name, record))
}

/// Per-service state for com-amazonaws-mediapackagev2-mediapackagev2. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMediapackagev2Mediapackagev2State {
    /// Every channel group, keyed by [`ChannelGroupKey`] — its Region and its name.
    ///
    /// PRIVATE, and reached only through the accessors below, so no operation can resolve
    /// a name without naming the Region its request was served in.
    channel_groups: BTreeMap<ChannelGroupKey, ChannelGroup>,
}

impl ComAmazonawsMediapackagev2Mediapackagev2State {
    /// The channel group `name` names IN `region`, or `None` for a name only another
    /// Region holds: that group does not exist here.
    pub fn channel_group(&self, region: &str, name: &str) -> Option<&ChannelGroup> {
        self.channel_groups.get(&channel_group_key(region, name))
    }

    /// The channel group `name` names in `region`, for an operation that writes to the
    /// group or to a resource beneath it.
    pub fn channel_group_mut(&mut self, region: &str, name: &str) -> Option<&mut ChannelGroup> {
        self.channel_groups
            .get_mut(&channel_group_key(region, name))
    }

    /// Whether `region` already holds a channel group under `name`.
    pub fn has_channel_group(&self, region: &str, name: &str) -> bool {
        self.channel_groups
            .contains_key(&channel_group_key(region, name))
    }

    /// Store `group` under its own Region and name.
    pub fn insert_channel_group(&mut self, group: ChannelGroup) {
        let key = channel_group_key(&group.region, &group.name);
        self.channel_groups.insert(key, group);
    }

    /// Remove the channel group `name` names in `region`, reporting what was stored.
    pub fn remove_channel_group(&mut self, region: &str, name: &str) -> Option<ChannelGroup> {
        self.channel_groups.remove(&channel_group_key(region, name))
    }

    /// Every channel group stored in `region`, as `(name, record)` pairs in name order —
    /// what `ListChannelGroups` pages over.
    pub fn channel_groups_in(
        &self,
        region: &str,
    ) -> impl Iterator<Item = (&String, &ChannelGroup)> {
        region_slice(&self.channel_groups, region)
    }

    /// How many channel groups `region` holds — the count the per-Region ceiling is
    /// judged against, since a group of another Region takes nothing from this one.
    pub fn channel_group_count(&self, region: &str) -> usize {
        self.channel_groups_in(region).count()
    }

    /// How many channel groups this emulator holds across every Region — the count the
    /// store-wide ceiling ([`MAX_CHANNEL_GROUPS_TOTAL`]) is judged against.
    pub fn channel_group_total(&self) -> usize {
        self.channel_groups.len()
    }

    /// The tags of the resource `target` names, or `None` when this emulator holds no
    /// such resource in `region`.
    ///
    /// Tags live on the resource itself, so this walks the one containment hierarchy the
    /// store keeps rather than keeping a second index beside it.
    pub fn tags_of(
        &self,
        region: &str,
        target: &TagTarget,
    ) -> Option<&HashMap<String, String>> {
        let group = self.resolved_group(region, target)?;
        match &target.resource {
            TaggedResource::ChannelGroup { .. } => Some(&group.tags),
            TaggedResource::Channel { channel, .. } => Some(&group.channels.get(channel)?.tags),
            TaggedResource::OriginEndpoint {
                channel, endpoint, ..
            } => Some(
                &group
                    .channels
                    .get(channel)?
                    .origin_endpoints
                    .get(endpoint)?
                    .tags,
            ),
            TaggedResource::HarvestJob {
                channel,
                endpoint,
                job,
                ..
            } => Some(
                &group
                    .channels
                    .get(channel)?
                    .origin_endpoints
                    .get(endpoint)?
                    .harvest_jobs
                    .get(job)?
                    .tags,
            ),
        }
    }

    /// The channel group a tagging ARN reaches from a request served in `region`.
    ///
    /// `None` when the ARN names a scope this emulator does not hold the resource in
    /// ([`TagTarget::reaches`] — another account, or another Region): the resource it
    /// names lives in that scope's namespace, so from this endpoint there is nothing of
    /// that name to tag, report or untag.
    fn resolved_group(&self, region: &str, target: &TagTarget) -> Option<&ChannelGroup> {
        target.reaches(region).then_some(())?;
        self.channel_group(region, target.resource.group())
    }

    /// Apply `mutate` to the tags of the resource `target` names and commit the change it
    /// made (new `ModifiedAt`, new entity tag), reporting whether the resource was there.
    ///
    /// The tags are part of each record's content, so a tag write that CHANGED them
    /// commits a new version of the record, and one that stored exactly the tags that were
    /// already there commits nothing — the same rule every other write is judged by
    /// ([`VersionedRecord::commit_if_changed`]).
    ///
    /// `false` means the resource the ARN names does not exist in the scope a request
    /// served in `region` reaches (see [`Self::resolved_group`]), so there was nothing to
    /// write to.
    pub fn with_tags_mut(
        &mut self,
        region: &str,
        target: &TagTarget,
        at: ::aws_smithy_types::DateTime,
        mutate: impl FnOnce(&mut HashMap<String, String>),
    ) -> bool {
        if !target.reaches(region) {
            return false;
        }
        let Some(group) = self.channel_group_mut(region, target.resource.group()) else {
            return false;
        };
        match &target.resource {
            TaggedResource::ChannelGroup { .. } => {
                let before = group.configuration_fingerprint();
                mutate(&mut group.tags);
                group.commit_if_changed(&before, at);
                true
            }
            TaggedResource::Channel { channel, .. } => {
                let Some(channel) = group.channels.get_mut(channel) else {
                    return false;
                };
                let before = channel.configuration_fingerprint();
                mutate(&mut channel.tags);
                channel.commit_if_changed(&before, at);
                true
            }
            TaggedResource::OriginEndpoint {
                channel, endpoint, ..
            } => {
                let Some(endpoint) = group
                    .channels
                    .get_mut(channel)
                    .and_then(|channel| channel.origin_endpoints.get_mut(endpoint))
                else {
                    return false;
                };
                let before = endpoint.configuration_fingerprint();
                mutate(&mut endpoint.tags);
                endpoint.commit_if_changed(&before, at);
                true
            }
            TaggedResource::HarvestJob {
                channel,
                endpoint,
                job,
                ..
            } => {
                let Some(job) = group
                    .channels
                    .get_mut(channel)
                    .and_then(|channel| channel.origin_endpoints.get_mut(endpoint))
                    .and_then(|endpoint| endpoint.harvest_jobs.get_mut(job))
                else {
                    return false;
                };
                let before = job.configuration_fingerprint();
                mutate(&mut job.tags);
                job.commit_if_changed(&before, at);
                true
            }
        }
    }

    /// The origin endpoint an egress request to `region`'s data plane addresses, or
    /// `None` when any level of the path does not resolve there.
    pub fn endpoint_at(&self, region: &str, target: &EgressTarget) -> Option<&OriginEndpoint> {
        self.channel_group(region, &target.group)?
            .channels
            .get(&target.channel)?
            .origin_endpoints
            .get(&target.endpoint)
    }

    /// The channel an ingest request to `region`'s data plane addresses, or `None` when
    /// the group or the channel does not resolve there, or when the channel it names
    /// publishes no ingest endpoint for the path to belong to
    /// ([`Channel::receives_ingest`]).
    pub fn channel_at_mut(
        &mut self,
        region: &str,
        target: &IngestTarget,
    ) -> Option<&mut Channel> {
        self.channel_group_mut(region, &target.group)?
            .channels
            .get_mut(&target.channel)
            .filter(|channel| channel.receives_ingest())
    }

    /// How many bytes the ingest data plane is retaining across every channel.
    ///
    /// Summed from the store rather than tracked in a counter beside it: a counter can
    /// drift from the map it describes (a channel delete drops a whole map of objects at
    /// once), and this walk is bounded by the declared ceilings — at most
    /// [`MAX_CHANNEL_GROUPS`] x [`MAX_CHANNELS_PER_GROUP`] x
    /// [`MAX_INGESTED_OBJECTS_PER_CHANNEL`] lengths to add up — so it cannot be made
    /// expensive by a caller either.
    pub fn retained_ingested_bytes(&self) -> usize {
        self.channel_groups
            .values()
            .flat_map(|group| group.channels.values())
            .flat_map(|channel| channel.ingested_objects.values())
            .map(Vec::len)
            .sum()
    }

    /// Record one ingested object against the channel the path names, reporting what
    /// happened.
    ///
    /// Both store bounds are judged HERE, under the caller's single hold of the store
    /// lock, so the decision and the write cannot be interleaved with another push: the
    /// per-channel object count ([`MAX_INGESTED_OBJECTS_PER_CHANNEL`]) and the store-wide
    /// retained bytes ([`MAX_INGESTED_BYTES`]), counting the bytes a REPLACED object
    /// gives back. Nothing is written unless the answer is [`IngestOutcome::Recorded`].
    ///
    /// `body` is already clamped to [`MAX_INGESTED_OBJECT_BYTES`] by the caller, which is
    /// what keeps a single push from being read into memory unbounded.
    pub fn record_ingested_object(
        &mut self,
        region: &str,
        target: &IngestTarget,
        body: Vec<u8>,
    ) -> IngestOutcome {
        let retained = self.retained_ingested_bytes();
        let Some(channel) = self.channel_at_mut(region, target) else {
            return IngestOutcome::NoSuchChannel;
        };
        let replaced = channel
            .ingested_objects
            .get(&target.object)
            .map_or(0, Vec::len);
        if replaced == 0
            && channel.ingested_objects.len() >= MAX_INGESTED_OBJECTS_PER_CHANNEL
        {
            return IngestOutcome::ObjectCountReached;
        }
        if retained - replaced + body.len() > MAX_INGESTED_BYTES {
            return IngestOutcome::StoreFull;
        }
        channel.ingested_objects.insert(target.object.clone(), body);
        IngestOutcome::Recorded
    }
}

/// A persisted channel group: the top-level resource that owns the DNS every channel
/// and origin endpoint beneath it shares.
#[derive(Debug, Clone)]
pub struct ChannelGroup {
    pub name: String,
    /// The Region this group was created in — the one its ARN and its egress domain
    /// carry, and the only one it resolves, lists and mutates in. See
    /// [`ChannelGroupKey`].
    pub region: String,
    pub arn: String,
    /// The DNS domain this group's egress is announced under — the value its `EgressDomain`
    /// member carries, which is a domain NAME and nothing else: no scheme, no port, no path.
    /// Minted once by the create, in the zone AWS publishes this service's per-group egress
    /// in ([`egress_domain`]), and stored, so every later read of this group reports the same
    /// name.
    pub egress_domain: String,
    /// The `host[:port]` every URL beneath this group is rooted on — its channels' ingest
    /// endpoints and its endpoints' manifests ([`announced_data_plane_domain`]).
    ///
    /// SEPARATE from [`ChannelGroup::egress_domain`] because a URL's authority may carry the
    /// port this emulator is reached on, which a domain name may not: the two differ whenever
    /// the group's plane is announced on one of the emulator's own addresses, and are the same
    /// published hostname when it is announced in AWS's own DNS.
    pub data_plane_authority: String,
    pub description: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    /// How many times this group has been committed, starting at 1 for the create.
    ///
    /// Part of [`ChannelGroup::recompute_etag`]'s input, which is what makes every
    /// commit produce a DIFFERENT entity tag even when it writes the same values: a
    /// conditional update that lost a race must find the tag it presented stale.
    pub version: u64,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this group carried, if any — what an
    /// SDK's automatic retry of that create sends again.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this group, so a repeat
    /// carrying the same token can be told apart from a changed one.
    pub fingerprint: String,
    pub channels: BTreeMap<String, Channel>,
}

/// A persisted record whose writes are VERSIONED: every committed change bumps its
/// commit count, restamps `ModifiedAt` and re-derives its entity tag, so the tag a caller
/// holds names exactly one version of the record.
///
/// Implemented by every resource a caller-submitted write can store content into, so the
/// question each of those handlers has to answer before it commits — did this write
/// actually CHANGE the record? — is decided in ONE place instead of once per handler.
pub trait VersionedRecord {
    /// A fingerprint of everything this record IS, except the bookkeeping a commit writes
    /// (its commit count, its entity tag and its modification instant) and the
    /// sub-resources hanging off it, which commit versions of their own.
    ///
    /// Compared only within one request: derived `Debug` output is not a stability
    /// contract, so the value is never stored or returned.
    fn configuration_fingerprint(&self) -> String;

    /// Commit a change: bump the version, stamp `at` on `ModifiedAt`, re-derive the tag.
    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime);

    /// Commit the write that has just run, but only if it CHANGED the record.
    ///
    /// `before` is [`Self::configuration_fingerprint`] taken before the write. A write
    /// that changed ANY member commits a new version of the record, which rotates the
    /// entity tag — so the tag the caller held is necessarily stale, which is what makes a
    /// conditional write that lost a race detectable. A write that stored exactly what was
    /// already there changed nothing, so it commits nothing: neither the tag nor
    /// `ModifiedAt` moves, and the tag the caller holds stays usable as the precondition of
    /// the write it is entitled to make.
    fn commit_if_changed(&mut self, before: &str, at: ::aws_smithy_types::DateTime) {
        if self.configuration_fingerprint() != before {
            self.commit_version(at);
        }
    }
}

/// One record's fingerprint, taken from the record with its bookkeeping and its
/// sub-resources already scrubbed, plus the tags scrubbed out of it.
///
/// Rendered through the record's own `Debug`, so it covers every member BY CONSTRUCTION —
/// including a member added to the struct later, which is exactly what a hand-listed
/// digest gets wrong. The tags come separately, through [`tag_fingerprint`], because a
/// `HashMap`'s `Debug` order is unspecified and a rehash must not look like a change.
fn scrubbed_fingerprint(
    scrubbed: &impl ::std::fmt::Debug,
    tags: &HashMap<String, String>,
) -> String {
    format!("{}\u{1f}{scrubbed:?}", tag_fingerprint(tags))
}

impl VersionedRecord for ChannelGroup {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        // The channels beneath the group are resources in their own right, each with its
        // own version and entity tag, so they are no part of the group's content.
        own.channels = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
}

impl VersionedRecord for Channel {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        // The attached policy, the origin endpoints and what the ingest data plane
        // accepted are not channel members — no `Channel*` shape in the model carries
        // them and they take no part in the channel's entity tag either — so they are no
        // part of its content here.
        own.policy = None;
        own.origin_endpoints = BTreeMap::new();
        own.ingested_objects = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
}

impl VersionedRecord for OriginEndpoint {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        // The attached policy and the harvest jobs are sub-resources of the endpoint, not
        // endpoint members — no `OriginEndpoint*` shape in the model carries them and they
        // take no part in the endpoint's entity tag either — and each commits versions of
        // its own, so they are no part of its content here. The same scrubbing the channel
        // copy above applies to the sub-resources hanging off a channel.
        own.policy = None;
        own.harvest_jobs = BTreeMap::new();
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
}

impl VersionedRecord for HarvestJobRecord {
    fn configuration_fingerprint(&self) -> String {
        let mut own = self.clone();
        own.version = 0;
        own.etag = String::new();
        own.modified_at = ::aws_smithy_types::DateTime::from_secs(0);
        let tags = ::std::mem::take(&mut own.tags);
        scrubbed_fingerprint(&own, &tags)
    }

    fn commit_version(&mut self, at: ::aws_smithy_types::DateTime) {
        self.commit(at);
    }
}

impl ChannelGroup {
    /// Recompute this group's entity tag from its own content.
    ///
    /// The single place the group's ETag is derived, so the create path and every update
    /// path agree on what the tag is a function of.
    pub fn recompute_etag(&mut self) {
        self.etag = etag(&[
            self.arn.clone(),
            self.egress_domain.clone(),
            self.description.clone().unwrap_or_default(),
            self.version.to_string(),
            tag_fingerprint(&self.tags),
        ]);
    }

    /// Commit a change: bump the version, stamp `modified_at`, and derive the new ETag.
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }
}

/// A persisted channel: the input side of the pipeline, with the ingest endpoints a
/// contributing encoder pushes to.
#[derive(Debug, Clone)]
pub struct Channel {
    pub name: String,
    pub group_name: String,
    pub arn: String,
    pub description: Option<String>,
    /// Materialised at create time from the submitted value or the service's default, so
    /// it is never absent: see [`materialized_input_type`]. The input type is immutable,
    /// so no later write replaces it.
    pub input_type: String,
    pub ingest_endpoints: Vec<IngestEndpoint>,
    /// Materialised at create time rather than only when the caller sends one: the
    /// service reports a channel's switching state on every projection of it — see
    /// [`materialized_input_switch_configuration`].
    pub input_switch_configuration: InputSwitchConfiguration,
    /// Materialised at create time, for the same reason as
    /// [`Channel::input_switch_configuration`]: see
    /// [`materialized_output_header_configuration`].
    pub output_header_configuration: OutputHeaderConfiguration,
    pub multiview_configuration: Option<MultiviewConfiguration>,
    /// Materialised at create time on a channel whose input type may carry the member,
    /// and absent on one that may not: see [`materialized_output_locking_mode`].
    pub output_locking_mode: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    pub reset_at: Option<::aws_smithy_types::DateTime>,
    /// See [`ChannelGroup::version`].
    pub version: u64,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this channel carried, if any.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this channel.
    pub fingerprint: String,
    /// The resource policy attached to this channel by `PutChannelPolicy`, or `None`
    /// while none has ever been attached.
    ///
    /// The policy is a SUB-RESOURCE of the channel, not one of the channel's own
    /// members: no `Channel*` shape in the model carries it, so it takes no part in the
    /// channel's ETag, and because it is stored inside the channel record it disappears
    /// with the channel. `None` is what makes `GetChannelPolicy` on a never-attached
    /// policy a 404, and `DeleteChannelPolicy` what puts it back.
    pub policy: Option<String>,
    /// What the channel's ingest data plane has accepted, keyed by the request path the
    /// contributing encoder `PUT` it to. Bounded by
    /// [`MAX_INGESTED_OBJECTS_PER_CHANNEL`] / [`MAX_INGESTED_OBJECT_BYTES`].
    ///
    /// Ingested content is not a channel member either, so it stays out of the ETag for
    /// the same reason the policy does.
    pub ingested_objects: BTreeMap<String, Vec<u8>>,
    pub origin_endpoints: BTreeMap<String, OriginEndpoint>,
}

impl Channel {
    /// Recompute this channel's entity tag from its own content. See
    /// [`ChannelGroup::recompute_etag`].
    pub fn recompute_etag(&mut self) {
        self.etag = etag(&[
            self.arn.clone(),
            self.description.clone().unwrap_or_default(),
            self.input_type.clone(),
            self.output_locking_mode.clone().unwrap_or_default(),
            // The APPLIED switching and CMSD-header state is part of the channel's
            // content, so a channel that differs only in it has a different ETag. Each
            // member is rendered explicitly — `mqcs`/`publish` as 0 or 1, an absent
            // preferred input as the empty string — rather than through a derived `Debug`.
            format!(
                "{}/{}/{}",
                u8::from(
                    self.input_switch_configuration
                        .mqcs_input_switching
                        .unwrap_or_default()
                ),
                self.input_switch_configuration
                    .preferred_input
                    .map(|preferred| preferred.to_string())
                    .unwrap_or_default(),
                u8::from(
                    self.output_header_configuration
                        .publish_mqcs
                        .unwrap_or_default()
                ),
            ),
            self.version.to_string(),
            tag_fingerprint(&self.tags),
        ]);
    }

    /// Commit a change. See [`ChannelGroup::commit`].
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }

    /// Whether this channel composites video from the source channels in its multiview
    /// configuration instead of receiving ingest of its own, which is what the model's
    /// `InputType` doc says a [`INPUT_TYPE_MULTIVIEW`] channel does ("The channel receives
    /// no ingest of its own. Instead, it composites video from the source channels in its
    /// MultiviewConfiguration").
    ///
    /// Decided from the stored `InputType`, which is materialised at create time and
    /// immutable, so it is the one answer every operation that has nothing to act on such
    /// a channel branches on — rather than each handler inferring it from a different
    /// member.
    pub fn is_multiview(&self) -> bool {
        is_multiview_input_type(&self.input_type)
    }

    /// Discard everything this channel's ingest data plane has accepted.
    ///
    /// The one place a channel's retained content is dropped short of dropping the whole
    /// channel record, so the content stops being retained AND the budget it occupied —
    /// the per-channel object count ([`MAX_INGESTED_OBJECTS_PER_CHANNEL`]) and this
    /// channel's share of the store-wide retained bytes ([`MAX_INGESTED_BYTES`]), which
    /// [`ComAmazonawsMediapackagev2Mediapackagev2State::retained_ingested_bytes`] sums off
    /// the same map — is free for a later push.
    ///
    /// Touches no member of the channel's CONFIGURATION, so it neither bumps the version
    /// nor rotates the entity tag: ingested content takes no part in either (see
    /// [`Channel::ingested_objects`]).
    pub fn discard_ingested_content(&mut self) {
        self.ingested_objects.clear();
    }

    /// Whether this channel's ingest data plane serves a push.
    ///
    /// Read off the ingest endpoints the channel PUBLISHES rather than decided again from
    /// the input type: the data plane serves exactly the URLs the control plane handed
    /// out, so a channel that publishes none ([`ingest_endpoints`]) has no reachable
    /// ingest path and cannot accumulate content it never received.
    pub fn receives_ingest(&self) -> bool {
        !self.ingest_endpoints.is_empty()
    }

    /// Every harvest job this channel holds, whichever of its origin endpoints owns it.
    ///
    /// A job record is persisted on the endpoint whose content it exports, but the model
    /// gives the job's NAME the CHANNEL's namespace
    /// (`CreateHarvestJobRequest$HarvestJobName`: "This name must be unique within the
    /// channel"), so this walk — not one endpoint's map — is the collection a job name is
    /// judged against.
    pub fn harvest_jobs(&self) -> impl Iterator<Item = &HarvestJobRecord> {
        self.origin_endpoints
            .values()
            .flat_map(|endpoint| endpoint.harvest_jobs.values())
    }

    /// The harvest job of this channel called `name`, whichever of its origin endpoints
    /// persists it — the lookup every decision about a job's identity goes through,
    /// because that identity is the channel's ([`Channel::harvest_jobs`]).
    pub fn harvest_job(&self, name: &str) -> Option<&HarvestJobRecord> {
        self.harvest_jobs().find(|job| job.name == name)
    }

    /// Whether any origin endpoint of this channel already holds a harvest job called
    /// `name` — the question a create naming its job is refused by.
    pub fn holds_harvest_job(&self, name: &str) -> bool {
        self.harvest_job(name).is_some()
    }

    /// Every harvest job of this channel as `match_client_token` takes it: the candidate
    /// set a create's `ClientToken` is resolved in.
    ///
    /// The scope is the CHANNEL, the namespace a job's identity lives in, and NOT the
    /// addressed endpoint's map: a token already spent on a job under a sibling endpoint
    /// of the same channel must be found, or a second create under it would be read as a
    /// first attempt and mint a second job holding one token.
    pub fn harvest_job_token_candidates(
        &self,
    ) -> impl Iterator<Item = (&str, Option<&str>, &str)> {
        self.harvest_jobs().map(|job| {
            (
                job.name.as_str(),
                job.client_token.as_deref(),
                job.fingerprint.as_str(),
            )
        })
    }

    /// A harvest job name no job of this channel holds, derived from `seed` and the
    /// instant the job is created — what the service calls a job the caller left unnamed.
    ///
    /// Each candidate is [`derived_harvest_job_name`] under its own salt, so successive
    /// candidates differ; one more candidate is tried than the channel holds names, which
    /// is enough for one of them to be free unless two digests agree. `None` reports that
    /// impossible case rather than handing back a name already in use. The names the
    /// channel holds are gathered once, so the walk costs one pass over them however many
    /// candidates it has to try.
    pub fn unused_harvest_job_name(
        &self,
        seed: &str,
        at: ::aws_smithy_types::DateTime,
    ) -> Option<String> {
        let taken: ::std::collections::BTreeSet<&str> =
            self.harvest_jobs().map(|job| job.name.as_str()).collect();
        (0..=taken.len())
            .map(|attempt| derived_harvest_job_name(seed, at, taken.len() + attempt))
            .find(|candidate| !taken.contains(candidate.as_str()))
    }
}

/// How many characters of the digest a service-chosen harvest job name carries.
const DERIVED_HARVEST_JOB_NAME_CHARS: usize = 32;

/// One candidate name for a harvest job the caller did not name: a prefix of the digest
/// over the endpoint the job belongs to (`seed`), the instant it was created and `salt`.
///
/// Hex, so the name satisfies the model's `ResourceName` pattern `^[a-zA-Z0-9_-]+$` and
/// its 1..256 length, which is what lets every later request address the job by it.
fn derived_harvest_job_name(
    seed: &str,
    at: ::aws_smithy_types::DateTime,
    salt: usize,
) -> String {
    let digest = hex::encode(field_digest(&[
        seed.to_string(),
        at.as_secs_f64().to_string(),
        salt.to_string(),
    ]));
    digest[..DERIVED_HARVEST_JOB_NAME_CHARS].to_string()
}

/// A persisted origin endpoint: the output side, holding the manifests whose URLs
/// players fetch.
#[derive(Debug, Clone)]
pub struct OriginEndpoint {
    pub name: String,
    pub channel_name: String,
    pub group_name: String,
    pub arn: String,
    pub container_type: String,
    pub segment: Segment,
    pub description: Option<String>,
    /// Materialised at create time, but only when the parent channel's input type may
    /// carry a startover window at all: an endpoint of a channel that may not has none
    /// and reports none. See [`materialized_startover_window_seconds`].
    pub startover_window_seconds: Option<i32>,
    pub hls_manifests: Vec<GetHlsManifestConfiguration>,
    pub low_latency_hls_manifests: Vec<GetLowLatencyHlsManifestConfiguration>,
    pub dash_manifests: Vec<GetDashManifestConfiguration>,
    pub mss_manifests: Vec<GetMssManifestConfiguration>,
    pub force_endpoint_error_configuration: Option<ForceEndpointErrorConfiguration>,
    /// Materialised at create time: the service reports the separator it applied on
    /// every projection of the endpoint, the list projection included.
    pub uri_separator: String,
    /// Materialised at create time like [`OriginEndpoint::uri_separator`], but only when
    /// the parent channel's input type may carry the member at all: an endpoint of a
    /// channel that may not has no mode and reports none. See
    /// [`materialized_stream_name_output_mode`].
    pub stream_name_output_mode: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    pub reset_at: Option<::aws_smithy_types::DateTime>,
    /// See [`ChannelGroup::version`].
    pub version: u64,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this endpoint carried, if any.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this endpoint.
    pub fingerprint: String,
    /// The resource policy attached to this origin endpoint, or `None` while none is.
    ///
    /// The policy is a sub-resource of the endpoint rather than a resource of its own —
    /// `Put*` replaces it wholesale, `Get*` reports it and `Delete*` detaches it — so it
    /// is persisted ON the endpoint: deleting the endpoint takes its policy with it, and
    /// no second map can disagree about which policy an endpoint carries. It is also what
    /// gates the endpoint's egress data plane (see `crate::service`), and being a
    /// sub-resource it takes no part in the endpoint's ETag.
    pub policy: Option<EndpointPolicy>,
    /// Every harvest job created against this endpoint, keyed by job name.
    ///
    /// A job is addressed only THROUGH its endpoint — every harvest-job operation binds a
    /// URI under the endpoint's path, and the job's ARN spells the same containment — so a
    /// job cannot outlive the endpoint it was created against and is persisted on it. That
    /// is also why the endpoint's delete takes the jobs with it rather than refusing while
    /// any is left: the model declares no `ConflictException` for `DeleteOriginEndpoint`,
    /// while the two deletes above it declare one and answer it
    /// ([`children_attached`]).
    pub harvest_jobs: BTreeMap<String, HarvestJobRecord>,
}

/// A policy attached to an origin endpoint: the policy document, plus the optional CDN
/// authorization settings submitted with it (the two travel together on every
/// `PutOriginEndpointPolicy`/`GetOriginEndpointPolicy` pair).
#[derive(Debug, Clone)]
pub struct EndpointPolicy {
    pub policy: String,
    pub cdn_auth_configuration: Option<CdnAuthConfiguration>,
}

impl OriginEndpoint {
    /// Commit a change. See [`ChannelGroup::commit`].
    ///
    /// The tag is derived by [`origin_endpoint_etag`], the ONE place an endpoint's ETag
    /// comes from, so the tag a create reports, the tag an update rotates and the tag a
    /// tag change rotates are the same function of the same resource.
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.etag = origin_endpoint_etag(self);
    }

    /// The manifest names this endpoint publishes, in the families a harvest job can name
    /// — the set a job's `HarvestedManifests` is resolved against.
    pub fn published_manifests(&self) -> PublishedManifests<'_> {
        PublishedManifests {
            hls: &self.hls_manifests,
            low_latency_hls: &self.low_latency_hls_manifests,
            dash: &self.dash_manifests,
        }
    }
}

/// A set of published manifests, per harvestable family: what a harvest job's
/// `HarvestedManifests` has to resolve against.
///
/// Borrowed rather than owned, so ONE predicate judges both the set an endpoint publishes
/// now ([`OriginEndpoint::published_manifests`]) and the set a write is about to leave it
/// publishing. The model's `HarvestedManifests` carries the HLS, low-latency HLS and DASH
/// families only, so an endpoint's MSS manifests are not part of the set.
#[derive(Debug, Clone, Copy)]
pub struct PublishedManifests<'a> {
    pub hls: &'a [GetHlsManifestConfiguration],
    pub low_latency_hls: &'a [GetLowLatencyHlsManifestConfiguration],
    pub dash: &'a [GetDashManifestConfiguration],
}

/// Whether `requested` names any manifest `published` does not carry.
///
/// A harvested manifest is addressed by the `ManifestName` the endpoint publishes it
/// under, per family: HLS and low-latency HLS and DASH each have their own list, so each
/// requested family is resolved against the matching one.
///
/// A PREDICATE rather than the offending name: what a caller is told is the rule that was
/// broken, so the name it sent is never needed outside this check. The ONE place the
/// reference between a harvest job and its endpoint's manifests is resolved — read by the
/// create that establishes it ([`validation_failed`] with
/// [`VALIDATION_HARVESTED_MANIFEST_NOT_FOUND_ON_ENDPOINT`]) and by the update that would
/// break it ([`harvest_job_harvests_unpublished_manifest`]).
pub fn names_unpublished_manifest(
    requested: &HarvestedManifests,
    published: PublishedManifests<'_>,
) -> bool {
    /// The per-family rule, stated once: a requested name resolves only if the same family
    /// publishes a manifest with that name. Both sides stay iterators, so a family is
    /// walked only until a name fails to resolve.
    fn any_unpublished<'a, Requested, Published>(
        mut requested: Requested,
        published: Published,
    ) -> bool
    where
        Requested: Iterator<Item = &'a str>,
        Published: Iterator<Item = &'a str> + Clone,
    {
        requested.any(|name| !published.clone().any(|known| known == name))
    }
    any_unpublished(
        requested
            .hls_manifests
            .iter()
            .flatten()
            .map(|manifest| manifest.manifest_name.as_str()),
        published
            .hls
            .iter()
            .map(|published| published.manifest_name.as_str()),
    ) || any_unpublished(
        requested
            .low_latency_hls_manifests
            .iter()
            .flatten()
            .map(|manifest| manifest.manifest_name.as_str()),
        published
            .low_latency_hls
            .iter()
            .map(|published| published.manifest_name.as_str()),
    ) || any_unpublished(
        requested
            .dash_manifests
            .iter()
            .flatten()
            .map(|manifest| manifest.manifest_name.as_str()),
        published
            .dash
            .iter()
            .map(|published| published.manifest_name.as_str()),
    )
}

/// Whether a harvest job of `endpoint` that has NOT finished harvests a manifest the
/// `replacement` manifest set would not publish — the stored cross-reference a write that
/// replaces the endpoint's manifests would leave pointing at nothing.
///
/// Only a job that can still harvest holds such a reference: a job in a status it can no
/// longer leave ([`HarvestJobRecord::is_terminal`]) is a record of work that is over, and
/// the manifests it names are the ones it was harvesting rather than ones it still needs.
/// Statuses the clock already owed are expected to have been materialised
/// ([`HarvestJobRecord::materialise_due_transitions`]) before this is asked.
pub fn harvest_job_harvests_unpublished_manifest(
    endpoint: &OriginEndpoint,
    replacement: PublishedManifests<'_>,
) -> bool {
    endpoint
        .harvest_jobs
        .values()
        .filter(|job| !job.is_terminal())
        .any(|job| names_unpublished_manifest(&job.harvested_manifests, replacement))
}

/// The shortest time a harvest job spends in each status it has yet to leave.
///
/// AWS does not publish this latency, so it is the EMULATOR's own span for a transition the
/// clock owes. It is chosen so that a caller that acts straight after a call still observes
/// the status that call left behind and the refusals that status carries: a client that
/// polls after `CreateHarvestJob` still sees `QUEUED`, and a job created for a window that
/// had ALREADY closed is still `QUEUED` and then `IN_PROGRESS` — cancellable, which is what
/// `CancelHarvestJob` serves while a job has not reached a status it can no longer leave —
/// instead of crossing both transitions before the caller's next request arrives.
const HARVEST_JOB_TRANSITION_MILLIS: i64 = 3_000;

/// `at`, moved on by one [`HARVEST_JOB_TRANSITION_MILLIS`] span.
fn after_harvest_job_span(at: ::aws_smithy_types::DateTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(
        at.as_secs_f64() + (HARVEST_JOB_TRANSITION_MILLIS as f64 / 1000.0),
    )
}

/// Whichever of two instants comes later.
///
/// The modelled timestamp is compared through its epoch-seconds value, the same way every
/// other instant in this crate is ordered.
fn later_instant(
    one: ::aws_smithy_types::DateTime,
    other: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    if other.as_secs_f64() > one.as_secs_f64() {
        other
    } else {
        one
    }
}

/// A persisted harvest job: an export of an origin endpoint's content to S3.
#[derive(Debug, Clone)]
pub struct HarvestJobRecord {
    pub name: String,
    pub group_name: String,
    pub channel_name: String,
    pub endpoint_name: String,
    pub arn: String,
    pub description: Option<String>,
    pub harvested_manifests: HarvestedManifests,
    pub schedule_configuration: HarvesterScheduleConfiguration,
    pub destination: Destination,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: ::aws_smithy_types::DateTime,
    pub modified_at: ::aws_smithy_types::DateTime,
    /// When the clock owes this job its `QUEUED` -> `IN_PROGRESS` transition: the start of
    /// the window the caller scheduled, never sooner than one span after the create (see
    /// [`harvest_job_in_progress_at`]).
    pub in_progress_at: ::aws_smithy_types::DateTime,
    /// See [`ChannelGroup::version`].
    pub version: u64,
    pub etag: String,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the create that minted this job carried, if any — what an SDK's
    /// automatic retry of that create sends again.
    pub client_token: Option<String>,
    /// The fingerprint of the create request that minted this job, so a repeat carrying
    /// the same token can be told apart from a changed one.
    pub fingerprint: String,
}

impl HarvestJobRecord {
    /// Recompute this job's entity tag from its own content.
    pub fn recompute_etag(&mut self) {
        self.etag = etag(&[
            self.arn.clone(),
            self.status.clone(),
            self.description.clone().unwrap_or_default(),
            self.version.to_string(),
            tag_fingerprint(&self.tags),
        ]);
    }

    /// Commit a change. See [`ChannelGroup::commit`].
    pub fn commit(&mut self, at: ::aws_smithy_types::DateTime) {
        self.version = self.version.saturating_add(1);
        self.modified_at = at;
        self.recompute_etag();
    }

    /// Whether this job has reached a status it can no longer leave.
    pub fn is_terminal(&self) -> bool {
        self.status == HARVEST_JOB_CANCELLED || self.work_has_ended()
    }

    /// Whether this job's WORK has ended — a status cancelling cannot reach, because the
    /// harvest itself decided it rather than a caller
    /// ([`HARVEST_JOB_COMPLETED`], [`HARVEST_JOB_FAILED`]).
    ///
    /// The narrower half of [`HarvestJobRecord::is_terminal`], which also holds for the
    /// status a cancellation PRODUCES: a rule about what a cancellation may still do asks
    /// this one, so the state a cancellation writes never makes cancelling refuse itself.
    pub fn work_has_ended(&self) -> bool {
        self.status == HARVEST_JOB_FAILED || self.status == HARVEST_JOB_COMPLETED
    }

    /// When the clock owes this job its exit from `IN_PROGRESS`.
    ///
    /// The end of the window the CALLER scheduled, the same half of
    /// `ScheduleConfiguration` that [`harvest_job_in_progress_at`] takes the start from —
    /// but never before the job has been `IN_PROGRESS` for one
    /// [`HARVEST_JOB_TRANSITION_MILLIS`] span, so a job whose window had already ended
    /// when it was created still passes through the status it is leaving.
    fn terminal_at(&self) -> ::aws_smithy_types::DateTime {
        later_instant(
            after_harvest_job_span(self.in_progress_at),
            self.schedule_configuration.end_time,
        )
    }

    /// The instant the clock made this job's EXPORT due, or `None` while it is not owed.
    ///
    /// A job owes its export once the window it was scheduled for has ended
    /// ([`HarvestJobRecord::terminal_at`]) and it is still harvesting; the status it ends
    /// in is then decided by what that export did, which is why no status is moved here
    /// (see [`harvest_export`]). The instant is the one the export fell DUE at, so the
    /// commit that records its outcome carries it rather than the moment a client looked.
    pub fn export_due_at(
        &self,
        at: ::aws_smithy_types::DateTime,
    ) -> Option<::aws_smithy_types::DateTime> {
        let terminal_at = self.terminal_at();
        (self.status == HARVEST_JOB_IN_PROGRESS && at.as_secs_f64() >= terminal_at.as_secs_f64())
            .then_some(terminal_at)
    }

    /// Materialise the status change the CLOCK already owed this job by `at`.
    ///
    /// Called by the reporting operations as well as the mutating ones, because this
    /// transition was due at the same instant whether or not anybody read the job. It is
    /// decided by the window the CALLER scheduled: `QUEUED` -> `IN_PROGRESS` when that
    /// window begins ([`harvest_job_in_progress_at`]), no sooner than one
    /// [`HARVEST_JOB_TRANSITION_MILLIS`] span after the job was created.
    ///
    /// The job's exit from `IN_PROGRESS` is NOT here: the status it ends in reports what
    /// its export did, so it is recorded by the one path that carries that export out
    /// ([`harvest_export::settle_due_harvests`]), which calls this first.
    ///
    /// `at` decides only WHETHER the transition has fallen due; the change is committed
    /// with the instant it fell DUE, so `ModifiedAt` and the version the job reports are a
    /// function of its own schedule rather than of when a client happened to look.
    pub fn materialise_due_transitions(&mut self, at: ::aws_smithy_types::DateTime) {
        let in_progress_at = self.in_progress_at;
        if self.status == HARVEST_JOB_QUEUED && at.as_secs_f64() >= in_progress_at.as_secs_f64() {
            self.status = HARVEST_JOB_IN_PROGRESS.to_string();
            self.commit(in_progress_at);
        }
    }
}

/// The instant the clock owes `IN_PROGRESS` to a harvest job created at `created_at` for
/// a window that begins at `start_time`.
///
/// The schedule the caller submitted is WHEN THE JOB RUNS
/// (`CreateHarvestJobRequest$ScheduleConfiguration`: "The configuration for when the
/// harvest job should run, including start and end times"), so a job whose window has not
/// begun stays `QUEUED` until it does; a job whose window has already begun waits only the
/// emulator's own [`HARVEST_JOB_TRANSITION_MILLIS`] span, which is never zero. The end of
/// the same window is read by [`HarvestJobRecord::terminal_at`].
pub fn harvest_job_in_progress_at(
    created_at: ::aws_smithy_types::DateTime,
    start_time: ::aws_smithy_types::DateTime,
) -> ::aws_smithy_types::DateTime {
    later_instant(after_harvest_job_span(created_at), start_time)
}

/// The input-switch configuration a channel is stored with.
///
/// MediaPackage materialises the block on the server side, so a channel created without
/// one still has a switching state to report: the submitted block if there is one, with
/// `MQCSInputSwitching` filled in from the model's documented default ("Default is
/// false") when the caller left it out. The recorded service reports the member on a
/// channel of ANY input type, not only on one that may carry a SUBMITTED one, which is
/// what tells this member apart from [`materialized_output_locking_mode`].
/// `PreferredInput` has no documented default and the service reports none, so an omitted
/// one stays omitted.
pub fn materialized_input_switch_configuration(
    submitted: Option<InputSwitchConfiguration>,
) -> InputSwitchConfiguration {
    let mut configuration = submitted.unwrap_or_default();
    configuration.mqcs_input_switching = Some(configuration.mqcs_input_switching.unwrap_or(false));
    configuration
}

/// The output-header configuration a channel is stored with — the same materialisation
/// as [`materialized_input_switch_configuration`], one member wide: CMSD header
/// publication is off unless the caller turned it on. The off state is the one the
/// recorded service reports for a channel created without the block, on a channel of any
/// input type.
pub fn materialized_output_header_configuration(
    submitted: Option<OutputHeaderConfiguration>,
) -> OutputHeaderConfiguration {
    let mut configuration = submitted.unwrap_or_default();
    configuration.publish_mqcs = Some(configuration.publish_mqcs.unwrap_or(false));
    configuration
}

/// The input type a channel is stored with: the one the request named, else the HLS the
/// model's `InputType` doc declares for an unprovided value ("If unprovided, the value
/// defaults to HLS").
///
/// Materialised rather than held as the raw optional, because the service applies the
/// default at create time: every projection of a channel — the create response, a later
/// read, an update's response and the list item — reports a concrete input type.
pub fn materialized_input_type(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_INPUT_TYPE.to_string())
}

/// The output locking mode a channel is stored with.
///
/// The model documents a service-applied default for the member ("If you don't specify a
/// value, the default is `EPOCH_LOCKED`") AND restricts which input types may carry it at
/// all, so the default is materialised only on a channel whose input type may carry it; a
/// channel that may not has no mode and reports none — which is what the recorded service
/// reports for such a channel. The restriction is read off the same table a SUBMITTED
/// mode is refused by ([`channel_carries_member`]), so a stored mode and a refused one can
/// never disagree about which input types carry the member. The default is also the one
/// value every input type that carries the member may have, so applying it can never
/// store the value [`channel_output_locking_mode_value_violation`] refuses.
pub fn materialized_output_locking_mode(
    submitted: Option<String>,
    input_type: &str,
) -> Option<String> {
    if !channel_carries_member(|members| members.output_locking_mode = true, input_type) {
        return None;
    }
    Some(submitted.unwrap_or_else(|| DEFAULT_OUTPUT_LOCKING_MODE.to_string()))
}

/// Whether an origin endpoint on a channel with this output locking mode may carry a
/// segment output-timestamp mode at all.
///
/// `Segment$OutputTimestampMode` is "only configurable on channels with
/// `OutputLockingMode` set to `NON_EPOCH_LOCKED`", and a channel that may not carry an
/// output locking mode at all ([`materialized_output_locking_mode`]) carries none — so
/// its endpoints may not carry the mode either. The ONE reading of that restriction:
/// both the materialisation of the member's documented default
/// ([`materialized_segment`]) and the refusal of a SUBMITTED mode
/// ([`output_timestamp_mode_violation`]) decide through it, so a stored mode and a
/// refused one can never disagree about which channels carry the member.
pub fn channel_carries_output_timestamp_mode(channel_output_locking_mode: Option<&str>) -> bool {
    channel_output_locking_mode == Some(OUTPUT_TIMESTAMP_MODE_LOCKING_MODE)
}

/// The refusal for a request that names a segment output-timestamp mode on a channel
/// whose output locking mode may not carry one, or `None` when the channel may — the
/// `ValidationExceptionType` the model declares for exactly this combination.
pub fn output_timestamp_mode_violation(
    submitted: Option<&Segment>,
    channel_output_locking_mode: Option<&str>,
) -> Option<ValidationException> {
    let names_mode = submitted.is_some_and(|segment| segment.output_timestamp_mode.is_some());
    if !names_mode || channel_carries_output_timestamp_mode(channel_output_locking_mode) {
        return None;
    }
    Some(validation_failed(
        "segment.outputTimestampMode",
        &format!(
            "OutputTimestampMode can only be configured when the channel OutputLockingMode \
             is {OUTPUT_TIMESTAMP_MODE_LOCKING_MODE}. To use OutputTimestampMode, create an \
             origin endpoint on a channel with {OUTPUT_TIMESTAMP_MODE_LOCKING_MODE} output \
             locking mode"
        ),
        VALIDATION_ONLY_NON_EPOCH_LOCKED_ALLOW_OUTPUT_TIMESTAMP_MODE,
    ))
}

/// What an origin endpoint's segment configuration is materialised against: the values
/// OUTSIDE the submitted `Segment` that decide which service-applied members the stored
/// configuration carries.
///
/// Passed to [`materialized_segment`] by both of the endpoint's writers, so a create and
/// an update read the same facts about the endpoint when they apply the service's own
/// values.
#[derive(Debug, Clone, Copy)]
pub struct SegmentContext<'a> {
    /// The parent channel's materialised `OutputLockingMode`, which decides whether the
    /// endpoint carries an output-timestamp mode at all
    /// ([`channel_carries_output_timestamp_mode`]).
    pub channel_output_locking_mode: Option<&'a str>,
    /// The endpoint's `ContainerType`, which decides which `EncryptionMethod` member names
    /// the method the segments are encrypted with — and therefore whether that method
    /// takes a constant initialization vector at all
    /// ([`encryption_carries_constant_initialization_vector`]).
    pub container_type: &'a str,
    /// The endpoint's ARN: the identity the constant initialization vector the service
    /// creates is derived from ([`created_constant_initialization_vector`]), so one
    /// endpoint reports one vector for its whole lifetime and no two endpoints share one.
    pub endpoint_arn: &'a str,
}

/// The segment configuration an origin endpoint is stored with.
///
/// `Segment` is required on every origin-endpoint response and optional on the request,
/// so the service applies its own segmentation settings to whatever the caller omitted
/// and reports the result: the duration and the base name from
/// [`DEFAULT_SEGMENT_DURATION_SECONDS`] and [`DEFAULT_SEGMENT_NAME`], the TS passthrough
/// toggles on and the extra I-frame-only stream off — the state the recorded service
/// reports for an endpoint created without a segment configuration — and
/// [`DEFAULT_OUTPUT_TIMESTAMP_MODE`] on an endpoint whose channel may carry an
/// output-timestamp mode.
///
/// `Scte` and `Encryption` are reported only when they were CONFIGURED, so an endpoint
/// created without one carries none — but a block that IS configured is materialised like
/// any other part of the segment, because the model documents the service as supplying
/// members inside it: the vector, the CMAF DRM-metadata toggle and the key provider's own
/// documented values of [`materialized_encryption`], and the segment-side SCTE choice of
/// [`materialized_scte`]. The manifest-side SCTE blocks are the same rule one level out
/// ([`materialized_scte_hls`], [`materialized_scte_dash`]).
///
/// The output-timestamp mode is materialised the way a channel's own output locking mode
/// is: the member has a documented service-applied default AND the model restricts which
/// parents may carry it, so the default is applied exactly where the endpoint's channel
/// may carry one ([`channel_carries_output_timestamp_mode`]) and the member stays absent
/// on an endpoint whose channel may not. Every endpoint that can report a mode therefore
/// holds a concrete one from its create onwards, which is what makes a member documented
/// "immutable after endpoint creation" immutable from the first write on.
pub fn materialized_segment(submitted: Option<Segment>, at: SegmentContext<'_>) -> Segment {
    let mut segment = submitted.unwrap_or_default();
    let submitted_output_timestamp_mode = segment.output_timestamp_mode.take();
    segment.output_timestamp_mode = channel_carries_output_timestamp_mode(
        at.channel_output_locking_mode,
    )
    .then(|| {
        submitted_output_timestamp_mode
            .unwrap_or_else(|| DEFAULT_OUTPUT_TIMESTAMP_MODE.to_string())
    });
    segment.segment_duration_seconds = Some(
        segment
            .segment_duration_seconds
            .unwrap_or(DEFAULT_SEGMENT_DURATION_SECONDS),
    );
    segment.segment_name = Some(
        segment
            .segment_name
            .unwrap_or_else(|| DEFAULT_SEGMENT_NAME.to_string()),
    );
    segment.ts_use_audio_rendition_group = Some(
        segment
            .ts_use_audio_rendition_group
            .unwrap_or(DEFAULT_TS_USE_AUDIO_RENDITION_GROUP),
    );
    segment.include_iframe_only_streams = Some(
        segment
            .include_iframe_only_streams
            .unwrap_or(DEFAULT_INCLUDE_IFRAME_ONLY_STREAMS),
    );
    segment.ts_include_dvb_subtitles = Some(
        segment
            .ts_include_dvb_subtitles
            .unwrap_or(DEFAULT_TS_INCLUDE_DVB_SUBTITLES),
    );
    segment.scte = materialized_scte(segment.scte.take());
    segment.encryption = segment
        .encryption
        .take()
        .map(|encryption| materialized_encryption(encryption, at));
    segment
}

/// The segment-side SCTE block an origin endpoint is stored with: the submitted one with
/// the choice the service applies to it ([`DEFAULT_SCTE_IN_SEGMENTS`]) when the request
/// named none, and `None` for an endpoint that configured no SCTE block at all.
///
/// `ScteFilter` and `CustomAdTypes` pass through: the model documents no value the service
/// supplies for either — an unspecified `CustomAdTypes` is documented as "empty (only
/// default ad types are used)", which is the absent list rather than a value to report.
pub fn materialized_scte(submitted: Option<Scte>) -> Option<Scte> {
    let mut scte = submitted?;
    scte.scte_in_segments = Some(
        scte.scte_in_segments
            .unwrap_or_else(|| DEFAULT_SCTE_IN_SEGMENTS.to_string()),
    );
    Some(scte)
}

/// The HLS manifest's SCTE block as it is stored: the submitted one with the service's own
/// manifest-side choice ([`DEFAULT_SCTE_IN_MANIFESTS`]) applied when the request named
/// none, and `None` for a manifest configuration that configured no block at all. The ad
/// marker passes through — the model documents no value the service supplies for it.
pub fn materialized_scte_hls(submitted: Option<ScteHls>) -> Option<ScteHls> {
    let mut scte = submitted?;
    scte.scte_in_manifests = Some(
        scte.scte_in_manifests
            .unwrap_or_else(|| DEFAULT_SCTE_IN_MANIFESTS.to_string()),
    );
    Some(scte)
}

/// The DASH manifest's SCTE block as it is stored. The model states the manifest-side
/// default on this block in the same words it states it on the HLS one, so both read it
/// off [`DEFAULT_SCTE_IN_MANIFESTS`].
pub fn materialized_scte_dash(submitted: Option<ScteDash>) -> Option<ScteDash> {
    let mut scte = submitted?;
    scte.scte_in_manifests = Some(
        scte.scte_in_manifests
            .unwrap_or_else(|| DEFAULT_SCTE_IN_MANIFESTS.to_string()),
    );
    Some(scte)
}

/// The startover window an origin endpoint is stored with: the one the request named, or
/// the service's own ([`DEFAULT_STARTOVER_WINDOW_SECONDS`]) — on an endpoint whose parent
/// channel's input type may carry a window at all
/// ([`input_type_carries_startover_window`]), and `None` on one whose may not, the way
/// [`materialized_stream_name_output_mode`] withholds the other restricted member with a
/// documented default. Shared by the create and the update so one write cannot leave the
/// endpoint reporting a window the other would not.
pub fn materialized_startover_window_seconds(
    submitted: Option<i32>,
    input_type: &str,
) -> Option<i32> {
    if !input_type_carries_startover_window(input_type) {
        return None;
    }
    Some(submitted.unwrap_or(DEFAULT_STARTOVER_WINDOW_SECONDS))
}

/// The URI separator an origin endpoint is stored with — the submitted one, else
/// [`DEFAULT_URI_SEPARATOR`].
pub fn materialized_uri_separator(submitted: Option<String>) -> String {
    submitted.unwrap_or_else(|| DEFAULT_URI_SEPARATOR.to_string())
}

/// The stream-name output mode an origin endpoint is stored with.
///
/// The model documents a service-applied default for the member ("If you don't specify a
/// value, the default is `INDEX`") AND restricts which channel input types may carry it at
/// all ("valid only when the associated channel's `InputType` is `HLS`"), so the default is
/// materialised only under a channel whose input type may carry the member; an endpoint of
/// a channel that may not has no mode and reports none. The restriction is read off the
/// same place a SUBMITTED mode is refused by
/// ([`stream_name_output_mode_input_type_violation`]), so a stored mode and a refused one
/// can never disagree about which channels carry the member — exactly as
/// [`materialized_output_locking_mode`] does for the channel member documented this way.
pub fn materialized_stream_name_output_mode(
    submitted: Option<String>,
    input_type: &str,
) -> Option<String> {
    if !input_type_carries_stream_name_output_mode(input_type) {
        return None;
    }
    Some(submitted.unwrap_or_else(|| DEFAULT_STREAM_NAME_OUTPUT_MODE.to_string()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMediapackagev2Mediapackagev2State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMediapackagev2Mediapackagev2State>)
}

/// The region this request is addressed to, read from its SigV4 credential scope.
///
/// Every signed AWS SDK request carries the region in `Credential=.../<region>/...`;
/// an unsigned one falls back to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `now`, as the modelled timestamp type.
pub fn now() -> ::aws_smithy_types::DateTime {
    let since_epoch = ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default();
    // Millisecond resolution: restJson1 renders a timestamp as epoch seconds with a
    // fractional part, and keeping it coarser than the f64 it is serialized through
    // means the value a client reads back is exactly the value that was stored.
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_millis() as f64 / 1000.0)
}

// The fixed not-found sentence for each resource type lives with the rest of this
// service's refusal bodies, as `ResourceKind::not_found`.

/// The ARN of a channel group.
pub fn channel_group_arn(region: &str, group: &str) -> String {
    ::engine::emulator_arn(SIGNING_NAME, region, &format!("channelGroup/{group}"))
}

/// The ARN of a channel, which nests under its group's resource path.
pub fn channel_arn(region: &str, group: &str, channel: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("channelGroup/{group}/channel/{channel}"),
    )
}

/// The ARN of an origin endpoint, which nests under its channel's resource path.
pub fn origin_endpoint_arn(region: &str, group: &str, channel: &str, endpoint: &str) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!("channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}"),
    )
}

/// The ARN of a harvest job, which nests under its origin endpoint's resource path.
pub fn harvest_job_arn(
    region: &str,
    group: &str,
    channel: &str,
    endpoint: &str,
    job: &str,
) -> String {
    ::engine::emulator_arn(
        SIGNING_NAME,
        region,
        &format!(
            "channelGroup/{group}/channel/{channel}/originEndpoint/{endpoint}/harvestJob/{job}"
        ),
    )
}

/// Which resource a `TagArn` names.
///
/// Every level of the hierarchy this service mints an ARN for and stores a tag map on,
/// in the resource-path form the `*_arn` builders above produce. A level the service
/// tags is a level the tagging operations reach: the deepest of them, the harvest job,
/// carries `Tags` on its create and on its read just as its ancestors do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaggedResource {
    ChannelGroup {
        group: String,
    },
    Channel {
        group: String,
        channel: String,
    },
    OriginEndpoint {
        group: String,
        channel: String,
        endpoint: String,
    },
    HarvestJob {
        group: String,
        channel: String,
        endpoint: String,
        job: String,
    },
}

impl TaggedResource {
    /// The channel group at the root of this resource's path — the record every
    /// taggable level of the hierarchy is reached through.
    pub fn group(&self) -> &str {
        match self {
            Self::ChannelGroup { group }
            | Self::Channel { group, .. }
            | Self::OriginEndpoint { group, .. }
            | Self::HarvestJob { group, .. } => group,
        }
    }
}

/// What a tagging `ResourceArn` names: the account and the Region the ARN is scoped to,
/// and the resource its path names inside that scope.
///
/// The account and the Region are read rather than skipped because both are part of the
/// resource's identity: the same path under another account, or in another Region (see
/// [`ChannelGroupKey`]), names a different resource, so a request can only reach the one
/// its own scope holds — [`TagTarget::reaches`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagTarget {
    pub account: String,
    pub region: String,
    pub resource: TaggedResource,
}

impl TagTarget {
    /// Whether a tagging request served in `region` can reach the resource this ARN
    /// names at all.
    ///
    /// An ARN addresses one resource in one account's Region-scoped namespace, and every
    /// resource this emulator holds belongs to the single account its own ARNs carry
    /// ([`::engine::emulator_account_id`]) — so an ARN naming any other account, or a
    /// Region other than the one the request is served in, names nothing that is here to
    /// report, tag or untag. The one reading of that scope, so the reporting path
    /// ([`ServiceState::resolved_group`]) and the writing path
    /// ([`ServiceState::with_tags_mut`]) can never disagree about which ARNs reach a
    /// resource.
    pub fn reaches(&self, region: &str) -> bool {
        self.account == ::engine::emulator_account_id() && self.region == region
    }
}

/// The resource a tagging `ResourceArn` names, or `None` when the string is not a
/// syntactically valid ARN for a resource of THIS service.
///
/// An ARN is `arn:<partition>:<service>:<region>:<account>:<resource>` — six
/// colon-separated fields, the first literally `arn` — and this service's resource path
/// is `channelGroup/<g>[/channel/<c>[/originEndpoint/<e>[/harvestJob/<j>]]]`, one
/// `<type>/<name>` pair per level of the containment hierarchy. Every path the `*_arn`
/// builders above can produce parses back, so no ARN this service HANDED a caller is
/// refused as malformed. Anything else cannot name a MediaPackage resource, which is a
/// different failure from naming one that is absent: the caller has to fix the ARN
/// rather than create the resource.
///
/// The SCOPE fields an ARN carries — its account and its Region — are kept rather than
/// dropped, because they are part of what the ARN addresses; whether the scope they name
/// is one this emulator holds a resource in is [`TagTarget::reaches`], and an ARN of a
/// scope that holds nothing is an absent resource rather than a malformed ARN.
pub fn parse_tag_arn(arn: &str) -> Option<TagTarget> {
    // The six fields come from the SHARED ARN parse — the form a service reads its OWN
    // resource ARNs with ([`::service_common::arn::parse_of`]), over the same reader the
    // caller-supplied ARN members of this model are judged by ([`cdn_auth_rejection`]), so
    // this crate reads one ARN grammar rather than two.
    let parsed = ::service_common::arn::parse_of(arn, SIGNING_NAME)?;
    let segments: Vec<&str> = parsed.resource.split('/').collect();
    if segments.iter().any(|segment| segment.is_empty()) {
        return None;
    }
    let resource = match segments.as_slice() {
        ["channelGroup", group] => TaggedResource::ChannelGroup {
            group: (*group).to_string(),
        },
        ["channelGroup", group, "channel", channel] => TaggedResource::Channel {
            group: (*group).to_string(),
            channel: (*channel).to_string(),
        },
        ["channelGroup", group, "channel", channel, "originEndpoint", endpoint] => {
            TaggedResource::OriginEndpoint {
                group: (*group).to_string(),
                channel: (*channel).to_string(),
                endpoint: (*endpoint).to_string(),
            }
        }
        [
            "channelGroup",
            group,
            "channel",
            channel,
            "originEndpoint",
            endpoint,
            "harvestJob",
            job,
        ] => TaggedResource::HarvestJob {
            group: (*group).to_string(),
            channel: (*channel).to_string(),
            endpoint: (*endpoint).to_string(),
            job: (*job).to_string(),
        },
        _ => return None,
    };
    Some(TagTarget {
        account: parsed.account.to_string(),
        region: parsed.region.to_string(),
        resource,
    })
}

/// Whether `policy` lets an UNAUTHENTICATED caller perform `action`.
///
/// An anonymous request is the `*` principal, so a statement GRANTS it only by naming
/// `*` as its principal. `Deny` wins over `Allow`, as in IAM. Used by the egress data
/// plane, where the endpoint's resource policy is the only thing that can authorize a
/// player with no credentials.
///
/// This evaluator understands `Effect`, `Action`, `Principal` and `Resource` only, so it
/// fails CLOSED on anything else — in BOTH directions, which are opposite readings of the
/// same ignorance. A statement carrying a member it cannot evaluate
/// ([`UNEVALUATED_STATEMENT_MEMBERS`] — `Condition`, `NotAction`, `NotPrincipal`,
/// `NotResource`) never GRANTS, because honouring a conditional `Allow` as an
/// unconditional one would authorize a fetch on a condition nothing checked. A `Deny`
/// still DENIES unless a member this evaluator can judge rules this request out
/// ([`statement_excludes_anonymous_request`]), so a `Deny` written in the inverted or
/// conditional form — which reaches everything outside what it names, an anonymous player
/// included — is honoured rather than dropped.
///
/// `resource` is the ARN of the resource the request addresses, which a statement's
/// `Resource` is matched against: a statement that names no `Resource` applies to the
/// resource its policy is attached to, which is what a resource policy means.
pub fn policy_allows_anonymous(policy: &str, action: &str, resource: &str) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let statements: Vec<&::serde_json::Value> = match parsed.get("Statement") {
        Some(::serde_json::Value::Array(list)) => list.iter().collect(),
        Some(single @ ::serde_json::Value::Object(_)) => vec![single],
        _ => return false,
    };
    let has_effect = |statement: &::serde_json::Value, effect: &str| {
        statement.get("Effect").and_then(::serde_json::Value::as_str) == Some(effect)
    };
    // The DENY pass: an explicit `Deny` the caller stored applies to this request unless a
    // member this evaluator CAN judge rules the request out of it.
    if statements.iter().any(|statement| {
        has_effect(statement, "Deny")
            && !statement_excludes_anonymous_request(statement, action, resource)
    }) {
        return false;
    }
    // The ALLOW pass: a grant has to be stated in full, in members this evaluator judges,
    // and narrowed by nothing it cannot.
    statements.iter().any(|statement| {
        has_effect(statement, "Allow")
            && !carries_unevaluated_member(statement)
            && statement_names_action(statement, action)
            && statement_names_anonymous_principal(statement)
            && statement_names_resource(statement, resource)
    })
}

/// Whether a member this evaluator CAN judge puts this anonymous request outside the
/// statement — the reading the `Deny` pass of [`policy_allows_anonymous`] needs.
///
/// A statement reaches a request unless one of its dimensions says otherwise, so the
/// question per dimension is not "does a member name this request" but "does a member
/// this evaluator reads name something else": an `Action` list none of whose entries is
/// the action, a `Principal` naming someone other than everyone, or a `Resource` list no
/// pattern of which covers the addressed ARN. A dimension stated only in its inverted
/// form (`NotAction`, `NotPrincipal`, `NotResource`), left unstated, or written in a shape
/// this evaluator cannot read, is judged by nothing and so excludes nothing — the closed
/// side for a `Deny`, since an inverted member matches everything OUTSIDE what it names
/// and an anonymous request can be exactly that.
fn statement_excludes_anonymous_request(
    statement: &::serde_json::Value,
    action: &str,
    resource: &str,
) -> bool {
    statement_excludes_action(statement, action)
        || statement_excludes_anonymous_principal(statement)
        || statement_excludes_resource(statement, resource)
}

/// Whether the statement's `Action` names actions and none of them is `action`.
fn statement_excludes_action(statement: &::serde_json::Value, action: &str) -> bool {
    statement
        .get("Action")
        .and_then(declared_names)
        .is_some_and(|names| !names.iter().any(|name| action_name_matches(name, action)))
}

/// Whether the statement's `Principal` names principals and everyone is not among them.
fn statement_excludes_anonymous_principal(statement: &::serde_json::Value) -> bool {
    statement.get("Principal").is_some_and(|declared| {
        principal_is_readable(declared) && !statement_names_anonymous_principal(statement)
    })
}

/// Whether a `Principal` is written in a form this evaluator reads in FULL, so that a
/// value of it not being `*` is a judgement rather than a gap: the bare string, an array
/// of strings, or a map of either ([`statement_names_anonymous_principal`]).
fn principal_is_readable(declared: &::serde_json::Value) -> bool {
    let names_principals = |value: &::serde_json::Value| match value {
        ::serde_json::Value::String(_) => true,
        ::serde_json::Value::Array(list) => {
            list.iter().all(::serde_json::Value::is_string)
        }
        _ => false,
    };
    match declared {
        ::serde_json::Value::Object(map) => map.values().all(names_principals),
        other => names_principals(other),
    }
}

/// Whether the statement's `Resource` names patterns and none of them covers `resource`.
fn statement_excludes_resource(statement: &::serde_json::Value, resource: &str) -> bool {
    statement
        .get("Resource")
        .and_then(declared_names)
        .is_some_and(|patterns| {
            !patterns
                .iter()
                .any(|pattern| arn_pattern_matches(pattern, resource))
        })
}

/// The statement members [`policy_allows_anonymous`] cannot evaluate, and so refuses to
/// grant past.
///
/// Each of them narrows or inverts what a statement reaches, so ignoring one would widen
/// the grant: a `Condition` makes the `Allow` conditional, and the `Not*` forms match
/// everything OUTSIDE the value they name.
const UNEVALUATED_STATEMENT_MEMBERS: &[&str] =
    &["Condition", "NotAction", "NotPrincipal", "NotResource"];

/// Whether a statement carries a member this evaluator does not understand.
fn carries_unevaluated_member(statement: &::serde_json::Value) -> bool {
    UNEVALUATED_STATEMENT_MEMBERS
        .iter()
        .any(|member| statement.get(member).is_some())
}

/// Whether a statement's `Resource` covers `resource`.
///
/// A statement that names no `Resource` applies to the resource its policy is attached
/// to, so it covers the addressed one. A statement that DOES name one covers the request
/// only when a named pattern matches the addressed ARN, under the wildcards IAM allows in
/// a resource pattern ([`arn_pattern_matches`]).
fn statement_names_resource(statement: &::serde_json::Value, resource: &str) -> bool {
    let Some(declared) = statement.get("Resource") else {
        return true;
    };
    declared_names(declared).is_some_and(|patterns| {
        patterns
            .iter()
            .any(|pattern| arn_pattern_matches(pattern, resource))
    })
}

/// The names a statement member declares, or `None` when the member is written in a shape
/// this evaluator cannot read as a list of names.
///
/// IAM writes `Action` and `Resource` either as one string or as an array of them, and
/// every member of such a list is read the same way whichever member carries it. A list
/// is read only when EVERY entry is a name: one entry this evaluator cannot read could
/// have been the name that decides the statement, so the member as a whole is a gap rather
/// than a shorter list.
fn declared_names(declared: &::serde_json::Value) -> Option<Vec<&str>> {
    match declared {
        ::serde_json::Value::String(one) => Some(vec![one.as_str()]),
        ::serde_json::Value::Array(list) => list.iter().map(::serde_json::Value::as_str).collect(),
        _ => None,
    }
}

/// Whether an IAM resource pattern matches `value`.
///
/// IAM resource patterns take two wildcards: `*` for any run of characters (including
/// none) and `?` for exactly one. The match is anchored at both ends, and `*` backtracks,
/// so a pattern with several of them matches the way IAM's does.
fn arn_pattern_matches(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut at_pattern, mut at_value) = (0usize, 0usize);
    // Where the most recent `*` was, and how much of the value it had consumed, so a
    // failed match can give it one more character and try again.
    let mut star: Option<(usize, usize)> = None;
    while at_value < value.len() {
        let literal = at_pattern < pattern.len()
            && (pattern[at_pattern] == '?' || pattern[at_pattern] == value[at_value]);
        if literal {
            at_pattern += 1;
            at_value += 1;
        } else if at_pattern < pattern.len() && pattern[at_pattern] == '*' {
            star = Some((at_pattern, at_value));
            at_pattern += 1;
        } else if let Some((star_pattern, star_value)) = star {
            at_pattern = star_pattern + 1;
            at_value = star_value + 1;
            star = Some((star_pattern, star_value + 1));
        } else {
            return false;
        }
    }
    // The value is exhausted, so what is left of the pattern may only be wildcards that
    // match nothing.
    pattern[at_pattern..].iter().all(|glob| *glob == '*')
}

/// Whether a statement's `Action` covers `action`, honouring the `*` wildcard IAM allows
/// both on its own and as a trailing one (`service:*`).
fn statement_names_action(statement: &::serde_json::Value, action: &str) -> bool {
    statement
        .get("Action")
        .and_then(declared_names)
        .is_some_and(|names| names.iter().any(|name| action_name_matches(name, action)))
}

/// Whether one declared action name covers `action`, honouring the `*` wildcard IAM
/// allows both on its own and as a trailing one (`service:*`).
fn action_name_matches(name: &str, action: &str) -> bool {
    match name.strip_suffix('*') {
        Some(prefix) => action.starts_with(prefix),
        None => name.eq_ignore_ascii_case(action),
    }
}

/// Whether a statement's `Principal` names everyone — the only principal an anonymous
/// request can match. IAM writes it either as the bare string `"*"` or as a map whose
/// values are `"*"` (`{"AWS": "*"}`).
fn statement_names_anonymous_principal(statement: &::serde_json::Value) -> bool {
    let is_everyone = |value: &::serde_json::Value| match value {
        ::serde_json::Value::String(one) => one == "*",
        ::serde_json::Value::Array(list) => list
            .iter()
            .any(|entry| entry.as_str() == Some("*")),
        _ => false,
    };
    match statement.get("Principal") {
        Some(::serde_json::Value::Object(map)) => map.values().any(is_everyone),
        Some(value) => is_everyone(value),
        None => false,
    }
}

/// The egress domain a channel group is given: a DOMAIN NAME and nothing else — dotted
/// labels, no scheme, no port and no path — so it can be resolved, or handed to a CDN as an
/// origin domain name, as it stands.
///
/// It is the host every manifest URL beneath the group is served from in AWS, so all of the
/// group's channels and endpoints share one DNS name, and it is the value the group's
/// `EgressDomain` member carries for EVERY create: the name AWS publishes this service's
/// per-group egress under, in the service's own regional zone ([`AWS_DNS_ZONE`]), whatever
/// address the creating request arrived on. The address a caller reaches this emulator at is
/// a different value, carried by the full URLs rooted on the group's data-plane authority
/// ([`announced_data_plane_domain`]), which is where a port may travel.
///
/// The two opaque labels are derived from the account and from the group's identity, so
/// the domain is a function of the resource (stable across reads, distinct per group)
/// rather than of the moment it was created. The labels in front of the zone name the plane,
/// the Region and the group, which is what lets the one reading of a published host
/// ([`data_plane_of_host`], [`data_plane_region`]) recognise the name as this service's and
/// serve the plane it announces.
pub fn egress_domain(region: &str, group: &str) -> String {
    let account = ::engine::emulator_account_id();
    let group_label = short_digest(&[account, region, group], 8);
    let account_label = short_digest(&[account], 12);
    format!("{group_label}.egress.{account_label}.{SIGNING_NAME}.{region}.{AWS_DNS_ZONE}")
}

/// The DNS zone AWS itself publishes this service's endpoints and its per-group data-plane
/// hostnames in.
const AWS_DNS_ZONE: &str = "amazonaws.com";

/// The ingest host for a group, derived from the SAME domain egress is served from —
/// the guarantee the service makes is that both sides of a group share its DNS.
fn ingest_domain(egress_domain: &str) -> String {
    egress_domain.replacen(EGRESS_HOST_LABEL, INGEST_HOST_LABEL, 1)
}

/// The label that marks a hostname as this service's EGRESS data plane, as
/// [`egress_domain`] builds it.
const EGRESS_HOST_LABEL: &str = ".egress.";

/// The label that marks a hostname as this service's INGEST data plane, as
/// [`ingest_domain`] builds it.
const INGEST_HOST_LABEL: &str = ".ingest.";

/// Which half of this service's data plane a hostname addresses.
///
/// The control plane is reached at `mediapackagev2.<region>.amazonaws.com`, whose first
/// label names the service; the data plane is reached at the per-group hostnames
/// [`egress_domain`] and [`ingest_domain`] build, whose first label is opaque. Those
/// hostnames are the ones this service PUBLISHES in `EgressDomain` and in a channel's
/// `IngestEndpoints[].Url`, so recognising them is how a request to a published URL
/// reaches the service that published it instead of the control-plane resolver.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataPlane {
    /// A contributing encoder pushing content to a channel.
    Ingest,
    /// A player fetching content from an origin endpoint.
    Egress,
}

/// The half of the data plane `host` addresses, or `None` for any other hostname
/// (the control-plane endpoint included).
pub fn data_plane_of_host(host: &str) -> Option<DataPlane> {
    let host = host_name(host);
    if !host.contains(&format!(".{SIGNING_NAME}.")) {
        return None;
    }
    if host.contains(INGEST_HOST_LABEL) {
        return Some(DataPlane::Ingest);
    }
    if host.contains(EGRESS_HOST_LABEL) {
        return Some(DataPlane::Egress);
    }
    None
}

/// The Region a published data-plane hostname is served in, or `None` when the host
/// carries no region label.
///
/// MediaPackage's data plane is regional exactly as its control plane is: the hostnames
/// [`egress_domain`] and [`ingest_domain`] publish end in `<service>.<region>.amazonaws.com`,
/// so the published URL itself names the Region whose resources it serves — which is how
/// a push or a fetch resolves the group it addresses without a credential scope to read.
pub fn data_plane_region(host: &str) -> Option<String> {
    let host = host_name(host);
    let (_, after_service) = host.split_once(&format!(".{SIGNING_NAME}."))?;
    let region = after_service.split('.').next()?;
    (!region.is_empty()).then(|| region.to_string())
}

/// The bare hostname of a `Host` header or URI authority: no userinfo, no port.
fn host_name(host: &str) -> &str {
    let host = host.rsplit('@').next().unwrap_or(host);
    host.split(':').next().unwrap_or(host)
}

/// What an egress request path names: the endpoint whose policy gates it, plus the
/// object beneath that endpoint.
///
/// The path is the one [`manifest_url`] builds: `/out/v1/<group>/<channel>/<endpoint>/…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EgressTarget {
    pub group: String,
    pub channel: String,
    pub endpoint: String,
    /// Everything after the endpoint — the manifest or segment being fetched.
    pub object: String,
}

/// The endpoint and object an egress path names, or `None` when the path is not one this
/// service publishes.
pub fn parse_egress_path(path: &str) -> Option<EgressTarget> {
    let rest = path.strip_prefix("/out/v1/")?;
    let mut segments = rest.splitn(4, '/');
    let (Some(group), Some(channel), Some(endpoint), Some(object)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if [group, channel, endpoint, object]
        .iter()
        .any(|segment| segment.is_empty())
    {
        return None;
    }
    Some(EgressTarget {
        group: group.to_string(),
        channel: channel.to_string(),
        endpoint: endpoint.to_string(),
        object: object.to_string(),
    })
}

/// What an ingest request path names: the channel the content is being pushed to.
///
/// The path is the one [`ingest_endpoints`] builds:
/// `/in/v1/<group>/<index>/<channel>/…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestTarget {
    pub group: String,
    pub channel: String,
    /// Everything after the channel — what the encoder is writing.
    pub object: String,
}

/// The channel an ingest path names, or `None` when the path is not one this service
/// publishes.
pub fn parse_ingest_path(path: &str) -> Option<IngestTarget> {
    let rest = path.strip_prefix("/in/v1/")?;
    let mut segments = rest.splitn(4, '/');
    let (Some(group), Some(_index), Some(channel), Some(object)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if [group, channel, object]
        .iter()
        .any(|segment| segment.is_empty())
    {
        return None;
    }
    Some(IngestTarget {
        group: group.to_string(),
        channel: channel.to_string(),
        object: object.to_string(),
    })
}

/// The ingest endpoints a channel of this input type is created with, derived from the
/// parent group's announced data-plane domain and the channel's name: the redundant pair
/// for a channel that receives ingest of its own, and NONE for one that does not (see
/// [`is_multiview_input_type`]) — a channel with no ingest stream of its own has no URL
/// for a contributing encoder to push to, and `IngestEndpoints` carries no `@required` on
/// any channel shape, so reporting none is what the model represents that with.
///
/// Each endpoint of the pair is a SEPARATE ingest target, so on the hostnames AWS's own
/// addressing publishes each is published on its own: the group's ingest DNS
/// ([`ingest_domain`]) with a first label of its own, built from the channel's identity and
/// that endpoint's ordinal ([`ingest_endpoint_label`]). Two redundant endpoints that
/// differed only in their URL PATH would be one host to a client that pins or
/// load-balances per ingest endpoint, which cannot distinguish the two publishers
/// MediaPackage hands them to. On the EMULATOR'S own endpoint there is one address to
/// publish on, so the pair is distinguished the only way it can be there — by the ordinal
/// their paths already carry (see [`data_plane_url`]).
pub fn ingest_endpoints(
    data_plane_authority: &str,
    region: &str,
    group: &str,
    channel: &str,
    input_type: &str,
) -> Vec<IngestEndpoint> {
    if is_multiview_input_type(input_type) {
        return Vec::new();
    }
    (1..=INGEST_ENDPOINTS_PER_CHANNEL)
        .map(|index| IngestEndpoint {
            id: Some(index.to_string()),
            url: Some(data_plane_url(
                &ingest_endpoint_domain(data_plane_authority, channel, index),
                region,
                &format!("/in/v1/{group}/{index}/{channel}/index"),
            )),
        })
        .collect()
}

/// The domain ONE ingest endpoint of a channel is published on: its own hostname under the
/// group's ingest DNS when the group's data plane is announced on the hostnames AWS's own
/// addressing publishes, and the emulator's own address otherwise — which is the one
/// address a client on this machine can reach, for every endpoint of every channel.
fn ingest_endpoint_domain(data_plane_authority: &str, channel: &str, ordinal: u8) -> String {
    if data_plane_of_host(data_plane_authority).is_none() {
        return data_plane_authority.to_string();
    }
    let domain = ingest_domain(data_plane_authority);
    // Everything after the group's own first label: the DNS both halves of a group's data
    // plane share, which is what each endpoint's own label is prefixed onto.
    let group_dns = domain
        .split_once('.')
        .map_or(domain.as_str(), |(_label, rest)| rest);
    format!(
        "{}.{group_dns}",
        ingest_endpoint_label(&domain, channel, ordinal)
    )
}

/// The first label of ONE ingest endpoint's hostname: a label derived from the channel's
/// identity, carrying that endpoint's ordinal.
///
/// A function of the resource and the ordinal, so the host is stable across reads, distinct
/// per endpoint of a channel and distinct per channel of a group — never of the moment the
/// channel was created. The digest part is this emulator's own opaque label; what the
/// service guarantees about it is that the endpoints of one channel differ in it.
fn ingest_endpoint_label(ingest_domain: &str, channel: &str, ordinal: u8) -> String {
    format!(
        "{}-{ordinal}",
        short_digest(&[ingest_domain, channel], INGEST_ENDPOINT_LABEL_CHARS)
    )
}

/// How many digest characters an ingest endpoint's own label carries before its ordinal.
const INGEST_ENDPOINT_LABEL_CHARS: usize = 6;

/// The container-format suffix a manifest's URL ends in.
///
/// One entry per manifest family MediaPackage serves; the family decides the suffix,
/// not the endpoint's container type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ManifestKind {
    /// HLS and low-latency HLS both publish an `.m3u8` playlist.
    Hls,
    /// DASH publishes an `.mpd` media presentation description.
    Dash,
    /// Microsoft Smooth Streaming publishes an `.ism` presentation manifest.
    Mss,
}

impl ManifestKind {
    fn suffix(self) -> &'static str {
        match self {
            Self::Hls => ".m3u8",
            Self::Dash => ".mpd",
            Self::Mss => ".ism/Manifest",
        }
    }
}

/// The egress URL of one manifest: the group's announced data-plane authority plus the path
/// that names the group, the channel, the endpoint and the manifest.
pub fn manifest_url(
    data_plane_authority: &str,
    region: &str,
    group: &str,
    channel: &str,
    endpoint: &str,
    manifest_name: &str,
    kind: ManifestKind,
) -> String {
    data_plane_url(
        data_plane_authority,
        region,
        &format!(
            "/out/v1/{group}/{channel}/{endpoint}/{manifest_name}{}",
            kind.suffix()
        ),
    )
}

/// Where a manifest's URL is rooted: the parent group's announced data-plane authority and
/// the Region it serves, plus the names of the group, the channel and the origin endpoint
/// the manifest belongs to.
///
/// Passed to the `Create*` -> `Get*` conversions below so every manifest family builds
/// its URL from the same values.
#[derive(Debug, Clone, Copy)]
pub struct ManifestLocation<'a> {
    pub data_plane_authority: &'a str,
    pub region: &'a str,
    pub group: &'a str,
    pub channel: &'a str,
    pub endpoint: &'a str,
}

impl ManifestLocation<'_> {
    fn url(&self, manifest_name: &str, kind: ManifestKind) -> String {
        manifest_url(
            self.data_plane_authority,
            self.region,
            self.group,
            self.channel,
            self.endpoint,
            manifest_name,
            kind,
        )
    }
}

/// The stored form of a submitted HLS manifest: everything the caller asked for, plus
/// the URL the service computes for it and the manifest settings the service applies to
/// what the caller left out ([`DEFAULT_MANIFEST_WINDOW_SECONDS`],
/// [`DEFAULT_URI_PATH_TYPE`], [`DEFAULT_CHILD_MANIFEST_NAME`], child-manifest URL
/// encoding off, and the manifest-side SCTE choice inside a configured `ScteHls`).
pub fn stored_hls_manifest(
    submitted: CreateHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetHlsManifestConfiguration {
    GetHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: Some(
            submitted
                .child_manifest_name
                .unwrap_or_else(|| DEFAULT_CHILD_MANIFEST_NAME.to_string()),
        ),
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        program_date_time_interval_seconds: submitted.program_date_time_interval_seconds,
        scte_hls: materialized_scte_hls(submitted.scte_hls),
        filter_configuration: submitted.filter_configuration,
        start_tag: submitted.start_tag,
        url_encode_child_manifest: Some(
            submitted
                .url_encode_child_manifest
                .unwrap_or(DEFAULT_URL_ENCODE_CHILD_MANIFEST),
        ),
        uri_path_type: Some(
            submitted
                .uri_path_type
                .unwrap_or_else(|| DEFAULT_URI_PATH_TYPE.to_string()),
        ),
    }
}

/// The stored form of a submitted low-latency HLS manifest. Low-latency HLS publishes
/// the same `.m3u8` playlist as plain HLS, so it shares the URL rule and the same
/// manifest-setting defaults, child manifest name included.
pub fn stored_low_latency_hls_manifest(
    submitted: CreateLowLatencyHlsManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetLowLatencyHlsManifestConfiguration {
    GetLowLatencyHlsManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Hls),
        manifest_name: submitted.manifest_name,
        child_manifest_name: Some(
            submitted
                .child_manifest_name
                .unwrap_or_else(|| DEFAULT_CHILD_MANIFEST_NAME.to_string()),
        ),
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        program_date_time_interval_seconds: submitted.program_date_time_interval_seconds,
        scte_hls: materialized_scte_hls(submitted.scte_hls),
        filter_configuration: submitted.filter_configuration,
        start_tag: submitted.start_tag,
        url_encode_child_manifest: Some(
            submitted
                .url_encode_child_manifest
                .unwrap_or(DEFAULT_URL_ENCODE_CHILD_MANIFEST),
        ),
        uri_path_type: Some(
            submitted
                .uri_path_type
                .unwrap_or_else(|| DEFAULT_URI_PATH_TYPE.to_string()),
        ),
    }
}

/// The stored form of a submitted DASH manifest. The window and the URI path type are
/// the same server-applied settings the HLS families carry, and the bandwidth reporting
/// mode is the DASH family's own one ([`DEFAULT_DASH_BANDWIDTH_REPORTING_MODE`]);
/// everything else is reported only when it was configured.
pub fn stored_dash_manifest(
    submitted: CreateDashManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetDashManifestConfiguration {
    GetDashManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Dash),
        manifest_name: submitted.manifest_name,
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        filter_configuration: submitted.filter_configuration,
        min_update_period_seconds: submitted.min_update_period_seconds,
        min_buffer_time_seconds: submitted.min_buffer_time_seconds,
        suggested_presentation_delay_seconds: submitted.suggested_presentation_delay_seconds,
        segment_template_format: submitted.segment_template_format,
        period_triggers: submitted.period_triggers,
        scte_dash: materialized_scte_dash(submitted.scte_dash),
        drm_signaling: submitted.drm_signaling,
        utc_timing: submitted.utc_timing,
        profiles: submitted.profiles,
        base_urls: submitted.base_urls,
        program_information: submitted.program_information,
        dvb_settings: submitted.dvb_settings,
        compactness: submitted.compactness,
        bandwidth_reporting_mode: Some(
            submitted
                .bandwidth_reporting_mode
                .unwrap_or_else(|| DEFAULT_DASH_BANDWIDTH_REPORTING_MODE.to_string()),
        ),
        audio_timeline_pattern: submitted.audio_timeline_pattern,
        subtitle_configuration: submitted.subtitle_configuration,
        uri_path_type: Some(
            submitted
                .uri_path_type
                .unwrap_or_else(|| DEFAULT_URI_PATH_TYPE.to_string()),
        ),
        availability_start_time_configuration: submitted.availability_start_time_configuration,
    }
}

/// The stored form of a submitted Microsoft Smooth Streaming manifest. MSS models no
/// URI path type, so the window is the only server-applied setting it carries.
pub fn stored_mss_manifest(
    submitted: CreateMssManifestConfiguration,
    at: ManifestLocation<'_>,
) -> GetMssManifestConfiguration {
    GetMssManifestConfiguration {
        url: at.url(&submitted.manifest_name, ManifestKind::Mss),
        manifest_name: submitted.manifest_name,
        filter_configuration: submitted.filter_configuration,
        manifest_window_seconds: Some(
            submitted
                .manifest_window_seconds
                .unwrap_or(DEFAULT_MANIFEST_WINDOW_SECONDS),
        ),
        manifest_layout: submitted.manifest_layout,
    }
}

/// The names of the multiview channels in `group` that list `channel_name` among their
/// `MultiviewConfiguration.AvailableSources`.
///
/// The model calls `AttachedMultiviewChannels` a read-only field: it is the inverse of
/// the available-source lists the group's other channels were created or updated with, so
/// every channel reader DERIVES it from the store rather than storing it on the channel.
/// `None` when nothing references the channel, so the member is reported only once there
/// is an attachment to report.
pub fn attached_multiview_channels(group: &ChannelGroup, channel_name: &str) -> Option<Vec<String>> {
    // `channels` is a BTreeMap, so the names come out in name order.
    let attached: Vec<String> = group
        .channels
        .values()
        .filter(|candidate| {
            candidate
                .multiview_configuration
                .as_ref()
                .is_some_and(|multiview| {
                    multiview
                        .available_sources
                        .iter()
                        .any(|source| source == channel_name)
                })
        })
        .map(|candidate| candidate.name.clone())
        .collect();
    (!attached.is_empty()).then_some(attached)
}

/// A length-prefixed digest over the fields the caller supplies.
///
/// The one hashing primitive this module has: [`etag`] renders an entity tag from it and
/// [`request_fingerprint`] a create's idempotency fingerprint. The length prefix is what
/// makes two different field sequences unable to collide, so what the digest means is
/// decided entirely by which fields the caller passes.
fn field_digest(fields: &[String]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for field in fields {
        // Length-prefixed, so ["a", "bc"] and ["ab", "c"] cannot hash alike.
        hasher.update(field.len().to_le_bytes());
        hasher.update(field.as_bytes());
    }
    hasher.finalize().into()
}

/// The entity tag over the fields the caller supplies: the BASE64 of their digest
/// ([`field_digest`]).
///
/// Each resource's `recompute_etag` (and [`origin_endpoint_etag`]) builds its `ETag` here,
/// so the whole service hands entity tags out in one encoding. That encoding is base64 of
/// the 32 digest bytes — the form every `ETag` MediaPackage issues takes, a 44-character
/// token ending in the base64 padding character — rather than the digest's hex, which
/// would make a token this emulator issues twice as long as the real service's and so not
/// interchangeable with one a client recorded, persisted or pattern-checked. The modelled
/// `EntityTag` (`@length` 1..256, `@pattern ^[\S]+$`) admits the base64 alphabet, padding
/// included.
pub fn etag(fields: &[String]) -> String {
    ::base64::Engine::encode(
        &::base64::engine::general_purpose::STANDARD,
        field_digest(fields),
    )
}

/// A tag map rendered in a canonical order, for feeding to [`etag`]. A `HashMap`
/// iterates in an arbitrary order, so hashing it directly would make the ETag depend on
/// the iteration rather than on the tags.
pub fn tag_fingerprint(tags: &HashMap<String, String>) -> String {
    let mut pairs: Vec<(&String, &String)> = tags.iter().collect();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("\u{1f}")
}

// =============================================================================
// Idempotency: a create's `ClientToken`
// =============================================================================

/// What a create request's `ClientToken` matched among the resources already stored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenMatch {
    /// Nothing stored carries this token (or the request carried none), so the create
    /// goes ahead as a first attempt.
    Fresh,
    /// THIS request already created the named resource: answer with what was stored
    /// then, and create nothing.
    Replay(String),
    /// A stored resource carries this token but was created by a DIFFERENT request.
    Mismatch,
}

/// Match a create request's idempotency token against the resources already stored at
/// the same level of the hierarchy.
///
/// `stored` yields one `(name, the token that create carried, the fingerprint of that
/// create's request)` per sibling. Every create in this model binds `ClientToken` with
/// `@idempotencyToken` and is `@idempotent`, and the model's `ConflictExceptionType`
/// carries both `RESOURCE_ALREADY_EXISTS` and `IDEMPOTENT_PARAMETER_MISMATCH`: a repeat
/// of the SAME request under the same token replays the first result, while a repeat
/// that changed any member is a conflict. The name is part of the fingerprint, so
/// reusing a token for a differently-named resource is a mismatch too.
pub fn match_client_token<'a>(
    token: Option<&str>,
    fingerprint: &str,
    stored: impl Iterator<Item = (&'a str, Option<&'a str>, &'a str)>,
) -> TokenMatch {
    let Some(token) = token else {
        return TokenMatch::Fresh;
    };
    for (name, stored_token, stored_fingerprint) in stored {
        if stored_token == Some(token) {
            return if stored_fingerprint == fingerprint {
                TokenMatch::Replay(name.to_string())
            } else {
                TokenMatch::Mismatch
            };
        }
    }
    TokenMatch::Fresh
}

/// The fingerprint of a create request: a digest of every member it carried EXCEPT its
/// `ClientToken`, which is the identity of the request rather than part of it.
///
/// Two requests share a fingerprint exactly when they ask for the same resource with the
/// same configuration, which is what distinguishes an SDK's retry of one create from a
/// second, different create sent under the same token.
///
/// A caller renders each field; an OPTIONAL member is rendered so that ABSENT and
/// present-but-default stay apart (`format!("{:?}", member)` does), because a member the
/// request leaves out is materialised to the service's own default: were the two
/// flattened to one field, a repeat that submitted that default would ask for a different
/// resource under the first attempt's fingerprint.
pub fn request_fingerprint(fields: &[String]) -> String {
    // The same length-prefixed digest an ETag is built from, for the same reason: no two
    // different field sequences may collide. A fingerprint is never on the wire — it is
    // only ever compared with another fingerprint of this store — so it is rendered in hex
    // rather than in the base64 an `ETag` has to be handed out in.
    hex::encode(field_digest(fields))
}

/// The conflict a repeat create under a token that was used for a DIFFERENT request is
/// refused with: 409, `IDEMPOTENT_PARAMETER_MISMATCH`, and the service's fixed sentence.
///
/// The sentence names no value from the request — least of all the `ClientToken` itself,
/// which is the caller's own credential-like identifier for the attempt. It follows the
/// rule the refusal builders below state: a refusal's words depend only on WHAT was
/// refused, never on the identifiers the caller sent.
pub fn idempotent_parameter_mismatch() -> ConflictException {
    ConflictException {
        message: Some(
            "You've specified a client token with your request using parameter values \
             that differ from a previous request that used the same client token. Check \
             your parameters and then try again"
                .to_string(),
        ),
        conflict_exception_type: Some("IDEMPOTENT_PARAMETER_MISMATCH".to_string()),
    }
}

// =============================================================================
// Refusals: the MODELED error bodies this service answers with
//
// Every constructor here builds one of the model's own client errors
// (`ResourceNotFoundException`, `ConflictException`, `ValidationException`), so a
// refusal carries the members the model declares rather than a bare sentence.
//
// The rule they all follow: mediapackagev2 states a refusal in words that depend only
// on WHAT was refused — the kind of resource, the constraint that was violated — and
// never on the identifiers the caller sent. The discriminator member
// (`ResourceTypeNotFound`, `ConflictExceptionType`, `ValidationExceptionType`) and the
// sentence are two readings of that same fact, so they are chosen together, here, and
// no handler words a refusal of its own.
// =============================================================================

/// Which of a channel's restricted members a request carries.
///
/// The `InputType` restrictions below are about the REQUEST, not about any one
/// operation's input shape, so each handler reduces its own generated input to this and
/// the rule table is written once. `CreateChannel` and `UpdateChannel` therefore cannot
/// disagree about which input type may carry which member.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubmittedChannelMembers {
    pub output_locking_mode: bool,
    pub mqcs_input_switching: bool,
    pub preferred_input: bool,
    pub publish_mqcs: bool,
    pub multiview_configuration: bool,
}

/// A channel member the service only lets a particular set of input types carry.
struct ChannelInputTypeRule {
    /// The member, as a validation message names it (lowerCamel, as on the wire).
    member: &'static str,
    /// The `InputType` values that may carry it, from the model's `InputType` enum.
    allowed: &'static [&'static str],
    /// The `ValidationExceptionType` the model declares for this combination.
    exception_type: &'static str,
    /// The `ValidationExceptionType` for the OPPOSITE direction, on a member whose doc
    /// also calls it REQUIRED on the input types that may carry it: the one the model
    /// names for the member being absent on such an input type. `None` on a member the
    /// docs only permit, never require.
    required_as: Option<&'static str>,
    /// What the member is called in the sentence the refusal states.
    label: &'static str,
    /// Whether the request carries the member at all.
    present: fn(&SubmittedChannelMembers) -> bool,
}

/// Every `InputType` restriction the model states on the PRESENCE of a channel member,
/// one row per `ValidationExceptionType` that names one. A restriction on a member's
/// VALUE rather than on its presence is stated beside it, in
/// [`channel_output_locking_mode_value_violation`].
///
/// The rows are read off the model and off the refusal the service itself answers, not
/// guessed. `MQCSInputSwitching` / `PreferredInput` / `PublishMQCS` carry "valid only when
/// InputType is CMAF" and `MultiviewConfiguration` "required when InputType is MULTIVIEW,
/// and can't be set for any other input type" in their own member docs. For
/// `OutputLockingMode` the recorded refusal names the input types that may configure the
/// member — "OutputLockingMode can only be configured when the channel InputType is CMAF
/// or MULTIVIEW" — which is also the only reading that leaves the model's
/// `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` discriminator something to refuse: a
/// multiview channel carries the member, at one of its two values only
/// ([`channel_output_locking_mode_value_violation`]). `ValidationExceptionType` carries
/// exactly one discriminator per row.
const CHANNEL_INPUT_TYPE_RULES: &[ChannelInputTypeRule] = &[
    ChannelInputTypeRule {
        member: OUTPUT_LOCKING_MODE_MEMBER,
        allowed: &["CMAF", INPUT_TYPE_MULTIVIEW],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_OUTPUT_LOCKING_MODE",
        // The member has a documented DEFAULT, so an absent one is materialised rather
        // than refused: see `materialized_output_locking_mode`.
        required_as: None,
        label: "OutputLockingMode",
        present: |submitted| submitted.output_locking_mode,
    },
    ChannelInputTypeRule {
        member: "inputSwitchConfiguration.mqcsInputSwitching",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_INPUT_SWITCHING",
        required_as: None,
        label: "MQCS input switching",
        present: |submitted| submitted.mqcs_input_switching,
    },
    ChannelInputTypeRule {
        member: "inputSwitchConfiguration.preferredInput",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_PREFERRED_INPUT_CONFIGURATION",
        required_as: None,
        label: "PreferredInput",
        present: |submitted| submitted.preferred_input,
    },
    ChannelInputTypeRule {
        member: "outputHeaderConfiguration.publishMqcs",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_MQCS_OUTPUT_CONFIGURATION",
        required_as: None,
        label: "PublishMQCS",
        present: |submitted| submitted.publish_mqcs,
    },
    ChannelInputTypeRule {
        member: "multiviewConfiguration",
        allowed: &[INPUT_TYPE_MULTIVIEW],
        exception_type: "MULTIVIEW_CONFIGURATION_NOT_ALLOWED",
        // The only member the docs state in BOTH directions: "required when InputType is
        // MULTIVIEW, and can't be set for any other input type".
        required_as: Some("MULTIVIEW_CONFIGURATION_REQUIRED"),
        label: "MultiviewConfiguration",
        present: |submitted| submitted.multiview_configuration,
    },
];

/// The `InputType` whose channels are the only ones that carry a multiview
/// configuration, and which require one, from the model's `InputType` enum.
pub const INPUT_TYPE_MULTIVIEW: &str = "MULTIVIEW";

/// Whether a channel of this input type composites video from the source channels in its
/// multiview configuration instead of receiving ingest of its own, which is what the
/// model's `InputType` doc says a [`INPUT_TYPE_MULTIVIEW`] channel does ("The channel
/// receives no ingest of its own").
///
/// Takes the input type rather than a stored channel, so the create path — which settles
/// the materialised input type before any channel record exists — decides it from the
/// same place [`Channel::is_multiview`] does.
pub fn is_multiview_input_type(input_type: &str) -> bool {
    input_type == INPUT_TYPE_MULTIVIEW
}

/// The first `InputType` restriction a request breaks, as the modeled
/// `ValidationException` to answer it with, or `None` when every member it carries is
/// allowed on `input_type`.
pub fn channel_input_type_violation(
    submitted: SubmittedChannelMembers,
    input_type: &str,
) -> Option<ValidationException> {
    CHANNEL_INPUT_TYPE_RULES
        .iter()
        .find(|rule| (rule.present)(&submitted) && !rule.allowed.contains(&input_type))
        .map(|rule| {
            validation_failed(
                rule.member,
                &format!(
                    "{} can only be configured when the channel InputType is {}. To use \
                     {}, create a channel with {} input type",
                    rule.label,
                    rule.allowed.join(" or "),
                    rule.label,
                    rule.allowed.join(" or ")
                ),
                rule.exception_type,
            )
        })
}

/// Whether a channel of this input type may carry a restricted member at all.
///
/// `member` marks the ONE member it is asked about on an otherwise empty
/// [`SubmittedChannelMembers`], so the answer comes off the same table a SUBMITTED member
/// is refused by ([`channel_input_type_violation`]). A materialisation that WITHHOLDS a
/// member from a channel whose input type may not carry it decides through here —
/// [`materialized_output_locking_mode`] does, for the one channel member the model
/// documents with both a default and that restriction — so a stored member and a refused
/// one can never disagree about which input types carry it.
pub fn channel_carries_member(member: fn(&mut SubmittedChannelMembers), input_type: &str) -> bool {
    let mut submitted = SubmittedChannelMembers::default();
    member(&mut submitted);
    channel_input_type_violation(submitted, input_type).is_none()
}

/// The member an input type REQUIRES that the request does not carry, as the modeled
/// `ValidationException` to answer it with, or `None` when every member `input_type`
/// requires is there.
///
/// The other direction of [`channel_input_type_violation`], off the SAME rule table: a
/// member whose doc both restricts it to particular input types and calls it required on
/// them is refused when it is MISSING on one of those types, with the
/// `ValidationExceptionType` the model names for that case.
///
/// Only a request that describes a WHOLE channel can be judged this way — the create. On
/// an update, an absent member means "leave what is stored", not "the channel has none",
/// so an update checks the present-but-not-allowed direction alone.
pub fn channel_required_member_violation(
    submitted: SubmittedChannelMembers,
    input_type: &str,
) -> Option<ValidationException> {
    CHANNEL_INPUT_TYPE_RULES
        .iter()
        .find_map(|rule| {
            let exception_type = rule.required_as?;
            let missing = rule.allowed.contains(&input_type) && !(rule.present)(&submitted);
            missing.then_some((rule, exception_type))
        })
        .map(|(rule, exception_type)| {
            validation_failed(
                rule.member,
                &format!(
                    "{} is required when the channel InputType is {}. Specify {}, or use \
                     another input type, and then try again",
                    rule.label,
                    rule.allowed.join(" or "),
                    rule.label
                ),
                exception_type,
            )
        })
}

/// The member path of a channel's output locking mode, as a validation message names it
/// (lowerCamel, as on the wire). Written once: the member is restricted twice, on its
/// PRESENCE by [`CHANNEL_INPUT_TYPE_RULES`] and on its VALUE by
/// [`channel_output_locking_mode_value_violation`], and both refusals path it the same.
const OUTPUT_LOCKING_MODE_MEMBER: &str = "outputLockingMode";

/// The `OutputLockingMode` no channel of a multiview composition may have.
///
/// A multiview channel composites tiles from several channels into one output stream,
/// which only lines up while every stream in the composition carries the deterministic,
/// epoch-aligned segment boundaries the other mode does not: the model's
/// `OutputLockingMode` doc gives `EPOCH_LOCKED` "deterministic sequence numbering and
/// fixed segment boundaries aligned to epoch time" and this value "duration-based segment
/// combining and monotonically increasing sequence numbers starting from 0". The model
/// names the refusal once per side of the composition —
/// `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` for the multiview channel itself
/// ([`channel_output_locking_mode_value_violation`]) and `MULTIVIEW_SOURCE_NON_EPOCH_LOCKED`
/// for a tile source it names ([`multiview_source_violation`]) — so both read this one
/// value and cannot disagree about which mode a composition admits.
pub const MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE: &str = "NON_EPOCH_LOCKED";

/// The refusal for a request that asks for an output locking mode the channel's input
/// type may not have, or `None` when it may.
///
/// The VALUE half of the member's restriction: [`channel_input_type_violation`] decides
/// which input types carry the member at all, and a member an input type DOES carry can
/// still be restricted to some of its values, with its own `ValidationExceptionType` —
/// which is the only kind of refusal `MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED` can
/// name, since a multiview channel carries the member. Decided from the request and the
/// input type alone, so the create settles it before anything is stored; `UpdateChannel`
/// does not bind the member, so no later write can introduce the refused value.
pub fn channel_output_locking_mode_value_violation(
    submitted: Option<&str>,
    input_type: &str,
) -> Option<ValidationException> {
    let refused = submitted == Some(MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE)
        && is_multiview_input_type(input_type);
    refused.then(|| {
        validation_failed(
            OUTPUT_LOCKING_MODE_MEMBER,
            &format!(
                "OutputLockingMode can't be {MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE} when \
                 the channel InputType is {INPUT_TYPE_MULTIVIEW}. {CHECK_THE_VALUE}"
            ),
            VALIDATION_MULTIVIEW_INPUT_TYPE_WITH_NON_EPOCH_LOCKED,
        )
    })
}

/// Which of an origin endpoint's restricted settings a request carries.
///
/// An endpoint setting can be restricted by its PARENT channel's input type rather than by
/// the endpoint's own members, which is why those restrictions are stated here rather than
/// in [`CHANNEL_INPUT_TYPE_RULES`]: they are read against the input type of the channel the
/// endpoint hangs under. Like [`SubmittedChannelMembers`] one level up, this is about the
/// REQUEST rather than about either operation's input shape, so `CreateOriginEndpoint` and
/// `UpdateOriginEndpoint` reduce their own generated inputs to it and the rule table
/// ([`ORIGIN_ENDPOINT_INPUT_TYPE_RULES`]) is written once.
#[derive(Debug, Clone, Copy, Default)]
pub struct SubmittedOriginEndpointMembers {
    pub stream_name_output_mode: bool,
    pub force_endpoint_error_configuration: bool,
    /// Whether the request names the ISM container type: the restriction is on the VALUE
    /// of a required member rather than on the member being present at all.
    pub ism_container_type: bool,
    pub low_latency_hls_manifests: bool,
    pub mss_manifests: bool,
    pub startover_window_seconds: bool,
}

/// An origin-endpoint setting the service only lets the endpoints of particular parent
/// `InputType`s carry. The mirror of [`ChannelInputTypeRule`] one level down, without the
/// required-on-that-input-type direction: no endpoint setting is documented as required by
/// an input type.
struct OriginEndpointInputTypeRule {
    /// The member, as a validation message names it (lowerCamel, as on the wire).
    member: &'static str,
    /// The `InputType` values whose channels may carry it, from the model's `InputType`
    /// enum.
    allowed: &'static [&'static str],
    /// The `ValidationExceptionType` the model declares for this combination.
    exception_type: &'static str,
    /// What the setting is called in the sentence the refusal states. Written to read
    /// inside a sentence, which is where it lands: [`validation_message`] states the
    /// member path ahead of it.
    label: &'static str,
    /// Whether the request carries the setting at all.
    present: fn(&SubmittedOriginEndpointMembers) -> bool,
}

/// The `InputType` values whose channels are NOT the one that composites its video from
/// other channels ([`INPUT_TYPE_MULTIVIEW`]), from the model's `InputType` enum — the
/// input types a setting a multiview channel's endpoint may not carry is allowed under.
/// Written once, so the rows of [`ORIGIN_ENDPOINT_INPUT_TYPE_RULES`] that share that
/// restriction cannot drift apart.
const INPUT_TYPES_EXCEPT_MULTIVIEW: &[&str] = &["HLS", "CMAF"];

/// Every parent-`InputType` restriction the model states for an origin endpoint's
/// settings, one row per `ValidationExceptionType` that names one.
///
/// The rows are read off the model rather than guessed: `StreamNameOutputMode`'s own doc
/// says the setting "is valid only when the associated channel's `InputType` is `HLS`",
/// and `ValidationExceptionType` names one value per remaining combination —
/// `ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION` for the failover
/// settings, and `MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER`,
/// `MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST`, `MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST`
/// and `MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED` for the four an endpoint of a channel that
/// composites its tiles from other channels may not carry — so those four are allowed
/// under [`INPUT_TYPES_EXCEPT_MULTIVIEW`].
///
/// The rows are walked in this order, so the ISM container type — a required member, and
/// what the endpoint IS — is reported before the MSS manifests only an ISM container
/// serves ([`container_manifest_conflict`]).
const ORIGIN_ENDPOINT_INPUT_TYPE_RULES: &[OriginEndpointInputTypeRule] = &[
    OriginEndpointInputTypeRule {
        member: "streamNameOutputMode",
        allowed: &["HLS"],
        exception_type: "ONLY_HLS_INPUT_TYPE_ALLOW_STREAM_NAME_OUTPUT_MODE",
        label: "StreamNameOutputMode",
        present: |submitted| submitted.stream_name_output_mode,
    },
    OriginEndpointInputTypeRule {
        member: "forceEndpointErrorConfiguration",
        allowed: &["CMAF"],
        exception_type: "ONLY_CMAF_INPUT_TYPE_ALLOW_FORCE_ENDPOINT_ERROR_CONFIGURATION",
        label: "ForceEndpointErrorConfiguration",
        present: |submitted| submitted.force_endpoint_error_configuration,
    },
    OriginEndpointInputTypeRule {
        member: "containerType",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER",
        label: "an ISM container type",
        present: |submitted| submitted.ism_container_type,
    },
    OriginEndpointInputTypeRule {
        member: "lowLatencyHlsManifests",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_INPUT_TYPE_WITH_LL_HLS_MANIFEST",
        label: "a low-latency HLS manifest",
        present: |submitted| submitted.low_latency_hls_manifests,
    },
    OriginEndpointInputTypeRule {
        member: "mssManifests",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_INPUT_TYPE_WITH_MSS_MANIFEST",
        label: "an MSS manifest",
        present: |submitted| submitted.mss_manifests,
    },
    OriginEndpointInputTypeRule {
        member: "startoverWindowSeconds",
        allowed: INPUT_TYPES_EXCEPT_MULTIVIEW,
        exception_type: "MULTIVIEW_STARTOVER_WINDOW_NOT_ALLOWED",
        label: "StartoverWindowSeconds",
        present: |submitted| submitted.startover_window_seconds,
    },
];

/// The first parent-`InputType` restriction a request breaks, as the modeled
/// `ValidationException` to answer it with, or `None` when every setting it carries is
/// allowed under `input_type`.
///
/// The sentence is the one every other input-type restriction of this service states
/// ([`channel_input_type_violation`]), off the same table the settings an endpoint is
/// STORED with are materialised from ([`origin_endpoint_carries_member`]). Shared by the
/// create and the update so the two cannot disagree about which channels carry what.
pub fn origin_endpoint_input_type_violation(
    submitted: SubmittedOriginEndpointMembers,
    input_type: &str,
) -> Option<ValidationException> {
    ORIGIN_ENDPOINT_INPUT_TYPE_RULES
        .iter()
        .find(|rule| (rule.present)(&submitted) && !rule.allowed.contains(&input_type))
        .map(|rule| {
            validation_failed(
                rule.member,
                &format!(
                    "{} can only be configured when the channel InputType is {}. To use \
                     {}, create a channel with {} input type",
                    rule.label,
                    rule.allowed.join(" or "),
                    rule.label,
                    rule.allowed.join(" or ")
                ),
                rule.exception_type,
            )
        })
}

/// Whether an endpoint of a channel of this input type may carry a restricted setting at
/// all.
///
/// `member` marks the ONE setting it is asked about on an otherwise empty
/// [`SubmittedOriginEndpointMembers`], so the answer comes off the same table a SUBMITTED
/// setting is refused by ([`origin_endpoint_input_type_violation`]) — exactly as
/// [`channel_carries_member`] does one level up. A materialisation that WITHHOLDS a
/// setting from an endpoint whose parent input type may not carry it decides through here,
/// so a stored setting and a refused one can never disagree.
pub fn origin_endpoint_carries_member(
    member: fn(&mut SubmittedOriginEndpointMembers),
    input_type: &str,
) -> bool {
    let mut submitted = SubmittedOriginEndpointMembers::default();
    member(&mut submitted);
    origin_endpoint_input_type_violation(submitted, input_type).is_none()
}

/// Whether a channel of this input type may carry an origin endpoint's stream-name output
/// mode at all.
pub fn input_type_carries_stream_name_output_mode(input_type: &str) -> bool {
    origin_endpoint_carries_member(
        |members| members.stream_name_output_mode = true,
        input_type,
    )
}

/// Whether an endpoint of a channel of this input type may carry a startover window at
/// all.
pub fn input_type_carries_startover_window(input_type: &str) -> bool {
    origin_endpoint_carries_member(
        |members| members.startover_window_seconds = true,
        input_type,
    )
}

/// The refusal a SUBMITTED stream-name output mode earns under a channel whose input type
/// may not carry the member, or `None` when the request names no mode or the channel may
/// carry one.
///
/// The one row of [`ORIGIN_ENDPOINT_INPUT_TYPE_RULES`] that is about this member, kept as
/// its own entry point because the update reads it beside the member's immutability.
pub fn stream_name_output_mode_input_type_violation(
    submitted: Option<&str>,
    input_type: &str,
) -> Option<ValidationException> {
    origin_endpoint_input_type_violation(
        SubmittedOriginEndpointMembers {
            stream_name_output_mode: submitted.is_some(),
            ..SubmittedOriginEndpointMembers::default()
        },
        input_type,
    )
}

/// The `InputType` a multiview channel's source channels must have, from
/// `MultiviewConfiguration$AvailableSources`' doc ("must have an `InputType` of `CMAF`").
pub const MULTIVIEW_SOURCE_INPUT_TYPE: &str = "CMAF";

/// The member path of a submitted multiview configuration's source list, as a validation
/// message names it (lowerCamel, as on the wire).
const MULTIVIEW_SOURCES_MEMBER: &str = "multiviewConfiguration.availableSources";

/// The member path of a submitted multiview configuration's layout list.
const MULTIVIEW_LAYOUTS_MEMBER: &str = "multiviewConfiguration.availableLayouts";

/// The first entry a list repeats, or `None` when every entry is distinct.
fn first_repeated_entry(entries: &[String]) -> Option<&str> {
    let mut seen: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    entries
        .iter()
        .find(|entry| !seen.insert(entry.as_str()))
        .map(String::as_str)
}

/// The first repetition a submitted multiview configuration carries, as the modeled
/// `ValidationException` to answer it with.
///
/// Each of the two lists names a SET — the sources a player may be given as tiles and the
/// layouts it may request — so "each must appear at most once"
/// (`MultiviewConfiguration$AvailableLayouts`' doc), and each list has its own
/// `ValidationExceptionType` for a repeat. About the request alone, so it is decided
/// before the store is consulted.
pub fn multiview_repetition_violation(
    multiview: &MultiviewConfiguration,
) -> Option<ValidationException> {
    if first_repeated_entry(&multiview.available_sources).is_some() {
        return Some(validation_failed(
            MULTIVIEW_SOURCES_MEMBER,
            &format!(
                "Each source channel can appear at most once in a multiview channel's \
                 available sources. {CHECK_THE_VALUE}"
            ),
            "MULTIVIEW_DUPLICATE_SOURCE",
        ));
    }
    first_repeated_entry(&multiview.available_layouts).map(|_| {
        validation_failed(
            MULTIVIEW_LAYOUTS_MEMBER,
            &format!(
                "Each layout can appear at most once in a multiview channel's available \
                 layouts. {CHECK_THE_VALUE}"
            ),
            "MULTIVIEW_DUPLICATE_LAYOUT",
        )
    })
}

/// The first available source a submitted multiview configuration names that `group`
/// cannot serve as a tile, as the modeled `ValidationException` to answer it with.
///
/// `AvailableSources` names other CHANNELS, so it is resolved against the store the way
/// every other reference to a resource is: a name no channel of the SAME group carries is
/// the model's not-found discriminator, one whose channel is not of the input type a
/// tile source must have is its invalid-input-type one
/// (`MultiviewConfiguration$AvailableSources`: "Each source channel must be in the same
/// channel group as the multiview channel, and must have an `InputType` of `CMAF`"), and
/// one whose channel is locked the way no channel of a composition may be
/// ([`MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE`]) is the source side of that same rule. The
/// sources are walked in the order the request listed them, so the refusal is a function
/// of the request and not of the store's iteration.
pub fn multiview_source_violation(
    group: &ChannelGroup,
    multiview: &MultiviewConfiguration,
) -> Option<ValidationException> {
    for source in &multiview.available_sources {
        let Some(channel) = group.channels.get(source) else {
            return Some(validation_failed(
                MULTIVIEW_SOURCES_MEMBER,
                &format!(
                    "Each source channel of a multiview channel must be a channel of the \
                     same channel group. {CHECK_THE_VALUE}"
                ),
                "MULTIVIEW_SOURCE_NOT_FOUND",
            ));
        };
        if channel.input_type != MULTIVIEW_SOURCE_INPUT_TYPE {
            return Some(validation_failed(
                MULTIVIEW_SOURCES_MEMBER,
                &format!(
                    "Each source channel of a multiview channel must have an InputType of \
                     {MULTIVIEW_SOURCE_INPUT_TYPE}. {CHECK_THE_VALUE}"
                ),
                "MULTIVIEW_SOURCE_INVALID_INPUT_TYPE",
            ));
        }
        // The other side of the composition's one locking rule: a tile source whose own
        // output locking mode is the one no channel of a composition may have cannot be
        // composited, which is the refusal the model names for a source.
        if channel.output_locking_mode.as_deref() == Some(MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE) {
            return Some(validation_failed(
                MULTIVIEW_SOURCES_MEMBER,
                &format!(
                    "A source channel of a multiview channel can't have an \
                     OutputLockingMode of {MULTIVIEW_REFUSED_OUTPUT_LOCKING_MODE}. \
                     {CHECK_THE_VALUE}"
                ),
                VALIDATION_MULTIVIEW_SOURCE_NON_EPOCH_LOCKED,
            ));
        }
    }
    None
}

/// Why a pagination token this service did not issue is refused — the detail
/// [`validation_message`] states after the member path. The same sentence whichever list
/// is being paged, and about the token rather than about the caller's data.
pub const INVALID_PAGINATION_TOKEN_DETAIL: &str = "You've provided an invalid pagination \
     token in your request. Check your token and then try again";

/// The member a paged list carries its continuation token in, as the wire names it
/// (`httpQuery("nextToken")` on every list's `NextToken`) and therefore as a validation
/// message paths it.
const NEXT_TOKEN_MEMBER: &str = "nextToken";

/// The modeled 400 a paged list refuses a token this service did not issue with.
///
/// Every paged list answers this one class, so the member path, the detail and the
/// discriminator are assembled here once rather than per list: four lists refusing one
/// class cannot then word it four ways.
pub fn invalid_pagination_token() -> ValidationException {
    validation_failed(
        NEXT_TOKEN_MEMBER,
        INVALID_PAGINATION_TOKEN_DETAIL,
        VALIDATION_INVALID_PAGINATION_TOKEN,
    )
}

// =============================================================================
// Validation: which manifests an origin endpoint's container type can serve
// =============================================================================

/// One of the four manifest families an origin endpoint's request members carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestFamily {
    Hls,
    LowLatencyHls,
    Dash,
    Mss,
}

impl ManifestFamily {
    /// Every family, in the order the request declares their members.
    pub const ALL: [Self; 4] = [Self::Hls, Self::LowLatencyHls, Self::Dash, Self::Mss];

    /// The request member that carries this family, as a validation message names it.
    pub fn member(self) -> &'static str {
        match self {
            Self::Hls => "hlsManifests",
            Self::LowLatencyHls => "lowLatencyHlsManifests",
            Self::Dash => "dashManifests",
            Self::Mss => "mssManifests",
        }
    }

    /// How a validation message names the family.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hls => "HLS",
            Self::LowLatencyHls => "low-latency HLS",
            Self::Dash => "DASH",
            Self::Mss => "MSS",
        }
    }

    /// The container format this family's manifests are published in — the suffix their
    /// egress URL ends in. The HLS families share it: both publish an `.m3u8` playlist.
    pub fn kind(self) -> ManifestKind {
        match self {
            Self::Hls | Self::LowLatencyHls => ManifestKind::Hls,
            Self::Dash => ManifestKind::Dash,
            Self::Mss => ManifestKind::Mss,
        }
    }
}

/// How many manifests a request carries per family, in [`ManifestFamily::ALL`] order —
/// the array [`manifest_set_violation`] reads.
///
/// Declared here, beside `ManifestFamily::ALL`, because the array's POSITIONS are what
/// carry the family: the counts are placed by mapping over `ALL` itself, so the order of
/// the array can only ever be the order that enum declares, and a family added to it is a
/// non-exhaustive `match` here rather than a count silently read as another family's.
/// `CreateOriginEndpoint` and `UpdateOriginEndpoint` both write the whole manifest set and
/// both count it through here. A family the request left out carries no manifest, which is
/// the same count as an empty list.
pub fn manifest_counts<Hls, LowLatencyHls, Dash, Mss>(
    hls_manifests: Option<&[Hls]>,
    low_latency_hls_manifests: Option<&[LowLatencyHls]>,
    dash_manifests: Option<&[Dash]>,
    mss_manifests: Option<&[Mss]>,
) -> [usize; ManifestFamily::ALL.len()] {
    ManifestFamily::ALL.map(|family| match family {
        ManifestFamily::Hls => hls_manifests.map_or(0, <[Hls]>::len),
        ManifestFamily::LowLatencyHls => {
            low_latency_hls_manifests.map_or(0, <[LowLatencyHls]>::len)
        }
        ManifestFamily::Dash => dash_manifests.map_or(0, <[Dash]>::len),
        ManifestFamily::Mss => mss_manifests.map_or(0, <[Mss]>::len),
    })
}

/// The manifest names a request carries, in [`ManifestFamily::ALL`] order and, within a
/// family, in the order the request listed them — what [`manifest_name_collision`] reads.
///
/// Declared beside [`manifest_counts`] and built the same way, by mapping over `ALL`
/// itself, so the positions of the array can only ever be the order that enum declares.
fn submitted_manifest_names(
    hls_manifests: Option<&[CreateHlsManifestConfiguration]>,
    low_latency_hls_manifests: Option<&[CreateLowLatencyHlsManifestConfiguration]>,
    dash_manifests: Option<&[CreateDashManifestConfiguration]>,
    mss_manifests: Option<&[CreateMssManifestConfiguration]>,
) -> [Vec<String>; ManifestFamily::ALL.len()] {
    ManifestFamily::ALL.map(|family| match family {
        ManifestFamily::Hls => hls_manifests.map_or_else(Vec::new, |manifests| {
            manifests
                .iter()
                .map(|manifest| manifest.manifest_name.clone())
                .collect()
        }),
        ManifestFamily::LowLatencyHls => {
            low_latency_hls_manifests.map_or_else(Vec::new, |manifests| {
                manifests
                    .iter()
                    .map(|manifest| manifest.manifest_name.clone())
                    .collect()
            })
        }
        ManifestFamily::Dash => dash_manifests.map_or_else(Vec::new, |manifests| {
            manifests
                .iter()
                .map(|manifest| manifest.manifest_name.clone())
                .collect()
        }),
        ManifestFamily::Mss => mss_manifests.map_or_else(Vec::new, |manifests| {
            manifests
                .iter()
                .map(|manifest| manifest.manifest_name.clone())
                .collect()
        }),
    })
}

/// The `ValidationExceptionType` a manifest set that uses one name twice carries, from the
/// model's `ValidationExceptionType` enum.
pub const VALIDATION_MANIFEST_NAME_COLLISION: &str = "MANIFEST_NAME_COLLISION";

/// The second manifest of a submitted set that would be published at a URL another
/// manifest of the same set is already published at, as the modeled `ValidationException`
/// to answer it with, or `None` when every manifest the request carries has an egress URL
/// of its own.
///
/// A manifest name is "a short string that's appended to the endpoint URL" that "creates a
/// unique path to this endpoint", so it is what makes the URL this service computes and
/// persists for a manifest ([`manifest_url`], through [`stored_hls_manifest`] and its
/// siblings) identify THAT manifest. Two manifests collide exactly when that URL is the
/// same one, which is when they share a name AND a container format
/// ([`ManifestFamily::kind`]) — so the namespace a name is taken from is the endpoint's,
/// not one family's: the HLS and low-latency HLS families both publish an `.m3u8`
/// playlist, which is why the model documents that you "can't use the same manifest name
/// if you use HLS manifest and low-latency HLS manifest"
/// (`CreateHlsManifestConfiguration$ManifestName`) and names
/// `MANIFEST_NAME_COLLISION` for it. The set is walked in [`ManifestFamily::ALL`] order,
/// so the refusal is a function of the request and not of which family the service happens
/// to read first. About the request alone, and the same rule for `CreateOriginEndpoint` and
/// `UpdateOriginEndpoint`, which both write the endpoint's whole manifest set.
pub fn manifest_name_collision(
    hls_manifests: Option<&[CreateHlsManifestConfiguration]>,
    low_latency_hls_manifests: Option<&[CreateLowLatencyHlsManifestConfiguration]>,
    dash_manifests: Option<&[CreateDashManifestConfiguration]>,
    mss_manifests: Option<&[CreateMssManifestConfiguration]>,
) -> Option<ValidationException> {
    let names = submitted_manifest_names(
        hls_manifests,
        low_latency_hls_manifests,
        dash_manifests,
        mss_manifests,
    );
    let mut taken: BTreeSet<(ManifestKind, &str)> = BTreeSet::new();
    for (family, names) in ManifestFamily::ALL.into_iter().zip(names.iter()) {
        for (index, name) in names.iter().enumerate() {
            if taken.insert((family.kind(), name.as_str())) {
                continue;
            }
            return Some(validation_failed(
                &format!("{}.manifestName", element_path(family.member(), index)),
                &format!(
                    "Each manifest of an origin endpoint must have a manifest name no \
                     other manifest published in the same format uses. {CHECK_THE_VALUE}"
                ),
                VALIDATION_MANIFEST_NAME_COLLISION,
            ));
        }
    }
    None
}

/// The `ContainerType` whose endpoints package Microsoft Smooth Streaming, from the
/// model's `ContainerType` enum.
///
/// The one container the model singles out by name in its `ValidationExceptionType`
/// values (`ISM_CONTAINER_WITH_KEY_ROTATION`,
/// `ENCRYPTION_CONTRACT_WITH_ISM_CONTAINER_INCOMPATIBLE`,
/// `MULTIVIEW_INPUT_TYPE_WITH_ISM_CONTAINER`), so the value is written once and the rules
/// that read it cannot drift.
pub const CONTAINER_TYPE_ISM: &str = "ISM";

/// Whether this container type is the Microsoft Smooth Streaming one.
pub fn is_ism_container(container: &str) -> bool {
    container == CONTAINER_TYPE_ISM
}

/// The `ValidationExceptionType` that names the refusal of `family` on a `container`
/// endpoint, or `None` when that container serves that family.
///
/// The combinations are exactly the ones the model's `ValidationExceptionType` enum
/// names — `TS_CONTAINER_TYPE_WITH_DASH_MANIFEST`, `TS_CONTAINER_TYPE_WITH_MSS_MANIFEST`,
/// `CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST`, `ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST`,
/// `ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST`, `ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST` —
/// so the table is read off the model rather than guessed: a TS container serves the HLS
/// families, CMAF adds DASH, and ISM serves MSS alone. A container value the model does
/// not name constrains nothing here; the boundary validator has already refused it
/// against the `ContainerType` enum.
pub fn container_manifest_conflict(
    container: &str,
    family: ManifestFamily,
) -> Option<&'static str> {
    match (container, family) {
        ("TS", ManifestFamily::Dash) => Some("TS_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        ("TS", ManifestFamily::Mss) => Some("TS_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        ("CMAF", ManifestFamily::Mss) => Some("CMAF_CONTAINER_TYPE_WITH_MSS_MANIFEST"),
        ("ISM", ManifestFamily::Hls) => Some("ISM_CONTAINER_TYPE_WITH_HLS_MANIFEST"),
        ("ISM", ManifestFamily::LowLatencyHls) => {
            Some("ISM_CONTAINER_TYPE_WITH_LL_HLS_MANIFEST")
        }
        ("ISM", ManifestFamily::Dash) => Some("ISM_CONTAINER_TYPE_WITH_DASH_MANIFEST"),
        _ => None,
    }
}

/// A refused manifest set, as the `(message, ValidationExceptionType)` pair the
/// operation puts on its `ValidationException`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestSetViolation {
    pub message: String,
    pub validation_exception_type: &'static str,
}

/// Which operation is checking a manifest set — the two differ only in what a
/// manifest-count sentence tells the caller to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestSetOn {
    Create,
    Update,
}

/// Check the manifest set an origin endpoint is being created or updated with: its total
/// count against both ends of the permitted range, and each present family against the
/// container type.
///
/// `counts` is how many manifests the request carries per family, in
/// [`ManifestFamily::ALL`] order, as [`manifest_counts`] — the one builder of that array —
/// emits it. `None` means the set is acceptable. This is the same
/// rule for `CreateOriginEndpoint` and `UpdateOriginEndpoint`: both carry the whole
/// manifest set and the same required `ContainerType`, so an endpoint's stored manifest
/// count stays inside the range whichever operation last wrote it.
///
/// A count outside the range is an input failure, reported on `ValidationException` with
/// the discriminator for the end that was crossed — `NUM_MANIFESTS_LOW` below
/// [`MIN_MANIFESTS_PER_ORIGIN_ENDPOINT`], `NUM_MANIFESTS_HIGH` above
/// [`MAX_MANIFESTS_PER_ORIGIN_ENDPOINT`] — not a quota refusal.
pub fn manifest_set_violation(
    container: &str,
    counts: [usize; ManifestFamily::ALL.len()],
    on: ManifestSetOn,
) -> Option<ManifestSetViolation> {
    let retry = match on {
        ManifestSetOn::Create => "create",
        ManifestSetOn::Update => "update",
    };
    let total: usize = counts.iter().sum();
    if total < MIN_MANIFESTS_PER_ORIGIN_ENDPOINT {
        return Some(ManifestSetViolation {
            message: validation_message(
                None,
                &format!(
                    "You've requested an origin endpoint without a manifest. Your origin \
                     endpoint needs to contain at least one manifest. Add a manifest and \
                     try to {retry} your origin endpoint again"
                ),
            ),
            validation_exception_type: "NUM_MANIFESTS_LOW",
        });
    }
    if total > MAX_MANIFESTS_PER_ORIGIN_ENDPOINT {
        return Some(ManifestSetViolation {
            message: validation_message(
                None,
                &format!(
                    "You've requested an origin endpoint with {total} manifests. Your \
                     origin endpoint can contain at most \
                     {MAX_MANIFESTS_PER_ORIGIN_ENDPOINT} manifests. Remove a manifest and \
                     try to {retry} your origin endpoint again"
                ),
            ),
            validation_exception_type: "NUM_MANIFESTS_HIGH",
        });
    }
    for (family, count) in ManifestFamily::ALL.into_iter().zip(counts) {
        if count == 0 {
            continue;
        }
        if let Some(exception_type) = container_manifest_conflict(container, family) {
            return Some(ManifestSetViolation {
                // About the RELATIONSHIP between `ContainerType` and the manifest list,
                // both of them the request's own members, so the refusal is stated at the
                // request itself rather than at either member — the rule
                // [`cross_member_validation_failed`] states for every cross-member
                // refusal this service makes, which for the request's own members is the
                // bare request path [`validation_message`] renders for `None`.
                message: validation_message(
                    None,
                    &container_manifest_requirement(container, family),
                ),
                validation_exception_type: exception_type,
            });
        }
    }
    None
}

/// The requirement a manifest family its container type cannot serve failed: that family
/// is not served by an endpoint of that container type.
///
/// It names the family and the container type, which are the two things the caller has to
/// reconcile, and nothing else from the request. The sentence ends in a space, which is
/// part of the text this class is reported with.
fn container_manifest_requirement(container: &str, family: ManifestFamily) -> String {
    format!(
        "{} manifest is not supported on endpoint with {container} container type. ",
        family.label()
    )
}

/// The ETag of an origin endpoint: a digest over everything the endpoint is made of, so
/// an update that changes any of it rotates the tag and one that changes none keeps it.
///
/// `CreateOriginEndpoint` and `UpdateOriginEndpoint` both build it here, so the tag a
/// create reports and the tag an update rotates are the same function of the same
/// resource.
pub fn origin_endpoint_etag(endpoint: &OriginEndpoint) -> String {
    let manifest_names = |names: Vec<&str>| names.join("\u{1f}");
    etag(&[
        endpoint.arn.clone(),
        endpoint.container_type.clone(),
        endpoint.description.clone().unwrap_or_default(),
        // Rendered so that an ABSENT window and a present one stay apart: an endpoint of a
        // channel that may not carry the member has none at all.
        format!("{:?}", endpoint.startover_window_seconds),
        format!("{:?}", endpoint.segment),
        manifest_names(
            endpoint
                .hls_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        manifest_names(
            endpoint
                .low_latency_hls_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        manifest_names(
            endpoint
                .dash_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        manifest_names(
            endpoint
                .mss_manifests
                .iter()
                .map(|m| m.manifest_name.as_str())
                .collect(),
        ),
        format!("{:?}", endpoint.force_endpoint_error_configuration),
        endpoint.uri_separator.clone(),
        // Rendered so that an ABSENT mode and a present one stay apart: an endpoint of a
        // channel that may not carry the member has none at all.
        format!("{:?}", endpoint.stream_name_output_mode),
        tag_fingerprint(&endpoint.tags),
        // The commit count, so a commit that writes the same values still rotates the
        // tag — which is what a conditional update that lost a race needs. See
        // [`ChannelGroup::version`].
        endpoint.version.to_string(),
    ])
}

// =============================================================================
// Resource policies: the shared document check and the wording around it
// =============================================================================

/// The `ValidationExceptionType` a policy document this service cannot accept carries,
/// from the model's `ValidationExceptionType` enum.
pub const VALIDATION_INVALID_POLICY: &str = "INVALID_POLICY";

/// Why a policy document this service cannot accept was refused — the requirement the
/// `Policy` member failed, stated in the service's own envelope by
/// [`validation_failed`].
///
/// The whole sentence is fixed text for the `INVALID_POLICY` class: it states the
/// requirement (the document has to be a valid IAM policy) and tells the caller to look at
/// what it sent, and it interpolates nothing from the document that was submitted — a
/// caller's policy text is never echoed back. Shared by the policy writers
/// (`PutChannelPolicy` and `PutOriginEndpointPolicy`) so one rejected document cannot be
/// worded two ways.
pub const INVALID_POLICY_REQUIREMENT: &str = "The policy you entered isn't valid. The \
    policy must be a valid IAM policy. Check your policy and then try again";

/// The member path a refused policy document is reported at: the `Policy` member of both
/// policy writers, in lowerCamel as the wire names it.
pub const POLICY_MEMBER: &str = "policy";

/// The modelled 400 both policy writers refuse a document they cannot accept with.
///
/// One constructor for the `INVALID_POLICY` class, so the two writers answer the same
/// member path, the same requirement and the same discriminator.
pub fn invalid_policy() -> ValidationException {
    validation_failed(
        POLICY_MEMBER,
        INVALID_POLICY_REQUIREMENT,
        VALIDATION_INVALID_POLICY,
    )
}

/// Whether `policy` is a policy document this service accepts.
///
/// A resource policy is an IAM policy document: a JSON OBJECT carrying a `Statement`.
/// Anything else — text that is not JSON, a JSON array or scalar, an object with no
/// `Statement` — is the model's `INVALID_POLICY` validation case, which is why the
/// service's `ValidationExceptionType` enum names it. Shared by the policy writers
/// (`PutChannelPolicy` and `PutOriginEndpointPolicy`) so both decide validity the same
/// way.
pub fn is_policy_document(policy: &str) -> bool {
    let Ok(::serde_json::Value::Object(document)) = ::serde_json::from_str(policy) else {
        return false;
    };
    document.contains_key("Statement")
}

// =============================================================================
// Caller-supplied ARNs: what a member documented as another service's resource names
// =============================================================================

/// What an input member this model DOCUMENTS as the ARN of another AWS resource has to
/// name.
///
/// Every such member is a plain string whose model states a `@length` and nothing else
/// (`smithy_query(query="shape",shape="CdnAuthConfiguration")`, `shape="SpekeKeyProvider"`),
/// so the boundary validator admits any text of the right size and reading the ARN form is
/// the handler's job. It HAS to be read: the service assumes the role and reads the secret
/// those members name, so a value that is not an ARN of the documented service and resource
/// type names neither and the request is an input failure rather than a configuration that
/// happens not to work.
///
/// The type and the constants are the SHARED ones ([`::service_common::arn::Requirement`]),
/// so this service and every other that binds a caller-supplied resource ARN read it
/// against one requirement rather than against a copy each. What stays here is which of
/// THIS model's members are ARNs ([`cdn_auth_rejection`], [`speke_role_arn_rejection`]) and
/// the envelope its refusal travels in.
pub type ArnRequirement = ::service_common::arn::Requirement;

/// A secret in AWS Secrets Manager: `CdnAuthConfiguration$CdnIdentifierSecretArns` is
/// "The ARN for the secret in Secrets Manager that your CDN uses for authorization to
/// access the endpoint".
pub const SECRETS_MANAGER_SECRET_ARN: ArnRequirement =
    ::service_common::arn::SECRETS_MANAGER_SECRET;

/// An IAM role: `CdnAuthConfiguration$SecretsRoleArn` is "The ARN for the IAM role that
/// gives MediaPackage read access to Secrets Manager and KMS for CDN authorization", and
/// `SpekeKeyProvider$RoleArn` "The ARN for the IAM role granted by the key provider that
/// provides access to the key provider API" — whose doc also states the form,
/// `arn:aws:iam::{accountID}:role/{name}`.
pub const IAM_ROLE_ARN: ArnRequirement = ::service_common::arn::IAM_ROLE;

/// `ValidationExceptionType` values a refused caller-supplied ARN carries, from the
/// model's `ValidationExceptionType` enum.
///
/// The enum names one value per way such a value can be wrong, which is what a caller
/// branches on: the ARN's FORM ([`VALIDATION_MALFORMED_SECRET_ARN`],
/// [`VALIDATION_ROLE_ARN_INVALID_FORMAT`]), the scope it names
/// ([`VALIDATION_SECRET_FROM_DIFFERENT_REGION`],
/// [`VALIDATION_SECRET_FROM_DIFFERENT_ACCOUNT`]) and a list that names one secret twice
/// ([`VALIDATION_DUPLICATED_SECRET`]).
pub const VALIDATION_MALFORMED_SECRET_ARN: &str = "MALFORMED_SECRET_ARN";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_SECRET_FROM_DIFFERENT_REGION: &str = "SECRET_FROM_DIFFERENT_REGION";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_SECRET_FROM_DIFFERENT_ACCOUNT: &str = "SECRET_FROM_DIFFERENT_ACCOUNT";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_DUPLICATED_SECRET: &str = "DUPLICATED_SECRET";
/// See [`VALIDATION_MALFORMED_SECRET_ARN`].
pub const VALIDATION_ROLE_ARN_INVALID_FORMAT: &str = "ROLE_ARN_INVALID_FORMAT";

/// The requirement an ARN member that names the wrong KIND of thing failed, worded the way
/// this service words every other requirement: the rule about the member, then the sentence
/// telling the caller to look at what it sent (see [`constraint_detail`]).
fn arn_requirement_detail(requirement: ArnRequirement) -> String {
    format!("Member {}. {CHECK_THE_VALUE}", requirement.phrase())
}

/// The requirement a well-formed ARN naming another `scope` (a Region, an account) failed.
fn arn_scope_detail(scope: &str, expected: &str) -> String {
    format!("Member must name a resource in {scope} {expected}. {CHECK_THE_VALUE}")
}

/// The member path a list element is at, spelled as the framework's own constraint walk
/// spells it: the list's path, the element's 1-BASED position, then the collection step
/// (`aws_smithy_schema_server::constraints::path`). One spelling, so a refusal from a
/// handler names a position the same way a refusal from the boundary does.
fn element_path(list_path: &str, index: usize) -> String {
    format!("{list_path}.{}.member", index + 1)
}

/// The collection a pagination token was issued for.
///
/// A token names a position in ONE collection, so what it resumes has to be read against
/// the collection the request is paging: every list here walks a name-ordered map of the
/// same shape, so without that binding a cursor issued for one of them is a
/// plausible-looking cursor in each of the others. The variants are the collections the
/// model declares a paged list over, each carrying the names that identify it — the
/// Region included, since the Region is part of every resource's identity here.
#[derive(Debug, Clone, Copy)]
pub enum PageScope<'a> {
    /// The channel groups one Region holds.
    ChannelGroups { region: &'a str },
    /// The channels of one channel group.
    Channels {
        region: &'a str,
        channel_group: &'a str,
    },
    /// The origin endpoints of one channel.
    OriginEndpoints {
        region: &'a str,
        channel_group: &'a str,
        channel: &'a str,
    },
    /// The harvest jobs of one channel group: the cursor keys that group's whole job
    /// order, and the request's channel, endpoint and status members narrow what that
    /// order reports rather than naming a collection of their own.
    HarvestJobs {
        region: &'a str,
        channel_group: &'a str,
    },
}

impl PageScope<'_> {
    /// What identifies this collection: the level it is at, then the names of the
    /// resources above it.
    fn parts(&self) -> Vec<&str> {
        match self {
            Self::ChannelGroups { region } => vec!["channelGroups", region],
            Self::Channels {
                region,
                channel_group,
            } => vec!["channels", region, channel_group],
            Self::OriginEndpoints {
                region,
                channel_group,
                channel,
            } => vec!["originEndpoints", region, channel_group, channel],
            Self::HarvestJobs {
                region,
                channel_group,
            } => vec!["harvestJobs", region, channel_group],
        }
    }
}

/// How many hex characters of the binding digest a pagination token opens with.
const PAGE_TOKEN_DIGEST_LEN: usize = 24;

/// The digest that binds a token to the collection it walks AND to the position it names.
///
/// [`short_digest`] length-prefixes each part, so no two collections — a Region's groups
/// and a group's channels, two groups of the same name in two Regions — share a digest,
/// and no edit of the cursor keeps one: the token reads only as the whole value a list
/// handed out. The digest is not a secret and is not an authentication of the caller —
/// what it decides is whether a token names a page of THIS collection at all.
fn page_token_digest(scope: PageScope<'_>, last: &str) -> String {
    let mut parts = scope.parts();
    parts.push(last);
    short_digest(&parts, PAGE_TOKEN_DIGEST_LEN)
}

/// Whether `text` is the lowercase hexadecimal this service's tokens are written in.
fn is_lowercase_hex(text: &str) -> bool {
    text.bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

/// The pagination token a list of `scope` that stopped after `last` issues — opaque to
/// the caller, and decoded by [`page_resume_after`].
///
/// The token is that binding digest followed by the hex of the name the page stopped
/// after, so it is self-describing: resuming it needs no server-side cursor record, and
/// a reporting operation never has to write one to serve a page.
pub fn page_token(scope: PageScope<'_>, last: &str) -> String {
    format!(
        "{}{}",
        page_token_digest(scope, last),
        hex::encode(last.as_bytes())
    )
}

/// The name a pagination token resumes after, or `None` when the token is not one this
/// service issued FOR `scope`.
///
/// The strict inverse of [`page_token`]: the token has to carry the lowercase hex of a
/// non-empty name behind the digest that name has for `scope`. Every token this service
/// issues for this collection satisfies that, so anything else — a cursor another of this
/// service's lists issued, a hand-written hex run, a token whose digest or cursor was
/// edited or truncated — resumes nothing, and the list answers the modelled
/// [`invalid_pagination_token`] refusal instead of seeking to a position that was never a
/// page boundary of this collection.
pub fn page_resume_after(scope: PageScope<'_>, token: &str) -> Option<String> {
    let digest = token.get(..PAGE_TOKEN_DIGEST_LEN)?;
    let cursor = token.get(PAGE_TOKEN_DIGEST_LEN..)?;
    if !is_lowercase_hex(cursor) {
        return None;
    }
    let name = String::from_utf8(hex::decode(cursor).ok()?).ok()?;
    if name.is_empty() || digest != page_token_digest(scope, &name) {
        return None;
    }
    Some(name)
}

/// The smallest `MaxResults` the model's `ListResourceMaxResults` range allows.
const MAX_RESULTS_MIN: usize = 1;
/// The largest `MaxResults` the model's `ListResourceMaxResults` range allows.
const MAX_RESULTS_MAX: usize = 100;
/// The page size the model declares as `MaxResults`'s default.
const MAX_RESULTS_DEFAULT: usize = 10;

/// The page size a list serves, from the request's `MaxResults`.
///
/// The model bounds `MaxResults` to 1..=100 and defaults it to 10, and the request
/// boundary has already refused anything outside that range, so a value outside it here
/// means the member was absent: fall back to the modelled default. Every paged list in
/// this service resolves its page size here, so a caller cannot force a larger response
/// from one list than from another.
pub fn page_size(max_results: i32) -> usize {
    match usize::try_from(max_results) {
        Ok(size) if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&size) => size,
        _ => MAX_RESULTS_DEFAULT,
    }
}

/// One page of a name-keyed collection: the records this page reports, and the token the
/// next page resumes from (`None` once nothing is left).
///
/// `records` is a name-ordered map's iterator, `scope` the collection being walked (what
/// the issued token is bound to, see [`PageScope`]), `resume_after` the name the previous
/// page stopped at (from [`page_resume_after`]) and `size` the resolved [`page_size`].
/// This page starts strictly beyond `resume_after`, so no record is returned twice and
/// none is skipped, and a token is issued only when a record is actually left over.
/// Shared by every paged list so the two cannot drift.
pub fn page<'a, T>(
    records: impl Iterator<Item = (&'a String, &'a T)>,
    scope: PageScope<'_>,
    resume_after: Option<&str>,
    size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let mut remaining = records
        .filter(|(name, _)| resume_after.is_none_or(|after| name.as_str() > after));
    let page: Vec<(&String, &T)> = remaining.by_ref().take(size).collect();
    let next_token = page
        .last()
        .filter(|_| remaining.next().is_some())
        .map(|(last, _)| page_token(scope, last));
    (page.into_iter().map(|(_, record)| record).collect(), next_token)
}

/// The first `len` hex characters of the digest of `parts` — a short, stable label for
/// a name that must look opaque to a client but has to be reproducible.
fn short_digest(parts: &[&str], len: usize) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part.len().to_le_bytes());
        hasher.update(part.as_bytes());
    }
    let digest = hex::encode(hasher.finalize());
    digest[..len].to_string()
}

#[cfg(test)]
mod tests;
mod data_plane_addressing;
pub use data_plane_addressing::*;
mod refusals;
pub use refusals::*;
