Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.get-metric-data-avg-of-array-counts-missing-as-zero cloudwatch.get-metric-data-if-true-branch-missing-point-is-zero cloudwatch.get-metric-data-fill-from-another-metric cloudwatch.get-metric-data-if-without-false-value-filters-series cloudwatch.get-metric-data-lambda-source-error-reports-internal-error-series cloudwatch.get-metric-data-lambda-source-validation-error-refuses-request cloudwatch.get-metric-data-metrics-string-filter-matches-query-ids cloudwatch.get-metric-data-range-and-relative-dynamic-labels-resolve cloudwatch.get-metric-data-region-and-account-dynamic-labels-resolve cloudwatch.get-metric-data-search-dynamic-label-resolves-per-series
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.get-metric-data-avg-of-array-counts-missing-as-zero|run#9|AssertionError|assert got[1] == [25.0, 10.0], (
Issue-Key: cloudwatch.get-metric-data-if-true-branch-missing-point-is-zero|run#9|AssertionError|assert got[1] == [20.0, 0.0, 5.0], (
Issue-Key: cloudwatch|GetMetricData|ValidationError
Issue-Key: cloudwatch.get-metric-data-metrics-string-filter-matches-query-ids|run#9|AssertionError|assert by_id[1] == [3.0], (
Issue-Key: cloudwatch.get-metric-data-range-and-relative-dynamic-labels-resolve|run#10|AssertionError|assert "${" not in label, f"the {labels[qid]!r} label came back verbatim: {label!r}"
Issue-Key: cloudwatch.get-metric-data-region-and-account-dynamic-labels-resolve|run#9|AssertionError|assert got["Label"] == want, f"expected the label to render {want!r}, got {got['Label']!r}"
Issue-Key: cloudwatch.get-metric-data-search-dynamic-label-resolves-per-series|run#10|AssertionError|assert by_value == want, (
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch: 7 net-new metric-math causes in GetMetricData — missing points are not zero in array/IF aggregation, FILL(m1,m2) and 2-argument IF are refused, LAMBDA/MINUTE-class functions do not exist, METRICS("string") matches nothing, and dynamic labels are returned verbatim

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Seven independent defects in the GetMetricData expression engine and label renderer. All are grounded in failing behavioral checks of service cloudwatch and none is marked OWNED or ALREADY FILED. Smithy (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801) MetricDataQuery$Expression: "a metric math expression to be performed on the returned data ... see Metric Math Syntax and Functions"; MetricDataQuery$Label: "You can put dynamic expressions into a label ... see Using Dynamic Labels". GetMetricData models exactly one error, InvalidNextToken (400); every refusal below is therefore not a modelled outcome for a well-formed expression.

Cause 1 of 7: in an aggregate over an array of metrics, a timestamp where one input has no datapoint is not counted as zero.
  AWS: metric math aggregate functions over an array ([m1, m2]) evaluate at every timestamp in the union of the inputs and treat a missing datapoint of a member as 0 in the aggregation and in its divisor, so AVG([m1,m2]) over (30,20) then (20,missing) is 25 then 10.
  Emulator: the missing member is dropped from the aggregation, so the second point is the surviving value's own average: [25.0, 20.0].
  Why: a client averaging a fleet of series silently reads a per-present-member average instead of a per-member one, so a dip to zero in one series never moves the aggregate and an alarm on it never fires.
  Fix: the metric-math evaluator's array-aggregation path (AVG/SUM/MIN/MAX over an array of query ids) and its treatment of absent datapoints.
  Traces: cloudwatch.get-metric-data-avg-of-array-counts-missing-as-zero ('AVG([m1, m2]) must be (30+20)/2 then (20+0)/2, got [25.0, 20.0]').

Cause 2 of 7: IF's true branch yields nothing where the chosen series has no datapoint, instead of 0.
  AWS: IF(cond, a, b) is evaluated per timestamp over the union of the inputs; where the condition holds but the selected series has no datapoint at that timestamp, the result is 0 and the timestamp is still returned.
  Emulator: that timestamp is dropped entirely — three expected points come back as two.
  Why: a conditional series silently shortens, so downstream aggregation, alarm evaluation over missing data, and graphs lose the very timestamps the condition selected.
  Fix: the metric-math IF evaluator's per-timestamp alignment and its missing-datapoint result.
  Traces: cloudwatch.get-metric-data-if-true-branch-missing-point-is-zero ('expected [m2=20, 0 for the true-but-missing m2 point, 5 for the false point], got [20.0, 5.0]').

Cause 3 of 7: FILL(m1, m2) — filling from another metric — is refused as invalid.
  AWS: FILL's second argument may be a number, the keywords REPEAT or LINEAR, OR another metric/expression, in which case m1's missing datapoints are filled with that series' value at the same timestamp. A well-formed FILL(m1, m2) returns 200 with the filled series.
  Emulator: HTTP 400 ValidationError, message "Error in expression 'f': FILL fills with a number, REPEAT or LINEAR" — the emulator's own wording for a restriction AWS does not impose; the whole request is refused.
  Why: a valid dashboard/alarm expression that backfills a primary series from a fallback series cannot be used at all; the client sees a validation error for an expression AWS accepts.
  Fix: the metric-math FILL argument validator and evaluator (accept a series-valued second argument and fill per timestamp).
  Traces: cloudwatch.get-metric-data-fill-from-another-metric.

Cause 4 of 7: IF with the false value omitted is refused.
  AWS: IF(cond, value) is well-formed — with no false value the datapoints where the condition does not hold are filtered out, leaving only the matching timestamps and values.
  Emulator: HTTP 400 ValidationError, message "Error in expression 'slow': IF takes a condition, a value when it holds and a value when it does not" — again an emulator-authored refusal of a legal arity.
  Why: the standard filtering idiom IF(m1 > 10, m1) is unusable, so every filter-by-threshold expression fails closed with a validation error.
  Fix: the metric-math IF arity validator (2 or 3 arguments) and the 2-argument filtering semantics.
  Traces: cloudwatch.get-metric-data-if-without-false-value-filters-series.

Cause 5 of 7: the LAMBDA() metric-math function does not exist, so no CloudWatch custom data source can be queried.
  AWS: LAMBDA('<function>', args...) in MetricDataQuery$Expression invokes the named Lambda data-source connector with a GetMetricDataRequest event and returns its MetricDataResults; when the connector answers an Error object with Code 'Validation', the whole GetMetricData call is refused 400 ValidationError with the message "Error in expression '<query id>': <the data source's Value>" (fixed text 'Error in expression ' + the query's own Id + ': ', then the data source's own message); a connector the caller may not invoke yields a per-result StatusCode 'Forbidden'.
  Emulator: every expression naming LAMBDA is refused 400 ValidationError "Error in expression '<id>': Function 'LAMBDA' not found", so neither the success path, the Forbidden path nor the Validation-refusal wording can be exercised.
  Why: CloudWatch's multi-data-source feature is entirely unavailable, and the refusal a client would code against (its own connector's message) is replaced by a 'function not found' parse error.
  Fix: the metric-math function registry plus a LAMBDA evaluator that invokes the connector, maps its MetricDataResults onto the requested grid, maps an Error{Code:Validation} onto the request-wide ValidationError message above, and maps a missing invoke permission onto StatusCode 'Forbidden'.
  Traces: cloudwatch.get-metric-data-lambda-source-error-reports-internal-error-series, cloudwatch.get-metric-data-lambda-source-validation-error-refuses-request. (The same missing function also fails cloudwatch__lambda.lambda-data-source-without-datasource-principal-permission-rejected, filed separately under that service heading.)

Cause 6 of 7: METRICS("string") selects nothing.
  AWS: METRICS() returns the array of all MetricStat queries in the request, and METRICS("substring") returns those whose query Id CONTAINS the substring — the metric names are irrelevant. SUM(METRICS("errors")) over queries errors_throttle=1, errors_fault=2, req=4 is 3; SUM(METRICS("Requests")) is empty because no Id contains it.
  Emulator: SUM(METRICS("errors")) returns an empty series — the filter matches no query at all.
  Why: the documented grouping idiom for request-scoped roll-ups yields nothing, so a dashboard/alarm expression that totals a family of queries by Id reads empty rather than their sum.
  Fix: the METRICS() implementation — match the argument against each query's Id by substring and return the array of those queries.
  Traces: cloudwatch.get-metric-data-metrics-string-filter-matches-query-ids ('METRICS("errors") must sum the two queries whose Id contains 'errors' (1+2), got []').

Cause 7 of 7: dynamic labels are never rendered — the ${...} token is echoed verbatim in MetricDataResult$Label.
  AWS: a Label containing a dynamic expression is rendered per returned series before it is serialized: ${MIN_MAX_RANGE} and ${FIRST_LAST_RANGE} render the numeric range of the series' own datapoints, ${LAST_TIME_RELATIVE} and ${FIRST_LAST_TIME_RANGE} render times derived from them, ${PROP('Region')} and ${PROP('AccountId')} render the series' region and account id, and ${PROP('Dim.<name>')} renders THAT series' value for the dimension — so a SEARCH returning three hosts yields three different labels. Fixed text in the label is kept verbatim; only the ${...} tokens are substituted, from the result series and the request's region/account.
  Emulator: the Label is returned exactly as submitted — 'spread ${MIN_MAX_RANGE}', "${PROP('Region')}:${PROP('AccountId')}" and "latency on ${PROP('Dim.Host')}" (identical for every searched series).
  Why: every client that labels a graph, a report or a notification by dynamic label shows raw template text, and per-series labels built from a dimension cannot distinguish the series at all.
  Fix: the GetMetricData label renderer (run per MetricDataResult, after the values are computed, over the range/relative-time/PROP token families) rather than the serializer passing Label through.
  Traces: cloudwatch.get-metric-data-range-and-relative-dynamic-labels-resolve, cloudwatch.get-metric-data-region-and-account-dynamic-labels-resolve, cloudwatch.get-metric-data-search-dynamic-label-resolves-per-series.

Note on severity: causes 3, 4 and 5 are refusals of requests AWS answers 200, so a client cannot work around them; the rest are wrong 200 bodies.

## Evidence
Suite: service cloudwatch scored 704/732 (96%), 28 failed; 10 are OWNED by open issues and skipped. The 8 checks below back the 7 causes.

[mismatch] cloudwatch.get-metric-data-avg-of-array-counts-missing-as-zero
  AssertionError: AVG([m1, m2]) must be (30+20)/2 then (20+0)/2, got [25.0, 20.0]
[mismatch] cloudwatch.get-metric-data-if-true-branch-missing-point-is-zero
  AssertionError: expected [m2=20, 0 for the true-but-missing m2 point, 5 for the false point], got [20.0, 5.0]
[error] cloudwatch.get-metric-data-fill-from-another-metric
  botocore ValidationException (ValidationError) calling GetMetricData: Error in expression 'f': FILL fills with a number, REPEAT or LINEAR
  Expected (AWS): HTTP 200, series [1.0, 8.0] (m1's 1 kept, the gap filled with m2's 8).
[error] cloudwatch.get-metric-data-if-without-false-value-filters-series
  botocore ValidationException (ValidationError): Error in expression 'slow': IF takes a condition, a value when it holds and a value when it does not
  Expected (AWS): HTTP 200, values [20.0, 30.0] at minutes 1 and 3.
[error] cloudwatch.get-metric-data-lambda-source-error-reports-internal-error-series
  botocore ValidationException (ValidationError): Error in expression 'healthy': Function 'LAMBDA' not found
[error] cloudwatch.get-metric-data-lambda-source-validation-error-refuses-request
  botocore ValidationException (ValidationError): Error in expression 'good': Function 'LAMBDA' not found
  Expected (AWS): the control query returns the connector's series with StatusCode 'Complete'; the sibling rejecting query makes the whole call fail with HTTP 400, code ValidationError, message "Error in expression 'bad': <the connector's own Value>".
[mismatch] cloudwatch.get-metric-data-metrics-string-filter-matches-query-ids
  AssertionError: METRICS("errors") must sum the two queries whose Id contains 'errors' (1+2), got []
[mismatch] cloudwatch.get-metric-data-range-and-relative-dynamic-labels-resolve
  AssertionError: the 'spread ${MIN_MAX_RANGE}' label came back verbatim: 'spread ${MIN_MAX_RANGE}'
[mismatch] cloudwatch.get-metric-data-region-and-account-dynamic-labels-resolve
  AssertionError: expected the label to render '<region>:<account>', got "${PROP('Region')}:${PROP('AccountId')}"
[mismatch] cloudwatch.get-metric-data-search-dynamic-label-resolves-per-series
  AssertionError: each searched series must carry its own Host in the label; got "latency on ${PROP('Dim.Host')}" for all three series

The region, account id, namespaces, host names and timestamps in the recorded runs are per-run values filling the templates above, not requirements; the requirement is that the ${...} tokens are substituted from the series and the request.

## Steps to Reproduce
Common setup: build and boot the emulator from this checkout; publish the datapoints each cause names with cw:PutMetricData into a unique namespace, then read them back with cw:GetMetricData (Period 60, ScanBy TimestampAscending) over a window containing them.
1. Cause 1: publish m1=30,20 and m2=20 (second minute missing) and query Expression 'AVG([m1,m2])' -> AWS [25.0, 10.0]; emulator [25.0, 20.0].
2. Cause 2: query an IF whose true branch selects a series missing a point in the window -> AWS keeps that timestamp with value 0; emulator drops it.
3. Cause 3: publish Primary at t0 and Fallback at t0,t1; query Expression 'FILL(m1, m2)' -> AWS 200 with [1.0, 8.0]; emulator 400 ValidationError 'FILL fills with a number, REPEAT or LINEAR'.
4. Cause 4: publish Latency 5,20,8,30 and query Expression 'IF(m1 > 10, m1)' -> AWS 200 with [20.0, 30.0]; emulator 400 ValidationError.
5. Cause 5: create a Lambda connector that answers a GetMetricData event, lambda:AddPermission for principal lambda.datasource.cloudwatch.amazonaws.com, then query Expression LAMBDA('<fn>', ...) -> AWS 200 with the connector's series (and 400 ValidationError "Error in expression '<id>': <connector message>" when the connector answers Error Code Validation); emulator 400 ValidationError "Function 'LAMBDA' not found".
6. Cause 6: submit MetricStat queries with Ids errors_throttle, errors_fault, req and the expression 'SUM(METRICS("errors"))' -> AWS the sum of the two 'errors*' queries; emulator empty.
7. Cause 7: submit any query with Label 'spread ${MIN_MAX_RANGE}', "${PROP('Region')}:${PROP('AccountId')}" or "latency on ${PROP('Dim.Host')}" (the last over a SEARCH returning several dimension values) -> AWS renders each token per series; emulator echoes the token.
8. Re-replay every check id in trace_ids to confirm the fix.
