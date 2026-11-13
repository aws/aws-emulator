Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-enforced-guardrail-configuration
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-enforced-guardrail-configuration bedrock-foundation-model-agreement-and-access
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|DeleteEnforcedGuardrailConfiguration|4xx->4xx|$.message
Issue-Key: bedrock|ListFoundationModelAgreementOffers|4xx->2xx|status
Issue-Key: bedrock|ListFoundationModelAgreementOffers|4xx->2xx|$.message
Issue-Key: bedrock|ListFoundationModelAgreementOffers|4xx->2xx|$headers.x-amzn-errortype
Issue-Commit: 194ed8012355835d7728b0a2042bc47874feddc4

# bedrock error paths diverge: DeleteEnforcedGuardrailConfiguration not-found message reworded, ListFoundationModelAgreementOffers skips modelId validation

> _Note: this issue was identified against the code base at commit `194ed8012355835d7728b0a2042bc47874feddc4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteEnforcedGuardrailConfiguration

## Description
This bundle carries THREE net-new causes on the bedrock control plane, all on refusal paths: the emulator either rewords a modeled error's fixed message or does not produce the refusal at all. A fixer MUST close all three before submitting.

--- Cause 1 of 3: DeleteEnforcedGuardrailConfiguration's ResourceNotFoundException message is emulator-authored ---
Fixture: bedrock-enforced-guardrail-configuration, step task#7 (operation DeleteEnforcedGuardrailConfiguration).

AWS: Smithy (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json) models DeleteEnforcedGuardrailConfiguration as DELETE /enforcedGuardrailsConfiguration/{configId}, success code 200, @idempotent, input member configId (httpLabel, required, target AccountEnforcedGuardrailConfigurationId, @pattern ^[a-z0-9]+$), modeled errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). The message text itself is recorded behavior only: deleting a configId the account's enforced-guardrail configuration does not (or no longer) contain is refused 404 ResourceNotFoundException with the wholly fixed sentence "No default guardrail configuration found for the specified config identifier." Every word of that message is FIXED TEXT — it has no variable parts, and in particular it does NOT echo the submitted configId, the account id or the region.

Emulator: status (404) and error code (ResourceNotFoundException) are correct, but the message is the emulator's own prose that INTERPOLATES the requested configId: "no enforced guardrail configuration <configId> exists in this account" (the replayed run shows the generated id filling that slot; the id value is a per-run value, the interpolation is the defect). Replay detail: "$.message: value_mismatch".

Why: the error message is part of the operation's contract. A client, test, log scraper or SDK-level matcher that keys on AWS's fixed sentence does not match the emulator's text, so the "configuration already gone" branch is mis-handled or reported as an unknown failure even though the code and status are right. The emulator also teaches callers a request-echoing not-found shape AWS never emits.

Fix: the DeleteEnforcedGuardrailConfiguration handler's not-found path — the construction/serialization of its modeled ResourceNotFoundException — must emit AWS's fixed sentence verbatim with no interpolation of configId or account. Status and error code already match and must not change.

Note: the open BEHAVIORAL issue 20261031T121416Z-bedrock-deleteenforcedguardrailconfiguration-resourcenotfoundexception-for-an-unknown-configid-uses.md describes the same defect from the behavioral suite, but it carries no wire replay key (no Issue-Trace/Issue-Step), so this wire step is unowned and is filed here with its replay binding.

--- Cause 2 of 3: ListFoundationModelAgreementOffers answers 200 where AWS refuses 400 (setup path) ---
Fixture: bedrock-foundation-model-agreement-and-access, step setup#1 (operation ListFoundationModelAgreementOffers).

AWS: Smithy models ListFoundationModelAgreementOffers as GET /list-foundation-model-agreement-offers/{modelId}, @readonly, success code 200, input member modelId (httpLabel, required, target BedrockModelId, @length max 140, @pattern ^[a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([a-z0-9-]{1,63}[.]){0,2}[a-z0-9-]{1,63}([:][a-z0-9-]{1,63}){0,2}(/[a-z0-9]{12}|)$), output members modelId + offers (both required), modeled errors AccessDeniedException (403), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). Recorded behavior: a modelId that does not resolve to a model offering an agreement is refused with HTTP 400 (ValidationException class) rather than being answered with an offers list.

Emulator: answers HTTP 200 with a success body. Replay detail: "status: expected 400, got 200".

Why: the request never fails, so a client that drives marketplace/agreement enrollment off this call believes an offer exists for a model that has none and proceeds to the next step (create agreement / put use case) on fabricated data instead of surfacing the validation error.

Fix: ListFoundationModelAgreementOffers' handler must gate on modelId resolution/validation BEFORE producing the offers list, and refuse with the modeled 400 ValidationException instead of returning 200.

--- Cause 3 of 3: ListFoundationModelAgreementOffers omits the 400 ValidationException "The provided model identifier is invalid." ---
Fixture: bedrock-foundation-model-agreement-and-access, step task#2 (operation ListFoundationModelAgreementOffers).

AWS: same operation and same modeled errors as Cause 2. For a modelId that is not a valid/resolvable foundation-model identifier, AWS answers HTTP 400 with the x-amzn-errortype error header set (ValidationException class) and the body message "The provided model identifier is invalid." — that sentence is entirely FIXED TEXT, with no variable parts: it does not echo the submitted modelId, account or region.

Emulator: answers HTTP 200 with no $.message and no x-amzn-errortype header at all. Replay detail: "status: expected 400, got 200; $.message: missing (expected 'The provided model identifier is invalid.', got None); $headers.x-amzn-errortype: missing".

Why: the invalid-identifier branch is unreachable in the emulator, so the error class, the error header and the fixed message a caller matches on are all absent. Anything that distinguishes "model has no offers" from "model identifier is invalid" cannot do so, and error-handling code paths in SDK consumers are never exercised.

Fix: the same handler/validation layer as Cause 2 — validate modelId against the modeled BedrockModelId pattern and the served model catalogue, and emit the modeled ValidationException (HTTP 400, x-amzn-errortype: ValidationException) with the fixed message above.

Traces: bedrock-enforced-guardrail-configuration#task#7 (Cause 1), bedrock-foundation-model-agreement-and-access#setup#1 (Cause 2), bedrock-foundation-model-agreement-and-access#task#2 (Cause 3).

## Evidence
Wire corpus simparity-wire-aws-v1, replayed against the emulator booted from this checkout (emulator_up=true, bootstrap_signal=false, score 59.4%).

Cause 1 — bedrock-enforced-guardrail-configuration#task#7, DeleteEnforcedGuardrailConfiguration:
  expected_status 404, actual_status 404
  detail: $.message: value_mismatch
    expected: 'No default guardrail configuration found for the specified config identifier.'   (fixed text, no variable parts)
    actual:   'no enforced guardrail configuration 00000152rh36 exists in this account'        (emulator prose; '00000152rh36' is this run's generated configId filling an interpolation slot AWS's message does not have)
  Error code and status are correct; only the message diverges.

Cause 2 — bedrock-foundation-model-agreement-and-access#setup#1, ListFoundationModelAgreementOffers:
  expected_status 400, actual_status 200
  detail: status: expected 400, got 200

Cause 3 — bedrock-foundation-model-agreement-and-access#task#2, ListFoundationModelAgreementOffers:
  expected_status 400, actual_status 200
  detail: status: expected 400, got 200
          $.message: missing (expected 'The provided model identifier is invalid.', got None)
          $headers.x-amzn-errortype: missing (expected a ValidationException error header, got None)

Model citations (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json):
  DeleteEnforcedGuardrailConfiguration: DELETE /enforcedGuardrailsConfiguration/{configId}, 200, idempotent; configId httpLabel required @pattern ^[a-z0-9]+$; errors AccessDenied(403)/InternalServer(500)/ResourceNotFound(404)/Throttling(429)/Validation(400).
  ListFoundationModelAgreementOffers: GET /list-foundation-model-agreement-offers/{modelId}, 200, readonly; modelId httpLabel required @length max 140 @pattern ^[a-z0-9-]{1,63}[.]{1}[a-z0-9-]{1,63}([a-z0-9-]{1,63}[.]){0,2}[a-z0-9-]{1,63}([:][a-z0-9-]{1,63}){0,2}(/[a-z0-9]{12}|)$; offerType httpQuery enum; output modelId+offers required; errors AccessDenied(403)/InternalServer(500)/ResourceNotFound(404)/Throttling(429)/Validation(400).

No other open issue carries the replay key for any of these three steps (the replay reported owned_by=[] for all three); the similar open behavioral issue is named in the description for Cause 1, and the ListFoundationModelAgreementOffers 501 issue 20261024T175106Z is already resolved, so these are net-new.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1, or reproduce by hand against the bedrock control plane endpoint:

Cause 1 (fixture bedrock-enforced-guardrail-configuration, step task#7):
1. CreateGuardrail(name=<unique>, blockedInputMessaging=..., blockedOutputsMessaging=..., wordPolicyConfig=...) and wait for READY.
2. CreateGuardrailVersion(guardrailIdentifier=<id>) and wait for that version to be READY.
3. PutEnforcedGuardrailConfiguration(guardrailInferenceConfig={guardrailIdentifier:<id>, guardrailVersion:<version>}) -> note configId.
4. DeleteEnforcedGuardrailConfiguration(configId=<configId>) — succeeds.
5. DeleteEnforcedGuardrailConfiguration(configId=<the same configId>) again.
   Expect: HTTP 404, ResourceNotFoundException, message exactly "No default guardrail configuration found for the specified config identifier."
   Observe: HTTP 404, ResourceNotFoundException, message "no enforced guardrail configuration <configId> exists in this account".

Causes 2 and 3 (fixture bedrock-foundation-model-agreement-and-access, steps setup#1 and task#2):
1. GET /list-foundation-model-agreement-offers/<modelId> (ListFoundationModelAgreementOffers) for a modelId that does not resolve to a foundation model offering an agreement in this account/region.
   Expect: HTTP 400 with the x-amzn-errortype error header set to the ValidationException class and body message "The provided model identifier is invalid."
   Observe: HTTP 200 with a success body, no $.message and no x-amzn-errortype header.
2. Repeat with the offerType query parameter set — the same 200 is returned.
