Issue-Source: trace-sweep-divergence
Issue-Trace: logs-create-delivery-record-field-length-bounds
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-create-delivery-record-field-length-bounds logs-delivery-destination-policy-length-bounds logs-delivery-source-configuration-value-length-bounds logs-delivery-source-s3-configuration-refused-key-over-length logs-describe-configuration-templates-log-types-count-bounds logs-create-scheduled-query-lookup-table-tag-key-length logs-scheduled-query-schedule-start-time-lower-bound logstagkey-create-log-group-tag-key-length-bounds logs-untag-resource-empty-key-list-rejected logs-describe-log-groups-account-identifier-count-bounds
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateDelivery|4xx->2xx|$.__type
Issue-Key: logs|CreateDelivery|4xx->2xx|$.message
Issue-Key: logs|CreateDelivery|4xx->2xx|status
Issue-Key: logs|CreateDelivery|2xx->2xx|$.delivery.recordFields[*]
Issue-Key: logs|PutDeliveryDestinationPolicy|4xx->2xx|$.__type
Issue-Key: logs|PutDeliveryDestinationPolicy|4xx->2xx|$.message
Issue-Key: logs|PutDeliveryDestinationPolicy|4xx->2xx|status
Issue-Key: logs|PutDeliverySource|4xx->2xx|$.__type
Issue-Key: logs|PutDeliverySource|4xx->2xx|$.message
Issue-Key: logs|PutDeliverySource|4xx->2xx|status
Issue-Key: logs|GetDeliverySource|4xx->2xx|$.__type
Issue-Key: logs|GetDeliverySource|4xx->2xx|$.message
Issue-Key: logs|GetDeliverySource|4xx->2xx|status
Issue-Key: logs|DescribeConfigurationTemplates|4xx->2xx|$.__type
Issue-Key: logs|DescribeConfigurationTemplates|4xx->2xx|$.message
Issue-Key: logs|DescribeConfigurationTemplates|4xx->2xx|status
Issue-Key: logs|CreateScheduledQuery|4xx->2xx|$.__type
Issue-Key: logs|CreateScheduledQuery|4xx->2xx|$.message
Issue-Key: logs|CreateScheduledQuery|4xx->2xx|status
Issue-Key: logs|CreateLogGroup|4xx->2xx|$
Issue-Key: logs|CreateLogGroup|4xx->2xx|status
Issue-Key: logs|UntagResource|4xx->2xx|$
Issue-Key: logs|UntagResource|4xx->2xx|status
Issue-Key: logs|DescribeLogGroups|4xx->2xx|$.__type
Issue-Key: logs|DescribeLogGroups|4xx->2xx|$.message
Issue-Key: logs|DescribeLogGroups|4xx->2xx|status
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs skips ten request-validation refusals, answering 200 where AWS returns 400

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
This issue bundles 10 causes that share ONE fault: pre-flight checks CloudWatch Logs performs are missing from the emulator's handlers, so requests AWS refuses 400 are accepted 200 — and in two cases the accepted request also poisons the state that later steps read.

Cause 1 of 10: CreateDelivery with record fields that are not valid for the delivery's source/destination pair. AWS: 400 ValidationException, message `Provided invalid record fields` (fixed text, no variable part). Emulator: 200, creating the delivery. Why: a client believes a delivery was configured with fields AWS would have rejected, and the delivery it gets back is not one AWS can produce. Fix: CreateDelivery must validate recordFields against the configuration template for the source's logType before creating. Traces: logs-create-delivery-record-field-length-bounds#task#7.

Cause 2 of 10: the delivery created by cause 1 echoes the rejected field. AWS: the delivery does not exist, so no recordFields entry exists. Emulator: `delivery.recordFields[29]` carries the 64-character field name. Why: downstream reads diverge. Fix: follows from cause 1. Traces: logs-create-delivery-record-field-length-bounds#task#8.

Cause 3 of 10: PutDeliveryDestinationPolicy with an over-long policy document. AWS: 400 ValidationException, message `Delivery Destination Policy is too long.` (fixed text). Emulator: 200. Why: a policy larger than the service's ceiling is stored, so the emulator accepts configurations AWS cannot hold. Fix: PutDeliveryDestinationPolicy's deliveryDestinationPolicy size check. Traces: logs-delivery-destination-policy-length-bounds#task#4.

Cause 4 of 10: PutDeliverySource supplying deliverySourceConfiguration. AWS: 400 ValidationException, message `No delivery source configuration allowed` (fixed text) — the member is modelled but not accepted on this path. Emulator: 200, creating the source. Why: a caller believes a sampling/source configuration was applied. Fix: PutDeliverySource's handler must refuse a non-empty deliverySourceConfiguration. Traces: logs-delivery-source-configuration-value-length-bounds#task#4 (also logs-delivery-source-s3-configuration-refused-key-over-length#task#2, #task#3).

Cause 5 of 10: GetDeliverySource for the source cause 4 should never have created. AWS: 400 ResourceNotFoundException, message `Requested Delivery Source does not exist in this account.` (fixed text). Emulator: 200. Why: the absence check a client relies on reports presence. Fix: follows from cause 4. Traces: logs-delivery-source-configuration-value-length-bounds#task#5.

Cause 6 of 10: DescribeConfigurationTemplates with logTypes but no service. AWS: 400 ValidationException, message `Service is required when logTypes is provided` (fixed text). Emulator: 200, serving a catalogue. Why: a client's under-specified filter silently returns the wrong catalogue instead of being corrected. Fix: DescribeConfigurationTemplates' cross-member requirement check. Traces: logs-describe-configuration-templates-log-types-count-bounds#task#1.

Cause 7 of 10: CreateScheduledQuery with an out-of-bounds lookup-table tag key. AWS: 400 ValidationException naming `destinationConfiguration.lookupTableConfiguration.tags` with the aggregated TagKey constraint set (@length 1..128, @pattern ^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$). Emulator: 200. Why: tags AWS refuses are attached to the lookup table. Fix: CreateScheduledQuery must validate the nested lookup-table tag map. Traces: logs-create-scheduled-query-lookup-table-tag-key-length#task#3.

Cause 8 of 10: CreateLogGroup with an out-of-bounds tag key. AWS: 400 InvalidParameterException naming `tags` with `Map keys must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$]` (Smithy TagKey). Emulator: 200 with an empty body. Why: the log group is created with tag keys AWS rejects, so tag-based discovery diverges. Fix: CreateLogGroup's tag-map validation. Traces: logstagkey-create-log-group-tag-key-length-bounds#task#1 (also task#2).

Cause 9 of 10: UntagResource with an empty tagKeys list. AWS: 400 InvalidParameterException, message `No tag keys provided.` (fixed text). Emulator: 200 with an empty body. Why: a client's no-op mistake is silently accepted. Fix: UntagResource's tagKeys emptiness check. Traces: logs-untag-resource-empty-key-list-rejected#task#2.

Cause 10 of 10: DescribeLogGroups with duplicate accountIdentifiers. AWS: 400 InvalidParameterException, message `Account identifiers provided in request must be unique!` (fixed text). Emulator: 200. Why: a cross-account listing with a duplicated account is answered instead of corrected, so result counts differ from AWS. Fix: DescribeLogGroups' accountIdentifiers uniqueness check. Traces: logs-describe-log-groups-account-identifier-count-bounds#task#3.

## Evidence
logs-create-delivery-record-field-length-bounds#task#7: expected 400 ValidationException "Provided invalid record fields", actual 200; #task#8 then echoes delivery.recordFields[29]='ffff...' (64 chars) that should not exist.
logs-delivery-destination-policy-length-bounds#task#4: expected 400 ValidationException "Delivery Destination Policy is too long.", actual 200.
logs-delivery-source-configuration-value-length-bounds#task#4: expected 400 ValidationException "No delivery source configuration allowed", actual 200; #task#5: expected 400 ResourceNotFoundException "Requested Delivery Source does not exist in this account.", actual 200.
logs-describe-configuration-templates-log-types-count-bounds#task#1: expected 400 ValidationException "Service is required when logTypes is provided", actual 200.
logs-create-scheduled-query-lookup-table-tag-key-length#task#3: expected 400 ValidationException on destinationConfiguration.lookupTableConfiguration.tags, actual 200.
logstagkey-create-log-group-tag-key-length-bounds#task#1: expected 400 InvalidParameterException on 'tags' map keys, actual 200 {}.
logs-untag-resource-empty-key-list-rejected#task#2: expected 400 InvalidParameterException "No tag keys provided.", actual 200 {}.
logs-describe-log-groups-account-identifier-count-bounds#task#3: expected 400 InvalidParameterException "Account identifiers provided in request must be unique!", actual 200.

## Steps to Reproduce
1. Replay logs-create-delivery-record-field-length-bounds steps task#7 and task#8: the invalid record-field set is accepted and then echoed.
2. Replay logs-delivery-destination-policy-length-bounds#task#4, logs-describe-configuration-templates-log-types-count-bounds#task#1, logs-create-scheduled-query-lookup-table-tag-key-length#task#3, logstagkey-create-log-group-tag-key-length-bounds#task#1, logs-untag-resource-empty-key-list-rejected#task#2 and logs-describe-log-groups-account-identifier-count-bounds#task#3: each answers 200 where the recording is a 400 refusal with the quoted message.
3. Replay logs-delivery-source-configuration-value-length-bounds steps task#4 and task#5: PutDeliverySource accepts a deliverySourceConfiguration AWS refuses, and the resulting source is then found by GetDeliverySource where AWS answers ResourceNotFoundException.
