Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-start-protected-job-validation-surface
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-start-protected-job-validation-surface
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|StartProtectedJob|4xx->4xx|$.fieldList
Issue-Key: cleanrooms|StartProtectedJob|4xx->4xx|$.message
Issue-Key: cleanrooms|StartProtectedJob|4xx->4xx|$.reason
Issue-Commit: ac2180ebc98f9a8a7fe48c15d1d2a1999d7a4a92

# cleanrooms ValidationException omits reason and fieldList and uses generic validation wording

> _Note: this issue was identified against the code base at commit `ac2180ebc98f9a8a7fe48c15d1d2a1999d7a4a92`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartProtectedJob

## Description
The Clean Rooms ValidationException body the emulator serves is not AWS's: it carries only a generic `message` and omits the modeled `reason` and `fieldList` members entirely, and its message text uses a different fixed template.

Cause 1 of 1: StartProtectedJob rejects an out-of-range computeConfiguration.worker.number with a ValidationException whose body is missing `reason` and `fieldList`, and whose `message` uses the generic "N validation error(s) detected: ..." wording instead of Clean Rooms' "The following request parameter is invalid: ..." wording.

AWS: Smithy (com.amazonaws.cleanrooms) — StartProtectedJob POST /memberships/{membershipIdentifier}/protectedJobs, errors include ValidationException (client, httpError 400). Shape ValidationException has members `message`, `reason` (enum ValidationExceptionReason: FIELD_VALIDATION_FAILED | INVALID_CONFIGURATION | INVALID_QUERY | IAM_SYNCHRONIZATION_DELAY, documented "A reason code for the exception") and `fieldList` (ValidationExceptionFieldList, documented "Validation errors for specific input parameters"), each ValidationExceptionField carrying required `name` ("The name of the input parameter") and required `message` ("A message for the input validation error"). WorkerComputeConfiguration.number is @range(min 2, max 1024) with documented engine-specific minima ("PySpark jobs support a minimum value of 4"), which is the bound the recorded rejection enforces. So for a field-constraint failure AWS answers HTTP 400, error code ValidationException, with all three members populated: reason = FIELD_VALIDATION_FAILED, fieldList = one entry per violating member, and a message whose fixed text is "The following request parameter is invalid: " + the per-field constraint sentence + "." (trailing period). The per-field sentence itself has fixed text "Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have value greater than or equal to <limit>", where <value> is the rejected value taken from the request, <memberPath> is the member's full input path and <limit> is the modeled/engine minimum — all three are variables, not literals; fieldList[].name is the leaf member name (here the leaf of that path).

Emulator: returns the correct status 400, but the body contains only `message`, set to the generic Amazon-style aggregate "1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must have value greater than or equal to <limit>" — no leading "The following request parameter is invalid: ", no trailing period — and omits `reason` (expected 'FIELD_VALIDATION_FAILED', got None) and `fieldList` (expected one {name, message} entry for the violating member, got None).

Why: a client that reads ValidationException.reason to distinguish a field-constraint failure from INVALID_CONFIGURATION/INVALID_QUERY/IAM_SYNCHRONIZATION_DELAY sees no reason code at all and cannot branch; a client (console, SDK-driven form validation, retry logic) that walks fieldList to attribute the error to a specific input parameter gets nothing and must fall back to parsing prose, and that prose does not match AWS's template either, so string matching on the documented wording also fails. Field-level error reporting and reason-based error handling are both silently broken for every Clean Rooms validation failure, not just this member.

Fix: the cleanrooms validation/error-serialization layer that builds ValidationException — it must populate `reason` with the appropriate ValidationExceptionReason (FIELD_VALIDATION_FAILED for member-constraint violations) and emit a `fieldList` entry per violating member with that member's name and its constraint sentence, and wrap the aggregate message in Clean Rooms' "The following request parameter is invalid: ..." template rather than the generic "N validation error(s) detected: ..." one. This is shared error shaping, so the same fix must apply to StartProtectedJob's validation path and the service's other input-validated operations. Related but distinct open issues on the sibling service: 20261127T195958Z-cleanroomsml-validationexception-message-echoes-the-rejected-name-value.md and 20261127T195958Z-cleanroomsml-start-jobs-validate-input-before-authorization-returning.md — neither covers cleanrooms' missing reason/fieldList.

Traces: cleanrooms-start-protected-job-validation-surface#task#3 and #task#4 (same cluster).

## Evidence
Fixture cleanrooms-start-protected-job-validation-surface, step task#3 (cluster also covers task#4), operation StartProtectedJob.

Status: expected 400, got 400 (status agrees).

Body diff reported by replay:
  $.reason: missing — expected 'FIELD_VALIDATION_FAILED', got None
  $.fieldList: missing — expected [{'name': 'number', 'message': "Value '3' at 'computeConfiguration.worker.number' failed to satisfy constraint: Member must have value greater than or equal to 4"}], got None
  $.message: value_mismatch
    expected: "The following request parameter is invalid: Value '3' at 'computeConfiguration.worker.number' failed to satisfy constraint: Member must have value greater than or equal to 4."
    actual:   "1 validation error detected: Value '3' at 'computeConfiguration.worker.number' failed to satisfy constraint: Member must have value greater than or equal to 4"

Template vs values: the fixed text of the AWS message is "The following request parameter is invalid: " ... " failed to satisfy constraint: Member must have value greater than or equal to " ... "."; the variables are the rejected value (3 — from the request), the member path (computeConfiguration.worker.number — from the input shape) and the limit (4 — the PySpark worker minimum documented on WorkerComputeConfiguration.number). fieldList[].name is the leaf member name (number). The emulator's differing part is the fixed text, not the values.

Model citations: ValidationException{message, reason: ValidationExceptionReason, fieldList: ValidationExceptionFieldList}; ValidationExceptionField{name required, message required}; ValidationExceptionReason values FIELD_VALIDATION_FAILED, INVALID_CONFIGURATION, INVALID_QUERY, IAM_SYNCHRONIZATION_DELAY; WorkerComputeConfiguration.number @range(min=2,max=1024), doc "PySpark jobs support a minimum value of 4".

## Steps to Reproduce
1. git_clone() and boot the emulator built from the checkout.
2. Replay fixture cleanrooms-start-protected-job-validation-surface, step task#3 (and task#4): POST /memberships/{membershipIdentifier}/protectedJobs with computeConfiguration.worker.number = 3 (below the PySpark minimum of 4).
3. Observe HTTP 400 ValidationException. Inspect the JSON body: it contains only `message`, with the generic "1 validation error detected: ..." prefix and no trailing period.
4. Expected (AWS): the same 400 ValidationException with `reason` = "FIELD_VALIDATION_FAILED", a `fieldList` holding one entry {name: "number", message: "<the per-field constraint sentence>"}, and `message` = "The following request parameter is invalid: <the per-field constraint sentence>.".
