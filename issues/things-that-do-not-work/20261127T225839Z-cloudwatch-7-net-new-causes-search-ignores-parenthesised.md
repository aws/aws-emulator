Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.get-metric-data-search-parenthesised-boolean-groups cloudwatch.get-metric-data-search-punctuation-delimited-terms-match-composite-token cloudwatch.get-metric-data-search-tokens-match-case-insensitively-including-composites cloudwatch.get-metric-data-time-functions-derive-from-timestamps cloudwatch.get-metric-data-trimmed-statistics-withheld-for-negative-values cloudwatch.list-metrics-recently-active-keys-off-publication-not-timestamp cloudwatch.put-metric-data-body-over-five-mebibytes-rejected cloudwatch.awsquery-put-metric-data-gzip-body-accepted
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.get-metric-data-search-parenthesised-boolean-groups|_poll#7|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.get-metric-data-search-punctuation-delimited-terms-match-composite-token|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch.get-metric-data-search-tokens-match-case-insensitively-including-composites|run#12|AssertionError|assert hits == want, f"expected matches {want!r}, got {hits!r}"
Issue-Key: cloudwatch.get-metric-data-trimmed-statistics-withheld-for-negative-values|run#11|AssertionError|assert {k: v[1] for k, v in negative.items()} == {k: [] for k in stats}, (
Issue-Key: cloudwatch.list-metrics-recently-active-keys-off-publication-not-timestamp|run#8|AssertionError|assert recent == ["Backfilled", "Live"], (
Issue-Key: cloudwatch.put-metric-data-body-over-five-mebibytes-rejected|_refusal#13|AssertionError|raise AssertionError(f"PutMetricData was expected to be refused but returned {result!r}")
Issue-Key: cloudwatch.awsquery-put-metric-data-gzip-body-accepted|run#9|AssertionError|assert status == 200, f"the gzip-encoded awsQuery PutMetricData returned HTTP {status}: {body[:300]!r}"
Issue-Related: 20261127T225839Z-cloudwatch-7-net-new-metric-math-causes-in.md
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch: 7 net-new causes — SEARCH ignores parenthesised boolean groups and punctuation-joined terms while matching tokens too loosely, MINUTE-class time functions are missing, trimmed statistics are served for negative series, ListMetrics RecentlyActive keys off the datapoint timestamp, PutMetricData never refuses an over-5-MiB body, and a gzip-encoded awsQuery body is not decoded

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Seven further independent defects in service cloudwatch, grounded in failing behavioral checks that are neither OWNED nor ALREADY FILED. They are separate from the metric-math/label bundle filed alongside this issue (same service, different components: the search index, the statistic engine, metric discovery, and the awsQuery front door).

Cause 1 of 7: SEARCH ignores parenthesised OR/AND groups.
  AWS: a SEARCH expression's filter accepts boolean operators and parenthesised groups, including nesting: SEARCH('{ns,FunctionName} (Errors OR Invocations)', 'Sum', 60) finds every series whose tokens match either term, and SEARCH('{ns,FunctionName} MetricName="Errors" OR (MetricName="Invocations" AND (ProjectA OR ProjectB))', ...) finds the Errors series plus only the ProjectA/ProjectB Invocations series.
  Emulator: neither grouped expression ever selects the expected series; the check polls to its cap and is cut at the 60 s runner limit with the grouped searches still unsatisfied, while the same namespace's plain-term search is satisfied first (the positive control).
  Why: every dashboard or alarm built on a grouped search expression returns the wrong series set (or none), so fleets selected by boolean groups silently go unmonitored.
  Fix: the SEARCH filter parser/evaluator — parentheses, AND/OR precedence and nesting over the token and MetricName= predicates.
  Traces: cloudwatch.get-metric-data-search-parenthesised-boolean-groups (timed out with the grouped searches unmatched; the plain-term control matched).

Cause 2 of 7: a search term joined by a non-alphanumeric delimiter does not match the composite token it names.
  AWS: metric names are tokenized on non-alphanumeric boundaries and the composite is itself a token, so a term written with ANY delimiter — 'network-errors', 'network/errors', 'network+errors', 'Network_Errors' — matches the metric 'Network-Errors_2' and nothing else.
  Emulator: the hyphenated form matches (control) but the other delimiter forms never do; the check polls to its cap and is cut at the 60 s runner limit.
  Why: a client whose search term is typed with a different separator than the metric name uses finds nothing, so equivalent search expressions behave differently.
  Fix: the search tokenizer — tokenize the TERM on the same non-alphanumeric boundaries as the metric name before matching, so all delimiter forms of one composite are equivalent.
  Traces: cloudwatch.get-metric-data-search-punctuation-delimited-terms-match-composite-token.

Cause 3 of 7: SEARCH matches a term whose own tokenization splits a word, which AWS does not match.
  AWS: tokens are matched case-insensitively, including the composite tokens generated from a CamelCase name: for the metric 'CustomCount1' the terms count, COUNT, CustomCount, Count1, customcount and count1 all match, but 'couNT' does not — the term's own tokenization splits it ('cou','NT') and neither part is a generated token.
  Emulator: 'couNT' also matches, so the match is a plain case-insensitive substring test rather than token matching.
  Why: searches are over-broad — series a client deliberately excluded by casing boundaries are pulled into a graph or an alarm's SEARCH expression.
  Fix: the search matcher must tokenize the term the same way it tokenizes the metric name and compare tokens, not lowercase substrings.
  Traces: cloudwatch.get-metric-data-search-tokens-match-case-insensitively-including-composites (expected 'couNT': [], got 'couNT': [1.0]; every other term matched as expected).

Cause 4 of 7: the metric-math time functions do not exist.
  AWS: MINUTE, HOUR, DATE, MONTH, YEAR and EPOCH of a series return, per datapoint, that timestamp's UTC field (EPOCH the seconds since the epoch). A request using them returns 200.
  Emulator: HTTP 400 ValidationError "Error in expression 't': Function 'MINUTE' not found" — the first of the family is already unknown, so the whole request is refused.
  Why: time-of-day and calendar-window expressions (business-hours masks, day-of-month roll-ups) are unusable and fail as validation errors rather than returning data.
  Fix: register and implement the time functions in the metric-math function registry, deriving each value from the datapoint's UTC timestamp.
  Traces: cloudwatch.get-metric-data-time-functions-derive-from-timestamps.

Cause 5 of 7: trimmed/percentile-family statistics are served for a series that holds a negative value.
  AWS: percentile (p50), trimmed-mean (tm90), winsorized-mean (wm90) and IQM are unavailable once a metric's values include a negative number — the series comes back with no datapoints for those statistics, while an all-positive series yields all of them.
  Emulator: p50 is correctly withheld, but tm90, wm90 and IQM are computed and returned for a series containing -1 (values 1.148…, 1.333…, 1.666… for the recorded run).
  Why: a client reading a trimmed statistic over a signed series (deltas, balances, drift) is served a number AWS never serves, so dashboards and alarms act on a statistic that does not exist for that metric.
  Fix: the statistic engine's availability rule — the same negative-value guard that already withholds percentiles must cover the trimmed-mean, winsorized-mean and IQM family.
  Traces: cloudwatch.get-metric-data-trimmed-statistics-withheld-for-negative-values.

Cause 6 of 7: ListMetrics RecentlyActive filters on the datapoint's timestamp instead of its publication time.
  AWS: Smithy ListMetricsInput$RecentlyActive — "To filter the results to show only metrics that have had data points PUBLISHED in the past three hours, specify this parameter with a value of PT3H." A metric published moments ago whose only datapoint is stamped 6 hours in the past is therefore recently active and must be listed.
  Emulator: with RecentlyActive=PT3H only the metric whose datapoint timestamp is recent is listed; the back-dated-but-just-published metric is omitted (['Live'] instead of ['Backfilled','Live']), while an unfiltered ListMetrics lists both.
  Why: a client discovering live metrics (agents and batch jobs that backfill timestamps) loses exactly the metrics it is publishing right now, so discovery-driven dashboards and alarm wiring skip them.
  Fix: metric-discovery state must record and filter on the publication instant of the last datum, not the datum's timestamp.
  Traces: cloudwatch.list-metrics-recently-active-keys-off-publication-not-timestamp.

Cause 7 of 7: PutMetricData accepts a body larger than the 5 MiB limit instead of refusing it.
  AWS: an uncompressed PutMetricData body of exactly 5 MiB (5 242 880 bytes) is ingested; one byte more is refused with HTTP 413 and error code RequestEntityTooLarge, whose message is the fixed text 'Request size ' + the measured request size in bytes + ' exceeded 5242880 bytes', and none of that request's data is stored.
  Emulator: the 5 242 881-byte body is answered HTTP 200 with an empty PutMetricData result — no refusal, no 413, no RequestEntityTooLarge, and the oversized request's data is accepted.
  Why: a client relying on the limit to size its batches is never told it exceeded it; a request AWS rejects wholesale is partially or fully ingested in the emulator, so the two disagree on what data exists, and client-side 413 retry/split logic is never exercised.
  Fix: a request-size guard on the PutMetricData path (before ingestion) that refuses over-limit bodies with 413 RequestEntityTooLarge and the message template above, and stores nothing from the refused request.
  Traces: cloudwatch.put-metric-data-body-over-five-mebibytes-rejected.

Cause 8 of 7 is not claimed; the seventh and final cause is the gzip body below — counted as cause 7's sibling in the same front door, so note it explicitly as an eighth item a fixer must close:
  Cause 8 of 8: a gzip-encoded awsQuery body is not decoded. AWS: PutMetricData carries @requestCompression{encodings:[gzip]} in the Smithy model, so a POST whose body is the urlencoded awsQuery form compressed with gzip and marked 'Content-Encoding: gzip' is accepted (HTTP 200) and its datapoints read back exactly like an uncompressed publish of the same payload. Emulator: the identical uncompressed request is accepted, but the gzip-encoded one is answered HTTP 400 with an awsQuery ErrorResponse of Type Sender, Code InvalidAction and message 'unknown action: request is missing the `Action` field' — the front door parses the compressed bytes as a form body, so the Action parameter is never found. Why: every SDK that enables request compression (the default for PutMetricData in current SDKs) is refused with a nonsensical InvalidAction, so large publishes fail on the wire. Fix: the awsQuery front door must decode Content-Encoding: gzip before parsing the form body (the same decode the JSON/CBOR front doors need), not the PutMetricData handler. Traces: cloudwatch.awsquery-put-metric-data-gzip-body-accepted.

## Evidence
Suite: service cloudwatch scored 704/732 (96%), 28 failed; 10 OWNED and skipped. The 8 checks below back the causes above (the metric-math/label causes are filed in the sibling cloudwatch issue).

[mismatch] cloudwatch.get-metric-data-search-parenthesised-boolean-groups — Check timed out after 60.0s with '(Errors OR Invocations)' and the nested group never selecting the published series; the plain-term search over the same namespace matched first.
[mismatch] cloudwatch.get-metric-data-search-punctuation-delimited-terms-match-composite-token — Check timed out after 60.0s; 'network-errors' matched 'Network-Errors_2' but 'network/errors', 'network+errors' and 'Network_Errors' did not.
[mismatch] cloudwatch.get-metric-data-search-tokens-match-case-insensitively-including-composites
  AssertionError: expected matches {... 'couNT': [] ...}, got {... 'couNT': [1.0] ...}
[error] cloudwatch.get-metric-data-time-functions-derive-from-timestamps
  botocore ValidationException (ValidationError) calling GetMetricData: Error in expression 't': Function 'MINUTE' not found
  Expected (AWS): HTTP 200 and MINUTE/HOUR/DATE/MONTH/YEAR/EPOCH each returning the datapoint's UTC field.
[mismatch] cloudwatch.get-metric-data-trimmed-statistics-withheld-for-negative-values
  AssertionError: a series holding -1 must yield none of ['IQM', 'p50', 'tm90', 'wm90'], got {'p': [], 'tm': [1.148148148148148], 'wm': [1.3333333333333333], 'iqm': [1.6666666666666667]}
  (the all-positive twin yielded every statistic — positive control)
[mismatch] cloudwatch.list-metrics-recently-active-keys-off-publication-not-timestamp
  AssertionError: both metrics were published moments ago, so both are recently active; got ['Live']
[mismatch] cloudwatch.put-metric-data-body-over-five-mebibytes-rejected
  AssertionError: PutMetricData was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for a measured 5242881-byte body
  Expected (AWS): HTTP 413, Code RequestEntityTooLarge, Message 'Request size 5242881 exceeded 5242880 bytes' (the size is the measured request size, the limit is fixed), nothing stored; the 5242880-byte body accepted and readable.
[mismatch] cloudwatch.awsquery-put-metric-data-gzip-body-accepted
  AssertionError: the gzip-encoded awsQuery PutMetricData returned HTTP 400: '<?xml version="1.0"?><ErrorResponse><Error><Type>Sender</Type><Code>InvalidAction</Code><Message>unknown action: request is missing the `Action` field</Message></Error><RequestId>...</RequestId></ErrorResponse>'
  Expected (AWS): HTTP 200 for the gzip-encoded body, with its 40 datapoints reading back exactly like the uncompressed publish's 40.

Namespaces, metric names, request ids and the 5242881-byte measurement of the recorded runs are per-run values filling the templates; the limit (5242880) and the message's fixed text are the requirements.

## Steps to Reproduce
Build and boot the emulator from this checkout, then:
1. Cause 1: PutMetricData three series in one namespace with dimension FunctionName (Errors/ProjectB-api, Invocations/ProjectA-api, Invocations/Other-api); GetMetricData with Expression SEARCH('{<ns>,FunctionName} (Errors OR Invocations)', 'Sum', 60) -> AWS all three; then the nested group 'MetricName="Errors" OR (MetricName="Invocations" AND (ProjectA OR ProjectB))' -> AWS the Errors series plus ProjectA's Invocations only. Emulator: neither selects the expected set.
2. Cause 2: publish 'Network-Errors_2' and 'DiskReads'; SEARCH with the terms 'network-errors', 'network/errors', 'network+errors', 'Network_Errors' -> AWS each finds only Network-Errors_2; emulator only the hyphenated form.
3. Cause 3: publish 'CustomCount1'; SEARCH the term 'couNT' -> AWS no match; emulator matches.
4. Cause 4: GetMetricData with Expression 'MINUTE(m1)' -> AWS 200 with the timestamp's UTC minute; emulator 400 ValidationError "Function 'MINUTE' not found".
5. Cause 5: PutMetricData one datum with Values [-1,2,3] and a twin with [1,2,3]; GetMetricData MetricStat Stat p50/tm90/wm90/IQM over each -> AWS: all four empty for the negative series, all four present for the positive one; emulator serves tm90/wm90/IQM for the negative series.
6. Cause 6: PutMetricData two metrics in one namespace, one stamped 6 h ago and one now, both published now; ListMetrics(Namespace=ns) -> both; ListMetrics(Namespace=ns, RecentlyActive='PT3H') -> AWS both; emulator only the recently stamped one.
7. Cause 7: PutMetricData with an uncompressed body of exactly 5242880 bytes (accepted) and one of 5242881 bytes -> AWS 413 RequestEntityTooLarge 'Request size 5242881 exceeded 5242880 bytes' and no data stored; emulator 200 for both. Verify with GetMetricStatistics that the refused request's series is absent.
8. Cause 8: POST the urlencoded awsQuery PutMetricData form twice, once plain and once gzip-compressed with Content-Encoding: gzip, both SigV4-signed -> AWS 200 for both and identical readback; emulator 400 InvalidAction for the gzip one.
9. Re-replay every check id in trace_ids to confirm the fix.
