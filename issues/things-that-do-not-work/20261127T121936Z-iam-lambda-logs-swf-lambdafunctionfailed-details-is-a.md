Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs__swf.lambda-task-unregistered-function-name-fails-in-history
Issue-Key: iam__lambda__logs__swf.lambda-task-unregistered-function-name-fails-in-history|run#31|AssertionError|assert details.startswith(expected_prefix) and details.endswith(expected_suffix), (
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# iam__lambda__logs__swf: LambdaFunctionFailed.details is a Rust Debug dump of the SDK error instead of AWS's '<message> (Service: AWSLambda; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)' line

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RespondDecisionTaskCompleted

## Description
Cause 1 of 1: when a SWF ScheduleLambdaFunction task fails because the Lambda invocation was refused, SWF writes the invoker's error into `LambdaFunctionFailed.details`. The emulator writes its internal Rust error chain there instead of the single AWS-formatted line. Failing check: iam__lambda__logs__swf.lambda-task-unregistered-function-name-fails-in-history.

AWS: recorded behavior — a ScheduleLambdaFunction decision naming a function that was never created records LambdaFunctionScheduled, LambdaFunctionStarted and a linked LambdaFunctionFailed whose `reason` is the Lambda error code (`ResourceNotFoundException`, which the emulator already gets right) and whose `details` is exactly ONE line in the AWS-SDK-for-Java error format: `<Lambda's own message> (Service: AWSLambda; Status Code: <the invoke's HTTP status>; Error Code: <the Lambda error code>; Request ID: <the invoke's request id>; Proxy: null)`. Fixed text of that template: ` (Service: AWSLambda; Status Code: `, `; Error Code: `, `; Request ID: ` and `; Proxy: null)`. Its variable parts come from the failed invoke — Lambda's message (for an absent function, `Function not found: arn:aws:lambda:<region>:<account>:function:<name>:$LATEST`), the status (404 here), the error code and the request id; the request id differs per run and is not itself a requirement. Smithy: `LambdaFunctionFailedEventAttributes` models `reason` (FailureReason, max 256) and `details` (Data, max 32768) as "The reason provided for the failure" / "The details of the failure"; the exact rendering is recorded behavior.

Emulator: `details` holds the formatted Rust error chain, beginning `service error: ResourceNotFoundException: Function not found: ...: ResourceNotFoundException: Function not found: ...` and then the `Debug` rendering of the SDK error — `(ServiceError(ServiceError { source: ResourceNotFoundException(ResourceNotFoundException { type: Some("User"), message: Some(...), meta: ErrorMetadata { code: Some("ResourceNotFoundException"), ..., extras: Some({"aws_request_id": "..."}) } }), raw: Response { status: StatusCode(404), headers: {...}, body: SdkBody { inner: Once(Some(b"{...}")) }, retry ...` — i.e. the message is duplicated twice, internal Rust type names, header maps and the raw JSON body leak into a history field, and none of the `(Service: AWSLambda; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)` framing is present.

Why: deciders parse `details` to classify a Lambda task failure (and the suffix `; Proxy: null)` plus the `Error Code:`/`Status Code:` fields are what that parsing keys on). Against the emulator any such parse fails, the 32768-byte Data bound is burned on a debug dump, and emulator-internal types are exposed as if they were AWS content.

Fix: the Lambda-invocation leg of the SWF decision engine (the ScheduleLambdaFunction path that records LambdaFunctionFailed in services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/respond_decision_task_completed.rs) must render the invoke failure into the single AWS line — taking message, HTTP status, error code and request id off the invoke response — instead of formatting the SDK error with `{}`/`{:?}`.

Traces: iam__lambda__logs__swf.lambda-task-unregistered-function-name-fails-in-history (the same check's `reason == 'ResourceNotFoundException'` assertion passes; only `details` diverges).

## Evidence
Check iam__lambda__logs__swf.lambda-task-unregistered-function-name-fails-in-history:
  assert details.startswith(expected_prefix) and details.endswith(expected_suffix)
  AssertionError: expected details to read 'Function not found: arn:aws:lambda:<region>:<account>:function:<missing-fn>:$LATEST (Service: AWSLambda; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: ' + <request id> + '; Proxy: null)', got 'service error: ResourceNotFoundException: Function not found: ...:$LATEST: ResourceNotFoundException: Function not found: ...:$LATEST (ServiceError(ServiceError { source: ResourceNotFoundException(ResourceNotFoundException { type: Some("User"), message: Some("Function not found: ..."), meta: ErrorMetadata { code: Some("ResourceNotFoundException"), message: Some("Function not found: ..."), extras: Some({"aws_request_id": "<id>"}) } }), raw: Response { status: StatusCode(404), headers: {"content-type": "application/json", "x-amzn-errortype": "ResourceNotFoundException", "x-amzn-requestid": "<id>", "content-length": "177"}, body: SdkBody { inner: Once(Some(b"{\"__type\":\"ResourceNotFoundException\",\"Message\":\"Function not found: ...\",\"Type\":\"User\"}")) }, retry ...'

Expected (AWS): LambdaFunctionFailed.reason == 'ResourceNotFoundException' (matches today) and details == '<Lambda message> (Service: AWSLambda; Status Code: <status>; Error Code: <code>; Request ID: <request id>; Proxy: null)' — one line, message stated once, no Rust/SDK internals. The request id and the generated function name are per-run values filling the template, not part of it.

## Steps to Reproduce
1. Create an IAM execution role for Lambda and a real, Active Lambda function with it (the positive control), plus an IAM role trusted by swf.amazonaws.com allowing lambda:InvokeFunction.
2. RegisterDomain + RegisterWorkflowType in SWF; StartWorkflowExecution passing that role as `lambdaRole`.
3. PollForDecisionTask, then RespondDecisionTaskCompleted with a ScheduleLambdaFunction decision whose `name` is a function name that was never created.
4. GetWorkflowExecutionHistory and read the LambdaFunctionFailed event: AWS's `details` is the single '(Service: AWSLambda; Status Code: 404; Error Code: ResourceNotFoundException; Request ID: <id>; Proxy: null)' line prefixed by Lambda's 'Function not found: <function arn>:$LATEST' message; the emulator's `details` is the Rust error-chain/Debug dump shown above.
5. Repeat the decision with the real function name — it completes, so only the failure's details rendering diverges.
