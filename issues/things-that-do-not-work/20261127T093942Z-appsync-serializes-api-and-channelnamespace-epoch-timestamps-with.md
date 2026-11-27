Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-channel-namespace-code-handlers-size-limit
Issue-Step: setup#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-channel-namespace-code-handlers-size-limit appsync-create-api-empty-tag-key appsync-channel-namespace-name-pattern-rejections appsync-delete-channel-namespace-name-length-bounds appsync-event-api-update-name-length-bounds
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|CreateApi|2xx->2xx|$.api.created
Issue-Key: appsync|UpdateChannelNamespace|2xx->2xx|$.channelNamespace.created
Issue-Key: appsync|UpdateChannelNamespace|2xx->2xx|$.channelNamespace.lastModified
Issue-Key: appsync|GetChannelNamespace|2xx->2xx|$.channelNamespace.created
Issue-Key: appsync|GetChannelNamespace|2xx->2xx|$.channelNamespace.lastModified
Issue-Key: appsync|CreateChannelNamespace|2xx->2xx|$.channelNamespace.created
Issue-Key: appsync|CreateChannelNamespace|2xx->2xx|$.channelNamespace.lastModified
Issue-Key: appsync|UpdateApi|2xx->2xx|$.api.created
Issue-Key: appsync|GetApi|2xx->2xx|$.api.created
Issue-Commit: 5d024fed191bbfdb41e9accab576b1e1cc16d383

# AppSync serializes Api and ChannelNamespace epoch timestamps with sub-second precision

> _Note: this issue was identified against the code base at commit `5d024fed191bbfdb41e9accab576b1e1cc16d383`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApi

## Description
Every AppSync response that carries an Api or ChannelNamespace document serializes its epoch-second timestamp members with a fractional (microsecond) component, which does not match the shape of the recorded AWS value for those members. The member values themselves are per-run clock times and are not asserted by the corpus — what diverges is the serialized number's shape, on six operations. 6 causes in this issue; all six must be closed before submitting.

AWS (common to all six): the appsync service is restJson1 and both Api$created and ChannelNamespace$created/$lastModified target the plain Timestamp shape with no @timestampFormat, so the protocol default epoch-seconds applies and the member is serialized as a JSON number. Recorded behavior only: the corpus does not assert the instant (it is a clock value), it asserts the member's shape, and the emulator's value fails that assertion. Hypothesis: AWS emits these epoch-second numbers at whole-second granularity, whereas the emulator emits a float with seven fractional digits (e.g. 1795771775.1714046).
Emulator (common to all six): the handler writes the system clock straight through as a fractional epoch, so every one of these documents carries a sub-second value.
Why (common to all six): a client that round-trips the document, keys a cache or an idempotency check on the serialized timestamp, or deserializes into a whole-second type sees a value shaped differently from AWS's; and the emulator exposes a resolution AWS does not publish, so equality against a re-read document behaves differently.
Fix (common to all six): the timestamp serialization for these members — truncate to the protocol's epoch-second granularity in the Api/ChannelNamespace serializer (one shared projection, not six handlers), so Causes 1-6 close together but each is replayed separately.

=== Cause 1 of 6: CreateApi response Api$created ===
Traces: appsync-channel-namespace-code-handlers-size-limit#setup#1 (cluster also covers appsync-create-api-empty-tag-key#task#2).

=== Cause 2 of 6: UpdateChannelNamespace response ChannelNamespace$created and $lastModified ===
Traces: appsync-channel-namespace-code-handlers-size-limit#task#3.

=== Cause 3 of 6: GetChannelNamespace response ChannelNamespace$created and $lastModified ===
Traces: appsync-channel-namespace-code-handlers-size-limit#task#4 (cluster also covers #task#7, appsync-channel-namespace-name-pattern-rejections#task#6 and appsync-delete-channel-namespace-name-length-bounds#task#5).

=== Cause 4 of 6: CreateChannelNamespace response ChannelNamespace$created and $lastModified ===
Traces: appsync-channel-namespace-name-pattern-rejections#task#5.

=== Cause 5 of 6: UpdateApi response Api$created ===
Traces: appsync-event-api-update-name-length-bounds#task#2.

=== Cause 6 of 6: GetApi response Api$created ===
Traces: appsync-event-api-update-name-length-bounds#task#3 (cluster also covers #task#6).

## Evidence
All six clusters are 200/200 with the only divergence on the timestamp member, reported as a shape divergence against a value the corpus does not assert literally:
Cause 1 — CreateApi: $.api.created value_mismatch (expected '<shaped-not-asserted>', got 1795771775.1714046).
Cause 2 — UpdateChannelNamespace: $.channelNamespace.created got 1795771775.2317617; $.channelNamespace.lastModified got 1795771775.2418585.
Cause 3 — GetChannelNamespace: $.channelNamespace.created and $.channelNamespace.lastModified, same shape.
Cause 4 — CreateChannelNamespace: $.channelNamespace.created and $.channelNamespace.lastModified got 1795771872.661911.
Cause 5 — UpdateApi: $.api.created got 1795771783.4499881.
Cause 6 — GetApi: $.api.created got 1795771783.4499881.
(The specific instants above are this run's clock and are not the requirement; the fractional component is.)

## Steps to Reproduce
CreateApi(name=<valid>, eventConfig=<valid>) and inspect api.created; UpdateApi / GetApi on the same api and inspect api.created; CreateChannelNamespace / UpdateChannelNamespace / GetChannelNamespace on that api and inspect channelNamespace.created and channelNamespace.lastModified. Each must be serialized as a restJson1 epoch-second number at whole-second granularity (no fractional part), the same shape AWS records for these members.
