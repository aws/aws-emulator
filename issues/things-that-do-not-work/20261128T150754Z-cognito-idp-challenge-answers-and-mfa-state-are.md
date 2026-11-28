Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-respond-to-auth-challenge-refuses-a-session-whose-user-was-deleted cognito-idp.mfa-setup-answer-on-a-pool-without-mfa-rejected cognito-idp.device-srp-answer-to-a-new-password-challenge-refused-and-spends-the-session cognito-idp.select-mfa-type-answer-to-a-totp-challenge-rejected cognito-idp.mfa-setup-session-verify-for-a-deleted-user-rejected cognito-idp.admin-set-user-mfa-preference-method-not-enrolled-rejected cognito-idp.set-software-token-mfa-preference-unregistered-rejected cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected cognito-idp.associate-software-token-rejected-until-pool-enables-totp cognito-idp.software-token-verified-while-pool-mfa-off-takes-effect-on-reenable cognito-idp.verify-software-token-with-totp-off-in-the-pool-rejected cognito-idp.set-user-settings-refused-while-pool-mfa-off cognito-idp.admin-set-user-settings-sms-option-needs-pool-sms-mfa-rejected cognito-idp.set-user-settings-attribute-name-over-32-rejected cognito-idp.set-user-settings-rejects-non-phone-mfa-attribute cognito-idp.pending-software-token-challenge-completes-after-pool-mfa-turned-off cognito-idp.admin-confirm-sign-up-confirms-after-code-attempts-exhausted cognito-idp.confirm-sign-up-attempt-limit-rejected-after-repeated-invalid-codes cognito-idp.admin-respond-new-password-preferred-username-taken-alias-rejected cognito-idp.new-password-challenge-taken-preferred-username-rejected cognito-idp.post-authentication-error-rejects-new-password-challenge-response
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-respond-to-auth-challenge-refuses-a-session-whose-user-was-deleted|run#13|AssertionError|assert got == _DEAD_SESSION, f"answering for a deleted user should give {_DEAD_SESSION!r}, got {got!r}"
Issue-Key: cognito-idp.mfa-setup-answer-on-a-pool-without-mfa-rejected|run#14|AssertionError|assert got == _MISMATCH, f"an MFA_SETUP answer gave {got!r}, expected {_MISMATCH!r}"
Issue-Key: cognito-idp.device-srp-answer-to-a-new-password-challenge-refused-and-spends-the-session|run#15|AssertionError|assert got == want, f"DEVICE_SRP_AUTH before authenticating answered {got!r}, expected {want!r}"
Issue-Key: cognito-idp.select-mfa-type-answer-to-a-totp-challenge-rejected|run#23|AssertionError|assert got == _MISMATCH, f"a SELECT_MFA_TYPE answer gave {got!r}, expected {_MISMATCH!r}"
Issue-Key: cognito-idp.mfa-setup-session-verify-for-a-deleted-user-rejected|run#14|AssertionError|assert refused == ("UserNotFoundException", "User does not exist."), (
Issue-Key: cognito-idp.admin-set-user-mfa-preference-method-not-enrolled-rejected|run#17|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.set-software-token-mfa-preference-unregistered-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.associate-software-token-rejected-until-pool-enables-totp|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {got!r}")
Issue-Key: cognito-idp.software-token-verified-while-pool-mfa-off-takes-effect-on-reenable|_refusal#20|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: cognito-idp.verify-software-token-with-totp-off-in-the-pool-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {sorted(result)!r}")
Issue-Key: cognito-idp.set-user-settings-refused-while-pool-mfa-off|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: cognito-idp.admin-set-user-settings-sms-option-needs-pool-sms-mfa-rejected|_refusal#11|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: cognito-idp.set-user-settings-attribute-name-over-32-rejected|run#24|AssertionError|assert got == (
Issue-Key: cognito-idp.set-user-settings-rejects-non-phone-mfa-attribute|run#15|AssertionError|assert got == ("InvalidParameterException", "Only phone_number attribute is currently supported as a MFA option."), (
Issue-Key: cognito-idp.pending-software-token-challenge-completes-after-pool-mfa-turned-off|run#26|AssertionError|assert "ChallengeName" not in later, (
Issue-Key: cognito-idp.admin-confirm-sign-up-confirms-after-code-attempts-exhausted|run#13|AssertionError|assert got == locked, f"{_MAX_GUESSES} wrong codes never reached the attempt lockout; the last answered {got!r}"
Issue-Key: cognito-idp.confirm-sign-up-attempt-limit-rejected-after-repeated-invalid-codes|run#13|AssertionError|assert capped_answer == _ATTEMPTS_SPENT, (
Issue-Key: cognito-idp.admin-respond-new-password-preferred-username-taken-alias-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {sorted(result)!r}")
Issue-Key: cognito-idp.new-password-challenge-taken-preferred-username-rejected|_refused#17|AssertionError|raise AssertionError(f"the challenge response was expected to be refused but returned {result!r}")
Issue-Key: cognito-idp.post-authentication-error-rejects-new-password-challenge-response|_refusal#29|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {sorted(out)!r}")
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp: challenge answers and MFA state are judged wrongly (10 causes: session refusals classed as CodeMismatch, MFA-preference wording, TOTP/SMS gating, code-attempt lockout, alias conflicts, PostAuthentication)

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminRespondToAuthChallenge

## Description
Ten defects in the challenge/MFA state machine. Each diverging status, error code and message below is the requirement; the ids and generated values in the recordings are only fillers.

Cause 1 of 10: every unexpected challenge answer is classed as CodeMismatchException.
AWS: the answer is first judged against the session and the challenge it carries, and each case has its own modelled refusal — a session whose user was deleted and an MFA_SETUP answer on a pool without MFA give NotAuthorizedException 'Invalid session for the user.'; a DEVICE_SRP_AUTH answer before authenticating gives NotAuthorizedException 'Cannot initiate DEVICE_SRP_AUTH flow without first authenticating'; a SELECT_MFA_TYPE answer to a TOTP challenge gives NotAuthorizedException 'The chosen MFA method cannnot be triggered.' (AWS's spelling).
Emulator: answers CodeMismatchException with 'Invalid code/session provided' or 'Invalid code or auth state for the user.' in all four cases.
Why: clients retry a code they cannot fix instead of restarting the sign-in, and the session-spending semantics differ.
Fix: the (Admin)RespondToAuthChallenge answer dispatcher must validate session/challenge/user before comparing codes.
Traces: cognito-idp.admin-respond-to-auth-challenge-refuses-a-session-whose-user-was-deleted, cognito-idp.mfa-setup-answer-on-a-pool-without-mfa-rejected, cognito-idp.device-srp-answer-to-a-new-password-challenge-refused-and-spends-the-session, cognito-idp.select-mfa-type-answer-to-a-totp-challenge-rejected.

Cause 2 of 10: an MFA-setup session for a deleted user hides the missing user.
AWS: VerifySoftwareToken through the setup session of a deleted user refuses UserNotFoundException 'User does not exist.'
Emulator: answers NotAuthorizedException 'Invalid session for the user.'
Why: the caller is told to restart sign-in for a user that no longer exists.
Fix: resolve the session's user before validating the session in VerifySoftwareToken.
Traces: cognito-idp.mfa-setup-session-verify-for-a-deleted-user-rejected.

Cause 3 of 10: the MFA-preference refusal is worded wrongly.
AWS: (Admin)SetUserMFAPreference naming a method the user has not enrolled refuses InvalidParameterException 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA' (the method name comes from the request).
Emulator: answers 'User has not verified software token mfa'.
Why: message divergence on the documented refusal for all three preference paths (not enrolled, unregistered, conflicting Preferred).
Fix: the SetUserMFAPreference/AdminSetUserMFAPreference validation message.
Traces: cognito-idp.admin-set-user-mfa-preference-method-not-enrolled-rejected, cognito-idp.set-software-token-mfa-preference-unregistered-rejected, cognito-idp.set-user-mfa-preference-conflicting-preferred-method-rejected.

Cause 4 of 10: TOTP enrolment is not gated on the pool's MFA configuration.
AWS: while the pool has software-token MFA off, AssociateSoftwareToken and VerifySoftwareToken are refused (recorded behavior only: the recordings show a ClientError); enrolment becomes possible once SetUserPoolMfaConfig turns TOTP on.
Emulator: answers HTTP 200 with a SecretCode and verifies the token regardless of the pool configuration.
Why: a pool with MFA disabled can enrol and verify TOTP factors, so the pool-level switch means nothing.
Fix: the AssociateSoftwareToken/VerifySoftwareToken handlers must check the pool's MfaConfiguration and SoftwareTokenMfaConfiguration.
Traces: cognito-idp.associate-software-token-rejected-until-pool-enables-totp, cognito-idp.software-token-verified-while-pool-mfa-off-takes-effect-on-reenable, cognito-idp.verify-software-token-with-totp-off-in-the-pool-rejected.

Cause 5 of 10: SetUserSettings/AdminSetUserSettings accept SMS MFA options the pool does not offer.
AWS: refuses the call when the pool has MFA off / no SMS MFA (recorded behavior only: a ClientError in both recordings).
Emulator: answers HTTP 200 with an empty body.
Why: per-user MFA settings are stored for a pool that cannot deliver them.
Fix: pool-MFA validation in the (Admin)SetUserSettings handlers.
Traces: cognito-idp.set-user-settings-refused-while-pool-mfa-off, cognito-idp.admin-set-user-settings-sms-option-needs-pool-sms-mfa-rejected.

Cause 6 of 10: the MFAOptions.AttributeName refusals are hand-written.
AWS: a 33-character AttributeName is refused with the validation message "1 validation error detected: Value '<name>' at 'mFAOptions.1.member.attributeName' failed to satisfy constraint: Member must have length less than or equal to 32" (note the member path's lower-cased first letter); a well-formed but non-phone name is refused InvalidParameterException 'Only phone_number attribute is currently supported as a MFA option.'
Emulator: answers 'Invalid AttributeName: <name>, the only attribute name an MFA option can name is phone_number' in both cases, so the length bound is never reported.
Why: both refusals carry text AWS never sends, and the length constraint is invisible.
Fix: SetUserSettings input validation plus the semantic check's message.
Traces: cognito-idp.set-user-settings-attribute-name-over-32-rejected, cognito-idp.set-user-settings-rejects-non-phone-mfa-attribute.

Cause 7 of 10: a pending SOFTWARE_TOKEN_MFA challenge survives the pool turning MFA off.
AWS: once SetUserPoolMfaConfig turns MFA off, the next sign-in is not challenged and completes with tokens.
Emulator: still answers ChallengeName 'SOFTWARE_TOKEN_MFA'.
Why: a pool that disabled MFA locks out users whose enrolment is pending.
Fix: evaluate the pool's live MFA configuration when deciding the sign-in challenge.
Traces: cognito-idp.pending-software-token-challenge-completes-after-pool-mfa-turned-off.

Cause 8 of 10: the confirmation-code attempt lockout is missing and the wrong error is used.
AWS: repeated wrong ConfirmSignUp codes end in CodeMismatchException 'Exceed max confirmation code attempts. Please resend confirmation code and retry.'; after the attempts are exhausted AdminConfirmSignUp still confirms the user.
Emulator: answers ExpiredCodeException 'Invalid code provided, please request a code again.' indefinitely — 20 wrong codes never reach a lockout.
Why: a client cannot distinguish an expired code from an exhausted attempt budget, and the lockout AWS enforces does not exist.
Fix: per-user confirmation-attempt accounting in the ConfirmSignUp handler and its error classification.
Traces: cognito-idp.confirm-sign-up-attempt-limit-rejected-after-repeated-invalid-codes, cognito-idp.admin-confirm-sign-up-confirms-after-code-attempts-exhausted.

Cause 9 of 10: alias conflicts are not enforced when answering the NEW_PASSWORD_REQUIRED challenge.
AWS: a challenge response whose userAttributes.preferred_username is already held by another user in an alias pool is refused (recorded behavior only: the recordings show a ClientError and no tokens).
Emulator: completes the challenge and returns AuthenticationResult.
Why: two users end up holding the same alias, which AWS makes impossible.
Fix: alias-uniqueness validation in the challenge-response attribute write.
Traces: cognito-idp.admin-respond-new-password-preferred-username-taken-alias-rejected, cognito-idp.new-password-challenge-taken-preferred-username-rejected.

Cause 10 of 10: a failing PostAuthentication trigger does not fail the challenge response.
AWS: when the PostAuthentication trigger errors, the RespondToAuthChallenge that completes the sign-in is refused and no tokens are issued.
Emulator: returns AuthenticationResult/ChallengeParameters as if the trigger had succeeded.
Why: a pool that uses PostAuthentication to veto sign-ins cannot block one.
Fix: propagate trigger failures from the challenge-response path.
Traces: cognito-idp.post-authentication-error-rejects-new-password-challenge-response.

## Evidence
Cause 1 — deleted user's session: expected ('NotAuthorizedException', 'Invalid session for the user.'), got ('CodeMismatchException', 'Invalid code/session provided'). MFA_SETUP on a pool without MFA: got ('CodeMismatchException', 'Invalid code or auth state for the user.'). DEVICE_SRP_AUTH before authenticating: expected ('NotAuthorizedException', 'Cannot initiate DEVICE_SRP_AUTH flow without first authenticating'). SELECT_MFA_TYPE answer to TOTP: expected ('NotAuthorizedException', 'The chosen MFA method cannnot be triggered.').
Cause 2 — expected ('UserNotFoundException', 'User does not exist.'), got ('NotAuthorizedException', 'Invalid session for the user.').
Cause 3 — expected message 'User does not have delivery config set to turn on SOFTWARE_TOKEN_MFA', got 'User has not verified software token mfa' (all three checks).
Cause 4 — AssociateSoftwareToken with TOTP off returned {'SecretCode': 'JBHUYMDQJBFU4TZYO5EUSTDUK5LTEQLN', ... 'HTTPStatusCode': 200}; VerifySoftwareToken succeeded with keys ['SecretCode'].
Cause 5 — SetUserSettings/AdminSetUserSettings returned {'ResponseMetadata': {'HTTPStatusCode': 200 ...}} where a refusal was recorded.
Cause 6 — 33-character AttributeName: expected "1 validation error detected: Value '<name>' at 'mFAOptions.1.member.attributeName' failed to satisfy constraint: Member must have length less than or equal to 32", got 'Invalid AttributeName: sms_code_destination_attribute_x, the only attribute name an MFA option can name is phone_number'; 32-character non-phone name: expected 'Only phone_number attribute is currently supported as a MFA option.'
Cause 7 — "a sign-in after MFA was turned off was still challenged: 'SOFTWARE_TOKEN_MFA'".
Cause 8 — "20 wrong codes never reached the attempt lockout; the last answered ('ExpiredCodeException', 'Invalid code provided, please request a code again.')"; expected ('CodeMismatchException', 'Exceed max confirmation code attempts. Please resend confirmation code and retry.').
Cause 9 — "the challenge response was expected to be refused but returned {'ChallengeParameters': {}, 'AuthenticationResult': {...}}".
Cause 10 — "the call was expected to be refused but returned ['AuthenticationResult', 'ChallengeParameters', 'ResponseMetadata']".

## Steps to Reproduce
1. Replay the check ids in trace_ids.
2. Cause 1: start an MFA challenge, AdminDeleteUser, then AdminRespondToAuthChallenge with the session; separately answer MFA_SETUP on a pool with MFA off, DEVICE_SRP_AUTH on a NEW_PASSWORD_REQUIRED session, and SELECT_MFA_TYPE on a SOFTWARE_TOKEN_MFA challenge.
3. Cause 3: AdminSetUserMFAPreference with SoftwareTokenMfaSettings.Enabled=true for a user who never verified TOTP.
4. Cause 4: on a pool with MfaConfiguration=OFF call AssociateSoftwareToken/VerifySoftwareToken.
5. Cause 5/6: call SetUserSettings with MFAOptions=[{DeliveryMedium:'SMS', AttributeName:'phone_number'}] on a pool with MFA off, then with a 33-character and a 32-character non-phone AttributeName.
6. Cause 7: enrol TOTP, leave the challenge pending, SetUserPoolMfaConfig MfaConfiguration=OFF, sign in again.
7. Cause 8: SignUp, then ConfirmSignUp with wrong codes repeatedly.
8. Cause 9: AdminCreateUser two users in an alias pool, answer NEW_PASSWORD_REQUIRED with the other user's preferred_username.
9. Cause 10: attach a PostAuthentication trigger that errors and complete a NEW_PASSWORD_REQUIRED challenge.
