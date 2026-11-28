// SPDX-License-Identifier: CC0-1.0

//! The front-door route a channel's PLAYBACK URL is fetched on: how this service claims a
//! request that followed the `playbackUrl` it announced, and how it answers one.
//!
//! The announced value is an `https://` URL on a name of this service's own
//! ([`crate::state::announced_playback_url`]: a minted label over this service's regional
//! playback labels, in the zone the caller's addressing puts this emulator in, carrying no
//! port), so what is claimed here is the PATH that URL carries, whatever host it was fetched
//! at — this emulator's own endpoint for a player configured with it, and the announced name
//! for one that resolves it ([`crate::state::publish_playback_addresses`] is where an
//! in-process client reads which service answers at that name). The request is unsigned,
//! because a viewer holds no credentials, and on a path the model binds no operation to.
//! Such a request resolves no modelled operation, which is exactly the seam
//! [`::smithy_server_runtime::Service::override_routing_error`] is reached at, and so is where
//! the playlist is served; [`::smithy_server_runtime::Service::can_handle`] claims it first so
//! the router does not answer `404 UnknownService` for an address this service itself
//! published.
//!
//! Only a path naming a channel THIS store holds is claimed, so nothing modelled is
//! diverted here and an address this emulator never announced is answered the way the
//! router answers an address nobody published. What is served comes from the plane that
//! packaged it ([`crate::state::playback_plane`]), which is also where the layout of the
//! three paths one channel serves is stated — and WHO may be served it is
//! [`crate::state::playback_authorization`]: a public channel is served to any viewer, as
//! AWS serves one, and a private channel only to a viewer whose playback token this
//! account's imported keys verify for it.

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;

use crate::state::{
    parse_playback_path, playback_authorization, playback_channel_arn, serve_playback,
    PlaybackAnswer, PlaybackAuthorization, PLAYBACK_METHODS,
};

/// The status a playback request is answered with when this service's store is unreachable
/// — the plane's equivalent of the modelled server failure a handler answers with, in a
/// plane that has no modelled envelope to state it in.
const STORE_UNAVAILABLE_STATUS: u16 = 500;

/// The status a viewer is refused a PRIVATE channel's playback with: the channel is
/// "enabled for playback authorization" and the request presented no token this account's
/// imported playback keys verify for it (`crate::state::playback_authorization`). It is a
/// refusal of the credential rather than of the path, so it is not the plane's 404.
const UNAUTHORIZED_STATUS: u16 = 403;

/// Whether this request addresses the playback path of a channel THIS store holds.
///
/// The path carries the channel's own identity, so a path naming a channel this store does
/// not hold is claimed by nobody — which is what an address this emulator never announced
/// should answer.
pub(super) fn claims_playback_path<Eng: Send + Sync + 'static>(
    engine: &Eng,
    parts: &::smithy_server_runtime::http::request::Parts,
) -> bool {
    // The PATH the request addressed. `http::Uri::path` is that reading for a request's
    // PARTS — the type has already split an absolute-form target's authority off — which is
    // the same path the answer below reads with the workspace's reader of a raw target
    // (`service_common::request_path`), so the claim and the answer cannot disagree.
    let Some(target) = parse_playback_path(parts.uri.path()) else {
        return false;
    };
    let Some(state) = crate::state::service_state(engine) else {
        return false;
    };
    let store = state.lock();
    playback_channel_arn(&store, &target).is_some()
}

/// Serve one request on a channel's playback path, or `None` for a request that is not on
/// one — and for one whose path names a channel this store does not hold, which is an
/// address this emulator never announced: both keep the framework's own routing envelope,
/// exactly as [`claims_playback_path`] claims neither.
///
/// A PRIVATE channel is authorized first: this seam is reached before authentication
/// (which is what lets a credential-less viewer fetch a PUBLIC channel at all, as AWS
/// serves one), so a channel "enabled for playback authorization" is served only to a
/// request carrying a playback token this account's imported keys verify for it — the one
/// reading of that in [`crate::state::playback_authorization`] — and refused
/// [`UNAUTHORIZED_STATUS`] otherwise, without the stored media being read.
///
/// The decision is the plane's ([`serve_playback`]), under one hold of the store's guard;
/// this renders the answer it reports as the HTTP response that answer names, after the
/// guard is released.
pub(super) fn serve_authorized_playback_path<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
) -> Option<RtResponse> {
    // The PATH the request addressed, read by the workspace's one reader of it, so a
    // request that carries its endpoint in an absolute-form URI is read exactly like one
    // that carries it in the host.
    let path = ::service_common::request_path(req.uri());
    let target = parse_playback_path(path)?;
    let Some(state) = crate::state::service_state(engine) else {
        return Some(plane_status(STORE_UNAVAILABLE_STATUS));
    };
    // The token a viewer presents travels in the URI's own query string, which is read by
    // the workspace's one reader of that, for the same reason the path is.
    let query = ::service_common::request_query(req.uri());
    let answer = {
        let store = state.lock();
        match playback_authorization(&store, &target, query) {
            PlaybackAuthorization::Granted => serve_playback(&store, &target, req.method()),
            // The refusal is decided from the channel and the presented token alone, so
            // nothing of the channel's media is read for a viewer that may not have it.
            PlaybackAuthorization::Refused => {
                audit_refusal(path);
                return Some(plane_status(UNAUTHORIZED_STATUS));
            }
            PlaybackAuthorization::UnknownChannel => PlaybackAnswer::UnknownChannel,
        }
    };
    match answer {
        PlaybackAnswer::Served {
            content,
            media_type,
        } => Some(served(req, content, media_type)),
        // Nothing is packaged there: a channel that is not broadcasting, a broadcast whose
        // first segments are not packaged yet, or a segment that has left the live window.
        PlaybackAnswer::NotFound => Some(plane_status(404)),
        PlaybackAnswer::MethodRefused => Some(method_refused()),
        PlaybackAnswer::UnknownChannel => None,
    }
}

/// The record a REFUSED viewer leaves: which playback path was refused, and that it was
/// refused for want of a verified playback token.
///
/// This is the one surface of this service the framework does not authenticate, so its
/// refusals are attributable: the path names the channel the viewer asked for, which is a
/// public identifier the viewer already holds. The token itself is a CREDENTIAL and is
/// never recorded, nor is any header of the request.
fn audit_refusal(path: &str) {
    ::tracing::info!(
        plane = "playback",
        path = %path,
        outcome = "refused-unauthorized",
        "ivs playback authorization"
    );
}

/// One answer of this plane that carries no body: the status, and the framing header that
/// states the length it has.
fn plane_status(status: u16) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(status).expect("the status is a constant of this module"),
        SdkBody::empty(),
    );
    response.headers_mut().insert("content-length", "0");
    response
}

/// A 405 naming the methods a viewer reads with, as RFC 9110 requires of one.
fn method_refused() -> RtResponse {
    let mut response = plane_status(405);
    response.headers_mut().insert("allow", PLAYBACK_METHODS);
    response
}

/// Serve `content` as `media_type`: the bytes, their length and their type — and for a
/// `HEAD`, the same answer without the body, which is what that method asks for.
fn served(req: &RtRequest, content: Vec<u8>, media_type: &'static str) -> RtResponse {
    let length = content.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(200_u16).expect("200 is a valid status"),
        match req.method() == "HEAD" {
            true => SdkBody::empty(),
            false => SdkBody::from(content),
        },
    );
    response.headers_mut().insert("content-type", media_type);
    response
        .headers_mut()
        .insert("content-length", length.to_string());
    response
}
