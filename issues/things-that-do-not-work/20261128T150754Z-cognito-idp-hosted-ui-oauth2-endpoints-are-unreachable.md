Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.hosted-ui-authorize-redirects-to-its-login-page-with-the-request-parameters cognito-idp.hosted-ui-authorize-with-an-unlisted-redirect-uri-rejected cognito-idp.hosted-ui-logout-redirects-to-the-sign-out-url-and-ends-the-session cognito-idp.hosted-ui-sign-in-redirects-to-the-callback-with-a-single-use-code cognito-idp.oauth2-client-credentials-token-carries-resource-server-scopes cognito-idp.oauth2-pkce-authorization-code-second-exchange-rejected cognito-idp.oauth2-token-revoked-refresh-token-rejected cognito-idp.oauth2-token-undefined-resource-server-scope-rejected cognito-idp.oauth2-token-wrong-client-secret-rejected cognito-idp.openid-configuration-of-a-pool-with-a-prefix-domain-names-its-hosted-endpoints cognito-idp.user-password-auth-id-and-access-tokens-split-identity-and-scope-claims-but-share-sub-and-iss cognito-idp.the-id-and-access-tokens-of-one-sign-in-share-an-event-id cognito-idp.admin-list-user-auth-events-lists-newest-sign-in-first cognito-idp.oauth2-token-deleted-client-secret-rejected-while-the-added-secret-is-granted cognito-idp.access-token-generation-applies-only-once-the-pre-token-trigger-is-v2 cognito-idp.v1-pre-token-trigger-object-claim-value-rejected-until-the-trigger-is-v2 cognito-idp.v2-pre-token-trigger-sets-access-token-aud-only-to-the-app-client-id cognito-idp.v1-pre-token-trigger-overrides-of-sub-token-use-and-cognito-claims-are-ignored cognito-idp.pre-token-trigger-empty-group-override-suppresses-group-claims-and-null-keeps-them cognito-idp.pre-token-trigger-groups-to-override-returned-as-a-string-rejected cognito-idp.pre-token-trigger-preferred-role-returned-as-a-list-rejected cognito-idp.pre-token-trigger-receives-client-metadata-only-from-respond-to-auth-challenge
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.hosted-ui-authorize-redirects-to-its-login-page-with-the-request-parameters|send#12|EgressRefused|with self._opener.open(req, timeout=30) as resp: # customer-built AWS host
Issue-Key: cognito-idp.hosted-ui-authorize-with-an-unlisted-redirect-uri-rejected|send#14|EgressRefused|with self._opener.open(req, timeout=30) as resp: # customer-built AWS host
Issue-Key: cognito-idp.hosted-ui-logout-redirects-to-the-sign-out-url-and-ends-the-session|send#14|EgressRefused|with self._opener.open(req, timeout=30) as resp: # customer-built AWS host
Issue-Key: cognito-idp.hosted-ui-sign-in-redirects-to-the-callback-with-a-single-use-code|send#16|EgressRefused|with self._opener.open(req, timeout=30) as resp: # customer-built AWS host
Issue-Key: cognito-idp.oauth2-client-credentials-token-carries-resource-server-scopes|_token#15|EgressRefused|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: cognito-idp.oauth2-pkce-authorization-code-second-exchange-rejected|go#16|EgressRefused|with self.opener.open(req, timeout=30) as resp:
Issue-Key: cognito-idp.oauth2-token-revoked-refresh-token-rejected|_form#16|EgressRefused|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: cognito-idp.oauth2-token-undefined-resource-server-scope-rejected|_token#17|EgressRefused|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: cognito-idp.oauth2-token-wrong-client-secret-rejected|_token#15|EgressRefused|with urllib.request.urlopen(req, timeout=30) as resp:
Issue-Key: cognito-idp.openid-configuration-of-a-pool-with-a-prefix-domain-names-its-hosted-endpoints|run#19|AssertionError|assert paths == HOSTED_PATHS, f"the hosted endpoints' paths were {paths!r}"
Issue-Key: cognito-idp.user-password-auth-id-and-access-tokens-split-identity-and-scope-claims-but-share-sub-and-iss|run#25|AssertionError|assert issuer_labels == want_labels, (
Issue-Key: cognito-idp.the-id-and-access-tokens-of-one-sign-in-share-an-event-id|run#13|AssertionError|assert "event_id" in id_claims, f"the ID token has no event_id claim: {sorted(id_claims)!r}"
Issue-Key: cognito-idp.admin-list-user-auth-events-lists-newest-sign-in-first|run#12|KeyError|first_event = _claims(first["IdToken"])["event_id"]
Issue-Key: cognito-idp.oauth2-token-deleted-client-secret-rejected-while-the-added-secret-is-granted|run#35|AssertionError|assert (status, body) == (400, _REFUSED), (
Issue-Key: cognito-idp.access-token-generation-applies-only-once-the-pre-token-trigger-is-v2|run#8|AssertionError|assert "hue" not in v1_access and "levels" not in v1_access, (
Issue-Key: cognito-idp.v1-pre-token-trigger-object-claim-value-rejected-until-the-trigger-is-v2|run#15|AssertionError|assert number_id.get("shade") == "7", (
Issue-Key: cognito-idp.v2-pre-token-trigger-sets-access-token-aud-only-to-the-app-client-id|run#7|AssertionError|assert own.get("aud") == client_id, (
Issue-Key: cognito-idp.v1-pre-token-trigger-overrides-of-sub-token-use-and-cognito-claims-are-ignored|run#16|AssertionError|assert "cognito:tier" not in id_claims, (
Issue-Key: cognito-idp.pre-token-trigger-empty-group-override-suppresses-group-claims-and-null-keeps-them|run#12|AssertionError|assert id_kept.get("cognito:groups") == ["leads"], (
Issue-Key: cognito-idp.pre-token-trigger-groups-to-override-returned-as-a-string-rejected|_refusal#12|TypeError|raise AssertionError(f"expected the sign-in to be refused, but it returned {sorted(result)!r}")
Issue-Key: cognito-idp.pre-token-trigger-preferred-role-returned-as-a-list-rejected|_refusal#12|TypeError|raise AssertionError(f"expected the sign-in to be refused, but it returned {sorted(result)!r}")
Issue-Key: cognito-idp.pre-token-trigger-receives-client-metadata-only-from-respond-to-auth-challenge|run#17|AssertionError|assert _seen(by_password) == "null", (
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp: hosted UI/OAuth2 endpoints are unreachable, token claims are wrong, and the pre-token trigger contract diverges (10 causes)

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolDomain

## Description
Ten defects in the hosted-UI/OAuth2 surface, the claims the emulator mints, and the PreTokenGeneration trigger contract.

Cause 1 of 10: the hosted UI/OAuth2 endpoints are announced on a host the emulator does not serve.
AWS: a prefix domain is served at https://<domain>.auth.<region>.amazoncognito.com and the /oauth2/authorize, /oauth2/token, /logout and /oauth2/userInfo endpoints answer there.
Emulator: returns that amazoncognito.com host to the client while serving only its own endpoint, so every hosted-UI/OAuth2 call is refused before it leaves the runner ("egress refused: '<domain>.auth.us-west-2.amazoncognito.com' is not the endpoint under test"). The emulator announces an address other than the one it is reached on.
Why: the whole hosted-UI and OAuth2 token surface is unusable — authorize, the code exchange, PKCE replay refusal, client_credentials, revoked-refresh-token and wrong-secret refusals can none of them be reached.
Fix: the domain/endpoint URLs the CreateUserPoolDomain/DescribeUserPoolDomain state and the hosted-UI router announce must resolve to the endpoint the emulator serves. Related open issue 20261125T015700Z-cognito-idp-reopens-the-pool-s-hosted-ui-oauth2-domain.md owns three other hosted-UI checks; these are not marked owned.
Traces: cognito-idp.hosted-ui-authorize-redirects-to-its-login-page-with-the-request-parameters, cognito-idp.hosted-ui-authorize-with-an-unlisted-redirect-uri-rejected, cognito-idp.hosted-ui-logout-redirects-to-the-sign-out-url-and-ends-the-session, cognito-idp.hosted-ui-sign-in-redirects-to-the-callback-with-a-single-use-code, cognito-idp.oauth2-client-credentials-token-carries-resource-server-scopes, cognito-idp.oauth2-pkce-authorization-code-second-exchange-rejected, cognito-idp.oauth2-token-revoked-refresh-token-rejected, cognito-idp.oauth2-token-undefined-resource-server-scope-rejected, cognito-idp.oauth2-token-wrong-client-secret-rejected.

Cause 2 of 10: the OIDC discovery document gives path-prefixed endpoints.
AWS: /.well-known/openid-configuration of a pool with a prefix domain names authorization_endpoint '.../oauth2/authorize', token_endpoint '.../oauth2/token', userinfo_endpoint '.../oauth2/userInfo', revocation_endpoint '.../oauth2/revoke' and end_session_endpoint '.../logout' on the domain host — the path never carries the domain name.
Emulator: serves paths '/<domain>/oauth2/authorize', '/<domain>/oauth2/token', '/<domain>/oauth2/userInfo', '/<domain>/oauth2/revoke', '/<domain>/logout'.
Why: an OIDC client that follows discovery builds URLs no AWS deployment would use.
Fix: the discovery-document serializer's endpoint construction.
Traces: cognito-idp.openid-configuration-of-a-pool-with-a-prefix-domain-names-its-hosted-endpoints.

Cause 3 of 10: minted tokens carry a host-based issuer.
AWS: the iss claim of the ID and access token is https://cognito-idp.<region>.<domain>/<pool id>.
Emulator: issues iss 'https://127.0.0.1:<port>/<pool id>', whose first labels are the address it is reached on.
Why: any verifier that pins the issuer (JWKS discovery, API Gateway authorizers, Verified Permissions identity sources) rejects the token.
Fix: the token minting/issuer construction. Related open issues 20261122T054242Z-cognito-idp-3-net-new-causes-id-token.md and 20261111T061507Z-cognito-idp-3-net-new-causes-the-id.md own other issuer checks; this check is not marked owned.
Traces: cognito-idp.user-password-auth-id-and-access-tokens-split-identity-and-scope-claims-but-share-sub-and-iss.

Cause 4 of 10: there is no event_id claim and no auth-event id.
AWS: the ID and access token of one sign-in share an event_id claim, and the same id appears as the EventId of the AdminListUserAuthEvents record for that sign-in (newest first).
Emulator: the ID token claim set is ['aud','auth_time','cognito:username','exp','iat','iss','jti','origin_jti','sub','token_use'] — no event_id — so the auth-event correlation cannot be made (the replay fails with KeyError 'event_id').
Why: sign-in records cannot be correlated with the tokens they issued; risk/threat-protection clients lose their join key.
Fix: mint event_id into both tokens and record it as the auth event's EventId.
Traces: cognito-idp.the-id-and-access-tokens-of-one-sign-in-share-an-event-id, cognito-idp.admin-list-user-auth-events-lists-newest-sign-in-first.

Cause 5 of 10: the /oauth2/token client-authentication failure uses the wrong status and body.
AWS: a deleted/incorrect client secret is refused HTTP 400 with body {'error': 'invalid_client', 'error_description': 'invalid_client_secret'}.
Emulator: answers HTTP 401 with {'error': 'invalid_client'} and no error_description.
Why: OAuth clients branch on the 400/401 distinction and on error_description.
Fix: the token endpoint's client-authentication error response.
Traces: cognito-idp.oauth2-token-deleted-client-secret-rejected-while-the-added-secret-is-granted.

Cause 6 of 10: the PreTokenGeneration trigger version is not honoured.
AWS: a V1_0 trigger's reply may only use the V1 containers (claimsToAddOrOverride/groupOverrideDetails), and V2-only containers (accessTokenGeneration, idTokenGeneration, scopesToAdd) are ignored; a V2_0 trigger is the reverse. Under V1_0 a non-string claim value is coerced to its string form (7 becomes '7').
Emulator: applies V2-only containers on a V1_0 trigger (the access token gains claims and the ID token gains V2-only values) and keeps a number claim as a JSON number.
Why: pools pinned to V1_0 see claims AWS would never issue, so token contents depend on the emulator rather than the configured version.
Fix: the PreTokenGeneration response handling must branch on the configured LambdaVersion before applying containers, and coerce V1 claim values to strings. Related open issue 20261128T011751Z-cognito-idp-lambda-trigger-contract-diverges-clientmetadata-propagation.md owns the cognito-idp__lambda checks of this family.
Traces: cognito-idp.access-token-generation-applies-only-once-the-pre-token-trigger-is-v2, cognito-idp.v1-pre-token-trigger-object-claim-value-rejected-until-the-trigger-is-v2, cognito-idp.v2-pre-token-trigger-sets-access-token-aud-only-to-the-app-client-id.

Cause 7 of 10: reserved claims from the trigger reach the token.
AWS: a trigger cannot add or override sub, token_use or any cognito:-prefixed claim; such entries are dropped. A V2_0 trigger CAN set the access token's aud to the app client id.
Emulator: lets an invented cognito: claim through into the ID token, and leaves the access token's aud unset (None) when the trigger sets it.
Why: a trigger can forge identity claims that relying parties trust, and the legitimate aud override does not work.
Fix: the claim-merge step's reserved-claim filter and the access-token aud handling.
Traces: cognito-idp.v1-pre-token-trigger-overrides-of-sub-token-use-and-cognito-claims-are-ignored, cognito-idp.v2-pre-token-trigger-sets-access-token-aud-only-to-the-app-client-id.

Cause 8 of 10: a null groupOverrideDetails is treated as an empty override.
AWS: groupOverrideDetails = null leaves the user's cognito:groups claim untouched; only an explicitly empty override suppresses the group claims.
Emulator: drops cognito:groups (None) for the null reply.
Why: triggers that return null for the container silently lose group claims, changing authorisation decisions.
Fix: distinguish a null container from an empty one in the trigger-reply merge.
Traces: cognito-idp.pre-token-trigger-empty-group-override-suppresses-group-claims-and-null-keeps-them.

Cause 9 of 10: an unparseable trigger reply is accepted instead of failing the sign-in.
AWS: when the trigger returns groupsToOverride as a string, or preferredRole as a list, the sign-in is refused (an InvalidLambdaResponseException/UserLambdaValidationException class refusal) and no tokens are issued.
Emulator: completes the sign-in and returns tokens; the replay fails while formatting the unexpected success.
Why: a broken trigger silently issues tokens whose group/role claims AWS would have refused to mint.
Fix: validate the trigger reply's container types and refuse the sign-in on a mismatch.
Traces: cognito-idp.pre-token-trigger-groups-to-override-returned-as-a-string-rejected, cognito-idp.pre-token-trigger-preferred-role-returned-as-a-list-rejected.

Cause 10 of 10: ClientMetadata is passed to PreTokenGeneration from (Admin)InitiateAuth.
AWS: PreTokenGeneration receives ClientMetadata only from RespondToAuthChallenge (and the refresh-token exchange via GetTokensFromRefreshToken); from (Admin)InitiateAuth the trigger sees null.
Emulator: forwards the InitiateAuth ClientMetadata to the trigger.
Why: triggers that key behaviour on the presence of metadata behave differently than on AWS. Related open issue 20261128T011751Z-cognito-idp-lambda-trigger-contract-diverges-clientmetadata-propagation.md covers the cognito-idp__lambda variants.
Fix: the ClientMetadata propagation in the (Admin)InitiateAuth token-generation path.
Traces: cognito-idp.pre-token-trigger-receives-client-metadata-only-from-respond-to-auth-challenge.

## Evidence
Cause 1 — "Error: CapabilityNotOffered: the target serves no address for sim2real-hui-authz-222f9b4398ed.auth.us-west-2.amazoncognito.com ... simparity.egress.EgressRefused: egress refused: ... is not the endpoint under test (127.0.0.1) ... the emulator announces an address other than the endpoint it is reached on" (same for the 8 other hosted-UI/OAuth2 checks, each with its own domain prefix).
Cause 2 — "the hosted endpoints' paths were {'authorization_endpoint': '/sim2real-oidc-domain-0ba38cb50047/oauth2/authorize', 'token_endpoint': '/sim2real-oidc-domain-0ba38cb50047/oauth2/token', 'userinfo_endpoint': '/.../oauth2/userInfo', 'revocation_endpoint': '/.../oauth2/revoke', 'end_session_endpoint': '/.../logout'}".
Cause 3 — "IdToken iss 'https://127.0.0.1:40361/us-west-2_072df2805' has labels ('https', ['127','0'], '/us-west-2_072df2805'), expected ('https', ['cognito-idp','us-west-2'], '/us-west-2_072df2805')".
Cause 4 — "the ID token has no event_id claim: ['aud','auth_time','cognito:username','exp','iat','iss','jti','origin_jti','sub','token_use']"; AdminListUserAuthEvents replay raised KeyError: 'event_id'.
Cause 5 — "the deleted secret was answered HTTP 401 with {'error': 'invalid_client'}, expected 400 {'error': 'invalid_client', 'error_description': 'invalid_client_secret'}".
Cause 6 — "V1_0 trigger: access token gained 'red'/[1, 'two', True]"; "V1_0 number claim: ID token shade is 7, expected the string '7'"; "aud set to the app client ID: access token aud is None (client ID has 26 chars)".
Cause 7 — "a cognito: claim the trigger invented reached the ID token: 'forged-by-trigger'".
Cause 8 — "a null groupOverrideDetails must keep cognito:groups ['leads'], got None".
Cause 9 — the sign-in returned a result instead of a ClientError (replay raised TypeError while rendering "expected the sign-in to be refused, but it returned ...").
Cause 10 — "after password AdminInitiateAuth the trigger saw '{\"stage\": \"initiate\"}', expected 'null'".

## Steps to Reproduce
1. Replay the check ids in trace_ids.
2. Cause 1/2: CreateUserPoolDomain with a prefix domain, then GET /.well-known/openid-configuration and follow authorization_endpoint / token_endpoint.
3. Cause 3/4: InitiateAuth USER_PASSWORD_AUTH and decode the ID and access tokens (check iss and event_id), then AdminListUserAuthEvents.
4. Cause 5: POST /oauth2/token with grant_type=refresh_token and a deleted client secret.
5. Cause 6-9: attach a PreTokenGeneration trigger with LambdaVersion V1_0 returning V2-only containers, a number claim, a cognito:-prefixed claim, groupOverrideDetails=null, groupsToOverride as a string and preferredRole as a list; sign in after each.
6. Cause 10: AdminInitiateAuth with ClientMetadata and inspect the trigger event.
