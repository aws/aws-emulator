Issue-Source: trace-sweep-divergence
Issue-Trace: connect-stop-contact-media-processing-refusals
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connect-stop-contact-media-processing-refusals connect-data-table-evaluation-unknown-instance-and-bound-refusals evaluate-data-table-values-request-refusals connect-test-case-execution-unknown-instance-and-parameter-refusals connect-test-case-execution-unknown-instance-refusals
Issue-Shape-Id: com.amazonaws.connect#AmazonConnectService
Issue-Key: connect|StopContactMediaProcessing|4xx->4xx|status
Issue-Key: connect|EvaluateDataTableValues|4xx->4xx|$.message
Issue-Key: connect|EvaluateDataTableValues|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|ListTestCaseExecutions|4xx->4xx|status
Issue-Related: 20261128T134211Z-connect-request-validation-refusals-answer-an-unmodeled-validationexception.md
Issue-Commit: 447941bda47a9e01bc13409633bd0ec832671375

# connect constraint walk preempts resource resolution and uses a stale enum set, echoing rejected values

> _Note: this issue was identified against the code base at commit `447941bda47a9e01bc13409633bd0ec832671375`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopContactMediaProcessing

## Description
4 net-new causes about WHEN and HOW the emulator's pre-handler constraint walk fires. In two of them it refuses 400 for a request AWS accepts into the handler and then answers 404; in the other two it reports the right rule but with the wrong text — it echoes the rejected value AWS redacts, and it enumerates a stale enum value set. Unlike the companion bundle (the '1 validation error detected' envelope leaking where AWS uses its own sentence), these are cases where the envelope itself is what AWS uses, or where no refusal was due at all.

Cause 1 of 4 — StopContactMediaProcessing: an over-long ContactId is refused 400 instead of resolved and 404'd (fixture connect-stop-contact-media-processing-refusals, step task#2).
AWS: Smithy: StopContactMediaProcessing.ContactId targets ContactId with @length min 1 max 256 and the operation lists ResourceNotFoundException (404). Recorded: HTTP 404, member `Message` = `Resource not found of Type: ContactId` — AWS resolves the contact and reports it missing rather than enforcing the modelled length bound at the edge. Fixed text: "Resource not found of Type: "; variable part: the resource type name of the input member that could not be resolved (ContactId here).
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException` and "1 validation error detected: Value '<the 257-char contactId, echoed in full>' at 'contactId' failed to satisfy constraint: Member must have length less than or equal to 256".
Why: the status CLASS is wrong — a client distinguishing 'bad request, do not retry with another id' from 'no such contact' takes the wrong branch, and error-shape handling for ResourceNotFoundException never runs. The rejected contact id is also reflected back in full.
Fix: StopContactMediaProcessing's handler/validation ordering — do not let the pre-handler constraint walk shadow contact resolution for ContactId.
Traces: connect-stop-contact-media-processing-refusals#task#2.

Cause 2 of 4 — ListTestCaseExecutions: a status value AWS accepts is refused 400 instead of reaching the unknown-instance 404 (fixture connect-test-case-execution-unknown-instance-and-parameter-refusals, step task#5).
AWS: Smithy: ListTestCaseExecutions.Status targets enum TestCaseExecutionStatus, whose on-disk values are [INITIATED, PASSED, FAILED, IN_PROGRESS, STOPPED]; the operation lists ResourceNotFoundException (404). Recorded behavior shows the live service ALSO accepts PROCESSING — the request passes validation and is refused HTTP 404 with `The specified resource was not found.` because the instance does not exist. That sentence is ResourceNotFoundException's documented text, fixed in full with no variable part. The on-disk model's value list is therefore stale relative to the recording.
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException` and "1 validation error detected: Value 'PROCESSING' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [INITIATED, PASSED, FAILED, IN_PROGRESS, STOPPED]".
Why: a valid filter value is rejected outright, so the operation cannot be called with it at all; and the 400/404 class difference misleads a client about whether the instance exists.
Fix: ListTestCaseExecutions' validation — widen the accepted TestCaseExecutionStatus set to include PROCESSING, and resolve the instance before (or regardless of) that check.
Traces: connect-test-case-execution-unknown-instance-and-parameter-refusals#task#5.

Cause 3 of 4 — ListTestCaseExecutions: the off-enum status refusal echoes the value and lists the wrong value set (fixture connect-test-case-execution-unknown-instance-and-parameter-refusals, step task#4).
AWS: Recorded: HTTP 400 with message "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [IN_PROGRESS, INITIATED, STOPPED, PASSED, PROCESSING, FAILED]". So for this @httpQuery-bound enum AWS DOES use the framework envelope, but it writes `Value at '<member>'` with NO rejected value interpolated, and it enumerates SIX members including PROCESSING. Fixed text: the envelope and the rule clause; variable part: the member path from the request, plus the enum's own value set.
Emulator: HTTP 400 with "1 validation error detected: Value 'FINISHED' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [INITIATED, PASSED, FAILED, IN_PROGRESS, STOPPED]" — it interpolates the caller's rejected value where AWS omits it, and advertises a 5-value set.
Why: the message tells a client the wrong set of legal values (PROCESSING missing), which is the same stale enum as cause 2 seen from the message side; and it discloses caller input AWS deliberately leaves out of the error body.
Fix: the constraint walk's enum-violation message template (omit the value for this binding) plus the TestCaseExecutionStatus value set.
Traces: connect-test-case-execution-unknown-instance-and-parameter-refusals#task#4 (also connect-test-case-execution-unknown-instance-refusals#task#2).

Cause 4 of 4 — EvaluateDataTableValues: the maxResults range refusal echoes the rejected value (fixture connect-data-table-evaluation-unknown-instance-and-bound-refusals, step task#3).
AWS: Smithy: EvaluateDataTableValues.MaxResults targets MaxResult100 with @range min 1 max 100, bound @httpQuery "maxResults". Recorded: HTTP 400 with message "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1" — framework envelope, no value interpolated. Fixed text: the envelope and the rule clause; variable parts: the member path and the modelled bound.
Emulator: HTTP 400 with `x-amzn-errortype: ValidationException` and "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
Why: byte-for-byte message matching fails for every caller, and the emulator again reflects caller input AWS omits. Note this differs from ListTestCaseExecutions' own maxResults rule, which AWS answers with the sentence 'MaxResults must be at least 1' (filed in the companion bundle) — the two operations are not interchangeable.
Fix: the constraint walk's range-violation message template for @httpQuery-bound members.
Traces: connect-data-table-evaluation-unknown-instance-and-bound-refusals#task#3 (also evaluate-data-table-values-request-refusals#task#2).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true). All four clusters are verdict=mismatch, service=connect, stub_path empty, owned_by empty.

Cause 1 — StopContactMediaProcessing, connect-stop-contact-media-processing-refusals#task#2: `status: expected 404, got 400`; `$.Message: value_mismatch (expected 'Resource not found of Type: ContactId', got "1 validation error detected: Value 'kkkk...' [257 bytes] at 'contactId' failed to satisfy constraint: Member must have length less than or equal to 256")`; `$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')`.

Cause 2 — ListTestCaseExecutions, connect-test-case-execution-unknown-instance-and-parameter-refusals#task#5: `status: expected 404, got 400`; `$.message: value_mismatch (expected 'The specified resource was not found.', got "1 validation error detected: Value 'PROCESSING' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [INITIATED, PASSED, FAILED, IN_PROGRESS, STOPPED]")`; errortype got 'ValidationException'.

Cause 3 — ListTestCaseExecutions, connect-test-case-execution-unknown-instance-and-parameter-refusals#task#4: `$.message: value_mismatch (expected "1 validation error detected: Value at 'status' failed to satisfy constraint: Member must satisfy enum value set: [IN_PROGRESS, INITIATED, STOPPED, PASSED, PROCESSING, FAILED]", got "1 validation error detected: Value 'FINISHED' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [INITIATED, PASSED, FAILED, IN_PROGRESS, STOPPED]")`. Expected set has 6 members, actual 5 (PROCESSING absent); expected omits the rejected value, actual interpolates 'FINISHED'. Status 400 both sides.

Cause 4 — EvaluateDataTableValues, connect-data-table-evaluation-unknown-instance-and-bound-refusals#task#3: `$.message: value_mismatch (expected "1 validation error detected: Value at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1", got "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1")`; errortype got 'ValidationException'. Status 400 both sides.

Model citations: smithy_query operation=StopContactMediaProcessing,ListTestCaseExecutions,EvaluateDataTableValues and shape=TestCaseExecutionStatus,ResourceNotFoundException on services/service-com-amazonaws-connect-amazonconnectservice/smithy-model/com-amazonaws-connect-amazonconnectservice.json. TestCaseExecutionStatus on disk = [INITIATED, PASSED, FAILED, IN_PROGRESS, STOPPED]; the recording shows AWS enumerating and accepting PROCESSING as well.

## Steps to Reproduce
1. Boot the connect emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Representative: replay fixture connect-stop-contact-media-processing-refusals step task#2 — POST /contact/stop-contact-media-processing with a 257-character ContactId. Observe HTTP 400 ValidationException instead of HTTP 404 with Message 'Resource not found of Type: ContactId'.
3. Replay connect-test-case-execution-unknown-instance-and-parameter-refusals#task#5 — GET /test-case-executions/{InstanceId}?status=PROCESSING for an instance that does not exist. Observe HTTP 400 rejecting PROCESSING instead of HTTP 404 'The specified resource was not found.'
4. Replay connect-test-case-execution-unknown-instance-and-parameter-refusals#task#4 — ?status=FINISHED. Observe the enum-violation message interpolating 'FINISHED' and listing 5 values, where AWS writes "Value at 'status'" and lists 6 including PROCESSING.
5. Replay connect-data-table-evaluation-unknown-instance-and-bound-refusals#task#3 — EvaluateDataTableValues with ?maxResults=0. Observe "Value '0' at 'maxResults'" where AWS writes "Value at 'maxResults'".
6. Confirm: causes 1 and 2 answer 404 with the modelled ResourceNotFoundException text, and causes 3 and 4 answer the framework envelope with the value omitted and PROCESSING present in the enum set.
