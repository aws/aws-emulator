Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.lambda-sdk-pipes-describe-missing-permission-negative lambda__pipes__sqs.lambda-sdk-pipes-describe-pipe-positive lambda__pipes__sqs.lambda-sdk-pipes-start-pipe-missing-permission-negative lambda__pipes__sqs.lambda-sdk-pipes-start-pipe-positive lambda__pipes__sqs.lambda-sdk-pipes-stop-pipe-positive lambda__pipes__sqs.lambda-sdk-sqs-receivemessage-from-handler-positive lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-from-handler-positive lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-missing-permission-negative
Issue-Key: lambda__pipes__sqs.lambda-sdk-pipes-describe-missing-permission-negative|_invoke_lambda#9|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Key: lambda__pipes__sqs.lambda-sdk-pipes-describe-pipe-positive|_invoke_lambda#7|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Key: lambda__pipes__sqs.lambda-sdk-pipes-start-pipe-missing-permission-negative|_invoke_lambda#7|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Key: lambda__pipes__sqs.lambda-sdk-pipes-start-pipe-positive|_invoke_lambda#7|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Key: lambda__pipes__sqs.lambda-sdk-pipes-stop-pipe-positive|_invoke_lambda#7|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Key: lambda__pipes__sqs.lambda-sdk-sqs-receivemessage-from-handler-positive|_invoke_lambda#9|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Key: lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-from-handler-positive|_invoke_lambda#5|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Key: lambda__pipes__sqs.lambda-sdk-sqs-sendmessage-missing-permission-negative|_invoke_lambda#7|AssertionError|assert "FunctionError" not in resp, f"lambda invocation raised unhandled error: {resp}"
Issue-Commit: 30d5092aa3ebb101aaddada2a4fffe08f2e86509

# lambda__pipes__sqs: the Python Lambda runtime provides no AWS SDK, so a handler cannot call EventBridge Pipes or SQS — every invocation faults Runtime.ImportModuleError

> _Note: this issue was identified against the code base at commit `30d5092aa3ebb101aaddada2a4fffe08f2e86509`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
This issue carries 1 net-new root cause. It must be closed before submitting.

Cause 1 of 1: the emulator's Python Lambda runtime makes no AWS SDK importable, so a handler that does `import boto3` never runs; every in-function pipes:DescribePipe / pipes:StartPipe / pipes:StopPipe and sqs:SendMessage / sqs:ReceiveMessage call, and the role-scoped allow/deny grading built on those calls, is unreachable.

AWS: Lambda's managed Python runtimes (python3.9 … python3.13) ship boto3 and botocore in the runtime image, so a deployment package containing only the handler module may `import boto3` and call other services with the credentials Lambda injects for the execution role. A successful RequestResponse Invoke answers HTTP 200 with NO X-Amz-Function-Error header and the handler's return value as the payload; when the execution role lacks the action, the SDK call inside the handler raises the service's own authorization error (AccessDeniedException / `User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: <action> on resource: <arn>`), which the handler can catch and return as data — the invocation still answers 200 with no FunctionError. Smithy: Invoke → InvocationResponse.StatusCode (httpResponseCode), FunctionError (X-Amz-Function-Error), Payload (httpPayload); Invoke models no "SDK missing" error, because on AWS the SDK is present. Recorded behavior for these checks: the handler returns the DescribePipe view of the named pipe, or the StartPipe/StopPipe result, or the SendMessage/ReceiveMessage result — and the denied twin returns the AccessDeniedException detail — all with no FunctionError.

Emulator: the import fails before the handler runs. Invoke answers HTTP 200 with the header `x-amz-function-error: Unhandled` (FunctionError 'Unhandled'), ExecutedVersion $LATEST and a 182-byte payload `{"errorMessage": "Unable to import module '<handler-module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-invocation id>", "stackTrace": []}`. Fixed text: `Unable to import module '` … `': No module named 'boto3'` and errorType `Runtime.ImportModuleError`; the variable parts are the handler module taken from the function's Handler and the per-invocation requestId — neither recorded value is part of the requirement.

Why: a client relying on AWS's behavior cannot drive or inspect a pipe, or move a message, from inside a function. Both the positive arms (DescribePipe/StartPipe/StopPipe/SendMessage/ReceiveMessage seen from a handler) and the negative arms fail identically, so a denial produced by the execution role is indistinguishable from a broken deployment package and IAM enforcement observed through a function cannot be graded at all.

Fix: the Lambda Invoke execution path's Python runtime environment — services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/invoke.rs and the runtime/sandbox it delegates to when it imports and calls the handler — must make boto3/botocore importable in the handler interpreter and point their default endpoint and credentials at the emulator, so in-handler client calls are served and authorized like any other request.

Traces: the 8 failing check ids in trace_ids. This cause is already filed for OTHER services (20261105T081727Z-lambda-sns-sqs-the-emulator-s-python-lambda.md, 20261105T081139Z-dynamodb-lambda-s3-secretsmanager-the-in-process-lambda.md); neither carries this service's checks or their replay keys.

## Evidence
Expected (AWS): Invoke of a handler that imports boto3 answers HTTP 200 with no X-Amz-Function-Error / FunctionError and the handler's payload (the DescribePipe view, the StartPipe/StopPipe result, or the SQS Send/Receive result); the denied twin answers 200 with the handler's AccessDeniedException detail.
Actual (emulator): every one of these invocations answers HTTP 200 with 'x-amz-function-error': 'Unhandled' / 'FunctionError': 'Unhandled', content-length 182, and payload {"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", "requestId": "<id>", "stackTrace": []}. Each check fails with `AssertionError: lambda invocation raised unhandled error: {... 'x-amz-function-error': 'Unhandled' ... 'FunctionError': 'Unhandled' ...}`:
  .lambda-sdk-pipes-describe-pipe-positive
  .lambda-sdk-pipes-describe-missing-permission-negative
  .lambda-sdk-pipes-start-pipe-positive
  .lambda-sdk-pipes-start-pipe-missing-permission-negative
  .lambda-sdk-pipes-stop-pipe-positive
  .lambda-sdk-sqs-receivemessage-from-handler-positive
  .lambda-sdk-sqs-sendmessage-from-handler-positive
  .lambda-sdk-sqs-sendmessage-missing-permission-negative

## Steps to Reproduce
CreatePipe with an SQS source and a Lambda target. Then CreateFunction with Runtime=python3.12 and a single-module package whose handler does `import boto3` and calls boto3.client('pipes').describe_pipe(Name=<pipe>) (and a twin that calls sqs.send_message / sqs.receive_message), granting the execution role those actions; Invoke with InvocationType=RequestResponse.
AWS: HTTP 200, no X-Amz-Function-Error, the handler's result.
Emulator: HTTP 200 with x-amz-function-error: Unhandled and payload {"errorMessage": "Unable to import module '<module>': No module named 'boto3'", "errorType": "Runtime.ImportModuleError", ...}.
Repeat with the action removed from the role: AWS returns the handler's AccessDeniedException detail, the emulator returns the identical ImportModuleError. Replay the 8 check ids in trace_ids.
