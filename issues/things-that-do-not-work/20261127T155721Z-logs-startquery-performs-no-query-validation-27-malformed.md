Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.insights-abs-wrong-arity-rejected logs.insights-coalesce-without-arguments-rejected logs.insights-concat-without-arguments-rejected logs.insights-count-distinct-wrong-arity-rejected logs.insights-frommillis-wrong-arity-rejected logs.insights-greatest-without-arguments-rejected logs.insights-ispresent-wrong-argument-count-rejected logs.insights-isvalidip-with-two-arguments-rejected logs.insights-replace-missing-replacement-argument-rejected logs.insights-stddev-second-argument-rejected logs.insights-strcontains-wrong-arity-rejected logs.insights-tomillis-wrong-argument-count-rejected logs.insights-unmask-wrong-arity-rejected logs.insights-pct-percent-past-100-rejected logs.insights-limit-above-maximum-rejected logs.insights-bin-ten-second-buckets-agree-across-unit-spellings logs.insights-stats-bin-period-without-a-unit-rejected logs.insights-dedup-followed-by-a-command-other-than-limit-rejected logs.insights-diff-without-a-preceding-pattern-rejected logs.insights-eleventh-stats-command-rejected logs.insights-fields-dangling-token-rejected logs.insights-filter-with-a-missing-operand-rejected logs.insights-parse-regex-without-named-capture-rejected logs.insights-sort-ahead-of-last-stats-rejected logs.insights-sort-after-the-last-stats-orders-the-aggregated-rows logs.insights-sorts-last-outside-stats-or-without-field-rejected logs.insights-source-sixth-name-prefix-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.insights-abs-wrong-arity-rejected|_refused#11|AssertionError|raise AssertionError(f"StartQuery accepted a query it should refuse, answering {answer!r}")
Issue-Key: logs.insights-coalesce-without-arguments-rejected|_refusal#11|AssertionError|raise AssertionError(f"StartQuery accepted a query it must refuse and answered {answer!r}")
Issue-Key: logs.insights-concat-without-arguments-rejected|_refused#12|AssertionError|raise AssertionError(f"StartQuery accepted a query it should refuse, answering {answer!r}")
Issue-Key: logs.insights-count-distinct-wrong-arity-rejected|_rejection#13|AssertionError|raise AssertionError(f"StartQuery accepted a malformed aggregation, answering {answer!r}")
Issue-Key: logs.insights-frommillis-wrong-arity-rejected|_refused#12|AssertionError|raise AssertionError(f"StartQuery accepted a query it should refuse, answering {answer!r}")
Issue-Key: logs.insights-greatest-without-arguments-rejected|_refusal#11|AssertionError|raise AssertionError(f"StartQuery accepted a query AWS refuses and answered {answer!r}")
Issue-Key: logs.insights-ispresent-wrong-argument-count-rejected|_refused#11|AssertionError|raise AssertionError(f"the malformed query was accepted: {reply!r}")
Issue-Key: logs.insights-isvalidip-with-two-arguments-rejected|_refusal#14|AssertionError|raise AssertionError(f"expected StartQuery to be refused, but it returned {result!r}")
Issue-Key: logs.insights-replace-missing-replacement-argument-rejected|_refusal#12|AssertionError|raise AssertionError(f"StartQuery accepted a query it should refuse, returning {answer!r}")
Issue-Key: logs.insights-stddev-second-argument-rejected|_start_refused#25|AssertionError|raise AssertionError(f"StartQuery took {request['queryString']!r} as valid: {started!r}")
Issue-Key: logs.insights-strcontains-wrong-arity-rejected|_refused#12|AssertionError|raise AssertionError(f"StartQuery accepted a query it should refuse, answering {answer!r}")
Issue-Key: logs.insights-tomillis-wrong-argument-count-rejected|_code_and_text#12|AssertionError|raise AssertionError(f"StartQuery should have refused this query but returned {ok!r}")
Issue-Key: logs.insights-unmask-wrong-arity-rejected|_refused#21|AssertionError|raise AssertionError(f"StartQuery accepted a query AWS refuses, answering {answer!r}")
Issue-Key: logs.insights-pct-percent-past-100-rejected|_refused#23|AssertionError|raise AssertionError(f"query {request['queryString']!r} was accepted: {out!r}")
Issue-Key: logs.insights-limit-above-maximum-rejected|_refused#12|AssertionError|raise AssertionError(f"expected a refusal, the call returned {answer!r}")
Issue-Key: logs.insights-bin-ten-second-buckets-agree-across-unit-spellings|_refusal#23|AssertionError|raise AssertionError(f"expected StartQuery to be refused, it returned {out!r}")
Issue-Key: logs.insights-stats-bin-period-without-a-unit-rejected|_refusal#17|AssertionError|raise AssertionError(f"StartQuery accepted a query AWS refuses and returned {got!r}")
Issue-Key: logs.insights-dedup-followed-by-a-command-other-than-limit-rejected|_refusal#13|AssertionError|raise AssertionError(f"StartQuery accepted a query AWS refuses: {answer!r}")
Issue-Key: logs.insights-diff-without-a-preceding-pattern-rejected|_refusal#15|AssertionError|raise AssertionError(f"StartQuery accepted a query AWS refuses and returned {got!r}")
Issue-Key: logs.insights-eleventh-stats-command-rejected|_refused#11|AssertionError|raise AssertionError(f"StartQuery was expected to refuse the query but returned {answer!r}")
Issue-Key: logs.insights-fields-dangling-token-rejected|_refusal_of#13|AssertionError|raise AssertionError(f"StartQuery took a malformed query and answered {answer!r}")
Issue-Key: logs.insights-filter-with-a-missing-operand-rejected|_refused#13|AssertionError|raise AssertionError(f"StartQuery started a query AWS refuses: {accepted!r}")
Issue-Key: logs.insights-parse-regex-without-named-capture-rejected|_refusal#15|AssertionError|raise AssertionError(f"StartQuery accepted the query and returned {out!r}")
Issue-Key: logs.insights-sort-ahead-of-last-stats-rejected|_refusal#22|AssertionError|raise AssertionError(f"StartQuery accepted {request['queryString']!r}: {accepted!r}")
Issue-Key: logs.insights-sort-after-the-last-stats-orders-the-aggregated-rows|_refused#17|AssertionError|raise AssertionError(f"StartQuery was expected to refuse the query but returned {answer!r}")
Issue-Key: logs.insights-sorts-last-outside-stats-or-without-field-rejected|_refused#11|AssertionError|raise AssertionError(f"StartQuery accepted a query it must refuse and answered {answer!r}")
Issue-Key: logs.insights-source-sixth-name-prefix-rejected|_start_fails#22|AssertionError|raise AssertionError(f"StartQuery ran {request['queryString']!r}: {began!r}")
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs StartQuery performs no query validation — 27 malformed CWLI queries AWS refuses with MalformedQueryException are accepted and given a queryId

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
`StartQuery` does not validate the submitted `queryString` at all: every malformed query below is accepted with HTTP 200 and a `queryId`, where AWS refuses it synchronously with HTTP 400 `MalformedQueryException`. Smithy (logs): `StartQuery` declares `MalformedQueryException` precisely so a syntactically or semantically invalid query is refused at submission; the CWLI arity and command-ordering rules are documented behavior.

Cause 1 of 2 — function arity and argument-range rules are not checked.
AWS: a wrong argument count, or an argument outside its documented range, is refused with MalformedQueryException naming the offending function and its position in the query string (fixed text is the refusal sentence shape; the function name and offsets come from the request).
Emulator: each call returns `{'queryId': '<id>'}` with HTTP 200.
Affected rules: `abs` (wrong arity), `coalesce` (no arguments), `concat` (no arguments), `countDistinct` (wrong arity), `fromMillis` (wrong arity), `greatest` (no arguments), `ispresent` (wrong argument count), `isValidIp` (two arguments), `replace` (missing replacement argument), `stddev` (a second argument), `strcontains` (wrong arity), `toMillis` (wrong argument count), `unmask` (wrong arity), `pct` (percent past 100), `limit` (above the maximum), `bin`/`stats ... by bin(...)` (period without a unit).
Traces: logs.insights-abs-wrong-arity-rejected, logs.insights-coalesce-without-arguments-rejected, logs.insights-concat-without-arguments-rejected, logs.insights-count-distinct-wrong-arity-rejected, logs.insights-frommillis-wrong-arity-rejected, logs.insights-greatest-without-arguments-rejected, logs.insights-ispresent-wrong-argument-count-rejected, logs.insights-isvalidip-with-two-arguments-rejected, logs.insights-replace-missing-replacement-argument-rejected, logs.insights-stddev-second-argument-rejected, logs.insights-strcontains-wrong-arity-rejected, logs.insights-tomillis-wrong-argument-count-rejected, logs.insights-unmask-wrong-arity-rejected, logs.insights-pct-percent-past-100-rejected, logs.insights-limit-above-maximum-rejected, logs.insights-bin-ten-second-buckets-agree-across-unit-spellings, logs.insights-stats-bin-period-without-a-unit-rejected.

Cause 2 of 2 — command sequencing and shape rules are not checked.
AWS: refuses with MalformedQueryException when a command appears where the grammar does not allow it, or is incomplete.
Emulator: returns a queryId with HTTP 200.
Affected rules: `dedup` followed by a command other than `limit`; `diff` without a preceding `pattern`; an eleventh `stats` command; a dangling token in `fields`; `filter` with a missing operand; `parse` with a regex that has no named capture; `sort` placed ahead of the last `stats`; `sort` placed after the last `stats` where AWS refuses the form used; `sortsLast` outside `stats` or without a field; a `SOURCE logGroups(namePrefix: [...])` list with a sixth prefix (the documented cap is five).
Traces: logs.insights-dedup-followed-by-a-command-other-than-limit-rejected, logs.insights-diff-without-a-preceding-pattern-rejected, logs.insights-eleventh-stats-command-rejected, logs.insights-fields-dangling-token-rejected, logs.insights-filter-with-a-missing-operand-rejected, logs.insights-parse-regex-without-named-capture-rejected, logs.insights-sort-ahead-of-last-stats-rejected, logs.insights-sort-after-the-last-stats-orders-the-aggregated-rows, logs.insights-sorts-last-outside-stats-or-without-field-rejected, logs.insights-source-sixth-name-prefix-rejected.

Why: the whole contract of StartQuery is that a bad query is rejected before it is charged for and before the client starts polling. Accepting it means a client gets a queryId, polls GetQueryResults, and receives either empty results or a bare `Failed` with no diagnosis — so a typo is indistinguishable from 'no data matched'. Query-builder UIs and CI validation that rely on StartQuery to reject bad input lose their only check, and every negative test in this area passes vacuously.

Fix: the StartQuery handler's query validation — parse `queryString` against the CWLI grammar and check function arity/argument ranges and command sequencing, refusing with MalformedQueryException (HTTP 400) before a queryId is minted.

## Evidence
Every one of the 27 checks reports the same divergence shape, e.g.:
"StartQuery accepted a query it should refuse, answering {'queryId': '00000000-0000-4000-8000-000000000001', 'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}";
"StartQuery accepted 'stats sum(amount) as total by region | sort total asc | stats max(total) as top by region': {'queryId': ...}";
"query 'stats pct(ms, 101) as top' was accepted: {'queryId': ...}";
"StartQuery took 'stats stddev(bytes, 2) as spread' as valid: {'queryId': ...}";
"StartQuery ran \"SOURCE logGroups(namePrefix: [<six prefixes>]) | stats count(*) as hits\": {'queryId': ...}".
Expected in each case: HTTP 400 MalformedQueryException and no queryId.

## Steps to Reproduce
1. Create a log group.
2. Call logs:StartQuery with any of the malformed queries above, e.g. 'fields abs() as v', 'stats pct(ms, 101) as top', 'stats stddev(bytes, 2) as spread', 'dedup svc | fields svc', 'pattern-less diff', 'fields a,', 'filter a =', 'parse @message /(\\d+)/', 'stats sum(x) by r | sort r asc | stats max(x)', 'SOURCE logGroups(namePrefix: [p1,p2,p3,p4,p5,p6])'.
3. Each returns HTTP 200 with a queryId instead of MalformedQueryException.
