Issue-Source: trace-sweep-divergence
Issue-Trace: quicksight-automation-job-payload-and-id-constraints-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: quicksight-automation-job-payload-and-id-constraints-rejected quicksight-start-automation-job-unknown-automation-rejected quicksight-cancel-ingestion-unknown-dataset-and-invalid-id-rejected quicksight-dashboard-snapshot-job-on-missing-dashboard-rejected quicksight-predict-qa-results-parameter-constraints-rejected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|StartAutomationJob|4xx->4xx|$.message
Issue-Key: quicksight|CancelIngestion|4xx->4xx|$.Message
Issue-Key: quicksight|CancelIngestion|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: quicksight|StartDashboardSnapshotJob|4xx->4xx|$.message
Issue-Key: quicksight|PredictQAResults|4xx->4xx|$.message
Issue-Commit: 8c779968f63825f959c68d9ef908c64428218fcd

# QuickSight input-validation rejections render non-AWS message text and a non-modeled error type

> _Note: this issue was identified against the code base at commit `8c779968f63825f959c68d9ef908c64428218fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationJob

## Description
The quicksight emulator rejects malformed input with the right HTTP status (400) but renders the rejection differently from AWS in four distinct ways. Each cause below is a separate defect in the emulator's constraint-violation rendering / error selection and must be fixed and re-replayed independently. THIS BUNDLE CONTAINS 4 CAUSES.

Cause 1 of 4: StartAutomationJob - @pattern violation message must report the regex with its anchors stripped.
AWS: Smithy: StartAutomationJob.InputPayload targets SensitiveIOPayload, @pattern "^[\\s\\S]*[{\\[].*[}\\]]\\s*$", @sensitive, @length min 1 max 7000000; modeled error InvalidParameterValueException (httpError 400, member Message). AWS's standard constraint-violation body for a pattern violation is "1 validation error detected: Value at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", and the pattern it prints is the modelled regex with the leading '^' and trailing '$' anchors removed - i.e. "[\\s\\S]*[{\\[].*[}\\]]\\s*". Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member name taken from the request (here the lowerCamel wire name 'inputPayload') and the anchor-stripped pattern taken from the model. The offending value itself is omitted because the member is @sensitive.
Emulator: emits the same sentence but prints the raw anchored regex, "...Member must satisfy regular expression pattern: ^[\\s\\S]*[{\\[].*[}\\]]\\s*$".
Why: a client (or a test) that matches or displays the reported pattern sees a regex AWS never returns; anything that feeds the reported pattern back into a client-side validator gets a different, anchored expression than the service advertises.
Fix: the shared constraint-violation message formatter used by quicksight's request validation - strip the '^' prefix and '$' suffix from the modelled pattern before interpolating it.
Traces: quicksight-automation-job-payload-and-id-constraints-rejected#task#1 (plus task#2, task#3) and quicksight-start-automation-job-unknown-automation-rejected#task#2.

Cause 2 of 4: CancelIngestion - an out-of-range IngestionId must be rejected as InvalidParameterValueException with QuickSight's own message, not as a generic ValidationException.
AWS: Smithy: CancelIngestion.IngestionId is an @httpLabel targeting IngestionId with @length min 1 max 128 and @pattern "^[a-zA-Z0-9-_]+$"; the operation's modeled 400 is InvalidParameterValueException, whose body member is "Message" (capital M) and whose only other member is RequestId. ValidationException is NOT in this operation's modeled error set. Recorded behavior: AWS answers 400 with body key "Message" reading "Invalid IngestionId. Check if it contains invalid characters or exceeds length limit: <ingestionId>" and does not advertise an x-amzn-errortype of ValidationException. Fixed text: "Invalid IngestionId. Check if it contains invalid characters or exceeds length limit: "; variable part: the IngestionId exactly as supplied in the request URI.
Emulator: answers 400 with the generic framework message "1 validation error detected: Value '<ingestionId>' at 'ingestionId' failed to satisfy constraint: Member must have length less than or equal to 128", under the lowercase body key "message", and adds the response header x-amzn-errortype: ValidationException where the recording asserts no such error type.
Why: the SDK dispatches on x-amzn-errortype and on the error shape's members, so a caller's `except InvalidParameterValueException` / `catch InvalidParameterValueException` branch never fires - the error surfaces as an unmodeled/unknown exception - and `e.Message` is empty because the emulator wrote `message`. Error-handling code that is correct against AWS breaks against the emulator.
Fix: CancelIngestion's validation path - validate IngestionId there and raise the modeled InvalidParameterValueException with QuickSight's IngestionId wording, serialized with the "Message" member and the matching x-amzn-errortype, instead of letting the generic ValidationException pre-validator answer.
Traces: quicksight-cancel-ingestion-unknown-dataset-and-invalid-id-rejected#task#3.

Cause 3 of 4: StartDashboardSnapshotJob - a @length violation on a LIST member must print the collection, not an item count.
AWS: Smithy: StartDashboardSnapshotJob.SnapshotConfiguration is @required; SnapshotConfiguration.FileGroups targets SnapshotFileGroupList with @length min 1 max 7, so an empty list violates the minimum. Recorded behavior: AWS answers 400 with "1 validation error detected: Value '[]' at 'snapshotConfiguration.fileGroups' failed to satisfy constraint: Member must have length greater than or equal to 1". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must have length greater than or equal to "; variable parts: the rendered collection value from the request (an empty list renders as the literal []), the dotted wire path of the member, and the modelled minimum.
Emulator: renders the collection as a count instead of a value - "Value '0 items' at 'snapshotConfiguration.fileGroups' failed to satisfy constraint: Member must have length greater than or equal to 1".
Why: the message is a documented part of the error contract; a client that parses or asserts on the reported value sees a phrase ('N items') AWS never emits, and the rendering is wrong for every list/map length violation in the service, not just this member.
Fix: the constraint-violation message formatter's value renderer - render list and map values in AWS's bracketed form rather than as an '<n> items' summary.
Traces: quicksight-dashboard-snapshot-job-on-missing-dashboard-rejected#task#4.

Cause 4 of 4: PredictQAResults - a @range violation message must omit the offending value.
AWS: Smithy: PredictQAResults.MaxTopicsToConsider is an integer with @range min 1 max 4; modeled 400 is InvalidParameterValueException. Recorded behavior: AWS answers 400 with "1 validation error detected: Value at 'maxTopicsToConsider' failed to satisfy constraint: Member must have value less than or equal to 4" - the value-less form of the sentence. Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must have value less than or equal to "; variable parts: the member's wire name from the request and the modelled maximum. No request value appears in the message.
Emulator: interpolates the supplied number - "1 validation error detected: Value '5' at 'maxTopicsToConsider' failed to satisfy constraint: Member must have value less than or equal to 4" (the specific number is just this recording's input; the defect is that any value is printed at all).
Why: the emitted sentence differs from AWS's for every range violation on this operation, so assertions and log-matching that work against AWS fail, and a value the caller marked as part of a sensitive request payload is echoed back in the error body.
Fix: the range-check branch of quicksight's request validation / its message formatter - emit the value-less "Value at '<member>'" form for @range violations.
Traces: quicksight-predict-qa-results-parameter-constraints-rejected#task#1 (plus task#2, task#3).

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up, /__admin/health 200, data listener :38899; 65 steps, 45 passed, 20 mismatched, score 69.2%). All four clusters agree on HTTP status 400 and diverge on the response body / error type:

Cause 1 - fixture quicksight-automation-job-payload-and-id-constraints-rejected step task#1, operation StartAutomationJob, verdict mismatch, 400 -> 400, key $.message:
  expected: "1 validation error detected: Value at 'inputPayload' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\s\\S]*[{\\[].*[}\\]]\\s*"
  actual:   "1 validation error detected: Value at 'inputPayload' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\s\\S]*[{\\[].*[}\\]]\\s*$"
  cluster also covers task#2, task#3 and quicksight-start-automation-job-unknown-automation-rejected#task#2 (affected_count 4).

Cause 2 - fixture quicksight-cancel-ingestion-unknown-dataset-and-invalid-id-rejected step task#3, operation CancelIngestion, verdict mismatch, 400 -> 400:
  $.Message expected: "Invalid IngestionId. Check if it contains invalid characters or exceeds length limit: <ingestionId>"
  actual:   "1 validation error detected: Value '<ingestionId>' at 'ingestionId' failed to satisfy constraint: Member must have length less than or equal to 128" (and written under the lowercase key $.message)
  $headers.x-amzn-errortype expected: <shaped-not-asserted>; actual: "ValidationException" - a type not in CancelIngestion's modeled error set (AccessDenied/InternalFailure/InvalidParameterValue/ResourceExists/ResourceNotFound/Throttling).

Cause 3 - fixture quicksight-dashboard-snapshot-job-on-missing-dashboard-rejected step task#4, operation StartDashboardSnapshotJob, verdict mismatch, 400 -> 400, key $.message:
  expected: "...Value '[]' at 'snapshotConfiguration.fileGroups' failed to satisfy constraint: Member must have length greater than or equal to 1"
  actual:   "...Value '0 items' at 'snapshotConfiguration.fileGroups' failed to satisfy constraint: Member must have length greater than or equal to 1"

Cause 4 - fixture quicksight-predict-qa-results-parameter-constraints-rejected step task#1, operation PredictQAResults, verdict mismatch, 400 -> 400, key $.message:
  expected: "1 validation error detected: Value at 'maxTopicsToConsider' failed to satisfy constraint: Member must have value less than or equal to 4"
  actual:   "1 validation error detected: Value '5' at 'maxTopicsToConsider' failed to satisfy constraint: Member must have value less than or equal to 4"
  cluster also covers task#2 and task#3 (affected_count 3).

Model evidence (services/service-com-amazonaws-quicksight-quicksight-20180401/smithy-model/com-amazonaws-quicksight-quicksight-20180401.json): SensitiveIOPayload @pattern "^[\\s\\S]*[{\\[].*[}\\]]\\s*$" @sensitive; CancelIngestion.IngestionId @length 1..128 @pattern "^[a-zA-Z0-9-_]+$"; InvalidParameterValueException {Message, RequestId} httpError 400; SnapshotConfiguration.FileGroups @required @length 1..7; PredictQAResults.MaxTopicsToConsider @range 1..4.

Note: open issue "QuickSight DescribeDataSource 404 and CreateFolder 409 error bodies emit a synthetic RequestId member AWS leaves out" also concerns quicksight error bodies but covers different operations and the RequestId member, not these message templates or the ValidationException substitution; it does not cover any cause in this bundle.

## Steps to Reproduce
Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1, or drive the four steps directly against the quicksight endpoint:

1. Cause 1 - POST /accounts/<12-digit account>/automation-groups/<uuid>/automations/<uuid>/jobs with an InputPayload that does not match SensitiveIOPayload's @pattern (a string containing no {..} or [..] span). Observe 400 and read $.message: the reported regex still carries the '^' and '$' anchors. Fixture quicksight-automation-job-payload-and-id-constraints-rejected step task#1.
2. Cause 2 - DELETE /accounts/<12-digit account>/data-sets/<dataSetId>/ingestions/<ingestionId> with an IngestionId longer than 128 characters. Observe 400: body carries the lowercase "message" key with the generic "1 validation error detected..." sentence and the header x-amzn-errortype: ValidationException, instead of InvalidParameterValueException with a "Message" member reading "Invalid IngestionId. Check if it contains invalid characters or exceeds length limit: <ingestionId>". Fixture quicksight-cancel-ingestion-unknown-dataset-and-invalid-id-rejected step task#3.
3. Cause 3 - POST /accounts/<12-digit account>/dashboards/<dashboardId>/snapshot-jobs with SnapshotConfiguration.FileGroups set to an empty list. Observe 400 and read $.message: the value renders as '0 items' instead of '[]'. Fixture quicksight-dashboard-snapshot-job-on-missing-dashboard-rejected step task#4.
4. Cause 4 - POST /accounts/<12-digit account>/qa/predict with MaxTopicsToConsider above the modelled maximum of 4. Observe 400 and read $.message: the supplied number is interpolated as "Value '<n>' at 'maxTopicsToConsider'" where AWS emits the value-less "Value at 'maxTopicsToConsider'". Fixture quicksight-predict-qa-results-parameter-constraints-rejected step task#1.

After fixing, re-replay all four fixture/step bindings (issue_trace/issue_step plus the three in trace_ids) - each cause has its own cluster and passing one does not clear the others.
