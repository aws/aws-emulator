Issue-Source: trace-sweep-divergence
Issue-Trace: networkflowmonitor-monitor-query-limit-bounds
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkflowmonitor-monitor-query-limit-bounds networkflowmonitor-query-calls-on-absent-owner networkflowmonitor-workload-insights-query-unknown-scope-rejected networkflowmonitor-start-query-workload-insights-top-contributors-data-validation networkflowmonitor-start-query-workload-insights-top-contributors-validation networkflowmonitor-workload-insights-results-unknown-scope
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|StartQueryMonitorTopContributors|4xx->4xx|$.message
Issue-Key: networkflowmonitor|StartQueryMonitorTopContributors|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|StartQueryMonitorTopContributors|4xx->4xx|status
Issue-Key: networkflowmonitor|GetQueryResultsMonitorTopContributors|4xx->4xx|$.message
Issue-Key: networkflowmonitor|GetQueryResultsMonitorTopContributors|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|GetQueryResultsMonitorTopContributors|4xx->4xx|status
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributorsData|4xx->4xx|$.message
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributorsData|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributorsData|4xx->4xx|status
Issue-Key: networkflowmonitor|GetQueryStatusWorkloadInsightsTopContributorsData|4xx->4xx|$.message
Issue-Key: networkflowmonitor|GetQueryStatusWorkloadInsightsTopContributorsData|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|GetQueryStatusWorkloadInsightsTopContributorsData|4xx->4xx|status
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributorsData|4xx->4xx|$.message
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributorsData|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributorsData|4xx->4xx|status
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributors|4xx->4xx|$.message
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributors|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|StartQueryWorkloadInsightsTopContributors|4xx->4xx|status
Issue-Key: networkflowmonitor|GetQueryStatusWorkloadInsightsTopContributors|4xx->4xx|$.message
Issue-Key: networkflowmonitor|GetQueryStatusWorkloadInsightsTopContributors|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|GetQueryStatusWorkloadInsightsTopContributors|4xx->4xx|status
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributors|4xx->4xx|$.message
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributors|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributors|4xx->4xx|status
Issue-Commit: ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7

# networkflowmonitor query-interface refusals for an absent monitor or scope use the wrong error class and wording

> _Note: this issue was identified against the code base at commit `ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryMonitorTopContributors

## Description
The NetworkFlowMonitor query interface (the StartQuery* / GetQueryStatus* / GetQueryResults* families) has to resolve the monitor or the scope that owns the query before it can do anything else. When that owner does not exist, AWS refuses the call as a client input problem: ValidationException, HTTP 400, with one of two FIXED sentences and nothing interpolated into them -- "Specified monitor does not exist." for the monitor-owned operations and "Specified scope does not exist." for the scope-owned ones. Those sentences carry NO request- or state-derived values: no monitor name, no scope id, no ARN.

The emulator instead runs the absent owner through a gate that raises either AccessDeniedException (HTTP 403, x-amzn-errortype: AccessDeniedException) with "monitor supplied doesn't exist" / "scope supplied doesn't exist", or ResourceNotFoundException (HTTP 404, x-amzn-errortype: ResourceNotFoundException) with an emulator-authored sentence that interpolates the resolved ARN -- "Monitor [<monitor-arn>] doesn't exist" / "There's no scope with ARN: [<scope-arn>]". The ARN in those texts is a value taken from the request and the emulator's state; it is not part of any AWS template, because AWS's template has no slot for it.

8 causes follow. All eight are the same gate seen through a different operation, and each needs its own operation's refusal corrected.

Cause 1 of 8: StartQueryMonitorTopContributors on an absent monitor.
  AWS:      Smithy: StartQueryMonitorTopContributors binds POST /monitors/{monitorName}/topContributorsQueries and models ValidationException (httpError 400) among its errors; ResourceNotFoundException is NOT modelled on it at all. Recorded response: 400 with message "Specified monitor does not exist." (fixed text in full; no variable part).
  Emulator: 403, x-amzn-errortype: AccessDeniedException, message "monitor supplied doesn't exist".
  Why:      a client that branches on the error class sees an authorization failure instead of a bad-input failure. SDK retry/credential-refresh paths treat 403 AccessDenied as a principal problem, so a caller that merely typed a monitor name that is not there is told its credentials are at fault, and the error shape it is handed (AccessDeniedException) is not one the operation's model lists for this condition.
  Fix:      the owner-resolution step of the StartQueryMonitorTopContributors handler in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops -- it must emit the modelled ValidationException with the fixed sentence rather than an access-denied refusal.
  Traces:   networkflowmonitor-monitor-query-limit-bounds#task#3.

Cause 2 of 8: GetQueryResultsMonitorTopContributors on an absent monitor.
  AWS:      Smithy: GetQueryResultsMonitorTopContributors models both ResourceNotFoundException (404) and ValidationException (400); the recording shows AWS picks ValidationException for an absent monitor. Recorded response: 400, message "Specified monitor does not exist." (entirely fixed text).
  Emulator: 404, x-amzn-errortype: ResourceNotFoundException, message "Monitor [arn:aws:networkflowmonitor:<region>:<account>:monitor/<monitor-name>] doesn't exist" -- the ARN is interpolated from the request's monitorName plus the emulator's region/account.
  Why:      a client that distinguishes "this identifier is not acceptable input" (400) from "the addressed resource is gone" (404) takes the wrong branch, and any client matching on the message text never matches, because AWS's text has no ARN in it.
  Fix:      the owner-resolution step of the GetQueryResultsMonitorTopContributors handler.
  Traces:   networkflowmonitor-query-calls-on-absent-owner#task#1.

Cause 3 of 8: GetQueryResultsWorkloadInsightsTopContributorsData on an absent scope.
  AWS:      Smithy: GetQueryResultsWorkloadInsightsTopContributorsData binds GET /workloadInsights/{scopeId}/topContributorsDataQueries/{queryId}/results and models ValidationException (400). Recorded response: 400, message "Specified scope does not exist." (entirely fixed text).
  Emulator: 404, x-amzn-errortype: ResourceNotFoundException, message "There's no scope with ARN: [arn:aws:networkflowmonitor:<region>:<account>:scope/<scope-id>]" -- the scope ARN is built from the request's scopeId and the emulator's region/account.
  Why:      same misclassification as cause 2, on the workload-insights data family; a 404 also invites a client to treat the call as retriable against a different endpoint rather than as a rejected input.
  Fix:      the owner-resolution step of the GetQueryResultsWorkloadInsightsTopContributorsData handler.
  Traces:   networkflowmonitor-query-calls-on-absent-owner#task#4.

Cause 4 of 8: GetQueryStatusWorkloadInsightsTopContributorsData on an absent scope.
  AWS:      Smithy: GetQueryStatusWorkloadInsightsTopContributorsData models ValidationException (400), AccessDeniedException (403), ServiceQuotaExceededException (402), ThrottlingException (429), InternalServerException (500) -- and NOT ResourceNotFoundException. Recorded response: 400, message "Specified scope does not exist." (entirely fixed text).
  Emulator: 403, x-amzn-errortype: AccessDeniedException, message "scope supplied doesn't exist".
  Why:      the caller is told it lacks permission on a scope it simply never created, so a client with correct credentials retries or re-authenticates forever instead of correcting the scope id.
  Fix:      the owner-resolution step of the GetQueryStatusWorkloadInsightsTopContributorsData handler.
  Traces:   networkflowmonitor-query-calls-on-absent-owner#task#5, networkflowmonitor-workload-insights-query-unknown-scope-rejected#task#1.

Cause 5 of 8: StartQueryWorkloadInsightsTopContributorsData on an absent scope.
  AWS:      Smithy: StartQueryWorkloadInsightsTopContributorsData binds POST /workloadInsights/{scopeId}/topContributorsDataQueries and models ValidationException (400), not ResourceNotFoundException. Recorded response: 400, message "Specified scope does not exist." (entirely fixed text).
  Emulator: 403, x-amzn-errortype: AccessDeniedException, message "scope supplied doesn't exist".
  Why:      a query that can never be started is reported as a permissions problem, so the failure is unactionable for the caller.
  Fix:      the owner-resolution step of the StartQueryWorkloadInsightsTopContributorsData handler.
  Traces:   networkflowmonitor-start-query-workload-insights-top-contributors-data-validation#task#1.

Cause 6 of 8: StartQueryWorkloadInsightsTopContributors on an absent scope.
  AWS:      Smithy: StartQueryWorkloadInsightsTopContributors binds POST /workloadInsights/{scopeId}/topContributorsQueries and models ValidationException (400). Recorded response: 400, message "Specified scope does not exist." (entirely fixed text).
  Emulator: 403, x-amzn-errortype: AccessDeniedException, message "scope supplied doesn't exist".
  Why:      as cause 5, on the non-data workload-insights start path.
  Fix:      the owner-resolution step of the StartQueryWorkloadInsightsTopContributors handler.
  Traces:   networkflowmonitor-start-query-workload-insights-top-contributors-validation#task#1.

Cause 7 of 8: GetQueryStatusWorkloadInsightsTopContributors on an absent scope.
  AWS:      Smithy: GetQueryStatusWorkloadInsightsTopContributors models ValidationException (400) and does not model ResourceNotFoundException. Recorded response: 400, message "Specified scope does not exist." (entirely fixed text).
  Emulator: 403, x-amzn-errortype: AccessDeniedException, message "scope supplied doesn't exist".
  Why:      a polling client cannot tell a rejected scope id from a denied principal, so it cannot decide whether to stop polling.
  Fix:      the owner-resolution step of the GetQueryStatusWorkloadInsightsTopContributors handler.
  Traces:   networkflowmonitor-workload-insights-query-unknown-scope-rejected#task#2.

Cause 8 of 8: GetQueryResultsWorkloadInsightsTopContributors on a well-formed but absent scope.
  AWS:      Smithy: GetQueryResultsWorkloadInsightsTopContributors models ValidationException (400) and ResourceNotFoundException (404); the recording shows AWS answers an absent scope with ValidationException. Recorded response: 400, message "Specified scope does not exist." (entirely fixed text).
  Emulator: 404, x-amzn-errortype: ResourceNotFoundException, message "There's no scope with ARN: [arn:aws:networkflowmonitor:<region>:<account>:scope/<scope-id>]" -- scope ARN interpolated from the request.
  Why:      as cause 3, on the non-data results path; a client keying on 404 for "create the scope first" and on 400 for "fix the request" is routed to the wrong remedy.
  Fix:      the owner-resolution step of the GetQueryResultsWorkloadInsightsTopContributors handler.
  Traces:   networkflowmonitor-workload-insights-results-unknown-scope#task#2.

Note: the sibling control-plane operations are already right -- GetMonitor/DeleteMonitor answer an absent monitor with ResourceNotFoundException 404 and CreateMonitor answers an unknown scope with AccessDeniedException 403, and the recordings confirm those. The defect is confined to the query interface, which AWS refuses as ValidationException 400 instead. No open issue covers networkflowmonitor.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout; 8 divergence clusters, all verdict=mismatch, service=networkflowmonitor.

1) networkflowmonitor-monitor-query-limit-bounds#task#3 StartQueryMonitorTopContributors
   expected: status 400, $.message = 'Specified monitor does not exist.'
   actual:   status 403, $.message = "monitor supplied doesn't exist", $headers.x-amzn-errortype = 'AccessDeniedException'

2) networkflowmonitor-query-calls-on-absent-owner#task#1 GetQueryResultsMonitorTopContributors
   expected: status 400, $.message = 'Specified monitor does not exist.'
   actual:   status 404, $.message = "Monitor [arn:aws:networkflowmonitor:us-west-2:000000000000:monitor/sim2real-probe-nfm-absent-17727870] doesn't exist", $headers.x-amzn-errortype = 'ResourceNotFoundException'

3) networkflowmonitor-query-calls-on-absent-owner#task#4 GetQueryResultsWorkloadInsightsTopContributorsData
   expected: status 400, $.message = 'Specified scope does not exist.'
   actual:   status 404, $.message = "There's no scope with ARN: [arn:aws:networkflowmonitor:us-west-2:000000000000:scope/00000000-0000-4000-8000-000000000000]", $headers.x-amzn-errortype = 'ResourceNotFoundException'

4) networkflowmonitor-query-calls-on-absent-owner#task#5 (also networkflowmonitor-workload-insights-query-unknown-scope-rejected#task#1) GetQueryStatusWorkloadInsightsTopContributorsData
   expected: status 400, $.message = 'Specified scope does not exist.'
   actual:   status 403, $.message = "scope supplied doesn't exist", $headers.x-amzn-errortype = 'AccessDeniedException'

5) networkflowmonitor-start-query-workload-insights-top-contributors-data-validation#task#1 StartQueryWorkloadInsightsTopContributorsData
   expected: status 400, $.message = 'Specified scope does not exist.'
   actual:   status 403, $.message = "scope supplied doesn't exist", $headers.x-amzn-errortype = 'AccessDeniedException'

6) networkflowmonitor-start-query-workload-insights-top-contributors-validation#task#1 StartQueryWorkloadInsightsTopContributors
   expected: status 400, $.message = 'Specified scope does not exist.'
   actual:   status 403, $.message = "scope supplied doesn't exist", $headers.x-amzn-errortype = 'AccessDeniedException'

7) networkflowmonitor-workload-insights-query-unknown-scope-rejected#task#2 GetQueryStatusWorkloadInsightsTopContributors
   expected: status 400, $.message = 'Specified scope does not exist.'
   actual:   status 403, $.message = "scope supplied doesn't exist", $headers.x-amzn-errortype = 'AccessDeniedException'

8) networkflowmonitor-workload-insights-results-unknown-scope#task#2 GetQueryResultsWorkloadInsightsTopContributors
   expected: status 400, $.message = 'Specified scope does not exist.'
   actual:   status 404, $.message = "There's no scope with ARN: [arn:aws:networkflowmonitor:us-west-2:000000000000:scope/8beb51c8-1d2e-4fdf-8c57-75c5b3adf590]", $headers.x-amzn-errortype = 'ResourceNotFoundException'

The ARNs and scope ids above are values from each recording's own request/state and are NOT part of the required message; AWS's required messages are the two fixed sentences with no interpolation.

Model evidence (services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json): every GetQueryStatus*/StartQuery* operation in the query interface models ValidationException (httpError 400) and does NOT model ResourceNotFoundException; the GetQueryResults* operations model both, and the recordings show AWS selects ValidationException for an absent owner.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a NetworkFlowMonitor client at it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) with replay_wire_corpus(); the eight clusters listed in the evidence all report verdict=mismatch.
3. Or by hand, against an account with NO monitor named <absent-monitor> and NO scope <absent-scope-id>:
   a. POST /monitors/<absent-monitor>/topContributorsQueries with a valid body -> emulator answers 403 AccessDeniedException "monitor supplied doesn't exist"; AWS answers 400 ValidationException "Specified monitor does not exist."
   b. GET /monitors/<absent-monitor>/topContributorsQueries/<any-query-id>/results -> emulator answers 404 ResourceNotFoundException with the monitor ARN interpolated; AWS answers 400 ValidationException "Specified monitor does not exist."
   c. POST /workloadInsights/<absent-scope-id>/topContributorsQueries and .../topContributorsDataQueries -> emulator answers 403 AccessDeniedException "scope supplied doesn't exist"; AWS answers 400 ValidationException "Specified scope does not exist."
   d. GET /workloadInsights/<absent-scope-id>/topContributorsQueries/<id>/status and .../topContributorsDataQueries/<id>/status -> emulator answers 403 AccessDeniedException; AWS answers 400 ValidationException "Specified scope does not exist."
   e. GET /workloadInsights/<absent-scope-id>/topContributorsQueries/<id>/results and .../topContributorsDataQueries/<id>/results -> emulator answers 404 ResourceNotFoundException with the scope ARN interpolated; AWS answers 400 ValidationException "Specified scope does not exist."
4. Compare status, x-amzn-errortype and $.message against the expectations in the evidence.
