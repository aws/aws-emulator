Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__kinesis.kinesis-parameters-on-a-queue-target-rejected-while-the-same-parameters-on-a-stream-deliver events__kinesis.kinesis-partition-key-path-outside-dot-notation-rejected-while-the-dot-path-keys-records events__kinesis.kinesis-target-input-transformer-writes-the-rendered-template-keyed-by-an-original-event-field events__kinesis.kinesis-partition-key-path-reads-the-original-event-under-an-input-transformer events__kinesis.kinesis-target-constant-input-replaces-every-event-yet-each-record-is-keyed-by-its-own-event events__kinesis.kinesis-target-unquoted-text-input-template-rejected events__kinesis.kinesis-target-without-role-arn-rejected events__kinesis.kinesis-target-without-role-arn-rejected-until-the-target-names-a-role events__kinesis.put-targets-kinesis-stream-without-role-arn-rejected
Issue-Key: events__kinesis.kinesis-parameters-on-a-queue-target-rejected-while-the-same-parameters-on-a-stream-deliver|_refusal#16|AssertionError|raise AssertionError(f"expected PutTargets to be refused, but it answered {result!r}")
Issue-Key: events__kinesis.kinesis-partition-key-path-outside-dot-notation-rejected-while-the-dot-path-keys-records|_refusal#15|AssertionError|raise AssertionError(f"expected PutTargets to be refused, but it answered {result!r}")
Issue-Key: events__kinesis.kinesis-target-input-transformer-writes-the-rendered-template-keyed-by-an-original-event-field|run#24|AssertionError|assert rec["PartitionKey"] == json.dumps(shard_key), (
Issue-Key: events__kinesis.kinesis-partition-key-path-reads-the-original-event-under-an-input-transformer|run#27|Timeout|time.sleep(2)
Issue-Key: events__kinesis.kinesis-target-constant-input-replaces-every-event-yet-each-record-is-keyed-by-its-own-event|_read_until#22|Timeout|time.sleep(2)
Issue-Key: events__kinesis.kinesis-target-unquoted-text-input-template-rejected|_expect_refusal#24|AssertionError|assert got == want, f"{what} answered {got!r}, expected {want!r}"
Issue-Key: events__kinesis.kinesis-target-without-role-arn-rejected|run#19|AssertionError|assert got == ("ValidationException",
Issue-Key: events__kinesis.kinesis-target-without-role-arn-rejected-until-the-target-names-a-role|_expect_refusal#19|AssertionError|assert got == want, f"the role-less Kinesis target was refused with {got!r}, expected {want!r}"
Issue-Key: events__kinesis.put-targets-kinesis-stream-without-role-arn-rejected|_expect_refusal#23|AssertionError|assert got == want, f"{what} answered {got!r}, expected {want!r}"
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__kinesis: 5 causes — KinesisParameters are neither restricted nor applied (records keyed '{}'), and the role-less and bad-template refusals are re-worded

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 5: KinesisParameters on a non-Kinesis target are accepted. AWS: KinesisParameters only apply to a Kinesis data-stream target (Smithy: Target.KinesisParameters documents "the custom parameter you can use to control the shard assignment, when the target is a Kinesis data stream"); supplying them on an SQS-queue target is refused ValidationException naming the unsupported parameter for that target, while the same parameters on a stream target are accepted and deliver. Emulator: `expected PutTargets to be refused, but it answered {'FailedEntryCount': 0, 'FailedEntries': []}`. Checks: events__kinesis.kinesis-parameters-on-a-queue-target-rejected-while-the-same-parameters-on-a-stream-deliver.

Cause 2 of 5: a PartitionKeyPath outside JSON dot notation is accepted. AWS: refuses ValidationException — the path must be dot-notation JSONPath (Smithy: KinesisParameters.PartitionKeyPath required, @length 0..256, documented as "the JSON path to be extracted from the event"); a bracket-notation/invalid path is rejected while the dot path keys the records. Emulator: returns 200 with FailedEntryCount 0. Checks: events__kinesis.kinesis-partition-key-path-outside-dot-notation-rejected-while-the-dot-path-keys-records.

Cause 3 of 5: PartitionKeyPath is never resolved — records are keyed with the literal `{}` instead of the value extracted from the ORIGINAL event. AWS: the partition key is the JSON-encoded value at PartitionKeyPath read from the original event, even when the target also carries a constant Input or an InputTransformer (the transformer rewrites the DATA, never the key); each record is keyed by its own event. Emulator: `record partition key is '{}'; expected the original event's detail.shard_key JSON-encoded, '"key-<id>"'`, and the constant-input and input-transformer variants exhaust their 60s budget. Checks: events__kinesis.kinesis-target-input-transformer-writes-the-rendered-template-keyed-by-an-original-event-field, events__kinesis.kinesis-partition-key-path-reads-the-original-event-under-an-input-transformer, events__kinesis.kinesis-target-constant-input-replaces-every-event-yet-each-record-is-keyed-by-its-own-event.

Cause 4 of 5: the refusal message for a bare-text InputTemplate is emulator-authored. AWS: PutTargets refuses an InputTemplate that is not valid JSON with ValidationException `Invalid InputTemplate for target <target-id>` — fixed text `Invalid InputTemplate for target ` + the target Id from the request (and the refusal is final for the call). Emulator: answers `InputTemplate for target <id> is not valid JSON.`. Checks: events__kinesis.kinesis-target-unquoted-text-input-template-rejected.

Cause 5 of 5: the role-less-target refusal message is emulator-authored. AWS: a Kinesis-stream target with no RoleArn is refused ValidationException `Rule <rule-name> does not have RoleArn assigned to invoke target <target-arn>.` — fixed text with the rule name and target ARN taken from the request. Emulator: answers `RoleArn is required for target <target-arn>.`. Checks: events__kinesis.kinesis-target-without-role-arn-rejected, events__kinesis.kinesis-target-without-role-arn-rejected-until-the-target-names-a-role, events__kinesis.put-targets-kinesis-stream-without-role-arn-rejected.

Why: streams are written with a constant partition key, so every event lands on one shard and ordering/sharding guarantees a client designs for are lost; invalid target configurations are stored and only fail at delivery time; and callers matching AWS's refusal text (or reading it) get the wrong diagnosis.
Fix: the events PutTargets validation (per-target-type parameter support, PartitionKeyPath syntax, the two refusal messages) and the Kinesis target delivery path (resolve PartitionKeyPath against the original event before applying Input/InputTransformer).
Traces: the nine check ids listed per cause above.

## Evidence
Causes 1-2: `AssertionError: expected PutTargets to be refused, but it answered {'FailedEntryCount': 0, 'FailedEntries': [], ...}`
Cause 3: `AssertionError: record partition key is '{}'; expected the original event's detail.shard_key JSON-encoded, '"key-74e47229aff9"'` (plus two 60s timeouts)
Cause 4: answered ('ValidationException', 'InputTemplate for target text-line is not valid JSON.', False), expected ('ValidationException', 'Invalid InputTemplate for target text-line', True)
Cause 5: answered ('ValidationException', 'RoleArn is required for target <stream-arn>.'), expected ('ValidationException', 'Rule <rule> does not have RoleArn assigned to invoke target <stream-arn>.')

## Steps to Reproduce
1. PutTargets with KinesisParameters on an SQS-queue target — observe 200.
2. PutTargets with KinesisParameters.PartitionKeyPath in bracket notation — observe 200.
3. PutTargets on a Kinesis stream with PartitionKeyPath=$.detail.shard_key and a RoleArn (also with a constant Input, and with an InputTransformer); PutEvents and read the stream records' PartitionKey.
4. PutTargets with an InputTransformer whose InputTemplate is bare text — compare the message.
5. PutTargets on a Kinesis stream with no RoleArn — compare the message.
