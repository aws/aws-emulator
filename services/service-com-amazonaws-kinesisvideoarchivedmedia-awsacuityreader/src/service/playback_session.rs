// SPDX-License-Identifier: CC0-1.0

//! The PLAYBACK plane of a streaming session: the playlists, the manifest and the
//! media fragments a player fetches from the URL `GetHLSStreamingSessionURL` and
//! `GetDASHStreamingSessionURL` hand out.
//!
//! RULE: a URL an operation returns for the caller to FETCH is served by this
//! emulator at the address it reported. Those two operations report "an
//! authenticated URL (that includes an encrypted session token) for the session's
//! HLS master playlist (the root resource needed for streaming with HLS)" — a URL
//! on the endpoint the caller reached this emulator on
//! ([`crate::state::session_url`]) — and the documentation then names the
//! resources a media player calls for, from that playlist down to the media:
//! `GetHLSMasterPlaylist`, `GetHLSMediaPlaylist`, `GetMP4InitFragment`,
//! `GetMP4MediaFragment` and `GetTSFragment` for HLS, the MPEG-DASH manifest and
//! the same MP4 pair for MPEG-DASH. This module serves exactly those, at the paths
//! [`crate::state::session_resource_path`] lays out, so a player that follows the
//! URL is answered by a playlist instead of by the front door.
//!
//! # What it serves is the session that was minted
//!
//! Every answer is computed from the [`crate::state::StreamingSession`] the
//! minting call recorded and from the archive the stream holds at that instant,
//! so each parameter of the request that minted it is observable on the wire:
//!
//! * `ContainerFormat` decides whether the media playlist names the MP4 pair or
//!   the TS fragment ("If the ContainerFormat is MPEG_TS, this API is used instead
//!   of GetMP4InitFragment and GetMP4MediaFragment");
//! * `DiscontinuityMode` decides where the media playlist carries discontinuity
//!   markers, `DisplayFragmentTimestamp` whether it carries each fragment's start
//!   timestamp, and for MPEG-DASH those two decide the `kvs:ts` and `kvs:fn`
//!   attributes of a manifest's `S` elements;
//! * the session's selection — its window, its timestamp origin and its
//!   `MaxMediaPlaylistFragmentResults` / `MaxManifestFragmentResults` cap — is
//!   what decides the segments the playlist lists and the only media the session
//!   serves, because "the media that is made available through the playlist
//!   consists only of the requested stream, time range, and format".
//!
//! A session whose playlist GROWS ([`crate::state::StreamingSession::grows`]) has
//! that selection applied to the archive AGAIN on every fetch, so a player that
//! re-reads the playlist of a LIVE or LIVE_REPLAY session is served the fragments
//! ingested since — which is what such a playlist, left open and typed
//! `dynamic` here, tells it to poll for. A session over a closed stretch of the
//! past serves the playlist it was minted with, less whatever retention has
//! dropped from the archive since.
//!
//! # Who may reach it
//!
//! A player holds no AWS credentials: it fetches the URL it was handed, unsigned,
//! on a path this service's model binds no operation to. Such a request names no
//! service at all, so [`Service::can_handle`](smithy_server_runtime::Service::can_handle)
//! claims it by its PATH ([`claims_playback_path`]) — otherwise the router answers
//! `404 UnknownService` for an address this emulator itself published — and it is
//! answered through
//! [`Service::override_routing_error`](smithy_server_runtime::Service::override_routing_error),
//! the seam a claimed request that resolves no modelled operation reaches. The
//! session token in the URL is what authorises it: a token no live session answers
//! to is served nothing, which is also how an EXPIRED session answers, because
//! "when a session expires, no new calls to GetHLSMasterPlaylist,
//! GetHLSMediaPlaylist, GetMP4InitFragment, GetMP4MediaFragment, or GetTSFragment
//! can be made for that session".
//!
//! Serving a session resource writes nothing: the session record and the archive
//! are read, so what a player fetches cannot change what any later caller sees.
//!
//! # What one request costs
//!
//! Bounded by the session and the archive, not by the request: a playlist lists
//! the fragments the session selects (at most the `MaxMediaPlaylistFragmentResults`
//! / `MaxManifestFragmentResults` the model caps at 5,000, and at most the
//! [`crate::state::MAX_ARCHIVED_FRAGMENTS_PER_STREAM`] the archive holds), and a
//! media answer reads ONE archived fragment — whose bytes the archive's own
//! [`crate::state::MAX_ARCHIVED_MEDIA_BYTES_PER_STREAM`] ceiling caps, and whose
//! container is read under the element budget
//! [`crate::service::archived_container`] applies. Nothing here is proportional to
//! anything a playback request carries.

use ::std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_common::h264_mp4::VideoTrack;

use crate::service::playback_manifest::{
    self, DASH_MANIFEST_MEDIA_TYPE, FRAGMENTED_MP4, HLS_PLAYLIST_MEDIA_TYPE,
};
use crate::service::playback_media::{self, MP4_MEDIA_TYPE, TS_MEDIA_TYPE};
use crate::state::{
    epoch_millis, service_state, SessionProtocol, StreamingSession, DASH_MANIFEST_RESOURCE,
    FRAGMENT_NUMBER_FIELD, HLS_MASTER_PLAYLIST_RESOURCE, HLS_MEDIA_PLAYLIST_RESOURCE,
    MP4_INIT_FRAGMENT_RESOURCE, MP4_MEDIA_FRAGMENT_RESOURCE, SESSION_PATH_VERSION,
    SESSION_TOKEN_FIELD, TRACK_NUMBER_FIELD, TS_FRAGMENT_RESOURCE,
};

/// The methods a player reads a session resource with, as an `Allow` header states
/// them.
const PLAYBACK_METHODS: &str = "GET, HEAD";

/// The media type the plane's one non-media answer carries: a playback URL is
/// opened by a media player rather than by an AWS SDK, so what is served for an
/// address that resolves to nothing is the status itself with a one-line reason,
/// and no API error envelope.
const PLAIN_MEDIA_TYPE: &str = "text/plain; charset=utf-8";

/// What a session resource that resolves to nothing is answered with.
const NO_SESSION: &str = "no streaming session resource at this location\n";

/// Which resource of a session one request addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    /// The HLS master playlist, the root resource of an HLS session.
    MasterPlaylist,
    /// The HLS media playlist of one track.
    MediaPlaylist,
    /// The MPEG-DASH manifest, the root resource of an MPEG-DASH session.
    Manifest,
    /// The MP4 initialization fragment of one track.
    InitFragment,
    /// One MP4 media fragment.
    MediaFragment,
    /// One MPEG TS fragment.
    TsFragment,
}

impl Resource {
    /// The resource `protocol`'s playback plane serves under this name, or `None`
    /// when that plane serves no such resource — an MPEG-DASH session has no
    /// playlists and an HLS session no manifest.
    fn of_name(protocol: SessionProtocol, name: &str) -> Option<Self> {
        let resource = match name {
            HLS_MASTER_PLAYLIST_RESOURCE => Self::MasterPlaylist,
            HLS_MEDIA_PLAYLIST_RESOURCE => Self::MediaPlaylist,
            DASH_MANIFEST_RESOURCE => Self::Manifest,
            MP4_INIT_FRAGMENT_RESOURCE => Self::InitFragment,
            MP4_MEDIA_FRAGMENT_RESOURCE => Self::MediaFragment,
            TS_FRAGMENT_RESOURCE => Self::TsFragment,
            _ => return None,
        };
        let serves = !matches!(
            (protocol, resource),
            (SessionProtocol::Hls, Self::Manifest)
                | (
                    SessionProtocol::Dash,
                    Self::MasterPlaylist | Self::MediaPlaylist | Self::TsFragment
                )
        );
        serves.then_some(resource)
    }

    /// Whether `session` serves this resource at all.
    ///
    /// The media resources of a session are the ones its `ContainerFormat` names:
    /// an `MPEG_TS` session serves the TS fragment, which "is used instead of
    /// GetMP4InitFragment and GetMP4MediaFragment", and a session of MP4 fragments
    /// serves that pair and no TS fragment. The playlists and the manifest are a
    /// session's whatever it packages its media in.
    fn served_by(self, session: &StreamingSession) -> bool {
        let fragmented_mp4 = session.container_format == FRAGMENTED_MP4;
        match self {
            Self::InitFragment | Self::MediaFragment => fragmented_mp4,
            Self::TsFragment => !fragmented_mp4,
            _ => true,
        }
    }
}

/// One fragment of a session's playlist, as the archive holds it.
///
/// The timestamp is the one the session's own fragment selector reads
/// ([`StreamingSession::origin`]), because that is the timestamp a playlist or
/// manifest reports for a fragment.
#[derive(Debug, Clone)]
pub(crate) struct PlayedFragment {
    /// The fragment number the archive holds it under, which is how a media
    /// fragment addresses it.
    pub(crate) number: String,
    /// The fragment's selected start timestamp, in milliseconds since the epoch.
    pub(crate) start_millis: i64,
    /// The playback duration the archive records for it, in milliseconds.
    pub(crate) duration_millis: u64,
    /// How many media bytes it holds, which is what a playlist's peak rate is
    /// computed from.
    pub(crate) size_bytes: u64,
}

/// What one session serves, as the playlists and the manifest report it: the
/// session as it was minted, the fragments it lists right now, and the video
/// track those fragments declare.
#[derive(Debug)]
pub(crate) struct PlaybackView<'a> {
    /// The session the URL's token names.
    pub(crate) session: &'a StreamingSession,
    /// Its playlist, in the order the session's own selection puts it in.
    pub(crate) fragments: Vec<PlayedFragment>,
    /// The media sequence number of the first of them, which rises as fragments
    /// slide out of the front of a growing playlist.
    pub(crate) media_sequence: u64,
    /// The video track the session's media declares.
    pub(crate) track: VideoTrack,
    /// That track's `TrackNumber`, which a playlist addresses a per-track
    /// resource by.
    pub(crate) track_number: u64,
}

/// What a playback answer is computed from, COPIED out of the store: everything
/// the rendering and the muxing below need, so neither runs while the shared store
/// is locked.
#[derive(Debug)]
struct Resolved {
    /// The session the URL's token names, as it was minted.
    session: StreamingSession,
    /// Its playlist, as the archive stands now.
    fragments: Vec<PlayedFragment>,
    /// The media sequence number of its first fragment.
    media_sequence: u64,
    /// The archived container the session's video track is read from: the first
    /// fragment of its playlist.
    first_media: Vec<u8>,
    /// The fragment a media request asked for and the container that holds it,
    /// when the session serves one under that number.
    asked: Option<(PlayedFragment, Vec<u8>)>,
}

/// What `token` resolves to in `store`, or `None` when it resolves to nothing this
/// plane serves.
///
/// Everything this reads is stored state: the session record, the stream the
/// control plane holds for it right now, and the fragments of the session's
/// playlist the archive holds at this instant — which, for a session whose
/// playlist grows, is re-selected here rather than read from the mint
/// ([`StreamingSession::playlist`]), so a fragment ingested since is served and
/// one retention has dropped is not. Nothing is written.
fn resolved_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &crate::state::ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState,
    protocol: SessionProtocol,
    resource: Resource,
    token: &str,
    asked_number: &str,
    now: SystemTime,
) -> Option<Resolved> {
    // The session the token names, while its own `Expires` window is open, and on
    // the plane it was minted for: an HLS token addresses no MPEG-DASH manifest.
    let session = store
        .live_session(token, now)
        .filter(|session| session.protocol == protocol)?;
    // A session serves the media resources its own `ContainerFormat` names, and no
    // others.
    if !resource.served_by(session) {
        return None;
    }
    // The stream the session plays, as the control plane holds it RIGHT NOW: a
    // session over a stream that has since been deleted serves nothing, because
    // the archive it played belongs to a stream that is no longer there.
    let stream = store
        .resolve_stream(engine, None, Some(&session.stream_name), None)
        .ok()?;

    let playlist = session.playlist(&stream, now);
    let played: Vec<(&crate::state::StoredFragment, PlayedFragment)> = playlist
        .fragment_numbers
        .iter()
        .filter_map(|number| stream.fragment(number))
        .map(|fragment| {
            (
                fragment,
                PlayedFragment {
                    number: fragment.fragment_number.clone(),
                    start_millis: epoch_millis(session.origin.of(fragment)),
                    duration_millis: fragment.duration_millis,
                    size_bytes: fragment.size_bytes(),
                },
            )
        })
        .collect();
    let (first, _) = played.first()?;

    Some(Resolved {
        session: session.clone(),
        media_sequence: playlist.first_sequence,
        first_media: first.media.clone(),
        asked: played
            .iter()
            .find(|(_, played)| played.number == asked_number)
            .map(|(fragment, played)| (played.clone(), fragment.media.clone())),
        fragments: played.into_iter().map(|(_, played)| played).collect(),
    })
}

/// Whether this request addresses a resource of this service's playback plane.
///
/// The decision is the PATH alone — `/<hls|dash>/v1/<resource>` — so the plane
/// claims its own two path spaces and nothing else: a request for a token no
/// session answers to is still a request to this plane, and is answered by it
/// with nothing rather than by the router's `404 UnknownService`. No peer of this
/// signing name binds `hls` or `dash` as a first path segment, and this service's
/// own model binds neither (`OPERATIONS.md` lists its six `POST` routes), so
/// nothing modelled is diverted here.
#[must_use]
pub fn claims_playback_path(parts: &::smithy_server_runtime::http::request::Parts) -> bool {
    addressed(parts.uri.path()).is_some()
}

/// Serve one request to a session resource, or `None` for a request that is not on
/// one — which keeps the framework's own routing envelope, exactly as
/// [`claims_playback_path`] claims nothing for it.
pub fn serve_playback_path<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
) -> Option<RtResponse> {
    // The PATH the request addressed, read by the workspace's one reader of it, so
    // a request that carries its endpoint in an absolute-form URI is read exactly
    // like one that carries it in the host.
    let (protocol, resource) = addressed(::service_common::request_path(req.uri()))?;
    if !matches!(req.method(), "GET" | "HEAD") {
        return Some(method_refused());
    }
    let token = field(req.uri(), SESSION_TOKEN_FIELD).unwrap_or_default();
    let asked_number = field(req.uri(), FRAGMENT_NUMBER_FIELD).unwrap_or_default();
    let Some(state) = service_state(engine) else {
        // Unreachable in the assembled emulator, and answered as the server
        // failure it would be rather than as an empty playlist.
        return Some(answer(req, 500, PLAIN_MEDIA_TYPE, NO_SESSION.as_bytes().to_vec()));
    };

    // PHASE ONE, under the store's guard: WHAT the session serves, copied out of
    // the store — the session record, its playlist, and the archived containers the
    // answer is computed from. Nothing is read inside a container and nothing is
    // written here, so the shared state is held for a lookup and a copy rather than
    // for the length of a mux, and no other caller of this store waits on one.
    let resolved = {
        let store = state.lock();
        resolved_session(
            engine,
            &store,
            protocol,
            resource,
            &token,
            &asked_number,
            SystemTime::now(),
        )
    };
    let Some(resolved) = resolved else {
        return Some(not_found(req));
    };

    // PHASE TWO, with the guard released: the track the session's media declares,
    // read from the first fragment it serves — the geometry and parameter sets a
    // playlist reports and an initialization fragment carries.
    let Ok(track) = playback_media::fragment_track(&resolved.first_media) else {
        return Some(not_found(req));
    };
    // A per-track resource is served for the track the session's media declares
    // and for no other: a request naming another track addresses a track this
    // session does not serve.
    if field(req.uri(), TRACK_NUMBER_FIELD)
        .is_some_and(|asked| asked != track.number.to_string())
    {
        return Some(not_found(req));
    }

    let asked = resolved.asked;
    let view = PlaybackView {
        session: &resolved.session,
        fragments: resolved.fragments,
        media_sequence: resolved.media_sequence,
        track: track.track,
        track_number: track.number,
    };

    // A fault reading or writing the session's media is reported as the absence of
    // that resource and RECORDED, so a container this plane could not serve — one
    // whose frames do not read, or whose media is over the ceilings one answer may
    // assemble — is diagnosable rather than silently missing.
    let written = |resource: &str, written: Result<Vec<u8>, crate::service::archived_container::ContainerFault>| {
        match written {
            Ok(bytes) => Some(bytes),
            Err(fault) => {
                ::tracing::warn!(
                    resource,
                    fault = ?fault,
                    "the archived media of a streaming session could not be served"
                );
                None
            }
        }
    };

    let served = match resource {
        Resource::MasterPlaylist => Some((
            playback_manifest::master_playlist(&view).into_bytes(),
            HLS_PLAYLIST_MEDIA_TYPE,
        )),
        Resource::MediaPlaylist => Some((
            playback_manifest::media_playlist(&view).into_bytes(),
            HLS_PLAYLIST_MEDIA_TYPE,
        )),
        Resource::Manifest => Some((
            playback_manifest::dash_manifest(&view).into_bytes(),
            DASH_MANIFEST_MEDIA_TYPE,
        )),
        Resource::InitFragment => written(
            MP4_INIT_FRAGMENT_RESOURCE,
            playback_media::mp4_init_fragment(&view.track),
        )
        .map(|bytes| (bytes, MP4_MEDIA_TYPE)),
        Resource::MediaFragment | Resource::TsFragment => {
            asked.and_then(|(played, media)| match resource {
                Resource::TsFragment => written(
                    TS_FRAGMENT_RESOURCE,
                    playback_media::ts_fragment(
                        &media,
                        played.start_millis.saturating_sub(view.first_start_millis()),
                    ),
                )
                .map(|bytes| (bytes, TS_MEDIA_TYPE)),
                _ => written(
                    MP4_MEDIA_FRAGMENT_RESOURCE,
                    playback_media::mp4_media_fragment(&media, played.duration_millis),
                )
                .map(|bytes| (bytes, MP4_MEDIA_TYPE)),
            })
        }
    };

    match served {
        Some((bytes, media_type)) => Some(answer(req, 200, media_type, bytes)),
        // Media this plane could not read or write is media it has nothing to
        // serve for: the archived container of a session whose media declares no
        // H.264 video track, or a fragment number the session does not serve.
        None => Some(not_found(req)),
    }
}

impl PlaybackView<'_> {
    /// Where the session's playlist starts, in milliseconds since the epoch: the
    /// selected timestamp of its first fragment, which every reported offset of
    /// the session is counted from.
    pub(crate) fn first_start_millis(&self) -> i64 {
        self.fragments
            .first()
            .map_or(0, |fragment| fragment.start_millis)
    }

    /// The path, relative to a resource of this session, naming `resource` of the
    /// same session: the resource name and this session's own token.
    ///
    /// RELATIVE, so a player resolves it against the URL it was handed and comes
    /// back to the endpoint it reached this emulator on, whatever that was.
    pub(crate) fn sibling(&self, resource: &str, extra: &[(&str, &str)]) -> String {
        let mut path = format!("{resource}?{SESSION_TOKEN_FIELD}={}", self.session.token);
        for (field, value) in extra {
            path.push('&');
            path.push_str(field);
            path.push('=');
            path.push_str(value);
        }
        path
    }
}

/// The session resource `path` addresses, or `None` when it addresses none.
///
/// The layout is [`crate::state::session_resource_path`]'s, read back:
/// `/<protocol>/v1/<resource>`.
fn addressed(path: &str) -> Option<(SessionProtocol, Resource)> {
    let mut segments = path.split('/').filter(|segment| !segment.is_empty());
    let protocol = SessionProtocol::of_url_prefix(segments.next()?)?;
    if segments.next()? != SESSION_PATH_VERSION {
        return None;
    }
    let resource = Resource::of_name(protocol, segments.next()?)?;
    segments.next().is_none().then_some((protocol, resource))
}

/// The decoded value of one query field of `uri`, or `None` when it carries none.
fn field(uri: &str, name: &str) -> Option<String> {
    let query = uri.split_once('?').map(|(_, query)| query)?;
    ::service_common::query_parameter(query, name)
        .map(::smithy_server_runtime::percent_decode)
}

/// What a request for a resource that resolves to nothing is answered with.
fn not_found(req: &RtRequest) -> RtResponse {
    answer(req, 404, PLAIN_MEDIA_TYPE, NO_SESSION.as_bytes().to_vec())
}

/// A `405` naming the methods a player reads with, as RFC 9110 §15.5.6 requires of
/// one.
fn method_refused() -> RtResponse {
    let mut response = RtResponse::new(playback_status(405), SdkBody::empty());
    let _ = response
        .headers_mut()
        .try_insert("allow", PLAYBACK_METHODS.to_string());
    let _ = response.headers_mut().try_insert("content-length", "0");
    response
}

/// One answer of this plane: the bytes, their media type and their length — and
/// for a `HEAD`, the same answer without the body, which is what that method asks
/// for.
///
/// The headers are set through the NON-panicking setter: `Headers::insert` aborts
/// on a value it cannot represent, and nothing a request can reach may panic.
/// Every value here is this module's own, so neither can fail; a header that
/// somehow did is dropped rather than taken down the process with.
fn answer(req: &RtRequest, status: u16, media_type: &str, bytes: Vec<u8>) -> RtResponse {
    let length = bytes.len();
    let body = match req.method() {
        "HEAD" => SdkBody::empty(),
        _ => SdkBody::from(bytes),
    };
    let mut response = RtResponse::new(playback_status(status), body);
    let _ = response
        .headers_mut()
        .try_insert("content-type", media_type.to_string());
    let _ = response
        .headers_mut()
        .try_insert("content-length", length.to_string());
    response
}

/// A status this plane answers with. Every call site passes a constant, so the
/// conversion cannot fail.
fn playback_status(code: u16) -> StatusCode {
    StatusCode::try_from(code).expect("the playback plane answers constant statuses")
}
