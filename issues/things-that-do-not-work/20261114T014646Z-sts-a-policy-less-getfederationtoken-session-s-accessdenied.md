Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.get-federation-token-without-policy-denies-actions
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.get-federation-token-without-policy-denies-actions|run#15|AssertionError|assert message == expected_message, (
Issue-Commit: 927eef52ff1b7195638ad898bf452ad5f9c2d613

# sts: a policy-less GetFederationToken session's AccessDenied says "no identity-based policy allows" where AWS says "no session policy allows"

> _Note: this issue was identified against the code base at commit `927eef52ff1b7195638ad898bf452ad5f9c2d613`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFederationToken

## Description
Cause 1 of 1: when GetFederationToken is called WITHOUT a Policy, the resulting federated-user session correctly has no permissions and the emulator correctly refuses s3:ListAllMyBuckets with HTTP 403 / AccessDenied, but the refusal message attributes the implicit deny to the wrong policy type: it ends `because no identity-based policy allows the s3:ListAllMyBuckets action` where AWS ends it `because no session policy allows the s3:ListAllMyBuckets action`. Failing check: sts.get-federation-token-without-policy-denies-actions.

AWS: Smithy com.amazonaws.sts#GetFederationTokenRequest$Policy — "This parameter is optional. However, if you do not pass any session policies, then the resulting federated user session has no permissions." The withheld permission therefore comes from the SESSION-policy boundary, not from the principal's identity policies (the calling IAM user's own policy does allow s3:ListAllMyBuckets — the check proves it with a positive control before minting), so the called service words the implicit deny with the session-policy clause. Recorded behavior only for the exact text:
  User: <FederatedUser.Arn> is not authorized to perform: <service>:<Action> because no session policy allows the <service>:<Action> action
Fixed text: `User: `, ` is not authorized to perform: `, ` because no session policy allows the `, ` action`. Variable parts taken from the request and emulator state: the principal ARN is exactly the FederatedUser.Arn GetFederationToken returned (arn:aws:sts::<account>:federated-user/<Name>) and the action is the action attempted. No account id or generated federation name from a single run is part of the requirement. HTTP status 403 and code AccessDenied are already correct.

Emulator: status 403, code AccessDenied and the principal ARN and action in the message are all correct; only the trailing clause diverges — `because no identity-based policy allows the s3:ListAllMyBuckets action`.

Why: that clause is the only part of an AWS implicit-deny message that tells a caller WHICH policy withheld the action. An operator debugging a federation broker reads "no identity-based policy allows ..." and widens the IAM user's identity policy, which changes nothing — the boundary is the (absent) session policy. Suites and log scrapers that match AWS's documented distinction between identity-based, session and resource-based implicit denies never match.

Fix: the authorization decision in the request-authorization path of the service that accepts the federation credential (S3 here) must record that the deciding boundary was the credential's session scope, and its AccessDenied message construction must then select the session-policy clause; the scope itself is written by the GetFederationToken handler onto the minted session in the STS crate's session state, including the empty-scope (no Policy passed) case.

Related but NOT covering this check: open issue 20261113T073604Z-sts-a-getfederationtoken-session-policy-refusal-is-attributed.md describes the same wrong clause for an action absent from a SUPPLIED session policy, and does not carry this check's replay key (this one is the no-Policy-at-all path, and its message has no `on resource: "..."` segment).

Traces: sts.get-federation-token-without-policy-denies-actions.

## Evidence
Check sts.get-federation-token-without-policy-denies-actions [mismatch].

Expected (AWS): after resp = sts.get_federation_token(Name=<name>, DurationSeconds=900) with NO Policy, an S3 client built from resp["Credentials"] calling list_buckets() is refused HTTP 403 / Code=AccessDenied with Message:
  User: <FederatedUser.Arn> is not authorized to perform: s3:ListAllMyBuckets because no session policy allows the s3:ListAllMyBuckets action

Actual (emulator): HTTP 403 / Code=AccessDenied (both correct) with Message:
  User: <FederatedUser.Arn> is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action

Replay output:
  File "check_sts_get_federation_token_without_policy_denies_actions.py", line 112, in run
    assert message == expected_message
  AssertionError: expected the refusal message 'User: arn:aws:sts::000000000000:federated-user/sim2real-behav-fed-7dbdf3edbd13 is not authorized to perform: s3:ListAllMyBuckets because no session policy allows the s3:ListAllMyBuckets action', got 'User: arn:aws:sts::000000000000:federated-user/sim2real-behav-fed-7dbdf3edbd13 is not authorized to perform: s3:ListAllMyBuckets because no identity-based policy allows the s3:ListAllMyBuckets action'

(The account id and federated-user name above are per-run values that fill the template; only the clause ` because no session policy allows the ` / ` action` is the diverging requirement.)

Preceding legs that pass: the ambient caller's own list_buckets() succeeds (positive control, so the denial is attributable to the missing session policy, and "no identity-based policy allows" is factually false for this principal's identity policies), the mint returns Credentials and a FederatedUser.Arn, and the error code stabilises past credential-propagation transients at AccessDenied.

## Steps to Reproduce
1. Boot the emulator and replay sts.get-federation-token-without-policy-denies-actions.
2. Or by hand, with long-term IAM user (or root) credentials whose identity policy allows sts:GetFederationToken and s3:ListAllMyBuckets: call s3.list_buckets() with those credentials — it succeeds (positive control).
3. resp = sts.get_federation_token(Name=<<=32 chars>, DurationSeconds=900) with NO Policy argument; note resp["FederatedUser"]["Arn"].
4. Build an S3 client from resp["Credentials"] (AccessKeyId / SecretAccessKey / SessionToken) and call list_buckets(), retrying InvalidClientTokenId/InvalidAccessKeyId propagation transients.
5. It is refused 403 / AccessDenied — correct — but Error.Message ends `because no identity-based policy allows the s3:ListAllMyBuckets action`. AWS ends it `because no session policy allows the s3:ListAllMyBuckets action`, with the same principal ARN (the FederatedUser ARN) and action.
