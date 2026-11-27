Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.execute-change-set-token-reused-on-another-change-set-rejected cloudformation.nested-change-set-without-iam-capability-execute-rejected
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|ExecuteChangeSet|InvalidChangeSetStatus
Issue-Key: cloudformation.nested-change-set-without-iam-capability-execute-rejected|run#36|AssertionError|assert (bad["Status"], bad["ExecutionStatus"]) == ("FAILED", "UNAVAILABLE"), (
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# CloudFormation: 2 net-new causes — a retried ExecuteChangeSet with the same ClientRequestToken is refused, and a nested change set with IAM resources is created without CAPABILITY_IAM

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Two net-new causes in CloudFormation's change-set surface.

Cause 1 of 2: ExecuteChangeSet is not idempotent for a repeated call that carries the ClientRequestToken that already executed that same change set.
AWS: re-issuing ExecuteChangeSet for a change set already in ExecutionStatus EXECUTE_COMPLETE with the SAME ClientRequestToken is accepted as a no-op (HTTP 200, empty body) — that is what makes the token a client request token; the stack is left untouched and its resource set unchanged. Only a DIFFERENT change set presented with an already-used token is refused, with TokenAlreadyExistsException and the message `The client request token <token> has already been used for this stack. Retry the request with a different client request token` (fixed text; the token is the value from the request). Recorded behaviour confirms the positive control: the first execute, then the identical retry, both succeed.
Emulator: the identical retry is refused with HTTP 400 `InvalidChangeSetStatus` and the message `ChangeSet [<change-set-arn>] cannot be executed in its current execution status of [EXECUTE_COMPLETE]`, so the check cannot even reach the TokenAlreadyExistsException assertion. The status check is applied before, and in place of, the token-replay check.
Why: every SDK and the AWS CLI retry ExecuteChangeSet on a transport error with the same client request token. Against the emulator that retry turns a succeeded deployment into a hard 400, so a deploy that actually applied is reported as failed.
Fix: ExecuteChangeSet's handler — the execution-status guard must be evaluated after the client-request-token replay lookup, and a replay of the token that executed this very change set must return success instead of InvalidChangeSetStatus.

Cause 2 of 2: CreateChangeSet for a template whose NESTED stack creates IAM resources succeeds without CAPABILITY_IAM.
AWS: capabilities are enforced across the whole nested-stack tree; a change set whose child template contains an IAM resource and whose CreateChangeSet call passes no Capabilities settles with Status FAILED and ExecutionStatus UNAVAILABLE (the StatusReason naming the requires-capabilities refusal), so it can never be executed. Recorded behaviour only for the exact StatusReason text.
Emulator: the change set settles ('CREATE_COMPLETE', 'AVAILABLE') and is executable, i.e. the capability gate is applied only to resources declared directly in the root template.
Why: the capability acknowledgement is a privilege gate. A caller that relies on CloudFormation refusing an un-acknowledged IAM change can create IAM principals from a nested template with no acknowledgement at all.
Fix: the capability computation that CreateChangeSet performs — it must walk nested AWS::CloudFormation::Stack templates when collecting the required capabilities.

Traces: cloudformation.execute-change-set-token-reused-on-another-change-set-rejected; cloudformation.nested-change-set-without-iam-capability-execute-rejected.

## Evidence
Cause 1 — ExecuteChangeSet(StackName=<stack>, ChangeSetName='initial', ClientRequestToken=<token>) called a second time after the first execute reached CREATE_COMPLETE:
  expected: 200, no-op, stack resources unchanged ([Marker0])
  actual:   botocore.errorfactory.InvalidChangeSetStatusException: An error occurred (InvalidChangeSetStatus) when calling the ExecuteChangeSet operation: ChangeSet [arn:aws:cloudformation:<region>:<account>:changeSet/initial/<id>] cannot be executed in its current execution status of [EXECUTE_COMPLETE]

Cause 2 — CreateChangeSet with no Capabilities for a root template whose nested stack holds an IAM resource:
  expected: (Status, ExecutionStatus) == ('FAILED', 'UNAVAILABLE')
  actual:   ('CREATE_COMPLETE', 'AVAILABLE')

## Steps to Reproduce
Cause 1:
1. CreateChangeSet(ChangeSetType=CREATE, ChangeSetName='initial') and wait for CREATE_COMPLETE.
2. ExecuteChangeSet(ChangeSetName='initial', ClientRequestToken=T); wait for the stack to reach CREATE_COMPLETE.
3. ExecuteChangeSet(ChangeSetName='initial', ClientRequestToken=T) again — expect 200 and an unchanged stack; the emulator raises InvalidChangeSetStatus.

Cause 2:
1. CreateStack/CreateChangeSet for a root template containing an AWS::CloudFormation::Stack child whose template declares an AWS::IAM::Role, passing no Capabilities.
2. DescribeChangeSet and read (Status, ExecutionStatus) — expect ('FAILED','UNAVAILABLE'); the emulator reports ('CREATE_COMPLETE','AVAILABLE').
