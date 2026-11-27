Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# UpdateImageGenerationConfiguration never raises its modeled NoDataRetentionException: the zero-retention gate its sibling applies is skipped

_Filed: 2026-11-27T23:24:48Z_
_Operation: UpdateImageGenerationConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateImageGenerationConfiguration` declares `NoDataRetentionException` and the error shape documents its one trigger — the stream's data retention being zero — but the handler resolves the stream and writes the configuration unconditionally. A stream created with `DataRetentionInHours: 0` has an image-generation configuration stored and reported back by `DescribeImageGenerationConfiguration` with a 200.

The crate already implements exactly this gate for the same error class: `UpdateMediaStorageConfiguration` refuses when `stream.data_retention_in_hours <= 0`, using the service-wide sentence `state::ZERO_DATA_RETENTION_MESSAGE`, whose doc comment introduces it as "How this service words a `NoDataRetentionException`" — i.e. the wording is stated once for the whole service, not for one operation. This handler is a path that needs that same check and does not perform it, so a modeled, deterministic, state-driven failure can never be produced on this surface.

## Evidence

Modeled error and its documented trigger: `smithy_query(query="doc", shape="NoDataRetentionException")` → "<p>The Stream data retention in hours is equal to zero.</p>", and the operation declares it — `src/ops/update_image_generation_configuration.rs:69`:
```
//! * `NoDataRetentionException` (HTTP 400) — The Stream data retention in hours is equal to
```
`src/ops/update_image_generation_configuration.rs:101`:
```
    NoDataRetentionException(NoDataRetentionException),
```

The whole mutation, with no retention check — `src/ops/update_image_generation_configuration.rs:174-181`:
```
let stream = store
    .resolve_mut(input.stream_name.as_deref(), input.stream_arn.as_deref())
    .map_err(lookup_error)?;

// The request REPLACES whatever the stream carried, and a request carrying no
// structure clears it: "If null, the configuration will be deleted from the
// stream" (`ImageGenerationConfiguration` docs).
stream.image_generation_configuration = input.image_generation_configuration;
```

Sibling in the same crate that applies the gate — `src/ops/update_media_storage_configuration.rs:226-241`:
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
                ),
```
and the sentence is stated service-wide — `src/state.rs:199-206`:
```
/// How this service words a `NoDataRetentionException`.
...
pub const ZERO_DATA_RETENTION_MESSAGE: &str = "The stream data retention is set to 0 hours.";
```

Test coverage: the zero-retention refusal is pinned only for the media-storage path (`tests/all/stream_configuration.rs:746-747` creates `no-retention-stream` with `"DataRetentionInHours": 0` and lines 832-847 assert `NoDataRetentionException`). The configuration round-trip test uses a stream created with `"DataRetentionInHours": 24` (`tests/all/stream_configuration.rs:591`, fn `a_written_configuration_reads_back_and_a_cleared_one_reads_absent` at :585), so no test pins acceptance on a zero-retention stream.

## Steps to Reproduce

1. POST /createStream with {"StreamName": "no-retention", "DataRetentionInHours": 0}
2. POST /updateImageGenerationConfiguration with {"StreamName": "no-retention", "ImageGenerationConfiguration": {"Status": "ENABLED", "ImageSelectorType": "SERVER_TIMESTAMP", "DestinationConfig": {"Uri": "s3://b/p", "DestinationRegion": "us-east-1"}, "SamplingInterval": 3000, "Format": "JPEG"}} — observe HTTP 200
3. POST /describeImageGenerationConfiguration for the same stream — the configuration is reported, though the operation's modeled NoDataRetentionException is the answer the crate gives for a zero-retention stream on /updateMediaStorageConfiguration
