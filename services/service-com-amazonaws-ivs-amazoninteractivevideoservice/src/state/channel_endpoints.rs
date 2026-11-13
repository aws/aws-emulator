// SPDX-License-Identifier: CC0-1.0

//! The two ADDRESSES every channel this service reports announces — the one an encoder
//! CONTRIBUTES to and the one a player FETCHES from — built here and nowhere else, so
//! every operation that projects a channel announces the same pair.
//!
//! Both name THIS emulator, because this emulator is the only thing serving the channel:
//!
//! * `ingestEndpoint`, and the `srt.endpoint` that equals it, is "part of the definition
//!   of an ingest server, used when you set up streaming software"
//!   (`smithy_query(query="shape",shape="Channel,Srt")`) — an address a client takes to an
//!   encoder, so it is the `host[:port]` this deployment answers on
//!   ([`announced_authority`]), not a name belonging to someone else. It is the account's,
//!   not the channel's: every channel this emulator serves reports the SAME value, so a
//!   client that cached the ingest server it was handed once configures the next channel's
//!   encoder correctly, and nothing of a channel's own identity is in it.
//! * `playbackUrl` is a URL a player FETCHES ("URL of the master playlist, required by the
//!   video player to play the HLS stream", `Stream$playbackUrl`), so its host is that same
//!   address and everything the published hostname carried is in the path AWS lays the
//!   playlist out at: the Region, the account and the channel id ([`playback_path`]), all
//!   three read off the channel's own ARN.
//!
//! Both are therefore composed at PROJECTION time, per request ([`announced_channel`],
//! [`announced_stream`]), and neither is stored: a channel reached at one address is never
//! answered another address's, and a read composes the values without writing anything.
//!
//! Nothing here is ever CONNECTED to. This module formats strings into a response and
//! holds no outbound client, socket or resolver; every address it composes is one of this
//! process's own inbound addresses, so no value it reports leads off this machine.

use super::*;

/// The path AWS serves a channel's HLS master playlist at, which is what a `playbackUrl`
/// carries after its host.
const PLAYBACK_PATH_PREFIX: &str = "/api/video/v1";

/// The extension of that playlist: an HLS master playlist.
const PLAYBACK_PATH_SUFFIX: &str = ".m3u8";

/// One channel as this service ANNOUNCES it to the request in `ctx`: the record it holds,
/// with both addresses composed for the address that request reached this emulator at.
///
/// Every operation that answers a `Channel` runs its record through this, so the single
/// read, the batch read, the create and the update all announce one pair of addresses. The
/// `srt` block keeps the passphrase the create generated and stored — the secret is the
/// channel's, while the endpoint beside it is the deployment's.
#[must_use]
pub fn announced_channel(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    channel: Channel,
) -> Channel {
    let ingest_endpoint = announced_ingest_endpoint(ctx);
    let playback_url = channel
        .arn
        .as_deref()
        .and_then(|arn| announced_playback_url(ctx, arn));
    let srt = channel.srt.map(|srt| crate::ops::types::Srt {
        endpoint: Some(ingest_endpoint.clone()),
        ..srt
    });
    Channel {
        ingest_endpoint: Some(ingest_endpoint),
        playback_url,
        srt,
        ..channel
    }
}

/// The ingest server every channel of this account contributes to: the `host[:port]` this
/// emulator answers on.
///
/// It is the ACCOUNT's and not a channel's — two channels report the same value, which is
/// what lets one encoder configuration serve the next channel — and it is the address a
/// client is already talking to, because an encoder can only reach what this machine
/// serves.
#[must_use]
pub fn announced_ingest_endpoint(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    announced_authority(ctx)
}

/// One live stream as this service announces it: "the stream is played back from its
/// channel's own URL", so the member is composed from the channel the stream is ingested
/// on, by the same rule [`announced_channel`] applies.
#[must_use]
pub fn announced_stream(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    stream: Stream,
) -> Stream {
    let playback_url = stream
        .channel_arn
        .as_deref()
        .and_then(|arn| announced_playback_url(ctx, arn));
    Stream {
        playback_url,
        ..stream
    }
}

/// The absolute URL a player fetches the channel `channel_arn` names at, or `None` when
/// that ARN names no channel — in which case there is no channel to compose a URL for and
/// the member is reported as absent rather than as a URL to nowhere.
#[must_use]
pub fn announced_playback_url(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    channel_arn: &str,
) -> Option<String> {
    let (region, account, channel_id) = channel_of_arn(channel_arn)?;
    Some(format!(
        "https://{}{}",
        announced_authority(ctx),
        playback_path(region, account, channel_id)
    ))
}

/// The `host[:port]` every address this emulator announces is reached at.
///
/// In the order the other data planes of this tree announce an address in:
///
/// * a request that addressed the EMULATOR'S OWN endpoint
///   (`service_common::data_plane_addressing::reached_emulator_endpoint`) comes from a
///   client that resolves nothing else, so the URL is announced on that same address,
///   port included;
/// * a channel read through any other request on an emulator that IS served on a port
///   (`service_common::data_plane_addressing::served_endpoint_authority`) is announced on
///   that port, the one address this process answers on, so a channel reported to an
///   in-process caller of another service names an address its own client can reach;
/// * an emulator served on no port at all has one base and announces it, so the value
///   still names this emulator rather than a host belonging to someone else.
///
/// A playback URL puts it behind `https`: every listener of this process presents the
/// core's certificate, and an HLS master playlist is fetched over HTTPS. An ingest
/// endpoint reports it as it stands, because `Channel$ingestEndpoint` carries a bare
/// address a client configures streaming software with rather than a URL.
fn announced_authority(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    let base = ctx.endpoint();
    let parts = ctx.parts();
    ::service_common::data_plane_addressing::reached_emulator_endpoint(
        &parts.uri.to_string(),
        parts
            .headers
            .get(::http::header::HOST)
            .and_then(|value| value.to_str().ok()),
    )
    .or_else(|| {
        ::service_common::data_plane_addressing::served_endpoint_authority(base.as_str())
    })
    .unwrap_or_else(|| {
        ::service_common::data_plane_addressing::url_authority(base.as_str())
            .trim_end_matches('/')
            .to_string()
    })
}

/// The path a channel's master playlist is served at:
/// `/api/video/v1/<region>.<account>.channel.<channel-id>.m3u8`, the layout AWS's own
/// playback URLs carry — the whole identity of the channel, which is why it survives
/// moving the value off AWS's hostname and onto this emulator's address.
fn playback_path(region: &str, account: &str, channel_id: &str) -> String {
    format!(
        "{PLAYBACK_PATH_PREFIX}/{region}.{account}.channel.{channel_id}{PLAYBACK_PATH_SUFFIX}"
    )
}

/// The Region, the account and the channel id a `ChannelArn` names
/// (`arn:aws:ivs:<region>:<account>:channel/<id>`, the form the modelled `ChannelArn`
/// pattern pins), or `None` for a value that is not one.
fn channel_of_arn(arn: &str) -> Option<(&str, &str, &str)> {
    let segments: Vec<&str> = arn.splitn(6, ':').collect();
    let [_, _, _, region, account, resource] = segments[..] else {
        return None;
    };
    let id = resource.strip_prefix("channel/")?;
    (!region.is_empty() && !account.is_empty() && !id.is_empty())
        .then_some((region, account, id))
}
