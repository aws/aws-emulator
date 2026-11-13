Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.logs-insights-get-query-results-query-id-is-a-durable-snapshot-handle logs.logs-insights-get-query-results-paginates-cwli-results-into-disjoint-pages logs.logs-insights-start-query-limit-parameter-bounds-the-rows
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.logs-insights-get-query-results-query-id-is-a-durable-snapshot-handle|expect_error#3|AssertionError|assert got == want, f"expected {want!r}, got {got!r}"
Issue-Key: logs.logs-insights-get-query-results-paginates-cwli-results-into-disjoint-pages|expect_error#25|AssertionError|assert got == want, f"expected {want!r}, got {got!r}"
Issue-Key: logs.logs-insights-start-query-limit-parameter-bounds-the-rows|run#16|AssertionError|assert _ns(wider) == ["0", "1", "2", "3", "4"], (
Issue-Commit: 4d52475541f700a1053a505adbc9b4b259e3075a

# CloudWatch Logs Insights: GetQueryResults' unknown-queryId and foreign-nextToken refusals diverge, and StartQuery's limit parameter does not override a `| limit` command (3 causes)

> _Note: this issue was identified against the code base at commit `4d52475541f700a1053a505adbc9b4b259e3075a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResults

## Description
Three net-new root causes in the CloudWatch Logs Insights query-results plane. None is covered by the open Insights issues 20261109T001104Z-logs-5-net-new-causes-cloudwatch-logs-insights.md or 20261108T145640Z-logs-9-net-new-causes-cloudwatch-logs-insights.md, which carry other Insights checks (query refusal messages, stats/sort/parse semantics); these three concern GetQueryResults' handle refusals and StartQuery's limit parameter. A fixer must close all three before submitting.

=== Cause 1 of 3: GetQueryResults refuses an unknown queryId with ResourceNotFoundException instead of InvalidParameterException, and with StopQuery's message text. Failing check: logs.logs-insights-get-query-results-query-id-is-a-durable-snapshot-handle ===
AWS: Smithy: GetQueryResults takes queryId (QueryId, required, @length 1..256) and models both InvalidParameterException ('A parameter is specified incorrectly', HTTP 400) and ResourceNotFoundException ('The specified resource does not exist', HTTP 400); StopQuery models the same pair. Recorded behavior: the two operations deliberately pick DIFFERENT members of that pair for the same unissued handle. GetQueryResults is refused HTTP 400 with error code InvalidParameterException and the message template `The provided queryId = <queryId> is invalid ` — fixed text `The provided queryId = ` and ` is invalid ` (a space on each side of `=`, and a TRAILING space after `invalid`), with <queryId> the value copied from the request. StopQuery for the same id is refused HTTP 400 ResourceNotFoundException with the different template `The provided queryId=<queryId> is invalid.` (no spaces around `=`, terminating period). This holds both for a well-formed-but-unissued UUID and for a non-UUID string such as `not-a-query-id`.
Emulator: answers GetQueryResults with StopQuery's refusal — error code ResourceNotFoundException and message `The provided queryId=<queryId> is invalid.`. Replay: expected ('InvalidParameterException', 'The provided queryId = 0319d195-b8cf-44bc-afc3-2ce664d267bd is invalid '), got ('ResourceNotFoundException', 'The provided queryId=0319d195-b8cf-44bc-afc3-2ce664d267bd is invalid.'). (The UUID is the per-run handle minted by the check, not part of the template.)
Why: a client distinguishes 'this handle was never valid' from 'the resource is gone' by the modelled exception class. Because the code is wrong, `except logs.exceptions.InvalidParameterException` never fires and the error surfaces as ResourceNotFoundException (or falls through to a bare ClientError), so retry/claim-recovery logic that keys on the code takes the wrong branch. The message difference additionally breaks anything matching the refusal text.
Fix: the GetQueryResults handler's unknown-query branch. It returns ResourceNotFoundException built from the shared `crate::state::invalid_query_id_message` helper, which is StopQuery's wording; GetQueryResults needs its own InvalidParameterException and its own message template, leaving StopQuery's untouched.
Traces: logs.logs-insights-get-query-results-query-id-is-a-durable-snapshot-handle; services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_query_results.rs:263-270 (the `store.queries.get(...)` else-branch) and its use of `crate::state::invalid_query_id_message`.

=== Cause 2 of 3: GetQueryResults' refusal of a nextToken minted for a different query uses the wrong message. Failing check: logs.logs-insights-get-query-results-paginates-cwli-results-into-disjoint-pages ===
AWS: Smithy: GetQueryResults$nextToken (GetQueryResultsNextToken, @length 0..1024), 'The token for the next set of items to return'; InvalidParameterException, HTTP 400. Recorded behavior: passing a nextToken that belongs to another query's result set is refused HTTP 400, error code InvalidParameterException, message `Invalid token for query (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <request-id>; Proxy: null)`. The fixed text is `Invalid token for query ` followed by this service's SDK error envelope, whose stated service (`AWSLogs`), status (`400`) and error code (`InvalidParameterException`) are exactly the ones the response itself answers with; the only variable part is <request-id>, the 36-character per-call request id (never a requirement in its own right).
Emulator: refuses with the right code but a different sentence and no envelope: `The specified nextToken is invalid.`. Replay: expected ('InvalidParameterException', 'Invalid token for query (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <no request id>; Proxy: null)'), got ('InvalidParameterException', 'The specified nextToken is invalid.').
Why: callers and operators read the envelope to tell a cross-query token mix-up apart from an expired or truncated token; the generic sentence loses the service/status/code/request-id triage data that every other CloudWatch Logs refusal of this service carries, so a paginating client cannot log a correlatable refusal.
Fix: the GetQueryResults handler's cursor-decode branch. It passes the shared `crate::state::INVALID_NEXT_TOKEN_MESSAGE` to its invalid_parameter helper; it needs this operation's own refusal text wrapped in the SDK error envelope helper (`crate::state::with_sdk_error_envelope`) the service already uses for StartQuery's refusals.
Traces: logs.logs-insights-get-query-results-paginates-cwli-results-into-disjoint-pages; services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_query_results.rs:286-296 and `crate::state::INVALID_NEXT_TOKEN_MESSAGE`.

=== Cause 3 of 3: StartQuery's `limit` parameter does not override a `| limit` command in the query string — the emulator applies the narrower of the two. Failing check: logs.logs-insights-start-query-limit-parameter-bounds-the-rows ===
AWS: Smithy: StartQuery$limit (EventsLimitStartQuery, @range 1..100000) — 'The maximum number of log events to return from the query. The maximum limit is 100,000.' Recorded behavior only for the precedence rule: the request parameter is authoritative over a `| limit N` command in the query string, in BOTH directions. A query `fields n | sort n asc | limit 2` started with limit=5 over 12 matching events returns the parameter's 5 rows; the same query written `... | limit 5` started with limit=3 returns the parameter's 3 rows. With limit=3 the statistics report resultCount 3 of recordsMatched 12 — the limit bounds the returned rows, not the match count.
Emulator: takes the smaller of the parameter and the pipeline command. For limit=5 with `| limit 2` it returned 2 rows: 'limit=5 with "| limit 2" returned n ["0", "1"], expected the parameter\'s 5 rows 0..4'. The narrower direction (limit=3 with `| limit 5`) is therefore never exercised.
Why: a caller that widens the StartQuery limit above the limit baked into a saved query string silently receives the smaller page and, because the response status is Complete and no nextToken is returned, reads a truncated result set as the whole result set. Row counts computed from such a page are wrong with no error anywhere.
Fix: the row-limit application performed when a stored query is run — the Insights execution path in the service state (`crate::state::insights`), which must use the stored QueryRecord.limit in place of, not in conjunction with, any `limit` command the compiled pipeline carries. StartQuery itself already records the parameter correctly.
Traces: logs.logs-insights-start-query-limit-parameter-bounds-the-rows; services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/start_query.rs:645 (`record.limit = input.limit`), consumed by `crate::state::insights::prepare`/`run` as called from services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/get_query_results.rs:277,301.

## Evidence
Cause 1 — logs.logs-insights-get-query-results-query-id-is-a-durable-snapshot-handle:
  AssertionError: expected ('InvalidParameterException', 'The provided queryId = 0319d195-b8cf-44bc-afc3-2ce664d267bd is invalid '), got ('ResourceNotFoundException', 'The provided queryId=0319d195-b8cf-44bc-afc3-2ce664d267bd is invalid.')
  Expected: HTTP 400, code InvalidParameterException, message `The provided queryId = <queryId from the request> is invalid ` (spaces around `=`, trailing space).
  Actual: HTTP 400, code ResourceNotFoundException, message `The provided queryId=<queryId> is invalid.` — StopQuery's template, which the check separately asserts for StopQuery and which must stay distinct.

Cause 2 — logs.logs-insights-get-query-results-paginates-cwli-results-into-disjoint-pages:
  AssertionError: expected ('InvalidParameterException', 'Invalid token for query (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <no request id>; Proxy: null)'), got ('InvalidParameterException', 'The specified nextToken is invalid.')
  Expected message: fixed text `Invalid token for query ` + SDK envelope `(Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <36-char per-call request id>; Proxy: null)`.
  Actual message: `The specified nextToken is invalid.` (no envelope; the check reports `<no request id>` precisely because the emulator's text carries none).

Cause 3 — logs.logs-insights-start-query-limit-parameter-bounds-the-rows:
  AssertionError: limit=5 with '| limit 2' returned n ['0', '1'], expected the parameter's 5 rows 0..4
  Expected: StartQuery limit=5 over `fields n | sort n asc | limit 2` with 12 matching events -> 5 rows (n = 0..4). The converse, limit=3 over `... | limit 5` -> 3 rows (n = 0..2).
  Actual: 2 rows (n = 0, 1) — the pipeline command won over the parameter.
  Earlier in the same check limit=3 with no `| limit` correctly returned 3 rows with resultCount 3 / recordsMatched 12, so the parameter is honoured only when the query string carries no limit command.

## Steps to Reproduce
Common setup (all three causes): create a log group, create log stream `s`, PutLogEvents 12 events stamped t0+i seconds with message {"n": i} for i in 0..11, and use the window (t0-60, t0+600).

Cause 1:
1. logs GetQueryResults with queryId set to a UUID the emulator never issued (and again with the non-UUID string `not-a-query-id`).
2. Observe the error code and message. Expected HTTP 400 InvalidParameterException `The provided queryId = <that id> is invalid ` (trailing space); the emulator answers ResourceNotFoundException `The provided queryId=<that id> is invalid.`.
3. For contrast, StopQuery with the same unknown id must keep answering ResourceNotFoundException `The provided queryId=<that id> is invalid.` — the two templates must not be unified.

Cause 2:
1. StartQuery `fields n | sort n asc` over the window; poll GetQueryResults to status Complete (12 records matched).
2. GetQueryResults(queryId=qid, maxItems=5) and keep its nextToken (call it T).
3. StartQuery the same query again to get a second, different queryId (other_qid); poll it to Complete.
4. GetQueryResults(queryId=other_qid, maxItems=5, nextToken=T). Expected HTTP 400 InvalidParameterException `Invalid token for query (Service: AWSLogs; Status Code: 400; Error Code: InvalidParameterException; Request ID: <id>; Proxy: null)`; the emulator answers `The specified nextToken is invalid.`.

Cause 3:
1. StartQuery queryString `fields n | sort n asc | limit 2` with limit=5 over the window; poll GetQueryResults to Complete.
2. Expected 5 rows with n = 0,1,2,3,4; the emulator returns 2 rows (n = 0,1).
3. StartQuery queryString `fields n | sort n asc | limit 5` with limit=3; expected 3 rows (n = 0,1,2).
4. StartQuery with limit=100001 must still be refused InvalidParameterException `1 validation error detected: Value at 'limit' failed to satisfy constraint: Member must have value less than or equal to 100000`.
