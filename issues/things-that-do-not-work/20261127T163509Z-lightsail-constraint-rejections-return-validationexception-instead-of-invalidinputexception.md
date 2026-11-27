Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-bucket-metric-data-request-validation
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-bucket-metric-data-request-validation lightsail-contact-method-verification-errors lightsail-contact-method-verification-refusals lightsail-gui-session-and-contact-verification-refusals lightsail-gui-session-and-verification-refusals lightsail-database-metric-and-log-request-validation lightsail-relational-database-log-events-errors lightsail-distribution-metric-data-request-validation lightsail-metric-and-log-stream-refusals lightsail-gui-session-and-metric-query-error-paths lightsail-load-balancer-metric-data-refusals lightsail-relational-database-log-streams-refusals
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|GetBucketMetricData|4xx->4xx|$.__type
Issue-Key: lightsail|GetBucketMetricData|4xx->4xx|$.code
Issue-Key: lightsail|GetBucketMetricData|4xx->4xx|$.message
Issue-Key: lightsail|SendContactMethodVerification|4xx->4xx|$.__type
Issue-Key: lightsail|SendContactMethodVerification|4xx->4xx|$.code
Issue-Key: lightsail|SendContactMethodVerification|4xx->4xx|$.message
Issue-Key: lightsail|GetRelationalDatabaseLogEvents|4xx->4xx|$.__type
Issue-Key: lightsail|GetRelationalDatabaseLogEvents|4xx->4xx|$.code
Issue-Key: lightsail|GetRelationalDatabaseLogEvents|4xx->4xx|$.message
Issue-Key: lightsail|GetDistributionMetricData|4xx->4xx|$.__type
Issue-Key: lightsail|GetDistributionMetricData|4xx->4xx|$.code
Issue-Key: lightsail|GetDistributionMetricData|4xx->4xx|$.message
Issue-Key: lightsail|GetLoadBalancerMetricData|4xx->4xx|$.__type
Issue-Key: lightsail|GetLoadBalancerMetricData|4xx->4xx|$.code
Issue-Key: lightsail|GetLoadBalancerMetricData|4xx->4xx|$.message
Issue-Key: lightsail|GetRelationalDatabaseLogStreams|4xx->4xx|$.__type
Issue-Key: lightsail|GetRelationalDatabaseLogStreams|4xx->4xx|$.code
Issue-Key: lightsail|GetRelationalDatabaseLogStreams|4xx->4xx|$.message
Issue-Commit: b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1

# Lightsail constraint rejections return ValidationException instead of InvalidInputException

> _Note: this issue was identified against the code base at commit `b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBucketMetricData

## Description
Six Lightsail operations reject an out-of-contract input by emitting the generic Smithy-constraint error `ValidationException` with the machine-generated "N validation error(s) detected: ..." text, instead of the only client input error Lightsail models for these operations: `InvalidInputException` carrying its own `code` member and Lightsail's own prose. Smithy: InvalidInputException is a client error with httpError 400 and members {code, docs, message, tip}, and it is the modelled input error on every operation below; `ValidationException` is not in any of their error lists. This bundle has 6 causes and all 6 must be closed.

Cause 1 of 6: GetBucketMetricData, `period` below the modelled minimum.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Invalid period, must be between 60 and 86400." — the whole message is fixed text with no request-derived parts. Smithy: GetBucketMetricData.period -> MetricPeriod @range min 60 max 86400, required; errors include InvalidInputException (400).
  Emulator: 400 but `__type` = "ValidationException", no `code` member at all, `message` = "1 validation error detected: Value '59' at 'period' failed to satisfy constraint: Member must have value greater than or equal to 60".
  Why: a client that branches on the error type or on InvalidInputException.code never sees the input error Lightsail documents, so retry/repair logic keyed to InvalidInputException falls through to a generic unknown-error path and the operation looks like a server fault rather than a bad argument.
  Fix: the request-validation layer for GetBucketMetricData in the lightsail service crate's ops module — the modelled @range/@pattern/@enum rejections must be mapped to InvalidInputException with Lightsail's message, not surfaced by the generic constraint validator.
  Traces: lightsail-bucket-metric-data-request-validation#task#5.

Cause 2 of 6: SendContactMethodVerification with a protocol outside the modelled set.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "The contact protocol <protocol> is not supported. The valid contact protocol values are [Email]." — fixed text around two variable parts: `<protocol>` is the value taken from the request's `protocol` member, and the bracketed list is the modelled value set. Smithy: SendContactMethodVerification.protocol -> ContactMethodVerificationProtocol, enum values ["Email"], required; errors include InvalidInputException (400).
  Emulator: 400 with `__type` = "ValidationException", no `code`, `message` = "1 validation error detected: Value 'SMS' at 'protocol' failed to satisfy constraint: Member must satisfy enum value set: [Email]".
  Why: the caller loses the service-level statement of which protocols are usable in this account/region and gets an error type it cannot match on.
  Fix: SendContactMethodVerification's validation in the lightsail ops module.
  Traces: lightsail-contact-method-verification-errors#task#1, lightsail-contact-method-verification-refusals#task#1, lightsail-gui-session-and-contact-verification-refusals#task#5, lightsail-gui-session-and-verification-refusals#task#6.

Cause 3 of 6: GetRelationalDatabaseLogEvents with a `relationalDatabaseName` that fails the resource-name pattern.
  AWS: 400 `InvalidInputException`, `code` = "InvalidResourceName", `message` = "<name> is not a valid resource name. Resource names must be at least 2 characters long and contain only alphanumerics, -, _, and ." — the sentence after the name is fixed text; `<name>` is the rejected value copied from the request. Smithy: GetRelationalDatabaseLogEvents.relationalDatabaseName -> ResourceName @pattern ^\w[\w\-]*\w$, required; errors include InvalidInputException (400).
  Emulator: 400 with `__type` = "ValidationException", no `code` (AWS sets the distinct code "InvalidResourceName" here, not "InvalidInputException"), `message` = "1 validation error detected: Value '-...-' at 'relationalDatabaseName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\w[\\w\\-]*\\w$".
  Why: the emulator leaks the raw regex instead of the human-readable naming rule, and drops the `InvalidResourceName` code that clients use to distinguish a malformed name from any other bad argument.
  Fix: ResourceName validation shared by the relational-database operations in the lightsail ops module.
  Traces: lightsail-database-metric-and-log-request-validation#task#7, lightsail-database-metric-and-log-request-validation#task#8, lightsail-relational-database-log-events-errors#task#2.

Cause 4 of 6: GetDistributionMetricData, `period` above the modelled maximum.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Invalid period, must be between 60 to 86400." — all fixed text. Note the distribution wording is "between 60 to 86400" where the bucket operation (Cause 1) says "between 60 and 86400"; both are fixed per-operation text and must be reproduced per operation. Smithy: GetDistributionMetricData.period -> MetricPeriod @range min 60 max 86400, required; errors include InvalidInputException (400).
  Emulator: 400 with `__type` = "ValidationException", no `code`, `message` = "1 validation error detected: Value '86401' at 'period' failed to satisfy constraint: Member must have value less than or equal to 86400".
  Why: same loss of error type and `code` as Cause 1, on the distribution metric path.
  Fix: GetDistributionMetricData's validation in the lightsail ops module.
  Traces: lightsail-distribution-metric-data-request-validation#task#3, lightsail-distribution-metric-data-request-validation#task#4, lightsail-metric-and-log-stream-refusals#task#6.

Cause 5 of 6: GetLoadBalancerMetricData with `period` = 0.
  AWS: 400 `InvalidInputException`, `code` = "InvalidParams", `message` = "There were <n> issues found with your request. Period must be greater than 0 and a multiple of 60. Your requested number of datapoints exceeds the limit of 1,440. Please reduce the datapoints requested by increasing Period, or decreasing the time range.." — the prefix "There were <n> issues found with your request. " is fixed text with `<n>` the number of issues AWS found, followed by one fixed sentence per issue. AWS evaluates ALL request issues and reports them together (here 2: the period rule and the datapoint-count limit), it does not stop at the first. Smithy: GetLoadBalancerMetricData.period -> MetricPeriod @range min 60 max 86400; errors include InvalidInputException (400).
  Emulator: 400 with `__type` = "ValidationException", no `code`, and a single-issue message "1 validation error detected: Value '0' at 'period' failed to satisfy constraint: Member must have value greater than or equal to 60".
  Why: besides the wrong error type and missing code, the caller is told about one problem when two must be fixed, so the next request fails again.
  Fix: GetLoadBalancerMetricData's validation in the lightsail ops module — it must accumulate issues into the aggregated InvalidInputException/InvalidParams message rather than returning the first constraint failure.
  Traces: lightsail-gui-session-and-metric-query-error-paths#task#10, lightsail-load-balancer-metric-data-refusals#task#2, lightsail-load-balancer-metric-data-refusals#task#3, lightsail-load-balancer-metric-data-refusals#task#5.

Cause 6 of 6: GetRelationalDatabaseLogStreams with a `relationalDatabaseName` that fails the resource-name pattern.
  AWS: 400 `InvalidInputException`, `code` = "InvalidResourceName", `message` = "<name> is not a valid resource name. Resource names must be at least 2 characters long and contain only alphanumerics, -, _, and ." with `<name>` from the request. Smithy: GetRelationalDatabaseLogStreams.relationalDatabaseName -> ResourceName @pattern ^\w[\w\-]*\w$, required; errors include InvalidInputException (400).
  Emulator: 400 with `__type` = "ValidationException", no `code`, and the raw-regex constraint message.
  Why: as Cause 3, on a second operation — so the fix must live in the shared name validator, not one handler.
  Fix: the shared ResourceName validation in the lightsail ops module.
  Traces: lightsail-relational-database-log-streams-refusals#task#2, lightsail-relational-database-log-streams-refusals#task#3.

Related but not covering this: open issue 20261028T134325Z-lightsail-getrelationaldatabaselogevents-validation.md concerns out-of-range startTime/endTime being silently coerced, not the error type/code emitted for constraint violations.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 6 divergence clusters (verdict=mismatch, service=lightsail):

1. lightsail-bucket-metric-data-request-validation#task#5 GetBucketMetricData — expected 400 $.__type='InvalidInputException', $.code='InvalidInputException', $.message='Invalid period, must be between 60 and 86400.'; actual 400 $.__type='ValidationException', $.code missing, $.message="1 validation error detected: Value '59' at 'period' failed to satisfy constraint: Member must have value greater than or equal to 60".

2. lightsail-contact-method-verification-errors#task#1 SendContactMethodVerification (+3 affected) — expected $.__type='InvalidInputException', $.code='InvalidInputException', $.message='The contact protocol SMS is not supported. The valid contact protocol values are [Email].'; actual $.__type='ValidationException', $.code missing, $.message="1 validation error detected: Value 'SMS' at 'protocol' failed to satisfy constraint: Member must satisfy enum value set: [Email]".

3. lightsail-database-metric-and-log-request-validation#task#7 GetRelationalDatabaseLogEvents (+2 affected) — expected $.__type='InvalidInputException', $.code='InvalidResourceName', $.message='-<name>- is not a valid resource name. Resource names must be at least 2 characters long and contain only alphanumerics, -, _, and .'; actual $.__type='ValidationException', $.code missing, $.message="1 validation error detected: Value '-<name>-' at 'relationalDatabaseName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\w[\\w\\-]*\\w$".

4. lightsail-distribution-metric-data-request-validation#task#3 GetDistributionMetricData (+2 affected) — expected $.__type='InvalidInputException', $.code='InvalidInputException', $.message='Invalid period, must be between 60 to 86400.'; actual $.__type='ValidationException', $.code missing, $.message="1 validation error detected: Value '86401' at 'period' failed to satisfy constraint: Member must have value less than or equal to 86400".

5. lightsail-gui-session-and-metric-query-error-paths#task#10 GetLoadBalancerMetricData (+3 affected) — expected $.__type='InvalidInputException', $.code='InvalidParams', $.message='There were 2 issues found with your request. Period must be greater than 0 and a multiple of 60. Your requested number of datapoints exceeds the limit of 1,440. Please reduce the datapoints requested by increasing Period, or decreasing the time range..'; actual $.__type='ValidationException', $.code missing, single-issue constraint message.

6. lightsail-relational-database-log-streams-refusals#task#2 GetRelationalDatabaseLogStreams (+1 affected) — expected $.__type='InvalidInputException', $.code='InvalidResourceName', naming-rule prose; actual $.__type='ValidationException', $.code missing, raw-regex constraint message.

Model check (services/service-com-amazonaws-lightsail-lightsail-20161128/smithy-model/com-amazonaws-lightsail-lightsail-20161128.json): InvalidInputException = client error, httpError 400, members {code, docs, message, tip}; it is in the errors list of all six operations. ValidationException is in none of them.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay lightsail-bucket-metric-data-request-validation step task#5 (GetBucketMetricData with period=59) and read $.__type, $.code, $.message of the 400 body.
3. Replay lightsail-contact-method-verification-errors#task#1 (SendContactMethodVerification protocol=SMS).
4. Replay lightsail-database-metric-and-log-request-validation#task#7 (GetRelationalDatabaseLogEvents with a name failing ^\w[\w\-]*\w$).
5. Replay lightsail-distribution-metric-data-request-validation#task#3 (GetDistributionMetricData with period=86401).
6. Replay lightsail-gui-session-and-metric-query-error-paths#task#10 (GetLoadBalancerMetricData with period=0).
7. Replay lightsail-relational-database-log-streams-refusals#task#2 (GetRelationalDatabaseLogStreams with an invalid name).
In every case the body must be InvalidInputException with the documented `code` and Lightsail prose; the emulator answers ValidationException with a generated constraint message and no `code`.
