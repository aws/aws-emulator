Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# Re-pointing an already-mapped channel to a different stream is accepted and silently drops the old mapping, instead of the modelled ResourceInUseException

_Filed: 2026-11-28T00:31:53Z_
_Operation: UpdateMediaStorageConfiguration | Class: error-response_

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateMediaStorageConfiguration` implements only HALF of the mapping conflict its `ResourceInUseException` is documented for. The model's sentence covers two mapping directions — "the input StreamARN **or ChannelARN** … is already mapped to a different Kinesis Video Stream resource" — and the handler checks only the stream side: it refuses when ANOTHER channel already points at the stream being claimed (`store.mapped_resources(&stream_arn)`), but never reads the channel's OWN current mapping (`channel.media_storage_stream_arn`).

So an ENABLED channel C already mapped to stream A can be re-enabled onto stream B: the call is answered 200, `channel.media_storage_stream_arn` is overwritten, and stream A's mapping disappears from state with no refusal and no trace — `DescribeMappedResourceConfiguration` for A then reports an empty list, exactly as if the caller had disabled it. The documented remedy list in the same error makes the channel side explicit ("The DescribeMediaStorageConfiguration API to determine what the stream given channel is mapped to"), so the condition the handler skips is the one that error names first.

The crate's own message constant documents the narrower scope it implements, which is where the half-implementation shows: `MEDIA_STORAGE_IN_USE_MESSAGE`'s doc enumerates "the stream is already mapped to another channel, or either resource is not `ACTIVE`" — the channel-already-mapped case of the model's sentence is simply absent.

## Evidence

Model (smithy_query query="doc" shape="ResourceInUseException"), also reproduced verbatim in the handler's own header at `src/ops/update_media_storage_configuration.rs:72-78`:
```
//! * `ResourceInUseException` (HTTP 400) — When the input StreamARN or ChannelARN in
//!   CLOUD_STORAGE_MODE is already mapped to a different Kinesis Video Stream resource, or if
//!   the provided input StreamARN or ChannelARN is not in Active status, try one of the
//!   following : The DescribeMediaStorageConfiguration API to determine what the stream given
//!   channel is mapped to. The DescribeMappedResourceConfiguration API to determine the
//!   channel that the given stream is mapped to.
```

The only mapping check the handler makes — `src/ops/update_media_storage_configuration.rs:264-280` (stream side only):
```
            if store
                .mapped_resources(&stream_arn)
                .into_iter()
                .filter_map(|item| item.arn)
                .any(|arn| arn != channel_arn)
            {
                return Err(
                    UpdateMediaStorageConfigurationErrorModeled::ResourceInUseException(
```

The unconditional overwrite that follows — `src/ops/update_media_storage_configuration.rs:297-298`:
```
        channel.media_storage_status = input.media_storage_configuration.status;
        channel.media_storage_stream_arn = mapped_stream_arn;
```
(no read of `channel.media_storage_stream_arn` anywhere in the file, although the field exists and is documented in `src/state.rs:483-485`: "The stream the media-storage configuration points at, `None` while it is disabled.")

The crate's own statement of the scope it implements — `src/state.rs:181-188`:
```
/// How this service words the `ResourceInUseException` of a media-storage UPDATE.
///
/// The failure class `UpdateMediaStorageConfiguration` declares it for — the stream is
/// already mapped to another channel, or either resource is not `ACTIVE` — carries ONE
/// sentence, entirely fixed
```

Test coverage: `tests/all/stream_configuration.rs:812-830` (test fn `a_mapped_resource_is_reported_from_the_mapping_that_created_it`, line 735) pins ONLY the stream-side refusal — "A stream holds at most one channel's media … so a second channel claiming it is refused" — and `tests/all/stream_configuration.rs:871-898` pins the DISABLED path. No test re-points an already-ENABLED channel at a second stream, so nothing justifies the current acceptance.

## Steps to Reproduce

1. POST /createStream {"StreamName":"stream-a","DataRetentionInHours":24} and POST /createStream {"StreamName":"stream-b","DataRetentionInHours":24}
2. POST /createSignalingChannel {"ChannelName":"ingest-channel"}; wait out the activation span so all three are ACTIVE
3. POST /updateMediaStorageConfiguration {"ChannelARN":<channel>,"MediaStorageConfiguration":{"Status":"ENABLED","StreamARN":<stream-a arn>}} — 200
4. POST /describeMappedResourceConfiguration {"StreamName":"stream-a"} — the channel is reported as mapped
5. POST /updateMediaStorageConfiguration {"ChannelARN":<channel>,"MediaStorageConfiguration":{"Status":"ENABLED","StreamARN":<stream-b arn>}} — expected 400 ResourceInUseException (the input ChannelARN is already mapped to a different Kinesis Video Stream resource); actual 200
6. POST /describeMappedResourceConfiguration {"StreamName":"stream-a"} — now an empty MappedResourceConfigurationList: stream-a's mapping was dropped without any refusal
