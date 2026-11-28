Issue-Source: trace-sweep-divergence
Issue-Trace: logs-aggregate-summaries-account-identifier-length
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-aggregate-summaries-account-identifier-length logs-aggregate-summaries-name-pattern-length-bounds logs-create-delivery-record-field-length-bounds logs-create-delivery-tag-value-pattern logs-delivery-tag-value-length-bound logs-delivery-source-configuration-value-length-bounds logsdslogtype-delivery-source-log-type-length-bound logs-describe-deliveries-source-name-charset logs-describe-delivery-destinations-names-pattern logs-untag-resource-tag-key-length-bounds logs-list-log-groups-field-index-name-grammar logs-list-log-groups-tag-filter-value-length-bounds logs-list-log-groups-tag-value-wildcard-grammar logscfgtplpat-configuration-templates-log-type-pattern logs-anomaly-detector-list-filter-arn-wildcard-suffix-refused logs-list-log-anomaly-detectors-filter-arn-pattern-invalid logs-put-subscription-filter-empty-destination-arn
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|ListAggregateLogGroupSummaries|4xx->4xx|$.message
Issue-Key: logs|CreateDelivery|4xx->4xx|$.message
Issue-Key: logs|PutDeliverySource|4xx->4xx|$.message
Issue-Key: logs|DescribeDeliveries|4xx->4xx|$.message
Issue-Key: logs|DescribeDeliveryDestinations|4xx->4xx|$.message
Issue-Key: logs|UntagResource|4xx->4xx|$.message
Issue-Key: logs|ListLogGroups|4xx->4xx|$.message
Issue-Key: logs|DescribeConfigurationTemplates|4xx->4xx|$.message
Issue-Key: logs|ListLogAnomalyDetectors|4xx->4xx|$.message
Issue-Key: logs|PutSubscriptionFilter|4xx->4xx|$.message
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs collection and null constraint failures are rendered with per-element paths instead of AWS's aggregated error

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAggregateLogGroupSummaries

## Description
This issue bundles 10 causes that share ONE fault: the emulator's constraint-violation message formatter is not AWS's. When the violated constraint sits on a member of a list or on a map key/value, AWS reports ONE error against the COLLECTION member and nests every constraint of the element shape inside `Member must satisfy constraint: [ ... ]`; the emulator instead walks the collection, reports the offending element by index as `<member>.<n>.member` (or `<member>.<key>`), and emits one error per violated constraint, so the error count in the prefix also diverges. The same formatter also drops AWS's rejected-value segment and has no form for a missing required member.

Message anatomy — fixed text: `<n> validation error(s) detected: Value [<value>] at '<member>' failed to satisfy constraint: <constraint sentence>`, where for a collection member the constraint sentence is `Member must satisfy constraint: [<comma-separated element constraints>]`. Variable parts: the member path and the rejected value come from the request; the bounds, patterns and enum sets come from the model. Nothing per-run appears.

Cause 1 of 10: ListAggregateLogGroupSummaries.accountIdentifiers. AWS: Smithy AccountId @length 12..12 and @pattern ^\d{12}$, reported as `1 validation error detected: Value at 'accountIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: ^\d{12}$]`. Emulator: `2 validation errors detected: Value at 'accountIdentifiers.1.member' ... length greater than or equal to 12; Value at 'accountIdentifiers.1.member' ... pattern ^\d{12}$`. Why: a client parsing the refusal cannot find the member it submitted and sees a different error count. Fix: the collection arm of the shared validation-message renderer. Traces: logs-aggregate-summaries-account-identifier-length#task#5 (also task#6, logs-aggregate-summaries-name-pattern-length-bounds#task#5).

Cause 2 of 10: CreateDelivery.recordFields. AWS: `1 validation error detected: Value at 'recordFields' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 64, Member must have length greater than or equal to 1]`. Emulator: `... Value at 'recordFields.30.member' ... length less than or equal to 64`. Why/Fix: as cause 1. Traces: logs-create-delivery-record-field-length-bounds#task#5 (also task#6, logs-create-delivery-tag-value-pattern#task#5, logs-delivery-tag-value-length-bound#task#5).

Cause 3 of 10: PutDeliverySource.deliverySourceConfiguration map values. AWS: `1 validation error detected: Value at 'deliverySourceConfiguration' failed to satisfy constraint: Map value must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 1]`. Emulator: `... Value at 'deliverySourceConfiguration.samplingRate' ... length less than or equal to 255` — it names the map KEY from the request instead of using AWS's `Map value must satisfy constraint` form. Why/Fix: as cause 1, map arm. Traces: logs-delivery-source-configuration-value-length-bounds#task#2 (also task#3, logsdslogtype-delivery-source-log-type-length-bound#task#3).

Cause 4 of 10: DescribeDeliveries.deliverySourceNames. AWS: aggregated `[Member must have length less than or equal to 60, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\w-]*]` against 'deliverySourceNames'. Emulator: single pattern error against 'deliverySourceNames.1.member'. Why/Fix: as cause 1. Traces: logs-describe-deliveries-source-name-charset#task#2 (also task#3).

Cause 5 of 10: DescribeDeliveryDestinations.names. Same divergence against 'names' (Smithy DeliverySourceName @length 1..60, @pattern ^[\w-]*$). Emulator: 'names.1.member' with only the pattern error. Why/Fix: as cause 1. Traces: logs-describe-delivery-destinations-names-pattern#task#4 (also task#5).

Cause 6 of 10: UntagResource.tagKeys. AWS: aggregated `[Member must have length less than or equal to 128, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$]` against 'tagKeys' (Smithy TagKey). Emulator: 'tagKeys.1.member' with only the max-length error. Why/Fix: as cause 1. Traces: logs-untag-resource-tag-key-length-bounds#task#3 (also task#4).

Cause 7 of 10: ListLogGroups.fieldIndexNames. AWS: aggregated `[Member must have length less than or equal to 512, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\p{L}\p{M}\p{N}\p{P}\p{S}\p{Zs}]+]` against 'fieldIndexNames' (recorded behavior; the live character class is wider than the checked-in FieldIndexName pattern ^[\.\-_/#A-Za-z0-9]+$). Emulator: two errors against 'fieldIndexNames.1.member' quoting the narrow anchored pattern. Why/Fix: as cause 1, plus the element pattern itself. Traces: logs-list-log-groups-field-index-name-grammar#task#9 (also logs-list-log-groups-tag-filter-value-length-bounds#task#3, logs-list-log-groups-tag-value-wildcard-grammar#task#2, #task#3).

Cause 8 of 10: DescribeConfigurationTemplates.logTypes. AWS: aggregated `[Member must have length less than or equal to 255, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\w]*]` against 'logTypes' (Smithy LogType). Emulator: 'logTypes.1.member' with the anchored pattern ^[\w]*$. Why/Fix: as cause 1. Traces: logscfgtplpat-configuration-templates-log-type-pattern#task#1 (also task#3).

Cause 9 of 10: ListLogAnomalyDetectors.filterLogGroupArn drops the rejected value. AWS: `1 validation error detected: Value '<the submitted ARN>' at 'filterLogGroupArn' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w#+=/:,.@-]*` — the `Value '<x>' at` segment is part of the sentence, with `<x>` taken from the request. Emulator: omits it entirely (`Value at 'filterLogGroupArn' ...`). Why: the caller loses the echo that tells it which value was rejected. Fix: the renderer must include the value segment for scalar string violations. Traces: logs-anomaly-detector-list-filter-arn-wildcard-suffix-refused#task#4 (also logs-list-log-anomaly-detectors-filter-arn-pattern-invalid#task#6).

Cause 10 of 10: PutSubscriptionFilter with a null destinationArn. AWS: `1 validation error detected: Value null at 'destinationArn' failed to satisfy constraint: Member must not be null` — a required-member violation has its own fixed sentence. Emulator: coerces the absent member to an empty string and reports `Member must have length greater than or equal to 1`. Why: a client cannot distinguish an omitted required member from an empty one. Fix: the renderer needs a null/required arm, applied before the @length arm. Traces: logs-put-subscription-filter-empty-destination-arn#task#2.

## Evidence
Representative (logs-aggregate-summaries-account-identifier-length#task#5, 400/400):
Expected $.message: "1 validation error detected: Value at 'accountIdentifiers' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: ^\\d{12}$]"
Actual   $.message: "2 validation errors detected: Value at 'accountIdentifiers.1.member' failed to satisfy constraint: Member must have length greater than or equal to 12; Value at 'accountIdentifiers.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^\\d{12}$"
Same shape on recordFields, deliverySourceConfiguration (map), deliverySourceNames, names, tagKeys, fieldIndexNames and logTypes.
Value segment dropped: expected "Value 'arn:aws:logs:...:log-group:...:*' at 'filterLogGroupArn' ...", actual "Value at 'filterLogGroupArn' ...".
Null member: expected "Value null at 'destinationArn' failed to satisfy constraint: Member must not be null", actual "... Member must have length greater than or equal to 1".

## Steps to Reproduce
1. Replay logs-aggregate-summaries-account-identifier-length and compare step task#5's $.message with the recording: the emulator reports per-element paths and two errors where AWS reports one aggregated error against the collection member.
2. Repeat for logs-create-delivery-record-field-length-bounds#task#5, logs-delivery-source-configuration-value-length-bounds#task#2, logs-describe-deliveries-source-name-charset#task#2, logs-describe-delivery-destinations-names-pattern#task#4, logs-untag-resource-tag-key-length-bounds#task#3, logs-list-log-groups-field-index-name-grammar#task#9 and logscfgtplpat-configuration-templates-log-type-pattern#task#1.
3. Replay logs-anomaly-detector-list-filter-arn-wildcard-suffix-refused#task#4 — the Value '<x>' at segment is missing.
4. Replay logs-put-subscription-filter-empty-destination-arn#task#2 — a null required member is reported as a length violation.
