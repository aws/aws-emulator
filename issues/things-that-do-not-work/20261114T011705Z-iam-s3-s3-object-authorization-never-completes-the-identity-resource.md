Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__s3.resource-policy-alone-authorizes-a-principal-with-no-identity-policy
Issue-Key: iam__s3.resource-policy-alone-authorizes-a-principal-with-no-identity-policy|_poll#20|Timeout|time.sleep(interval_s)
Issue-Commit: 7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69

# S3 object authorization never completes the identity/resource-policy union legs: the bucket-policy-alone GetObject check never converges and the run is killed at the 60 s budget

> _Note: this issue was identified against the code base at commit `7eb3f68345fb0e1e7b2dcd6e02146b3b6c63cb69`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetObject

## Description
One net-new root cause in the S3 object-read authorization path.

Cause 1 of 1: a same-account S3 object read whose ONLY grant is the bucket policy never becomes authorized, so the check's convergence poll never satisfies its predicate and the check is killed by the runner's time budget with no leg completing.

AWS: recorded behavior only for the union rule's sentence; the behavior itself is AWS's documented same-account evaluation rule — within one account, identity-based and resource-based policies UNION, so an IAM user with NO s3:GetObject grant in any identity policy CAN read an object when the bucket policy allows `s3:GetObject` on that object ARN for that user's ARN as Principal, and the read returns the object's exact bytes with HTTP 200. The recording's other legs state the matching refusal shape: once the bucket policy is deleted and no identity policy grants the action, the identical signed GetObject is refused HTTP 403 with Error.Code `AccessDenied` and Error.Message `User: <user arn> is not authorized to perform: s3:GetObject on resource: "<object arn>" because no identity-based policy allows the s3:GetObject action` — fixed text around three values taken from the request and state: the caller's IAM user ARN, the object ARN, and the action name. Adding an identity-policy grant alone (with no bucket policy present) must authorize the read again.

Emulator: the check never gets past its convergence polls. It is terminated by the runner with `Check timed out after 60.0s` and no AWS response is recorded for any leg, where every other S3-plus-IAM check in this service group completes well inside that budget (iam__s3 is 17/21). Hypothesis (the replay records only the timeout, so the attribution is a hypothesis and not a recorded response): the non-converging poll is the bucket-policy-alone leg. That leg loops `GetObject` with the minted user's key every 5 s until the bytes come back, and its own failure text names this exact emulator shape — "an emulator that consults only identity policies is refused here". An authorizer that evaluates only the caller's identity policies returns AccessDenied forever on that leg, so the predicate can never be satisfied and the poll burns the whole budget instead of raising its own message. The two other candidates in the same check are the preceding credential-health poll (a freshly minted IAM user access key signing s3:ListBuckets under an s3:ListAllMyBuckets grant never succeeding) and the identity-policy-alone leg; a fixer should re-replay this check and read which poll is in flight when the budget expires.

Why: a client that relies on the same-account union — the extremely common pattern of granting object access from the bucket policy and keeping the principal's identity policy empty, used by cross-team bucket sharing, data-lake bucket policies and every "grant read via bucket policy" runbook — gets its reads refused with AccessDenied against this emulator even though the policy it just wrote names the principal and the object. Worse, here it does not even get a refusal to act on: the call hangs in retry until the caller's own deadline, so the failure looks like an unavailable service rather than an authorization decision, and the AccessDenied branch a client would handle is never reached.

Fix: the S3 GetObject authorization path — the object-read authorizer must evaluate the bucket's resource policy together with the caller's identity policies and permit the read when either side allows it for a same-account principal (and must refuse with the AccessDenied message above, naming the caller's user ARN, the object ARN and s3:GetObject, when neither does). If the non-converging poll turns out to be the credential-health control instead, the defect is in the signing/credential path that resolves a freshly minted IAM user access key, not in the union rule.

Traces: iam__s3.resource-policy-alone-authorizes-a-principal-with-no-identity-policy

## Evidence
Check iam__s3.resource-policy-alone-authorizes-a-principal-with-no-identity-policy [mismatch].

Replay output, verbatim and complete:
  Error: Check timed out after 60.0s

No emulator response is recorded: the check is killed before any leg's assertion runs, so the divergence evidenced here is the non-convergence itself. Context from the check source that bounds the attribution:
  - line 114761: `_poll(lambda: _outcome(lambda: as_user.list_buckets()), lambda o: o == "OK", timeout_s=240, interval_s=5, ...)` — credential-health control for the minted key.
  - line 114807: `_poll(lambda: _read_body(as_user, bucket, key_name), lambda b: b == _BODY, timeout_s=240, interval_s=5, description="the bucket policy alone authorizes GetObject and the real bytes come back")` — the union leg; its assertion text reads "an emulator that consults only identity policies is refused here".
  - line 114871: the identity-policy-alone leg, same 240 s poll shape.
Each poll's own deadline (240 s) exceeds the runner's 60 s budget, so a poll whose predicate is never satisfied ends as a flat timeout rather than as the poll's own "did not happen within 240s" message.

Expected (AWS), for the leg this is attributed to: with bucket policy {Effect: Allow, Principal: {AWS: <user arn>}, Action: "s3:GetObject", Resource: <object arn>} in place and the user holding ONLY an s3:ListAllMyBuckets inline policy, the signed GetObject returns HTTP 200 and the exact bytes that were PUT.
Expected refusal shape for the following leg (bucket policy deleted, no identity grant): HTTP 403, Error.Code "AccessDenied", Error.Message "User: <user arn> is not authorized to perform: s3:GetObject on resource: \"<object arn>\" because no identity-based policy allows the s3:GetObject action" — fixed text with the user ARN, object ARN and action taken from the request.

That 17 of 21 iam__s3 checks pass inside the same budget is what rules out a generally slow emulator as the explanation.

## Steps to Reproduce
1. s3:CreateBucket <bucket> in the runner's region and s3:PutObject <bucket>/sim2real/union/payload.bin with a known body.
2. iam:CreateUser <user>; iam:PutUserPolicy giving it ONLY {Effect: Allow, Action: "s3:ListAllMyBuckets", Resource: "arn:aws:s3:::*"} (this grant can never overlap an object, so it is only a credential-health control).
3. iam:CreateAccessKey for <user> and build an S3 client signing with that key.
4. Credential-health control: call s3:ListBuckets with that key and confirm it succeeds. If this never succeeds, the defect is in the minted-key credential path.
5. Baseline: s3:GetObject(<bucket>, <key>) with that key -> EXPECT 403 AccessDenied with "User: <user arn> is not authorized to perform: s3:GetObject on resource: \"<object arn>\" because no identity-based policy allows the s3:GetObject action".
6. s3:PutBucketPolicy with {Version: "2012-10-17", Statement: [{Sid: "ResourcePolicyOnly", Effect: "Allow", Principal: {AWS: <user arn>}, Action: "s3:GetObject", Resource: <object arn>}]}; confirm iam:ListUserPolicies still reports only the ListAllMyBuckets policy.
7. s3:GetObject(<bucket>, <key>) with the same key. EXPECT HTTP 200 and the exact body from step 1; OBSERVED the call never succeeds — the retry loop around it never converges and the run is killed at the 60 s budget.
8. s3:DeleteBucketPolicy and repeat step 5's refusal; then iam:PutUserPolicy granting s3:GetObject on <object arn> with no bucket policy present and confirm the read is authorized again.
