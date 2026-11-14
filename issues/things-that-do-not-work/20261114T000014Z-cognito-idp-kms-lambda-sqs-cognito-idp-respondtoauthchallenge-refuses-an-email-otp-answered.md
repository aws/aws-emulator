Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected
Issue-Key: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected|run#19|AssertionError|assert (resp["Error"]["Code"], resp["Error"]["Message"]) == (
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# cognito-idp RespondToAuthChallenge refuses an EMAIL_OTP answered after AuthSessionValidity expired with ExpiredCodeException instead of NotAuthorizedException 'Invalid session for the user, session is expired.'

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RespondToAuthChallenge

## Description
One cause.

Cause 1 of 1: the expired auth SESSION is reported as an expired CODE, so the wrong modeled error class and message are served.
AWS: RespondToAuthChallenge carries a Session (Smithy: SessionType @length 20..4096, sensitive) whose lifetime is the app client's AuthSessionValidity (minimum 3 minutes). When a challenge is answered after that window closes, AWS refuses the call on the session, not on the code: NotAuthorizedException (modeled on RespondToAuthChallenge) with the fixed text "Invalid session for the user, session is expired." — the whole message is fixed text, with no value taken from the request. The email OTP itself outlives the session, so ExpiredCodeException (also modeled, but for a code that has expired or been superseded) is the wrong class here. The model does not state the message, so this wording is recorded behavior only. No AuthenticationResult accompanies the refusal, and the user is not locked out: a freshly started USER_AUTH/EMAIL_OTP challenge signs them in.
Emulator: answers ExpiredCodeException with "Invalid code provided, please request a code again." for a real, unused 8-digit EMAIL_OTP answered 30 s past a 3-minute AuthSessionValidity. It therefore attributes the refusal to the code rather than the session clock, changing both the error code and the message (and, because the two classes carry different modeled httpError values, the status class of the refusal as well).
Why: a client cannot distinguish 'your session timed out, restart the auth flow with InitiateAuth' from 'that code is wrong/expired, request another code'. ExpiredCodeException drives clients into re-requesting an OTP on the same dead session, which can never succeed, instead of restarting authentication; retry and UX logic keyed on NotAuthorizedException never fires. Note the open issue 20261025T182751Z-cognito-idp-kms-lambda-sqs-the-email-otp.md covered an earlier failure of this scenario (no real code was ever issued because the CustomEmailSender trigger was never invoked); a real code now arrives and the scenario reaches the refusal, where the class and text diverge. That issue does not carry this replay key.
Fix: the RespondToAuthChallenge handler's challenge-session validation in the cognito-idp crate — the session's expiry (derived from the app client's AuthSessionValidity at the time the challenge was issued) must be checked before, and independently of, the OTP's own validity, and must raise NotAuthorizedException with AWS's session-expired text.
Traces: cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected.

## Evidence
Verbatim from the replay of check cognito-idp__kms__lambda__sqs.deep-cognito-idp-1-real-code-email-otp-session-expiry-rejected:

AssertionError: an OTP answered after the session window was refused with {'Message': 'Invalid code provided, please request a code again.', 'Code': 'ExpiredCodeException'}, expected NotAuthorizedException

Expected (recorded AWS): Error.Code 'NotAuthorizedException', Error.Message 'Invalid session for the user, session is expired.' (entirely fixed text).
Actual (emulator): Error.Code 'ExpiredCodeException', Error.Message 'Invalid code provided, please request a code again.'
The positive legs of the same check pass: the same flow answered 30 s BEFORE the window closes issues tokens, so the emulator does refuse the late answer — only its attribution (code vs session), class and message diverge.

## Steps to Reproduce
1. Create an ESSENTIALS-tier user pool with SignInPolicy.AllowedFirstAuthFactors=[PASSWORD, EMAIL_OTP] and a CustomEmailSender trigger so a real OTP is delivered.
2. Create/update an app client with ExplicitAuthFlows=[ALLOW_USER_AUTH, ALLOW_USER_PASSWORD_AUTH, ALLOW_REFRESH_TOKEN_AUTH] and AuthSessionValidity=3 (minutes).
3. SignUp + ConfirmSignUp a user.
4. InitiateAuth(AuthFlow='USER_AUTH', AuthParameters={USERNAME, PREFERRED_CHALLENGE='EMAIL_OTP'}) and keep the returned Session; read the delivered 8-digit OTP.
5. Wait 3 minutes 30 seconds (past AuthSessionValidity), then RespondToAuthChallenge(ChallengeName='EMAIL_OTP', Session=<that session>, ChallengeResponses={USERNAME, EMAIL_OTP_CODE=<that real code>}).
6. Observe ExpiredCodeException 'Invalid code provided, please request a code again.'; AWS answers NotAuthorizedException 'Invalid session for the user, session is expired.' with no AuthenticationResult.
