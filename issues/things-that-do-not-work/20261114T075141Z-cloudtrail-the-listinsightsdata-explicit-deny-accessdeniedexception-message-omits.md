Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail.list-insights-data-permission-denied
Issue-Shape-Id: com.amazonaws.cloudtrail#CloudTrail_20131101
Issue-Key: cloudtrail.list-insights-data-permission-denied|run#12|AssertionError|assert message == (
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudtrail: the ListInsightsData explicit-deny AccessDeniedException message omits the "on resource: <arn>" clause

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInsightsData

## Description
One net-new root cause in CloudTrail's authorization-refusal message rendering.

Cause 1 of 1: the AccessDeniedException raised when an assumed-role session's identity-based policy carries an explicit Deny for cloudtrail:ListInsightsData is worded without the resource clause. Failing check ids: cloudtrail.list-insights-data-permission-denied.

AWS: recorded behavior only for the message text — the CloudTrail Smithy model (ListInsightsData: input InsightSource @required, target ResourceArn, @length 3..256, @pattern ^[a-zA-Z0-9._/\-:]+$; DataType @required; modelled errors InvalidParameterException, OperationNotPermittedException, UnsupportedOperationException) models no AccessDeniedException at all, so the refusal comes from the common IAM authorization layer. AWS answers the explicit-deny case with HTTP 400 and error code AccessDeniedException, and the IAM explicit-deny sentence is:

  "User: <principal-arn> is not authorized to perform: <service>:<Action> on resource: <resource-arn> with an explicit deny in an identity-based policy"

Fixed text of that template: "User: ", " is not authorized to perform: ", " on resource: ", " with an explicit deny in an identity-based policy". Variable parts taken from the request and from state: the caller's principal ARN (for an STS session, arn:aws:sts::<account>:assumed-role/<role-name>/<session-name>, where account is the caller's account, role-name the assumed role and session-name the RoleSessionName the caller passed), the action name (service prefix plus the operation, here cloudtrail:ListInsightsData), and the resource ARN — for this operation the InsightSource value supplied in the request.

Emulator: it does evaluate the Deny and returns the right code (AccessDeniedException), but the message stops after the action and jumps straight to the deny clause, dropping the " on resource: <InsightSource>" segment entirely:

  "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: cloudtrail:ListInsightsData with an explicit deny in an identity-based policy"

(the principal ARN and action render correctly; only the resource clause is missing). The check's assertion on the full expected sentence therefore fails.

Why: callers and operators parse this sentence to learn WHICH resource the deny applied to — IAM troubleshooting runbooks, policy-simulator-style tooling and log scrapers extract the "on resource: <arn>" substring to pin the denied target. With the clause absent, a client cannot tell from the error which InsightSource (which trail or event data store) was refused, and any assertion or regex written against AWS's wording fails even though the authorization decision itself was correct. Code-only matching hides the defect; anything reading the message breaks.

Fix: the shared IAM authorization-denial message formatter used by the CloudTrail handlers — the AccessDeniedException renderer must interpolate the resource ARN it evaluated the policy against (for ListInsightsData, the request's InsightSource) into the " on resource: " slot, for both the explicit-deny and the implicit-deny wordings, rather than emitting the action followed directly by the deny clause.

Traces: cloudtrail.list-insights-data-permission-denied — the check creates a minimal trail, puts an ApiCallRateInsight insight selector, creates a role whose inline policy Allows cloudtrail:DescribeTrails/ListChannels and Denies cloudtrail:ListInsightsData, assumes it, calls ListInsightsData(InsightSource=<trail arn>, DataType="InsightsEvents") and asserts the full explicit-deny sentence. Not covered by any open issue: 20261023T135552Z-cloudtrail-has-no-updatetrail-concurrency-control-cloudformation-never.md reports authorization never being evaluated, whereas here authorization IS evaluated and only the message is short a clause.

## Evidence
Expected (AWS wording, variable slots filled from the request/state):
  code: AccessDeniedException
  message: "User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: cloudtrail:ListInsightsData on resource: <InsightSource arn> with an explicit deny in an identity-based policy"

Actual (emulator):
  code: AccessDeniedException (correct)
  message: "User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: cloudtrail:ListInsightsData with an explicit deny in an identity-based policy"

Replay output:
  File "check_cloudtrail_list_insights_data_permission_denied.py", line 42, in run
    assert message == (
  AssertionError: User: arn:aws:sts::000000000000:assumed-role/sim2real-ct-denied-<id>/sim2real-ct-sess-<id> is not authorized to perform: cloudtrail:ListInsightsData with an explicit deny in an identity-based policy

Delta: the substring " on resource: <InsightSource>" is absent. (Account id, role suffix and session suffix above are per-run values that fill the template; they are not part of the requirement.)

## Steps to Reproduce
1. Create an S3 bucket with the standard CloudTrail bucket policy and a trail over it (CreateTrail, IsMultiRegionTrail=False).
2. PutInsightSelectors on that trail with InsightSelectors=[{InsightType: ApiCallRateInsight, EventCategories: [Management]}].
3. CreateRole with a trust policy allowing sts:AssumeRole from the account root, then PutRolePolicy with an inline document that Allows [cloudtrail:DescribeTrails, cloudtrail:ListChannels] on * and Denies cloudtrail:ListInsightsData on *.
4. AssumeRole into that role with an explicit RoleSessionName and build a cloudtrail client from the vended credentials.
5. Call ListInsightsData(InsightSource=<the trail ARN>, DataType="InsightsEvents") with that session.
6. Observe AccessDeniedException. Its message ends "...is not authorized to perform: cloudtrail:ListInsightsData with an explicit deny in an identity-based policy", with no " on resource: <trail ARN>" clause between the action and the deny clause.
