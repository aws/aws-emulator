Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.verify-software-token-wrong-code
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.verify-software-token-wrong-code|run#18|AssertionError|assert message == expected, (
Issue-Commit: 6f8ce328875e76c843b69942a6e480ca9be8ba40

# cognito-idp: SetUserMFAPreference uses the pool's no-delivery-config refusal message for a user who simply has no verified software token, so AWS's 'User has not verified software token mfa' is never returned

> _Note: this issue was identified against the code base at commit `6f8ce328875e76c843b69942a6e480ca9be8ba40`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SetUserMFAPreference

## Description
One net-new behavioral root cause in cognito-idp, grounded in a failing check of the behavioral suite.

=== Cause 1 of 1: the InvalidParameterException message for preferring software-token MFA is the pool-level delivery-config sentence even when the pool HAS software-token MFA on and it is the USER that has no verified token. ===

AWS: SetUserMFAPreference (and its admin sibling) refuses a request that enables/prefers software-token MFA for a user who cannot use that factor with InvalidParameterException (Smithy: SetUserMFAPreference declares InvalidParameterException @httpError 400, plus ForbiddenException/NotAuthorizedException 403, OperationNotEnabledException 400, ResourceNotFoundException 404; input member SoftwareTokenMfaSettings -> SoftwareTokenMfaSettingsType, documented as "A user's preference for using time-based one-time password (TOTP) multi-factor authentication (MFA). Turns TOTP MFA on and off..."). The model does not spell the message out, so the two messages below are recorded behavior, each attested by checks of this suite. AWS distinguishes TWO different rules behind that one error class:
  (a) the POOL has no software-token delivery configured (SetUserPoolMfaConfig was never given SoftwareTokenMfaConfiguration={'Enabled': True}): message 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA'. This is the case the suite's cognito-idp.set-software-token-mfa-preference-unregistered-rejected / admin-set-user-mfa-preference-method-not-enrolled-rejected / set-user-mfa-preference-conflicting-preferred-method-rejected checks exercise, and those three pass today.
  (b) the POOL does have software-token MFA turned on and AssociateSoftwareToken has been called, but the USER has no VERIFIED software token (VerifySoftwareToken never returned Status=SUCCESS for them): message 'User has not verified software token mfa'. This is the case the failing check exercises.
Both messages are entirely FIXED text: no username, pool id, client id or factor name is interpolated from the request or from state, and both come back as HTTP 400 / InvalidParameterException. In case (b) nothing is stored — a following GetUser reports no SOFTWARE_TOKEN_MFA in UserMFASettingList — and once VerifySoftwareToken succeeds with a correct TOTP code the identical SetUserMFAPreference call succeeds and SOFTWARE_TOKEN_MFA is listed.

Emulator: the status and error code are right (HTTP 400, InvalidParameterException) but the emulator renders ONE message for both rules — the pool-level sentence 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA' — even for case (b), where the pool's SoftwareTokenMfaConfiguration IS Enabled=true, AssociateSoftwareToken returned a SecretCode, and only the user-level verification is missing. The two rules are not told apart: whichever single sentence the shared rejection carries, one of the two cases is always wrong. (The suite shows the emulator has flipped from the other sentence to this one: open issue 20261115T175613Z-cognito-idp-4-net-new-causes-a-spent.md Cause 2 reports the emulator answering 'User has not verified software token mfa' where case (a) wants the delivery-config text; that cause's three checks now pass and THIS check, which wants the verification text for case (b), now fails. That issue does not own this check's replay key and its Cause 2 describes the opposite direction, so it does not cover this defect; closing it by swapping the single message again would simply re-break those three. The fix must branch on which precondition failed.)
Not a REOPENS: fixed issue 20261025T192709Z-cognito-idp-setusermfapreference-answers-501-notimplemented-instead-of.md only made this call refuse at all instead of answering 501 NotImplemented; AWS refuses this call too, so the fix did not leave a call wrongly refused — only the refusal's message text diverges.

Why: an HTTP status, an error code and an error message are all requirements. A client that matches AWS's sentence to tell its user what to do next gets the wrong remedy: AWS's case (b) text says the USER must verify their authenticator (the caller should run AssociateSoftwareToken + VerifySoftwareToken), while the emulator's text says the POOL's delivery configuration is missing (an administrator action on the user pool that is in fact already done). Fixtures and assertions recorded against AWS for the enrolment flow cannot be replayed, and the two distinct failure modes become indistinguishable to any caller that keys on the message.

Fix: the per-factor availability rejection in the shared state — crate::state::User::set_mfa_preference and the rejection's .message() it returns (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/state.rs), rendered by src/ops/set_user_mfa_preference.rs (the user.set_mfa_preference(&requested, now).map_err(|rejection| invalid_parameter(rejection.message()))? call) and identically by src/ops/admin_set_user_mfa_preference.rs. That rejection needs to distinguish the pool-level precondition (the pool's SoftwareTokenMfaConfiguration is not enabled) from the user-level one (the user has no verified software token) and carry the corresponding fixed sentence; the rule itself, the 400 and the InvalidParameterException class are already correct, and nothing may be written on either path.

Traces: cognito-idp.verify-software-token-wrong-code.

## Evidence
Verbatim from the replay:

cognito-idp.verify-software-token-wrong-code (check_cognito_idp_verify_software_token_wrong_code.py line 89):
  AssertionError: enabling MFA without a verified token: expected message 'User has not verified software token mfa', got 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA'

Expected: HTTP 400, Error.Code InvalidParameterException, Error.Message exactly 'User has not verified software token mfa' — entirely fixed text, nothing interpolated from the request or state — for SetUserMFAPreference(AccessToken, SoftwareTokenMfaSettings={'Enabled': True, 'PreferredMfa': True}) issued against a pool whose SoftwareTokenMfaConfiguration is Enabled=true, for a user who has an AssociateSoftwareToken SecretCode but no successful VerifySoftwareToken. The preceding legs of the check pass: the wrong TOTP code is correctly rejected by VerifySoftwareToken, and GetUser correctly reports no SOFTWARE_TOKEN_MFA in UserMFASettingList.
Actual: HTTP 400, Error.Code InvalidParameterException (both correct), Error.Message 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA' — the pool-level sentence, which is AWS's text only for the different case where the pool has no software-token delivery configured at all.

Corroborating, from the same suite run: cognito-idp.set-software-token-mfa-preference-unregistered-rejected, cognito-idp.admin-set-user-mfa-preference-method-not-enrolled-rejected and cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected are NOT in this run's failing list — they assert the delivery-config sentence and pass. So the emulator serves a single message for two distinct AWS rules, and exactly one of the two cases is wrong at any time.

Message source: services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/set_user_mfa_preference.rs:212-213 renders rejection.message() from crate::state::User::set_mfa_preference (src/state.rs), which is why the admin handler reports the same text.

## Steps to Reproduce
1. CreateUserPool(PoolName=...); SetUserPoolMfaConfig(UserPoolId, SoftwareTokenMfaConfiguration={'Enabled': True}, MfaConfiguration='OPTIONAL') — note the pool's software-token delivery IS configured.
2. CreateUserPoolClient with GenerateSecret=False and ExplicitAuthFlows ALLOW_ADMIN_USER_PASSWORD_AUTH / ALLOW_USER_PASSWORD_AUTH / ALLOW_REFRESH_TOKEN_AUTH.
3. AdminCreateUser(MessageAction='SUPPRESS') + AdminSetUserPassword(Permanent=True); AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) to obtain an AccessToken.
4. AssociateSoftwareToken(AccessToken) -> SecretCode. Compute the current RFC 6238 TOTP code for that secret and submit a DIFFERENT (wrong) code to VerifySoftwareToken: it is refused / answers Status='ERROR', so the user still has NO verified software token.
5. SetUserMFAPreference(AccessToken, SoftwareTokenMfaSettings={'Enabled': True, 'PreferredMfa': True}). Expect HTTP 400 InvalidParameterException with Error.Message exactly 'User has not verified software token mfa'; observe 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA'.
6. Confirm nothing was stored: GetUser(AccessToken) must not list SOFTWARE_TOKEN_MFA in UserMFASettingList.
7. Positive control: VerifySoftwareToken with a correct, fresh TOTP code (Status='SUCCESS'), then re-issue the identical SetUserMFAPreference call — it must succeed and GetUser must list SOFTWARE_TOKEN_MFA.
8. Regression control for the OTHER rule, which must keep its own sentence: create a second pool WITHOUT SetUserPoolMfaConfig software-token delivery and issue the same SetUserMFAPreference call — it must answer 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA'.
