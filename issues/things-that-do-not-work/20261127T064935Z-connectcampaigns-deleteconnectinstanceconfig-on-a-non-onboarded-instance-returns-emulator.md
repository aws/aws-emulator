Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaigns.instance-onboarding-job-lifecycle
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connectcampaigns.instance-onboarding-job-lifecycle|run#22|AssertionError|assert double_delete == ("ResourceNotFoundException", _NOT_ONBOARDED), double_delete
Issue-Commit: 7fb2ed4170396fe0a0c384ff71365204f905b96a

# DeleteConnectInstanceConfig on a non-onboarded instance returns emulator-authored not-found text instead of AWS's 'Instance is not onboarded'

> _Note: this issue was identified against the code base at commit `7fb2ed4170396fe0a0c384ff71365204f905b96a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteConnectInstanceConfig

## Description
ONE cause on the ConnectCampaigns v1 service (Smithy model services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/smithy-model — AmazonConnectCampaignService).

Cause 1 of 1: a repeat DeleteConnectInstanceConfig, issued after the instance's campaigns config has already been deleted (so the instance is no longer onboarded), is refused with the correct error code but the wrong message text. Failing check id: connectcampaigns.instance-onboarding-job-lifecycle.

AWS: Smithy — DeleteConnectInstanceConfig is DELETE /connect-instance/{connectInstanceId}/config; input connectInstanceId is @required @httpLabel, target InstanceId @length 0..256, @pattern ^[a-zA-Z0-9_\-.]*$; modeled errors AccessDeniedException(403), InternalServerException(500), InvalidStateException(409), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400); the operation is @idempotent and the success body is empty. The model states no message text, so the wording is recorded behavior only: AWS answers HTTP 404 / ResourceNotFoundException with message 'Instance is not onboarded'. That string is ENTIRELY FIXED text for this error class — it interpolates nothing from the request and in particular does NOT name the instance id. (Note the deletion is not a silent success: the second delete must still be refused.)

Emulator: returns the right error code and class (ResourceNotFoundException) but message 'ConnectInstanceConfig not found <connectInstanceId>.' — a different template whose fixed parts are 'ConnectInstanceConfig not found ' and the trailing '.', and whose single variable part is the connectInstanceId echoed from the request's httpLabel. The uuid seen in one replay (4cd1aa0d-5ae8-497f-901e-e5a6892bcee8) is that run's request value, not part of the template. Replay output: AssertionError ('ResourceNotFoundException', 'ConnectInstanceConfig not found 4cd1aa0d-5ae8-497f-901e-e5a6892bcee8.') where ('ResourceNotFoundException', 'Instance is not onboarded') was expected.

Why: the message is part of the contract for an error class. A client that matches, logs or surfaces the refusal reason gets an internal state-store noun ('ConnectInstanceConfig') plus a resource id, where AWS states a condition about the instance's onboarding state. Message assertions and operator-facing diagnostics diverge, and the emulator leaks an internal type name AWS never exposes.

Fix: the DeleteConnectInstanceConfig handler's not-found refusal path for connect-campaigns — where the ResourceNotFoundException is constructed — must emit the service's fixed wording for a not-onboarded instance rather than the generic '<StateType> not found <id>.' formatter shared with GetConnectInstanceConfig (GetConnectInstanceConfig's own not-found message 'ConnectInstanceConfig not found <id>.' is what this check asserts and accepts, so the two paths must not share one formatter).

Related open issue: 20261124T184219Z-connect-campaigns-connectcampaigns-v1-error-text-and-batch-failure-reasons.md describes the same wording defect from the WIRE corpus (its Cause 1, keyed to fixture connectcampaigns-error-onboarding-missing-instance#task#5). It does not carry this behavioral check's replay key, so it does not cover this check; fixing it should also close this one, and this issue records the behavioral check that must be re-replayed to demonstrate that.

Traces: behavioral check connectcampaigns.instance-onboarding-job-lifecycle (the double-delete assertion after DeleteConnectInstanceConfig succeeded once).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, emulator up (60/62 passed).

[mismatch] connectcampaigns.instance-onboarding-job-lifecycle
  check_connectcampaigns_instance_onboarding_job_lifecycle.py line 82:
    assert double_delete == ("ResourceNotFoundException", _NOT_ONBOARDED), double_delete
  AssertionError: ('ResourceNotFoundException', 'ConnectInstanceConfig not found 4cd1aa0d-5ae8-497f-901e-e5a6892bcee8.')

Expected (recorded AWS): HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message 'Instance is not onboarded' — wholly fixed text, no request or state value interpolated.
Actual (emulator): HTTP 404, Error.Code 'ResourceNotFoundException', Error.Message 'ConnectInstanceConfig not found <connectInstanceId>.' — fixed parts 'ConnectInstanceConfig not found ' + '.', variable part = connectInstanceId taken from the request's httpLabel.

The error code and status match; the divergence is the message text only. Everything earlier in the lifecycle (StartInstanceOnboardingJob, the idempotent repeat call, GetInstanceOnboardingJobStatus reaching SUCCEEDED, GetConnectInstanceConfig, the first DeleteConnectInstanceConfig and the post-delete GetConnectInstanceConfig not-found) passed, so this is an isolated message-template defect on the delete path.

Model evidence (smithy_query, operation=DeleteConnectInstanceConfig): http {method DELETE, uri /connect-instance/{connectInstanceId}/config, code 200}, idempotent true, output shape Unit, errors include ResourceNotFoundException httpError 404 and InvalidStateException httpError 409.

## Steps to Reproduce
1. Clone the repo and boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1, check connectcampaigns.instance-onboarding-job-lifecycle.
3. Equivalently by hand: create a Connect instance and wait for InstanceStatus ACTIVE; call connectcampaigns StartInstanceOnboardingJob(connectInstanceId=<id>, encryptionConfig={enabled:false}) and wait for GetInstanceOnboardingJobStatus to report SUCCEEDED; call DeleteConnectInstanceConfig(connectInstanceId=<id>) once (succeeds).
4. Call DeleteConnectInstanceConfig(connectInstanceId=<id>) a SECOND time. The reply is HTTP 404 ResourceNotFoundException — correct — but Error.Message is 'ConnectInstanceConfig not found <id>.' instead of AWS's fixed 'Instance is not onboarded'.
5. After the fix, the second delete must refuse with ResourceNotFoundException and message exactly 'Instance is not onboarded', while GetConnectInstanceConfig for the same un-onboarded instance keeps its own message 'ConnectInstanceConfig not found <id>.', and the check must replay clean.
