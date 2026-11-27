Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__logs.account-level-incoming-metrics-count-new-events cloudwatch__logs.data-protection-audit-publishes-log-events-with-findings cloudwatch__logs.get-metric-data-insight-rule-metric-reads-contributor-counts cloudwatch__logs.insight-rule-log-group-name-prefix-wildcard-matches-groups
Issue-Key: cloudwatch__logs.account-level-incoming-metrics-count-new-events|_poll_until#9|AssertionError|raise AssertionError(f"{description} not observed within {waited}s; last saw {last!r}")
Issue-Key: cloudwatch__logs.data-protection-audit-publishes-log-events-with-findings|_poll_until#17|AssertionError|raise AssertionError(f"{description} not observed within {waited}s; last saw {last!r}")
Issue-Key: cloudwatch__logs|GetMetricData|ValidationError
Issue-Key: cloudwatch__logs.insight-rule-log-group-name-prefix-wildcard-matches-groups|_poll#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s; last {last!r}")
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# cloudwatch: CloudWatch Logs vended metrics and Contributor Insights are not published — account-level IncomingLogEvents and LogEventsWithFindings stay 0, INSIGHT_RULE_METRIC is unknown to GetMetricData, and a prefix-wildcard insight rule matches no groups

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Cause 1 of 4 — the account-level `AWS/Logs` `IncomingLogEvents` metric is never published.
AWS: PutLogEvents increments `AWS/Logs IncomingLogEvents` both per log group and at account scope (no LogGroupName dimension); the account-scoped datapoint appears within the metric's publishing window. Recorded behavior only (a vended-metric rule).
Emulator: GetMetricStatistics/GetMetricData for the account-level metric keeps returning 0 for the whole observation window.
Traces: cloudwatch__logs.account-level-incoming-metrics-count-new-events.

Cause 2 of 4 — the data-protection audit metric `LogEventsWithFindings` is never published.
AWS: a log group with a data-protection policy publishes `AWS/Logs LogEventsWithFindings` counting events in which a managed data identifier matched; after writing two matching events the Sum reaches at least 2.
Emulator: the Sum stays 0 for the whole window.
Traces: cloudwatch__logs.data-protection-audit-publishes-log-events-with-findings.

Cause 3 of 4 — `INSIGHT_RULE_METRIC` is not a known metric-math function.
AWS: `GetMetricData` accepts a metric-math expression `INSIGHT_RULE_METRIC(<ruleName>, <metricName>)` and returns the rule's contributor counts. Smithy (cloudwatch): `MetricDataQuery.Expression` is the metric-math string; an unknown function is a `ValidationError`, so the function's presence is the requirement.
Emulator: the call is refused — HTTP 400 `ValidationError` with message `Error in expression 'u': Function 'INSIGHT_RULE_METRIC' not found` (fixed text `Function 'INSIGHT_RULE_METRIC' not found`; `'u'` is the query id from the request).
Traces: cloudwatch__logs.get-metric-data-insight-rule-metric-reads-contributor-counts.

Cause 4 of 4 — a Contributor Insights rule whose `LogGroupNames` ends in a `*` prefix wildcard matches no log groups.
AWS: a rule definition naming `my-prefix*` applies to every existing and future log group with that prefix, and events written to them are counted into the rule's metrics.
Emulator: the rule's counted events stay at 0 with no contributors for the whole window.
Traces: cloudwatch__logs.insight-rule-log-group-name-prefix-wildcard-matches-groups.

Why: these four are the entire observability surface a client builds on top of CloudWatch Logs. Alarms on IncomingLogEvents and LogEventsWithFindings never fire (they read as missing data, so `TreatMissingData` decides the state rather than the workload), dashboards built on INSIGHT_RULE_METRIC fail with a ValidationError rather than rendering, and a prefix-scoped insight rule silently reports no traffic — all of which read as 'the system is quiet' instead of 'the metric does not exist'.

Fix: the CloudWatch Logs vended-metric publisher (account-scope IncomingLogEvents, data-protection LogEventsWithFindings), the CloudWatch metric-math evaluator (add INSIGHT_RULE_METRIC), and the Contributor Insights rule matcher (prefix-wildcard log-group selection).

## Evidence
Cause 1: "account-level IncomingLogEvents not observed within 54.9s; last saw 0".
Cause 2: "LogEventsWithFindings Sum >= 2 not observed within 54.8s; last saw 0".
Cause 3: botocore.errorfactory.ValidationException — An error occurred (ValidationError) when calling the GetMetricData operation: Error in expression 'u': Function 'INSIGHT_RULE_METRIC' not found.
Cause 4: "the wildcard-matched groups' three events counted not met within 45.0s; last (0.0, [])".

## Steps to Reproduce
1. PutLogEvents into a new log group, then read AWS/Logs IncomingLogEvents with no LogGroupName dimension — 0.
2. Attach a data-protection policy with a managed data identifier, write two matching events, then read AWS/Logs LogEventsWithFindings — Sum 0.
3. Call GetMetricData with Expression="INSIGHT_RULE_METRIC('<rule>', 'UniqueContributors')" — ValidationError, function not found.
4. PutInsightRule whose LogGroupNames is '<prefix>*', write events into two groups with that prefix, and read the rule's metrics — 0 counted, no contributors.
