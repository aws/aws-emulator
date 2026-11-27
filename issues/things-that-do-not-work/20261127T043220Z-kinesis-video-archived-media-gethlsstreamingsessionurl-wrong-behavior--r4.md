Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0ba3642f58277fa2cf06ae884d17310e90ffbba

# Producer-timestamp deduplication keeps the NEWEST ingested fragment, where the model says only the oldest ingested collection is returned

_Filed: 2026-11-27T04:32:20Z_
_Operation: GetHLSStreamingSessionURL | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0ba3642f58277fa2cf06ae884d17310e90ffbba`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When a streaming session's selector reads PRODUCER_TIMESTAMP and several archived fragments carry the same producer timestamp, the crate collapses the group to the fragment with the LARGEST fragment number. The crate's own state module documents a fragment number as increasing with ingestion order, so the kept fragment is the most recently INGESTED one. The model's sentence for this rule says the opposite: "only the oldest ingested collection of fragments are returned". The playlist/manifest a session is minted over therefore lists the wrong member of every duplicate-producer-timestamp group, and because the same selection drives the media a player then fetches, the session serves the wrong archived bytes for those instants.

The helper is shared, so `GetDASHStreamingSessionURL` (which reaches it through the same `ResolvedStream::session_playlist`) and the LIVE freshness check `holds_live_fragments` inherit the same inversion; the doc sentence quoted below appears verbatim on both protocols' selectors.

## Evidence

Smithy, `HLSFragmentSelector$FragmentSelectorType` documentation (smithy_query query="doc" shape="...#HLSFragmentSelector" member="FragmentSelectorType"):
"Fragments that have duplicate producer timestamps are deduplicated. This means that if producers are producing a stream of fragments with producer timestamps that are approximately equal to the true clock time, the HLS media playlists will contain all of the fragments within the requested timestamp range. If some fragments are ingested within the same time range and very different points in time, only the oldest ingested collection of fragments are returned."
(The identical sentence is on `DASHFragmentSelector$FragmentSelectorType`, with "MPEG-DASH manifest" in place of "HLS media playlists".)

The code keeps the LARGEST fragment number — `services/service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader/src/state.rs:398-412`:
```
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
```
and that number is the INGESTION order — `state.rs:135-137`:
```
    /// The fragment number, a decimal string that increases with ingestion
    /// order (`^[0-9]+$`).
    pub fragment_number: String,
```
so the surviving fragment is the newest ingested, not the oldest.

The crate's own justification for "newest" quotes a phrase the model does not carry for this rule — `state.rs:371-376`:
```
/// The rule both streaming-session operations document for the manifest or
/// playlist they mint a session over: when the fragment selector reads producer
/// timestamps and several fragments carry the same start timestamp, only the
/// newest of them — the one with the largest fragment number — belongs in the
/// snapshot, and "the other fragments are not included".
```
Reached from the session mint via `state.rs:581-594` (`session_playlist`) and `src/ops/get_hls_streaming_session_url.rs:457-463` (`stream.minted_session_fragment_numbers(...)`).

Test coverage: `tests/all/get_dash_streaming_session_url.rs:991` (`fragments_sharing_one_producer_start_timestamp_collapse_to_the_newest`) and `:1113` pin the current direction, with the stated reason being the same unsourced phrase (`get_dash_streaming_session_url.rs:979-984`: "collapsed to the one with the largest fragment number before the snapshot is capped — \"the other fragments are not included\""), which the quoted model sentence contradicts; those rows (expected `&[11]`, `&[22]`) would have to expect the oldest of each group instead.

## Steps to Reproduce

1. Register a stream with DataRetentionInHours > 0 and ingest two fragments with the SAME producer timestamp: fragment number 10 ingested first, fragment number 11 ingested second.
2. POST /getHLSStreamingSessionURL with PlaybackMode ON_DEMAND and HLSFragmentSelector = { FragmentSelectorType: "PRODUCER_TIMESTAMP", TimestampRange: { StartTimestamp: <instant - 1s>, EndTimestamp: <instant + 1s> } }.
3. Fetch the returned URL's media playlist and read the FragmentNumber query field of its segment URLs.
4. Observed: the playlist lists fragment 11 (the newest ingested). Expected per HLSFragmentSelector$FragmentSelectorType: "only the oldest ingested collection of fragments are returned", i.e. fragment 10.
