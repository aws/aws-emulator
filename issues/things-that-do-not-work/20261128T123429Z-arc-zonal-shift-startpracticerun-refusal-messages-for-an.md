Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-zonal-shift.start-practice-run-away-from-zone-outside-resource-rejected arc-zonal-shift.start-practice-run-on-blocked-date-rejected
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift.start-practice-run-away-from-zone-outside-resource-rejected|run#13|AssertionError|assert (code, message) == expected, (
Issue-Key: arc-zonal-shift.start-practice-run-on-blocked-date-rejected|run#12|AssertionError|assert (code, message) == expected, f"expected {expected!r}, got {(code, message)!r}"
Issue-Commit: b880698c4f2cb4e70545429d30696ac71b7bccc0

# arc-zonal-shift StartPracticeRun refusal messages for an unsupported awayFrom zone and for a blocked date do not match AWS's wording

> _Note: this issue was identified against the code base at commit `b880698c4f2cb4e70545429d30696ac71b7bccc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPracticeRun

## Description
Two root causes, both in the StartPracticeRun refusal path of the arc-zonal-shift service. In both the HTTP status and the error code are correct and the refusal itself (no shift is created, the later unblocked call succeeds) is correct; only the serialized `message` string diverges from AWS's wording for that error class. A sibling divergence of the same kind on a different error class is already filed as 20261121T012137Z-arc-zonal-shift-startpracticerun-conflictexception-message-for-a.md (red blocking alarm, ConflictException/PracticeBlockingAlarmsRed); that issue does NOT cover either cause below, which are distinct error classes with distinct messages.

Cause 1 of 2 — ValidationException message when `awayFrom` names an Availability Zone the managed resource is not managed for.
Failing check: arc-zonal-shift.start-practice-run-away-from-zone-outside-resource-rejected.
AWS: Smithy (services/service-com-amazonaws-arczonalshift-percdataplane) models StartPracticeRun (POST /practiceruns, 200) with error ValidationException @httpError 400, members `message` (String, @required) and `reason` (ValidationExceptionReason, @required). ValidationExceptionReason includes `UnsupportedAz`, which is the reason for this refusal (an AZ of the Region that the load balancer does not use). The model does not document the message text, so for the text this is recorded behavior only: the recording shows AWS answering HTTP 400, code `ValidationException`, reason `UnsupportedAz`, message `Cannot start a zonal shift away from the requested availability zone because the resource is not managed for it.` That sentence is entirely FIXED text for this error class — it interpolates nothing from the request or from state (no zone id, no resource identifier, no member name), so the whole string is the requirement and nothing in it is per-run.
Emulator: for the identical call the emulator answers code `ValidationException` with the message `Invalid availability zone.` — a different, generic sentence, not a differently-filled template.
Why: the request is not malformed, it is unsupported for this resource. A client (or operator runbook, alerting rule, or retry policy) that distinguishes "this AZ is not one the resource is managed for" from "the awayFrom value is syntactically invalid" by the message sees a string AWS never returns, and reports an operator-fixable configuration mismatch as a malformed-input bug. The message is also the only discriminator for clients that do not read the `reason` member.
Fix: the error-construction site of the awayFrom/managed-zone precondition in the StartPracticeRun handler of the arc-zonal-shift service crate — the precondition detection and the 400/ValidationException classification are already correct; only the literal text is wrong. While that site is edited, confirm the response carries `reason = UnsupportedAz`, which the model marks @required (the check asserts it immediately after the message, so its current value is unobserved).
Traces: arc-zonal-shift.start-practice-run-away-from-zone-outside-resource-rejected. Everything else the check asserts up to that point passes (the NLB registers with ARC, CreatePracticeRunConfiguration succeeds, and after the message assertion it goes on to require that no shift was created and that StartPracticeRun away from a zone the load balancer does use returns ACTIVE and is listed).

Cause 2 of 2 — ConflictException message when today's UTC date is a blocked date of the practice run configuration.
Failing check: arc-zonal-shift.start-practice-run-on-blocked-date-rejected.
AWS: Smithy models StartPracticeRun with error ConflictException @httpError 409, members `message` (String, @required), `reason` (ConflictExceptionReason, @required) and optional `zonalShiftId`. ConflictExceptionReason includes `PracticeInBlockedDates`, the reason for this refusal. The model does not document the message text, so for the text this is recorded behavior only: the recording shows AWS answering HTTP 409, code `ConflictException`, reason `PracticeInBlockedDates`, message `Cannot start practice run during blocked dates`. That message is entirely FIXED text — it does not interpolate the blocked date, today's date, or the resource identifier — so the whole string is the requirement.
Emulator: for the identical call the emulator answers code `ConflictException` with the message `Cannot start a practice run because today is a blocked date for the requested resource.`
Why: a client that branches on, logs, or asserts on the conflict message — the usual way to tell a blocked-date conflict from the other eleven ConflictExceptionReason conflicts when `reason` is not inspected — sees a sentence it has never seen from AWS, so suppression windows and scheduling logic that match AWS's wording silently stop matching.
Fix: the error-construction site of the blocked-dates precondition in the StartPracticeRun handler of the arc-zonal-shift service crate. The precondition itself is right (the blocked dates round-trip through GetManagedResource, no shift is created while blocked, and StartPracticeRun succeeds once the blocked date is moved a year out); only the emitted text is wrong. Confirm `reason = PracticeInBlockedDates` is carried as the model requires.
Traces: arc-zonal-shift.start-practice-run-on-blocked-date-rejected.

## Evidence
Cause 1 — check arc-zonal-shift.start-practice-run-away-from-zone-outside-resource-rejected, failing assertion at check_arc_zonal_shift_start_practice_run_away_from_zone_outside_resource_rejected.py line 176:

  AssertionError: expected ('ValidationException', 'Cannot start a zonal shift away from the requested availability zone because the resource is not managed for it.') for awayFrom 'usw2-az3' outside the load balancer, got ('ValidationException', 'Invalid availability zone.')

  Expected (AWS, from the recording):
    HTTP 400
    Error.Code    = ValidationException
    Error.Message = "Cannot start a zonal shift away from the requested availability zone because the resource is not managed for it."  (fixed text in full; no request- or state-derived values)
    reason        = UnsupportedAz   (Smithy: ValidationException.reason is @required, enum ValidationExceptionReason)
  Actual (emulator):
    Error.Code    = ValidationException   (correct)
    Error.Message = "Invalid availability zone."   (diverges)
  (The zone id 'usw2-az3' in the assertion text is the per-run value the check happened to pick — it is NOT part of the message; AWS's message contains no zone id.)

Cause 2 — check arc-zonal-shift.start-practice-run-on-blocked-date-rejected, failing assertion at check_arc_zonal_shift_start_practice_run_on_blocked_date_rejected.py line 181:

  AssertionError: expected ('ConflictException', 'Cannot start practice run during blocked dates'), got ('ConflictException', 'Cannot start a practice run because today is a blocked date for the requested resource.')

  Expected (AWS, from the recording):
    HTTP 409
    Error.Code    = ConflictException
    Error.Message = "Cannot start practice run during blocked dates"  (fixed text in full; the blocked date itself is not interpolated)
    reason        = PracticeInBlockedDates   (Smithy: ConflictException.reason is @required, enum ConflictExceptionReason)
  Actual (emulator):
    Error.Code    = ConflictException   (correct)
    Error.Message = "Cannot start a practice run because today is a blocked date for the requested resource."   (diverges)

In both checks every other assertion reached before the message comparison passed: the NLB was created, opted in and registered with ARC (GetManagedResource showed appliedWeights for both zones), CreatePracticeRunConfiguration succeeded, and in cause 2 GetManagedResource returned exactly the blockedDates that were configured.

## Steps to Reproduce
Shared setup (both causes):
1. Create an internal ELBv2 network load balancer across two subnets in two Availability Zones, wait for it to be available, and set its attribute zonal_shift.config.enabled = true.
2. Poll arc-zonal-shift GetManagedResource(resourceIdentifier=<lb arn>) until appliedWeights has both zones (the load balancer is registered with ARC).
3. Create a CloudWatch metric alarm on an unpublished metric with TreatMissingData=notBreaching and poll DescribeAlarms until StateValue == OK; use its ARN as the outcome alarm.

Cause 1 (unsupported awayFrom zone):
4. arc-zonal-shift CreatePracticeRunConfiguration(resourceIdentifier=<lb arn>, outcomeAlarms=[{type: CLOUDWATCH, alarmIdentifier: <outcome alarm arn>}]).
5. ec2 DescribeAvailabilityZones(Filters=[{Name: zone-type, Values: [availability-zone]}]) and pick a ZoneId of the Region that is NOT one of the load balancer's two zones.
6. Call arc-zonal-shift StartPracticeRun(resourceIdentifier=<lb arn>, awayFrom=<that unused zone id>, comment="away from an unused zone").
7. Observe HTTP 400 / ValidationException as expected, but Error.Message = "Invalid availability zone." instead of AWS's "Cannot start a zonal shift away from the requested availability zone because the resource is not managed for it."; also check that reason == UnsupportedAz.

Cause 2 (blocked date):
4. arc-zonal-shift CreatePracticeRunConfiguration(resourceIdentifier=<lb arn>, outcomeAlarms=[{type: CLOUDWATCH, alarmIdentifier: <outcome alarm arn>}], blockedDates=[yesterday, today, tomorrow in UTC ISO-8601 dates]).
5. Call arc-zonal-shift StartPracticeRun(resourceIdentifier=<lb arn>, awayFrom=<one of the load balancer's zones>, comment="on a blocked date").
6. Observe HTTP 409 / ConflictException as expected, but Error.Message = "Cannot start a practice run because today is a blocked date for the requested resource." instead of AWS's "Cannot start practice run during blocked dates"; also check that reason == PracticeInBlockedDates.
7. UpdatePracticeRunConfiguration(resourceIdentifier=<lb arn>, blockedDates=[a date ~366 days ahead]) and repeat the StartPracticeRun call: it succeeds with status ACTIVE, confirming only the message text is at fault.
