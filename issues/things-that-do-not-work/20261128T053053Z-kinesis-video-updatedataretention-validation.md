Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# UpdateDataRetention mutates a stream already marked DELETING, which DeleteStream refuses for that exact state

_Filed: 2026-11-28T05:30:53Z_
_Operation: UpdateDataRetention | Class: validation_

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeleteStream` gates its mutation on the stream's status: a record already in `DELETING` is refused 400 `ResourceInUseException` with the crate's `STREAM_NOT_ACTIVE_MESSAGE`, whose doc cites the `ResourceInUseException` shape documentation for precisely this condition. `UpdateDataRetention` — which declares the same `ResourceInUseException` and is a mutation of the same record — applies no status gate at all: it resolves the stream, checks `CurrentVersion`, then writes a new retention AND mints a fresh `Version` on a record whose removal the clock already owes. The caller gets a 200 and a silently discarded write: `deletes_at` is unchanged, so `settle_clock` drops the stream (and the accepted update) a moment later, and in the meantime the stream's `Version` has been re-minted, invalidating the token the caller's pending delete was holding. The same omission is in `UpdateNotificationConfiguration`, `UpdateImageGenerationConfiguration` and `UpdateMediaStorageConfiguration`'s stream side (which does gate, on ACTIVE) — this issue tracks the retention path, the one that re-mints the version. The gate at stake is only the already-`DELETING` one: a stream still in `CREATING` must stay mutable, as the fixed DeleteStream issue established ("reserve ResourceInUseException/400 for a stream that is already DELETING").

## Evidence

Sibling that applies the gate — services/service-com-amazonaws-kinesisvideo-kinesisvideo-20170930/src/ops/delete_stream.rs:201-218:
```
// The state gate, before the version guard: the ONE state that blocks marking a
// stream for deletion is a deletion already under way, and that is the condition
// `ResourceInUseException` names ("if the provided input StreamARN or ChannelARN
// is not in Active status … The DescribeStream … API to determine the status of
// the resource") ...
if stream.status == crate::state::STATUS_DELETING {
    return Err(DeleteStreamErrorModeled::ResourceInUseException(
        ResourceInUseException {
            message: Some(crate::state::STREAM_NOT_ACTIVE_MESSAGE.to_string()),
```
The rule stated once, for streams in general — src/state.rs:234-240:
```
/// How this service words a `ResourceInUseException` for a stream that EXISTS but is
/// not in `ACTIVE` status — the case the shape is documented for ("if the provided
/// input StreamARN or ChannelARN is not in Active status … The DescribeStream or
/// DescribeSignalingChannel API to determine the status of the resource") ...
pub const STREAM_NOT_ACTIVE_MESSAGE: &str = "The requested stream is not active.";
```
The path that skips it — src/ops/update_data_retention.rs:201-205 resolves and then goes straight to the version guard, with no reference to `stream.status` anywhere in the handler:
```
let stream = store
    .resolve(input.stream_name.as_deref(), input.stream_arn.as_deref())
    .map_err(lookup_error)?;
let stream_name = stream.stream_name.clone();
let stored_version = stream.version.clone();
```
and it writes regardless — update_data_retention.rs:276-281:
```
let version = store.mint_version();
let Some(stream) = store.streams.get_mut(&stream_name) else {
    return Err(lookup_error(crate::state::LookupError::NotFound));
};
stream.data_retention_in_hours = i32::try_from(updated).unwrap_or(i32::MAX);
stream.version = version;
```
`UpdateDataRetention` declares the error that covers this state — OPERATIONS.md:210: "**Errors:** ... `ResourceInUseException (HTTP 400)` ...".
Test coverage: tests/all/stream_record_state_model.rs:196-211 pins the DELETING refusal for a second `DeleteStream` (400, ResourceInUseException, "The requested stream is not active."); no test issues an UpdateDataRetention against a DELETING stream, so the refusal can be added without touching existing assertions.

## Steps to Reproduce

1. POST /createStream {"StreamName": "doomed", "DataRetentionInHours": 2} and read the Version from /describeStream.
2. POST /deleteStream {"StreamARN": "<arn>"} → 200; /describeStream now reports Status DELETING (tests/all/stream_record_state_model.rs:170-180).
3. POST /deleteStream again → 400 ResourceInUseException "The requested stream is not active." (the gate at delete_stream.rs:211).
4. POST /updateDataRetention {"StreamARN": "<arn>", "CurrentVersion": "<version>", "Operation": "INCREASE_DATA_RETENTION", "DataRetentionChangeInHours": 1} → observe HTTP 200 and, via /describeStream, a changed DataRetentionInHours and a NEW Version on a record the clock is about to remove; wait out STREAM_DELETION_SPAN and the accepted update is gone with the stream.
