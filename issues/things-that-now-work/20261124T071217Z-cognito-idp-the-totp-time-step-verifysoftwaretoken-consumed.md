Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.mfa-setup-challenge-flow-software-token
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|AdminRespondToAuthChallenge|ExpiredCodeException
Issue-Commit: df434ad525a41e469c72bb6203f43a3d7d3f2407

# cognito-idp: the TOTP time step VerifySoftwareToken consumed during MFA_SETUP is left spent, so the first SOFTWARE_TOKEN_MFA sign-in after enrollment is refused ExpiredCodeException

> _Note: this issue was identified against the code base at commit `df434ad525a41e469c72bb6203f43a3d7d3f2407`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminRespondToAuthChallenge

## Description
One net-new root cause on the software-token MFA enrollment-to-sign-in path. The MFA_SETUP challenge itself is now issued and its completion works (that part of open issue 20261029T182116Z-cognito-idp-mfa-challenge-state-machine-mfa-setup.md has moved on); the check now fails one leg later, on the FIRST sign-in that presents the freshly enrolled authenticator, which no open issue covers.

Cause 1 of 1 — a registration's TOTP time step is treated as spent for the subsequent authentication challenge.

AWS: recorded behavior only for the acceptance (the model states no code-reuse rule). The recording of this check shows the documented TOTP enrollment flow completing end to end: with MfaConfiguration=ON and no factor enrolled, (Admin)InitiateAuth answers ChallengeName='MFA_SETUP'; AssociateSoftwareToken(Session) returns a SecretCode and a Session; VerifySoftwareToken(Session, UserCode=<TOTP for the current step>) returns Status='SUCCESS' and a Session; AdminRespondToAuthChallenge(ChallengeName='MFA_SETUP', that Session, ChallengeResponses={USERNAME}) returns AuthenticationResult; the NEXT AdminInitiateAuth then answers ChallengeName='SOFTWARE_TOKEN_MFA', and AdminRespondToAuthChallenge(ChallengeName='SOFTWARE_TOKEN_MFA', ChallengeResponses={USERNAME, SOFTWARE_TOKEN_MFA_CODE=<TOTP read at that moment>}) is ACCEPTED and returns an AuthenticationResult whose AccessToken GetUser resolves to that same user — even though that sign-in happens within the same ~30 s RFC 6238 time step whose one-time password the immediately preceding VerifySoftwareToken consumed. Smithy: AdminRespondToAuthChallenge declares ExpiredCodeException (client, HTTP 400, 'This exception is thrown if a code has expired') and CodeMismatchException (400) among its errors; VerifySoftwareToken's own model declares no expired-code error.

Emulator: that second AdminRespondToAuthChallenge is refused 400 ExpiredCodeException with the message 'Your software token has already been used once.' The registration records the time step it verified as spent (state/software_tokens.rs: register_software_token -> User::spend_software_token_step), and the authentication check then treats every step at or below the spent one as used (User::software_token_step_spent: `step <= spent`), so answer_mfa_challenge returns MfaAnswer::CodeAlreadyUsed and the handler maps it to ExpiredCodeException. Because the drift window is only +/-1 step, no code the authenticator can show in that step is acceptable: the sign-in cannot be completed until the clock rolls into a later step.

Message detail: 'Your software token has already been used once.' is fixed text that interpolates nothing — it is the right sentence for a genuinely replayed one-time password, but the wrong answer for a code that only a REGISTRATION consumed; the required response here is 200 with an AuthenticationResult, not a 400 of any code.

Why: a client that follows AWS's documented TOTP setup flow enrolls the factor successfully and is then locked out of the very next sign-in — the standard 'enroll, then sign in' sequence an app performs in one user session. A caller cannot distinguish this from a real replay attack, and any suite that exercises TOTP MFA end to end passes on AWS and fails on the emulator.

Fix: the software-token spend bookkeeping. Registering an authenticator proves possession; it does not authenticate a sign-in, so the step it consumed must not be charged against the user's authentication codes — either register_software_token stops recording the step as spent, or the spent-step check in answer_mfa_challenge (state/mfa_challenge.rs) only consults steps spent by an accepted authentication. Keep the genuine single-use rule for two successive sign-ins with the same code.

Traces: cognito-idp.mfa-setup-challenge-flow-software-token.

## Evidence
Verbatim from the replay of cognito-idp.mfa-setup-challenge-flow-software-token:

botocore.errorfactory.ExpiredCodeException: An error occurred (ExpiredCodeException) when calling the AdminRespondToAuthChallenge operation: Your software token has already been used once.

The check reached that call only after the earlier legs SUCCEEDED against the emulator: AdminInitiateAuth answered ChallengeName='MFA_SETUP', AssociateSoftwareToken returned a SecretCode, VerifySoftwareToken returned Status='SUCCESS', the MFA_SETUP AdminRespondToAuthChallenge returned an AuthenticationResult whose AccessToken GetUser resolved to the user, and the subsequent AdminInitiateAuth answered ChallengeName='SOFTWARE_TOKEN_MFA'. The failing call is the SOFTWARE_TOKEN_MFA response, whose ChallengeResponses carried {USERNAME, SOFTWARE_TOKEN_MFA_CODE=<TOTP for the current step>}; the check only tolerates CodeMismatchException while retrying adjacent steps and re-raises anything else, so the ExpiredCodeException ends the run.

Expected: HTTP 200 with AuthenticationResult (AccessToken/IdToken/RefreshToken) for the enrolled factor.
Actual: HTTP 400 ExpiredCodeException, message 'Your software token has already been used once.' (fixed text, no interpolated values).

Emulator source of the refusal: state/software_tokens.rs register_software_token() calls User::spend_software_token_step(step) for the code VerifySoftwareToken verified; User::software_token_step_spent() then answers true for every `step <= spent`; state/mfa_challenge.rs answer_mfa_challenge() returns MfaAnswer::CodeAlreadyUsed, and ops/admin_respond_to_auth_challenge.rs maps that to code_already_used() -> ExpiredCodeException with crate::state::SOFTWARE_TOKEN_CODE_USED_MESSAGE.

## Steps to Reproduce
1. CreateUserPool (lenient password policy), CreateUserPoolClient with ExplicitAuthFlows including ALLOW_ADMIN_USER_PASSWORD_AUTH, then SetUserPoolMfaConfig(MfaConfiguration='ON', SoftwareTokenMfaConfiguration={Enabled: true}).
2. AdminCreateUser + AdminSetUserPassword(Permanent=true) for a user with no enrolled factor.
3. AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) -> ChallengeName='MFA_SETUP', keep the Session.
4. AssociateSoftwareToken(Session=<that session>) -> SecretCode + Session.
5. VerifySoftwareToken(Session=<association session>, UserCode=<TOTP(SecretCode) for the current 30 s step>) -> Status='SUCCESS' + Session.
6. AdminRespondToAuthChallenge(ChallengeName='MFA_SETUP', Session=<verification session>, ChallengeResponses={USERNAME}) -> AuthenticationResult.
7. AdminInitiateAuth(ADMIN_USER_PASSWORD_AUTH) again -> ChallengeName='SOFTWARE_TOKEN_MFA'.
8. Within the same 30 s time step, AdminRespondToAuthChallenge(ChallengeName='SOFTWARE_TOKEN_MFA', Session=<that session>, ChallengeResponses={USERNAME, SOFTWARE_TOKEN_MFA_CODE=<TOTP(SecretCode) now>}).
   Expected: 200 with AuthenticationResult, and GetUser(AccessToken) returning that username.
   Observed: 400 ExpiredCodeException 'Your software token has already been used once.'
