Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: arc-zonal-shift.delete-practice-run-configuration-while-autoshift-enabled-rejected
Issue-Shape-Id: com.amazonaws.arczonalshift#PercDataPlane
Issue-Key: arc-zonal-shift.delete-practice-run-configuration-while-autoshift-enabled-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: a4d278f1279dca95bd1b1693fbff7a33c3289259

# arc-zonal-shift: DeletePracticeRunConfiguration autoshift-ENABLED refusal answers emulator-invented prose instead of AWS's fixed ConflictException message

> _Note: this issue was identified against the code base at commit `a4d278f1279dca95bd1b1693fbff7a33c3289259`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeletePracticeRunConfiguration

## Description
One net-new root cause in arc-zonal-shift, found by replaying the behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout. THIS BUNDLE HAS 1 CAUSE.

Cause 1 of 1: DeletePracticeRunConfiguration, refused because zonal autoshift is still ENABLED for the resource, carries the wrong error message.

AWS: Smithy (services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json): DeletePracticeRunConfiguration is DELETE /configuration/{resourceIdentifier} (code 200), input resourceIdentifier (@required, @httpLabel, @length 8..1024), and it models ConflictException (errorKind client, httpError 409) whose members are message (@required), reason (@required, ConflictExceptionReason) and zonalShiftId. The model states nothing about message composition, so recorded behavior only for the text: while zonalAutoshiftStatus is ENABLED for the resource, the delete is refused with 409 ConflictException and the message "Zonal autoshift is ENABLED. Cannot delete a practice run configuration for the requested resource." That message class is entirely FIXED text — it has no variable parts: no resource ARN, no configuration id, and the autoshift status token appears as the literal word ENABLED in the first sentence, not as a value formatted from state in prose of the emulator's own design.

Emulator: the HTTP status and the error code are correct (409 ConflictException); the sole divergence is the message, which is 'Zonal autoshift is enabled for the resource. Cannot delete the practice run configuration.' — a paraphrase the emulator composes itself (lower-cased "enabled", "for the resource" in the first sentence, and a definite-article second sentence) rather than AWS's fixed sentence pair.

Why: an error message is a requirement. A client, contract test, UI or log pipeline that classifies this refusal by AWS's wording never sees the sentence AWS emits, so a correct-status, correct-code refusal is unrecognizable; retry/remediation logic keyed on "Cannot delete a practice run configuration" does not fire. Because the check stops at the message assertion, the refusal's downstream guarantees (configuration still present, and the delete succeeding after autoshift is DISABLED) are unverified behind this divergence.

Fix: the DeletePracticeRunConfiguration handler's autoshift-status precondition branch (the ENABLED guard) in the arc-zonal-shift service crate must emit the fixed sentence rather than formatting descriptive prose from state. This is a NEW message path: the open issue 20261109T190734Z-arc-zonal-shift-startzonalshift-cancelzonalshift-updatezonalshift-refusal-messages.md carries the same invented-prose fault for StartZonalShift/CancelZonalShift/UpdateZonalShift only, and 20261109T145139Z-arc-zonal-shift-validationexception-messages-interpolate-the-rejected.md covers the shared ValidationException formatter; neither names DeletePracticeRunConfiguration or its ConflictException.

Traces: arc-zonal-shift.delete-practice-run-configuration-while-autoshift-enabled-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, replayed against the emulator built from this checkout (branch issues/finder-wi-06fc719c0d5e48748eda474ac8a0ab61-1794610162867): 14/26 checks passed; arc-zonal-shift 14/23 with 9 failures, of which 8 are marked ALREADY FILED (unimplemented CancelPracticeRun / StartPracticeRun stubs) or OWNED by open issues. The remaining failure is this one.

[mismatch] arc-zonal-shift.delete-practice-run-configuration-while-autoshift-enabled-rejected
  code: ConflictException — match
  message: expected 'Zonal autoshift is ENABLED. Cannot delete a practice run configuration for the requested resource.'
           actual   'Zonal autoshift is enabled for the resource. Cannot delete the practice run configuration.'
  AssertionError raised at check line 38: assert message == expected_message

Fixed vs variable parts: the entire expected message is fixed text (both sentences, including the upper-case status word ENABLED and the trailing period). No part of it is taken from the request or from emulator state — the resource ARN under test, the generated resource name and the configuration's alarm ARN must not appear in it, and do not appear in AWS's recorded message.

Model evidence (smithy_query on services/service-com-amazonaws-arczonalshift-percdataplane/smithy-model/com-amazonaws-arczonalshift-percdataplane.json):
  DeletePracticeRunConfiguration: DELETE /configuration/{resourceIdentifier} (code 200), idempotent; input {resourceIdentifier required, httpLabel, length 8..1024}; output {arn, name, zonalAutoshiftStatus all required}; errors AccessDeniedException 403, ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400.
  UpdateZonalAutoshiftConfiguration: PUT /managedresources/{resourceIdentifier} (code 200); input {resourceIdentifier required, zonalAutoshiftStatus required enum} — used to put the resource into the ENABLED state this refusal depends on.
  ConflictException: errorKind client, httpError 409, members message (required), reason (required, ConflictExceptionReason), zonalShiftId. The model says nothing about message text, hence "recorded behavior only" above.

## Steps to Reproduce
1. Clone this checkout, build and boot the emulator, then replay the behavioral suite simparity-behavioral-aws-v1 (service arc-zonal-shift).
2. Create an NLB that ARC registers as a managed resource, and a CloudWatch alarm in OK state to use as the outcome alarm.
3. arc.create_practice_run_configuration(resourceIdentifier=<lb arn>, outcomeAlarms=[{type: CLOUDWATCH, alarmIdentifier: <alarm arn>}]).
4. arc.update_zonal_autoshift_configuration(resourceIdentifier=<lb arn>, zonalAutoshiftStatus='ENABLED').
5. arc.delete_practice_run_configuration(resourceIdentifier=<lb arn>). Expect HTTP 409 ConflictException with the message exactly 'Zonal autoshift is ENABLED. Cannot delete a practice run configuration for the requested resource.' Observe code ConflictException but message 'Zonal autoshift is enabled for the resource. Cannot delete the practice run configuration.'
6. Also confirm the refusal's effects, which this check cannot reach today: get_managed_resource(resourceIdentifier=<lb arn>) must still report practiceRunConfiguration, and after update_zonal_autoshift_configuration(..., zonalAutoshiftStatus='DISABLED') the delete_practice_run_configuration must succeed and leave practiceRunConfiguration absent.
7. Re-replay to confirm the fix: arc-zonal-shift.delete-practice-run-configuration-while-autoshift-enabled-rejected.
