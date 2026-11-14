Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.e2e-federation-token-policy-scopes-down-then-expires sts.get-federation-token-cannot-call-iam-denied sts.concurrent-get-federation-token-calls-produce-independent-policies
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: s3|ListBuckets|AccessDenied
Issue-Key: sts.concurrent-get-federation-token-calls-produce-independent-policies|run#5|AssertionError|assert s3_result == _SUCCEEDED, (
Issue-Commit: 927eef52ff1b7195638ad898bf452ad5f9c2d613

# sts: a GetFederationToken inline session policy's Allow is not honored — the allowed action (s3:ListAllMyBuckets) is refused with AccessDenied "no identity-based policy allows", so every federation session has no permissions

> _Note: this issue was identified against the code base at commit `927eef52ff1b7195638ad898bf452ad5f9c2d613`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFederationToken

## Description
REOPENS 20261111T093908Z-sts-a-getfederationtoken-session-policy-does-not-scope.md: its fix left the call refused.

Cause 1 of 1: credentials minted by GetFederationToken with an inline session policy that ALLOWS an action are now refused that very action. The previous state (that issue) was over-permissive — an action absent from the session policy was served 200. Enforcement now exists, but it denies the allowed action too: the session scope behaves as an empty/deny-all policy and the deciding boundary is reported as the principal's identity policies. Failing checks: sts.e2e-federation-token-policy-scopes-down-then-expires, sts.get-federation-token-cannot-call-iam-denied, sts.concurrent-get-federation-token-calls-produce-independent-policies.

AWS: Smithy com.amazonaws.sts#GetFederationTokenRequest$Policy is documented as an inline session policy: "When you pass session policies, the session permissions are the intersection of the IAM user policies and the session policies that you pass", and "if you do not pass any session policies, then the resulting federated user session has no permissions." So with a caller whose own identity policy allows s3:ListAllMyBuckets and an inline session policy of {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]}, s3:ListAllMyBuckets is INSIDE the intersection and S3 ListBuckets must succeed with HTTP 200 and a Buckets key; the same holds for an allow-all session policy ({"Effect":"Allow","Action":"*","Resource":"*"}). Only actions outside the intersection are refused (HTTP 403 / AccessDenied), and then with the session-policy clause (`... because no session policy allows the <service>:<Action> action`), never the identity-based one. Recorded behavior only for the exact refusal text; the intersection rule itself is the modelled/documented requirement quoted above.

Emulator: the mint is faithful (FederatedUser.Arn ends in federated-user/<Name>, Credentials are returned and GetCallerIdentity on them answers), but the FIRST call the session policy explicitly allows is refused: S3 ListBuckets returns HTTP 403 / Code=AccessDenied with Message `User: <FederatedUser.Arn> is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action`. Fixed text of that message: `User: `, ` is not authorized to perform: `, ` because no identity-based policy allows the `, ` action`; its variable parts (the federated-user ARN, the action) come from the request and emulator state — the account id and generated federation name in any one replay line are per-run values, not requirements. Three checks diverge the same way: sts.e2e-federation-token-policy-scopes-down-then-expires and sts.get-federation-token-cannot-call-iam-denied raise the refusal as an uncaught AccessDenied on list_buckets, and sts.concurrent-get-federation-token-calls-produce-independent-policies asserts `expected ListBuckets to succeed, got ('AccessDenied', 'User: arn:aws:sts::...:federated-user/fed-... is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action')`. Two further requirements are blocked behind this refusal and are NOT claimed here because the replays never reached them: the per-session isolation of concurrently issued session policies (a sibling's grant must not leak in, and the sibling's action must be denied with AccessDeniedException for secretsmanager:ListSecrets) and the rule that a federation session calling IAM is refused InvalidClientTokenId with "The security token included in the request is invalid".

Why: a client that uses GetFederationToken to hand out deliberately down-scoped credentials gets credentials that can do NOTHING, so the entire federation flow is unusable against the emulator — every happy-path leg of a least-privilege broker (list, read, the allow-all admin session) fails with AccessDenied that AWS serves 200. Worse, the message tells the operator the IAM user's identity policy is missing the permission, so the obvious remedy (widening the user's identity policy) cannot help: the grant that is being ignored is the session policy itself.

Fix: the session scope recorded by the GetFederationToken handler (STS crate session state) must carry the parsed inline session policy's statements and be evaluated as an INTERSECTION with the caller's identity policies by the request-authorization path of the service that accepts the credential (S3 request authorization is the enforcement site these checks exercise) — an allow present in both must authorize the request; an empty/absent session policy is the only case that denies everything. The implicit-deny message serializer in that same path must also attribute the deny to the session policy when the session scope is the deciding boundary.

Traces: sts.e2e-federation-token-policy-scopes-down-then-expires, sts.get-federation-token-cannot-call-iam-denied, sts.concurrent-get-federation-token-calls-produce-independent-policies.

## Evidence
Checks sts.e2e-federation-token-policy-scopes-down-then-expires [error], sts.get-federation-token-cannot-call-iam-denied [error], sts.concurrent-get-federation-token-calls-produce-independent-policies [mismatch].

Expected (AWS): with resp = sts.get_federation_token(Name=<name>, Policy=json.dumps({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]}), DurationSeconds=900) and an S3 client built from resp["Credentials"], s3.list_buckets() returns HTTP 200 with a Buckets key (the action is in the intersection of the IAM user's policies and the session policy). Same for an allow-all session policy.

Actual (emulator): HTTP 403 / Code=AccessDenied, Message `User: <FederatedUser.Arn> is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action`.

Replay output, sts.e2e-federation-token-policy-scopes-down-then-expires:
  botocore.errorfactory.AccessDenied: An error occurred (AccessDenied) when calling the ListBuckets operation: User: arn:aws:sts::000000000000:federated-user/sim2real-fed-7c0384379a58 is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action
(raised at the leg documented as "Allowed action succeeds": listed = fed_s3.list_buckets())

Replay output, sts.get-federation-token-cannot-call-iam-denied (session policy is Allow Action "*" Resource "*"; the leg is the control "the session is live and its policy is honored outside IAM"):
  botocore.errorfactory.AccessDenied: An error occurred (AccessDenied) when calling the ListBuckets operation: User: arn:aws:sts::000000000000:federated-user/sim2real-behav-fed-af5c8534beb6 is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action

Replay output, sts.concurrent-get-federation-token-calls-produce-independent-policies:
  File "check_sts_concurrent_get_federation_token_calls_produce_independent_policies.py", line 53, in run
    assert s3_result == _SUCCEEDED
  AssertionError: session policy granted 's3:ListAllMyBuckets': expected ListBuckets to succeed, got ('AccessDenied', 'User: arn:aws:sts::000000000000:federated-user/fed-c263b1edd9 is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action') -- a sibling call's grant must not leak in

(Account id and federation names above are per-run values filling the message template; the requirement that diverges is that an action ALLOWED by the session policy is refused at all.)

Preceding legs that pass, so the mint is not the defect: GetFederationToken returns Credentials and a FederatedUser.Arn ending in federated-user/<Name>, and GetCallerIdentity on those credentials answers (the liveness probe in sts.get-federation-token-cannot-call-iam-denied returns).

## Steps to Reproduce
1. Boot the emulator and replay sts.e2e-federation-token-policy-scopes-down-then-expires, sts.get-federation-token-cannot-call-iam-denied and sts.concurrent-get-federation-token-calls-produce-independent-policies.
2. Or by hand, with long-term IAM user (or root) credentials whose identity policy allows sts:GetFederationToken and s3:ListAllMyBuckets: confirm the ambient caller can list buckets (positive control).
3. resp = sts.get_federation_token(Name=<<=32 chars>, DurationSeconds=900, Policy=json.dumps({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]})).
4. Build an S3 client from resp["Credentials"] (AccessKeyId / SecretAccessKey / SessionToken) and call s3.list_buckets(). AWS returns 200 with Buckets; the emulator returns 403 AccessDenied "... because no identity-based policy allows the s3:ListAllMyBuckets action".
5. Repeat step 3 with Policy = Allow Action "*" Resource "*" — the allow-all session is denied the same way.
6. Issue six tokens concurrently, three with Allow s3:ListAllMyBuckets and three with Allow secretsmanager:ListSecrets, and call the granted action on each: every granted call is refused, so per-session isolation cannot be observed either.
