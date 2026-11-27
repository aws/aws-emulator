Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.insights-lookup-renamed-match-field-enriches-and-overwrites logs.insights-pattern-diff-compares-current-period-with-preceding-one logs.insights-entity-attributes-exposed-as-entity-fields logs.insights-dedup-keeps-every-event-missing-the-key logs.insights-isempty-and-isblank-classify-missing-empty-and-whitespace logs.insights-isempty-argument-count-rejected logs.insights-ispresent-partitions-events-by-field-existence logs.insights-strcontains-matches-substring-case-sensitively logs.insights-abs-yields-magnitude-and-compares-by-it logs.insights-sort-orders-values-naturally-with-non-numbers-first logs.insights-at-prefixed-json-key-discovered-with-an-extra-at logs.sql-query-with-invalid-regex-ends-failed-rejected logs.get-log-object-record-pointer-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.insights-lookup-renamed-match-field-enriches-and-overwrites|_insights#19|AssertionError|assert got["status"] in ("Scheduled", "Running"), (
Issue-Key: logs.insights-pattern-diff-compares-current-period-with-preceding-one|run#21|AssertionError|assert res["status"] == "Complete", f"diff query ended {res['status']!r}"
Issue-Key: logs.insights-entity-attributes-exposed-as-entity-fields|_query_until#7|AssertionError|raise AssertionError(f"{what}: {query!r} never returned the expected rows in {budget_s}s; last={last!r}")
Issue-Key: logs.insights-dedup-keeps-every-event-missing-the-key|run#17|AssertionError|assert kept == want, f"dedup svc returned {kept!r}, expected {want!r}"
Issue-Key: logs.insights-isempty-and-isblank-classify-missing-empty-and-whitespace|run#12|AssertionError|assert got.get("e") == want_e, f"isempty(v) for {body!r} was {got.get('e')!r}, expected {want_e!r}"
Issue-Key: logs.insights-isempty-argument-count-rejected|run#17|AssertionError|assert rows == want, f"{good!r} returned {rows!r}; expected {want!r}"
Issue-Key: logs.insights-ispresent-partitions-events-by-field-existence|run#24|AssertionError|assert got == want, f"(seq, ispresent(tier), ispresent(zone)) per event: expected {want!r}, got {got!r}"
Issue-Key: logs.insights-strcontains-matches-substring-case-sensitively|run#17|AssertionError|assert hits == want_hits, f'strcontains(text, "ERROR") per event was {hits!r}, expected {want_hits!r}'
Issue-Key: logs.insights-abs-yields-magnitude-and-compares-by-it|run#24|AssertionError|assert got == want, f"abs(n) > {threshold} yielded {got!r}; expected {want!r}"
Issue-Key: logs.insights-sort-orders-values-naturally-with-non-numbers-first|run#7|AssertionError|assert asc == want, f"sort v asc gave {asc!r}; the documented natural order is {want!r}"
Issue-Key: logs.insights-at-prefixed-json-key-discovered-with-an-extra-at|run#5|AssertionError|assert rows == [{"e": "v1", "k": "1"}], f"@@example.com read back {rows!r}; expected e=v1, k=1"
Issue-Key: logs.sql-query-with-invalid-regex-ends-failed-rejected|run#12|AssertionError|assert good["status"] == "Complete", f"the valid-pattern query ended {good['status']!r}"
Issue-Key: logs.get-log-object-record-pointer-rejected|_query_pointer#10|AssertionError|assert time.monotonic() < deadline, f"query never finished: {result['status']!r}"
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs Insights: 9 more evaluation divergences — lookup and pattern|diff fail, @entity fields are empty, dedup drops events missing the key, boolean functions return 'true'/'false' instead of '1'/'0', sort is not natural order, @-prefixed JSON keys are mis-discovered, a valid SQL query fails, and a GetLogObject pointer query never finishes

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQuery

## Description
Cause 1 of 9 — the `lookup` command fails.
AWS: `lookup <table> <field> as <alias> OUTPUT <cols>` enriches each row from a lookup table, overwriting an existing column of the same name, and the query completes. Emulator: the query ends `Failed`.
Traces: logs.insights-lookup-renamed-match-field-enriches-and-overwrites.

Cause 2 of 9 — `pattern ... | diff` fails.
AWS: `diff` after `pattern` compares the current period with the preceding one and completes. Emulator: ends `Failed`.
Traces: logs.insights-pattern-diff-compares-current-period-with-preceding-one.

Cause 3 of 9 — `@entity.*` fields are not exposed.
AWS: an event written with an `entity` resolves to the discovered fields `@entity.KeyAttributes.<name>` and `@entity.Attributes.<name>`. Emulator: the query returns a single empty row (`last=[{}]`) for the whole window.
Traces: logs.insights-entity-attributes-exposed-as-entity-fields.

Cause 4 of 9 — `dedup` drops events that do not have the key.
AWS: `dedup <field>` keeps the first event per distinct value AND keeps every event in which the field is absent. Emulator: the trailing event missing the field is dropped — returned `[{'seq': '1', 'svc': 'web'}, {'seq': '3'}, {'seq': '4', 'svc': 'db'}]` where AWS also returns `{'seq': '5'}`.
Traces: logs.insights-dedup-keeps-every-event-missing-the-key.

Cause 5 of 9 — boolean-valued functions render `true`/`false` instead of `1`/`0`.
AWS: `isempty`, `isblank`, `ispresent`, `strcontains` and boolean comparisons project the strings `1` (true) and `0` (false) in query results. Emulator: they project `true`/`false`, so every consumer of the value sees a different type and a different literal.
Traces: logs.insights-isempty-and-isblank-classify-missing-empty-and-whitespace, logs.insights-isempty-argument-count-rejected, logs.insights-ispresent-partitions-events-by-field-existence, logs.insights-strcontains-matches-substring-case-sensitively, logs.insights-abs-yields-magnitude-and-compares-by-it.

Cause 6 of 9 — `sort` does not use natural order.
AWS: `sort <field> asc` orders values naturally with non-numbers first, e.g. `['@x', 'A', 'host-20', 'host-100', 'item2', 'item9', 'item10', '5', '12']`. Emulator: plain lexicographic order, `['5', '12', '@x', 'A', 'host-100', 'host-20', 'item10', 'item2', 'item9']`.
Traces: logs.insights-sort-orders-values-naturally-with-non-numbers-first.

Cause 7 of 9 — an `@`-prefixed JSON key is mis-discovered.
AWS: a JSON key beginning with `@` is discovered with one extra `@` prefix, so `{"@example.com": ...}` is addressable and both sibling keys read back. Emulator: the row reads back as `[{'k': '1'}]` — the `@`-prefixed key is missing entirely.
Traces: logs.insights-at-prefixed-json-key-discovered-with-an-extra-at.

Cause 8 of 9 — a valid SQL-dialect query fails.
AWS: with `queryLanguage=SQL`, a query using a well-formed regex completes (only the invalid-regex twin ends `Failed`). Emulator: the valid-pattern query also ends `Failed`.
Traces: logs.sql-query-with-invalid-regex-ends-failed-rejected.

Cause 9 of 9 — the query behind a GetLogObject pointer never finishes.
AWS: the query used to obtain a `logObjectPointer` reaches a terminal status so the pointer can be used. Emulator: the query stays `Scheduled` for the whole window and never finishes. Smithy (logs): `QueryStatus` includes Scheduled|Running|Complete|Failed|Cancelled|Timeout.
Traces: logs.get-log-object-record-pointer-rejected.

Why: causes 1-3 and 8-9 remove whole features (lookup enrichment, pattern diffing, entity attribution, the SQL dialect, log-object retrieval) — a client's query returns nothing and cannot be retried into success. Causes 4-7 are worse than an outage because they are silently wrong: `dedup` loses rows, every boolean column has the wrong literal (so `where v = '1'` downstream never matches), and `sort` returns a different order from AWS for the same data, so paged or top-N results differ.

Fix: the CloudWatch Logs Insights evaluator — implement `lookup`, `diff`, entity-field discovery and the SQL dialect; correct `dedup`'s missing-key handling, the boolean projection ('1'/'0'), the natural-sort comparator and `@`-prefixed JSON key discovery; and make the pointer-producing query reach a terminal status.

## Evidence
Cause 1: "query 'lookup <table> code as errcode OUTPUT label, team | display errcode, label, team | sort @timestamp asc' ended 'Failed', not Complete".
Cause 2: "diff query ended 'Failed'".
Cause 3: "@entity fields: 'fields @entity.KeyAttributes.Type as t, ...' never returned the expected rows in 44.9s; last=[{}]".
Cause 4: "dedup svc returned [{'seq': '1', 'svc': 'web'}, {'seq': '3'}, {'seq': '4', 'svc': 'db'}], expected [... , {'seq': '5'}]".
Cause 5: "isempty(v) for {'v': ''} was 'true', expected '1'"; "(seq, ispresent(tier), ispresent(zone)) per event: expected [('1','1','0'), ...], got [('1','true','false'), ...]"; "strcontains(text, \"ERROR\") per event was {... 'upper': 'true' ...}, expected {... 'upper': '1' ...}".
Cause 6: "sort v asc gave ['5','12','@x','A','host-100','host-20','item10','item2','item9']; the documented natural order is ['@x','A','host-20','host-100','item2','item9','item10','5','12']".
Cause 7: "@@example.com read back [{'k': '1'}]; expected e=v1, k=1".
Cause 8: "the valid-pattern query ended 'Failed'".
Cause 9: "query never finished: 'Scheduled'".

## Steps to Reproduce
1. Create a log group and write the events each check describes; run the quoted query with logs:StartQuery and poll GetQueryResults.
2. lookup / diff / SQL queries end 'Failed'; @entity.* returns an empty row; dedup omits the key-less event; isempty/isblank/ispresent/strcontains project 'true'/'false'; sort returns lexicographic order; an @-prefixed JSON key is absent.
3. Run the query that yields a logObjectPointer — it stays 'Scheduled'.
