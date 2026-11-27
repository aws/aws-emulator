Issue-Source: trace-sweep-divergence
Issue-Trace: glue-batch-stop-job-run-input-refusals
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: glue-batch-stop-job-run-input-refusals glue-ml-task-run-cancel-empty-transform-id glue-batch-stop-job-run-refuses-every-entry-of-an-unknown-job glue-partition-batch-and-page-size-limits
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue|BatchStopJobRun|4xx->4xx|$.__type
Issue-Key: glue|BatchStopJobRun|4xx->4xx|$.message
Issue-Key: glue|CancelMLTaskRun|4xx->4xx|$.__type
Issue-Key: glue|CancelMLTaskRun|4xx->4xx|$.message
Issue-Key: glue|BatchDeletePartition|4xx->4xx|$.message
Issue-Commit: bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5

# Glue constraint refusals: wrong exception class and the offending value omitted from the ValidationException message

> _Note: this issue was identified against the code base at commit `bb517dad8aa2a6dcfbb56aa45c4ce46fe44a83f5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchStopJobRun

## Description
Four net-new root causes in glue, all in the path that refuses an input that violates a modeled constraint. Two pick the wrong exception class for an empty required name, and two serialize AWS's ValidationException message with the offending value cut out.

Cause 1 of 4: BatchStopJobRun with an empty JobName is refused as ValidationException instead of InvalidInputException.
AWS: BatchStopJobRun.JobName is `@required` and targets NameString with `@length min 1, max 255` (Smithy: BatchStopJobRun input JobName -> NameString, length {min:1,max:255}; modeled errors are InternalServiceException, InvalidInputException, OperationTimeoutException — ValidationException is NOT bound to this operation). An empty JobName is refused with HTTP 400 and the awsJson1.1 error body `{"__type": "InvalidInputException"}`; the recorded refusal carries no message member at all, so the error code is the whole requirement (recorded behavior for the message's absence).
Emulator: HTTP 400 with `__type` `ValidationException` and an emulator-synthesized lowercase `message` member, `"1 validation error detected: Value '' at 'jobName' failed to satisfy constraint: Member must have length greater than or equal to 1"` — an error class the operation does not model, plus a message AWS does not send.
Why: a client catching InvalidInputException (the class the SDK's modeled error union for this operation exposes) does not catch ValidationException at all — it falls through to the generic/unmodeled-error branch, so the refusal is reported as an unknown service error rather than handled.
Fix: input validation in the BatchStopJobRun handler of the glue service crate — the constraint check must raise the operation's modeled InvalidInputException with no message member, not the generic ValidationException used for Smithy constraint violations.
Traces: glue-batch-stop-job-run-input-refusals#task#2.

Cause 2 of 4: CancelMLTaskRun with an empty TransformId is refused as ValidationException instead of InvalidInputException.
AWS: CancelMLTaskRun.TransformId is `@required` targeting HashString with `@length min 1, max 255`; the operation models EntityNotFoundException, InternalServiceException, InvalidInputException and OperationTimeoutException and does NOT model ValidationException (Smithy: CancelMLTaskRun). An empty TransformId is refused 400 `InvalidInputException` with no message member in the recorded body.
Emulator: 400 with `__type` `ValidationException` and the synthesized message `"1 validation error detected: Value '' at 'transformId' failed to satisfy constraint: Member must have length greater than or equal to 1"`.
Why: same as Cause 1 — the refusal arrives as an error class the operation's modeled union does not contain, so typed client error handling for ML task-run cancellation never fires.
Fix: input validation in the CancelMLTaskRun handler of the glue service crate (same shared constraint-violation mapper as Cause 1).
Traces: glue-ml-task-run-cancel-empty-transform-id#task#1.

Cause 3 of 4: BatchStopJobRun's over-length JobRunIds ValidationException message omits the rendered value AWS includes.
AWS: BatchStopJobRun.JobRunIds targets BatchStopJobRunJobRunIdList with `@length min 1, max 25` (Smithy). A list of 26 ids is refused 400 with the standard constraint message whose fixed text is `1 validation error detected: Value '<rendered value>' at '<memberName>' failed to satisfy constraint: Member must have length less than or equal to <max>`. The variable parts are the rendered request value (the list, bracketed and comma-space separated), the serialized member name taken from the request (`jobRunIds`) and the modeled maximum (25) — all three come from the request and the model, never from one run.
Emulator: the same message with the `Value '<rendered value>' ` segment deleted: `"1 validation error detected: Value at 'jobRunIds' failed to satisfy constraint: Member must have length less than or equal to 25"`.
Why: the offending value is the only part of the message that tells a caller or a log reader WHICH input was rejected; a client (or a human) diffing or parsing the standard constraint text sees a sentence no AWS region emits and cannot recover the rejected value.
Fix: the constraint-violation message formatter shared by the glue crate's validation layer — it must render the offending value into the message, for list members as `[e0, e1, ...]`.
Traces: glue-batch-stop-job-run-input-refusals#task#4.

Cause 4 of 4: BatchDeletePartition's over-length PartitionsToDelete ValidationException message omits the rendered value the same way.
AWS: BatchDeletePartition.PartitionsToDelete targets BatchDeletePartitionValueList with `@length min 0, max 25` (Smithy). A list of 26 value sets is refused 400 with the same standard text, and the rendered value is the list of PartitionValueList structures as the service renders them (`[PartitionValueList(values=[<v>]), ...]`), with the member name `partitionsToDelete` and the maximum 25 from the model.
Emulator: `"1 validation error detected: Value at 'partitionsToDelete' failed to satisfy constraint: Member must have length less than or equal to 25"` — same omission as Cause 3, confirming one shared formatter rather than a per-operation slip.
Why: as Cause 3 — the rejected input is unrecoverable from the refusal.
Fix: same shared constraint-message formatter, including the structure-member rendering used for list-of-structure members.
Traces: glue-partition-batch-and-page-size-limits#task#4.

Related but not covering: 20261127T083840Z-glue-listentities-refuses-a-missing-connectionname-with-invalidinputexception.md records the OPPOSITE substitution on ListEntities (InvalidInputException where AWS sends ValidationException), and 20261127T133330Z-glue-batchgetdataqualityrulesetevaluationrun-s-malformed-runid-invalidinputexception-carries-the.md records an InvalidInputException carrying a message AWS omits. Neither covers these four operations.

## Evidence
Cause 1 — fixture glue-batch-stop-job-run-input-refusals step task#1 (BatchStopJobRun, empty JobName), expected HTTP 400 / actual HTTP 400:
  $.__type: value_mismatch (expected 'InvalidInputException', got 'ValidationException')
  $.message: unexpected_present (expected None, got "1 validation error detected: Value '' at 'jobName' failed to satisfy constraint: Member must have length greater than or equal to 1")

Cause 2 — fixture glue-ml-task-run-cancel-empty-transform-id step task#1 (CancelMLTaskRun), expected 400 / actual 400:
  $.__type: value_mismatch (expected 'InvalidInputException', got 'ValidationException')
  $.message: unexpected_present (expected None, got "1 validation error detected: Value '' at 'transformId' failed to satisfy constraint: Member must have length greater than or equal to 1")

Cause 3 — fixture glue-batch-stop-job-run-input-refusals step task#4, expected 400 / actual 400:
  $.message: value_mismatch
    expected "1 validation error detected: Value '[jr_00, jr_01, ... , jr_25]' at 'jobRunIds' failed to satisfy constraint: Member must have length less than or equal to 25"
    got      "1 validation error detected: Value at 'jobRunIds' failed to satisfy constraint: Member must have length less than or equal to 25"
  (The 26 ids are request values; the fixed text and the limit 25 are the requirement.)

Cause 4 — fixture glue-partition-batch-and-page-size-limits step task#4, expected 400 / actual 400:
  $.message: value_mismatch
    expected "1 validation error detected: Value '[PartitionValueList(values=[p00]), ... , PartitionValueList(values=[p25])]' at 'partitionsToDelete' failed to satisfy constraint: Member must have length less than or equal to 25"
    got      "1 validation error detected: Value at 'partitionsToDelete' failed to satisfy constraint: Member must have length less than or equal to 25"

## Steps to Reproduce
Cause 1: POST AWSGlue.BatchStopJobRun with {"JobName": "", "JobRunIds": ["jr_00"]}; read __type and the message member of the 400 body.
Cause 2: POST AWSGlue.CancelMLTaskRun with {"TransformId": "", "TaskRunId": "tr_00"}; read __type and the message member.
Cause 3: POST AWSGlue.BatchStopJobRun with a valid JobName and 26 JobRunIds; read $.message of the 400 body.
Cause 4: CreateDatabase, CreateTable with PartitionKeys, then POST AWSGlue.BatchDeletePartition with 26 entries in PartitionsToDelete; read $.message of the 400 body.
