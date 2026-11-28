// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-chimesdkmessaging-chimemessagingservice service.
//!
//! The channel-messaging slice keeps every channel this service minted, keyed by its
//! ARN, and the messages sent to each one inside that channel's record — so deleting a
//! channel drops its messages with it and no second map can disagree about either. The
//! channel FLOWS this service minted are kept the same way, keyed by their own ARN, and
//! the channel flow a channel is ASSOCIATED with is held on the channel's own record,
//! because a channel carries at most one of them.
//!
//! Timestamps are held as epoch MILLISECONDS rather than as
//! [`aws_smithy_types::DateTime`], because the list operations have to order and range
//! filter them and `DateTime` carries no total order. [`date_time`] converts one back
//! for the wire and [`millis_of`] converts an inbound filter bound in.
//!
//! Three things this service reads are NOT in the struct below, because another Chime
//! service reads or writes them too and the engine keys state by type: the app-instance
//! ADMIN directory ([`app_instance_admins`]), the family's ARN-keyed TAG directory
//! ([`chime_tags`]), which holds the tags of every channel and channel flow — including
//! the ones a create was given — and the family's identity NAME directory
//! ([`identity_names`]), which is where the name of the `AppInstanceUser` or
//! `AppInstanceBot` an [`Identity`] names is published.
//!
//! A participant is therefore stored as its ARN alone: every `Identity` this service puts
//! on the wire is PROJECTED from that ARN at read time ([`identity_of`]), so a renamed
//! identity is reported under its current name rather than the one it carried when the
//! membership, ban, moderator, channel or message was written.

use std::collections::{HashMap, VecDeque};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    Channel, ChannelAssociatedWithFlowSummary, ChannelBan, ChannelBanSummary, ChannelFlow,
    ChannelFlowSummary, ChannelMembership, ChannelMembershipSummary, ChannelMessage,
    ChannelMessageStatusStructure, ChannelMessageSummary, ChannelModerator,
    ChannelModeratorSummary, ChannelSummary, ElasticChannelConfiguration, ExpirationSettings,
    Identity, MessageAttributeValue, Processor, Target,
};

/// How many `CreateChannel` idempotency records this emulator keeps.
///
/// The records are the emulator's own bookkeeping rather than an emulated resource, and
/// no operation in the API deletes one, so the map needs a bound of its own or a caller
/// could grow it without limit: past this many records the OLDEST is dropped. The worst
/// consequence is that a replay of a very old `CreateChannel` mints a new channel
/// instead of answering with the first one's ARN. A record is a `ChimeArn` (modelled
/// `len 5..1600`) keyed by an `AppInstanceArn` (1600) and a `ClientRequestToken`
/// (modelled `len 2..64`), so 4096 records bound this map at about 13 MiB.
///
/// `CreateChannelFlow`'s ledger is bounded the same way, for the same reason and with the
/// same consequence.
pub const MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS: usize = 4096;

/// How many tags one resource of THIS service carries at once: the bound its own model
/// declares on `TagList` (`@length(min: 1, max: 50)`,
/// `smithy_query(query="operation",operation="TagResource")`), which is the family's
/// usual one — so the shared directory's default bound is this service's bound too.
pub const MAX_TAGS_PER_RESOURCE: usize = ::service_common::chime_tags::MAX_TAGS_PER_RESOURCE;

/// What the service states when a `ChannelFlowArn` names no channel flow it holds.
///
/// A `ChannelFlowArn` is an app instance's ARN extended by `/channel-flow/<id>`
/// (`CreateChannelFlow` derives it that way), so an ARN this service holds no channel flow
/// for names an app instance scope it cannot resolve for the caller. The REQUIREMENT for
/// that refusal is the modelled `ForbiddenException` — HTTP 403 with `Code` `Forbidden`,
/// the only terminal client error either of the two operations that read a channel flow by
/// ARN models for an ARN it cannot resolve (neither lists `NotFoundException`) — and the
/// sentence is the one Amazon Chime SDK Messaging itself returns for it (observed in the
/// AWS wire recording this behaviour was implemented against). It names no value from the
/// request, so it is the same sentence for every caller.
///
/// Read by `DescribeChannelFlow` and `DeleteChannelFlow`, so the two sides of one rule
/// cannot drift.
pub const UNRESOLVABLE_APP_INSTANCE: &str =
    "An invalid App Instance ARN was supplied for the input parameter.";

/// How many bytes the digest every id this service mints is laid out from carries:
/// SHA-256's output length. Hex-rendered whole, it is the 64-character opaque id a
/// `MessageId` is, inside `MessageId`'s modelled maximum
/// (`smithy_query(query="shape", shape="MessageId")`: `len 1..128`); its first 16 bytes
/// are the ones a generated ARN id segment's uuid is laid out from.
const ID_DIGEST_BYTES: usize = 32;

/// How many channel flows this emulator stores at once.
///
/// `CreateChannelFlow` is the only way to add one and `DeleteChannelFlow` the only way to
/// remove one, and that reclamation is the CALLER's option — so without a cap of its own
/// the map grows with the request stream. Past this many live channel flows
/// `CreateChannelFlow` answers the modelled `ResourceLimitExceededException` instead of
/// minting another, which is also how the real service reports a quota it will not
/// exceed. A `ChannelFlowRecord` holds at most `Name` 256, three `Processor`s of
/// (`Name` 256 + `LambdaFunctionArn` 2048), so on the order of 7 KiB; 256 flows bound the
/// map at about 2 MiB. This emulator's own choice, not a modelled or documented service
/// limit.
pub const MAX_STORED_CHANNEL_FLOWS: usize = 256;

/// How many channels this emulator stores at once.
///
/// `CreateChannel` is the only way to add one and `DeleteChannel` the only way to remove
/// one, and that reclamation is the CALLER's option — so without a cap of its own the
/// map grows with the request stream. Past this many live channels `CreateChannel`
/// answers the modelled `ResourceLimitExceededException` instead of minting another,
/// which is also how the real service reports a quota it will not exceed.
///
/// Memory: a `ChannelRecord`'s own members are at most `Name` 256 + `Metadata` 1024 +
/// `ChannelId` 64 + three `ChimeArn`s (3 x 1600), so on the order of 6 KiB; 256 channels
/// bound the records themselves at about 2 MiB, plus at most
/// [`MAX_MESSAGE_BYTES_PER_CHANNEL`] of messages each (256 MiB in total) — a bound every
/// retention path respects, because a message larger than [`MAX_MESSAGE_BYTES`] is
/// refused rather than stored.
pub const MAX_STORED_CHANNELS: usize = 256;

/// How many memberships one channel keeps.
///
/// `CreateChannelMembership` is the only way to add one and `DeleteChannelMembership` /
/// `CreateChannelBan` the only ways to remove one, and that reclamation is the CALLER's
/// option — so the collection needs a bound of its own. Past this many members
/// `CreateChannelMembership` answers the modelled `ResourceLimitExceededException`
/// instead of adding another, which is also how the real service reports a quota it will
/// not exceed. This emulator's own choice, not a modelled or documented service limit.
pub const MAX_MEMBERSHIPS_PER_CHANNEL: usize = 256;

/// How many bans one channel keeps, for the same reason and with the same refusal as
/// [`MAX_MEMBERSHIPS_PER_CHANNEL`] — `DeleteChannelBan` is the only way one leaves.
pub const MAX_BANS_PER_CHANNEL: usize = 256;

/// How many moderators one channel keeps, for the same reason and with the same refusal
/// as [`MAX_MEMBERSHIPS_PER_CHANNEL`] — `DeleteChannelModerator` is the only way one
/// leaves.
pub const MAX_MODERATORS_PER_CHANNEL: usize = 64;

/// How many messages one channel keeps.
///
/// `SendChannelMessage` appends one record per request and this slice implements no
/// message delete, so the Vec needs a bound of its own: past this many the OLDEST
/// message is dropped, together with the `message_idempotency` record that named it — so
/// that map can never outgrow the messages it points at. An evicted message is simply no
/// longer there: `GetChannelMessage` answers its modelled 404 for it.
pub const MAX_MESSAGES_PER_CHANNEL: usize = 1024;

/// How many caller-supplied bytes one message may carry.
///
/// `Content` is modelled `NonEmptyContent, len 1..` — no MAXIMUM — so nothing in the
/// model stops one request from carrying a gigabyte, and a retained message that large
/// would break the aggregate bound below however the eviction is written. This emulator
/// therefore caps the message it will ACCEPT: `SendChannelMessage` refuses a larger one
/// with the modelled `BadRequestException` naming this limit, so every stored message
/// fits inside [`MAX_MESSAGE_BYTES_PER_CHANNEL`] and can be read back by the
/// `GetChannelMessage` that follows it. The value is this emulator's own choice, not a
/// modelled or documented service limit.
pub const MAX_MESSAGE_BYTES: usize = 64 * 1024;

/// How many caller-supplied bytes one channel's messages keep.
///
/// The oldest messages are dropped until the channel's stored bytes (see
/// [`ChannelMessageRecord::stored_bytes`]) are inside this budget — UNCONDITIONALLY, so
/// there is no retention path a request can take that this budget does not cover. It is
/// a whole multiple of [`MAX_MESSAGE_BYTES`], so a channel always keeps at least the
/// message it just accepted.
pub const MAX_MESSAGE_BYTES_PER_CHANNEL: usize = 16 * MAX_MESSAGE_BYTES;

/// How many entries one request's `MessageAttributes` map may carry.
///
/// `MessageAttributeMap` carries no modelled length, and an entry costs memory even when
/// its key and values are empty, so the count needs a cap of its own or a single request
/// could retain millions of near-zero-byte entries inside the byte budget.
/// `SendChannelMessage` refuses a larger map with the modelled `BadRequestException`.
/// This emulator's own choice, not a modelled or documented service limit.
pub const MAX_MESSAGE_ATTRIBUTES: usize = 64;

/// How many `StringValues` one message attribute may carry, for the same reason and with
/// the same refusal as [`MAX_MESSAGE_ATTRIBUTES`].
pub const MAX_MESSAGE_ATTRIBUTE_VALUES: usize = 64;

/// What one element of a stored collection costs beyond its own bytes: the `String` /
/// `Target` header the element itself occupies. Counting it is what keeps a request of
/// many EMPTY elements from being accounted at zero and so escaping the retention
/// budget.
const COLLECTION_ELEMENT_OVERHEAD: usize = std::mem::size_of::<String>();

/// The caller-supplied footprint a `SendChannelMessage` request would store, which is
/// what [`MAX_MESSAGE_BYTES`] refuses on and [`MAX_MESSAGE_BYTES_PER_CHANNEL`] budgets.
///
/// One function so the size the handler refuses on and the size the store accounts for
/// can never disagree, and it measures RETAINED FOOTPRINT rather than string lengths
/// alone: every map entry and every collection element is charged for the storage it
/// occupies, so a request made of empty strings cannot be accounted at zero.
pub fn message_bytes(
    content: &str,
    metadata: Option<&str>,
    content_type: Option<&str>,
    attributes: Option<&HashMap<String, MessageAttributeValue>>,
    target: Option<&[Target]>,
) -> usize {
    let attribute_bytes = attributes.map_or(0, |attributes| {
        attributes
            .iter()
            .map(|(key, value)| {
                std::mem::size_of::<String>()
                    + std::mem::size_of::<MessageAttributeValue>()
                    + key.len()
                    + value.string_values.as_ref().map_or(0, |values| {
                        values
                            .iter()
                            .map(|value| value.len() + COLLECTION_ELEMENT_OVERHEAD)
                            .sum::<usize>()
                    })
            })
            .sum()
    });
    // `TargetList` is modelled `len 1..1`, so the model itself bounds this collection;
    // it is charged for anyway, so no stored member escapes the accounting.
    let target_bytes = target.map_or(0, |targets| {
        targets
            .iter()
            .map(|target| {
                std::mem::size_of::<Target>()
                    + target.member_arn.as_ref().map_or(0, String::len)
            })
            .sum::<usize>()
    });
    content.len()
        + metadata.map_or(0, str::len)
        + content_type.map_or(0, str::len)
        + attribute_bytes
        + target_bytes
}

/// Per-service state for com-amazonaws-chimesdkmessaging-chimemessagingservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsChimesdkmessagingChimemessagingserviceState {
    /// Every live channel, keyed by its `ChannelArn`. Channels and the messages they own
    /// are emulated RESOURCES, reclaimed by `DeleteChannel`; because that reclamation is
    /// the caller's option, the map is additionally capped at [`MAX_STORED_CHANNELS`]
    /// and each channel's messages at [`MAX_MESSAGES_PER_CHANNEL`] /
    /// [`MAX_MESSAGE_BYTES_PER_CHANNEL`], so the store cannot grow with the request
    /// stream. No response returns more than the modelled `MaxResults` maximum at once.
    pub channels: HashMap<String, ChannelRecord>,
    /// Every live channel flow, keyed by its `ChannelFlowArn`. A channel flow is an
    /// emulated RESOURCE, reclaimed by `DeleteChannelFlow`; because that reclamation is
    /// the caller's option, the map is additionally capped at
    /// [`MAX_STORED_CHANNEL_FLOWS`], so the store cannot grow with the request stream.
    pub channel_flows: HashMap<String, ChannelFlowRecord>,
    /// `(AppInstanceArn, ClientRequestToken) -> ChannelArn`: the result the FIRST
    /// `CreateChannel` carrying that `@idempotencyToken` produced, so a replay answers
    /// with it instead of minting a second channel. The record is the recorded outcome
    /// of that request, so it outlives the channel itself — which is why it is read and
    /// written through [`Self::remembered_channel`] / [`Self::remember_channel`], which
    /// keep the map inside [`MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS`].
    create_channel_idempotency: IdempotencyLedger,
    /// The same ledger for `CreateChannelFlow`, whose `ClientRequestToken` is modelled
    /// `@idempotencyToken` too — read and written through
    /// [`Self::remembered_channel_flow`] / [`Self::remember_channel_flow`].
    create_channel_flow_idempotency: IdempotencyLedger,
    /// Draws [`Self::id_digest`], so two ids minted in the same nanosecond differ.
    id_counter: u64,
}

/// What the FIRST request carrying an `@idempotencyToken` produced, so a replay of it
/// answers with that result instead of minting a second resource.
///
/// One type for every operation in this slice that models such a token, so the bound and
/// the eviction order are stated once: the oldest record is dropped past
/// [`MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS`], because no operation in the API deletes
/// one and the map would otherwise grow with the request stream.
#[derive(Debug, Default)]
struct IdempotencyLedger {
    /// `(resource scope, ClientRequestToken) -> the ARN that request returned`.
    records: HashMap<(String, String), String>,
    /// The order those records were written in, so the oldest can be dropped first.
    order: VecDeque<(String, String)>,
}

impl IdempotencyLedger {
    /// The ARN an earlier request with this key returned, if the record is still held.
    fn remembered(&self, key: &(String, String)) -> Option<&String> {
        self.records.get(key)
    }

    /// Record what this request returned, dropping the oldest record once the ledger is
    /// at [`MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS`].
    fn remember(&mut self, key: (String, String), arn: String) {
        if self.records.insert(key.clone(), arn).is_none() {
            self.order.push_back(key);
        }
        while self.order.len() > MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS {
            if let Some(oldest) = self.order.pop_front() {
                self.records.remove(&oldest);
            }
        }
    }
}

impl ComAmazonawsChimesdkmessagingChimemessagingserviceState {
    /// Mint the id segment of an ARN this service derives for a resource it creates: a
    /// channel id or a channel flow id.
    ///
    /// Both carry the same id shape, because both are the generated last segment of an
    /// app-instance-scoped ARN of this service (`…/channel/<id>`,
    /// `…/channel-flow/<id>`): the canonical hyphenated lowercase UUID form, which is the
    /// form `CreateChannel` documents for an id the caller did not supply —
    /// `smithy_query(query="doc", shape="CreateChannelRequest", member="ChannelId")`: "An
    /// ID for the channel being created. If you do not specify an ID, a UUID will be
    /// created for the channel." — and which fits `ChannelId`'s modelled `len 1..64` and
    /// its `^[A-Za-z0-9]([A-Za-z0-9:\-_.@]{0,62}[A-Za-z0-9])?$`. The family's one
    /// rendering of 16 opaque bytes in that form is
    /// [`::service_common::uuid_from_random_bytes`], so the version and variant bits and
    /// the group boundaries are not restated here.
    ///
    /// The bytes handed to it come from [`Self::id_digest`], so two ids minted in the same
    /// nanosecond differ and nothing the caller sent is recoverable from the result.
    pub fn mint_resource_id(&mut self, salt: &str) -> String {
        let digest = self.id_digest(salt);
        let bytes: [u8; 16] = digest[..16]
            .try_into()
            .expect("16 of the digest's 32 bytes");
        ::service_common::uuid_from_random_bytes(bytes)
    }

    /// Mint a `MessageId` for a message this service accepts.
    ///
    /// A message id carries a different shape from the ARN id segments above: it is no
    /// ARN segment and the API documents no UUID for it, so it is an OPAQUE id — a FULL
    /// lowercase hex digest, 64 undelimited characters, inside `MessageId`'s modelled
    /// `len 1..128` and `^[-_a-zA-Z0-9]*$`
    /// (`smithy_query(query="operation", operation="SendChannelMessage")`), which is the
    /// shape the ids Amazon Chime SDK Messaging assigns to messages carry.
    ///
    /// The digest comes from [`Self::id_digest`], the same source the ARN id segments
    /// draw, so the uniqueness argument is the one stated there.
    pub fn mint_message_id(&mut self, salt: &str) -> String {
        hex::encode(self.id_digest(salt))
    }

    /// The digest every id this service mints is laid out from: a SHA-256 over `salt`, a
    /// monotonic counter and the current instant.
    ///
    /// The counter is what makes two ids minted in the same nanosecond differ, and the
    /// digest is what keeps `salt` — which carries the caller's own ARNs — unrecoverable
    /// from the id.
    fn id_digest(&mut self, salt: &str) -> [u8; ID_DIGEST_BYTES] {
        self.id_counter = self.id_counter.wrapping_add(1);
        let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
        sha2::Digest::update(&mut hasher, salt.as_bytes());
        sha2::Digest::update(&mut hasher, self.id_counter.to_be_bytes());
        sha2::Digest::update(&mut hasher, now_nanos().to_be_bytes());
        let digest = sha2::Digest::finalize(hasher);
        digest.into()
    }

    /// The `ChannelArn` an earlier `CreateChannel` with this `(AppInstanceArn,
    /// ClientRequestToken)` returned, if the record is still held.
    pub fn remembered_channel(&self, key: &(String, String)) -> Option<&String> {
        self.create_channel_idempotency.remembered(key)
    }

    /// Record what this `CreateChannel` returned, dropping the oldest record once the
    /// ledger is at [`MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS`].
    pub fn remember_channel(&mut self, key: (String, String), channel_arn: String) {
        self.create_channel_idempotency.remember(key, channel_arn);
    }

    /// The `ChannelFlowArn` an earlier `CreateChannelFlow` with this `(AppInstanceArn,
    /// ClientRequestToken)` returned, if the record is still held.
    pub fn remembered_channel_flow(&self, key: &(String, String)) -> Option<&String> {
        self.create_channel_flow_idempotency.remembered(key)
    }

    /// Record what this `CreateChannelFlow` returned, dropping the oldest record once the
    /// ledger is at [`MAX_CREATE_CHANNEL_IDEMPOTENCY_RECORDS`].
    pub fn remember_channel_flow(&mut self, key: (String, String), channel_flow_arn: String) {
        self.create_channel_flow_idempotency
            .remember(key, channel_flow_arn);
    }

    /// The channel `channel_arn` names, or the `Message` the refusal owes when this
    /// service holds no channel for it.
    ///
    /// Resolving a channel ARN for the caller is an ACCESS decision, not a lookup that
    /// reports absence: an ARN this service holds no channel for names a channel scope it
    /// cannot resolve, so the REQUIREMENT is the modelled `ForbiddenException` — HTTP 403
    /// with `Code` `Forbidden` — carrying [`INSUFFICIENT_ACCESS_MESSAGE`]. That is also
    /// the only terminal client error most of the operations that resolve a channel
    /// model: all but one of them declare no `NotFoundException` at all, so a 404 is an
    /// error they cannot raise. The same rule already governs this service's other
    /// ARN-addressed resource ([`UNRESOLVABLE_APP_INSTANCE`]).
    pub fn resolve_channel(&self, channel_arn: &str) -> Result<&ChannelRecord, &'static str> {
        self.channels
            .get(channel_arn)
            .ok_or(INSUFFICIENT_ACCESS_MESSAGE)
    }

    /// [`Self::resolve_channel`] for a caller that goes on to WRITE the channel it
    /// resolves, so the unresolvable case is worded by the one rule either way.
    pub fn resolve_channel_mut(
        &mut self,
        channel_arn: &str,
    ) -> Result<&mut ChannelRecord, &'static str> {
        self.channels
            .get_mut(channel_arn)
            .ok_or(INSUFFICIENT_ACCESS_MESSAGE)
    }

    /// The channel `channel_arn` names when `caller` may READ it, or the `Message` the
    /// refusal owes.
    ///
    /// One function for the whole gate a read passes, because the three ways it can fail
    /// are one refusal: a channel this service cannot resolve ([`Self::resolve_channel`]),
    /// a PRIVATE channel the caller is not in, and a caller BANNED from it
    /// ([`ChannelRecord::read_refusal`], which words the ban for its reason). Every one of
    /// them is the modelled `ForbiddenException`.
    pub fn readable_channel(
        &self,
        channel_arn: &str,
        caller: &str,
    ) -> Result<&ChannelRecord, &'static str> {
        let channel = self.resolve_channel(channel_arn)?;
        match channel.read_refusal(caller) {
            Some(reason) => Err(reason),
            None => Ok(channel),
        }
    }

    /// Every channel this service holds that `channel_flow_arn`'s channel flow is
    /// associated with.
    ///
    /// One reader for the whole association view, so the operation that LISTS those
    /// channels and the one that refuses to delete a channel flow still associated with
    /// any of them cannot disagree about which channels those are. The channels come out
    /// in no particular order, because a `HashMap` has none: a caller that needs a stable
    /// order imposes it (the listing orders by `ChannelArn`).
    pub fn channels_with_flow<'a>(
        &'a self,
        channel_flow_arn: &'a str,
    ) -> impl Iterator<Item = &'a ChannelRecord> {
        self.channels
            .values()
            .filter(move |channel| channel.channel_flow_arn.as_deref() == Some(channel_flow_arn))
    }

    /// True when `arn` still names a live resource of this service.
    pub fn resource_exists(&self, arn: &str) -> bool {
        self.channels.contains_key(arn) || self.channel_flows.contains_key(arn)
    }

    /// Whether a tagging call may address `arn`.
    ///
    /// The family's one rule for that question lives in
    /// [`::service_common::chime_tags::addresses_taggable_resource`]; this store answers
    /// the half of it only this service can — whether an ARN that names a resource THIS
    /// service mints ([`names_messaging_resource`]) is one it still holds.
    pub fn taggable(&self, arn: &str) -> bool {
        ::service_common::chime_tags::addresses_taggable_resource(
            arn,
            names_messaging_resource(arn).then(|| self.resource_exists(arn)),
        )
    }
}

/// Whether `arn` names a resource THIS service mints, so that this service's store is
/// what says whether it exists.
///
/// Both of them hang off an app instance under a segment of their own: a channel is
/// `arn:…:chime:…:app-instance/<id>/channel/<id>` and a channel flow
/// `…/channel-flow/<id>` (`CreateChannel` and `CreateChannelFlow` derive the ARN that
/// way). The app instance itself and its identity children (`…/user/<id>`, `…/bot/<id>`)
/// are the Chime SDK Identity service's, and every other family member's resources carry
/// a different resource type altogether, so none of them reads as this service's.
#[must_use]
pub fn names_messaging_resource(arn: &str) -> bool {
    let Some(resource) = ::service_common::arn::resource(arn) else {
        return false;
    };
    let tokens: Vec<&str> = resource.split('/').collect();
    match tokens.as_slice() {
        ["app-instance", id, child, child_id] => {
            !id.is_empty() && !child_id.is_empty() && matches!(*child, "channel" | "channel-flow")
        }
        _ => false,
    }
}

/// A channel flow minted by `CreateChannelFlow`: the container for the processor Lambda
/// functions messages sent to an associated channel go through.
#[derive(Debug, Clone)]
pub struct ChannelFlowRecord {
    pub arn: String,
    pub app_instance_arn: String,
    pub name: String,
    /// The processors as the caller declared them, in request order.
    pub processors: Vec<Processor>,
    pub created_millis: i64,
    pub last_updated_millis: i64,
}

impl ChannelFlowRecord {
    /// The modelled `ChannelFlow` this record describes, as `DescribeChannelFlow`
    /// returns it.
    pub fn to_channel_flow(&self) -> ChannelFlow {
        ChannelFlow {
            channel_flow_arn: Some(self.arn.clone()),
            processors: Some(self.processors.clone()),
            name: Some(self.name.clone()),
            created_timestamp: Some(date_time(self.created_millis)),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
        }
    }

    /// The modelled `ChannelFlowSummary` this record describes, as `ListChannelFlows`
    /// returns it. The summary carries no timestamps.
    pub fn to_summary(&self) -> ChannelFlowSummary {
        ChannelFlowSummary {
            channel_flow_arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            processors: Some(self.processors.clone()),
        }
    }
}

/// A channel minted by `CreateChannel`, with the messages sent to it.
#[derive(Debug, Clone)]
pub struct ChannelRecord {
    pub arn: String,
    pub app_instance_arn: String,
    pub channel_id: String,
    pub name: String,
    /// `ChannelMode` wire value — see `CHANNEL_MODE_VALUES`.
    pub mode: String,
    /// `ChannelPrivacy` wire value — see `CHANNEL_PRIVACY_VALUES`.
    pub privacy: String,
    pub metadata: Option<String>,
    /// The identity that created the channel, which the model calls `CreatedBy`. Held as
    /// an ARN, because the `Name` the wire shape carries beside it is the identity's
    /// current one ([`identity_of`]) rather than a snapshot taken at create time.
    pub created_by_arn: String,
    pub created_millis: i64,
    pub last_updated_millis: i64,
    /// Set by the last PERSISTENT message sent to the channel; `None` until then.
    pub last_message_millis: Option<i64>,
    pub elastic_channel_configuration: Option<ElasticChannelConfiguration>,
    pub expiration_settings: Option<ExpirationSettings>,
    /// The `ChannelFlowArn` of the channel flow `AssociateChannelFlow` associated with
    /// this channel, and `None` while no flow is associated.
    ///
    /// The association is held HERE, on the channel, rather than as a list on the channel
    /// flow: a channel carries at most one channel flow — the wire `Channel` has a single
    /// `ChannelFlowArn` and the association is addressed as one sub-resource of the
    /// channel (`PUT /channels/{ChannelArn}/channel-flow`) — so one owner of the fact
    /// means `DescribeChannel` and `ListChannelsAssociatedWithChannelFlow` can never
    /// disagree about it, and deleting the channel takes its association with it.
    pub channel_flow_arn: Option<String>,
    /// Messages in the order they were accepted, which is ASCENDING `CreatedTimestamp`.
    pub messages: Vec<ChannelMessageRecord>,
    /// `ClientRequestToken -> MessageId` for this channel's `SendChannelMessage` calls.
    pub message_idempotency: HashMap<String, String>,
    /// The channel's members, in the order they were added — the set `SendChannelMessage`
    /// and the privacy rule read, `ListChannelMemberships` pages and `SearchChannels`
    /// matches on.
    pub memberships: Vec<ChannelMembershipRecord>,
    /// The members banned from the channel, in the order they were banned.
    pub bans: Vec<ChannelBanRecord>,
    /// The channel's moderators, in the order they were added. The `AppInstanceUser` that
    /// created the channel is its first one.
    pub moderators: Vec<ChannelModeratorRecord>,
}

/// One member added to a channel by `CreateChannelMembership`.
#[derive(Debug, Clone)]
pub struct ChannelMembershipRecord {
    pub member_arn: String,
    /// `ChannelMembershipType` wire value — see `CHANNEL_MEMBERSHIP_TYPE_VALUES`.
    pub r#type: String,
    /// The ARN of the caller that added this member, which the model calls `InvitedBy`.
    pub invited_by_arn: String,
    pub created_millis: i64,
    pub last_updated_millis: i64,
}

/// One member banned from a channel by `CreateChannelBan`.
#[derive(Debug, Clone)]
pub struct ChannelBanRecord {
    pub member_arn: String,
    /// The ARN of the caller that banned the member.
    pub created_by_arn: String,
    pub created_millis: i64,
}

/// One moderator of a channel: the caller that created it, or one added by
/// `CreateChannelModerator`.
#[derive(Debug, Clone)]
pub struct ChannelModeratorRecord {
    pub moderator_arn: String,
    /// The ARN of the caller that promoted the moderator.
    pub created_by_arn: String,
    pub created_millis: i64,
}

/// One message appended to a channel by `SendChannelMessage`.
#[derive(Debug, Clone)]
pub struct ChannelMessageRecord {
    pub message_id: String,
    pub content: String,
    pub content_type: Option<String>,
    pub metadata: Option<String>,
    /// `ChannelMessageType` wire value — see `CHANNEL_MESSAGE_TYPE_VALUES`.
    pub r#type: String,
    /// `ChannelMessagePersistenceType` wire value — see
    /// `CHANNEL_MESSAGE_PERSISTENCE_TYPE_VALUES`.
    pub persistence: String,
    pub created_millis: i64,
    pub last_updated_millis: i64,
    pub last_edited_millis: Option<i64>,
    /// The ARN of the identity that sent the message, which the model calls `Sender`.
    pub sender_arn: String,
    pub redacted: bool,
    /// `ChannelMessageStatus` wire value — see `CHANNEL_MESSAGE_STATUS_VALUES`.
    pub status: String,
    pub message_attributes: Option<HashMap<String, MessageAttributeValue>>,
    pub target: Option<Vec<Target>>,
}

impl ChannelRecord {
    /// The modelled `Channel` this record describes, as `DescribeChannel` returns it.
    pub fn to_channel(&self, names: &IdentityNameDirectory) -> Channel {
        Channel {
            name: Some(self.name.clone()),
            channel_arn: Some(self.arn.clone()),
            mode: Some(self.mode.clone()),
            privacy: Some(self.privacy.clone()),
            metadata: self.metadata.clone(),
            created_by: Some(identity_of(&self.created_by_arn, names)),
            created_timestamp: Some(date_time(self.created_millis)),
            last_message_timestamp: self.last_message_millis.map(date_time),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
            channel_flow_arn: self.channel_flow_arn.clone(),
            elastic_channel_configuration: self.elastic_channel_configuration.clone(),
            expiration_settings: self.expiration_settings.clone(),
        }
    }

    /// The modelled `ChannelSummary` this record describes, as `ListChannels` returns it.
    pub fn to_summary(&self) -> ChannelSummary {
        ChannelSummary {
            name: Some(self.name.clone()),
            channel_arn: Some(self.arn.clone()),
            mode: Some(self.mode.clone()),
            privacy: Some(self.privacy.clone()),
            metadata: self.metadata.clone(),
            last_message_timestamp: self.last_message_millis.map(date_time),
        }
    }

    /// The modelled `ChannelAssociatedWithFlowSummary` this record describes, as
    /// `ListChannelsAssociatedWithChannelFlow` returns it: the channel's own name, ARN,
    /// mode, privacy and metadata, which is every member that shape carries.
    pub fn to_associated_with_flow_summary(&self) -> ChannelAssociatedWithFlowSummary {
        ChannelAssociatedWithFlowSummary {
            name: Some(self.name.clone()),
            channel_arn: Some(self.arn.clone()),
            mode: Some(self.mode.clone()),
            privacy: Some(self.privacy.clone()),
            metadata: self.metadata.clone(),
        }
    }

    /// The message with `message_id`, if this channel holds it.
    pub fn message(&self, message_id: &str) -> Option<&ChannelMessageRecord> {
        self.messages.iter().find(|m| m.message_id == message_id)
    }

    /// The membership of `member_arn`, if this channel holds one.
    pub fn membership(&self, member_arn: &str) -> Option<&ChannelMembershipRecord> {
        self.memberships
            .iter()
            .find(|m| m.member_arn == member_arn)
    }

    /// Whether `member_arn` is a member of this channel.
    pub fn is_member(&self, member_arn: &str) -> bool {
        self.membership(member_arn).is_some()
    }

    /// The moderator record of `moderator_arn`, if this channel holds one.
    pub fn moderator(&self, moderator_arn: &str) -> Option<&ChannelModeratorRecord> {
        self.moderators
            .iter()
            .find(|m| m.moderator_arn == moderator_arn)
    }

    /// Whether `moderator_arn` moderates this channel.
    pub fn is_moderator(&self, moderator_arn: &str) -> bool {
        self.moderator(moderator_arn).is_some()
    }

    /// The ban on `member_arn`, if this channel holds one.
    pub fn ban(&self, member_arn: &str) -> Option<&ChannelBanRecord> {
        self.bans.iter().find(|b| b.member_arn == member_arn)
    }

    /// Whether `member_arn` is banned from this channel.
    pub fn is_banned(&self, member_arn: &str) -> bool {
        self.ban(member_arn).is_some()
    }

    /// Whether `caller` may READ this channel's messages.
    ///
    /// The model states the rule on `CreateChannelMembership`: a PUBLIC channel's messages
    /// can be listed without a membership, a PRIVATE channel's only by its members. A
    /// moderator of the channel reads it the way a member does.
    pub fn readable_by(&self, caller: &str) -> bool {
        self.privacy != "PRIVATE" || self.is_member(caller) || self.is_moderator(caller)
    }

    /// The `Message` a read of this channel by `caller` is refused with, or `None` when
    /// the caller may read it.
    ///
    /// One function, so every reporting operation on a channel refuses for the same two
    /// reasons and words each one the same way: a member BANNED from the channel is out
    /// of it altogether, whatever the channel's privacy, and a caller that is not a member
    /// cannot read a PRIVATE one.
    pub fn read_refusal(&self, caller: &str) -> Option<&'static str> {
        if self.is_banned(caller) {
            return Some(BANNED_MESSAGE);
        }
        (!self.readable_by(caller)).then_some(INSUFFICIENT_ACCESS_MESSAGE)
    }

    /// Drop `member_arn`'s membership, answering whether this channel held one.
    pub fn remove_membership(&mut self, member_arn: &str) -> bool {
        let before = self.memberships.len();
        self.memberships.retain(|m| m.member_arn != member_arn);
        self.memberships.len() != before
    }

    /// Append `message`, recording `client_request_token` as the token that produced it.
    ///
    /// The OLDEST messages are then dropped until the channel is inside both
    /// [`MAX_MESSAGES_PER_CHANNEL`] and [`MAX_MESSAGE_BYTES_PER_CHANNEL`], and each
    /// eviction takes the `message_idempotency` record that named the evicted message
    /// with it — so neither collection grows with the request stream, and the map can
    /// never outgrow the messages it points at. Both bounds apply unconditionally; a
    /// message too large to sit inside the byte budget is refused by
    /// `SendChannelMessage` ([`MAX_MESSAGE_BYTES`]) rather than stored here, so the
    /// message just appended is always still there for the get that follows.
    pub fn append_message(
        &mut self,
        message: ChannelMessageRecord,
        client_request_token: String,
    ) {
        let message_id = message.message_id.clone();
        self.messages.push(message);
        self.message_idempotency
            .insert(client_request_token, message_id);
        while !self.messages.is_empty()
            && (self.messages.len() > MAX_MESSAGES_PER_CHANNEL
                || self.stored_bytes() > MAX_MESSAGE_BYTES_PER_CHANNEL)
        {
            let evicted = self.messages.remove(0);
            self.message_idempotency
                .retain(|_, id| *id != evicted.message_id);
        }
    }

    /// The caller-supplied bytes this channel's messages hold.
    pub fn stored_bytes(&self) -> usize {
        self.messages
            .iter()
            .map(ChannelMessageRecord::stored_bytes)
            .sum()
    }
}

impl ChannelMessageRecord {
    /// The modelled `ChannelMessage` this record describes, as `GetChannelMessage`
    /// returns it. The channel ARN is the owning channel's, which the record does not
    /// repeat.
    ///
    /// A REDACTED message carries no `Content` and no `MessageAttributes`: redaction
    /// removes what the message carried while the message itself stays in the channel.
    pub fn to_channel_message(
        &self,
        channel_arn: &str,
        names: &IdentityNameDirectory,
    ) -> ChannelMessage {
        ChannelMessage {
            channel_arn: Some(channel_arn.to_string()),
            message_id: Some(self.message_id.clone()),
            content: self.visible_content(),
            metadata: self.metadata.clone(),
            r#type: Some(self.r#type.clone()),
            created_timestamp: Some(date_time(self.created_millis)),
            last_edited_timestamp: self.last_edited_millis.map(date_time),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
            sender: Some(identity_of(&self.sender_arn, names)),
            redacted: self.redacted,
            persistence: Some(self.persistence.clone()),
            status: Some(self.status_structure()),
            message_attributes: self.visible_attributes(),
            // Sub-channels belong to elastic channels, which this slice does not serve.
            sub_channel_id: None,
            content_type: self.content_type.clone(),
            target: self.target.clone(),
        }
    }

    /// The modelled `ChannelMessageSummary`, as `ListChannelMessages` returns it.
    pub fn to_summary(&self, names: &IdentityNameDirectory) -> ChannelMessageSummary {
        ChannelMessageSummary {
            message_id: Some(self.message_id.clone()),
            content: self.visible_content(),
            metadata: self.metadata.clone(),
            r#type: Some(self.r#type.clone()),
            created_timestamp: Some(date_time(self.created_millis)),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
            last_edited_timestamp: self.last_edited_millis.map(date_time),
            sender: Some(identity_of(&self.sender_arn, names)),
            redacted: self.redacted,
            status: Some(self.status_structure()),
            message_attributes: self.visible_attributes(),
            content_type: self.content_type.clone(),
            target: self.target.clone(),
        }
    }

    /// The `Content` a reader sees: the content as sent, or none once the message was
    /// redacted.
    fn visible_content(&self) -> Option<String> {
        (!self.redacted).then(|| self.content.clone())
    }

    /// The `MessageAttributes` a reader sees, which redaction removes with the content.
    fn visible_attributes(&self) -> Option<HashMap<String, MessageAttributeValue>> {
        if self.redacted {
            return None;
        }
        self.message_attributes.clone()
    }

    /// Redact this message: its content, metadata and attributes are dropped from the
    /// store and the message itself stays in the channel, so every later reader reports
    /// the message with `Redacted` true and none of what it carried.
    pub fn redact(&mut self, now: i64) {
        self.content.clear();
        self.metadata = None;
        self.message_attributes = None;
        self.redacted = true;
        self.last_updated_millis = now;
    }

    /// This message's delivery status as the modelled structure.
    pub fn status_structure(&self) -> ChannelMessageStatusStructure {
        ChannelMessageStatusStructure {
            value: Some(self.status.clone()),
            // `Detail` carries the reason a channel flow denied or failed a message;
            // nothing in this slice can produce one.
            detail: None,
        }
    }

    /// The caller-supplied footprint this record holds, which is what
    /// [`MAX_MESSAGE_BYTES_PER_CHANNEL`] budgets. Every stored caller-supplied member is
    /// charged, `Target` included.
    pub fn stored_bytes(&self) -> usize {
        self.message_id.len()
            + message_bytes(
                &self.content,
                self.metadata.as_deref(),
                self.content_type.as_deref(),
                self.message_attributes.as_ref(),
                self.target.as_deref(),
            )
    }
}

impl ChannelMembershipRecord {
    /// The modelled `ChannelMembership`, as `DescribeChannelMembership` returns it.
    pub fn to_channel_membership(
        &self,
        channel_arn: &str,
        names: &IdentityNameDirectory,
    ) -> ChannelMembership {
        ChannelMembership {
            invited_by: Some(identity_of(&self.invited_by_arn, names)),
            r#type: Some(self.r#type.clone()),
            member: Some(identity_of(&self.member_arn, names)),
            channel_arn: Some(channel_arn.to_string()),
            created_timestamp: Some(date_time(self.created_millis)),
            last_updated_timestamp: Some(date_time(self.last_updated_millis)),
            // Sub-channels belong to elastic channels, which this slice does not serve.
            sub_channel_id: None,
        }
    }

    /// The modelled `ChannelMembershipSummary`, as `ListChannelMemberships` returns it.
    pub fn to_summary(&self, names: &IdentityNameDirectory) -> ChannelMembershipSummary {
        ChannelMembershipSummary {
            member: Some(identity_of(&self.member_arn, names)),
        }
    }
}

impl ChannelBanRecord {
    /// The modelled `ChannelBan`, as `DescribeChannelBan` returns it.
    pub fn to_channel_ban(
        &self,
        channel_arn: &str,
        names: &IdentityNameDirectory,
    ) -> ChannelBan {
        ChannelBan {
            member: Some(identity_of(&self.member_arn, names)),
            channel_arn: Some(channel_arn.to_string()),
            created_timestamp: Some(date_time(self.created_millis)),
            created_by: Some(identity_of(&self.created_by_arn, names)),
        }
    }

    /// The modelled `ChannelBanSummary`, as `ListChannelBans` returns it.
    pub fn to_summary(&self, names: &IdentityNameDirectory) -> ChannelBanSummary {
        ChannelBanSummary {
            member: Some(identity_of(&self.member_arn, names)),
        }
    }
}

impl ChannelModeratorRecord {
    /// The modelled `ChannelModerator`, as `DescribeChannelModerator` returns it.
    pub fn to_channel_moderator(
        &self,
        channel_arn: &str,
        names: &IdentityNameDirectory,
    ) -> ChannelModerator {
        ChannelModerator {
            moderator: Some(identity_of(&self.moderator_arn, names)),
            channel_arn: Some(channel_arn.to_string()),
            created_timestamp: Some(date_time(self.created_millis)),
            created_by: Some(identity_of(&self.created_by_arn, names)),
        }
    }

    /// The modelled `ChannelModeratorSummary`, as `ListChannelModerators` returns it.
    pub fn to_summary(&self, names: &IdentityNameDirectory) -> ChannelModeratorSummary {
        ChannelModeratorSummary {
            moderator: Some(identity_of(&self.moderator_arn, names)),
        }
    }
}

/// The `Message` this service answers an authorization refusal with.
///
/// Amazon Chime SDK Messaging words every `ForbiddenException` the same way, whatever
/// the caller's role and whichever resource it asked for — so the sentence is stated
/// ONCE here and every operation's refusal reads it, rather than each one inventing its
/// own wording. The reason a particular request was refused stays in the comment at the
/// refusal, which is where a maintainer needs it; a client branches on the `Code`, which
/// is `Forbidden`.
pub const INSUFFICIENT_ACCESS_MESSAGE: &str =
    "You do not have sufficient access to perform this action.";

/// The `Message` a request by a member BANNED from the channel is refused with.
///
/// A ban is the one refusal this service words for its reason rather than with
/// [`INSUFFICIENT_ACCESS_MESSAGE`]: a banned member is out of the channel, so every
/// operation on it — sending a message as much as reading one — tells them the ban is
/// why.
pub const BANNED_MESSAGE: &str = "App instance user is banned from Channel.";

/// The `Message` a read of a BAN the channel does not hold is refused with, beside the
/// modelled `NotFoundException` that `DescribeChannelBan` declares.
///
/// The counterpart of [`BANNED_MESSAGE`], which the operations a banned member is refused
/// on carry: this service names the ban in both answers rather than describing the
/// request, so neither sentence carries a value from it.
pub const NOT_BANNED_MESSAGE: &str = "App instance user is not banned from the specified Channel.";

/// The `Message` a read of a MODERATOR the channel does not hold is refused with, beside
/// the modelled `NotFoundException` that `DescribeChannelModerator` declares.
///
/// Like [`NOT_BANNED_MESSAGE`], the sentence Amazon Chime SDK Messaging itself returns for
/// it (observed in the AWS wire recording this behaviour was implemented against): it
/// names neither the user nor the channel, so it is the same sentence for every caller.
pub const NOT_A_MODERATOR_MESSAGE: &str =
    "App instance user is not a moderator of the specified Channel.";

/// The `Message` a `DeleteChannelFlow` naming a channel flow that is still associated
/// with a channel is refused with, beside the modelled `ForbiddenException` (HTTP 403,
/// `Code` `Forbidden`) rather than the `ConflictException` that operation also declares.
///
/// The model states the rule on the operation itself: deleting a channel flow "works only
/// when the channel flow is not associated with any channel". The status, code and
/// sentence are the ones Amazon Chime SDK Messaging itself answers for that refusal
/// (observed in the AWS wire recording this behaviour was implemented against). The
/// sentence names no channel, so it is the same sentence however many channels carry the
/// flow and whichever they are — the listing the model points at is what reports those.
pub const CHANNEL_FLOW_STILL_ASSOCIATED_MESSAGE: &str =
    "Channel flow associated with a channel cannot be deleted.";

/// The `Message` a `DisassociateChannelFlow` naming a channel flow the channel is not
/// associated with is refused with, beside the modelled `BadRequestException`.
///
/// The association is the sub-resource the request addresses, so a channel that carries no
/// association, or carries a DIFFERENT channel flow's, holds nothing for this request to
/// remove: the request names a channel flow that is not this channel's, which is what the
/// sentence states.
pub const CHANNEL_FLOW_NOT_ASSOCIATED_MESSAGE: &str =
    "Channel flow provided in the request is not associated with the channel.";

/// The `Message` a `NextToken` this service cannot resolve is refused with, beside HTTP 400
/// `ValidationException`.
///
/// Amazon Chime SDK Messaging words this refusal "Pagination token exception in operation
/// '<OperationName>': <reason>" — the error is framework-level and in no operation's
/// modelled error set, so the form is the one the AWS wire recordings this was implemented
/// against carry. The operation is named because the same token means nothing to another
/// one, and the reason says HOW the token failed to yield a cursor.
///
/// A `NextToken` of this service is its own opaque cursor ([`encode_next_token`]) rather
/// than AWS's wrapped ciphertext, so the one failure it can report is the one AWS reports
/// for a token whose payload does not come back out of it: a token this service did not
/// issue, and one that no longer names a position in the view it was issued for, are that
/// same failure.
pub fn pagination_token_exception(operation: &str) -> String {
    format!("Pagination token exception in operation '{operation}': decryption error")
}

/// The `Message` a request whose `@idempotencyToken` was already used for a DIFFERENT
/// request is refused with, beside the modelled `ConflictException`.
///
/// One token stands for one request: replaying it answers with the resource the first
/// request produced, and reusing it for another request is a conflict rather than a second
/// resource. Stated once here so `CreateChannel` and `SendChannelMessage` refuse a reused
/// token identically.
pub const TOKEN_CONFLICT_MESSAGE: &str =
    "The provided ClientRequestToken is already associated with an existing resource.";

/// Now, as epoch milliseconds — the resolution the restJson1 timestamps carry.
pub fn now_millis() -> i64 {
    let now = std::time::SystemTime::now();
    match now.duration_since(std::time::UNIX_EPOCH) {
        Ok(since_epoch) => i64::try_from(since_epoch.as_millis()).unwrap_or(i64::MAX),
        Err(before_epoch) => -i64::try_from(before_epoch.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// Now, as epoch nanoseconds, which is what the digest behind every id this service mints
/// hashes beside its counter.
fn now_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since_epoch| since_epoch.as_nanos())
}

/// An epoch-milliseconds instant as the modelled timestamp type.
pub fn date_time(millis: i64) -> DateTime {
    DateTime::from_millis(millis)
}

/// An inbound modelled timestamp as epoch milliseconds, for range comparison.
pub fn millis_of(value: &DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// The identity NAME directory this service reads an [`Identity`]'s `Name` out of.
pub type IdentityNameDirectory = ::service_common::chime_identity_names::ChimeIdentityNameDirectory;

/// The modelled `Identity` of the `AppInstanceUser` or `AppInstanceBot` `identity_arn`
/// names: its ARN, and the `Name` it is registered under.
///
/// The name is not this service's own state — an identity is registered, renamed and
/// deleted through Chime SDK Identity — so it is read from the family's shared directory
/// ([`identity_names`]) at the moment the response is built. An identity the directory
/// holds no entry for carries no name, which is what the optional member says: nothing is
/// invented for it.
pub fn identity_of(identity_arn: &str, names: &IdentityNameDirectory) -> Identity {
    Identity {
        arn: Some(identity_arn.to_string()),
        name: names.name_of(identity_arn).map(str::to_string),
    }
}

/// Encode a pagination cursor as an opaque `NextToken`.
///
/// Hex keeps the token opaque (a caller cannot read a resource id out of it) while
/// staying inside `NextToken`'s `.*` pattern and 2048-character bound.
pub fn encode_next_token(cursor: &str) -> String {
    hex::encode(cursor.as_bytes())
}

/// The cursor inside a `NextToken`, or `None` when the token was not one this
/// service issued.
pub fn decode_next_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsChimesdkmessagingChimemessagingserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsChimesdkmessagingChimemessagingserviceState>)
}

/// The SHARED app-instance ADMIN directory in the shared engine: call it with a
/// handler's `&*self.engine`.
///
/// An `AppInstanceAdmin` is a resource of the Chime SDK Identity service, which records
/// every promotion in ONE directory under one shared type
/// ([`::service_common::chime_app_instance_admins::AppInstanceAdminDirectory`]); this
/// service READS it, because the model reserves two of its actions for an administrator
/// of the app instance — listing an account's PRIVATE channels (`ListChannels`) and
/// deleting a channel message (`DeleteChannelMessage`). The engine keys state by TYPE, so
/// a copy of its own inside
/// [`ComAmazonawsChimesdkmessagingChimemessagingserviceState`] would never see what the
/// identity service wrote.
///
/// LOCK ORDER: a reader takes [`service_state`] first and this directory second, and
/// never the other way round, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn app_instance_admins<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<
    ::engine::ServiceState<
        ::service_common::chime_app_instance_admins::AppInstanceAdminDirectory,
    >,
> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(
            ::engine::InMemoryEngine::state::<
                ::service_common::chime_app_instance_admins::AppInstanceAdminDirectory,
            >,
        )
}

/// The SHARED Chime TAG directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// A resource's tags are keyed by ARN and live in ONE directory
/// ([`::service_common::chime_tags::ChimeTagDirectory`]) rather than on the channel and
/// channel-flow records of this state struct, because the family's generic tagging routes
/// are served by one member for every member's resource: a copy inside
/// [`ComAmazonawsChimesdkmessagingChimemessagingserviceState`] would be invisible to the
/// sibling serving a tagging call, which would then report "no tags" for a channel that
/// has them.
///
/// LOCK ORDER: a writer takes [`service_state`] first and this directory second, and no
/// reader holds this one while taking another store, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn chime_tags<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<::service_common::chime_tags::ChimeTagDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<::service_common::chime_tags::ChimeTagDirectory>)
}

/// The SHARED Chime IDENTITY NAME directory in the shared engine: call it with a handler's
/// `&*self.engine`.
///
/// The name an `AppInstanceUser` or `AppInstanceBot` is registered under is a resource of
/// the Chime SDK Identity service, which publishes it in ONE ARN-keyed directory
/// ([`::service_common::chime_identity_names::ChimeIdentityNameDirectory`]); this service
/// READS it, because every participant it reports — a channel's creator, a message's
/// sender, a member, a banned member, a moderator, an inviter — is an `Identity` of an ARN
/// and that name. The engine keys state by TYPE, so a copy of its own inside
/// [`ComAmazonawsChimesdkmessagingChimemessagingserviceState`] would never see what the
/// identity service wrote.
///
/// LOCK ORDER: a reader takes [`service_state`] first and this directory second, and never
/// the other way round, so the order cannot invert.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn identity_names<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<IdentityNameDirectory>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<IdentityNameDirectory>)
}

/// The `aws.api#service$arnNamespace` a `LambdaFunctionArn` carries, which is what
/// `service_common::arn::region_of` matches a processor's ARN against.
const LAMBDA_ARN_VENDOR: &str = "lambda";

/// Why `CreateChannelFlow` must refuse a processor whose Lambda function this emulator
/// cannot resolve, or `None` when the channel flow may be written.
///
/// A channel flow's processors ARE Lambda functions: Amazon Chime SDK Messaging resolves
/// `Processor.Configuration.Lambda.ResourceArn` when the flow is created — not when a
/// message is processed — and refuses the request, persisting nothing, when that function
/// cannot be invoked. Lambda is reached the way any emulated service reaches another:
/// through the in-process callback
/// ([`smithy_server_runtime::OperationContext::callback`]), so this crate names no other
/// service crate.
///
/// `None` also covers the case where this emulator has no Lambda to ask (a router with
/// only this service mounted, say): nothing is then known about the function, and a
/// refusal has to be grounded in what Lambda answered, never assumed. What this emulator
/// does NOT do is decide whether an existing function is invocable BY the Chime SDK —
/// that is a resource policy on another account's function, which nothing here can read,
/// so it is never fabricated either way.
///
/// An ARN that is not a Lambda function ARN with a Region LABEL is refused WITHOUT asking
/// anything: there is no endpoint such an ARN resolves to, so nothing about it could be
/// looked up. The Region is read through `service_common::arn::region_of`, the one owner of
/// that rule — it reports the field only when it is a Region label (`[a-z0-9-]`, bounded,
/// non-empty), precisely because the value ends up in an endpoint URL's authority. The
/// model's `LambdaFunctionArn` pattern already refuses such an ARN before this handler
/// runs, which makes this the second of two gates rather than the only one.
pub async fn unresolvable_processor_lambda(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    resource_arn: &str,
) -> Option<String> {
    let Some(region) = service_common::arn::region_of(resource_arn, LAMBDA_ARN_VENDOR) else {
        return Some(format!(
            "{resource_arn} is not a Lambda function ARN naming a Region, so no Lambda \
             function can be resolved from it"
        ));
    };
    let config = ::aws_sdk_lambda::Config::builder()
        .behavior_version(::aws_sdk_lambda::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_lambda::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://lambda.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch for
        // a stalled stream, and each of those would need a sleep implementation this
        // client is not given. The same goes for caching the identity: the credentials
        // are handed over directly.
        .retry_config(::aws_sdk_lambda::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_lambda::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_lambda::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_lambda::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    let answer = ::aws_sdk_lambda::Client::from_conf(config)
        .get_function()
        .function_name(resource_arn)
        .send()
        .await;
    match answer {
        // The function is there, so nothing about it is a ground to refuse on.
        Ok(_) => None,
        // Lambda says this account holds no such function: the refusal is grounded.
        Err(error) if function_is_absent(&error) => Some(format!(
            "the processor Lambda function {resource_arn} could not be resolved, so it \
             cannot be invoked to process messages"
        )),
        // Any other answer is this emulator failing to ask, not Lambda reporting a bad
        // function, so it refuses nothing — and it is worth a warning, because a
        // deployment with no Lambda to ask is one where this gate does not run.
        Err(error) => {
            ::tracing::warn!(
                %resource_arn,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "could not resolve a channel flow processor's Lambda function; the \
                 channel flow is created without that check"
            );
            None
        }
    }
}

/// Whether a `GetFunction` failure is Lambda reporting that the function is not there,
/// rather than this emulator failing to ask.
///
/// One other service resolves a caller-supplied Lambda function the same way — see
/// `lambda_function_is_absent` in
/// `services/service-com-amazonaws-bedrockagentcorecontrol-amazonbedrockagentcorecontrol/src/state.rs`
/// — so the in-process client policy above and this classifier are to be MIGRATED into
/// `service-common` as one `lambda` reader, the way the ARN Region read already lives
/// there. They are left here only because this change does not own that service's issue.
fn function_is_absent(
    error: &::aws_sdk_lambda::error::SdkError<
        ::aws_sdk_lambda::operation::get_function::GetFunctionError,
        ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
    >,
) -> bool {
    error
        .as_service_error()
        .is_some_and(::aws_sdk_lambda::operation::get_function::GetFunctionError::is_resource_not_found_exception)
}
