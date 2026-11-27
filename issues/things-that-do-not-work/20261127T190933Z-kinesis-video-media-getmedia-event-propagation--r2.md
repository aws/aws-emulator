Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: de97aca7e4877ce07b58fd1c0c479e4626263ea0

# A stream at its media ceiling evicts the fragment the ingest JUST stored when that fragment was captured earlier than the log's oldest, so PutMedia acknowledges PERSISTED and the archive plane keeps media GetMedia can never serve

_Filed: 2026-11-27T19:09:33Z_
_Operation: GetMedia | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `de97aca7e4877ce07b58fd1c0c479e4626263ea0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ingest_fragment` writes one produced fragment to BOTH read planes of a stream — this crate's media log (what `GetMedia` streams) and the Kinesis Video Archived Media archive — and then bounds what one stream's media log holds by calling `drop_oldest_beyond_the_ceiling`. That eviction removes `fragments.remove(0)`: the first element of a vector ordered by PRODUCER capture instant, not the fragment written longest ago.

The same function, two lines earlier, deliberately INSERTS the newly received fragment at a position inside that vector rather than at its end, because a reconnecting producer's session legitimately names instants an earlier session already covered (a RELATIVE session restarts its timecodes at 0). When such a fragment is inserted at index 0 and the log is at either per-stream ceiling, the very fragment just stored is the one `remove(0)` throws away.

The consequences are all cross-plane:

* `PutMedia` answers `200` with a `PERSISTED` acknowledgement carrying the minted fragment number for media the media log no longer holds, so `GetMedia` (`EARLIEST`, `NOW`, any cursor) never serves that chunk — the latest chunk a producer wrote is missing from the stream, and no error is reported anywhere.
* `archive_fragment` runs AFTER the eviction, from a clone taken before it, so the archive plane (`ListFragments` / `GetMediaForFragmentList`) still reports the fragment this service's own log discarded — exactly the divergence `ingest_fragment`'s doc says must not happen.
* `MediaStream::dropped` is incremented for that eviction, so `forget_cursors_behind_the_log` then purges continuation cursors at absolute position 0 even though the fragment they name is still held.

This is not a complaint about the ceiling values: it is the eviction picking the wrong element. The floor the code implements (`fragments.len() > 1`) protects the LAST element of the vector (newest by capture instant), while the rule the function's own doc states — "The NEWEST fragment is never dropped, however large it is: a stream that holds nothing a producer just wrote would report media it never took" — is about the fragment just written.

## Evidence

The rule the eviction is documented to follow — services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/src/state.rs:790-798:

    /// Drop the oldest fragments of `stream` until the log is inside the ceilings
    /// [`MAX_STORED_FRAGMENTS_PER_STREAM`] and [`MAX_STORED_MEDIA_BYTES_PER_STREAM`] set,
    /// counting what went in [`MediaStream::dropped`] so a cursor still names the fragment it
    /// was minted for.
    ///
    /// The NEWEST fragment is never dropped, however large it is: a stream that holds nothing
    /// a producer just wrote would report media it never took. One fragment of its own is
    /// therefore the floor, and the per-request ceiling
    /// ([`crate::service::matroska::MAX_INGESTED_BYTES_PER_REQUEST`]) is what bounds that one.

The code under it, which removes the FIRST element of a producer-time-ordered vector — src/state.rs:799-812:

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

The sibling half of the SAME function that puts the just-received fragment at index 0 — src/state.rs:420-441:

    // The log is ordered by the instant the producer captured each fragment at, which
    // is the order its readers resolve a cursor in ([`MediaStream::fragments`]). A
    // producer may open a new session whose timecodes name instants an earlier session
    // already covered — a RELATIVE session restarts its offsets at 0 — so the fragment
    // takes its place in that order rather than the end of the vector, ...
    let position = stream
        .fragments
        .iter()
        .rposition(|held| held.producer_timestamp <= produced.producer_timestamp)
        .map_or(0, |last| last + 1);
    stream.fragments.insert(
        position,
        Fragment { ... },
    );
    drop_oldest_beyond_the_ceiling(stream);

(`position` is 0 whenever no held fragment was captured at or before the new one, i.e. for every reconnecting session that restates an earlier instant.)

The cross-plane invariant this breaks — src/state.rs:389-408 (doc of `ingest_fragment`):

    /// This is the ingest plane's one write. A fragment a producer submits is readable
    /// two ways — as a chunk of the live stream (`GetMedia`, this service's own log) and
    /// as an archived fragment (`ListFragments`, `GetMediaForFragmentList`, ...) — and those are two VIEWS of one ingested fragment,
    /// not two fragments: both are written here, from the same bytes, ...
    /// A read plane that reported a fragment the other did not would be reporting media nothing
    /// produced.
    ...
    /// the emulator runs, so once a log is at
    /// [`MAX_STORED_FRAGMENTS_PER_STREAM`] or [`MAX_STORED_MEDIA_BYTES_PER_STREAM`] the
    /// oldest fragments are dropped to make room for the newest.

The archive write happens after the eviction, from the clone taken before it — src/state.rs:438 (`chunk: produced.chunk.clone()`), src/state.rs:441 (`drop_oldest_beyond_the_ceiling(stream);`), src/state.rs:446-456 (`archive_fragment(engine, &registered, archive::StoredFragment::new(fragment_number.clone(), ..., produced.chunk));`).

The producer is told the fragment is stored — src/service/put_media.rs:64-67:

    //! Every accepted Cluster is persisted through
    //! [`...::ingest_fragment`],
    //! the one write of the producer plane, so a fragment this plane acknowledges is the
    //! fragment `GetMedia`, `ListFragments` and `GetMediaForFragmentList` go on to report.

and src/service/put_media.rs:477-481 pushes the `PERSISTED` acknowledgement for the fragment number `ingest_fragment` returned, whatever the eviction then did with it.

The out-of-order session is a supported, tested path — src/service/put_media.rs:388-395: "A new connection states a new baseline — a RELATIVE session's timecodes legitimately restart at 0, naming instants its predecessor's offsets already covered — so nothing in it is compared against what another session left, and the log is kept in time order where it is written instead"; the insert ordering is pinned by tests/all/put_media_reconnecting_producer.rs:400-479.

What `GetMedia` then serves — src/ops/get_media.rs:731-744 measures the session over `&fragments[start..]` and src/ops/get_media.rs:775-798 emits exactly those chunks, so a fragment removed from `MediaStream::fragments` is unreachable by every selector type.

Test coverage: the per-stream ceiling is pinned ONLY for an appending producer — tests/all/put_media_ingest.rs:1525-1620 (`one_streams_log_is_bounded_and_a_cursor_survives_the_drop`) fills the log with strictly increasing timecodes (`cluster(at as u64 * 10, ...)`) and then overflows it with later ones (`cluster(100_000 + at as u64 * 10, ...)`), asserting "expected the five oldest fragments dropped" and "expected the newest ingested fragment {newest} last". No test ingests an EARLIER-captured fragment into a log that is at a ceiling, so nothing pins the behaviour reported here.

## Steps to Reproduce

1. CreateStream "probe" on the Kinesis Video control plane, then GetDataEndpoint(StreamName=probe, APIName=PUT_MEDIA) and GetDataEndpoint(StreamName=probe, APIName=GET_MEDIA) to get the two announced addresses.
2. POST /putMedia at the ingest address with `x-amzn-stream-name: probe`, `x-amzn-fragment-timecode-type: ABSOLUTE`, a declared `x-amz-content-sha256`, and a container with ONE Cluster of about 20 MB of frames at Timestamp 1700000010000. Observe 200 with BUFFERING/RECEIVED/PERSISTED for fragment 1.
3. POST /getMedia at the read address with {"StreamName":"probe","StartSelector":{"StartSelectorType":"EARLIEST"}} and confirm the payload carries fragment 1.
4. POST /putMedia again, same headers, with ONE Cluster of about 10 MB at Timestamp 1700000000000 — ten seconds EARLIER than the fragment already held (a reconnecting producer restating an earlier instant). Observe 200 with PERSISTED for fragment 2.
5. Internally: `ingest_fragment` inserts fragment 2 at index 0 (no held fragment was captured at or before it), the log now sums ~30 MB > MAX_STORED_MEDIA_BYTES_PER_STREAM (25_000_000) with len 2 > 1, so `drop_oldest_beyond_the_ceiling` runs `fragments.remove(0)` and deletes fragment 2 — the fragment just acknowledged — leaving only fragment 1 and `dropped == 1`.
6. POST /getMedia with EARLIEST (and again with NOW): the payload carries only fragment 1's chunk and its AWS_KINESISVIDEO_FRAGMENT_NUMBER "1". Fragment 2, acknowledged PERSISTED, is served by no selector.
7. Call ListFragments / GetMediaForFragmentList on the archived-media endpoint for "probe": fragment 2 is still reported there, so the two read planes of one stream disagree about what was ingested.
8. The count ceiling reproduces the same loss without large bodies: ingest MAX_STORED_FRAGMENTS_PER_STREAM (1024) Clusters with increasing ABSOLUTE timecodes, then one more Cluster whose timecode is earlier than all of them — len becomes 1025 > 1024, `remove(0)` deletes the fragment just stored, and GetMedia never serves it.
