Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config|run#13|AssertionError|assert details["DeliveryMedium"] == "EMAIL", (
Issue-Commit: 9dce3feac93335da51a8ae3fd80bb5ebd7138744

# cognito-idp: ForgotPassword picks SMS for a phone-first pool that has no SmsConfiguration instead of falling through to verified_email

> _Note: this issue was identified against the code base at commit `9dce3feac93335da51a8ae3fd80bb5ebd7138744`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ForgotPassword

## Description
One net-new cause: the emulator's ForgotPassword delivery-channel selection consults only the recovery-mechanism priority order and the user's verified attributes; it never checks whether the pool can actually deliver on the selected channel. A pool whose AccountRecoverySetting is phone-first (Priority 1 = verified_phone_number, Priority 2 = verified_email) but that carries NO SmsConfiguration therefore gets SMS selected for any user with a verified phone number, where AWS skips the undeliverable priority-1 mechanism and falls through to priority-2 verified_email.

Cause 1 of 1: ForgotPassword returns DeliveryMedium 'SMS' / AttributeName 'phone_number' on a pool with no SmsConfiguration.
  AWS: Smithy ForgotPassword doc — "Sends a password-reset confirmation code to the email address or phone number of the requested username. The message delivery method is determined by the user's available attributes and the AccountRecoverySetting configuration of the user pool." AccountRecoverySettingType.RecoveryMechanisms is a priority-ordered list of RecoveryOptionType {Priority (1 = highest), Name in [verified_email, verified_phone_number, admin_only]}, and RecoveryOptionType's doc states the pool "sends account-recovery messages to" the higher-priority attribute "but falls back to" the other one. A user pool with no SmsConfiguration has no SMS delivery capability at all, so the verified_phone_number mechanism is not an available delivery method for it and the next mechanism in priority order is used. The output is CodeDeliveryDetailsType {DeliveryMedium (DeliveryMediumType enum) = 'EMAIL', AttributeName = 'email', Destination = the MASKED form of that user's email address — masking keeps the leading character of the local part and substitutes '*' for the rest; the concrete masked string is a function of the user's stored email, never a fixed literal}. Status 200. A user who has no phone attribute at all must get the identical triple, since the phone made no difference.
  Emulator: HTTP 200 with CodeDeliveryDetails.DeliveryMedium 'SMS' (and correspondingly the phone-number attribute/destination) for a user whose phone_number_verified = 'true' on a pool created WITHOUT SmsConfiguration, where AWS answers DeliveryMedium 'EMAIL' with AttributeName 'email' and a masked email destination. The check asserts the premise from the service first (DescribeUserPool shows RecoveryMechanisms[0].Name == 'verified_phone_number' and no SmsConfiguration member), so the divergence is attributable purely to the missing delivery capability.
  Why: the pool cannot send SMS, so the code the emulator claims to have sent by SMS can never arrive. A recovery UI reads CodeDeliveryDetails to tell the user where to look and branches on DeliveryMedium (phone-digit hint vs email hint, and which attribute the follow-up ConfirmForgotPassword verifies); against the emulator it sends every such user to a dead SMS channel and the account is effectively unrecoverable, while against AWS the same pool recovers by email. It also hides a real misconfiguration class (phone-first recovery with no SMS configured) that AWS silently degrades.
  Fix: the ForgotPassword handler's delivery-medium selection (and the CodeDeliveryDetails it serializes) — when walking the pool's AccountRecoverySetting in priority order, skip a mechanism whose channel the pool cannot deliver on (verified_phone_number when the pool has no SmsConfiguration) and continue to the next mechanism, reporting the matching AttributeName/DeliveryMedium and the masked destination for the attribute actually used.
  Traces: cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config.

Related but not covering: open issue 20261126T084136Z-cognito-idp-forgotpassword-ignores-verified-attribute-state-and.md covers ForgotPassword ignoring verified-attribute state and the recovery preference order (and the missing InvalidParameterException when nothing is verified). This cause is the pool's DELIVERY CAPABILITY gate: here the priority order IS honoured and the phone IS verified, and the defect is that an SMS-incapable pool still selects SMS. The suite did not mark this check as owned by that issue.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout; 794/856 passed, cognito-idp 627/656. Failing check (mismatch) cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config:

  File "check_cognito_idp_refarch_cog_verified_phone_still_recovers_by_email_without_sms_config.py", line 61, in run
    assert details["DeliveryMedium"] == "EMAIL"
  AssertionError: the priority-1 verified_phone_number mechanism is SKIPPED because the pool has no SmsConfiguration, so recovery falls through to priority-2 verified_email even for a user whose phone IS verified; Cognito reported DeliveryMedium 'SMS'

Expected (AWS): CodeDeliveryDetails.DeliveryMedium == 'EMAIL', AttributeName == 'email', Destination == masked email (contains '*', starts with the first character of the local part, != the address itself).
Actual (emulator): CodeDeliveryDetails.DeliveryMedium == 'SMS'.

Premise asserted from the emulator itself before the failure, so it is not a harness assumption:
  - DescribeUserPool: AccountRecoverySetting.RecoveryMechanisms[0].Name == 'verified_phone_number' (phone-first pool) and no 'SmsConfiguration' member present.
  - AdminGetUser: phone_number_verified == 'true' and email_verified == 'true'; UserStatus CONFIRMED.

Model evidence: Smithy ForgotPassword doc — "The message delivery method is determined by the user's available attributes and the AccountRecoverySetting configuration of the user pool"; AccountRecoverySettingType.RecoveryMechanisms = priority-ordered list of RecoveryOptionType {Priority range 1-2, Name enum [verified_email, verified_phone_number, admin_only]} whose doc describes the lower-priority mechanism as the fallback; output CodeDeliveryDetailsType = {Destination, DeliveryMedium (enum), AttributeName}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite; see check cognito-idp.refarch-cog-verified-phone-still-recovers-by-email-without-sms-config.
2. CreateUserPool with UsernameAttributes=['email'], AutoVerifiedAttributes=['email'], AdminCreateUserConfig.AllowAdminCreateUserOnly=true and AccountRecoverySetting.RecoveryMechanisms=[{Priority:1,Name:'verified_phone_number'},{Priority:2,Name:'verified_email'}]. Pass NO SmsConfiguration.
3. CreateUserPoolClient on that pool with ExplicitAuthFlows=['ALLOW_USER_PASSWORD_AUTH','ALLOW_REFRESH_TOKEN_AUTH'].
4. Confirm the premise: DescribeUserPool returns RecoveryMechanisms[0].Name == 'verified_phone_number' and carries no SmsConfiguration.
5. AdminCreateUser (MessageAction=SUPPRESS) with UserAttributes email/email_verified=true/phone_number/phone_number_verified=true, then AdminSetUserPassword(..., Permanent=true) so the user is CONFIRMED.
6. Call ForgotPassword(ClientId=<client>, Username=<email>). Observe CodeDeliveryDetails.DeliveryMedium 'SMS' instead of 'EMAIL' with AttributeName 'email' and a masked email Destination.
7. Create a second CONFIRMED user with only a verified email (no phone attribute) and call ForgotPassword for it: AWS returns the identical EMAIL/email triple as in step 6, which is the comparison the emulator breaks.
