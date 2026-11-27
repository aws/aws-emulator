Issue-Source: trace-sweep-divergence
Issue-Trace: kinesis-add-tags-to-stream-accepts-a-129-character-tag-key
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: kinesis-add-tags-to-stream-accepts-a-129-character-tag-key kinesis-create-stream-mode-outside-enum-rejected kinesis-create-stream-name-length-bounds kinesis-create-stream-record-distribution-strategy kinesis-decrease-retention-malformed-stream-id-rejected kinesis-decrease-retention-refuses-a-stream-name-outside-its-charset kinesis-enhanced-monitoring-metric-name-outside-enum-rejected kinesis-enhanced-monitoring-shard-level-metrics-count-bounds kinesis-record-distribution-strategy-enum-values kinesis-stop-encryption-key-id-length-limit stop-stream-encryption-type-outside-enum-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|DescribeStreamSummary|2xx->2xx|$.StreamDescriptionSummary.QueueCount
Issue-Commit: 40df241c1ce9889c9be50646fd3e82dae29dae7f

# DescribeStreamSummary omits StreamDescriptionSummary.QueueCount from every stream summary

> _Note: this issue was identified against the code base at commit `40df241c1ce9889c9be50646fd3e82dae29dae7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStreamSummary

## Description
=== Cause 1 of 1: the StreamDescriptionSummary the emulator serves has no QueueCount member ===
AWS: recorded behavior only — the on-disk Smithy model's StreamDescriptionSummary declares StreamName, StreamARN, StreamId, StreamStatus, StreamModeDetails, RetentionPeriodHours, StreamCreationTimestamp, EnhancedMonitoring, EncryptionType, KeyId, OpenShardCount, ConsumerCount, WarmThroughput, MaxRecordSizeInKiB, ChannelCount and RecordDistributionStrategy, but no QueueCount, so the model says nothing about this member and the recording is the authority. Recorded behavior: every successful DescribeStreamSummary response (HTTP 200) carries StreamDescriptionSummary.QueueCount, serialized as an integer alongside the other per-stream counters (ConsumerCount, ChannelCount), and its value for a stream with nothing attached is 0 — i.e. the counter is always present, not omitted when it is zero.
Emulator: HTTP 200 with an otherwise matching StreamDescriptionSummary in which QueueCount is absent entirely (observed None). This is the single largest cluster in the corpus: 23 replayed steps across 13 fixtures diverge on exactly this one missing key, on streams in every shape the corpus builds (provisioned and on-demand, encrypted and not, with and without enhanced monitoring), so the member is never emitted rather than being emitted conditionally.
Why: QueueCount is a counter clients read directly. In a typed SDK the member deserializes as absent/null where AWS always supplies a number, so code that reads or compares it (dashboards, capacity checks, a converge loop asserting a stream has no attached queues) either throws on the missing key or silently treats "no value" as a meaningful state. Because DescribeStreamSummary is the cheapest way to inspect a stream, this absence is visible on nearly every stream inspection the corpus performs.
Fix: the StreamDescriptionSummary projection in the DescribeStreamSummary handler's output serializer (and the stream state it reads from) — carry a queue counter per stream and always serialize it, defaulting to 0.
Traces: kinesis-add-tags-to-stream-accepts-a-129-character-tag-key#task#2 plus the other 22 steps in the cluster (kinesis-create-stream-mode-outside-enum-rejected#task#5, kinesis-create-stream-name-length-bounds#task#3, kinesis-create-stream-record-distribution-strategy#task#5, kinesis-decrease-retention-malformed-stream-id-rejected#task#6 and #task#8, kinesis-decrease-retention-refuses-a-stream-name-outside-its-charset#task#2/#4/#6/#8, kinesis-enhanced-monitoring-metric-name-outside-enum-rejected#task#4 and #task#6, kinesis-enhanced-monitoring-shard-level-metrics-count-bounds#task#5/#7/#9, kinesis-record-distribution-strategy-enum-values#task#5 and #task#7, kinesis-stop-encryption-key-id-length-limit#task#3/#5/#9/#11, stop-stream-encryption-type-outside-enum-rejected#task#8 and #task#10).

## Evidence
kinesis-add-tags-to-stream-accepts-a-129-character-tag-key#task#2, DescribeStreamSummary, status 200 expected and 200 observed; $.StreamDescriptionSummary.QueueCount: missing — expected 0, got None. Identical divergence on 22 further steps in the same cluster (divergence_key service=kinesis|DescribeStreamSummary|status=200->200|keys=$.StreamDescriptionSummary.QueueCount), affected_count 23. All other members of StreamDescriptionSummary match in those responses.

## Steps to Reproduce
1. kinesis.create_stream(StreamName=N, ShardCount=1) and wait for ACTIVE.
2. kinesis.describe_stream_summary(StreamName=N) and inspect StreamDescriptionSummary: expect a QueueCount member present with value 0; observe the member absent from the response body.
3. Repeat on an on-demand stream and on a stream with enhanced monitoring enabled or encryption started — the member is expected to be present in all of them.
4. Re-replay kinesis-add-tags-to-stream-accepts-a-129-character-tag-key#task#2 (and any other step of that cluster).
