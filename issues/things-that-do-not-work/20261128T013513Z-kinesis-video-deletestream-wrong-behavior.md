Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 86895b449138760999bdbe2446231d266f2d8819

# DeleteStream ignores the media-storage mapping its sibling delete is interlocked on, leaving a channel pointing at a removed stream

_Filed: 2026-11-28T01:35:13Z_
_Operation: DeleteStream | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `86895b449138760999bdbe2446231d266f2d8819`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A signaling channel's enabled media-storage configuration is the ONE place this crate holds the channel↔stream mapping (`state::mapped_resources`). Two handlers already guard it: `UpdateMediaStorageConfiguration` refuses a stream another channel already holds, and `DeleteSignalingChannel` refuses to delete a channel whose configuration is still `ENABLED`. `DeleteStream` consults neither `mapped_resources` nor the channel index: it marks the stream `DELETING` and `settle_clock` then removes the record outright, while the channel keeps `media_storage_status = ENABLED` and `media_storage_stream_arn = Some(<ARN of the removed stream>)`. The persisted state is then self-contradictory: `DescribeMediaStorageConfiguration` reports an `ENABLED` configuration whose `StreamARN` resolves to nothing, `DescribeMappedResourceConfiguration` cannot report the mapping at all (its stream lookup 404s), and `GetSignalingChannelEndpoint` keeps granting the `WEBRTC` endpoint on the strength of that dangling configuration.

## Evidence

The interlock the crate applies on the other side of the same mapping — src/ops/delete_signaling_channel.rs:188-203:
```
        // A channel still linked to a stream is ingesting into it, so the delete is
        // refused while its media-storage configuration is ENABLED — the interlock an
        // operator or IaC flow relies on, and one of the cases
        // `ResourceInUseException` is modeled for ...
        if channel.media_storage_status == crate::state::MEDIA_STORAGE_ENABLED {
            return Err(DeleteSignalingChannelErrorModeled::ResourceInUseException(
```
The mapping is held once, in the channel record — src/state.rs:608-613:
```
    /// A mapping exists in exactly one direction and is held once: a signaling channel's
    /// media-storage configuration names the stream its data is stored in ... so the resources mapped TO a stream are the channels whose enabled
    /// configuration points at it
```
The path that skips it — src/ops/delete_stream.rs:241-244 (the only gates above it are the `DELETING` status check at line 211 and the `CurrentVersion` check at line 222; `mapped_resources` is never called in this file):
```
        stream.status = crate::state::STATUS_DELETING.to_string();
        stream.activates_at = None;
        stream.deletes_at =
            Some(::std::time::Instant::now() + crate::state::STREAM_DELETION_SPAN);
```
and the removal that strands the reference — src/state.rs:574-576:
```
        for (stream_name, stream_arn) in due {
            self.streams.remove(&stream_name);
            self.stream_arn_index.remove(&stream_arn);
```
(nothing in `settle_clock` touches `signaling_channels[*].media_storage_stream_arn`).

No test covers the combination: tests/all/stream_configuration.rs:734-899 exercises the mapping and its refusals but never deletes a mapped stream, and the DeleteStream tests (tests/all/stream_deletion_state.rs, tests/all/stream_record_state_model.rs) never create a mapping.

## Steps to Reproduce

1. POST /createStream {"StreamName":"ingest-stream","DataRetentionInHours":24} and POST /createSignalingChannel {"ChannelName":"ingest-channel"}; wait for both to become ACTIVE.
2. POST /updateMediaStorageConfiguration {"ChannelARN":<channel>,"MediaStorageConfiguration":{"Status":"ENABLED","StreamARN":<stream>}} — 200.
3. POST /deleteStream {"StreamARN":<stream>} — 200; wait past crate::state::STREAM_DELETION_SPAN so settle_clock removes the record (POST /describeStream now 404 ResourceNotFoundException).
4. POST /describeMediaStorageConfiguration {"ChannelARN":<channel>} — observe {"Status":"ENABLED","StreamARN":<the deleted stream's ARN>}: the channel still points at a stream the store no longer holds.
5. POST /getSignalingChannelEndpoint {"ChannelARN":<channel>,"SingleMasterChannelEndpointConfiguration":{"Protocols":["WEBRTC"],"Role":"MASTER"}} — still answered 200 off that dangling configuration, and POST /deleteSignalingChannel {"ChannelARN":<channel>} is still refused 400 ResourceInUseException "Disable MediaStorageConfiguration to delete the channel".
