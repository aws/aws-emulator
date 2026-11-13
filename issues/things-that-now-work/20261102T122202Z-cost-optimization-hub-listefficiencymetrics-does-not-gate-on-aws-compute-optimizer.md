Issue-Source: trace-sweep-divergence
Issue-Trace: cost-optimization-hub-efficiency-metrics-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cost-optimization-hub-efficiency-metrics-workflow
Issue-Shape-Id: com.amazonaws.costoptimizationhub#CostOptimizationHubService
Issue-Key: cost-optimization-hub|ListEfficiencyMetrics|4xx->2xx|$.__type
Issue-Key: cost-optimization-hub|ListEfficiencyMetrics|4xx->2xx|$.message
Issue-Key: cost-optimization-hub|ListEfficiencyMetrics|4xx->2xx|status
Issue-Commit: 75d52ea0c257a701244c018d7c3f550d51e1d46f

# ListEfficiencyMetrics does not gate on AWS Compute Optimizer opt-in

> _Note: this issue was identified against the code base at commit `75d52ea0c257a701244c018d7c3f550d51e1d46f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListEfficiencyMetrics

## Description
Cause 1 of 1: ListEfficiencyMetrics answers 200 with a metrics payload for an account that is not opted into AWS Compute Optimizer, instead of refusing the call with a 400 ValidationException. Fixture cost-optimization-hub-efficiency-metrics-workflow, step task#2.

AWS: ListEfficiencyMetrics is a Cost Optimization Hub read whose efficiency/savings time series is derived from AWS Compute Optimizer analysis, so the account must be opted into Compute Optimizer before it can be served. When the calling account is not opted in, the operation does not return a series: it fails the request as a client-side validation failure. Smithy: ListEfficiencyMetrics declares error ValidationException (errorKind client, httpError 400), whose required member is `message` plus optional `reason`/`fields` (ValidationException doc: "The input fails to satisfy the constraints specified by an Amazon Web Services service."); the other modeled errors are AccessDeniedException (403), ThrottlingException (429) and InternalServerException (500). The model documents no opt-in rule itself, so for the precondition and its wording this is recorded behavior only: the recording shows HTTP 400 with awsJson `__type` = "com.amazonaws.costoptimizationhub#ValidationException" and `message` = "ListEfficiencyMetrics requires your account to be opted into AWS Compute Optimizer. Please enable Compute Optimizer in the AWS Compute Optimizer console or via the Compute Optimizer API." In that message the fixed text is "<Operation> requires your account to be opted into AWS Compute Optimizer. Please enable Compute Optimizer in the AWS Compute Optimizer console or via the Compute Optimizer API." and the only variable part is the operation name, taken from the request being served (here ListEfficiencyMetrics); no account id, time period, granularity or other per-run value appears in it.

Emulator: the request is accepted and served as a success. Replay reports status expected 400, got 200, with `$.__type` missing (expected 'com.amazonaws.costoptimizationhub#ValidationException', got None) and `$.message` missing (expected the opt-in text above, got None) — i.e. the body is a normal ListEfficiencyMetricsResponse rather than an error envelope. The handler in services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_efficiency_metrics.rs validates maxResults, granularity, timePeriod, groupBy, orderBy and nextToken, then goes straight to readable_recommendation_view/dedupe_by_resource/totals_by_group; there is no Compute Optimizer opt-in precondition anywhere in the request walk, and the service's state tracks no Compute Optimizer opt-in flag for the caller (only Cost Optimization Hub enrollment via UpdateEnrollmentStatus/ListEnrollmentStatuses and preferences). The operation's own module doc instead treats a no-spend account as a successful group carrying an explanatory `message`, which is a different, non-failing path.

Why: a client that handles the documented precondition gets it wrong in both directions. Code that catches ValidationException to tell the user to enable Compute Optimizer never runs, so the setup step is silently skipped; and the 200 body it receives instead is an authoritative-looking efficiency/savings series computed for an account whose Compute Optimizer analysis does not exist. Dashboards, cost reports and gating logic built on that read a zero/placeholder series as real optimization data, and a conformance or negative test that asserts the 400 fails against the emulator while passing against AWS.

Fix: the ListEfficiencyMetrics handler and its validation — add the Compute Optimizer opt-in precondition to the handler's pre-dispatch checks (before the recommendation view is read), raising ListEfficiencyMetricsErrorModeled::ValidationException with the fixed message text above and the request's operation name, and back it with the per-account Compute Optimizer opt-in state the service does not currently hold. Note the two open cost-optimization-hub issues (UpdatePreferences validation, and the ListRecommendations organization-view permission gate in state.rs) are unrelated to this precondition and do not cover it.

Traces: cost-optimization-hub-efficiency-metrics-workflow#task#2 (cluster of 1).

## Evidence
Fixture cost-optimization-hub-efficiency-metrics-workflow, step task#2, operation ListEfficiencyMetrics, verdict mismatch.

Expected (recorded AWS response):
  HTTP 400
  $.__type  = "com.amazonaws.costoptimizationhub#ValidationException"
  $.message = "ListEfficiencyMetrics requires your account to be opted into AWS Compute Optimizer. Please enable Compute Optimizer in the AWS Compute Optimizer console or via the Compute Optimizer API."

Actual (emulator):
  HTTP 200
  $.__type  missing (None)
  $.message missing (None)
  body is a successful ListEfficiencyMetricsResponse

Replay detail verbatim: "status: expected 400, got 200 / $.__type: missing (expected 'com.amazonaws.costoptimizationhub#ValidationException', got None) / $.message: missing (expected 'ListEfficiencyMetrics requires your account to be opted into AWS Compute Optimizer. Please enable Compute Optimizer in the AWS Compute Optimizer console or via the Compute Optimizer API.', got None)"

Model support: smithy_query(operation="ListEfficiencyMetrics") lists ValidationException errorKind=client httpError=400; smithy_query(shape="ValidationException") shows required member `message` (plus optional `reason`, `fields`). Handler source with no opt-in gate: services/service-com-amazonaws-costoptimizationhub-costoptimizationhubservice/src/ops/list_efficiency_metrics.rs lines 196-320.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay fixture cost-optimization-hub-efficiency-metrics-workflow and stop at step task#2 — the ListEfficiencyMetrics call made for an account that has not opted into AWS Compute Optimizer.
3. Equivalently, by hand: POST to the cost-optimization-hub endpoint with X-Amz-Target AWSCostOptimizationHubService.ListEfficiencyMetrics and a body naming a valid required granularity ("Monthly" or "Daily") and timePeriod, from an account with no Compute Optimizer opt-in (the emulator holds no such opt-in state for any account, so any caller reproduces it).
4. Observe HTTP 200 and a ListEfficiencyMetricsResponse body. Expected: HTTP 400 with __type "com.amazonaws.costoptimizationhub#ValidationException" and message "ListEfficiencyMetrics requires your account to be opted into AWS Compute Optimizer. Please enable Compute Optimizer in the AWS Compute Optimizer console or via the Compute Optimizer API."
