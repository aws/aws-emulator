// SPDX-License-Identifier: CC0-1.0

//! The SESSIONS a channel's broadcasts leave behind: one record per stream a contribution
//! brought up, kept after that stream has ended so the session reads can report on it.
//!
//! RULE: a stream session is the record of ONE broadcast — "the Amazon IVS configuration
//! that the customer provisioned, the ingest configurations that the broadcaster used, and
//! the most recent Amazon IVS stream events it encountered"
//! (`smithy_query(query="shape",shape="StreamSession")`) — so it is written by the
//! contribution that started the broadcast ([`start_session`]), carries the properties the
//! channel had "at the time of going live" (`StreamSession$channel`), and is CLOSED when
//! the broadcast ends ([`end_session`]): `startTime` is "time when the channel went live"
//! and `endTime` "time when the channel went offline … For live streams, this is `NULL`".
//!
//! The ingest configuration is the ENCODER's, not this emulator's: it is filled from the
//! metadata the publishing encoder sent on the contribute connection
//! ([`ingest_configuration_of_metadata`]), so a session reports the width, height, codecs
//! and rates the broadcaster really published with, and reports nothing where the encoder
//! sent nothing.
//!
//! The records are bounded: a channel keeps its [`MAX_SESSIONS_PER_CHANNEL`] most recent
//! sessions, because a channel can be broadcast to as often as a caller likes while the
//! store lives for the life of the process, and `DeleteChannel` drops them with the channel
//! they belong to.

use super::*;

/// How many stream sessions one channel keeps.
///
/// The emulator's own bound, not a published AWS limit: sessions accumulate one per
/// broadcast with nothing to remove them, so the oldest is dropped past this. It is set
/// high enough that the session reads of an ordinary workflow — which look at the current
/// or the previous broadcast — never miss a session.
pub const MAX_SESSIONS_PER_CHANNEL: usize = 20;

/// The one track a channel ingests when multitrack is not enabled, which is what the model
/// names it: "If multitrack is not enabled, this is Track0 (the sole track)"
/// (`VideoConfiguration$track`).
const SOLE_TRACK: &str = "Track0";

/// One broadcast of one channel, as this store holds it.
#[derive(Debug, Clone)]
pub struct StreamSessionRecord {
    /// The stream the broadcast was ingested as, which is the id a session read asks for.
    pub stream_id: String,
    /// When the channel went live.
    pub start_time: SystemTime,
    /// When the channel went offline, or `None` while the broadcast is still live.
    pub end_time: Option<SystemTime>,
    /// The channel's properties at the time of going live, which is what the model says
    /// this member carries — a snapshot, so a later update of the channel does not rewrite
    /// the history of a broadcast that is already over.
    pub channel: Channel,
    /// The recording configuration the channel named at that time, when it named one.
    pub recording_configuration: Option<RecordingConfiguration>,
    /// The failover slate configuration the channel named at that time, when it named one:
    /// "the properties of the failover slate configuration at the time of going live"
    /// (`StreamSession$failoverSlateConfiguration`), so it is snapshotted beside the
    /// recording configuration and for the same reason.
    pub failover_slate_configuration: Option<FailoverSlateConfiguration>,
    /// What the broadcaster's encoder said it was sending, or `None` until it said
    /// anything.
    pub ingest_configuration: Option<IngestConfiguration>,
}

/// Record the broadcast `stream_id` on `channel_arn` as started at `started_at`.
///
/// Called by the contribution that brought the channel live and by nothing else: the
/// control plane has no operation that starts a broadcast. A session already recorded for
/// the same stream id is left as it stands, so a reconnecting publisher of the same stream
/// does not restart its history.
pub fn start_session(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    stream_id: &str,
    started_at: SystemTime,
) {
    let Some(channel) = state.channels.get(channel_arn).cloned() else {
        return;
    };
    let recording_configuration = channel
        .recording_configuration_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
        .and_then(|arn| state.recording_configurations.get(arn))
        .map(|record| record.configuration.clone());
    // The same reading for the failover slate the channel names, through the one resolver
    // of that association ([`channel_failover_slate_configuration`]).
    let failover_slate_configuration =
        channel_failover_slate_configuration(state, channel_arn).cloned();
    let sessions = state
        .stream_sessions
        .entry(channel_arn.to_string())
        .or_default();
    if sessions
        .iter()
        .any(|session| session.stream_id == stream_id)
    {
        return;
    }
    sessions.push(StreamSessionRecord {
        stream_id: stream_id.to_string(),
        start_time: started_at,
        end_time: None,
        channel,
        recording_configuration,
        failover_slate_configuration,
        ingest_configuration: None,
    });
    while sessions.len() > MAX_SESSIONS_PER_CHANNEL {
        sessions.remove(0);
    }
}

/// Close the OPEN session of `channel_arn` at `ended_at`: the channel has gone offline, so
/// the session that was live now reports when it ended.
///
/// A channel with no open session is left alone, which is what a second end of the same
/// broadcast — a publisher dropping after `StopStream` already disconnected it — must do.
pub fn end_session(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    ended_at: SystemTime,
) {
    let Some(sessions) = state.stream_sessions.get_mut(channel_arn) else {
        return;
    };
    for session in sessions.iter_mut().rev() {
        if session.end_time.is_none() {
            session.end_time = Some(ended_at);
            return;
        }
    }
}

/// Record what the encoder publishing `stream_id` on `channel_arn` says it is sending.
///
/// The encoder may send its metadata again mid-broadcast (a resolution change), and the
/// session reports what it last said.
pub fn set_session_ingest_configuration(
    state: &mut ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    stream_id: &str,
    ingest_configuration: IngestConfiguration,
) {
    let Some(sessions) = state.stream_sessions.get_mut(channel_arn) else {
        return;
    };
    if let Some(session) = sessions
        .iter_mut()
        .rev()
        .find(|session| session.stream_id == stream_id)
    {
        session.ingest_configuration = Some(ingest_configuration);
    }
}

/// The session a read asks for: the one `stream_id` names, or — "if no streamId is
/// provided, this returns the most recent stream session for the channel, if it exists"
/// (`GetStreamSessionRequest$streamId`) — the most recent one.
#[must_use]
pub fn stream_session(
    state: &ComAmazonawsIvsAmazoninteractivevideoserviceState,
    channel_arn: &str,
    stream_id: Option<&str>,
) -> Option<StreamSessionRecord> {
    let sessions = state.stream_sessions.get(channel_arn)?;
    match stream_id {
        Some(stream_id) => sessions
            .iter()
            .rev()
            .find(|session| session.stream_id == stream_id),
        None => sessions.last(),
    }
    .cloned()
}

/// One session as this service ANNOUNCES it to the request in `ctx`, on a deployment whose
/// contribute plane answers at `contribute`: the record it holds, with the channel it
/// snapshotted announced through the projection every channel read shares, so the addresses
/// a session reports are the addresses that caller is told everywhere else.
#[must_use]
pub fn announced_stream_session(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    contribute: Option<&str>,
    record: StreamSessionRecord,
) -> StreamSession {
    let ingest_configurations = record.ingest_configuration.clone().map(|ingest| {
        // "If multitrack is not enabled, `ingestConfiguration` and `ingestConfigurations`
        // contain the same data, namely information about Track0 (the sole track)."
        IngestConfigurations {
            video_configurations: ingest.video.clone().into_iter().collect(),
            audio_configurations: ingest.audio.clone().into_iter().collect(),
        }
    });
    StreamSession {
        stream_id: Some(record.stream_id),
        start_time: Some(::aws_smithy_types::DateTime::from(record.start_time)),
        end_time: record.end_time.map(::aws_smithy_types::DateTime::from),
        channel: Some(announced_channel(ctx, contribute, record.channel)),
        ingest_configuration: record.ingest_configuration,
        ingest_configurations,
        recording_configuration: record.recording_configuration,
        failover_slate_configuration: record.failover_slate_configuration,
        // No stream event is recorded for a broadcast this emulator ingested: the list is
        // "Amazon IVS events that the stream encountered", and a broadcast that ran and
        // ended encountered none, so the member is the empty list rather than an invented
        // event.
        truncated_events: Some(Vec::new()),
    }
}

/// The ingest configuration an encoder's own metadata states.
///
/// The members are the encoder's, one for one: a value the encoder did not send is left
/// absent (a string) or at the zero the model defaults it to (a number), because nothing
/// about the stream is known where the broadcaster said nothing. The track name is the sole
/// track's, since a channel this plane ingests carries one.
#[must_use]
pub fn ingest_configuration_of_metadata(metadata: &PublishedMetadata) -> IngestConfiguration {
    IngestConfiguration {
        video: Some(VideoConfiguration {
            codec: metadata.video_codec_id.and_then(video_codec_name),
            encoder: metadata.encoder.clone(),
            // The expected ingest bitrate is "bits per second", and an encoder states it
            // in kilobits per second.
            target_bitrate: metadata.video_bitrate_kbps.map_or(0, |kbps| {
                i64::from(kbps).saturating_mul(1_000)
            }),
            target_framerate: metadata.video_frame_rate.map_or(0, |rate| rate as i64),
            video_height: metadata.video_height.map_or(0, i64::from),
            video_width: metadata.video_width.map_or(0, i64::from),
            track: Some(SOLE_TRACK.to_string()),
            ..VideoConfiguration::default()
        }),
        audio: Some(AudioConfiguration {
            codec: metadata.audio_codec_id.and_then(audio_codec_name),
            target_bitrate: metadata.audio_bitrate_kbps.map_or(0, |kbps| {
                i64::from(kbps).saturating_mul(1_000)
            }),
            sample_rate: metadata.audio_sample_rate.map_or(0, i64::from),
            channels: metadata.audio_channels.map_or(0, i64::from),
            track: Some(SOLE_TRACK.to_string()),
        }),
    }
}

/// The codec NAME a publisher's video codec id states, or `None` for an id this service
/// cannot name a codec for — in which case the member is reported absent rather than under
/// an invented name.
///
/// A publisher states the codec as the FLV codec id its packets carry (the `videocodecid`
/// of its metadata), while `VideoConfiguration$codec` is the "codec used for the video
/// encoding" as a NAME, so the id is mapped to the RFC 6381 identifier of the codec it
/// names: 7 is AVC (H.264) and 12 is HEVC, the two an RTMP publisher sends video with.
fn video_codec_name(codec_id: u32) -> Option<String> {
    match codec_id {
        7 => Some("avc1".to_string()),
        12 => Some("hvc1".to_string()),
        _ => None,
    }
}

/// The same for a publisher's audio codec id: 10 is AAC and 2 is MP3, the two an RTMP
/// publisher sends audio with.
fn audio_codec_name(codec_id: u32) -> Option<String> {
    match codec_id {
        2 => Some("mp3".to_string()),
        10 => Some("mp4a.40.2".to_string()),
        _ => None,
    }
}

/// What a publishing encoder said it was sending, as the contribute plane read it off the
/// connection.
///
/// A plain record rather than the wire library's own type, so the rule above is stated
/// over this service's own vocabulary and the wire module is the only thing that names
/// RTMP's.
#[derive(Debug, Clone, Default)]
pub struct PublishedMetadata {
    /// Video-resolution width in pixels, as the encoder stated it.
    pub video_width: Option<u32>,
    /// Video-resolution height in pixels.
    pub video_height: Option<u32>,
    /// The codec id the video is encoded with, as the encoder's metadata states it.
    pub video_codec_id: Option<u32>,
    /// The framerate the encoder is sending.
    pub video_frame_rate: Option<f32>,
    /// The video bitrate the encoder is configured for, in kilobits per second.
    pub video_bitrate_kbps: Option<u32>,
    /// The codec id the audio is encoded with.
    pub audio_codec_id: Option<u32>,
    /// The audio bitrate, in kilobits per second.
    pub audio_bitrate_kbps: Option<u32>,
    /// How many audio samples are recorded per second.
    pub audio_sample_rate: Option<u32>,
    /// How many audio channels the encoder is sending.
    pub audio_channels: Option<u32>,
    /// The software or hardware the video is encoded with.
    pub encoder: Option<String>,
}
