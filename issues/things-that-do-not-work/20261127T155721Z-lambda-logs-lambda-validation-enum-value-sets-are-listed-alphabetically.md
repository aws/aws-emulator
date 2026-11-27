Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__logs.invoke-with-unknown-log-type-rejected-and-runs-no-handler lambda__logs.system-log-level-outside-debug-info-warn-rejected lambda__logs.python-stdout-stderr-child-and-logging-output-reach-the-invocation-stream
Issue-Key: lambda__logs.invoke-with-unknown-log-type-rejected-and-runs-no-handler|run#26|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: lambda__logs.system-log-level-outside-debug-info-warn-rejected|run#15|AssertionError|assert got == expected, f"SystemLogLevel ERROR answered {got!r}, expected {expected!r}"
Issue-Key: lambda__logs.python-stdout-stderr-child-and-logging-output-reach-the-invocation-stream|run#10|AssertionError|assert resp["StatusCode"] == 200 and "FunctionError" not in resp, (
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# lambda: validation enum value sets are listed alphabetically instead of in the model's member order (logType, loggingConfig.systemLogLevel), and a Python handler's stdout/stderr invocation fails Unhandled

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 3 — the `logType` enum value set in the validation message is alphabetised.
AWS: `Invoke` with an unmodelled `LogType` is refused with `ValidationException` and the message `1 validation error detected: Value '<the value sent>' at 'logType' failed to satisfy constraint: Member must satisfy enum value set: [Tail, None]`. The bracketed list is the enum's member declaration order from the model, not a sorted list; fixed text is the sentence template, and the quoted value is the one from the request. Smithy (lambda): `LogType` enum members are declared `Tail`, `None`.
Emulator: the code and template are right but the list is `[None, Tail]` — sorted alphabetically.
Traces: lambda__logs.invoke-with-unknown-log-type-rejected-and-runs-no-handler.

Cause 2 of 3 — the same defect on `loggingConfig.systemLogLevel`.
AWS: `1 validation error detected: Value '<value>' at 'loggingConfig.systemLogLevel' failed to satisfy constraint: Member must satisfy enum value set: [WARN, INFO, DEBUG]` — again the model's member order. Smithy (lambda): `SystemLogLevel` members are declared `DEBUG`, `INFO`, `WARN`, and AWS renders this set as `[WARN, INFO, DEBUG]`.
Emulator: `[DEBUG, INFO, WARN]`.
Traces: lambda__logs.system-log-level-outside-debug-info-warn-rejected.

Cause 3 of 3 — an invocation whose handler writes to stdout/stderr (directly, from a child process and through the `logging` module) fails.
AWS: all three output channels are captured into the invocation's log stream and the invocation itself succeeds (`StatusCode` 200, no `FunctionError`).
Emulator: the first invoke answers HTTP 200 with `FunctionError: 'Unhandled'` — the handler did not complete.
Traces: lambda__logs.python-stdout-stderr-child-and-logging-output-reach-the-invocation-stream.

Why: causes 1-2 break clients that assert on the exact refusal message (a common way to surface an invalid enum back to a user) and misrepresent the service's own constraint text. Cause 3 is a functional failure: a function that writes to stdout/stderr or spawns a child process — the overwhelming majority of real handlers — fails the invocation instead of running, so neither its result nor its logs are produced.

Fix: the shared validation-message renderer for enum constraints (preserve the model's member order rather than sorting) and the Lambda invocation runtime's handling of handler stdout/stderr and child processes.

## Evidence
Cause 1: got ('ValidationException', "1 validation error detected: Value 'Full' at 'logType' failed to satisfy constraint: Member must satisfy enum value set: [None, Tail]"); expected the same sentence with "[Tail, None]".
Cause 2: got ('ValidationException', "1 validation error detected: Value 'ERROR' at 'loggingConfig.systemLogLevel' failed to satisfy constraint: Member must satisfy enum value set: [DEBUG, INFO, WARN]"); expected "... [WARN, INFO, DEBUG]".
Cause 3: "invoke 0 answered 200 with error 'Unhandled'".

## Steps to Reproduce
1. Call lambda:Invoke with LogType='Full' and compare the Message's bracketed enum list with [Tail, None].
2. Call UpdateFunctionConfiguration with LoggingConfig.SystemLogLevel='ERROR' and compare the bracketed list with [WARN, INFO, DEBUG].
3. Deploy a Python function whose handler prints to stdout, writes to stderr, spawns a child process that prints, and logs via the logging module; invoke it — FunctionError 'Unhandled'.
