Issue-Source: trace-sweep-divergence
Issue-Trace: logs-list-anomalies-limit-range-bounds
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-list-anomalies-limit-range-bounds logs-aggregate-log-group-summaries-limit-bounds logs-aggregate-summaries-name-pattern-length-bounds logs-list-aggregate-summaries-data-sources-count-bounds logs-describe-queries-unknown-log-group-answers-empty logsqlang-describe-queries-query-language-filter logs-list-log-groups-field-index-name-grammar logs-update-anomaly-pattern-id-fixed-length logs-update-anomaly-suppression-type-outside-enum logs-update-anomaly-suppression-unit-enum logs-create-delivery-suffix-path-length-bounds logs-create-scheduled-query-lookup-table-tag-key-length logs-scheduled-query-schedule-start-time-lower-bound logs-scheduled-query-name-length-bound
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|ListAnomalies|2xx->4xx|$.anomalies
Issue-Key: logs|ListAnomalies|2xx->4xx|status
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->4xx|$.aggregateLogGroupSummaries
Issue-Key: logs|ListAggregateLogGroupSummaries|2xx->4xx|status
Issue-Key: logs|DescribeQueries|2xx->4xx|$.queries
Issue-Key: logs|DescribeQueries|2xx->4xx|status
Issue-Key: logs|ListLogGroups|2xx->4xx|$.logGroups
Issue-Key: logs|ListLogGroups|2xx->4xx|status
Issue-Key: logs|UpdateAnomaly|2xx->4xx|$
Issue-Key: logs|UpdateAnomaly|2xx->4xx|status
Issue-Key: logs|CreateDelivery|2xx->4xx|$.delivery
Issue-Key: logs|CreateDelivery|2xx->4xx|status
Issue-Key: logs|CreateScheduledQuery|2xx->4xx|$.scheduledQueryArn
Issue-Key: logs|CreateScheduledQuery|2xx->4xx|$.state
Issue-Key: logs|CreateScheduledQuery|2xx->4xx|status
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.creationTime
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.endTimeOffset
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.executionRoleArn
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.lastUpdatedTime
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.logGroupIdentifiers
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.name
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.queryLanguage
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.queryString
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.scheduleExpression
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.scheduleType
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.scheduledQueryArn
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.startTimeOffset
Issue-Key: logs|GetScheduledQuery|2xx->4xx|$.state
Issue-Key: logs|GetScheduledQuery|2xx->4xx|status
Issue-Key: logs|DeleteScheduledQuery|2xx->4xx|$
Issue-Key: logs|DeleteScheduledQuery|2xx->4xx|status
Issue-Key: logs|GetScheduledQuery|4xx->4xx|$.__type
Issue-Key: logs|GetScheduledQuery|4xx->4xx|$.message
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs refuses ten requests AWS accepts, answering 400 where AWS returns 200

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAnomalies

## Description
This issue bundles 10 causes that share ONE fault: the emulator applies bounds and lookups that CloudWatch Logs does not, so requests AWS answers 200 are refused 400. In every case the recorded AWS response is a success (usually an empty page or an empty body), and the emulator's 400 makes the operation unusable at the edge of its documented input range.

Cause 1 of 10: ListAnomalies at the edge of its limit range. AWS: 200 with `{"anomalies": []}`. Emulator: 400. Why: a client paginating anomalies with a boundary limit gets a hard failure instead of an empty page. Fix: ListAnomalies' input validation (the limit range check). Traces: logs-list-anomalies-limit-range-bounds#task#4.

Cause 2 of 10: ListAggregateLogGroupSummaries at its limit/name-pattern/data-source bounds. AWS: 200 with `{"aggregateLogGroupSummaries": []}`. Emulator: 400. Why/Fix: as cause 1, in ListAggregateLogGroupSummaries' validation. Traces: logs-aggregate-log-group-summaries-limit-bounds#task#2 (also logs-aggregate-summaries-name-pattern-length-bounds#task#6, logs-list-aggregate-summaries-data-sources-count-bounds#task#3).

Cause 3 of 10: DescribeQueries for a log group that does not exist. AWS: 200 with `{"queries": []}` — the operation is a filtered listing and does not resolve the group. Emulator: 400. Why: a client cannot distinguish "no queries" from "bad request", and query discovery against a not-yet-created group breaks. Fix: DescribeQueries' handler must not resolve logGroupName as a precondition. Traces: logs-describe-queries-unknown-log-group-answers-empty#task#4 (also task#8, logsqlang-describe-queries-query-language-filter#task#6, #task#7).

Cause 4 of 10: ListLogGroups with field-index-name filters. AWS: 200 with `{"logGroups": []}`. Emulator: 400. Why/Fix: as cause 1, in ListLogGroups' filter validation. Traces: logs-list-log-groups-field-index-name-grammar#task#4 (also task#5, task#6, task#7, task#8).

Cause 5 of 10: UpdateAnomaly with a well-formed patternId. AWS: 200 with an empty body. Emulator: 400 ResourceNotFoundException `No matching anomaly found for patternId: <patternId>`. Why: suppressing or un-suppressing an anomaly by pattern id never succeeds. Fix: UpdateAnomaly's resolution of patternId against the anomaly state. Traces: logs-update-anomaly-pattern-id-fixed-length#task#3 (also logs-update-anomaly-suppression-type-outside-enum#task#6, logs-update-anomaly-suppression-unit-enum#task#5).

Cause 6 of 10: CreateDelivery with an empty s3DeliveryConfiguration.suffixPath. AWS: 200, returning the delivery with `s3DeliveryConfiguration.suffixPath: ""`. Emulator: 400. Why: an empty suffix path — the documented way to write objects at the bucket root — cannot be configured. Fix: CreateDelivery's suffixPath validation (its lower bound is 0, not 1). Traces: logs-create-delivery-suffix-path-length-bounds#task#10.

Cause 7 of 10: CreateScheduledQuery with a legal lookup-table tag key. AWS: 200, returning scheduledQueryArn and `state: DISABLED`. Emulator: 400. Why: scheduled queries with a lookup-table destination cannot be created. Fix: CreateScheduledQuery's destinationConfiguration.lookupTableConfiguration.tags validation. Traces: logs-create-scheduled-query-lookup-table-tag-key-length#task#4.

Cause 8 of 10: GetScheduledQuery with a long-but-legal identifier. AWS: 200, returning the whole scheduled-query record (name, queryString, scheduleExpression, scheduleType, state, offsets, executionRoleArn, timestamps, scheduledQueryArn). Emulator: 400. Why: a scheduled query whose name approaches the name bound can never be read back. Fix: the identifier bound must be applied to the NAME form only; the ARN form of the identifier is legitimately longer. Traces: logs-scheduled-query-name-length-bound#task#7.

Cause 9 of 10: DeleteScheduledQuery with the same identifier. AWS: 200 with an empty body. Emulator: 400 ValidationException `1 validation error detected: Value at 'identifier' failed to satisfy constraint: Member must have length less than or equal to 300`. Why: such a scheduled query cannot be deleted, so it is permanently stranded. Fix: as cause 8, in DeleteScheduledQuery's validation. Traces: logs-scheduled-query-name-length-bound#task#8.

Cause 10 of 10: GetScheduledQuery for a deleted scheduled query. AWS: 400 ResourceNotFoundException `Scheduled query with identifier <identifier> not found` (fixed text with the identifier from the request). Emulator: 400 ValidationException with the 300-character length message, because the same over-eager bound fires first. Why: the client is told its request is malformed rather than that the resource is gone. Fix: as cause 8 — once the bound is corrected the lookup must run and raise ResourceNotFoundException. Traces: logs-scheduled-query-name-length-bound#task#9.

## Evidence
logs-list-anomalies-limit-range-bounds#task#4: expected 200 {"anomalies": []}, actual 400.
logs-aggregate-log-group-summaries-limit-bounds#task#2: expected 200 {"aggregateLogGroupSummaries": []}, actual 400.
logs-describe-queries-unknown-log-group-answers-empty#task#4: expected 200 {"queries": []}, actual 400.
logs-list-log-groups-field-index-name-grammar#task#4: expected 200 {"logGroups": []}, actual 400.
logs-update-anomaly-pattern-id-fixed-length#task#3: expected 200 {}, actual 400 ResourceNotFoundException "No matching anomaly found for patternId: ...".
logs-create-delivery-suffix-path-length-bounds#task#10: expected 200 with delivery.s3DeliveryConfiguration.suffixPath="", actual 400.
logs-create-scheduled-query-lookup-table-tag-key-length#task#4: expected 200 with scheduledQueryArn and state=DISABLED, actual 400.
logs-scheduled-query-name-length-bound#task#7: expected 200 full GetScheduledQuery record, actual 400; #task#8: expected 200 {}, actual 400 ValidationException "... 'identifier' ... less than or equal to 300"; #task#9: expected 400 ResourceNotFoundException "Scheduled query with identifier <arn> not found", actual the same length ValidationException.

## Steps to Reproduce
1. Replay logs-list-anomalies-limit-range-bounds, logs-aggregate-log-group-summaries-limit-bounds, logs-describe-queries-unknown-log-group-answers-empty and logs-list-log-groups-field-index-name-grammar: the listed steps answer 400 where the recording is a 200 empty page.
2. Replay logs-update-anomaly-pattern-id-fixed-length#task#3: UpdateAnomaly answers ResourceNotFoundException for a patternId AWS accepts.
3. Replay logs-create-delivery-suffix-path-length-bounds#task#10 and logs-create-scheduled-query-lookup-table-tag-key-length#task#4: both creations are refused where AWS succeeds.
4. Replay logs-scheduled-query-name-length-bound steps task#7, task#8 and task#9: the 300-character identifier bound fires on the ARN form, so the record cannot be read, deleted, or reported missing with the right error.
