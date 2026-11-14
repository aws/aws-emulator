Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.cognito-jwt-claims-verifiable cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned cognito-idp.cognito-groups-claim-ordered-by-precedence cognito-idp.id-token-claims-derive-from-the-user-attribute-set cognito-idp.revoke-token-kills-only-the-origin-jti-family-of-that-refresh-token cognito-idp.deep-cognito-idp-3-enforce-app-client-default-attr-perms cognito-idp.deep-cognito-idp-3-enforce-app-client-read-attrs-withheld cognito-idp.deep-cognito-idp-3-enforce-app-client-write-attrs-rejected cognito-idp.deep-cognito-idp-2-token-rotation-grace-period cognito-idp.deep-cognito-idp-2-token-rotation-grace-replay-supersedes-rotated-token cognito-idp.deep-cognito-idp-2-token-rotation-invalid-config-rejected cognito-idp.deep-cognito-idp-2-token-rotation-legacy-refresh-flow-rejected cognito-idp.deep-cognito-idp-2-token-rotation-reuse-exception cognito-idp.token-validity-out-of-range-fails cognito-idp.refresh-token-expires-after-validity-period
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.cognito-jwt-claims-verifiable|run#13|AssertionError|assert header.get("alg") == "RS256", f"{label} header alg expected RS256, got {header.get('alg')}"
Issue-Key: cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned|_jwk_for_kid#22|HTTPError|with urllib.request.urlopen(url, timeout=30) as r: # noqa: S310 - fixed https/http scheme
Issue-Key: cognito-idp.cognito-groups-claim-ordered-by-precedence|run#17|AssertionError|assert groups == expected_order, (
Issue-Key: cognito-idp.id-token-claims-derive-from-the-user-attribute-set|run#18|AssertionError|assert missing == [], (
Issue-Key: cognito-idp.revoke-token-kills-only-the-origin-jti-family-of-that-refresh-token|run#23|AssertionError|assert claims_one_refreshed.get("origin_jti") == claims_one["origin_jti"], (
Issue-Key: cognito-idp.deep-cognito-idp-3-enforce-app-client-default-attr-perms|run#22|AssertionError|assert "name" not in narrowed and "custom:tier" not in narrowed and "email" in narrowed, (
Issue-Key: cognito-idp.deep-cognito-idp-3-enforce-app-client-read-attrs-withheld|run#20|AssertionError|assert "name" not in r_user, (
Issue-Key: cognito-idp.deep-cognito-idp-3-enforce-app-client-write-attrs-rejected|_error#13|_UnexpectedSuccess|raise _UnexpectedSuccess(f"call expected to be refused SUCCEEDED: {resp!r}")
Issue-Key: cognito-idp.deep-cognito-idp-2-token-rotation-grace-period|run#36|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.deep-cognito-idp-2-token-rotation-grace-replay-supersedes-rotated-token|_error#19|AssertionError|raise AssertionError(f"call expected to fail SUCCEEDED (response keys {keys!r})")
Issue-Key: cognito-idp.deep-cognito-idp-2-token-rotation-invalid-config-rejected|_error#6|AssertionError|raise AssertionError("call expected to be refused SUCCEEDED")
Issue-Key: cognito-idp.deep-cognito-idp-2-token-rotation-legacy-refresh-flow-rejected|run#12|AssertionError|assert code == "UnsupportedOperationException", (
Issue-Key: cognito-idp.deep-cognito-idp-2-token-rotation-reuse-exception|run#21|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.token-validity-out-of-range-fails|_client_error#9|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: cognito-idp.refresh-token-expires-after-validity-period|_rejection#14|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: issued tokens diverge — ES256 instead of RS256, no JWKS document, missing/unordered claims, Read/WriteAttributes unenforced, refresh-token rotation and validity limits wrong (8 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Eight root causes in token issuance and token-scoped authorization. Cause 6 lets a client read attributes it was not granted, so the bundle is HIGH. The open issue 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md covers the cognito-idp__lambda replay keys; causes 1 and 2 below are the cognito-idp-only checks it does not carry.

Cause 1 of 8 — tokens are signed with ES256.
AWS: Cognito signs ID and access tokens with RS256; the JWS header is {alg:'RS256', kid:<key id>, typ? }, and the kid resolves in the pool's JWKS.
Emulator: the header is {'alg':'ES256','kid':...,'typ':'JWT'}.
Why: any verifier configured for RS256 (every published Cognito verification snippet, aws-jwt-verify, API Gateway JWT authorizers) rejects every token the emulator issues.
Fix: the token signer / pool key material.
Traces: cognito-idp.cognito-jwt-claims-verifiable.

Cause 2 of 8 — the pool's JWKS document is not served.
AWS: GET https://cognito-idp.<region>.amazonaws.com/<poolId>/.well-known/jwks.json returns 200 with the pool's keys.
Emulator: HTTP 404 Not Found.
Why: a client cannot verify any token, and a forged token cannot be shown to be unsigned.
Fix: the pool's well-known JWKS route.
Traces: cognito-idp.forged-access-tokens-are-refused-and-verifiably-unsigned.

Cause 3 of 8 — cognito:groups is not ordered by Precedence.
AWS: the ID token's cognito:groups claim lists the user's groups ordered by ascending Precedence.
Emulator: the claim is in membership-insertion order.
Fix: group-claim assembly in the token builder.
Traces: cognito-idp.cognito-groups-claim-ordered-by-precedence.

Cause 4 of 8 — ID token omits stored user attributes.
AWS: every stored user attribute appears as an ID-token claim of the same name (including custom: attributes and email_verified).
Emulator: the claim set is only ['aud','auth_time','cognito:username','email','exp','iat','iss','jti','origin_jti','sub','token_use'] — 'custom:level', 'custom:tier', 'email_verified' and 'given_name' were stored but absent.
Fix: ID-token claim assembly from the user's attribute set.
Traces: cognito-idp.id-token-claims-derive-from-the-user-attribute-set.

Cause 5 of 8 — origin_jti is not preserved across REFRESH_TOKEN_AUTH.
AWS: tokens minted by REFRESH_TOKEN_AUTH keep the origin_jti of the family, which is what makes RevokeToken kill exactly that family.
Emulator: a new origin_jti is generated on refresh.
Why: revocation no longer reaches the refreshed tokens.
Fix: carry origin_jti through the refresh path.
Traces: cognito-idp.revoke-token-kills-only-the-origin-jti-family-of-that-refresh-token.

Cause 6 of 8 — app-client ReadAttributes/WriteAttributes are not enforced.
AWS: GetUser through a client whose ReadAttributes is ['email'] returns only email (plus sub); UpdateUserAttributes naming an attribute outside WriteAttributes is refused.
Emulator: GetUser returns the full attribute set (['custom:tier','email','given_name','name','sub']) and UpdateUserAttributes succeeds with 200.
Why: a client scoped to a narrow attribute set can read and write everything.
Fix: apply the app client's ReadAttributes/WriteAttributes in GetUser and UpdateUserAttributes.
Traces: cognito-idp.deep-cognito-idp-3-enforce-app-client-default-attr-perms, cognito-idp.deep-cognito-idp-3-enforce-app-client-read-attrs-withheld, cognito-idp.deep-cognito-idp-3-enforce-app-client-write-attrs-rejected.

Cause 7 of 8 — refresh-token rotation semantics.
AWS (Smithy: UpdateUserPoolClient/CreateUserPoolClient take RefreshTokenRotation; UnsupportedOperationException and InvalidParameterException are the refusal classes): a replayed rotated refresh token outside the grace period is refused with the fixed message 'Refresh token reuse detected'; a replay inside the grace period must not supersede the rotated token; an invalid RefreshTokenRotation configuration is refused at Create/UpdateUserPoolClient; legacy REFRESH_TOKEN_AUTH on a rotating client is UnsupportedOperationException.
Emulator: the reuse message is 'Refresh token has been invalidated by refresh token rotation.'; the grace-period replay SUCCEEDS (returned AccessToken/IdToken/RefreshToken); the invalid rotation configuration is accepted; and legacy REFRESH_TOKEN_AUTH answers InvalidParameterException 'REFRESH_TOKEN_AUTH flow not enabled for this client'.
Fix: the RefreshTokenRotation state machine and its validation.
Traces: cognito-idp.deep-cognito-idp-2-token-rotation-grace-period, cognito-idp.deep-cognito-idp-2-token-rotation-grace-replay-supersedes-rotated-token, cognito-idp.deep-cognito-idp-2-token-rotation-invalid-config-rejected, cognito-idp.deep-cognito-idp-2-token-rotation-legacy-refresh-flow-rejected, cognito-idp.deep-cognito-idp-2-token-rotation-reuse-exception.

Cause 8 of 8 — token-validity ranges are not validated.
AWS: Smithy AccessTokenValidity @range min 1 max 86400 and IdTokenValidity likewise, interpreted together with TokenValidityUnits — AccessTokenValidity=25 with units 'hours' exceeds the 24h ceiling and is refused with InvalidParameterException; a RefreshTokenValidity below the 60-minute floor is refused too.
Emulator: both clients are created successfully (AccessTokenValidity=25/hours; RefreshTokenValidity=59/minutes).
Why: a pool is left issuing tokens with lifetimes AWS cannot produce.
Fix: Create/UpdateUserPoolClient token-validity validation against TokenValidityUnits.
Traces: cognito-idp.token-validity-out-of-range-fails, cognito-idp.refresh-token-expires-after-validity-period.

## Evidence
Verbatim from the replay:
- 'IdToken header alg expected RS256, got ES256'
- urllib.error.HTTPError: HTTP Error 404: Not Found (fetching the pool's JWKS)
- 'IdToken cognito:groups must be ordered by ascending Precedence [lo, mid, hi] (1, 5, 10), got [hi, lo, mid]'
- "['custom:level','custom:tier','email_verified','given_name'] are stored but absent from the claims ['aud','auth_time','cognito:username','email','exp','iat','iss','jti','origin_jti','sub','token_use']"
- "REFRESH_TOKEN_AUTH must preserve origin_jti; expected '257a0545-...', got '133a9bac-...'"
- "GetUser through the ReadAttributes=['email'] client leaked `name`" ; 'call expected to be refused SUCCEEDED: {CodeDeliveryDetailsList: [], HTTPStatusCode: 200}'
- "expected message 'Refresh token reuse detected', got 'Refresh token has been invalidated by refresh token rotation.'" ; 'call expected to fail SUCCEEDED (response keys [AccessToken, ExpiresIn, IdToken, RefreshToken, TokenType])' ; "REFRESH_TOKEN_AUTH on a rotating client returned 'InvalidParameterException' ('REFRESH_TOKEN_AUTH flow not enabled for this client'), expected UnsupportedOperationException"
- 'real AWS rejects this call, but it SUCCEEDED and returned {... AccessTokenValidity: 25, TokenValidityUnits: {AccessToken: hours} ...}'

## Steps to Reproduce
1. AdminInitiateAuth and base64-decode the IdToken header -> expect alg RS256.
2. GET /<poolId>/.well-known/jwks.json against the emulator endpoint -> expect 200 with keys.
3. Create three groups with Precedence 1/5/10, add one user to all three, sign in -> check cognito:groups ordering.
4. Store custom: attributes + given_name, sign in -> check ID-token claims.
5. CreateUserPoolClient(ReadAttributes=['email']) and GetUser with its token -> expect only email+sub.
6. CreateUserPoolClient(AccessTokenValidity=25, TokenValidityUnits={'AccessToken':'hours'}) -> expect InvalidParameterException.
