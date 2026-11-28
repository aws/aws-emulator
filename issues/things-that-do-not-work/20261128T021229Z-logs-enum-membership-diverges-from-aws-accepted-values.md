Issue-Source: trace-sweep-divergence
Issue-Trace: list-integrations-status-filter-enum
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: list-integrations-status-filter-enum create-log-anomaly-detector-frequency-outside-enum-refused logs-anomaly-detector-kms-key-id-length logs-list-scheduled-queries-schedule-type-values
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|ListIntegrations|4xx->4xx|$.message
Issue-Key: logs|CreateLogAnomalyDetector|4xx->2xx|$.__type
Issue-Key: logs|CreateLogAnomalyDetector|4xx->2xx|$.message
Issue-Key: logs|CreateLogAnomalyDetector|4xx->2xx|status
Issue-Key: logs|ListLogAnomalyDetectors|2xx->2xx|$.anomalyDetectors[*]
Issue-Key: logs|ListIntegrations|2xx->4xx|$.integrationSummaries
Issue-Key: logs|ListIntegrations|2xx->4xx|$.integrations
Issue-Key: logs|ListIntegrations|2xx->4xx|status
Issue-Key: logs|ListIntegrations|4xx->2xx|$.__type
Issue-Key: logs|ListIntegrations|4xx->2xx|$.message
Issue-Key: logs|ListIntegrations|4xx->2xx|status
Issue-Key: logs|ListScheduledQueries|4xx->4xx|$.message
Issue-Key: logs|ListScheduledQueries|2xx->4xx|$.scheduledQueries
Issue-Key: logs|ListScheduledQueries|2xx->4xx|status
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs enum membership diverges from AWS: accepted values refused and refused values accepted

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListIntegrations

## Description
This issue bundles 7 causes that share ONE fault: the emulator's enum value sets for three members do not hold the values CloudWatch Logs accepts, so the admission decision itself (not merely the message) is wrong in both directions — a legal value is refused 400 and an illegal value is admitted 200.

Cause 1 of 7: ListIntegrations.integrationStatus value set. AWS: the checked-in Smithy IntegrationStatus lists PROVISIONING, ACTIVE, FAILED, but the recorded service refusal names the set `[CREATE_FAILED, ROLLBACK, ACTIVE, PROVISIONING]` (recorded behavior only — the live enum is wider than the model and does not contain FAILED). Emulator: refuses with `[ACTIVE, FAILED, PROVISIONING]`. Why: a client filtering on CREATE_FAILED or ROLLBACK is told those are not legal values. Fix: the ListIntegrations input validation's enum table. Traces: list-integrations-status-filter-enum#task#5.

Cause 2 of 7: ListIntegrations refuses a value AWS accepts. AWS: 200 with `{"integrationSummaries": [], "integrations": []}` for a legal integrationStatus. Emulator: 400. Why: integration listing by status is unusable for the statuses the emulator does not know. Fix: as cause 1. Traces: list-integrations-status-filter-enum#task#3.

Cause 3 of 7: ListIntegrations admits a value AWS refuses. AWS: 400 InvalidParameterException, message `1 validation error detected: Value at 'integrationStatus' failed to satisfy constraint: Member must satisfy enum value set: [CREATE_FAILED, ROLLBACK, ACTIVE, PROVISIONING]` (fixed text; the member path comes from the request). Emulator: 200 with no body members. Why: a client's bad filter is silently treated as a successful empty listing. Fix: as cause 1. Traces: list-integrations-status-filter-enum#task#4.

Cause 4 of 7: CreateLogAnomalyDetector admits evaluationFrequency=ONE_MIN. AWS: 400 InvalidParameterException `1 validation error detected: Value at 'evaluationFrequency' failed to satisfy constraint: Member must satisfy enum value set: [FIVE_MIN, FIFTEEN_MIN, TEN_MIN, THIRTY_MIN, ONE_HOUR]` — ONE_MIN is a modelled variant that the service does not accept. Emulator: 200, creating the detector. Why: a caller believes a one-minute evaluation cadence was configured when AWS would have rejected the request. Fix: the CreateLogAnomalyDetector validation's accepted-frequency set (the Smithy enum is a superset of what the operation admits). Traces: create-log-anomaly-detector-frequency-outside-enum-refused#task#5 (also logs-anomaly-detector-kms-key-id-length#task#4).

Cause 5 of 7: the detector created by cause 4 is then listed. AWS: ListLogAnomalyDetectors returns no detector for the log group, because creation was refused. Emulator: returns a detector with evaluationFrequency ONE_MIN and anomalyDetectorStatus TRAINING. Why: downstream state diverges, so a test that asserts the detector was NOT created passes against AWS and fails here. Fix: follows from cause 4. Traces: create-log-anomaly-detector-frequency-outside-enum-refused#task#6.

Cause 6 of 7: ListScheduledQueries.scheduleType value set. AWS: the recorded refusal names `[SCHEDULED, AWS_MANAGED, CUSTOMER_MANAGED]`, wider than the checked-in Smithy ScheduleType (CUSTOMER_MANAGED, AWS_MANAGED). Emulator: `[AWS_MANAGED, CUSTOMER_MANAGED]`. Why: the message advertises a value set a client cannot rely on. Fix: the ListScheduledQueries input validation's enum table. Traces: logs-list-scheduled-queries-schedule-type-values#task#3.

Cause 7 of 7: ListScheduledQueries refuses scheduleType=SCHEDULED. AWS: 200 with `{"scheduledQueries": []}`. Emulator: 400. Why: listing scheduled queries by that schedule type is impossible. Fix: as cause 6. Traces: logs-list-scheduled-queries-schedule-type-values#task#4.

## Evidence
list-integrations-status-filter-enum#task#5 (400/400): expected $.message enum set [CREATE_FAILED, ROLLBACK, ACTIVE, PROVISIONING], actual [ACTIVE, FAILED, PROVISIONING].
list-integrations-status-filter-enum#task#3: expected 200 with integrationSummaries=[] and integrations=[], actual 400.
list-integrations-status-filter-enum#task#4: expected 400 InvalidParameterException with the enum message, actual 200 (no __type, no message).
create-log-anomaly-detector-frequency-outside-enum-refused#task#5: expected 400 InvalidParameterException for evaluationFrequency=ONE_MIN, actual 200; #task#6 then lists {evaluationFrequency: ONE_MIN, anomalyDetectorStatus: TRAINING} where AWS lists nothing.
logs-list-scheduled-queries-schedule-type-values#task#3: expected set [SCHEDULED, AWS_MANAGED, CUSTOMER_MANAGED], actual [AWS_MANAGED, CUSTOMER_MANAGED]; #task#4: expected 200 scheduledQueries=[], actual 400.

## Steps to Reproduce
1. Call ListIntegrations with integrationStatus=CREATE_FAILED: AWS answers 200 with empty lists, the emulator answers 400 (fixture list-integrations-status-filter-enum#task#3).
2. Call ListIntegrations with a value outside AWS's set: AWS answers 400 InvalidParameterException, the emulator answers 200 (#task#4), and the refusal it does emit names the wrong value set (#task#5).
3. Call CreateLogAnomalyDetector with evaluationFrequency=ONE_MIN: AWS answers 400, the emulator answers 200 and the detector then appears in ListLogAnomalyDetectors (create-log-anomaly-detector-frequency-outside-enum-refused#task#5, #task#6).
4. Call ListScheduledQueries with scheduleType=SCHEDULED: AWS answers 200 with an empty page, the emulator answers 400 (logs-list-scheduled-queries-schedule-type-values#task#4).
