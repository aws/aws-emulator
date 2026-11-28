Issue-Source: trace-sweep-divergence
Issue-Trace: cognito-idp-acr-traj-forgot-password-falls-to-email
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cognito-idp-acr-traj-forgot-password-falls-to-email
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$.__type
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$.message
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: cognito-idp|ForgotPassword|4xx->2xx|status
Issue-Key: cognito-idp|ForgotPassword|2xx->2xx|$.CodeDeliveryDetails.Destination
Issue-Related: 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md
Issue-Commit: 47635055653444c8189ab0ed1a8047834cbe5b1c

# cognito-idp ForgotPassword ignores verified-attribute state and AccountRecoverySetting when choosing the reset channel (2 causes)

> _Note: this issue was identified against the code base at commit `47635055653444c8189ab0ed1a8047834cbe5b1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgotPassword

## Description
Two net-new causes sharing one root defect: the emulator's ForgotPassword does not consult the user's verified-attribute state or the pool's AccountRecoverySetting when it decides where to send the reset code. It neither refuses a user with nothing verified, nor follows the recovery preference order when both attributes exist in different verification states.

Cause 1 of 2: ForgotPassword for a user with no verified email and no verified phone number succeeds instead of being refused.
  AWS: Smithy ForgotPassword documentation: "Sends a password-reset confirmation code to the email address or phone number of the requested username. The message delivery method is determined by the user's available attributes and the AccountRecoverySetting configuration of the user pool. ... If neither a verified phone number nor a verified email exists, Amazon Cognito responds with an InvalidParameterException error." ForgotPassword models InvalidParameterException with httpError 400. The recorded response is HTTP 400 with __type "InvalidParameterException", the x-amzn-errortype header naming it, and message "Cannot reset password for the user as there is no registered/verified email or phone_number" — that message is entirely fixed text for this error class (it names the attribute classes, not the user).
  Emulator: HTTP 200 with an empty body — no __type, no message, no x-amzn-errortype header. The operation reports success for a user that has no deliverable recovery channel at all.
  Why: a client is told a reset code was sent when none could be. Recovery UIs advance to the "enter your code" step for an account that can never receive one, and the AWS-documented guard that tells an application to collect and verify a contact attribute first is never exercised. Any test asserting the refusal for an unverifiable user passes vacuously.
  Fix: the ForgotPassword handler's pre-delivery validation — check the user's verified email/phone state before selecting a delivery medium and raise InvalidParameterException (400) with AWS's fixed message when neither is verified.
  Traces: cognito-idp-acr-traj-forgot-password-falls-to-email#task#10.

Cause 2 of 2: when the recovery channel should fall to email, ForgotPassword still reports SMS delivery to the phone number.
  AWS: Smithy ForgotPassword output CodeDeliveryDetails (CodeDeliveryDetailsType with members Destination, DeliveryMedium (DeliveryMediumType enum), AttributeName), and the operation doc: "The message delivery method is determined by the user's available attributes and the AccountRecoverySetting configuration of the user pool." So the triple AWS returns is a function of which attributes are verified and of the pool's recovery-mechanism priority: the chosen medium is EMAIL with AttributeName "email" when email is the eligible/preferred recovery attribute, and SMS with AttributeName "phone_number" only when the phone number is. Destination is the masked form of that attribute's value, taken from the user's state (the recording's 's***@e***' and '+*******0100' are masks of one run's values, not requirements).
  Emulator: returns AttributeName "phone_number", DeliveryMedium "SMS" and a masked phone destination where AWS selects AttributeName "email", DeliveryMedium "EMAIL" and the masked email — i.e. it picks the phone number regardless of the verification state and the recovery preference that make email the channel for this user.
  Why: a client reads CodeDeliveryDetails to tell the user where to look for the code, so it instructs them to check SMS for a code that AWS would have mailed; it also implies a phone channel is usable for an account whose phone is not an eligible recovery attribute. Flows that branch on DeliveryMedium (e.g. to show a phone-digit hint vs an email hint, or to pick the verification attribute for the follow-up ConfirmForgotPassword) take the wrong branch.
  Fix: the ForgotPassword handler's delivery-medium selection and the CodeDeliveryDetails it serializes — resolve the channel from the user's verified attributes ordered by the pool's AccountRecoverySetting priority, then report the matching AttributeName/DeliveryMedium/masked Destination.
  Traces: cognito-idp-acr-traj-forgot-password-falls-to-email#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 (200 fixtures, 1815 steps) against the emulator built from this checkout. Both clusters reported owned_by = [].

Cause 1 — fixture cognito-idp-acr-traj-forgot-password-falls-to-email step task#10, ForgotPassword:
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterException', got None)
  $.message: missing (expected 'Cannot reset password for the user as there is no registered/verified email or phone_number', got None)
  $headers.x-amzn-errortype: missing (expected the InvalidParameterException errortype, got None)

Cause 2 — same fixture, step task#11, ForgotPassword, expected 200 / actual 200:
  $.CodeDeliveryDetails.AttributeName: value_mismatch (expected 'email', got 'phone_number')
  $.CodeDeliveryDetails.DeliveryMedium: value_mismatch (expected 'EMAIL', got 'SMS')
  $.CodeDeliveryDetails.Destination: value_mismatch (expected a masked email, got a masked phone number)

Model evidence: Smithy ForgotPassword doc — "The message delivery method is determined by the user's available attributes and the AccountRecoverySetting configuration of the user pool" and "If neither a verified phone number nor a verified email exists, Amazon Cognito responds with an InvalidParameterException error"; ForgotPassword errors include InvalidParameterException (httpError 400); output CodeDeliveryDetailsType = {Destination, DeliveryMedium (enum), AttributeName}.

No open issue owns these steps. 20261027T020619Z-cognito-idp-lambda-the-usermigration-trigger-is-never.md touches ForgotPassword but only as the driver for the UserMigration Lambda trigger, and 20261102T111936Z-cognito-idp-response-payloads-diverge-read-paths-omit.md covers read-path payloads, not ForgotPassword channel selection.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1; see fixture cognito-idp-acr-traj-forgot-password-falls-to-email.
2. Cause 1: create a user pool with an AccountRecoverySetting, create a user whose email and phone_number are both unverified (or absent), then call ForgotPassword(ClientId=<client>, Username=<user>). Observe HTTP 200 with an empty body instead of HTTP 400 InvalidParameterException 'Cannot reset password for the user as there is no registered/verified email or phone_number' — step task#10.
3. Cause 2: bring the account into the state where email is the eligible/preferred recovery attribute and the phone number is not, then call ForgotPassword again. Observe CodeDeliveryDetails = {AttributeName 'phone_number', DeliveryMedium 'SMS', masked phone} instead of {AttributeName 'email', DeliveryMedium 'EMAIL', masked email} — step task#11.
4. Both steps replay from the one fixture, so re-replaying cognito-idp-acr-traj-forgot-password-falls-to-email confirms a fix for both causes.
