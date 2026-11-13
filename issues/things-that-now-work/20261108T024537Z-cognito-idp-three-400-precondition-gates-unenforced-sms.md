Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-acr-traj-enable-phone-recovery-update-mistakes
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-acr-traj-enable-phone-recovery-update-mistakes cognito-idp-acr-traj-recovery-setting-update-mistakes cognito-idp-acr-traj-sms-mfa-config-mistakes cognito-idp-acr-traj-forgot-password-falls-to-email
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|UpdateUserPool|4xx->2xx|$
Issue-Key: cognito-idp|UpdateUserPool|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|UpdateUserPool|4xx->2xx|status
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|$.__type
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|$.message
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|SetUserPoolMfaConfig|4xx->2xx|status
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$.__type
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$.message
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|status
Issue-Commit: 5772abe834f24ca0a7d173ddfaf967982b48def5

# cognito-idp: three 400 precondition gates unenforced — SMS role trust and password-reset destination

> _Note: this issue was identified against the code base at commit `5772abe834f24ca0a7d173ddfaf967982b48def5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateUserPool

## Description
Three distinct cognito-idp precondition gates are not implemented: a write that AWS refuses with HTTP 400 and a modeled exception is instead accepted with HTTP 200 and an empty/success body. All three share one shape of defect — the handler performs no precondition check before mutating state or claiming success — but each is its own cause and each must be closed separately.

Cause 1 of 3: UpdateUserPool accepts an SmsConfiguration whose IAM role does not trust Cognito.
  AWS: Smithy: UpdateUserPool input member SmsConfiguration (SmsConfigurationType, carrying SnsCallerArn/ExternalId); modeled error InvalidSmsRoleTrustRelationshipException, errorKind=client, @httpError 400. Its model documentation states it "is thrown when the trust relationship is not valid for the role provided for SMS configuration. This can happen if you don't trust cognito-idp.amazonaws.com or the external ID provided in the role does not match what is provided in the SMS configuration for the user pool." So when UpdateUserPool (here, enabling phone-based account recovery / SMS delivery) supplies an SnsCallerArn whose role trust policy does not allow cognito-idp.amazonaws.com to assume it, or whose ExternalId does not match, AWS refuses the whole update: HTTP 400, __type InvalidSmsRoleTrustRelationshipException, header x-amzn-errortype naming the same type, and message "Role does not have a trust relationship allowing Cognito to assume the role". That message is entirely fixed text for this error class — it interpolates no request or state value — so it must be reproduced verbatim.
  Emulator: answers HTTP 200 with an empty body ({}), applies the update, and emits no __type, no message and no x-amzn-errortype header.
  Why: a client that configures SMS MFA/recovery with a mis-trusted SNS caller role is told the pool is correctly configured. The failure then surfaces much later as undeliverable SMS at sign-in time rather than at configuration time, and error-handling code that branches on InvalidSmsRoleTrustRelationshipException never runs. Infrastructure tests that assert the bad role is rejected pass against the emulator and fail against AWS.
  Fix: UpdateUserPool's handler validation — resolve SnsCallerArn against IAM state and verify the role's trust policy admits cognito-idp.amazonaws.com (and that ExternalId matches) before any pool mutation, raising InvalidSmsRoleTrustRelationshipException (400) otherwise.
  Traces: cognito-idp-acr-traj-enable-phone-recovery-update-mistakes#task#5 (cluster also covers cognito-idp-acr-traj-recovery-setting-update-mistakes#task#5).

Cause 2 of 3: SetUserPoolMfaConfig accepts an SmsMfaConfiguration whose IAM role does not trust Cognito.
  AWS: Smithy: SetUserPoolMfaConfig input member SmsMfaConfiguration (SmsMfaConfigType, whose SmsConfiguration member carries the SNS caller role); modeled error InvalidSmsRoleTrustRelationshipException, errorKind=client, @httpError 400. Same rule as cause 1 applied on the MFA-config path: a role that does not grant cognito-idp.amazonaws.com sts:AssumeRole (or an ExternalId mismatch) makes the call fail with HTTP 400, __type InvalidSmsRoleTrustRelationshipException, x-amzn-errortype header, and the fixed message "Role does not have a trust relationship allowing Cognito to assume the role".
  Emulator: answers HTTP 200 and returns the MFA configuration as if it had been accepted; __type, message and x-amzn-errortype are all absent.
  Why: SMS MFA is silently "enabled" on top of a role Cognito cannot assume, so the pool reports an MFA posture it cannot honour and callers never see the configuration error AWS raises.
  Fix: SetUserPoolMfaConfig's handler validation — the same SNS-caller-role trust check as UpdateUserPool, run before the MFA configuration is stored.
  Traces: cognito-idp-acr-traj-sms-mfa-config-mistakes#task#6.

Cause 3 of 3: ForgotPassword succeeds for a user with no verified email or phone number.
  AWS: Smithy: ForgotPassword, output CodeDeliveryDetailsType, modeled error InvalidParameterException (errorKind=client, @httpError 400). A password-reset code can only be delivered over a registered and verified contact attribute, so when the resolved user has neither a verified email nor a verified phone_number (and the pool's AccountRecoverySettingType offers no usable mechanism), AWS refuses rather than inventing a destination: HTTP 400, __type InvalidParameterException, x-amzn-errortype header, message "Cannot reset password for the user as there is no registered/verified email or phone_number". The whole message is fixed text for this error class; it carries no request- or state-derived values.
  Emulator: answers HTTP 200 with a CodeDeliveryDetails body, i.e. it reports a delivery for a user that has no verified destination, and emits no error type, message or x-amzn-errortype header.
  Why: the recovery flow appears to start, so a client waits for a code that can never arrive instead of being told at once that the account has no verified contact. Negative tests for unrecoverable accounts cannot be written against the emulator.
  Fix: ForgotPassword's handler validation — after username resolution, select the delivery destination from the user's verified contact attributes honouring the pool's AccountRecoverySetting, and raise InvalidParameterException (400) with that exact message when no verified email or phone_number exists.
  Traces: cognito-idp-acr-traj-forgot-password-falls-to-email#task#10.

Not covered by existing open issues: the already-open "cognito-idp: ten request-validation and precondition gates accept calls AWS refuses" (20261029T184956Z) and "cognito-idp: nine validation/enforcement gates missing" (20261103T075017Z) carry their own replay keys and do not own these steps (the replay reports them unowned); "cognito-idp: contact-attribute handling diverges — ForgotPassword picks the wrong code-delivery destination" (20261103T190406Z) concerns destination selection when a verified contact DOES exist, not the no-verified-contact refusal in cause 3; "the user-import plane diverges" (20261030T174613Z) covers only StartUserImportJob's CloudWatch-Logs role trust check, not the SMS/SNS caller-role trust check in causes 1-2.

## Evidence
Cause 1 — fixture cognito-idp-acr-traj-enable-phone-recovery-update-mistakes, step task#5, UpdateUserPool:
  expected HTTP 400, body {"__type": "InvalidSmsRoleTrustRelationshipException", "message": "Role does not have a trust relationship allowing Cognito to assume the role"}, header x-amzn-errortype present
  actual   HTTP 200, body {} (empty_body), no x-amzn-errortype header
  cluster also contains cognito-idp-acr-traj-recovery-setting-update-mistakes#task#5 (affected_count 2)

Cause 2 — fixture cognito-idp-acr-traj-sms-mfa-config-mistakes, step task#6, SetUserPoolMfaConfig:
  expected HTTP 400, $.__type = "InvalidSmsRoleTrustRelationshipException", $.message = "Role does not have a trust relationship allowing Cognito to assume the role", x-amzn-errortype present
  actual   HTTP 200, $.__type missing, $.message missing, x-amzn-errortype missing

Cause 3 — fixture cognito-idp-acr-traj-forgot-password-falls-to-email, step task#10, ForgotPassword:
  expected HTTP 400, $.__type = "InvalidParameterException", $.message = "Cannot reset password for the user as there is no registered/verified email or phone_number", x-amzn-errortype present
  actual   HTTP 200, $.__type missing, $.message missing, x-amzn-errortype missing

Model evidence: InvalidSmsRoleTrustRelationshipException — errorKind client, @httpError 400, bound to both UpdateUserPool and SetUserPoolMfaConfig; InvalidParameterException — errorKind client, @httpError 400, bound to ForgotPassword (Smithy model com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json).

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Cause 1: replay fixture cognito-idp-acr-traj-enable-phone-recovery-update-mistakes and inspect step task#5 — UpdateUserPool supplies an SmsConfiguration whose SnsCallerArn role does not trust cognito-idp.amazonaws.com; the emulator answers 200 {} instead of 400 InvalidSmsRoleTrustRelationshipException. Same for cognito-idp-acr-traj-recovery-setting-update-mistakes#task#5.
4. Cause 2: replay fixture cognito-idp-acr-traj-sms-mfa-config-mistakes and inspect step task#6 — SetUserPoolMfaConfig with an SmsMfaConfiguration carrying the same mis-trusted role answers 200 instead of 400 InvalidSmsRoleTrustRelationshipException.
5. Cause 3: replay fixture cognito-idp-acr-traj-forgot-password-falls-to-email and inspect step task#10 — ForgotPassword for a user with no verified email or phone_number answers 200 with CodeDeliveryDetails instead of 400 InvalidParameterException "Cannot reset password for the user as there is no registered/verified email or phone_number".
6. After fixing, re-replay all three fixtures (plus cognito-idp-acr-traj-recovery-setting-update-mistakes) and confirm each step now matches status, __type, message and the x-amzn-errortype header.
