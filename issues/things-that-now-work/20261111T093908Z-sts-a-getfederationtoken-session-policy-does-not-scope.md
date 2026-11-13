Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.federation-token-session-policy-scope-and-expiration-at-requested-duration
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.federation-token-session-policy-scope-and-expiration-at-requested-duration|_client_error#19|AssertionError|raise AssertionError("the call succeeded where AWS refuses it")
Issue-Commit: 1dbd6c9a628cf270d6e2528da937a9703bd8df7f

# sts: a GetFederationToken session policy does not scope the minted credentials — s3:CreateBucket, absent from the session policy, is served 200 instead of AccessDenied

> _Note: this issue was identified against the code base at commit `1dbd6c9a628cf270d6e2528da937a9703bd8df7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFederationToken

## Description
Cause 1 of 1: the inline session policy passed to GetFederationToken has no effect on what the credentials it mints are allowed to do; a call outside the session policy is served instead of refused.

AWS: Smithy (com.amazonaws.sts#GetFederationToken) models Policy as sessionPolicyDocumentType (@length 1..2048, @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]+$) and documents it as an inline *session policy*: "When you pass session policies, the session permissions are the intersection of the IAM user policies and the session policies that you pass ... You cannot use session policies to grant more permissions than those that are defined in the permissions policy of the IAM user", and "if you do not pass any session policies, then the resulting federated user session has no permissions." So a federation session whose inline policy allows only s3:ListAllMyBuckets may list buckets, and any other action — even one the underlying IAM user's own policy allows — falls outside the intersection and is refused by the called service with HTTP 403 and error code AccessDenied. For S3 CreateBucket the recorded refusal message is `User: <FederatedUser.Arn> is not authorized to perform: s3:CreateBucket on resource: "<bucket arn>" because no session policy allows the s3:CreateBucket action`; its fixed text is `User: `, ` is not authorized to perform: `, ` on resource: `, and ` because no session policy allows the ` / ` action`, while the principal ARN (the FederatedUser ARN STS returned, arn:aws:sts::<account>:federated-user/<Name>), the action name and the quoted resource ARN (arn:aws:s3:::<bucket> for CreateBucket) are values taken from the request and the emulator's state — never the literal account, user name or bucket name of one recording. Recorded behavior only for the exact message text; the intersection rule itself is modelled/documented as quoted above.

Emulator: the mint itself is faithful on the legs the check reaches first — Expiration is issuance + the requested DurationSeconds (900 s) within tolerance, FederatedUser.Arn ends in federated-user/<Name>, and the allowed s3:ListAllMyBuckets succeeds — but the very next call, s3:CreateBucket, which the session policy does NOT allow, SUCCEEDS (2xx, bucket created) where AWS refuses it. The replay fails at the refusal helper with `AssertionError: the call succeeded where AWS refuses it` (check_sts_federation_token_session_policy_scope_and_expiration_at_requested_duration.py line 163 -> _client_error line 30). Because the call succeeded, the subsequent assertions on the AccessDenied code and message, on the absence of the bucket, and on the DurationSeconds=899 ValidationError refusal were never exercised by this replay and are not claimed here.

Why: a client that uses GetFederationToken to hand out deliberately down-scoped credentials gets credentials with the full permissions of the calling IAM user. Every negative test of a least-privilege federation flow passes against the emulator and fails against AWS, and a mutating call a caller believes is impossible (here: creating a bucket) actually changes state, so the emulator is overly permissive in exactly the direction that hides a privilege escalation.

Fix: the session's SCOPE must be recorded where GetFederationToken mints the credential (the GetFederationToken handler and the STS crate's session state) and read by the service that later ACCEPTS the credential, so that service can evaluate (action, resource) against the intersection of the IAM user's identity policies and the session policy before serving the request; S3's request authorization is the enforcement site this check exercises. Related but NOT covering this check: open issue 20261029T120000Z-sts-3-of-the-8-session-scoping-causes-remain-open.md (its cause 1 of 8 describes the same missing session-scope directory, but does not carry this check's replay key), and open issue 20261024T065715Z-iam-s3-sts-s3-requests-under-a-scoped-session-are-never.md.

Traces: sts.federation-token-session-policy-scope-and-expiration-at-requested-duration.

## Evidence
Check sts.federation-token-session-policy-scope-and-expiration-at-requested-duration [mismatch].

Expected (AWS): with a GetFederationToken inline session policy of {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]}, a CreateBucket call with those credentials is refused HTTP 403 / Code=AccessDenied with Message `User: <FederatedUser.Arn> is not authorized to perform: s3:CreateBucket on resource: "arn:aws:s3:::<bucket>" because no session policy allows the s3:CreateBucket action`, and the bucket does not appear in the account's ListBuckets afterwards.

Actual (emulator): CreateBucket succeeds. Replay output:
  File "check_sts_federation_token_session_policy_scope_and_expiration_at_requested_duration.py", line 163, in run
    code, message = _client_error(lambda: fed_s3.create_bucket(**create_kwargs))
  File "check_sts_federation_token_session_policy_scope_and_expiration_at_requested_duration.py", line 30, in _client_error
    raise AssertionError("the call succeeded where AWS refuses it")
  AssertionError: the call succeeded where AWS refuses it

Passing legs that precede the divergence (so the mint itself is not the defect): Expiration == response Date + 900 s within 120 s, FederatedUser.Arn ends in federated-user/<Name>, and the session-policy-allowed s3:ListAllMyBuckets succeeded.

## Steps to Reproduce
1. Boot the emulator and replay sts.federation-token-session-policy-scope-and-expiration-at-requested-duration.
2. Or by hand: iam.create_user + iam.put_user_policy granting Allow on [sts:GetFederationToken, s3:ListAllMyBuckets, s3:CreateBucket] on "*", then iam.create_access_key for that user.
3. With that user's long-term key: sts.get_federation_token(Name=<<=32 chars>, DurationSeconds=900, Policy=json.dumps({"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]})).
4. Build an S3 client from the returned Credentials (AccessKeyId/SecretAccessKey/SessionToken). s3.list_buckets() succeeds — correct, the session policy allows it.
5. Call s3.create_bucket(Bucket=<new name>[, CreateBucketConfiguration={"LocationConstraint": <region>} outside us-east-1]). The emulator creates the bucket and returns 2xx; AWS refuses with 403 / AccessDenied and the "because no session policy allows the s3:CreateBucket action" message, and the bucket is absent from a subsequent admin list_buckets().
