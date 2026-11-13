// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-ivs-amazoninteractivevideoservice service,
//! plus the few rules more than one operation in the channel lifecycle applies
//! (ARN minting, id minting, generating the secrets a caller ingests with,
//! pagination tokens, the two type-dependent `Channel`
//! rules the model documents, how this service WORDS the failures it answers, and
//! the modelled constraints it enforces in its own handlers).

use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints;
use aws_smithy_types::{BigDecimal, BigInteger, DateTime, Document};
use regex::Regex;
use sha2::{Digest, Sha256};
use smithy_server_runtime::{Violation, ViolationKind};

/// The two addresses a channel announces — the ingest server a client contributes to and
/// the playback URL a player fetches, both of them this emulator's own address — kept in
/// one module beside the state they are built from, and re-exported here so a handler
/// reaches them where it reaches the rest of this service's rules.
mod channel_endpoints;

pub use channel_endpoints::*;

use crate::ops::types::{
    AdConfiguration, Channel, ChannelNotBroadcasting, ConflictException, InternalServerException,
    MultitrackInputConfiguration, PlaybackKeyPair, RecordingConfiguration, RenditionConfiguration,
    ResourceNotFoundException, ServiceQuotaExceededException, ServiceUnavailable, Stream, StreamKey,
    ThumbnailConfiguration, ValidationException,
};
use crate::scaffolded::Scaffolded;

/// The region every minted ARN falls back to when the request carries no SigV4
/// credential scope to read one from (an unsigned, anonymous call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// Per-service state for com-amazonaws-ivs-amazoninteractivevideoservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsIvsAmazoninteractivevideoserviceState {
    /// Every persisted channel, keyed by its ARN. A `BTreeMap` rather than a hash
    /// map so `ListChannels` has ONE stable order, which is what makes a
    /// `nextToken` able to resume exactly where the previous page stopped.
    pub channels: BTreeMap<String, Channel>,
    /// Every persisted stream key, keyed by its ARN. `StreamKey::channel_arn` binds
    /// a key to its channel: `ListStreamKeys` filters on it and `DeleteChannel`
    /// drops every key that names the channel it removed.
    pub stream_keys: BTreeMap<String, StreamKey>,
    /// Every imported playback key pair, keyed by its ARN. A `BTreeMap` for the
    /// same reason as [`Self::channels`]: one stable order for the list that reads
    /// it. A pair's `fingerprint` is derived from the imported key material, so it
    /// is also what identifies the key itself inside this map.
    pub playback_key_pairs: BTreeMap<String, PlaybackKeyPair>,
    /// The stream each channel carries, keyed by the ARN of that channel: a
    /// channel ingests at most one stream at a time, so its ARN identifies the
    /// stream. A channel with no entry here — or one whose entry has left the
    /// `LIVE` state — is not broadcasting, which is the whole distinction
    /// `GetStream`'s `ChannelNotBroadcasting` answer carries. An entry is only
    /// ever reported through its channel, and `DeleteChannel` drops it with the
    /// channel it belongs to.
    pub streams: BTreeMap<String, Stream>,
    /// Every viewer-session revocation this store has started, keyed by the
    /// `(channelArn, viewerId)` pair it names and holding the HIGHEST viewer
    /// session version revoked for that pair so far. A `BTreeMap` for the same
    /// reason as the other two: one stable order over the pairs.
    ///
    /// `viewerId` comes from the request, so the live channel set does NOT bound
    /// this map's key space: one channel admits as many pairs as a caller invents
    /// viewer ids. [`MAX_VIEWER_SESSION_REVOCATIONS`] is what bounds it, enforced
    /// by [`start_viewer_session_revocation`] — the only writer.
    pub viewer_session_revocations: BTreeMap<(String, String), i32>,
    /// Every persisted ad configuration, keyed by its ARN. A `BTreeMap` for the
    /// same reason as `channels`: `ListAdConfigurations` is paginated, so the one
    /// stable order is what lets a `nextToken` resume where a page stopped.
    pub ad_configurations: BTreeMap<String, AdConfiguration>,
    /// Every persisted recording configuration, keyed by its ARN, each with the
    /// instant its `CREATING` → `ACTIVE` transition falls due. A `BTreeMap` for the
    /// same reason as [`Self::channels`]: one stable order for
    /// `ListRecordingConfigurations` to page over.
    pub recording_configurations: BTreeMap<String, RecordingConfigurationRecord>,
    /// How many ids this store has minted. Part of every id's seed, so no two
    /// minted ids collide even when two calls land in the same nanosecond.
    pub minted: u64,
}

/// A persisted recording configuration together with the instant at which the
/// `CREATING` → `ACTIVE` transition the clock owes it becomes due.
#[derive(Debug, Clone)]
pub struct RecordingConfigurationRecord {
    /// What every read of this resource answers.
    pub configuration: RecordingConfiguration,
    /// When the owed transition falls due; see [`RECORDING_CONFIGURATION_ACTIVATION`].
    pub active_at: SystemTime,
}

impl ComAmazonawsIvsAmazoninteractivevideoserviceState {
    /// A fresh lowercase-hex id of `bytes` bytes (so `2 * bytes` characters).
    ///
    /// Hex is a subset of the alphabet every IVS id constraint allows — an ARN's
    /// resource segment is `[a-zA-Z0-9-]+` — so a minted id always satisfies the
    /// modelled pattern. `minted` advances on every call and is hashed into the
    /// seed together with the wall clock and the process id, so repeated calls
    /// never return the same id.
    pub fn mint_hex(&mut self, label: &str, bytes: usize) -> String {
        self.minted = self.minted.wrapping_add(1);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let seed = format!("{label}|{}|{}|{nanos}", self.minted, std::process::id());
        let digest = Sha256::digest(seed.as_bytes());
        hex::encode(&digest[..bytes.min(digest.len())])
    }

    /// Materialise every `CREATING` → `ACTIVE` transition the clock already owes.
    ///
    /// A recording configuration is minted in `CREATING` and AWS turns it `ACTIVE`
    /// on its own, so the move is owed by the clock at
    /// [`RecordingConfigurationRecord::active_at`] whether or not anybody calls.
    /// Every operation that reports a recording configuration runs this first, so a
    /// read materialises only what was already due and never advances a resource
    /// because it was looked at.
    pub fn settle_recording_configurations(&mut self) {
        let now = SystemTime::now();
        for record in self.recording_configurations.values_mut() {
            if record.configuration.state == RECORDING_CONFIGURATION_CREATING
                && record.active_at <= now
            {
                record.configuration.state = RECORDING_CONFIGURATION_ACTIVE.to_string();
            }
        }
    }

    /// Whether any channel still POINTS AT `arn`.
    ///
    /// A channel is the only resource in this service that references another: it
    /// names a recording configuration, a playback restriction policy and an ad
    /// configuration, each by ARN. Those three ARN namespaces are disjoint (the
    /// modelled patterns pin the resource segment of each), so one lookup over all
    /// three members answers "is this resource still in use?" for every kind of
    /// resource a channel can reference, without the caller saying which kind it
    /// asked about.
    ///
    /// The answer is a yes/no and not the channel's ARN, because the only thing
    /// this service does with it is refuse a delete, and that refusal names the
    /// resource the request named and no other (see
    /// [`resource_referenced_message`]). A member left as the empty string
    /// references nothing and is simply never equal to a well-formed ARN.
    pub fn is_referenced_by_channel(&self, arn: &str) -> bool {
        self.channels.values().any(|channel| {
            let references = [
                channel.recording_configuration_arn.as_deref(),
                channel.playback_restriction_policy_arn.as_deref(),
                channel.ad_configuration_arn.as_deref(),
            ];
            references
                .into_iter()
                .flatten()
                .any(|reference| reference == arn)
        })
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsIvsAmazoninteractivevideoserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsIvsAmazoninteractivevideoserviceState>)
}

/// The region to mint this request's ARNs in: the region of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when the request is unsigned.
///
/// A scope region is honoured only when it can appear in an IVS ARN at all — the
/// modelled `ChannelArn` pattern pins that segment to `[a-z0-9-]+` — so a hostile
/// or nonsense scope can never make this service mint an ARN its own model
/// rejects.
pub fn request_region(parts: &http::request::Parts) -> String {
    emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region segment of an ARN (`arn:aws:ivs:<region>:<account>:<resource>`), so a
/// resource minted for an existing one lands in the same region as its parent.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// A `ChannelArn`: `arn:aws:ivs:<region>:<account>:channel/<id>`.
pub fn channel_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("channel/{id}"))
}

/// A `StreamKeyArn`: `arn:aws:ivs:<region>:<account>:stream-key/<id>`.
pub fn stream_key_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("stream-key/{id}"))
}

/// A `PlaybackKeyPairArn`: `arn:aws:ivs:<region>:<account>:playback-key/<id>`, the
/// form the modelled `PlaybackKeyPairArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:playback-key/[a-zA-Z0-9-]+$` pins.
pub fn playback_key_pair_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("playback-key/{id}"))
}

/// An `AdConfigurationArn`: `arn:aws:ivs:<region>:<account>:ad-configuration/<id>`,
/// the form the modelled `AdConfigurationArn` pattern
/// `^arn:aws:ivs:[a-z0-9-]+:[0-9]+:ad-configuration/[a-zA-Z0-9-]+$` pins.
pub fn ad_configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("ad-configuration/{id}"))
}

/// A `RecordingConfigurationArn`:
/// `arn:aws:ivs:<region>:<account>:recording-configuration/<id>`.
pub fn recording_configuration_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn("ivs", region, &format!("recording-configuration/{id}"))
}

/// The state a recording configuration is minted in, from the modelled
/// `RecordingConfigurationState` values `CREATING | CREATE_FAILED | ACTIVE`.
pub const RECORDING_CONFIGURATION_CREATING: &str = "CREATING";

/// The state a recording configuration settles in once the clock owes the move:
/// "When the state is `ACTIVE`, the configuration is ready for recording a channel
/// stream" (model doc on `RecordingConfiguration$state`).
pub const RECORDING_CONFIGURATION_ACTIVE: &str = "ACTIVE";

/// How long after `CreateRecordingConfiguration` the `CREATING` → `ACTIVE`
/// transition falls due.
///
/// AWS publishes no duration for it, so this is the emulator's own choice inside
/// the project's 250-750 ms band for a transition the clock owes. It is
/// deliberately non-zero: a caller that reads straight after the create still
/// observes `CREATING`, exactly as it would against AWS.
pub const RECORDING_CONFIGURATION_ACTIVATION: Duration = Duration::from_millis(500);

/// The ceiling on how many recording configurations this store holds, for the same
/// reason as [`MAX_CHANNELS`]: a create is the only thing that adds one and the
/// store lives for the life of the process, so caller-driven growth needs a bound.
/// Past it, `CreateRecordingConfiguration` answers the
/// `ServiceQuotaExceededException` (HTTP 402) it models. The number is the
/// emulator's own bound, not a published AWS default (AWS publishes the quota as
/// adjustable), and is set high enough that no ordinary workflow meets it.
pub const MAX_RECORDING_CONFIGURATIONS: usize = 1_000;

/// Refuse a rendition configuration whose members cannot hold together, returning
/// the message to put on a `ValidationException`.
///
/// From the model doc on `RenditionConfiguration$renditionSelection`: "If `CUSTOM`
/// is specified, a set of renditions must be specified in the `renditions` field."
/// An absent or empty list names no rendition, so neither is a set.
///
/// The refusal is stated the way this service states a cross-member rejection (see
/// [`member_constraint_message`]), the same form — including the snake_case member
/// name the recorded multitrack refusal uses — as the multitrack/container-format
/// pair, so one class of refusal reads one way across the service.
pub fn check_rendition_configuration(rendition: &RenditionConfiguration) -> Result<(), String> {
    let selection = rendition.rendition_selection.as_deref().unwrap_or("");
    let renditions = rendition.renditions.as_deref().unwrap_or(&[]);
    if selection == "CUSTOM" && renditions.is_empty() {
        return Err(member_constraint_message(
            "rendition_configuration.renditions",
            "invalid value for rendition configuration rendition selection and renditions",
        ));
    }
    Ok(())
}

/// The rendition configuration to persist for what the caller sent: the structure
/// itself, materialised whether or not the request named it, with every member the
/// request omitted filled from the model's documented default.
///
/// From the model doc on `RenditionConfiguration$renditionSelection`: "Indicates
/// which set of renditions are recorded for a stream. … Default: `ALL`." A documented
/// default is what a member reports when the request chose nothing for it, and a
/// structure every member of which has one is itself defaulted rather than dropped —
/// which is what the recorded answers show: a recording configuration created with no
/// `renditionConfiguration` member reports the default selection all the same. So an
/// absent structure is the same request as an empty one.
pub fn effective_rendition_configuration(
    requested: Option<RenditionConfiguration>,
) -> RenditionConfiguration {
    let mut rendition = requested.unwrap_or_default();
    if rendition
        .rendition_selection
        .as_deref()
        .is_none_or(str::is_empty)
    {
        rendition.rendition_selection = Some("ALL".to_string());
    }
    rendition
}

/// The thumbnail configuration to persist for what the caller sent: the structure
/// itself, materialised whether or not the request named it, with every member the
/// request omitted filled from the model's documented default.
///
/// From the model docs on `ThumbnailConfiguration`: `recordingMode` "Default:
/// `INTERVAL`"; `targetIntervalSeconds` "is configurable (and required) only if
/// `recordingMode` is `INTERVAL`. Default: 60"; `storage` "Default: `SEQUENTIAL`";
/// `resolution` "Default: Null (source resolution is returned)", so an omitted
/// resolution — and only it — stays absent. As with
/// [`effective_rendition_configuration`], an omitted structure is the same request as
/// an empty one, because every other member of it has a documented default.
pub fn effective_thumbnail_configuration(
    requested: Option<ThumbnailConfiguration>,
) -> ThumbnailConfiguration {
    let mut thumbnail = requested.unwrap_or_default();
    if thumbnail.recording_mode.as_deref().is_none_or(str::is_empty) {
        thumbnail.recording_mode = Some("INTERVAL".to_string());
    }
    if thumbnail.recording_mode.as_deref() == Some("INTERVAL")
        && thumbnail.target_interval_seconds.is_none()
    {
        thumbnail.target_interval_seconds = Some(60);
    }
    if thumbnail.storage.is_none() {
        thumbnail.storage = Some(vec!["SEQUENTIAL".to_string()]);
    }
    thumbnail
}

/// The ceiling on how many channels this store holds: the cap that bounds
/// caller-driven growth.
///
/// A generated emulator keeps its resources in memory for the life of the process
/// and a create is the only thing that adds one, so the store needs a maximum entry
/// count — neither a TTL nor an eviction policy is available to it, because
/// `GetChannel` must still answer what `CreateChannel` created. `CreateChannel`
/// refuses past this cap with the service's own modelled
/// `ServiceQuotaExceededException` (HTTP 402), which IVS declares on it. The number is
/// the emulator's own bound, not a published AWS default: AWS publishes the channel
/// quota as adjustable, so no single value is the contract, and the cap is set high
/// enough that no ordinary workflow meets it.
pub const MAX_CHANNELS: usize = 5_000;

/// How many stream keys one channel may hold.
///
/// Unlike [`MAX_CHANNELS`] this is not an emulator-chosen bound: a channel is created
/// WITH its one stream key and the quota is one key per channel, so the recorded
/// behaviour is that a second `CreateStreamKey` on the same channel is refused with
/// `ServiceQuotaExceededException` (HTTP 402) and that deleting the existing key frees
/// the slot again. `CreateStreamKey` is the only operation that can exceed it.
pub const MAX_STREAM_KEYS_PER_CHANNEL: usize = 1;

/// The name the stream-key quota's refusal states — the quota is on the resource, so
/// the message names the resource type rather than the operation. See
/// [`quota_exceeded_message`].
pub const STREAM_KEY_QUOTA_RESOURCE: &str = "stream-key";

/// The same, for the channel cap [`MAX_CHANNELS`] bounds.
pub const CHANNEL_QUOTA_RESOURCE: &str = "channel";

/// The same, for the recording-configuration cap [`MAX_RECORDING_CONFIGURATIONS`]
/// bounds.
pub const RECORDING_CONFIGURATION_QUOTA_RESOURCE: &str = "recording-configuration";

/// The same, for the ad-configuration cap [`MAX_AD_CONFIGURATIONS`] bounds.
pub const AD_CONFIGURATION_QUOTA_RESOURCE: &str = "ad-configuration";

/// The same, for the cap on how many `(channelArn, viewerId)` pairs this store tracks a
/// viewer-session revocation for ([`MAX_VIEWER_SESSION_REVOCATIONS`]). Both surfaces
/// that start a revocation state it, so neither words that refusal on its own.
pub const VIEWER_SESSION_REVOCATION_QUOTA_RESOURCE: &str = "viewer-session-revocation";

/// The ceiling on how many playback key pairs this store holds, for the reason
/// given on [`MAX_CHANNELS`]: `ImportPlaybackKeyPair` is the only thing that adds
/// one, it lives for the life of the process, and the operation models
/// `ServiceQuotaExceededException` (HTTP 402) for exactly this refusal. The number
/// is the emulator's own bound, not a published AWS default — AWS publishes the
/// playback-key-pair quota as adjustable — and is set high enough that no ordinary
/// workflow meets it.
pub const MAX_PLAYBACK_KEY_PAIRS: usize = 1_000;

/// The ceiling on how many `(channelArn, viewerId)` pairs this store tracks a
/// viewer-session revocation for — a caller-driven growth path bounded for the same
/// reason as [`MAX_CHANNELS`]: the store has neither a TTL nor an eviction policy,
/// and a pair's `viewerId` comes from the request, so nothing else bounds how many
/// pairs a caller can name. IVS publishes no quota for revocations, so like the
/// others this number is the emulator's own bound, set high enough that no ordinary
/// workflow meets it.
///
/// [`start_viewer_session_revocation`] enforces it: past the ceiling a pair that is
/// not already tracked is REFUSED rather than recorded, and the batch form reports
/// that refusal as the per-item failure it is.
pub const MAX_VIEWER_SESSION_REVOCATIONS: usize = 10_000;

/// The ceiling on how many ad configurations this store holds. See [`MAX_CHANNELS`]
/// for why a cap exists at all: `CreateAdConfiguration` is the only thing that adds
/// one, it never expires, and past the cap the operation answers the
/// `ServiceQuotaExceededException` (HTTP 402) IVS models on it. The number is the
/// emulator's own bound, not a published AWS default.
pub const MAX_AD_CONFIGURATIONS: usize = 1_000;

/// How many stream keys are currently bound to `channel_arn`, which is what
/// [`MAX_STREAM_KEYS_PER_CHANNEL`] bounds.
pub fn stream_key_count(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> usize {
    state
        .stream_keys
        .values()
        .filter(|key| key.channel_arn.as_deref() == Some(channel_arn))
        .count()
}

/// Mint a stream key bound to `channel_arn` and persist it.
///
/// Shared by `CreateChannel` (which mints a channel's first key) and
/// `CreateStreamKey` (which adds another), so both produce keys of one form and
/// both land in the one `stream_keys` map `ListStreamKeys` reads.
///
/// `None` when no secret was generated for the key's value; see
/// [`generated_secret`].
pub fn mint_stream_key(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    credentials: &::emulator_server::CredentialStore,
    region: &str,
    channel_arn: &str,
    tags: Option<std::collections::HashMap<String, String>>,
) -> Option<StreamKey> {
    let id = state.mint_hex("stream-key", 6);
    // The key's last field is the ingest SECRET, so it is generated rather than
    // derived (see [`generated_secret`]); the `[0-9a-f]` alphabet and the width are
    // the ones this service's stream keys carry.
    let secret = generated_secret(credentials, STREAM_KEY_SECRET_ALPHABET, STREAM_KEY_SECRET_LEN)?;
    let arn = stream_key_arn(region, &id);
    let key = StreamKey {
        arn: Some(arn.clone()),
        value: Some(format!("sk_{region}_{id}_{secret}")),
        channel_arn: Some(channel_arn.to_string()),
        tags: Some(tags.unwrap_or_default()),
    };
    state.stream_keys.insert(arn, key.clone());
    Some(key)
}

/// Which characters a secret this service hands out is written in.
///
/// Neither of the two shapes that carry one (`StreamKeyValue` and `SrtPassphrase`) is
/// constrained by the model — `smithy_query(query="shape",
/// shape="StreamKeyValue,SrtPassphrase")` gives a bare `@sensitive` string for each — so
/// the alphabet of each is this service's own, kept here beside the mint that applies it.
type SecretAlphabet = fn(&char) -> bool;

/// A stream key's last field reads as lowercase hexadecimal.
const STREAM_KEY_SECRET_ALPHABET: SecretAlphabet = is_lowercase_hex;

/// How many characters that field carries.
const STREAM_KEY_SECRET_LEN: usize = 32;

/// An `Srt` passphrase reads as mixed-case alphanumerics: the class AWS's auto-generated
/// SRT ingest passphrase is shown in, and an alphabet an SRT client passes through
/// unescaped in the `passphrase` parameter of its ingest URL.
const SRT_PASSPHRASE_ALPHABET: SecretAlphabet = char::is_ascii_alphanumeric;

/// How many characters an `Srt` passphrase carries.
///
/// The model states no width for `SrtPassphrase`, and SRT itself admits a passphrase of
/// 10 to 79 characters, so this is the width of this service's generated ingest secrets —
/// the same as [`STREAM_KEY_SECRET_LEN`], well inside what an SRT client accepts.
const SRT_PASSPHRASE_LEN: usize = 32;

fn is_lowercase_hex(character: &char) -> bool {
    character.is_ascii_digit() || matches!(*character, 'a'..='f')
}

/// The auto-generated `passphrase` of a channel's `Srt` block: the one secret that
/// enables encryption on this channel's SRT ingest.
///
/// `None` when no secret was generated for it; see [`generated_secret`].
pub fn mint_srt_passphrase(credentials: &::emulator_server::CredentialStore) -> Option<String> {
    generated_secret(credentials, SRT_PASSPHRASE_ALPHABET, SRT_PASSPHRASE_LEN)
}

/// `len` characters over `alphabet`, cut from a freshly GENERATED secret.
///
/// A stream key's value and an `Srt` passphrase are secrets handed to a caller — an
/// encoder presents them to ingest — but neither is a credential of this emulator and
/// neither may ever be accepted as one. Nothing in a service may construct a secret
/// itself, so the characters come from a secret minted by the one component that
/// generates them, the emulator's credential store, which is then removed from that store
/// again: the caller gets the secret, the emulator keeps no credential it would accept,
/// and nothing accumulates per call. Only the characters `alphabet` admits are kept,
/// because the shape being filled carries no others.
///
/// This is what separates a secret from an id: [`ComAmazonawsIvsAmazoninteractivevideoserviceState::mint_hex`]
/// derives an id from the store's mint counter, the clock and the process id, which makes
/// ids unique by construction — fine for a channel id or a host label, and wrong for a
/// secret, which must not be reproducible from the sequence of calls that produced it.
///
/// `None` when the store generated no secret, or one holding fewer than `len` usable
/// characters, which leaves the operation to refuse ([`secret_unavailable`]) rather than
/// report a value it made up.
fn generated_secret(
    credentials: &::emulator_server::CredentialStore,
    alphabet: SecretAlphabet,
    len: usize,
) -> Option<String> {
    let minted = credentials.create(&::emulator_server::aws_credentials::AWS_SESSION);
    let secret: String = minted
        .secret(::emulator_server::aws_credentials::SESSION_TOKEN)
        .unwrap_or_default()
        .chars()
        .filter(alphabet)
        .take(len)
        .collect();
    // Neither secret is a credential of this emulator; see above.
    let _ = credentials.delete(&minted.id);
    (secret.chars().count() == len).then_some(secret)
}

/// The `StreamState` of a channel that is ingesting, from the modelled
/// `StreamState` enum (`LIVE | OFFLINE`, see
/// [`crate::ops::types::STREAM_STATE_VALUES`]).
pub const STREAM_STATE_LIVE: &str = "LIVE";

/// The `StreamHealth` of a stream whose ingest session is connected and
/// delivering, from the modelled `StreamHealth` enum (`HEALTHY | STARVING |
/// UNKNOWN`, see [`crate::ops::types::STREAM_HEALTH_VALUES`]).
pub const STREAM_HEALTH_HEALTHY: &str = "HEALTHY";

/// A fresh `StreamId`.
///
/// The modelled shape is exactly 26 characters and matches `^st-[a-zA-Z0-9]+$`,
/// so an id is the `st-` prefix plus 23 characters from that alphabet; hex is a
/// subset of it, and [`ComAmazonawsIvsAmazoninteractivevideoserviceState::mint_hex`]
/// never repeats a value.
fn mint_stream_id(state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState) -> String {
    let digest = state.mint_hex("stream", 12);
    format!("st-{}", &digest[..23])
}

/// The LIVE stream `channel_arn` is currently ingesting, or `None` when that
/// channel is not broadcasting.
///
/// Only a stream in the `LIVE` state is an active one: an entry that has left
/// that state is a channel whose broadcast has ended, which every operation
/// about the active stream must treat exactly like a channel that never had one.
pub fn live_stream(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> Option<Stream> {
    state
        .streams
        .get(channel_arn)
        .filter(|stream| stream.state.as_deref() == Some(STREAM_STATE_LIVE))
        .cloned()
}

/// Start the stream an ingest session has brought up on `channel_arn`, persist it
/// and return it.
///
/// One place mints a stream, so every operation that reports one answers the same
/// representation: the stream is bound to the channel it is ingested on, is played
/// back from that channel's own `playbackUrl`, and starts at the instant the
/// ingest connected. `viewerCount` is 0 because no playback session is attached to
/// it yet, and `health` is `HEALTHY` because an ingest session is connected and
/// delivering — a measurement that would move it to `STARVING` belongs to whoever
/// is feeding the ingest. A channel that is ALREADY broadcasting keeps the stream
/// it has: IVS holds one stream per channel, so a second ingest cannot mint a
/// second stream id for the same channel.
pub fn start_stream(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> Stream {
    if let Some(live) = live_stream(state, channel_arn) {
        return live;
    }
    let stream_id = mint_stream_id(state);
    let stream = Stream {
        channel_arn: Some(channel_arn.to_string()),
        stream_id: Some(stream_id),
        // The playback URL is not stored with the stream: it is composed for the
        // request that reports it, from the channel this stream is ingested on
        // (`announced_stream`), so a stream announces the address its reader reached
        // this emulator at rather than the one some earlier call did.
        playback_url: None,
        start_time: Some(::aws_smithy_types::DateTime::from(
            std::time::SystemTime::now(),
        )),
        state: Some(STREAM_STATE_LIVE.to_string()),
        health: Some(STREAM_HEALTH_HEALTHY.to_string()),
        viewer_count: 0,
    };
    state.streams.insert(channel_arn.to_string(), stream.clone());
    stream
}

/// The one way [`start_viewer_session_revocation`] can fail: the store is at
/// [`MAX_VIEWER_SESSION_REVOCATIONS`] and the pair is not one it already tracks.
///
/// A type rather than a message, so each caller words the refusal the way its own
/// response shape needs and the store holds no wire text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerSessionRevocationCapReached {
    /// The cap that was reached, so a caller can name it.
    pub cap: usize,
}

/// Start revoking `viewer_id`'s sessions on `channel_arn` up to and including
/// `versions_less_than_or_equal_to`, and answer the version threshold now in force
/// for that pair — or [`ViewerSessionRevocationCapReached`] when tracking a NEW pair
/// would take this store past [`MAX_VIEWER_SESSION_REVOCATIONS`], which is the only
/// way this can fail.
///
/// From the model doc on `viewerSessionVersionsLessThanOrEqualTo`: "An optional
/// filter on which versions of the viewer session to revoke. All versions less than
/// or equal to the specified version will be revoked." A later call naming a LOWER
/// version therefore cannot un-revoke what an earlier call already revoked, so the
/// stored threshold is the maximum over the calls — which also makes a repeated call
/// idempotent.
///
/// Raising an EXISTING pair's threshold adds no entry, so it is never refused: the
/// cap bounds how many pairs are tracked, not how often one is revoked.
///
/// Shared so that every operation that starts a revocation (the single-pair
/// `StartViewerSessionRevocation` and the batch form that the model documents as
/// "performs StartViewerSessionRevocation on multiple channel ARN and viewer ID
/// pairs") writes one record of one form and honours one cap.
pub fn start_viewer_session_revocation(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    viewer_id: &str,
    versions_less_than_or_equal_to: i32,
) -> Result<i32, ViewerSessionRevocationCapReached> {
    let key = (channel_arn.to_string(), viewer_id.to_string());
    if !state.viewer_session_revocations.contains_key(&key)
        && state.viewer_session_revocations.len() >= MAX_VIEWER_SESSION_REVOCATIONS
    {
        // A record whose channel is gone can never be revoked against again — this
        // service mints a fresh random id for every channel, so a deleted ARN never
        // comes back — so those dead records are dropped before the cap is weighed.
        let ComAmazonawsIvsAmazoninteractivevideoserviceState {
            channels,
            viewer_session_revocations,
            ..
        } = state;
        viewer_session_revocations.retain(|(arn, _), _| channels.contains_key(arn));
        if state.viewer_session_revocations.len() >= MAX_VIEWER_SESSION_REVOCATIONS {
            return Err(ViewerSessionRevocationCapReached {
                cap: MAX_VIEWER_SESSION_REVOCATIONS,
            });
        }
    }
    let threshold = state
        .viewer_session_revocations
        .entry(key)
        .or_insert(versions_less_than_or_equal_to);
    *threshold = (*threshold).max(versions_less_than_or_equal_to);
    Ok(*threshold)
}

/// The `preset` a channel of `channel_type` carries, given the `requested` value.
///
/// From the model doc on `Channel$preset`: "Optional transcode preset for the
/// channel. This is selectable only for `ADVANCED_HD` and `ADVANCED_SD` channel
/// types. For those channel types, the default `preset` is
/// `HIGHER_BANDWIDTH_DELIVERY`. For other channel types (`BASIC` and `STANDARD`),
/// `preset` is the empty string."
pub fn effective_preset(channel_type: &str, requested: Option<&str>) -> String {
    match channel_type {
        "ADVANCED_HD" | "ADVANCED_SD" => requested
            .filter(|preset| !preset.is_empty())
            .unwrap_or("HIGHER_BANDWIDTH_DELIVERY")
            .to_string(),
        _ => String::new(),
    }
}

/// The multitrack input configuration to persist for what the caller sent: the
/// structure itself, materialised whether or not the request named it, with every
/// member the request omitted filled from the default the service applies.
///
/// `MultitrackInputConfiguration$enabled` is modelled `@default(false)`, so a channel
/// that named no multitrack input has multitrack input disabled. The model documents
/// `policy` and `maximumResolution` only as "Required if `enabled` is `true`" and
/// gives neither a default in prose, so the pair a channel reports when it chose
/// neither comes from the recorded answers, which carry the permissive policy and the
/// largest resolution of each enumeration (`MULTITRACK_POLICY_VALUES`,
/// `MULTITRACK_MAXIMUM_RESOLUTION_VALUES`) — never a value outside them.
pub fn effective_multitrack_input_configuration(
    requested: Option<MultitrackInputConfiguration>,
) -> MultitrackInputConfiguration {
    let mut multitrack = requested.unwrap_or_default();
    if multitrack.policy.as_deref().is_none_or(str::is_empty) {
        multitrack.policy = Some(MULTITRACK_POLICY_DEFAULT.to_string());
    }
    if multitrack
        .maximum_resolution
        .as_deref()
        .is_none_or(str::is_empty)
    {
        multitrack.maximum_resolution = Some(MULTITRACK_MAXIMUM_RESOLUTION_DEFAULT.to_string());
    }
    multitrack
}

/// The `policy` a channel reports when neither its request nor an earlier update
/// named one: multitrack input is ALLOWED rather than required, the permissive member
/// of `MULTITRACK_POLICY_VALUES`.
const MULTITRACK_POLICY_DEFAULT: &str = "ALLOW";

/// The `maximumResolution` a channel reports when neither its request nor an earlier
/// update named one: the largest member of `MULTITRACK_MAXIMUM_RESOLUTION_VALUES`,
/// so the default caps nothing the channel type itself admits.
const MULTITRACK_MAXIMUM_RESOLUTION_DEFAULT: &str = "FULL_HD";

/// The failover-slate configuration ARN a channel projection reports.
///
/// A channel's optional configuration ARNs are reported as a PRESENT key holding the
/// empty string while nothing is configured, never as an absent member — the rule the
/// model writes out for each one it declares ("Default: \"\" (empty string, recording is
/// disabled)" on `Channel$recordingConfigurationArn`, the same sentence on
/// `playbackRestrictionPolicyArn`), and the way every handler here persists them
/// (`Some(input.recording_configuration_arn.unwrap_or_default())`). A client decides
/// "is this configured?" by comparing the value with the empty string, so an absent key
/// is a different answer from "nothing is configured".
///
/// The failover slate is one of those ARNs on the wire and the one this model revision
/// does not declare on `Channel` or `ChannelSummary`
/// (`smithy_query(query="shape",shape="Channel,ChannelSummary")`), so it has no field on
/// the stored record and no input member to set it: every channel this store holds has
/// no failover slate, and this is the value that reports it. It is read by the channel
/// wire projection in `crate::service`, which is where a member no shape declares can be
/// written at all.
pub const CHANNEL_FAILOVER_SLATE_CONFIGURATION_ARN: &str = "";

/// Refuse a channel whose `containerFormat` and multitrack configuration cannot
/// hold together, returning the message to put on a `ValidationException`.
///
/// From the model doc on `Channel$containerFormat`: "If
/// `multitrackInputConfiguration` is specified and `enabled` is `true`, then
/// `containerFormat` is required and must be set to `FRAGMENTED_MP4`. Otherwise,
/// `containerFormat` may be set to `TS` or `FRAGMENTED_MP4`."
///
/// The refusal is stated the way IVS states a cross-member rejection (see
/// [`member_constraint_message`]): it names the member whose value cannot hold and the
/// constraint the pair violated.
pub fn check_container_format(
    container_format: &str,
    multitrack: Option<&MultitrackInputConfiguration>,
) -> Result<(), String> {
    if multitrack.is_some_and(|config| config.enabled) && container_format != "FRAGMENTED_MP4" {
        return Err(member_constraint_message(
            MULTITRACK_ENABLED_MEMBER,
            MULTITRACK_CONTAINER_FORMAT_CONSTRAINT,
        ));
    }
    Ok(())
}

/// The member a multitrack/container-format rejection names: the `enabled` flag of the
/// request's `multitrackInputConfiguration`, which is the value that makes
/// `containerFormat` constrained at all.
///
/// Spelled in snake_case because that is how the RECORDED refusal spells it
/// ("Request member: multitrack_input_configuration.enabled failed to satisfy the
/// constraints: …"), not by analogy with anything else: this message is IVS's own text
/// for a cross-member rejection, where the member name is part of what was recorded. It
/// is NOT the dotted lowerCamel path [`MemberRule::path`] and
/// [`violation_clause`] use — those state a CONSTRAINT-WALK violation, whose path the
/// framework builds from the member names the request sends.
const MULTITRACK_ENABLED_MEMBER: &str = "multitrack_input_configuration.enabled";

/// The constraint a multitrack/container-format rejection names: the pair of values that
/// cannot hold together, from the `Channel$containerFormat` model doc.
const MULTITRACK_CONTAINER_FORMAT_CONSTRAINT: &str =
    "invalid value for multitrack input configuration enabled and container format";

/// The stamp a `ListChannels` page token carries, so a token issued for one list
/// cannot be replayed against another.
pub const CHANNEL_PAGE: &str = "channel";

/// The stamp a `ListStreamKeys` page token carries. See [`CHANNEL_PAGE`].
pub const STREAM_KEY_PAGE: &str = "stream-key";

/// The stamp a `ListRecordingConfigurations` page token carries. See
/// [`CHANNEL_PAGE`].
pub const RECORDING_CONFIGURATION_PAGE: &str = "recording-configuration";

/// The stamp a `ListPlaybackKeyPairs` page token carries. See [`CHANNEL_PAGE`].
pub const PLAYBACK_KEY_PAIR_PAGE: &str = "playback-key";

/// The stamp a `ListAdConfigurations` page token carries. See [`CHANNEL_PAGE`].
pub const AD_CONFIGURATION_PAGE: &str = "ad-configuration";

/// The prefix of every ARN this service mints, and so of every page boundary a
/// token may legitimately name.
const ARN_PREFIX: &str = "arn:aws:ivs:";

/// Encode a page boundary as an opaque `PaginationToken`.
///
/// The token carries the sort key of the LAST item the page returned, so the next
/// page is everything ordered strictly after it — a resume point that stays correct
/// when items are added or removed between pages, which a positional offset would
/// not. It is STAMPED with `kind` and a format version so a forged or replayed value
/// is rejected rather than honoured as some other list's boundary. Lowercase hex
/// keeps the token inside the modelled `PaginationToken` pattern
/// `^[a-zA-Z0-9+/=_-]*$` for any key, and the stamp plus a 128-byte ARN encodes to
/// well under the modelled 1024-character bound.
pub fn encode_page_token(kind: &str, last_key: &str) -> String {
    hex::encode(format!("ivs:1:{kind}:{last_key}").as_bytes())
}

/// The page boundary a `kind` token names, or `None` when it is not a token this
/// service issued for that list — which the caller is told as a
/// `ValidationException`, never as a silently truncated page.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let key = decoded.strip_prefix(&format!("ivs:1:{kind}:"))?;
    // Every key this service pages on is an ARN it minted, so anything else is a
    // boundary it could not have issued.
    key.starts_with(ARN_PREFIX).then(|| key.to_string())
}

/// The page size a list serves for the `maxResults` the caller sent, bounded by the
/// `maximum` that list's own modelled `max*Results` shape declares.
///
/// Shared by every list operation, so the three cannot drift apart: an absent
/// `maxResults` means the whole page, and a value the modelled range would refuse —
/// which the framework has already rejected before any handler runs — can never
/// reach [`page`] as a zero or an unbounded size.
pub fn page_size(requested: Option<i32>, maximum: i32) -> usize {
    let maximum = usize::try_from(maximum).unwrap_or(1).max(1);
    usize::try_from(requested.unwrap_or(i32::MAX))
        .unwrap_or(maximum)
        .clamp(1, maximum)
}

/// Cut one page out of an ordered, already-filtered `(key, item)` sequence, and
/// issue the token that resumes after it.
///
/// Shared by every list operation so they cannot drift apart: one more than the page
/// is read, so a token is issued only when a further page really exists, and the
/// token names the last key this page returned. A page holds at least one item —
/// every modelled `max*Results` has a minimum of 1, and a zero-sized page would
/// answer an empty list with no token, which reads as "nothing left" while items
/// remain.
pub fn page<'a, T>(
    kind: &str,
    ordered: impl Iterator<Item = (&'a String, &'a T)>,
    after: Option<&str>,
    page_size: usize,
) -> (Vec<&'a T>, Option<String>) {
    let page_size = page_size.max(1);
    let mut window: Vec<(&'a String, &'a T)> = ordered
        .filter(|(key, _)| after.is_none_or(|boundary| key.as_str() > boundary))
        .take(page_size.saturating_add(1))
        .collect();
    let next_token = if window.len() > page_size {
        window.truncate(page_size);
        window
            .last()
            .map(|(key, _)| encode_page_token(kind, key.as_str()))
    } else {
        None
    };
    (
        window.into_iter().map(|(_, item)| item).collect(),
        next_token,
    )
}

// =============================================================================
// How this service WORDS the failures it answers
// =============================================================================
//
// Every message below is built from the REQUEST (the ARN it named, the member it
// sent, the quota it met) by one rule, stated once here and used by every handler
// that answers that failure, so two operations cannot word the same refusal two
// ways. The error CODE and the HTTP status are not here: they stay model-derived,
// on the modelled error shape the operation declares (see
// `crate::scaffolded::Scaffolded::stated`).

/// How IVS states a request that named a resource it does not hold: the ARN the
/// request named, under a fixed template.
///
/// Recorded identically for `GetChannel`, `UpdateChannel`, `CreateStreamKey`,
/// `GetPlaybackKeyPair` and `StartViewerSessionRevocation`, which is what makes it a
/// rule of the service rather than wording of one operation.
pub fn resource_not_found_message(arn: &str) -> String {
    format!("Resource: {arn} not found")
}

/// How IVS states an operation on a channel that is not currently ingesting
/// (`ChannelNotBroadcasting`, HTTP 404): it names the channel ARN the request sent.
pub fn channel_offline_message(channel_arn: &str) -> String {
    format!("Channel: {channel_arn} is not currently online")
}

/// How IVS states a delete refused because a channel still POINTS AT the resource the
/// request named (`ConflictException`, HTTP 409): one attachment sentence naming the
/// action it refused — `ivs:` plus the operation name — and the resource that request
/// named, and nothing else. The channel holding the reference is NOT part of it, so a
/// refusal never discloses a resource the caller did not name.
///
/// `resource_kind` is the kind the sentence spells before the ARN, for the operations
/// whose sentence names it (`ad configuration`); where the service names the ARN alone
/// it is `None`. Both per-operation parts come from the call site, because one delete's
/// sentence is not another's.
///
/// AWS documents the status and the code for this refusal ("If you try to delete a
/// recording configuration that is associated with a channel, you will get an error
/// (409 ConflictException)"); the sentence is the one the service answers for the
/// class, with the request's ARN as its only variable part.
pub fn resource_referenced_message(
    operation: &str,
    resource_kind: Option<&str>,
    arn: &str,
) -> String {
    let subject = match resource_kind {
        Some(kind) => format!("{kind} {arn}"),
        None => arn.to_string(),
    };
    format!("Unable to perform ivs:{operation} while {subject} is still attached to a channel")
}

/// How IVS states a quota refusal (`ServiceQuotaExceededException`, HTTP 402): it names
/// the resource the quota is on and the limit that was reached, not the operation.
pub fn quota_exceeded_message(resource: &str, limit: usize) -> String {
    format!("Resource: {resource} quota exceeded limit: {limit}")
}

/// How IVS states an import of playback key material it already holds
/// (`ConflictException`, HTTP 409): one fixed sentence naming the member whose value is
/// already present, with nothing of the request in it.
pub const DUPLICATE_PLAYBACK_KEY_MESSAGE: &str =
    "A playback-key resource already exists with the given publicKeyMaterial";

/// How IVS states a rejection of one member's VALUE for a reason the model cannot
/// express — a pair of members that cannot hold together: it names the member and the
/// constraint the request violated.
///
/// Recorded for the multitrack/container-format pair (see [`check_container_format`]).
pub fn member_constraint_message(member: &str, constraint: &str) -> String {
    format!("Request member: {member} failed to satisfy the constraints: {constraint}")
}

/// How IVS states a value outside a modelled `@pattern`: the operation, the dotted
/// member path, and the pattern itself.
pub fn pattern_violation_message(operation: &str, path: &str, pattern: &str) -> String {
    format!("{operation}.{path} does not match the required pattern {pattern}")
}

/// How IVS states a value below a modelled `@length` minimum: the operation, the dotted
/// member path, the minimum, and what the request actually sent.
pub fn min_length_violation_message(
    operation: &str,
    path: &str,
    minimum: usize,
    found: usize,
) -> String {
    format!("{operation}.{path} is required to have a minimum length of {minimum}, found {found}")
}

/// How IVS states a value above a modelled `@length` maximum. The minimum form above is
/// the recorded one; this is that same sentence with the bound it states, because one
/// derivation covers both ends of a `@length`.
pub fn max_length_violation_message(
    operation: &str,
    path: &str,
    maximum: usize,
    found: usize,
) -> String {
    format!("{operation}.{path} is required to have a maximum length of {maximum}, found {found}")
}

/// The wire error code IVS's TAGGING surface answers for an ARN that names no resource.
///
/// It is NOT the `ResourceNotFoundException` the rest of the service answers and the
/// model declares for these three operations: the recorded responses for
/// `ListTagsForResource` / `TagResource` / `UntagResource` carry the distinct code
/// `NotFoundException`, which is why it travels as
/// `crate::scaffolded::Scaffolded::unmodeled` rather than as a modelled variant.
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The HTTP status that recorded tagging not-found response carries.
pub const TAGGING_NOT_FOUND_STATUS: u16 = 404;

/// The text that recorded tagging not-found response carries: one fixed,
/// resource-agnostic string, built from nothing in the request.
pub const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

// The failures themselves, each built once from its message rule. `variant` is the
// operation's OWN modelled error constructor — `GetChannelErrorModeled::
// ResourceNotFoundException`, say — so the compiler still refuses a failure the
// operation's model does not declare, and the error code and HTTP status stay derived
// from that modelled shape. Only the BODY is this service's own rendering: AWS puts the
// text under the key `message`, which the modelled `exceptionMessage` member is not (see
// `crate::scaffolded::STATED_MESSAGE`). The modelled struct is filled in with the same
// text, so the error is complete as a Rust value too.

/// The `ResourceNotFoundException` (HTTP 404) this service answers when a request names
/// a resource it does not hold. See [`resource_not_found_message`].
pub fn resource_not_found<E>(
    variant: fn(ResourceNotFoundException) -> E,
    arn: &str,
) -> Scaffolded<E> {
    let message = resource_not_found_message(arn);
    Scaffolded::stated(
        variant(ResourceNotFoundException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ChannelNotBroadcasting` (HTTP 404) this service answers when an operation needs
/// a channel that is ingesting and the channel is not. See [`channel_offline_message`].
pub fn channel_not_broadcasting<E>(
    variant: fn(ChannelNotBroadcasting) -> E,
    channel_arn: &str,
) -> Scaffolded<E> {
    let message = channel_offline_message(channel_arn);
    Scaffolded::stated(
        variant(ChannelNotBroadcasting {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ServiceQuotaExceededException` (HTTP 402) this service answers when a create
/// would exceed a quota. See [`quota_exceeded_message`].
pub fn quota_exceeded<E>(
    variant: fn(ServiceQuotaExceededException) -> E,
    resource: &str,
    limit: usize,
) -> Scaffolded<E> {
    let message = quota_exceeded_message(resource, limit);
    Scaffolded::stated(
        variant(ServiceQuotaExceededException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ConflictException` (HTTP 409) this service answers when a delete would leave a
/// channel pointing at something that no longer exists. See
/// [`resource_referenced_message`] for the sentence and for what `operation` and
/// `resource_kind` are.
pub fn resource_referenced<E>(
    variant: fn(ConflictException) -> E,
    operation: &str,
    resource_kind: Option<&str>,
    arn: &str,
) -> Scaffolded<E> {
    let message = resource_referenced_message(operation, resource_kind, arn);
    Scaffolded::stated(
        variant(ConflictException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ConflictException` (HTTP 409) this service answers when the resource a create
/// would mint is already there, with `message` from one of the rules above.
pub fn already_exists<E>(
    variant: fn(ConflictException) -> E,
    message: impl Into<String>,
) -> Scaffolded<E> {
    let message = message.into();
    Scaffolded::stated(
        variant(ConflictException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `ValidationException` (HTTP 400) this service answers for a request it refuses on
/// its own, with `message` from one of the rules above.
pub fn validation<E>(variant: fn(ValidationException) -> E, message: String) -> Scaffolded<E> {
    Scaffolded::stated(
        variant(ValidationException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The `InternalServerException` (HTTP 500) a handler answers when this service's store
/// is unreachable — the one failure that is the emulator's own, not the request's.
pub fn store_unavailable<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    let message = STORE_UNAVAILABLE_MESSAGE.to_string();
    Scaffolded::stated(
        variant(InternalServerException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The text [`store_unavailable`] states. AWS documents no message for this (it is an
/// emulator fault, not a service behaviour), so it says plainly what failed.
pub const STORE_UNAVAILABLE_MESSAGE: &str = "the IVS store is unavailable";

/// The `InternalServerException` (HTTP 500) a create answers when the emulator's
/// credential store generated no secret for the ingest credential it owes the caller —
/// the emulator's own failure, like [`store_unavailable`]. A service never constructs a
/// secret itself, so a create that cannot obtain one refuses rather than reporting a
/// value it made up.
pub fn secret_unavailable<E>(variant: fn(InternalServerException) -> E) -> Scaffolded<E> {
    let message = SECRET_UNAVAILABLE_MESSAGE.to_string();
    Scaffolded::stated(
        variant(InternalServerException {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The text [`secret_unavailable`] states, worded like [`STORE_UNAVAILABLE_MESSAGE`]:
/// AWS documents no message for an emulator fault.
pub const SECRET_UNAVAILABLE_MESSAGE: &str = "the IVS ingest secret generator is unavailable";

/// The same fault as [`store_unavailable`] for the two BATCH reads, whose model declares
/// `ServiceUnavailable` and not `InternalServerException`
/// (`smithy_query(query="operation", operation="BatchGetChannel,BatchGetStreamKey")`).
pub fn service_unavailable<E>(variant: fn(ServiceUnavailable) -> E) -> Scaffolded<E> {
    let message = STORE_UNAVAILABLE_MESSAGE.to_string();
    Scaffolded::stated(
        variant(ServiceUnavailable {
            exception_message: Some(message.clone()),
            ..Default::default()
        }),
        message,
    )
}

/// The not-found the TAGGING surface answers for an ARN naming no resource:
/// [`TAGGING_NOT_FOUND_CODE`] with [`TAGGING_NOT_FOUND_STATUS`], which is not the
/// modelled error shape — hence the unmodelled channel.
pub fn tagging_not_found<E>() -> Scaffolded<E> {
    Scaffolded::unmodeled(
        TAGGING_NOT_FOUND_CODE,
        TAGGING_NOT_FOUND_STATUS,
        TAGGING_NOT_FOUND_MESSAGE,
    )
}

// =============================================================================
// How this service states a CONSTRAINT refusal
// =============================================================================
//
// The framework's constraint walk runs between deserialize and `handle` and words a
// violation the same way for every service ("1 validation error detected: Value '…' at
// '…' failed to satisfy constraint: …"). IVS words it from the OPERATION and the member
// path instead ("CreateChannel.name does not match the required pattern …"), and the
// operation is not reachable at the framework's refusal seam — `Service::override_refusal`
// is handed the violations alone. `crate::service`'s per-operation wrapper collects the
// framework's OWN violations and brings the operation with it; the sentences are here,
// beside the message rules above, and not one modelled constraint is restated: every
// bound and pattern a clause prints comes off the `Violation` the walk produced.

/// One violation as this service states it: the operation, the dotted member path and
/// the constraint the request violated.
///
/// The `@pattern` and `@length` forms are the recorded ones. The remaining kinds are
/// derived from them by the same construction — the operation and member path, then the
/// requirement — because AWS's exact wording for those is not recorded; the ERROR CODE
/// and status they travel with are the modelled ones either way.
pub fn violation_clause(operation: &str, violation: &Violation) -> String {
    let path = violation.path.as_str();
    match &violation.kind {
        ViolationKind::Pattern(pattern) => pattern_violation_message(operation, path, pattern),
        ViolationKind::MinLength(minimum) => min_length_violation_message(
            operation,
            path,
            usize::try_from(*minimum).unwrap_or(0),
            found_length(violation),
        ),
        ViolationKind::MaxLength(maximum) => max_length_violation_message(
            operation,
            path,
            usize::try_from(*maximum).unwrap_or(0),
            found_length(violation),
        ),
        ViolationKind::MissingRequiredMember => format!("{operation}.{path} is required"),
        ViolationKind::MinValue(minimum) => {
            format!("{operation}.{path} is required to have a minimum value of {minimum}")
        }
        ViolationKind::MaxValue(maximum) => {
            format!("{operation}.{path} is required to have a maximum value of {maximum}")
        }
        ViolationKind::EnumValue(allowed) => format!(
            "{operation}.{path} is required to be one of [{}]",
            allowed.join(", ")
        ),
        ViolationKind::UniqueItems => {
            format!("{operation}.{path} is required to have unique values")
        }
        // `ViolationKind` is `#[non_exhaustive]`: a constraint class the framework gains
        // later is stated in the same construction rather than silently unworded.
        _ => format!("{operation}.{path} does not satisfy the constraint the model declares"),
    }
}

/// `violations` as this service states them for `operation`, or `None` when there is
/// nothing to refuse.
///
/// Each recorded refusal carries ONE violation and states its sentence alone — no count
/// prefix, unlike the framework's envelope — so one violation is its clause; several are
/// joined, in the order the walk collected them.
pub fn stated_violations(operation: &str, violations: &[Violation]) -> Option<String> {
    match violations {
        [] => None,
        [only] => Some(violation_clause(operation, only)),
        many => Some(
            many.iter()
                .map(|violation| violation_clause(operation, violation))
                .collect::<Vec<String>>()
                .join("; "),
        ),
    }
}

/// The length the request actually sent, for a `@length` violation.
///
/// A `Violation` carries WHAT was sent, not its size, so the size is read off that
/// rendering: the framework renders a collection as `"<n> items"` and a string as the
/// string itself. A string longer than the framework's 256-character echo cap arrives
/// elided, which is the one case this count is short — it affects the number in the
/// message and nothing about the refusal.
fn found_length(violation: &Violation) -> usize {
    let Some(value) = violation.value.as_deref() else {
        return 0;
    };
    value
        .strip_suffix(" items")
        .and_then(|count| count.parse().ok())
        .unwrap_or_else(|| value.chars().count())
}

/// The violation for a `@required` member the request did not send.
///
/// Absence is found while READING the request, so it arrives as a codec error rather
/// than from the constraint walk; this is the same `Violation` the framework's own
/// `refusal_of` builds for it, so one refusal is not stated two ways.
pub fn missing_member_violation(member_name: String) -> Violation {
    Violation {
        path: member_name,
        kind: ViolationKind::MissingRequiredMember,
        value: None,
        sensitive: false,
    }
}

// =============================================================================
// How this service spells the dotted member path a refusal names
// =============================================================================
//
// IVS names each step of that path the way the MODEL names the value at it, and names
// an element of a list by its ZERO-based position: the recorded refusal for an element
// of `CreateAdConfiguration`'s MediaTailor list reads
// `CreateAdConfiguration.mediaTailorPlaybackConfiguration.0.playbackConfigurationArn`,
// where `MediaTailorPlaybackConfiguration` is the shape the list's member targets
// (`smithy_query(query="shape", shape="MediaTailorPlaybackConfigurationsList")`) and `0`
// is the first element. The framework's walk spells the same position
// `mediaTailorPlaybackConfigurations.1.member.playbackConfigurationArn`: it counts
// elements from one and marks the step into a collection with the synthetic segment the
// Smithy list shape names its member by, neither of which IVS carries.
//
// A list member's own segment and an element's leaf are already the model's names, so
// only the step INTO an element is respelled — and the shape it steps into is the
// element's own, which no `Schema` carries (a list member schema holds a sub-member
// named `member`, not its target). It is read off the VALUE instead: the generated
// serialization writes a structure element against that structure's own schema, so one
// walk of the request records, per list, the shape its elements are.

/// The segment the framework's walk marks a step from a collection into one of its
/// elements with — `aws_smithy_schema_server::constraints::path`'s collection step, the
/// Smithy default name of a list shape's member — and the one segment an IVS member path
/// does not carry.
const COLLECTION_STEP: &str = "member";

/// What the model names the elements of each list ONE request carries, keyed by the list
/// member's own path segment.
///
/// Built by [`element_steps`] from the request itself, so a list the request did not send
/// has no entry and a path through it is left exactly as the walk spelled it.
#[derive(Debug, Default)]
pub struct ElementSteps {
    steps: HashMap<String, String>,
}

impl ElementSteps {
    /// `path` as IVS spells it: every step into a list element named by the element's
    /// own shape and its zero-based position, with the walk's collection segment gone.
    ///
    /// A step whose list this request did not carry keeps the list member's own
    /// segment — the model gives that element no shape of its own, which is the case for
    /// a list of scalars — so the position is still restated from zero and nothing else
    /// about the path changes.
    pub fn respell(&self, path: &str) -> String {
        let segments: Vec<&str> = path.split('.').collect();
        let mut spelled: Vec<String> = Vec::with_capacity(segments.len());
        let mut at = 0;
        while at < segments.len() {
            match element_position(&segments[at..]) {
                Some(position) => {
                    spelled.push(
                        self.steps
                            .get(segments[at])
                            .cloned()
                            .unwrap_or_else(|| segments[at].to_string()),
                    );
                    spelled.push(position.to_string());
                    at += 3;
                }
                None => {
                    spelled.push(segments[at].to_string());
                    at += 1;
                }
            }
        }
        spelled.join(".")
    }
}

/// The ZERO-based position `segments` begins a step into, when it begins one: a
/// collection's own segment, then the position the walk counted from one, then the
/// walk's [`COLLECTION_STEP`]. `None` for anything else — a plain member, or a map entry,
/// whose segment is the caller's own key rather than a position.
fn element_position(segments: &[&str]) -> Option<usize> {
    let [_, position, step, ..] = segments else {
        return None;
    };
    if *step != COLLECTION_STEP {
        return None;
    }
    position.parse::<usize>().ok()?.checked_sub(1)
}

/// Read `input` for the shape the model gives the elements of every list it carries.
///
/// One walk of the request, driven exactly as a codec drives it, so the shapes recorded
/// are the ones the model declares rather than anything restated here.
pub fn element_steps(input: &dyn SerializableStruct, schema: &Schema) -> ElementSteps {
    let mut recorder = ElementRecorder::default();
    // The root input is a struct; drive it the way `constraints::validate` does.
    let _ = recorder.write_struct(schema, input);
    ElementSteps {
        steps: recorder.steps,
    }
}

/// The walk [`element_steps`] runs: it reads the request's shape and nothing of its
/// values, so no value a caller sent can reach a message through it.
#[derive(Debug, Default)]
struct ElementRecorder {
    /// The collections and structures currently open, innermost last.
    open: Vec<OpenShape>,
    /// What each list's elements are, keyed by the list member's path segment.
    steps: HashMap<String, String>,
}

/// One shape the walk is inside, and what it names the values written directly in it.
#[derive(Debug)]
enum OpenShape {
    /// A list, named by its member's own path segment: its elements are a step into it.
    List(String),
    /// A map: an entry is named by the key the caller chose, never by a shape.
    Map,
    /// A structure: its members name themselves.
    Struct,
}

impl ElementRecorder {
    /// Record `schema` as the shape of the elements of the list this value is an element
    /// of — when it is one, and not a member of a structure or the value of a map entry.
    fn record_element(&mut self, schema: &Schema) {
        if let Some(OpenShape::List(member)) = self.open.last() {
            let member = member.clone();
            // The same lowerCamel spelling a path segment takes, applied to the shape
            // the element IS rather than to a member name.
            let element = constraints::path::member(schema.shape_id().shape_name());
            self.steps.insert(member, element);
        }
    }
}

impl ShapeSerializer for ElementRecorder {
    fn write_struct(
        &mut self,
        schema: &Schema,
        value: &dyn SerializableStruct,
    ) -> Result<(), SerdeError> {
        self.record_element(schema);
        self.open.push(OpenShape::Struct);
        let walked = value.serialize_members(self);
        self.open.pop();
        walked
    }

    fn write_list(
        &mut self,
        schema: &Schema,
        write_elements: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.open.push(OpenShape::List(constraints::path::member(
            schema.member_name().unwrap_or_default(),
        )));
        let walked = write_elements(self);
        self.open.pop();
        walked
    }

    fn write_map(
        &mut self,
        _schema: &Schema,
        write_entries: &dyn Fn(&mut dyn ShapeSerializer) -> Result<(), SerdeError>,
    ) -> Result<(), SerdeError> {
        self.open.push(OpenShape::Map);
        let walked = write_entries(self);
        self.open.pop();
        walked
    }

    // A leaf is never a shape this walk steps into, and its VALUE is not read at all:
    // every scalar write is accepted and recorded nowhere.
    fn write_boolean(&mut self, _schema: &Schema, _value: bool) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_byte(&mut self, _schema: &Schema, _value: i8) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_short(&mut self, _schema: &Schema, _value: i16) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_integer(&mut self, _schema: &Schema, _value: i32) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_long(&mut self, _schema: &Schema, _value: i64) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_float(&mut self, _schema: &Schema, _value: f32) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_double(&mut self, _schema: &Schema, _value: f64) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_big_integer(
        &mut self,
        _schema: &Schema,
        _value: &BigInteger,
    ) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_big_decimal(
        &mut self,
        _schema: &Schema,
        _value: &BigDecimal,
    ) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_string(&mut self, _schema: &Schema, _value: &str) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_blob(&mut self, _schema: &Schema, _value: &[u8]) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_timestamp(&mut self, _schema: &Schema, _value: &DateTime) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_document(&mut self, _schema: &Schema, _value: &Document) -> Result<(), SerdeError> {
        Ok(())
    }

    fn write_null(&mut self, _schema: &Schema) -> Result<(), SerdeError> {
        Ok(())
    }
}

// =============================================================================
// The two constraints this service enforces in its OWN handlers
// =============================================================================
//
// Everything the model declares is enforced by the framework's walk and stated by
// `violation_clause` above. These two are different, and each says why:
// `RESOURCE_ARN` RELAXES a modelled pattern no IVS ARN can satisfy (so the walk must
// not see it, hence the member's `skip_validation`), and `TAGS` bounds the RESOURCE's
// whole tag set, which no single request's input shape expresses.

/// A modelled `@pattern`: the source text, which the refusal message prints, beside the
/// compiled form the check runs.
pub struct ModelPattern {
    /// The pattern exactly as the model declares it.
    pub source: &'static str,
    /// The same pattern, compiled once on first use.
    compiled: LazyLock<Regex>,
}

impl ModelPattern {
    /// Whether `value` satisfies the pattern.
    pub fn matches(&self, value: &str) -> bool {
        self.compiled.is_match(value)
    }
}

impl std::fmt::Debug for ModelPattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelPattern")
            .field("source", &self.source)
            .finish_non_exhaustive()
    }
}

/// Declare a [`ModelPattern`] from one literal, so the text the message prints and the
/// expression the check runs can never be two different patterns.
macro_rules! model_pattern {
    ($name:ident, $source:literal) => {
        static $name: ModelPattern = ModelPattern {
            source: $source,
            compiled: LazyLock::new(|| {
                Regex::new($source).expect("a pattern this service's model declares compiles")
            }),
        };
    };
}

model_pattern!(
    RESOURCE_ARN_PATTERN,
    "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]+/[a-zA-Z0-9-]+$"
);

/// One member's constraints, checked by the handler and stated the way
/// [`violation_clause`] states the framework's.
#[derive(Debug)]
pub struct MemberRule {
    /// The dotted member path the message names, as the request spells it (lowerCamel,
    /// and qualified for a member of a nested structure).
    pub path: &'static str,
    /// The `@length` minimum, if the member has one.
    pub min_length: Option<usize>,
    /// The `@length` maximum, if the member has one.
    pub max_length: Option<usize>,
    /// The `@pattern`, if the member has one.
    pub pattern: Option<&'static ModelPattern>,
}

impl MemberRule {
    /// Check a string member of `operation`'s request. `Err` is the text to put on the
    /// `ValidationException`.
    ///
    /// The bounds are checked before the pattern, so a value that breaks both is
    /// reported against its length — the order the framework's own walk uses.
    pub fn check(&self, operation: &str, value: &str) -> Result<(), String> {
        self.check_length(operation, value.chars().count())?;
        match self.pattern {
            Some(pattern) if !pattern.matches(value) => Err(pattern_violation_message(
                operation,
                self.path,
                pattern.source,
            )),
            _ => Ok(()),
        }
    }

    /// Check a collection member's entry count against the `@length`: a list's elements
    /// or a map's entries, which is what `@length` bounds on a collection.
    pub fn check_length(&self, operation: &str, found: usize) -> Result<(), String> {
        if let Some(minimum) = self.min_length {
            if found < minimum {
                return Err(min_length_violation_message(
                    operation, self.path, minimum, found,
                ));
            }
        }
        if let Some(maximum) = self.max_length {
            if found > maximum {
                return Err(max_length_violation_message(
                    operation, self.path, maximum, found,
                ));
            }
        }
        Ok(())
    }
}

/// The `resourceArn` of `ListTagsForResource` / `TagResource` / `UntagResource` ->
/// `ResourceArn`: `@length(1..128)` and
/// `@pattern("^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]/[a-zA-Z0-9-]+$")`.
///
/// The pattern enforced here repeats the resource-type group (`[a-z-]+`) where the
/// model writes it once (`[a-z-]`). Taken literally the modelled group matches exactly
/// ONE character, so no IVS ARN that exists — `channel/…`, `stream-key/…`,
/// `playback-key/…` — could ever satisfy it, and the recordings show AWS accepting a
/// channel ARN here and round-tripping its tags, and answering a well-formed
/// `playback-key/…` ARN with a not-found rather than a validation error. The repeated
/// group is the only reading of the model under which the recorded behaviour is
/// possible; everything else about the pattern, and the length, is the model's. Because
/// the modelled pattern cannot be enforced as written, the member carries
/// `skip_validation` and this rule takes its place.
pub static RESOURCE_ARN: MemberRule = MemberRule {
    path: "resourceArn",
    min_length: Some(1),
    max_length: Some(128),
    pattern: Some(&RESOURCE_ARN_PATTERN),
};

/// The `tags` a resource carries -> `Tags`: `@length(0..50)` on the map
/// (`smithy_query(query="shape", shape="Tags")`).
///
/// The walk bounds the map ONE REQUEST sent; the quota is on the resource, so
/// `TagResource` checks the set its call would leave behind — which is a rule about
/// stored state, not about an input shape, and so cannot come from the walk.
pub static TAGS: MemberRule = MemberRule {
    path: "tags",
    min_length: Some(0),
    max_length: Some(50),
    pattern: None,
};

/// Whether the channel `channel_arn` names is ingesting right now — the state
/// `PutMetadata`, `StopStream` and `InsertAdBreak` need before they can act.
///
/// It is [`live_stream`] read as a yes/no, so every operation that needs an ACTIVE
/// stream agrees with `GetStream` about whether there is one: a channel is broadcasting
/// exactly when this store holds a `LIVE` stream for it. A channel goes live only by
/// PUBLISHING to the ingest endpoint `CreateChannel` advertises (the control plane has
/// no operation that starts a stream), and this emulator serves no ingest data plane, so
/// no channel is ingesting today and those operations answer the
/// `ChannelNotBroadcasting` their models declare for exactly this case — from the
/// absence of a stream in the store, never from a status invented for work nothing ran.
pub fn is_broadcasting(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> bool {
    live_stream(state, channel_arn).is_some()
}

/// End the stream `channel_arn` is ingesting, so the channel stops broadcasting.
///
/// `StopStream` disconnects the ingest session, after which the channel has no active
/// stream: `GetStream` answers `ChannelNotBroadcasting` again and so do the operations
/// that need one. The record is dropped rather than left behind in another state —
/// `GetStream` reports the ACTIVE stream, and a stream that has ended is not one.
pub fn end_stream(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) {
    state.streams.remove(channel_arn);
}

/// Whether this store holds the channel `channel_arn` names.
pub fn channel_exists(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> bool {
    state.channels.contains_key(channel_arn)
}

// =============================================================================
// Resources addressed by ARN: the tagging surface
// =============================================================================

/// The resource type an IVS ARN names (`channel`, `stream-key`, `playback-key`, …), or
/// `None` when the string is not an IVS resource ARN at all.
///
/// The type is the sixth `:`-field up to its `/`, from the ARN form every ARN shape in
/// this model pins: `arn:aws:ivs:<region>:<account>:<type>/<id>`.
pub fn arn_resource_type(arn: &str) -> Option<&str> {
    arn.split(':').nth(5)?.split('/').next()
}

/// The tags attached to the resource `arn` names, or `None` when this store holds no
/// such resource.
///
/// Only the resource types this service can create are reachable: a channel
/// (`CreateChannel`), a stream key (`CreateChannel` / `CreateStreamKey`) and a recording
/// configuration (`CreateRecordingConfiguration`). Every other IVS resource type's
/// create is still a scaffold stub, so this store holds none of them, and a well-formed
/// ARN naming one is answered exactly as an ARN naming a deleted resource is — not found.
pub fn resource_tags(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    arn: &str,
) -> Option<HashMap<String, String>> {
    match arn_resource_type(arn)? {
        "channel" => state
            .channels
            .get(arn)
            .map(|channel| channel.tags.clone().unwrap_or_default()),
        "stream-key" => state
            .stream_keys
            .get(arn)
            .map(|key| key.tags.clone().unwrap_or_default()),
        "recording-configuration" => state
            .recording_configurations
            .get(arn)
            .map(|record| record.configuration.tags.clone().unwrap_or_default()),
        _ => None,
    }
}

/// The `tags` member of the resource `arn` names, to be changed in place.
///
/// It hands back the stored `Option` ITSELF rather than a map, so a call that adds no
/// entry — an `UntagResource` naming keys the resource does not carry, say — leaves a
/// record whose `tags` was absent absent, instead of flipping every later read of that
/// resource from no `tags` member to an empty one. `None` when this store holds no such
/// resource; see [`resource_tags`].
pub fn resource_tags_entry<'a>(
    state: &'a mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    arn: &str,
) -> Option<&'a mut Option<HashMap<String, String>>> {
    match arn_resource_type(arn)? {
        "channel" => state.channels.get_mut(arn).map(|channel| &mut channel.tags),
        "stream-key" => state.stream_keys.get_mut(arn).map(|key| &mut key.tags),
        "recording-configuration" => state
            .recording_configurations
            .get_mut(arn)
            .map(|record| &mut record.configuration.tags),
        _ => None,
    }
}

