Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role
Issue-Key: lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role|run#24|AssertionError|assert masked == (
Issue-Commit: 9ef334bf99662e2a416b822a9dc7703a11a389e9

# lambda__stepfunctions: a denied legacy Lambda-ARN Task's cause carries the SDK-v1 '(Service: AWSLambda; ...; Proxy: null)' suffix instead of AWS's '(Service: Lambda, Status Code: 403, Request ID: ...) (SDK Attempt Count: n)' (1 cause)

> _Note: this issue was identified against the code base at commit `9ef334bf99662e2a416b822a9dc7703a11a389e9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
1 net-new root cause, grounded in the one failing behavioral check under `### lambda__stepfunctions` that the suite marked neither ALREADY FILED nor OWNED.

Cause 1 of 1: when a Step Functions Task uses the LEGACY Lambda integration — the function ARN itself as `Resource`, with the state input passed as the event and no Payload/FunctionName wrapping — and the invoking identity is denied lambda:InvokeFunction, the execution's failure `cause` is rendered with the wrong SDK suffix for that integration flavour. Failing check id: lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role.

AWS: recorded behavior only — the Lambda Smithy model declares no AccessDeniedException on Invoke, so the requirement is the recorded wire text; Smithy sfn StartSyncExecution returns `status` (SyncExecutionStatus), `error` (SensitiveError, length 0-256) and `cause` (SensitiveCause, length 0-32768), and the same `error`/`cause` pair is returned by DescribeExecution. The execution reaches status FAILED with error `Lambda.AWSLambdaException` (the emulator already matches this) and a cause whose fixed text is:
  `User: <sts assumed-role ARN of the identity that performed the invoke> is not authorized to perform: lambda:InvokeFunction on resource: <function ARN> because no identity-based policy allows the lambda:InvokeFunction action (Service: Lambda, Status Code: 403, Request ID: <request id>) (SDK Attempt Count: <n>)`
Fixed text: the IAM sentence template (`User: ... is not authorized to perform: <action> on resource: <resource> because no identity-based policy allows the <action> action`), and the COMMA-separated parenthesised suffix with literal tokens `Service: Lambda` and `Status Code: 403`, followed by a separate `(SDK Attempt Count: <n>)` clause. Variable parts taken from the request and from state: the assumed-role ARN (partition, account, role name, per-call session name), the function ARN, the per-call request id, and the attempt count. There is no `Error Code:` token and no `Proxy: null` token in this form. Also recorded: a denied execution produces NO `output` member at all.

Emulator: the IAM sentence is right, but the suffix is the SDK-v1/Java-v1 style `(Service: AWSLambda; Status Code: 403; Error Code: AccessDeniedException; Request ID: <uuid>; Proxy: null)` — semicolon-separated instead of comma-separated, service name `AWSLambda` instead of `Lambda`, an extra `Error Code: AccessDeniedException` token, an extra `Proxy: null` token, and no trailing `(SDK Attempt Count: <n>)` clause. The status (403) and the error name (`Lambda.AWSLambdaException`) already match; only the suffix rendering diverges.

Note on an apparently conflicting open issue: 20261125T091810Z-lambda-stepfunctions-the-cause-of-a-lambda-invoke.md records the OPPOSITE requirement for the OPTIMIZED `arn:aws:states:::lambda:invoke` integration (there AWS serves the semicolon/`AWSLambda`/`Error Code`/`Proxy: null` form and the emulator serves the comma form). Taken together, the recordings show AWS renders a different SDK suffix for the two integration flavours and the emulator renders ONE suffix for both, so the two paths are effectively swapped. That issue does not carry this check's replay key and does not cover this cause; this one is specifically the legacy function-ARN Resource path. Open 20261109T210043Z-lambda-stepfunctions-8-net-new-causes-step-functions-ignores-task.md and 20261109T210043Z-cloudformation-lambda-stepfunctions-a-step-functions-lambda-task-denied-lambda-invokefunction.md also touch Task Credentials/denial wording but carry other replay keys.

Why: a client that branches on the denial — matching the service name, the 403 authorization class, or an error-code token in the cause before retrying, alarming or reporting the missing grant — sees a service name (`AWSLambda`) and clauses (`Error Code`, `Proxy`) that AWS does not emit for a legacy-ARN Lambda Task, and loses the attempt-count clause it does emit. Log pipelines, dashboards and regex-based runbooks keyed on AWS's suffix never match, so the denial is misattributed.

Fix: the Step Functions error serializer for the LEGACY Lambda-ARN Task integration — the component that wraps a Lambda refusal into the execution's error/cause (and thus into the TaskFailed / ExecutionFailed history event details and the StartSyncExecution `cause` member). It must render the comma-separated suffix with `Service: Lambda`, the refusal's status code and the request id, followed by the separate `(SDK Attempt Count: <n>)` clause, and must not emit `AWSLambda`, `Error Code:` or `Proxy: null` on this path — while leaving the optimized `lambda:invoke` path's own recorded suffix intact.

Traces: lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role (the no-Credentials negative control of an otherwise passing Credentials.RoleArn.$ scenario).

## Evidence
Cause 1 — check lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role (verdict: mismatch; assertion at line 103 of the check, after the per-call session name, request id and attempt count are masked).

Expected (AWS), session name / request id / attempt count masked:
  'User: arn:<partition>:sts::<account>:assumed-role/<exec-role>/<session> is not authorized to perform: lambda:InvokeFunction on resource: <function arn> because no identity-based policy allows the lambda:InvokeFunction action (Service: Lambda, Status Code: 403, Request ID: <request-id>) (SDK Attempt Count: <n>)'

Actual (emulator):
  'User: arn:aws:sts::000000000000:assumed-role/sim2real-behav-tc2-role-7d5eb66ebeaa/2de4321574624e22b09a3bf80a85de3c is not authorized to perform: lambda:InvokeFunction on resource: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-tc2-fn-6e7fa1497596 because no identity-based policy allows the lambda:InvokeFunction action (Service: AWSLambda; Status Code: 403; Error Code: AccessDeniedException; Request ID: 20b83cd1-ba50-46c9-b271-8c83f9b97fc4; Proxy: null)'

Fixed text vs per-run values: the account id (000000000000), the role name, the function name, the session name (2de43215...) and the request id (20b83cd1-...) above are this run's values and are NOT part of the requirement. The requirement is the suffix shape: separator `,` not `;`, service token `Service: Lambda` not `Service: AWSLambda`, no `Error Code: ...` clause, no `Proxy: null` clause, and a trailing ` (SDK Attempt Count: <n>)` clause.

Already matching in this run: StartSyncExecution returned status `FAILED`, error `Lambda.AWSLambdaException`, HTTP 200 on the StartSyncExecution call itself, and the IAM sentence naming the execution role's own assumed-role session and the function ARN. Smithy sfn: StartSyncExecution output members `status` (required, SyncExecutionStatus), `error` (SensitiveError 0-256), `cause` (SensitiveCause 0-32768); errors InvalidArn/InvalidExecutionInput/InvalidName/StateMachineDeleting/StateMachineDoesNotExist/StateMachineTypeNotSupported, all 400 — none applies, the call succeeds and only the cause text diverges.

## Steps to Reproduce
1. lambda: CreateFunction (python3.12, a handler returning {'ok': True, 'echo': event}) and wait for it to become Active.
2. iam: CreateRole trusting states.amazonaws.com with NO lambda:InvokeFunction grant (the execution role).
3. stepfunctions: CreateStateMachine of type EXPRESS with one Task whose `Resource` IS the function ARN (the legacy integration: no `arn:aws:states:::lambda:invoke`, no Parameters/Payload wrapping) and NO `Credentials` block, using that execution role.
4. StartSyncExecution with input {"marker": <v>}.
5. Observe status == FAILED and error == "Lambda.AWSLambdaException" (both correct), then compare `cause`: the emulator appends `(Service: AWSLambda; Status Code: 403; Error Code: AccessDeniedException; Request ID: <id>; Proxy: null)` where AWS appends `(Service: Lambda, Status Code: 403, Request ID: <id>) (SDK Attempt Count: <n>)`. Mask only the assumed-role session name, the request id and the attempt count before comparing.
6. Equivalent: re-replay check lambda__stepfunctions.task-credentials-dynamic-rolearn-on-legacy-lambda-arn-task-assumes-task-role.
