Issue-Source: trace-sweep-divergence
Issue-Trace: connectcampaigns-onboarding-request-validation
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connectcampaigns-onboarding-request-validation
Issue-Shape-Id: com.amazonaws.connectcampaigns#AmazonConnectCampaignService
Issue-Key: connect-campaigns|StartInstanceOnboardingJob|4xx->4xx|status
Issue-Key: connect-campaigns|StartInstanceOnboardingJob|4xx->4xx|$
Issue-Related: 20261127T095710Z-connect-campaigns-connectcampaigns-startinstanceonboardingjob-validation-refusal-diverges-in-message-text.md
Issue-Commit: 87d71c43287cb88d437f1b22d9a50457d3db170d

# ConnectCampaigns StartInstanceOnboardingJob input refusals diverge: unmodeled instance-id length limit and self-authored error body

> _Note: this issue was identified against the code base at commit `87d71c43287cb88d437f1b22d9a50457d3db170d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceOnboardingJob

## Description
TWO causes on the ConnectCampaigns v1 service (Smithy: services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/smithy-model/com-amazonaws-connectcampaigns-amazonconnectcampaignservice.json — AmazonConnectCampaignService, restJson1, sigv4Name 'connect-campaigns'). Both are on how StartInstanceOnboardingJob refuses a malformed request, and they share one fault: the emulator decides the refusal itself — which error class to raise, and what body to put on it — instead of reproducing the refusal the service produces for that input class. BOTH must be closed before this issue is resolved.

Cause 1 of 2: a connectInstanceId longer than 36 codepoints is answered as a missing instance (404) instead of a bad request (400).
AWS: StartInstanceOnboardingJob is PUT /connect-instance/{connectInstanceId}/onboarding; connectInstanceId is @required @httpLabel targeting InstanceId (@length 0..256, @pattern ^[a-zA-Z0-9_\-.]*$), and the operation models ValidationException (400) and ResourceNotFoundException (404) (Smithy: StartInstanceOnboardingJob input + shapes InstanceId, ValidationException, ResourceNotFoundException). The 36-codepoint ceiling is NOT in the model, so for it this is recorded behavior only: the recording shows the service refusing an over-long id with HTTP 400 and $.message = 'Invalid parameter.DataValidationException:ql.itemdb.schema.DataValidationException: attribute INSTANCE_ID: length (codepoints) out of range: does not conform to 0..36' — i.e. the service's persistence layer rejects the attribute on length BEFORE any instance lookup happens, so an over-long id is a request fault, never an absent resource. In that message the whole 'Invalid parameter.DataValidationException:ql.itemdb.schema.DataValidationException: attribute ' ... ': length (codepoints) out of range: does not conform to ' ... text is FIXED; its variable parts are the offending attribute's name (INSTANCE_ID) and that attribute's permitted range (0..36). No value from the request and nothing from state is interpolated — in particular the submitted id is NOT echoed.
Emulator: the id passes the modeled 0..256 length check, so the handler proceeds straight to the Connect instance registry, finds no instance and answers HTTP 404 ResourceNotFoundException with $.message = 'ConnectInstanceId not found <the submitted id>.' and x-amzn-errortype = 'ResourceNotFoundException' (which also fails the recorded header's shape assertion). The status, the error code and the message are all wrong for this input class.
Why: a client cannot tell a malformed request from a nonexistent instance. 400 is a client-fault, non-retryable, fix-the-input signal; 404 ResourceNotFoundException makes callers (and SDK/CLI error handling, provisioning retries, 'create the instance first' recovery paths) conclude the instance is absent and go look for or create one, when the request itself can never be accepted. It also leaks the full oversized id back in the message, which AWS does not do.
Fix: the StartInstanceOnboardingJob handler's validation path for connect-campaigns — the instance-id check must enforce the service's effective attribute limit and raise the modeled ValidationException with the service's data-validation wording, before the registry existence lookup runs.
Traces: connectcampaigns-onboarding-request-validation#task#2.

Cause 2 of 2: a request whose required encryptionConfig is null gets an emulator-authored validation body where the service sends none.
AWS: encryptionConfig is @required on StartInstanceOnboardingJobRequest (target EncryptionConfig). The model states no message for the frontend's rejection of a null required member, so this is recorded behavior only: the recorded response is HTTP 400 with an EMPTY JSON body ({} — no __type, no message member) and an x-amzn-ErrorType header whose value the corpus asserts by shape rather than literally. Hypothesis: the request is rejected by the protocol/serialization layer ahead of the operation, which answers with the error-type header alone and no modeled error payload.
Emulator: answers the right status (400) but with a body it writes itself — {"__type": "ValidationException", "message": "1 validation error detected: Value null at 'encryptionConfig' failed to satisfy constraint: Member must not be null"} — and with x-amzn-errortype serialized as the bare shape name 'ValidationException', which fails the recorded header's shape assertion.
Why: for an error class the status, the error code and the message are the contract. A client that discriminates restJson1 errors on the __type member, or that surfaces/asserts on $.message, sees a modeled ValidationException with a constraint sentence no AWS deployment emits for this input, instead of the bodyless refusal the service returns and the error-type form it carries. Error-handling and message assertions written against AWS take the wrong branch here.
Fix: the connect-campaigns request-deserialization/validation layer and its error serializer for this operation — a null required top-level member must produce the service's own bodyless 400 with the service's error-type value, rather than being turned into a locally-worded modeled ValidationException with the bare shape name in the header.
Traces: connectcampaigns-onboarding-request-validation#task#8.

Neither cluster is covered by an already-filed issue: both carry empty stub_path and empty owned_by. The open issue 20261127T095710Z-connect-campaigns-connectcampaigns-startinstanceonboardingjob-validation-refusal-diverges-in-message-text.md is on the same operation but is keyed to fixture start-instance-onboarding-job-validation steps task#3/task#5 (the keyArn-missing message and the error-type header there) and carries neither of these replay keys; the open issue 20261124T184219Z-connect-campaigns-connectcampaigns-v1-error-text-and-batch-failure-reasons.md describes the same family (emulator-authored error text on this service) but is keyed to DeleteConnectInstanceConfig / GetCampaignStateBatch. Fixing either does not demonstrate either cause here.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 6 fixtures, 28 steps, 24 passed, 4 mismatched, score 85.7%). Both clusters below: stub_path empty, owned_by empty, affected_count 1.

Cause 1 — divergence_key service=connect-campaigns|StartInstanceOnboardingJob|status=400->404|keys=$.message,$headers.x-amzn-errortype,status; fixture connectcampaigns-onboarding-request-validation, step task#2:
  status: expected 400, got 404
  $.message: value_mismatch (expected 'Invalid parameter.DataValidationException:ql.itemdb.schema.DataValidationException: attribute INSTANCE_ID: length (codepoints) out of range: does not conform to 0..36', got 'ConnectInstanceId not found <submitted oversized id>.')
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ResourceNotFoundException')
Expected (recorded AWS): HTTP 400, data-validation refusal; fixed text 'Invalid parameter.DataValidationException:ql.itemdb.schema.DataValidationException: attribute <ATTRIBUTE>: length (codepoints) out of range: does not conform to <min>..<max>' with the attribute name and its range as the only variable parts.
Actual (emulator): HTTP 404 ResourceNotFoundException, message 'ConnectInstanceId not found <id>.' built from the request's id.
Model evidence: connectInstanceId -> InstanceId @length 0..256 (so the modeled check passes an id of ~250 chars); ValidationException httpError 400, ResourceNotFoundException httpError 404.
Source: services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/ops/start_instance_onboarding_job.rs lines 213-255 — the only pre-lookup check is check_encryption_config (lines 138-153); the id goes straight to crate::state::connect_instance_refusal and the NoSuchInstance arm (lines 231-244) builds the 404 via crate::state::connect_instance_not_found_message with x_amz_error_type: None.

Cause 2 — divergence_key service=connect-campaigns|StartInstanceOnboardingJob|status=400->400|keys=$,$headers.x-amzn-errortype; fixture connectcampaigns-onboarding-request-validation, step task#8:
  $: value_mismatch (expected {}, got {'__type': 'ValidationException', 'message': "1 validation error detected: Value null at 'encryptionConfig' failed to satisfy constraint: Member must not be null"})
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
Expected (recorded AWS): HTTP 400 with an empty JSON body and an error-type header asserted by shape.
Actual (emulator): HTTP 400 carrying a full modeled ValidationException payload plus the bare shape name in x-amzn-errortype. Status matches; the body and the header form do not.
Model evidence: encryptionConfig @required on StartInstanceOnboardingJobRequest; ValidationException members message (@required) and xAmzErrorType (@httpHeader 'x-amzn-ErrorType').

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus for trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture connectcampaigns-onboarding-request-validation step task#2 — PUT /connect-instance/<an id longer than 36 codepoints but within 256>/onboarding (StartInstanceOnboardingJob) with a valid encryptionConfig. The emulator answers HTTP 404 ResourceNotFoundException ('ConnectInstanceId not found <id>.'); AWS answers HTTP 400 with the attribute-length data-validation message for INSTANCE_ID and does not echo the id.
4. Cause 2: replay fixture connectcampaigns-onboarding-request-validation step task#8 — the same operation with the required encryptionConfig member null/absent. The emulator answers HTTP 400 with {"__type":"ValidationException","message":"1 validation error detected: Value null at 'encryptionConfig' failed to satisfy constraint: Member must not be null"} and x-amzn-errortype 'ValidationException'; AWS answers HTTP 400 with an empty JSON body and its own error-type header value.
5. After the fix, both steps must replay with no status, no $ / $.message and no $headers.x-amzn-errortype divergence for StartInstanceOnboardingJob.
