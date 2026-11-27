// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime service.
//!
//! The stage lifecycle (`CreateStage`, `GetStage`, `ListStages`, `UpdateStage`,
//! `DeleteStage`), the token issuance that makes a stage usable
//! (`CreateParticipantToken`), the stage sessions a stage's participants are
//! grouped into (`GetParticipant`, `ListParticipants`), the occurrences during one of
//! those sessions (`ListParticipantEvents`), the compositions a stage is composed
//! into (`StartComposition`, `GetComposition`, `StopComposition`) and the public keys a
//! customer signs its own participant tokens with (`ImportPublicKey`, `GetPublicKey`,
//! `ListPublicKeys`) all
//! read and write the records below, so the records, the id minting they share and the
//! modelled defaults they apply live here rather than in any one operation's file.
//!
//! The module is split by concern: the stage sessions and the participants, events and
//! replications inside them in [`sessions`], the addresses every stage this service
//! reports announces in [`stage_endpoints`], the socket an encoder publishes a stage's
//! contribution to and what an accepted publish writes in [`ingest_plane`], the pagination
//! tokens every listing pages with in [`page_tokens`], and how this service words and
//! classifies the failures it answers in [`refusals`]. Each part is re-exported here, so a
//! handler reaches all of it as `crate::state::…`.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait};
use smithy_server_runtime::{Violation, ViolationKind};

use crate::ops::types::{
    AutoParticipantRecordingConfiguration, Composition, CompositionRecordingHlsConfiguration,
    CompositionSummary, CompositionThumbnailConfiguration, ConflictException,
    CreateIngestConfigurationOutput, Destination, DestinationConfiguration, DestinationDetail,
    DestinationSummary, EncoderConfiguration, EncoderConfigurationSummary, Event,
    GridConfiguration, IngestConfiguration,
    IngestConfigurationSummary, InternalServerException, LayoutConfiguration, Participant,
    ParticipantRecordingHlsConfiguration, ParticipantReplica, ParticipantSummary,
    ParticipantThumbnailConfiguration, ParticipantToken, PipConfiguration, PublicKey,
    PublicKeySummary, RecordingConfiguration, RedundantIngestCredential,
    ResourceNotFoundException,
    S3DestinationConfiguration, S3StorageConfiguration,
    ServiceQuotaExceededException, Stage, StageSession, StageSessionSummary,
    StageSummary,
    StartParticipantReplicationOutput, StopParticipantReplicationOutput, StorageConfiguration,
    StorageConfigurationSummary, UpdateIngestConfigurationOutput, ValidationException, Video,
};
use crate::scaffolded::Scaffolded;

/// Lifetime a participant token gets when the request omits `duration`.
///
/// The model documents it on `CreateParticipantTokenRequest$duration` and on
/// `ParticipantToken$duration`: "Duration (in minutes), after which the token
/// expires. Default: 720 (12 hours)."
pub const DEFAULT_TOKEN_DURATION_MINUTES: i32 = 720;

/// Capabilities a participant token gets when the request omits `capabilities`.
///
/// The model documents it on `CreateParticipantTokenRequest$capabilities`: "Set of
/// capabilities that the user is allowed to perform in the stage. Default: `PUBLISH,
/// SUBSCRIBE`."
pub const DEFAULT_TOKEN_CAPABILITIES: [&str; 2] = ["PUBLISH", "SUBSCRIBE"];

/// [`DEFAULT_TOKEN_CAPABILITIES`] as the modelled `ParticipantTokenCapabilities` list,
/// so both operations that issue a token apply the documented default from one place.
pub fn default_token_capabilities() -> Vec<String> {
    DEFAULT_TOKEN_CAPABILITIES
        .iter()
        .map(|capability| (*capability).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// The modelled defaults of a stage's individual-participant-recording
// configuration
//
// `AutoParticipantRecordingConfiguration` is reported for every stage, so each member
// the caller left out is reported with the default the model documents for it. Every
// constant below cites the member doc it comes from; none is this emulator's choice.
// ---------------------------------------------------------------------------

/// `storageConfigurationArn` of a stage with no storage configuration attached.
///
/// Documented on `AutoParticipantRecordingConfiguration$storageConfigurationArn`:
/// "Default: `\"\"` (empty string, no storage configuration is specified)."
pub const NO_STORAGE_CONFIGURATION_ARN: &str = "";

/// Media types recorded when the configuration names none.
///
/// Documented on `AutoParticipantRecordingConfiguration$mediaTypes`: "Types of media to
/// be recorded. Default: `AUDIO_VIDEO`."
pub const DEFAULT_RECORDING_MEDIA_TYPE: &str = "AUDIO_VIDEO";

/// Window in which a publisher's reconnect is merged into one recording, when the
/// configuration does not size it.
///
/// Documented on `AutoParticipantRecordingConfiguration$recordingReconnectWindowSeconds`:
/// "The default value is 0, which disables merging."
pub const DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS: i32 = 0;

/// Whether replica participants are recorded, when the configuration does not say.
///
/// Documented on `AutoParticipantRecordingConfiguration$recordParticipantReplicas`:
/// "Optional field to disable replica participant recording. … Default: `true`."
///
/// The model declares `@default false` for the same member, so the shape keeps the
/// member's PRESENCE (see the hand-written `AutoParticipantRecordingConfiguration` in
/// `crate::ops::types`) and this default applies only where the request carried no
/// value: a `false` a caller sends to disable replica participant recording is reported
/// as it arrived.
pub const DEFAULT_RECORD_PARTICIPANT_REPLICAS: bool = true;

/// Thumbnail-generation interval, when the thumbnail configuration does not set one.
///
/// Documented on `ParticipantThumbnailConfiguration$targetIntervalSeconds`: "The targeted
/// thumbnail-generation interval in seconds. … Default: 60." The thumbnail configuration
/// of a composition destination states the same value for the same member —
/// `CompositionThumbnailConfiguration$targetIntervalSeconds`: "The targeted
/// thumbnail-generation interval in seconds. Default: 60." — so both are reported from
/// here.
pub const DEFAULT_THUMBNAIL_INTERVAL_SECONDS: i32 = 60;

/// Thumbnail storage format, when the thumbnail configuration names none.
///
/// Documented on `ParticipantThumbnailConfiguration$storage`: "Indicates the format in
/// which thumbnails are recorded. … Default: `SEQUENTIAL`." The composition-side member
/// `CompositionThumbnailConfiguration$storage` states the same value ("You can enable
/// both `SEQUENTIAL` and `LATEST`. Default: `SEQUENTIAL`."), so both are reported from
/// here.
pub const DEFAULT_THUMBNAIL_STORAGE: &str = "SEQUENTIAL";

/// Thumbnail recording mode, when the thumbnail configuration names none.
///
/// Documented on `ParticipantThumbnailConfiguration$recordingMode`: "Thumbnail recording
/// mode. Default: `DISABLED`."
pub const DEFAULT_THUMBNAIL_RECORDING_MODE: &str = "DISABLED";

/// The one thumbnail recording mode a thumbnail-generation INTERVAL may be configured in.
///
/// Documented on `ParticipantThumbnailConfiguration$targetIntervalSeconds`: "The targeted
/// thumbnail-generation interval in seconds. This is configurable only if `recordingMode`
/// is `INTERVAL`." The model carries no trait for that pairing — the interval's only
/// constraint is `@range 1..86400` and neither member constrains the other — so the seam
/// that resolves a stage's recording configuration enforces it
/// ([`check_thumbnail_interval_mode`]), the way `CreateIngestConfiguration` enforces the
/// documented pairing of its own two members.
pub const INTERVAL_THUMBNAIL_RECORDING_MODE: &str = "INTERVAL";

/// The dotted path of the member that carries a stage's thumbnail-generation interval, as
/// the wire spells it — what a refusal of that member names.
pub const RECORDING_THUMBNAIL_INTERVAL_MEMBER: &str =
    "autoParticipantRecordingConfiguration.thumbnailConfiguration.targetIntervalSeconds";

/// The requirement a refusal of the thumbnail interval states, inside the
/// member-constraint envelope ([`member_constraint_message`]): the mode the interval is
/// configurable in, and the mode this request's configuration is actually in.
fn thumbnail_interval_mode_requirement(mode: &str) -> String {
    format!(
        "targetIntervalSeconds is configurable only when recordingMode is \
         {INTERVAL_THUMBNAIL_RECORDING_MODE}, found {mode}"
    )
}

/// The DECISION [`thumbnail_interval_mode_requirement`] words: a thumbnail configuration
/// that CONFIGURES `targetIntervalSeconds` must be in the one mode the member is
/// documented as configurable in, and one that configures it in any other mode is refused
/// instead of persisting a value the model says cannot be set.
///
/// `requested` is the thumbnail configuration the request carried, which is `None` exactly
/// when it carried none; the interval is "configured" exactly when the request sent it, so
/// a configuration that leaves the member out is accepted in every mode and reports
/// [`DEFAULT_THUMBNAIL_INTERVAL_SECONDS`]. The mode the pairing is read against is the
/// EFFECTIVE one — what the request sent, else the documented
/// [`DEFAULT_THUMBNAIL_RECORDING_MODE`] — so an omitted `recordingMode` decides the
/// outcome exactly as the value it defaults to would.
///
/// `Err` is the sentence to put on the calling operation's `ValidationException`, the way
/// [`check_at_most_one_member`] and the other member checks of this service hand theirs
/// back.
pub fn check_thumbnail_interval_mode(
    requested: Option<&ParticipantThumbnailConfiguration>,
) -> Result<(), String> {
    let Some(requested) = requested else {
        return Ok(());
    };
    if requested.target_interval_seconds.is_none() {
        return Ok(());
    }
    let mode = requested
        .recording_mode
        .as_deref()
        .unwrap_or(DEFAULT_THUMBNAIL_RECORDING_MODE);
    if mode == INTERVAL_THUMBNAIL_RECORDING_MODE {
        return Ok(());
    }
    Err(member_constraint_message(
        RECORDING_THUMBNAIL_INTERVAL_MEMBER,
        &thumbnail_interval_mode_requirement(mode),
    ))
}

/// Target duration of a recorded HLS segment, when the HLS configuration does not set one.
///
/// Documented on `ParticipantRecordingHlsConfiguration$targetSegmentDurationSeconds`:
/// "Defines the target duration for recorded segments generated when recording a stage
/// participant. … Default: 6."
pub const DEFAULT_HLS_TARGET_SEGMENT_DURATION_SECONDS: i32 = 6;

/// The value a member reports when the request left it out: `documented`, the default
/// the member's own doc states — never the Smithy `@default` the framework would
/// materialise for it.
///
/// `requested` is what arrived, which is `None` exactly when the request did not carry
/// the member: the shapes whose documented default differs from their declared
/// `@default` keep that presence (see `crate::ops::types`), so a value the caller DID
/// send — the declared default included — is reported as it arrived, and the documented
/// default is reported only in its absence. One member, one default, whatever its
/// siblings carry.
pub fn documented_default<T>(requested: Option<T>, documented: T) -> T {
    requested.unwrap_or(documented)
}

/// [`DEFAULT_RECORDING_MEDIA_TYPE`] as the modelled `ParticipantRecordingMediaTypeList`.
pub fn default_recording_media_types() -> Vec<String> {
    vec![DEFAULT_RECORDING_MEDIA_TYPE.to_string()]
}

/// [`DEFAULT_THUMBNAIL_STORAGE`] as the modelled `ThumbnailStorageTypeList`.
pub fn default_thumbnail_storage() -> Vec<String> {
    vec![DEFAULT_THUMBNAIL_STORAGE.to_string()]
}

/// The thumbnail configuration a stage reports, with each member the caller left out
/// filled in from the model's documented default for it.
///
/// Reached only for a configuration that already satisfies the pairing its own
/// documentation carries ([`check_thumbnail_interval_mode`]), so an interval this fills in
/// from the request is one the request was allowed to configure.
pub fn effective_thumbnail_configuration(
    requested: Option<ParticipantThumbnailConfiguration>,
) -> ParticipantThumbnailConfiguration {
    let requested = requested.unwrap_or_default();
    ParticipantThumbnailConfiguration {
        target_interval_seconds: Some(
            requested
                .target_interval_seconds
                .unwrap_or(DEFAULT_THUMBNAIL_INTERVAL_SECONDS),
        ),
        storage: Some(requested.storage.unwrap_or_else(default_thumbnail_storage)),
        recording_mode: Some(
            requested
                .recording_mode
                .unwrap_or_else(|| DEFAULT_THUMBNAIL_RECORDING_MODE.to_string()),
        ),
    }
}

/// The HLS configuration a stage reports, with the one member the caller left out filled
/// in from the model's documented default for it.
pub fn effective_hls_configuration(
    requested: Option<ParticipantRecordingHlsConfiguration>,
) -> ParticipantRecordingHlsConfiguration {
    ParticipantRecordingHlsConfiguration {
        target_segment_duration_seconds: Some(
            requested
                .and_then(|configuration| configuration.target_segment_duration_seconds)
                .unwrap_or(DEFAULT_HLS_TARGET_SEGMENT_DURATION_SECONDS),
        ),
    }
}

/// The recording configuration attached to a stage whose request supplied none, or
/// supplied one with no storage configuration: every member at the default the model
/// documents for it.
pub fn default_recording_configuration() -> AutoParticipantRecordingConfiguration {
    AutoParticipantRecordingConfiguration {
        storage_configuration_arn: NO_STORAGE_CONFIGURATION_ARN.to_string(),
        media_types: Some(default_recording_media_types()),
        thumbnail_configuration: Some(effective_thumbnail_configuration(None)),
        recording_reconnect_window_seconds: Some(DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS),
        hls_configuration: Some(effective_hls_configuration(None)),
        record_participant_replicas: Some(DEFAULT_RECORD_PARTICIPANT_REPLICAS),
    }
}

/// The recording configuration a stage holds, given what its create or update request
/// supplied: the ONE place the configuration's documented defaults are applied, so
/// `CreateStage` and `UpdateStage` attach the same configuration for the same request.
///
/// A request that supplied no configuration, and one that supplied a configuration with
/// an empty `storageConfigurationArn`, both get [`default_recording_configuration`]: the
/// model documents that reset on the member itself — "To disable individual participant
/// recording, set this to `\"\"`; other fields in this object will get reset to their
/// defaults when sending `\"\"`."
///
/// Otherwise each member the caller left out is filled in from its documented default,
/// through [`documented_default`] for the two the model also declares a Smithy
/// `@default` for — the shape keeps their presence, so a value the caller sent is
/// reported as it arrived and the documented default applies only where the member was
/// absent.
pub fn effective_recording_configuration(
    requested: Option<AutoParticipantRecordingConfiguration>,
) -> AutoParticipantRecordingConfiguration {
    let requested = requested.unwrap_or_default();
    if requested.storage_configuration_arn.is_empty() {
        return default_recording_configuration();
    }
    AutoParticipantRecordingConfiguration {
        storage_configuration_arn: requested.storage_configuration_arn,
        media_types: Some(
            requested
                .media_types
                .unwrap_or_else(default_recording_media_types),
        ),
        thumbnail_configuration: Some(effective_thumbnail_configuration(
            requested.thumbnail_configuration,
        )),
        recording_reconnect_window_seconds: Some(documented_default(
            requested.recording_reconnect_window_seconds,
            DEFAULT_RECORDING_RECONNECT_WINDOW_SECONDS,
        )),
        hls_configuration: Some(effective_hls_configuration(requested.hls_configuration)),
        // The same documented default [`default_recording_configuration`] reports, so
        // one member of one shape has ONE default whatever its siblings carry.
        record_participant_replicas: Some(documented_default(
            requested.record_participant_replicas,
            DEFAULT_RECORD_PARTICIPANT_REPLICAS,
        )),
    }
}

/// Whether `configuration` records a participant that is a REPLICA of another stage.
///
/// `recordParticipantReplicas` is the member that says so — "Optional field to disable
/// replica participant recording" — carrying what the request set it to where it set it,
/// and [`DEFAULT_RECORD_PARTICIPANT_REPLICAS`] where it did not. The ONE reading of that
/// member, so what a read of a participant reports about its recording and what an update
/// refuses over cannot disagree about it.
pub fn records_participant_replicas(
    configuration: &AutoParticipantRecordingConfiguration,
) -> bool {
    documented_default(
        configuration.record_participant_replicas,
        DEFAULT_RECORD_PARTICIPANT_REPLICAS,
    )
}

/// The dotted path of the member that names the storage configuration a stage records
/// its participants into, as the wire spells it — what a refusal of that member names.
pub const RECORDING_STORAGE_CONFIGURATION_MEMBER: &str =
    "autoParticipantRecordingConfiguration.storageConfigurationArn";

/// The resource kind a recording configuration's reference names, as
/// [`unresolved_reference_message`] states it.
const STORAGE_CONFIGURATION_RESOURCE: &str = "storage configuration";

/// The recording configuration a stage holds, given what its create or update request
/// supplied AND the storage configurations this account actually holds: the ONE place a
/// supplied configuration is checked ([`check_thumbnail_interval_mode`]), defaulted
/// ([`effective_recording_configuration`]) and resolved, so a create and an update accept
/// and refuse the same request.
///
/// A member the model documents as configurable only while a SIBLING member carries a
/// particular value is refused where the request configures it in any other —
/// [`check_thumbnail_interval_mode`] is that check for the thumbnail interval, the only
/// member of this configuration whose documentation carries such a pairing.
///
/// A non-empty `storageConfigurationArn` is a REFERENCE to a storage configuration of
/// this same service ("ARN of the StorageConfiguration resource to use for individual
/// participant recording"), so one this store holds no configuration for is refused
/// instead of being copied into the stage: a stage attached to a configuration that does
/// not exist would report recording as wired up while nothing can record. `Err` is the
/// sentence to put on the calling operation's `ValidationException`, worded by
/// [`unresolved_reference_message`]; `operation` is that operation's name. Called before
/// anything is minted or written, so a refused request leaves the store as it was.
///
/// The empty string names no resource — it is the documented "no storage configuration"
/// value — so it is resolved by nothing and keeps the documented reset.
///
/// The reference is resolved in `region`'s view of the store, the region the calling
/// request was processed in: a configuration of another region is not one this account
/// view holds, so it resolves to nothing exactly as an ARN nothing was ever created for
/// does ([`in_request_region`]).
pub fn resolved_recording_configuration(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    operation: &str,
    requested: Option<AutoParticipantRecordingConfiguration>,
    region: &str,
) -> Result<AutoParticipantRecordingConfiguration, String> {
    // The one pairing the thumbnail configuration's own documentation carries, read off the
    // request as it arrived — before any default is filled in or any member is reset — so
    // what the request CONFIGURES decides the outcome and a value the model says cannot be
    // set is never persisted ([`check_thumbnail_interval_mode`]).
    check_thumbnail_interval_mode(
        requested
            .as_ref()
            .and_then(|configuration| configuration.thumbnail_configuration.as_ref()),
    )?;
    let configuration = effective_recording_configuration(requested);
    let arn = configuration.storage_configuration_arn.as_str();
    if !arn.is_empty() && state.storage_configuration_in(region, arn).is_none() {
        return Err(unresolved_reference_message(
            operation,
            RECORDING_STORAGE_CONFIGURATION_MEMBER,
            STORAGE_CONFIGURATION_RESOURCE,
            arn,
        ));
    }
    Ok(configuration)
}

/// The member that names the stage an ingest configuration is associated with, as the
/// wire spells it — what a refusal of that member names.
pub const INGEST_CONFIGURATION_STAGE_MEMBER: &str = "stageArn";

/// The resource kind a stage reference names, as [`unresolved_reference_message`]
/// states it.
const STAGE_RESOURCE: &str = "stage";

/// The stage an ingest configuration is associated with, given what its create or update
/// request supplied AND the stages this account actually holds: the ONE place a supplied
/// association is both defaulted and resolved, so a create and an update accept and
/// refuse the same request.
///
/// A non-empty `stageArn` is a REFERENCE to a stage of this same service ("ARN of the
/// stage with which the IngestConfiguration is associated"), so one this store holds no
/// stage for is refused instead of being persisted: the association is what a publisher's
/// ingest is routed into, so a configuration pointing at a stage that does not exist
/// would report an association nothing can serve. `Err` is the sentence to put on the
/// calling operation's `ValidationException`, worded by [`unresolved_reference_message`];
/// `operation` is that operation's name. Called before anything is minted or written, so
/// a refused request leaves the store as it was.
///
/// An absent member, and the empty string the modelled `IngestConfigurationStageArn`
/// admits, both name no stage — [`NO_ASSOCIATED_STAGE_ARN`] — so they are resolved by
/// nothing.
///
/// The reference is resolved in `region`'s view of the store, the region the calling
/// request was processed in, the same way a stage's recording reference is: a stage of
/// another region is not one this account view holds.
pub fn resolved_stage_association(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    operation: &str,
    requested: Option<String>,
    region: &str,
) -> Result<String, String> {
    let stage_arn = requested.unwrap_or_else(|| NO_ASSOCIATED_STAGE_ARN.to_string());
    if stage_arn != NO_ASSOCIATED_STAGE_ARN && state.stage_in(region, &stage_arn).is_none() {
        return Err(unresolved_reference_message(
            operation,
            INGEST_CONFIGURATION_STAGE_MEMBER,
            STAGE_RESOURCE,
            &stage_arn,
        ));
    }
    Ok(stage_arn)
}

// ---------------------------------------------------------------------------
// A stage's data-plane endpoints
//
// The model's `StageEndpoints` doc tells a caller to cache these values "at stage
// creation", so every stage carries all four from the moment it exists — and each names
// THIS emulator, because this emulator is the thing serving the stage. They are composed
// per request from the address that request reached this process at rather than stored,
// which is why they live in [`stage_endpoints`] beside the rest of the addressing rather
// than on the record: see that module.
// ---------------------------------------------------------------------------

/// What `activeSessionId` reports for a stage no session is active in.
///
/// Recorded behaviour: AWS reports the member as the empty string rather than leaving it
/// out, so a caller that reads the field off a stage always finds it and compares it
/// against a session id. (The model's `StageSessionId` `@length 16..16` describes an
/// actual session id; the empty value is the "no session" reading, and output is not
/// constraint-checked.)
pub const NO_ACTIVE_SESSION_ID: &str = "";

/// Page size a listing uses when the request omits `maxResults`.
///
/// The model documents it on `ListStagesRequest$maxResults` ("Maximum number of
/// results to return. Default: 50.") and states the same default on
/// `ListParticipantsRequest$maxResults`, `ListParticipantReplicasRequest$maxResults`,
/// `ListParticipantEventsRequest$maxResults` and `ListStageSessionsRequest$maxResults`,
/// and the same again on `ListPublicKeysRequest$maxResults`, so those listings page by
/// it.
pub const DEFAULT_LIST_PAGE_SIZE: usize = 50;

/// Total size an `attributes` map may reach, on every shape of this service that
/// carries one.
///
/// The model documents it on `CreateParticipantTokenRequest$attributes`,
/// `ParticipantTokenConfiguration$attributes` and
/// `CreateIngestConfigurationRequest$attributes` — each "The maximum length of this
/// field is 1 KB total." It is a documented limit the model carries no constraint trait
/// for, so the handler enforces it and answers `ValidationException`.
///
/// The measure is the one [`attributes_size`] computes: the size of the map as the
/// request carried it, which is its JSON encoding.
pub const MAX_ATTRIBUTES_BYTES: usize = 1024;

/// How the refusal of [`MAX_ATTRIBUTES_BYTES`] SPELLS the limit it states.
///
/// The model states the bound in kilobytes ("The maximum length of this field is 1 KB
/// total."), and the recorded refusal quotes it in those words rather than in the byte
/// count it works out to, so the sentence spells it the way the documentation a caller
/// read does.
pub const MAX_ATTRIBUTES_SIZE_LABEL: &str = "1 KB";

/// Region an ARN names when the request carries no usable SigV4 credential scope.
pub const DEFAULT_REGION: &str = "us-east-1";

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller and reclaimed only by an explicit
// `DeleteStage`, so each growth site carries a cap and nothing caller-sized reaches
// the store unchecked. IVS RealTime publishes no per-account stage or participant
// -token quota this emulator can quote, so both values are this emulator's own
// resource guard, set well above any realistic emulated use; a request past either
// is answered with the modelled `ServiceQuotaExceededException` (HTTP 402), which is
// what the real service answers when an account's quota is reached.
//
// The product is therefore finite: at most `MAX_STAGES` stages, each with at most
// `MAX_PARTICIPANT_TOKENS_PER_STAGE` token records, each record holding members the
// model already bounds (`userId` 0..128, `capabilities` 0..2, and `attributes`
// capped at [`MAX_ATTRIBUTES_BYTES`]).
// ---------------------------------------------------------------------------

/// Stages this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `CreateStage` grows.
pub const MAX_STAGES: usize = 1_000;

/// Participant-token records this emulator keeps per stage. Not an AWS quota — this
/// emulator's own resource guard on the store `CreateParticipantToken` grows.
pub const MAX_PARTICIPANT_TOKENS_PER_STAGE: usize = 1_000;

/// Ingest configurations this emulator keeps for one engine. Not an AWS quota — this
/// emulator's own resource guard on the one store `CreateIngestConfiguration` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_INGEST_CONFIGURATIONS: usize = 1_000;

/// Encoder configurations this emulator keeps for one engine. Not an AWS quota — this
/// emulator's own resource guard on the one store `CreateEncoderConfiguration` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402). A record is reclaimed only by an
/// explicit `DeleteEncoderConfiguration`.
pub const MAX_ENCODER_CONFIGURATIONS: usize = 1_000;

// ---------------------------------------------------------------------------
// The modelled defaults of an encoder configuration's video settings
//
// `EncoderConfiguration$video` is reported for every encoder configuration, and each
// member of `Video` is optional with a documented default, so a request that supplies
// none — or only some — still reads back a complete video configuration. Every constant
// below cites the member doc it comes from; none is this emulator's choice.
// ---------------------------------------------------------------------------

/// Video-resolution width of an encoder configuration whose request did not set one.
///
/// Documented on `Video$width`: "Video-resolution width. This must be an even number.
/// … Default: 1280."
pub const DEFAULT_VIDEO_WIDTH: i32 = 1280;

/// Video-resolution height of an encoder configuration whose request did not set one.
///
/// Documented on `Video$height`: "Video-resolution height. This must be an even number.
/// … Default: 720."
pub const DEFAULT_VIDEO_HEIGHT: i32 = 720;

/// Frame rate of an encoder configuration whose request did not set one.
///
/// Documented on `Video$framerate`: "Video frame rate, in fps. Default: 30."
pub const DEFAULT_VIDEO_FRAMERATE: f32 = 30.0;

/// Output bitrate, in bps, of an encoder configuration whose request did not set one.
///
/// Documented on `Video$bitrate`: "Bitrate for generated output, in bps. Default:
/// 2500000." — the "bitrate 2500 kbps" the `EncoderConfiguration$video` doc names.
pub const DEFAULT_VIDEO_BITRATE: i32 = 2_500_000;

/// Total pixels (`width` times `height`) a video configuration may reach.
///
/// Documented on both `Video$width` and `Video$height`: "Note that the maximum value is
/// determined by `width` times `height`, such that the maximum total pixels is 2073600
/// (1920x1080 or 1080x1920)." The model bounds each member on its own (`@range 2..1920`)
/// and carries no trait for their product, so the handler enforces it and answers the
/// modelled `ValidationException`.
pub const MAX_VIDEO_PIXELS: i64 = 2_073_600;

/// The video configuration an encoder configuration holds, given what its creating
/// request supplied: each member the request left out filled in from the model's
/// documented default for it.
///
/// Every member is reported, so a caller reading `video` back finds the resolution,
/// frame rate and bitrate the encoder will actually use rather than the absence of the
/// members it did not name.
pub fn effective_video(requested: Option<Video>) -> Video {
    let requested = requested.unwrap_or_default();
    Video {
        width: Some(requested.width.unwrap_or(DEFAULT_VIDEO_WIDTH)),
        height: Some(requested.height.unwrap_or(DEFAULT_VIDEO_HEIGHT)),
        framerate: Some(requested.framerate.unwrap_or(DEFAULT_VIDEO_FRAMERATE)),
        bitrate: Some(requested.bitrate.unwrap_or(DEFAULT_VIDEO_BITRATE)),
    }
}

/// Page size `ListEncoderConfigurations` uses when the request omits `maxResults`.
///
/// The model documents it on `ListEncoderConfigurationsRequest$maxResults`: "Maximum
/// number of results to return. Default: 100."
pub const DEFAULT_ENCODER_CONFIGURATION_PAGE_SIZE: usize = 100;

/// Storage configurations this emulator keeps for one engine. Not an AWS quota — this
/// emulator's own resource guard on the one store `CreateStorageConfiguration` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_STORAGE_CONFIGURATIONS: usize = 1_000;

/// The ceiling the model puts on one `ListStorageConfigurations` page, from
/// `MaxStorageConfigurationResults`' `@range(1..100)`
/// (`smithy_query(query="shape",shape="MaxStorageConfigurationResults")`).
pub const MAX_STORAGE_CONFIGURATION_RESULTS: usize = 100;

/// Page size `ListStorageConfigurations` uses when the request omits `maxResults`.
///
/// The model documents it on `ListStorageConfigurationsRequest$maxResults`: "Default:
/// your service quota or 100, whichever is smaller." The quota on this store is
/// [`MAX_STORAGE_CONFIGURATIONS`] and the published 100 is the member's own modelled
/// maximum, so the default is computed from the two rather than written out — a listing
/// with a smaller quota than 100 pages by the quota, exactly as documented.
pub const DEFAULT_STORAGE_CONFIGURATION_PAGE_SIZE: usize =
    if MAX_STORAGE_CONFIGURATIONS < MAX_STORAGE_CONFIGURATION_RESULTS {
        MAX_STORAGE_CONFIGURATIONS
    } else {
        MAX_STORAGE_CONFIGURATION_RESULTS
    };

/// Public keys this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `ImportPublicKey` grows, set the same way
/// [`MAX_STAGES`] is and answered with the same modelled `ServiceQuotaExceededException`
/// (HTTP 402).
pub const MAX_PUBLIC_KEYS: usize = 1_000;

/// Stage sessions this emulator keeps per stage. Not an AWS quota — this emulator's
/// own resource guard on the store [`join_stage_session`] grows, set the same way
/// [`MAX_STAGES`] is. A session is not created by an API call (see
/// [`StageSessionRecord`]), so reaching it is not a request to refuse: the join is
/// simply not recorded.
pub const MAX_STAGE_SESSIONS_PER_STAGE: usize = 1_000;

/// Participants this emulator keeps per stage session, bounded the same way and for
/// the same reason as [`MAX_STAGE_SESSIONS_PER_STAGE`].
pub const MAX_PARTICIPANTS_PER_SESSION: usize = 1_000;

/// Occurrences this emulator keeps per stage session, bounded the same way and for the
/// same reason as [`MAX_STAGE_SESSIONS_PER_STAGE`]: an occurrence is not an API call
/// either (see [`EventRecord`]), so reaching the bound is not a request to refuse — the
/// occurrence is simply not recorded.
pub const MAX_EVENTS_PER_SESSION: usize = 10_000;

// ---------------------------------------------------------------------------
// What an ingest configuration reports beyond what the creating request said
//
// `CreateIngestConfiguration` takes the protocol, the stage and the publisher's
// identity; the three members an `IngestConfiguration` reports that no request
// supplies — its state, its stage when the request named none, and its redundant
// credentials — come from here, each from the model's own doc for that member. The
// state is the one of them that is not fixed: it follows the publishers the
// configuration's stage holds ([`ingest_configuration_publishers`]).
// ---------------------------------------------------------------------------

/// State of an ingest configuration nobody is publishing to.
///
/// One of the two values the modelled `IngestConfigurationState` carries
/// (`smithy_query(query="shape",shape="IngestConfigurationState")`), and the one a
/// configuration reports while no publisher is broadcasting with it. It is not a
/// transition the clock owes: what moves a configuration off it is a publisher
/// connecting, not time passing, so it does not change on its own.
pub const NO_PUBLISHER_INGEST_CONFIGURATION_STATE: &str = "INACTIVE";

/// State of an ingest configuration a publisher is broadcasting with right now.
///
/// The other value of the modelled `IngestConfigurationState`, and the one its member
/// doc names: `IngestConfiguration$state` "is `ACTIVE` if a publisher currently is
/// publishing to the stage associated with the ingest configuration". Which publishers
/// those are is [`ingest_configuration_publishers`].
pub const PUBLISHING_INGEST_CONFIGURATION_STATE: &str = "ACTIVE";

/// `stageArn` of an ingest configuration associated with no stage.
///
/// `IngestConfiguration$stageArn` is `@required` while
/// `CreateIngestConfigurationRequest$stageArn` is optional, and the
/// `IngestConfigurationStageArn` both target admits the empty string (its pattern's
/// first alternative, `^$`). Reporting the member as empty is the one reading under
/// which both hold: a caller that reads `stageArn` off a configuration always finds it
/// and compares it against a stage ARN, exactly as [`NO_ACTIVE_SESSION_ID`] works for a
/// stage's session.
pub const NO_ASSOCIATED_STAGE_ARN: &str = "";

// ---------------------------------------------------------------------------
// The members this service MATERIALISES when the creating request leaves them out
//
// Two optional members of a created resource are reported by the recorded responses as
// the empty string rather than left out: an encoder configuration's `name` and an ingest
// configuration's `userId`. Each targets a string whose `@length` admits the empty value
// (`EncoderConfigurationName` is `@length(0..128)`, `UserId` is `@length(0..128)` —
// `smithy_query(query="shape",shape="EncoderConfigurationName,UserId")`), so the empty
// string is a value the member can hold rather than a stand-in for absence.
//
// So each is materialised ONCE, by the operation that creates the resource, and stored on
// the record as a `String`. That is what makes every projection of that resource — the
// create response, the single-resource read, the update response and a listing's summary —
// report the member: none of them decides whether to carry it, because the record holds a
// value. It is the same reading [`NO_ACTIVE_SESSION_ID`] and [`NO_ASSOCIATED_STAGE_ARN`]
// already apply to the two members whose documentation states the empty default outright.
// ---------------------------------------------------------------------------

/// `name` of a resource whose creating request named none.
///
/// `EncoderConfiguration$name` is "optional name to identify the resource", and a
/// configuration created without one reports the member as empty rather than omitting it.
/// `PublicKey$name` ("Public key name", `@length(0..128)` through `PublicKeyName`) is read
/// the same way, so one constant stands for both.
pub const UNNAMED_RESOURCE_NAME: &str = "";

/// `userId` of an ingest configuration whose creating request supplied none.
///
/// `IngestConfiguration$userId` is the "customer-assigned name to help identify the
/// participant using the IngestConfiguration", and a configuration created without one
/// reports the member as empty rather than omitting it.
pub const UNNAMED_PARTICIPANT_USER_ID: &str = "";

/// Credentials an ingest configuration carries BESIDE its own `streamKey` when
/// `redundantIngest` is enabled.
///
/// `IngestConfiguration$redundantIngestCredentials` is "a list of redundant ingest
/// credentials, present only when `redundantIngest` is set to `true`", and the Redundant
/// Ingest section of *IVS RTMP Publishing* it points at describes redundant ingest as
/// publishing one source to the service's redundant ingest endpoints beside the primary
/// one, so the list holds ONE credential per redundant endpoint — the two the service
/// offers beside the primary path the configuration's own
/// `streamKey`/`participantId` publish to. What the model states is applied exactly: the
/// member is absent unless redundant ingest is on, and each entry carries its own
/// participant id and its own ingest key.
pub const REDUNDANT_INGEST_CREDENTIALS: usize = 2;

// ---------------------------------------------------------------------------
// Participant replication
//
// `StartParticipantReplication` replicates a PUBLISHING participant of one stage into
// another, and `ListParticipantReplicas` reports the replicas a participant has. The
// values neither request supplies — the replication state, the reconnect window a
// request leaves out, the capability that makes a participant a publisher — are fixed
// here, each from the model's own doc or value list.
// ---------------------------------------------------------------------------

/// The replication state of a replica that has been started and not stopped.
///
/// `ReplicationState`'s value list is `ACTIVE` | `STOPPED`
/// (`smithy_query(query="shape",shape="ReplicationState")`), and the model's
/// `StartParticipantReplicationRequest$reconnectWindowSeconds` doc names the running
/// state: "replication will continue to be `ACTIVE`".
pub const REPLICATION_STATE_ACTIVE: &str = "ACTIVE";

/// The replication state of a replica whose replication has been stopped.
///
/// The other value of that same list, and the one `StopParticipantReplication` leaves a
/// replication in: the list carries no intermediate value, so a stopped replication is
/// stopped from the moment the stop is accepted and a reader never sees it running again.
pub const REPLICATION_STATE_STOPPED: &str = "STOPPED";

/// The capability a participant token must grant for its participant to publish.
///
/// From `ParticipantTokenCapability`'s value list (`PUBLISH` | `SUBSCRIBE`), the same
/// value [`DEFAULT_TOKEN_CAPABILITIES`] grants by default. Replication is of a
/// *publishing* participant, so a participant that cannot publish has nothing to
/// replicate.
pub const PUBLISH_CAPABILITY: &str = "PUBLISH";

/// Reconnect window a replication gets when the request omits `reconnectWindowSeconds`.
///
/// The model documents it on `StartParticipantReplicationRequest$reconnectWindowSeconds`:
/// "If the participant disconnects and then reconnects within the specified interval,
/// replication will continue to be `ACTIVE`. Default: 0."
pub const DEFAULT_REPLICATION_RECONNECT_WINDOW_SECONDS: i32 = 0;

/// Participant replicas this emulator keeps per source stage. Not an AWS quota — this
/// emulator's own resource guard on the one store `StartParticipantReplication` grows,
/// set the same way [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402). The records are reclaimed when either
/// stage of a replica is deleted.
pub const MAX_PARTICIPANT_REPLICAS_PER_STAGE: usize = 1_000;

// ---------------------------------------------------------------------------
// Compositions
//
// `StartComposition` composites a stage into one or two destinations, and
// `GetComposition` reports the composition back. The values no request supplies — the
// layout members a request leaves out, the state a started composition is in, and when
// the clock owes it the next one — are fixed here, each from the model's own doc or
// value list.
// ---------------------------------------------------------------------------

/// `CompositionState` a composition is in from the moment `StartComposition` returns
/// until the clock owes it [`COMPOSITION_ACTIVATION_SPAN`] later.
///
/// From `CompositionState`'s value list (`STARTING` | `ACTIVE` | `STOPPING` | `FAILED` |
/// `STOPPED`, `crate::ops::types::COMPOSITION_STATE_VALUES`): a composition that has just
/// been started is starting, not yet running.
pub const COMPOSITION_STATE_STARTING: &str = "STARTING";

/// `CompositionState` a started composition settles into, from the same value list.
pub const COMPOSITION_STATE_ACTIVE: &str = "ACTIVE";

/// `DestinationState` a destination of a starting composition is in, from
/// `DestinationState`'s value list (`crate::ops::types::DESTINATION_STATE_VALUES`).
pub const DESTINATION_STATE_STARTING: &str = "STARTING";

/// `DestinationState` a destination settles into with its composition.
pub const DESTINATION_STATE_ACTIVE: &str = "ACTIVE";

/// How long after `StartComposition` the clock owes a composition and its destinations
/// the `STARTING` -> `ACTIVE` transition.
///
/// AWS publishes no span for it, so this is the emulator's own, inside the band this
/// tree fixes for a transition the clock owes. It is deliberately NOT zero: a caller
/// that reads the composition back straight after starting it must still observe
/// `STARTING`, the way it would against AWS.
pub const COMPOSITION_ACTIVATION_SPAN: Duration = Duration::from_millis(500);

/// Aspect ratio of a grid layout's non-featured tiles when the request sets none.
///
/// Documented on `GridConfiguration$videoAspectRatio`: "`VIDEO` is 16:9, `SQUARE` is
/// 1:1, and `PORTRAIT` is 3:4. Default: `VIDEO`."
pub const DEFAULT_GRID_VIDEO_ASPECT_RATIO: &str = "VIDEO";

/// Fill mode of a grid layout's tiles when the request sets none.
///
/// Documented on `GridConfiguration$videoFillMode`: "When not set, `videoFillMode`
/// defaults to `COVER` fill mode for participants in the grid" — the tiles the grid
/// lays out, which is what this member configures.
pub const DEFAULT_GRID_VIDEO_FILL_MODE: &str = "COVER";

/// Fill mode of a picture-in-picture layout's tiles when the request sets none.
///
/// Documented on `PipConfiguration$videoFillMode`: "Default: `COVER`."
pub const DEFAULT_PIP_VIDEO_FILL_MODE: &str = "COVER";

/// What a picture-in-picture window does when every participant has left, when the
/// request does not say.
///
/// Documented on `PipConfiguration$pipBehavior`: "`STATIC` (maintains original
/// position/size) or `DYNAMIC` (expands to full composition). Default: `STATIC`."
pub const DEFAULT_PIP_BEHAVIOR: &str = "STATIC";

/// Corner a picture-in-picture window sits in when the request does not say.
///
/// Documented on `PipConfiguration$pipPosition`: "Default: `BOTTOM_RIGHT`."
pub const DEFAULT_PIP_POSITION: &str = "BOTTOM_RIGHT";

/// Spacing between a grid layout's participant tiles, when the request does not size it.
///
/// Documented on `GridConfiguration$gridGap`: "Specifies the spacing between participant
/// tiles in pixels. Default: `2`." `PipConfiguration$gridGap` targets the same `GridGap`
/// shape and documents `0` instead, so the default belongs to the member rather than to
/// the shape, and the picture-in-picture layout keeps its own.
///
/// The model declares `@default 0` for the member, so the shape keeps the member's
/// PRESENCE (see the hand-written `GridConfiguration` in `crate::ops::types`) and this
/// default applies only where the request carried no value: a `0` the caller asks for —
/// the published minimum of the member's `@range` — is reported as it arrived.
pub const DEFAULT_GRID_GAP_PIXELS: i32 = 2;

/// Whether a grid omits participants with stopped video, when the request does not say.
///
/// Documented on `GridConfiguration$omitStoppedVideo`: "Determines whether to omit
/// participants with stopped video in the composition. Default: `false`" — the same
/// value the model declares as the member's `@default`.
pub const DEFAULT_OMIT_STOPPED_VIDEO: bool = false;

/// `featuredParticipantAttribute` of a layout that features no participant.
///
/// Documented on `GridConfiguration$featuredParticipantAttribute` and, in the same words,
/// on `PipConfiguration$featuredParticipantAttribute`: "Default: `\"\"` (no featured
/// participant)." Both layouts report the member from here, the way every other
/// empty-string documented default of this service is reported (see
/// [`NO_STORAGE_CONFIGURATION_ARN`]).
pub const NO_FEATURED_PARTICIPANT_ATTRIBUTE: &str = "";

/// `pipParticipantAttribute` of a picture-in-picture layout that names no PiP participant.
///
/// Documented on `PipConfiguration$pipParticipantAttribute`: "Default: `\"\"` (no PiP
/// participant)."
pub const NO_PIP_PARTICIPANT_ATTRIBUTE: &str = "";

/// Compositions this emulator keeps for one engine. Not an AWS quota — this emulator's
/// own resource guard on the one store `StartComposition` grows, set the same way
/// [`MAX_STAGES`] is and answered with the same modelled
/// `ServiceQuotaExceededException` (HTTP 402).
pub const MAX_COMPOSITIONS: usize = 1_000;

/// Page size `ListCompositions` uses when the request omits `maxResults`.
///
/// The model documents it on `ListCompositionsRequest$maxResults`: "Maximum number of
/// results to return. Default: 100."
pub const DEFAULT_COMPOSITION_PAGE_SIZE: usize = 100;

/// Characters in a minted composition id. The model's `CompositionArn` pattern allows
/// `[a-zA-Z0-9-]+` after `composition/`, and caps the whole ARN at 128 characters.
pub const COMPOSITION_ID_LEN: usize = 12;

/// Characters in a minted destination id. `Destination$id` is a plain `String`
/// ("Unique identifier for this destination, assigned by IVS"), so the id charset
/// [`mint_id`] produces satisfies it.
pub const DESTINATION_ID_LEN: usize = 12;

/// The composition ARN for `composition_id`, in the form the model's `CompositionArn`
/// pattern requires: `arn:aws:ivs:<region>:<account>:composition/<id>`.
pub fn composition_arn(region: &str, composition_id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("composition/{composition_id}"))
}

/// The grid layout `requested` describes, with the documented default for every member
/// it left out — each applied where the request carried no value, so a value the request
/// did carry is reported as it arrived.
///
/// `gridGap` is the member whose documented default (`2`) differs from the `@default`
/// (`0`) the model declares for it, which is why the shape keeps every member's
/// presence; `omitStoppedVideo` documents the value the model declares, so its default
/// is reached by the same route and reports the same thing either way.
///
/// `participantOrderAttribute` is the one member left absent when the request omits it:
/// its doc states no default, only that an unset and an empty attribute name order
/// participants the same way.
fn effective_grid(requested: GridConfiguration) -> GridConfiguration {
    GridConfiguration {
        featured_participant_attribute: Some(
            requested
                .featured_participant_attribute
                .unwrap_or_else(|| NO_FEATURED_PARTICIPANT_ATTRIBUTE.to_string()),
        ),
        omit_stopped_video: Some(documented_default(
            requested.omit_stopped_video,
            DEFAULT_OMIT_STOPPED_VIDEO,
        )),
        video_aspect_ratio: Some(
            requested
                .video_aspect_ratio
                .unwrap_or_else(|| DEFAULT_GRID_VIDEO_ASPECT_RATIO.to_string()),
        ),
        video_fill_mode: Some(
            requested
                .video_fill_mode
                .unwrap_or_else(|| DEFAULT_GRID_VIDEO_FILL_MODE.to_string()),
        ),
        grid_gap: Some(documented_default(
            requested.grid_gap,
            DEFAULT_GRID_GAP_PIXELS,
        )),
        participant_order_attribute: requested.participant_order_attribute,
    }
}

/// The picture-in-picture layout `requested` describes, with the documented default for
/// every member it left out.
///
/// `gridGap`, `pipOffset` and `omitStoppedVideo` each document the same default the model
/// declares as their `@default`, so the value the framework materialised for an absent
/// member is already the documented one and is reported as it arrived — which is why this
/// shape keeps the macro's members where `GridConfiguration` keeps its own presence.
///
/// `pipWidth` and `pipHeight` are deliberately left absent when the request omits them:
/// the model documents that each is then "based on the size of the composition and the
/// aspect ratio of the participant's video" rather than taking a fixed value, and
/// `participantOrderAttribute` is left absent because its doc states no default.
fn effective_pip(requested: PipConfiguration) -> PipConfiguration {
    PipConfiguration {
        featured_participant_attribute: Some(
            requested
                .featured_participant_attribute
                .unwrap_or_else(|| NO_FEATURED_PARTICIPANT_ATTRIBUTE.to_string()),
        ),
        pip_participant_attribute: Some(
            requested
                .pip_participant_attribute
                .unwrap_or_else(|| NO_PIP_PARTICIPANT_ATTRIBUTE.to_string()),
        ),
        video_fill_mode: Some(
            requested
                .video_fill_mode
                .unwrap_or_else(|| DEFAULT_PIP_VIDEO_FILL_MODE.to_string()),
        ),
        pip_behavior: Some(
            requested
                .pip_behavior
                .unwrap_or_else(|| DEFAULT_PIP_BEHAVIOR.to_string()),
        ),
        pip_position: Some(
            requested
                .pip_position
                .unwrap_or_else(|| DEFAULT_PIP_POSITION.to_string()),
        ),
        ..requested
    }
}

/// The layout a composition runs with, from what the request asked for.
///
/// `Composition$layout` is `@required`, so every composition reports one even when the
/// starting request named none: the model documents the fallback on
/// `LayoutConfiguration$grid` — "Default: Grid layout" — so a request that configures
/// neither layout gets a grid with every documented grid default.
pub fn effective_layout(requested: Option<LayoutConfiguration>) -> LayoutConfiguration {
    let requested = requested.unwrap_or_default();
    if let Some(pip) = requested.pip {
        return LayoutConfiguration {
            grid: None,
            pip: Some(effective_pip(pip)),
        };
    }
    LayoutConfiguration {
        grid: Some(effective_grid(requested.grid.unwrap_or_default())),
        pip: None,
    }
}

// ---------------------------------------------------------------------------
// The modelled defaults of a composition destination's configuration
//
// A destination configuration is stored and reported the way every other configuration
// this service stores is: each member the request left out of an object it DID supply is
// reported with the default that member's own doc states. The constants come from the
// member docs; none is this emulator's choice.
// ---------------------------------------------------------------------------

/// Target duration of a segment recorded by composite recording, when the recording
/// configuration of a composition destination does not set one.
///
/// Documented on `CompositionRecordingHlsConfiguration$targetSegmentDurationSeconds`:
/// "Defines the target duration for recorded segments generated when using composite
/// recording. Default: 2." Composite recording is not the participant recording
/// [`DEFAULT_HLS_TARGET_SEGMENT_DURATION_SECONDS`] cites, and the two members document
/// different values, so each has its own constant.
pub const DEFAULT_COMPOSITE_HLS_TARGET_SEGMENT_DURATION_SECONDS: i32 = 2;

/// The HLS configuration a composition destination's recording reports, with the one
/// member the request left out filled in from the model's documented default for it.
fn effective_composite_hls_configuration(
    requested: Option<CompositionRecordingHlsConfiguration>,
) -> CompositionRecordingHlsConfiguration {
    CompositionRecordingHlsConfiguration {
        target_segment_duration_seconds: Some(
            requested
                .and_then(|configuration| configuration.target_segment_duration_seconds)
                .unwrap_or(DEFAULT_COMPOSITE_HLS_TARGET_SEGMENT_DURATION_SECONDS),
        ),
    }
}

/// The recording configuration a composition destination reports, given the one its
/// starting request supplied.
///
/// `hlsConfiguration` is reported for every recording configuration that exists: the
/// member is there "to return information about how the recording will be configured",
/// and the recording is configured with the documented segment duration whether the
/// request named it or not.
///
/// `format` is reported as it arrived: `RecordingConfiguration$format` documents no
/// default, so an absent one is not filled in.
fn effective_composite_recording_configuration(
    requested: RecordingConfiguration,
) -> RecordingConfiguration {
    RecordingConfiguration {
        hls_configuration: Some(effective_composite_hls_configuration(
            requested.hls_configuration,
        )),
        format: requested.format,
    }
}

/// One thumbnail configuration of a composition destination, with each member the request
/// left out filled in from the model's documented default for it — the composition-side
/// counterpart of [`effective_thumbnail_configuration`], over the shape that carries no
/// `recordingMode`.
fn effective_composition_thumbnail_configuration(
    requested: CompositionThumbnailConfiguration,
) -> CompositionThumbnailConfiguration {
    CompositionThumbnailConfiguration {
        target_interval_seconds: Some(
            requested
                .target_interval_seconds
                .unwrap_or(DEFAULT_THUMBNAIL_INTERVAL_SECONDS),
        ),
        storage: Some(requested.storage.unwrap_or_else(default_thumbnail_storage)),
    }
}

/// The S3 destination a composition records to, from what the starting request asked for:
/// every member of every configuration object the request supplied, with the model's
/// documented default where the request left the member out.
///
/// The two optional objects are defaulted only where the request SUPPLIED them, which is
/// what the model gives their absence meaning for: `recordingConfiguration` is "an
/// optional customer specification", and `thumbnailConfigurations` is what "allows you to
/// enable/disable the recording of thumbnails for a `Composition`" — and this thumbnail
/// shape, unlike the stage-side one, carries no `recordingMode` member that could report
/// thumbnails as off, so an element conjured for a request that supplied none would
/// report a composition as generating thumbnails it never asked for. A documented default
/// fills a member left out of an object that exists; it does not create the object.
/// `storageConfigurationArn` and `encoderConfigurationArns` are `@required`, so neither
/// can be absent.
fn effective_s3_destination_configuration(
    requested: S3DestinationConfiguration,
) -> S3DestinationConfiguration {
    S3DestinationConfiguration {
        storage_configuration_arn: requested.storage_configuration_arn,
        encoder_configuration_arns: requested.encoder_configuration_arns,
        recording_configuration: requested
            .recording_configuration
            .map(effective_composite_recording_configuration),
        thumbnail_configurations: requested.thumbnail_configurations.map(|configurations| {
            configurations
                .into_iter()
                .map(effective_composition_thumbnail_configuration)
                .collect()
        }),
    }
}

/// The configuration one destination of a composition runs with: the ONE place a
/// destination configuration's documented defaults are applied, so what `StartComposition`
/// persists already carries them and every read of the destination reports the same thing.
///
/// A channel destination has nothing to fill in: `ChannelDestinationConfiguration$channelArn`
/// is `@required` and its `encoderConfigurationArn` documents no default, so it is
/// reported as it arrived — as is `DestinationConfiguration$name`.
pub fn effective_destination_configuration(
    requested: DestinationConfiguration,
) -> DestinationConfiguration {
    DestinationConfiguration {
        name: requested.name,
        channel: requested.channel,
        s3: requested.s3.map(effective_s3_destination_configuration),
    }
}

// Each constant below marks the page token of ONE listing, so a token from elsewhere is
// refused rather than silently read as a position. The marker a token actually carries is
// the constant plus the SCOPE the issuing request paged within ([`scope_marker`]), so two
// calls of the same listing over different records — another region, another stage,
// another session, another participant — do not share a token either.

/// Marks a `ListStages` page token as this service's, so a token from elsewhere is
/// refused rather than silently read as a position.
const PAGE_TOKEN_PREFIX: &str = "ivs-realtime-stage-after:";

/// Marks a `ListParticipants` page token, the same way [`PAGE_TOKEN_PREFIX`] marks a
/// `ListStages` one. The prefixes differ so a token issued by one listing names no
/// position in the other: the two page over different records, and a position in one
/// is meaningless in the other.
const PARTICIPANT_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-participant-after:";

/// The same marker for a `ListParticipantReplicas` page, kept distinct from
/// [`PAGE_TOKEN_PREFIX`] so a token issued by one listing names no position in the
/// other and is refused there.
const REPLICA_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-replica-after:";

/// The same marker for a `ListStorageConfigurations` page, kept distinct from the two
/// above so a token issued by one listing names no position in another and is refused
/// there.
const STORAGE_CONFIGURATION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-storage-configuration-after:";

/// The same marker for a `ListStageSessions` page, kept distinct from the others so a
/// token issued by one listing names no position in another and is refused there.
const STAGE_SESSION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-stage-session-after:";

/// The same marker for a `ListParticipantEvents` page, kept distinct from the others so
/// a token issued by one listing names no position in another and is refused there.
const PARTICIPANT_EVENT_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-participant-event-after:";

/// The same marker for a `ListCompositions` page, kept distinct from the others so a
/// token issued by one listing names no position in another and is refused there.
const COMPOSITION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-composition-after:";

/// The same marker for a `ListEncoderConfigurations` page, kept distinct from the others
/// so a token issued by one listing names no position in another and is refused there.
const ENCODER_CONFIGURATION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-encoder-configuration-after:";

/// The same marker for a `ListIngestConfigurations` page, kept distinct from the others
/// so a token issued by one listing names no position in another and is refused there.
const INGEST_CONFIGURATION_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-ingest-configuration-after:";

/// The same marker for a `ListPublicKeys` page, kept distinct from the others so a token
/// issued by one listing names no position in another and is refused there.
const PUBLIC_KEY_PAGE_TOKEN_PREFIX: &str = "ivs-realtime-public-key-after:";

/// One stage, as `CreateStage` persisted it and `GetStage` / `ListStages` report it.
#[derive(Debug, Clone)]
pub struct StageRecord {
    /// The stage ARN, and the single key every lookup, update and delete uses.
    pub arn: String,
    pub name: Option<String>,
    /// `None` until a session is active in the stage; no operation in this slice
    /// starts one. Reported as [`NO_ACTIVE_SESSION_ID`] while it is `None`.
    pub active_session_id: Option<String>,
    pub tags: HashMap<String, String>,
    /// The stage's individual-participant-recording configuration. Not optional: every
    /// stage has one, with the modelled defaults where the creating request supplied
    /// none ([`effective_recording_configuration`]).
    pub auto_participant_recording_configuration: AutoParticipantRecordingConfiguration,
    /// When the stage was created. No member of the modelled `Stage` /
    /// `StageSummary` reports it and pagination orders by [`Self::sequence`], so
    /// nothing reads it yet; it is recorded because the stage-session and
    /// participant operations (each its own gap issue) report times relative to the
    /// stage's creation.
    pub created_at: SystemTime,
    /// Creation order, and the stable key `ListStages` pages over, so deleting a
    /// stage mid-pagination cannot shift a later page onto an already-returned one.
    pub sequence: u64,
}

impl StageRecord {
    /// The modelled `Stage` this record reports to the request in `ctx`.
    ///
    /// Everything but the endpoints is the record as it is stored; the endpoints are
    /// composed for the address this request reached the emulator at and for the socket
    /// this deployment's contribution plane answers on
    /// ([`announced_stage_endpoints`]), because they are addresses a client then dials and
    /// this emulator is the thing serving the stage.
    ///
    /// `contribute` is the `host:port` that plane answers at
    /// ([`contribute_authority`], read off the store the caller already holds), or `None`
    /// for a deployment that could open none.
    pub fn to_stage(
        &self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        contribute: Option<&str>,
    ) -> Stage {
        Stage {
            arn: self.arn.clone(),
            name: self.name.clone(),
            // Always reported, the empty value included: see [`NO_ACTIVE_SESSION_ID`].
            active_session_id: Some(self.active_session_id.clone().unwrap_or_default()),
            // Always reported, empty map included: `tags` is how a caller reads the
            // tags back, and an absent member is not the same answer as "none".
            tags: Some(self.tags.clone()),
            auto_participant_recording_configuration: Some(
                self.auto_participant_recording_configuration.clone(),
            ),
            endpoints: Some(announced_stage_endpoints(ctx, contribute)),
        }
    }

    /// The modelled `StageSummary` this record reports in a `ListStages` page.
    pub fn to_summary(&self) -> StageSummary {
        StageSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            // The same member of the same stage as [`Self::to_stage`] reports, so a
            // summary and a read answer alike.
            active_session_id: Some(self.active_session_id.clone().unwrap_or_default()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One issued participant token, as `CreateStage` and `CreateParticipantToken`
/// persisted it.
#[derive(Debug, Clone)]
pub struct ParticipantTokenRecord {
    pub participant_id: String,
    pub token: String,
    pub user_id: Option<String>,
    pub attributes: HashMap<String, String>,
    /// The lifetime the token was issued for: what the request asked for, or
    /// [`DEFAULT_TOKEN_DURATION_MINUTES`] when it asked for none. What
    /// [`expiration_time`] was computed from.
    pub duration_minutes: i32,
    /// The `duration` the REQUEST carried, `None` when it carried none — the presence a
    /// reported token can keep (see [`Self::to_participant_token`] and
    /// [`ReportedTokenDuration`]).
    pub requested_duration: Option<i32>,
    /// The capabilities the token really grants: what the request asked for, or
    /// [`default_token_capabilities`] when it asked for none. What every reader of this
    /// participant decides from, and not what the token REPORTS.
    pub capabilities: Vec<String>,
    /// The `capabilities` the REQUEST carried, `None` when it carried none — the presence
    /// the reported token keeps (see [`Self::to_participant_token`]).
    pub requested_capabilities: Option<Vec<String>>,
    pub expiration_time: SystemTime,
    /// When the token was REVOKED, and `None` while it has not been: the instant
    /// [`revoke_connection_token`] wrote down. A revoked token is no longer an identity
    /// this service issued against the stage ([`stage_participant`]), so it admits its
    /// holder to nothing, and the record stays — keeping the id it spent, so no later
    /// issuance mints the same one.
    pub revoked_at: Option<SystemTime>,
}

/// Which lifetime a reported `ParticipantToken` states, chosen by the operation doing the
/// reporting.
///
/// `ParticipantToken$duration` is optional and carries no `@default`, so the model leaves
/// each response free to restate the lifetime the service resolved or to echo only the one
/// the caller asked for; the two operations that mint tokens answer differently, so the
/// choice belongs to the caller of [`ParticipantTokenRecord::to_participant_token`] rather
/// than to the record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportedTokenDuration {
    /// The lifetime the token really has, defaults included: what the token's own
    /// `expirationTime` was computed from.
    Effective,
    /// Only a lifetime the REQUEST stated: a token the service sized from
    /// [`DEFAULT_TOKEN_DURATION_MINUTES`] reports no `duration` at all, and its lifetime
    /// is observable through `expirationTime`.
    AsRequested,
}

impl ParticipantTokenRecord {
    /// Whether this token has been revoked, which is what makes it admit nobody.
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// Revoke the token at `at`, answering whether THIS call is what revoked it.
    ///
    /// The one writer of [`Self::revoked_at`]: a token already revoked keeps the instant
    /// it was revoked at, so revoking it again changes nothing.
    pub fn revoke(&mut self, at: SystemTime) -> bool {
        if self.is_revoked() {
            return false;
        }
        self.revoked_at = Some(at);
        true
    }

    /// The modelled `ParticipantToken` this record reports, with `duration` stated as
    /// `reported` asks for.
    ///
    /// Every other optional member echoes what the REQUEST carried: `userId` and
    /// `attributes` as supplied, and `capabilities` because the reported member ("Set of
    /// capabilities that the user is allowed to perform in the stage.") documents no
    /// default and is `@length 0..2`, so a caller may legally ask for an empty set and a
    /// token that grants no capability carries no `capabilities` member rather than an
    /// empty list. The server-minted members — `participantId`, `token` and
    /// `expirationTime` — are always reported.
    pub fn to_participant_token(&self, reported: ReportedTokenDuration) -> ParticipantToken {
        ParticipantToken {
            participant_id: Some(self.participant_id.clone()),
            token: Some(self.token.clone()),
            user_id: self.user_id.clone(),
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
            duration: match reported {
                ReportedTokenDuration::Effective => Some(self.duration_minutes),
                ReportedTokenDuration::AsRequested => self.requested_duration,
            },
            // Reported only when the request named something, the same way `attributes`
            // is (see the rule above).
            capabilities: self
                .requested_capabilities
                .clone()
                .filter(|capabilities| !capabilities.is_empty()),
            expiration_time: Some(::aws_smithy_types::DateTime::from(self.expiration_time)),
        }
    }
}

/// One ingest credential this service minted: the participant a publisher appears as in
/// the stage, and the ingest key it publishes with.
///
/// Reported both as an `IngestConfiguration`'s own `participantId`/`streamKey` and as one
/// entry of its `redundantIngestCredentials`, which the model gives the same two members.
#[derive(Debug, Clone)]
pub struct IngestCredential {
    pub participant_id: String,
    pub stream_key: String,
}

impl IngestCredential {
    /// The modelled `RedundantIngestCredential` this credential reports.
    pub fn to_redundant_ingest_credential(&self) -> RedundantIngestCredential {
        RedundantIngestCredential {
            participant_id: Some(self.participant_id.clone()),
            stream_key: Some(self.stream_key.clone()),
        }
    }
}

/// One ingest configuration, as `CreateIngestConfiguration` persisted it and
/// `GetIngestConfiguration` reports it.
#[derive(Debug, Clone)]
pub struct IngestConfigurationRecord {
    /// The ingest-configuration ARN, and the single key every lookup uses.
    pub arn: String,
    pub name: Option<String>,
    /// `RTMP` or `RTMPS`: the protocol the creating request chose, which the model
    /// makes `@required` on both the request and the reported configuration.
    pub ingest_protocol: String,
    /// The primary ingest key, minted once at creation ([`mint_stream_key`]) and
    /// reported unchanged on every read, so a publisher that cached it keeps working.
    pub stream_key: String,
    /// The stage this configuration is associated with, or [`NO_ASSOCIATED_STAGE_ARN`]
    /// when the creating request named none.
    pub stage_arn: String,
    /// The participant this configuration's publisher appears as in the stage.
    pub participant_id: String,
    /// The user id the creating request gave, or [`UNNAMED_PARTICIPANT_USER_ID`] when it
    /// gave none: materialised at creation, so it is a value this record HOLDS rather
    /// than one each projection decides, and every projection reports it.
    pub user_id: String,
    pub redundant_ingest: bool,
    /// The backup credentials redundant ingest takes, minted beside the primary one when
    /// the switch is set — at creation, or by the update that sets it. Empty for a
    /// configuration whose [`Self::redundant_ingest`] was never set, and kept once minted;
    /// see [`REDUNDANT_INGEST_CREDENTIALS`] and [`Self::apply_redundant_ingest`].
    pub redundant_ingest_credentials: Vec<IngestCredential>,
    pub attributes: HashMap<String, String>,
    pub tags: HashMap<String, String>,
    /// Creation order, and the stable key a page over these records resumes from — the
    /// same role it plays on a [`StageRecord`].
    pub sequence: u64,
}

impl IngestConfigurationRecord {
    /// The modelled `IngestConfiguration` this record reports, read against `store`:
    /// the configuration's `state` is not a stored value but a fact about the stage it
    /// is associated with right now, so it is computed from the records that hold that
    /// fact on every read ([`Self::state`]).
    pub fn to_ingest_configuration(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ) -> IngestConfiguration {
        IngestConfiguration {
            name: self.name.clone(),
            arn: self.arn.clone(),
            ingest_protocol: self.ingest_protocol.clone(),
            stream_key: self.stream_key.clone(),
            stage_arn: self.stage_arn.clone(),
            participant_id: self.participant_id.clone(),
            state: self.state(store),
            // Always reported, the empty string included: the user id is materialised on
            // the record ([`Self::user_id`]), so a caller cannot read "no user id" as
            // "the member is not served".
            user_id: Some(self.user_id.clone()),
            redundant_ingest: self.redundant_ingest,
            // Reported exactly when this configuration HOLDS backup credentials, which is
            // when its `redundantIngest` was set — "present only when `redundantIngest` is
            // set to `true`", as the member's own doc states what materialises them. They
            // are then the configuration's own
            // ([`IngestConfigurationRecord::apply_redundant_ingest`]), so a configuration
            // whose switch has since gone off still reports the keys it handed out, and one
            // whose switch was never on reports no member at all.
            redundant_ingest_credentials: (!self.redundant_ingest_credentials.is_empty()).then(
                || {
                    self.redundant_ingest_credentials
                        .iter()
                        .map(IngestCredential::to_redundant_ingest_credential)
                        .collect()
                },
            ),
            // Reported only when the configuration carries attributes, the same way a
            // participant token's are: the member is optional, and an absent member is
            // how the model says "none".
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `IngestConfigurationSummary` this record reports —
    /// `ListIngestConfigurations`' "summary information only".
    ///
    /// Every member it carries is the member of the same name on the full
    /// [`Self::to_ingest_configuration`], read off the same record at the same instant,
    /// so a listing and a read cannot disagree about one configuration. The summary shape
    /// carries neither the ingest keys (`streamKey`,
    /// `redundantIngestCredentials`) nor `attributes` nor `tags`
    /// (`smithy_query(query="shape",shape="IngestConfigurationSummary")`), which is why a
    /// page reports fewer members than a read rather than different ones.
    pub fn to_summary(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ) -> IngestConfigurationSummary {
        IngestConfigurationSummary {
            name: self.name.clone(),
            arn: self.arn.clone(),
            ingest_protocol: self.ingest_protocol.clone(),
            stage_arn: self.stage_arn.clone(),
            participant_id: self.participant_id.clone(),
            state: self.state(store),
            // The same member of the same record [`Self::to_ingest_configuration`]
            // reports, so a summary and a read answer alike.
            user_id: Some(self.user_id.clone()),
            redundant_ingest: self.redundant_ingest,
        }
    }

    /// The `state` this configuration reports right now, read against `store`.
    ///
    /// `ACTIVE` exactly while a publisher is broadcasting with this configuration, as
    /// the member's own doc states it; `INACTIVE` otherwise. Not a stored value but a
    /// fact about the stage the configuration is associated with at this instant, so it
    /// is computed from the records that hold that fact
    /// ([`ingest_configuration_publishers`]) on every read — and computed HERE, the one
    /// place, so a full read and a listing's summary cannot report two states for one
    /// configuration.
    pub fn state(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    ) -> String {
        if ingest_configuration_publishers(store, self).is_empty() {
            NO_PUBLISHER_INGEST_CONFIGURATION_STATE.to_string()
        } else {
            PUBLISHING_INGEST_CONFIGURATION_STATE.to_string()
        }
    }

    /// Leave this configuration associated with NO stage, reporting
    /// [`NO_ASSOCIATED_STAGE_ARN`] for `stageArn`.
    ///
    /// The ONE writer of the "no stage" reading, so every operation that ends an
    /// association (a publisher's disconnect, the deletion of the stage itself) leaves
    /// the same value on the record — and `stageArn` never reports a stage this store
    /// no longer holds.
    pub fn clear_stage_association(&mut self) {
        self.stage_arn = NO_ASSOCIATED_STAGE_ARN.to_string();
    }

    /// Leave this configuration with redundant ingest `redundant_ingest`, taking `minted`
    /// as its backup credentials where the configuration carries none yet and the switch
    /// turns the feature ON.
    ///
    /// The ONE writer of the pair `redundantIngest`/`redundantIngestCredentials`, so what
    /// a configuration's backup credentials are does not depend on which operation last
    /// wrote the switch. The credentials are MATERIALISED by the switch and then belong to
    /// the configuration: `IngestConfiguration$redundantIngestCredentials` is "present only
    /// when `redundantIngest` is set to `true`", which is what brings them into existence,
    /// and the recorded service keeps reporting them once minted — switching the feature off
    /// stops the redundant paths being published to, it does not revoke the ingest keys
    /// already handed out, so a publisher that cached one keeps working and a caller that
    /// switches the feature back on broadcasts to the same paths. A configuration whose
    /// switch was never on therefore carries no list at all.
    pub fn apply_redundant_ingest(
        &mut self,
        redundant_ingest: bool,
        minted: Vec<IngestCredential>,
    ) {
        self.redundant_ingest = redundant_ingest;
        if redundant_ingest && self.redundant_ingest_credentials.is_empty() {
            self.redundant_ingest_credentials = minted;
        }
    }

    /// Whether `participant_id` is one of the participants THIS configuration's ingest
    /// credentials were minted for: its own, or one of its redundant ones.
    ///
    /// The ONE test for "is this participant an identity of this configuration", so the
    /// lookup that resolves a participant of a stage ([`stage_participant`]) and the one
    /// that finds a configuration's publishers ([`ingest_configuration_publishers`])
    /// cannot drift apart.
    pub fn mints_participant(&self, participant_id: &str) -> bool {
        self.participant_id == participant_id
            || self
                .redundant_ingest_credentials
                .iter()
                .any(|credential| credential.participant_id == participant_id)
    }

    /// Whether the participant `record` broadcasts with THIS configuration: its join
    /// named this configuration, or it publishes as one of the participants this
    /// configuration's ingest credentials were minted for.
    ///
    /// The ONE predicate for "does this participant broadcast with this configuration",
    /// so the lookup that finds a configuration's publishers
    /// ([`ingest_configuration_publishers`]) and the one that finds a participant's
    /// configuration ([`participant_ingest_configuration`]) cannot drift apart.
    pub fn publishes_participant(&self, record: &ParticipantRecord) -> bool {
        record.ingest_configuration_arn.as_deref() == Some(self.arn.as_str())
            || self.mints_participant(&record.participant_id)
    }
}

/// One encoder configuration, as `CreateEncoderConfiguration` persisted it and
/// `GetEncoderConfiguration` reports it.
#[derive(Debug, Clone)]
pub struct EncoderConfigurationRecord {
    /// The encoder-configuration ARN, and the single key every lookup and delete uses.
    pub arn: String,
    /// The name the creating request gave, or [`UNNAMED_RESOURCE_NAME`] when it gave
    /// none: materialised at creation, so it is a value this record HOLDS rather than one
    /// each projection decides, and every projection reports it.
    pub name: String,
    /// The transcoding settings. Not optional: every encoder configuration has them,
    /// with the modelled defaults where the creating request supplied none
    /// ([`effective_video`]).
    pub video: Video,
    pub tags: HashMap<String, String>,
    /// Creation order, and the stable key `ListEncoderConfigurations` pages over — the
    /// same role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl EncoderConfigurationRecord {
    /// The modelled `EncoderConfiguration` this record reports.
    pub fn to_encoder_configuration(&self) -> EncoderConfiguration {
        EncoderConfiguration {
            arn: self.arn.clone(),
            // Always reported, the empty string included: the name is materialised on the
            // record ([`Self::name`]), so a caller reading it back finds a value wherever
            // the configuration has one.
            name: Some(self.name.clone()),
            video: Some(self.video.clone()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `EncoderConfigurationSummary` a page of this record reports.
    ///
    /// `EncoderConfigurationSummary` carries three of the four members
    /// `EncoderConfiguration` does — `arn`, `name` and `tags`, but not `video`
    /// (`smithy_query(query="shape",shape="EncoderConfiguration,EncoderConfigurationSummary")`)
    /// — and each is projected off this one record, so a summary and a read answer alike
    /// on every member both carry.
    pub fn to_summary(&self) -> EncoderConfigurationSummary {
        EncoderConfigurationSummary {
            arn: self.arn.clone(),
            // The same member of the same record [`Self::to_encoder_configuration`]
            // reports, so a summary and a read answer alike.
            name: Some(self.name.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One storage configuration, as `CreateStorageConfiguration` persisted it and
/// `GetStorageConfiguration` / `ListStorageConfigurations` report it.
#[derive(Debug, Clone)]
pub struct StorageConfigurationRecord {
    /// The storage-configuration ARN, and the single key every lookup uses.
    pub arn: String,
    /// The name the creating request gave, which "does not need to be unique"
    /// (`CreateStorageConfigurationRequest$name`), or `None` when it named none.
    pub name: Option<String>,
    /// The S3 bucket recorded video is written to. `@required` on the creating
    /// request, so every record has one.
    pub bucket_name: String,
    pub tags: HashMap<String, String>,
    /// When the configuration was created. Reported by no member of the modelled
    /// `StorageConfiguration`, and recorded for the same reason
    /// [`StageRecord::created_at`] is.
    pub created_at: SystemTime,
    /// Creation order, and the stable key `ListStorageConfigurations` pages over — the
    /// same role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl StorageConfigurationRecord {
    /// The modelled `StorageConfiguration` this record reports.
    pub fn to_storage_configuration(&self) -> StorageConfiguration {
        StorageConfiguration {
            arn: self.arn.clone(),
            name: self.name.clone(),
            s3: Some(self.to_s3()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `StorageConfigurationSummary` this record reports in a
    /// `ListStorageConfigurations` page.
    ///
    /// `StorageConfigurationSummary` carries the same four members as
    /// `StorageConfiguration` (`smithy_query(query="shape",
    /// shape="StorageConfiguration,StorageConfigurationSummary")`), so a summary and a
    /// read answer alike, member for member.
    pub fn to_summary(&self) -> StorageConfigurationSummary {
        StorageConfigurationSummary {
            arn: self.arn.clone(),
            name: self.name.clone(),
            s3: Some(self.to_s3()),
            tags: Some(self.tags.clone()),
        }
    }

    /// The S3 destination both projections report, built once so they cannot differ.
    fn to_s3(&self) -> S3StorageConfiguration {
        S3StorageConfiguration {
            bucket_name: self.bucket_name.clone(),
        }
    }
}

/// One public key, as `ImportPublicKey` persisted it and `GetPublicKey` reports it.
///
/// A public key is the half of a customer-generated key pair this service holds, so the
/// stage participant tokens the customer signs with the private half can be verified:
/// nothing of the private half reaches this record.
#[derive(Debug, Clone)]
pub struct PublicKeyRecord {
    /// The public-key ARN, and the single key every lookup and delete uses.
    pub arn: String,
    /// The name the importing request gave, or [`UNNAMED_RESOURCE_NAME`] when it gave
    /// none: materialised at import, so it is a value this record HOLDS rather than one
    /// each projection decides, exactly as [`EncoderConfigurationRecord::name`] is.
    pub name: String,
    /// The PEM text the import carried, reported back verbatim by `PublicKey$publicKeyMaterial`
    /// ("Public key material").
    pub public_key_material: String,
    /// The key's fingerprint, computed from its DER `SubjectPublicKeyInfo` at import by
    /// `crate::ops::import_public_key`. Stored rather than recomputed per read, so every
    /// projection of this key answers the one value — and it identifies the key MATERIAL,
    /// which is what makes a re-import of the same key a conflict.
    pub fingerprint: String,
    pub tags: HashMap<String, String>,
    /// When the key was imported. Reported by no member of the modelled `PublicKey`, and
    /// recorded for the same reason [`StageRecord::created_at`] is.
    pub created_at: SystemTime,
    /// Import order, and the stable key `ListPublicKeys` pages over — the same role
    /// [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

impl PublicKeyRecord {
    /// The modelled `PublicKey` this record reports.
    pub fn to_public_key(&self) -> PublicKey {
        PublicKey {
            arn: Some(self.arn.clone()),
            // Always reported, the empty string included: the name is materialised on the
            // record ([`Self::name`]), so a caller reading it back finds a value wherever
            // the key has one.
            name: Some(self.name.clone()),
            public_key_material: Some(self.public_key_material.clone()),
            fingerprint: Some(self.fingerprint.clone()),
            // Always reported, empty map included: `tags` is how a caller reads the tags
            // back, exactly as on a stage.
            tags: Some(self.tags.clone()),
        }
    }

    /// The modelled `PublicKeySummary` this record reports in a `ListPublicKeys` page.
    ///
    /// `PublicKeySummary` carries three of the five members `PublicKey` does — `arn`,
    /// `name` and `tags`, and neither `publicKeyMaterial` nor `fingerprint`
    /// (`smithy_query(query="shape",shape="PublicKey,PublicKeySummary")`) — and each is
    /// projected off this one record, so a summary and a read answer alike on every
    /// member both carry.
    pub fn to_summary(&self) -> PublicKeySummary {
        PublicKeySummary {
            arn: Some(self.arn.clone()),
            // The same members of the same record [`Self::to_public_key`] reports, so a
            // summary and a read answer alike.
            name: Some(self.name.clone()),
            tags: Some(self.tags.clone()),
        }
    }
}

/// One destination of a composition, as `StartComposition` persisted it.
#[derive(Debug, Clone)]
pub struct DestinationRecord {
    /// The id IVS assigns the destination (`Destination$id`), minted at start.
    pub id: String,
    /// One of `crate::ops::types::DESTINATION_STATE_VALUES`;
    /// [`DESTINATION_STATE_STARTING`] until the composition's
    /// [`CompositionRecord::active_at`].
    pub state: String,
    /// When the destination started, which is when its composition did.
    pub started_at: SystemTime,
    /// When it stopped; `None` while it is running.
    pub ended_at: Option<SystemTime>,
    /// What the starting request asked this destination to be —
    /// `Destination$configuration`, "configuration used to create this destination" —
    /// with the model's documented default for every member it left out
    /// ([`effective_destination_configuration`]).
    pub configuration: DestinationConfiguration,
    /// Where an S3 destination's recording lands; `None` for a channel destination,
    /// which records nothing. See [`recording_prefix`].
    pub detail: Option<DestinationDetail>,
}

impl DestinationRecord {
    /// Whether this destination refers to the resource `arn` of THIS endpoint's store.
    ///
    /// The members a destination configuration names such a resource with: the encoder
    /// configuration of a channel destination (`ChannelDestinationConfiguration$encoderConfigurationArn`),
    /// and the storage configuration and encoder configurations of an S3 one
    /// (`S3DestinationConfiguration$storageConfigurationArn` and `$encoderConfigurationArns`).
    /// `channelArn` is deliberately not one of them: it names a low-latency channel of
    /// the sibling endpoint, which this store does not hold — the same boundary
    /// `StartComposition` draws when it resolves these references.
    pub fn references(&self, arn: &str) -> bool {
        let channel_reference = self.configuration.channel.as_ref().is_some_and(|channel| {
            channel.encoder_configuration_arn.as_deref() == Some(arn)
        });
        let s3_reference = self.configuration.s3.as_ref().is_some_and(|s3| {
            s3.storage_configuration_arn == arn
                || s3
                    .encoder_configuration_arns
                    .iter()
                    .any(|encoder_configuration_arn| encoder_configuration_arn == arn)
        });
        channel_reference || s3_reference
    }

    /// Whether this destination has the ENCODER configuration `arn` attached: the
    /// encoder configuration of a channel destination
    /// (`ChannelDestinationConfiguration$encoderConfigurationArn`) or one of those of an
    /// S3 one (`S3DestinationConfiguration$encoderConfigurationArns`).
    ///
    /// Narrower than [`Self::references`], which answers "does this destination name that
    /// resource at all" for any resource of this store, a storage configuration included:
    /// `ListCompositions`' `filterByEncoderConfigurationArn` selects on the encoder
    /// configuration alone.
    pub fn references_encoder_configuration(&self, arn: &str) -> bool {
        let channel_reference = self
            .configuration
            .channel
            .as_ref()
            .is_some_and(|channel| channel.encoder_configuration_arn.as_deref() == Some(arn));
        let s3_reference = self.configuration.s3.as_ref().is_some_and(|s3| {
            s3.encoder_configuration_arns
                .iter()
                .any(|encoder_configuration_arn| encoder_configuration_arn == arn)
        });
        channel_reference || s3_reference
    }

    /// The modelled `Destination` this record reports.
    pub fn to_destination(&self) -> Destination {
        Destination {
            id: self.id.clone(),
            state: self.state.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
            configuration: self.configuration.clone(),
            detail: self.detail.clone(),
        }
    }

    /// The modelled `DestinationSummary` a `ListCompositions` page reports this
    /// destination as.
    ///
    /// `DestinationSummary` carries the four members `Destination` carries besides the
    /// configuration and the detail
    /// (`smithy_query(query="shape",shape="Destination,DestinationSummary")`), each read
    /// off the same field [`Self::to_destination`] reports it from, so a summary and a
    /// read answer alike.
    pub fn to_summary(&self) -> DestinationSummary {
        DestinationSummary {
            id: self.id.clone(),
            state: self.state.clone(),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// The S3 prefix an S3 destination's recording is stored under.
///
/// `S3Detail$recordingPrefix` is documented only as "The S3 bucket prefix under which
/// the recording is stored" and the model publishes no format for it, so the prefix is
/// DERIVED from the two ids that identify the recording — the composition and the
/// destination within it. That makes it unique per destination and stable across reads,
/// which is what a caller can rely on; it is not read back off any request.
pub fn recording_prefix(composition_id: &str, destination_id: &str) -> String {
    format!("{composition_id}/{destination_id}")
}

/// One composition, as `StartComposition` persisted it and `GetComposition` reports it.
#[derive(Debug, Clone)]
pub struct CompositionRecord {
    /// The composition ARN, and the single key every lookup uses.
    pub arn: String,
    /// The id the ARN ends in, which [`recording_prefix`] is built from.
    pub composition_id: String,
    /// The stage being composited.
    pub stage_arn: String,
    /// One of `crate::ops::types::COMPOSITION_STATE_VALUES`;
    /// [`COMPOSITION_STATE_STARTING`] until [`Self::active_at`].
    pub state: String,
    /// The layout the composition runs with: what the request asked for, with the
    /// model's documented default for everything it left out
    /// ([`effective_layout`]). Not optional — `Composition$layout` is `@required`.
    pub layout: LayoutConfiguration,
    /// The composition's destinations, in the order the request listed them.
    pub destinations: Vec<DestinationRecord>,
    pub tags: HashMap<String, String>,
    /// The `@idempotencyToken` the starting request carried, if any: a start offering it
    /// again names this composition rather than a second one to mint, so it is refused.
    pub idempotency_token: Option<String>,
    /// What the starting request asked for, as one comparable value — see
    /// [`composition_fingerprint`]. A repeat under [`Self::idempotency_token`] is only a
    /// repeat of this composition's own request when it asks for the same thing, which is
    /// what decides WHICH refusal a token offered again is stated with.
    pub request_fingerprint: String,
    /// When the composition started (`Composition$startTime`).
    pub started_at: SystemTime,
    /// When it stopped (`Composition$endTime`); `None` while it is running.
    pub ended_at: Option<SystemTime>,
    /// The instant the clock owes this composition the `STARTING` -> `ACTIVE`
    /// transition: its start plus [`COMPOSITION_ACTIVATION_SPAN`]. Stored, never
    /// slept on.
    pub active_at: SystemTime,
    /// The instant the clock owes this composition the stop that ends it because the
    /// stage it composites has held no participant since one
    /// [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] before it — so that the end it begins is
    /// over, resource and all, when the published [`COMPOSITION_EMPTY_STAGE_TIMEOUT`] runs
    /// out; `None` while a participant is in that stage.
    ///
    /// Written only by [`Self::observe_stage_occupancy`], through the store-wide
    /// `ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::observe_stage_occupancy`
    /// that every change to a stage's participants goes through, so what the timeout is
    /// measured from is the instant the stage was actually left empty. Stored, never
    /// slept on.
    pub empty_stage_stop_at: Option<SystemTime>,
    /// Start order, and the stable key a listing of compositions pages over.
    pub sequence: u64,
}

impl CompositionRecord {
    /// Materialise the transition the clock has already made due.
    ///
    /// A starting composition — and every destination of it — becomes `ACTIVE`
    /// [`COMPOSITION_ACTIVATION_SPAN`] after the call that started it, and a stopping one
    /// becomes `STOPPED` [`COMPOSITION_STOP_SPAN`] after the call that stopped it,
    /// whether or not anybody calls again: this only writes down a change that was
    /// already owed at `now`, so a reporting operation may call it. A composition whose
    /// stage has held no participant for a whole [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] is
    /// stopped here too, by the clock rather than by a caller ([`Self::begin_stop`] at
    /// the instant that span ran out), and walks the same steps from there. The last
    /// step a
    /// stopped composition owes — the deletion of the ephemeral resource — is not a state
    /// to write but a record to drop, which is [`Self::is_reclaimed`]'s question and
    /// `ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::materialize_due_compositions`'s
    /// to act on.
    pub fn materialize_due(&mut self, now: SystemTime) {
        // The stop the clock owes a composition whose stage has been empty for a whole
        // [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] begins AT the instant it fell due, not at
        // this call: everything the stop owes afterwards is owed from there, so a caller
        // that reads late sees exactly what one reading on time saw.
        if self.is_running() {
            if let Some(due) = self.empty_stage_stop_at.filter(|due| now >= *due) {
                self.begin_stop(due);
            }
        }
        let due = self.state_at(now);
        if due == self.state {
            return;
        }
        let due = due.to_string();
        if due == COMPOSITION_STATE_ACTIVE {
            for destination in &mut self.destinations {
                if destination.state == DESTINATION_STATE_STARTING {
                    destination.state = DESTINATION_STATE_ACTIVE.to_string();
                }
            }
        } else if due == COMPOSITION_STATE_STOPPED {
            for destination in &mut self.destinations {
                if destination.state == DESTINATION_STATE_STOPPING {
                    destination.state = DESTINATION_STATE_STOPPED.to_string();
                }
            }
        }
        self.state = due;
    }

    /// The `CompositionState` this composition is in at `now`, whether or not anybody has
    /// called since the clock made it due.
    ///
    /// A composition's position in its lifecycle is a function of the instants the start
    /// and the end recorded — [`Self::active_at`] for `STARTING` -> `ACTIVE`, one
    /// [`COMPOSITION_STOP_SPAN`] after the instant its end began ([`Self::stop_began_at`]
    /// — a caller's stop, or the empty-stage timeout running out) for `STOPPING` ->
    /// `STOPPED` — so this is what every reader asks, and the stored `state` is only where
    /// [`Self::materialize_due`] writes the answer down. A reader holding the store
    /// immutably therefore reaches the same answer as one that can write, instead of
    /// seeing a transition that is due but not yet recorded. A state the clock owes
    /// nothing from is reported as stored.
    pub fn state_at(&self, now: SystemTime) -> &str {
        // A composition whose stage has been empty for a whole
        // [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] is stopped by the clock exactly as a
        // caller's stop stops it, so both run out of the one instant the stop began
        // ([`Self::stop_began_at`]) and reach the same two states from it.
        if self.is_running() {
            if let Some(began) = self.stop_began_at(now) {
                return if now >= began + COMPOSITION_STOP_SPAN {
                    COMPOSITION_STATE_STOPPED
                } else {
                    COMPOSITION_STATE_STOPPING
                };
            }
        }
        match self.state.as_str() {
            COMPOSITION_STATE_STARTING if now >= self.active_at => COMPOSITION_STATE_ACTIVE,
            COMPOSITION_STATE_STOPPING
                if self
                    .ended_at
                    .is_some_and(|ended_at| now >= ended_at + COMPOSITION_STOP_SPAN) =>
            {
                COMPOSITION_STATE_STOPPED
            }
            state => state,
        }
    }

    /// The modelled `Composition` this record reports.
    pub fn to_composition(&self) -> Composition {
        Composition {
            arn: self.arn.clone(),
            stage_arn: self.stage_arn.clone(),
            state: self.state.clone(),
            layout: self.layout.clone(),
            destinations: self
                .destinations
                .iter()
                .map(DestinationRecord::to_destination)
                .collect(),
            // Always reported, empty map included: `tags` is how a caller reads the
            // tags back, and an absent member is not the same answer as "none" — the
            // same rule [`StageRecord::to_stage`] follows.
            tags: Some(self.tags.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
        }
    }

    /// Whether any destination of this composition has the encoder configuration `arn`
    /// attached — see [`DestinationRecord::references_encoder_configuration`].
    ///
    /// `ListCompositionsRequest$filterByEncoderConfigurationArn` filters the list "to
    /// match the specified EncoderConfiguration attached to at least one of its output",
    /// which is exactly this question.
    pub fn uses_encoder_configuration(&self, arn: &str) -> bool {
        self.destinations
            .iter()
            .any(|destination| destination.references_encoder_configuration(arn))
    }

    /// The modelled `CompositionSummary` this record reports in a `ListCompositions`
    /// page.
    ///
    /// `CompositionSummary` carries the same members as `Composition` except the layout
    /// and the per-destination configuration and detail
    /// (`smithy_query(query="shape",shape="Composition,CompositionSummary")`), so a
    /// summary and a read answer alike member for member on everything both carry.
    pub fn to_summary(&self) -> CompositionSummary {
        CompositionSummary {
            arn: self.arn.clone(),
            stage_arn: self.stage_arn.clone(),
            destinations: self
                .destinations
                .iter()
                .map(DestinationRecord::to_summary)
                .collect(),
            state: self.state.clone(),
            // Always reported, empty map included, the way [`Self::to_composition`]
            // reports it.
            tags: Some(self.tags.clone()),
            start_time: Some(::aws_smithy_types::DateTime::from(self.started_at)),
            end_time: self.ended_at.map(::aws_smithy_types::DateTime::from),
        }
    }
}

// ---------------------------------------------------------------------------
// Stopping a composition
//
// `StopComposition` ends a composition `StartComposition` started, and so does the clock
// when the stage a composition composites is left with nobody in it. A composition is
// EPHEMERAL: the model's `StartComposition` doc says it "is an ephemeral resource that
// exists after this operation returns successfully" and that the "Composition stops and
// the resource is deleted … When StopComposition is called", "After a 1-minute timeout,
// when all participants are disconnected from the stage" and "After a 1-minute timeout,
// if there are no participants in the stage when StartComposition is called". So an end
// is the start of the composition's end, whichever of those began it — the clock owes it
// `STOPPED` one span later and the deletion of the resource one retention span after
// that, both
// materialised by [`CompositionRecord::materialize_due`] beside the `STARTING` ->
// `ACTIVE` transition a start owes, and neither ever slept on.
// ---------------------------------------------------------------------------

/// `CompositionState` a composition is in from the moment `StopComposition` returns until
/// the clock owes it [`COMPOSITION_STOP_SPAN`] later, from `CompositionState`'s value
/// list (`crate::ops::types::COMPOSITION_STATE_VALUES`).
pub const COMPOSITION_STATE_STOPPING: &str = "STOPPING";

/// `CompositionState` a stopping composition settles into, from the same value list.
/// Terminal: the ephemeral resource is reclaimed from this state and reaches no other.
pub const COMPOSITION_STATE_STOPPED: &str = "STOPPED";

/// `DestinationState` a destination of a stopping composition is in, from
/// `DestinationState`'s value list (`crate::ops::types::DESTINATION_STATE_VALUES`).
pub const DESTINATION_STATE_STOPPING: &str = "STOPPING";

/// `DestinationState` a destination settles into with the composition it belongs to.
pub const DESTINATION_STATE_STOPPED: &str = "STOPPED";

/// How long after `StopComposition` the clock owes a composition and its destinations
/// the `STOPPING` -> `STOPPED` transition. The deletion of the ephemeral resource that
/// follows is one [`COMPOSITION_RETENTION_SPAN`] later, not one of these.
///
/// AWS publishes no span for it, so this is the emulator's own, inside the band
/// this tree fixes for a transition the clock owes and set the same way
/// [`COMPOSITION_ACTIVATION_SPAN`] is. It is deliberately NOT zero: a caller that reads
/// the composition back straight after stopping it must still observe `STOPPING`, the way
/// it would against AWS.
pub const COMPOSITION_STOP_SPAN: Duration = Duration::from_millis(500);

/// How long a composition that has reached [`COMPOSITION_STATE_STOPPED`] goes on being
/// reported before the clock deletes the ephemeral resource.
///
/// The model says the "Composition stops and the resource is deleted" but publishes no
/// span between the two, and the stop is what a caller WAITS for: the documented way to
/// learn that a broadcast is over — and when it ended, which `Composition$endTime` is the
/// only report of — is to read the composition back until it answers with its terminal
/// state. A record that outlived its stop by less than the interval such a caller reads
/// at would never be seen in that state at all: the read before it would answer `STOPPING`
/// and the one after it the modelled `ResourceNotFoundException`, which is also the answer
/// for a composition that never existed. So the terminal state is kept for a span measured
/// in SECONDS, long enough that several reads of an ordinary poll loop land inside it,
/// after which the ephemeral resource is deleted rather than kept for ever.
pub const COMPOSITION_RETENTION_SPAN: Duration = Duration::from_secs(12);

/// How long after the stage it composites is left without participants a composition still
/// EXISTS — the span after which it has stopped and its ephemeral resource is gone.
///
/// PUBLISHED, so it keeps the value AWS states rather than a span of this tree's
/// choosing: `StartComposition` documents that a "Composition stops and the resource is
/// deleted … After a 1-minute timeout, when all participants are disconnected from the
/// stage. After a 1-minute timeout, if there are no participants in the stage when
/// StartComposition is called." Both clauses are the same timeout on the same condition —
/// the stage has nobody in it — so one span serves both, measured from the instant the
/// stage was left empty or, for a composition started against an empty stage, from the
/// start.
///
/// What the minute bounds is the whole end the doc states, the stop AND the deletion, so
/// the end BEGINS one [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] in rather than when the minute
/// runs out: a caller watching a composition nobody publishes to sees it reach `STOPPED`
/// with its end time while the minute is still running, and finds the resource gone once
/// it has run out.
pub const COMPOSITION_EMPTY_STAGE_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a composition goes on composing while the stage it composites holds no
/// participant, before the clock owes it the stop that ends it.
///
/// The published [`COMPOSITION_EMPTY_STAGE_TIMEOUT`] is what a composition of an empty
/// stage has left, and an end takes time the doc gives no span for: `STOPPING` for one
/// [`COMPOSITION_STOP_SPAN`], then `STOPPED` for one [`COMPOSITION_RETENTION_SPAN`], and
/// only then the deletion the same sentence states. Begin that end when the minute runs
/// out and the minute would bound only its first step, leaving the terminal state readable
/// for a span no caller can observe; so it begins one whole end before the minute is up,
/// which is what this span is. The composition therefore stops inside the published
/// timeout and is deleted when it expires, both — as the doc has them — "after a 1-minute
/// timeout".
pub const COMPOSITION_EMPTY_STAGE_STOP_SPAN: Duration = COMPOSITION_EMPTY_STAGE_TIMEOUT
    .saturating_sub(COMPOSITION_RETENTION_SPAN)
    .saturating_sub(COMPOSITION_STOP_SPAN);

// The end has to FIT inside the published timeout for that derivation to mean anything: a
// subtraction that saturated would stop a composition the instant its stage fell empty,
// which is neither what the doc states nor a span a caller could observe the composing
// state in. Changing one of the three spans so it no longer fits is therefore a compile
// error rather than a silent change of behaviour.
const _: () = assert!(!COMPOSITION_EMPTY_STAGE_STOP_SPAN.is_zero());

impl CompositionRecord {
    /// Whether this composition is still composing, so a stop is the next thing that can
    /// happen to it.
    ///
    /// The two states a composition reaches on its way up ([`COMPOSITION_STATE_STARTING`]
    /// and [`COMPOSITION_STATE_ACTIVE`]); from any other one the broadcast is already
    /// ending or has ended, which is what makes a second stop the modelled conflict.
    ///
    /// Asked of the stored state, because the only transitions the clock owes are between
    /// two states this answers the same for ([`Self::state_at`] moves `STARTING` to
    /// `ACTIVE` and `STOPPING` to `STOPPED`): whether the due transition has been written
    /// down yet cannot change the answer.
    pub fn is_running(&self) -> bool {
        self.state == COMPOSITION_STATE_STARTING || self.state == COMPOSITION_STATE_ACTIVE
    }

    /// Whether this composition is still using the resources its destinations name, so a
    /// delete of one of them would leave it composing against a resource that is gone.
    ///
    /// Every state on this side of a FINISHED stop: the model tells a caller that wants
    /// to delete such a resource to "first use StopComposition and wait for it to
    /// complete", so a composition that is starting, composing or still stopping is using
    /// them, and one whose stop has completed is not.
    pub fn is_using_resources(&self) -> bool {
        self.is_running() || self.state == COMPOSITION_STATE_STOPPING
    }

    /// Whether any destination of this composition refers to the resource `arn` — see
    /// [`DestinationRecord::references`].
    pub fn references(&self, arn: &str) -> bool {
        self.destinations
            .iter()
            .any(|destination| destination.references(arn))
    }

    /// Begin the stop of this composition at `now`: the composition and every destination
    /// it broadcasts to enter their stopping state and record when the broadcast ended.
    ///
    /// The one place a composition's end begins, for both of the ways the model says it
    /// does: a caller's `StopComposition`, and the clock running out the
    /// [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] of a composition whose stage holds nobody
    /// ([`Self::materialize_due`], which passes the instant the timeout ran out).
    ///
    /// Everything after that is the clock's ([`Self::materialize_due`]): `STOPPED` one
    /// [`COMPOSITION_STOP_SPAN`] later, and the deletion of the ephemeral resource one
    /// [`COMPOSITION_RETENTION_SPAN`] after THAT.
    pub fn begin_stop(&mut self, now: SystemTime) {
        self.state = COMPOSITION_STATE_STOPPING.to_string();
        self.ended_at = Some(now);
        for destination in &mut self.destinations {
            destination.state = DESTINATION_STATE_STOPPING.to_string();
            destination.ended_at = Some(now);
        }
    }

    /// Whether the clock has reclaimed this composition's ephemeral resource by `now`, so
    /// nothing may report it any more.
    ///
    /// Due one [`COMPOSITION_RETENTION_SPAN`] after the stop FINISHED — which is that span
    /// plus one [`COMPOSITION_STOP_SPAN`] after the stop began — computed from the instant
    /// that end began ([`Self::stop_began_at`]) and [`Self::state_at`] rather than from the
    /// stored state, so a reader that holds the record immutably reaches the same answer as
    /// one that can write ([`Self::materialize_due`] is what removes it).
    ///
    /// Both ways a composition's end begins — a caller's `StopComposition` and the clock
    /// running out [`COMPOSITION_EMPTY_STAGE_STOP_SPAN`] on a stage holding nobody — are
    /// answered from that one instant, so a stop nobody called for is readable as
    /// `STOPPED`, with its end time, for exactly as long as one a caller asked for.
    pub fn is_reclaimed(&self, now: SystemTime) -> bool {
        self.stopped_at(now)
            .is_some_and(|stopped| now >= stopped + COMPOSITION_RETENTION_SPAN)
    }

    /// The instant this composition's stop FINISHED, as of `now`: one
    /// [`COMPOSITION_STOP_SPAN`] after the end began ([`Self::stop_began_at`]), and `None`
    /// until the clock has reached it — so this answers "is this composition in its
    /// terminal state, and since when".
    fn stopped_at(&self, now: SystemTime) -> Option<SystemTime> {
        if self.state_at(now) != COMPOSITION_STATE_STOPPED {
            return None;
        }
        self.stop_began_at(now)
            .map(|began| began + COMPOSITION_STOP_SPAN)
    }

    /// The instant this composition's broadcast ends, as of `now`: the instant a stop
    /// recorded ([`Self::begin_stop`]), or — for a composition no caller stopped — the
    /// empty-stage deadline once the clock has reached it.
    ///
    /// `None` while the composition is still composing and the clock owes it no stop, so
    /// this is the one question "has this composition's end begun, and when" that both
    /// ends of the lifecycle are answered from.
    fn stop_began_at(&self, now: SystemTime) -> Option<SystemTime> {
        self.ended_at
            .or_else(|| self.empty_stage_stop_at.filter(|due| now >= *due))
    }

    /// Record what the stage this composition composites holds at `now`: a participant in
    /// it keeps the composition composing, and a stage left without one puts the
    /// composition on the published [`COMPOSITION_EMPTY_STAGE_TIMEOUT`], one whole end
    /// ([`COMPOSITION_EMPTY_STAGE_STOP_SPAN`]) into which the clock owes it the stop that
    /// ends it.
    ///
    /// Idempotent while the stage stays empty — the deadline is measured from the instant
    /// the stage was LEFT empty, so observing the same emptiness again does not push it
    /// back — and it says nothing about a composition that is no longer composing: one
    /// already ending has the instant of its end recorded.
    pub fn observe_stage_occupancy(&mut self, occupied: bool, now: SystemTime) {
        if !self.is_running() {
            return;
        }
        if occupied {
            self.empty_stage_stop_at = None;
        } else if self.empty_stage_stop_at.is_none() {
            self.empty_stage_stop_at = Some(now + COMPOSITION_EMPTY_STAGE_STOP_SPAN);
        }
    }
}

// ---------------------------------------------------------------------------
// Stage sessions and the participants in them
//
// A participant reaches a stage through the stage's DATA plane — it joins over WebRTC
// with the participant token it was issued, or publishes to the WHIP/RTMP endpoints
// `CreateStage` advertises. No operation of this control plane makes a participant
// join: the sibling `ivs` emulator records the same thing about a channel going live
// (`is_broadcasting`), and this service already records it about a publisher
// ([`NO_PUBLISHER_INGEST_CONFIGURATION_STATE`]). So the records below are written by
// [`join_stage_session`] / [`leave_stage_session`] — the one entry point a stage data
// plane uses, and the one place the modelled defaults of a joining participant are
// applied — over the session [`ensure_active_session`] opens, and by the one control-plane
// call that does put a participant into a stage, a replication starting
// ([`record_replicated_participant`], which goes through that same entry point); and the
// reporting operations (`GetParticipant`, `ListParticipants`, `ListStageSessions` and
// `GetStageSession`) answer from exactly what those recorded, never from a value
// invented for a join that never happened.
// ---------------------------------------------------------------------------

/// `ParticipantState` of a participant that is in the stage right now.
///
/// One of the two values the modelled `ParticipantState` enum carries
/// (`smithy_query(query="shape",shape="ParticipantState")`), and the state a
/// participant is in from the moment it joins.
pub const PARTICIPANT_STATE_CONNECTED: &str = "CONNECTED";

/// `ParticipantState` of a participant that has left the stage, the other value that
/// enum carries. A participant that leaves is not forgotten: the model documents
/// `ParticipantSummary$firstJoinTime` as "when the participant FIRST joined the stage
/// session" and `published` as "whether the participant EVER published", so the
/// session keeps the record and moves its state.
pub const PARTICIPANT_STATE_DISCONNECTED: &str = "DISCONNECTED";

/// `ParticipantRecordingState` of a participant nothing is recording.
///
/// `DISABLED` is the value the modelled `ParticipantRecordingState` carries for
/// exactly this (and the only one of its values that is NOT a
/// `ParticipantRecordingFilterByRecordingState`, which lists the five states a
/// recording passes through). It is what a joining participant gets unless the
/// recorder reports otherwise: this emulator writes no participant media — it serves
/// no S3 data plane — so a state out of that five would name work nothing ran.
pub const NOT_RECORDED_PARTICIPANT_RECORDING_STATE: &str = "DISABLED";

/// The `ParticipantRecordingState` values a recording that has ENDED carries.
///
/// `ParticipantRecordingState` carries six values
/// (`smithy_query(query="shape",shape="ParticipantRecordingState")`):
/// [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`], which is nothing recording the
/// participant at all, and the five states a recording passes through (the values of
/// `ParticipantRecordingFilterByRecordingState`). These two are the ones a recording
/// comes to REST in — it stopped, or it failed — and the three that remain (`STARTING`,
/// `ACTIVE`, `STOPPING`) are the ones media is still being written in.
pub const FINISHED_PARTICIPANT_RECORDING_STATES: [&str; 2] = ["STOPPED", "FAILED"];

/// Whether `recording_state` is a state of a recording that is RUNNING right now.
///
/// The one reading of "recording is active" for a participant: neither "nothing records
/// it" ([`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`]) nor a recording that has come to
/// rest ([`FINISHED_PARTICIPANT_RECORDING_STATES`]) is a running recording; every other
/// value of the enum is one.
pub fn participant_recording_running(recording_state: &str) -> bool {
    recording_state != NOT_RECORDED_PARTICIPANT_RECORDING_STATE
        && !FINISHED_PARTICIPANT_RECORDING_STATES.contains(&recording_state)
}

/// `ReplicationType` of a participant that is neither replicated to another stage nor
/// a replica from one.
///
/// The model documents it on `ParticipantSummary$replicationType`: "Indicates if the
/// participant has been replicated to another stage or is a replica from another
/// stage. Default: `NONE`." `StartParticipantReplication` is what moves a participant
/// off it, which [`session_participant_replications`] reads back.
pub const REPLICATION_TYPE_NONE: &str = "NONE";

/// `ReplicationType` of a participant that is being replicated INTO another stage: the
/// publishing participant a replication names, as the session it publishes in reports
/// it.
///
/// One of the three values the modelled `ReplicationType` carries
/// (`smithy_query(query="shape",shape="ReplicationType")`), and the "has been
/// replicated to another stage" half of what the member indicates.
pub const REPLICATION_TYPE_SOURCE: &str = "SOURCE";

/// `ReplicationType` of a participant that IS a replica from another stage: the record
/// the DESTINATION stage's session holds for a replicated publisher.
///
/// The third value of that enum, the "is a replica from another stage" half of the
/// member, and the only one whose `sourceStageArn` / `sourceSessionId` the model reports
/// ("if `replicationType` is `REPLICA`").
pub const REPLICATION_TYPE_REPLICA: &str = "REPLICA";

/// How one participant of one session takes part in a replication right now: the
/// `replicationType` it reports, that replication's own state, and — for a replica — the
/// stage and session it is replicated from.
///
/// Held by no [`ParticipantRecord`]: a replication is stored once, as the
/// [`ParticipantReplicaRecord`] that started it, and these four members are read off it
/// ([`session_participant_replications`]), so a participant cannot report a replication this store
/// no longer holds and two readers cannot disagree about one replication.
#[derive(Debug, Clone)]
pub struct ParticipantReplication {
    /// [`REPLICATION_TYPE_NONE`], [`REPLICATION_TYPE_SOURCE`] or
    /// [`REPLICATION_TYPE_REPLICA`].
    pub replication_type: String,
    /// The replication's own state ([`REPLICATION_STATE_ACTIVE`] / `STOPPED`), absent
    /// when no replication names this participant.
    pub replication_state: Option<String>,
    /// Where a replica is replicated from, both reported only "if `replicationType` is
    /// `REPLICA`", as the two members' docs state.
    pub source_stage_arn: Option<String>,
    pub source_session_id: Option<String>,
}

impl Default for ParticipantReplication {
    /// What a participant no replication names reports: the default the model documents
    /// on `replicationType` ("Default: `NONE`"), with the three members that describe a
    /// replication absent.
    fn default() -> Self {
        Self {
            replication_type: REPLICATION_TYPE_NONE.to_string(),
            replication_state: None,
            source_stage_arn: None,
            source_session_id: None,
        }
    }
}

/// `ParticipantProtocol` of a participant this store holds no broadcast protocol for.
///
/// One of the four values the modelled `ParticipantProtocol` enum carries
/// (`smithy_query(query="shape",shape="ParticipantProtocol")`), and the only one that is
/// not a protocol: `Participant$protocol` is the "type of ingest protocol that the
/// participant employs for broadcasting", so a participant that broadcasts nothing — and
/// one whose join named no protocol and used no ingest configuration — carries this.
pub const PARTICIPANT_PROTOCOL_UNKNOWN: &str = "UNKNOWN";

/// What `Participant$recordingS3BucketName` and `$recordingS3Prefix` report for a
/// participant that is not being recorded.
///
/// Both members document exactly this value: the bucket and prefix the participant is
/// being recorded to "if individual participant recording is enabled, or `\"\"` (empty
/// string), if recording is not enabled".
pub const NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION: &str = "";

/// One participant of one stage session, as the stage's data plane recorded it
/// joining ([`join_stage_session`]) and `ListParticipants` reports it.
#[derive(Debug, Clone)]
pub struct ParticipantRecord {
    /// The participant id it joined under: the one its participant token carries, or
    /// the one its ingest credential publishes as.
    pub participant_id: String,
    /// The customer-assigned name the token carried, when it carried one.
    pub user_id: Option<String>,
    /// The attributes the join carried: the ones its token was issued with, or — for a
    /// replica — the ones the replication merged for it. No member of
    /// `ParticipantSummary` reports them — `Participant$attributes` does, which
    /// `GetParticipant` reports — so they are recorded here where the join supplies
    /// them, and a later join of the same participant records its own
    /// ([`join_stage_session`]).
    pub attributes: HashMap<String, String>,
    /// [`PARTICIPANT_STATE_CONNECTED`] or [`PARTICIPANT_STATE_DISCONNECTED`].
    pub state: String,
    /// When this participant FIRST joined this session; unchanged by a later rejoin,
    /// as the member's own doc requires.
    pub first_join_time: SystemTime,
    /// Whether it EVER published to the session, so it stays set once set.
    pub published: bool,
    /// What the recorder reports for it; [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`]
    /// when nothing records it.
    pub recording_state: String,
    /// Whether the publisher reached the stage over redundant ingest.
    pub redundant_ingest: bool,
    /// The ingest configuration the join NAMED, when it named one. `None` leaves
    /// [`participant_ingest_configuration`] to resolve the configuration whose ingest
    /// credentials were minted for this participant, which is what the reported
    /// `ingestConfigurationArn` ([`participant_ingest_configuration_arn`]) and `protocol`
    /// ([`participant_protocol`]) are both read from.
    pub ingest_configuration_arn: Option<String>,
    /// What the stage's data plane observed about the joining client. No member of
    /// `ParticipantSummary` reports any of it; `Participant`'s client members do, which
    /// is why it is recorded here where the join observes it.
    pub client: ParticipantClient,
    /// The broadcast protocol the data plane saw this participant publish with, when it
    /// saw one. `None` leaves [`participant_protocol`] to resolve it from the ingest
    /// configuration the participant publishes through, and finally to
    /// [`PARTICIPANT_PROTOCOL_UNKNOWN`].
    pub protocol: Option<String>,
    /// Join order within the session, and the stable key `ListParticipants` pages
    /// over — the same role [`StageRecord::sequence`] plays for `ListStages`.
    pub sequence: u64,
}

/// What a stage's data plane observed about the client one participant joined with.
///
/// The six members `Participant` carries about the joining client — its Internet
/// Service Provider, operating system, browser and SDK version — each of which the model
/// targets at `ParticipantClientAttribute` and none of which any request of this control
/// plane supplies. A member the data plane did not observe stays `None`, which is how the
/// model says "not known": the alternative would be a value nothing reported.
#[derive(Debug, Default, Clone)]
pub struct ParticipantClient {
    pub isp_name: Option<String>,
    pub os_name: Option<String>,
    pub os_version: Option<String>,
    pub browser_name: Option<String>,
    pub browser_version: Option<String>,
    pub sdk_version: Option<String>,
}

impl ParticipantClient {
    /// Take over every member `observed` names, keeping what an earlier connection
    /// observed for the ones it does not.
    ///
    /// The ONE rule for "this connection observed nothing about that", applied per member
    /// and shared with the rest of a rejoin ([`join_stage_session`]): an absent member
    /// means the data plane did not observe it on this connection, never that an earlier
    /// observation is withdrawn — exactly as an absent `ingestConfigurationArn` or
    /// recording state leaves the record's own value standing.
    pub fn observe(&mut self, observed: &ParticipantClient) {
        for (member, value) in [
            (&mut self.isp_name, &observed.isp_name),
            (&mut self.os_name, &observed.os_name),
            (&mut self.os_version, &observed.os_version),
            (&mut self.browser_name, &observed.browser_name),
            (&mut self.browser_version, &observed.browser_version),
            (&mut self.sdk_version, &observed.sdk_version),
        ] {
            if value.is_some() {
                *member = value.clone();
            }
        }
    }
}

/// Where one participant's individual recording is written: the S3 bucket and the prefix
/// under it, both empty for a participant that is not being recorded.
#[derive(Debug, Clone)]
pub struct ParticipantRecordingLocation {
    pub bucket_name: String,
    pub prefix: String,
}

impl ParticipantRecordingLocation {
    /// What both members report when nothing records the participant — see
    /// [`NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION`].
    pub fn not_recorded() -> Self {
        Self {
            bucket_name: NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION.to_string(),
            prefix: NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION.to_string(),
        }
    }
}

/// The S3 prefix one participant's individual recording is stored under.
///
/// `Participant$recordingS3Prefix` is documented only as the "S3 prefix of the S3 bucket
/// where the participant is being recorded" and the model publishes no format for it
/// beyond `@pattern ^[a-zA-Z0-9-]*$` (which admits no `/`), so the prefix is DERIVED from
/// the two ids that identify the recording — the session and the participant within it,
/// both minted by this service from [`mint_id`]'s id characters. That makes it unique per
/// participant and stable across reads, which is what a caller can rely on, and it keeps
/// a publisher that disconnects and reconnects on the same prefix within the session, as
/// the member's merge note describes; it is not read back off any request.
///
/// Every id character outside the member's own pattern is rendered as `-`, so the prefix
/// satisfies it whatever the ids carry: `ParticipantId` admits `_`, which
/// `recordingS3Prefix` does not, and the data plane names the participant that joined.
///
/// The companion of [`recording_prefix`], which does the same for a composition's S3
/// destination under its own member's pattern.
pub fn participant_recording_prefix(session_id: &str, participant_id: &str) -> String {
    format!("{session_id}-{participant_id}")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character
            } else {
                '-'
            }
        })
        .collect()
}

/// Where the participant `record` of the session `session_id` of `stage_arn` is being
/// recorded, or [`ParticipantRecordingLocation::not_recorded`] when nothing records it.
///
/// Both members report their value only "if individual participant recording is
/// enabled", and the stage's `autoParticipantRecordingConfiguration` states every
/// condition of that, each on one of its members. Recording has somewhere to go:
/// `storageConfigurationArn` names the storage configuration the media is written to, and
/// [`NO_STORAGE_CONFIGURATION_ARN`] is that member's documented "no storage configuration
/// is specified". And the configuration does not EXCLUDE this participant:
/// `recordParticipantReplicas` is "an optional field to disable replica participant
/// recording. If this is set to `false` when a participant is a replica, replica
/// participants are not recorded" — so a participant this session holds as a
/// [`REPLICATION_TYPE_REPLICA`] ([`session_participant_is_replica`], the one answer the
/// reported `replicationType` is read from too) is recorded nowhere on a stage whose
/// configuration turns replica recording off, whatever storage it names.
///
/// Beside the stage's own configuration it is the RECORDER that reports whether this
/// participant is being recorded, which [`ParticipantRecord::recording_state`] holds:
/// [`NOT_RECORDED_PARTICIPANT_RECORDING_STATE`] is the one value of
/// `ParticipantRecordingState` that is not a state of a running recording, so a
/// participant carrying it is recorded nowhere even on a stage that configures storage.
/// The two members answer the empty string exactly when one of those conditions fails.
///
/// The bucket is then the one that storage configuration holds
/// ([`StorageConfigurationRecord::bucket_name`]) and the prefix is
/// [`participant_recording_prefix`] — both derived from the stored records on every read,
/// the way an ingest configuration's `state` is, so a stage whose recording configuration
/// changes reports the change at once.
pub fn participant_recording_location(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    session_id: &str,
    record: &ParticipantRecord,
) -> ParticipantRecordingLocation {
    if record.recording_state == NOT_RECORDED_PARTICIPANT_RECORDING_STATE {
        return ParticipantRecordingLocation::not_recorded();
    }
    let Some(configuration) = state
        .stages
        .get(stage_arn)
        .map(|stage| &stage.auto_participant_recording_configuration)
    else {
        return ParticipantRecordingLocation::not_recorded();
    };
    if !records_participant_replicas(configuration)
        && session_participant_is_replica(state, stage_arn, session_id, &record.participant_id)
    {
        return ParticipantRecordingLocation::not_recorded();
    }
    let storage = Some(configuration.storage_configuration_arn.as_str())
        .filter(|arn| *arn != NO_STORAGE_CONFIGURATION_ARN)
        .and_then(|arn| state.storage_configurations.get(arn));
    match storage {
        Some(storage) => ParticipantRecordingLocation {
            bucket_name: storage.bucket_name.clone(),
            prefix: participant_recording_prefix(session_id, &record.participant_id),
        },
        None => ParticipantRecordingLocation::not_recorded(),
    }
}

/// One participant of a stage whose individual recording is running right now: the
/// session it is in, the participant it is, and the recording state the recorder reports
/// for it.
#[derive(Debug, Clone)]
pub struct RecordingParticipant {
    pub session_id: String,
    pub participant_id: String,
    /// The `ParticipantRecordingState` the recorder reports — one of the values
    /// [`participant_recording_running`] admits.
    pub recording_state: String,
}

/// The participants of `stage_arn` whose individual recording is running right now, in
/// the order the sessions of the stage hold them.
///
/// The ONE lookup for "is a recording running in this stage", so what a read of a
/// participant reports about its recording and what an update refuses over cannot
/// disagree: a recording is running for a participant that is still
/// [`PARTICIPANT_STATE_CONNECTED`], whose recorder state is a running one
/// ([`participant_recording_running`]), and whose media has somewhere to go that the
/// stage's `autoParticipantRecordingConfiguration` does not exclude it from — which is
/// exactly when [`participant_recording_location`] reports a bucket for it: the
/// configuration names a storage configuration this store holds ("Individual participant
/// recording cannot be started unless a storage configuration is specified", as
/// `AutoParticipantRecordingConfiguration$storageConfigurationArn` states) and, for a
/// participant that is a replica, it has not turned replica recording off
/// ([`records_participant_replicas`]).
pub fn stage_recording_participants(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
) -> Vec<RecordingParticipant> {
    let mut recording = Vec::new();
    let Some(sessions) = state.stage_sessions.get(stage_arn) else {
        return recording;
    };
    for session in sessions {
        for participant in &session.participants {
            if participant.state != PARTICIPANT_STATE_CONNECTED
                || !participant_recording_running(&participant.recording_state)
            {
                continue;
            }
            let location =
                participant_recording_location(state, stage_arn, &session.session_id, participant);
            if location.bucket_name == NOT_RECORDED_PARTICIPANT_RECORDING_S3_LOCATION {
                continue;
            }
            recording.push(RecordingParticipant {
                session_id: session.session_id.clone(),
                participant_id: participant.participant_id.clone(),
                recording_state: participant.recording_state.clone(),
            });
        }
    }
    recording
}

/// The ingest configuration the participant `record` of `stage_arn` broadcasts with, or
/// `None` when it broadcasts with none.
///
/// The one its join named, and otherwise one of `stage_arn`'s configurations whose ingest
/// credentials were minted for that participant — the one predicate
/// [`IngestConfigurationRecord::publishes_participant`] states, which
/// [`ingest_configuration_publishers`] answers in the other direction, so what a read of
/// a participant says about its ingest and what a read of the configuration says about
/// its publishers cannot disagree.
pub fn participant_ingest_configuration<'a>(
    state: &'a ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    record: &ParticipantRecord,
) -> Option<&'a IngestConfigurationRecord> {
    if let Some(arn) = record.ingest_configuration_arn.as_deref() {
        if let Some(configuration) = state.ingest_configurations.get(arn) {
            return Some(configuration);
        }
    }
    // By creation order, not by however the store happens to hold the configurations: a
    // read of one participant must not report one configuration's protocol now and
    // another's on the next call.
    state
        .ingest_configurations
        .values()
        .filter(|configuration| {
            configuration.stage_arn == stage_arn && configuration.publishes_participant(record)
        })
        .min_by_key(|configuration| configuration.sequence)
}

/// The ARN `Participant$ingestConfigurationArn` reports for the participant `record` of
/// `stage_arn` — "the participant's ingest configuration" — or `None` when it broadcasts
/// with none.
///
/// Read off the configuration [`participant_ingest_configuration`] resolves, the one
/// answer to "which configuration does this participant broadcast with", rather than off
/// the join's own member: a publisher that reached the stage on a configuration's ingest
/// credential named no ARN when it joined, yet broadcasts with that configuration — which
/// is what the read in the other direction ([`ingest_configuration_publishers`]) reports
/// of it. So the ARN a participant reports and the protocol it reports
/// ([`participant_protocol`]) name ONE configuration, and neither contradicts what a read
/// of that configuration says about its publishers.
pub fn participant_ingest_configuration_arn(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    record: &ParticipantRecord,
) -> Option<String> {
    participant_ingest_configuration(state, stage_arn, record)
        .map(|configuration| configuration.arn.clone())
}

/// The `ParticipantProtocol` the participant `record` of `stage_arn` broadcasts with.
///
/// `Participant$protocol` is the "type of ingest protocol that the participant employs
/// for broadcasting", so it is the protocol of whatever the participant publishes
/// through: the one the data plane observed, else the `ingestProtocol` of the ingest
/// configuration it broadcasts with ([`participant_ingest_configuration`]) — `RTMP` or
/// `RTMPS`, the two values that member carries — and [`PARTICIPANT_PROTOCOL_UNKNOWN`]
/// when this store holds neither.
pub fn participant_protocol(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    record: &ParticipantRecord,
) -> String {
    if let Some(protocol) = record.protocol.as_deref() {
        return protocol.to_string();
    }
    participant_ingest_configuration(state, stage_arn, record)
        .map(|configuration| configuration.ingest_protocol.clone())
        .unwrap_or_else(|| PARTICIPANT_PROTOCOL_UNKNOWN.to_string())
}

impl ParticipantRecord {
    /// The modelled `ParticipantSummary` this record reports in a `ListParticipants`
    /// page.
    ///
    /// `replication` is what the replications this store holds say about this participant
    /// of this session ([`session_participant_replications`]): the four replication members are a
    /// fact about a replication, not about the join that recorded this participant, so
    /// they are reported from the replication itself on every read. `ingestConfigurationArn`
    /// is likewise a fact about the configurations `store` holds for `stage_arn`
    /// ([`participant_ingest_configuration_arn`]), computed on every read exactly as the
    /// full [`Self::to_participant`] computes it.
    pub fn to_summary(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
        stage_arn: &str,
        replication: &ParticipantReplication,
    ) -> ParticipantSummary {
        ParticipantSummary {
            participant_id: Some(self.participant_id.clone()),
            // Reported only when the token named one: the member is optional, and an
            // absent member is how the model says "no user id", exactly as on a
            // participant token.
            user_id: self.user_id.clone(),
            state: Some(self.state.clone()),
            first_join_time: Some(::aws_smithy_types::DateTime::from(self.first_join_time)),
            published: self.published,
            recording_state: Some(self.recording_state.clone()),
            replication_type: Some(replication.replication_type.clone()),
            replication_state: replication.replication_state.clone(),
            source_stage_arn: replication.source_stage_arn.clone(),
            source_session_id: replication.source_session_id.clone(),
            redundant_ingest: self.redundant_ingest,
            ingest_configuration_arn: participant_ingest_configuration_arn(
                store, stage_arn, self,
            ),
        }
    }

    /// The modelled `Participant` this record reports — what `GetParticipant` answers,
    /// read against `store` for the session `session_id` of `stage_arn`.
    ///
    /// Every member `ParticipantSummary` also carries is reported exactly as
    /// [`Self::to_summary`] reports it, so a read of one participant and the page it
    /// appears in cannot disagree about it. The members only `Participant` carries come
    /// from the same record — the attributes the join supplied and what it observed about
    /// the client — except the two that are not stored values but facts about the stage
    /// right now: where the participant's media is being recorded
    /// ([`participant_recording_location`]), the ingest configuration it broadcasts with
    /// ([`participant_ingest_configuration_arn`]) and the protocol that configuration
    /// ingests ([`participant_protocol`]), each computed on every read — and the four
    /// replication members, read off the replications the store holds
    /// ([`session_participant_replications`]) exactly as a summary's are.
    pub fn to_participant(
        &self,
        store: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
        stage_arn: &str,
        session_id: &str,
    ) -> Participant {
        let recording = participant_recording_location(store, stage_arn, session_id, self);
        let replication = session_participant_replications(store, stage_arn, session_id)
            .remove(&self.participant_id)
            .unwrap_or_default();
        Participant {
            participant_id: Some(self.participant_id.clone()),
            // Reported only when the token named one, exactly as on a summary.
            user_id: self.user_id.clone(),
            state: Some(self.state.clone()),
            first_join_time: Some(::aws_smithy_types::DateTime::from(self.first_join_time)),
            // Reported only when the participant's identity carried attributes, the same
            // way a participant token's and an ingest configuration's are: the member is
            // optional, and an absent member is how the model says "none".
            attributes: (!self.attributes.is_empty()).then(|| self.attributes.clone()),
            published: self.published,
            isp_name: self.client.isp_name.clone(),
            os_name: self.client.os_name.clone(),
            os_version: self.client.os_version.clone(),
            browser_name: self.client.browser_name.clone(),
            browser_version: self.client.browser_version.clone(),
            sdk_version: self.client.sdk_version.clone(),
            // Always reported, the empty value included: both members document that
            // empty string as what a participant nothing records carries.
            recording_s3_bucket_name: Some(recording.bucket_name),
            recording_s3_prefix: Some(recording.prefix),
            recording_state: Some(self.recording_state.clone()),
            protocol: Some(participant_protocol(store, stage_arn, self)),
            replication_type: Some(replication.replication_type),
            replication_state: replication.replication_state,
            source_stage_arn: replication.source_stage_arn,
            source_session_id: replication.source_session_id,
            redundant_ingest: self.redundant_ingest,
            ingest_configuration_arn: participant_ingest_configuration_arn(
                store, stage_arn, self,
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// The occurrences of a stage session
//
// The model's `Event` doc fixes what one is: "An occurrence during a stage session." So
// an event is recorded where the occurrence happens and nowhere else — a participant
// joining or leaving the session ([`join_stage_session`] / [`leave_stage_session`]) and
// a publisher's replication starting (`StartParticipantReplication`) — and
// `ListParticipantEvents` reports exactly those records. The names of `EventName` that
// nothing here records are the ones nothing here does: this emulator serves no WebRTC
// subscribe, fails no ingest and exchanges no participant token, so a `SUBSCRIBE_*`, an
// error event or a `TOKEN_EXCHANGED` would name something that never happened.
// ---------------------------------------------------------------------------

/// Per-service state for com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime. A handler reaches it through [`service_state`].
///
/// ONE store per engine, holding the resources of every region a caller has addressed,
/// each keyed by its own ARN — and an ARN names the region its resource was created in.
/// A request is answered from the view of the region IT was processed in, so the region
/// is a term of every lookup over these maps rather than a partition of them: the
/// accessors below take the region and the listings page in it ([`in_request_region`]),
/// and a request signed for one region finds nothing another region's endpoint created.
#[derive(Debug, Default)]
pub struct ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState {
    /// Every live stage, keyed by its ARN.
    pub stages: HashMap<String, StageRecord>,
    /// The tokens issued against a stage, keyed by that stage's ARN. Removed with
    /// the stage.
    pub participant_tokens: HashMap<String, Vec<ParticipantTokenRecord>>,
    /// The sessions a stage has had, keyed by that stage's ARN and held in start
    /// order. A session is opened by [`ensure_active_session`] and its participants
    /// written by [`join_stage_session`] / [`leave_stage_session`] — the stage's data
    /// plane, not this control plane — and the sessions go with the stage.
    pub stage_sessions: HashMap<String, Vec<StageSessionRecord>>,
    /// Every live ingest configuration, keyed by its ARN.
    pub ingest_configurations: HashMap<String, IngestConfigurationRecord>,
    /// Every live encoder configuration, keyed by its ARN.
    pub encoder_configurations: HashMap<String, EncoderConfigurationRecord>,
    /// Every live storage configuration, keyed by its ARN.
    pub storage_configurations: HashMap<String, StorageConfigurationRecord>,
    /// Every imported public key, keyed by its ARN.
    pub public_keys: HashMap<String, PublicKeyRecord>,
    /// Every composition that has been started and not yet reclaimed, keyed by its ARN.
    /// Ephemeral: the clock drops the record once the stop `StopComposition` began has
    /// finished (see [`ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState::materialize_due_compositions`]).
    pub compositions: HashMap<String, CompositionRecord>,
    /// The started replications of a stage's participants, keyed by that SOURCE stage's
    /// ARN and held in start order — the order `ListParticipantReplicas` pages over.
    /// Removed when either stage a replica names is deleted.
    pub participant_replicas: HashMap<String, Vec<ParticipantReplicaRecord>>,
    /// The socket this deployment's stage CONTRIBUTION plane answers RTMP and RTMPS on,
    /// once a call that announces it has opened it, and the handle that keeps that port
    /// open — see [`ensure_ingest_plane`]. One socket serves the whole deployment, because
    /// the endpoints a stage announces carry no part of a stage's own identity and the
    /// ingest key a publisher presents is what names the stage it publishes to.
    pub ingest_plane: Option<StageIngestPlane>,
    /// How many publish connections the contribution plane is serving for one participant
    /// of one session right now ([`IngestPublishConnection`]). Redundant ingest publishes
    /// one participant over more than one connection, so the participant leaves when the
    /// last of them is gone; an entry lives exactly as long as a connection does.
    pub ingest_publish_connections: HashMap<IngestPublishConnection, u32>,
    /// Monotonic counter behind every id this service mints, so two stages created
    /// in the same instant still get distinct ids and a distinct page position.
    next_id: u64,
}

impl ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState {
    /// The next value of the service's monotonic id counter.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_id = self.next_id.saturating_add(1);
        self.next_id
    }

    /// The highest sequence this store has handed out: every page position any listing
    /// of this service can have issued lies in `1..=issued_through()`, because a
    /// position is the sequence of a record the store drew from [`Self::next_sequence`],
    /// which counts up from one. A token naming any other position was issued by
    /// nothing, so the page-token codec reads none from it.
    pub fn issued_through(&self) -> u64 {
        self.next_id
    }

    // -----------------------------------------------------------------------
    // The region's view of this store
    //
    // Every accessor below takes the region the calling request was processed in and
    // answers about that region's resources only, through the one region term
    // [`in_request_region`] states: a resource belongs to the region its ARN names, and
    // an operation answers from the view of its own request's region. The maps
    // themselves are reached directly only by the writers that already know which
    // region's resource they hold (a create, and the data-plane helpers that follow a
    // stored reference).
    // -----------------------------------------------------------------------

    /// The stage `arn` names in `region`'s view, or `None` when that view holds none.
    pub fn stage_in(&self, region: &str, arn: &str) -> Option<&StageRecord> {
        self.stages.get(request_scoped(arn, region)?)
    }

    /// [`Self::stage_in`], to be changed in place.
    pub fn stage_in_mut(&mut self, region: &str, arn: &str) -> Option<&mut StageRecord> {
        self.stages.get_mut(request_scoped(arn, region)?)
    }

    /// The ingest configuration `arn` names in `region`'s view, or `None` when that view
    /// holds none.
    pub fn ingest_configuration_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&IngestConfigurationRecord> {
        self.ingest_configurations
            .get(request_scoped(arn, region)?)
    }

    /// [`Self::ingest_configuration_in`], to be changed in place.
    pub fn ingest_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut IngestConfigurationRecord> {
        self.ingest_configurations
            .get_mut(request_scoped(arn, region)?)
    }

    /// The encoder configuration `arn` names in `region`'s view, or `None` when that
    /// view holds none.
    pub fn encoder_configuration_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&EncoderConfigurationRecord> {
        self.encoder_configurations
            .get(request_scoped(arn, region)?)
    }

    /// [`Self::encoder_configuration_in`], to be changed in place.
    pub fn encoder_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut EncoderConfigurationRecord> {
        self.encoder_configurations
            .get_mut(request_scoped(arn, region)?)
    }

    /// The storage configuration `arn` names in `region`'s view, or `None` when that
    /// view holds none.
    pub fn storage_configuration_in(
        &self,
        region: &str,
        arn: &str,
    ) -> Option<&StorageConfigurationRecord> {
        self.storage_configurations
            .get(request_scoped(arn, region)?)
    }

    /// [`Self::storage_configuration_in`], to be changed in place.
    pub fn storage_configuration_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut StorageConfigurationRecord> {
        self.storage_configurations
            .get_mut(request_scoped(arn, region)?)
    }

    /// The public key `arn` names in `region`'s view, or `None` when that view holds
    /// none.
    pub fn public_key_in(&self, region: &str, arn: &str) -> Option<&PublicKeyRecord> {
        self.public_keys.get(request_scoped(arn, region)?)
    }

    /// [`Self::public_key_in`], to be changed in place.
    pub fn public_key_in_mut(&mut self, region: &str, arn: &str) -> Option<&mut PublicKeyRecord> {
        self.public_keys.get_mut(request_scoped(arn, region)?)
    }

    /// The composition `arn` names in `region`'s view, or `None` when that view holds
    /// none. Call it after [`Self::materialize_due_compositions`], so a composition
    /// whose ephemeral resource the clock has reclaimed is not reported as live.
    pub fn composition_in(&self, region: &str, arn: &str) -> Option<&CompositionRecord> {
        self.compositions.get(request_scoped(arn, region)?)
    }

    /// [`Self::composition_in`], to be changed in place.
    pub fn composition_in_mut(
        &mut self,
        region: &str,
        arn: &str,
    ) -> Option<&mut CompositionRecord> {
        self.compositions.get_mut(request_scoped(arn, region)?)
    }

    /// Every stage of `region` in creation order — the order `ListStages` pages over.
    pub fn stages_in_order(&self, region: &str) -> Vec<&StageRecord> {
        let mut stages: Vec<&StageRecord> = self
            .stages
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        stages.sort_by_key(|record| record.sequence);
        stages
    }

    /// Every composition of `region` in start order — the order `ListCompositions` pages
    /// over, so a composition starting or being reclaimed mid-pagination cannot shift a
    /// later page onto an already-returned one.
    pub fn compositions_in_order(&self, region: &str) -> Vec<&CompositionRecord> {
        let mut compositions: Vec<&CompositionRecord> = self
            .compositions
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        compositions.sort_by_key(|record| record.sequence);
        compositions
    }

    /// Every ingest configuration of `region` in creation order — the order
    /// `ListIngestConfigurations` pages over, so a configuration created or deleted
    /// mid-pagination cannot shift a later page onto an already-returned one.
    pub fn ingest_configurations_in_order(&self, region: &str) -> Vec<&IngestConfigurationRecord> {
        let mut configurations: Vec<&IngestConfigurationRecord> = self
            .ingest_configurations
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        configurations.sort_by_key(|record| record.sequence);
        configurations
    }

    /// Every encoder configuration of `region` in creation order — the order
    /// `ListEncoderConfigurations` pages over, so a configuration created or deleted
    /// mid-pagination cannot shift a later page onto an already-returned one.
    pub fn encoder_configurations_in_order(&self, region: &str) -> Vec<&EncoderConfigurationRecord> {
        let mut configurations: Vec<&EncoderConfigurationRecord> = self
            .encoder_configurations
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        configurations.sort_by_key(|record| record.sequence);
        configurations
    }

    /// Every storage configuration of `region` in creation order — the order
    /// `ListStorageConfigurations` pages over.
    pub fn storage_configurations_in_order(
        &self,
        region: &str,
    ) -> Vec<&StorageConfigurationRecord> {
        let mut configurations: Vec<&StorageConfigurationRecord> = self
            .storage_configurations
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        configurations.sort_by_key(|record| record.sequence);
        configurations
    }

    /// Every public key of `region` in import order — the order `ListPublicKeys` pages
    /// over, so a key imported or deleted mid-pagination cannot shift a later page onto
    /// an already-returned one.
    pub fn public_keys_in_order(&self, region: &str) -> Vec<&PublicKeyRecord> {
        let mut public_keys: Vec<&PublicKeyRecord> = self
            .public_keys
            .values()
            .filter(|record| in_request_region(&record.arn, region))
            .collect();
        public_keys.sort_by_key(|record| record.sequence);
        public_keys
    }

    /// Every session the stage `stage_arn` has had, in start order — the order
    /// `ListStageSessions` pages over, so a session opening or ending mid-pagination
    /// cannot shift a later page onto an already-returned one. A stage `region`'s view
    /// holds no session for (one that never had a participant, one it does not hold at
    /// all, or one of another region) has none.
    pub fn stage_sessions_in_start_order(
        &self,
        region: &str,
        stage_arn: &str,
    ) -> Vec<&StageSessionRecord> {
        let mut sessions: Vec<&StageSessionRecord> = request_scoped(stage_arn, region)
            .and_then(|stage_arn| self.stage_sessions.get(stage_arn))
            .map(|sessions| sessions.iter().collect())
            .unwrap_or_default();
        sessions.sort_by_key(|record| record.sequence);
        sessions
    }

    /// The session `session_id` of the stage `stage_arn` in `region`'s view, or `None`
    /// when that view holds no such session for that stage.
    ///
    /// The lookup is by BOTH keys: a session id names a session of one stage, so a
    /// session of another stage is not found here even though every id this store
    /// mints is unique.
    pub fn stage_session_in(
        &self,
        region: &str,
        stage_arn: &str,
        session_id: &str,
    ) -> Option<&StageSessionRecord> {
        self.stage_session(request_scoped(stage_arn, region)?, session_id)
    }

    /// The replications sourced from the stage `stage_arn` in `region`'s view, in the
    /// order they were started. Empty for a stage that view holds no replication for.
    pub fn participant_replicas_in(
        &self,
        region: &str,
        stage_arn: &str,
    ) -> &[ParticipantReplicaRecord] {
        request_scoped(stage_arn, region)
            .and_then(|stage_arn| self.participant_replicas.get(stage_arn))
            .map_or(&[], Vec::as_slice)
    }

    /// The session `session_id` of the stage `stage_arn`, whatever region that stage is
    /// in, or `None` when this store holds no such session for it.
    ///
    /// The region-less lookup, for the writers and the data-plane helpers that already
    /// hold the stage: a handler resolving an ARN a REQUEST supplied uses
    /// [`Self::stage_session_in`] instead, so it answers only about its own region.
    pub fn stage_session(&self, stage_arn: &str, session_id: &str) -> Option<&StageSessionRecord> {
        self.stage_sessions
            .get(stage_arn)?
            .iter()
            .find(|session| session.session_id == session_id)
    }

    /// Write down every composition transition the clock has already made due, and drop
    /// the compositions whose ephemeral resource it has reclaimed.
    ///
    /// Each one was owed at [`CompositionRecord::active_at`], at
    /// [`CompositionRecord::empty_stage_stop_at`], or one
    /// [`COMPOSITION_STOP_SPAN`] after an end began, whether or not anybody called, so this
    /// changes no composition that a caller reading at the same instant would have seen
    /// differently — which is why a reporting operation may call it. A stopped
    /// composition's record goes the same way: the model says the "Composition stops and
    /// the resource is deleted … When StopComposition is called", and after the same
    /// published timeout when the stage it composites holds no participant, so the record
    /// is reclaimed rather than kept in a terminal state forever — but only one whole
    /// [`COMPOSITION_RETENTION_SPAN`] after it reached that terminal state, so the stop a
    /// caller is waiting for is readable long enough to be read.
    pub fn materialize_due_compositions(&mut self, now: SystemTime) {
        for composition in self.compositions.values_mut() {
            composition.materialize_due(now);
        }
        self.compositions
            .retain(|_, composition| !composition.is_reclaimed(now));
    }

    /// Whether the stage `stage_arn` holds any participant right now: a session of it
    /// with a [`PARTICIPANT_STATE_CONNECTED`] record in it.
    ///
    /// The ONE answer to "are there participants in this stage", so what a composition's
    /// empty-stage timeout turns on is exactly what `ListParticipants` reports of the same
    /// stage. `false` for a stage this store holds no session of, and for one whose every
    /// record has left.
    pub fn stage_has_participants(&self, stage_arn: &str) -> bool {
        self.stage_sessions.get(stage_arn).is_some_and(|sessions| {
            sessions.iter().any(|session| {
                session
                    .participants
                    .iter()
                    .any(|participant| participant.state == PARTICIPANT_STATE_CONNECTED)
            })
        })
    }

    /// Tell every composition of `stage_arn` what that stage holds at `now`
    /// ([`CompositionRecord::observe_stage_occupancy`]).
    ///
    /// Called from each of the three places that answer can change — a participant joining
    /// ([`join_stage_session`]), one leaving ([`leave_stage_session`]) and a composition
    /// starting against a stage — so the deadline the clock owes a composition's stop at is
    /// decided in one place for all of them, and a composition of a stage nobody is in
    /// cannot go on composing forever.
    pub fn observe_stage_occupancy(&mut self, stage_arn: &str, now: SystemTime) {
        // What the clock already owed is written down FIRST, so a participant arriving
        // after a composition's timeout has run out cannot take back a stop that was
        // already due: the compositions this reaches are the ones a caller reading at
        // this instant would have seen.
        self.materialize_due_compositions(now);
        let occupied = self.stage_has_participants(stage_arn);
        for composition in self
            .compositions
            .values_mut()
            .filter(|composition| composition.stage_arn == stage_arn)
        {
            composition.observe_stage_occupancy(occupied, now);
        }
    }

    /// The composition of `region` started under the `@idempotencyToken` `token`, if that
    /// region's view holds one.
    ///
    /// A composition is keyed by its own ARN, so the token is searched for rather than
    /// indexed: the store is bounded by [`MAX_COMPOSITIONS`], and whether a start offers a
    /// token this region's view already holds a composition under is what this answers. A
    /// token is a token of the region it was first used in — the composition it names
    /// exists only there — so the same token offered through another region's endpoint
    /// names nothing and starts a composition of its own.
    pub fn composition_by_idempotency_token(
        &self,
        region: &str,
        token: &str,
    ) -> Option<&CompositionRecord> {
        self.compositions.values().find(|composition| {
            composition.idempotency_token.as_deref() == Some(token)
                && in_request_region(&composition.arn, region)
        })
    }

    /// The composition using the resource `arn` right now, if this store holds one: the
    /// first one started among those whose destinations refer to it
    /// ([`CompositionRecord::references`]) and whose stop has not completed
    /// ([`CompositionRecord::is_using_resources`]).
    ///
    /// The ONE lookup for "is a composition using this resource", so what a delete of an
    /// encoder or storage configuration refuses over is exactly what the compositions
    /// `GetComposition` reports say. Call it after
    /// [`Self::materialize_due_compositions`], so a composition whose stop the clock has
    /// already finished holds nothing back.
    pub fn composition_using(&self, arn: &str) -> Option<&CompositionRecord> {
        self.compositions
            .values()
            .filter(|composition| composition.is_using_resources() && composition.references(arn))
            .min_by_key(|composition| composition.sequence)
    }
}

// The seven CORS and security response headers this model binds as output members are a
// property of the ENDPOINT, not of the resource any one response reports, and AWS answers
// every call of this service with the same fixed value for each. They are stated once
// below, from the recorded responses of the operations whose output shapes bind them, so
// one answer of this service cannot publish a different policy from another.

/// Which web origin may read an answer of this endpoint: any one, as this endpoint's own
/// answers state.
///
/// `CreateIngestConfigurationResponse$accessControlAllowOrigin` binds
/// `@httpHeader("Access-Control-Allow-Origin")`
/// (`smithy_query(query="shape",shape="CreateIngestConfigurationResponse")`), and the value
/// is the endpoint's, not a fact about the resource the answer reports: every recorded
/// answer of this service carries the any-origin grant.
///
/// The header publishes which origins may READ an answer; it admits no request. Which
/// requests are served is not this service's to decide and is not widened by stating it:
/// the router refuses a request a browser sends from another site before any handler runs
/// (`refuse_cross_site_browsers`), and the browser-facing listeners drop every
/// `access-control-*` header a handler wrote and answer with the operator's own CORS policy
/// instead.
pub const ACCESS_CONTROL_ALLOW_ORIGIN: &str = "*";

/// Response headers a browser client may read off an answer of this endpoint.
///
/// The service's own fixed list, as the recorded responses state it: every header its
/// answers carry — the two CORS headers, the five caching and security policies, the two
/// content headers, and the six the model's `@cors` trait also names in
/// `additionalExposedHeaders` (`smithy_query(query="service_info")`: `x-amzn-errortype`,
/// `x-amzn-requestid`, `x-amzn-errormessage`, `x-amzn-trace-id`, `x-amz-apigw-id` and
/// `date`). Stated in the order and spelling the recorded responses use, since the value a
/// client reads is the header's text.
pub const ACCESS_CONTROL_EXPOSE_HEADERS: &str = "Access-Control-Allow-Origin,\
     Access-Control-Expose-Headers,Cache-Control,Content-Length,Content-Security-Policy,\
     Content-Type,date,Strict-Transport-Security,x-amz-apigw-id,x-amzn-errormessage,\
     x-amzn-errortype,x-amzn-requestid,x-amzn-trace-id,X-Content-Type-Options,\
     X-Frame-Options";

/// How long an answer of this endpoint may be reused: not at all, by either directive the
/// recorded responses state — no store of it, and no cached copy served without
/// revalidation.
///
/// Every operation of this service reports state a later call can change — a stage's
/// `activeSessionId`, an ingest configuration's `state` — so no answer is a representation
/// a client or a proxy may serve again.
pub const CACHE_CONTROL: &str = "no-store, no-cache";

/// What an answer of this endpoint may load: the policy this endpoint's own answers state —
/// same-origin sources only, and any subresource reference upgraded to HTTPS before it is
/// fetched. It governs what a document loads, and an answer of this service is a JSON
/// document that references nothing.
pub const CONTENT_SECURITY_POLICY: &str = "default-src 'self'; upgrade-insecure-requests;";

/// How long a client holds this endpoint to TLS, stated as the header's `max-age` in
/// seconds and applied to its subdomains too. The span is this emulator's own choice; what
/// the policy says is that a client which once reached the endpoint never tries cleartext
/// to it again.
pub const STRICT_TRANSPORT_SECURITY: &str = "max-age=47304000; includeSubDomains";

/// Whether a client may second-guess the content type of an answer: it may not. The one
/// value this header defines.
pub const X_CONTENT_TYPE_OPTIONS: &str = "nosniff";

/// Whether a browser may frame an answer of this endpoint: it may not. The header defines
/// `DENY` and `SAMEORIGIN`, and an API answer belongs in no frame at all.
pub const X_FRAME_OPTIONS: &str = "DENY";

/// The headers above, each paired with the name the model's `@httpHeader` trait gives it:
/// the seven every answer of this endpoint carries.
///
/// A SUCCESS does not go through this table: there they are modelled output members
/// ([`ResponseHeaderPolicy`]), so the protocol serializer writes each under the name its
/// own `@httpHeader` trait carries. The table is for the responses whose modelled members
/// never reach the wire — every failure of this service renders the one-member body
/// `crate::scaffolded::STATED_ERROR_SCHEMA` describes rather than the modelled error
/// shape, though the same members are on each of those shapes too — so `crate::service`
/// writes them onto the response itself, at the one place every response passes through.
/// The set is the SAME one a success carries, so the policy a client reads off this
/// endpoint does not depend on whether the call succeeded.
pub const RESPONSE_HEADER_POLICY: [(&str, &str); 7] = [
    ("Access-Control-Allow-Origin", ACCESS_CONTROL_ALLOW_ORIGIN),
    (
        "Access-Control-Expose-Headers",
        ACCESS_CONTROL_EXPOSE_HEADERS,
    ),
    ("Cache-Control", CACHE_CONTROL),
    ("Content-Security-Policy", CONTENT_SECURITY_POLICY),
    ("Strict-Transport-Security", STRICT_TRANSPORT_SECURITY),
    ("X-Content-Type-Options", X_CONTENT_TYPE_OPTIONS),
    ("X-Frame-Options", X_FRAME_OPTIONS),
];

/// An output shape of this service that binds the CORS and security response headers.
///
/// Four of this model's output shapes bind those members —
/// `CreateIngestConfigurationResponse`, `StartParticipantReplicationResponse`,
/// `StopParticipantReplicationResponse` and `UpdateIngestConfigurationResponse`
/// (`smithy_query(query="shape",shape="CreateIngestConfigurationResponse")`) — and the
/// values are a property of the ENDPOINT rather than of the resource any one of them
/// reports, so they are filled in here, in one place, instead of in each handler.
pub trait ResponseHeaderPolicy {
    /// This output with each member the shape binds carrying the endpoint's value for it,
    /// so the protocol serializer writes them on the response.
    #[must_use]
    fn with_response_header_policy(self) -> Self;
}

/// Implement [`ResponseHeaderPolicy`] for each output shape that binds those members, so
/// one shape cannot report a different policy from another.
macro_rules! impl_response_header_policy {
    ($($output:ty),+ $(,)?) => {
        $(impl ResponseHeaderPolicy for $output {
            fn with_response_header_policy(mut self) -> Self {
                self.access_control_allow_origin =
                    Some(ACCESS_CONTROL_ALLOW_ORIGIN.to_string());
                self.access_control_expose_headers =
                    Some(ACCESS_CONTROL_EXPOSE_HEADERS.to_string());
                self.cache_control = Some(CACHE_CONTROL.to_string());
                self.content_security_policy =
                    Some(CONTENT_SECURITY_POLICY.to_string());
                self.strict_transport_security = Some(STRICT_TRANSPORT_SECURITY.to_string());
                self.x_content_type_options = Some(X_CONTENT_TYPE_OPTIONS.to_string());
                self.x_frame_options = Some(X_FRAME_OPTIONS.to_string());
                self
            }
        })+
    };
}

// The still-scaffolded operation of the four is implemented here as well: the policy is
// the shape's, not the handler's, so implementing that operation is a call to this trait
// rather than seven more assignments.
impl_response_header_policy!(
    CreateIngestConfigurationOutput,
    StartParticipantReplicationOutput,
    StopParticipantReplicationOutput,
    UpdateIngestConfigurationOutput,
);

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState>)
}

/// The modelled 500 a handler answers with when the shared engine cannot hand back
/// this service's store. Unreachable in this emulator (the router builds one
/// `InMemoryEngine` and hands it to every operation), but a handler must answer
/// rather than panic.
///
/// `variant` is the calling operation's OWN modelled error constructor, so the compiler
/// still refuses a failure the operation does not model and the code and status stay
/// model-derived; only the BODY is this service's rendering (see
/// [`crate::scaffolded::Scaffolded::stated`]).
pub fn store_unavailable<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    internal_server(variant, STORE_UNAVAILABLE_MESSAGE)
}

/// The modelled 500 `variant` names, stating `message`. The one construction behind
/// every `InternalServerException` this service answers, so two of them cannot be
/// assembled two ways.
fn internal_server<E>(variant: fn(InternalServerException) -> E, message: &str) -> Scaffolded<E> {
    Scaffolded::stated(
        variant(InternalServerException {
            exception_message: Some(message.to_string()),
            ..Default::default()
        }),
        message.to_string(),
    )
}

/// The text [`store_unavailable`] states. AWS documents no message for this (it is an
/// emulator fault, not a service behaviour), so it says plainly what failed.
pub const STORE_UNAVAILABLE_MESSAGE: &str =
    "the IVS RealTime store is not reachable from the engine serving this request";

/// The modelled 500 a handler answers with when the emulator's credential store minted
/// no secret for a participant token, so no token is issued rather than one this service
/// made up itself.
pub fn token_minting_failed<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    internal_server(variant, TOKEN_MINTING_FAILED_MESSAGE)
}

/// The text [`token_minting_failed`] states.
pub const TOKEN_MINTING_FAILED_MESSAGE: &str =
    "the participant token could not be minted, so no token was issued";

/// The modelled 500 a handler answers when the emulator's credential store minted no
/// secret for an ingest key, so no ingest configuration is created rather than one
/// carrying a stream key this service made up itself.
pub fn stream_key_minting_failed<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    internal_server(variant, STREAM_KEY_MINTING_FAILED_MESSAGE)
}

/// The text [`stream_key_minting_failed`] states.
pub const STREAM_KEY_MINTING_FAILED_MESSAGE: &str =
    "the ingest stream key could not be minted, so no ingest configuration was created";

/// Characters in a minted stage id. The model's `StageArn` pattern allows
/// `[a-zA-Z0-9-]+` after `stage/`, and caps the whole ARN at 128 characters.
pub const STAGE_ID_LEN: usize = 12;

/// Characters in a minted participant id. The model's `ParticipantTokenId` allows
/// `[a-zA-Z0-9-_]{0,64}`.
pub const PARTICIPANT_ID_LEN: usize = 12;

/// Characters in a minted ingest-configuration id. The model's
/// `IngestConfigurationArn` pattern allows `[a-zA-Z0-9-]+` after
/// `ingest-configuration/`, and caps the whole ARN at 128 characters.
pub const INGEST_CONFIGURATION_ID_LEN: usize = 12;

/// Characters in a minted encoder-configuration id. The model's
/// `EncoderConfigurationArn` pattern allows `[a-zA-Z0-9-]+` after
/// `encoder-configuration/`, and caps the whole ARN at 128 characters.
pub const ENCODER_CONFIGURATION_ID_LEN: usize = 12;

/// Characters in a minted storage-configuration id. The model's
/// `StorageConfigurationArn` pattern allows `[a-zA-Z0-9-]+` after
/// `storage-configuration/`, and caps the whole ARN at 128 characters.
pub const STORAGE_CONFIGURATION_ID_LEN: usize = 12;

/// Characters in a minted public-key id. The model's `PublicKeyArn` pattern allows
/// `[a-zA-Z0-9-]+` after `public-key/`, and caps the whole ARN at 128 characters.
pub const PUBLIC_KEY_ID_LEN: usize = 12;

/// Characters a minted stage session id carries after its `st-` prefix.
///
/// The model's `StageSessionId` is `@length 16..16` with `@pattern ^st-[a-zA-Z0-9]+$`,
/// so a session id is `st-` followed by exactly thirteen id characters.
pub const STAGE_SESSION_ID_LEN: usize = 13;

/// A fresh id for a resource this service mints: `len` lowercase-hex characters, which
/// stays inside the `[a-zA-Z0-9-]` / `[a-zA-Z0-9-_]` id charsets the model's `StageArn`
/// and `ParticipantTokenId` patterns allow.
///
/// The leading digits are the store's monotonic `sequence`, which is distinct for every
/// resource this store mints, so two ids of one store run can never collide. The trailing
/// digits are the low bits of the creation instant, so a store that starts over after a
/// restart does not re-issue the ids of the run before it.
pub fn mint_id(sequence: u64, len: usize) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    // `% 0x1_0000_0000` keeps the four low hex bytes of the instant; the cast cannot
    // truncate after it.
    let instant = u64::try_from(nanos % 0x1_0000_0000).unwrap_or(0);
    let mut id = format!("{sequence:06x}{instant:08x}");
    id.truncate(len);
    id
}

/// The stage ARN for `stage_id`, in the form the model's `StageArn` pattern
/// requires: `arn:aws:ivs:<region>:<account>:stage/<id>`.
pub fn stage_arn(region: &str, stage_id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("stage/{stage_id}"))
}

/// The ingest-configuration ARN for `ingest_configuration_id`, in the form the model's
/// `IngestConfigurationArn` pattern requires:
/// `arn:aws:ivs:<region>:<account>:ingest-configuration/<id>`.
pub fn ingest_configuration_arn(region: &str, ingest_configuration_id: &str) -> String {
    ::engine::emulator_arn(
        "ivs",
        region,
        &format!("ingest-configuration/{ingest_configuration_id}"),
    )
}

/// The encoder-configuration ARN for `encoder_configuration_id`, in the form the model's
/// `EncoderConfigurationArn` pattern requires:
/// `arn:aws:ivs:<region>:<account>:encoder-configuration/<id>`.
pub fn encoder_configuration_arn(region: &str, encoder_configuration_id: &str) -> String {
    ::engine::emulator_arn(
        "ivs",
        region,
        &format!("encoder-configuration/{encoder_configuration_id}"),
    )
}

/// The storage-configuration ARN for `storage_configuration_id`, in the form the
/// model's `StorageConfigurationArn` pattern requires:
/// `arn:aws:ivs:<region>:<account>:storage-configuration/<id>`.
pub fn storage_configuration_arn(region: &str, storage_configuration_id: &str) -> String {
    ::engine::emulator_arn(
        "ivs",
        region,
        &format!("storage-configuration/{storage_configuration_id}"),
    )
}

/// The public-key ARN for `public_key_id`, in the form the model's `PublicKeyArn`
/// pattern requires: `arn:aws:ivs:<region>:<account>:public-key/<id>`.
pub fn public_key_arn(region: &str, public_key_id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("public-key/{public_key_id}"))
}

/// An identity this store has just minted for a resource it is about to hold.
#[derive(Debug)]
pub struct MintedResource {
    /// The store sequence the identity was drawn at, which the record keeps as its page
    /// position so a list pages the resources in creation order.
    pub sequence: u64,
    /// The id itself: [`mint_id`] over the drawn sequence, at the length the resource's
    /// modelled id allows.
    pub id: String,
    /// The ARN naming [`Self::id`], built by the resource's own ARN constructor.
    pub arn: String,
}

/// A fresh id of `id_len` characters that no resource `taken` reports as already named,
/// re-minted from a fresh sequence while it is, with the sequence first drawn.
///
/// The ONE place this service's "mint a fresh id, re-mint on collision" rule lives, so
/// every identity it issues is unique by construction and not by probability. The id and
/// the drawn sequence are independent: the sequence is the record's page position, kept
/// whatever re-minting the collision check needed.
fn mint_unique_id<T>(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    id_len: usize,
    taken: T,
) -> (u64, String)
where
    T: Fn(&ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState, &str) -> bool,
{
    let sequence = state.next_sequence();
    let mut id = mint_id(sequence, id_len);
    while taken(state, &id) {
        let retry = state.next_sequence();
        id = mint_id(retry, id_len);
    }
    (sequence, id)
}

/// A fresh identity for a resource keyed by its ARN: an id of `id_len` characters, the
/// ARN `arn_of` builds around it, and the sequence the record keeps.
///
/// `holds` answers whether this store already holds a resource under a candidate ARN —
/// the lookup of the map the caller is about to insert into — and the identity is
/// re-minted while it does, through the one rule [`mint_unique_id`] states. Every
/// creating operation of this service mints through here, so one ARN can never denote
/// two resources and the six creates cannot drift apart on how they ensure it.
pub fn mint_unique_arn<A, H>(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    id_len: usize,
    arn_of: A,
    holds: H,
) -> MintedResource
where
    A: Fn(&str) -> String,
    H: Fn(&ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState, &str) -> bool,
{
    let (sequence, id) = mint_unique_id(state, id_len, |state, candidate| {
        holds(state, &arn_of(candidate))
    });
    let arn = arn_of(&id);
    MintedResource { sequence, id, arn }
}

/// When a token issued now for `duration_minutes` expires.
pub fn expiration_time(duration_minutes: i32) -> SystemTime {
    let minutes = u64::try_from(duration_minutes).unwrap_or(0);
    SystemTime::now() + Duration::from_secs(minutes.saturating_mul(60))
}

/// A fresh stage session id, in the form the model's `StageSessionId` requires:
/// `st-` plus [`STAGE_SESSION_ID_LEN`] id characters, sixteen in total.
pub fn stage_session_id(sequence: u64) -> String {
    format!("st-{}", mint_id(sequence, STAGE_SESSION_ID_LEN))
}

/// The id of the session active in the stage `stage_arn`, opening one if the stage has
/// none, or `None` when this store holds no such stage (or when it is at
/// [`MAX_STAGE_SESSIONS_PER_STAGE`] sessions, which no emulated use reaches).
///
/// A stage session is the span a stage has participants in: the model's `StageSession`
/// doc describes it as beginning "when the first participant joins" and the stage
/// reporting it as `activeSessionId` until it ends. Nothing joins an emulated stage over
/// the API, so the two things that put a participant into a stage open the session it
/// needs through here — `StartParticipantReplication`, which puts a publisher's replica
/// into the destination stage and names the session its publisher is in on the source
/// side, and [`join_stage_session`], the stage data plane's own entry point. A stage that
/// already has one keeps it, so every replica and every participant of one stage names
/// the same session and a caller that read `activeSessionId` off the stage reads the same
/// value again.
///
/// A stage session "begins when the first participant joins a stage", so it begins at `at`
/// — the instant of the join that needed it.
///
/// The ONE place a session begins, so the id a stage reports and the
/// [`StageSessionRecord`] the reporting operations read are one session and not two.
pub fn ensure_active_session(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    at: SystemTime,
) -> Option<String> {
    let active = state
        .stages
        .get(stage_arn)?
        .active_session_id
        .clone()
        // The empty value is the "no session" reading of the member
        // ([`NO_ACTIVE_SESSION_ID`]), not a session id.
        .filter(|session_id| !session_id.is_empty());
    if let Some(session_id) = active {
        return Some(session_id);
    }
    // Bound the one store a session opening grows; the records are reclaimed with the
    // stage.
    if state.stage_sessions.get(stage_arn).map_or(0, Vec::len) >= MAX_STAGE_SESSIONS_PER_STAGE {
        return None;
    }
    let sequence = state.next_sequence();
    let session_id = stage_session_id(sequence);
    state
        .stage_sessions
        .entry(stage_arn.to_string())
        .or_default()
        .push(StageSessionRecord {
            session_id: session_id.clone(),
            start_time: at,
            end_time: None,
            participants: Vec::new(),
            events: Vec::new(),
            sequence,
        });
    // The stage now reports the session a caller passes to `ListParticipants`, which is
    // what `Stage$activeSessionId` is for.
    state.stages.get_mut(stage_arn)?.active_session_id = Some(session_id.clone());
    Some(session_id)
}

/// What this service holds about one participant of one stage.
#[derive(Debug, Clone)]
pub struct StageParticipant {
    /// Whether the participant may publish to the stage. Only a publisher can be
    /// replicated, and only an identity that carries [`PUBLISH_CAPABILITY`] — or an
    /// ingest credential, which exists to broadcast — may publish.
    pub publishes: bool,
    /// The customer-assigned name the participant's identity carries, when it carries
    /// one: the `userId` a record of this participant reports.
    pub user_id: Option<String>,
    /// The attributes the participant's identity carries, which a replica's attributes
    /// are merged over.
    pub attributes: HashMap<String, String>,
}

/// The identity this service ISSUED for `participant_id` against the stage `stage_arn`,
/// or `None` when it issued none.
///
/// A participant id is "assigned by IVS and returned by `CreateParticipantToken`" (the
/// model's doc for the member, on every operation that takes one) or minted with an
/// ingest configuration's credentials, so the identities this store holds for a stage are
/// exactly its token records and the credentials of the ingest configurations associated
/// with it. The ONE lookup for "what did this service issue this participant for this
/// stage, and may it publish there" — the capability a request to publish on its behalf
/// turns on, and the name and attributes such a request carries over.
///
/// This is NOT the question "is this participant in this stage, and what is it doing
/// there": a stage holds whatever participant records its sessions hold, including one
/// whose identity was issued against another stage, which is [`attached_participant`] and
/// its existence half [`participant_attached_to_stage`].
pub fn stage_participant(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> Option<StageParticipant> {
    if let Some(record) = state
        .participant_tokens
        .get(stage_arn)
        .and_then(|records| {
            records.iter().find(|record| {
                record.participant_id == participant_id
                    // A REVOKED token is no longer an identity this service issued: it
                    // "can no longer connect to a stage", so it empowers and names
                    // nobody there ([`revoke_connection_token`]).
                    && !record.is_revoked()
            })
        })
    {
        return Some(StageParticipant {
            publishes: record
                .capabilities
                .iter()
                .any(|capability| capability == PUBLISH_CAPABILITY),
            user_id: record.user_id.clone(),
            attributes: record.attributes.clone(),
        });
    }
    state
        .ingest_configurations
        .values()
        .find(|configuration| {
            configuration.stage_arn == stage_arn
                && configuration.mints_participant(participant_id)
        })
        .map(|configuration| StageParticipant {
            // An ingest configuration is a publisher's credential: it carries the ingest
            // key a broadcast is published with, so its participant publishes.
            publishes: true,
            // The user id this credential was created WITH, or none when it was created
            // without one: the configuration materialises that member as the empty string
            // for its own projections ([`UNNAMED_PARTICIPANT_USER_ID`]), and a participant
            // carries a user id only where one was really named.
            user_id: (!configuration.user_id.is_empty()).then(|| configuration.user_id.clone()),
            attributes: configuration.attributes.clone(),
        })
}

/// What the SESSIONS of the stage `stage_arn` hold for `participant_id` — the record a
/// session of that stage joined for it, whatever put it there — or `None` when no session
/// of that stage holds one.
///
/// A record is a participant of the stage in its own right, with no identity this service
/// issued against that stage behind it: a replica a replication put into its destination
/// stage carries an id issued against the SOURCE stage ([`record_replicated_participant`]),
/// and it is the record that then says what that participant is named, what attributes it
/// carries, and whether it is publishing in this stage — the "currently is publishing"
/// question read off the session's own occurrences ([`StageSessionRecord::is_publishing`]),
/// never [`ParticipantRecord::published`], which only says it ever did.
///
/// A publishing record is the one answered when a stage holds several, and otherwise the
/// newest, so a participant that has been in a stage more than once is described by the
/// record that is live there.
fn recorded_stage_participant(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> Option<StageParticipant> {
    let mut held: Option<StageParticipant> = None;
    for session in state.stage_sessions.get(stage_arn)?.iter() {
        let Some(record) = session
            .participants
            .iter()
            .find(|participant| participant.participant_id == participant_id)
        else {
            continue;
        };
        let publishes =
            record.state == PARTICIPANT_STATE_CONNECTED && session.is_publishing(participant_id);
        let view = StageParticipant {
            publishes,
            user_id: record.user_id.clone(),
            attributes: record.attributes.clone(),
        };
        if publishes {
            return Some(view);
        }
        // The sessions of a stage are held in the order they began, so the last record
        // found is the newest one.
        held = Some(view);
    }
    held
}

/// The participant `participant_id` the stage `stage_arn` HAS, as an operation acting on a
/// participant of that stage needs it: whether it publishes there, and the name and
/// attributes it carries.
///
/// A stage has the participants its own records hold — every participant record in a
/// session of that stage, whatever opened it and whatever state it is now in
/// ([`recorded_stage_participant`]) — together with the identities this service issued
/// against the stage ([`stage_participant`]), which a holder can present to the stage
/// whether or not it has joined yet. So it publishes there when the identity it holds
/// carries [`PUBLISH_CAPABILITY`] (or is an ingest credential, which exists to broadcast)
/// or when the record the stage holds for it is publishing media in that stage right now;
/// what it is NAMED is the identity's where this service issued one, and the record's
/// otherwise, which is all a replica of another stage's publisher has.
///
/// The ONE answer to "does this stage have this participant, and what is it doing there",
/// so an operation that acts on a participant of a stage and the readers that report that
/// stage's participants (`ListParticipants`, `GetParticipant`) cannot disagree about which
/// participants the stage has. `None` for a stage this store holds nothing of, and for a
/// participant whose records and identities all belong to other stages.
pub fn attached_participant(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> Option<StageParticipant> {
    let issued = stage_participant(state, stage_arn, participant_id);
    let recorded = recorded_stage_participant(state, stage_arn, participant_id);
    let publishes = issued.as_ref().is_some_and(|issued| issued.publishes)
        || recorded.as_ref().is_some_and(|recorded| recorded.publishes);
    let mut participant = issued.or(recorded)?;
    participant.publishes = publishes;
    Some(participant)
}

/// Whether the participant `participant_id` is attached to the stage `stage_arn` — the
/// relation `DisconnectParticipant$stageArn` names ("ARN of the stage to which the
/// participant is attached").
///
/// The existence half of [`attached_participant`], which is where the relation itself is
/// stated: an operation that only needs to know whether the stage has the participant asks
/// here, one that acts on what it is doing there asks that.
pub fn participant_attached_to_stage(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
    participant_id: &str,
) -> bool {
    attached_participant(state, stage_arn, participant_id).is_some()
}

/// One publisher broadcasting with an ingest configuration right now: the session of
/// the configuration's stage it is in, and the participant it publishes as.
#[derive(Debug, Clone)]
pub struct IngestPublisher {
    /// The session of the configuration's stage the publisher joined.
    pub session_id: String,
    /// The participant the publisher appears as in that session.
    pub participant_id: String,
}

/// The publishers broadcasting with `configuration` right now, in the order the
/// sessions of its stage hold them.
///
/// `IngestConfiguration$state` is `ACTIVE` "if a publisher currently is publishing to
/// the stage associated with the ingest configuration", so a publisher of a
/// configuration is a participant of a session of THAT stage which is still
/// [`PARTICIPANT_STATE_CONNECTED`], which is publishing MEDIA to that session right now
/// ([`StageSessionRecord::is_publishing`]), and which reached the stage with this
/// configuration's credentials — recorded with the configuration's ARN, or publishing
/// as one of the participants its ingest credentials were minted for. Those are the
/// records the stage's data plane writes ([`join_stage_session`]) and
/// `ListParticipants` reports; a configuration associated with no stage
/// ([`NO_ASSOCIATED_STAGE_ARN`]) has nothing to publish to, so it has no publisher.
///
/// Connection alone is NOT enough: a participant that joined to subscribe only, and one
/// whose media has stopped, is connected to the stage without publishing to it, and the
/// member's condition is "currently is publishing". The same wording governs what hangs
/// off this lookup — a delete refuses only while "the publisher is actively streaming to
/// a stage", and `force` disconnects the participant only "when a participant is actively
/// publishing" — so publishing is required here, in the one place, rather than at each
/// caller.
///
/// The ONE lookup for "is a publisher broadcasting with this configuration", so what a
/// read reports about a configuration and what a delete refuses over cannot disagree.
pub fn ingest_configuration_publishers(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    configuration: &IngestConfigurationRecord,
) -> Vec<IngestPublisher> {
    let mut publishers = Vec::new();
    if configuration.stage_arn == NO_ASSOCIATED_STAGE_ARN {
        return publishers;
    }
    let Some(sessions) = state.stage_sessions.get(&configuration.stage_arn) else {
        return publishers;
    };
    for session in sessions {
        for participant in &session.participants {
            if participant.state != PARTICIPANT_STATE_CONNECTED {
                continue;
            }
            // Publishing right now, read off the session's own occurrences through the
            // crate's single answer to that question — not `ParticipantRecord::published`,
            // which is "whether the participant EVER published" and stays set once set.
            if !session.is_publishing(&participant.participant_id) {
                continue;
            }
            let publishes_with_configuration = configuration.publishes_participant(participant);
            if publishes_with_configuration {
                publishers.push(IngestPublisher {
                    session_id: session.session_id.clone(),
                    participant_id: participant.participant_id.clone(),
                });
            }
        }
    }
    publishers
}

/// Leave every ingest configuration associated with `stage_arn` associated with no
/// stage.
///
/// A non-empty `IngestConfiguration$stageArn` is a REFERENCE to a stage of this service
/// ([`resolved_stage_association`] refuses one this store holds no stage for), so a
/// stage that goes away takes the references to it with it: once the stage is gone, no
/// configuration reports its ARN. The configurations themselves stay — they keep their
/// ingest keys and can be associated with another stage — so only the association is
/// dropped, through the one writer of the "no stage" reading
/// ([`IngestConfigurationRecord::clear_stage_association`]).
pub fn disassociate_stage_from_ingest_configurations(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    stage_arn: &str,
) {
    // A configuration associated with no stage is already where this leaves it, so the
    // "no stage" value is never treated as a stage to disassociate from.
    if stage_arn == NO_ASSOCIATED_STAGE_ARN {
        return;
    }
    for configuration in state.ingest_configurations.values_mut() {
        if configuration.stage_arn == stage_arn {
            configuration.clear_stage_association();
        }
    }
}

/// The region the request was processed in: the one its SigV4 credential scope names, so
/// a caller reads back the region it addressed and is answered from that region's view of
/// the account ([`in_request_region`]).
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, and for a credential scope
/// whose region is not a token the model's ARN patterns accept (`[a-z0-9-]+`). It lives
/// here, beside that constant, because it is the rule for every ARN this service mints
/// and for every resource it reports, not just the stage ARNs of this slice.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region element of an IVS ARN: the fourth `:`-field of the form every ARN shape in
/// this model pins (`arn:aws:ivs:<region>:<account>:<type>/<id>`), or `None` when the
/// string carries no such field.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3)
}

/// Whether the resource `arn` names is one a request processed in `region` is answered
/// about.
///
/// Each resource of this service is created through one region's endpoint and carries
/// that region in its ARN ([`request_region`] is what every mint fills the element
/// from), and an operation answers from the account view of the region ITS OWN request
/// was processed in, so an ARN naming another region names nothing it can report, change
/// or delete. Every record is keyed by its own ARN, so this predicate over the ARN a
/// request supplied is the region term of the lookup — the one place that term is
/// stated, used by the accessors below and by the listings' page order.
pub fn in_request_region(arn: &str, region: &str) -> bool {
    arn_region(arn) == Some(region)
}

/// `arn` itself when [`in_request_region`] holds for it, so a region-scoped lookup reads
/// `self.stages.get(request_scoped(arn, region)?)`.
fn request_scoped<'a>(arn: &'a str, region: &str) -> Option<&'a str> {
    in_request_region(arn, region).then_some(arn)
}

/// What one request supplies for one participant token: the four members
/// `ParticipantTokenConfiguration` (a `CreateStage` entry) and
/// `CreateParticipantTokenRequest` have in common. Both operations hand this to
/// [`issue_token_record`], so neither can size, name or empower a token differently from
/// the other.
#[derive(Debug, Default)]
pub struct RequestedToken {
    pub user_id: Option<String>,
    pub attributes: Option<HashMap<String, String>>,
    pub duration: Option<i32>,
    pub capabilities: Option<Vec<String>>,
}

/// Mint one participant token for `stage_arn` from what the request supplied, applying
/// the model's documented defaults to what it left out.
///
/// `in_flight` holds the records this same request has already minted but not yet
/// stored, so the id check covers them too: `CreateStage` issues up to twelve tokens
/// before any of them reaches the store.
///
/// The ONE place a token record is built: `CreateStage` (once per
/// `participantTokenConfigurations` entry) and `CreateParticipantToken` both call it, so
/// the id length, the documented defaults and the minting cannot drift apart between
/// them. The record is returned rather than stored: each caller owns where it belongs in
/// the store, and a request refused after this point persists nothing.
///
/// `None` only when the credential store minted no secret for the token, which the caller
/// answers with [`token_minting_failed`] rather than by returning a token this service
/// made up.
pub fn issue_token_record(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    credentials: &::emulator_server::CredentialStore,
    stage_arn: &str,
    in_flight: &[ParticipantTokenRecord],
    requested: RequestedToken,
) -> Option<ParticipantTokenRecord> {
    // A fresh id per issuance, re-minted on the (vanishingly unlikely) chance it already
    // names a participant of this stage — stored or minted earlier in this same request.
    // Minted through the one place this service applies that rule, so the id a token
    // carries is unique by construction and not by probability.
    let (_, participant_id) = mint_unique_id(state, PARTICIPANT_ID_LEN, |state, candidate| {
        in_flight
            .iter()
            .any(|record| record.participant_id == candidate)
            || state
                .participant_tokens
                .get(stage_arn)
                .is_some_and(|records| {
                    records
                        .iter()
                        .any(|record| record.participant_id == candidate)
                })
    });

    // The documented defaults are applied to what the token IS — the lifetime its
    // expiry is computed from, and the capability set every reader of this participant
    // decides from. What the request itself stated is kept beside each of them, because
    // that is what a reported token echoes (see
    // `ParticipantTokenRecord::to_participant_token` and `ReportedTokenDuration`).
    let duration_minutes = requested
        .duration
        .unwrap_or(DEFAULT_TOKEN_DURATION_MINUTES);
    // The expiry is settled before the token is minted, because the token itself claims
    // it: the instant inside the token and the `expirationTime` the record reports are the
    // same one.
    let expiration_time = expiration_time(duration_minutes);
    let token = mint_participant_token(credentials, stage_arn, &participant_id, expiration_time)?;
    Some(ParticipantTokenRecord {
        participant_id,
        token,
        user_id: requested.user_id,
        attributes: requested.attributes.unwrap_or_default(),
        duration_minutes,
        requested_duration: requested.duration,
        capabilities: requested
            .capabilities
            .clone()
            .unwrap_or_else(default_token_capabilities),
        requested_capabilities: requested.capabilities,
        expiration_time,
        // Issued, not revoked: a token is revoked only by the operation that revokes it.
        revoked_at: None,
    })
}

/// Total size of an `attributes` map, against [`MAX_ATTRIBUTES_BYTES`]: the number of
/// bytes the map occupies as JSON — the braces, the quoted and escaped keys, the colons,
/// the commas and the quoted and escaped values — which is the form every request
/// carries one in and the form the documented 1 KB bound is measured on. The encoding is
/// `serde_json`'s, not a rendering of this service's own, so the count is the one the
/// wire would carry; a map's entry order does not change its length.
///
/// A map this service cannot encode is one no request could have carried and none of its
/// readers could report, so it counts as past the bound rather than inside it.
pub fn attributes_size(attributes: &HashMap<String, String>) -> usize {
    ::serde_json::to_vec(attributes).map_or(usize::MAX, |encoded| encoded.len())
}

/// The `typ` of a token in the compact serialization (RFC 7519 §5.1).
const TOKEN_TYPE: &str = "JWT";

/// The JWS algorithm a token this service issues is authenticated under: the HMAC with
/// SHA-256 of RFC 7518 §3.2, computed over the token's own encoded header and claim set.
const TOKEN_SIGNING_ALGORITHM: &str = "HS256";

/// `time` as whole seconds since the epoch, which is how a claim carries an instant
/// (RFC 7519 §2's `NumericDate`).
fn unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(UNIX_EPOCH)
        .map(|since_epoch| i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// The client token a participant joins the stage `stage_arn` with, as the participant
/// `participant_id`, until `expires_at`.
///
/// The FORM is a JSON Web Token in compact serialization (RFC 7515 §3.1, RFC 7519 §3):
/// `base64url(header) '.' base64url(claims) '.' base64url(signature)`, each segment in the
/// URL-safe alphabet without padding (RFC 7515 §2). That is the serialization the clients
/// of this API parse a token in — they split it on `.` and base64url-decode the segments
/// before presenting it — so a value carrying no segments at all is rejected before any
/// stage is reached, whatever its contents. The model's own `ParticipantToken` doc fixes
/// what a caller may rely on BEYOND the form: "Treat tokens as opaque; i.e., do not build
/// functionality based on token contents. The format of tokens could change in the
/// future."
///
/// The claim set is what this service already knows about the token, each claim the one
/// RFC 7519 §4.1 registers for it: the stage it admits its holder to (`aud`), the
/// participant it admits them as (`jti`) and the instant it expires (`exp`, the same
/// instant the reported `expirationTime` carries). Nothing else the request supplied
/// travels in it.
///
/// The signature is the MAC of [`TOKEN_SIGNING_ALGORITHM`] over the encoded header and
/// claim set, keyed by a secret minted per issuance by the one component of this emulator
/// that generates secrets — the credential store — which is then removed from it again:
/// the caller gets a token no other party can produce, the emulator keeps no credential it
/// would accept as SigV4 (this token authenticates to a stage, not to the emulator's API),
/// and nothing accumulates per call. The same pattern the MWAA and Grafana emulators use
/// for their caller-facing tokens. So every token is distinct, and none is echoed back
/// from the request.
///
/// `None` when the store minted no session-token secret, which leaves the operation to
/// answer [`token_minting_failed`] rather than report a token it made up.
pub fn mint_participant_token(
    credentials: &::emulator_server::CredentialStore,
    stage_arn: &str,
    participant_id: &str,
    expires_at: SystemTime,
) -> Option<String> {
    let minted = credentials.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let secret = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .map(str::to_string);
    // The signing key is not a credential of this emulator; see above.
    let _ = credentials.delete(&minted.id);
    let secret = secret?;

    let header = ::serde_json::json!({ "typ": TOKEN_TYPE, "alg": TOKEN_SIGNING_ALGORITHM });
    let claims = ::serde_json::json!({
        "aud": stage_arn,
        "jti": participant_id,
        "exp": unix_seconds(expires_at),
    });
    // RFC 7515 §5.1: the signature is computed over the ENCODED header and claim set
    // joined by '.', so those exact bytes are what the token carries. The segment coding
    // is this tree's one base64url coder, beside the JOSE reading that reads such a token.
    let signing_input = format!(
        "{}.{}",
        ::service_common::jwt::encode_base64url(header.to_string().as_bytes()),
        ::service_common::jwt::encode_base64url(claims.to_string().as_bytes())
    );
    let key = ::ring::hmac::Key::new(::ring::hmac::HMAC_SHA256, secret.as_bytes());
    let signature = ::ring::hmac::sign(&key, signing_input.as_bytes());
    Some(format!(
        "{signing_input}.{}",
        ::service_common::jwt::encode_base64url(signature.as_ref())
    ))
}

/// One ingest credential: a fresh participant id, and the ingest key minted for it.
///
/// The ONE place an ingest credential is built, so the primary credential an ingest
/// configuration carries and each of its redundant ones are minted alike (the model
/// gives both the same two members, `participantId` and `streamKey`).
///
/// The participant id comes off the store's monotonic counter, so it is distinct from
/// every other id this store mints — including the participant ids of the stage's
/// tokens — by construction rather than by probability.
///
/// `None` only when the credential store minted no secret, which the caller answers
/// with [`stream_key_minting_failed`] rather than by reporting a key it made up.
pub fn mint_ingest_credential(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    credentials: &::emulator_server::CredentialStore,
    region: &str,
) -> Option<IngestCredential> {
    let participant_id = mint_id(state.next_sequence(), PARTICIPANT_ID_LEN);
    let stream_key = mint_stream_key(credentials, region, &participant_id)?;
    Some(IngestCredential {
        participant_id,
        stream_key,
    })
}

/// The backup credentials a configuration with redundant ingest enabled carries beside
/// its own: [`REDUNDANT_INGEST_CREDENTIALS`] of them, each minted the single way this
/// service mints an ingest credential ([`mint_ingest_credential`]).
///
/// The ONE place the set is minted, so the creating operation and the update that switches
/// the feature on produce the same credentials in the same count, and neither decides it
/// for itself.
///
/// `None` only when the credential store minted no secret for one of them, which the
/// caller answers with [`stream_key_minting_failed`] rather than by reporting keys it made
/// up — so a configuration never ends up with part of a set.
pub fn mint_redundant_ingest_credentials(
    state: &mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    credentials: &::emulator_server::CredentialStore,
    region: &str,
) -> Option<Vec<IngestCredential>> {
    (0..REDUNDANT_INGEST_CREDENTIALS)
        .map(|_| mint_ingest_credential(state, credentials, region))
        .collect()
}

/// The ingest key a publisher broadcasts with, for the participant `participant_id` of
/// a configuration created in `region`.
///
/// The FORM is the model's: `StreamKey`'s `@pattern`
/// `^rt_[0-9]+_[a-z0-9-]+_[a-zA-Z0-9-]+_.+$` reads as
/// `rt_<account>_<region>_<participant id>_<secret>`, so the account this emulator
/// presents, the region the creating request was signed for and the participant the key
/// publishes as are its first three fields, and the secret is the last.
///
/// The secret is a credential handed to the caller, and nothing in a service may
/// construct a secret itself, so it is minted by the one component of this emulator that
/// generates secrets — the credential store — and then removed from it again, exactly as
/// [`mint_participant_token`] does: an ingest key authenticates a broadcast to a stage,
/// not a request to the emulator's API, so the emulator keeps no credential it would
/// accept as SigV4 and nothing accumulates per call.
///
/// What is minted is RENDERED in the URL-safe alphabet, through the tree's one base64url
/// coder: the whole key is a PATH SEGMENT of the address an encoder is configured with
/// (`rtmp(s)://<endpoint>/<streamKey>`, the value `StageEndpoints$rtmp`/`$rtmps` is dialed
/// with), so a character that ends a path segment or begins a query would leave a key no
/// encoder can present. The rendering changes none of the secret's bytes, so the key is as
/// hard to guess as the secret the store minted.
///
/// `None` when the store minted no secret; see [`mint_ingest_credential`].
pub fn mint_stream_key(
    credentials: &::emulator_server::CredentialStore,
    region: &str,
    participant_id: &str,
) -> Option<String> {
    let minted = credentials.create(&::emulator_server::aws_credentials::AWS_ACCESS_KEY);
    let secret = minted
        .secret(::emulator_server::aws_credentials::SECRET_ACCESS_KEY)
        .map(str::to_string);
    // The ingest key is not a credential of this emulator; see above.
    let _ = credentials.delete(&minted.id);
    Some(format!(
        "rt_{}_{region}_{participant_id}_{}",
        ::engine::emulator_account_id(),
        ::service_common::jwt::encode_base64url(secret?.as_bytes()),
    ))
}

// =============================================================================
// How this service WORDS the failures it answers
// =============================================================================
//
// Every message below is built from the REQUEST (the resource it named, the member it
// sent, the quota it met) by one rule, stated once here and used by every handler that
// answers that failure, so two operations cannot word the same refusal two ways. The
// error CODE and the HTTP status are NOT here: they stay model-derived, on the modelled
// error shape the operation declares (see `crate::scaffolded::Scaffolded::stated`).

/// The resource type an IVS ARN names (`stage`, `encoder-configuration`, …), or `None`
/// when the string carries no sixth `:`-field at all.
///
/// The type is that field up to its `/`, from the ARN form every ARN shape in this model
/// pins: `arn:aws:ivs:<region>:<account>:<type>/<id>`.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    arn.split(':').nth(5)?.split('/').next()
}

/// A kind of resource this store holds — the taggable-resource table of this service,
/// stated here once for both the read ([`resource_tags`]) and the write
/// ([`resource_tags_mut`]) side of the `/tags/{resourceArn}` surface.
///
/// A stage, an ingest configuration, an encoder configuration, a storage configuration,
/// a composition and a public key are the resource types this store can hold:
/// `CreateStage`, `CreateIngestConfiguration`, `CreateEncoderConfiguration`,
/// `CreateStorageConfiguration`, `StartComposition` and `ImportPublicKey` are the
/// operations of this
/// service that mint one, so every other resource type in its ARN shapes names nothing,
/// and a well-formed ARN naming one is answered exactly as an ARN naming a deleted stage
/// is — not found. Each remaining create is its own gap issue, and when one lands its
/// kind joins this enum: both functions below dispatch on it exhaustively, so the
/// compiler then asks each of them where that record lives and the two sides of the
/// surface cannot disagree about which resources are taggable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggedResourceKind {
    /// A stage, from `CreateStage`.
    Stage,
    /// A composition, from `StartComposition`.
    Composition,
    /// An ingest configuration, from `CreateIngestConfiguration`.
    IngestConfiguration,
    /// An encoder configuration, from `CreateEncoderConfiguration`.
    EncoderConfiguration,
    /// A storage configuration, from `CreateStorageConfiguration`.
    StorageConfiguration,
    /// A public key, from `ImportPublicKey`.
    PublicKey,
}

impl TaggedResourceKind {
    /// The kind `arn`'s resource-type field names ([`arn_resource_type`]), or `None`
    /// when that field names a resource type this store never holds — or the string
    /// carries no such field at all.
    pub fn from_arn(arn: &str) -> Option<Self> {
        match arn_resource_type(arn)? {
            "stage" => Some(Self::Stage),
            "composition" => Some(Self::Composition),
            "ingest-configuration" => Some(Self::IngestConfiguration),
            "encoder-configuration" => Some(Self::EncoderConfiguration),
            "storage-configuration" => Some(Self::StorageConfiguration),
            "public-key" => Some(Self::PublicKey),
            _ => None,
        }
    }
}

/// The tags attached to the resource `arn` names in `region`'s view, or `None` when that
/// view holds no such resource.
///
/// Which resource types can be named at all is [`TaggedResourceKind`]'s table, so this
/// reader and [`resource_tags_mut`] answer about exactly the same set of resources.
///
/// `region` is the region the tagging request was processed in, and the lookups go
/// through the region-scoped accessors, so a resource of another region is not found
/// here either: the tagging surface answers about exactly the resources that region's
/// own readers report ([`in_request_region`]).
pub fn resource_tags(
    state: &ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
    region: &str,
) -> Option<HashMap<String, String>> {
    match TaggedResourceKind::from_arn(arn)? {
        TaggedResourceKind::Stage => state.stage_in(region, arn).map(|stage| stage.tags.clone()),
        TaggedResourceKind::Composition => state
            .composition_in(region, arn)
            // This reader holds the store immutably, so a composition the clock has
            // already reclaimed is skipped rather than dropped: the reclaim deadline is
            // computed from the instant the stop recorded, so a tag listing answers for
            // exactly the compositions `GetComposition` answers for at the same instant,
            // and the next call that can write clears the record.
            .filter(|composition| !composition.is_reclaimed(SystemTime::now()))
            .map(|composition| composition.tags.clone()),
        TaggedResourceKind::IngestConfiguration => state
            .ingest_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        TaggedResourceKind::EncoderConfiguration => state
            .encoder_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        TaggedResourceKind::StorageConfiguration => state
            .storage_configuration_in(region, arn)
            .map(|configuration| configuration.tags.clone()),
        TaggedResourceKind::PublicKey => state
            .public_key_in(region, arn)
            .map(|public_key| public_key.tags.clone()),
    }
}

/// The `tags` of the resource `arn` names in `region`'s view, to be changed in place.
/// `None` when that view holds no such resource; the resource types it can name are
/// [`TaggedResourceKind`]'s, the same table [`resource_tags`] reads.
pub fn resource_tags_mut<'a>(
    state: &'a mut ComAmazonawsIvsrealtimeAmazoninteractivevideoservicerealtimeState,
    arn: &str,
    region: &str,
) -> Option<&'a mut HashMap<String, String>> {
    // A composition's own lifetime is the clock's: materialise what it already owed, so
    // a tag cannot be written onto a composition that no longer exists — and what the
    // clock owes a replication first of the two, because a replication ending can be what
    // left the composited stage empty and so earned the composition its stop.
    let now = SystemTime::now();
    materialize_due_replications(state, now);
    state.materialize_due_compositions(now);
    match TaggedResourceKind::from_arn(arn)? {
        TaggedResourceKind::Stage => state.stage_in_mut(region, arn).map(|stage| &mut stage.tags),
        TaggedResourceKind::Composition => state
            .composition_in_mut(region, arn)
            .map(|composition| &mut composition.tags),
        TaggedResourceKind::IngestConfiguration => state
            .ingest_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        TaggedResourceKind::EncoderConfiguration => state
            .encoder_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        TaggedResourceKind::StorageConfiguration => state
            .storage_configuration_in_mut(region, arn)
            .map(|configuration| &mut configuration.tags),
        TaggedResourceKind::PublicKey => state
            .public_key_in_mut(region, arn)
            .map(|public_key| &mut public_key.tags),
    }
}

#[cfg(test)]
mod tests;

// The parts this module is split into, each one concern of the store, re-exported so
// every path a handler already uses (`crate::state::…`) keeps resolving.
mod connection_tokens;
mod ingest_plane;
mod page_tokens;
mod recording_bucket;
mod refusals;
mod sessions;
mod stage_endpoints;
pub use connection_tokens::*;
pub use ingest_plane::*;
pub use page_tokens::*;
pub use recording_bucket::*;
pub use refusals::*;
pub use sessions::*;
pub use stage_endpoints::*;
