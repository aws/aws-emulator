Issue-Source: trace-sweep-divergence
Issue-Trace: disable-kinesis-streaming-stream-arn-length-bounds
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: disable-kinesis-streaming-stream-arn-length-bounds dynamodb-restore-to-point-in-time-source-table-arn-length-bounds dynamodb-start-bulk-put-items-output-prefix-length dynamodb-batch-execute-statement-member-length-limit dynamodb-execute-transaction-statement-length-bounds
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|DisableKinesisStreamingDestination|4xx->4xx|$.message
Issue-Key: dynamodb|RestoreTableToPointInTime|4xx->4xx|$.message
Issue-Key: dynamodb|StartBulkPutItems|4xx->4xx|$.message
Issue-Key: dynamodb|BatchExecuteStatement|4xx->4xx|$.message
Issue-Key: dynamodb|ExecuteTransaction|4xx->4xx|$.message
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# DynamoDB length-constraint ValidationException messages abbreviate the rejected value instead of reproducing it

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableKinesisStreamingDestination

## Description
When a request member exceeds its modeled @length maximum, DynamoDB refuses it with HTTP 400 ValidationException whose message quotes the OFFENDING VALUE IN FULL. The emulator builds the same sentence but substitutes an abbreviation of the value, so the message is not the message AWS emits. The fixed text of the template is `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have length less than or equal to <max>`; the variable parts are the value (taken verbatim from the request), the member path and the limit (taken from the model). The emulator renders the value as `<first ~250 characters>... [<n> bytes]` instead.

THIS BUNDLE CONTAINS 5 CAUSES.

Cause 1 of 5: DisableKinesisStreamingDestination StreamArn over 1024 characters.
AWS: Smithy: DisableKinesisStreamingDestination.StreamArn -> StreamArn @length min 37 max 1024. A 1025-byte ARN is refused 400 ValidationException with `1 validation error detected: Value '<the full 1025-character ARN>' at 'streamArn' failed to satisfy constraint: Member must have length less than or equal to 1024`.
Emulator: identical fixed text and member path, but the value segment is cut off and replaced with `... [1025 bytes]`.
Why: a client or operator that matches or logs the refusal never sees the value AWS echoed, and any assertion on the documented message text fails; the byte-count suffix is emulator-invented text AWS never emits.
Fix: the shared ValidationException message builder in the DynamoDB request-validation component - the rejected value must be interpolated whole, with no length cap and no `[n bytes]` suffix.
Traces: disable-kinesis-streaming-stream-arn-length-bounds#task#5.

Cause 2 of 5: RestoreTableToPointInTime SourceTableArn over 1024 characters.
AWS: Smithy: RestoreTableToPointInTime source ARN -> TableArn/@length max 1024. The refusal is `1 validation error detected: Value '<full ARN>' at 'sourceTableArn' failed to satisfy constraint: Member must have length less than or equal to 1024`, 400 ValidationException.
Emulator: same sentence with the ARN abbreviated to its first ~250 characters plus `... [1025 bytes]`.
Why: same as Cause 1; this cluster covers four steps of the same fixture, so every length boundary of this member is affected.
Fix: as Cause 1 (the message builder), not the RestoreTableToPointInTime handler.
Traces: dynamodb-restore-to-point-in-time-source-table-arn-length-bounds#task#1 (cluster also covers task#2, task#3, task#4).

Cause 3 of 5: StartBulkPutItems OutputS3Configuration.s3KeyPrefix over 1024 characters.
AWS: Smithy: S3Prefix @length min 0 max 1024. Refusal `1 validation error detected: Value '<full prefix>' at 'outputS3Configuration.s3KeyPrefix' failed to satisfy constraint: Member must have length less than or equal to 1024`, 400 ValidationException.
Emulator: same text with the prefix abbreviated and `... [1025 bytes]` appended.
Why: as Cause 1. Note the member path itself is correct here, so only the value rendering diverges.
Fix: as Cause 1.
Traces: dynamodb-start-bulk-put-items-output-prefix-length#task#2.

Cause 4 of 5: BatchExecuteStatement Statements member statement over its length limit.
AWS: Smithy: PartiQLStatement @length min 1 max 8192. The recorded refusal is 400 ValidationException whose message quotes the whole offending PartiQL statement inside the `1 validation error detected: Value '<statement>' at '<path>' failed to satisfy constraint: ...` template.
Emulator: 400 with a $.message that differs from the recorded one. Hypothesis: the same value abbreviation as Causes 1-3, since the only diverging key is $.message on a pure length-constraint refusal whose value is a multi-kilobyte statement.
Why: as Cause 1 - the caller cannot see which statement of the batch was rejected.
Fix: as Cause 1; verify by re-replaying this step after the message builder stops abbreviating.
Traces: dynamodb-batch-execute-statement-member-length-limit#task#6.

Cause 5 of 5: ExecuteTransaction TransactStatements statement over its length limit.
AWS: Smithy: PartiQLStatement @length min 1 max 8192; ExecuteTransaction.TransactStatements @length 1..100. The recorded refusal is 400 ValidationException quoting the full statement in the same template.
Emulator: 400 with a diverging $.message. Hypothesis: the same value abbreviation as Causes 1-3.
Why: as Cause 1.
Fix: as Cause 1.
Traces: dynamodb-execute-transaction-statement-length-bounds#task#4.

Related open issues: 20261120T225636Z-dynamodb-validationexception-envelope-diverges-member-paths-value-rendering.md and 20261127T132819Z-dynamodb-validationexception-texts-are-generated-from-the-smithy.md describe neighbouring defects in the same message builder, but the replay report binds none of these five clusters to them (owned_by is empty for all five), so they are not gated by those issues and are filed here with their own replay keys.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout. All five clusters are verdict=mismatch, status 400 -> 400, stub_path empty, owned_by empty; the only diverging key is $.message.

Cause 1 - disable-kinesis-streaming-stream-arn-length-bounds#task#5 (affected_count 1):
  expected: "1 validation error detected: Value 'arn:aws:kinesis:::stream/qqq...<1025 characters in full>' at 'streamArn' failed to satisfy constraint: Member must have length less than or equal to 1024"
  got:      "1 validation error detected: Value 'arn:aws:kinesis:::stream/qqq...[truncated]... [1025 bytes]' at 'streamArn' failed to satisfy constraint: Member must have length less than or equal to 1024"

Cause 2 - dynamodb-restore-to-point-in-time-source-table-arn-length-bounds#task#1 (affected_count 4):
  expected the full 1025-character sourceTableArn inside the message; got the ARN cut after ~250 characters followed by "... [1025 bytes]".

Cause 3 - dynamodb-start-bulk-put-items-output-prefix-length#task#2 (affected_count 1):
  expected the full 'results/rrr...' prefix at 'outputS3Configuration.s3KeyPrefix'; got it cut after ~250 characters followed by "... [1025 bytes]".

Cause 4 - dynamodb-batch-execute-statement-member-length-limit#task#6 (affected_count 1):
  $.message value_mismatch; expected message quotes the full `INSERT INTO "<table>" VALUE {'pk': 'too-long', 'pad': 'ppp...'}` statement.

Cause 5 - dynamodb-execute-transaction-statement-length-bounds#task#4 (affected_count 1):
  $.message value_mismatch; expected message quotes the full `INSERT INTO "<table>" VALUE {'pk': 'over-limit', 'pad': 'zzz...'}` statement.

Model citations: StreamArn @length 37..1024, TableArn @length 1..1024, S3Prefix @length 0..1024, PartiQLStatement @length 1..8192 (dynamodb-20120810 Smithy model).

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: DisableKinesisStreamingDestination with a 1025-character StreamArn (fixture disable-kinesis-streaming-stream-arn-length-bounds, step task#5); assert the whole ARN appears in $.message and that the message contains no '[... bytes]' suffix.
3. Cause 2: RestoreTableToPointInTime with a 1025-character SourceTableArn (fixture dynamodb-restore-to-point-in-time-source-table-arn-length-bounds, steps task#1-task#4).
4. Cause 3: StartBulkPutItems with a 1025-character OutputS3Configuration.S3KeyPrefix (fixture dynamodb-start-bulk-put-items-output-prefix-length, step task#2).
5. Cause 4: BatchExecuteStatement with one statement over PartiQLStatement's 8192-character maximum (fixture dynamodb-batch-execute-statement-member-length-limit, step task#6).
6. Cause 5: ExecuteTransaction with one TransactStatements entry over the same maximum (fixture dynamodb-execute-transaction-statement-length-bounds, step task#4).
7. All five causes must pass before closing.
