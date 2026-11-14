Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.alias-weight-ladder-observed-distribution lambda.code-signing-config-switched-to-enforce-refuses-the-next-unsigned-layer-attach lambda.update-function-configuration-signature-errors lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after lambda.log-lambda-report-init-duration-only-on-cold-report-line lambda.deep-lambda-runtime-truth-timeout-overrun-rejected-and-post-deadline-work-never-runs lambda.out-of-memory-kill-discards-the-execution-environment lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.alias-weight-ladder-observed-distribution|run#38|AssertionError|assert over_message == "1 validation error detected: Value '{2=1.5}' at 'routingConfig.additionalVersionWeights' failed to satisfy constraint: Map value must sa
Issue-Key: lambda.code-signing-config-switched-to-enforce-refuses-the-next-unsigned-layer-attach|run#33|AssertionError|assert message == expected_message, (
Issue-Key: lambda.update-function-configuration-signature-errors|run#17|AssertionError|assert message == expected_message, (
Issue-Key: lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after|run#18|AssertionError|assert message == expected, (
Issue-Key: lambda.log-lambda-report-init-duration-only-on-cold-report-line|_report_of#14|AssertionError|assert m, f"REPORT line does not have the documented field layout: {lines[0]!r}"
Issue-Key: lambda.deep-lambda-runtime-truth-timeout-overrun-rejected-and-post-deadline-work-never-runs|run#27|AssertionError|assert "Status: timeout" in report, f"REPORT line lacks 'Status: timeout': {report!r}"
Issue-Key: lambda.out-of-memory-kill-discards-the-execution-environment|_assert_out_of_memory#9|AssertionError|assert body.get("errorType") == "Runtime.OutOfMemory", f"{label}: errorType {body!r}"
Issue-Key: lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state|run#7|AssertionError|assert set(body) == {"errorType", "errorMessage"}, f"{event}: keys {sorted(body)!r}"
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# lambda: 6 net-new causes — alias weight and oversize-Invoke refusal messages, CodeVerificationFailed naming the function instead of the layer, REPORT log line missing its trailing fields, an OOM kill reported as Runtime.ExitError, and extra keys in the error payload

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAlias

## Description
Six net-new root causes in the lambda crate. Each has its own failing check ids.

Cause 1 of 6: the alias routing-weight refusal message is not AWS's map-constraint message.
AWS: Smithy models AliasRoutingConfiguration.AdditionalVersionWeights as a map whose value shape `Weight` carries @range min 0.0 max 1.0. A weight outside that range is refused with a ValidationException whose message is AWS's standard constraint report for a MAP VALUE: `1 validation error detected: Value '{<version>=<weight>}' at 'routingConfig.additionalVersionWeights' failed to satisfy constraint: Map value must satisfy constraint: [Member must have value less than or equal to 1.0, Member must have value greater than or equal to 0.0, Member must not be null]`. The fixed text is everything but `'{<version>=<weight>}'`, whose version key and weight come from the request; the path is the whole map member, not an element of it, and the bounds are rendered as `1.0`/`0.0`.
Emulator: the message reports the offending element instead of the map and drops two of the three constraint clauses: `1 validation error detected: Value '1.5' at 'routingConfig.additionalVersionWeights.2' failed to satisfy constraint: Member must have value less than or equal to 1`.
Why: a client that surfaces or matches AWS's validation report gets a different path and a different constraint list, and the `1` vs `1.0` bound makes a pinned expectation fail.
Fix: the lambda alias validation path's constraint-report formatter for map-valued members (UpdateAlias/CreateAlias RoutingConfig).
Traces: lambda.alias-weight-ladder-observed-distribution.

Cause 2 of 6: CodeVerificationFailedException names the function ARN when the artifact that failed verification is a LAYER version.
AWS: Smithy binds CodeVerificationFailedException (httpError 400, members {Type, Message}) to the deploy operations. Its message is `Lambda cannot deploy the function. The function or layer might be signed using a signature that the client is not configured to accept. Check the provided signature for <resource>.` — the two sentences are fixed text and `<resource>` is the ARN of the artifact that actually failed verification: the LAYER VERSION ARN (`arn:aws:lambda:<region>:<account>:layer:<name>:<version>`) when an unsigned/mis-signed layer is attached, and the function ARN when the function's own package is at fault.
Emulator: for a layer attach under an Enforce CodeSigningConfig the message always names the FUNCTION ARN, so the refusal does not identify the offending layer.
Why: the message is the only place the refusal says which artifact to re-sign; naming the function sends the client to re-sign the wrong thing, and a function with several layers gives no way to find the bad one.
Fix: the code-signing enforcement path for layer attachment (UpdateFunctionConfiguration's Layers validation) — it must render the failing layer version's ARN into the message.
Traces: lambda.code-signing-config-switched-to-enforce-refuses-the-next-unsigned-layer-attach, lambda.update-function-configuration-signature-errors.

Cause 3 of 6: the oversize synchronous Invoke refusal message is the emulator's own sentence.
AWS: Smithy binds RequestTooLargeException (httpError 413) to Invoke for a payload over the synchronous request quota. The message is `Request must be smaller than <limit> bytes for the InvokeFunction operation` — fixed text with the quota (6291456 for RequestResponse) as the only variable part; it names the limit, not the submitted size, and names the `InvokeFunction` operation.
Emulator: the refusal reads `<n> byte payload is too large for the RequestResponse invocation type (limit 6291456 bytes)` — a different sentence that leads with the submitted size and names the invocation type instead of the operation.
Why: a client matching on AWS's quota sentence (a common way to distinguish a payload-quota refusal from other 413s) does not match, and the message does not read as Lambda's.
Fix: the Invoke handler's request-size validation message.
Traces: lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after.

Cause 4 of 6: the REPORT log line omits its documented trailing fields.
AWS: every invocation writes a REPORT line with the tab-separated field layout `REPORT RequestId: <id>\tDuration: <d> ms\tBilled Duration: <b> ms\tMemory Size: <m> MB\tMax Memory Used: <u> MB`, plus `\tInit Duration: <i> ms` on a cold start only, and `\tStatus: timeout` (and the matching error fields) when the invocation was killed by the timeout. Documented in the Lambda developer guide, 'Lambda function logs'; recorded behavior as far as the model goes.
Emulator: the REPORT line stops after `Memory Size` — `REPORT RequestId: <id>\tDuration: <d> ms\tBilled Duration: <b> ms\tMemory Size: <m> MB` — so `Max Memory Used` is always absent, `Init Duration` never distinguishes a cold report line from a warm one, and a timed-out invocation's report carries no `Status: timeout`.
Why: the REPORT line is the documented source for per-invocation memory and cold-start accounting and for recognising a timeout from logs; log-based cost/latency tooling and timeout alarms built on it read nothing.
Fix: the lambda invocation log writer's REPORT-line formatter (cold-start, max-memory and timeout-status fields).
Traces: lambda.log-lambda-report-init-duration-only-on-cold-report-line, lambda.deep-lambda-runtime-truth-timeout-overrun-rejected-and-post-deadline-work-never-runs.

Cause 5 of 6: an out-of-memory kill is reported as Runtime.ExitError instead of Runtime.OutOfMemory.
AWS: when a handler's allocation exceeds the function's MemorySize the execution environment is killed and the invocation's error payload carries `errorType` `Runtime.OutOfMemory` (and the environment is discarded), distinct from `Runtime.ExitError`, which is a runtime process exiting for another reason. Documented in the Lambda developer guide ('Troubleshoot invocation issues'); recorded behavior only as far as the model goes.
Emulator: the same invocation answers `{'errorType': 'Runtime.ExitError', 'errorMessage': 'RequestId: <id> Error: Runtime exited with error: signal: killed', ...}`, so the memory cause is lost.
Why: Runtime.OutOfMemory is the signal that tells a client to raise MemorySize; reported as a generic exit error, an OOM is indistinguishable from a crash and the fix is invisible.
Fix: the invocation error classifier — a sandbox kill attributable to the memory limit must be reported as Runtime.OutOfMemory with AWS's message.
Traces: lambda.out-of-memory-kill-discards-the-execution-environment.

Cause 6 of 6: the invocation error payload carries keys AWS does not return for a runtime-exit error.
AWS: for a Runtime.ExitError the response payload is exactly `{"errorType": "Runtime.ExitError", "errorMessage": "RequestId: <id> Error: Runtime exited with error: <detail>"}` — two keys and no more; the request id lives inside errorMessage, not as its own member, and there is no stackTrace member for a process-level exit.
Emulator: the payload is `{'errorMessage': ..., 'errorType': 'Runtime.ExitError', 'requestId': ..., 'stackTrace': []}` — two extra keys.
Why: clients deserialise this payload into a fixed error shape and some assert on its key set; extra members change the error document a caller stores or forwards.
Fix: the runtime error-payload serializer for process-exit errors — emit only errorType and errorMessage.
Traces: lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state.

## Evidence
Cause 1 — lambda.alias-weight-ladder-observed-distribution: AssertionError: expected the exact ValidationException message AWS returns, got "1 validation error detected: Value '1.5' at 'routingConfig.additionalVersionWeights.2' failed to satisfy constraint: Member must have value less than or equal to 1" (AWS: "1 validation error detected: Value '{2=1.5}' at 'routingConfig.additionalVersionWeights' failed to satisfy constraint: Map value must satisfy constraint: [Member must have value less than or equal to 1.0, Member must have value greater than or equal to 0.0, Member must not be null]"). Smithy: AdditionalVersionWeights value shape Weight @range 0.0..1.0.
Cause 2 — lambda.code-signing-config-switched-to-enforce-refuses-the-next-unsigned-layer-attach / lambda.update-function-configuration-signature-errors: the Enforce refusal message ends "... Check the provided signature for arn:aws:lambda:<region>:<account>:function:<fn>." where AWS names "arn:aws:lambda:<region>:<account>:layer:<layer>:<version>.". Smithy: CodeVerificationFailedException httpError 400, members {Type, Message}.
Cause 3 — lambda.deep-lambda-runtime-truth-oversize-request-rejected-before-handler-oversize-response-after: expected message 'Request must be smaller than 6291456 bytes for the InvokeFunction operation', got '<n> byte payload is too large for the RequestResponse invocation type (limit 6291456 bytes)'. Smithy: Invoke error RequestTooLargeException httpError 413.
Cause 4 — lambda.log-lambda-report-init-duration-only-on-cold-report-line: AssertionError: REPORT line does not have the documented field layout: 'REPORT RequestId: <id>\tDuration: 1232.53 ms\tBilled Duration: 1233 ms\tMemory Size: 160 MB'. lambda.deep-lambda-runtime-truth-timeout-overrun-rejected-and-post-deadline-work-never-runs: REPORT line lacks 'Status: timeout'.
Cause 5 — lambda.out-of-memory-kill-discards-the-execution-environment: errorType {'errorMessage': 'RequestId: <id> Error: Runtime exited with error: signal: killed', 'errorType': 'Runtime.ExitError', ...}; expected errorType 'Runtime.OutOfMemory'.
Cause 6 — lambda.python-sys-exit-reports-runtime-exit-error-and-resets-state: keys ['errorMessage', 'errorType', 'requestId', 'stackTrace']; expected exactly {'errorType', 'errorMessage'}.

## Steps to Reproduce
Cause 1: UpdateAlias/CreateAlias with RoutingConfig.AdditionalVersionWeights = {"2": 1.5} and compare the ValidationException message.
Cause 2: attach an Enforce CodeSigningConfig to a function, then UpdateFunctionConfiguration attaching a layer version that is unsigned or signed by a profile outside AllowedPublishers; read the CodeVerificationFailedException Message and check which ARN it names.
Cause 3: Invoke with a RequestResponse payload larger than 6291456 bytes and compare the 413 message.
Cause 4: invoke a function cold and then warm with LogType Tail and read the REPORT lines; invoke a handler that outlives its Timeout and read the REPORT line.
Cause 5: invoke a 128 MB function whose handler allocates ~400 MB and read the response payload's errorType.
Cause 6: invoke a Python handler that calls sys.exit(1) and compare the response payload's key set.
Then replay the check ids in trace_ids.
