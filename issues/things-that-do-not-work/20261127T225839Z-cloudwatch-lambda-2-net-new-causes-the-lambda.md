Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda.lambda-data-source-without-datasource-principal-permission-rejected cloudwatch__lambda.region-level-concurrent-executions-series-follows-invocations
Issue-Key: cloudwatch__lambda|GetMetricData|ValidationError
Issue-Key: cloudwatch__lambda.region-level-concurrent-executions-series-follows-invocations|_poll#16|AssertionError|raise AssertionError(f"{what} not reached within {timeout_s}s; last {last!r}")
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# cloudwatch__lambda: 2 net-new causes — the LAMBDA() custom-data-source function does not exist in GetMetricData, and AWS/Lambda ConcurrentExecutions has no Region-level (dimensionless) series

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Two independent defects in the CloudWatch/Lambda pairing, both grounded in failing checks of this service grouping that are neither OWNED nor ALREADY FILED.

Cause 1 of 2: a GetMetricData query naming a Lambda data source is refused because the LAMBDA metric-math function is unknown, so the data-source authorization behavior it exists to show can never be exercised.
  AWS: LAMBDA('<function>', args...) in MetricDataQuery$Expression (Smithy services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801, MetricDataQuery$Expression: "a metric math expression to be performed on the returned data") invokes the named connector with a GetMetricData event and returns its MetricDataResults. When the connector has NOT granted lambda:InvokeFunction to the principal lambda.datasource.cloudwatch.amazonaws.com, the request still succeeds (HTTP 200) and the affected MetricDataResult carries StatusCode 'Forbidden' with no values; once the permission is added, the same query returns the connector's series with StatusCode 'Complete'.
  Emulator: every such request is refused HTTP 400 ValidationError with the message "Error in expression 'q1': Function 'LAMBDA' not found" — the function is not in the metric-math registry, so neither the Forbidden nor the Complete outcome exists.
  Why: a client wiring a custom data source gets a request-wide validation error instead of a per-result authorization status, so it cannot distinguish 'the connector refused me' from 'my expression is malformed', and the whole multi-data-source feature is unusable.
  Fix: register and implement LAMBDA in the metric-math function registry — invoke the connector, map its MetricDataResults onto the requested grid, and map a missing invoke permission onto the per-result StatusCode 'Forbidden' rather than a request-wide refusal. (The same missing function also fails two checks under the cloudwatch heading, filed there; this check's own replay key is carried here.)
  Traces: cloudwatch__lambda.lambda-data-source-without-datasource-principal-permission-rejected.

Cause 2 of 2: AWS/Lambda ConcurrentExecutions is published only per function, never as the Region-level series.
  AWS: ConcurrentExecutions is published both with the FunctionName dimension AND as a Region-level series with NO dimensions, which is what account-wide concurrency alarms watch; after invocations, the dimensionless series' Maximum is at least 1 and never below the per-function peak.
  Emulator: the per-function series reaches the expected peak (positive control, satisfied first in the same check), but GetMetricStatistics for ConcurrentExecutions with Dimensions=[] stays 0.0 for the whole 300 s poll.
  Why: the account/Region concurrency alarm AWS documents cannot be driven at all — a client watching regional concurrency against its account limit reads a flat zero while functions are running concurrently.
  Fix: the Lambda invoke path's metric emission must publish the dimensionless (Region-level) ConcurrentExecutions series beside the per-function one.
  Traces: cloudwatch__lambda.region-level-concurrent-executions-series-follows-invocations ('Region-level ConcurrentExecutions >= 1 not reached within 300s; last 0.0').

The open issue 20261113T060617Z-lambda-invoke-refuses-every-managed-runtime-family-other-than.md owns two other failures of this grouping and does not carry either replay key above; 20261120T090000Z-lambda-post-runtime-extensions-duration-is-never-published.md concerns a different metric.

## Evidence
Suite: service cloudwatch__lambda scored 19/24 (79%), 5 failed; three are OWNED by open issues and skipped.

[error] cloudwatch__lambda.lambda-data-source-without-datasource-principal-permission-rejected
  botocore ValidationException (ValidationError) calling GetMetricData: Error in expression 'q1': Function 'LAMBDA' not found
  Expected (AWS): HTTP 200; the MetricDataResult for the LAMBDA query carries StatusCode 'Forbidden' while the connector has not granted lambda:InvokeFunction to lambda.datasource.cloudwatch.amazonaws.com, and the connector's series with StatusCode 'Complete' once it has.
  Actual (emulator): HTTP 400, code ValidationError, message "Error in expression 'q1': Function 'LAMBDA' not found".

[mismatch] cloudwatch__lambda.region-level-concurrent-executions-series-follows-invocations
  AssertionError: Region-level ConcurrentExecutions >= 1 not reached within 300s; last 0.0
  Control that passed: the same metric with Dimensions=[{FunctionName: <fn>}] reached Maximum >= 1 after three invokes of a handler that sleeps 2 s.
  Expected (AWS): GetMetricStatistics(AWS/Lambda, ConcurrentExecutions, Dimensions=[], Period 60, Statistics [Maximum]) -> Maximum >= 1 and >= the per-function peak.
  Actual (emulator): 0.0 throughout.

Function names, roles and the per-run peak values are per-run values; the requirements are the namespace, the metric name, the empty dimension set, and the relation to the per-function series.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: create a Lambda connector that answers a GetMetricData event with MetricDataResults, do NOT add the lambda.datasource.cloudwatch.amazonaws.com invoke permission, then call cw:GetMetricData with MetricDataQueries=[{Id:'q1', Expression: "LAMBDA('<fn>', ...)"}] over a 5-minute grid.
   -> AWS: HTTP 200 with the q1 result's StatusCode 'Forbidden'; after lambda:AddPermission for that principal, HTTP 200 with the connector's series and StatusCode 'Complete'.
   -> Emulator: HTTP 400 ValidationError "Function 'LAMBDA' not found" in both cases.
3. Cause 2: create a function whose handler sleeps 2 s, invoke it three times, wait out the 60 s aggregation bucket, then call cw:GetMetricStatistics(Namespace=AWS/Lambda, MetricName=ConcurrentExecutions, Dimensions=[], Period=60, Statistics=[Maximum]).
   -> AWS: Maximum >= 1 (and >= the FunctionName-dimensioned peak).
   -> Emulator: 0.0, while the FunctionName-dimensioned series reports the peak.
4. Re-replay both checks in trace_ids to confirm the fix.
