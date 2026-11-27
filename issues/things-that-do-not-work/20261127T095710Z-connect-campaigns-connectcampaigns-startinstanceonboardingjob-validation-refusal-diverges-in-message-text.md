Issue-Source: trace-sweep-divergence
Issue-Trace: start-instance-onboarding-job-validation
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: start-instance-onboarding-job-validation
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connect-campaigns|StartInstanceOnboardingJob|4xx->4xx|$.message
Issue-Key: connect-campaigns|StartInstanceOnboardingJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 279ebdf9ec42ad03fadad8a62f2bc4693848273e

# ConnectCampaigns StartInstanceOnboardingJob validation refusal diverges in message text and error-type header

> _Note: this issue was identified against the code base at commit `279ebdf9ec42ad03fadad8a62f2bc4693848273e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceOnboardingJob

## Description
TWO causes on the ConnectCampaigns v1 service (Smithy: services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/smithy-model — AmazonConnectCampaignService, version 2021-01-30, protocol restJson1, sigv4Name 'connect-campaigns'). Both are on StartInstanceOnboardingJob's 400 ValidationException refusal, and they share one fault: the emulator constructs that ValidationException itself — its own prose for the message and nothing at all for the modeled error-type member — instead of reproducing what the service returns for that failure class. Both must be closed before this issue is resolved.

Cause 1 of 2: the ValidationException message for an encryptionConfig with encryption enabled and no keyArn is emulator-authored prose.
AWS: StartInstanceOnboardingJob is PUT /connect-instance/{connectInstanceId}/onboarding (input connectInstanceId: @required @httpLabel, target InstanceId @length 0..256 @pattern ^[a-zA-Z0-9_\-.]*$; encryptionConfig: @required, target EncryptionConfig with members enabled (@required, boolean, default false), encryptionType (enum EncryptionType, values [KMS]) and keyArn (@length max 500)); modeled errors AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ThrottlingException(429), ValidationException(400 — members message @required and xAmzErrorType bound to httpHeader x-amzn-ErrorType). EncryptionConfig's own documentation states the rule but the model cannot express it ('If enabled, a KMS key id needs to be provided'), and the model states no message text, so the wording is recorded behavior only: the recorded response is HTTP 400 with $.message = 'Missing required parameter KeyArn.'. That text is entirely FIXED apart from the parameter name it names — 'Missing required parameter ' + the offending parameter name ('KeyArn') + '.' — and it interpolates nothing from the request or from state (no instance id, no key value).
Emulator: answers the right status (400) and the right error shape, but with $.message = 'encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled' — a different template entirely, written in the emulator's own voice: fixed text 'encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled' with a lowercase member-path prefix rather than AWS's 'Missing required parameter <Name>.' form.
Why: for an error class, the status, the error code and the message are the contract. A client that matches, logs or surfaces the refusal reason gets prose no AWS deployment ever emits; message assertions in SDK integration tests and operator diagnostics diverge, and the lowercase member-path dialect ('encryptionConfig.keyArn: ...') teaches callers a validation-message format this service does not use.
Fix: the StartInstanceOnboardingJob handler's validation path for connect-campaigns — the helper that refuses an inconsistent encryptionConfig and constructs the modeled ValidationException must emit the service's missing-required-parameter wording, with the parameter name as its only variable part, instead of its own member-path sentence.
Traces: start-instance-onboarding-job-validation#task#3 (cluster also covers #task#4).

Cause 2 of 2: the x-amzn-ErrorType header on the 400 ValidationException does not have the recorded shape.
AWS: ValidationException models xAmzErrorType @httpHeader('x-amzn-ErrorType') alongside the @required message, and the service's CORS config exposes x-amzn-errortype, so the header is part of this error's modeled wire form and the service populates it. The recorded value is not asserted literally by the corpus — it is shape-asserted — and the emulator's value fails that shape check. Hypothesis: AWS returns the coral-qualified error-type form for this service (the shape name followed by a ':' and a service-qualified type URI) rather than the bare shape name, which is why the recording's value is shape-matched instead of compared literally.
Emulator: on this step it gets the status (400) and the message right, but serializes x-amzn-errortype as the bare string 'ValidationException', which the replay reports as shape_mismatch against the recorded shape. Every ValidationException this operation builds sets the modeled member to None, so the value on the wire is whatever the protocol layer defaults to and nothing populates the modeled member.
Why: a client that routes on the error-type header (the restJson1 error-discrimination path, and browser callers that read the exposed header) sees a value in a different form from the one the service emits; anything parsing the qualified form, or asserting on it, does not get what AWS sends.
Fix: the connect-campaigns error serializer / the ValidationException construction in the StartInstanceOnboardingJob handler — the modeled xAmzErrorType member must be populated with the service's error-type value for the error being returned instead of left unset for the framework to default.
Traces: start-instance-onboarding-job-validation#task#5.

Neither cluster is covered by an already-filed issue: both carry empty stub_path and empty owned_by. The open issue 20261124T184219Z-connect-campaigns-connectcampaigns-v1-error-text-and-batch-failure-reasons.md describes the same family of fault (emulator-authored error text on this service) but is keyed to DeleteConnectInstanceConfig / GetCampaignStateBatch on different fixtures and steps and carries neither of these replay keys, so fixing it does not demonstrate either cause here.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 43 steps, 40 passed, 3 mismatched, score 93.0%). Both clusters: stub_path empty, owned_by empty.

Cause 1 — divergence_key service=connect-campaigns|StartInstanceOnboardingJob|status=400->400|keys=$.message, fixture start-instance-onboarding-job-validation, step task#3 (affected also: start-instance-onboarding-job-validation#task#4, affected_count 2):
  $.message: value_mismatch (expected 'Missing required parameter KeyArn.', got 'encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled')
Expected (recorded AWS): HTTP 400 ValidationException, message = 'Missing required parameter ' + <parameter name, here KeyArn> + '.'; fixed text is 'Missing required parameter ' and the trailing '.', the only variable part is the parameter name.
Actual (emulator): HTTP 400 ValidationException, message = 'encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled' — wholly fixed emulator-authored text, no value from the request.
Status and error shape match (400 -> 400, ValidationException); the divergence is the message text.
Source: services/service-com-amazonaws-connectcampaigns-amazonconnectcampaignservice/src/ops/start_instance_onboarding_job.rs lines 138-153 (fn check_encryption_config) hardcode that string, with x_amz_error_type: None.

Cause 2 — divergence_key service=connect-campaigns|StartInstanceOnboardingJob|status=400->400|keys=$headers.x-amzn-errortype, fixture start-instance-onboarding-job-validation, step task#5 (affected_count 1):
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'ValidationException')
Expected: HTTP 400 with x-amzn-ErrorType carrying the recorded error-type value, asserted by shape rather than literally.
Actual (emulator): HTTP 400 with x-amzn-errortype = the bare shape name 'ValidationException', which fails that shape assertion. The message on this step matched, so the message is not at issue here.
Model evidence: ValidationException members = message (@required, String) and xAmzErrorType (@httpHeader 'x-amzn-ErrorType'); service CORS additionalExposedHeaders includes x-amzn-errortype. Source: every ValidationException/ResourceNotFoundException/ConflictException/InternalServerException built in start_instance_onboarding_job.rs passes x_amz_error_type: None (lines 147, 240, 251, 281, 299).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire corpus for trace set simparity-wire-aws-v1.
3. Cause 1: replay fixture start-instance-onboarding-job-validation step task#3 (and task#4) — PUT /connect-instance/<an onboardable instance id>/onboarding (StartInstanceOnboardingJob) with encryptionConfig {enabled: true} and no keyArn. The reply is 400 ValidationException, but $.message reads 'encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled' instead of AWS's 'Missing required parameter KeyArn.'.
4. Cause 2: replay fixture start-instance-onboarding-job-validation step task#5 — the same operation's other validation refusal. The reply is 400 with the correct message, but the x-amzn-ErrorType response header carries the bare shape name 'ValidationException', which does not match the recorded header's asserted shape.
5. After the fix both steps must replay with no $.message and no $headers.x-amzn-errortype divergence for StartInstanceOnboardingJob.
