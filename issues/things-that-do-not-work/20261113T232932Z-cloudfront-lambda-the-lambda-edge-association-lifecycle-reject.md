Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected
Issue-Key: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected|_poll_until#42|Timeout|time.sleep(interval_s)
Issue-Commit: 9821cff84aed55823f685374d459e562c1c72f3b

# cloudfront__lambda: the Lambda@Edge association lifecycle (reject $LATEST, reject a role without edgelambda trust, then serve) makes no observable progress

> _Note: this issue was identified against the code base at commit `9821cff84aed55823f685374d459e562c1c72f3b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDistribution

## Description
Cause 1 of 1: the Lambda@Edge viewer-request association trajectory stalls — none of its four modeled refusals or its edge response was observable before the run budget expired.
AWS: Smithy (cloudfront 2020-05-31) models InvalidLambdaFunctionAssociation as a client error with httpError 400 and a single Message member; recorded behavior gives the two messages for this class. (a) A LambdaFunctionAssociations item whose LambdaFunctionARN ends in :$LATEST is refused 400 InvalidLambdaFunctionAssociation with "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: <the ARN from the request>" — the two sentences are fixed text, the ARN is the value from the request. (b) A genuinely published numbered version whose execution role is not assumable by edgelambda.amazonaws.com is refused 400 InvalidLambdaFunctionAssociation with "The function execution role must be assumable with edgelambda.amazonaws.com as well as lambda.amazonaws.com principals. Update the IAM role and try again. Role: <the role ARN from the function's configuration>" — again fixed text plus the role ARN from state. Neither refusal leaves a distribution behind (ListDistributions shows none with the run's Comment). Once UpdateAssumeRolePolicy adds the edgelambda principal, the SAME version ARN is accepted, GetDistributionConfig reads the association back as (version ARN, "viewer-request"), and after the distribution is Deployed the viewer-request function generates the response itself: 200, the x-sim2real-edge header carrying the run's marker, and a body of "<marker> <request uri>" for every path.
Emulator: the check makes no observable progress and the runner ends it with "Check timed out after 60.0s" — no refusal status, no error code, no edge response was recorded. The stall is inside this sequence (role propagation poll on CreateFunction, PublishVersion, the two CreateDistribution refusals, the accept-after-repair poll, the wait-for-Deployed, or the edge fetch); the sibling checks in this service that DID produce output are owned by 20261111T081112Z-cloudfront-lambda-a-cloudfront-distribution-s-edge-domain-never-serves.md and 20261103T062355Z-cloudfront-lambda-a-deployed-distribution-s-edge-domain-3d1d10.md, neither of which carries this check's replay key, so it is filed here.
Why: a client cannot validate a Lambda@Edge association at all — neither of the two refusals it must program against is observable, and the accepted-and-serving path produces nothing. A caller gets a hang rather than a 400 with a code it can branch on.
Fix: CreateDistribution's validation of DefaultCacheBehavior.LambdaFunctionAssociations (the $LATEST/unversioned-ARN check and the execution-role edgelambda-trust check, both raising InvalidLambdaFunctionAssociation with the messages above and creating no distribution), and the edge data plane that must run an associated viewer-request function and return its generated response.
Traces: cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected.

## Evidence
cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected:
  Error: Check timed out after 60.0s

Expected, in order:
  1. create_distribution with LambdaFunctionARN "<function arn>:$LATEST" on viewer-request -> (400, "InvalidLambdaFunctionAssociation", "The function ARN must reference a specific function version. (The ARN must end with the version number.) ARN: <function arn>:$LATEST").
  2. create_distribution with the published numbered version ARN while the execution role trusts only lambda.amazonaws.com -> (400, "InvalidLambdaFunctionAssociation", "The function execution role must be assumable with edgelambda.amazonaws.com as well as lambda.amazonaws.com principals. Update the IAM role and try again. Role: <role arn>").
  3. no distribution with the run's Comment exists after either refusal.
  4. after update_assume_role_policy adds edgelambda.amazonaws.com, the same version ARN is accepted and GetDistributionConfig reads back [(version ARN, "viewer-request")].
  5. once Deployed, GET <edge domain>/sim2real/first -> 200, x-sim2real-edge: <marker>, body "<marker> /sim2real/first".
Actual: none of these were reached — the run budget expired with no response recorded.

## Steps to Reproduce
1. Point cloudfront, lambda, iam and sts clients at the emulator with the CloudFront control plane and the Lambda function in us-east-1.
2. CreateRole with a trust policy allowing only lambda.amazonaws.com to sts:AssumeRole; CreateFunction (python handler that generates a viewer response, Publish=False) and wait for State Active.
3. PublishVersion to mint a numbered version.
4. create_distribution with a DefaultCacheBehavior whose LambdaFunctionAssociations has one viewer-request item naming "<function arn>:$LATEST" — expect 400 InvalidLambdaFunctionAssociation with the "must reference a specific function version" message.
5. Repeat with the numbered version ARN — expect 400 InvalidLambdaFunctionAssociation with the edgelambda-trust message.
6. UpdateAssumeRolePolicy to add edgelambda.amazonaws.com, retry the create, wait for Deployed, then GET <edge domain>/sim2real/first.
7. Observe the run never reaches any of these outcomes. Or replay cloudfront__lambda.xsvc-cloudfront-2-lambda-edge-unpublished-version-rejected.
