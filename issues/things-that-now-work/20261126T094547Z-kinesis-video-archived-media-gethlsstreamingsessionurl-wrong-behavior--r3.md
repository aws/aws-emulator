Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: abb6c9f0ec6415df39dd32bcb38d50bf198d6a28

# A LIVE_REPLAY session's playlist still cannot grow: the re-selection keeps the OLDEST max_fragments, so once the range holds that many, a newly ingested fragment is never added

_Filed: 2026-11-26T09:45:47Z_
_Operation: GetHLSStreamingSessionURL | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `abb6c9f0ec6415df39dd32bcb38d50bf198d6a28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A session whose playback mode is LIVE_REPLAY is treated as a GROWING session (`StreamingSession::grows()` is true for anything other than ON_DEMAND), so the playback plane re-selects its playlist from the archive on every fetch. But the re-selection for a range window takes the OLDEST `max_fragments` fragments of the range (`.take(max_fragments)` over the archive in ingestion order, with `first_sequence: 0`), which is the ON_DEMAND rule. The consequence: as soon as the requested range holds `max_fragments` fragments — the documented default for LIVE_REPLAY is 5 — every later fetch re-selects the SAME oldest N and a fragment ingested after that point can never enter the playlist, for the life of the session.

The playlist nevertheless advertises itself as growing: for a LIVE_REPLAY session the media playlist is written without `#EXT-X-PLAYLIST-TYPE:VOD` and without `#EXT-X-ENDLIST`, and the MPEG-DASH manifest is typed `dynamic`, so the player is told to keep polling a resource that can never change.

The LIVE branch of the same function does handle the cap correctly — it sorts by server timestamp and SKIPS the surplus from the front, keeping the newest N and raising `first_sequence` — so the two modes that `grows()` puts in one class behave in opposite ways. This is the residue of the (now fixed) "playlist frozen at mint" issue: the re-selection landed, but for LIVE_REPLAY the cap is still spent on the oldest fragments, so step 6 of that issue still reproduces whenever the range already holds the cap's worth of fragments. `GetDASHStreamingSessionURL` has the identical gap through the same shared site (`ResolvedStream::session_playlist`); a fix there covers both.

## Evidence

Model (the mode is defined as one whose playlist is UPDATED) — smithy_query(query="doc", shape="GetHLSStreamingSessionURLInput", member="PlaybackMode"):
> "LIVE_REPLAY: For sessions of this type, the HLS media playlist is updated similarly to how it is updated for LIVE mode except that it starts by including fragments from a given start time. Instead of fragments being added as they are ingested, fragments are added as the duration of the next fragment elapses. For example, if the fragments in the session are two seconds long, then a new fragment is added to the media playlist every two seconds."

The same rule, quoted by this crate on the predicate that classes LIVE_REPLAY as growing — src/state.rs:1076-1095:
```
    /// True for the two LIVE modes, whose playlist the model defines as a
    /// growing one: ... and
    /// of a `LIVE_REPLAY` range, "if the EndTimestamp is not set for
    /// LIVE_REPLAY mode then the session will continue to include newly
    /// ingested fragments until the session expires" ...
    pub fn grows(&self) -> bool {
        self.playback_mode != ON_DEMAND_PLAYBACK_MODE
    }
```

The selection that cannot grow — src/state.rs:593-610:
```
            // ON_DEMAND and LIVE_REPLAY serve the OLDEST fragments in the
            // requested range, up to the maximum.
            SessionWindow::Range { start_millis, end_millis } => SessionPlaylist {
                fragment_numbers: collapse_shared_start_timestamps(
                    self.fragments_in_range(origin, start_millis, end_millis),
                    origin,
                )
                .into_iter()
                .take(max_fragments)
                .map(|fragment| fragment.fragment_number.clone())
                .collect(),
                // The oldest fragment of the range is the first segment of the
                // playlist, so nothing has slid out in front of it.
                first_sequence: 0,
            },
```
against the LIVE branch of the same `match`, which spends the cap on the NEWEST — src/state.rs:580-591:
```
                let mut newest =
                    collapse_shared_start_timestamps(self.fragments.iter().collect(), origin);
                newest.sort_by_key(|fragment| epoch_millis(fragment.server_timestamp));
                let skip = newest.len().saturating_sub(max_fragments);
```

The cap that is reached by default — src/state.rs:659-667 and :916-923:
```
/// How many fragments a session's manifest or playlist carries when the request
/// names no maximum and the playback mode is LIVE or LIVE_REPLAY: "The default
/// is 5 fragments if PlaybackMode is LIVE or LIVE_REPLAY".
pub const DEFAULT_LIVE_SESSION_FRAGMENTS: usize = 5;
...
    let max_fragments = request
        .max_fragment_results
        .and_then(|max| usize::try_from(max).ok())
        .unwrap_or(if playback_mode == ON_DEMAND_PLAYBACK_MODE {
            DEFAULT_ON_DEMAND_SESSION_FRAGMENTS
        } else {
            DEFAULT_LIVE_SESSION_FRAGMENTS
        });
```

The other side — the answer tells the player it grows — src/service/playback_manifest.rs:118-125, :156-158 and :224-230:
```
    if !session.grows() {
        playlist.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    }
...
    if !session.grows() {
        playlist.push_str("#EXT-X-ENDLIST\n");
    }
...
        presentation = match session.grows() { true => "dynamic", false => "static" },
```

Test coverage: the growing-playlist test covers the LIVE path only — tests/all/archived_media_read_path.rs:3257 ("RULE: a streaming session whose playback mode defines a GROWING playlist — ...") and tests/all/get_dash_streaming_session_url.rs:1093 ("RULE (the same one, in the mode that selects by ingestion): a LIVE session ..."); nothing appends a fragment to a range that already holds the session's `max_fragments`, so the capped LIVE_REPLAY case is unpinned.

## Steps to Reproduce

1. Register a stream with DataRetentionInHours > 0 and ingest 5 fragments (numbers 1..5) with producer timestamps t0..t0+8s.
2. POST /getHLSStreamingSessionURL with {"StreamName": "<stream>", "PlaybackMode": "LIVE_REPLAY", "HLSFragmentSelector": {"FragmentSelectorType": "PRODUCER_TIMESTAMP", "TimestampRange": {"StartTimestamp": t0}}} — no MaxMediaPlaylistFragmentResults, so the documented default of 5 applies.
3. Follow the returned URL to the media playlist: it lists fragments 1..5, carries no #EXT-X-ENDLIST and no #EXT-X-PLAYLIST-TYPE:VOD, i.e. it tells the player to keep polling.
4. Ingest fragment 6 with a producer timestamp inside the (open-ended) range.
5. GET the same media playlist again: it still lists 1..5 with #EXT-X-MEDIA-SEQUENCE:0 — the re-selection at state.rs:599-606 takes the oldest 5 of the range again — although the mode is documented as one where "a new fragment is added to the media playlist every two seconds".
6. Repeat against /getDASHStreamingSessionURL: its type="dynamic" manifest's <SegmentTimeline> is likewise unchanged.
7. Contrast with PlaybackMode LIVE on the same archive: after ingesting fragment 6 the playlist re-selects the newest 5 (state.rs:580-591) and fragment 6 appears.
