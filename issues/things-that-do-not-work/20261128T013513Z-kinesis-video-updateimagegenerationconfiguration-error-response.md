Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 86895b449138760999bdbe2446231d266f2d8819

# NoDataRetentionException is implemented in one handler only: the image-generation, notification and edge-configuration updates accept a zero-retention stream

_Filed: 2026-11-28T01:35:13Z_
_Operation: UpdateImageGenerationConfiguration | Class: error-response_

> _Note: this issue was identified against the code base at commit `86895b449138760999bdbe2446231d266f2d8819`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Four operations declare `NoDataRetentionException`, whose modelled condition is stated by the shape itself: "The Stream data retention in hours is equal to zero." The crate words that refusal once, in a service-wide constant (`state::ZERO_DATA_RETENTION_MESSAGE`, documented as "How this service words a `NoDataRetentionException`"), and raises it from exactly ONE of the four handlers — `UpdateMediaStorageConfiguration`, which reads the stored `data_retention_in_hours` for precisely this test. The other three handlers that declare the same error (`UpdateImageGenerationConfiguration`, `UpdateNotificationConfiguration`, `StartEdgeConfigurationUpdate`) never read the field at all: each resolves the stream and writes the configuration, so a stream whose retention is 0 — the default `CreateStream` applies when `DataRetentionInHours` is absent — gets a 200 and a persisted configuration, and `NoDataRetentionException` is unreachable on three of the four surfaces that model it.

## Evidence

The check, applied by one sibling — src/ops/update_media_storage_configuration.rs:226-241:
```
            // "In order for WebRTC Ingestion to work, the stream must have data
            // retention enabled" — a stream that persists nothing cannot store a
            // channel's media, which is the case `NoDataRetentionException` is modeled
            // for.
            if stream.data_retention_in_hours <= 0 {
                return Err(
                    UpdateMediaStorageConfigurationErrorModeled::NoDataRetentionException(
                        NoDataRetentionException {
                            message: Some(
                                crate::state::ZERO_DATA_RETENTION_MESSAGE.to_string(),
```
The refusal is stated as the SERVICE's, not one operation's — src/state.rs:199-206:
```
/// How this service words a `NoDataRetentionException`.
...
/// definition of the class.
pub const ZERO_DATA_RETENTION_MESSAGE: &str = "The stream data retention is set to 0 hours.";
```
The modelled trigger, from `smithy_query(query="shape", shape="NoDataRetentionException")`: `"doc": "<p>The Stream data retention in hours is equal to zero.</p>"`, declared on all four operations (OPERATIONS.md:217 UpdateImageGenerationConfiguration, :231 UpdateNotificationConfiguration, :175 StartEdgeConfigurationUpdate, :224 UpdateMediaStorageConfiguration): "`NoDataRetentionException (HTTP 400)`".

The three handlers that skip it, each writing straight after the lookup:
- src/ops/update_image_generation_configuration.rs:174-181: `let stream = store.resolve_mut(...)...; stream.image_generation_configuration = input.image_generation_configuration;`
- src/ops/update_notification_configuration.rs:173-180: `let stream = store.resolve_mut(...)...; stream.notification_configuration = input.notification_configuration;`
- src/ops/start_edge_configuration_update.rs:211-256: `let stream = store.resolve_mut(...)...; stream.edge = Some(edge.clone());`
Grep confirms `ZERO_DATA_RETENTION_MESSAGE` and `data_retention_in_hours <= 0` appear only in update_media_storage_configuration.rs.

No test pins the permissive behaviour: tests/all/stream_configuration.rs:908-916 creates the edge-configuration stream with `"DataRetentionInHours": 24`, and the zero-retention stream it does create ("no-retention-stream", line 744-749) is only ever used to assert the media-storage refusal (line 834-850).

## Steps to Reproduce

1. POST /createStream {"StreamName":"zero-retention"} (no DataRetentionInHours, so the handler stores 0 — create_stream.rs:254) and wait for ACTIVE.
2. POST /updateImageGenerationConfiguration {"StreamName":"zero-retention","ImageGenerationConfiguration":{...valid structure...}} — observe HTTP 200, and /describeImageGenerationConfiguration reporting the stored configuration, where the modelled NoDataRetentionException condition ("The Stream data retention in hours is equal to zero") holds.
3. POST /updateNotificationConfiguration {"StreamName":"zero-retention","NotificationConfiguration":{...}} — same 200.
4. POST /startEdgeConfigurationUpdate {"StreamName":"zero-retention","EdgeConfig":{...}} — same 200.
5. Contrast: POST /updateMediaStorageConfiguration enabling a channel onto that same stream — 400 NoDataRetentionException with "The stream data retention is set to 0 hours."
