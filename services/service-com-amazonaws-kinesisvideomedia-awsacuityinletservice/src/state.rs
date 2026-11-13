// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kinesisvideomedia-awsacuityinletservice service.
//!
//! The service's whole surface is one operation, `GetMedia`, which READS the chunks a
//! producer previously wrote to a Kinesis video stream. So the durable thing this
//! service owns is the per-stream fragment log, plus the continuation cursors a
//! `GetMedia` response hands back so the next call resumes exactly where the last one
//! stopped.
//!
//! The STREAM itself is not this service's resource: a Kinesis video stream is created
//! by the Kinesis Video Streams control plane (`CreateStream`), reported by it
//! (`DescribeStream`) and removed by it (`DeleteStream`), and a data plane reached
//! through `GetDataEndpoint` reads THAT stream. So a request's `StreamName` /
//! `StreamARN` is resolved in that service's registry in the shared engine
//! ([`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::resolve_stream`]), whose
//! record decides the stream's identity — which is what makes a just-created stream
//! readable here (with an empty payload until something is produced to it), a removed
//! one a missing resource, and a stream created again under a removed one's name a
//! stream with no media of its own.
//!
//! The producer side of the media is this service's ingest plane
//! ([`crate::service::put_media`], `POST /putMedia`), and it writes through
//! [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::ingest_fragment`] — the
//! one entry point of a produced fragment. That entry point records the fragment on
//! BOTH read planes of the stream: this service's media log, which `GetMedia` streams,
//! and the Kinesis Video Archived Media archive, which `ListFragments` /
//! `GetMediaForFragmentList` report; one write, one fragment number and one set of
//! timestamps, so the two planes cannot disagree about what was ingested.
//! [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::put_fragment`] is the
//! media-log half of it, which a test seeding only this plane calls. `GetMedia` itself
//! never appends a fragment and never creates a stream.

use std::collections::HashMap;
use std::time::SystemTime;

use ::service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::state as control_plane;
use ::service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::state as archive;

/// The content type a Kinesis video stream reports when nothing else is known about
/// its media: Kinesis Video Streams stores producer fragments as Matroska, and
/// `GetMedia` answers `Content-Type: video/webm` for such a stream.
pub const DEFAULT_CONTENT_TYPE: &str = "video/webm";

/// One persisted fragment — what the service documentation calls a "chunk": the media
/// bytes the producer wrote plus the metadata Kinesis Video Streams records alongside
/// them.
#[derive(Debug, Clone)]
pub struct Fragment {
    /// The stream-scoped fragment number, a decimal string (the modelled
    /// `FragmentNumberString` is `^[0-9]+$`, 1..128), increasing with write order.
    pub fragment_number: String,
    /// When the producer says the fragment was captured.
    pub producer_timestamp: SystemTime,
    /// When this service accepted the fragment.
    pub server_timestamp: SystemTime,
    /// The opaque media bytes of the chunk, exactly as the producer wrote them.
    pub chunk: Vec<u8>,
}

/// A stream's persisted media log.
#[derive(Debug, Clone)]
pub struct MediaStream {
    /// The `StreamARN` of the stream whose media this is, and the key the log is held
    /// under: the identity the control plane minted for THIS incarnation of the stream.
    pub arn: String,
    /// The `StreamName` that stream is registered under. Carried for attribution only;
    /// a request is resolved through the registry, never through this field.
    pub name: String,
    /// The content type `GetMedia` reports for this stream's media.
    pub content_type: String,
    /// The fragments this log still holds, oldest first. `GetMedia` emits a contiguous
    /// run of these, so the index of a fragment in this vector IS the stream cursor.
    pub fragments: Vec<Fragment>,
    /// The number the next fragment written to this stream gets.
    pub next_fragment_number: u64,
    /// A short, stable id used to build this stream's continuation tokens. The stream
    /// NAME cannot be used for that: a name is up to 256 characters while a
    /// `ContinuationToken` is at most 128.
    pub id: u64,
    /// How many fragments this log has dropped off its oldest end
    /// ([`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::ingest_fragment`]
    /// bounds what one stream holds). It is what makes a cursor OUTLIVE a drop: a
    /// position in this log is `dropped` plus an index into [`Self::fragments`], and
    /// that absolute position is what a continuation token is minted from, so a token
    /// still names the fragment it was minted for after older ones have gone.
    pub dropped: u64,
    /// When this log was last written, as a sequence number this store hands out: higher
    /// is more recent. It is what picks the log to drop when the store is holding as many
    /// as it may ([`MAX_STORED_STREAM_LOGS`]).
    pub written_at: u64,
}

/// The most fragments one stream's media log holds.
///
/// A ceiling on the STORE, not an AWS limit: this emulator holds a stream's media in the
/// process's memory, and a producer may keep ingesting for as long as the emulator runs,
/// so a log that only ever grew would consume the host. It is the same count one ingest
/// request may submit ([`crate::service::matroska::MAX_CLUSTERS_PER_REQUEST`]), so a
/// stream always holds at least a whole session's worth of media.
pub const MAX_STORED_FRAGMENTS_PER_STREAM: usize = 1024;

/// The most media bytes one stream's media log holds, for the same reason.
///
/// One second of the media rate `GetMedia`'s documentation states ("up to 25 megabytes
/// per second"), which is also what bounds one read session and one ingest request — so
/// a stream's whole retained log can still be read back in one session.
pub const MAX_STORED_MEDIA_BYTES_PER_STREAM: usize = 25_000_000;

/// The most stream media logs this store holds at once.
///
/// A ceiling on the STORE for the same reason as the two above, and their global
/// counterpart: those bound what ONE stream holds, while a producer may keep addressing new
/// streams for as long as the emulator runs — and because a log is keyed by the stream's ARN
/// (see [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::streams`]), a stream
/// deleted and created again under one name leaves the previous incarnation's log behind,
/// which no identifier resolves to any more but which would stay resident. Retained media is
/// therefore bounded by this count times [`MAX_STORED_MEDIA_BYTES_PER_STREAM`] rather than
/// by how many streams have been written to.
///
/// When a new log would take the store past it, the log written longest ago goes, together
/// with the cursors that named it: its stream then reads as a stream nothing has been
/// produced to, which is a state every client already handles (a created stream reads empty)
/// and never a missing resource — the stream itself is the control plane's record, which
/// this store neither holds nor removes.
pub const MAX_STORED_STREAM_LOGS: usize = 64;

/// One fragment as a producer submitted it: the bytes and the facts an ingest records
/// about them, which the two read planes then report.
///
/// Carried as one value because a fragment's bytes and its timestamps are one fact about
/// one fragment: splitting them across an argument list is how a plane ends up recording
/// media under another fragment's instant.
#[derive(Debug, Clone)]
pub struct ProducedFragment {
    /// The media bytes of the fragment, exactly as the producer wrote them.
    pub chunk: Vec<u8>,
    /// When the producer says the fragment was captured.
    pub producer_timestamp: SystemTime,
    /// When this service accepted the fragment.
    pub server_timestamp: SystemTime,
    /// The playback span the fragment covers, which only the archive read plane reports
    /// (`Fragment$FragmentLengthInMilliseconds`).
    pub duration_millis: u64,
}

/// Where a continuation token resumes: the stream it was minted for, and the position of
/// the first fragment the next call must emit.
#[derive(Debug, Clone)]
pub struct Continuation {
    /// The `StreamARN` of the stream the token was minted for. A token presented
    /// against another stream does not resolve.
    pub stream_arn: String,
    /// The ABSOLUTE position in the stream's log of the first fragment still to deliver:
    /// counted from the first fragment ever written, so it survives the dropping of
    /// older fragments ([`MediaStream::dropped`]).
    pub next_fragment_position: u64,
}

/// Per-service state for com-amazonaws-kinesisvideomedia-awsacuityinletservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKinesisvideomediaAwsacuityinletserviceState {
    /// Per-stream fragment logs, keyed by the `StreamARN` the control plane minted for
    /// the stream. NOT a stream registry: an entry here is the media this service holds
    /// for a stream, and a stream with no entry is one nothing has been produced to —
    /// not a stream that does not exist.
    ///
    /// Keyed by the ARN rather than by the `StreamName`, because a name is reusable
    /// while the ARN carries the stream's creation instant: a stream deleted and
    /// created again under one name is a different stream, and must not inherit the
    /// previous incarnation's media or its cursors.
    ///
    /// How many entries this map holds is bounded by [`MAX_STORED_STREAM_LOGS`], as what
    /// one entry holds is bounded by [`MAX_STORED_FRAGMENTS_PER_STREAM`] and
    /// [`MAX_STORED_MEDIA_BYTES_PER_STREAM`] — so the media this store retains is bounded
    /// however many streams a producer addresses.
    pub streams: HashMap<String, MediaStream>,
    /// Continuation token -> cursor, so a token this service minted is resolved from
    /// persisted state rather than parsed back out of the token text.
    pub continuations: HashMap<String, Continuation>,
    /// The last log id handed out; `0` means none has been, so an id is always at least
    /// `1` and never collides with the "no log yet" id a log-less stream reports.
    next_stream_id: u64,
    /// The last write sequence number handed out ([`MediaStream::written_at`]), which
    /// orders the logs by how recently each was written.
    next_write_sequence: u64,
}

impl ComAmazonawsKinesisvideomediaAwsacuityinletserviceState {
    /// The media log of the registered stream `stream_arn` identifies, created empty
    /// when this is the first time it is named.
    ///
    /// Private: a log is not a stream. A stream EXISTS because the control plane
    /// registered it, so nothing outside this store creates a log — the one writer is
    /// [`put_fragment`](Self::put_fragment), which resolves the registered stream
    /// first.
    fn log_mut(&mut self, stream_arn: &str, stream_name: &str) -> &mut MediaStream {
        // Ids are handed out from 1, so the `0` a stream with no log reports can never
        // be a live log's id, and two streams never share a continuation-token prefix.
        let fresh_id = self.next_stream_id.saturating_add(1);
        if !self.streams.contains_key(stream_arn) {
            self.next_stream_id = fresh_id;
            // A log this store does not hold yet is one more log, so the global ceiling is
            // enforced here — the one place the number of logs grows.
            self.drop_logs_beyond_the_ceiling();
        }
        // Stamped on every write, so the log dropped when the ceiling is reached is the one
        // whose producer stopped writing longest ago.
        self.next_write_sequence = self.next_write_sequence.saturating_add(1);
        let written_at = self.next_write_sequence;
        let log = self
            .streams
            .entry(stream_arn.to_string())
            .or_insert_with(|| MediaStream {
                arn: stream_arn.to_string(),
                name: stream_name.to_string(),
                content_type: DEFAULT_CONTENT_TYPE.to_string(),
                fragments: Vec::new(),
                next_fragment_number: 1,
                id: fresh_id,
                dropped: 0,
                written_at,
            });
        log.written_at = written_at;
        log
    }

    /// Make room for one more media log: while the store holds as many as
    /// [`MAX_STORED_STREAM_LOGS`] allows, drop the one written longest ago and the cursors
    /// that named it.
    ///
    /// A loop rather than a single removal, so a ceiling lowered between builds settles on
    /// the first write instead of leaving the store over it forever. The cursors go with the
    /// log for the same reason they go with dropped fragments
    /// ([`forget_cursors_behind_the_log`](Self::forget_cursors_behind_the_log)): a cursor
    /// into media that is gone resolves to no position, and the next call presenting it is
    /// answered as the bad argument it is.
    fn drop_logs_beyond_the_ceiling(&mut self) {
        while self.streams.len() >= MAX_STORED_STREAM_LOGS {
            let Some(oldest) = self
                .streams
                .values()
                .min_by_key(|stream| stream.written_at)
                .map(|stream| stream.arn.clone())
            else {
                return;
            };
            self.streams.remove(&oldest);
            self.continuations
                .retain(|_, held| held.stream_arn != oldest);
        }
    }

    /// Append `chunk` to the media log of the stream `stream_name`/`stream_arn` names,
    /// and answer the fragment number it was stored under.
    ///
    /// The media-log half of a produced fragment: the writer of the chunk run `GetMedia`
    /// streams. The stream it addresses is resolved in the control-plane registry
    /// exactly as a read resolves it, so media is only ever written to a stream that
    /// EXISTS, under the identity that stream is registered with. `None` when the
    /// registry holds no such stream — the producer side of `ResourceNotFoundException`.
    ///
    /// The ingest plane writes through
    /// [`ingest_fragment`](Self::ingest_fragment) instead, which records the same
    /// fragment on the archive read plane as well.
    pub fn put_fragment<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
        chunk: Vec<u8>,
        producer_timestamp: SystemTime,
        server_timestamp: SystemTime,
    ) -> Option<String> {
        let (name, arn) = registered_stream(engine, stream_name, stream_arn)?;
        let stream = self.log_mut(&arn, &name);
        let fragment_number = stream.next_fragment_number.to_string();
        stream.next_fragment_number += 1;
        stream.fragments.push(Fragment {
            fragment_number: fragment_number.clone(),
            producer_timestamp,
            server_timestamp,
            chunk,
        });
        Some(fragment_number)
    }

    /// Record one produced fragment on every read plane of the stream
    /// `stream_name`/`stream_arn` names, and answer the fragment number it was stored
    /// under.
    ///
    /// This is the ingest plane's one write. A fragment a producer submits is readable
    /// two ways — as a chunk of the live stream (`GetMedia`, this service's own log) and
    /// as an archived fragment (`ListFragments`, `GetMediaForFragmentList`, the Kinesis
    /// Video Archived Media archive) — and those are two VIEWS of one ingested fragment,
    /// not two fragments: both are written here, from the same bytes, under the same
    /// minted fragment number and the same producer and server timestamps. A read plane
    /// that reported a fragment the other did not would be reporting media nothing
    /// produced.
    ///
    /// The fragment number the ingest minted, or `None` when the control-plane registry
    /// holds no such stream — the producer side of `ResourceNotFoundException`.
    ///
    /// What one stream HOLDS is bounded here, at the one site that grows it: this
    /// emulator keeps a stream's media in memory and a producer may ingest for as long as
    /// the emulator runs, so once a log is at
    /// [`MAX_STORED_FRAGMENTS_PER_STREAM`] or [`MAX_STORED_MEDIA_BYTES_PER_STREAM`] the
    /// oldest fragments are dropped to make room for the newest. The two read planes then
    /// report the fragments the stream still holds, which is what they already do for a
    /// stream nothing has been produced to; a cursor a reader holds survives the drop
    /// ([`MediaStream::dropped`]).
    pub fn ingest_fragment<Eng: Send + Sync + 'static>(
        &mut self,
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
        produced: ProducedFragment,
    ) -> Option<String> {
        let registered = registered_stream_record(engine, stream_name, stream_arn)?;
        let stream = self.log_mut(&registered.arn, &registered.name);
        let fragment_number = stream.next_fragment_number.to_string();
        stream.next_fragment_number += 1;
        stream.fragments.push(Fragment {
            fragment_number: fragment_number.clone(),
            producer_timestamp: produced.producer_timestamp,
            server_timestamp: produced.server_timestamp,
            chunk: produced.chunk.clone(),
        });
        drop_oldest_beyond_the_ceiling(stream);
        // The cursors that named the fragments which just went are dropped with them: a
        // continuation store that outlived the media would be the one piece of
        // per-fragment bookkeeping this ceiling did not bound.
        self.forget_cursors_behind_the_log(&registered.arn);
        archive_fragment(
            engine,
            &registered,
            archive::StoredFragment::new(
                fragment_number.clone(),
                produced.producer_timestamp,
                produced.server_timestamp,
                produced.duration_millis,
                produced.chunk,
            ),
        );
        Some(fragment_number)
    }

    /// The fragment number the NEXT fragment received on the stream
    /// `stream_name`/`stream_arn` names will carry.
    ///
    /// A fragment number is minted when the service RECEIVES the fragment, which is why
    /// the acknowledgement stream reports one for a fragment it has not stored yet —
    /// including the acknowledgement of a FAULT found in that fragment, which names the
    /// fragment it was found in. Reading it mints nothing and consumes nothing: the next
    /// fragment stored still takes this number, and a stream nothing has been produced to
    /// yet is not created by the read.
    pub fn received_fragment_number<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Option<String> {
        let registered = registered_stream_record(engine, stream_name, stream_arn)?;
        Some(
            self.streams
                .get(&registered.arn)
                .map_or(1, |stream| stream.next_fragment_number)
                .to_string(),
        )
    }

    /// Forget every continuation token of the stream `stream_arn` names whose fragment
    /// has been dropped off the log's oldest end.
    ///
    /// Called wherever the log drops fragments, so the continuation store is bounded by
    /// the same ceiling the media is: what is left is at most one cursor per fragment the
    /// stream still holds, plus the one that resumes at its end. A token this removes
    /// named a fragment that is gone, so the session it would have resumed can no longer
    /// be served from where it stopped, and the next call is answered as the
    /// bad-argument a token that resolves to no position earns.
    fn forget_cursors_behind_the_log(&mut self, stream_arn: &str) {
        let Some(dropped) = self.streams.get(stream_arn).map(|stream| stream.dropped) else {
            return;
        };
        self.continuations.retain(|_, held| {
            held.stream_arn != stream_arn || held.next_fragment_position >= dropped
        });
    }

    /// The media log held for the stream `arn` identifies, or `None` when nothing has
    /// been produced to that stream.
    pub fn log_by_arn(&self, arn: &str) -> Option<&MediaStream> {
        self.streams.get(arn)
    }

    /// The one stream a `GetMedia` request addresses.
    ///
    /// A stream EXISTS because the Kinesis Video Streams control plane created it, so
    /// the identifier is resolved in that service's registry in the shared engine, and
    /// its record is what the stream's name and ARN are read from — so both planes
    /// answer to the one identity, a stream created a moment ago is readable here, and
    /// a stream the control plane no longer holds is not. The FRAGMENTS are this
    /// service's own log for that stream, empty for a registered stream nothing has
    /// been produced to, and that emptiness is what an empty payload is computed from.
    ///
    /// `None` when the registry holds no stream for this identifier — the one condition
    /// `ResourceNotFoundException` documents ("The stream with the given name does not
    /// exist").
    ///
    /// The control-plane store is locked while this service's own store is already
    /// locked (the caller holds that guard); the control plane never reads this
    /// service's state, so there is no cycle between the two.
    pub fn resolve_stream<Eng: Send + Sync + 'static>(
        &self,
        engine: &Eng,
        stream_name: Option<&str>,
        stream_arn: Option<&str>,
    ) -> Option<ResolvedStream<'_>> {
        let (name, arn) = registered_stream(engine, stream_name, stream_arn)?;
        let log = self.streams.get(&arn);
        Some(ResolvedStream {
            name,
            arn,
            content_type: log.map_or_else(
                || DEFAULT_CONTENT_TYPE.to_string(),
                |held| held.content_type.clone(),
            ),
            id: log.map_or(0, |held| held.id),
            fragments: log.map_or(&[][..], |held| held.fragments.as_slice()),
        })
    }

    /// The token that resumes the stream `stream_arn` identifies at
    /// `next_fragment_index`, recording the cursor so the next request resolves it from
    /// state.
    ///
    /// Derived from the cursor (and the stream's own [`MediaStream::id`], which the
    /// caller has because it resolved the stream) rather than drawn at random, so
    /// re-reading the same fragments answers the same token instead of growing the map
    /// on every call. The text satisfies the modelled `ContinuationToken`
    /// (`^[a-zA-Z0-9_\.\-]+$`, 1..128).
    ///
    /// `next_fragment_index` is an index into the log AS IT IS NOW; what is recorded (and
    /// what the token text carries) is the ABSOLUTE position that index names, so the
    /// token keeps naming the same fragment after older ones are dropped.
    pub fn record_continuation(
        &mut self,
        stream_arn: &str,
        stream_id: u64,
        next_fragment_index: usize,
    ) -> String {
        // A cursor past the end of the log names no chunk, so never record one: the
        // caller resolved `next_fragment_index` against this same store, and the clamp
        // keeps a future caller from minting a token that resumes nowhere. A stream
        // this store holds no log for cannot be clamped against, and a cursor recorded
        // for one is caught instead when the token is resolved (see
        // [`continuation`](Self::continuation)).
        let held = self.streams.get(stream_arn);
        let next_fragment_index = held.map_or(next_fragment_index, |stream| {
            next_fragment_index.min(stream.fragments.len())
        });
        let next_fragment_position = held
            .map_or(0, |stream| stream.dropped)
            .saturating_add(next_fragment_index as u64);
        let token = format!("kvm-{stream_id}-{next_fragment_position}");
        self.continuations
            .entry(token.clone())
            .or_insert_with(|| Continuation {
                stream_arn: stream_arn.to_string(),
                next_fragment_position,
            });
        token
    }

    /// The INDEX a previously minted token resumes at in the log of the stream
    /// `stream_arn` identifies, which holds `fragment_count` fragments now.
    ///
    /// `None` when the token was never minted here, was minted for another stream, or
    /// names a position past that stream's log: a cursor names a position in one
    /// stream's fragment log, so both bindings are enforced here rather than left to
    /// each caller. A cursor exactly at the end of the log is live — it is where the
    /// next fragment will land, so it resumes with an empty payload.
    ///
    /// A token whose own fragment has since been DROPPED off the log's oldest end is no
    /// longer held at all ([`forget_cursors_behind_the_log`](Self::forget_cursors_behind_the_log)),
    /// so it resolves to nothing rather than silently resuming somewhere else.
    pub fn continuation(
        &self,
        token: &str,
        stream_arn: &str,
        fragment_count: usize,
    ) -> Option<usize> {
        let continuation = self.continuations.get(token)?;
        if continuation.stream_arn != stream_arn {
            return None;
        }
        let dropped = self.streams.get(stream_arn).map_or(0, |stream| stream.dropped);
        let index = usize::try_from(continuation.next_fragment_position.saturating_sub(dropped))
            .unwrap_or(usize::MAX);
        (index <= fragment_count).then_some(index)
    }
}

/// The one stream a `GetMedia` request addresses: its IDENTITY as the Kinesis Video
/// Streams control plane holds it, with the fragments this service owns for it.
///
/// Read through
/// [`ComAmazonawsKinesisvideomediaAwsacuityinletserviceState::resolve_stream`] on every
/// call, so a stream the control plane created or removed since the last read is
/// observed by the next one rather than by a copy taken earlier.
#[derive(Debug)]
pub struct ResolvedStream<'a> {
    /// The stream's `StreamName`.
    pub name: String,
    /// The stream's `StreamARN`, which is also the key its media log is held under.
    pub arn: String,
    /// The content type `GetMedia` reports for this stream's media.
    pub content_type: String,
    /// The short, stable id this stream's continuation tokens are built from; `0` for a
    /// stream with no log yet, which carries no fragment and so mints no token (a live
    /// log's id is always at least `1`).
    pub id: u64,
    /// This stream's fragment log, oldest first; empty for a registered stream nothing
    /// has been produced to.
    pub fragments: &'a [Fragment],
}

/// The `StreamName` and `StreamARN` the Kinesis Video Streams control plane registry
/// holds for this identifier pair, or `None` when it holds no such stream.
///
/// The registry is that service's state in the SHARED engine, so a stream a client
/// created through `CreateStream` is the very record read here — no copy of it is kept
/// on this side.
///
/// The registry's clock is settled first, so a stream whose removal the control plane
/// already owed (`DeleteStream` fixes that instant) is gone here too: that removal was
/// due at the same instant whether or not anybody called, so it changes nothing a later
/// caller would not have seen anyway.
///
/// The record's `Status` is not read: a stream is resolved whatever status the control
/// plane holds it in, so whether a read of a stream that is still creating (or already
/// marked for deletion) should be refused belongs with the stream-state work on the
/// control plane, not here.
fn registered_stream<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> Option<(String, String)> {
    let stream = registered_stream_record(engine, stream_name, stream_arn)?;
    Some((stream.name, stream.arn))
}

/// The resource an IAM decision about a media call is taken on: the `StreamARN` of the
/// stream the request addresses.
///
/// The control-plane registry holds the one ARN both planes address a stream by, so that
/// ARN is what a policy's `Resource` element is matched against whenever the identifier
/// resolves. For an identifier the registry does not hold there is no ARN to state — a
/// stream's ARN carries the creation instant the control plane assigned it, which an
/// unregistered name has none of — so the decision is taken on the identifier the request
/// carried, which is everything the request said about the resource. Either way the
/// decision is taken before anything about the stream is reported, so a caller IAM refuses
/// learns nothing about whether the stream is there.
pub fn addressed_stream_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    stream_name: Option<&str>,
    stream_arn: Option<&str>,
) -> String {
    registered_stream(engine, stream_name, stream_arn)
        .map(|(_, arn)| arn)
        .or_else(|| stream_arn.or(stream_name).map(str::to_string))
        .unwrap_or_default()
}

/// What the Kinesis Video Streams control plane holds for one stream: its identity, and
/// the two configured values a read plane reports about the stream itself rather than
/// about its media.
///
/// Read on every call, never cached, for the reason [`registered_stream`] gives.
#[derive(Debug, Clone)]
struct RegisteredStream {
    /// The registered `StreamName`.
    name: String,
    /// The registered `StreamARN`.
    arn: String,
    /// The stream's `MediaType`, empty when the stream was created without one.
    media_type: String,
    /// The stream's retention AS OF THIS CALL, in hours.
    data_retention_in_hours: u32,
}

/// The stream the control-plane registry holds for this identifier pair, or `None` when
/// it holds none. The one resolver behind every read and every write here — see
/// [`registered_stream`] for why the registry, and not a copy of it, decides.
fn registered_stream_record<Eng: Send + Sync + 'static>(
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
        media_type: stream.media_type.clone().unwrap_or_default(),
        data_retention_in_hours: u32::try_from(stream.data_retention_in_hours).unwrap_or_default(),
    })
}

/// Record one ingested fragment in the Kinesis Video Archived Media archive of
/// `registered`, so the archive read plane reports the fragment this service's media log
/// just took.
///
/// The archive entry is created on first ingest and re-created whenever the stream's
/// registered ARN has changed, because an ARN carries the stream's creation instant: a
/// stream deleted and created again under one name is a different stream and must not
/// inherit the previous incarnation's fragments. The stream's `MediaType` and retention
/// are re-read from the registry on every ingest, so the archive reports what the
/// control plane holds now.
///
/// The archive store is locked while this service's own store is already locked (the
/// caller holds that guard); the archive read plane never reads this service's state, so
/// there is no cycle between the two. A missing archive store is not an error here: the
/// fragment is still on this service's own log, and an emulator assembled without the
/// archive plane has no reader for it.
///
/// What the archive HOLDS is bounded by the archive crate itself, inside its
/// `append_fragment` — the one site that grows it — so the two planes' ceilings are each
/// stated by the store they bound rather than by this writer.
fn archive_fragment<Eng: Send + Sync + 'static>(
    engine: &Eng,
    registered: &RegisteredStream,
    fragment: archive::StoredFragment,
) {
    let Some(store) = archive::service_state(engine) else {
        return;
    };
    let mut store = store.lock();
    let held = store
        .streams
        .get(&registered.name)
        .is_some_and(|held| held.arn == registered.arn);
    if held {
        if let Some(stream) = store.streams.get_mut(&registered.name) {
            stream.media_type = registered.media_type.clone();
            stream.data_retention_in_hours = registered.data_retention_in_hours;
        }
    } else {
        store.put_stream(archive::ArchivedStream {
            name: registered.name.clone(),
            arn: registered.arn.clone(),
            data_retention_in_hours: registered.data_retention_in_hours,
            media_type: registered.media_type.clone(),
            archive_content_type: archive::ARCHIVE_CONTENT_TYPE.to_string(),
            codec_private_data: None,
            fragments: Vec::new(),
        });
    }
    store.append_fragment(&registered.name, fragment);
}

/// Drop the oldest fragments of `stream` until the log is inside the ceilings
/// [`MAX_STORED_FRAGMENTS_PER_STREAM`] and [`MAX_STORED_MEDIA_BYTES_PER_STREAM`] set,
/// counting what went in [`MediaStream::dropped`] so a cursor still names the fragment it
/// was minted for.
///
/// The NEWEST fragment is never dropped, however large it is: a stream that holds nothing
/// a producer just wrote would report media it never took. One fragment of its own is
/// therefore the floor, and the per-request ceiling
/// ([`crate::service::matroska::MAX_INGESTED_BYTES_PER_REQUEST`]) is what bounds that one.
fn drop_oldest_beyond_the_ceiling(stream: &mut MediaStream) {
    let mut bytes: usize = stream
        .fragments
        .iter()
        .map(|fragment| fragment.chunk.len())
        .sum();
    while stream.fragments.len() > MAX_STORED_FRAGMENTS_PER_STREAM
        || (bytes > MAX_STORED_MEDIA_BYTES_PER_STREAM && stream.fragments.len() > 1)
    {
        let dropped = stream.fragments.remove(0);
        bytes = bytes.saturating_sub(dropped.chunk.len());
        stream.dropped = stream.dropped.saturating_add(1);
    }
}

/// `t` as milliseconds since the Unix epoch (negative before it), the unit Kinesis
/// Video Streams reports its `AWS_KINESISVIDEO_*_TIMESTAMP` MKV tags in.
pub fn epoch_millis(t: SystemTime) -> i64 {
    match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => -i64::try_from(e.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKinesisvideomediaAwsacuityinletserviceState>)
}
