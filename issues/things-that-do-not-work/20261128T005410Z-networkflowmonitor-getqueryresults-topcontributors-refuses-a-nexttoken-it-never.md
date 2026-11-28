Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: networkflowmonitor.top-contributors-results-unissued-next-token-answers-empty-page
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|GetQueryResultsWorkloadInsightsTopContributors|ValidationException
Issue-Commit: 73e1e77630833aa0f20a2b43ded051b89907df1b

# networkflowmonitor GetQueryResults*TopContributors refuses a nextToken it never issued instead of answering an empty last page

> _Note: this issue was identified against the code base at commit `73e1e77630833aa0f20a2b43ded051b89907df1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResultsWorkloadInsightsTopContributors

## Description
1 cause.

Cause 1 of 1: GetQueryResultsWorkloadInsightsTopContributors rejects a continuation token that these query results never issued, where AWS serves the same empty last page.
  AWS:      Smithy: GetQueryResultsWorkloadInsightsTopContributors binds GET /workloadInsights/{scopeId}/topContributorsQueries/{queryId}/results, is @paginated with inputToken/outputToken nextToken, pageSize maxResults and items topContributors; nextToken is an unconstrained optional httpQuery String (no @pattern, no length), so the model states no token validity rule. Recorded behavior: for a scope whose query completed with no agent data, the results read answers HTTP 200 with topContributors == [] and NO nextToken member -- and the same read with an arbitrary nextToken the service never issued answers that same empty page (HTTP 200, topContributors == [], no nextToken carried onward). The token is not validated against the page sequence; an unknown token resolves to the end of the result set.
  Emulator: the read with an unissued token is refused -- ValidationException with message "nextToken: no page of these query results issued a resume token" (a sentence the emulator authored; AWS has no such message on this path).
  Why:      pagination loops and resumed/replayed workflows hand back tokens that may be stale or from a different run. On AWS that is harmless -- the caller gets an empty terminal page and stops. Against the emulator the same loop dies with a 400, so a client that persists a token across restarts, or retries a page, turns a completed query into a hard failure instead of reading its final empty page. It also makes the operation's paginated contract untestable: a caller cannot probe the tail of the token sequence.
  Fix:      the nextToken handling of the GetQueryResultsWorkloadInsightsTopContributors handler in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops (and the matching GetQueryResultsMonitorTopContributors path the same check reads next, which was never reached because the refusal aborted the check) -- an unrecognised token must resolve to the empty final page, not raise ValidationException.
  Traces:   networkflowmonitor.top-contributors-results-unissued-next-token-answers-empty-page.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout; service heading networkflowmonitor, check networkflowmonitor.top-contributors-results-unissued-next-token-answers-empty-page, verdict=error.

The check had already got through the parts that match AWS: CreateScope -> SUCCEEDED, CreateMonitor -> ACTIVE, StartQueryWorkloadInsightsTopContributors and StartQueryMonitorTopContributors -> SUCCEEDED, and the first (token-less) GetQueryResultsWorkloadInsightsTopContributors page came back as the expected empty page with no nextToken. The divergence is the next call only:

  GetQueryResultsWorkloadInsightsTopContributors(scopeId, queryId, nextToken='sim2real-page-token-never-issued')
  expected: HTTP 200, topContributors == [], no nextToken in the body
  actual:   botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetQueryResultsWorkloadInsightsTopContributors operation: nextToken: no page of these query results issued a resume token

The token string above is the check's own literal; the requirement is that ANY token the results never issued is answered with the empty final page rather than refused. Because the call raised, the check's two remaining assertions on GetQueryResultsMonitorTopContributors (empty page, then the same unissued token) were never exercised.

Model evidence (services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json): GetQueryResultsWorkloadInsightsTopContributors input nextToken is {target: String, httpQuery: 'nextToken'} with no constraints; the operation is paginated {inputToken: nextToken, outputToken: nextToken, pageSize: maxResults, items: topContributors} and its http code is 200.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a NetworkFlowMonitor client at it.
2. CreateScope over this account/Region and wait for GetScope status SUCCEEDED; CreateMonitor on that scope and wait for monitorStatus ACTIVE.
3. StartQueryWorkloadInsightsTopContributors(scopeId, metricName=DATA_TRANSFERRED, destinationCategory=UNCLASSIFIED, a 50-minute window) and poll GetQueryStatusWorkloadInsightsTopContributors until SUCCEEDED.
4. GetQueryResultsWorkloadInsightsTopContributors(scopeId, queryId) -> empty page, no nextToken (matches AWS).
5. Repeat that call with nextToken set to any string the service never issued, e.g. 'sim2real-page-token-never-issued' -> emulator raises 400 ValidationException 'nextToken: no page of these query results issued a resume token'; AWS answers 200 with topContributors == [] and no nextToken.
6. Do the same against GetQueryResultsMonitorTopContributors(monitorName, queryId, nextToken=<unissued>) -- AWS likewise answers the empty page.
7. Re-replay the check listed in trace_ids.
