Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectcampaignsv2.start-onboarding-custom-encryption-without-key-rejected
Issue-Shape-Id: com.amazonaws.connectcampaignsv2#AmazonConnectCampaignServiceV2
Issue-Key: connectcampaignsv2.start-onboarding-custom-encryption-without-key-rejected|run#10|AssertionError|assert err["Message"] == "Missing required parameter KeyArn.", (
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# ConnectCampaignsV2 StartInstanceOnboardingJob: ValidationException for encryption enabled without keyArn carries emulator-authored message instead of AWS's 'Missing required parameter <Name>.'

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInstanceOnboardingJob

## Description
ONE net-new root cause on the ConnectCampaignsV2 service (Smithy model services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/smithy-model/com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2.json, AmazonConnectCampaignServiceV2, protocol restJson1).

Cause 1 of 1: the 400 ValidationException that refuses an encryptionConfig with encryption enabled and no key ARN is worded by the emulator rather than by the service.
Failing check ids: connectcampaignsv2.start-onboarding-custom-encryption-without-key-rejected

AWS: Smithy: StartInstanceOnboardingJob is PUT /v2/connect-instance/{connectInstanceId}/onboarding; input connectInstanceId (InstanceId, @required @httpLabel, @length 1..256, @pattern ^[-_.a-zA-Z0-9]+$) and encryptionConfig (@required, EncryptionConfig). EncryptionConfig members: enabled (@required boolean, @default false), encryptionType (enum EncryptionType), keyArn (EncryptionKey, @length max 500). EncryptionConfig's documentation states the rule the shape cannot express: "If disabled, service will perform encryption with its own key. If enabled, a KMS key id needs to be provided and KMS charges will apply. KMS is only type supported". Modeled errors include ValidationException (httpError 400, members message @required and xAmzErrorType @httpHeader x-amzn-ErrorType). The model states no message text, so the wording is recorded behavior only: the recorded refusal is HTTP 400, Error Code ValidationException, Error Message "Missing required parameter KeyArn.". In that message the FIXED text is "Missing required parameter " plus the trailing "."; the only VARIABLE part is the name of the missing parameter (here "KeyArn", taken from the request's offending member). Nothing else is interpolated — no instance id, no key value, no member path.

Emulator: returns the right status and error code (HTTP 400 ValidationException) but with Error Message "encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled" — a wholly different, wholly fixed template in the emulator's own voice: a lowercase dotted member-path prefix ("encryptionConfig.keyArn: ") followed by its own sentence, interpolating nothing from the request.

Why: for a refusal class the status, the error code and the message are the contract. A client that matches, logs, or surfaces the validation reason — SDK integration tests asserting on Message, operator diagnostics, support tooling that parses the missing-parameter name out of the sentence — receives prose no AWS deployment emits, and is taught a validation-message dialect ("<memberPath>: <sentence>") this service does not use. The check also asserts the refused call leaves no onboarding record and no instance config and that a subsequent service-owned-encryption onboarding reaches SUCCEEDED; those assertions were never reached because the message assertion fails first, so they are unverified rather than known-good.

Fix: the StartInstanceOnboardingJob handler's encryptionConfig validation path in the ConnectCampaignsV2 crate — the helper that refuses an inconsistent encryptionConfig and constructs the modeled ValidationException must emit the service's missing-required-parameter wording with the parameter name as its only variable part, instead of its own member-path sentence. The same helper's enum-value refusal path builds its message the same way and should be reviewed for the same dialect.

Related but NOT covering this: open issue 20261127T095710Z-connect-campaigns-connectcampaigns-startinstanceonboardingjob-validation-refusal-diverges-in-message-text.md describes the identical fault on the ConnectCampaigns **v1** service (service token connect-campaigns, a different crate and a different Smithy model, uri /connect-instance/{id}/onboarding) and carries only that service's wire replay keys; it does not carry this behavioral check and fixing it does not demonstrate this cause. Open issues 20261124T184219Z (v1 error text), 20261126T214053Z (PutInstanceCommunicationLimits gating), 20261126T225621Z (CreateCampaign union constraint docs) and 20261127T064935Z (DeleteConnectInstanceConfig not-found text) address different operations and do not cover this refusal.

Traces: behavioral check connectcampaignsv2.start-onboarding-custom-encryption-without-key-rejected. Emulator source: services/service-com-amazonaws-connectcampaignsv2-amazonconnectcampaignservicev2/src/ops/start_instance_onboarding_job.rs lines 139-174 (fn check_encryption_config), where line 166-171 hardcodes the diverging string and line 147 leaves x_amz_error_type unset.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 25/26 passed, 1 mismatched, 0 errored, 0 skipped, bootstrap_signal false. Service connectcampaignsv2 25/26 (96%).

[mismatch] connectcampaignsv2.start-onboarding-custom-encryption-without-key-rejected
  File "check_connectcampaignsv2_start_onboarding_custom_encryption_without_key_rejected.py", line 114, in run
    assert err["Message"] == "Missing required parameter KeyArn.", (
  AssertionError: unexpected refusal message 'encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled'

Expected (recorded AWS): HTTP 400, Error Code ValidationException, Error Message = "Missing required parameter " + <name of the missing parameter, here KeyArn> + "." — fixed text with the parameter name as its only variable part.
Actual (emulator): HTTP 400, Error Code ValidationException (both correct), Error Message = "encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled" — entirely different fixed text, no value drawn from the request or from state.

The check's Code assertion (err["Code"] == "ValidationException") passed; the divergence is the message text alone. The assertions that follow in the same check — ResourceNotFoundException with message starting "Instance onboarding record does not exist" from GetInstanceOnboardingJobStatus, ResourceNotFoundException with message starting "ConnectInstanceConfig not found" from GetConnectInstanceConfig, and a subsequent encryptionConfig {enabled:false} onboarding settling SUCCEEDED with a persisted config — are not reached and are therefore unverified.

Model evidence (smithy_query on this service's model): EncryptionConfig = {enabled @required/@default false, encryptionType, keyArn @length max 500} with the quoted doc; ValidationException = {message @required, xAmzErrorType @httpHeader 'x-amzn-ErrorType'}, httpError 400.
Source: services/.../connectcampaignsv2.../src/ops/start_instance_onboarding_job.rs:166-171.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 (bound to run_behavioral_suite).
3. Replay check connectcampaignsv2.start-onboarding-custom-encryption-without-key-rejected: create a CONNECT_MANAGED Amazon Connect instance, wait until DescribeInstance reports InstanceStatus ACTIVE, then call connectcampaignsv2 StartInstanceOnboardingJob with connectInstanceId=<that instance id> and encryptionConfig={"enabled": true} (no keyArn).
4. Observe the refusal: HTTP 400 with Error Code ValidationException (correct) but Error Message 'encryptionConfig.keyArn: a KMS key arn is required when encryption is enabled' instead of AWS's 'Missing required parameter KeyArn.'.
5. After the fix, the message must be the missing-required-parameter sentence with the offending parameter name as its only variable part, and the whole check must then proceed to confirm GetInstanceOnboardingJobStatus/GetConnectInstanceConfig still report ResourceNotFoundException for the untouched instance and that onboarding with encryptionConfig {enabled:false} settles SUCCEEDED.
