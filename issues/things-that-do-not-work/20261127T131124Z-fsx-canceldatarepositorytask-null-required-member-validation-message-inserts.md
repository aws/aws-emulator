Issue-Source: trace-sweep-divergence
Issue-Trace: fsx-cancel-data-repository-task-refusals
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: fsx-cancel-data-repository-task-refusals
Issue-Shape-Id: com.amazonaws.fsx#AWSSimbaAPIService_v20180301
Issue-Key: fsx|CancelDataRepositoryTask|4xx->4xx|$.Message
Issue-Commit: 40889a98079cc23783ceec0f1776329f5efdb100

# fsx: CancelDataRepositoryTask null-required-member validation message inserts a literal "Value null" segment

> _Note: this issue was identified against the code base at commit `40889a98079cc23783ceec0f1776329f5efdb100`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelDataRepositoryTask

## Description
Cause 1 of 1: the emulator's request-validation message formatter for a MISSING (null) required member emits the "Value <x> at '<member>'" form with the literal word `null` substituted for the value, instead of AWS's distinct null-member form that carries no value segment at all.

AWS: FSx validates CancelDataRepositoryTask's required input member before dispatch (Smithy: CancelDataRepositoryTask.input member TaskId, target TaskId, @required, clientOptional, @length min 12 max 128, @pattern ^(task-[0-9a-f]{17,})$; modeled client errors BadRequest/DataRepositoryTaskNotFound/DataRepositoryTaskEnded/UnsupportedOperation, all httpError 400). The AWS-SDK-shared validation preamble has two distinct shapes, and the null case is the one without a value: for an ABSENT/null required member AWS answers HTTP 400 with the message "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must not be null", whereas only a PRESENT value that fails a length/pattern constraint gets the "Value '<value>' at '<member>' failed to satisfy constraint: ..." form. Fixed text of the null-member message: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must not be null". Its only variable part is the member path taken from the request's input shape (here the lowerCamel wire path for the required member, 'taskId'); the error count prefix is the number of validation failures detected in that request. No value from the request or from emulator state appears in this message, because there is no value — that is precisely what the message reports.

Emulator: answers the correct HTTP 400 for the refusal, but the Message body member reads "1 validation error detected: Value null at 'taskId' failed to satisfy constraint: Member must not be null" — the constrained-value template rendered with the word `null` where a value would go, producing the extra "null " segment after "Value ".

Why: SDKs, retry/classification layers and test assertions key off the exact validation preamble to distinguish a missing required member from a present-but-invalid one. A client (or a recorded-response assertion, or any log/alert matcher) that matches AWS's null-member text fails to match, and tooling that parses the segment after "Value " as the offending value reads the string "null" as a submitted value that was never sent — so a required-member omission is misreported as a malformed-value rejection.

Fix: the shared request-validation message formatter used by the FSx operation's input validation (the code that builds the "N validation error detected: ..." preamble for CancelDataRepositoryTask's required TaskId check) must select the null-member template — "Value at '<path>' failed to satisfy constraint: Member must not be null" — and emit no value segment when the member is absent/null, reserving the "Value '<value>' at ..." template for members that are present and fail a length/pattern/range constraint. The HTTP status (400) and the error shape are already correct; only the message rendering diverges.

Traces: fsx-cancel-data-repository-task-refusals step task#4 (expected vs actual differ only in $.Message; cluster size 1).

## Evidence
Fixture fsx-cancel-data-repository-task-refusals, step task#4, operation CancelDataRepositoryTask (service fsx). Verdict: mismatch. Expected status 400, actual status 400 — status agrees. Diverging key $.Message (value_mismatch):
  expected: "1 validation error detected: Value at 'taskId' failed to satisfy constraint: Member must not be null"
  actual:   "1 validation error detected: Value null at 'taskId' failed to satisfy constraint: Member must not be null"
Difference is the literal inserted token "null " immediately after "Value ". Fixed text of the required message: "1 validation error detected: Value at '" + <member path> + "' failed to satisfy constraint: Member must not be null". Variable parts: the leading error count (number of validation failures in the request) and the member path from the request's input shape ('taskId' for CancelDataRepositoryTask's required TaskId). Nothing else in this message is request- or state-derived, and no per-run value (request id, timestamp, generated id) is involved in the divergence. Divergence key: service=fsx|CancelDataRepositoryTask|status=400->400|keys=$.Message. No stub_path and no owning open issue; the open fsx issues (fsx CreateDataRepositoryTask stub, fsx__lambda UnknownServiceError) cover different operations and do not carry this replay key.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus), trace set simparity-wire-aws-v1.
2. Fixture fsx-cancel-data-repository-task-refusals drives CancelDataRepositoryTask with the required input member omitted/null (step task#4).
3. Observe the 400 response body's Message member: the emulator returns "1 validation error detected: Value null at 'taskId' failed to satisfy constraint: Member must not be null" where AWS returns "1 validation error detected: Value at 'taskId' failed to satisfy constraint: Member must not be null".
4. Equivalent by hand: POST the FSx JSON-RPC target AWSSimbaAPIService_v20180301.CancelDataRepositoryTask with body {} (or {"TaskId": null}) and compare the Message member against the AWS null-required-member preamble.
