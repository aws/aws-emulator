// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader service.
//!
//! This service is a READ plane over a fragment archive: it has no create
//! operation of its own, so everything the handlers answer is computed from
//! state somebody else wrote.
//!
//! A stream itself belongs to the Kinesis Video Streams CONTROL plane
//! (`CreateStream` creates it, `UpdateDataRetention` changes its retention,
//! `DeleteStream` removes it), so a read resolves the stream it addresses in
//! that service's registry in the shared engine — not in a second copy of it
//! held here, and not in this store, which holds no stream of its own: an
//! identifier that registry does not hold ACTIVE is a missing resource whatever
//! this archive holds for it. The FRAGMENTS of a resolved stream are this
//! service's own archive, populated through
//! [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::put_stream`] /
//! [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::append_fragment`],
//! which an ingest path (or a seed/test harness) calls on the one shared engine
//! store under the ARN the registry holds that stream with; a registered stream
//! nothing has been ingested into simply has an empty archive.
//!
//! The query helpers (stream resolution, timestamp-range filtering, pagination
//! cursors) live here beside the data they read, so every operation resolves a
//! stream and filters fragments by exactly the same rule.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state as control_plane;

/// Milliseconds since the Unix epoch for `t`, negative for an instant before it.
///
/// Every timestamp comparison in this service happens in this integer space, so
/// a range bound and a fragment timestamp are compared exactly rather than
/// through a lossy float.
#[must_use]
pub fn epoch_millis(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_millis())
            .map_or(i64::MIN, |millis| -millis),
    }
}

/// Milliseconds since the Unix epoch for a wire timestamp.
///
/// Exact: the `DateTime` seconds and sub-second nanos are combined as integers,
/// so a request's range bound lands in the same integer space the archive's own
/// timestamps are compared in.
#[must_use]
pub fn date_time_millis(value: &::aws_smithy_types::DateTime) -> i64 {
    value
        .secs()
        .saturating_mul(1000)
        .saturating_add(i64::from(value.subsec_nanos() / 1_000_000))
}

/// The instant `millis` milliseconds from the Unix epoch names, and the epoch
/// itself for an offset the platform's clock cannot represent.
///
/// TOTAL, through the checked arithmetic: `millis` spans the whole `i64` range
/// (`date_time_millis` saturates a wire timestamp into it rather than rejecting
/// one), and near either end the offset is some nine quadrillion seconds, which
/// the plain `Add`/`Sub` impls PANIC on wherever the clock cannot hold it. The
/// epoch is the answer for such a value because it names no instant a fragment
/// can be stamped at, and the only alternative is a panicking conversion of a
/// number a caller chose.
#[must_use]
pub fn from_epoch_millis(millis: i64) -> SystemTime {
    let offset = Duration::from_millis(millis.unsigned_abs());
    let shifted = if millis >= 0 {
        UNIX_EPOCH.checked_add(offset)
    } else {
        UNIX_EPOCH.checked_sub(offset)
    };
    shifted.unwrap_or(UNIX_EPOCH)
}

/// `t` as the wire timestamp a response member carries, at the millisecond
/// precision the archive records.
#[must_use]
pub fn to_date_time(t: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(epoch_millis(t))
}

/// Which of a fragment's two timestamps a selector reads.
///
/// The wire values are the `FragmentSelectorType` / `ClipFragmentSelectorType` /
/// `HLSFragmentSelectorType` enum value set (`PRODUCER_TIMESTAMP`,
/// `SERVER_TIMESTAMP`) — one Rust type because all three enums carry the same
/// two values and mean the same thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimestampOrigin {
    /// The timestamp the producer stamped on the fragment.
    Producer,
    /// The timestamp the service stamped when it received the fragment.
    Server,
}

impl TimestampOrigin {
    /// Parse a wire enum value, or `None` when it is outside the value set.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "PRODUCER_TIMESTAMP" => Some(Self::Producer),
            "SERVER_TIMESTAMP" => Some(Self::Server),
            _ => None,
        }
    }

    /// The wire enum value for this origin.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Producer => "PRODUCER_TIMESTAMP",
            Self::Server => "SERVER_TIMESTAMP",
        }
    }

    /// The timestamp of `fragment` this origin selects.
    #[must_use]
    pub fn of(self, fragment: &StoredFragment) -> SystemTime {
        match self {
            Self::Producer => fragment.producer_timestamp,
            Self::Server => fragment.server_timestamp,
        }
    }
}

/// One archived fragment: its identity, its two timestamps, its duration and
/// the media bytes the read operations concatenate.
#[derive(Debug, Clone)]
pub struct StoredFragment {
    /// The fragment number, a decimal string that increases with ingestion
    /// order (`^[0-9]+$`).
    pub fragment_number: String,
    /// The timestamp the producer stamped on the fragment.
    pub producer_timestamp: SystemTime,
    /// The timestamp the service stamped when it received the fragment.
    pub server_timestamp: SystemTime,
    /// The playback duration of the fragment.
    pub duration_millis: u64,
    /// The archived media bytes of this fragment.
    pub media: Vec<u8>,
}

impl StoredFragment {
    /// An archived fragment carrying `media`.
    #[must_use]
    pub fn new(
        fragment_number: impl Into<String>,
        producer_timestamp: SystemTime,
        server_timestamp: SystemTime,
        duration_millis: u64,
        media: Vec<u8>,
    ) -> Self {
        Self {
            fragment_number: fragment_number.into(),
            producer_timestamp,
            server_timestamp,
            duration_millis,
            media,
        }
    }

    /// The size the `Fragment$FragmentSizeInBytes` member reports.
    ///
    /// DERIVED from the stored media rather than held as a second field: a
    /// stored size could disagree with the bytes `GetMediaForFragmentList`
    /// returns for the same fragment, and then one of the two is a lie.
    #[must_use]
    pub fn size_bytes(&self) -> u64 {
        self.media.len() as u64
    }
}

/// One stream's archive.
#[derive(Debug, Clone)]
pub struct ArchivedStream {
    /// The stream name (`StreamName`).
    pub name: String,
    /// The stream ARN (`StreamARN`).
    pub arn: String,
    /// How long the stream retains data. `0` means no retention is configured,
    /// which is what makes the archived-media operations answer
    /// `NoDataRetentionException`.
    pub data_retention_in_hours: u32,
    /// The stream's `MediaType` (e.g. `video/h264`).
    pub media_type: String,
    /// The content type of the archived container, reported by
    /// `GetMediaForFragmentList$ContentType`. Kinesis Video Streams archives
    /// fragments as Matroska (MKV), served as `video/webm`.
    pub archive_content_type: String,
    /// The stream's codec private data, when the producer supplied it.
    pub codec_private_data: Option<Vec<u8>>,
    /// The archive, kept in ingestion order (ascending fragment number).
    pub fragments: Vec<StoredFragment>,
}

/// The content type Kinesis Video Streams archives fragments as: the archive is
/// Matroska, served as `video/webm`.
pub const ARCHIVE_CONTENT_TYPE: &str = "video/webm";

/// The ceiling on the bytes ONE archived-media response may assemble.
///
/// The service publishes this ceiling for the clip it builds itself ("The
/// output will contain the first 100 MB or the first 200 fragments from the
/// specified start timestamp"), so the same number bounds every payload this
/// service assembles. It is a bound on the PRODUCT, not on the request: the
/// modelled list and page limits cap how many fragments a request may name,
/// but a caller may name the largest archived fragment repeatedly, so the
/// bytes must be bounded separately from the count. The limit is published as
/// a round "100 MB" with no unit base, so one is chosen here and named.
pub const MAX_PAYLOAD_BYTES: usize = 100 * 1024 * 1024;

/// The most fragments one stream's archive HOLDS.
///
/// A ceiling on the STORE, not an AWS limit: this emulator keeps archived media
/// in the process's memory, and an ingest plane may keep writing to a stream for
/// as long as the emulator runs, so an archive that only ever grew would consume
/// the host. [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::append_fragment`]
/// is the one site that grows it and the one that enforces this.
pub const MAX_ARCHIVED_FRAGMENTS_PER_STREAM: usize = 1024;

/// The most media bytes one stream's archive HOLDS, for the same reason.
///
/// It bounds the PRODUCT rather than the count, because a count ceiling alone
/// would admit that many of the largest fragment a producer can write. The
/// figure is one second of the media rate Kinesis Video Streams states for a
/// media session ("up to 25 megabytes per second", `GetMedia`), so a stream's
/// whole retained archive still fits inside one [`MAX_PAYLOAD_BYTES`] response.
pub const MAX_ARCHIVED_MEDIA_BYTES_PER_STREAM: usize = 25_000_000;

/// The refusal every archived-media operation that models
/// `NoDataRetentionException` carries: the error is raised for "a stream that
/// does not retain data (that is, has a `DataRetentionInHours` of 0)", and the
/// condition is that retention value alone, so the message is entirely fixed
/// text naming no stream.
pub const NO_DATA_RETENTION_MESSAGE: &str = "The stream data retention is set to 0 hours.";

impl ArchivedStream {
    /// A stream with an empty archive, with the ARN shape the model's
    /// `ResourceARN` pattern requires
    /// (`arn:aws:kinesisvideo:<region>:<account>:stream/<name>/<creation epoch seconds>`).
    #[must_use]
    pub fn new(
        name: impl Into<String>,
        region: &str,
        created_at: SystemTime,
        data_retention_in_hours: u32,
    ) -> Self {
        let name = name.into();
        let created_secs = epoch_millis(created_at).max(0) / 1000;
        let arn = ::engine::emulator_arn(
            "kinesisvideo",
            region,
            &format!("stream/{name}/{created_secs}"),
        );
        Self {
            name,
            arn,
            data_retention_in_hours,
            media_type: "video/h264".to_string(),
            archive_content_type: ARCHIVE_CONTENT_TYPE.to_string(),
            codec_private_data: None,
            fragments: Vec::new(),
        }
    }

    /// Whether this stream retains data at all. A stream with no retention
    /// holds no archive, which the archived-media operations report as
    /// `NoDataRetentionException`.
    #[must_use]
    pub fn retains_data(&self) -> bool {
        self.data_retention_in_hours > 0
    }

    /// The fragment with this fragment number, if the archive holds it.
    #[must_use]
    pub fn fragment(&self, fragment_number: &str) -> Option<&StoredFragment> {
        fragment_in(&self.fragments, fragment_number)
    }

    /// The archived fragments whose `origin` timestamp falls in the inclusive
    /// range `[start_millis, end_millis]`, in ingestion order.
    #[must_use]
    pub fn fragments_in_range(
        &self,
        origin: TimestampOrigin,
        start_millis: i64,
        end_millis: i64,
    ) -> Vec<&StoredFragment> {
        fragments_in_range(&self.fragments, origin, start_millis, end_millis)
    }

    /// The total size of `fragments`, saturating rather than overflowing.
    #[must_use]
    pub fn total_bytes(fragments: &[&StoredFragment]) -> usize {
        fragments
            .iter()
            .fold(0usize, |sum, fragment| {
                sum.saturating_add(fragment.media.len())
            })
    }

    /// The media bytes of `fragments`, concatenated in the order given, or
    /// `Err(total)` when those bytes would exceed [`MAX_PAYLOAD_BYTES`].
    ///
    /// The ceiling is checked BEFORE anything is allocated, so no caller can
    /// materialise an unbounded response: a request names a bounded COUNT of
    /// fragments (the model's list and page limits), but a count says nothing
    /// about bytes, and a list may name one large fragment many times.
    pub fn concatenate(fragments: &[&StoredFragment]) -> Result<Vec<u8>, usize> {
        let total = Self::total_bytes(fragments);
        if total > MAX_PAYLOAD_BYTES {
            return Err(total);
        }
        let mut payload = Vec::with_capacity(total);
        for fragment in fragments {
            payload.extend_from_slice(&fragment.media);
        }
        Ok(payload)
    }
}

/// The fragment of `fragments` carrying this fragment number, if there is one.
///
/// The one lookup rule every operation that addresses a fragment by number
/// uses, so [`ArchivedStream`] and [`ResolvedStream`] cannot drift apart.
#[must_use]
pub fn fragment_in<'a>(
    fragments: &'a [StoredFragment],
    fragment_number: &'_ str,
) -> Option<&'a StoredFragment> {
    fragments
        .iter()
        .find(|f| f.fragment_number == fragment_number)
}

/// The fragments of `fragments` whose `origin` timestamp falls in the inclusive
/// range `[start_millis, end_millis]`, in ingestion order.
///
/// The one range rule every selector-driven operation uses, and deliberately a
/// rule about a fragment's own START: the timestamp ranges of the streaming
/// sessions and of the clip select a fragment that way — "Only fragments that
/// start exactly at or after StartTimestamp are included in the session.
/// Fragments that start before StartTimestamp and continue past it aren't
/// included in the session." An operation that asks instead which media was
/// PLAYING over a range reads [`range_holds_media`] /
/// [`fragment_covers_instant`].
#[must_use]
pub fn fragments_in_range(
    fragments: &[StoredFragment],
    origin: TimestampOrigin,
    start_millis: i64,
    end_millis: i64,
) -> Vec<&StoredFragment> {
    fragments
        .iter()
        .filter(|f| {
            let at = epoch_millis(origin.of(f));
            at >= start_millis && at <= end_millis
        })
        .collect()
}

/// `fragments` with the fragments that share one START timestamp collapsed to
/// the one carrying the LARGEST fragment number, in the order they were given.
///
/// The rule both streaming-session operations document for the manifest or
/// playlist they mint a session over: when the fragment selector reads producer
/// timestamps and several fragments carry the same start timestamp, only the
/// newest of them — the one with the largest fragment number — belongs in the
/// snapshot, and "the other fragments are not included". Fragments with
/// DIFFERENT start timestamps are all kept, even where their durations overlap.
///
/// A selector reading SERVER timestamps collapses nothing: the rule is stated
/// for `PRODUCER_TIMESTAMP` alone, and a server timestamp is the instant the
/// service received the fragment, which identifies it.
///
/// Applied BEFORE any ordering or cap, so the fragment that is kept takes its
/// place in the snapshot by its OWN timestamps, and a maximum-results ceiling
/// is spent on the fragments the snapshot carries rather than on the superseded
/// ones.
#[must_use]
pub fn collapse_shared_start_timestamps<'a>(
    fragments: Vec<&'a StoredFragment>,
    origin: TimestampOrigin,
) -> Vec<&'a StoredFragment> {
    if origin != TimestampOrigin::Producer {
        return fragments;
    }
    let mut kept: Vec<&'a StoredFragment> = Vec::with_capacity(fragments.len());
    // Where in `kept` the fragment held for one start timestamp sits, so a
    // group is found by its timestamp rather than by re-scanning what is kept.
    let mut held_at: HashMap<i64, usize> = HashMap::with_capacity(fragments.len());
    for fragment in fragments {
        let at = epoch_millis(origin.of(fragment));
        match held_at.get(&at).copied() {
            Some(index) => {
                if numeric_order(&fragment.fragment_number, &kept[index].fragment_number)
                    == std::cmp::Ordering::Greater
                {
                    kept[index] = fragment;
                }
            }
            None => {
                held_at.insert(at, kept.len());
                kept.push(fragment);
            }
        }
    }
    kept
}

/// Whether the media `fragment` archives was being recorded AT `at_millis`, read
/// on the `origin` timestamp field: its own selected timestamp starts at or
/// before that instant and its playback duration still covers it.
///
/// A fragment of no duration covers the single instant it is stamped at, so
/// every archived fragment holds media for at least its own start.
#[must_use]
pub fn fragment_covers_instant(
    fragment: &StoredFragment,
    origin: TimestampOrigin,
    at_millis: i64,
) -> bool {
    let from = epoch_millis(origin.of(fragment));
    let duration = i64::try_from(fragment.duration_millis).unwrap_or(i64::MAX);
    let until = from.saturating_add(duration);
    from <= at_millis && (at_millis < until || at_millis == from)
}

/// Whether the media `fragment` archives plays anywhere inside the inclusive
/// range `[start_millis, end_millis]`, read on `origin`.
///
/// That is the PLAYBACK relation, not the start relation
/// [`fragments_in_range`] selects on: a fragment that started before the range
/// and is still playing inside it holds media for that range exactly as one that
/// starts inside it does.
#[must_use]
pub fn fragment_covers_range(
    fragment: &StoredFragment,
    origin: TimestampOrigin,
    start_millis: i64,
    end_millis: i64,
) -> bool {
    let from = epoch_millis(origin.of(fragment));
    (from >= start_millis && from <= end_millis)
        || fragment_covers_instant(fragment, origin, start_millis)
}

/// Whether `fragments` holds any media playing inside the inclusive range
/// `[start_millis, end_millis]`, read on `origin`.
///
/// The one "this range has media" rule, so an operation that reads an image from
/// whichever fragment was recording at an instant decides absence on the same
/// relation it reads those bytes on.
#[must_use]
pub fn range_holds_media(
    fragments: &[StoredFragment],
    origin: TimestampOrigin,
    start_millis: i64,
    end_millis: i64,
) -> bool {
    fragments
        .iter()
        .any(|f| fragment_covers_range(f, origin, start_millis, end_millis))
}

/// The one stream an archived-media read addresses.
///
/// Its IDENTITY and RETENTION are the Kinesis Video Streams control plane's,
/// read through [`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::resolve_stream`]
/// on every call, so a retention change the control plane accepted is observed
/// by the very next read rather than by a copy taken earlier. Its FRAGMENTS are
/// this service's archive for THAT stream record, which is empty for a stream
/// nothing has been ingested into — including a stream created again under a
/// name whose previous incarnation left an archive behind.
#[derive(Debug)]
pub struct ResolvedStream<'a> {
    /// The stream's `StreamName`.
    pub name: String,
    /// The stream's `StreamARN`.
    pub arn: String,
    /// How long the stream retains data, in hours. `0` means no retention is
    /// configured, which is what makes the archived-media operations answer
    /// `NoDataRetentionException`.
    pub data_retention_in_hours: i64,
    /// The content type of the archived container, reported by
    /// `GetMediaForFragmentList$ContentType`.
    pub archive_content_type: String,
    /// This stream's archive, in ingestion order (ascending fragment number).
    pub fragments: &'a [StoredFragment],
}

impl ResolvedStream<'_> {
    /// Whether this stream retains data at all. A stream with no retention
    /// holds no archive, which the archived-media operations report as
    /// `NoDataRetentionException`.
    #[must_use]
    pub fn retains_data(&self) -> bool {
        self.data_retention_in_hours > 0
    }

    /// The fragment with this fragment number, if the archive holds it.
    #[must_use]
    pub fn fragment(&self, fragment_number: &str) -> Option<&StoredFragment> {
        fragment_in(self.fragments, fragment_number)
    }

    /// The archived fragments whose `origin` timestamp falls in the inclusive
    /// range `[start_millis, end_millis]`, in ingestion order.
    #[must_use]
    pub fn fragments_in_range(
        &self,
        origin: TimestampOrigin,
        start_millis: i64,
        end_millis: i64,
    ) -> Vec<&StoredFragment> {
        fragments_in_range(self.fragments, origin, start_millis, end_millis)
    }

    /// Whether this stream's archive holds media playing anywhere inside the
    /// inclusive range `[start_millis, end_millis]`, read on `origin` — the
    /// fragment that started before the range and is still playing inside it
    /// included.
    #[must_use]
    pub fn holds_media_in_range(
        &self,
        origin: TimestampOrigin,
        start_millis: i64,
        end_millis: i64,
    ) -> bool {
        range_holds_media(self.fragments, origin, start_millis, end_millis)
    }

    /// The manifest/playlist a streaming session over this stream serves from
    /// the archive AS IT STANDS: the fragment numbers `window` selects, at most
    /// `max_fragments` of them as `cap` spends that maximum, and where the
    /// first of them sits in that selection.
    ///
    /// Read at every fetch of a session whose playlist grows
    /// ([`StreamingSession::grows`]), so a fragment ingested after the session
    /// was minted is in the answer the next fetch is served, and read once for
    /// one whose window is a closed stretch of the past.
    ///
    /// The one selection rule both streaming-session operations use, so an HLS
    /// and an MPEG-DASH session over the same archive and the same parameters
    /// carry the same fragments. In EVERY playback mode the fragments that
    /// share one start timestamp are collapsed to the newest of them before the
    /// cap is applied ([`collapse_shared_start_timestamps`]), and in every mode
    /// the cap is then spent on the end of the window [`PlaylistCap`] names.
    ///
    /// SELECTION alone: whether a session over this window exists at all is
    /// [`Self::minted_session_fragment_numbers`]'s question, because the
    /// freshness a LIVE session is minted against is a condition on the request
    /// rather than on each later fetch.
    #[must_use]
    pub fn session_playlist(
        &self,
        window: SessionWindow,
        origin: TimestampOrigin,
        max_fragments: usize,
        cap: PlaylistCap,
    ) -> SessionPlaylist {
        let selected = match window {
            SessionWindow::Live => {
                // LIVE takes the whole archive as its window. The ordering is
                // by server timestamp whatever `FragmentSelectorType` the
                // request names, because that is what the selector's own
                // documentation says of LIVE: "the most recently ingested
                // fragments based on server timestamps are included" — "this
                // means that even if fragments ingested in the past have
                // producer timestamps with values now, they are not included".
                // Collapsed FIRST, so the fragment that is kept is placed by
                // its OWN server timestamp: it is the one the playlist carries,
                // and LIVE serves the most recently ingested of what it
                // carries.
                let mut newest =
                    collapse_shared_start_timestamps(self.fragments.iter().collect(), origin);
                newest.sort_by_key(|fragment| epoch_millis(fragment.server_timestamp));
                newest
            }
            // ON_DEMAND and LIVE_REPLAY take the fragments of the requested
            // range, in the order the archive holds them.
            SessionWindow::Range {
                start_millis,
                end_millis,
            } => collapse_shared_start_timestamps(
                self.fragments_in_range(origin, start_millis, end_millis),
                origin,
            ),
        };
        // The maximum, spent the way this session's playback mode fills its
        // playlist from the window: from the stream head, from the start of
        // the range as playback elapses, or from the start of the range once
        // and for all.
        let skip = cap.skipped(&selected, max_fragments);
        SessionPlaylist {
            fragment_numbers: selected
                .into_iter()
                .skip(skip)
                .take(max_fragments)
                .map(|fragment| fragment.fragment_number.clone())
                .collect(),
            // Every fragment of the window dropped in FRONT of the first one
            // listed has slid out of the playlist, so the first segment's media
            // sequence number is exactly how many of them there are.
            first_sequence: u64::try_from(skip).unwrap_or(u64::MAX),
        }
    }

    /// Whether this stream's head is fresh enough at `now` for a LIVE session
    /// over it to exist: whether the archive holds a fragment ingested within
    /// [`LIVE_FRAGMENT_WINDOW`].
    ///
    /// The condition `ResourceNotFoundException`'s own documentation puts on
    /// MINTING such a session — "if a session with a PlaybackMode of LIVE is
    /// requested for a stream that has no fragments within the last 30 seconds"
    /// — read on the same collapsed set the playlist is selected from, so both
    /// answers are about the fragments a session would carry.
    #[must_use]
    pub fn holds_live_fragments(&self, origin: TimestampOrigin, now: SystemTime) -> bool {
        let window_start = epoch_millis(now - LIVE_FRAGMENT_WINDOW);
        collapse_shared_start_timestamps(self.fragments.iter().collect(), origin)
            .iter()
            .any(|fragment| epoch_millis(fragment.server_timestamp) >= window_start)
    }

    /// The fragments a streaming session minted at `now` serves, or EMPTY when
    /// such a session would serve nothing — which both streaming-session
    /// operations report as `ResourceNotFoundException` with
    /// [`NO_FRAGMENTS_MESSAGE`].
    ///
    /// The one existence rule both of them use: no fragment in the requested
    /// range for a session over a range, and no fragment in the last 30 seconds
    /// for a LIVE one.
    #[must_use]
    pub fn minted_session_fragment_numbers(
        &self,
        window: SessionWindow,
        origin: TimestampOrigin,
        max_fragments: usize,
        cap: PlaylistCap,
        now: SystemTime,
    ) -> Vec<String> {
        if matches!(window, SessionWindow::Live) && !self.holds_live_fragments(origin, now) {
            return Vec::new();
        }
        self.session_playlist(window, origin, max_fragments, cap)
            .fragment_numbers
    }
}

/// `Expires` when a streaming-session request names none: "The default is 300
/// (5 minutes)."
pub const DEFAULT_SESSION_EXPIRES_SECS: u32 = 300;

/// How many fragments a session's manifest or playlist carries when the request
/// names no maximum and the playback mode is LIVE or LIVE_REPLAY: "The default
/// is 5 fragments if PlaybackMode is LIVE or LIVE_REPLAY".
pub const DEFAULT_LIVE_SESSION_FRAGMENTS: usize = 5;

/// How many fragments a session's manifest or playlist carries when the request
/// names no maximum and the playback mode is ON_DEMAND: "and 1,000 if
/// PlaybackMode is ON_DEMAND".
pub const DEFAULT_ON_DEMAND_SESSION_FRAGMENTS: usize = 1000;

/// How recent a fragment must be for a LIVE session to exist: the
/// `ResourceNotFoundException` documentation states the error is thrown "if a
/// session with a PlaybackMode of LIVE is requested for a stream that has no
/// fragments within the last 30 seconds".
pub const LIVE_FRAGMENT_WINDOW: Duration = Duration::from_secs(30);

/// The refusal a streaming-session request carries when the stream holds no
/// fragment the session could serve — none in the requested range for
/// `ON_DEMAND`/`LIVE_REPLAY`, none in the last 30 seconds for `LIVE`, the two
/// cases `ResourceNotFoundException`'s own documentation gives these
/// operations. Entirely fixed text: it names neither the stream nor the window.
pub const NO_FRAGMENTS_MESSAGE: &str =
    "No fragments found in the stream for the streaming request.";

/// The host a session URL names when the request carries neither a `Host`
/// header nor an authority to read the emulator's own endpoint from. A
/// loopback literal, never a real AWS host: the session this URL names is
/// served by this emulator.
pub const LOOPBACK_HOST: &str = "127.0.0.1";

/// The path segment that follows a playback protocol's own prefix in every
/// session resource's path.
pub const SESSION_PATH_VERSION: &str = "v1";

/// The query field a session resource is addressed with: the session token
/// `GetHLSStreamingSessionURL` / `GetDASHStreamingSessionURL` put in the URL
/// they handed out ("an authenticated URL (that includes an encrypted session
/// token)").
pub const SESSION_TOKEN_FIELD: &str = "SessionToken";

/// The query field naming WHICH track of the session a playlist or
/// initialization fragment is asked for: "There is a distinct HLS media
/// playlist for the video track and the audio track (if applicable)".
pub const TRACK_NUMBER_FIELD: &str = "TrackNumber";

/// The query field naming WHICH archived fragment a media fragment is asked
/// for, by the fragment number the archive holds it under.
pub const FRAGMENT_NUMBER_FIELD: &str = "FragmentNumber";

/// The HLS master playlist: the "root resource needed for streaming with HLS",
/// which `GetHLSStreamingSessionURL` reports the URL of.
pub const HLS_MASTER_PLAYLIST_RESOURCE: &str = "getHLSMasterPlaylist.m3u8";

/// The HLS media playlist of one track, which the master playlist names: it
/// "contains a URL to access the MP4 initialization fragment ... and URLs to
/// access the MP4 media fragments".
pub const HLS_MEDIA_PLAYLIST_RESOURCE: &str = "getHLSMediaPlaylist.m3u8";

/// The MP4 initialization fragment, which "contains only the codec private data
/// for the stream and respective track, which the media player needs to decode
/// the media frames".
pub const MP4_INIT_FRAGMENT_RESOURCE: &str = "getMP4InitFragment.mp4";

/// One MP4 media fragment: the "encoded fragment's media frames and their
/// timestamps".
pub const MP4_MEDIA_FRAGMENT_RESOURCE: &str = "getMP4MediaFragment.mp4";

/// One MPEG TS fragment, "containing both initialization and media data for all
/// tracks in the stream" — the resource a session whose `ContainerFormat` is
/// `MPEG_TS` serves media as, "instead of GetMP4InitFragment and
/// GetMP4MediaFragment".
pub const TS_FRAGMENT_RESOURCE: &str = "getTSFragment.ts";

/// The MPEG-DASH manifest: the root resource of an MPEG-DASH session, which
/// `GetDASHStreamingSessionURL` reports the URL of.
pub const DASH_MANIFEST_RESOURCE: &str = "getMPDManifest.mpd";

/// The path, on this emulator, one resource of a streaming session is served
/// at: `/<protocol>/v1/<resource>?SessionToken=<token>`.
///
/// The ONE place that layout is written, so the URL an operation reports and
/// the plane that serves it ([`crate::service::playback_session`]) address the
/// same resources — and so a playlist can name a sibling resource by a path
/// relative to its own.
#[must_use]
pub fn session_resource_path(protocol: SessionProtocol, resource: &str, token: &str) -> String {
    format!(
        "/{}/{SESSION_PATH_VERSION}/{resource}?{SESSION_TOKEN_FIELD}={token}",
        protocol.url_prefix()
    )
}

/// The URL a minted streaming session is reported in: the manifest or playlist
/// resource of `protocol`, carrying `token`, on the endpoint the CALLER reached
/// this emulator on.
///
/// The host is read from the request itself — its `Host` header, else the
/// authority of the URI it addressed, else [`LOOPBACK_HOST`] — exactly as the
/// real service answers on its own data endpoint, and deliberately names no
/// real AWS host: this emulator serves the playback session
/// ([`crate::service::playback_session`]), so a player or test that follows the
/// URL comes back here and is answered by that playlist. The one rule both
/// streaming-session operations use, so they cannot drift on which endpoint a
/// session URL names.
#[must_use]
pub fn session_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    protocol: SessionProtocol,
    token: &str,
) -> String {
    let host = ctx
        .parts()
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| ctx.parts().uri.host())
        .unwrap_or(LOOPBACK_HOST);
    format!(
        "https://{host}{}",
        session_resource_path(protocol, protocol.manifest_resource(), token)
    )
}

/// Which fragments of a stream a streaming session serves, as the request's
/// `PlaybackMode` and fragment selector describe them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionWindow {
    /// `LIVE`: the most recently INGESTED fragments, and only while the stream
    /// head is fresh — a LIVE session over a stream with no fragment in the
    /// last [`LIVE_FRAGMENT_WINDOW`] serves nothing.
    Live,
    /// `ON_DEMAND` / `LIVE_REPLAY`: the fragments whose selected timestamp
    /// falls in the inclusive requested range, of which the mode's
    /// [`PlaylistCap`] keeps at most its maximum. An unset bound leaves that
    /// end of the range open.
    Range {
        /// The inclusive start of the requested range, in epoch milliseconds.
        start_millis: i64,
        /// The inclusive end of the requested range, in epoch milliseconds.
        end_millis: i64,
    },
}

/// What a streaming session's playlist or manifest lists at one instant.
///
/// A session whose playlist GROWS ([`StreamingSession::grows`]) is answered from
/// a fresh one of these on every fetch, so the list a player re-reads is the
/// archive as it stands then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlaylist {
    /// The fragment numbers the playlist lists, in playback order.
    pub fragment_numbers: Vec<String>,
    /// The media sequence number of the first of them: "the Media Sequence
    /// Number of the first Media Segment that appears in a Playlist file"
    /// (RFC 8216 §4.3.3.2). It rises as fragments slide out of the front of a
    /// growing playlist, because "the Media Sequence Number MUST be incremented
    /// by 1 for each Media Segment that is removed" (§6.2.2), so a player that
    /// re-reads such a playlist is never handed one number for two fragments.
    pub first_sequence: u64,
}

/// HOW a session's playlist spends its fragment maximum: which fragments of its
/// window the maximum is filled from, and how that selection moves while the
/// session lives.
///
/// The documented difference between the three playback modes, in the one place
/// the selection reads it. The maximum-fragments member of either
/// streaming-session input says "when the PlaybackMode is LIVE, the most recent
/// fragments are returned up to this value. When the PlaybackMode is ON_DEMAND,
/// the oldest fragments are returned, up to this maximum number", and
/// `LIVE_REPLAY` is the mode between the two: its playlist "is updated
/// similarly to how it is updated for LIVE mode except that it starts by
/// including fragments from a given start time. Instead of fragments being
/// added as they are ingested, fragments are added as the duration of the next
/// fragment elapses."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaylistCap {
    /// `LIVE`: the maximum is filled from the HEAD of the stream, so the
    /// playlist carries the most recently ingested fragments and everything
    /// older has slid out in front of them.
    Head,
    /// `LIVE_REPLAY`: the maximum is filled from the START of the requested
    /// range and its front advances with the playback that has ELAPSED since
    /// the session was minted — one fragment leaves the playlist for each
    /// fragment duration gone by, so a fragment ingested later enters it once
    /// the fragments in front of it have played.
    Replay {
        /// How long ago the session was minted, in milliseconds: the playback
        /// time that has elapsed since its range began.
        elapsed_millis: i64,
    },
    /// `ON_DEMAND`: the maximum is filled from the start of the requested range
    /// and never moves, because the playlist of this mode is complete when it
    /// is minted.
    Fixed,
}

impl PlaylistCap {
    /// How `playback_mode`'s playlist spends its maximum `elapsed` after the
    /// session was minted.
    ///
    /// `ON_DEMAND` is the one mode whose playlist is complete at the mint — it
    /// "is typically static for sessions with a PlaybackType of ON_DEMAND" —
    /// `LIVE` is the one that plays the stream head, "continually updated with
    /// the latest fragments as they become available", and `LIVE_REPLAY` is the
    /// one that replays the archive from a given start time at the pace the
    /// fragments themselves play at.
    #[must_use]
    pub fn of_playback_mode(playback_mode: &str, elapsed: Duration) -> Self {
        match playback_mode {
            ON_DEMAND_PLAYBACK_MODE => Self::Fixed,
            LIVE_REPLAY_PLAYBACK_MODE => Self::Replay {
                elapsed_millis: i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX),
            },
            // [`LIVE_PLAYBACK_MODE`], and with it the mode a request that names
            // none asks for: "The default is LIVE".
            _ => Self::Head,
        }
    }

    /// How `playback_mode`'s playlist spends its maximum at the instant the
    /// session is MINTED, where no playback has elapsed yet: a `LIVE_REPLAY`
    /// playlist therefore "starts by including fragments from a given start
    /// time".
    #[must_use]
    pub fn at_mint(playback_mode: &str) -> Self {
        Self::of_playback_mode(playback_mode, Duration::ZERO)
    }

    /// Whether the playlist this maximum belongs to GROWS: whether re-reading
    /// it can show a fragment it did not list before.
    ///
    /// True of the two selections that MOVE, false of the fixed one, because
    /// that is the same fact about the playback mode: a complete playlist is
    /// one whose fragments were chosen once.
    #[must_use]
    pub fn grows(self) -> bool {
        !matches!(self, Self::Fixed)
    }

    /// How many of `selected` sit in FRONT of the fragments a playlist of at
    /// most `max_fragments` carries — the ones that have left it, which is also
    /// the media sequence number of its first segment.
    ///
    /// No selection advances past the end of what the archive holds: there is
    /// no later fragment to fill the playlist with, so its front waits there
    /// until one is ingested, which is what keeps a session's playlist the size
    /// its maximum asks for.
    #[must_use]
    pub fn skipped(self, selected: &[&StoredFragment], max_fragments: usize) -> usize {
        let furthest = selected.len().saturating_sub(max_fragments);
        match self {
            Self::Head => furthest,
            Self::Fixed => 0,
            Self::Replay { elapsed_millis } => played_out(selected, elapsed_millis).min(furthest),
        }
    }
}

/// How many of `fragments`, counted from the front, have finished playing
/// `elapsed_millis` after playback started at the first of them.
///
/// The pace `LIVE_REPLAY` adds fragments at: "fragments are added as the
/// duration of the next fragment elapses. For example, if the fragments in the
/// session are two seconds long, then a new fragment is added to the media
/// playlist every two seconds" — so one leaves the front of the playlist for
/// each fragment duration that has gone by, at the durations the archive itself
/// records.
#[must_use]
fn played_out(fragments: &[&StoredFragment], elapsed_millis: i64) -> usize {
    let mut played = 0i64;
    let mut count = 0usize;
    for fragment in fragments {
        played =
            played.saturating_add(i64::try_from(fragment.duration_millis).unwrap_or(i64::MAX));
        if played > elapsed_millis {
            break;
        }
        count += 1;
    }
    count
}

/// `PlaybackMode` LIVE: the mode that plays the stream head, the only one that
/// carries no timestamp range, and the value a request naming no mode means
/// ("The default is LIVE").
pub const LIVE_PLAYBACK_MODE: &str = "LIVE";

/// `PlaybackMode` ON_DEMAND: the mode that plays a closed range of the archive,
/// so it is the one whose range must carry both bounds.
pub const ON_DEMAND_PLAYBACK_MODE: &str = "ON_DEMAND";

/// `PlaybackMode` LIVE_REPLAY: the mode that REPLAYS the archive from a given
/// start time, "useful to be able to start playback from when an event is
/// detected", and the one whose range may be left open at its end.
pub const LIVE_REPLAY_PLAYBACK_MODE: &str = "LIVE_REPLAY";

/// The bounds one streaming-session request's fragment selector asks for, in
/// epoch milliseconds, with each bound present only when the request carried it.
///
/// The two protocols model that range as two shapes with the same two members
/// (`HLSTimestampRange` and `DASHTimestampRange`), so each handler reads its own
/// shape and hands the bounds to [`session_parameters`] in this one form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SelectedTimestampRange {
    /// `TimestampRange$StartTimestamp`, when the request carried one.
    pub start_millis: Option<i64>,
    /// `TimestampRange$EndTimestamp`, when the request carried one.
    pub end_millis: Option<i64>,
}

/// One streaming-session request's mode, session parameters and selected range,
/// as either streaming-session operation presents them to
/// [`session_parameters`].
#[derive(Debug, Clone, Copy)]
pub struct SessionRequest<'a> {
    /// The input member this protocol carries its fragment selector in
    /// (`HLSFragmentSelector` / `DASHFragmentSelector`) — the member every
    /// refusal below names, and the only thing that differs between the two
    /// operations' refusal sentences.
    pub selector_member: &'a str,
    /// `PlaybackMode` as the request carried it, or `None` for the default.
    pub playback_mode: Option<&'a str>,
    /// The values this protocol's `PlaybackMode` enum admits.
    pub playback_mode_values: &'a [&'a str],
    /// `Expires`, in seconds, as the request carried it.
    pub expires: Option<i32>,
    /// The protocol's own maximum-fragment member
    /// (`MaxMediaPlaylistFragmentResults` / `MaxManifestFragmentResults`).
    pub max_fragment_results: Option<i64>,
    /// The selector's `TimestampRange`, when the selector carried one.
    pub range: Option<SelectedTimestampRange>,
    /// Which of a fragment's two timestamps the selector reads
    /// (`FragmentSelectorType`), defaulted by the caller: the end of a range
    /// read by server timestamp is the one the clock bounds.
    pub origin: TimestampOrigin,
    /// The instant the request is decided as of, which the server-timestamp end
    /// is compared against.
    pub now: SystemTime,
}

/// What a streaming-session request resolves to: its parameters with the
/// documented defaults applied, and the window of the archive the session
/// serves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionParameters {
    /// `PlaybackMode`, defaulted to [`LIVE_PLAYBACK_MODE`].
    pub playback_mode: String,
    /// `Expires`, defaulted to [`DEFAULT_SESSION_EXPIRES_SECS`].
    pub expires_in_secs: u32,
    /// How many fragments the manifest or playlist carries, defaulted per mode
    /// ([`DEFAULT_LIVE_SESSION_FRAGMENTS`] /
    /// [`DEFAULT_ON_DEMAND_SESSION_FRAGMENTS`]).
    pub max_fragments: usize,
    /// Which fragments of the archive the session serves.
    pub window: SessionWindow,
}

/// The documented session-parameter defaults and the playback mode's selector
/// requirements, applied to one streaming-session request.
///
/// The one rule both streaming-session operations use, so they cannot drift on
/// what an unstated parameter means or on which selector a mode admits: the
/// `PlaybackMode` default, the `Expires` and maximum-fragment defaults, the
/// `LIVE` session that must carry no `TimestampRange`, the range a non-`LIVE`
/// session must carry, the `StartTimestamp` every such range must carry, the
/// `EndTimestamp` `ON_DEMAND` must carry, the end that must be later than the
/// start and within [`MAX_RANGE_SPAN_MILLIS`] of it, the end a selector reading
/// server timestamps may not place ahead of the clock, and the window built from
/// those bounds. The two operations differ only
/// in [`SessionRequest::selector_member`], which every refusal that names the
/// selector is worded with, and in the per-protocol members each input
/// additionally carries, which stay in their own handlers.
///
/// `Err` carries the refusal's sentence; each caller reports it as its own
/// operation's `InvalidArgumentException`. The `PlaybackMode` membership check is
/// belt-and-braces rather than the authoritative enum check: the framework's walk
/// already refuses a value outside a modelled enum before a handler runs.
pub fn session_parameters(request: SessionRequest<'_>) -> Result<SessionParameters, String> {
    let member = request.selector_member;
    let playback_mode = request
        .playback_mode
        .unwrap_or(LIVE_PLAYBACK_MODE)
        .to_string();
    if !request.playback_mode_values.contains(&playback_mode.as_str()) {
        return Err(format!(
            "PlaybackMode must be one of [{}]",
            request.playback_mode_values.join(", ")
        ));
    }
    let expires_in_secs = request
        .expires
        .and_then(|secs| u32::try_from(secs).ok())
        .unwrap_or(DEFAULT_SESSION_EXPIRES_SECS);
    let max_fragments = request
        .max_fragment_results
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(if playback_mode == ON_DEMAND_PLAYBACK_MODE {
            DEFAULT_ON_DEMAND_SESSION_FRAGMENTS
        } else {
            DEFAULT_LIVE_SESSION_FRAGMENTS
        });

    // The range the selector asks for, against the playback mode's documented
    // requirements.
    let live = playback_mode == LIVE_PLAYBACK_MODE;
    let window = if live {
        if request.range.is_some() {
            return Err(format!(
                "{member}.TimestampRange must not be set when PlaybackMode is LIVE"
            ));
        }
        SessionWindow::Live
    } else {
        let Some(range) = request.range else {
            return Err(format!(
                "{member} with a TimestampRange is required when PlaybackMode is {playback_mode}"
            ));
        };
        let Some(start_millis) = range.start_millis else {
            return Err(format!(
                "{member}.TimestampRange.StartTimestamp is required"
            ));
        };
        if playback_mode == ON_DEMAND_PLAYBACK_MODE && range.end_millis.is_none() {
            return Err(format!(
                "{member}.TimestampRange.EndTimestamp is required when PlaybackMode is ON_DEMAND"
            ));
        }
        // STRICTER than the `TimestampRange` of `ListFragments`, which accepts a
        // single-instant range and bounds no span: both streaming ranges' own
        // documentation says the end "must be within 24 hours of the specified
        // StartTimestamp, and it must be later than the StartTimestamp value",
        // wording that range does not carry. The divergence is the model's, not
        // an inconsistency here.
        if let Some(end_millis) = range.end_millis {
            if end_millis <= start_millis {
                return Err(TIMESTAMP_RANGE_ORDER_MESSAGE.to_string());
            }
            // The other half of that same sentence: the end "must be within 24
            // hours of the specified StartTimestamp", so no session is minted
            // over a wider span.
            if !range_within_max_span(start_millis, end_millis) {
                return Err(TIMESTAMP_RANGE_SPAN_MESSAGE.to_string());
            }
            // The clause the same documentation adds for a selector reading
            // server timestamps: such an end "must be in the past", so no
            // session is minted over a window the service cannot yet have
            // stamped a fragment in.
            if !range_end_requestable(request.origin, end_millis, epoch_millis(request.now)) {
                return Err(SERVER_RANGE_END_IN_PAST_MESSAGE.to_string());
            }
        }
        SessionWindow::Range {
            start_millis,
            // An unset bound leaves that end of the range open, which is what a
            // LIVE_REPLAY session with no `EndTimestamp` asks for.
            end_millis: range.end_millis.unwrap_or(i64::MAX),
        }
    };

    Ok(SessionParameters {
        playback_mode,
        expires_in_secs,
        max_fragments,
        window,
    })
}

/// Which playback protocol a streaming session serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionProtocol {
    /// HTTP Live Streaming, from `GetHLSStreamingSessionURL`.
    Hls,
    /// MPEG-DASH, from `GetDASHStreamingSessionURL`.
    Dash,
}

impl SessionProtocol {
    /// The path segment the session's playlist URL uses.
    #[must_use]
    pub fn url_prefix(self) -> &'static str {
        match self {
            Self::Hls => "hls",
            Self::Dash => "dash",
        }
    }

    /// The protocol a playback path's FIRST segment names, or `None` when the
    /// segment names neither playback plane.
    ///
    /// The inverse of [`Self::url_prefix`], written against it rather than
    /// against a second copy of the two segments.
    #[must_use]
    pub fn of_url_prefix(prefix: &str) -> Option<Self> {
        [Self::Hls, Self::Dash]
            .into_iter()
            .find(|protocol| protocol.url_prefix() == prefix)
    }

    /// The resource a media player fetches first for this protocol: the HLS
    /// master playlist (`GetHLSMasterPlaylist`) or the MPEG-DASH manifest
    /// (`GetDASHManifest`), the "root resource needed for streaming" each
    /// operation's own documentation names.
    #[must_use]
    pub fn manifest_resource(self) -> &'static str {
        match self {
            Self::Hls => HLS_MASTER_PLAYLIST_RESOURCE,
            Self::Dash => DASH_MANIFEST_RESOURCE,
        }
    }
}

/// A recorded streaming session: what a session URL resolves back to.
#[derive(Debug, Clone)]
pub struct StreamingSession {
    /// The opaque session token carried in the session URL.
    pub token: String,
    /// The name of the stream the session plays.
    pub stream_name: String,
    /// The protocol the session serves.
    pub protocol: SessionProtocol,
    /// The session's `PlaybackMode`.
    pub playback_mode: String,
    /// Which of a fragment's two timestamps the session's fragment selector
    /// reads, which is also the timestamp a playlist or manifest REPORTS for a
    /// fragment: "When HLSFragmentSelector is SERVER_TIMESTAMP, the timestamps
    /// will be the server start timestamps. Similarly, when HLSFragmentSelector
    /// is PRODUCER_TIMESTAMP, the timestamps will be the producer start
    /// timestamps."
    pub origin: TimestampOrigin,
    /// The session's `ContainerFormat`. An MPEG-DASH session carries no such
    /// parameter: its manifest serves the MP4 initialization and media
    /// fragments `GetMP4InitFragment`/`GetMP4MediaFragment` answer, so it is
    /// recorded as the fragmented-MP4 container.
    pub container_format: String,
    /// The session's `DiscontinuityMode`, which only an HLS session carries:
    /// `None` for an MPEG-DASH session, whose input models no such parameter.
    pub discontinuity_mode: Option<String>,
    /// The session's `DisplayFragmentTimestamp`.
    pub display_fragment_timestamp: String,
    /// The session's `DisplayFragmentNumber`, which only an MPEG-DASH session
    /// carries: `None` for an HLS session, whose input models no such
    /// parameter.
    pub display_fragment_number: Option<String>,
    /// The playlist snapshot taken when the session was minted: the fragment
    /// numbers it selected then, capped by the request's max-results. What a
    /// session whose playlist does NOT grow serves for its whole life
    /// ([`Self::grows`]).
    pub fragment_numbers: Vec<String>,
    /// Which fragments of the archive this session's request asked for, which
    /// is the window a growing session re-selects over on every fetch.
    pub window: SessionWindow,
    /// How many fragments its playlist or manifest carries at most: the
    /// request's `MaxMediaPlaylistFragmentResults` /
    /// `MaxManifestFragmentResults`, or the mode's documented default.
    pub max_fragments: usize,
    /// When the session was created.
    pub created_at: SystemTime,
    /// How long after `created_at` the session URL stays valid.
    pub expires_in_secs: u32,
}

impl StreamingSession {
    /// How this session's playlist spends its fragment maximum at `now`: the
    /// rule its playback mode fills that maximum by, with the playback that
    /// has elapsed since the session was minted
    /// ([`PlaylistCap::of_playback_mode`]).
    #[must_use]
    pub fn cap(&self, now: SystemTime) -> PlaylistCap {
        PlaylistCap::of_playback_mode(
            &self.playback_mode,
            now.duration_since(self.created_at).unwrap_or(Duration::ZERO),
        )
    }

    /// Whether this session's playlist GROWS: whether the fragments it serves
    /// are re-selected from the archive on every fetch, so fragments ingested
    /// after it was minted appear in it.
    ///
    /// True for the two LIVE modes, whose playlist the model defines as a
    /// growing one: of `LIVE`, "the HLS media playlist is continually updated
    /// with the latest fragments as they become available. We recommend that
    /// the media player retrieve a new playlist on a one-second interval"; and
    /// of a `LIVE_REPLAY` range, "if the EndTimestamp is not set for
    /// LIVE_REPLAY mode then the session will continue to include newly
    /// ingested fragments until the session expires" — a range that carries an
    /// end still grows, up to that end, which is what its own window bounds.
    /// False for `ON_DEMAND`, which plays a closed stretch of the past: "the
    /// HLS media playlist is typically static for sessions with a PlaybackType
    /// of ON_DEMAND".
    ///
    /// The ONE predicate for that difference, read off the same rule the
    /// SELECTION spends its maximum by ([`Self::cap`]), so what a playlist or
    /// manifest ADVERTISES — an `EXT-X-ENDLIST`-closed VOD playlist, a `static`
    /// presentation — cannot drift from whether re-reading it can show anything
    /// new. WHICH rule a mode follows does not depend on the instant it is
    /// asked at; only how far that rule has advanced does.
    #[must_use]
    pub fn grows(&self) -> bool {
        PlaylistCap::at_mint(&self.playback_mode).grows()
    }

    /// What this session's playlist or manifest lists over `stream` at `now`.
    ///
    /// A growing session re-selects from the archive by its own recorded
    /// selection — its window, its timestamp origin, its maximum fragment count
    /// and the rule that maximum is filled by — so a fragment ingested since
    /// the mint is listed once that rule reaches it, and one the selection has
    /// moved past is not. A session that does not grow lists the snapshot it
    /// was minted with.
    #[must_use]
    pub fn playlist(&self, stream: &ResolvedStream<'_>, now: SystemTime) -> SessionPlaylist {
        if self.grows() {
            return stream.session_playlist(
                self.window,
                self.origin,
                self.max_fragments,
                self.cap(now),
            );
        }
        SessionPlaylist {
            fragment_numbers: self.fragment_numbers.clone(),
            first_sequence: 0,
        }
    }

    /// When this session's URL stops being valid.
    #[must_use]
    pub fn expires_at(&self) -> SystemTime {
        self.created_at + Duration::from_secs(u64::from(self.expires_in_secs))
    }

    /// Whether the session is still one its playback resources may be served
    /// for at `now`: "When a session expires, no new calls to
    /// GetHLSMasterPlaylist, GetHLSMediaPlaylist, GetMP4InitFragment,
    /// GetMP4MediaFragment, or GetTSFragment can be made for that session."
    #[must_use]
    pub fn is_live_at(&self, now: SystemTime) -> bool {
        self.expires_at() > now
    }
}

/// The refusal an identifier no stream answers to carries, reported as
/// `ResourceNotFoundException` (404) — the error raised when Kinesis Video
/// Streams "can't find the stream that you specified". Entirely fixed text: it
/// names neither the stream nor which identifier form was used.
///
/// The control plane words its own 404 with the same sentence, and this copy is
/// DELIBERATE rather than an oversight: each service states the refusals it
/// answers with, and a reader of this crate must be able to see what it sends
/// without following a dependency.
pub const STREAM_NOT_FOUND_MESSAGE: &str = "The requested stream is not found or not active.";

/// The refusal a request that does not name exactly one stream carries.
///
/// `StreamName` and `StreamARN` are each documented as "You must specify either
/// the StreamName or the StreamARN", so one identifier is required and two are
/// a contradiction; the message states that whole rule, which is why both cases
/// carry it.
pub const ONE_STREAM_IDENTIFIER_MESSAGE: &str =
    "One of StreamName or StreamARN must be provided, but not both.";

/// The emulator endpoint this request reached, when it reached one this emulator serves
/// rather than a host that names a service.
///
/// This is what scopes a read to the stream the address it arrived on was announced for
/// ([`ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState::resolve_stream`]): a
/// `GetDataEndpoint` call names the stream it wants an endpoint for, so the archived-media
/// address it hands back is that stream's.
pub fn addressed_endpoint(ctx: &::smithy_server_runtime::OperationContext<'_>) -> Option<String> {
    ::service_common::data_plane_addressing::reached_emulator_endpoint_of_parts(ctx.parts())
}

/// The refusal a timestamp range whose end does not come after its start
/// carries: `EndTimestamp` "must be later than the StartTimestamp value".
pub const TIMESTAMP_RANGE_ORDER_MESSAGE: &str = "StartTimestamp must be before EndTimestamp.";

/// How many milliseconds a range whose `EndTimestamp` "must be within 24 hours
/// of the specified StartTimestamp" may span.
///
/// Every range of this service whose end carries that sentence shares this one
/// bound: `HLSTimestampRange`, `DASHTimestampRange` and `ClipTimestampRange`.
/// The ranges that do NOT carry it keep their own documented bound instead —
/// `ListFragments`'s `TimestampRange`, whose end is documented as nothing more
/// than "the ending timestamp in the range of timestamps for which to return
/// fragments", and `GetImages`, whose documented window is the narrower one its
/// own handler states — so neither is checked against this one.
pub const MAX_RANGE_SPAN_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// The refusal a range wider than [`MAX_RANGE_SPAN_MILLIS`] carries, worded with
/// the documented bound it exceeds.
pub const TIMESTAMP_RANGE_SPAN_MESSAGE: &str =
    "EndTimestamp must be within 24 hours of StartTimestamp.";

/// Whether the span between the two bounds of a range, in epoch milliseconds, is
/// one the documented 24-hour window admits.
///
/// The bound is INCLUSIVE — the end must be "within 24 hours" of the start — so a
/// range exactly that wide is admitted and only a wider one is refused.
#[must_use]
pub fn range_within_max_span(start_millis: i64, end_millis: i64) -> bool {
    end_millis.saturating_sub(start_millis) <= MAX_RANGE_SPAN_MILLIS
}

/// The refusal a range read by SERVER timestamp carries when its end lies ahead
/// of the clock, worded with the clause it breaks: such an `EndTimestamp` "must
/// be in the past".
pub const SERVER_RANGE_END_IN_PAST_MESSAGE: &str =
    "EndTimestamp must be in the past when FragmentSelectorType is SERVER_TIMESTAMP.";

/// Whether the end of a range, in epoch milliseconds, is one a selector reading
/// `origin` may ask for as of `now_millis`.
///
/// Every range of this service whose end carries the 24-hour sentence carries
/// one more clause with it — "If FragmentSelectorType for the request is
/// SERVER_TIMESTAMP, this value must be in the past" — so an end AHEAD of the
/// clock is requestable only by producer timestamp, which the producer stamps
/// and the service therefore cannot bound. An end AT the clock is not ahead of
/// it, so only a later one is refused.
#[must_use]
pub fn range_end_requestable(origin: TimestampOrigin, end_millis: i64, now_millis: i64) -> bool {
    match origin {
        TimestampOrigin::Producer => true,
        TimestampOrigin::Server => end_millis <= now_millis,
    }
}

/// Why a `StreamName`/`StreamARN` pair did not name one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamLookupError {
    /// The request did not name exactly one stream: a client error the
    /// archived-media operations report as `InvalidArgumentException` (400),
    /// stated with [`ONE_STREAM_IDENTIFIER_MESSAGE`].
    InvalidArgument(String),
    /// Exactly one of the two was given, and no stream answers to it: reported
    /// as `ResourceNotFoundException` (404), stated with
    /// [`STREAM_NOT_FOUND_MESSAGE`].
    NotFound(String),
}

/// A pagination cursor over one stream's archive, carried in `NextToken`.
///
/// Self-describing, so a replayed token continues where the previous page
/// stopped without the service holding per-token server state: it carries
/// EVERY request member that determines the result set its offset indexes into
/// — the stream it was issued for, the selector the first page used (the model
/// makes `FragmentSelector` optional precisely because a continuation re-uses
/// the selector the token remembers), and, for a page of sampled instants, the
/// sampling interval those instants were computed at — plus the offset the next
/// page starts at. A token "specifies where to start paginating the next set"
/// of ONE query's results, so a continuation is served against the set the
/// token names, never against a differently shaped one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FragmentCursor {
    /// The name of the stream the token was issued for.
    pub stream_name: String,
    /// The timestamp field the first page selected on.
    pub origin: TimestampOrigin,
    /// The inclusive start of the selected range.
    pub start_millis: i64,
    /// The inclusive end of the selected range.
    pub end_millis: i64,
    /// How many results earlier pages already returned: matching fragments for a
    /// fragment listing, sampled instants for an image page.
    pub offset: usize,
    /// The sampling interval, in milliseconds, the instants this cursor's offset
    /// indexes into were computed at, when the paged result set is a sampled one.
    /// [`None`] for a fragment listing, whose results the archive itself
    /// enumerates, and for an image page whose query named no interval.
    pub interval_millis: Option<i64>,
}

impl FragmentCursor {
    /// Encode this cursor as a `NextToken`.
    ///
    /// Base64 of the cursor's JSON, which satisfies the model's
    /// `^[a-zA-Z0-9+/]+={0,2}$` token pattern (the standard base64 alphabet
    /// plus at most two `=` of padding).
    #[must_use]
    pub fn encode(&self) -> String {
        let mut json = ::serde_json::json!({
            "s": self.stream_name,
            "o": self.origin.as_str(),
            "f": self.start_millis,
            "t": self.end_millis,
            "i": self.offset,
        });
        // Only a sampled result set has an interval, so a cursor without one
        // encodes exactly as it always did.
        if let (Some(object), Some(interval)) = (json.as_object_mut(), self.interval_millis) {
            object.insert("v".to_string(), ::serde_json::json!(interval));
        }
        ::aws_smithy_types::base64::encode(json.to_string())
    }

    /// Decode a `NextToken` produced by [`Self::encode`], or `None` when it is
    /// not one.
    #[must_use]
    pub fn decode(token: &str) -> Option<Self> {
        let raw = ::aws_smithy_types::base64::decode(token).ok()?;
        let json: ::serde_json::Value = ::serde_json::from_slice(&raw).ok()?;
        Some(Self {
            stream_name: json.get("s")?.as_str()?.to_string(),
            origin: TimestampOrigin::parse(json.get("o")?.as_str()?)?,
            start_millis: json.get("f")?.as_i64()?,
            end_millis: json.get("t")?.as_i64()?,
            offset: usize::try_from(json.get("i")?.as_u64()?).ok()?,
            interval_millis: match json.get("v") {
                None => None,
                // A token carrying the member but no number for it is not one
                // this service minted.
                Some(value) => Some(value.as_i64()?),
            },
        })
    }
}

/// The most streaming sessions this service keeps at once.
///
/// A session is minted per request, so the store that holds them needs a
/// ceiling of its own: expiry alone bounds nothing inside one `Expires` window
/// (up to 12 hours). The number is this emulator's own choice — AWS publishes
/// no per-account session cap — and it is well above any playback a caller can
/// have in flight, so reaching it means sessions are being created and
/// abandoned, which is exactly when the oldest is the right one to drop.
pub const MAX_STREAMING_SESSIONS: usize = 1024;

/// Per-service state for com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState {
    /// Every stream's archive, keyed by stream name.
    pub streams: HashMap<String, ArchivedStream>,
    /// `StreamARN` -> stream name: the IDENTITY index an archive is found
    /// through. An archive belongs to the one stream record it was ingested for,
    /// and a stream's creation instant is part of its ARN, so looking an archive
    /// up by the ARN the control plane holds a stream with is what keeps a read
    /// off an archive a previous incarnation of that name left behind.
    pub arn_index: HashMap<String, String>,
    /// Every minted streaming session, keyed by its session token.
    pub sessions: HashMap<String, StreamingSession>,
    /// Monotonic counter behind [`Self::mint_session_token`], so two sessions
    /// minted in the same instant still get distinct tokens.
    session_ordinal: u64,
}

impl ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState {
    /// Add (or replace) a stream's archive, keeping [`Self::arn_index`] in step.
    ///
    /// Replacing a stream drops the ARN the previous entry was indexed under:
    /// a re-created stream has a new ARN (its creation instant is part of it),
    /// and a stale ARN that still resolved to the live stream would answer a
    /// request that named a stream which no longer exists.
    pub fn put_stream(&mut self, stream: ArchivedStream) {
        if let Some(replaced) = self.streams.get(&stream.name) {
            if replaced.arn != stream.arn {
                self.arn_index.remove(&replaced.arn);
            }
        }
        self.arn_index
            .insert(stream.arn.clone(), stream.name.clone());
        self.streams.insert(stream.name.clone(), stream);
    }

    /// Append one ingested fragment to a stream's archive, keeping it in
    /// ascending fragment-number order. Answers `false` when no such stream
    /// exists.
    ///
    /// The archive is already ordered, so the insertion point is found by
    /// binary search rather than by re-sorting the whole archive per ingest.
    ///
    /// What one stream's archive HOLDS is bounded here, at the one site that
    /// grows it: this emulator keeps archived media in the process's memory and
    /// an ingest plane may keep writing for as long as the emulator runs, so
    /// once an archive is at [`MAX_ARCHIVED_FRAGMENTS_PER_STREAM`] or
    /// [`MAX_ARCHIVED_MEDIA_BYTES_PER_STREAM`] its OLDEST fragments are dropped
    /// to make room for the newest. The reads then report the fragments the
    /// stream still holds, which is what they already do for a window that holds
    /// none. A `NextToken` minted before a drop counts from the start of the
    /// result list, so a page taken across one may skip a fragment — the same
    /// artifact a real archive's retention produces mid-pagination.
    pub fn append_fragment(&mut self, stream_name: &str, fragment: StoredFragment) -> bool {
        let Some(stream) = self.streams.get_mut(stream_name) else {
            return false;
        };
        let at = stream
            .fragments
            .binary_search_by(|held| {
                numeric_order(&held.fragment_number, &fragment.fragment_number)
            })
            .unwrap_or_else(|insertion_point| insertion_point);
        stream.fragments.insert(at, fragment);
        // The newest fragment is never dropped, however large it is: an archive
        // that held nothing a producer just wrote would report media it never
        // took.
        let mut bytes: usize = stream
            .fragments
            .iter()
            .map(|held| held.media.len())
            .sum();
        while stream.fragments.len() > MAX_ARCHIVED_FRAGMENTS_PER_STREAM
            || (bytes > MAX_ARCHIVED_MEDIA_BYTES_PER_STREAM && stream.fragments.len() > 1)
        {
            let dropped = stream.fragments.remove(0);
            bytes = bytes.saturating_sub(dropped.media.len());
        }
        true
    }

    /// Resolve the one stream a request names, at the address it arrived on.
    ///
    /// The model documents `StreamName` and `StreamARN` as "Specify either this
    /// parameter or the StreamName parameter", so exactly one must be present:
    /// neither and both are client errors.
    ///
    /// A stream EXISTS because the Kinesis Video Streams control plane created
    /// it, so the identifier is resolved in that service's registry in the
    /// shared engine — the ONE place a stream's existence is held — and its
    /// record is what the stream's ARN and current retention are read from,
    /// re-read here on every call so a retention change is visible to the next
    /// read. A registered stream is READ only while its record is in the one
    /// status that serves reads, `ACTIVE`: a record the registry still holds in
    /// a non-terminal status — created and not yet active, or marked for
    /// deletion — is as unreadable as one the registry no longer holds at all,
    /// and an identifier the registry does not hold is a missing resource
    /// however much media this service's archive still holds for it. All of
    /// them are reported with the same [`STREAM_NOT_FOUND_MESSAGE`] ("not found
    /// or not active").
    ///
    /// This service's archive is not a registration of its own: it is the media
    /// an ingest wrote for ONE stream record, held under the ARN that record
    /// was registered with — a stream's creation instant is part of its ARN, so
    /// the ARN names the incarnation the media was ingested into
    /// ([`Self::put_stream`]). A read therefore attaches the archive the
    /// RESOLVED record's own ARN is indexed under, and the archive of a stream
    /// the registry has removed is served to nobody: not under the identifier it
    /// was ingested with, and not as the archive of a later stream created with
    /// the same name, which is a different stream with a new ARN and an empty
    /// archive until something is ingested into it.
    ///
    /// `addressed_endpoint` is the emulator address the request reached, when it
    /// reached one this emulator serves
    /// ([`::service_common::data_plane_addressing::reached_emulator_endpoint_of_parts`]).
    /// A `GetDataEndpoint` call names the stream it wants an endpoint for and is
    /// answered with that stream's own endpoint, so an announced address serves
    /// the ONE stream it was discovered for: a read at it naming another stream
    /// asked an endpoint for a stream it does not serve, which is as unreadable
    /// as a stream that is not there and carries the same sentence. An address
    /// this emulator announced nothing at — its control-plane front door, a
    /// public AWS host a client resolved — scopes nothing, so a read there
    /// resolves on the registry alone.
    ///
    /// The control-plane store is locked while this service's own store is
    /// already locked (the caller holds that guard); the control plane never
    /// reads this service's state, so there is no cycle between the two.
    pub fn resolve_stream<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        addressed_endpoint: Option<&str>,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Result<ResolvedStream<'_>, StreamLookupError> {
        match (stream_name, stream_arn) {
            (Some(_), Some(_)) | (None, None) => {
                return Err(StreamLookupError::InvalidArgument(
                    ONE_STREAM_IDENTIFIER_MESSAGE.to_string(),
                ))
            }
            _ => {}
        }

        // Which stream the ADDRESS serves, when it is one this emulator announced for a
        // stream: everything resolved below has to be that stream.
        let announced_for = addressed_endpoint
            .and_then(|address| control_plane::published_endpoint_stream(engine, address));
        let serves = |arn: &str| announced_for.as_deref().is_none_or(|served| served == arn);

        // The stream as the control plane holds it right now. An identifier that
        // registry does not hold names no stream at all: this store holds the
        // ARCHIVE of a stream, never a stream, so there is nothing here for a
        // read to fall back on.
        let Some(registered) = registered_stream(engine, stream_name, stream_arn) else {
            return Err(StreamLookupError::NotFound(
                STREAM_NOT_FOUND_MESSAGE.to_string(),
            ));
        };
        // A record the registry holds in a status that does not serve reads
        // is not a readable stream: the archived-media plane refuses it as
        // the missing resource it is, rather than reporting the archive of
        // a stream that is not there yet or is on its way out.
        if !registered.readable() || !serves(&registered.arn) {
            return Err(StreamLookupError::NotFound(
                STREAM_NOT_FOUND_MESSAGE.to_string(),
            ));
        }
        // THIS record's archive: the entry held under the resolved ARN, the
        // identity an archive is written and indexed under ([`Self::put_stream`]).
        // `None` — an empty archive, which is what the per-operation
        // no-fragments behaviour is computed from — both for a registered stream
        // nothing has been ingested into and for one whose name an earlier,
        // removed stream left an archive under: that archive belongs to the ARN
        // it was ingested for, which is not this stream's.
        let archive = self
            .arn_index
            .get(&registered.arn)
            .and_then(|name| self.streams.get(name));
        Ok(ResolvedStream {
            name: registered.name,
            arn: registered.arn,
            data_retention_in_hours: registered.data_retention_in_hours,
            archive_content_type: archive.map_or_else(
                || ARCHIVE_CONTENT_TYPE.to_string(),
                |held| held.archive_content_type.clone(),
            ),
            fragments: archive.map_or(&[][..], |held| held.fragments.as_slice()),
        })
    }

    /// Mint a fresh, unique streaming-session token.
    ///
    /// Opaque and URL-safe: a monotonic ordinal and the creation instant, so
    /// two sessions for the same stream and the same parameters never share a
    /// token.
    pub fn mint_session_token(&mut self, created_at: SystemTime) -> String {
        self.session_ordinal = self.session_ordinal.wrapping_add(1);
        let nanos = created_at
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        format!("{:016x}{:032x}", self.session_ordinal, nanos)
    }

    /// Record a minted session, so the URL it is reported in resolves back to
    /// the session's own parameters and playlist snapshot.
    ///
    /// The store is BOUNDED here, at the one site that grows it, because a
    /// session is minted per request:
    ///
    /// * every session whose `Expires` window the clock has already closed is
    ///   dropped — the session record carries its own TTL
    ///   ([`StreamingSession::expires_at`]), and an expired playback session
    ///   can no longer be used ("When a session expires, no new calls to
    ///   GetHLSMasterPlaylist ... can be made for that session"), so removing
    ///   it changes nothing a caller could still observe;
    /// * and if live sessions alone reach [`MAX_STREAMING_SESSIONS`], the
    ///   oldest is evicted to make room, so the map can never hold more than
    ///   that many entries however many requests arrive.
    ///
    /// Every operation that mints a session — HLS and MPEG-DASH alike —
    /// records it through this one method, so the bound covers them all.
    pub fn record_session(&mut self, session: StreamingSession) {
        let now = session.created_at;
        self.sessions.retain(|_, held| held.is_live_at(now));
        while self.sessions.len() >= MAX_STREAMING_SESSIONS {
            let oldest = self
                .sessions
                .values()
                .min_by_key(|held| held.created_at)
                .map(|held| held.token.clone());
            match oldest {
                Some(token) => {
                    self.sessions.remove(&token);
                }
                None => break,
            }
        }
        self.sessions.insert(session.token.clone(), session);
    }

    /// The session `token` names, while its `Expires` window is still open at
    /// `now`.
    ///
    /// The one rule for which session a playback request may be served for: an
    /// expired session is answered exactly as a token no session was ever
    /// minted for, because "when a session expires, no new calls to
    /// GetHLSMasterPlaylist ... can be made for that session". Nothing is
    /// written here — the record stays until the next mint drops it
    /// ([`Self::record_session`]), which is what keeps a playback read from
    /// changing what a later caller sees.
    #[must_use]
    pub fn live_session(&self, token: &str, now: SystemTime) -> Option<&StreamingSession> {
        self.sessions
            .get(token)
            .filter(|held| held.is_live_at(now))
    }
}

/// Order two fragment numbers the way ingestion order does: numerically when
/// both are decimal (the model's `^[0-9]+$`), lexicographically otherwise, so a
/// non-decimal number still sorts deterministically instead of panicking.
fn numeric_order(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u128>(), b.parse::<u128>()) {
        (Ok(a), Ok(b)) => a.cmp(&b),
        _ => a.cmp(b),
    }
}

/// What the Kinesis Video Streams control plane holds for one stream: the half
/// of a [`ResolvedStream`] this service does not own.
#[derive(Debug, Clone)]
struct RegisteredStream {
    /// The registered `StreamName`.
    name: String,
    /// The registered `StreamARN`.
    arn: String,
    /// The stream's retention AS OF THIS CALL.
    data_retention_in_hours: i64,
    /// The stream's `Status` AS OF THIS CALL, which decides whether the record
    /// is one an archived-media read may serve.
    status: String,
}

impl RegisteredStream {
    /// Whether this record is one the archived-media plane reads.
    ///
    /// `ACTIVE` is the only stream status that serves reads: the other values
    /// `StreamInfo$Status` takes are the non-terminal ones a stream passes
    /// through on its way in or out, and the archived-media plane words its
    /// refusal for both halves at once — the stream is "not found or not
    /// active".
    fn readable(&self) -> bool {
        self.status == control_plane::STATUS_ACTIVE
    }
}

/// The stream the Kinesis Video Streams control plane registry holds under this
/// `StreamName`/`StreamARN`, or `None` when it holds none.
///
/// The registry is that service's state in the SHARED engine, so a stream a
/// client created through `CreateStream` is the very record read here — no copy
/// of it is kept on this side, and nothing here writes to it.
///
/// The registry's own clock is settled first, which is not a mutation this read
/// invents: the move out of `CREATING` and the removal of a stream marked for
/// deletion were each owed at a fixed instant whether or not anybody called, so
/// a read that did not settle them would report a stream in a state the clock
/// had already left — a deleted stream, in particular, would stay readable for
/// as long as nothing else touched the control plane.
///
/// The record's `Status` travels with it, because a stream is readable here only
/// while the control plane holds it `ACTIVE`.
fn registered_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> Option<RegisteredStream> {
    let registry = control_plane::service_state(engine)?;
    let mut registry = registry.lock();
    registry.settle_clock();
    let stream = registry.resolve(stream_name, stream_arn).ok()?;
    Some(RegisteredStream {
        name: stream.stream_name.clone(),
        arn: stream.stream_arn.clone(),
        data_retention_in_hours: i64::from(stream.data_retention_in_hours),
        status: stream.status.clone(),
    })
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideoarchivedmediaAwsacuityreaderState>)
}

#[cfg(test)]
mod tests {
    use super::{epoch_millis, from_epoch_millis};
    use std::time::UNIX_EPOCH;

    /// RULE: the epoch-millisecond conversion is TOTAL over the whole `i64`
    /// range — every offset the platform's clock can represent round-trips, and
    /// one it cannot answers the epoch instead of panicking.
    ///
    /// The rows span what the conversion generalises over: the sign and the
    /// magnitude (zero, a millisecond, a realistic archive instant, a thousand
    /// years).
    #[test]
    fn the_epoch_conversion_round_trips_every_representable_offset() {
        // (case, the millisecond offset)
        let rows: [(&str, i64); 6] = [
            ("the epoch itself", 0),
            ("one millisecond after", 1),
            ("one millisecond before", -1),
            ("an archived fragment's instant", 1_761_000_000_000),
            ("an instant before the epoch", -1_761_000_000_000),
            ("a thousand years on", 31_556_952_000_000),
        ];
        for (case, millis) in rows {
            let actual = epoch_millis(from_epoch_millis(millis));
            assert_eq!(
                actual, millis,
                "case {case:?}: expected {millis} ms from the epoch, got {actual}"
            );
        }
    }

    /// RULE (the other half): at either end of the `i64` range the conversion
    /// ANSWERS — the offset there is some nine quadrillion seconds, which the
    /// plain `Add`/`Sub` impls panic on. A platform whose clock represents it
    /// reads the same number back; one whose clock does not answers the epoch.
    #[test]
    fn the_epoch_conversion_answers_at_both_ends_of_the_range() {
        for (case, millis) in [
            ("the largest i64", i64::MAX),
            ("one short of the largest", i64::MAX - 1),
            ("the smallest i64", i64::MIN),
            ("one past the smallest", i64::MIN + 1),
        ] {
            let instant = from_epoch_millis(millis);
            let actual = epoch_millis(instant);
            assert!(
                actual == millis || instant == UNIX_EPOCH,
                "case {case:?}: expected {millis} ms from the epoch or the epoch itself, got \
                 {actual}"
            );
        }
    }
}
