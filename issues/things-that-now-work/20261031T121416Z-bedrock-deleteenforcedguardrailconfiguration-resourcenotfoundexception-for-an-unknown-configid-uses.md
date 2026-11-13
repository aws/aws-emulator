Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock.enforced-guardrail-configuration-put-list-delete
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock.enforced-guardrail-configuration-put-list-delete|run#21|AssertionError|assert message == "No default guardrail configuration found for the specified config identifier.", (
Issue-Commit: 32f27f5adb5ef4f1260f24418d9e3155f5d658ad

# bedrock DeleteEnforcedGuardrailConfiguration: ResourceNotFoundException for an unknown configId uses an emulator-authored message instead of AWS's fixed text

> _Note: this issue was identified against the code base at commit `32f27f5adb5ef4f1260f24418d9e3155f5d658ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteEnforcedGuardrailConfiguration

## Description
Cause 1 of 1: DeleteEnforcedGuardrailConfiguration refuses an unknown/already-deleted configId with the right error code but the wrong message text.

AWS: Smithy (com.amazonaws.bedrock, services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json) DeleteEnforcedGuardrailConfiguration = DELETE /enforcedGuardrailsConfiguration/{configId}, success code 200, idempotent, input member configId (httpLabel, required, target AccountEnforcedGuardrailConfigurationId, @pattern ^[a-z0-9]+$), modelled errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Message: recorded behavior only — deleting a configId that the account's enforced-guardrail configuration does not (or no longer) contains is refused with ResourceNotFoundException whose message is the wholly fixed text "No default guardrail configuration found for the specified config identifier." Every word of that message is fixed text: the message has NO variable parts, and in particular it does NOT echo the submitted configId, the account or the region.

Emulator: the refusal happens and the error code is correct (ResourceNotFoundException), but the message is the emulator's own wording, "no enforced guardrail configuration <configId> exists in this account" — lower-cased, differently phrased, and interpolating the requested configId (the recorded run shows the id value filling that slot; the id itself is a per-run value, the interpolation is the defect). The preceding put/list/delete steps of the check all pass, so this is purely the message produced on the not-found path.

Why: the error message is part of the operation's contract. A client (or a test, a log scraper, a support runbook, an SDK-level matcher) that keys on AWS's fixed sentence does not match the emulator's text, so the "configuration already gone" branch is mis-handled or reported as an unknown failure even though the code and status are right. The emulator's message additionally leaks a shape AWS never emits (a request-echoing not-found message), which trains callers on text production will never send.

Fix: the DeleteEnforcedGuardrailConfiguration handler's not-found path (its error construction / serializer for the modelled ResourceNotFoundException) must emit AWS's fixed sentence verbatim, with no interpolation of the configId or account; the HTTP status (404) and error code (ResourceNotFoundException) are already correct and must not change.

Traces: bedrock.enforced-guardrail-configuration-put-list-delete (the assertion at the end of the check, after the successful put/list/delete sequence).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check [mismatch] bedrock.enforced-guardrail-configuration-put-list-delete.

The check creates a guardrail, publishes a version, calls PutEnforcedGuardrailConfiguration, verifies the entry is listed with the right guardrailId/guardrailVersion, calls DeleteEnforcedGuardrailConfiguration(configId=<minted id>) and verifies the list returns to its pre-check entries — all of that PASSES. It then repeats the delete with the same configId:

  code, message = _error(lambda: bedrock.delete_enforced_guardrail_configuration(configId=config_id))
  assert code == "ResourceNotFoundException"                     # PASSES
  assert message == "No default guardrail configuration found for the specified config identifier."

Failure reported:
  File "check_bedrock_enforced_guardrail_configuration_put_list_delete.py", line 124, in run
  AssertionError: unexpected ResourceNotFoundException message 'no enforced guardrail configuration 0000017f1t1k exists in this account'

Expected error message (fixed text, no variable parts):
  "No default guardrail configuration found for the specified config identifier."
Actual error message (emulator-authored, interpolates the request's configId — '0000017f1t1k' is this run's generated id, not part of any template):
  "no enforced guardrail configuration 0000017f1t1k exists in this account"

Error code and HTTP status are correct per the model (ResourceNotFoundException, httpError 404); only the message diverges.

Not covered by the open issue 20261029T172312Z-bedrock-control-plane-is-over-permissive-enforced-guardrail.md, which concerns DeleteGuardrail not consulting the enforced configuration and six other over-permissive/over-strict pre-conditions — none of them is DeleteEnforcedGuardrailConfiguration's not-found message.

## Steps to Reproduce
Boot the emulator from this checkout and replay bedrock.enforced-guardrail-configuration-put-list-delete (suite simparity-behavioral-aws-v1), or by hand against the bedrock control plane:
1. CreateGuardrail(name=<unique>, blockedInputMessaging=..., blockedOutputsMessaging=..., wordPolicyConfig={wordsConfig:[{text:'sim2realforbiddenterm'}]}) and wait for the guardrail to be READY.
2. CreateGuardrailVersion(guardrailIdentifier=<id>) and wait for that version to be READY.
3. PutEnforcedGuardrailConfiguration(guardrailInferenceConfig={guardrailIdentifier:<id>, guardrailVersion:<version>}) -> note configId.
4. ListEnforcedGuardrailsConfiguration — the entry is present (passes today).
5. DeleteEnforcedGuardrailConfiguration(configId=<configId>) — succeeds and the list returns to its prior entries (passes today).
6. DeleteEnforcedGuardrailConfiguration(configId=<the same configId>) again. Expect ResourceNotFoundException (404) with message exactly "No default guardrail configuration found for the specified config identifier."; observe ResourceNotFoundException with message "no enforced guardrail configuration <configId> exists in this account".
