Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.insights-concat-joins-fields-per-event-and-groups-by-joined-key logs.insights-string-functions-compute-documented-values logs.insights-datefloor-without-a-period-rejected logs.insights-stats-by-datefloor-buckets-events-by-period logs.insights-earliest-without-field-rejected logs.insights-latest-without-argument-or-outside-stats-rejected logs.insights-selection-and-spread-aggregates-match-hand-computation logs.insights-stats-earliest-by-key-spans-streams-and-skips-events-missing-the-field logs.insights-stats-latest-by-key-follows-event-time-not-arrival-order logs.insights-sorts-first-aggregates-in-natural-order-per-group logs.insights-sorts-first-malformed-use-rejected logs.insights-sortslast-by-group-follows-natural-sort-order logs.insights-from-millis-renders-epoch-field-as-timestamp logs.insights-tomillis-offsets-keep-millisecond-precision logs.insights-greatest-takes-row-maximum-over-numeric-operands logs.insights-isvalidip-flags-only-well-formed-v4-and-v6-addresses logs.insights-ip-and-json-parse-functions-compute-documented-values logs.insights-numeric-and-datetime-functions-compute-documented-values logs.insights-replace-substitutes-every-literal-occurrence logs.insights-unmask-function-returns-the-original-of-a-masked-event
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.insights-concat-joins-fields-per-event-and-groups-by-joined-key|_query#14|AssertionError|assert got["status"] == "Complete", f"{text!r} ended {got['status']!r}"
Issue-Key: logs.insights-string-functions-compute-documented-values|_query_until#5|AssertionError|assert resp["status"] == "Complete", (
Issue-Key: logs.insights-datefloor-without-a-period-rejected|_complete_rows#13|AssertionError|assert res["status"] == "Complete", f"{query!r} ended {res['status']!r}, expected 'Complete'"
Issue-Key: logs.insights-stats-by-datefloor-buckets-events-by-period|_query_rows#18|AssertionError|assert status in ("Scheduled", "Running"), f"query {text!r} ended in status {status!r}"
Issue-Key: logs.insights-earliest-without-field-rejected|_answer#20|AssertionError|assert state["status"] == "Complete", f"{request['queryString']!r}: {state!r}"
Issue-Key: logs.insights-latest-without-argument-or-outside-stats-rejected|_answer#14|AssertionError|assert got["status"] == "Complete", f"{q!r} ended {got['status']!r}"
Issue-Key: logs.insights-selection-and-spread-aggregates-match-hand-computation|_query_until#8|AssertionError|assert resp["status"] == "Complete", (
Issue-Key: logs.insights-stats-earliest-by-key-spans-streams-and-skips-events-missing-the-field|_query_once#9|AssertionError|assert got["status"] == "Complete", f"query {text!r} ended {got['status']!r}"
Issue-Key: logs.insights-stats-latest-by-key-follows-event-time-not-arrival-order|_ask#13|AssertionError|assert resp["status"] == "Complete", f"query ended {resp['status']!r}: {text!r}"
Issue-Key: logs.insights-sorts-first-aggregates-in-natural-order-per-group|_collect#11|AssertionError|assert resp["status"] == "Complete", f"query {qid} ended {resp['status']!r}"
Issue-Key: logs.insights-sorts-first-malformed-use-rejected|_collect#10|AssertionError|assert resp["status"] == "Complete", f"query {qid} ended {resp['status']!r}"
Issue-Key: logs.insights-sortslast-by-group-follows-natural-sort-order|_answer#17|AssertionError|assert reply["status"] == "Complete", f"{text!r} ended as {reply['status']!r}, expected 'Complete'"
Issue-Key: logs.insights-from-millis-renders-epoch-field-as-timestamp|_collect#11|AssertionError|assert resp["status"] == "Complete", f"query {qid} ended {resp['status']!r}"
Issue-Key: logs.insights-tomillis-offsets-keep-millisecond-precision|_select#15|AssertionError|assert out["status"] == "Complete", f"query {text!r} stopped as {out['status']!r}"
Issue-Key: logs.insights-greatest-takes-row-maximum-over-numeric-operands|_collect#10|AssertionError|assert resp["status"] == "Complete", f"query {qid} ended {resp['status']!r}"
Issue-Key: logs.insights-isvalidip-flags-only-well-formed-v4-and-v6-addresses|_query_rows#16|AssertionError|assert status in ("Scheduled", "Running"), f"query {text!r} ended in status {status!r}"
Issue-Key: logs.insights-ip-and-json-parse-functions-compute-documented-values|_query_until#5|AssertionError|assert resp["status"] == "Complete", (
Issue-Key: logs.insights-numeric-and-datetime-functions-compute-documented-values|_query_until#6|AssertionError|assert resp["status"] == "Complete", (
Issue-Key: logs.insights-replace-substitutes-every-literal-occurrence|_rows_by_key#11|AssertionError|assert got["status"] == "Complete", f"{query!r} ended {got['status']!r}, expected 'Complete'"
Issue-Key: logs.insights-unmask-function-returns-the-original-of-a-masked-event|run#20|AssertionError|assert answer["status"] == "Complete", f"{query!r} ended {answer['status']!r}"
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs Insights: 10 documented CWLI functions parse but are not evaluated — concat, datefloor, earliest/latest, sortsFirst/sortsLast, fromMillis/toMillis, greatest, isValidIp*, ceil and the other numeric/datetime functions, replace and unmask all end the query 'Failed'

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Ten groups of documented CWLI functions are accepted by `StartQuery` but cannot be evaluated: `GetQueryResults` reports `status: 'Failed'` (with `recordsScanned` > 0 and `results: []`) instead of `Complete` with rows. AWS runs each of these to completion and returns the computed values. Recorded behavior only for the computed values themselves; Smithy (logs): `GetQueryResults.status` is the `QueryStatus` enum Scheduled|Running|Complete|Failed|Cancelled|Timeout|Unknown, so `Failed` on a valid query is the divergence.

Cause 1 of 10 — `concat(...)`: a query selecting or grouping by `concat(a, '/', b)` ends `Failed`.
Traces: logs.insights-concat-joins-fields-per-event-and-groups-by-joined-key, logs.insights-string-functions-compute-documented-values.

Cause 2 of 10 — `datefloor(<timestamp>, <period>)`: ends `Failed`, both in `fields` and as a `stats ... by` key.
Traces: logs.insights-datefloor-without-a-period-rejected, logs.insights-stats-by-datefloor-buckets-events-by-period.

Cause 3 of 10 — the `earliest()`/`latest()` selection aggregates: `stats earliest(<field>) [by <key>]` and `stats latest(<field>) [by <key>]` end `Failed`; AWS resolves them by event time (not arrival order) and skips events missing the field.
Traces: logs.insights-earliest-without-field-rejected, logs.insights-latest-without-argument-or-outside-stats-rejected, logs.insights-selection-and-spread-aggregates-match-hand-computation, logs.insights-stats-earliest-by-key-spans-streams-and-skips-events-missing-the-field, logs.insights-stats-latest-by-key-follows-event-time-not-arrival-order.

Cause 4 of 10 — `sortsFirst()`/`sortsLast()` aggregates end `Failed`; AWS returns the natural-sort extremes per group.
Traces: logs.insights-sorts-first-aggregates-in-natural-order-per-group, logs.insights-sorts-first-malformed-use-rejected, logs.insights-sortslast-by-group-follows-natural-sort-order.

Cause 5 of 10 — `fromMillis()`/`toMillis()` end `Failed`; AWS renders an epoch field as a timestamp and preserves millisecond precision across offsets.
Traces: logs.insights-from-millis-renders-epoch-field-as-timestamp, logs.insights-tomillis-offsets-keep-millisecond-precision.

Cause 6 of 10 — `greatest()` (row maximum over numeric operands) ends `Failed`.
Traces: logs.insights-greatest-takes-row-maximum-over-numeric-operands.

Cause 7 of 10 — `isValidIp()`/`isValidIpV4()`/`isValidIpV6()` end `Failed`.
Traces: logs.insights-isvalidip-flags-only-well-formed-v4-and-v6-addresses, logs.insights-ip-and-json-parse-functions-compute-documented-values.

Cause 8 of 10 — the numeric/datetime function family (`ceil`, and the others exercised alongside it) ends `Failed`, e.g. `fields ceil(ms / 4) as v`.
Traces: logs.insights-numeric-and-datetime-functions-compute-documented-values.

Cause 9 of 10 — `replace(<field>, <find>, <replacement>)` ends `Failed`; AWS substitutes every literal occurrence.
Traces: logs.insights-replace-substitutes-every-literal-occurrence.

Cause 10 of 10 — `unmask(@message)` ends `Failed`; AWS returns the original of a data-protection-masked event to a caller with `logs:Unmask`.
Traces: logs.insights-unmask-function-returns-the-original-of-a-masked-event.

Why: a `Failed` status with no error detail is the worst possible answer — the client cannot tell a broken query from a broken service, cannot retry usefully, and loses the result entirely. Every one of these functions is the documented way to express its computation, so dashboards, saved queries and alarm queries using them return no data at all, and the positive controls inside the corresponding negative tests fail too, which masks the arity refusals those tests exist to check.

Fix: the CloudWatch Logs Insights query evaluator's function/aggregate implementations — implement these function groups (and make an unimplemented function report a MalformedQueryException at StartQuery rather than a silent Failed at evaluation time).

## Evidence
concat: "fields seq, concat(host, '/', verb, ':', port, zone) as joined | sort seq asc" ended 'Failed'; "fields concat(lvl, '-', ip) as v" ended 'Failed'.
datefloor: "'fields @message, datefloor(@timestamp, 1m) as floored' ended 'Failed', expected 'Complete'"; "query 'stats count(*) as n by datefloor(@timestamp, 5m) as bucket' ended in status 'Failed'".
earliest/latest: "'stats earliest(release) as first_release by svc': {... 'results': [], 'statistics': {'recordsMatched': 0.0, 'recordsScanned': 5.0, ...}, 'status': 'Failed'}"; "'stats latest(amt) as newest, count() as n by kind | sort kind asc' ended 'Failed'".
sortsFirst/sortsLast: "'stats sortsFirst(v) as first, sortsLast(v) as last by grp | sort grp asc' ended as 'Failed', expected 'Complete'".
fromMillis/toMillis: "query ... ended 'Failed'"; "query 'fields seq, toMillis(@timestamp) - <epoch-ms> as off | sort off desc' stopped as 'Failed'".
greatest: "query ... ended 'Failed'".
isValidIp*: "fields isValidIp(ip) as v ... ended 'Failed'; a documented query must complete"; "query 'fields id, isValidIp(addr) as any, isValidIpV4(addr) as v4, isValidIpV6(addr) as v6' ended in status 'Failed'".
ceil: "fields ceil(ms / 4) as v ... ended 'Failed'; a documented query must complete".
replace: "\"fields k, p, replace(p, 'a', 'x') as every_a, ...\" ended 'Failed', expected 'Complete'".
unmask: "'fields @message, unmask(@message) as raw | filter @message like /signin ok/' ended 'Failed'".

## Steps to Reproduce
1. Create a log group and write a handful of events with numeric, string, IP and epoch fields.
2. For each function group, call logs:StartQuery with the query shown in the evidence and poll GetQueryResults.
3. Every query returns status 'Failed' with an empty results list instead of 'Complete' with the computed rows.
