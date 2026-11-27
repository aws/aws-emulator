Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__kinesis__kms__sqs.kms-encrypted-stream-target-role-without-generate-data-key-dead-letters-while-a-keyed-twin-delivers
Issue-Key: events__kinesis__kms__sqs.kms-encrypted-stream-target-role-without-generate-data-key-dead-letters-while-a-keyed-twin-delivers|run#39|Timeout|page = kinesis.get_records(ShardIterator=it, Limit=100)
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# events__kinesis__kms__sqs: an EventBridge target role without kms:GenerateDataKey still delivers to an SSE-KMS stream and never dead-letters NO_PERMISSIONS

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause, grounded in a failing behavioral check (the check exhausted its wait and the suite cut it off at the 60 s cap with no verdict).

Cause 1 of 1: delivery to a Kinesis target whose stream is encrypted with a customer managed key is not authorized against the target role's KMS permissions, so no NO_PERMISSIONS dead-letter is ever produced.
  AWS: recorded behavior only. A rule whose target role is allowed kinesis:PutRecord but NOT kms:GenerateDataKey on the stream's CMK writes nothing to the stream and dead-letters the event to the rule's DLQ with the failure attributes naming ERROR_CODE 'NO_PERMISSIONS' (while a twin role holding both permissions writes the record).
  Emulator: the check waits for that dead-letter and never sees it; it is cut off at the 60 s cap with no verdict. Hypothesis (consistent with the sibling divergences filed for iam__kinesis__kms__logs and the open issue 20261117T122437Z-kinesis-kms-putrecord-on-an-sse-kms-stream.md): the emulator serves the PutRecord without consulting KMS for the target role, so the delivery succeeds and nothing is ever dead-lettered.
  Why: a client relying on EventBridge's dead-letter path to surface a missing kms: permission on a target gets neither the refusal nor the DLQ message — a mis-permissioned target looks healthy.
  Fix: the eventbridge target-delivery path for Kinesis targets — authorize the target role for the stream's CMK (kms:GenerateDataKey) before writing, and emit the NO_PERMISSIONS dead-letter with its failure attributes when it is denied.
  Traces: events__kinesis__kms__sqs.kms-encrypted-stream-target-role-without-generate-data-key-dead-letters-while-a-keyed-twin-delivers.

## Evidence
events__kinesis__kms__sqs.kms-encrypted-stream-target-role-without-generate-data-key-dead-letters-while-a-keyed-twin-delivers —
Error: Check timed out after 60.0s, waiting for the NO_PERMISSIONS dead-letter that never arrived. Expected: the event is not written to the stream and lands in the rule's DLQ with ERROR_CODE 'NO_PERMISSIONS'; the twin role with both permissions writes the record.

## Steps to Reproduce
1. Create a symmetric CMK and a Kinesis stream with SSE-KMS under it.
2. Create two target roles: one allowed kinesis:PutRecord only, one allowed kinesis:PutRecord plus kms:GenerateDataKey on the key.
3. Create a rule per role targeting the stream, each with an SQS dead-letter queue, then PutEvents matching both.
4. Observe the stream/DLQ: no dead-letter with ERROR_CODE NO_PERMISSIONS appears for the key-less role (AWS produces one, and writes nothing to the stream).
