Issue-Source: trace-sweep-divergence
Issue-Trace: networkflowmonitor-query-calls-on-absent-owner
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkflowmonitor-query-calls-on-absent-owner networkflowmonitor-monitor-query-status-name-constraints
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|StopQueryMonitorTopContributors|2xx->4xx|$
Issue-Key: networkflowmonitor|StopQueryMonitorTopContributors|2xx->4xx|status
Issue-Key: networkflowmonitor|GetQueryStatusMonitorTopContributors|2xx->4xx|$.status
Issue-Key: networkflowmonitor|GetQueryStatusMonitorTopContributors|2xx->4xx|status
Issue-Key: networkflowmonitor|StopQueryWorkloadInsightsTopContributorsData|2xx->4xx|$
Issue-Key: networkflowmonitor|StopQueryWorkloadInsightsTopContributorsData|2xx->4xx|status
Issue-Key: networkflowmonitor|StopQueryWorkloadInsightsTopContributors|2xx->4xx|$
Issue-Key: networkflowmonitor|StopQueryWorkloadInsightsTopContributors|2xx->4xx|status
Issue-Commit: ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7

# networkflowmonitor stop-query and status-query calls are refused 403 instead of being served

> _Note: this issue was identified against the code base at commit `ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopQueryMonitorTopContributors

## Description
Three of the NetworkFlowMonitor query-interface operations that AWS answers with a successful response are refused outright by the emulator with AccessDeniedException / HTTP 403. The StopQuery* family is idempotent in the model and AWS answers it with an empty 200 body whether or not there is a live query (or even an owner) behind the identifiers; GetQueryStatusMonitorTopContributors answers 200 with a status enum. In every case the emulator short-circuits on its owner-existence gate and never reaches the handler, so the operation has no success path at all.

4 causes follow; each is a separate operation whose success path must be restored.

Cause 1 of 4: StopQueryMonitorTopContributors answers 403 instead of an empty 200.
  AWS:      Smithy: StopQueryMonitorTopContributors binds DELETE /monitors/{monitorName}/topContributorsQueries/{queryId}, is marked @idempotent, its output shape StopQueryMonitorTopContributorsOutput has NO members, and its http code is 200. ResourceNotFoundException is not modelled on it. Recorded response: 200 with body {}.
  Emulator: 403 with body {"__type": "AccessDeniedException", "message": "monitor supplied doesn't exist"}.
  Why:      stopping a query is the documented cleanup step after StartQuery*, and an idempotent stop is what a client issues in a finally/teardown block. A 403 there turns ordinary teardown into a hard failure and makes the caller believe its principal lost access to the monitor.
  Fix:      the StopQueryMonitorTopContributors handler in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops -- the owner gate must not deny; the operation returns the empty 200 output.
  Traces:   networkflowmonitor-query-calls-on-absent-owner#task#3.

Cause 2 of 4: StopQueryWorkloadInsightsTopContributorsData answers 403 instead of an empty 200.
  AWS:      Smithy: StopQueryWorkloadInsightsTopContributorsData binds DELETE /workloadInsights/{scopeId}/topContributorsDataQueries/{queryId} with an empty output shape and http code 200; ResourceNotFoundException is not modelled. Recorded response: 200 with body {}.
  Emulator: 403 with body {"__type": "AccessDeniedException", "message": "scope supplied doesn't exist"}.
  Why:      as cause 1, on the workload-insights data family: teardown of a data query cannot succeed.
  Fix:      the StopQueryWorkloadInsightsTopContributorsData handler.
  Traces:   networkflowmonitor-query-calls-on-absent-owner#task#6.

Cause 3 of 4: StopQueryWorkloadInsightsTopContributors answers 403 instead of an empty 200.
  AWS:      Smithy: StopQueryWorkloadInsightsTopContributors binds DELETE /workloadInsights/{scopeId}/topContributorsQueries/{queryId} with an empty output shape and http code 200. Recorded response: 200 with body {}.
  Emulator: 403 with body {"__type": "AccessDeniedException", "message": "scope supplied doesn't exist"}.
  Why:      as cause 1, on the non-data workload-insights family.
  Fix:      the StopQueryWorkloadInsightsTopContributors handler.
  Traces:   networkflowmonitor-query-calls-on-absent-owner#task#7.

Cause 4 of 4: GetQueryStatusMonitorTopContributors answers 403 instead of 200 with a status.
  AWS:      Smithy: GetQueryStatusMonitorTopContributors binds GET /monitors/{monitorName}/topContributorsQueries/{queryId}/status, http code 200, and its output has one required member `status` targeting the QueryStatus enum; ResourceNotFoundException is not modelled on it. Recorded response: 200 with $.status = 'SUCCEEDED' -- recorded both for a monitor that exists and for one that does not, so the status read does not gate on the owner.
  Emulator: 403, no `status` member in the body at all (the AccessDeniedException envelope is returned instead).
  Why:      the status poll is the only way a client learns a query has finished before calling GetQueryResults*. With it refused, the whole start -> poll -> read workflow is unusable: a client cannot reach a terminal status and will either spin or abort.
  Fix:      the GetQueryStatusMonitorTopContributors handler -- it must serve the modelled 200 with the query's status instead of denying on owner resolution.
  Traces:   networkflowmonitor-monitor-query-status-name-constraints#task#3, networkflowmonitor-query-calls-on-absent-owner#task#2.

No open issue covers networkflowmonitor. A companion issue filed in the same pass covers the query interface's absent-owner REFUSALS (wrong class/wording on StartQuery*/GetQueryResults*/GetQueryStatusWorkloadInsights*); this one covers the paths where AWS does not refuse at all.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout; 4 divergence clusters, all verdict=mismatch, service=networkflowmonitor.

1) networkflowmonitor-query-calls-on-absent-owner#task#3 StopQueryMonitorTopContributors
   expected: status 200, body {}
   actual:   status 403, body {'__type': 'AccessDeniedException', 'message': "monitor supplied doesn't exist"}

2) networkflowmonitor-query-calls-on-absent-owner#task#6 StopQueryWorkloadInsightsTopContributorsData
   expected: status 200, body {}
   actual:   status 403, body {'__type': 'AccessDeniedException', 'message': "scope supplied doesn't exist"}

3) networkflowmonitor-query-calls-on-absent-owner#task#7 StopQueryWorkloadInsightsTopContributors
   expected: status 200, body {}
   actual:   status 403, body {'__type': 'AccessDeniedException', 'message': "scope supplied doesn't exist"}

4) networkflowmonitor-monitor-query-status-name-constraints#task#3 (also networkflowmonitor-query-calls-on-absent-owner#task#2) GetQueryStatusMonitorTopContributors
   expected: status 200, $.status = 'SUCCEEDED'
   actual:   status 403, $.status missing (None)

Model evidence (services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json): StopQueryMonitorTopContributors is @idempotent with an empty output shape and http {method: DELETE, code: 200}; StopQueryWorkloadInsightsTopContributors and StopQueryWorkloadInsightsTopContributorsData likewise have empty outputs and code 200; GetQueryStatusMonitorTopContributors has output {status: QueryStatus (required, enum)} and code 200. None of the four models ResourceNotFoundException, and AccessDeniedException is modelled only as the common authorization error, not as the owner-absent outcome.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a NetworkFlowMonitor client at it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) with replay_wire_corpus(); the four clusters listed in the evidence all report verdict=mismatch.
3. Or by hand:
   a. DELETE /monitors/<any-monitor-name>/topContributorsQueries/<any-query-id> -> emulator answers 403 AccessDeniedException; AWS answers 200 with an empty JSON body.
   b. DELETE /workloadInsights/<any-scope-id>/topContributorsDataQueries/<any-query-id> -> emulator answers 403; AWS answers 200 {}.
   c. DELETE /workloadInsights/<any-scope-id>/topContributorsQueries/<any-query-id> -> emulator answers 403; AWS answers 200 {}.
   d. GET /monitors/<monitor-name>/topContributorsQueries/<query-id>/status, both for a monitor created via CreateMonitor and for one that was never created -> emulator answers 403 with no `status` member; AWS answers 200 with $.status = 'SUCCEEDED'.
4. Compare status and body against the expectations in the evidence.
