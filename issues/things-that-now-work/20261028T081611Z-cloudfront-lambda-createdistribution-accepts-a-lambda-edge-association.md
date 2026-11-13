Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected
Issue-Key: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected|_error_of#25|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Commit: 51acfe3f2ac52bf94dbabe00a764942f56e02eae

# cloudfront__lambda: CreateDistribution accepts a Lambda@Edge association whose function execution role does not trust edgelambda.amazonaws.com, instead of refusing it with InvalidLambdaFunctionAssociation

> _Note: this issue was identified against the code base at commit `51acfe3f2ac52bf94dbabe00a764942f56e02eae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
This issue carries ONE net-new root cause in the cloudfront Lambda@Edge association path.

Cause 1 of 1: CreateDistribution does not validate the TRUST POLICY of the execution role behind a LambdaFunctionAssociation, so a distribution naming a correctly published function version whose role trusts only lambda.amazonaws.com is created instead of refused.

AWS: Smithy: CreateDistribution -> POST /2020-05-31/distribution, modelled error InvalidLambdaFunctionAssociation (errorKind client, httpError 400, doc "The specified Lambda@Edge function association is invalid."); CacheBehavior/DefaultCacheBehavior carry LambdaFunctionAssociations whose LambdaFunctionAssociation.LambdaFunctionARN is @required and documented "You must specify the ARN of a function version; you can't specify an alias or $LATEST." The model does not spell out the trust-policy rule, so for that part: recorded behavior only. What the recording requires is that a Lambda@Edge association is refused with HTTP 400, error code InvalidLambdaFunctionAssociation, and the message "The function execution role must be assumable with edgelambda.amazonaws.com as well as lambda.amazonaws.com principals. Update the IAM role and try again. Role: <roleArn>". In that message everything up to and including "Role: " is FIXED text; the only variable part is <roleArn>, the ARN of the execution role of the function named by LambdaFunctionARN, taken from the emulator's own IAM state — never a literal from one recording. AWS also leaves NO distribution behind for that refusal, and accepts the SAME version ARN once the role's AssumeRolePolicyDocument is updated to trust both lambda.amazonaws.com and edgelambda.amazonaws.com.

Emulator: the call SUCCEEDS. CreateDistribution returns a Distribution (Status "InProgress", a freshly minted Id/ARN/DomainName) with the DefaultCacheBehavior.LambdaFunctionAssociations entry stored as sent, so no refusal is produced at all and a distribution IS left behind. The emulator does reject the sibling case correctly (the $LATEST/unqualified-ARN beat in the same check passes), which localises the gap to the trust-policy condition specifically, not to association validation as a whole.

Why: edgelambda.amazonaws.com trust is the thing that makes a function invocable by the CloudFront edge. A client that validates its stack against this emulator sees the association accepted and believes the trigger will fire; against real AWS the same CreateDistribution is refused with a 400 whose message tells it exactly which role to fix. The emulator therefore turns a deterministic, self-describing deployment failure into a distribution that is created but whose edge trigger can never run — and because the refusal is absent, the "no distribution left behind" guarantee is broken too, leaking a resource on a path AWS never creates one on.

Fix: the CreateDistribution handler's Lambda@Edge association validation in the cloudfront crate — the same validation step that already implements the unqualified-ARN/$LATEST rule must additionally resolve the referenced function's execution role, inspect its assume-role trust policy for an sts:AssumeRole statement admitting edgelambda.amazonaws.com alongside lambda.amazonaws.com, and on failure emit the 400 InvalidLambdaFunctionAssociation above WITHOUT persisting any distribution. UpdateDistribution shares this validation and must be gated the same way.

Traces: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected.

## Evidence
Cause 1 — cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected [mismatch]:
  Error: File "check_cloudfront__lambda_xsvc_cloudfront_2_lambda_edge_unpublished_version_rejected.py", line 287, in run
    no_trust = _error_of(lambda: create_dist(version_arn, "notrust"))
  File "...", line 96, in _error_of
    raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
  AssertionError: expected the call to be rejected, but it succeeded: {'Id': ..., 'ARN': 'arn:aws:cloudfront::<account>:distribution/<id>', 'Status': 'InProgress', ..., 'DistributionConfig': {..., 'DefaultCacheBehavior': {..., 'LambdaFunctionAssociations': ...}}}

Expected (the check's want_no_trust tuple): (400, 'InvalidLambdaFunctionAssociation', 'The function execution role must be assumable with edgelambda.amazonaws.com as well as lambda.amazonaws.com principals. Update the IAM role and try again. Role: <roleArn>').
Actual: HTTP 200/201 with a created Distribution; no error raised; a distribution is left behind where AWS creates none.

Setup that makes this unambiguous: the check creates the IAM role with a trust policy naming ONLY lambda.amazonaws.com, creates the function in us-east-1, waits for State == Active, and calls PublishVersion so the association names a genuine numbered version ARN (the ARN rule is satisfied). The immediately preceding beat of the same check — associating <functionArn>:$LATEST — IS refused correctly by the emulator, so the only broken condition is the role's trust policy. The generated distribution id, ARN, domain name and timestamps differ per run and are not requirements; the status, error code and message template are.

## Steps to Reproduce
1. IAM CreateRole with AssumeRolePolicyDocument = {"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"lambda.amazonaws.com"},"Action":"sts:AssumeRole"}]} — i.e. edgelambda.amazonaws.com deliberately NOT trusted.
2. Lambda CreateFunction in us-east-1 with that role (Publish=false); poll GetFunctionConfiguration until State == Active; PublishVersion to mint version "1"; let version_arn = "<functionArn>:1".
3. CloudFront (us-east-1 endpoint) CreateDistribution with one custom origin and DefaultCacheBehavior { CachePolicyId = 4135ea2d-6df8-44a3-9df3-4b5a84be39ad, LambdaFunctionAssociations = { Quantity: 1, Items: [{ LambdaFunctionARN: version_arn, EventType: "viewer-request", IncludeBody: false }] } }.
4. Expect HTTP 400, Error.Code "InvalidLambdaFunctionAssociation", Error.Message "The function execution role must be assumable with edgelambda.amazonaws.com as well as lambda.amazonaws.com principals. Update the IAM role and try again. Role: <the role ARN from step 1>", and NO distribution created (ListDistributions shows none with that Comment).
5. Observe: the call succeeds and returns a Distribution with the association stored.
6. Control that must keep working: IAM UpdateAssumeRolePolicy to trust both lambda.amazonaws.com and edgelambda.amazonaws.com, then re-issue step 3 with the SAME version_arn — it must be accepted and GetDistributionConfig must echo (version_arn, "viewer-request").

Re-replay cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected to confirm.
