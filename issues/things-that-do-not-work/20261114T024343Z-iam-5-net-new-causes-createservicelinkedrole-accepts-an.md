Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam.create-service-linked-role-unsupported-service-rejected iam.get-account-summary iam.session-policy-intersects-the-role-policy iam.iam-access-key-crud iam.user-access-key-round-trips-status-and-never-used iam.requested-region-for-global-iam-is-us-east-1-other-region-denied
Issue-Shape-Id: com.amazonaws.iam#AWSIdentityManagementV20100508
Issue-Key: iam.create-service-linked-role-unsupported-service-rejected|_error#7|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: iam.get-account-summary|run#12|AssertionError|assert message == _no_policy_denial(unpriv_arn, "GetAccountSummary", message), (
Issue-Key: iam.session-policy-intersects-the-role-policy|run#32|AssertionError|assert grant_msg == expected_grant_msg, (
Issue-Key: iam.iam-access-key-crud|run#9|AssertionError|assert key_id.startswith("AKIA"), f"expected AKIA-prefixed access key id, got {key_id!r}"
Issue-Key: iam.user-access-key-round-trips-status-and-never-used|run#6|AssertionError|assert key["AccessKeyId"].startswith("AKIA"), f"a long-term key id starts AKIA: {key['AccessKeyId']!r}"
Issue-Key: iam.requested-region-for-global-iam-is-us-east-1-other-region-denied|_until_allowed#15|Timeout|time.sleep(3)
Issue-Commit: 3e55f33fd01bce8af894192328b0955056ba342a

# iam: 5 net-new causes — CreateServiceLinkedRole accepts an unknown service principal, AccessDenied messages drop the authorization-details pointer and render the resource as "user <name>", access key ids are EMULATOR-prefixed, and a new user key is never authorized

> _Note: this issue was identified against the code base at commit `3e55f33fd01bce8af894192328b0955056ba342a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLinkedRole

## Description
Five net-new root causes in the IAM control plane, all reached by failing checks in the `iam` suite section. All five must be closed before this issue is submitted.

Cause 1 of 5: CreateServiceLinkedRole creates a role for a service principal AWS does not support.
AWS: Smithy iam#CreateServiceLinkedRole — input AWSServiceName (required, groupNameType, @length 1-128, @pattern ^[\w+=,.@-]+$), output Role; modelled errors InvalidInput(400), LimitExceeded(409), NoSuchEntity(404), ServiceFailure(500). AWS only mints service-linked roles for service principals that have registered an SLR; for any other value it refuses with HTTP 403 error code `AccessDenied` and the message `<AWSServiceName> is not a valid AWS service name (Service Principal)` — fixed text with one variable part, the AWSServiceName copied from the request — and no role appears under /aws-service-role/. Recorded behavior only for the code/message pair (the model does not enumerate AccessDenied for this operation); a real principal (e.g. autoscaling.amazonaws.com) succeeds in the same session, so the refusal is not a permission problem.
Emulator: the call returns HTTP 200 and fabricates a role for the bogus principal — Path `/aws-service-role/<bogus>/`, RoleName `AWSServiceRoleFor<Bogus>`, a minted RoleId/Arn and a trust policy naming the nonexistent service — and the role is then listed under PathPrefix=/aws-service-role/.
Why: a client that typos or hard-codes an unsupported service principal is told the SLR exists; it then grants to and depends on a role no AWS service will ever assume, and the account accumulates roles AWS would never create. The documented negative class (AccessDenied naming the invalid service principal) is unreachable.
Fix: AWSServiceName validation in the CreateServiceLinkedRole handler — a registry of supported service principals, refusing anything else with 403 AccessDenied and the message template above before any state is written.
Traces: iam.create-service-linked-role-unsupported-service-rejected.

Cause 2 of 5: the IAM AccessDenied message omits AWS's trailing authorization-details pointer.
AWS: an IAM no-policy denial ends with a pointer to a per-request, server-minted authorization id, given twice: `. Go to https://<console region>.console.aws.amazon.com/iam/home?region=<console region>#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>`. Everything but the authorization id is fixed text (the console region is IAM's global partition region, us-east-1); the id itself is per-request and is the only part a client reads back. It follows the standard reason clause `User: <principal arn> is not authorized to perform: <action> on resource: <resource> because no identity-based policy allows the <action> action`. Smithy: GetAccountSummary models only ServiceFailure, so the denial class is recorded behavior only.
Emulator: the message stops at the reason clause — no `Go to https://…/authorization-details/…` segment and no GetRequestAuthorizationDetails pointer.
Why: an operator or tool that extracts the authorization id to call GetRequestAuthorizationDetails has nothing to extract, so the documented diagnosis path for a refusal is unavailable, and any client asserting on AWS's denial text diverges.
Related: open issue 20261109T163411Z-iam-3-net-new-causes-denials-omit-the.md describes this same cause for iam.get-account-authorization-details / iam.session-policy-intersects-the-role-policy's narrowing assertion; it does not carry the replay keys of the checks listed here, so these failures are not covered by it.
Fix: the denial-message builder in the IAM authorization gate must mint the per-request authorization id and append the pointer clause.
Traces: iam.get-account-summary, iam.session-policy-intersects-the-role-policy.

Cause 3 of 5: the denial message names the resource by entity type and name instead of its ARN.
AWS: in the reason clause above, `<resource>` is the resource the request acted on, written exactly as AWS writes resources in denials — the full ARN (`arn:aws:iam::<account>:user/<user name>`) for a named IAM entity, or `*` where the action has no resource. The variable parts are the account id and entity name from the request/state; the `arn:aws:iam::` framing is fixed.
Emulator: for a denied iam:CreateUser the message reads `... on resource: user sim2real-behav-sess-probe-<id> ...` — the entity type and bare name instead of the ARN (the name itself is a per-run value; the missing ARN framing is the defect).
Why: callers and log pipelines parse the resource out of a denial to identify what was refused and to build the matching policy statement Resource; a bare `user <name>` is not an ARN, so the extracted value cannot be used, and the message does not match AWS's text.
Fix: the resource rendering used by the IAM denial-message builder (the same component as Cause 2) must format the acted-on entity as its ARN.
Traces: iam.session-policy-intersects-the-role-policy.

Cause 4 of 5: CreateAccessKey mints an access key id that is not an AWS IAM key id.
AWS: Smithy iam#CreateAccessKey output AccessKey (required); AccessKey.AccessKeyId -> accessKeyIdType, @length 16-128, @pattern ^[\w]+$. AWS's unique-id prefixes are documented and load-bearing: a long-term IAM user access key id is 20 characters beginning `AKIA` (a temporary STS key begins `ASIA`). The prefix is the requirement; the 16 following characters are per-run.
Emulator: CreateAccessKey answers a 20-character id prefixed `EMULATOR` (observed `EMULATORAKSLJ7DYLTYE`, `EMULATORAKMBFRAQRAW2` — the suffixes are per-run values, the prefix is the defect), and ListAccessKeys echoes it.
Why: credential tooling classifies a key by its prefix (long-term vs temporary, IAM user vs role session); scanners, policy linters and clients validating the documented prefix misclassify or reject every key the emulator issues.
Related: open issue 20261111T103332Z-iam-7-net-new-causes-deleteaccountpasswordpolicy-is-unrouted.md states this cause (its Cause 6) for iam.minted-identifier-prefixes-and-arn-paths only; it does not carry the two checks listed here.
Fix: the access-key-id allocator used by CreateAccessKey.
Traces: iam.iam-access-key-crud, iam.user-access-key-round-trips-status-and-never-used.

Cause 5 of 5: a request signed with a freshly created IAM user access key is never authorized, so a global-IAM aws:RequestedRegion check never gets past its control call.
AWS: an access key returned by CreateAccessKey becomes usable within seconds; a client signing with it gets its user's grants, and because IAM is global every request to the IAM endpoint is authorized with aws:RequestedRegion = us-east-1 whatever region the client is configured for (so a StringEquals us-east-1 / StringNotEquals <other region> grant allows, and StringEquals <other region> / StringNotEquals us-east-1 denies with the AccessDenied message described in Causes 2-3). Recorded behavior only for the condition-key value — it is a service fact, not a modelled member.
Emulator: the check never gets a successful control call. Its only retry loop retries exclusively on `InvalidClientTokenId` and `AccessDenied` from iam:GetUser signed with the key it just created, and the check ends `Check timed out after 60.0s` without reaching any RequestedRegion assertion — so the emulator kept refusing a request signed with a valid, just-minted user access key (Hypothesis: the new key is not registered with the signature/authorization path, or user-policy grants are not applied to it). Whatever the mechanism, the emulator never allows the call AWS allows.
Why: every client that creates an access key and then uses it — the standard bootstrap for least-privilege test and tooling identities — is locked out, and no policy-evaluation behavior reached through such a key (including aws:RequestedRegion on the global IAM endpoint) can be exercised.
Fix: the credential-registration/authorization path for keys minted by CreateAccessKey (the IAM sigv4 credential lookup and the identity-policy evaluation for a user principal), and the aws:RequestedRegion value the IAM request context supplies.
Traces: iam.requested-region-for-global-iam-is-us-east-1-other-region-denied.

## Evidence
Behavioral suite replay (simparity-behavioral-aws-v1) against the emulator built from this checkout; iam section 397/464 passed.

Cause 1 — [mismatch] iam.create-service-linked-role-unsupported-service-rejected: AssertionError: expected a ClientError, but the call succeeded with {'Role': {'Path': '/aws-service-role/sim2real-not-a-real-service.amazonaws.com/', 'RoleName': 'AWSServiceRoleForSim2real-not-a-real-service', 'RoleId': 'AROAOWVOJOY3VBWOJCSHY', 'Arn': 'arn:aws:iam::000000000000:role/aws-service-role/sim2real-not-a-real-service.amazonaws.com/AWSServiceRoleForSim2real-not-a-real-service', ...}, 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}. Expected: AccessDenied with Message 'sim2real-not-a-real-service.amazonaws.com is not a valid AWS service name (Service Principal)' and nothing listed under PathPrefix=/aws-service-role/ for that principal (ids/names above are per-run values).

Cause 2 — [mismatch] iam.get-account-summary: AssertionError: expected AWS's no-policy denial of iam:GetAccountSummary; got 'User: arn:aws:iam::000000000000:user/nopriv-<id> is not authorized to perform: iam:GetAccountSummary on resource: * because no identity-based policy allows the iam:GetAccountSummary action' — the reason clause matches, the trailing '. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<authorization-id> for complete details, or call the GetRequestAuthorizationDetails API with the following authorization id: <authorization-id>' is absent.

Cause 3 — [mismatch] iam.session-policy-intersects-the-role-policy: AssertionError: expected message 'User: arn:aws:sts::000000000000:assumed-role/<role>/<session> is not authorized to perform: iam:CreateUser on resource: arn:aws:iam::000000000000:user/<probe user> because no identity-based policy allows the iam:CreateUser action. Go to https://us-east-1.console.aws.amazon.com/iam/home?region=us-east-1#/authorization-details/<id> ... authorization id: <id>', got '... on resource: user <probe user> because no identity-based policy allows the iam:CreateUser action' (resource rendered as 'user <name>', pointer clause absent).

Cause 4 — [mismatch] iam.iam-access-key-crud: AssertionError: expected AKIA-prefixed access key id, got 'EMULATORAKSLJ7DYLTYE'. [mismatch] iam.user-access-key-round-trips-status-and-never-used: AssertionError: a long-term key id starts AKIA: 'EMULATORAKMBFRAQRAW2'.

Cause 5 — [mismatch] iam.requested-region-for-global-iam-is-us-east-1-other-region-denied: 'Check timed out after 60.0s'. The check creates a user, puts a user policy with aws:RequestedRegion statements, creates an access key and then loops on iam:GetUser signed with that key, retrying only InvalidClientTokenId/AccessDenied; it never reached the first RequestedRegion assertion, so the control call was never allowed.

Model grounding (smithy_query on services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/com-amazonaws-iam-awsidentitymanagementv20100508.json): CreateServiceLinkedRole input AWSServiceName required (groupNameType, length 1-128, pattern ^[\w+=,.@-]+$), errors InvalidInput(400)/LimitExceeded(409)/NoSuchEntity(404)/ServiceFailure(500); CreateAccessKey output AccessKey required, accessKeyIdType @length 16-128 @pattern ^[\w]+$; GetAccountSummary input Unit, only ServiceFailure(500) modelled.

## Steps to Reproduce
1. git_clone the repo; build and boot the emulator from the checkout; run the behavioral suite (simparity-behavioral-aws-v1) and read the `iam` section.
2. Cause 1: call iam CreateServiceLinkedRole with AWSServiceName='sim2real-not-a-real-service.amazonaws.com' (after a successful CreateServiceLinkedRole for autoscaling.amazonaws.com to prove permissions); observe HTTP 200 and a fabricated role, then ListRoles(PathPrefix='/aws-service-role/'). Replay iam.create-service-linked-role-unsupported-service-rejected.
3. Cause 2: as a principal with no grant for iam:GetAccountSummary, call GetAccountSummary and read Error.Message — look for the trailing authorization-details / GetRequestAuthorizationDetails clause. Replay iam.get-account-summary.
4. Cause 3: assume a role under a session policy that does not allow iam:CreateUser, call CreateUser and read how Error.Message names the resource. Replay iam.session-policy-intersects-the-role-policy.
5. Cause 4: CreateUser then CreateAccessKey; inspect AccessKey.AccessKeyId's prefix and the id ListAccessKeys echoes. Replay iam.iam-access-key-crud and iam.user-access-key-round-trips-status-and-never-used.
6. Cause 5: CreateUser, PutUserPolicy granting iam:GetUser on that user plus aws:RequestedRegion-conditioned statements, CreateAccessKey, then sign iam:GetUser with the new key from a non-us-east-1 client; observe that it never succeeds (InvalidClientTokenId/AccessDenied) and that the region-conditioned allows/denies are never reached. Replay iam.requested-region-for-global-iam-is-us-east-1-other-region-denied.
