Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims
Issue-Key: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims|run#31|AssertionError|assert SUPPRESSED_CLAIM not in claims, (
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# cognito-idp__lambda: a V2 PreTokenGeneration trigger's claimsToSuppress is ignored — the suppressed claim stays in the issued access token

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth (pre-token-generation V2 trigger, access-token claim overrides)

## Description
Cause 1 of 1: the emulator applies only part of a V2 pre-token-generation response — additions and scopes land, suppressions do not.
AWS: a user pool with a `PreTokenGenerationConfig` whose LambdaVersion is V2_0 invokes the trigger before minting tokens and applies the function's `response.claimsAndScopeOverrideDetails.accessTokenGeneration` to the ACCESS token: `claimsToAddOrOverride` adds/overwrites the listed claims, `scopesToAdd` adds the listed scopes, and `claimsToSuppress` REMOVES the listed claims from the token entirely — including pool-generated claims such as `cognito:groups`. Documented in the Cognito developer guide, 'Pre token generation Lambda trigger' (V2_0); the cognito-idp Smithy model declares the trigger configuration but not the claim arithmetic, so the suppression rule is documented behavior rather than model text.
Emulator: the added claim is present (`custom_tier`) and the access token is the right token, but a claim named in `claimsToSuppress` is still in the decoded token. Observed with `cognito:groups` suppressed and still present in the claim set `['auth_time', 'client_id', 'cognito:groups', 'custom_tier', 'exp', 'iat', 'iss', 'jti', 'origin_jti', 'scope', 'sub', 'token_use', 'username']`. The check first proves the baseline token (no trigger attached) does carry the claim, so its later presence is attributable to the ignored suppression and not to the user's group membership.
Why: claimsToSuppress is a data-minimisation and authorization control — a client uses it to keep group membership or other sensitive attributes out of a token that is forwarded to third parties or to a resource server. The emulator hands out a token carrying exactly the claims the trigger removed, so a downstream authorizer that keys on `cognito:groups` grants access the token was supposed to withhold.
Fix: the cognito-idp token minting path's V2 pre-token-generation handling — after applying claimsToAddOrOverride and scopesToAdd it must also delete every claim named in claimsToSuppress (pool-generated claims included) before signing the access token.
Traces: cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims.

## Evidence
cognito-idp__lambda.pre-token-generation-v2-trigger-adds-suppresses-and-scopes-access-token-claims — AssertionError: the suppressed claim is still present even though the trigger removed it: ['auth_time', 'client_id', 'cognito:groups', 'custom_tier', 'exp', 'iat', 'iss', 'jti', 'origin_jti', 'scope', 'sub', 'token_use', 'username'].
Expected per AWS: with response.claimsAndScopeOverrideDetails.accessTokenGeneration = {claimsToAddOrOverride: {custom_tier: gold}, claimsToSuppress: ["cognito:groups"], scopesToAdd: ["<scope>"]}, the issued access token contains custom_tier and the added scope and does NOT contain cognito:groups.

## Steps to Reproduce
1. Create a user pool, a group, and a user who is a member of that group; confirm the baseline access token carries cognito:groups.
2. Deploy a Lambda whose handler sets event["response"]["claimsAndScopeOverrideDetails"] = {"accessTokenGeneration": {"claimsToAddOrOverride": {"custom_tier": "gold"}, "claimsToSuppress": ["cognito:groups"], "scopesToAdd": ["sim2real/read"]}}.
3. Attach it as the pool's PreTokenGeneration trigger with LambdaVersion V2_0 and grant cognito-idp invoke permission.
4. Authenticate the user and decode the access token: AWS omits cognito:groups; the emulator still includes it.
