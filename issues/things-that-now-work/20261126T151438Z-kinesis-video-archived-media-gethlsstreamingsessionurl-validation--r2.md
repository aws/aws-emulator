Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: ac0d009460b15386fb7f532991bd4fb2bd9b5be8

# Enforce the documented "EndTimestamp must be in the past" rule for a SERVER_TIMESTAMP session range

_Filed: 2026-11-26T15:14:38Z_
_Operation: GetHLSStreamingSessionURL | Class: validation_

> _Note: this issue was identified against the code base at commit `ac0d009460b15386fb7f532991bd4fb2bd9b5be8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`HLSTimestampRange$EndTimestamp` is documented with three requirements in one place: the end must be within 24 hours of `StartTimestamp`, it must be later than `StartTimestamp`, and "If FragmentSelectorType for the request is SERVER_TIMESTAMP, this value must be in the past." The shared helper `crate::state::session_parameters`, which both streaming-session handlers call, implements the first two — quoting both sentences in its comments — and never checks the third. An ON_DEMAND or LIVE_REPLAY request whose `FragmentSelectorType` is `SERVER_TIMESTAMP` and whose `EndTimestamp` lies in the future is accepted and a session is minted over a window the model says may not be requested, instead of `InvalidArgumentException` (400).

The same sentence appears verbatim on `DASHTimestampRange$EndTimestamp` (so `GetDASHStreamingSessionURL`, which calls the same helper, has the same hole) and on `ClipTimestampRange$EndTimestamp`, which `GetClip` checks with its own inline copy of the other two clauses (src/ops/get_clip.rs:371-378) and likewise skips. No comment, constant or test in the crate states a reason for leaving this clause out: `crate::state::MAX_RANGE_SPAN_MILLIS` and `TIMESTAMP_RANGE_ORDER_MESSAGE` document only the two clauses that are implemented.

## Evidence

Model — `smithy_query(query="doc", shape="HLSTimestampRange", member="EndTimestamp")`: "The end of the timestamp range for the requested media. This value must be within 24 hours of the specified StartTimestamp, and it must be later than the StartTimestamp value." / "If FragmentSelectorType for the request is SERVER_TIMESTAMP, this value must be in the past." (`DASHTimestampRange$EndTimestamp` and `ClipTimestampRange$EndTimestamp` carry the same sentence.)

The helper implements the two neighbouring clauses and stops — src/state.rs:952-967:
```
        // STRICTER than the `TimestampRange` of `ListFragments` ... both streaming ranges' own documentation says the
        // end "must be later than the StartTimestamp value" ...
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
        }
```
Nothing in `session_parameters` (src/state.rs:901-982) reads a clock, and the handler hands the bounds straight in — src/ops/get_hls_streaming_session_url.rs:348-355:
```
            range: selector.and_then(|s| s.timestamp_range.as_ref()).map(|range| {
                SelectedTimestampRange {
                    start_millis: range.start_timestamp.as_ref().map(date_time_millis),
                    end_millis: range.end_timestamp.as_ref().map(date_time_millis),
                }
            }),
```
Test coverage: tests/all/archived_media_read_path.rs:2136-2193 (`a_range_wider_than_the_documented_window_is_refused_in_every_operation_that_bounds_it`) pins the order and 24-hour clauses, and every row it builds uses `"FragmentSelectorType": "PRODUCER_TIMESTAMP"` (tests/all/archived_media_read_path.rs:2112), so no test states a reason for admitting a future `SERVER_TIMESTAMP` end.

## Steps to Reproduce

1. Seed one ACTIVE stream with retention > 0 holding fragments around instant T.
2. POST /getHLSStreamingSessionURL with `PlaybackMode: ON_DEMAND` and `HLSFragmentSelector: {FragmentSelectorType: "SERVER_TIMESTAMP", TimestampRange: {StartTimestamp: T, EndTimestamp: <now + 1 hour>}}` (span under 24 hours, end later than start).
3. Observe HTTP 200 and a minted session, although `HLSTimestampRange$EndTimestamp` documents that for `SERVER_TIMESTAMP` "this value must be in the past" — expected HTTP 400 `InvalidArgumentException`.
4. Repeat against /getDASHStreamingSessionURL (same shared helper) with `DASHFragmentSelector` for the same result.
