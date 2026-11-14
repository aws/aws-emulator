Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.durable-execution-lifecycle lambda.durable-execution-history-ordering lambda.stop-durable-execution-terminal-state-is-a-no-op lambda.environment-identity-warm-reuse-and-config-update-forces-cold lambda.log-lambda-report-log-level-with-text-format-rejected lambda.stream-provisioned-environment-reports-provisioned-initialization-type lambda.unzipped-package-over-250mb-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.durable-execution-lifecycle|_poll_until#10|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: lambda.durable-execution-history-ordering|_wait_for_execution_status#4|Timeout|time.sleep(3)
Issue-Key: lambda.stop-durable-execution-terminal-state-is-a-no-op|_poll_until#6|AssertionError|raise AssertionError(f"condition not met within {timeout_s}s; last={last!r}")
Issue-Key: lambda.environment-identity-warm-reuse-and-config-update-forces-cold|_assert_env_invariants#10|AssertionError|assert len({b["log_stream"] for b in served}) == 1, (
Issue-Key: lambda.log-lambda-report-log-level-with-text-format-rejected|_stream_of#14|AssertionError|assert len(holders) == 1, f"{line!r} must be in exactly one stream, found in {holders!r}"
Issue-Key: lambda.stream-provisioned-environment-reports-provisioned-initialization-type|run#36|AssertionError|assert len(burst_envs) == BURST, (
Issue-Key: lambda.unzipped-package-over-250mb-rejected|run#18|AssertionError|assert "FunctionError" not in resp, resp.get("FunctionError")
Issue-Commit: 2ac91d382b67c33b3b1ad8ceed805222f48956bf

# lambda: 5 net-new causes — durable executions never leave RUNNING, one execution environment owns many log streams, Text-format logs are never delivered, concurrent invokes are queued onto fewer environments, and a large package's invoke fails Unhandled

> _Note: this issue was identified against the code base at commit `2ac91d382b67c33b3b1ad8ceed805222f48956bf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDurableExecution

## Description
Five net-new root causes in the lambda crate, separate from the message/serializer defects filed alongside this issue. Each has its own failing check ids.

Cause 1 of 5: a started durable execution never reaches a terminal state.
AWS: Invoke with X-Amz-Durable-Execution-Name starts a durable execution (Smithy: Invoke carries DurableExecutionName and returns DurableExecutionArn). The execution runs the handler and then moves out of RUNNING to a terminal status (SUCCEEDED/FAILED/STOPPED); GetDurableExecution reports that status, its history is append-only and ordered, and StopDurableExecution on an already-terminal execution is a no-op that leaves the terminal status standing. A handler that sleeps 8 s therefore settles well inside a 45 s poll.
Emulator: GetDurableExecution reports `Status: 'RUNNING'` for the whole poll window — 45 s for an execution whose handler waits 8 s — so the execution never settles, no ordered history can be read, and the terminal-state no-op for StopDurableExecution cannot be exercised.
Why: durable execution is a workflow primitive; a client polling for completion waits forever, every history-based orchestration reads an empty/unordered log, and a stop against a finished execution cannot be distinguished from a stop against a live one.
Fix: the lambda durable-execution runner and its state store — the execution must transition out of RUNNING when the handler returns/fails, append its history events in order, and keep terminal executions terminal.
Traces: lambda.durable-execution-lifecycle, lambda.durable-execution-history-ordering, lambda.stop-durable-execution-terminal-state-is-a-no-op.
The open issue 20261027T203721Z-lambda-a-durable-execution-is-accepted-but-never-runs.md covers other durable-execution checks; it does not carry these three replay keys.

Cause 2 of 5: one execution environment emits more than one log stream.
AWS: an execution environment owns exactly one CloudWatch log stream for its whole life — every invocation served by the same environment writes into the same stream, and a new stream appears only when a new environment is created (e.g. after a configuration update forces a cold start). Documented in the Lambda developer guide ('Lambda function logs'); recorded behavior as far as the model goes.
Emulator: a sequence of warm invocations served by one environment reports six distinct log stream names (`2026/.../[$LATEST]<id>` with six different ids) for a single environment id, so stream identity does not track environment identity.
Why: environment identity read from the log stream is how clients attribute cold starts, reuse and per-environment state; with a new stream per invocation every invocation looks like a cold start and the warm-reuse/forced-cold contrast is unobservable.
Fix: the lambda execution-environment log-stream allocation — bind one stream to one environment for its lifetime.
Traces: lambda.environment-identity-warm-reuse-and-config-update-forces-cold.

Cause 3 of 5: with LoggingConfig LogFormat Text, the platform lines are never delivered to any log stream.
AWS: regardless of LogFormat, each invocation's `START RequestId: <id> Version: <qualifier>`, `END RequestId: <id>` and `REPORT RequestId: <id> ...` lines are delivered to exactly one log stream of the function's log group, and ApplicationLogLevel filters only application records, never these platform lines. Documented in the Lambda developer guide ('Configuring advanced logging controls'); recorded behavior as far as the model goes.
Emulator: the START line for the invocation's request id is found in NO stream of the group (`'START RequestId: <id> Version: $LATEST\n' must be in exactly one stream, found in []`), so under the Text format the platform lines are not delivered at all.
Why: START/END/REPORT are the per-invocation markers every log consumer keys on; with them absent an invocation leaves no trace in CloudWatch Logs and a log-level setting appears to suppress the platform record too.
Fix: the lambda log delivery path for LogFormat Text — emit the platform START/END/REPORT records into the invocation's stream independently of ApplicationLogLevel.
Traces: lambda.log-lambda-report-log-level-with-text-format-rejected.

Cause 4 of 5: simultaneous invocations are queued onto fewer execution environments than there are in-flight requests.
AWS: N truly simultaneous invocations of a function with sufficient concurrency are served by N distinct execution environments (each reports a distinct environment identity), and a provisioned environment reports initialization type `provisioned`.
Emulator: 6 simultaneous invocations were served by 5 execution environments, i.e. one request waited for an environment instead of getting its own, so nothing downstream of that point is a statement about concurrency.
Why: a client sizing concurrency, or relying on per-environment isolation (separate /tmp, separate module state), gets silent serialization: two concurrent requests can share one environment's state.
Fix: the lambda invoke admission/environment pool — allocate a fresh environment per concurrent request up to the concurrency limit instead of queueing onto an in-use one.
Traces: lambda.stream-provisioned-environment-reports-provisioned-initialization-type.

Cause 5 of 5: a deployment package whose unzipped size is comfortably under the 250 MB quota deploys but cannot be invoked.
AWS: the control plane refuses a package whose unzipped size exceeds the 250 MB quota, and accepts one below it; a function created from a package with a large padded entry becomes Active and invokes successfully, with the padded entry present at its full size in the task root. Documented in the Lambda quotas page; recorded behavior as far as the model goes.
Emulator: the under-the-limit control function is created and reaches Active, but Invoke answers with FunctionError `Unhandled` instead of the handler's result, so the package's contents are not usable at runtime.
Why: the quota check is only meaningful next to a working under-limit case; here every sizeable deployment package produces a function that fails at invoke time, and the handler's own output (including the deployed entry's size) is never returned.
Fix: the lambda package extraction/materialisation used by the execution environment (task-root layout for large or padded zip entries) and the invoke path that surfaces the resulting handler failure.
Traces: lambda.unzipped-package-over-250mb-rejected.

## Evidence
Cause 1 — lambda.durable-execution-lifecycle: AssertionError: condition not met within 45.0s; last={... 'InputPayload': '{"wait_seconds": 8}', 'Status': 'RUNNING', ...}. lambda.stop-durable-execution-terminal-state-is-a-no-op: condition not met within 45.0s; last={... 'Status': 'RUNNING' ...}. lambda.durable-execution-history-ordering: Check timed out after 60.0s. Expected per AWS: a terminal Status (SUCCEEDED/FAILED) and an ordered append-only history.
Cause 2 — lambda.environment-identity-warm-reuse-and-config-update-forces-cold: AssertionError: before the update: one execution environment must own exactly one log stream, got ['2026/11/14/[$LATEST]4218e012...', ... six distinct stream names ...] in <one environment id>.
Cause 3 — lambda.log-lambda-report-log-level-with-text-format-rejected: AssertionError: 'START RequestId: <id> Version: $LATEST\n' must be in exactly one stream, found in [].
Cause 4 — lambda.stream-provisioned-environment-reports-provisioned-initialization-type: AssertionError: 6 simultaneous invocations were served by 5 execution environment(s), so they were queued.
Cause 5 — lambda.unzipped-package-over-250mb-rejected: AssertionError: Unhandled (Invoke of the under-the-limit control function returned FunctionError 'Unhandled'); expected no FunctionError and a payload carrying the handler's marker and the padded entry's full size.

## Steps to Reproduce
Cause 1: Invoke with X-Amz-Durable-Execution-Name on a handler that sleeps 8 s, then poll GetDurableExecution for up to 45 s; AWS reports a terminal status, the emulator stays RUNNING. Call StopDurableExecution on a terminal execution and re-read the status.
Cause 2: invoke a function several times in a row without changing its configuration and collect the log_stream each invocation reports; AWS gives one stream per environment.
Cause 3: PutFunctionConfiguration with LoggingConfig LogFormat=Text and an ApplicationLogLevel, invoke once, then search the function's log group for the invocation's START line.
Cause 4: fire 6 simultaneous Invokes against a function with sufficient concurrency and count distinct execution-environment identities.
Cause 5: CreateFunction from a zip with a large padded entry whose unzipped size is well under 250 MB, wait for Active, and Invoke.
Then replay the check ids in trace_ids.
