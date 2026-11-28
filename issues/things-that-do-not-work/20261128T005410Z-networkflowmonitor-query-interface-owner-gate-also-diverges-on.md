Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.query-status-naming-missing-scope-rejected networkflowmonitor.monitor-query-results-after-monitor-delete-rejected networkflowmonitor.stop-query-workload-insights-top-contributors-malformed-query-id-rejected networkflowmonitor.stop-query-workload-insights-top-contributors-data-malformed-query-id-rejected networkflowmonitor.stop-query-monitor-top-contributors-non-uuid-query-id-rejected
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor.query-status-naming-missing-scope-rejected|run#12|AssertionError|assert refused == ("ValidationException", "Specified scope does not exist."), (
Issue-Key: networkflowmonitor.monitor-query-results-after-monitor-delete-rejected|run#29|AssertionError|assert got == gone, f"results of the deleted monitor's query answered {got!r}"
Issue-Key: networkflowmonitor|StopQueryWorkloadInsightsTopContributors|AccessDeniedException
Issue-Key: networkflowmonitor|StopQueryWorkloadInsightsTopContributorsData|AccessDeniedException
Issue-Key: networkflowmonitor|StopQueryMonitorTopContributors|AccessDeniedException
Issue-Commit: 73e1e77630833aa0f20a2b43ded051b89907df1b

# networkflowmonitor query-interface owner gate also diverges on the behavioral suite: absent/deleted scope and monitor refused as AccessDenied/NotFound, and StopQuery* denied instead of served

> _Note: this issue was identified against the code base at commit `73e1e77630833aa0f20a2b43ded051b89907df1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryStatusWorkloadInsightsTopContributors

## Description
The owner-resolution gate the emulator runs in front of the query interface diverges from AWS in two ways that the behavioral suite reaches on five separate checks: a scope or monitor that is not there is refused as an authorization or not-found failure instead of AWS's ValidationException, and the StopQuery* family -- which AWS serves with an empty 200 regardless of the owner -- is denied outright so it has no success path. 3 causes follow.

NOTE on existing issues: open issues 20261127T093619Z-networkflowmonitor-query-interface-refusals-for-an-absent-monitor.md and 20261127T093619Z-networkflowmonitor-stop-query-and-status-query-calls-are.md describe the same gate from the WIRE corpus and do not carry these behavioral check ids, so these checks are not covered by them; a fixer closing those should re-replay the ids in trace_ids here as well.

Cause 1 of 3: GetQueryStatusWorkloadInsightsTopContributors answers a scope id with no scope as AccessDeniedException instead of ValidationException.
  AWS:      Smithy: GetQueryStatusWorkloadInsightsTopContributors binds GET /workloadInsights/{scopeId}/topContributorsQueries/{queryId}/status and models ValidationException (httpError 400), AccessDeniedException (403), ServiceQuotaExceededException (402), ThrottlingException (429), InternalServerException (500) -- ResourceNotFoundException is NOT modelled. Recorded behavior: reading a query's status under a well-formed scope id that no scope has is refused with HTTP 400, code ValidationException, message "Specified scope does not exist." -- the whole sentence is fixed text, interpolating no scope id, name or ARN. The refusal is about the scope only: the same query id read under its own scope still answers 200 with status SUCCEEDED.
  Emulator: HTTP 403, code AccessDeniedException, message "scope supplied doesn't exist".
  Why:      a 403 AccessDenied tells an SDK and its caller that the principal is at fault; credential-refresh and retry paths engage, and a caller that merely passed a scope id it never created re-authenticates forever instead of correcting the id. The error class is also not one the operation models for this condition.
  Fix:      the owner-resolution step of the GetQueryStatusWorkloadInsightsTopContributors handler in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops -- emit the modelled ValidationException with the fixed sentence.
  Traces:   networkflowmonitor.query-status-naming-missing-scope-rejected.

Cause 2 of 3: GetQueryResultsMonitorTopContributors answers a DELETED monitor as ResourceNotFoundException with an interpolated ARN instead of ValidationException.
  AWS:      Smithy: GetQueryResultsMonitorTopContributors binds GET /monitors/{monitorName}/topContributorsQueries/{queryId}/results and models both ResourceNotFoundException (404) and ValidationException (400); recorded behavior is that AWS picks ValidationException for a monitor that is not there -- HTTP 400, message "Specified monitor does not exist.", entirely fixed text with no monitor name and no ARN in it. The same holds for a new StartQueryMonitorTopContributors on the deleted monitor. Before the delete, the identical results read is served (200 with a topContributors list), so the divergence is confined to the owner-absent branch.
  Emulator: HTTP 404, code ResourceNotFoundException, message "Monitor [<monitor-arn>] doesn't exist", where <monitor-arn> is built from the request's monitorName plus the emulator's region/account (the replay shows 'arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-nfm-gone-2a3bb88539b7'; that ARN is this run's value and is not part of any AWS template).
  Why:      a client that branches 404 ('the resource is gone, recreate it') vs 400 ('this input is not acceptable') takes the wrong branch, and anything matching AWS's sentence never matches because AWS's sentence has no ARN slot.
  Fix:      the owner-resolution step of the GetQueryResultsMonitorTopContributors handler (and the StartQueryMonitorTopContributors path the same check reads next, which the failure aborted before).
  Traces:   networkflowmonitor.monitor-query-results-after-monitor-delete-rejected.

Cause 3 of 3: StopQueryWorkloadInsightsTopContributors, StopQueryWorkloadInsightsTopContributorsData and StopQueryMonitorTopContributors deny on owner resolution instead of answering an empty 200.
  AWS:      Smithy: all three Stop operations bind DELETE on their query path with queryId as a required httpLabel, have EMPTY output shapes with http code 200, and do not model ResourceNotFoundException; AccessDeniedException is modelled only as the common authorization error. Recorded behavior: a stop naming a scope id or monitor name that does not exist, with a UUID-shaped query id, answers HTTP 200 with an empty body -- the operations are idempotent teardown calls and do not gate on the owner; only the FORM of the query id is checked (a non-UUID id is refused ValidationException 'String [{<queryId>}] is not a valid UUID', the rejected id copied from the request).
  Emulator: HTTP 403, code AccessDeniedException, message "scope supplied doesn't exist" (workload-insights paths) or "monitor supplied doesn't exist" (monitor path) -- raised before the handler, so the operation has no 200 path at all and the query-id form is never examined.
  Why:      StopQuery* is what a client issues in a finally/teardown block after StartQuery*. A 403 there turns ordinary cleanup into a hard failure and tells the caller its principal lost access to a scope/monitor, and because the deny precedes the form check, the malformed-id refusal AWS gives is unreachable too.
  Fix:      the owner gate in front of the StopQueryWorkloadInsightsTopContributors, StopQueryWorkloadInsightsTopContributorsData and StopQueryMonitorTopContributors handlers -- the owner must not be resolved for a deny; validate the query id's form and return the empty 200 output.
  Traces:   networkflowmonitor.stop-query-workload-insights-top-contributors-malformed-query-id-rejected, networkflowmonitor.stop-query-workload-insights-top-contributors-data-malformed-query-id-rejected, networkflowmonitor.stop-query-monitor-top-contributors-non-uuid-query-id-rejected.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout; service heading networkflowmonitor; 5 failing checks (2 mismatch, 3 error).

1) networkflowmonitor.query-status-naming-missing-scope-rejected (GetQueryStatusWorkloadInsightsTopContributors)
   A real scope and a real query were established first (both passed); then the same query id was read under a fresh random UUID scope id.
   expected: ('ValidationException', 'Specified scope does not exist.')
   actual:   ('AccessDeniedException', "scope supplied doesn't exist")  [HTTP 403]

2) networkflowmonitor.monitor-query-results-after-monitor-delete-rejected (GetQueryResultsMonitorTopContributors)
   The pre-delete read was served correctly; after DeleteMonitor and after GetMonitor reported the monitor gone:
   expected: ('ValidationException', 'Specified monitor does not exist.')
   actual:   ('ResourceNotFoundException', "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-nfm-gone-2a3bb88539b7] doesn't exist")  [HTTP 404]
   (the ARN is this run's monitor; AWS's required message contains no ARN)

3) networkflowmonitor.stop-query-workload-insights-top-contributors-malformed-query-id-rejected (StopQueryWorkloadInsightsTopContributors)
   First call, scopeId = a random UUID, queryId = a random UUID:
   expected: HTTP 200 (empty body)
   actual:   botocore.errorfactory.AccessDeniedException: An error occurred (AccessDeniedException) when calling the StopQueryWorkloadInsightsTopContributors operation: scope supplied doesn't exist

4) networkflowmonitor.stop-query-workload-insights-top-contributors-data-malformed-query-id-rejected (StopQueryWorkloadInsightsTopContributorsData)
   expected: HTTP 200 (empty body)
   actual:   AccessDeniedException: ... when calling the StopQueryWorkloadInsightsTopContributorsData operation: scope supplied doesn't exist

5) networkflowmonitor.stop-query-monitor-top-contributors-non-uuid-query-id-rejected (StopQueryMonitorTopContributors)
   expected: HTTP 200 (empty body) for a UUID query id on a monitor name that was never created
   actual:   AccessDeniedException: ... when calling the StopQueryMonitorTopContributors operation: monitor supplied doesn't exist
   Because the deny comes first, the three malformed-query-id assertions in checks 3-5 ('String [{<id>}] is not a valid UUID') were never exercised.

Model evidence (services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json): GetQueryStatusWorkloadInsightsTopContributors models ValidationException (400) and not ResourceNotFoundException; GetQueryResultsMonitorTopContributors models both, and the recordings show AWS answers an absent monitor with ValidationException 400; all three StopQuery* operations have empty output shapes with http code 200, queryId as a required httpLabel of plain String, and no ResourceNotFoundException.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a NetworkFlowMonitor client at it.
2. Absent scope on a status read: CreateScope, wait for SUCCEEDED, StartQueryWorkloadInsightsTopContributors and wait for SUCCEEDED; then GetQueryStatusWorkloadInsightsTopContributors(scopeId=<a random UUID no scope has>, queryId=<that query>) -> emulator answers 403 AccessDeniedException "scope supplied doesn't exist"; AWS answers 400 ValidationException 'Specified scope does not exist.' Confirm the same query id still reads SUCCEEDED under its own scope.
3. Deleted monitor on a results read: CreateMonitor on a SUCCEEDED scope, StartQueryMonitorTopContributors, wait for SUCCEEDED, read GetQueryResultsMonitorTopContributors (served). DeleteMonitor, wait until GetMonitor answers ResourceNotFoundException, then repeat the results read -> emulator answers 404 ResourceNotFoundException with the monitor ARN interpolated; AWS answers 400 ValidationException 'Specified monitor does not exist.' A new StartQueryMonitorTopContributors on the deleted monitor must answer the same refusal.
4. StopQuery* on an owner that does not exist: DELETE /workloadInsights/<random-uuid>/topContributorsQueries/<random-uuid>, DELETE /workloadInsights/<random-uuid>/topContributorsDataQueries/<random-uuid>, and DELETE /monitors/<never-created-name>/topContributorsQueries/<random-uuid> -> emulator answers 403 AccessDeniedException each time; AWS answers 200 with an empty body.
5. With that fixed, re-check the form rule on the same calls: a non-UUID query id must answer 400 ValidationException 'String [{<that id>}] is not a valid UUID'.
6. Re-replay the five checks listed in trace_ids.
