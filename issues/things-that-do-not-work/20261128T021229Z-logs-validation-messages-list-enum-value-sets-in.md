Issue-Source: trace-sweep-divergence
Issue-Trace: logs-describe-export-tasks-status-code-values
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-describe-export-tasks-status-code-values create-log-group-class-outside-enum-refused logstagchar-create-log-group-tag-value-charset logs-create-scheduled-query-state-enum logs-scheduled-query-s3-destination-role-arn-length-bound logs-delivery-destination-output-format-enum logs-list-anomalies-suppression-state-values logs-transformer-parse-to-ocsf-version-enum logs-describe-field-indexes-index-category-enum logs-get-scheduled-query-history-execution-status-enum describe-import-task-batches-status-filter-values create-log-anomaly-detector-frequency-outside-enum-refused
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeExportTasks|4xx->4xx|$.message
Issue-Key: logs|CreateLogGroup|4xx->4xx|$.message
Issue-Key: logs|CreateScheduledQuery|4xx->4xx|$.message
Issue-Key: logs|PutDeliveryDestination|4xx->4xx|$.message
Issue-Key: logs|ListAnomalies|4xx->4xx|$.message
Issue-Key: logs|PutTransformer|4xx->4xx|$.message
Issue-Key: logs|DescribeFieldIndexes|4xx->4xx|$.message
Issue-Key: logs|GetScheduledQueryHistory|4xx->4xx|$.message
Issue-Key: logs|DescribeImportTaskBatches|4xx->4xx|$.message
Issue-Key: logs|CreateLogAnomalyDetector|4xx->4xx|$.message
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs validation messages list enum value sets in sorted order, not AWS's declared order

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeExportTasks

## Description
This issue bundles 10 causes that share ONE fault: the emulator's enum-constraint message renderer emits the rejected enum's value set sorted lexicographically, while CloudWatch Logs emits each enum's value set in a fixed, service-declared order that is not alphabetical. Every cause is a 400 whose status and error code are already correct and whose `message` text is wrong only in the bracketed value list (and, for the list-typed members, in the wrapper described in the collection-rendering issue filed alongside this one).

Message anatomy for every cause below — fixed text: `1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]`. Variable parts: `<member>` is the member path taken from the request, `<values>` is that enum's value set in the service's own order. No per-run value (request id, timestamp, generated name) appears anywhere in the message, so the whole text is a requirement.

Cause 1 of 10: DescribeExportTasks.statusCode. AWS: Smithy ExportTaskStatusCode values CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING; the recorded refusal renders them as `[PENDING_CANCEL, COMPLETED, CANCELLED, PENDING, FAILED, RUNNING]` (recorded behavior only — the order is the service's, neither the model's declaration order nor sorted). Emulator: renders `[CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING]`. Why: a client that string-matches or surfaces the refusal text sees a sentence AWS never emits, and any test asserting the documented wording fails. Fix: the enum-constraint arm of the shared request-validation message renderer must carry a per-enum ordered value list rather than sorting the enum's variants. Traces: logs-describe-export-tasks-status-code-values#task#2 (also task#3).

Cause 2 of 10: CreateLogGroup.logGroupClass. AWS: Smithy LogGroupClass values STANDARD, INFREQUENT_ACCESS, DELIVERY, ESSENTIALS; recorded refusal renders `[ESSENTIALS, INFREQUENT_ACCESS, DELIVERY, STANDARD]`. Emulator: `[DELIVERY, ESSENTIALS, INFREQUENT_ACCESS, STANDARD]`. Why/Fix: as cause 1. Traces: create-log-group-class-outside-enum-refused#task#1 (also task#2, logstagchar-create-log-group-tag-value-charset#task#1).

Cause 3 of 10: CreateScheduledQuery.state. AWS: Smithy ScheduledQueryState values ENABLED, DISABLED; recorded refusal renders `[ENABLED, DISABLED]`. Emulator: `[DISABLED, ENABLED]`. Why/Fix: as cause 1. Traces: logs-create-scheduled-query-state-enum#task#3 (also task#4, logs-scheduled-query-s3-destination-role-arn-length-bound#task#5).

Cause 4 of 10: PutDeliveryDestination.outputFormat. AWS: Smithy OutputFormat values json, plain, w3c, raw, parquet; recorded refusal renders `[w3c, raw, json, plain, parquet]`. Emulator: `[json, parquet, plain, raw, w3c]`. Why/Fix: as cause 1. Traces: logs-delivery-destination-output-format-enum#task#3 (also task#4).

Cause 5 of 10: ListAnomalies.suppressionState. AWS: Smithy SuppressionState values SUPPRESSED, UNSUPPRESSED; recorded refusal renders `[UNSUPPRESSED, SUPPRESSED]`. Emulator: `[SUPPRESSED, UNSUPPRESSED]`. Why/Fix: as cause 1. Traces: logs-list-anomalies-suppression-state-values#task#5.

Cause 6 of 10: PutTransformer.transformerConfig[].parseToOCSF.ocsfVersion. AWS: Smithy OCSFVersion values V1.1, V1.5; recorded refusal renders `[V1.5, V1.1]`. Emulator: `[V1.1, V1.5]`. Why/Fix: as cause 1. Traces: logs-transformer-parse-to-ocsf-version-enum#task#2 (also task#6).

Cause 7 of 10: DescribeFieldIndexes.indexCategories. AWS: Smithy IndexCategory values DEFAULT, CUSTOM, AUTO, INACTIVE; recorded refusal renders `[DEFAULT, INACTIVE, AUTO, CUSTOM]`. Emulator: `[AUTO, CUSTOM, DEFAULT, INACTIVE]`. Why/Fix: as cause 1. Traces: logs-describe-field-indexes-index-category-enum#task#6 (also task#7).

Cause 8 of 10: GetScheduledQueryHistory.executionStatuses. AWS: recorded refusal renders `[Timeout, InvalidQuery, Complete, Failed, Running]` (recorded behavior only). Emulator: `[Complete, Failed, InvalidQuery, Running, Timeout]`. Why/Fix: as cause 1. Traces: logs-get-scheduled-query-history-execution-status-enum#task#5 (also task#6).

Cause 9 of 10: DescribeImportTaskBatches.batchImportStatus. AWS: recorded refusal renders `[IN_PROGRESS, COMPLETED, CANCELLED, FAILED]`. Emulator: `[CANCELLED, COMPLETED, FAILED, IN_PROGRESS]`. Why/Fix: as cause 1. Traces: describe-import-task-batches-status-filter-values#task#3.

Cause 10 of 10: CreateLogAnomalyDetector.evaluationFrequency. AWS: Smithy EvaluationFrequency values ONE_MIN, FIVE_MIN, TEN_MIN, FIFTEEN_MIN, THIRTY_MIN, ONE_HOUR; the recorded refusal renders `[FIVE_MIN, FIFTEEN_MIN, TEN_MIN, THIRTY_MIN, ONE_HOUR]` — a different order AND a set that omits ONE_MIN. Emulator: `[FIFTEEN_MIN, FIVE_MIN, ONE_HOUR, ONE_MIN, TEN_MIN, THIRTY_MIN]`. Why/Fix: as cause 1; the membership half of this divergence (ONE_MIN being accepted) is a separate issue filed alongside. Traces: create-log-anomaly-detector-frequency-outside-enum-refused#task#3 (also task#4).

## Evidence
Representative (DescribeExportTasks, logs-describe-export-tasks-status-code-values#task#2), status 400 both sides:
Expected $.message: "1 validation error detected: Value at 'statusCode' failed to satisfy constraint: Member must satisfy enum value set: [PENDING_CANCEL, COMPLETED, CANCELLED, PENDING, FAILED, RUNNING]"
Actual   $.message: "1 validation error detected: Value at 'statusCode' failed to satisfy constraint: Member must satisfy enum value set: [CANCELLED, COMPLETED, FAILED, PENDING, PENDING_CANCEL, RUNNING]"
The same sorted-vs-declared difference appears on logGroupClass, state, outputFormat, suppressionState, ocsfVersion, indexCategories, executionStatuses, batchImportStatus and evaluationFrequency; evaluationFrequency additionally lists ONE_MIN, which AWS's set omits.

## Steps to Reproduce
1. Replay fixture logs-describe-export-tasks-status-code-values and compare step task#2's response body with the recorded one.
2. Repeat for create-log-group-class-outside-enum-refused#task#1, logs-create-scheduled-query-state-enum#task#3, logs-delivery-destination-output-format-enum#task#3, logs-list-anomalies-suppression-state-values#task#5, logs-transformer-parse-to-ocsf-version-enum#task#2, logs-describe-field-indexes-index-category-enum#task#6, logs-get-scheduled-query-history-execution-status-enum#task#5, describe-import-task-batches-status-filter-values#task#3 and create-log-anomaly-detector-frequency-outside-enum-refused#task#3.
3. In each case the status (400) and error code match; only the bracketed enum value list in $.message differs, sorted instead of in AWS's order.
