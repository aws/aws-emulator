Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-gui-session-and-contact-verification-refusals
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-gui-session-and-contact-verification-refusals lightsail-gui-session-and-database-telemetry-refusals lightsail-gui-session-and-metric-query-error-paths lightsail-gui-session-and-verification-refusals lightsail-metric-and-gui-session-missing-resource lightsail-database-logs-and-container-metrics-missing-resource-errors lightsail-database-metric-and-log-request-validation lightsail-instance-metric-data-input-validation
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|StartGUISession|4xx->4xx|$.__type
Issue-Key: lightsail|StartGUISession|4xx->4xx|$.code
Issue-Key: lightsail|StartGUISession|4xx->4xx|$.message
Issue-Key: lightsail|CreateGUISessionAccessDetails|4xx->4xx|$.__type
Issue-Key: lightsail|CreateGUISessionAccessDetails|4xx->4xx|$.code
Issue-Key: lightsail|CreateGUISessionAccessDetails|4xx->4xx|$.message
Issue-Key: lightsail|StopGUISession|4xx->4xx|$.__type
Issue-Key: lightsail|StopGUISession|4xx->4xx|$.code
Issue-Key: lightsail|StopGUISession|4xx->4xx|$.message
Issue-Key: lightsail|GetContainerServiceMetricData|4xx->4xx|$.__type
Issue-Key: lightsail|GetContainerServiceMetricData|4xx->4xx|$.message
Issue-Key: lightsail|GetRelationalDatabaseMetricData|4xx->4xx|$.__type
Issue-Key: lightsail|GetRelationalDatabaseMetricData|4xx->4xx|$.code
Issue-Key: lightsail|GetInstanceMetricData|4xx->4xx|$.__type
Issue-Key: lightsail|GetInstanceMetricData|4xx->4xx|$.code
Issue-Related: 20261127T163509Z-lightsail-metric-validation-errors-carry-non-aws-error.md
Issue-Commit: b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1

# Lightsail missing-resource refusals use NotFoundException and precede request validation

> _Note: this issue was identified against the code base at commit `b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartGUISession

## Description
When a Lightsail request names a resource that does not exist, the emulator answers `NotFoundException` with `code` = "DoesNotExist" and its own "The <Type> does not exist: <name>" text. AWS answers `InvalidInputException` for these operations, with its own code and prose. Two of the causes are also an ORDERING defect: the emulator looks the resource up before validating the request, so it reports the missing resource where AWS reports the request-parameter violation it found first. Smithy: both InvalidInputException (httpError 400) and NotFoundException (httpError 404) are modelled on these operations, each with members {code, docs, message, tip} — so which one is chosen is observable, and the emulator's pairing of `NotFoundException` with HTTP 400 contradicts the model's own httpError 404 for that shape as well. This bundle has 6 causes and all 6 must be closed.

Cause 1 of 6: StartGUISession naming an instance that does not exist.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Requested resource not found." — the whole message is fixed text and deliberately names no resource. Smithy: StartGUISession.resourceName -> ResourceName, required; errors include InvalidInputException (400) and NotFoundException (404).
  Emulator: 400 with `__type` = "NotFoundException", `code` = "DoesNotExist", `message` = "The Instance does not exist: <instanceName>".
  Why: a client matching the modelled InvalidInputException never matches, and one matching NotFoundException would expect HTTP 404 per the model yet receives 400; the emulator also discloses the resource name AWS withholds in this message.
  Fix: StartGUISession's handler in the lightsail service crate's ops module — the not-found branch must raise InvalidInputException with AWS's fixed message.
  Traces: lightsail-gui-session-and-contact-verification-refusals#task#1, lightsail-gui-session-and-contact-verification-refusals#task#4, lightsail-gui-session-and-database-telemetry-refusals#task#1, lightsail-gui-session-and-metric-query-error-paths#task#1, lightsail-gui-session-and-verification-refusals#task#3, lightsail-metric-and-gui-session-missing-resource#task#3.

Cause 2 of 6: CreateGUISessionAccessDetails naming an instance that does not exist.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Requested resource not found." (fixed text).
  Emulator: 400 `__type` = "NotFoundException", `code` = "DoesNotExist", `message` = "The Instance does not exist: <instanceName>".
  Why: as Cause 1, on the access-details operation.
  Fix: CreateGUISessionAccessDetails' handler in the lightsail ops module.
  Traces: lightsail-gui-session-and-contact-verification-refusals#task#2, lightsail-gui-session-and-database-telemetry-refusals#task#3, lightsail-gui-session-and-metric-query-error-paths#task#3, lightsail-gui-session-and-verification-refusals#task#5.

Cause 3 of 6: StopGUISession naming an instance that does not exist.
  AWS: 400 `InvalidInputException`, `code` = "InvalidInputException", `message` = "Requested resource not found." (fixed text).
  Emulator: 400 `__type` = "NotFoundException", `code` = "DoesNotExist", `message` = "The Instance does not exist: <instanceName>".
  Why: as Cause 1, on the stop operation.
  Fix: StopGUISession's handler in the lightsail ops module.
  Traces: lightsail-gui-session-and-contact-verification-refusals#task#3, lightsail-gui-session-and-database-telemetry-refusals#task#2, lightsail-gui-session-and-metric-query-error-paths#task#2, lightsail-gui-session-and-verification-refusals#task#4.

Cause 4 of 6: GetContainerServiceMetricData naming a container service that does not exist.
  AWS: 400 `InvalidInputException`, `message` = 'Resource with name "<serviceName>" does not exist.' — fixed text around `<serviceName>`, the value taken from the request, quoted with double quotes. Smithy: GetContainerServiceMetricData.serviceName -> ContainerServiceName, required; errors include InvalidInputException (400) and NotFoundException (404).
  Emulator: 400 `__type` = "NotFoundException", `message` = "The ContainerService does not exist: <serviceName>" — wrong error type and a different message template.
  Why: the container-metrics path reports a missing service under an error type the operation's callers do not handle, with text that does not match AWS's template.
  Fix: GetContainerServiceMetricData's handler in the lightsail ops module.
  Traces: lightsail-database-logs-and-container-metrics-missing-resource-errors#task#3.

Cause 5 of 6: GetRelationalDatabaseMetricData where the request BOTH names a database that does not exist AND asks for more datapoints than the per-request limit.
  AWS: validates the request parameters before resolving the resource, so it answers 400 `InvalidInputException`, `code` = "InvalidParams", `message` = "There were <n> issues found with your request. Your requested number of datapoints exceeds the limit of 1,440. Please reduce the datapoints requested by increasing Period, or decreasing the time range.." — fixed prefix "There were <n> issues found with your request. " with `<n>` the issue count, then the fixed datapoint-limit sentence; the limit 1,440 is the service's fixed quota, not a value from this request. Smithy: GetRelationalDatabaseMetricData.period -> MetricPeriod @range 60..86400 with required startTime/endTime, so the datapoint count is a derived request-level rule; errors include InvalidInputException (400).
  Emulator: 400 `__type` = "NotFoundException", `code` = "DoesNotExist", `message` = "The RelationalDatabase does not exist: <name>" — it resolved the resource first and never ran the datapoint check.
  Why: the caller is told the wrong thing is wrong. It creates or renames the database, retries, and fails again on the datapoint limit it was never shown — and the datapoint rule is never exercised for any nonexistent resource.
  Fix: ordering in GetRelationalDatabaseMetricData's handler in the lightsail ops module — request-parameter validation (including the derived datapoint-count rule) must run before the state lookup.
  Traces: lightsail-database-metric-and-log-request-validation#task#2, lightsail-database-metric-and-log-request-validation#task#3, lightsail-database-metric-and-log-request-validation#task#5, lightsail-gui-session-and-database-telemetry-refusals#task#5.

Cause 6 of 6: GetInstanceMetricData where the request BOTH names an instance that does not exist AND exceeds the datapoint limit.
  AWS: 400 `InvalidInputException`, `code` = "InvalidParams", `message` = "There were <n> issues found with your request. Your requested number of datapoints exceeds the limit of 1,440. Please reduce the datapoints requested by increasing Period, or decreasing the time range.." — request validation first, same template as Cause 5.
  Emulator: 400 `__type` = "NotFoundException", `code` = "DoesNotExist", `message` = "The Instance does not exist: <name>".
  Why: as Cause 5, on the instance metric path — so the ordering rule has to hold across the metric handlers, not in one of them.
  Fix: ordering in GetInstanceMetricData's handler in the lightsail ops module.
  Traces: lightsail-gui-session-and-metric-query-error-paths#task#6, lightsail-instance-metric-data-input-validation#task#4.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 6 divergence clusters (verdict=mismatch, service=lightsail):

1. lightsail-gui-session-and-contact-verification-refusals#task#1 StartGUISession (affected_count 6) — $.__type: expected 'InvalidInputException', got 'NotFoundException'; $.code: expected 'InvalidInputException', got 'DoesNotExist'; $.message: expected 'Requested resource not found.', got 'The Instance does not exist: sim2real-probe-lightsail-gui-1a04c98a'.

2. lightsail-gui-session-and-contact-verification-refusals#task#2 CreateGUISessionAccessDetails (affected_count 4) — identical three-field divergence.

3. lightsail-gui-session-and-contact-verification-refusals#task#3 StopGUISession (affected_count 4) — identical three-field divergence.

4. lightsail-database-logs-and-container-metrics-missing-resource-errors#task#3 GetContainerServiceMetricData — $.__type: expected 'InvalidInputException', got 'NotFoundException'; $.message: expected 'Resource with name "sim2real-probe-lscs-205c5a9e" does not exist.', got 'The ContainerService does not exist: sim2real-probe-lscs-205c5a9e'.

5. lightsail-database-metric-and-log-request-validation#task#2 GetRelationalDatabaseMetricData (affected_count 4) — $.__type: expected 'InvalidInputException', got 'NotFoundException'; $.code: expected 'InvalidParams', got 'DoesNotExist'; $.message: expected 'There were 1 issues found with your request. Your requested number of datapoints exceeds the limit of 1,440. Please reduce the datapoints requested by increasing Period, or decreasing the time range..', got 'The RelationalDatabase does not exist: sim2real-probe-lsdb-8ecc51bc'.

6. lightsail-gui-session-and-metric-query-error-paths#task#6 GetInstanceMetricData (affected_count 2) — $.__type: expected 'InvalidInputException', got 'NotFoundException'; $.code: expected 'InvalidParams', got 'DoesNotExist'; $.message: expected the same datapoint-limit text, got 'The Instance does not exist: sim2real-probe-lightsail-vm-559db014'.

Model check: InvalidInputException httpError 400, NotFoundException httpError 404, both with a `code` member; both are modelled on every operation above. The emulator serves NotFoundException over HTTP 400, which matches neither the fixture nor the model's httpError for that shape.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay lightsail-gui-session-and-contact-verification-refusals steps task#1, task#2 and task#3 (StartGUISession / CreateGUISessionAccessDetails / StopGUISession against an instance name that was never created) and read $.__type, $.code and $.message.
3. Replay lightsail-database-logs-and-container-metrics-missing-resource-errors#task#3 (GetContainerServiceMetricData for a container service that does not exist).
4. Replay lightsail-database-metric-and-log-request-validation#task#2 (GetRelationalDatabaseMetricData for a nonexistent database with a window/period combination exceeding 1,440 datapoints) — AWS reports the datapoint-limit InvalidParams issue, the emulator reports the missing database.
5. Replay lightsail-gui-session-and-metric-query-error-paths#task#6 (GetInstanceMetricData, same shape of request).
