Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.get-durable-execution-state-invalid-checkpoint-token-rejected lambda.durable-execution-kms-enforcement lambda.durable-execution-name-conflict-rejected lambda.nodejs-missing-module-rejected-as-import-module-error lambda.layer-nodejs-other-major-directory-not-resolvable-rejected lambda.nodejs-pending-timer-runs-in-next-invocation-when-not-waiting
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.get-durable-execution-state-invalid-checkpoint-token-rejected|run#10|AssertionError|assert code == "InvalidParameterValueException", f"expected InvalidParameterValueException, got {code}"
Issue-Key: lambda.durable-execution-kms-enforcement|run#13|AssertionError|assert msg == disabled_msg, f"GetDurableExecution KMS refusal message: {msg!r}"
Issue-Key: lambda.durable-execution-name-conflict-rejected|run#11|AssertionError|assert msg == f"Execution already started: {arn1}", f"name-reuse refusal message: {msg!r}"
Issue-Key: lambda.nodejs-missing-module-rejected-as-import-module-error|run#8|AssertionError|assert body["errorType"] == etype, f"{list(files)}: expected errorType {etype!r}, got {body!r}"
Issue-Key: lambda.layer-nodejs-other-major-directory-not-resolvable-rejected|run#17|AssertionError|assert body.get("trace", [None])[0] == "Error: Cannot find module 'sim2real-l8-other'", (
Issue-Key: lambda.nodejs-pending-timer-runs-in-next-invocation-when-not-waiting|run#7|AssertionError|assert seen == {"phase": "observe", "fired": [tag]}, (
Issue-Commit: c607a2586d7f91da73a3e70d6fec1fead9553942

# lambda: 7 net-new causes — InvokeWithResponseStream delivers no chunks, GetDurableExecutionState answers NotImplemented, two durable-execution refusal messages are emulator-authored, and three Node.js runtime behaviours diverge

> _Note: this issue was identified against the code base at commit `c607a2586d7f91da73a3e70d6fec1fead9553942`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

> _Resolution: causes 2 to 7 are fixed — `GetDurableExecutionState` is implemented and validates
> its `CheckpointToken`, the durable-execution KMS and name-conflict refusals carry AWS's
> wording, node's ES-module resolution failures are reported as node's own resolver reports
> them, the require-stack trace is shaped like node's, and a node execution environment is kept
> standing so work a handler left on its event loop runs during the next invocation it serves.
> CAUSE 1 is NOT fixed: no service crate can deliver a response incrementally, because the
> emulator's own service contract hands the whole response over as one buffered value
> (`vendor/server/src/lib.rs:183`, `vendor/smithy-server-runtime/src/multi_service_router.rs:602`,
> both read-only). It is re-filed with its `Issue-Key` and its trace id as
> `issues/things-that-do-not-work/20261128T120000Z-lambda-invokewithresponsestream-answers-its-whole-event-stream-at-once.md`,
> so that check is not recorded as working; the same outstanding work is also named by
> `20261027T180000Z-lambda-invokewithresponsestream-refuses-every-real-streaming-invocation.md`._

## Operation
InvokeWithResponseStream

## Description
Seven distinct root causes behind seven failing checks in the lambda group, none of them carried by an open issue (the nearest neighbours — 20261107T091747Z-lambda-code-signing-not-enforced-no-in-flight.md for streaming, 20261107T135301Z-lambda-9-net-new-root-causes-in-durable.md for durable execution, 20261107T091747Z-lambda-lambda-layers-are-never-materialized-into-the.md for layers, 20261107T135301Z-lambda-10-net-new-root-causes-in-the--r2.md for the Node runtime — do not list these checks).

Cause 1 of 7: InvokeWithResponseStream delivers no response chunks at all. RE-FILED, NOT FIXED —
see the resolution note above and
`issues/things-that-do-not-work/20261128T120000Z-lambda-invokewithresponsestream-answers-its-whole-event-stream-at-once.md`.
AWS: Smithy lambda InvokeWithResponseStream (POST /2021-11-15/functions/{FunctionName}/response-streaming-invocations) returns output member EventStream (streaming, httpPayload) carrying PayloadChunk events as the handler writes them, followed by InvokeComplete; a handler that writes three chunks and then errors delivers the already-written chunks and reports the error in the terminating event.
Emulator: the client observes no chunks whatsoever — the control leg that must see the handler's records 1, 2, 3 sees an empty list, so neither the streamed prefix nor the trailing error can be asserted.
Why: a streaming client gets an empty body for a successful stream; progressive delivery, and the AWS contract that bytes already written survive a later handler error, are both unobservable.
Fix: the InvokeWithResponseStream handler and its event-stream serializer (PayloadChunk / InvokeComplete framing), plus the runtime side that forwards handler writes as they happen.
Traces: lambda.deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer

Cause 2 of 7: GetDurableExecutionState does not validate CheckpointToken; it answers NotImplemented.
AWS: Smithy lambda GetDurableExecutionState (GET /2025-12-01/durable-executions/{DurableExecutionArn}/state, required CheckpointToken with pattern ^[A-Za-z0-9+/]+={0,2}$) models InvalidParameterValueException (client, HTTP 400) among its errors; a syntactically valid token that was never issued for that execution is refused InvalidParameterValueException with the message "Invalid checkpoint token" (fixed text; no request or state values in it).
Emulator: the call is refused with error code NotImplemented, so no 400/InvalidParameterValueException and no message.
Why: a durable-execution client cannot distinguish a forged/stale checkpoint token from an unsupported API, and retry logic keyed on InvalidParameterValueException never triggers.
Fix: the GetDurableExecutionState operation handler and its CheckpointToken validation.
Traces: lambda.get-durable-execution-state-invalid-checkpoint-token-rejected

Cause 3 of 7: the KMSDisabledException message for durable-execution reads is emulator-authored.
AWS: Smithy lambda GetDurableExecution and StopDurableExecution both model KMSDisabledException (HTTP 502). When the key in the function's DurableConfig is disabled, the message is "Lambda was unable to access your execution data because the KMS key used is disabled. Check the KMS key configured in the function's durable config. KMS Exception: DisabledException KMS Message: <key-arn> is disabled." — all of that is fixed text except <key-arn>, which is the key ARN from the function's durable config.
Emulator: the error code is right but the message is "Lambda was unable to decrypt the durable execution data because the KMS key <key-arn> is disabled. Please check the function's KMS key settings." — different fixed text, and it omits the "KMS Exception: DisabledException KMS Message: ..." tail AWS appends.
Why: a client that matches on AWS's wording (or parses the embedded KMS exception name) misclassifies a key-state failure; message text is part of the error contract.
Fix: the KMS-state refusal path shared by the GetDurableExecution and StopDurableExecution handlers (error message construction).
Traces: lambda.durable-execution-kms-enforcement

Cause 4 of 7: the durable-execution name-conflict refusal message is emulator-authored.
AWS: re-invoking a durable function with a DurableExecutionName already in use and a DIFFERENT payload is refused DurableExecutionAlreadyStartedException with the message "Execution already started: <durable-execution-arn>" — fixed prefix plus the ARN of the existing execution taken from state.
Emulator: the code matches, but the message is "The durable execution <name> has already been started. Each durable execution name must be unique within the function. Use a different name or check the status of the existing execution." — different fixed text, and it names the execution NAME from the request instead of the existing execution's ARN.
Why: a client cannot recover the in-flight execution's ARN from the refusal (AWS's message is how you find it), and message-matching clients break.
Fix: the Invoke handler's durable-execution admission check (the DurableExecutionAlreadyStartedException message).
Traces: lambda.durable-execution-name-conflict-rejected

Cause 5 of 7: an ESM static import of a missing package is misclassified as Runtime.ImportModuleError.
AWS: for nodejs22.x, a CommonJS require of a missing module fails with errorType Runtime.ImportModuleError and errorMessage "Error: Cannot find module '<pkg>'\nRequire stack:\n- /var/task/index.js\n- /var/runtime/index.mjs", while an ES module (index.mjs) whose static import cannot be resolved fails with errorType "Error" and errorMessage "Cannot find package '<pkg>' imported from /var/task/<file>.mjs" (documented under Lambda troubleshooting-invocation). Fixed text apart from <pkg> and the handler file path from the deployment package.
Emulator: the ESM case is answered with errorType 'Runtime.ImportModuleError' and errorMessage "Error: Cannot find module '<pkg>'" — the CJS classification and the CJS wording, with no "Cannot find package ... imported from ..." form.
Why: a client (or an operator reading logs) cannot tell an ESM resolution failure from a CJS one, and error-type routing on Runtime.ImportModuleError vs Error is wrong.
Fix: the Node.js runtime's module-load error mapping in the invoke path (ESM vs CJS error type and message).
Traces: lambda.nodejs-missing-module-rejected-as-import-module-error

Cause 6 of 7: the module-not-found stack trace is not shaped like Node's.
AWS: when a handler's own require fails at invoke time, the error payload's errorMessage carries the message plus the "Require stack:" lines, and trace[0] is just the error line "Error: Cannot find module '<pkg>'", with the following entries being real Node frames (node:internal/modules/cjs/loader, /var/task/index.js, /var/runtime/index.mjs).
Emulator: trace[0] is the whole errorMessage including the embedded "Require stack:" block, and the remaining frames name an internal interpreter script ("at requireNotFound (eval_script:351:80)", "at callHandler (eval_script:1467:46)") rather than Node module-loader frames.
Why: any consumer that splits the trace array per frame, or that greps the first trace line, sees a multi-line blob and emulator-internal file names that do not exist in AWS.
Fix: the Node.js runtime's error serializer (trace array construction) in the invoke path.
Traces: lambda.layer-nodejs-other-major-directory-not-resolvable-rejected

Cause 7 of 7: a timer left pending by one invocation never runs during the next one on a warm environment.
AWS: the Node.js runtime does not drain or reset the event loop between invocations; a setTimeout scheduled by invocation N and not awaited fires during invocation N+1 in the same execution environment, so the next invocation observes its side effect.
Emulator: the next invocation observes nothing — the "fired" list is empty where it must contain the pending timer's tag.
Why: handlers that rely on warm-environment event-loop carry-over (background flushes, deferred writes) silently lose that work, and the AWS behaviour that an unawaited timer is not cancelled is inverted.
Fix: the Node.js runtime's per-invocation lifecycle in the execution environment — keep the event loop alive across invocations in a warm sandbox instead of tearing pending timers down at response time.
Traces: lambda.nodejs-pending-timer-runs-in-next-invocation-when-not-waiting

## Evidence
Replay output (behavioral suite, service heading ### lambda):

1) deep-lambda-runtime-truth-stream-error-after-first-chunk-rejected-in-trailer [mismatch]
   AssertionError: control records [] (the stream's chunks 1,2,3 were never delivered)

2) get-durable-execution-state-invalid-checkpoint-token-rejected [mismatch]
   AssertionError: expected InvalidParameterValueException, got NotImplemented
   (AWS: 400 InvalidParameterValueException, message exactly "Invalid checkpoint token")

3) durable-execution-kms-enforcement [mismatch]
   expected message: "Lambda was unable to access your execution data because the KMS key used is disabled. Check the KMS key configured in the function's durable config. KMS Exception: DisabledException KMS Message: <key-arn> is disabled."
   got message:      "Lambda was unable to decrypt the durable execution data because the KMS key <key-arn> is disabled. Please check the function's KMS key settings."
   (code KMSDisabledException matched; only the message diverges. <key-arn> is the durable config's key ARN, the rest is fixed text.)

4) durable-execution-name-conflict-rejected [mismatch]
   expected message: "Execution already started: <durable-execution-arn>"
   got message:      "The durable execution <name> has already been started. Each durable execution name must be unique within the function. Use a different name or check the status of the existing execution."

5) nodejs-missing-module-rejected-as-import-module-error [mismatch]
   ['index.mjs']: expected errorType 'Error' with errorMessage "Cannot find package '<pkg>' imported from /var/task/index.mjs", got {'errorType': 'Runtime.ImportModuleError', 'errorMessage': "Error: Cannot find module '<pkg>'", 'trace': ["Runtime.ImportModuleError: Error: Cannot find module '<pkg>'"]}

6) layer-nodejs-other-major-directory-not-resolvable-rejected [mismatch]
   trace[0] must be "Error: Cannot find module '<pkg>'"; got trace[0] = "Error: Cannot find module '<pkg>'\nRequire stack:\n- /var/task/index.js\n- /var/runtime/index.mjs" with following frames 'at requireNotFound (eval_script:351:80)', 'at required (eval_script:414:48)', 'at callHandler (eval_script:1467:46)'

7) nodejs-pending-timer-runs-in-next-invocation-when-not-waiting [mismatch]
   AssertionError: the previous invocation's pending timer must run during the next one, got {'phase': 'observe', 'fired': []}

(Package names, key ARNs, execution names and tags above are per-run values filling AWS's message/trace templates; the fixed text and the member/variable placement are the requirement.)

## Steps to Reproduce
1. Build and boot the emulator from the checkout, then replay the behavioral suite (run_behavioral_suite).
2. Re-run exactly the seven checks named in trace_ids under the lambda service group.
3. Manually, per cause: (1) create a response-streaming function that writes three chunks then throws and call InvokeWithResponseStream — the three chunks must arrive before the terminating event; (2) call GetDurableExecutionState on a finished durable execution with CheckpointToken='Zm9yZ2VkLXRva2Vu' — expect 400 InvalidParameterValueException "Invalid checkpoint token"; (3) start a durable execution on a function whose DurableConfig names a CMK, disable the key, then call GetDurableExecution(IncludeExecutionData=true) and StopDurableExecution — expect KMSDisabledException with AWS's wording; (4) Invoke twice with the same DurableExecutionName and different payloads — expect "Execution already started: <arn>"; (5)/(6) deploy nodejs22.x packages {index.mjs: import of a missing package} and {index.js: require of a missing layer module} and invoke — compare errorType/errorMessage/trace[0]; (7) invoke a warm nodejs22.x function that schedules an unawaited setTimeout, then invoke again — the second invocation must observe the timer firing.
