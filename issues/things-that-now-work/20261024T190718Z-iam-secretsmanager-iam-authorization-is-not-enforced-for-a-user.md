Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol
Issue-Key: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol|_error_code_and_status#12|AssertionError|raise AssertionError(f"expected the call to be denied, but it SUCCEEDED: {result!r}")
Issue-Commit: ee21bdc0559bb252101e2375a2c132d26bd8f920

# IAM authorization is not enforced for a user's own access key: an unauthorized ListUsers succeeds instead of AccessDenied 403

> _Note: this issue was identified against the code base at commit `ee21bdc0559bb252101e2375a2c132d26bd8f920`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListUsers

## Description
One root cause. Cause count: 1.

=== Cause 1 of 1: calls signed with a no-policy IAM user's access key are not authorized at all ===
AWS: a freshly created IAM user with no attached policy may sign requests, but every authorized action is refused. IAM's awsQuery protocol answers HTTP 403 with code AccessDenied and the documented message 'User: <user arn> is not authorized to perform: iam:ListUsers on resource: <resource> because no identity-based policy allows the iam:ListUsers action'; Secrets Manager's awsJson1_1 protocol answers the identical rejection at HTTP 400 with code AccessDeniedException and the analogous 'not authorized to perform: secretsmanager:ListSecrets ... because no identity-based policy allows ...' message. The status differs by protocol, not by principal - that relationship is the claim under test.
Emulator: the call made with that user's access key SUCCEEDS - ListUsers returns 200 with the user list ('expected the call to be denied, but it SUCCEEDED: {Users: [...], HTTPStatusCode: 200}'), so neither the 403/AccessDenied envelope nor the Secrets Manager 400/AccessDeniedException counterpart is ever produced.
Why: any test that proves a principal CANNOT do something passes vacuously. Least-privilege reviews, break-glass drills and negative IAM tests all go green against an emulator where credentials carry full authority, and the client cannot distinguish 'allowed' from 'unauthorized but unchecked'.
Fix: the request-authorization layer shared by the services - resolve the signing access key to its principal, evaluate that principal's identity-based policies, and refuse with the protocol's denial envelope (awsQuery: 403 AccessDenied; awsJson1_1: 400 AccessDeniedException).
Traces: iam__secretsmanager.xsvc-iam-2-secretsmanager-access-denied-status-code-differs-by-protocol.

## Evidence
'AssertionError: expected the call to be denied, but it SUCCEEDED: {Users: [{Path: /, UserName: sim2real-xsvc-iamsm-denied-<id>, ...}], IsTruncated: False, ResponseMetadata: {HTTPStatusCode: 200, ...}}'. The positive controls (the account's own credentials calling iam ListUsers and secretsmanager ListSecrets) succeeded first, so the denial under test is principal-specific and not an account-wide effect.

## Steps to Reproduce
iam CreateUser(UserName=<u>); iam CreateAccessKey(UserName=<u>); build an iam client and a secretsmanager client from that access key; call iam ListUsers(MaxItems=1) and secretsmanager ListSecrets(MaxResults=1) with them - both return 200 instead of AccessDenied (403, awsQuery) and AccessDeniedException (400, awsJson1_1).
