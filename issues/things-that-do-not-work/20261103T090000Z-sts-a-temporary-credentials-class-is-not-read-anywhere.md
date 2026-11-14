Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.assume-role-session-policy-cannot-exceed-role-permissions sts.assume-role-session-policy-cannot-exceed-role-permissions-denied sts.assume-role-session-policy-explicit-deny-wins sts.assume-role-session-tag-empty-string-value-preserved sts.assume-role-session-tag-unicode-value-round-trip sts.concurrent-assume-role-session-tag-updates-do-not-leak-across-sessions sts.concurrent-get-federation-token-calls-produce-independent-policies sts.get-caller-identity-cannot-be-denied-by-policy sts.get-federation-token-no-policy-grants-no-permissions sts.get-federation-token-without-policy-denies-actions sts.get-federation-token-cannot-call-iam-denied sts.get-federation-token-policy-document-unicode-round-trip sts.get-delegated-access-token-without-required-permission-denied sts.principal-type-condition-distinguishes-an-assumed-role-from-a-federated-user sts.e2e-federation-token-policy-restricts-then-expires sts.e2e-federation-token-policy-scopes-down-then-expires sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session sts.assume-role-external-id-required-denied sts.assume-role-session-policy-action-wildcard-granularity sts.assume-role-session-policy-intersection-allows-and-denies sts.assume-role-session-tag-decides-policy-condition sts.assume-role-trust-policy-principal-restriction-enforced sts.assumed-role-credentials-are-rejected-after-their-expiration-instant-passes sts.decode-authorization-message-round-trip sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them sts.e2e-assume-role-session-tag-propagates-to-resource-access sts.managed-policy-arn-and-inline-session-policy-intersect-to-the-narrower-of-the-two sts.permissions-boundary-denies-an-action-the-roles-own-identity-policy-allows sts.session-policy-region-condition-rejected-in-one-region-and-permitted-in-another sts.token-issue-time-is-present-only-for-the-temporary-half-of-one-identity sts.two-differently-scoped-sessions-used-concurrently-keep-their-own-authorization-outcomes
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.assume-role-session-policy-cannot-exceed-role-permissions|run#25|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.assume-role-session-policy-cannot-exceed-role-permissions-denied|run#20|AssertionError|assert code in allowed_codes, msg
Issue-Key: sts.assume-role-session-policy-explicit-deny-wins|run#28|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.assume-role-session-tag-empty-string-value-preserved|run#9|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.assume-role-session-tag-unicode-value-round-trip|run#16|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.concurrent-assume-role-session-tag-updates-do-not-leak-across-sessions|run#27|AssertionError|assert result["inner_ok"] is False, f"blue-tagged session leaked the red grant: {result}"
Issue-Key: sts.concurrent-get-federation-token-calls-produce-independent-policies|run#5|AssertionError|assert sm_result == ("AccessDeniedException", _SM_DENIED.format(arn=fed_arn)), (
Issue-Key: sts.get-caller-identity-cannot-be-denied-by-policy|_client_error#21|AssertionError|raise AssertionError(f"expected the call to fail, but it succeeded: {result!r}")
Issue-Key: sts.get-federation-token-no-policy-grants-no-permissions|run#18|AssertionError|assert code == "AccessDenied", (
Issue-Key: sts.get-federation-token-without-policy-denies-actions|probe#12|AssertionError|raise AssertionError(
Issue-Key: sts.get-federation-token-cannot-call-iam-denied|_error#13|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: sts.get-federation-token-policy-document-unicode-round-trip|_client_error#31|AssertionError|raise AssertionError(f"expected this call to be denied, but it SUCCEEDED and returned {result!r}")
Issue-Key: sts.get-delegated-access-token-without-required-permission-denied|_poll_until#18|AssertionError|raise AssertionError(f"{timeout_message} (last observed: {result!r})")
Issue-Key: sts.principal-type-condition-distinguishes-an-assumed-role-from-a-federated-user|run#51|AssertionError|assert role_outcomes == expected_role, (
Issue-Key: sts.e2e-federation-token-policy-restricts-then-expires|_client_error#27|AssertionError|raise AssertionError("the call succeeded where AWS refuses it")
Issue-Key: sts.e2e-federation-token-policy-scopes-down-then-expires|_client_error#18|AssertionError|raise AssertionError("the call succeeded where AWS refuses it")
Issue-Key: sts.a-chained-session-outlasts-the-parent-role-and-expires-later-than-the-parent-session|_poll#57|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-external-id-required-denied|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-session-policy-action-wildcard-granularity|_poll#32|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-session-policy-intersection-allows-and-denies|_poll#38|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-session-tag-decides-policy-condition|_poll#30|Timeout|time.sleep(interval_s)
Issue-Key: sts.assume-role-trust-policy-principal-restriction-enforced|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: sts.assumed-role-credentials-are-rejected-after-their-expiration-instant-passes|run#47|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Key: sts.decode-authorization-message-round-trip|_provoke_encoded_authorization_message#19|Timeout|time.sleep(2.0)
Issue-Key: sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them|_poll#43|Timeout|time.sleep(interval_s)
Issue-Key: sts.e2e-assume-role-session-tag-propagates-to-resource-access|_retry_until#32|Timeout|time.sleep(interval_s)
Issue-Key: sts.managed-policy-arn-and-inline-session-policy-intersect-to-the-narrower-of-the-two|_poll#51|Timeout|time.sleep(interval_s)
Issue-Key: sts.permissions-boundary-denies-an-action-the-roles-own-identity-policy-allows|_poll#44|Timeout|time.sleep(interval_s)
Issue-Key: sts.session-policy-region-condition-rejected-in-one-region-and-permitted-in-another|_poll#50|Timeout|time.sleep(interval_s)
Issue-Key: sts.token-issue-time-is-present-only-for-the-temporary-half-of-one-identity|_poll#46|Timeout|time.sleep(interval_s)
Issue-Key: sts.two-differently-scoped-sessions-used-concurrently-keep-their-own-authorization-outcomes|_poll#48|Timeout|time.sleep(interval_s)
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# STS: a temporary credential's CLASS is not read anywhere, and a deleted role's sessions keep working — the last two pieces of session-scoped authorization (2 causes)

> _Split out of `issues/things-that-now-work/20261029T120000Z-sts-3-of-the-8-session-scoping-causes-remain-open.md`, whose cause 5 is fixed (STS `GetWebIdentityToken` vends a JWT signed with a key of the account's issuer, and that issuer publishes its discovery document and its key set) and whose cause 1 is fixed at the Secrets Manager enforcement site that report named (a session policy now caps `secretsmanager:*` there, as it already did at S3, DynamoDB, EBS, CloudTrail Data, S3 Vectors and IAM's own gate). This report carries the replay keys that still do NOT pass, re-filed here so they are not recorded as working. Everything below is what that fix established about them; a later fixer starts from here rather than from scratch._
>
> _The causes are numbered as the parent report numbered them, so the three reports can be read together._

## Operation
GetFederationToken

## Description

Cause 1 of 8 (parent numbering), remaining half: a temporary credential's CLASS has no effect anywhere.

The DECISION and the DIRECTORY both exist now — `services/common/src/iam_authorization.rs` evaluates the documents, `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/src/state.rs`'s `SessionPolicies` / `ScopingPolicies` / `session_scoping_documents` record and resolve the scope a mint passed, and each data plane that gates a request reads it (`src/state/authorization.rs` or `src/service/authorization.rs` in the S3, DynamoDB, EBS, CloudTrail Data, S3 Vectors and Secrets Manager crates). What is still missing is everything that depends on WHICH KIND of credential was presented, which `crate::state::Caller::kind` carries and no gate reads:

* **A `GetFederationToken` session that passed NO policy has no permissions at all.** "Though the session policy parameters are optional, if you do not pass a policy, then the resulting federated user session has no permissions" (`GetFederationToken`, the STS model). A credential with an empty scope is currently read as a credential *capped by nothing* — `ScopingPolicies::decision` answers `Allowed` for an empty document set, which is right for a long-term key and for an `AssumeRole` session minted without session policies, and wrong for this one class. Traces: `sts.get-federation-token-no-policy-grants-no-permissions`, `sts.get-federation-token-without-policy-denies-actions`.
* **A `GetFederationToken` credential cannot call IAM at all, and a `GetSessionToken` credential cannot unless the mint carried MFA.** "You cannot call any IAM operations using the Amazon Web Services CLI or the Amazon Web Services API" (`GetFederationToken`); "you cannot call any IAM API operations unless MFA authentication information is included in the request" (`GetSessionToken`). STS already applies the STS half of this rule per class (`crate::state::caller_may_call`), and IAM's own gate applies no part of it: `signing_principal` resolves a long-term key and a ROLE SESSION only, so a federated-user credential carries no documents and is therefore allowed (the "callers that are not IAM users" note in `src/state/authorization.rs`). Trace: `sts.get-federation-token-cannot-call-iam-denied`.
* **`aws:PrincipalType` must select on the class** — `AssumedRole` for an `AssumeRole` session, `FederatedUser` for a federation token, `User` for a long-term key — which no gate publishes as a condition key. Trace: `sts.principal-type-condition-distinguishes-an-assumed-role-from-a-federated-user`.

Where the work is: the kind is already resolved beside the scope (`resolve_request_caller` answers `Caller { kind, session_policies, .. }`), so the change is to carry it into the ONE resolution of a scope — `session_scoping_documents` — and into the ONE reading of it (`ScopingPolicies::decision`), rather than into each gate: a `ScopingPolicies` that grants nothing is the shape the first bullet needs, and every gate then caps by it without a line of its own. That is a signature change at the seven call sites of `session_scoping_documents`, which is why the Secrets Manager fix did not make it. IAM's own gate needs the second bullet in addition, because the class ban is not a document.

Cause 8 of 8 (parent numbering), remaining half: a deleted role's sessions keep working, and the fifteen polling checks still do not complete.

AWS: deleting a role invalidates the credentials already issued for it, and re-creating a role of the same name does not restore them; a session policy, a trust-policy principal restriction, a region condition, a session-tag condition and a permissions boundary each produce AccessDenied/403 on the next call. Emulator: a session whose role IAM no longer holds is read as a principal carrying no documents and therefore ALLOWED (the same note as above), and because the role is resolved BY NAME (`store.roles.get(&name_key(role_name))`), re-creating it restores the session exactly. What is missing is the revocation hook the parent report named: `iam:DeleteRole` must give up the credentials of the sessions minted for that role (the records in `ComAmazonawsIamAwsidentitymanagementv20100508State::role_sessions`, through `ctx.credential_store().delete`), so the emulator's own authentication layer stops accepting them whatever service they are presented to. Trace: `sts.deleting-the-role-invalidates-credentials-already-issued-and-recreating-it-by-name-does-not-restore-them`.

The remaining fourteen polling checks each establish a positive control and then poll for a refusal; they complete once the first cause's remaining half lands at the services those checks probe, which is why they are carried here rather than closed.

## Evidence
From the parent report's replay (the behavioral suite cannot serve this issue's subset for a fresh replay — two `get-delegated-access-token` units are not listed in it — so the divergences below are the parent's, re-checked against the code as it stands):

* "a federated session with no session policy must have no permissions at all, got code='<call succeeded>'"; "expected the policy-less federation token to be refused, but the call SUCCEEDED and returned {… 'Buckets': [], 'Owner': {…}}" (HTTP 200).
* "expected a refusal, but the call succeeded: {'Users': [], …, 'HTTPStatusCode': 200}" (IAM `ListUsers` under a federation token).
* "aws:PrincipalType is wrong for an AssumeRole session: … got {'AssumedRole': '<call succeeded>', 'FederatedUser': '<call succeeded>', 'User': '<call succeeded>'}".
* "Error: Check timed out after 60.0s" for the fifteen polling checks.

## Steps to Reproduce
1. Boot the emulator and replay the check ids in trace_ids.
2. `sts.get_federation_token(Name='probe', DurationSeconds=900)` with NO `Policy`, then `s3.list_buckets` and `iam.list_users` with the returned credentials — the emulator answers 200 where AWS answers AccessDenied/403.
3. `sts.assume_role(...)`, then `iam.delete_role(RoleName=...)`, then any call with the session's credentials — the emulator keeps serving them.
