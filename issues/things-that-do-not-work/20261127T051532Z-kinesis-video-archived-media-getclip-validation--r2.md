Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1e1dd59f235ae7a2dd3040d21f370129bbbf8e9f

# GetClip accepts a SERVER_TIMESTAMP clip range whose EndTimestamp is not in the past

_Filed: 2026-11-27T05:15:32Z_
_Operation: GetClip | Class: validation_

> _Note: this issue was identified against the code base at commit `1e1dd59f235ae7a2dd3040d21f370129bbbf8e9f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ClipTimestampRange$EndTimestamp` carries three rules in one documented sentence: the end must be within 24 hours of the start, it must be later than the start, AND "If FragmentSelectorType for the request is SERVER_TIMESTAMP, this value must be in the past." The handler implements the first two of the three inline — and its comment literally calls the span rule "The other half of that same sentence" — but never checks the third, so a clip request whose `FragmentSelectorType` is `SERVER_TIMESTAMP` and whose `EndTimestamp` lies in the future is accepted instead of being refused as a bad argument (`InvalidArgumentException`, "A specified parameter exceeds its restrictions, is not supported, or can't be used"). The companion clause on `ClipTimestampRange$StartTimestamp` — "If FragmentSelectorType is SERVER_TIMESTAMP, the StartTimestamp must be later than the stream head" — is likewise unimplemented: `grep -n 'in the past' src/` matches nothing in the crate, and no comment, test or doc in the crate states a reason for leaving these clauses out.

## Evidence

Smithy model, `ClipTimestampRange$EndTimestamp` (smithy_query query="shape" shape="ClipTimestampRange"): "This value must be within 24 hours of the specified StartTimestamp, and it must be later than the StartTimestamp value. If FragmentSelectorType for the request is SERVER_TIMESTAMP, this value must be in the past." And `ClipTimestampRange$StartTimestamp`: "If FragmentSelectorType is SERVER_TIMESTAMP, the StartTimestamp must be later than the stream head."

Handler, services/service-com-amazonaws-kinesisvideoarchivedmedia-awsacuityreader/src/ops/get_clip.rs:363-375 — the only two checks the range gets:
```
            let start_millis = date_time_millis(&selector.timestamp_range.start_timestamp);
            let end_millis = date_time_millis(&selector.timestamp_range.end_timestamp);
            // `ClipTimestampRange$EndTimestamp` "must be later than the
            // StartTimestamp value", so a range that ends where it starts is as
            // much a refusal as one that ends before it.
            if end_millis <= start_millis {
                return Err(invalid(TIMESTAMP_RANGE_ORDER_MESSAGE.to_string()));
            }
            // The other half of that same sentence: the end "must be within 24
            // hours of the specified StartTimestamp".
            if !range_within_max_span(start_millis, end_millis) {
                return Err(invalid(TIMESTAMP_RANGE_SPAN_MESSAGE.to_string()));
            }
```
Nothing after that line reads the clock: the next statement is `let in_range = stream.fragments_in_range(origin, start_millis, end_millis);` (get_clip.rs:377).

No crate-side justification: grep for `in the past|must be in the past` over `src/` returns only unrelated matches (state.rs:575 "fragments ingested in the past"). Test coverage of the neighbouring clauses exists and is unaffected — tests/all/archived_media_read_path.rs:4698 `the_documented_span_refusal_holds_however_the_range_is_addressed_and_placed` and :4862 `a_range_on_or_inside_its_documented_span_is_answered_by_the_archive` use ranges based at T0_SECS = 1_761_000_000 (tests/all/archived_media_read_path.rs:44, "an instant well in the past"), so no existing row pins a future end timestamp as acceptable.

## Steps to Reproduce

1. Seed a stream with archived fragments and 24h retention (as tests/all/archived_media_read_path.rs::seeded() does).
2. POST /getClip with {StreamName: 'demo-stream', ClipFragmentSelector: {FragmentSelectorType: 'SERVER_TIMESTAMP', TimestampRange: {StartTimestamp: <now - 60>, EndTimestamp: <now + 3600>}}}.
3. Observe the request is processed as a normal clip request (200, or 404 when the range selects no fragment) instead of being refused with InvalidArgumentException for an EndTimestamp that is not in the past.
