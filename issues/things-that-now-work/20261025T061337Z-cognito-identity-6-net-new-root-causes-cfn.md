Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity.deploy-cfn-role-attachment-enforced-on-credentials cognito-identity.deploy-cfn-role-attachment-missing-unauthenticated-role-rejected cognito-identity.deploy-identity-pool-cfn-deletion-removes-pool cognito-identity.deploy-identity-pool-cfn-get-id-succeeds cognito-identity.deploy-identity-pool-cfn-stack-update-changes-runtime-behavior cognito-identity.identity-pool-create-time-tags-persist cognito-identity.update-identity-pool-developer-provider-name-immutable-rejected cognito-identity.delete-identities-partial-failure-rejects-whole-batch cognito-identity.set-principal-tag-attribute-map-exceeds-max-mappings-rejected cognito-identity.update-pool-disallow-unauth-blocks-get-id
Issue-Shape-Id: com.amazonaws.cognitoidentity#AWSCognitoIdentityService
Issue-Key: cognito-identity|GetId|ValidationException
Issue-Key: cognito-identity|DescribeIdentityPool|ValidationException
Issue-Key: cognito-identity.identity-pool-create-time-tags-persist|run#19|AssertionError|assert after.get("IdentityPoolTags") == {"env": "test", "owner": "fidelity-check"}, (
Issue-Key: cognito-identity.update-identity-pool-developer-provider-name-immutable-rejected|run#11|AssertionError|assert update_resp.get("DeveloperProviderName") == dev_provider, (
Issue-Key: cognito-identity.delete-identities-partial-failure-rejects-whole-batch|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-identity.set-principal-tag-attribute-map-exceeds-max-mappings-rejected|run#12|AssertionError|assert message == (
Issue-Key: cognito-identity.update-pool-disallow-unauth-blocks-get-id|run#12|AssertionError|assert message == expected_message, (
Issue-Commit: cfbb9a1e24eec7d8c98af7d6e9e62a0e8013f018

# cognito-identity: 6 net-new root causes — CFN Ref of AWS::Cognito::IdentityPool still yields the pool NAME, UpdateIdentityPool wipes IdentityPoolTags and mutates the immutable DeveloperProviderName, and 3 refusal-message rules diverge

> _Note: this issue was identified against the code base at commit `cfbb9a1e24eec7d8c98af7d6e9e62a0e8013f018`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIdentityPool

## Description
This issue bundles SIX distinct net-new root causes reached through cognito-identity behavioral checks in suite simparity-behavioral-ga-prod-v1. All six must be fixed and the full trace_ids list re-replayed before closing — each cause below carries its own failing check ids.

=== Cause 1 of 6: Ref / physical id of a CloudFormation AWS::Cognito::IdentityPool is the pool NAME, not the pool id ===
AWS: Ref on AWS::Cognito::IdentityPool resolves to the identity-pool id in REGION:GUID form — the modelled IdentityPoolId (Smithy: GetId.IdentityPoolId / DescribeIdentityPool.IdentityPoolId, @required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$) — so a stack Output or an intrinsic built from !Ref can be handed straight to GetId, DescribeIdentityPool, GetIdentityPoolRoles or SetIdentityPoolRoles.
Emulator: the stack reaches CREATE_COMPLETE and the Output/Ref carries the template's IdentityPoolName, so the very next cognito-identity call on that value is refused HTTP 400 ValidationException "1 validation error detected: Value '<pool name>' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+" (seen on GetId and on DescribeIdentityPool).
Why: no CloudFormation-deployed identity pool is addressable through the Cognito API, so every infrastructure-as-code flow (role attachment, stack-update-changes-behavior, delete-removes-pool) is a dead end even though CloudFormation reports success.
Fix: the CloudFormation resource provider for AWS::Cognito::IdentityPool — it must create the pool in cognito-identity state and record the real pool id as the resource's physical id (what Ref / GetAtt / Outputs resolve to); the stack-delete path must then delete that pool by id.
Related but NOT covering: open issues 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md (its Cause 6) and 20261023T185902Z-cognito-identity-unsigned-anonymous-requests-answer-unknownservice-and.md describe this same provider defect, but their replay keys were recorded against the PREVIOUS refusal code (InvalidParameterException); the refusal is now ValidationException, so none of the five check ids below are owned by them any more and two of them (deploy-identity-pool-cfn-deletion-removes-pool, deploy-identity-pool-cfn-get-id-succeeds) appear in neither.
Traces: cognito-identity.deploy-cfn-role-attachment-enforced-on-credentials, cognito-identity.deploy-cfn-role-attachment-missing-unauthenticated-role-rejected, cognito-identity.deploy-identity-pool-cfn-deletion-removes-pool, cognito-identity.deploy-identity-pool-cfn-get-id-succeeds, cognito-identity.deploy-identity-pool-cfn-stack-update-changes-runtime-behavior.

=== Cause 2 of 6: UpdateIdentityPool clears the pool's IdentityPoolTags ===
AWS: recorded behavior only — tags on an identity pool are a separately managed facet (TagResource / UntagResource / ListTagsForResource; Smithy: IdentityPool.IdentityPoolTags is an optional IdentityPoolTagsType map on both the input and the output of UpdateIdentityPool). An UpdateIdentityPool that changes unrelated members and omits IdentityPoolTags leaves the existing tag map intact, and a following DescribeIdentityPool still reports it verbatim.
Emulator: after UpdateIdentityPool(IdentityPoolId, IdentityPoolName=<new>, AllowUnauthenticatedIdentities=false) with no IdentityPoolTags member, DescribeIdentityPool reports NO IdentityPoolTags at all — the previously stored map is gone. (Create-time echo and the pre-update DescribeIdentityPool read both matched, so the loss happens on update.)
Why: any client that renames a pool or flips a flag silently destroys its tag set — cost allocation, ABAC conditions on aws:ResourceTag and tag-based inventory all break after a routine config change, with no error to signal it.
Fix: the UpdateIdentityPool handler (services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/src/ops/update_identity_pool.rs) / the pool record in crate::state — treat an absent IdentityPoolTags member as 'leave the stored tag map unchanged' rather than replacing the record wholesale.
Traces: cognito-identity.identity-pool-create-time-tags-persist.

=== Cause 3 of 6: UpdateIdentityPool applies (and reports) a change to the immutable DeveloperProviderName ===
AWS: recorded behavior only — DeveloperProviderName (Smithy: IdentityPool.DeveloperProviderName, @length{1,128} @pattern ^[\w._-]+$, documented as 'The "domain" by which Cognito will refer to your users') is immutable once set. UpdateIdentityPool carrying a different DeveloperProviderName is NOT refused (neither ResourceConflictException nor InvalidParameterException is raised), the call returns 200, and the stored value is unchanged — the response echoes the ORIGINAL DeveloperProviderName and a later DescribeIdentityPool agrees.
Emulator: the update is applied — the UpdateIdentityPool response reports the submitted new value ('login.sim2real.changed' where the pool was created with 'login.sim2real.original').
Why: a client can silently repoint the developer-provider domain of a live pool, orphaning every developer-authenticated identity that was linked under the original domain; and the response actively confirms a mutation AWS never performs, so no client can detect the divergence.
Fix: the UpdateIdentityPool handler (ops/update_identity_pool.rs) must drop a DeveloperProviderName that differs from the stored one (keeping the stored value in the echoed output) once the pool has one.
Traces: cognito-identity.update-identity-pool-developer-provider-name-immutable-rejected.

=== Cause 4 of 6: a list-member constraint violation is reported per-member instead of as AWS's aggregated list-level validation error ===
AWS: DeleteIdentities.IdentityIdsToDelete is IdentityIdList (Smithy: @length{1,60} over member IdentityId @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$; the operation does not model ValidationException — this is the framework's pre-handler constraint check). A batch containing one badly formatted id is refused HTTP 400 ValidationException with the message built from the request at the LIST path, rendering the whole submitted list once and the member constraint set together: "1 validation error detected: Value '[<id1>, <id2>]' at 'identityIdsToDelete' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 55, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+]".
Emulator: the status (400) and code (ValidationException) now match, but the message addresses the individual member — "1 validation error detected: Value 'not-a-real-identity-id' at 'identityIdsToDelete.2.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+" — a different path form and only the one violated constraint.
Why: the rule that produces this text is request-derived and is the contract clients and support tooling parse to find which input member was rejected; an indexed '.2.member' path that AWS never emits (and a constraint list AWS always emits in full) breaks that parsing, and the index form also differs between the two renderings.
Fix: the shared pre-handler constraint-validation/reporting path for cognito-identity list members (the renderer that builds the 'N validation error(s) detected' body), not the DeleteIdentities handler's own logic.
Related but NOT covering: open issue 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md Cause 3 names this check for the earlier InvalidParameterException-vs-ValidationException misclassification; the code is now correct and only this message rule remains, so its replay key no longer matches.
Traces: cognito-identity.delete-identities-partial-failure-rejects-whole-batch.

=== Cause 5 of 6: the over-limit map refusal renders an entry COUNT instead of the rejected map ===
AWS: SetPrincipalTagAttributeMap.PrincipalTags is PrincipalTags (Smithy: @length{min 0, max 50} map). A 51-entry map is refused HTTP 400 ValidationException with the offending value rendered from the request — "1 validation error detected: Value '{<k>=<v>, <k>=<v>, …}' at 'principalTags' failed to satisfy constraint: Member must have length less than or equal to 50" — i.e. the map's entries, in AWS's own stable key order (the check encodes that order; the requirement here is that the rejected value is rendered, not a summary of it).
Emulator: "1 validation error detected: Value '51 entries' at 'principalTags' failed to satisfy constraint: Member must have length less than or equal to 50" — the value is replaced by a count, so the body never names what was rejected.
Why: the message rule is request-derived; a client or operator diffing the rejected payload out of the error body gets no payload at all, and the body cannot be correlated with the request that produced it.
Fix: the same shared constraint-violation renderer for cognito-identity (map/collection value rendering), reached via the SetPrincipalTagAttributeMap input validation (ops/set_principal_tag_attribute_map.rs).
Traces: cognito-identity.set-principal-tag-attribute-map-exceeds-max-mappings-rejected.

=== Cause 6 of 6: the unauthenticated-access refusal interpolates the pool id into a message AWS keeps identifier-free ===
AWS: GetId models NotAuthorizedException (403). For a pool whose AllowUnauthenticatedIdentities is false, an anonymous GetId is refused 403 NotAuthorizedException with the fixed sentence AWS returns for this condition: "Unauthenticated access is not supported for this identity pool." — no pool identifier appears in it.
Emulator: same status and code, but the message is "Unauthenticated access is not supported for the identity pool '<region>:<guid>'." — locally authored prose that leaks the pool id.
Why: the condition's sentence is a known fixed rule, and operators and support tooling match on it; interpolating a per-request identifier makes the message unmatchable and puts a resource identifier into an error body AWS keeps clean.
Fix: the message construction in the GetId handler (ops/get_id.rs) for the unauthenticated-access refusal.
Related but NOT covering: open issue 20261025T005354Z-cognito-identity-getid-admits-unvalidatable-login-tokens-setprincipaltagattributemap.md Cause 4 describes exactly this message rule, but its trace list carries only cognito-identity.server-side-unauthenticated-role-blocked-without-unauth-access-enabled and cognito-identity.unauthenticated-disabled-pool-refuses-get-id-and-creates-no-identity — not this third check, whose replay key is not in it.
Traces: cognito-identity.update-pool-disallow-unauth-blocks-get-id.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1 (36/149 passed). Per-cause replay output:

Cause 1 — all five deploy checks abort on the first cognito-identity call made with the Ref'd value (verdict [error], botocore ClientError):
- deploy-cfn-role-attachment-enforced-on-credentials: "An error occurred (ValidationException) when calling the GetId operation: 1 validation error detected: Value 'sim2real-cip-b0756fd09a59' at 'identityPoolId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+" — the value came from {"Ref": "IdentityPool"} in the stack the check deploys.
- deploy-cfn-role-attachment-missing-unauthenticated-role-rejected: same refusal on GetId, Value 'sim2real-cip-0581853319e5'.
- deploy-identity-pool-cfn-stack-update-changes-runtime-behavior: same refusal on GetId, Value 'sim2real-behav-pool-d7ed347ee48a'.
- deploy-identity-pool-cfn-get-id-succeeds: same refusal on DescribeIdentityPool, Value 'sim2real-behav-pool-fd6061a75a52' (read from stack Output PoolId = !Ref IdentityPool).
- deploy-identity-pool-cfn-deletion-removes-pool: same refusal on DescribeIdentityPool, Value 'sim2real_cog_pool_64f3086cc0ba'.
Expected in every case: the Ref'd value is the pool id (REGION:GUID) and the call is served.

Cause 2 — identity-pool-create-time-tags-persist [mismatch]: CreateIdentityPool(IdentityPoolTags={'env':'test','owner':'fidelity-check'}) echoed the tags and the first DescribeIdentityPool reported them (both asserts passed); after UpdateIdentityPool(IdentityPoolName=<new name>, AllowUnauthenticatedIdentities=False) the check fails with "AssertionError: expected tags to survive an unrelated UpdateIdentityPool, got None".

Cause 3 — update-identity-pool-developer-provider-name-immutable-rejected [mismatch]: pool created with DeveloperProviderName='login.sim2real.original'; UpdateIdentityPool(..., DeveloperProviderName='login.sim2real.changed') returned 200 and the check fails with "AssertionError: UpdateIdentityPool response must not report the rejected DeveloperProviderName change, got 'login.sim2real.changed'".

Cause 4 — delete-identities-partial-failure-rejects-whole-batch [mismatch]: DeleteIdentities(IdentityIdsToDelete=['us-west-2:99561a1b-5b3d-43a5-ba95-003240aca1af', 'not-a-real-identity-id']) -> code ValidationException (matched). Message expected "1 validation error detected: Value '[us-west-2:99561a1b-5b3d-43a5-ba95-003240aca1af, not-a-real-identity-id]' at 'identityIdsToDelete' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 55, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+]"; got "1 validation error detected: Value 'not-a-real-identity-id' at 'identityIdsToDelete.2.member' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+:[0-9a-f-]+".

Cause 5 — set-principal-tag-attribute-map-exceeds-max-mappings-rejected [mismatch]: SetPrincipalTagAttributeMap(PrincipalTags={tag0..tag50}) -> code ValidationException (matched); message got "1 validation error detected: Value '51 entries' at 'principalTags' failed to satisfy constraint: Member must have length less than or equal to 50", expected the same sentence with the 51-entry map rendered as the value.

Cause 6 — update-pool-disallow-unauth-blocks-get-id [mismatch]: after UpdateIdentityPool(AllowUnauthenticatedIdentities=False) (the Describe read-back confirmed False), anonymous GetId -> NotAuthorizedException (matched); message got "Unauthenticated access is not supported for the identity pool 'us-west-2:c51c15b5-3143-45ed-bf4b-3f8f6edf7c93'.", expected "Unauthenticated access is not supported for this identity pool."

Model facts used (smithy_query on services/service-com-amazonaws-cognitoidentity-awscognitoidentityservice/smithy-model/com-amazonaws-cognitoidentity-awscognitoidentityservice.json): GetId.IdentityPoolId @required @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$, errors include NotAuthorizedException 403 / InvalidParameterException 400, auth [] + optionalAuth true; UpdateIdentityPool input/output shape IdentityPool with optional DeveloperProviderName @length{1,128} @pattern ^[\w._-]+$ (doc: 'The "domain" by which Cognito will refer to your users.') and optional IdentityPoolTags (IdentityPoolTagsType map); DeleteIdentities.IdentityIdsToDelete -> IdentityIdList @length{1,60} over IdentityId @length{1,55} @pattern ^[\w-]+:[0-9a-f-]+$, no ValidationException modelled; SetPrincipalTagAttributeMap.PrincipalTags -> PrincipalTags map @length{0,50}, errors include InvalidParameterException 400 / ResourceNotFoundException 404.

All ten check ids carry the service token 'cognito-identity', which is bootstrapped in this checkout.

## Steps to Reproduce
1. Clone the repo, boot the emulator and replay suite simparity-behavioral-ga-prod-v1; re-replay exactly the ten ids in trace_ids to confirm a fix.
2. Cause 1: CreateStack with a template containing {"IdentityPool": {"Type": "AWS::Cognito::IdentityPool", "Properties": {"IdentityPoolName": <name>, "AllowUnauthenticatedIdentities": true}}} and an Output Value={"Ref": "IdentityPool"}; wait for CREATE_COMPLETE, then call cognito-identity DescribeIdentityPool / GetId with that output. Expect the pool id (REGION:GUID) and a served call; observe the pool NAME and 400 ValidationException on 'identityPoolId'. Then DeleteStack and confirm DescribeIdentityPool on the real pool id answers ResourceNotFoundException.
3. Cause 2: CreateIdentityPool(IdentityPoolName=<n>, AllowUnauthenticatedIdentities=true, IdentityPoolTags={'env':'test','owner':'fidelity-check'}); DescribeIdentityPool (tags present); UpdateIdentityPool(IdentityPoolId=<id>, IdentityPoolName=<n2>, AllowUnauthenticatedIdentities=false) with NO IdentityPoolTags member; DescribeIdentityPool again. Expect the two tags; observe no IdentityPoolTags.
4. Cause 3: CreateIdentityPool(..., DeveloperProviderName='login.sim2real.original'); UpdateIdentityPool(same pool, DeveloperProviderName='login.sim2real.changed'). Expect 200 whose output still reports 'login.sim2real.original', and DescribeIdentityPool unchanged; observe the new value in both.
5. Cause 4: GetId on an unauthenticated-enabled pool to mint a real id, then DeleteIdentities(IdentityIdsToDelete=[<that id>, 'not-a-real-identity-id']). Expect 400 ValidationException whose message renders the whole list at 'identityIdsToDelete' with the full member constraint set; observe the per-member 'identityIdsToDelete.2.member' form.
6. Cause 5: SetPrincipalTagAttributeMap(IdentityPoolId=<pool>, IdentityProviderName=<a configured provider>, PrincipalTags={f'tag{i}': f'claim{i}' for i in range(51)}). Expect 400 ValidationException rendering the map as the value at 'principalTags'; observe "Value '51 entries'".
7. Cause 6: UpdateIdentityPool(AllowUnauthenticatedIdentities=False) on a pool, then anonymous GetId(IdentityPoolId=<pool>). Expect 403 NotAuthorizedException "Unauthenticated access is not supported for this identity pool."; observe the variant naming the pool id.
