Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# UpdateNotificationConfiguration never raises its modeled NoDataRetentionException: the zero-retention gate its sibling applies is skipped

_Filed: 2026-11-27T23:24:48Z_
_Operation: UpdateNotificationConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateNotificationConfiguration` declares `NoDataRetentionException`, whose documented trigger is a stream whose data retention is zero, but the handler resolves the stream and writes the notification configuration unconditionally. A stream created with `DataRetentionInHours: 0` gets a notification configuration stored and reported by `DescribeNotificationConfiguration` with a 200.

The same check exists in this crate for the same error class (`UpdateMediaStorageConfiguration`), and the refusal sentence is stated once for the whole service in `state.rs` as "How this service words a `NoDataRetentionException`". This handler is the path that needs it and skips it, so the modeled failure cannot be produced on this surface.

## Evidence

Modeled error and documented trigger: `smithy_query(query="doc", shape="NoDataRetentionException")` → "<p>The Stream data retention in hours is equal to zero.</p>"; declared on this operation at `src/ops/update_notification_configuration.rs:68`:
```
//! * `NoDataRetentionException` (HTTP 400) — The Stream data retention in hours is equal to
```
and `src/ops/update_notification_configuration.rs:100`:
```
    NoDataRetentionException(NoDataRetentionException),
```

The whole mutation, with no retention check — `src/ops/update_notification_configuration.rs:173-180`:
```
let stream = store
    .resolve_mut(input.stream_name.as_deref(), input.stream_arn.as_deref())
    .map_err(lookup_error)?;

// The request REPLACES whatever the stream carried, and a request carrying no
// structure clears it: "If the structure is null, the configuration will be
// deleted from the stream" (`NotificationConfiguration` on this input).
stream.notification_configuration = input.notification_configuration;
```

Sibling in the same crate that applies the gate — `src/ops/update_media_storage_configuration.rs:230-241`:
```
if stream.data_retention_in_hours <= 0 {
    return Err(
        UpdateMediaStorageConfigurationErrorModeled::NoDataRetentionException(
            NoDataRetentionException {
                message: Some(
                    crate::state::ZERO_DATA_RETENTION_MESSAGE.to_string(),
                ),
```
with the sentence stated service-wide at `src/state.rs:199-206` ("How this service words a `NoDataRetentionException`." … `pub const ZERO_DATA_RETENTION_MESSAGE: &str = "The stream data retention is set to 0 hours.";`).

Test coverage: `tests/all/stream_configuration.rs:585` (`a_written_configuration_reads_back_and_a_cleared_one_reads_absent`) drives the notification update against a stream created with `"DataRetentionInHours": 24` (line 591), so no existing test pins acceptance on a zero-retention stream; the zero-retention refusal is pinned only for media storage (lines 746-747 and 832-847).

## Steps to Reproduce

1. POST /createStream with {"StreamName": "no-retention-notify", "DataRetentionInHours": 0}
2. POST /updateNotificationConfiguration with {"StreamName": "no-retention-notify", "NotificationConfiguration": {"Status": "ENABLED", "DestinationConfig": {"Uri": "arn:aws:sns:us-east-1:000000000000:t"}}} — observe HTTP 200
3. POST /describeNotificationConfiguration for the same stream — the configuration is reported; the operation's modeled NoDataRetentionException is never reachable
