// SPDX-License-Identifier: CC0-1.0

//! The ad marker a broadcasting channel's playlist carries: what `InsertAdBreak`
//! writes, and the one rule it writes by, kept beside the rest of this service's
//! state so every operation that touches an ad break agrees on the record's form.
//!
//! An ad break is a marker placed in the playlist of the stream a channel is
//! currently ingesting ("Inserts an ad marker in the playlist for the specified
//! channel and duration using the ad configuration associated with the channel",
//! `smithy_query(query="doc",shape="InsertAdBreak")`), so the record belongs to the
//! BROADCAST rather than to the channel: it is written only while the channel has a
//! live stream and goes when that stream does ([`super::end_stream`]). That is also
//! what bounds this map — one entry per broadcasting channel.
//!
//! The configuration the marker is inserted WITH is the channel's own, so a channel
//! with none validly associated has nothing to insert the break with: that
//! precondition is on the request member that named the channel, and it is stated
//! here beside the write it guards ([`check_channel_ad_configuration`]).

use super::*;

/// How many characters an `AdBreakId` carries: the modelled shape is `@length 12..12`
/// over `^[a-zA-Z0-9]+$` (`smithy_query(query="shape",shape="AdBreakId")`).
const AD_BREAK_ID_LEN: usize = 12;

/// One ad break a caller has inserted into a channel's playlist.
#[derive(Debug, Clone)]
pub struct AdBreakRecord {
    /// The `adBreakId` the insert reported, "unique identifier for the ad break that
    /// was inserted into the playlist" — minted once, here, so the id a caller holds
    /// is the id this store carries.
    pub ad_break_id: String,
    /// How long the break runs, as the caller asked (`durationSeconds`, modelled
    /// `@range 1..300`).
    pub duration_seconds: i32,
    /// The ad configuration the break was inserted with: the channel's own
    /// `adConfigurationArn`, which is what the operation documents it uses. An insert
    /// needs one associated ([`check_channel_ad_configuration`]), so a persisted record
    /// carries the configuration it was inserted with.
    pub ad_configuration_arn: Option<String>,
    /// The stream whose playlist took the marker, so a record is never read as
    /// belonging to a later broadcast of the same channel.
    pub stream_id: Option<String>,
    /// When the marker was inserted.
    pub inserted_at: SystemTime,
    /// When the break is TARGETED to start: "target time at which the ad break starts"
    /// (`InsertAdBreakRequest$targetStartAt`), as the caller asked. A request that asked
    /// for no time targets the instant the marker was inserted, because that is when the
    /// playlist took it.
    pub target_start_at: SystemTime,
}

/// The ad configuration the channel `channel_arn` names has associated, as this store
/// holds it.
///
/// A channel's association is the ARN it carries in `adConfigurationArn`, so the
/// configuration itself is that ARN RESOLVED against the ad configurations this store
/// holds. A channel that names none — the member absent, or the empty string a create
/// persists for a caller that sent nothing — and one whose member names an ad
/// configuration this store does not hold, both have none associated: there is no
/// resource for the service to use.
pub fn channel_ad_configuration<'a>(
    state: &'a ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> Option<&'a AdConfiguration> {
    let arn = state
        .channels
        .get(channel_arn)?
        .ad_configuration_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())?;
    state.ad_configurations.get(arn)
}

/// Whether the channel `channel_arn` names may take an ad break at all: an insert uses
/// "the ad configuration associated with the channel"
/// (`smithy_query(query="doc",shape="InsertAdBreak")`), so a channel with none
/// associated carries no configuration to insert the break with and the REQUEST member
/// that named it is what fails — hence a `ValidationException` stated by
/// [`super::member_constraint_message`], the sentence this service words every refusal
/// of one member's value with.
///
/// This precondition is on the request's own member and not on the channel's live
/// state, so it is checked before broadcast state is: a channel with no ad
/// configuration is refused this way whether or not it happens to be ingesting.
pub fn check_channel_ad_configuration(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
) -> Result<(), String> {
    if channel_ad_configuration(state, channel_arn).is_none() {
        return Err(member_constraint_message(
            AD_BREAK_CHANNEL_MEMBER,
            &format!(
                "valid ad configuration is required to be associated with the channel \
                 requested for the ad break with ARN: {channel_arn}"
            ),
        ));
    }
    Ok(())
}

/// The input member an insert's refusals name: the `channelArn` the request sent
/// (`smithy_query(query="operation",operation="InsertAdBreak")`), spelled as the model
/// spells it because that is the member the caller wrote.
pub const AD_BREAK_CHANNEL_MEMBER: &str = "channelArn";

/// Insert an ad break of `duration_seconds` into the playlist of the stream
/// `channel_arn` is ingesting, persist it as that channel's current marker and return
/// it.
///
/// The caller has already established that the channel exists, has an ad configuration
/// associated and is broadcasting — which is what the operation's
/// `ResourceNotFoundException`, `ValidationException` and `ChannelNotBroadcasting`
/// answers separate — so this records the marker the live playlist took. A later insert
/// on the same channel replaces it: the playlist carries the break most recently marked
/// in it.
///
/// `requested_id` is the "caller-specified unique identifier for the ad break to insert
/// into the playlist" (`InsertAdBreakRequest$adBreakId`): the marker carries the id the
/// caller chose, and only a request that chose none has one minted for it, so the id a
/// caller holds is always the id this store carries. `requested_start` is the target time
/// the request asked for, defaulting to the instant the marker was inserted.
pub fn insert_ad_break(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    duration_seconds: i32,
    requested_id: Option<String>,
    requested_start: Option<SystemTime>,
) -> AdBreakRecord {
    let ad_configuration_arn = channel_ad_configuration(state, channel_arn)
        .map(|configuration| configuration.arn.clone());
    let stream_id = live_stream(state, channel_arn).and_then(|stream| stream.stream_id);
    // Hex is a subset of the `[a-zA-Z0-9]` alphabet the modelled id admits and
    // `mint_hex` never repeats a value, so one byte per two characters of the
    // modelled width gives an id that satisfies the shape and is unique per call.
    let ad_break_id = match requested_id.filter(|id| !id.is_empty()) {
        Some(id) => id,
        None => state.mint_hex("ad-break", AD_BREAK_ID_LEN / 2),
    };
    let inserted_at = SystemTime::now();
    let record = AdBreakRecord {
        ad_break_id,
        duration_seconds,
        ad_configuration_arn,
        stream_id,
        inserted_at,
        target_start_at: requested_start.unwrap_or(inserted_at),
    };
    state
        .ad_breaks
        .insert(channel_arn.to_string(), record.clone());
    record
}
