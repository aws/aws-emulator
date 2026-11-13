Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.error-propagation-queue-not-exist lambda__secretsmanager__sqs.error-propagation-secret-not-found lambda__secretsmanager__sqs.lambda-fetches-secret-by-arn lambda__secretsmanager__sqs.lambda-no-permissions-denied-both-services lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied lambda__secretsmanager__sqs.lambda-sends-message-to-sqs-format lambda__secretsmanager__sqs.lambda-sqs-missing-permission-denied lambda__secretsmanager__sqs.secret-arn-partial-resolution lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret lambda__secretsmanager__sqs.secret-version-stage-resolution lambda__secretsmanager__sqs.shared-state-concurrent-sqs-writes lambda__secretsmanager__sqs.sqs-resource-scoped-denies-other-queue
Issue-Key: lambda__secretsmanager__sqs.error-propagation-queue-not-exist|run#7|KeyError|assert control["payload"]["ok"] is True, (
Issue-Key: lambda__secretsmanager__sqs.error-propagation-secret-not-found|run#8|KeyError|assert control["payload"]["ok"] is True, f"positive control failed: {control}"
Issue-Key: lambda__secretsmanager__sqs.lambda-fetches-secret-by-arn|run#8|KeyError|assert result["payload"]["ok"] is True, (
Issue-Key: lambda__secretsmanager__sqs.lambda-no-permissions-denied-both-services|_invoke_until#11|AssertionError|raise AssertionError(
Issue-Key: lambda__secretsmanager__sqs.lambda-secretsmanager-missing-permission-denied|<lambda>#8|KeyError|lambda r: r["payload"]["ok"] is True,
Issue-Key: lambda__secretsmanager__sqs.lambda-sends-message-to-sqs-format|run#5|KeyError|assert result["payload"]["ok"] is True, (
Issue-Key: lambda__secretsmanager__sqs.lambda-sqs-missing-permission-denied|_invoke_until#8|AssertionError|raise AssertionError(
Issue-Key: lambda__secretsmanager__sqs.secret-arn-partial-resolution|run#11|KeyError|assert result["payload"]["ok"] is True
Issue-Key: lambda__secretsmanager__sqs.secret-resource-scoped-denies-other-secret|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: lambda__secretsmanager__sqs.secret-version-stage-resolution|run#10|KeyError|assert result["payload"]["ok"] is True, f"lambda invoke did not report ok: {result}"
Issue-Key: lambda__secretsmanager__sqs.shared-state-concurrent-sqs-writes|run#6|KeyError|assert r["payload"]["ok"] is True, f"worker {i} invoke did not report ok: {r}"
Issue-Key: lambda__secretsmanager__sqs.sqs-resource-scoped-denies-other-queue|<lambda>#6|KeyError|lambda r: r["payload"]["ok"] is True,
Issue-Commit: 8bf126a687f579c23502fa67f92e47f7c7638042

# lambda__secretsmanager__sqs: the in-process Lambda Python runtime has no AWS SDK, so every Lambda-driven Secrets Manager -> SQS flow answers Runtime.ImportModuleError instead of running the handler

> _Note: this issue was identified against the code base at commit `8bf126a687f579c23502fa67f92e47f7c7638042`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: a python3.x handler that does `import boto3` cannot be imported, so no Lambda-driven Secrets Manager / SQS work ever executes.

AWS: recorded behavior only for the runtime's contents (Smithy: Invoke POST /2015-03-31/functions/{FunctionName}/invocations; output StatusCode via httpResponseCode, FunctionError via header X-Amz-Function-Error, Payload via httpPayload). AWS's managed Python runtimes ship boto3 and botocore on the handler's module search path, so a handler that imports boto3, reads a secret and sends an SQS message initialises normally; Invoke answers 200 with NO X-Amz-Function-Error header and the handler's own JSON document as Payload. Runtime.ImportModuleError is reserved for a module the deployment package genuinely does not provide.

Emulator: HTTP 200 with header `x-amz-function-error: Unhandled` and the payload `{"errorMessage":"Unable to import module '<handler module>': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"<per-invocation id>","stackTrace":[]}`. The fixed text is `Unable to import module '` … `': No module named '` … `'` with errorType `Runtime.ImportModuleError`; the variable parts are the handler module name taken from the function's Handler (observed `lambda_function`) and the missing module name (`boto3`). requestId is a per-invocation value and is not part of the requirement.

Why: no SDK-using function can run, so the whole Secrets-Manager-then-SQS pipeline is unobservable: no message is ever sent, no secret is ever read by function code, and the negative paths cannot distinguish the AccessDeniedException / QueueDoesNotExist / ResourceNotFoundException the handler should surface from the emulator's own import failure — the error-propagation and resource-scoped-deny checks all die reading the handler's reply (`KeyError: 'ok'`), and the control arms that grant every permission report `control ... succeeds ... not met within 45.0s. Last result: {... 'Runtime.ImportModuleError' ...}`.

Fix: the Invoke execution path's runtime environment in the Lambda service (services/service-com-amazonaws-lambda-awsgirapiservice) — the in-process python3.x runtime that runs the deployment package must expose the managed runtime's bundled dependency set (boto3 + botocore, pointed at this emulator's endpoint with the function's execution-role credentials) on the handler's module search path. Related: the fixed issue 20261029T233239Z-lambda-synchronous-invoke-data-plane-answers-501-notimplemented.md stood this runtime up, and the open issue 20261104T160000Z-cloudformation-iam-lambda-s3-six-causes-awaiting-the-lambda-provisioner-and-an-in-process-runtime.md still records Invoke as unimplemented; neither covers the SDK gap reported here. The remaining failures of this group are owned by 20261029T193114Z-lambda-secretsmanager-sqs-2-net-new-causes-sqs.md and are not re-filed.

Traces: the 12 failing checks in trace_ids. Two show the import document verbatim; nine index into the returned document and raise `KeyError: 'ok'`; secret-resource-scoped-denies-other-secret exhausts the budget (`Check timed out after 60.0s`) in the same invoke-until-ok poll loop.

## Evidence
Expected (AWS): Invoke -> HTTP 200, no X-Amz-Function-Error header, Payload = the handler's reply, e.g. {"ok": true, "message_id": "..."}.
Actual (emulator): HTTP 200, `x-amz-function-error: Unhandled`, Payload = {"errorMessage":"Unable to import module 'lambda_function': No module named 'boto3'","errorType":"Runtime.ImportModuleError","requestId":"b71863ed-e4c8-453e-af11-99c6f5f2848a","stackTrace":[]} (requestId per-invocation; 'lambda_function' is the Handler module).
Direct: `AssertionError: control (both permissions granted) succeeds on both services not met within 45.0s. Last result: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named 'boto3'", 'errorType': 'Runtime.ImportModuleError', ...}}` (lambda-no-permissions-denied-both-services) and the same for lambda-sqs-missing-permission-denied.
Downstream: `KeyError: 'ok'` on `control["payload"]["ok"]` in error-propagation-queue-not-exist, error-propagation-secret-not-found, lambda-fetches-secret-by-arn, lambda-secretsmanager-missing-permission-denied, lambda-sends-message-to-sqs-format, secret-arn-partial-resolution, secret-version-stage-resolution, shared-state-concurrent-sqs-writes, sqs-resource-scoped-denies-other-queue.

## Steps to Reproduce
1. Replay the behavioral suite and run the lambda__secretsmanager__sqs group.
2. Or by hand: secretsmanager CreateSecret; sqs CreateQueue; lambda CreateFunction with Runtime=python3.12, Handler=lambda_function.handler and a package whose lambda_function.py does `import boto3`, reads the secret and calls sqs.send_message, returning {"ok": True, ...}.
3. lambda Invoke (RequestResponse) and observe HTTP 200 with x-amz-function-error: Unhandled and the Runtime.ImportModuleError 'No module named boto3' payload; the queue receives no message.
