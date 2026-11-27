Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__lambda__logs.e2e-lambda-logger-json-fragment-fields-are-summed-by-insights iam__lambda__logs.lambda-report-line-exposes-discovered-fields-in-insights iam__lambda__logs.function-url-config-duplicate-create-rejected-auth-switch-advances-last-modified
Issue-Key: iam__lambda__logs.e2e-lambda-logger-json-fragment-fields-are-summed-by-insights|_query_until#10|AssertionError|raise AssertionError(f"{what}: {query!r} never returned the expected rows in {budget_s}s; last={last!r}")
Issue-Key: iam__lambda__logs.lambda-report-line-exposes-discovered-fields-in-insights|_query_until#8|AssertionError|raise AssertionError(f"{what}: {query!r} never returned the expected rows in {budget_s}s; last={last!r}")
Issue-Key: lambda|CreateFunctionUrlConfig|ResourceConflictException
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# lambda + logs: Insights cannot query a function's structured logs (JSON fragment fields and REPORT-line fields return no rows), and CreateFunctionUrlConfig's duplicate refusal message is re-worded

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Cause 1 of 3 — fields embedded in a JSON log line are not discovered, so an Insights aggregation over them returns nothing.
AWS: when a Lambda writes a JSON object as its log message, CloudWatch Logs Insights discovers each top-level key as a queryable field, so `filter ispresent(orderId) | stats sum(amount) as total, count(*) as n` returns one row with the summed amount and the event count. Recorded behavior only (Insights field discovery is documented, not modelled).
Emulator: the query returns no rows for the whole observation window — `last=[]`.
Traces: iam__lambda__logs.e2e-lambda-logger-json-fragment-fields-are-summed-by-insights.

Cause 2 of 3 — the invocation `REPORT` line's fields are not discovered.
AWS: the platform-emitted `REPORT RequestId: ... Duration: ... Billed Duration: ... Memory Size: ... Max Memory Used: ...` line is parsed into the discovered fields `@type`, `@requestId`, `@duration`, `@billedDuration`, `@memorySize`, `@maxMemoryUsed`, so `filter @type = "REPORT" | fields @requestId, @duration, @billedDuration, @memorySize, @maxMemoryUsed` returns one row per invocation.
Emulator: the query returns no rows — `last=[]`.
Traces: iam__lambda__logs.lambda-report-line-exposes-discovered-fields-in-insights.

Cause 3 of 3 — the duplicate-FunctionUrlConfig refusal message is re-worded.
AWS: a second `CreateFunctionUrlConfig` for the same function/qualifier is refused with `ResourceConflictException` and the message `Failed to create function url config for [functionArn = <function ARN>]. Error message:  FunctionUrlConfig exists for this Lambda function` — fixed text is `Failed to create function url config for [functionArn = `, `]. Error message:  FunctionUrlConfig exists for this Lambda function`, with the function ARN as the only variable part (note the lowercase `url` and the two spaces after `Error message:`). Smithy (lambda): `CreateFunctionUrlConfig` models `ResourceConflictException`.
Emulator: the code is right, the message is `Failed to create function URL config for <function ARN>. FunctionUrlConfig exists for this Lambda function.` — different fixed text (uppercase `URL`, no `[functionArn = ...]` bracket form, no `Error message:` segment, trailing period).
Traces: iam__lambda__logs.function-url-config-duplicate-create-rejected-auth-switch-advances-last-modified.

Why: causes 1 and 2 remove the only general way to read structured Lambda telemetry — every dashboard, alarm query and cost/duration report built on JSON fields or on the REPORT line returns empty, which reads as 'no invocations' rather than 'field discovery is missing'. Cause 3 breaks clients that match on the exact refusal message (a common pattern for idempotent create-or-update logic).

Fix: the CloudWatch Logs Insights field-discovery stage (JSON message keys and the platform REPORT-line parser) for causes 1-2, and the CreateFunctionUrlConfig handler's conflict error message for cause 3.

## Evidence
Cause 1: "embedded JSON fields: 'filter ispresent(orderId) | stats sum(amount) as total, count(*) as n' never returned the expected rows in 42.36s; last=[]".
Cause 2: "REPORT fields: 'filter @type = \"REPORT\" | fields @requestId, @duration, @billedDuration, @memorySize, @maxMemoryUsed' never returned the expected rows in 42.47s; last=[]".
Cause 3: expected ('ResourceConflictException', 'Failed to create function url config for [functionArn = arn:aws:lambda:us-west-2:000000000000:function:<name>]. Error message:  FunctionUrlConfig exists for this Lambda function'); got ('ResourceConflictException', 'Failed to create function URL config for arn:aws:lambda:us-west-2:000000000000:function:<name>. FunctionUrlConfig exists for this Lambda function.').

## Steps to Reproduce
1. Invoke a Lambda that logs a JSON object with orderId/amount keys; run StartQuery 'filter ispresent(orderId) | stats sum(amount) as total, count(*) as n' over its log group — no rows.
2. Run StartQuery 'filter @type = "REPORT" | fields @requestId, @duration, @billedDuration, @memorySize, @maxMemoryUsed' over the same group — no rows.
3. Call CreateFunctionUrlConfig twice for one function and compare the second refusal's Message with AWS's wording.
