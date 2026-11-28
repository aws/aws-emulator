Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.function-url-on-tenant-isolated-function-rejected lambda.tenant-isolated-function-url-config-rejected lambda.provisioned-concurrency-on-tenant-isolated-function-rejected lambda.provisioned-concurrency-on-tenant-isolated-version-rejected lambda.tenant-isolated-provisioned-concurrency-rejected lambda.snapstart-on-tenant-isolated-function-rejected lambda.snapstart-with-tenant-isolation-rejected-at-create lambda.tenant-isolated-snapstart-rejected lambda.stream-tenant-id-on-function-without-tenancy-rejected lambda.dry-run-invoke-does-not-require-tenant-id-on-tenant-isolated-function lambda.invoke-client-context-over-3583-bytes-rejected lambda.response-stream-client-context-over-3583-bytes-rejected lambda.stream-invalid-client-context-rejected lambda.event-invoke-accepts-client-context-over-the-sync-limit lambda.event-invoke-drops-client-context-sync-invoke-delivers-it lambda.response-stream-invoke-payload-past-six-mb-rejected lambda.response-stream-invoke-request-at-six-mib-streamed-one-byte-over-rejected lambda.stream-invoke-request-one-byte-over-6mb-refused-before-handler lambda.stream-payload-one-byte-over-six-mib-rejected lambda.invoke-recursion-depth-exceeded-rejected lambda.stream-self-invocation-chain-stopped-by-recursion-guardrail-rejected lambda.invoke-capacity-provider-function-latest-qualifier-rejected lambda.invoke-unpublished-capacity-provider-function-rejected-no-published-version lambda.invoke-with-response-stream-runs-synchronously-for-every-invocation-type lambda.sqs-esm-tumbling-window-rejected lambda.snapstart-after-restore-hook-past-ten-seconds-invoke-rejected lambda.invoke-snapstart-after-restore-hook-that-raises-is-rejected lambda.code-location-download-requires-signature lambda.layer-content-download-requires-signature lambda.layer-download-signature-is-bound-to-the-object-key
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.function-url-on-tenant-isolated-function-rejected|_error_code#10|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: lambda.tenant-isolated-function-url-config-rejected|_refusal#13|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {outcome!r}")
Issue-Key: lambda.provisioned-concurrency-on-tenant-isolated-function-rejected|_error_code#12|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: lambda.provisioned-concurrency-on-tenant-isolated-version-rejected|run#21|AssertionError|raise AssertionError(f"provisioned concurrency was accepted: {leaked.get('Status')!r}")
Issue-Key: lambda.tenant-isolated-provisioned-concurrency-rejected|_refusal#10|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {outcome!r}")
Issue-Key: lambda.snapstart-on-tenant-isolated-function-rejected|_error_code#12|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: lambda.snapstart-with-tenant-isolation-rejected-at-create|run#13|AssertionError|assert isinstance(refused, ClientError), (
Issue-Key: lambda.tenant-isolated-snapstart-rejected|_refusal#10|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {outcome!r}")
Issue-Key: lambda.stream-tenant-id-on-function-without-tenancy-rejected|_refusal#7|AssertionError|raise AssertionError(f"the request was expected to be refused, but it returned {result!r}")
Issue-Key: lambda|Invoke|InvalidParameterValueException
Issue-Key: lambda.invoke-client-context-over-3583-bytes-rejected|_refusal#24|AssertionError|raise AssertionError(f"the oversized client context was accepted: {result!r}")
Issue-Key: lambda.response-stream-client-context-over-3583-bytes-rejected|_refusal#27|AssertionError|raise AssertionError(f"the oversized client context was accepted: {result!r}")
Issue-Key: lambda.stream-invalid-client-context-rejected|_refusal#8|AssertionError|raise AssertionError(f"the request was expected to be refused, but it returned {result!r}")
Issue-Key: lambda.event-invoke-accepts-client-context-over-the-sync-limit|_refusal#24|AssertionError|raise AssertionError(f"the synchronous invoke accepted the oversized context: {result!r}")
Issue-Key: lambda.event-invoke-drops-client-context-sync-invoke-delivers-it|run#28|AssertionError|assert all(seen == {"context_none": True, "custom": None} for seen in event_seen), (
Issue-Key: lambda.response-stream-invoke-payload-past-six-mb-rejected|run#19|AssertionError|assert got == expected, f"a {_LIMIT + 1}-byte payload was answered {got!r}, expected {expected!r}"
Issue-Key: lambda.response-stream-invoke-request-at-six-mib-streamed-one-byte-over-rejected|run#21|AssertionError|assert got == ("RequestEntityTooLargeException",
Issue-Key: lambda.stream-invoke-request-one-byte-over-6mb-refused-before-handler|run#16|AssertionError|assert got == (
Issue-Key: lambda.stream-payload-one-byte-over-six-mib-rejected|run#10|AssertionError|assert code == "RequestEntityTooLargeException", (
Issue-Key: lambda.invoke-recursion-depth-exceeded-rejected|run#24|AssertionError|assert refused == (
Issue-Key: lambda.stream-self-invocation-chain-stopped-by-recursion-guardrail-rejected|_start_chain#10|AssertionError|assert failure is None, f"the top-level invocation failed with {failure!r}: {payload[:300]!r}"
Issue-Key: lambda.invoke-capacity-provider-function-latest-qualifier-rejected|_refusal#31|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {got!r}")
Issue-Key: lambda.invoke-unpublished-capacity-provider-function-rejected-no-published-version|_refusal#32|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {got!r}")
Issue-Key: lambda.invoke-with-response-stream-runs-synchronously-for-every-invocation-type|run#12|AssertionError|assert resp["StatusCode"] == 200, f"{kind}: stream status {resp['StatusCode']!r}"
Issue-Key: lambda.sqs-esm-tumbling-window-rejected|_refused#30|AssertionError|raise AssertionError(f"expected a refusal, got an accepted call returning {answer!r}")
Issue-Key: lambda.snapstart-after-restore-hook-past-ten-seconds-invoke-rejected|run#12|AssertionError|assert control["StatusCode"] == 200 and "FunctionError" not in control, (
Issue-Key: lambda.invoke-snapstart-after-restore-hook-that-raises-is-rejected|_refusal#6|AssertionError|raise AssertionError(f"expected the invoke to be refused, it answered {out.get('StatusCode')!r}"
Issue-Key: lambda.code-location-download-requires-signature|run#15|AssertionError|assert refusal == (403, "SignatureDoesNotMatch", _SIGNATURE_DOES_NOT_MATCH), (
Issue-Key: lambda.layer-content-download-requires-signature|run#16|AssertionError|assert refusal == (400, "AuthorizationQueryParametersError", _MISSING_QUERY_AUTH), (
Issue-Key: lambda.layer-download-signature-is-bound-to-the-object-key|run#12|AssertionError|assert refusal == (403, "SignatureDoesNotMatch", _SIGNATURE_DOES_NOT_MATCH), (
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# lambda: tenant isolation, client context, oversize-payload and recursion limits are not enforced on the invoke and concurrency paths (10 causes)

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Ten net-new root causes on Lambda's invoke, concurrency and artifact-download paths. Each is a guard AWS applies that the emulator does not, or applies with the wrong error.

Cause 1 of 10: a function with TenancyConfig.TenantIsolationMode = PER_TENANT accepts features AWS forbids on it.
AWS: tenant-isolated functions may not have a function URL, provisioned concurrency, or SnapStart; each of CreateFunctionUrlConfig, PutProvisionedConcurrencyConfig and CreateFunction/UpdateFunctionConfiguration with SnapStart.ApplyOn=PublishedVersions is refused for such a function (recorded behavior: a 400-class refusal, nothing configured).
Emulator: accepts all of them — CreateFunctionUrlConfig answers 201 with a FunctionUrl, PutProvisionedConcurrencyConfig answers 202 with Status IN_PROGRESS, and CreateFunction answers 201/200 carrying both `'SnapStart': {'ApplyOn': 'PublishedVersions'}` and `'TenancyConfig': {'TenantIsolationMode': 'PER_TENANT'}`.
Why: a client building a multi-tenant function gets a configuration AWS would never have created, so it ships code whose isolation guarantees do not hold and which fails only at deploy time against real AWS.
Fix: a tenancy precondition check shared by the CreateFunctionUrlConfig, PutProvisionedConcurrencyConfig, CreateFunction and UpdateFunctionConfiguration handlers.
Traces: lambda.function-url-on-tenant-isolated-function-rejected, lambda.tenant-isolated-function-url-config-rejected, lambda.provisioned-concurrency-on-tenant-isolated-function-rejected, lambda.provisioned-concurrency-on-tenant-isolated-version-rejected, lambda.tenant-isolated-provisioned-concurrency-rejected, lambda.snapstart-on-tenant-isolated-function-rejected, lambda.snapstart-with-tenant-isolation-rejected-at-create, lambda.tenant-isolated-snapstart-rejected.

Cause 2 of 10: the TenantId requirement is applied in the wrong direction.
AWS: a DryRun invoke of a tenant-isolated function does not require a TenantId (no execution environment is entered), while InvokeWithResponseStream carrying a TenantId for a function with no tenancy configuration is refused.
Emulator: refuses the DryRun with 400 InvalidParameterValueException `The invoked function is enabled with tenancy configuration. Add a valid tenant ID in your request and try again.`, and accepts the TenantId on a non-tenant function, answering 200 with an event stream.
Why: permission/configuration smoke tests that use DryRun cannot run at all against a tenant-isolated function, and a client that leaks a TenantId onto the wrong function is never told.
Fix: the tenancy gate in the Invoke and InvokeWithResponseStream handlers — exempt DryRun, and reject TenantId when the target has no TenancyConfig.
Traces: lambda.dry-run-invoke-does-not-require-tenant-id-on-tenant-isolated-function, lambda.stream-tenant-id-on-function-without-tenancy-rejected.

Cause 3 of 10: the ClientContext limit is not enforced, and Event invocations are not stripped of it.
AWS: the X-Amz-Client-Context on a synchronous Invoke/InvokeWithResponseStream is capped (3583 bytes of base64) and an over-cap value is refused; an Event (asynchronous) invocation accepts a larger context but does NOT deliver it, so the handler sees no client context.
Emulator: accepts the oversized context on Invoke (200, ExecutedVersion $LATEST), on InvokeWithResponseStream (200, eventstream), and on the stream path with an invalid context; and it delivers the client context to the Event invocation's handler, which logged `[{'context_none': False, 'custom': {'app': 'orders', 'build': 'c184d2ac22d5'}}]` where AWS gives none.
Why: a client that sizes its context against the documented cap sees it silently accepted here and refused in production, and handlers written against AWS's 'Event invocations have no client context' rule take the wrong branch.
Fix: ClientContext length/decode validation and the per-InvocationType delivery rule in the Invoke and InvokeWithResponseStream handlers.
Traces: lambda.invoke-client-context-over-3583-bytes-rejected, lambda.response-stream-client-context-over-3583-bytes-rejected, lambda.stream-invalid-client-context-rejected, lambda.event-invoke-accepts-client-context-over-the-sync-limit, lambda.event-invoke-drops-client-context-sync-invoke-delivers-it.

Cause 4 of 10: the oversize-payload refusal on the streaming invoke path uses the wrong error code and message.
AWS: a payload one byte over 6291456 on InvokeWithResponseStream is refused with RequestEntityTooLargeException and the message `<N> byte payload is too large for the RequestResponse invocation type (limit 6291456 bytes)`, where `<N>` is the measured request size and 6291456 the fixed limit. Smithy: the lambda model carries RequestTooLargeException (httpError 413) but no RequestEntityTooLargeException, so this code and sentence are recorded behavior only for this operation.
Emulator: answers RequestTooLargeException with `Request must be smaller than 6291456 bytes for the RequestResponse invocation type` and HTTP 413.
Why: a client catching RequestEntityTooLargeException to chunk its payload never catches it, so the oversize request surfaces as an unhandled error class.
Fix: the oversize-payload refusal in the InvokeWithResponseStream handler — error code, message template and status.
Traces: lambda.response-stream-invoke-payload-past-six-mb-rejected, lambda.response-stream-invoke-request-at-six-mib-streamed-one-byte-over-rejected, lambda.stream-invoke-request-one-byte-over-6mb-refused-before-handler, lambda.stream-payload-one-byte-over-six-mib-rejected.

Cause 5 of 10: the recursive-invocation guardrail trips at the wrong depth and does not stop a streaming self-invocation chain.
AWS: Lambda counts recursion depth and stops the chain at depth 16, answering RecursiveInvocationException `Lambda has detected your function being invoked in a recursive loop with other Amazon Web Services resources and stopped your function's invocation.` A streaming self-invocation chain is stopped by the same guardrail rather than running until the sandbox times out.
Emulator: lets a 24-level chain run to level 17 before raising, and on the streaming path never trips at all — the top-level invocation ends `Unhandled` with `{"errorType":"Sandbox.Timedout","errorMessage":"RequestId: <id> Error: Task timed out after 240.00 seconds"}`.
Why: a client's runaway-loop protection is reported at a different depth, and on the streaming path the loop burns the whole timeout instead of being cut, so cost and latency guarantees differ.
Fix: recursion-depth accounting in the Invoke and InvokeWithResponseStream handlers (the depth counter propagated through the invocation context).
Traces: lambda.invoke-recursion-depth-exceeded-rejected, lambda.stream-self-invocation-chain-stopped-by-recursion-guardrail-rejected.

Cause 6 of 10: capacity-provider invoke preconditions are missing.
AWS: invoking a capacity-provider-backed function with the $LATEST qualifier is refused, and so is invoking one that has no published version.
Emulator: answers 200 with `ExecutedVersion: '$LATEST'` in both cases.
Why: a client learns nothing about the capacity-provider contract and builds a deployment that real AWS refuses.
Fix: the capacity-provider precondition in the Invoke handler.
Traces: lambda.invoke-capacity-provider-function-latest-qualifier-rejected, lambda.invoke-unpublished-capacity-provider-function-rejected-no-published-version.

Cause 7 of 10: InvokeWithResponseStream does not run synchronously for every InvocationType.
AWS: InvokeWithResponseStream is synchronous whatever InvocationType is supplied; a DryRun still answers with the 200 streaming status, not a 204.
Emulator: answers stream status 204 for DryRun.
Why: a client that validates permissions with DryRun on the streaming API cannot distinguish success from a missing body.
Fix: InvocationType handling in the InvokeWithResponseStream handler's response path.
Traces: lambda.invoke-with-response-stream-runs-synchronously-for-every-invocation-type.

Cause 8 of 10: CreateEventSourceMapping accepts TumblingWindowInSeconds for an SQS source.
AWS: tumbling windows are a Kinesis/DynamoDB-stream feature; an SQS event source mapping that sets TumblingWindowInSeconds is refused.
Emulator: answers 202 and stores the mapping with `'TumblingWindowInSeconds': 60` against an SQS EventSourceArn.
Why: a client gets a mapping whose declared windowing never takes effect, and the same template is refused by AWS.
Fix: per-source-type option validation in the CreateEventSourceMapping handler.
Traces: lambda.sqs-esm-tumbling-window-rejected.

Cause 9 of 10: the Python runtime has no `snapshot_restore_py` module, so SnapStart restore hooks never register and the hook-failure refusals never happen.
AWS: a python runtime with SnapStart exposes `snapshot_restore_py`; an after-restore hook that raises, or that overruns the ten-second budget, makes the invoke fail with the restore error.
Emulator: the handler cannot even import the module — the invoke answers 200 `Unhandled` with `{'errorMessage': "Unable to import module 'app': No module named 'snapshot_restore_py'", 'errorType': 'Runtime.ImportModuleError'}` — so the graded refusal is replaced by an import failure.
Why: no SnapStart-aware function can be exercised at all, and the import error masks the behavior under test.
Fix: ship `snapshot_restore_py` in the Python runtime layer of the invoke sandbox and wire its registered hooks into the restore phase.
Traces: lambda.snapstart-after-restore-hook-past-ten-seconds-invoke-rejected, lambda.invoke-snapstart-after-restore-hook-that-raises-is-rejected.

Cause 10 of 10: the presigned URLs in Code.Location and LayerVersion Content.Location are not signature-checked.
AWS: those URLs are presigned; a GET with a zeroed or altered signature is refused 403 SignatureDoesNotMatch, a GET with X-Amz-Signature removed is refused 400 AuthorizationQueryParametersError, and a signature issued for one object key does not authorise another key (403 SignatureDoesNotMatch).
Emulator: answers 403 with no error code or body at all in every case (`(403, None, None)`), so a stripped-signature request gets the wrong status and none of the three answers carries the error code AWS returns.
Why: a client cannot distinguish 'presign expired/tampered' from 'forbidden', and tooling that retries on AuthorizationQueryParametersError never sees it.
Fix: presign verification and error serialisation on the artifact-download endpoint that serves Code.Location and Content.Location.
Traces: lambda.code-location-download-requires-signature, lambda.layer-content-download-requires-signature, lambda.layer-download-signature-is-bound-to-the-object-key.

## Evidence
Cause 1: `expected a refusal, but the call succeeded: {... 'FunctionUrl': 'https://<host>/_aws/lambda-url/<id>/', 'AuthType': 'AWS_IAM' ...}`; `expected a refusal, but the call succeeded: {... 'Status': 'IN_PROGRESS' ...}`; `expected a refusal, but the call succeeded: {... 'SnapStart': {'ApplyOn': 'PublishedVersions'}, ... 'TenancyConfig': {'TenantIsolationMode': 'PER_TENANT'}}`.
Cause 2: `botocore.errorfactory.InvalidParameterValueException: ... The invoked function is enabled with tenancy configuration. Add a valid tenant ID in your request and try again.` on a DryRun; `the request was expected to be refused, but it returned {... 'ResponseStreamContentType': 'application/vnd.amazon.eventstream' ...}` for a TenantId on a non-tenant function.
Cause 3: `the oversized client context was accepted: {... 'StatusCode': 200 ...}`; `the Event invocation's handler logged [{'context_none': False, 'custom': {...}}], expected no client context`.
Cause 4: `a 6291457-byte payload was answered ('RequestTooLargeException', 'Request must be smaller than 6291456 bytes for the RequestResponse invocation type'), expected ('RequestEntityTooLargeException', '6291457 byte payload is too large for the RequestResponse invocation type (limit 6291456 bytes)')`.
Cause 5: `the 24-level chain ended with ('RecursiveInvocationException', "...stopped your function's invocation.") at level 17`; streaming chain: `Task timed out after 240.00 seconds`.
Cause 6: `the call was expected to be refused but returned {... 'StatusCode': 200, 'ExecutedVersion': '$LATEST' ...}`.
Cause 7: `DryRun: stream status 204`.
Cause 8: `expected a refusal, got an accepted call returning {... 'TumblingWindowInSeconds': 60, 'EventSourceArn': 'arn:aws:sqs:...' ...}`.
Cause 9: `quick-hook version answered 200 'Unhandled': {'errorMessage': "Unable to import module 'app': No module named 'snapshot_restore_py'", 'errorType': 'Runtime.ImportModuleError'}`.
Cause 10: `a GET of Code.Location with a zeroed signature returned (403, None, None); a forged presign must be refused with 403 SignatureDoesNotMatch`; `a presign missing X-Amz-Signature must be refused with 400 AuthorizationQueryParametersError`.

## Steps to Reproduce
Replay the 30 check ids in trace_ids. Minimal repros: (1) create a function with `--tenancy-config TenantIsolationMode=PER_TENANT`, then `create-function-url-config` / `put-provisioned-concurrency-config` / `--snap-start ApplyOn=PublishedVersions` — each is accepted; (2) `invoke --invocation-type DryRun` on that function — refused with InvalidParameterValueException; (3) `invoke --client-context <3584+ bytes of base64>` — accepted; and `invoke --invocation-type Event --client-context <ctx>` — the handler receives it; (4) `invoke-with-response-stream` with a 6291457-byte payload and compare error code/message; (5) chain 24 self-invocations and note the depth at which RecursiveInvocationException appears; (6) invoke a capacity-provider function with `--qualifier '$LATEST'`; (7) `invoke-with-response-stream --invocation-type DryRun`; (8) `create-event-source-mapping --event-source-arn <sqs queue> --tumbling-window-in-seconds 60`; (9) deploy a python function importing `snapshot_restore_py` and invoke it; (10) GET the `Code.Location` URL from `get-function` with the signature zeroed, removed, and re-pointed at another key.
