Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# StartEdgeConfigurationUpdate never raises its modeled NoDataRetentionException: the zero-retention gate its sibling applies is skipped

_Filed: 2026-11-27T23:24:48Z_
_Operation: StartEdgeConfigurationUpdate | Class: validation_

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartEdgeConfigurationUpdate` declares `NoDataRetentionException`, whose documented trigger is a stream whose data retention equals zero. The handler gates on the edge configuration's sync status (`ResourceInUseException`) but performs no retention check, so a stream created with `DataRetentionInHours: 0` has an edge configuration created, moved to `SYNCING`, and reported back with a 200.

The crate implements this exact check for the same error class in `UpdateMediaStorageConfiguration`, and `state.rs` states the refusal sentence once for the whole service. This handler is a path that needs that check and does not call it, so a modeled, deterministic failure cannot be produced on this surface.

## Evidence

Modeled error and documented trigger: `smithy_query(query="doc", shape="NoDataRetentionException")` → "<p>The Stream data retention in hours is equal to zero.</p>"; declared on this operation at `src/ops/start_edge_configuration_update.rs:103`:
```
//! * `NoDataRetentionException` (HTTP 400) — The Stream data retention in hours is equal to
```
and `src/ops/start_edge_configuration_update.rs:135`:
```
    NoDataRetentionException(NoDataRetentionException),
```

The handler's only gate is the sync status — `src/ops/start_edge_configuration_update.rs:211-236`:
```
let stream = store
    .resolve_mut(input.stream_name.as_deref(), input.stream_arn.as_deref())
    .map_err(lookup_error)?;

// "You will have to wait for the sync status to reach a terminal state such as:
// IN_SYNC, or SYNC_FAILED, before using this API again. ..."
if stream
    .edge
    .as_ref()
    .is_some_and(|edge| edge.sync_status == crate::state::SYNC_STATUS_SYNCING)
```
followed by `stream.edge = Some(edge.clone());` at line 256 — no reference to `data_retention_in_hours` anywhere in the file.

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
and the service-wide sentence at `src/state.rs:199-206` ("How this service words a `NoDataRetentionException`.").

Test coverage: the edge round-trip test (`tests/all/stream_configuration.rs:908`, `an_edge_configuration_round_trips_and_reaches_its_terminal_sync_status`) uses a stream created with `"DataRetentionInHours": 24` (line 913), so no test pins acceptance on a zero-retention stream.

## Steps to Reproduce

1. POST /createStream with {"StreamName": "no-retention-edge", "DataRetentionInHours": 0}
2. POST /startEdgeConfigurationUpdate with {"StreamName": "no-retention-edge", "EdgeConfig": {"HubDeviceArn": "arn:aws:iot:us-east-1:000000000000:thing/hub", "RecorderConfig": {"MediaSourceConfig": {"MediaUriSecretArn": "arn:aws:secretsmanager:us-east-1:000000000000:secret:rtsp", "MediaUriType": "RTSP_URI"}}}} — observe HTTP 200 with SyncStatus SYNCING
3. POST /describeEdgeConfiguration for the same stream — the configuration is reported; the operation's modeled NoDataRetentionException is never reachable
