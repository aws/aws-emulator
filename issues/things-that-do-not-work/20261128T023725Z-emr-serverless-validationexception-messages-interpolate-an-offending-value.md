Issue-Source: trace-sweep-divergence
Issue-Trace: emr-serverless-get-job-run-validation-rejected
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: emr-serverless-get-job-run-validation-rejected emr-serverless-list-job-runs-validation-rejected
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless|GetJobRun|4xx->4xx|$.message
Issue-Key: emr-serverless|ListJobRuns|4xx->4xx|$.message
Issue-Commit: 7da368b21ee7d04c70045aa1c38fe932bbd09ef4

# EMR Serverless ValidationException messages interpolate an offending-value fragment AWS omits

> _Note: this issue was identified against the code base at commit `7da368b21ee7d04c70045aa1c38fe932bbd09ef4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobRun

## Description
Two net-new root causes in how the EMR Serverless input-validation layer renders the body of its 400 ValidationException for a member-constraint violation. In both, the HTTP status (400) and the error code (ValidationException) are correct and only the `message` member diverges: the emulator's template splices a rendered form of the offending VALUE into the per-error clause, where AWS's clause carries only the member path and the constraint, and for a @pattern violation it also prints the model's anchored regex rather than the pattern text AWS reports. A fixer must close BOTH causes before submitting.

Smithy model used throughout: services/service-com-amazonaws-emrserverless-awstoledowebservice/smithy-model/com-amazonaws-emrserverless-awstoledowebservice.json (com.amazonaws.emrserverless#AwsToledoWebService).

Message anatomy shared by both causes. AWS's refusal body is `{"message": "<N> validation error detected: <clause>[; <clause>...]"}`. The fixed text is "<N> validation error(s) detected: ", then per clause "Value at '<memberPath>' failed to satisfy constraint: " followed by the constraint sentence; the only variable parts are N (the number of violated constraints), the member path taken from the request's binding (`jobRunId`, `states`), and the constraint's own literal from the model (the pattern text, the length bound). The offending value itself is NOT part of the clause for these members, and no synthesized stand-in for it is either.

--- Cause 1 of 2: GetJobRun's jobRunId @pattern refusal echoes the value and prints the anchored regex. Fixture emr-serverless-get-job-run-validation-rejected, step task#5 (cluster also covers task#6).
AWS: Smithy GetJobRun input jobRunId -> JobRunId, @httpLabel, @required, @length min 1 max 64, @pattern ^[0-9a-z]+$; modeled errors ValidationException (400), ResourceNotFoundException (404), InternalServerException (500). A jobRunId that violates the pattern is refused 400 ValidationException with the message "1 validation error detected: Value at 'jobRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z]+" (recorded response). Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts: the member path `jobRunId`, and the pattern text, which AWS reports WITHOUT the ^...$ anchors the model carries. The offending id does not appear anywhere in the message.
Emulator: the same call is refused 400 ValidationException with "1 validation error detected: Value '<the rejected id>' at 'jobRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z]+$" — i.e. a "Value '<x>' at" fragment carrying the rejected label value, and the anchored form of the regex. (The specific id in the recording is the fixture's request value, not part of the requirement; the defect is that ANY value is spliced in and that the anchors are printed.)
Why: a client that keys on the refusal text does not match. Pattern-violation handling is commonly asserted or parsed (SDK snapshot tests, retry/repair logic that extracts the constraint to re-prompt a user, log-based alerting), and both extra pieces break it: the value fragment makes the message vary per request so no fixed comparison or regexp anchored on "Value at '<member>'" matches, and the anchored regex is not the expression AWS tells the caller to satisfy, so a client that lifts the pattern out of the message and re-applies it is working from different text than AWS supplies. Echoing a caller-supplied label value back into an error body also re-emits untrusted input AWS deliberately leaves out.
Fix: the ValidationException message rendering in the emr-serverless input-validation layer (the constraint-violation formatter behind the GetJobRun handler's @httpLabel/@pattern checks) — emit the "Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" clause with no value fragment, and report the pattern with the model's leading ^ and trailing $ stripped.
Traces: emr-serverless-get-job-run-validation-rejected#task#5 (representative), emr-serverless-get-job-run-validation-rejected#task#6.

--- Cause 2 of 2: ListJobRuns's states @length refusal substitutes a synthesized "<n> items" value. Fixture emr-serverless-list-job-runs-validation-rejected, step task#7 (cluster also covers task#8, task#9, task#10).
AWS: Smithy ListJobRuns input states -> JobRunStateSet, @httpQuery "states", @uniqueItems, @length min 0 max 8; modeled errors ValidationException (400), InternalServerException (500). A `states` list longer than 8 entries is refused 400 ValidationException with the message "1 validation error detected: Value at 'states' failed to satisfy constraint: Member must have length less than or equal to 8" (recorded response). Fixed text: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must have length less than or equal to ". Variable parts: the member path `states` and the bound 8, taken from the model's @length max. Neither the list's contents nor its element count appears in the message.
Emulator: the same call is refused 400 ValidationException with "1 validation error detected: Value '9 items' at 'states' failed to satisfy constraint: Member must have length less than or equal to 8" — the same spurious "Value '<x>' at" fragment as cause 1, here filled with a synthesized count rendering ("<n> items") the service never emits. The count in the recording is the fixture's request size, not part of the requirement.
Why: same client impact as cause 1, and worse for collection members: the inserted fragment varies with the size of the request, so no message comparison is stable, and "'9 items'" reads as if it were the member's value, which can mislead an operator debugging a rejected filter into thinking the service saw a literal string. A caller that trims its filter on the documented refusal text never recognises it.
Fix: the same constraint-violation formatter in the emr-serverless input-validation layer, on the @length-on-list path behind the ListJobRuns handler's query-parameter validation — render the length clause from the member path and the modeled bound alone, with no rendering of the collection or its size.
Traces: emr-serverless-list-job-runs-validation-rejected#task#7 (representative of this cause), plus task#8, task#9, task#10 of the same fixture.

Relation to open issues: 20261119T140639Z-elasticmapreduce-emr-400-refusal-messages-name-the-wrong-member.md reports refusal-message defects for the *emr* (elasticmapreduce) service, a different service and a different code path, and does not cover emr-serverless. 20261127T222636Z-emr-serverless-6-net-new-root-causes-a.md cause 4 concerns the ListJobRuns unissued-nextToken refusal sentence (a hand-authored message for an unresolvable token), not the generic "<N> validation error detected" constraint template these two causes are about. Neither covers these causes, and no replayed step here was reported as owned.

## Evidence
Replay of simparity-wire-aws-v1 against this checkout: 12 fixtures replayed, 105 steps, 95 passed, 10 mismatched, score 90.5%; emulator_up true (/__admin/health 200). 2 divergence clusters, both emr-serverless, neither carrying stub_path or owned_by.

Cause 1 — [mismatch] fixture emr-serverless-get-job-run-validation-rejected, step task#5 (affected: task#5, task#6), operation GetJobRun, divergence_key service=emr-serverless|GetJobRun|status=400->400|keys=$.message
  Expected status 400, actual status 400 — status and error code agree; the diff is confined to $.message.
  $.message expected (AWS, recorded): "1 validation error detected: Value at 'jobRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z]+"
  $.message actual (emulator):      "1 validation error detected: Value '00GGGGGGGGGGG000' at 'jobRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z]+$"
  Two differences: (a) the inserted "Value '<request value>' at" fragment — '00GGGGGGGGGGG000' is this fixture's request value and is not part of the requirement; (b) the pattern printed as ^[0-9a-z]+$ instead of [0-9a-z]+.

Cause 2 — [mismatch] fixture emr-serverless-list-job-runs-validation-rejected, step task#7 (affected: task#7, task#8, task#9, task#10), operation ListJobRuns, divergence_key service=emr-serverless|ListJobRuns|status=400->400|keys=$.message
  Expected status 400, actual status 400 — again only $.message diverges.
  $.message expected (AWS, recorded): "1 validation error detected: Value at 'states' failed to satisfy constraint: Member must have length less than or equal to 8"
  $.message actual (emulator):      "1 validation error detected: Value '9 items' at 'states' failed to satisfy constraint: Member must have length less than or equal to 8"
  One difference: the inserted "Value '9 items' at" fragment; the '9' is this fixture's request list size, the requirement is that no value fragment is emitted at all. The bound 8 is the model's @length max and is rendered correctly.

Model corroboration (smithy_query, services/service-com-amazonaws-emrserverless-awstoledowebservice/smithy-model/com-amazonaws-emrserverless-awstoledowebservice.json): GetJobRun.jobRunId -> JobRunId @httpLabel @required @length 1..64 @pattern ^[0-9a-z]+$, errors ValidationException(400)/ResourceNotFoundException(404)/InternalServerException(500); ListJobRuns.states -> JobRunStateSet @httpQuery states @uniqueItems @length 0..8, errors ValidationException(400)/InternalServerException(500).

## Steps to Reproduce
Boot the emulator from this checkout and replay the two fixtures named in Issue-Trace / Issue-Trace-Ids (emr-serverless-get-job-run-validation-rejected#task#5 and emr-serverless-list-job-runs-validation-rejected#task#7). By hand, with botocore pointed at the emulator's data listener:

Cause 1 (jobRunId pattern refusal):
1. emr-serverless CreateApplication(releaseLabel='emr-7.13.0', type='SPARK'); poll GetApplication until state CREATED and keep its applicationId.
2. Call GetJobRun(applicationId=<that id>, jobRunId='00GGGGGGGGGGG000') — any value that satisfies @length 1..64 but violates @pattern ^[0-9a-z]+$ (uppercase letters here) works.
3. Expect HTTP 400 ValidationException with message exactly "1 validation error detected: Value at 'jobRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-z]+" — no value fragment, pattern printed unanchored.
4. Observe HTTP 400 ValidationException with "1 validation error detected: Value '00GGGGGGGGGGG000' at 'jobRunId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-z]+$".
5. Repeat with a different invalid id and confirm the emulator's message changes with the request value while AWS's does not.

Cause 2 (states length refusal):
1. Using the same applicationId, call ListJobRuns(applicationId=<that id>, states=[...]) with 9 distinct JobRunState values (e.g. SUBMITTED, PENDING, SCHEDULED, RUNNING, SUCCESS, FAILED, CANCELLING, CANCELLED, QUEUED) — one more than the @length max of 8.
2. Expect HTTP 400 ValidationException with message exactly "1 validation error detected: Value at 'states' failed to satisfy constraint: Member must have length less than or equal to 8".
3. Observe HTTP 400 ValidationException with "1 validation error detected: Value '9 items' at 'states' failed to satisfy constraint: Member must have length less than or equal to 8".
4. Confirm a legal filter of 8 or fewer states is still accepted.
