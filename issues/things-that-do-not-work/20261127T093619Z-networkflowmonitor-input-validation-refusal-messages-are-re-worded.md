Issue-Source: trace-sweep-divergence
Issue-Trace: networkflowmonitor-workload-insights-results-unknown-scope
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkflowmonitor-workload-insights-results-unknown-scope networkflowmonitor-monitor-query-status-name-constraints
Issue-Shape-Id: com.amazonaws.networkflowmonitor#NetworkFlowMonitor
Issue-Key: networkflowmonitor|GetQueryStatusMonitorTopContributors|4xx->4xx|$.message
Issue-Related: 20261127T093619Z-networkflowmonitor-query-interface-refusals-for-an-absent-monitor.md
Issue-Commit: ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7

# networkflowmonitor input-validation refusal messages are re-worded by the emulator

> _Note: this issue was identified against the code base at commit `ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetQueryResultsWorkloadInsightsTopContributors

## Description
Two NetworkFlowMonitor input-validation refusals come back with the right HTTP status and error class but a message the emulator authored itself rather than the sentence AWS returns. The error message is part of the contract, so each is a divergence in its own right.

3 causes follow.

Cause 1 of 3: GetQueryResultsWorkloadInsightsTopContributors rejects a malformed scopeId with the wrong sentence.
  AWS:      recorded behavior only -- the Smithy model constrains the scopeId httpLabel to the ScopeId shape but states no message text. Recorded response: 400 with message "ScopeId provided isn't valid." The whole sentence is fixed text; it names no value from the request (the offending scope id is NOT echoed), and the subject it names is the scopeId input MEMBER, not an ARN.
  Emulator: 400 with message 'The provided Scope ARN is not valid'.
  Why:      the emulator's sentence describes a different input ("Scope ARN") from the one the caller actually supplied on this path (a scopeId path label), so a caller reading the message looks for a malformed ARN it never sent. Any client or test matching AWS's text fails to match.
  Fix:      the scopeId validation in the GetQueryResultsWorkloadInsightsTopContributors handler in services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/src/ops -- the workload-insights results path must use the scopeId sentence, not the control-plane Scope-ARN sentence.
  Traces:   networkflowmonitor-workload-insights-results-unknown-scope#task#1.

Cause 2 of 3: GetQueryResultsWorkloadInsightsTopContributorsData rejects a malformed scopeId with the wrong sentence.
  AWS:      recorded behavior only, as cause 1. Recorded response: 400 with message "ScopeId provided isn't valid." (entirely fixed text).
  Emulator: 400 with message 'The provided Scope ARN is not valid'.
  Why:      as cause 1, on the topContributorsDataQueries results path.
  Fix:      the scopeId validation in the GetQueryResultsWorkloadInsightsTopContributorsData handler.
  Traces:   networkflowmonitor-workload-insights-results-unknown-scope#task#3.

Cause 3 of 3: the @pattern constraint-violation message quotes the regex with its anchors instead of stripping them.
  AWS:      Smithy: GetQueryStatusMonitorTopContributors.monitorName targets ResourceName with @pattern "^[a-zA-Z0-9_.-]+$" (and @length 1..255). AWS's constraint-violation sentence reports the pattern WITHOUT the ^ and $ anchors. Recorded response: 400 with message "1 validation error detected: Value '<supplied-value>' at 'monitorName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: ". Variable parts taken from the request and the model: the rejected value, the member name ('monitorName'), and the modelled pattern with its anchors dropped.
  Emulator: same status, class and sentence, but the pattern is quoted verbatim from the model, anchors included: "... regular expression pattern: ^[a-zA-Z0-9_.-]+$".
  Why:      a client (or an assertion) comparing the reported pattern against the pattern the service documents sees a different regex than AWS reports, and a caller that feeds the reported pattern back into its own validator gets a pattern AWS would never have printed. It is also inconsistent within this repo: the anchor-drop is owned by service_common::validation (see open issue 20261127T022844Z-aiops-createinvestigationgroup-maintainability--r2.md, which reports another crate hand-rolling the same drop), so this crate is bypassing the shared rule.
  Fix:      the request validation for GetQueryStatusMonitorTopContributors in this crate -- route the @pattern message through the shared anchor-dropping formatter in service_common::validation instead of emitting the raw modelled pattern.
  Traces:   networkflowmonitor-monitor-query-status-name-constraints#task#1.

No open issue covers networkflowmonitor.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout; 3 divergence clusters, all verdict=mismatch, service=networkflowmonitor, status matched and only $.message diverged.

1) networkflowmonitor-workload-insights-results-unknown-scope#task#1 GetQueryResultsWorkloadInsightsTopContributors (status 400 both sides)
   expected $.message: "ScopeId provided isn't valid."
   actual   $.message: 'The provided Scope ARN is not valid'

2) networkflowmonitor-workload-insights-results-unknown-scope#task#3 GetQueryResultsWorkloadInsightsTopContributorsData (status 400 both sides)
   expected $.message: "ScopeId provided isn't valid."
   actual   $.message: 'The provided Scope ARN is not valid'

3) networkflowmonitor-monitor-query-status-name-constraints#task#1 GetQueryStatusMonitorTopContributors (status 400 both sides)
   expected $.message: "1 validation error detected: Value 'sim2real~probe~nfm' at 'monitorName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_.-]+"
   actual   $.message: "1 validation error detected: Value 'sim2real~probe~nfm' at 'monitorName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"
   (the rejected value shown is this recording's input; the requirement is that the rejected value and member name come from the request and the pattern is the modelled one with ^ and $ removed)

Model evidence (services/service-com-amazonaws-networkflowmonitor-networkflowmonitor/smithy-model/com-amazonaws-networkflowmonitor-networkflowmonitor.json): monitorName on GetQueryStatusMonitorTopContributors targets ResourceName with pattern "^[a-zA-Z0-9_.-]+$", length 1..255; scopeId on both GetQueryResultsWorkloadInsightsTopContributors and ...Data is a required httpLabel targeting ScopeId. Note the Scope-ARN sentence IS correct on the control-plane GetScope path (networkflowmonitor-error-paths#task#3 records 'The provided Scope ARN is not valid' there) -- it is being reused on the workload-insights scopeId paths where AWS says "ScopeId provided isn't valid." instead.

## Steps to Reproduce
1. Boot the emulator built from this checkout and point a NetworkFlowMonitor client at it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1) with replay_wire_corpus(); the three clusters listed in the evidence all report verdict=mismatch on $.message only.
3. Or by hand:
   a. GET /workloadInsights/<malformed-scope-id>/topContributorsQueries/<query-id>/results -> emulator answers 400 'The provided Scope ARN is not valid'; AWS answers 400 "ScopeId provided isn't valid."
   b. GET /workloadInsights/<malformed-scope-id>/topContributorsDataQueries/<query-id>/status.../results -> same divergence on the ...Data results path.
   c. GET /monitors/<name-containing-a-character-outside-[a-zA-Z0-9_.-]>/topContributorsQueries/<query-id>/status -> emulator's message quotes the pattern as ^[a-zA-Z0-9_.-]+$; AWS quotes it as [a-zA-Z0-9_.-]+.
4. Compare $.message against the expectations in the evidence.
