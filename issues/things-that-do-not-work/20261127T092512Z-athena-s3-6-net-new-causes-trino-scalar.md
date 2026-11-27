Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__s3.array-constructor-functions-and-unnest athena__s3.cast-varchar-to-date-and-timestamp-keeps-the-full-value athena__s3.current-timestamp-is-a-zoned-millisecond-timestamp athena__s3.date-arithmetic-and-field-extraction-on-current-date athena__s3.date-timestamp-and-interval-literals-evaluate athena__s3.integer-addition-overflow-fails-the-query athena__s3.integer-division-by-zero-fails-the-query athena__s3.json-functions-extract-scalars-and-parse-documents athena__s3.lower-and-upper-map-non-ascii-letters athena__s3.math-functions-ceil-floor-power-sqrt-mod-ln athena__s3.regexp-functions-match-extract-and-replace athena__s3.string-functions-position-padding-and-splitting athena__s3.try-cast-yields-null-for-an-unparseable-value
Issue-Key: athena__s3.array-constructor-functions-and-unnest|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.cast-varchar-to-date-and-timestamp-keeps-the-full-value|run#4|AssertionError|assert rows == [["2024-01-02", "2024-01-01 10:00:00.000"]], f"the casts read back as {rows!r}"
Issue-Key: athena__s3.current-timestamp-is-a-zoned-millisecond-timestamp|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.date-arithmetic-and-field-extraction-on-current-date|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.date-timestamp-and-interval-literals-evaluate|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.integer-addition-overflow-fails-the-query|run#7|AssertionError|assert status["State"] == "FAILED", (
Issue-Key: athena__s3.integer-division-by-zero-fails-the-query|run#7|AssertionError|assert status["State"] == "FAILED", (
Issue-Key: athena__s3.json-functions-extract-scalars-and-parse-documents|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.lower-and-upper-map-non-ascii-letters|run#4|AssertionError|assert rows == [["äéö ab", "ÄÉÖ AB", "true"]], f"case mapping gave {rows!r}"
Issue-Key: athena__s3.math-functions-ceil-floor-power-sqrt-mod-ln|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.regexp-functions-match-extract-and-replace|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.string-functions-position-padding-and-splitting|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__s3.try-cast-yields-null-for-an-unparseable-value|_select#3|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# athena__s3: 6 net-new causes — Trino scalar function library absent, Trino syntax forms (ARRAY[...], try_cast, INTERVAL, position IN) unparsed, CAST varchar→date truncates to the year, integer overflow and division by zero SUCCEED, lower/upper are ASCII-only

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Six independent root causes in the SQL engine behind StartQueryExecution, all observed on constant-expression SELECTs with an S3 OutputLocation (no catalogued table involved). Each must be closed before this issue is submitted. Related, already-filed context: 20261111T090701Z-athena-s-sql-engine-refuses-trino-hive-statements.md and 20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md are marked fixed but left these expressions unsupported; 20261121T213708Z-athena-2-net-new-causes-an-integer-varchar.md is open for an integer+varchar expression and covers neither the overflow nor the divide-by-zero case below.

=== Cause 1 of 6: Athena's Trino scalar/aggregate function library is absent — the engine resolves SQLite's functions instead ===
AWS: recorded behavior only (the Smithy model carries no SQL grammar). Every one of these Trino built-ins is available to an Athena engine-v3 DML query and the statements below end SUCCEEDED with the computed values: date/time — now(), date_diff, date_add, date_trunc, day, year, day_of_week; math — ceil, floor, power, sqrt, mod, ln; regexp — regexp_like, regexp_extract, regexp_replace; JSON — json_extract_scalar, json_extract, json_format, json_parse; string — strpos, lpad, rpad, reverse, starts_with, split_part, chr, codepoint.
Emulator: each statement settles State FAILED carrying 'no such function: <first unresolved name> in <the statement> at offset <n>' — a SQLite function-resolution diagnostic. Observed first-unresolved names: now, date_diff, ceil, regexp_like, json_extract_scalar.
Why: ordinary scalar SQL fails outright instead of returning values, and the failure is reported as a nonexistent-function error that no AWS client can map to anything. Every query using Athena's standard function library is unusable.
Fix: the query engine's function registry and evaluation used by StartQueryExecution's execution path.
Traces: athena__s3.current-timestamp-is-a-zoned-millisecond-timestamp, athena__s3.date-arithmetic-and-field-extraction-on-current-date, athena__s3.math-functions-ceil-floor-power-sqrt-mod-ln, athena__s3.regexp-functions-match-extract-and-replace, athena__s3.json-functions-extract-scalars-and-parse-documents

=== Cause 2 of 6: Trino syntax forms are not parsed — ARRAY[...] constructors, try_cast(x AS t), INTERVAL literals / AT TIME ZONE, and position(x IN y) ===
AWS: recorded behavior only. All four forms parse and evaluate: `ARRAY[1, 2, 3]` / `ARRAY['a','b']` as array constructors (with cardinality/element_at/contains/array_join over them); `try_cast('abc' AS integer)` yielding NULL and `try_cast('42' AS integer)` yielding 42; `date '2024-01-31' + interval '1' month` and `timestamp '2012-10-31 01:00 UTC' AT TIME ZONE 'America/Los_Angeles'`; and `position('l' IN 'hello')`. Each statement ends SUCCEEDED.
Emulator: each settles FAILED with a SQLite parse diagnostic: 'near "[1, 2, 3]": syntax error ... at offset 24' for the array constructor; 'near "AS": syntax error ... at offset 22' for try_cast; 'near "MONTH": syntax error ...' for the interval literal (the statement is first rewritten into an internal `__athena_temporal_literal('date', '2024-01-31')` form, which is itself visible in the error text); and 'no such table: hello' for `position('l' IN 'hello')`, where the IN-operand is parsed as a table reference.
Why: a client's query is rejected for a grammar the service accepts, and the diagnostics leak an internal rewrite (`__athena_temporal_literal`) and a nonsensical 'no such table: hello' that no Athena client can interpret.
Fix: the SQL front-end's grammar (constructor, try_cast, interval/time-zone literal and position-IN forms) and the temporal-literal rewrite in the StartQueryExecution execution path.
Traces: athena__s3.array-constructor-functions-and-unnest, athena__s3.try-cast-yields-null-for-an-unparseable-value, athena__s3.date-timestamp-and-interval-literals-evaluate, athena__s3.string-functions-position-padding-and-splitting

=== Cause 3 of 6: CAST of a varchar to DATE/TIMESTAMP truncates the value to its year ===
AWS: recorded behavior only. Casting a varchar date/timestamp keeps the whole value: a DATE renders as 'YYYY-MM-DD' and a TIMESTAMP as 'YYYY-MM-DD HH:MM:SS.mmm' in the result set.
Emulator: the execution SUCCEEDS but both cast columns read back as just the four-digit year.
Why: silently wrong data — a client gets a plausible-looking value with the month, day and time-of-day discarded, so date filters, joins and partition keys derived from a cast are all wrong, with no error anywhere.
Fix: the engine's CAST/temporal value conversion and the result-set rendering of DATE/TIMESTAMP values.
Traces: athena__s3.cast-varchar-to-date-and-timestamp-keeps-the-full-value

=== Cause 4 of 6: integer addition overflow SUCCEEDS instead of failing the execution ===
AWS: recorded behavior only. An INTEGER expression that overflows the 32-bit range (2147483647 + 1) fails the query: the execution reaches terminal State FAILED with an AthenaError describing the overflow.
Emulator: the execution reaches SUCCEEDED.
Why: a caller relying on the failure to detect a type/range error consumes a silently promoted value instead, so arithmetic that AWS refuses to complete produces a result the client trusts.
Fix: integer arithmetic typing/overflow checking in the engine, and the terminal-state classification that must mark the execution FAILED.
Traces: athena__s3.integer-addition-overflow-fails-the-query

=== Cause 5 of 6: integer division by zero SUCCEEDS instead of failing the execution ===
AWS: recorded behavior only. `1 / 0` fails the query — terminal State FAILED with an AthenaError describing the division by zero.
Emulator: the execution reaches SUCCEEDED (and the check reports no StateChangeReason at all).
Why: the same silent-wrong-answer exposure as Cause 4: a divide-by-zero guard built on query failure never fires. (The same defect is also observed under the athena service heading as athena.batch-get-query-execution-reports-runtime-failed-query-beside-succeeded-twin, carried in the athena issue of this sweep.)
Fix: arithmetic evaluation and terminal-state classification in the engine.
Traces: athena__s3.integer-division-by-zero-fails-the-query

=== Cause 6 of 6: lower()/upper() map only ASCII letters ===
AWS: recorded behavior only. Athena's lower()/upper() are Unicode-aware: lower('ÄÉÖ AB') is 'äéö ab', upper('äéö ab') is 'ÄÉÖ AB', and the round-trip equality the check asserts is true.
Emulator: the execution SUCCEEDS but the non-ASCII letters pass through unchanged — only the ASCII characters are case-mapped — and the equality column is false.
Why: silently wrong string data for any non-ASCII input: case-insensitive comparisons, dedup keys and normalisation built on lower()/upper() diverge for every accented or non-Latin value, with no error raised.
Fix: the engine's lower/upper implementation (Unicode case mapping) in its string-function layer.
Traces: athena__s3.lower-and-upper-map-non-ascii-letters

## Evidence
Cause 1: 'SELECT current_timestamp AS ts, now() = current_timestamp AS same' ended 'FAILED': 'no such function: now ... at offset 32'; "SELECT date_diff('day', current_date, date_add('day', 3, current_date)) AS diff, day(date_trunc('month', current_date)) AS first_day, year(current_date) >= 2024 AS recent, day_of_week(current_date) BETWEEN 1 AND 7 AS dow_ok" ended 'FAILED': 'no such function: date_diff ... at offset 7'; 'SELECT ceil(1.2) AS c, floor(1.7) AS f, power(2, 10) AS p, sqrt(16) AS s, mod(7, 3) AS m, ln(1) AS l' ended 'FAILED': 'no such function: ceil ... at offset 7'; regexp statement ended 'FAILED': 'no such function: regexp_like ... at offset 7'; json statement ended 'FAILED': 'no such function: json_extract_scalar ... at offset 7'. Expected State SUCCEEDED with the computed values in all five.
Cause 2: "SELECT cardinality(ARRAY[1, 2, 3]) AS n, element_at(ARRAY['a', 'b'], 2) AS e, contains(ARRAY[1, 2], 2) AS c, array_join(ARRAY['x', 'y'], '-') AS j" ended 'FAILED': 'near "[1, 2, 3]": syntax error ... at offset 24'; "SELECT try_cast('abc' AS integer) AS bad, try_cast('42' AS integer) AS good" ended 'FAILED': 'near "AS": syntax error ... at offset 22'; "SELECT date '2024-01-31' + interval '1' month AS next_month, timestamp '2012-10-31 01:00 UTC' AT TIME ZONE 'America/Los_Angeles' AS la_time" ended 'FAILED': 'near "MONTH": syntax error in SELECT __athena_temporal_literal(\'date\', \'2024-01-31\') + INTERVAL \'1\' MONTH ... at offset 70'; the string-function statement containing position('l' IN 'hello') ended 'FAILED': 'no such table: hello'.
Cause 3: expected rows [['2024-01-02', '2024-01-01 10:00:00.000']]; got [['2024', '2024']].
Cause 4: expected terminal State 'FAILED' for integer 2147483647 + 1; got 'SUCCEEDED'.
Cause 5: expected terminal State 'FAILED' for 1 / 0; got 'SUCCEEDED' with StateChangeReason None.
Cause 6: expected rows [['äéö ab', 'ÄÉÖ AB', 'true']]; got [['ÄÉÖ ab', 'äéö AB', 'false']].

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids. For each: create a workgroup with a ResultConfiguration.OutputLocation in S3, StartQueryExecution the statement, poll GetQueryExecution to a terminal state, and GetQueryResults where values are asserted.
Cause 1: 'SELECT current_timestamp AS ts, now() = current_timestamp AS same'; "SELECT date_diff('day', current_date, date_add('day', 3, current_date)) AS diff, day(date_trunc('month', current_date)) AS first_day, year(current_date) >= 2024 AS recent, day_of_week(current_date) BETWEEN 1 AND 7 AS dow_ok"; 'SELECT ceil(1.2) AS c, floor(1.7) AS f, power(2, 10) AS p, sqrt(16) AS s, mod(7, 3) AS m, ln(1) AS l'; "SELECT regexp_like('GET /api/v1/items 200', ' [0-9]{3}$') AS m, regexp_extract('GET /api/v1/items 200', '/api/(v[0-9]+)/', 1) AS ver, regexp_replace('a1b22c', '[0-9]+', '#') AS r"; "SELECT json_extract_scalar('{\"user\":{\"id\":42}}', '$.user.id') AS id, json_format(json_extract('{\"a\":[1,2]}', '$.a')) AS frag, json_format(json_parse('{\"k\":true}')) AS doc".
Cause 2: "SELECT cardinality(ARRAY[1, 2, 3]) AS n, element_at(ARRAY['a', 'b'], 2) AS e, contains(ARRAY[1, 2], 2) AS c, array_join(ARRAY['x', 'y'], '-') AS j"; "SELECT try_cast('abc' AS integer) AS bad, try_cast('42' AS integer) AS good"; "SELECT date '2024-01-31' + interval '1' month AS next_month, timestamp '2012-10-31 01:00 UTC' AT TIME ZONE 'America/Los_Angeles' AS la_time"; "SELECT strpos('hello', 'l') AS sp, position('l' IN 'hello') AS pos, lpad('7', 3, '0') AS lp, rpad('7', 3, '0') AS rp, reverse('abc') AS rv, starts_with('abc', 'ab') AS sw, split_part('a,b,c', ',', 2) AS part, chr(65) AS ch, codepoint('A') AS cp".
Cause 3: SELECT the varchar-to-date and varchar-to-timestamp casts and read the two result columns.
Cause 4: SELECT the integer expression 2147483647 + 1.
Cause 5: SELECT 1 / 0.
Cause 6: SELECT lower(<a string with Ä É Ö and ASCII letters>), upper(<the lowercase form>) and the round-trip equality.
