Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.create-identity-pool-invalid-name-rejected cognito-identity.create-identity-pool-rejects-invalid-name cognito-identity.identity-pool-name-invalid-symbol-rejected cognito-identity.identity-pool-id-malformed-pattern-rejected cognito-identity.describe-identity-pool-malformed-id-format-rejected cognito-identity.delete-identities-oversized-batch-rejected cognito-identity.delete-identities-too-many-requests-limit cognito-identity.get-open-id-token-for-developer-identity-rejects-bad-token-duration cognito-identity.get-open-id-token-for-developer-identity-token-duration-out-of-range-rejected cognito-identity.open-id-token-developer-identity-duration-exceeded-rejected cognito-identity.role-mapping-invalid-key-rejected cognito-identity.set-identity-pool-roles-invalid-keys-rejected cognito-identity.set-principal-tag-attribute-map-exceeds-max-mappings-rejected cognito-identity.identity-pool-name-unicode-preserved cognito-identity.identity-pool-name-unicode-round-trip cognito-identity.identity-pool-empty-vs-absent-supported-login-providers-distinguished cognito-identity.supported-login-providers-empty-vs-absent-preserved cognito-identity.set-identity-pool-roles-invalid-role-arn-rejected cognito-identity.set-identity-pool-roles-rejects-cross-account-role cognito-identity.get-identity-pool-roles-unauthorized-for-foreign-pool cognito-identity.delete-identity-pool-not-authorized-cross-account cognito-identity.set-identity-pool-roles-concurrent-modification-error-path cognito-identity.tag-resource-exceeds-max-tag-count-rejected cognito-identity.tag-resource-key-exceeding-length-rejected cognito-identity.tag-resource-rejects-reserved-aws-key cognito-identity.get-principal-tag-attribute-map-default-empty cognito-identity.delete-identity-pool-is-not-idempotent cognito-identity.update-identity-pool-concurrent-modification-detected cognito-identity.create-identity-pool-limit-exceeded
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity.create-identity-pool-invalid-name-rejected|run#11|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: cognito-identity.create-identity-pool-rejects-invalid-name|run#4|AssertionError|assert err == "ValidationException", (
Issue-Key: cognito-identity.identity-pool-name-invalid-symbol-rejected|run#5|AssertionError|assert code == "ValidationException", (
Issue-Key: cognito-identity.identity-pool-id-malformed-pattern-rejected|run#6|AssertionError|assert code == "ValidationException", f"unexpected error code: {code!r}"
Issue-Key: cognito-identity.describe-identity-pool-malformed-id-format-rejected|run#10|AssertionError|assert err == expected, (
Issue-Key: cognito-identity.delete-identities-oversized-batch-rejected|run#5|AssertionError|assert code == "ValidationException", (
Issue-Key: cognito-identity.delete-identities-too-many-requests-limit|run#4|AssertionError|assert code == "ValidationException", (
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-rejects-bad-token-duration|run#7|AssertionError|assert code == "ValidationException", (
Issue-Key: cognito-identity.get-open-id-token-for-developer-identity-token-duration-out-of-range-rejected|run#8|AssertionError|assert code == "ValidationException", f"got {code!r}: {message!r}"
Issue-Key: cognito-identity.open-id-token-developer-identity-duration-exceeded-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: cognito-identity.role-mapping-invalid-key-rejected|run#6|AssertionError|assert code == "ValidationException", f"unexpected error code: {code!r}"
Issue-Key: cognito-identity.set-identity-pool-roles-invalid-keys-rejected|run#6|AssertionError|assert err == "ValidationException", (
Issue-Key: cognito-identity.set-principal-tag-attribute-map-exceeds-max-mappings-rejected|run#10|AssertionError|assert code == "ValidationException", f"CODE={code}"
Issue-Key: cognito-identity.identity-pool-name-unicode-preserved|_invalid_tag_error#12|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.identity-pool-name-unicode-round-trip|_error#13|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.identity-pool-empty-vs-absent-supported-login-providers-distinguished|run#10|AssertionError|assert "SupportedLoginProviders" not in pool_explicit_empty, (
Issue-Key: cognito-identity.supported-login-providers-empty-vs-absent-preserved|run#11|AssertionError|assert "SupportedLoginProviders" not in described_empty, (
Issue-Key: cognito-identity.set-identity-pool-roles-invalid-role-arn-rejected|run#11|AssertionError|assert code == "NotAuthorizedException", f"expected NotAuthorizedException, got {code!r}"
Issue-Key: cognito-identity.set-identity-pool-roles-rejects-cross-account-role|_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: cognito-identity.get-identity-pool-roles-unauthorized-for-foreign-pool|_refusal#8|AssertionError|raise AssertionError(
Issue-Key: cognito-identity.delete-identity-pool-not-authorized-cross-account|run#15|AssertionError|assert restricted_identity["Arn"] != own_identity["Arn"], (
Issue-Key: cognito-identity.set-identity-pool-roles-concurrent-modification-error-path|run#12|AssertionError|assert len(errors) >= 1, (
Issue-Key: cognito-identity.tag-resource-exceeds-max-tag-count-rejected|run#10|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: cognito-identity.tag-resource-key-exceeding-length-rejected|run#10|AssertionError|assert code == "ValidationException", f"expected ValidationException for a 129-char tag key, got {code!r}"
Issue-Key: cognito-identity.tag-resource-rejects-reserved-aws-key|run#5|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: cognito-identity.get-principal-tag-attribute-map-default-empty|run#4|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: cognito-identity.delete-identity-pool-is-not-idempotent|run#9|AssertionError|assert err_update == "ResourceNotFoundException", (
Issue-Key: cognito-identity.update-identity-pool-concurrent-modification-detected|run#9|AssertionError|assert err_code == "ConcurrentModificationException", err_code
Issue-Key: cognito-identity.create-identity-pool-limit-exceeded|_discover_providers_per_pool_quota#2|AssertionError|raise AssertionError(
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# cognito-identity: input validation and error classification diverge (8 causes) — constraint violations answer InvalidParameterException instead of ValidationException, charset/quota/role/authorization checks missing

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIdentityPool

## Description
This issue bundles EIGHT distinct root causes in the cognito-identity control plane. All eight must be fixed and re-replayed before closing.

=== Cause 1 of 8: member-constraint violations are reported as the modeled InvalidParameterException instead of the protocol-level ValidationException ===
AWS: the Smithy model constrains these members — CreateIdentityPool.IdentityPoolName @length{1,128} @pattern ^[\w\s+=,.@-]+$; IdentityPoolId @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$ (DescribeIdentityPool/SetIdentityPoolRoles/...); DeleteIdentities.IdentityIdsToDelete @length{1,60}; GetOpenIdTokenForDeveloperIdentity.TokenDuration @range{1,86400}; SetPrincipalTagAttributeMap.PrincipalTags @length{0,50}; SetIdentityPoolRoles.Roles @length{0,2} / RoleMappings @length{0,10}. None of these operations model a ValidationException error — AWS's constraint-validation layer refuses the request ahead of the operation with HTTP 400, error code ValidationException, and a message built from the request, e.g. "1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+" (recorded). The code and status are the requirement; the message rule (count prefix + member name + the violated constraint, pattern rendered unanchored) is known from the request, so it is a requirement too.
Emulator: returns HTTP 400 with code InvalidParameterException and only the bare constraint clause, e.g. "Value 'this-id-has-no-region-guid-separator' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\w-]+:[0-9a-f-]+$" (anchors included, no "N validation error(s) detected" prefix).
Why: clients and SDK error handlers branch on the error code; a modeled service exception (InvalidParameterException) is handled, logged and retried differently from the framework ValidationException, so request-shape bugs are misclassified as service-logic rejections.
Fix: the shared request-validation path for the cognito-identity service (the generated constraint checks that run before each operation handler) must emit ValidationException with AWS's message construction, not the operation's modeled InvalidParameterException.
Traces: cognito-identity.create-identity-pool-invalid-name-rejected, cognito-identity.create-identity-pool-rejects-invalid-name, cognito-identity.identity-pool-name-invalid-symbol-rejected, cognito-identity.identity-pool-id-malformed-pattern-rejected, cognito-identity.describe-identity-pool-malformed-id-format-rejected, cognito-identity.delete-identities-oversized-batch-rejected, cognito-identity.delete-identities-too-many-requests-limit, cognito-identity.get-open-id-token-for-developer-identity-rejects-bad-token-duration, cognito-identity.get-open-id-token-for-developer-identity-token-duration-out-of-range-rejected, cognito-identity.open-id-token-developer-identity-duration-exceeded-rejected, cognito-identity.role-mapping-invalid-key-rejected, cognito-identity.set-identity-pool-roles-invalid-keys-rejected, cognito-identity.set-principal-tag-attribute-map-exceeds-max-mappings-rejected.

=== Cause 2 of 8: character-set validation of pool names and tag values is not enforced ===
AWS: CreateIdentityPool.IdentityPoolName carries @pattern ^[\w\s+=,.@-]+$ (Smithy), so a name containing an accented letter or any symbol outside that class is refused. For IdentityPoolTags the model states only a length constraint on the value shape; recorded behavior shows AWS refuses a tag value containing an emoji with HTTP 400 InvalidParameterException (recorded message example: "Tags can only contain letters, numbers, spaces, and the following special characters: _ . : / = + - @"), while unicode letters are accepted and preserved byte-identically.
Emulator: CreateIdentityPool accepts both — a name outside the modeled pattern returns 200 with the name echoed, and an emoji tag value is accepted and persisted.
Why: the emulator is overly permissive: code written against it creates pools and tags that real AWS rejects, so the failure only appears in production.
Fix: CreateIdentityPool/UpdateIdentityPool validation — enforce the IdentityPoolName pattern and the tag-value character class before the pool is stored.
Traces: cognito-identity.identity-pool-name-unicode-preserved, cognito-identity.identity-pool-name-unicode-round-trip.

=== Cause 3 of 8: an explicitly empty SupportedLoginProviders map is echoed back as a present empty map ===
AWS: recorded behavior only — CreateIdentityPool called with SupportedLoginProviders={} returns a pool representation in which the member is ABSENT (AWS normalizes empty-vs-absent for this map), so "configured but empty" and "not configured" are indistinguishable on the wire.
Emulator: returns SupportedLoginProviders: {} (member present, empty).
Why: clients that test for the member's presence to decide whether federated providers are configured take the wrong branch.
Fix: the CreateIdentityPool/DescribeIdentityPool serializer (or the state normalization behind it) must drop empty maps for this member.
Traces: cognito-identity.identity-pool-empty-vs-absent-supported-login-providers-distinguished, cognito-identity.supported-login-providers-empty-vs-absent-preserved.

=== Cause 4 of 8: SetIdentityPoolRoles never verifies the role ARN it is handed ===
AWS: SetIdentityPoolRoles models NotAuthorizedException (403). Recorded behavior: a Roles value that is not a resolvable IAM role ARN is refused with NotAuthorizedException (Cognito cannot verify/assume it) rather than a syntax rejection, and a role ARN belonging to a different account is refused as well.
Emulator: a garbage ARN string is refused with InvalidParameterException (wrong code), and a cross-account role ARN is ACCEPTED with HTTP 200 and persisted.
Why: a pool can be left pointing at a role that cannot be assumed, and the error code clients key on for "role not usable" never appears; the cross-account case is a privilege-boundary gap.
Fix: SetIdentityPoolRoles handler — resolve each role ARN against IAM state, check the account, and refuse with NotAuthorizedException when it is not verifiable.
Traces: cognito-identity.set-identity-pool-roles-invalid-role-arn-rejected, cognito-identity.set-identity-pool-roles-rejects-cross-account-role.

=== Cause 5 of 8: no per-caller authorization — every set of credentials resolves to one principal ===
AWS: all cognito-identity control-plane operations are IAM-authorized; a caller with no identity-based policy allowing cognito-identity:GetIdentityPoolRoles on the pool ARN is refused with HTTP 403 AccessDeniedException, and the refusal message is built from the request ("User: <caller arn> is not authorized to perform: cognito-identity:GetIdentityPoolRoles on resource: <pool arn> because no identity-based policy allows the cognito-identity:GetIdentityPoolRoles action"). Granting exactly that action turns the identical call into a success.
Emulator: the zero-permission caller's GetIdentityPoolRoles returns HTTP 200 with the pool's roles. The underlying reason is that credentials are not resolved to distinct principals at all: a probe signed with a restricted principal's keys and the ambient caller both resolve to the same IAM user ARN (observed: arn:aws:iam::<account>:user/emulator for both), so no authorization decision can be made.
Why: every least-privilege / denied-access test passes against the emulator and fails in production; the emulator cannot be used to validate IAM scoping of Cognito pools at all.
Fix: credential resolution in the request-auth layer (map sigv4 access keys to the IAM principal that owns them) plus an authorization check in the cognito-identity operation handlers.
Traces: cognito-identity.get-identity-pool-roles-unauthorized-for-foreign-pool, cognito-identity.delete-identity-pool-not-authorized-cross-account.

=== Cause 6 of 8: ConcurrentModificationException is never raised for concurrent SetIdentityPoolRoles ===
AWS: SetIdentityPoolRoles models ConcurrentModificationException (HTTP 400) and recorded behavior shows at least one of several simultaneous writers to the same pool losing the race with that error.
Emulator: ten simultaneous SetIdentityPoolRoles calls on one pool all return HTTP 200; no writer is refused.
Why: clients that implement retry/backoff on ConcurrentModificationException have that path dead-coded, and last-writer-wins races go unnoticed until production.
Fix: the identity-pool state store behind SetIdentityPoolRoles needs an optimistic-concurrency guard that surfaces ConcurrentModificationException.
Traces: cognito-identity.set-identity-pool-roles-concurrent-modification-error-path.

=== Cause 7 of 8: negative/rejection paths land on unimplemented scaffold stubs, so clients see NotImplemented where AWS returns a modeled error ===
AWS: TagResource models InvalidParameterException (tag-count/key-length/reserved 'aws:' prefix refusals), UpdateIdentityPool models ResourceNotFoundException for a deleted pool and ConcurrentModificationException for a lost race, and GetPrincipalTagAttributeMap returns ResourceNotFoundException before any Set call (recorded).
Emulator: these calls answer the scaffold's NotImplemented error instead. Scaffold-stub issues exist for the operations themselves (20261023T135552Z-cognito-identity-getopenidtokenfordeveloperidentity-stub.md and the auto-filed TagResource/UpdateIdentityPool/ListTagsForResource/SetPrincipalTagAttributeMap stub issues); none of them covers these checks, which assert the rejection SEMANTICS of the operations once implemented.
Why: every error-handling branch a client has for Cognito tagging and pool updates is untestable, and a validation bug cannot be distinguished from a missing feature.
Fix: implement the handlers (tag_resource.rs, update_identity_pool.rs, get_principal_tag_attribute_map.rs under the cognito-identity service's ops/) including their validation and not-found/conflict classification.
Traces: cognito-identity.tag-resource-exceeds-max-tag-count-rejected, cognito-identity.tag-resource-key-exceeding-length-rejected, cognito-identity.tag-resource-rejects-reserved-aws-key, cognito-identity.get-principal-tag-attribute-map-default-empty, cognito-identity.delete-identity-pool-is-not-idempotent, cognito-identity.update-identity-pool-concurrent-modification-detected.

=== Cause 8 of 8: the account exposes no cognito-identity Service Quotas entries, and the provider-per-pool quota is not enforced ===
AWS: the "Amazon Cognito user pool providers per identity pool" quota is published under Service Quotas ServiceCode='cognito-identity' (default 50, adjustable) and CreateIdentityPool refuses one provider over the account's quota with LimitExceededException (modeled on CreateIdentityPool, HTTP 400).
Emulator: ListServiceQuotas / ListAWSDefaultServiceQuotas for ServiceCode='cognito-identity' yield no matching quota at all, so the limit cannot even be discovered, and CreateIdentityPool's enforcement of it is consequently unexercised.
Why: quota-aware clients see no limits in the emulator and get LimitExceededException only in production.
Fix: seed the cognito-identity quota catalog in the service-quotas state, and enforce the CognitoIdentityProviders count in the CreateIdentityPool handler with LimitExceededException.
Traces: cognito-identity.create-identity-pool-limit-exceeded.

## Evidence
Cause 1 — cognito-identity.describe-identity-pool-malformed-id-format-rejected:
  expected ('ValidationException', "1 validation error detected: Value 'this-id-has-no-region-guid-separator' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w-]+:[0-9a-f-]+")
  actual   ('InvalidParameterException', "Value 'this-id-has-no-region-guid-separator' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\w-]+:[0-9a-f-]+$")
  Same shape of divergence on: create-identity-pool-invalid-name-rejected ("expected ValidationException, got InvalidParameterException"), create-identity-pool-rejects-invalid-name ("expected ValidationException for malformed IdentityPoolName, got InvalidParameterException"), delete-identities-oversized-batch-rejected ("expected ValidationException for a 61-id batch (documented limit is 60), got 'InvalidParameterException'"), delete-identities-too-many-requests-limit, get-open-id-token-for-developer-identity-* ("expected ValidationException for out-of-range TokenDuration, got 'InvalidParameterException': Value '999999999' at 'tokenDuration' failed to satisfy constraint: Member must have value less than or equal to 86400"), open-id-token-developer-identity-duration-exceeded-rejected, role-mapping-invalid-key-rejected, set-identity-pool-roles-invalid-keys-rejected, set-principal-tag-attribute-map-exceeds-max-mappings-rejected ("CODE=InvalidParameterException"), identity-pool-id-malformed-pattern-rejected, identity-pool-name-invalid-symbol-rejected.

Cause 2 — identity-pool-name-unicode-round-trip: "real AWS rejects this call, but it SUCCEEDED and returned {'IdentityPoolId': 'us-west-2:...', 'IdentityPoolName': 'sim2real-idp-... Café', ... HTTPStatusCode: 200}". identity-pool-name-unicode-preserved: "CreateIdentityPool unexpectedly accepted an emoji tag value for pool 'sim2real-behav-pool-reject-...'" (AWS: InvalidParameterException).

Cause 3 — identity-pool-empty-vs-absent-supported-login-providers-distinguished: "expected CreateIdentityPool to omit SupportedLoginProviders for an explicit empty map, got {}"; supported-login-providers-empty-vs-absent-preserved: "expected an explicitly empty SupportedLoginProviders map to be normalized to an absent field, got {}".

Cause 4 — set-identity-pool-roles-invalid-role-arn-rejected: Roles={'authenticated': 'not-a-real-arn-just-a-long-garbage-string-1234567890'} -> "expected NotAuthorizedException, got 'InvalidParameterException'". set-identity-pool-roles-rejects-cross-account-role: "expected a ClientError, but the call succeeded and returned {... 'HTTPStatusCode': 200 ...}".

Cause 5 — get-identity-pool-roles-unauthorized-for-foreign-pool: a brand-new IAM user with zero attached policies calls GetIdentityPoolRoles and gets "expected the call to be refused, but it SUCCEEDED and returned {'IdentityPoolId': 'us-west-2:8510a93a-...', 'Roles': {}, 'HTTPStatusCode': 200}" (AWS: 403 AccessDeniedException). delete-identity-pool-not-authorized-cross-account: "the restricted probe must resolve to a distinct principal from the real caller, got 'arn:aws:iam::<account>:user/emulator' for both".

Cause 6 — set-identity-pool-roles-concurrent-modification-error-path: "expected at least one of 10 racers on the same pool to lose the race with ConcurrentModificationException, but every call succeeded: [('ok', ...) x10]".

Cause 7 — tag-resource-exceeds-max-tag-count-rejected: "expected InvalidParameterException, got NotImplemented"; tag-resource-key-exceeding-length-rejected: "expected ValidationException for a 129-char tag key, got 'NotImplemented'"; tag-resource-rejects-reserved-aws-key: "expected InvalidParameterException tagging with a reserved 'aws:' prefixed key, got NotImplemented"; get-principal-tag-attribute-map-default-empty: "expected ResourceNotFoundException before any Set call, got 'NotImplemented'"; delete-identity-pool-is-not-idempotent: "UpdateIdentityPool on a deleted pool must fail with ResourceNotFoundException, got NotImplemented"; update-identity-pool-concurrent-modification-detected: "AssertionError: NotImplemented".

Cause 8 — create-identity-pool-limit-exceeded: "could not discover the cognito-identity user-pool-providers-per-identity-pool quota via Service Quotas (ServiceCode='cognito-identity')" after trying both ListServiceQuotas and ListAWSDefaultServiceQuotas.

Model evidence (smithy_query, services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): CreateIdentityPool.IdentityPoolName @length{1,128} @pattern ^[\w\s+=,.@-]+$, errors [InvalidParameterException, LimitExceededException, NotAuthorizedException, ResourceConflictException, TooManyRequestsException, InternalErrorException]; DeleteIdentities.IdentityIdsToDelete @length{1,60}; GetOpenIdTokenForDeveloperIdentity.TokenDuration @range{1,86400}; SetPrincipalTagAttributeMap.PrincipalTags @length{0,50}; SetIdentityPoolRoles errors include ConcurrentModificationException and NotAuthorizedException; IdentityPoolId @pattern ^[\w-]+:[0-9a-f-]+$. No operation models ValidationException — it is the protocol constraint-validation error.

## Steps to Reproduce
Boot the emulator and run the behavioral suite; the 29 cognito-identity checks listed in trace_ids reproduce the eight causes. Minimal reproductions:
1. Cause 1: cognito-identity DescribeIdentityPool with IdentityPoolId='this-id-has-no-region-guid-separator' -> expect 400 ValidationException with the "1 validation error detected: ..." message; observe 400 InvalidParameterException. Same with DeleteIdentities on a 61-element IdentityIdsToDelete, and GetOpenIdTokenForDeveloperIdentity with TokenDuration=86401.
2. Cause 2: CreateIdentityPool(IdentityPoolName='pool Café', AllowUnauthenticatedIdentities=True) -> expect refusal; observe 200. CreateIdentityPool with an IdentityPoolTags value containing an emoji -> expect 400 InvalidParameterException; observe 200.
3. Cause 3: CreateIdentityPool(..., SupportedLoginProviders={}) -> expect the member absent in the response; observe SupportedLoginProviders: {}.
4. Cause 4: SetIdentityPoolRoles(Roles={'authenticated': 'not-a-real-arn-...'}) -> expect 403 NotAuthorizedException; observe 400 InvalidParameterException. Repeat with a role ARN in another account -> expect refusal; observe 200.
5. Cause 5: create an IAM user with no policies, sign GetIdentityPoolRoles with its keys -> expect 403 AccessDeniedException; observe 200. Call sts:GetCallerIdentity with those keys and with the ambient keys -> both return the same principal ARN.
6. Cause 6: issue 10 concurrent SetIdentityPoolRoles on one pool -> expect at least one ConcurrentModificationException; observe all 200.
7. Cause 7: TagResource with 51 tags / a 129-char key / an 'aws:'-prefixed key, UpdateIdentityPool on a deleted pool, GetPrincipalTagAttributeMap before any Set -> expect the modeled errors; observe NotImplemented.
8. Cause 8: service-quotas ListServiceQuotas(ServiceCode='cognito-identity') -> expect the "Amazon Cognito user pool providers per identity pool" quota; observe no quotas. Then CreateIdentityPool with 51 CognitoIdentityProviders -> expect LimitExceededException.
