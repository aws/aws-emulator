Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.stop-data-query-malformed-query-id-rejected networkflowmonitor.stop-workload-insights-query-malformed-id-rejected
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor.stop-data-query-malformed-query-id-rejected|run#12|AssertionError|assert (code, message) == ("ValidationException", f"String [{{{_MALFORMED_ID}}}] is not a valid UUID"), (
Issue-Key: networkflowmonitor.stop-workload-insights-query-malformed-id-rejected|run#11|AssertionError|assert got == ("ValidationException", f"String [{{{_NOT_A_UUID}}}] is not a valid UUID"), (
Issue-Commit: 73e1e77630833aa0f20a2b43ded051b89907df1b

# networkflowmonitor StopQuery* looks a malformed query id up instead of rejecting its form, so the refusal is the wrong sentence

> _Note: this issue was identified against the code base at commit `73e1e77630833aa0f20a2b43ded051b89907df1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopQueryWorkloadInsightsTopContributorsData

## Description
The StopQuery* operations of the workload-insights query interface check the FORM of the queryId before they look anything up: an id that is not a UUID is refused as a malformed input, quoting the id in AWS's fixed template, whereas a UUID-shaped id is accepted (even for a query that has already finished) and answers an empty 200. The emulator skips the form check and goes straight to a lookup, so a non-UUID id comes back as 'no such query' instead of 'not a valid UUID'. 2 causes follow, one per operation.

Cause 1 of 2: StopQueryWorkloadInsightsTopContributorsData refuses a non-UUID queryId with the wrong message.
  AWS:      Smithy: StopQueryWorkloadInsightsTopContributorsData binds DELETE /workloadInsights/{scopeId}/topContributorsDataQueries/{queryId}; queryId is a required httpLabel targeting plain String with NO @pattern, and ValidationException (httpError 400) is modelled on the operation, so the UUID rule is recorded behavior only. Recorded response for an id that is not a UUID: HTTP 400, code ValidationException, message "String [{<queryId>}] is not a valid UUID". Fixed text: "String [{", "}] is not a valid UUID"; the single variable part is the rejected queryId copied verbatim from the request (note the literal braces AWS wraps it in). The refusal has no effect on the scope's other queries: a query that had completed stays SUCCEEDED and its results stay readable, and stopping THAT query by its real UUID answers 200.
  Emulator: HTTP 400, code ValidationException (status and class right), but message "There's no query with id {<queryId>}" -- an emulator-authored sentence that reports the id as unknown rather than as malformed.
  Why:      the two conditions are different for a caller: a malformed id is a client bug to fix in the code, an unknown id is a legitimate outcome of a stop issued after the query already went away (which AWS answers 200 for anyway). A client or test matching AWS's sentence never matches, and a caller logging the message is told its id does not exist when in fact it was never a well-formed id.
  Fix:      the queryId validation in the StopQueryWorkloadInsightsTopContributorsData handler in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops -- check the UUID form first and emit the quoted-id ValidationException, before any query lookup.
  Traces:   networkflowmonitor.stop-data-query-malformed-query-id-rejected.

Cause 2 of 2: StopQueryWorkloadInsightsTopContributors refuses a non-UUID queryId with the wrong message.
  AWS:      Smithy: StopQueryWorkloadInsightsTopContributors binds DELETE /workloadInsights/{scopeId}/topContributorsQueries/{queryId}, queryId a required httpLabel of plain String (no @pattern), ValidationException (400) modelled. Recorded response for a non-UUID id: HTTP 400, ValidationException, message "String [{<queryId>}] is not a valid UUID" -- same template, same single request-derived value. The query started just before the refused stop still runs to SUCCEEDED, and the same stop with that query's own UUID is accepted.
  Emulator: HTTP 400, ValidationException, message "There's no query with id {<queryId>}".
  Why:      as cause 1, on the non-data top-contributors family.
  Fix:      the queryId validation in the StopQueryWorkloadInsightsTopContributors handler.
  Traces:   networkflowmonitor.stop-workload-insights-query-malformed-id-rejected.

Related open issue 20261127T093619Z-networkflowmonitor-input-validation-refusal-messages-are-re-worded.md reports other re-worded validation sentences in this crate (scopeId and @pattern messages); it does not name the StopQuery* queryId sentence, so neither of these causes is covered by it.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout; service heading networkflowmonitor, 2 failing checks, verdict=mismatch. Both checks had already driven a real scope to SUCCEEDED and a real query to SUCCEEDED, so the scope/owner resolution was NOT the thing diverging here -- only the malformed-id refusal.

1) networkflowmonitor.stop-data-query-malformed-query-id-rejected (StopQueryWorkloadInsightsTopContributorsData, queryId = 'query-7f3c-not-uuid')
   expected: ('ValidationException', 'String [{query-7f3c-not-uuid}] is not a valid UUID')
   actual:   ('ValidationException', "There's no query with id {query-7f3c-not-uuid}")

2) networkflowmonitor.stop-workload-insights-query-malformed-id-rejected (StopQueryWorkloadInsightsTopContributors, queryId = 'not-a-query-id')
   expected: ('ValidationException', 'String [{not-a-query-id}] is not a valid UUID')
   actual:   ('ValidationException', "There's no query with id {not-a-query-id}")

The ids shown are each check's own input; the requirement is the fixed template "String [{" + <the queryId from the request> + "}] is not a valid UUID" with the rejected id echoed verbatim.

Model evidence (services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json): on both Stop operations queryId is {target: String, httpLabel: true, required: true} with no pattern constraint, output is empty with http code 200, and the modelled client errors include ValidationException (400); ResourceNotFoundException is not modelled on either, so 'no such query' is not an outcome these operations have.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a NetworkFlowMonitor client at it.
2. CreateScope over this account/Region, wait for GetScope status SUCCEEDED, and start a workload-insights query so the scope has a real, finished query.
3. StopQueryWorkloadInsightsTopContributorsData(scopeId=<that scope>, queryId='query-7f3c-not-uuid') -> emulator answers 400 ValidationException "There's no query with id {query-7f3c-not-uuid}"; AWS answers 400 ValidationException 'String [{query-7f3c-not-uuid}] is not a valid UUID'.
4. StopQueryWorkloadInsightsTopContributors(scopeId=<that scope>, queryId='not-a-query-id') -> same divergence with that id.
5. Confirm the neighbours AWS allows: the finished query still reads SUCCEEDED after the refusal, its results are still served, and the same stop with the query's own UUID answers HTTP 200.
6. Re-replay the two checks listed in trace_ids.
