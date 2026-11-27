Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__lambda.lambda-target-in-another-account-without-role-arn-rejected
Issue-Key: cloudwatch__events__lambda.lambda-target-in-another-account-without-role-arn-rejected|run#26|AssertionError|assert got == want, f"PutTargets naming another account's function answered {got!r}, expected {want!r}"
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudwatch__events__lambda: PutTargets naming another account's Lambda function without RoleArn answers AccessDeniedException instead of AWS's ValidationException "RoleArn is required for target <arn>."

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: a cross-account target with no RoleArn is classified as an authorization failure rather than a request-validation failure, with an emulator-authored message.

AWS: PutTargets with a target ARN in another account and no RoleArn is refused HTTP 400 with error code ValidationException and the message `RoleArn is required for target <target-arn>.` — fixed text `RoleArn is required for target ` + the target's ARN taken from the request + `.`. Smithy (eventbridge): Target.RoleArn is optional in the model (@length 1..1600), so this is a handler-side request rule, and ValidationException is the service-common request-validation error; cross-account delivery is documented to require a RoleArn with the right permissions (see the Target shape documentation on another account's event bus/target).
Emulator: answers AccessDeniedException with `Access to the resource <arn> is denied. Reason: Adding cross-account target is not permitted.` — wrong error code (an authorization class, which clients do not retry or repair the same way) and wrong message, hiding the actual remedy (supply RoleArn).
Why: an SDK/IaC caller handling ValidationException to add the role instead surfaces an access-denied error to the user, and code branching on the error code takes the wrong path; the message does not say what to fix.
Fix: the cross-account target validation in the events PutTargets handler (classify a missing RoleArn as ValidationException with AWS's message, before any permission judgement).
Traces: cloudwatch__events__lambda.lambda-target-in-another-account-without-role-arn-rejected.

## Evidence
Expected: ('ValidationException', 'RoleArn is required for target arn:aws:lambda:<region>:111122223333:function:<fn>.')
Actual:   ('AccessDeniedException', 'Access to the resource arn:aws:lambda:<region>:111122223333:function:<fn> is denied. Reason: Adding cross-account target is not permitted.')

## Steps to Reproduce
1. events PutRule on a bus.
2. events PutTargets with a target whose Arn is a Lambda function ARN in a different account id and no RoleArn.
3. Observe the error code and message.
