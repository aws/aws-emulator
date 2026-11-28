Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.admin-confirm-sign-up-post-confirmation-timeout-rejected cognito-idp__lambda.refresh-token-exchange-pre-token-trigger-timeout-rejected cognito-idp__lambda.pre-sign-up-trigger-at-zero-reserved-concurrency-rejected-until-released cognito-idp__lambda.admin-initiate-auth-client-metadata-reaches-pre-authentication-but-not-pre-token-generation cognito-idp__lambda.initiate-auth-client-metadata-is-withheld-from-pre-token-generation cognito-idp__lambda.refresh-token-client-metadata-reaches-pre-token-generation-only-through-get-tokens cognito-idp__lambda.case-insensitive-pool-lowercases-username-and-preferred-username-in-get-user-and-trigger-input cognito-idp__lambda.claims-override-details-container-applies-on-v1-events-and-is-ignored-on-v2 cognito-idp__lambda.id-token-generation-container-applies-on-v2-events-and-is-ignored-on-v1 cognito-idp__lambda.get-user-rejected-once-v2-reply-suppresses-admin-scope-and-allowed-on-v1 cognito-idp__lambda.pre-token-claim-with-cognito-prefix-is-left-out-while-the-unprefixed-claim-is-issued cognito-idp__lambda.trigger-reply-cannot-add-or-override-cognito-prefixed-claims-but-adds-unprefixed-one cognito-idp__lambda.pre-token-suppressing-groups-also-drops-the-role-claims cognito-idp__lambda.pre-token-v2-scope-to-add-containing-a-space-is-dropped-from-the-access-token cognito-idp__lambda.post-authentication-error-rejected-without-admin-initiate-auth-client-metadata cognito-idp__lambda.pre-sign-up-auto-verify-email-without-email-rejected
Issue-Key: cognito-idp__lambda.admin-confirm-sign-up-post-confirmation-timeout-rejected|run#17|AssertionError|assert got == ("UnexpectedLambdaException",
Issue-Key: cognito-idp__lambda.refresh-token-exchange-pre-token-trigger-timeout-rejected|run#18|AssertionError|assert got == ("UnexpectedLambdaException",
Issue-Key: cognito-idp__lambda.pre-sign-up-trigger-at-zero-reserved-concurrency-rejected-until-released|run#21|AssertionError|assert error["Message"] == (
Issue-Key: cognito-idp__lambda.admin-initiate-auth-client-metadata-reaches-pre-authentication-but-not-pre-token-generation|run#22|AssertionError|assert claims.get("probe_meta") == json.dumps("absent"), (
Issue-Key: cognito-idp__lambda.initiate-auth-client-metadata-is-withheld-from-pre-token-generation|run#29|AssertionError|assert first.get("meta_seen") == "{}", (
Issue-Key: cognito-idp__lambda.refresh-token-client-metadata-reaches-pre-token-generation-only-through-get-tokens|run#38|AssertionError|assert via_initiate.get("meta_seen") == "{}", (
Issue-Key: cognito-idp__lambda.case-insensitive-pool-lowercases-username-and-preferred-username-in-get-user-and-trigger-input|run#9|AssertionError|assert view == folded, (
Issue-Key: cognito-idp__lambda.claims-override-details-container-applies-on-v1-events-and-is-ignored-on-v2|run#8|AssertionError|assert "region_code" not in v2_claims, (
Issue-Key: cognito-idp__lambda.id-token-generation-container-applies-on-v2-events-and-is-ignored-on-v1|run#8|AssertionError|assert "plan_label" not in v1_claims, (
Issue-Key: cognito-idp__lambda.get-user-rejected-once-v2-reply-suppresses-admin-scope-and-allowed-on-v1|run#5|AssertionError|assert _ADMIN_SCOPE in v1_scopes, f"V1_0 access token scopes are {v1_scopes!r}"
Issue-Key: cognito-idp__lambda.pre-token-claim-with-cognito-prefix-is-left-out-while-the-unprefixed-claim-is-issued|run#31|AssertionError|assert _RESERVED_CLAIM not in claims, (
Issue-Key: cognito-idp__lambda.trigger-reply-cannot-add-or-override-cognito-prefixed-claims-but-adds-unprefixed-one|run#6|AssertionError|assert "cognito:plan_label" not in claims, (
Issue-Key: cognito-idp__lambda.pre-token-suppressing-groups-also-drops-the-role-claims|run#39|AssertionError|assert leftover == [], (
Issue-Key: cognito-idp__lambda.pre-token-v2-scope-to-add-containing-a-space-is-dropped-from-the-access-token|run#32|AssertionError|assert leaked == [], f"the blank-space scope reached the scope claim as {leaked!r} in {blank!r}"
Issue-Key: cognito-idp__lambda.post-authentication-error-rejected-without-admin-initiate-auth-client-metadata|_refused#18|AssertionError|raise AssertionError(f"the sign-in was expected to be refused but returned {answer!r}")
Issue-Key: cognito-idp__lambda.pre-sign-up-auto-verify-email-without-email-rejected|_code_of#17|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# cognito-idp: Lambda trigger contract diverges — ClientMetadata propagation, pre-token claim/scope rules, trigger-failure errors and PreSignUp/PostAuthentication outcomes (10 causes)

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Ten net-new root causes in the Cognito user-pool Lambda-trigger contract. Open issue 20261031T170200Z-cognito-idp-lambda-reopens-cognito-idp-lambda-preauthentication-pretokengeneration-triggers-every.md covers one check with the same trigger-timeout symptom but carries none of the replay keys below.

Cause 1 of 10: a trigger invocation that fails is surfaced with the wrong error code and message.
AWS: when a user-pool trigger cannot be invoked or overruns its budget, Cognito answers 400 with the error class for that condition and a message naming the trigger and the underlying reason. Smithy: the trigger-failure errors modelled on these operations are UnexpectedLambdaException, InvalidLambdaResponseException and UserLambdaValidationException (all httpError 400); the recorded answers for a PostConfirmation timeout, a PreTokenGeneration timeout and a PreSignUp invocation throttled by zero reserved concurrency are NOT the `<Trigger> failed with error Socket timeout while invoking Lambda function.` / `<Trigger> failed with error Rate Exceeded.` pair the emulator emits.
Emulator: answers `('UnexpectedLambdaException', 'PostConfirmation failed with error Socket timeout while invoking Lambda function.')`, `('UnexpectedLambdaException', 'PreTokenGeneration failed with error Socket timeout while invoking Lambda function.')` and `'PreSignUp failed with error Rate Exceeded.'` for all three, so a timeout, a throttle and an invocation refusal are indistinguishable.
Why: clients branch on the error code to decide whether to retry (throttle) or to fail the sign-up (validation); one code and one sentence for every cause collapses that decision.
Fix: trigger-invocation failure classification in the Cognito trigger dispatcher shared by the AdminConfirmSignUp, SignUp and token-exchange handlers.
Traces: cognito-idp__lambda.admin-confirm-sign-up-post-confirmation-timeout-rejected, cognito-idp__lambda.refresh-token-exchange-pre-token-trigger-timeout-rejected, cognito-idp__lambda.pre-sign-up-trigger-at-zero-reserved-concurrency-rejected-until-released.

Cause 2 of 10: ClientMetadata is passed to PreTokenGeneration from operations that must withhold it.
AWS: ClientMetadata supplied to AdminInitiateAuth, InitiateAuth and the REFRESH_TOKEN exchange reaches PreAuthentication but is NOT passed to the PreTokenGeneration trigger; PreTokenGeneration only receives ClientMetadata through the GetTokensFromRefreshToken path.
Emulator: forwards it — the PreTokenGeneration trigger received `{"ticket": "...", "verdict": "allow"}`, `{"flow-marker": "m-..."}` and `{"channel": "initiate-auth", "refresh-marker": "r-..."}` on those three flows.
Why: a trigger written for AWS treats the presence of ClientMetadata as proof of the GetTokens path and mints claims it must not mint, so the emulator issues tokens AWS would not.
Fix: the ClientMetadata plumbing into the PreTokenGeneration trigger event in the AdminInitiateAuth, InitiateAuth and RespondToAuthChallenge/refresh handlers.
Traces: cognito-idp__lambda.admin-initiate-auth-client-metadata-reaches-pre-authentication-but-not-pre-token-generation, cognito-idp__lambda.initiate-auth-client-metadata-is-withheld-from-pre-token-generation, cognito-idp__lambda.refresh-token-client-metadata-reaches-pre-token-generation-only-through-get-tokens.

Cause 3 of 10: a case-insensitive pool does not lowercase preferred_username.
AWS: in a pool created with CaseSensitive=false, the username and the preferred_username alias are stored and returned lowercased, while other attributes keep their case.
Emulator: AdminGetUser returns `'preferred_username': 'MiXeD-Nick-<id>'` where AWS returns the lowercased form, and the same unlowercased value reaches the trigger input.
Why: alias lookups and trigger logic that compare preferred_username against a lowercased input stop matching.
Fix: alias normalisation for case-insensitive pools in the user-store write/read path that AdminGetUser, GetUser and the trigger event builder share.
Traces: cognito-idp__lambda.case-insensitive-pool-lowercases-username-and-preferred-username-in-get-user-and-trigger-input.

Cause 4 of 10: the version-specific claims containers of the PreTokenGeneration reply are applied regardless of the trigger event version.
AWS: `claimsOverrideDetails` is honoured only for V1_0 trigger events and ignored for V2_0; `claimsAndScopeOverrideDetails.idTokenGeneration` is honoured only for V2_0 and ignored for V1_0.
Emulator: applies both on both — the V2_0 ID token carried `region_code='north'` from the V1 container, and the V1_0 ID token carried `plan_label='gold'` from the V2-only container.
Why: a client that keeps both containers in one handler (the documented migration pattern) gets claims injected twice and tokens that differ from production.
Fix: event-version gating of the reply containers in the PreTokenGeneration reply processor.
Traces: cognito-idp__lambda.claims-override-details-container-applies-on-v1-events-and-is-ignored-on-v2, cognito-idp__lambda.id-token-generation-container-applies-on-v2-events-and-is-ignored-on-v1.

Cause 5 of 10: the access token issued on a V1_0 trigger event carries no scopes.
AWS: the access token minted through a V1_0 PreTokenGeneration event still carries its scopes (including `aws.cognito.signin.user.admin`), so GetUser is allowed; only a V2_0 reply that suppresses that scope causes GetUser to be refused.
Emulator: the V1_0 access token's scope claim is empty (`V1_0 access token scopes are []`).
Why: every scope-based authorisation against a V1_0 pool fails, and the V1/V2 difference the client is testing disappears.
Fix: the access-token scope claim construction in the token issuer for V1_0 trigger events.
Traces: cognito-idp__lambda.get-user-rejected-once-v2-reply-suppresses-admin-scope-and-allowed-on-v1.

Cause 6 of 10: a trigger reply can add or override `cognito:`-prefixed claims.
AWS: claims whose names begin with `cognito:` are reserved — a PreTokenGeneration reply that adds or overrides one is silently dropped, while an unprefixed claim from the same reply is issued.
Emulator: issues them — the ID token carried `cognito:tier = 'gold'` and a `cognito:`-prefixed claim added by the trigger reply appeared as `'gold'`.
Why: a trigger can forge reserved identity claims (`cognito:groups`, `cognito:username`, role claims), so an authoriser trusting them is bypassable on the emulator and not on AWS.
Fix: reserved-prefix filtering in the PreTokenGeneration reply processor before claims reach the token issuer.
Traces: cognito-idp__lambda.pre-token-claim-with-cognito-prefix-is-left-out-while-the-unprefixed-claim-is-issued, cognito-idp__lambda.trigger-reply-cannot-add-or-override-cognito-prefixed-claims-but-adds-unprefixed-one.

Cause 7 of 10: suppressing `cognito:groups` does not drop the role claims.
AWS: when a PreTokenGeneration reply suppresses `cognito:groups`, Cognito also drops `cognito:preferred_role` and `cognito:roles` from the ID token.
Emulator: the ID token still carries `['cognito:preferred_role', 'cognito:roles']`.
Why: a client suppressing group membership still leaks the role mapping it meant to hide.
Fix: the claim-suppression rule in the PreTokenGeneration reply processor — suppressing groups must cascade to the role claims.
Traces: cognito-idp__lambda.pre-token-suppressing-groups-also-drops-the-role-claims.

Cause 8 of 10: a V2 `scopesToAdd` entry containing a space is not dropped.
AWS: a scope value containing whitespace is invalid and is dropped from the access token's `scope` claim rather than split.
Emulator: splits it — the blank-space scope reached the claim as `['inventory', 'write']`, giving a final scope list of `['aws.cognito.signin.user.admin', 'inventory', 'write', 'inventory.read']`.
Why: a malformed scope silently becomes two valid-looking scopes, so a resource server grants access the client never requested.
Fix: scope validation in the PreTokenGeneration V2 reply processor (`claimsAndScopeOverrideDetails.accessTokenGeneration.scopesToAdd`).
Traces: cognito-idp__lambda.pre-token-v2-scope-to-add-containing-a-space-is-dropped-from-the-access-token.

Cause 9 of 10: a PostAuthentication trigger error does not fail the sign-in.
AWS: when the PostAuthentication trigger raises, AdminInitiateAuth is refused and no tokens are issued.
Emulator: answers 200 with a full AuthenticationResult (AccessToken, RefreshToken, ExpiresIn 3600, TokenType Bearer).
Why: a pool using PostAuthentication as a last authorisation gate issues tokens to a principal the trigger rejected — an authorisation bypass that does not exist on AWS.
Fix: the PostAuthentication trigger-outcome handling in the AdminInitiateAuth/InitiateAuth handlers — a trigger error must abort token issuance.
Traces: cognito-idp__lambda.post-authentication-error-rejected-without-admin-initiate-auth-client-metadata.

Cause 10 of 10: PreSignUp may auto-verify an email the user did not supply.
AWS: a PreSignUp reply setting `autoVerifyEmail` for a sign-up that carries no email attribute is refused — SignUp fails and no user is created.
Emulator: answers 200 with `{'UserConfirmed': True, 'UserSub': '<uuid>'}`.
Why: a confirmed user exists with a verified-email flag and no email address, so later password-reset and MFA flows have no destination and the pool's invariants are broken.
Fix: the PreSignUp reply validation in the SignUp handler.
Traces: cognito-idp__lambda.pre-sign-up-auto-verify-email-without-email-rejected.

## Evidence
Cause 1: `a post confirmation trigger past the limit should fail AdminConfirmSignUp, got ('UnexpectedLambdaException', 'PostConfirmation failed with error Socket timeout while invoking Lambda function.')`; `a pre token generation trigger that overruns the limit should fail the exchange, got ('UnexpectedLambdaException', 'PreTokenGeneration failed with error Socket timeout while invoking Lambda function.')`; `the refusal message was 'PreSignUp failed with error Rate Exceeded.'`.
Cause 2: `pre token generation received clientMetadata '{"ticket": "...", "verdict": "allow"}'; AdminInitiateAuth does not pass ClientMetadata to that trigger`; `AdminInitiateAuth ClientMetadata reached the trigger as '{"flow-marker": "m-..."}'`; `... as '{"channel": "initiate-auth", "refresh-marker": "r-..."}'`.
Cause 3: `admin_get_user: got {... 'preferred_username': 'MiXeD-Nick-<id>'}, expected {... 'preferred_username': 'mixed-nick-<id>' ...}`.
Cause 4: `V2_0 ID token carries region_code='north' from the V1 container`; `V1_0 ID token carries plan_label='gold' from a V2-only container`.
Cause 5: `V1_0 access token scopes are []`.
Cause 6: `a cognito:-prefixed claim cannot be added, yet the ID token carries 'cognito:tier' = 'gold'`; `a cognito:-prefixed claim was added: 'gold'`.
Cause 7: `suppressing cognito:groups must also drop both role claims, yet the ID token still carries ['cognito:preferred_role', 'cognito:roles']`.
Cause 8: `the blank-space scope reached the scope claim as ['inventory', 'write'] in ['aws.cognito.signin.user.admin', 'inventory', 'write', 'inventory.read']`.
Cause 9: `the sign-in was expected to be refused but returned {'ChallengeParameters': {}, 'AuthenticationResult': {'AccessToken': '<jwt>', 'ExpiresIn': 3600, 'TokenType': 'Bearer', 'RefreshToken': '<jwe>' ...}}`.
Cause 10: `the call was expected to be refused but returned {'UserConfirmed': True, 'UserSub': '<uuid>', ...}`.

## Steps to Reproduce
Replay the 16 check ids in trace_ids. Minimal repros: (1) attach a PostConfirmation trigger that sleeps past the budget and call AdminConfirmSignUp; set the PreSignUp function's reserved concurrency to 0 and call SignUp — compare error codes/messages; (2) call AdminInitiateAuth / InitiateAuth / the REFRESH_TOKEN exchange with `--client-metadata` and log the PreTokenGeneration event; (3) create a pool with `UsernameConfiguration CaseSensitive=false`, sign a user up with a mixed-case preferred_username and AdminGetUser; (4) return `claimsOverrideDetails` on a V2_0 event and `claimsAndScopeOverrideDetails.idTokenGeneration` on a V1_0 event and decode both tokens; (5) decode the access token minted on a V1_0 event and inspect `scope`; (6) return `claimsToAddOrOverride={'cognito:tier':'gold'}`; (7) return `claimsToSuppress=['cognito:groups']` and decode the ID token; (8) return `scopesToAdd=['inventory write']`; (9) make PostAuthentication raise and call AdminInitiateAuth; (10) return `autoVerifyEmail=true` from PreSignUp for a SignUp with no email attribute.
