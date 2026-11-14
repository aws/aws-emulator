Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.developer-user-already-registered-rejected cognito-identity.get-open-id-token-for-developer-identity-conflicting-identity-rejected cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-conflict cognito-identity.set-identity-pool-roles-invalid-role-arn-rejected cognito-identity.set-identity-pool-roles-rejects-cross-account-role cognito-identity.set-identity-pool-roles-concurrent-modification-error-path cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access cognito-identity.role-mapping-ambiguous-resolution-deny-blocks-credentials cognito-identity.vended-credentials-bounded-by-role-policy cognito-identity.create-identity-pool-limit-exceeded
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.developer-user-already-registered-rejected|run#13|AssertionError|assert err == ("NotAuthorizedException", "Logins don't match. Please include at least one valid login for this identity or identity pool."), f"got {err!r}"
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-conflicting-identity-rejected|run#11|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-conflict|run#9|AssertionError|assert code == "NotAuthorizedException", (
Issue-Key: cognito-identity.set-identity-pool-roles-invalid-role-arn-rejected|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-identity.set-identity-pool-roles-rejects-cross-account-role|run#9|AssertionError|assert code == "AccessDeniedException", (
Issue-Key: cognito-identity.set-identity-pool-roles-concurrent-modification-error-path|run#11|AssertionError|assert err == expected, (err, expected)
Issue-Key: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected|run#29|AssertionError|assert creds is None, (
Issue-Key: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access|run#25|AssertionError|assert issuer == f"https://{provider}", (
Issue-Key: cognito-identity.role-mapping-ambiguous-resolution-deny-blocks-credentials|run#26|AssertionError|assert recheck["Credentials"]["AccessKeyId"].startswith("ASIA"), (
Issue-Key: cognito-identity.vended-credentials-bounded-by-role-policy|run#24|AssertionError|assert cp_msg == expected, (
Issue-Key: cognito-identity.create-identity-pool-limit-exceeded|_discover_providers_per_pool_quota#2|AssertionError|raise AssertionError(
Issue-Commit: 36cbc41e18f8f7f03eaee998bf950ce282c69b6c

# cognito-identity: 9 net-new root causes — developer-identity link conflicts answer ResourceConflictException, SetIdentityPoolRoles refusals are re-coded/re-worded, a role without sts:TagSession still vends, vended keys are not ASIA keys, and a denial names identitypool/*

> _Note: this issue was identified against the code base at commit `36cbc41e18f8f7f03eaee998bf950ce282c69b6c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOpenIdTokenForDeveloperIdentity

## Description
Nine net-new root causes in the cognito-identity service. Each must be closed before submitting.

=== Cause 1 of 9: a developer-identity link conflict is refused as ResourceConflictException "Cannot merge these identities." instead of NotAuthorizedException ===
AWS: Smithy: GetOpenIdTokenForDeveloperIdentity models both NotAuthorizedException (403) and ResourceConflictException (409). Recorded behavior: when the request names an IdentityId together with a developer-provider user that is already linked to a DIFFERENT identity (or re-links an already-registered developer user to another identity, or supplies a developer login that does not belong to the named identity), AWS refuses with code NotAuthorizedException and message "Logins don't match. Please include at least one valid login for this identity or identity pool." The fixed text of that message is the whole sentence; it carries no values from the request. ResourceConflictException is AWS's code for a merge/link conflict on DeleteIdentities/MergeDeveloperIdentities-style state, not for this ownership mismatch.
Emulator: all three recordings answer code ResourceConflictException with message "Cannot merge these identities."
Why: a client branching on NotAuthorizedException (re-authenticate / drop the stale IdentityId) sees a 409 conflict instead, and retry logic built for merge conflicts loops on a request that can never succeed.
Fix: the GetOpenIdTokenForDeveloperIdentity handler's developer-identity ownership check — classify the ownership mismatch as NotAuthorizedException with the logins-mismatch message rather than reusing the merge-conflict branch. Already-open 20261113T131245Z-cognito-identity-developer-identity-ownership-refusals-are-mis.md and 20261030T212445Z-cognito-identity-developer-identity-token-and-credential-refusals.md describe neighbouring symptoms but do not carry these three checks.
Traces: cognito-identity.developer-user-already-registered-rejected, cognito-identity.get-open-id-token-for-developer-identity-conflicting-identity-rejected, cognito-identity.get-open-id-token-for-developer-identity-duplicate-registration-conflict.

=== Cause 2 of 9: SetIdentityPoolRoles with a value that is not a role ARN returns the cross-account message ===
AWS: Smithy: SetIdentityPoolRoles models NotAuthorizedException (403). Recorded behavior: a Roles value that is not an IAM role ARN at all is refused with NotAuthorizedException and message "Access to Role '<the supplied value>' is forbidden." — fixed text "Access to Role '" + "' is forbidden.", with the quoted part being the exact string supplied in the Roles map (here a long garbage string), and nothing is persisted.
Emulator: the code is right but the message is "Cross-account pass role is not allowed." — a different error class's text.
Why: the message is the only thing distinguishing "your ARN is malformed" from "your ARN belongs to another account"; a client (or an operator reading logs) is pointed at the wrong fix.
Fix: SetIdentityPoolRoles role-ARN validation — emit the forbidden-role message, interpolating the rejected value, for a value that is not a parseable same-account role ARN.
Traces: cognito-identity.set-identity-pool-roles-invalid-role-arn-rejected.

=== Cause 3 of 9: SetIdentityPoolRoles with a cross-account role ARN is refused with the wrong error code ===
AWS: recorded behavior (SetIdentityPoolRoles's modeled error list does not include it, so this is a service-common error): a well-formed role ARN whose account differs from the caller's is refused with code AccessDeniedException and message "Cross-account pass role is not allowed." (fixed text, no variable parts), and the previously stored Roles are left untouched.
Emulator: code NotAuthorizedException (the message matched).
Why: a client that distinguishes AccessDeniedException (a pass-role/trust problem it must fix in IAM) from NotAuthorizedException (a Cognito authorization problem) takes the wrong remediation path; error-code-driven retry/alerting classifies the failure wrongly.
Fix: SetIdentityPoolRoles role-ARN validation — classify the cross-account ARN as AccessDeniedException while keeping this exact message, and keep it distinct from Cause 2's branch.
Traces: cognito-identity.set-identity-pool-roles-rejects-cross-account-role.

=== Cause 4 of 9: ConcurrentModificationException on SetIdentityPoolRoles carries the wrong message ===
AWS: Smithy: SetIdentityPoolRoles models ConcurrentModificationException (400). Recorded behavior: overlapping SetIdentityPoolRoles calls on one pool refuse the losers with ConcurrentModificationException and message "Only one request to update resource type Identity Pool can be processed at a time" — fixed text naming the resource type being updated; no request values appear in it.
Emulator: the code is right, the message is "There are parallel requests to modify this resource."
Why: the message is a requirement of the error class; clients and operators that match on AWS's wording (and tests asserting it) treat the emulator's refusal as a different failure.
Fix: the SetIdentityPoolRoles concurrency guard's error construction.
Traces: cognito-identity.set-identity-pool-roles-concurrent-modification-error-path.

=== Cause 5 of 9: a principal-tag-mapped pool vends through a role whose trust policy does not allow sts:TagSession ===
AWS: Smithy: GetCredentialsForIdentity models InvalidIdentityPoolConfigurationException (400). Recorded behavior: when the pool has a principal-tag attribute map for the login provider, Cognito must pass session tags to STS; a mapped role whose trust policy allows only sts:AssumeRoleWithWebIdentity (no sts:TagSession) makes the vend fail with code InvalidIdentityPoolConfigurationException and message "Invalid identity pool configuration. Check assigned IAM roles for this pool." (fixed text, no variable parts) — repeatably, not transiently — while an otherwise identical pool with NO principal-tag map vends normally through the same trust shape. Adding sts:TagSession repairs the vend for the same identity.
Emulator: the mapped pool returns credentials (an AccessKeyId was handed back) for the same identity and login, with no refusal.
Why: this is over-permissive. The emulator hands out a session AWS would refuse, so a deployment missing sts:TagSession in its trust policy — exactly the misconfiguration that silently drops the tenant session tag and breaks aws:PrincipalTag-based isolation — passes locally and fails (or worse, leaks across tenants) in production.
Fix: GetCredentialsForIdentity's role-assumption path — when a principal-tag attribute map applies to the login's provider, require the resolved role's trust policy to allow sts:TagSession and refuse with InvalidIdentityPoolConfigurationException and the message above when it does not.
Traces: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-without-tag-session-rejected.

=== Cause 6 of 9: the user-pool ID token's iss claim is the emulator's own listener instead of the Cognito issuer URL ===
AWS: recorded behavior: the ID token minted for a user pool carries iss = "https://cognito-idp.<region>.amazonaws.com/<userPoolId>" — i.e. exactly "https://" + the identity pool's configured ProviderName for that user pool. The host portion is the canonical Cognito issuer, not the address the API was reached on, and an identity pool matches a login key against it.
Emulator: iss is the bare emulator listener with the pool id appended (observed: 'https://127.0.0.1:39133/us-west-2_c2854718b' where ProviderName was 'cognito-idp.us-west-2.amazonaws.com/us-west-2_c2854718b'). The host:port is per-run and is not the requirement; the requirement is that iss equals "https://" + ProviderName, whatever host the request arrived on.
Why: every consumer that validates iss — the identity pool's own Logins key matching, OIDC/JWT verifiers, API Gateway Cognito authorizers — rejects or mis-routes the token, so the federation chain cannot be exercised.
Fix: the user-pool token minting used by the auth operations (AdminInitiateAuth/InitiateAuth) — derive the iss claim from the user pool's region and pool id (the ProviderName form), not from the request's Host/listener address. Already-open 20261111T060814Z-cognito-identity-cognito-idp-the-id-token-s.md reports the same claim under the cognito-identity__cognito-idp heading; it does not carry this check.
Traces: cognito-identity.deploy-cognito-identity-3-cfn-principal-tag-governs-tenant-access.

=== Cause 7 of 9: GetCredentialsForIdentity vends an 'EMULATOR…' AccessKeyId instead of an STS temporary 'ASIA…' key ===
AWS: recorded behavior: the Credentials of a GetCredentialsForIdentity response are STS temporary credentials, so AccessKeyId begins with the 'ASIA' prefix that marks a temporary key (fixed prefix; the remaining characters are per-session). Smithy: GetCredentialsForIdentity.Credentials carries AccessKeyId/SecretKey/SessionToken/Expiration.
Emulator: the AccessKeyId is of the form 'EMULATOR<random>' (observed 'EMULATORASMP4NL5FMEN', and again 'EMULATORASU6SW5ILCEQ' in the Cause 5 recording), so the check that a mapped login still vends after a denial fails.
Why: callers and SDK tooling key off the key-id prefix to tell a temporary session from a long-lived user key (credential-type detection, log redaction, policy conditions); a non-ASIA key id makes a vended session indistinguishable from an IAM user key.
Fix: the credential-minting component behind GetCredentialsForIdentity (its STS session construction). Already-open issues report this prefix for other flows — 20261026T234020Z-cognito-identity-4-net-new-root-causes-web.md, 20261028T221339Z-cognito-identity-3-net-new-getcredentialsforidentity-root-causes.md, 20261029T071507Z-cognito-identity-getcredentialsforidentity-under-a-token-role-mapping.md — none of them carries this check's replay key.
Traces: cognito-identity.role-mapping-ambiguous-resolution-deny-blocks-credentials.

=== Cause 8 of 9: the AccessDenied message for a resource-less control-plane action names identitypool/* instead of identitypool/ ===
AWS: recorded behavior: a vended session calling cognito-identity:ListIdentityPools with no identity-based grant is refused with AccessDeniedException whose message is "User: <assumed-role arn>/CognitoIdentityCredentials is not authorized to perform: cognito-identity:ListIdentityPools on resource: arn:<partition>:cognito-identity:<region>:<account>:identitypool/ because no identity-based policy allows the cognito-identity:ListIdentityPools action". The fixed text is "User: … is not authorized to perform: … on resource: … because no identity-based policy allows the … action"; the variable parts are the caller's assumed-role ARN with session name, the action, and the resource ARN — and for this list action the ARN's resource segment ends at "identitypool/" with an EMPTY id, no wildcard.
Emulator: the same message but with resource "…:identitypool/*".
Why: an error message is a requirement of the refusal; a client (or a policy-debugging tool) parsing the denied resource out of the message sees a wildcard ARN that was never evaluated, and the wording no longer matches what AWS produces.
Fix: the IAM authorization denial-message renderer for cognito-identity actions — render the resource ARN of a pool-less control-plane action with an empty pool-id segment rather than substituting '*'.
Traces: cognito-identity.vended-credentials-bounded-by-role-policy.

=== Cause 9 of 9: the cognito-identity quotas are absent from Service Quotas, so the providers-per-pool limit cannot be discovered ===
AWS: recorded behavior: Service Quotas serves the cognito-identity quota catalog, so a client can look up the user-pool-providers-per-identity-pool quota for ServiceCode 'cognito-identity' and then observe CreateIdentityPool/UpdateIdentityPool refuse a pool exceeding it with LimitExceededException.
Emulator: no such quota is served for ServiceCode 'cognito-identity' ("could not discover the cognito-identity user-pool-providers-per-identity-pool quota via Service Quotas"), so the limit behavior of CreateIdentityPool is unreachable.
Why: a client that reads its limits from Service Quotas before provisioning sees cognito-identity as having no quotas at all, and the limit refusal CreateIdentityPool must produce is never exercised.
Fix: the Service Quotas default-quota catalog (its cognito-identity entries), and CreateIdentityPool's provider-count limit enforcement behind it. Already-open 20261111T131231Z-service-quotas-listawsdefaultservicequotas-default-catalog-incomplete-for-cognito-identity-cognito.md reports the same catalog gap; it does not carry this check's replay key.
Traces: cognito-identity.create-identity-pool-limit-exceeded.

## Evidence
Cause 1 — expected ('NotAuthorizedException', "Logins don't match. Please include at least one valid login for this identity or identity pool."); got ('ResourceConflictException', 'Cannot merge these identities.') in all three recordings.
Cause 2 — expected message "Access to Role 'not-a-real-arn-just-a-long-garbage-string-1234567890' is forbidden." (the quoted value is the string the request supplied); got 'Cross-account pass role is not allowed.' (code NotAuthorizedException was correct).
Cause 3 — expected code AccessDeniedException with message "Cross-account pass role is not allowed."; got code NotAuthorizedException.
Cause 4 — expected ('ConcurrentModificationException', 'Only one request to update resource type Identity Pool can be processed at a time'); got ('ConcurrentModificationException', 'There are parallel requests to modify this resource.').
Cause 5 — expected the mapped pool to refuse with ('InvalidIdentityPoolConfigurationException', 'Invalid identity pool configuration. Check assigned IAM roles for this pool.') on every attempt; got credentials (AccessKeyId 'EMULATORASU6SW5ILCEQ' in the run), while the control pool with no principal-tag map vended as expected.
Cause 6 — expected iss == "https://" + ProviderName, i.e. https://cognito-idp.us-west-2.amazonaws.com/<userPoolId>; got 'https://127.0.0.1:39133/<userPoolId>' (the listener host:port is per-run; the divergence is that iss is not the ProviderName issuer form).
Cause 7 — expected Credentials.AccessKeyId to start with 'ASIA'; got 'EMULATORASMP4NL5FMEN'.
Cause 8 — expected '… on resource: arn:aws:cognito-identity:<region>:<account>:identitypool/ because no identity-based policy allows the cognito-identity:ListIdentityPools action'; got the same message with 'identitypool/*'.
Cause 9 — expected a discoverable user-pool-providers-per-identity-pool quota for ServiceCode='cognito-identity'; got none (AssertionError: could not discover the cognito-identity user-pool-providers-per-identity-pool quota via Service Quotas).

## Steps to Reproduce
Cause 1: CreateIdentityPool; GetOpenIdTokenForDeveloperIdentity with Logins={<developer provider>: <user>} to create identity A; then call it again with IdentityId=<a different identity> and the same developer user (and the already-registered / re-link variants) -> expect NotAuthorizedException "Logins don't match. …", observe ResourceConflictException "Cannot merge these identities.".
Cause 2: CreateIdentityPool, then SetIdentityPoolRoles(Roles={'authenticated': 'not-a-real-arn-just-a-long-garbage-string-1234567890'}) -> expect NotAuthorizedException "Access to Role '<value>' is forbidden."; GetIdentityPoolRoles must still show no roles.
Cause 3: CreateIdentityPool, SetIdentityPoolRoles with a same-account role ARN (accepted), then with 'arn:aws:iam::111122223333:role/<name>' -> expect AccessDeniedException "Cross-account pass role is not allowed." and unchanged stored Roles.
Cause 4: CreateIdentityPool, then fire 10 concurrent SetIdentityPoolRoles calls with distinct authenticated role ARNs -> every loser must be ConcurrentModificationException "Only one request to update resource type Identity Pool can be processed at a time".
Cause 5: deploy a user pool + app client + two identity pools (one with an IdentityPoolPrincipalTag map {tenant: custom:tenant}, one without), each with a role trusted only for sts:AssumeRoleWithWebIdentity (no sts:TagSession); AdminInitiateAuth for a user, GetId + GetCredentialsForIdentity on both pools -> the unmapped pool vends, the mapped pool must refuse with InvalidIdentityPoolConfigurationException on repeated attempts.
Cause 6: AdminInitiateAuth (ADMIN_USER_PASSWORD_AUTH) and decode the ID token's iss claim -> it must equal "https://" + the user pool's ProviderName.
Cause 7: CreateIdentityPool with a Rules role mapping, vend via GetCredentialsForIdentity and inspect Credentials.AccessKeyId -> must begin with 'ASIA'.
Cause 8: vend credentials for an identity whose role has no permission policy, then call cognito-identity:ListIdentityPools with them and read the AccessDeniedException message's resource ARN.
Cause 9: call Service Quotas for ServiceCode='cognito-identity' and look up the user-pool-providers-per-identity-pool quota, then exceed it with CreateIdentityPool/UpdateIdentityPool.
