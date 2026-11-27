Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__ssm__sts.automation-assume-role-is-the-writer-and-its-absence-runs-as-caller iam__ssm__sts.describe-parameters-scoped-to-a-parameter-arn-denied iam__ssm__sts.get-document-denied-by-document-arn-while-list-documents-still-shows-it iam__ssm__sts.get-parameter-wildcard-action-authorizes-all-four-reads iam__ssm__sts.list-documents-allowed-only-on-a-document-arn-rejected iam__ssm__sts.register-window-task-without-pass-role-on-service-role-denied
Issue-Key: iam__ssm__sts.automation-assume-role-is-the-writer-and-its-absence-runs-as-caller|run#30|AssertionError|assert writer == want, (
Issue-Key: iam__ssm__sts.describe-parameters-scoped-to-a-parameter-arn-denied|_refusal#14|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: iam__ssm__sts.get-document-denied-by-document-arn-while-list-documents-still-shows-it|_denial#24|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: iam__ssm__sts.get-parameter-wildcard-action-authorizes-all-four-reads|_refusal#28|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: iam__ssm__sts.list-documents-allowed-only-on-a-document-arn-rejected|_refusal#20|AssertionError|raise AssertionError(f"the call was expected to be refused and succeeded: {out!r}")
Issue-Key: iam__ssm__sts.register-window-task-without-pass-role-on-service-role-denied|_refusal#23|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# ssm IAM authorization is over-permissive on five operations (DescribeParameters, ListDocuments, GetDocument Deny, PutParameter, RegisterTaskWithMaintenanceWindow) and the Automation assume-role session name is not Automation-<executionId> (6 causes)

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters

## Description
Six net-new causes in the ssm service: five authorization decisions that serve requests AWS refuses, and one wrong session identity for Automation.

Cause 1 of 6: an Automation step's AutomationAssumeRole session is named with the bare execution id instead of `Automation-<executionId>`.
AWS: recorded behavior only. A runbook step that runs under AutomationAssumeRole acts as an assumed-role session whose name is the fixed prefix `Automation-` followed by the execution's own AutomationExecutionId, so a parameter the step writes reports LastModifiedUser `arn:aws:sts::<account>:assumed-role/<role name>/Automation-<AutomationExecutionId>`. With NO assume role the same step acts as the CALLER, and LastModifiedUser is the caller's own ARN.
Emulator: the writer is `arn:aws:sts::<account>:assumed-role/<role name>/<AutomationExecutionId>` — the `Automation-` prefix is missing. Fixed text is the prefix; the execution id is the request's own value.
Why: the session name is how a client attributes a write to an automation rather than to a human, and how `aws:PrincipalArn`/session-name conditions and CloudTrail correlation are written. A policy or audit query matching `*/Automation-*` matches nothing, so automation-performed writes are indistinguishable from direct ones.
Fix: the ssm Automation executor's assume-role step — compose the RoleSessionName as `Automation-` + the execution id.
Traces: iam__ssm__sts.automation-assume-role-is-the-writer-and-its-absence-runs-as-caller.

Cause 2 of 6: DescribeParameters is served to a principal whose grant names a parameter ARN.
AWS: recorded behavior only (the ssm model declares no authorization error; the refusal is IAM's AccessDeniedException). `ssm:DescribeParameters` has NO resource type, so it is authorized only against `arn:aws:ssm:<region>:<account>:*`; a statement allowing it on one parameter's ARN does not authorize it and the call is REFUSED with code `AccessDeniedException` and message "User: " + the caller's session ARN + " is not authorized to perform: ssm:DescribeParameters on resource: arn:aws:ssm:<region>:<account>:* because no identity-based policy allows the ssm:DescribeParameters action". The twin role differing only in `Resource: "*"` lists the parameter; conversely DescribeParameters on `*` does NOT grant GetParameter on the parameter ARN.
Emulator: HTTP 200 and the parameter is listed — ['/<name>/entry'] — although the grant names only that parameter's ARN. The positive control in the same statement (GetParameter on the parameter ARN) works, so the policy is in effect.
Why: a client cannot scope parameter discovery at all; a policy that deliberately withholds account-wide DescribeParameters still enumerates parameters, and the documented no-resource-type rule is unobservable.
Fix: the ssm DescribeParameters handler's authorization step — authorize against `arn:aws:ssm:<region>:<account>:*` and refuse with the message above.
Traces: iam__ssm__sts.describe-parameters-scoped-to-a-parameter-arn-denied.

Cause 3 of 6: an explicit Deny on a document ARN does not refuse GetDocument.
AWS: recorded behavior only. With an identity policy that Allows `ssm:GetDocument` broadly and explicitly DENIES it for one document's ARN, GetDocument on that document is REFUSED with code `AccessDeniedException` and a message whose fixed ends are "User: arn:…" … " is not authorized to perform: ssm:GetDocument on resource: arn:…" … " with an explicit deny in an identity-based policy" (the explicit-deny wording, not the no-policy-allows wording); the session ARN and document ARN in between are the request's own values. ListDocuments still SHOWS the denied document, the document stays intact for its owner, and dropping the Deny restores the read.
Emulator: HTTP 200 with the document's full Content, DocumentVersion, Status and DocumentType.
Why: an explicit Deny is the only hard boundary an identity policy has. A client that quarantines a sensitive runbook by denying its ARN still serves its contents to every principal, and the explicit-deny wording — which is how an operator tells a Deny from a missing Allow — never appears.
Fix: the ssm GetDocument handler's authorization step — evaluate explicit Deny statements against the document ARN and emit the explicit-deny AccessDeniedException.
Traces: iam__ssm__sts.get-document-denied-by-document-arn-while-list-documents-still-shows-it.

Cause 4 of 6: PutParameter is served to a principal whose only grant is `ssm:GetParameter*`.
AWS: recorded behavior only. An action wildcard expands only over actions matching the pattern: `ssm:GetParameter*` covers GetParameter, GetParameters, GetParameterHistory and GetParametersByPath, and does NOT cover PutParameter. PutParameter is therefore REFUSED with code `AccessDeniedException` and message "User: " + the session ARN + " is not authorized to perform: ssm:PutParameter on resource: arn:aws:ssm:<region>:<account>:parameter<name> because no identity-based policy allows the ssm:PutParameter action", and the parameter keeps its previous value and Version.
Emulator: HTTP 200 — the write is served and the parameter advances (the reply carries the new Version and Tier), so the refused value is actually persisted.
Why: a read-only policy grants writes. Every `Get*`-style least-privilege grant silently becomes read-write, and the parameter's value and version history are mutated by a principal AWS would have refused — an unrecoverable side effect, not just a wrong status.
Fix: the ssm PutParameter handler's authorization step — match the requested action against the policy's action patterns rather than treating a prefix grant as covering the operation.
Traces: iam__ssm__sts.get-parameter-wildcard-action-authorizes-all-four-reads.

Cause 5 of 6: ListDocuments is served to a principal whose grant names a document ARN.
AWS: recorded behavior only. `ssm:ListDocuments` is authorized against the account-wide resource, so a statement allowing it on ONE document's ARN does not authorize the listing; it is REFUSED with code `AccessDeniedException` and a message whose fixed ends are "User: arn:…" … " is not authorized to perform: ssm:ListDocuments on resource: arn:…" … ":* because no identity-based policy allows the ssm:ListDocuments action". The twin role differing only in `Resource: "*"` lists the document.
Emulator: HTTP 200 with the whole DocumentIdentifiers catalog (every AWS-owned document — AWS-ASGEnterStandby, AWS-ApplyAnsiblePlaybooks, …).
Why: a document-scoped grant leaks the entire document catalog of the account/Region, which is exactly the enumeration a scoped policy exists to prevent; and the author cannot distinguish the scoped policy from the wildcard one, since both answer 200.
Fix: the ssm ListDocuments handler's authorization step — authorize against the account-wide ssm resource and refuse with the message above.
Traces: iam__ssm__sts.list-documents-allowed-only-on-a-document-arn-rejected.

Cause 6 of 6: RegisterTaskWithMaintenanceWindow is served without `iam:PassRole` on the task's ServiceRoleArn.
AWS: recorded behavior only. Registering a maintenance-window task hands a service role to SSM, so the caller must hold `iam:PassRole` for that role in addition to `ssm:RegisterTaskWithMaintenanceWindow`. A caller holding only the SSM action is REFUSED with code `AccessDeniedException` and message "User: " + the session ARN + " is not authorized to perform: iam:PassRole on resource: " + the service role's ARN + " because no identity-based policy allows the iam:PassRole action" — note the refused action is iam:PassRole, not the SSM action — and the window ends up holding only the task registered by the caller that DOES hold PassRole.
Emulator: HTTP 200 with a WindowTaskId; the task is registered and the window holds it.
Why: the PassRole boundary is the control that stops a low-privilege caller from escalating by handing a powerful role to a service. Without it, any principal that may register a window task can make SSM act as any role in the account, and the refusal that is supposed to name iam:PassRole never appears.
Fix: the ssm RegisterTaskWithMaintenanceWindow handler's authorization step — authorize `iam:PassRole` for the submitted ServiceRoleArn before registering, and refuse with the iam:PassRole AccessDeniedException.
Traces: iam__ssm__sts.register-window-task-without-pass-role-on-service-role-denied.

## Evidence
Cause 1 — expected LastModifiedUser 'arn:aws:sts::<account>:assumed-role/<role>/Automation-<executionId>'. Actual, verbatim: 'AssertionError: LastModifiedUser after the role-run step was \'arn:aws:sts::<account>:assumed-role/<role>/fe006c2f-c417-4267-8828-4ff330f761eb\', expected the role session' (the value is the bare execution id).

Cause 2 — expected ('AccessDeniedException', '… ssm:DescribeParameters on resource: arn:aws:ssm:<region>:<account>:* …'). Actual: 'AssertionError: the call was expected to be refused but returned [\'/<param>/entry\']'.

Cause 3 — expected AccessDeniedException ending ' with an explicit deny in an identity-based policy'. Actual: 'AssertionError: expected the call to be refused, but it succeeded: {\'Name\': \'<doc>\', \'DocumentVersion\': \'1\', \'Status\': \'Active\', \'Content\': \'{"schemaVersion": "2.2", …}\', \'DocumentType\': \'Command\', …HTTPStatusCode: 200}'.

Cause 4 — expected ('AccessDeniedException', '… ssm:PutParameter on resource: arn:aws:ssm:<region>:<account>:parameter<name> …') with the parameter unchanged. Actual: 'AssertionError: the call was expected to be refused but returned {\'Version\': 3, \'Tier\': \'Standard\', …HTTPStatusCode: 200}' — the write landed and advanced the version.

Cause 5 — expected AccessDeniedException ending ':* because no identity-based policy allows the ssm:ListDocuments action'. Actual: 'AssertionError: the call was expected to be refused and succeeded: {\'DocumentIdentifiers\': [{\'Name\': \'AWS-ASGEnterStandby\', …}, {\'Name\': \'AWS-ASGExitStandby\', …}, {\'Name\': \'AWS-ApplyAnsiblePlaybooks\', …}, …]}'.

Cause 6 — expected ('AccessDeniedException', 'User: <session arn> is not authorized to perform: iam:PassRole on resource: <service role arn> because no identity-based policy allows the iam:PassRole action'). Actual: 'AssertionError: the call was expected to be refused but returned {\'WindowTaskId\': \'56a59774-…\', …HTTPStatusCode: 200}'.

Execution ids, parameter/document/window/role names, account ids and request ids above are per-run values and are not part of the requirement.

## Steps to Reproduce
Cause 1: create an Automation runbook whose step writes an SSM parameter, run it once with AutomationAssumeRole and once without, then DescribeParameters for that parameter and read LastModifiedUser — expect 'arn:aws:sts::<account>:assumed-role/<role>/Automation-<executionId>' for the role run and the caller's ARN for the other.
Cause 2: create a parameter; create role A allowing ssm:GetParameter+ssm:DescribeParameters on the parameter ARN and role B allowing ssm:DescribeParameters on '*'. As A, GetParameter succeeds; DescribeParameters must be refused (observe it listing the parameter). As B, DescribeParameters lists it and GetParameter must be refused.
Cause 3: create two Command documents; grant a role ssm:GetDocument broadly with an explicit Deny on one document's ARN. GetDocument on the open one succeeds and ListDocuments shows both; GetDocument on the denied one must be refused with the explicit-deny wording (observe 200 with Content). Drop the Deny and the read must succeed.
Cause 4: create a parameter with two versions; grant a role only ssm:GetParameter* on its ARN. The four reads succeed; PutParameter(Overwrite=true) must be refused and leave the value/version unchanged (observe 200 and Version 3).
Cause 5: create an Automation document; create twin roles whose ssm:ListDocuments statement names the document ARN vs '*'. The wildcard role's ListDocuments is served; the scoped role's must be refused (observe the full catalog).
Cause 6: create a maintenance window, a runbook and an ssm.amazonaws.com service role; create one caller role with ssm:RegisterTaskWithMaintenanceWindow + iam:PassRole on the service role, and one with only the SSM action. The first registers a task; the second must be refused naming iam:PassRole (observe 200 with a WindowTaskId).
