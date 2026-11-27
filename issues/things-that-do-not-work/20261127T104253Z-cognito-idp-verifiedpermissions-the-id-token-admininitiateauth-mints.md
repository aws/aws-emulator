Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__verifiedpermissions.identity-source-naming-a-deleted-user-pool-rejected
Issue-Key: cognito-idp__verifiedpermissions.identity-source-naming-a-deleted-user-pool-rejected|run#24|AssertionError|assert cognito["issuer"] == issuer, (
Issue-Commit: 7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08

# cognito-idp__verifiedpermissions: the ID token AdminInitiateAuth mints carries iss = the emulator's bare listener authority (https://127.0.0.1:<port>/<poolId>), so it can never equal the cognito-idp.<region>.<domain>/<poolId> issuer GetIdentitySource stores for the same pool

> _Note: this issue was identified against the code base at commit `7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Cause 1 of 1: the `iss` claim of the ID token a user pool mints is the emulator's own serving authority (scheme+host+port of the endpoint under test) with the pool id as the path, instead of the pool's token issuer `https://cognito-idp.<region>.<domain>/<userPoolId>`. The divergence is visible as a self-inconsistency inside one emulator: Verified Permissions, asked for the identity source it just created for that very pool, reports the canonical issuer, while the pool's token names a different issuer, so the two cannot be joined. Failing check id for this cause: cognito-idp__verifiedpermissions.identity-source-naming-a-deleted-user-pool-rejected.

AWS: a user pool's token issuer is `https://cognito-idp.<region>.<domain>/<userPoolId>`, and that string is the `iss` claim of every ID and access token the pool issues. Smithy (verifiedpermissions model) CreateIdentitySource/GetIdentitySource expose CognitoUserPoolConfigurationDetail$issuer, documented as the issuer URL of the associated Amazon Cognito user pool — a value Verified Permissions DERIVES from the userPoolArn, i.e. the same issuer the pool stamps into its tokens. Smithy (cognito-idp model) AdminInitiateAuth returns AuthenticationResult{IdToken, AccessToken, RefreshToken}; the claim set inside the JWS is recorded behavior only — the recording shows an ID token whose `iss` host is `cognito-idp.<region>.<domain>` with path `/<userPoolId>`, equal to the issuer the identity source stores. No HTTP status or error code is involved: both calls succeed (HTTP 200) and the minted claim is wrong.

Emulator: AdminInitiateAuth succeeds and mints an ID token whose `iss` is the emulator's listener authority with the pool id appended — observed iss = 'https://127.0.0.1:37719/us-west-2_55e4baff6' — while GetIdentitySource for the identity source created from the same userPoolArn reports issuer = 'https://cognito-idp.us-west-2.amazonaws.com/us-west-2_55e4baff6'. The requirement violated is the FORM of the authority, not the per-run host/port: the first host label must be `cognito-idp`, the second the request's region, followed by at least one further non-empty label, and the path must be exactly `/<userPoolId>`. The observed authority carries no `cognito-idp` label and no region label at all, so nothing positional can be matched, and it disagrees with the issuer the emulator's own Verified Permissions state holds.

Why: `iss` is what identifies the pool to every token consumer. A client verifying a Cognito token derives `{iss}/.well-known/openid-configuration` and `{iss}/.well-known/jwks.json` from it, a Cognito Identity pool's Logins map is keyed by `cognito-idp.<region>.<domain>/<poolId>`, and Verified Permissions resolves a presented token to an identity source by matching the token's issuer against the stored issuer. With the bare endpoint authority in `iss`, any correct client builds the wrong discovery URLs and the wrong Logins key, and issuer-based resolution between the pool and its Verified Permissions identity source cannot be reproduced — the two sides of the same emulator state disagree about the pool's identity.

Fix: the token-issuance path of the cognito-idp crate — the component that computes the `iss` claim for the ID/access tokens on the AdminInitiateAuth (and InitiateAuth) response — must publish the pool's issuer in the `cognito-idp.<region>.<domain>/<userPoolId>` form, on an address the emulator actually serves, instead of echoing the listener authority the request arrived on; and it must agree with the issuer the verifiedpermissions identity-source state derives from the userPoolArn.

Related open issues that do NOT carry this check's key (and so do not cover it): 20261125T095227Z-cognito-identity-cognito-idp-the-id-token-admininitiateauth.md, 20261125T015700Z-cognito-idp-reopens-the-token-issuer-the-emulator-stamps-into.md, 20261122T054242Z-cognito-idp-3-net-new-causes-id-token.md and 20261111T061507Z-cognito-idp-3-net-new-causes-the-id.md. This filing records the divergence under the cognito-idp__verifiedpermissions heading, where it is the token issuer disagreeing with the issuer Verified Permissions stores for the same pool.

Traces: cognito-idp__verifiedpermissions.identity-source-naming-a-deleted-user-pool-rejected (assertion at check line 68/504, comparing GetIdentitySource's configuration.cognitoUserPoolConfiguration.issuer with the `iss` of the ID token AdminInitiateAuth returned). The earlier legs of that check pass: CreateIdentitySource naming a deleted pool is refused ValidationException 'User pool <poolId> does not exist.' and stores nothing, and the stored userPoolArn/clientIds read back correctly; the assertion stops the check before the `<poolId>|<sub>` principal-mapping leg is exercised.

## Evidence
Check cognito-idp__verifiedpermissions.identity-source-naming-a-deleted-user-pool-rejected, mismatch:
```
File "check_cognito_idp__verifiedpermissions_identity_source_naming_a_deleted_user_pool_rejected.py", line 68, in run
  assert cognito["issuer"] == issuer, (
AssertionError: stored issuer 'https://cognito-idp.us-west-2.amazonaws.com/us-west-2_55e4baff6' differs from the ID token's iss 'https://127.0.0.1:37719/us-west-2_55e4baff6'
```
Expected: the ID token returned by AdminInitiateAuth for pool <poolId> in region <region> carries iss = 'https://cognito-idp.<region>.<domain>/<poolId>' — scheme https, host label[0]='cognito-idp', label[1]=the request's region, at least one further non-empty label, path exactly '/<poolId>' — which is the same string GetIdentitySource reports as configuration.cognitoUserPoolConfiguration.issuer for the identity source built from that pool's ARN.
Actual: iss = the emulator's own serving authority with the pool id as path (scheme https, host '127.0.0.1', port = the emulator's listener port, path '/<poolId>'), while GetIdentitySource reports the canonical 'https://cognito-idp.<region>.amazonaws.com/<poolId>'. The host and port shown are per-run values and are not the requirement; the violated requirement is that the authority carries no 'cognito-idp' label and no region label, and that it does not equal the issuer the emulator itself stored for the same pool.
Both calls returned HTTP 200 — no status or error code is involved.

## Steps to Reproduce
1. cognito-idp: CreateUserPool, then CreateUserPoolClient with ExplicitAuthFlows ALLOW_ADMIN_USER_PASSWORD_AUTH and ALLOW_REFRESH_TOKEN_AUTH; AdminCreateUser(MessageAction='SUPPRESS'); AdminSetUserPassword(Permanent=True).
2. verifiedpermissions: CreatePolicyStore, then CreateIdentitySource(principalEntityType='Ledger::Member', configuration={'cognitoUserPoolConfiguration':{'userPoolArn':<the live pool's arn>,'clientIds':[<client id>]}}).
3. GetIdentitySource(policyStoreId=<store>, identitySourceId=<source>) and read configuration.cognitoUserPoolConfiguration.issuer — observed 'https://cognito-idp.<region>.amazonaws.com/<poolId>'.
4. cognito-idp: AdminInitiateAuth(AuthFlow='ADMIN_USER_PASSWORD_AUTH') and base64url-decode AuthenticationResult.IdToken's payload; read the `iss` claim — observed 'https://127.0.0.1:<port>/<poolId>'.
5. Compare: the two must be the same string. They are not, so issuer-based resolution of the token to its identity source, {iss}/.well-known/jwks.json discovery, and a cognito-identity Logins key built from the token cannot be reproduced.
