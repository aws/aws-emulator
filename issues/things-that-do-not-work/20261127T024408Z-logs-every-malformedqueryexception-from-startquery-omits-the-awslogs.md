Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected logs.logs-insights-semantics-semantic-error-rejected-at-start logs.logs-insights-semantics-syntax-error-rejected-at-start logs.logsquery-parse-with-an-unclosed-regex-group-rejected logs.logsquery-redefining-a-parsed-field-rejected logs.malformed-query-rejected logs.start-query-without-log-group-selector-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected|run#13|AssertionError|assert msg == expected, (
Issue-Key: logs.logs-insights-semantics-semantic-error-rejected-at-start|run#18|AssertionError|assert msg == expected, f"unknown function: expected message {expected!r}, got {msg!r}"
Issue-Key: logs.logs-insights-semantics-syntax-error-rejected-at-start|run#18|AssertionError|assert msg1 == expected1, f"unknown command: expected message {expected1!r}, got {msg1!r}"
Issue-Key: logs.logsquery-parse-with-an-unclosed-regex-group-rejected|run#17|AssertionError|assert code_msg == expected, (
Issue-Key: logs.logsquery-redefining-a-parsed-field-rejected|run#18|AssertionError|assert code_msg == expected, f"expected message {expected!r}, got {code_msg!r}"
Issue-Key: logs.malformed-query-rejected|run#15|AssertionError|assert code_msg == expected, f"expected message {expected!r}, got {code_msg!r}"
Issue-Key: logs.start-query-without-log-group-selector-rejected|run#16|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 34ec76825aeb60426f74f39ee1ee1124c6afb7e4

# logs: every MalformedQueryException from StartQuery omits the AWSLogs refusal suffix CloudWatch Logs appends to the message (1 cause, 7 checks)

> _Note: this issue was identified against the code base at commit `34ec76825aeb60426f74f39ee1ee1124c6afb7e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
One root cause behind seven failing `logs` checks.

Cause 1 of 1: CloudWatch Logs StartQuery refusals of an invalid query carry the bare compiler text, without the per-call suffix the service appends to the `MalformedQueryException` message.

AWS: Smithy — `StartQuery` models `MalformedQueryException` (errorKind client, httpError 400, members `queryCompileError` + `message`); the model does not state the message text, so the message itself is recorded behavior only. In every recording the `Error.Message` of a StartQuery refusal is the compiler's own sentence followed, in the same string, by the fixed suffix `" (Service: AWSLogs; Status Code: 400; Error Code: <code>; Request ID: <request id>; Proxy: null)"`. Fixed text in that suffix: `(Service: AWSLogs; Status Code: `, `; Error Code: `, `; Request ID: `, `; Proxy: null)`. Variable parts, taken from the refusal and the call: the status code (400 for MalformedQueryException), the error code (`MalformedQueryException`), and the per-call request id (a 36-character uuid). The leading sentence itself is also variable and is the compiler diagnostic for the specific query — e.g. `Unrecognized function name: <name> ([<start>,<end>])`, `unexpected symbol found <token> at line <n> and position <p>`, `No aggregation function expression allowed: <expr> ([<start>,<end>])`, `Ephemeral field is already defined: <field> ([<start>,<end>])`, `Compile Error - Invalid regular expression in query: '<regex>' — <reason>`, `Invalid syntax while using query definition snippets: <diagnostic>`. Status 400 and code `MalformedQueryException` are correct in the emulator today; only the message diverges.

Emulator: the message stops at the compiler sentence and the suffix is absent entirely. Replayed examples (expected → actual, both the whole `Error.Message`): `'Unrecognized function name: bogusfn ([18,28]) (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <uuid>; Proxy: null)'` → `'Unrecognized function name: bogusfn ([18,28])'`; `'unexpected symbol found frobnicate at line 1 and position 58 (Service: AWSLogs; ...)'` → `'unexpected symbol found frobnicate at line 1 and position 58'`; `'No aggregation function expression allowed: filtercount(*)>1 ([164,182]) (Service: AWSLogs; ...)'` → `'No aggregation function expression allowed: filtercount(*)>1 ([164,182])'`; `'Invalid syntax while using query definition snippets: unexpected symbol found fields at line 1 and position 0 (Service: AWSLogs; ...)'` → the same text without the suffix. The compiler sentences themselves — including their spans and positions, which are offsets into the service's rewritten query text and are read back out of the emulator's own message by the checks — match; the suffix is the whole divergence.

Why: a client that keys off the refusal message gets a string that does not parse as a CloudWatch Logs refusal. Callers and SDK/CLI log scrapers that read the `Error Code:`/`Request ID:` fields out of the Insights refusal text (the request id is NOT otherwise exposed on this error, which is why AWS embeds it) find nothing, so a support-facing retry/report path that echoes the service/request id for a malformed query silently loses it, and any equality or regex assertion on a documented Insights refusal message fails against the emulator.

Fix: the serialization of `MalformedQueryException` for the CloudWatch Logs query operations — the one place the compiler diagnostic is turned into the error `message` (it must append the `(Service: AWSLogs; Status Code: <status>; Error Code: <code>; Request ID: <this call's request id>; Proxy: null)` suffix, with the request id of the refusing call), not each individual query-compile site.

Traces: logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected, logs.logs-insights-semantics-semantic-error-rejected-at-start, logs.logs-insights-semantics-syntax-error-rejected-at-start, logs.logsquery-parse-with-an-unclosed-regex-group-rejected, logs.logsquery-redefining-a-parsed-field-rejected, logs.malformed-query-rejected, logs.start-query-without-log-group-selector-rejected. Related but not covering: open issue 20261029T182816Z-logs-startquery-and-eight-other-logs-operations-accept.md is about logs operations ACCEPTING or misclassifying calls AWS refuses; here the verdict and the code are already right and only the message text is wrong.

## Evidence
All seven checks assert on the complete `Error.Message` of a StartQuery refusal and differ only by the missing suffix.

1) logs.logs-insights-semantics-semantic-error-rejected-at-start
   expected: 'Unrecognized function name: bogusfn ([18,28]) (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)'
   got:      'Unrecognized function name: bogusfn ([18,28])'
   (the check's '<no request id>' placeholder is what it substitutes when it cannot find a 36-char request id in the emulator's message — i.e. the suffix was absent)

2) logs.logs-insights-semantics-syntax-error-rejected-at-start
   expected: 'unexpected symbol found frobnicate at line 1 and position 58 (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)'
   got:      'unexpected symbol found frobnicate at line 1 and position 58'

3) logs.deep-logs-1-logsql-sql-having-on-aggregate-expression-rejected (HAVING COUNT(*) > 1)
   expected: 'No aggregation function expression allowed: filtercount(*)>1 ([164,182]) (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: ...; Proxy: null)'
   got:      'No aggregation function expression allowed: filtercount(*)>1 ([164,182])'

4) logs.logsquery-parse-with-an-unclosed-regex-group-rejected
   expected: "Compile Error - Invalid regular expression in query: 'lat=(?<capture000>(?s)\\d+' — Unclosed group near index 25\nlat=(?<capture000>(?s)\\d+ (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)"
   got:      the same text with the suffix absent

5) logs.logsquery-redefining-a-parsed-field-rejected
   expected: 'Ephemeral field is already defined: lat ([118,137]) (Service: AWSLogs; ...; Proxy: null)'
   got:      'Ephemeral field is already defined: lat ([118,137])'

6) logs.malformed-query-rejected
   expected: 'unexpected symbol found | at line 1 and position 12 (Service: AWSLogs; ...; Proxy: null)'
   got:      'unexpected symbol found | at line 1 and position 12'

7) logs.start-query-without-log-group-selector-rejected
   expected: 'Invalid syntax while using query definition snippets: unexpected symbol found fields at line 1 and position 0 (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <no request id>; Proxy: null)'
   got:      'Invalid syntax while using query definition snippets: unexpected symbol found fields at line 1 and position 0'

Status (400) and error code (MalformedQueryException) were correct in every case, and no rejected call left a query behind.

## Steps to Reproduce
1. logs.CreateLogGroup a group.
2. Call logs.StartQuery against it with a query the Insights/LogsQL compiler refuses, e.g. queryString="this is not | valid @@ Insights >>> syntax {{{" (or 'fields @message | stats bogusfn(@message)', or a SQL query with HAVING COUNT(*) > 1).
3. Read the resulting ClientError: code is MalformedQueryException with HTTP 400 (correct), and Error.Message is only the compiler sentence.
4. AWS's message for the same refusal ends with ' (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <the call's 36-char request id>; Proxy: null)'. The emulator's message has no such suffix, so an exact-message comparison fails.
