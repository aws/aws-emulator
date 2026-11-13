Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.e2e-idc-application-auth-token-flow
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.e2e-idc-application-auth-token-flow|_error#12|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: 5ef8df7863fe90e85315789fc6fc07107c642aff

# redshift CreateRedshiftIdcApplication accepts an IdcInstanceArn naming a nonexistent IAM Identity Center instance (HTTP 200) instead of refusing with DependentServiceAccessDenied (1 cause)

> _Note: this issue was identified against the code base at commit `5ef8df7863fe90e85315789fc6fc07107c642aff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRedshiftIdcApplication

## Description
One root cause, in the redshift service: an over-permissive create. The open issue 20261029T200555Z-redshift-namespace-reserved-node-idc-application-hsm-refusals.md describes the same family (its Cause 6) but carries only the replay key of redshift.create-redshift-idc-application-dependent-service-rejected and does NOT carry this check's key, so this failure is filed separately and must be closed against the check id below.

Cause 1 of 1: CreateRedshiftIdcApplication does not resolve IdcInstanceArn (or IamRoleArn) against the dependent service and registers an application for an IdC instance that does not exist.

AWS: CreateRedshiftIdcApplication calls into IAM Identity Center to register the application for the supplied IdcInstanceArn. For a well-formed but nonexistent instance ARN (and a role ARN naming no existing role) the dependent call is denied, and Redshift surfaces that denial as `DependentServiceAccessDenied` with HTTP 403 — Smithy: CreateRedshiftIdcApplication errors include DependentServiceAccessDeniedFault (errorKind client, httpError 403, awsQueryError code `DependentServiceAccessDenied`, httpResponseCode 403); input members IdcInstanceArn, RedshiftIdcApplicationName (@length 1-63, @pattern ^[a-z][a-z0-9]*(-[a-z0-9]+)*$), IdcDisplayName and IamRoleArn are all required. Recorded behavior: the relayed message is the IdC denial, whose fixed text is "User: " + the caller's own principal ARN (from the caller's identity, e.g. as returned by sts:GetCallerIdentity) + " is not authorized to perform: sso:CreateApplication on resource: arn:aws:sso::*:application/" + the instance suffix taken from the request's IdcInstanceArn + "/* because no resource-based policy allows the sso:CreateApplication action"; the per-request " (Service: ..., Status Code: ..., Request ID: ...)" tail is the only varying part and is not part of the requirement. No application is left behind, so a following DescribeRedshiftIdcApplications does not list the requested name. Variable parts: the caller principal ARN and the IdC instance suffix; everything else is fixed text.

Emulator: the call SUCCEEDS with HTTP 200 and returns a fabricated RedshiftIdcApplication for the nonexistent instance — IdcOnboardStatus 'Completed', a minted RedshiftIdcApplicationArn and a minted IdcManagedApplicationArn under the fabricated ssoins- instance — so nothing is refused and the application is registered.

Why: a client relying on AWS's behavior treats a successful create as proof its IAM Identity Center wiring (instance ARN plus the IdC-trusting IAM role) is valid. Against the emulator every misconfigured IdC estate onboards 'Completed', the 403 branch (catch DependentServiceAccessDenied, fix the instance/role, retry) is never exercised, and every downstream Identity-Center auth flow is built on an application AWS would never have created. Because the refusal is skipped, the check's later legs (DeleteRedshiftIdcApplication on an absent ARN must fail RedshiftIdcApplicationNotExists, HTTP 404, with "The RedshiftIdcApplication with the ARN " + the requested ARN + " wasn't found.") are never reached and remain unverified.

Fix: the CreateRedshiftIdcApplication handler's precondition/validation step in the redshift service — resolve IdcInstanceArn (and the supplied IamRoleArn) against IAM Identity Center state before mutating state, and refuse with DependentServiceAccessDenied (403) and the relayed sso:CreateApplication denial message, leaving no application in the service state.

Traces: redshift.e2e-idc-application-auth-token-flow.

Note: in the same check GetIdentityCenterAuthToken correctly answered UnsupportedOperation with "Request rejected: Not an IDC user acting", so that leg is not part of this issue.

## Evidence
Cause 1 — failing check redshift.e2e-idc-application-auth-token-flow, at the CreateRedshiftIdcApplication leg:

AssertionError: expected a ClientError, but the call succeeded and returned {'RedshiftIdcApplication': {'IdcInstanceArn': 'arn:aws:sso:::instance/ssoins-0000000000000000', 'RedshiftIdcApplicationName': '<minted-name>', 'RedshiftIdcApplicationArn': 'arn:aws:redshift:<region>:<account>:redshiftidcapplication:<uuid>', 'IdcDisplayName': 'sim2real-behavioral-check', 'IamRoleArn': 'arn:aws:iam::<account>:role/sim2real-nonexistent-idc-role', 'IdcManagedApplicationArn': 'arn:aws:sso::<account>:application/ssoins-0000000000000000/apl-<id>', 'IdcOnboardStatus': 'Completed', 'Tags': []}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Expected (AWS): error code `DependentServiceAccessDenied`, HTTP 403, message "User: <caller principal ARN> is not authorized to perform: sso:CreateApplication on resource: arn:aws:sso::*:application/ssoins-<instance-suffix>/* because no resource-based policy allows the sso:CreateApplication action" — fixed text as quoted, with the caller principal ARN and the instance suffix supplied by the request/caller; the generated names, account id, region and uuids above are per-run values, not requirements.

Actual (emulator): HTTP 200, no Error member, the application registered and discoverable.

The check's positive control (DescribeClusters with MaxRecords=20) and its GetIdentityCenterAuthToken leg both succeeded before this assertion, so the failure is specific to the CreateRedshiftIdcApplication precondition.

## Steps to Reproduce
1. Boot the emulator and point a redshift client (and an sts client) at it.
2. Positive control: redshift.describe_clusters(MaxRecords=20) -> succeeds.
3. account = sts.get_caller_identity()['Account']; caller_arn = sts.get_caller_identity()['Arn'].
4. Call redshift.create_redshift_idc_application(IdcInstanceArn='arn:aws:sso:::instance/ssoins-0000000000000000', RedshiftIdcApplicationName=<fresh lowercase name matching ^[a-z][a-z0-9]*(-[a-z0-9]+)*$>, IdcDisplayName='sim2real-behavioral-check', IamRoleArn=f'arn:aws:iam::{account}:role/sim2real-nonexistent-idc-role').
5. Expect a ClientError with Error.Code == 'DependentServiceAccessDenied', HTTPStatusCode 403, and Error.Message (minus its per-request '(Service: ..., Request ID: ...)' suffix) == f"User: {caller_arn} is not authorized to perform: sso:CreateApplication on resource: arn:aws:sso::*:application/ssoins-0000000000000000/* because no resource-based policy allows the sso:CreateApplication action". Observed: HTTP 200 with a registered RedshiftIdcApplication.
6. Assert redshift.describe_redshift_idc_applications() does NOT list the name used in step 4 (the rejected create must leave no state).
7. Then redshift.delete_redshift_idc_application(RedshiftIdcApplicationArn=f'arn:aws:redshift:<region>:{account}:redshiftidcapplication:00000000-0000-0000-0000-000000000000') must fail 'RedshiftIdcApplicationNotExists' (HTTP 404) with "The RedshiftIdcApplication with the ARN <that ARN> wasn't found."
8. Re-replay redshift.e2e-idc-application-auth-token-flow to confirm the fix.
