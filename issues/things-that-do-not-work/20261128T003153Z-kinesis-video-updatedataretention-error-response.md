Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# UpdateDataRetention mutates a stream marked DELETING and never raises the ResourceInUseException the crate reserves for a non-ACTIVE stream

_Filed: 2026-11-28T00:31:53Z_
_Operation: UpdateDataRetention | Class: error-response_

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateDataRetention` declares `ResourceInUseException` and never constructs it: the handler resolves the stream, checks only `CurrentVersion` and the retention bounds, and then writes. It reads `stream.status` nowhere.

The crate already owns the rule for exactly this condition. `state::STREAM_NOT_ACTIVE_MESSAGE` exists solely to word "a `ResourceInUseException` for a stream that EXISTS but is not in `ACTIVE` status", and `DeleteStream` applies it: a stream whose removal is already under way (`DELETING`) is refused 400 rather than mutated. `UpdateMediaStorageConfiguration` applies the same gate for both resources it touches (`stream.status != STATUS_ACTIVE || channel_status != STATUS_ACTIVE`).

The observable consequence is a write that is accepted and then lost: a stream marked `DELETING` has a fixed removal instant (`Stream::deletes_at`, materialised by `settle_clock`), yet `UpdateDataRetention` answers 200 for it, stores the new retention and MINTS A NEW VERSION on a record that `settle_clock` then removes. A caller that reads the response is told the retention moved and is handed a version token for a stream that no longer exists moments later. The identical sibling handlers (`UpdateNotificationConfiguration`, `UpdateImageGenerationConfiguration`, `UpdateSignalingChannel`) share the same omission, but this one is the clearest because the record it writes is already scheduled for removal.

## Evidence

The crate's own rule, with the model sentence it comes from — `src/state.rs:234-240`:
```
/// How this service words a `ResourceInUseException` for a stream that EXISTS but is
/// not in `ACTIVE` status — the case the shape is documented for ("if the provided
/// input StreamARN or ChannelARN is not in Active status … The DescribeStream or
/// DescribeSignalingChannel API to determine the status of the resource"), as against
/// the absent stream [`STREAM_NOT_FOUND_MESSAGE`] reports.
pub const STREAM_NOT_ACTIVE_MESSAGE: &str = "The requested stream is not active.";
```

Sibling that applies it — `src/ops/delete_stream.rs:211-218`:
```
        if stream.status == crate::state::STATUS_DELETING {
            return Err(DeleteStreamErrorModeled::ResourceInUseException(
                ResourceInUseException {
                    message: Some(crate::state::STREAM_NOT_ACTIVE_MESSAGE.to_string()),
                },
            )
            .into());
        }
```
and `src/ops/update_media_storage_configuration.rs:249-262`:
```
            if stream.status != crate::state::STATUS_ACTIVE
                || channel_status != crate::state::STATUS_ACTIVE
            {
                return Err(
                    UpdateMediaStorageConfigurationErrorModeled::ResourceInUseException(
```

The path that skips it — `src/ops/update_data_retention.rs:195-281`: after `store.settle_clock()` the handler resolves the stream (line 201), compares `input.current_version` (line 211), bounds the result (lines 244-267) and writes:
```
        stream.data_retention_in_hours = i32::try_from(updated).unwrap_or(i32::MAX);
        stream.version = version;
```
No occurrence of `status`, `STATUS_ACTIVE`, `STATUS_DELETING` or `STREAM_NOT_ACTIVE_MESSAGE` in the file, and `UpdateDataRetentionErrorModeled::ResourceInUseException` (declared at `src/ops/update_data_retention.rs:129`) is never constructed.

The record it writes is one the clock owes a removal — `src/state.rs:312-314`:
```
    /// When the clock owes this stream its removal from the store, set by the delete
    /// that marked it for deletion; `None` while the stream is not being deleted.
    pub deletes_at: Option<Instant>,
```
set by `src/ops/delete_stream.rs:243-244` and acted on by `settle_clock` (`src/state.rs:569-576`).

Test coverage: `tests/all/stream_deletion_state.rs:212` (`deleting_a_stream_whose_removal_is_under_way_is_refused_as_in_use`) pins the DELETE-on-DELETING refusal only; `tests/all/stream_data_retention.rs:132-260` only updates ACTIVE streams. No test exercises UpdateDataRetention against a DELETING stream, so nothing pins the current acceptance.

## Steps to Reproduce

1. POST /createStream {"StreamName":"doomed","DataRetentionInHours":10}; wait out the activation span
2. POST /describeStream {"StreamName":"doomed"} — note Status ACTIVE and Version V
3. POST /deleteStream {"StreamARN":<arn>} — 200; the record is now Status DELETING with removal owed to the clock
4. POST /updateDataRetention {"StreamName":"doomed","CurrentVersion":V,"Operation":"INCREASE_DATA_RETENTION","DataRetentionChangeInHours":1} — expected 400 ResourceInUseException ("The requested stream is not active.", as DeleteStream answers for the same state); actual 200 with the retention moved and a fresh Version minted
5. POST /describeStream {"StreamName":"doomed"} — reports DataRetentionInHours 11 on a DELETING record; after the deletion span the stream is gone and the accepted write is lost
