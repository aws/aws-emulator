Issue-Source: trace-sweep-divergence
Issue-Trace: iotjobsdata-describe-job-execution-malformed-request-refusals
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotjobsdata-describe-job-execution-malformed-request-refusals iotjobsdata-pending-executions-thing-name-refusals iotjobsdata-start-next-pending-validation-refusals
Issue-Shape-Id: com.amazonaws.iotjobsdataplane#IotLaserThingJobManagerExternalService
Issue-Key: iot-jobs-data|GetPendingJobExecutions|4xx->4xx|$.message
Issue-Key: iot-jobs-data|StartNextPendingJobExecution|4xx->4xx|$.message
Issue-Related: 20261128T073933Z-iot-jobs-data-describejobexecution-answers-an-out-of.md
Issue-Commit: 4866adc0993f8fa67cc0ae020b08b6d4f21694d6

# iot-jobs-data validation refusals diverge from AWS constraint-message wording

> _Note: this issue was identified against the code base at commit `4866adc0993f8fa67cc0ae020b08b6d4f21694d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeJobExecution

## Description
Three request-validation refusals in iot-jobs-data return the right HTTP status but a message body that is not what AWS words for that constraint class: two echo the Smithy-anchored regex literal, and one replaces AWS's aggregated map-key constraint sentence with the emulator's own prose. The error message is part of the contract, so each is a divergence.

Shared fault: the constraint-violation message renderer for this service emits the raw model pattern and hand-written sentences instead of AWS's "<N> validation error detected: ..." template.

Cause 1 of 3 — DescribeJobExecution thingName @pattern refusal quotes the anchored regex.
AWS: thingName is an httpLabel targeting ThingName with @length min 1 max 128 and @pattern "^[a-zA-Z0-9:_-]+$" (Smithy: DescribeJobExecution.thingName -> ThingName). AWS's pattern refusal is 400 with the template "1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+". Fixed text: "<N> validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy constraint: Member must satisfy regular expression pattern: " and the regex body AWS publishes, which is the UNANCHORED form [a-zA-Z0-9:_-]+. Variable parts: the submitted value and the member name, both taken from the request.
Emulator: same status and template but the regex is printed with the model's anchors, "^[a-zA-Z0-9:_-]+$" (recorded run: value 'not.a.thing' at 'thingName').

Cause 2 of 3 — GetPendingJobExecutions thingName @pattern refusal quotes the anchored regex, identically.
AWS: GetPendingJobExecutions.thingName -> ThingName, @length 1..128, @pattern "^[a-zA-Z0-9:_-]+$" (Smithy); refusal message as in Cause 1, with the regex rendered unanchored.
Emulator: emits "... Member must satisfy regular expression pattern: ^[a-zA-Z0-9:_-]+$" (recorded run: value 'sim2real-probe-ijd.dot-430cfe60' at 'thingName').

Cause 3 of 3 — StartNextPendingJobExecution statusDetails map-key refusal uses emulator prose, not AWS's aggregated key-constraint message.
AWS: statusDetails targets DetailsMap, whose key DetailsKey carries @length min 1 max 128 and @pattern "^[a-zA-Z0-9:_-]+$" (Smithy: DetailsMap.key -> DetailsKey). A key violating those constraints is refused 400 with AWS's aggregated map-key form: "1 validation error detected: Value '{<key>=<value>}' at 'statusDetails' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+]". Fixed text: the "validation error detected" preamble, "at 'statusDetails' failed to satisfy constraint: Map keys must satisfy constraint:" and the bracketed list of all three key clauses in that order, with the regex unanchored. Variable parts: the offending map entry rendered as {key=value} and the member path, from the request.
Emulator: answers 400 but with its own single sentence of the form "the statusDetails name <key> is not 1..128 characters matching ^[a-zA-Z0-9:_-]+$" — it names no validation-error count, does not render the offending entry as AWS does, collapses the three key clauses into one, and again prints the anchored regex.

Why: clients, test suites and SDK-level error parsers match on these sentences to tell which member and which constraint failed, and tooling that surfaces the message to an operator shows text that does not exist on AWS. The anchored regex is especially misleading: it is the service's internal model literal, so a caller that copies the quoted pattern to pre-validate thing names applies a regex AWS never publishes. The collapsed map-key sentence hides that length and pattern are separate constraints on the key.

Fix: the shared constraint-violation serializer/validation layer for this service — render @pattern values in the unanchored form AWS publishes, and build map-key refusals from the aggregated "Map keys must satisfy constraint: [...]" template rather than a per-operation hand-written sentence. StartNextPendingJobExecution's statusDetails check belongs in that same request-validation layer, before any state read.

Related but not covering this: open issue 20261127T010512Z-iot-jobs-data-plane-updatejobexecution-error-response--r2 concerns UpdateJobExecution evaluating statusDetails after the stored execution is read, and 20261127T010512Z-iot-jobs-data-plane-startnextpendingjobexecution-maintainability concerns duplicated write sequencing; neither addresses the refusal wording above.

Traces: iotjobsdata-describe-job-execution-malformed-request-refusals#task#7 (Cause 1), iotjobsdata-pending-executions-thing-name-refusals#task#8 (Cause 2), iotjobsdata-start-next-pending-validation-refusals#task#7 (Cause 3).

## Evidence
Cause 1 — iotjobsdata-describe-job-execution-malformed-request-refusals#task#7, status 400 both sides:
  expected $.message = "1 validation error detected: Value 'not.a.thing' at 'thingName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+"
  actual   $.message = "1 validation error detected: Value 'not.a.thing' at 'thingName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9:_-]+$"
Cause 2 — iotjobsdata-pending-executions-thing-name-refusals#task#8 (GetPendingJobExecutions), status 400 both sides:
  expected $.message = "1 validation error detected: Value 'sim2real-probe-ijd.dot-430cfe60' at 'thingName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+"
  actual   same sentence with "^[a-zA-Z0-9:_-]+$"
Cause 3 — iotjobsdata-start-next-pending-validation-refusals#task#7 (StartNextPendingJobExecution), status 400 both sides:
  expected $.message = "1 validation error detected: Value '{bad key=fetching}' at 'statusDetails' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [a-zA-Z0-9:_-]+]"
  actual   $.message = "the statusDetails name bad key is not 1..128 characters matching ^[a-zA-Z0-9:_-]+$"
(The quoted thing names, map entry and job ids are per-run fixture values filling the templates; the divergence is in the fixed text and in the rendered regex.)

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay iotjobsdata-describe-job-execution-malformed-request-refusals and inspect step task#7: GET /things/not.a.thing/jobs/{jobId} — the 400 body quotes the anchored pattern ^[a-zA-Z0-9:_-]+$.
3. Replay iotjobsdata-pending-executions-thing-name-refusals, step task#8: GET /things/<name containing a dot>/jobs — same anchored pattern in the 400 body.
4. Replay iotjobsdata-start-next-pending-validation-refusals, step task#7: PUT /things/{thingName}/jobs/$next with statusDetails containing a key that has a space (e.g. "bad key") — the 400 body is the emulator's own sentence instead of AWS's aggregated "Map keys must satisfy constraint: [...]" message.
