Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.insights-count-distinct-counts-unique-values-per-group logs.insights-jsonparse-argument-count-rejected logs.insights-jsonparse-reaches-nested-keys-list-items-and-backticked-names logs.insights-unnest-emits-one-record-per-list-element logs.insights-filter-index-function-operand-rejected logs.insights-filter-index-in-list-returns-events-with-any-listed-value logs.insights-filter-index-on-indexed-field-returns-only-the-matching-event logs.insights-unnest-without-into-target-rejected logs.insights-anomaly-without-preceding-pattern-rejected logs.insights-anomaly-window-starting-before-group-creation-rejected logs.insights-limit-any-returns-n-distinct-matching-events
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: cloudwatch-logs|StartQuery|MalformedQueryException
Issue-Key: logs.insights-anomaly-window-starting-before-group-creation-rejected|run#15|AssertionError|assert got == ("MalformedQueryException", _BASELINE_REFUSAL), (
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs Insights: 6 CWLI commands/functions are unknown to the query parser — countDistinct, jsonParse, filterIndex, unnest, anomaly and 'limit any N' are refused with MalformedQueryException

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Six documented CloudWatch Logs Insights (CWLI) constructs are not in the query grammar, so `StartQuery` refuses valid queries with `MalformedQueryException` instead of running them. In each case AWS accepts the query and returns a `queryId`; the emulator's refusal message is its own parser's position report, e.g. `Unrecognized function name: <name> ([<start>,<end>])` or `unexpected symbol found <token> at line 1 and position <n>` — the fixed text is that sentence shape and the variable parts are the token/function name and offsets taken from the submitted query string. Smithy (logs): `StartQuery` models `MalformedQueryException` and takes the query in `queryString`; the CWLI grammar itself is documented behavior only.

Cause 1 of 6 — `countDistinct` aggregate is unrecognised. AWS: `stats countDistinct(<field>) [by <key>]` counts unique values per group. Emulator: `MalformedQueryException: Unrecognized function name: countDistinct`.
Traces: logs.insights-count-distinct-counts-unique-values-per-group.

Cause 2 of 6 — `jsonParse` function is unrecognised. AWS: `jsonParse(<field>)` parses a JSON string into an addressable structure, reaching nested keys, list items and backticked names. Emulator: `MalformedQueryException: Unrecognized function name: jsonParse`.
Traces: logs.insights-jsonparse-argument-count-rejected, logs.insights-jsonparse-reaches-nested-keys-list-items-and-backticked-names, logs.insights-unnest-emits-one-record-per-list-element.

Cause 3 of 6 — `filterIndex` command is unrecognised. AWS: `filterIndex <field> = <value>` and `filterIndex <field> in [<values>]` filter using a log group's field indexes. Emulator: `MalformedQueryException: unexpected symbol found filterIndex at line 1 and position 0`.
Traces: logs.insights-filter-index-function-operand-rejected, logs.insights-filter-index-in-list-returns-events-with-any-listed-value, logs.insights-filter-index-on-indexed-field-returns-only-the-matching-event.

Cause 4 of 6 — `unnest` command is unrecognised. AWS: `unnest <listField> into <target>` emits one record per list element. Emulator: `MalformedQueryException: unexpected symbol found unnest at line 1 and position <n>`.
Traces: logs.insights-unnest-without-into-target-rejected.

Cause 5 of 6 — `anomaly` command is unrecognised. AWS: `pattern ... | anomaly` detects anomalous patterns over the query window. Emulator: `MalformedQueryException: unexpected symbol found anomaly at line 1 and position <n>`.
Traces: logs.insights-anomaly-without-preceding-pattern-rejected, logs.insights-anomaly-window-starting-before-group-creation-rejected.

Cause 6 of 6 — `limit any <n>` is unrecognised. AWS: `... | limit any <n>` returns n matching events without ordering. Emulator: `MalformedQueryException: unexpected symbol found any 3 at line 1 and position <n>`.
Traces: logs.insights-limit-any-returns-n-distinct-matching-events.

Why: each of these is the only way to express its operation in CWLI. A client's saved query, dashboard widget or alarm built on any of them fails at submission with MalformedQueryException — not with a result it can interpret — and the negative tests for these constructs (wrong arity, missing `into`, `anomaly` without a preceding `pattern`) report the parser's generic refusal rather than the specific one AWS gives, so a client cannot distinguish 'my query is wrong' from 'this feature does not exist'.

Fix: the CloudWatch Logs Insights query parser/grammar and its function registry — add the `filterIndex`, `unnest` and `anomaly` commands, the `limit any <n>` form, and the `countDistinct` and `jsonParse` functions, with their own arity/sequencing refusals.

## Evidence
countDistinct: MalformedQueryException — "Unrecognized function name: countDistinct ([33,51])".
jsonParse: MalformedQueryException — "Unrecognized function name: jsonParse ([10,23])" / "([12,25])" / "([7,25])".
filterIndex: MalformedQueryException — "unexpected symbol found filterIndex at line 1 and position 0".
unnest: MalformedQueryException — "unexpected symbol found unnest at line 1 and position 36".
anomaly: MalformedQueryException — "unexpected symbol found anomaly at line 1 and position 49" (and position 19).
limit any: MalformedQueryException — "unexpected symbol found any 3 at line 1 and position 58".
In every case AWS accepts the query and returns a queryId.

## Steps to Reproduce
1. Create a log group, write a few events, then call logs:StartQuery with each of: 'stats countDistinct(f) by g', 'fields jsonParse(@message) as o', 'filterIndex tenant = "t1" | fields tenant', 'unnest items into item', 'pattern @message | anomaly', '... | limit any 3'.
2. Each call is refused with MalformedQueryException instead of returning a queryId.
