Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry|run#34|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 832e9f5059543777b388ffa865d01db1cd157680

# cognito-identity: an EXPIRED developer-identity OpenID token is refused by GetCredentialsForIdentity with the message "Invalid login token. Not a Cognito token." instead of "Invalid login token. Token is expired."

> _Note: this issue was identified against the code base at commit `832e9f5059543777b388ffa865d01db1cd157680`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCredentialsForIdentity

## Description
Cause 1 of 1: GetCredentialsForIdentity mis-classifies an EXPIRED pool-issued OpenID token as a non-Cognito token, so the expiry-specific refusal message is never emitted.

Failing check: cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry

AWS: the developer-authenticated flow mints a token with GetOpenIdTokenForDeveloperIdentity (Smithy com.amazonaws.cognitoidentity#AWSCognitoIdentityService, awsJson1_1: input IdentityPoolId required @length 1..55 @pattern ^[\w-]+:[0-9a-f-]+$, Logins required LoginsMap @length 0..10, TokenDuration @range 1..86400; output Token targets OIDCToken, @sensitive) and presents that same Token back as the Logins value keyed "cognito-identity.amazonaws.com" to GetCredentialsForIdentity (modeled errors include NotAuthorizedException, errorKind client, @httpError 403). While the token is inside its TokenDuration window (plus AWS's documented clock-skew grace, "usually no more than 5 minutes") AWS answers 200 with Credentials. Once the window has elapsed AWS refuses with HTTP 403 NotAuthorizedException and the message "Invalid login token. Token is expired." The model does not state the message text, so the message is recorded behavior only: it is entirely FIXED text — no identity id, pool id, provider name, duration or timestamp is interpolated into it — and it is distinct from "Invalid login token. Not a Cognito token.", which AWS reserves for a Logins value that is not a Cognito-issued OpenID token at all (wrong issuer / not a pool-minted token). In other words the two refusals share the code and status and differ only by that trailing sentence, which is how a caller tells "re-mint, your token aged out" from "you handed me something that is not a Cognito token".

Emulator: the pre-expiry legs of this scenario now behave correctly — a TokenDuration=86400 token vends credentials for the pool's authenticated role, GetCallerIdentity under them resolves to an assumed-role ARN, and a TokenDuration=1 token re-issues the same IdentityId and is accepted immediately after minting. After the 1 s window (and the grace buffer) elapses, GetCredentialsForIdentity for that same identity/token does refuse with code NotAuthorizedException, but the message is "Invalid login token. Not a Cognito token." The check asserts the expiry wording and fails: AssertionError: expected message 'Invalid login token. Token is expired.', got 'Invalid login token. Not a Cognito token.'

Why: a client relying on AWS's behavior branches on this message to distinguish a recoverable staleness from a configuration/issuer error. "Token is expired." tells an SDK or application layer to call GetOpenIdTokenForDeveloperIdentity again and retry; "Not a Cognito token." tells it the login value is of the wrong kind, which no refresh can repair — so credential-refresh logic, tests of token-TTL behavior, and operator diagnostics all draw the wrong conclusion from an ordinary token expiry. It also makes the genuinely-not-a-Cognito-token refusal indistinguishable from the ordinary expiry case, since one message now covers both.

Fix: the Logins token validation stage of the GetCredentialsForIdentity handler (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/get_credentials_for_identity.rs, the vend() path that validates each Logins entry before role resolution, together with the login-token classification helpers it calls in crate::state). The classification must check, in order, whether the presented value IS a token this service minted for this identity/pool and only then whether its recorded validity window (TokenDuration + clock-skew grace) has elapsed, emitting the expiry message for the second case while keeping the not-a-Cognito-token message for a value that is not pool-issued at all.

Related open issues that do NOT cover this (the replay report records neither as owning this check): 20261125T230334Z-cognito-identity-kms-s3-reopens-getcredentialsforidentity-refuses-a-just-minted-developer-identity.md is the opposite case — a VALID, unexpired pool-issued token wrongly refused with this same fixed sentence (here the valid token IS accepted and only the expired one is mis-worded); 20261120T185720Z-cognito-identity-login-conflict-and-login-token-refusals.md covers that sentence MISSING where it belongs on a genuinely non-Cognito token. Fixed issue 20261110T092801Z-cognito-identity-a-just-minted-developer-identity-openid.md covered a just-minted TokenDuration=1 token being refused as expired with no clock-skew grace; that leg now passes, so this is not a reopening of it — the call is refused only after the window really has elapsed, and the defect is the wording of that refusal.

## Evidence
Behavioral suite replay against the emulator built from this checkout: 313/341 passed. Failing check cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry [mismatch].

Expected (AWS): after a token minted with TokenDuration=1 has outlived its window plus the documented clock-skew grace, GetCredentialsForIdentity(IdentityId=<the identity>, Logins={"cognito-identity.amazonaws.com": <that token>}) -> HTTP 403 NotAuthorizedException with message "Invalid login token. Token is expired."

Actual (emulator): HTTP 403 NotAuthorizedException with message "Invalid login token. Not a Cognito token."

Replay output:
  File "check_cognito_identity_get_open_id_token_for_developer_identity_max_duration_token_remains_valid_until_expiry.py", line 123, in run
    assert message == expected, f"expected message {expected!r}, got {message!r}"
  AssertionError: expected message 'Invalid login token. Token is expired.', got 'Invalid login token. Not a Cognito token.'

All earlier assertions of the check passed, so the divergence is isolated to the refusal wording: the TokenDuration=86400 token vended a complete credential set with a future Expiration and an assumed-role GetCallerIdentity ARN, and the TokenDuration=1 token re-issued the same IdentityId and vended credentials before expiry. The error CODE (NotAuthorizedException) and status (403) match AWS.

Message classification: both strings are wholly FIXED text — "Invalid login token. Token is expired." and "Invalid login token. Not a Cognito token." interpolate nothing from the request or from state (no identity id, pool id, developer provider name, TokenDuration value or timestamp). Nothing in this evidence is a per-run value, so the divergence is the choice of fixed sentence, not any variable part.

## Steps to Reproduce
1. CreateIdentityPool(IdentityPoolName=<name>, AllowUnauthenticatedIdentities=False, DeveloperProviderName=<devProvider>).
2. Create an IAM role whose trust policy allows sts:AssumeRoleWithWebIdentity for Federated principal cognito-identity.amazonaws.com with StringEquals cognito-identity.amazonaws.com:aud = the pool id and ForAnyValue:StringLike cognito-identity.amazonaws.com:amr = authenticated; bind it with SetIdentityPoolRoles(Roles={"authenticated": <roleArn>}).
3. GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<devProvider>: <userId>}, TokenDuration=86400); confirm GetCredentialsForIdentity with that token answers 200 (it does today).
4. GetOpenIdTokenForDeveloperIdentity(IdentityPoolId=<pool>, Logins={<devProvider>: <same userId>}, TokenDuration=1); it returns the same IdentityId and a new Token. Confirm GetCredentialsForIdentity with it answers 200 immediately (it does today).
5. Wait past the 1 s duration plus the clock-skew grace window, polling GetCredentialsForIdentity(IdentityId=<that IdentityId>, Logins={"cognito-identity.amazonaws.com": <the TokenDuration=1 token>}) until it is refused.
6. AWS refuses 403 NotAuthorizedException "Invalid login token. Token is expired."; the emulator refuses 403 NotAuthorizedException "Invalid login token. Not a Cognito token."
7. Re-replay to confirm: behavioral check cognito-identity.get-open-id-token-for-developer-identity-max-duration-token-remains-valid-until-expiry.
