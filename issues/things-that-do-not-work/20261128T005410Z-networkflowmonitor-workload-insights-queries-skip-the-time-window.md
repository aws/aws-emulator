Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.data-query-start-beyond-seven-days-rejected networkflowmonitor.start-workload-insights-query-start-time-past-seven-days-rejected networkflowmonitor.data-query-zero-width-window-results-rejected
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor.data-query-start-beyond-seven-days-rejected|run#12|AssertionError|raise AssertionError(f"a startTime beyond seven days was accepted as query {accepted.get('queryId')!r}")
Issue-Key: networkflowmonitor.start-workload-insights-query-start-time-past-seven-days-rejected|_refusal#7|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: networkflowmonitor.data-query-zero-width-window-results-rejected|_refusal#22|AssertionError|raise AssertionError(f"the call was expected to be refused but answered {answer!r}")
Issue-Commit: 73e1e77630833aa0f20a2b43ded051b89907df1b

# networkflowmonitor workload-insights queries skip the time-window validation AWS applies (7-day look-back, zero-width window)

> _Note: this issue was identified against the code base at commit `73e1e77630833aa0f20a2b43ded051b89907df1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryWorkloadInsightsTopContributorsData

## Description
The NetworkFlowMonitor workload-insights query interface bounds the window a query may ask for: the look-back is at most seven days, and a window whose start equals its end is not answerable. The emulator applies neither rule, so a query AWS refuses is minted, runs to SUCCEEDED and even serves a result page. 3 causes follow; each needs the window rule applied in its own operation.

Cause 1 of 3: StartQueryWorkloadInsightsTopContributorsData accepts a startTime more than 7 days in the past.
  AWS:      Smithy: StartQueryWorkloadInsightsTopContributorsData binds POST /workloadInsights/{scopeId}/topContributorsDataQueries; startTime and endTime are required Timestamp members (timestampFormat date-time) and ValidationException (httpError 400) is modelled on the operation. The model states no bound on the window, so this is recorded behavior: a one-hour window starting one hour INSIDE the seven days is accepted and its query reaches SUCCEEDED, while the same one-hour window starting one hour BEYOND the seven days is refused with HTTP 400, code ValidationException, message "StartTime cannot be more than 7 days in the past". That whole sentence is fixed text -- it interpolates no value from the request (no timestamp, no limit, no member name).
  Emulator: the out-of-window call is accepted: HTTP 200 with a queryId in the response body, no refusal at all.
  Why:      a client that relies on the service to reject an over-long look-back is handed a query id for a window the service cannot answer. It then polls to SUCCEEDED and reads a page it believes covers the requested window, so the caller silently gets data for a period it never asked for (or empty data it reads as 'no traffic') instead of being told to narrow the window.
  Fix:      the time-window validation of the StartQueryWorkloadInsightsTopContributorsData handler in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops -- reject startTime older than seven days before now with the modelled ValidationException before a query is created.
  Traces:   networkflowmonitor.data-query-start-beyond-seven-days-rejected.

Cause 2 of 3: StartQueryWorkloadInsightsTopContributors accepts a startTime more than 7 days in the past.
  AWS:      Smithy: StartQueryWorkloadInsightsTopContributors binds POST /workloadInsights/{scopeId}/topContributorsQueries with the same required startTime/endTime Timestamps and models ValidationException (400). Recorded behavior: the same seven-day look-back rule, refused with HTTP 400, ValidationException, message "StartTime cannot be more than 7 days in the past" (entirely fixed text), while the neighbouring window one hour inside the seven days starts a query that reaches SUCCEEDED.
  Emulator: the out-of-window call answers HTTP 200 with a queryId (the replay shows queryId '00000000-0000-4000-9000-000000000002'; that value is this run's generated id, not part of the requirement).
  Why:      as cause 1, on the non-data top-contributors family: the unbounded look-back is accepted, so a caller never learns its window is outside the retained range.
  Fix:      the time-window validation of the StartQueryWorkloadInsightsTopContributors handler in the same crate.
  Traces:   networkflowmonitor.start-workload-insights-query-start-time-past-seven-days-rejected.

Cause 3 of 3: GetQueryResultsWorkloadInsightsTopContributorsData serves a result page for a zero-width window instead of refusing it.
  AWS:      Smithy: GetQueryResultsWorkloadInsightsTopContributorsData binds GET /workloadInsights/{scopeId}/topContributorsDataQueries/{queryId}/results, its output requires `unit` (MetricUnit enum) and `datapoints`, and ValidationException (400) is modelled. Recorded behavior: a query started with startTime == endTime is accepted by StartQuery* and runs to SUCCEEDED, but the results read for that query is refused with HTTP 400, code ValidationException, message "The parameter StartTime must not equal parameter EndTime. StartTime must be at least 15 minutes before the current time." -- two fixed sentences naming the input MEMBERS StartTime and EndTime, with nothing interpolated from the request. The refusal is specific to that query: results of a normal one-hour-window query of the same scope are still served afterwards.
  Emulator: the results read answers HTTP 200 with a modelled body ({'unit': 'Bytes', 'datapoints': []}) -- i.e. an empty, successful page.
  Why:      a collapsed window is a caller bug; AWS reports it at the read. A client gets a 200 with an empty datapoints list instead, which is indistinguishable from 'the window genuinely had no traffic', so a monitoring caller draws a zero line from a request the service never evaluated.
  Fix:      the results path of the GetQueryResultsWorkloadInsightsTopContributorsData handler -- it must re-check the stored query's window (start strictly before end) and emit the modelled ValidationException with that message rather than serialize an empty page.
  Traces:   networkflowmonitor.data-query-zero-width-window-results-rejected.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout; service heading networkflowmonitor, 3 failing checks, verdict=mismatch.

1) networkflowmonitor.data-query-start-beyond-seven-days-rejected (StartQueryWorkloadInsightsTopContributorsData)
   The check first proves the neighbour works: a one-hour window starting 7 days + 1 hour ago... minus, i.e. one hour INSIDE the seven days, is accepted and its query reaches SUCCEEDED (this part passed).
   expected for startTime = now - 7d - 1h: ClientError ('ValidationException', 'StartTime cannot be more than 7 days in the past')
   actual:   HTTP 200, queryId returned -- AssertionError: "a startTime beyond seven days was accepted as query '00000000-0000-4000-9000-000000000003'"

2) networkflowmonitor.start-workload-insights-query-start-time-past-seven-days-rejected (StartQueryWorkloadInsightsTopContributors)
   expected for startTime = now - 7d - 1h: ('ValidationException', 'StartTime cannot be more than 7 days in the past')
   actual:   HTTPStatusCode 200 with body {'queryId': '00000000-0000-4000-9000-000000000002'} -- AssertionError: "the call was expected to be refused but returned ..."

3) networkflowmonitor.data-query-zero-width-window-results-rejected (GetQueryResultsWorkloadInsightsTopContributorsData)
   Control (passed): a one-hour window query SUCCEEDS and its results are served with a non-empty `unit`.
   Then startTime == endTime: the query is accepted and reaches SUCCEEDED (as on AWS), and the results read is
   expected: ('ValidationException', 'The parameter StartTime must not equal parameter EndTime. StartTime must be at least 15 minutes before the current time.')
   actual:   HTTPStatusCode 200, body {'unit': 'Bytes', 'datapoints': []} -- AssertionError: "the call was expected to be refused but answered ..."

The query ids, request ids and the 'Bytes' unit above are this run's own values; the requirements are the statuses, the error code ValidationException and the two fixed message sentences quoted in the description.

Model evidence (services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json): both StartQueryWorkloadInsightsTopContributors and StartQueryWorkloadInsightsTopContributorsData take required startTime/endTime Timestamps (date-time) and model ValidationException with httpError 400; GetQueryResultsWorkloadInsightsTopContributorsData models ValidationException (400) alongside its required unit/datapoints output.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a NetworkFlowMonitor client at it.
2. CreateScope over this account/Region and wait for GetScope status SUCCEEDED.
3. Seven-day look-back, data family: StartQueryWorkloadInsightsTopContributorsData(scopeId, startTime = now - 7 days - 1 hour, endTime = now - 7 days, metricName=RETRANSMISSIONS, destinationCategory=INTRA_AZ) -> emulator answers 200 with a queryId; AWS answers 400 ValidationException 'StartTime cannot be more than 7 days in the past'. Repeat with startTime = now - 7 days + 1 hour to see the accepted neighbour.
4. Seven-day look-back, non-data family: StartQueryWorkloadInsightsTopContributors with the same out-of-window pair -> same divergence.
5. Zero-width window: StartQueryWorkloadInsightsTopContributorsData(scopeId, startTime = endTime = some instant an hour ago, metricName=DATA_TRANSFERRED, destinationCategory=UNCLASSIFIED); poll GetQueryStatusWorkloadInsightsTopContributorsData until SUCCEEDED; then GetQueryResultsWorkloadInsightsTopContributorsData for that queryId -> emulator answers 200 {'unit': ..., 'datapoints': []}; AWS answers 400 ValidationException 'The parameter StartTime must not equal parameter EndTime. StartTime must be at least 15 minutes before the current time.'
6. Re-replay the three checks listed in trace_ids to confirm.
