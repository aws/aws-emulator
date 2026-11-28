Issue-Source: trace-sweep-divergence
Issue-Trace: logs-create-delivery-suffix-path-length-bounds
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-create-delivery-suffix-path-length-bounds logs-create-delivery-tags-count-bounds logs-delivery-tag-value-length-bound logs-describe-deliveries-source-names-count-bounds logs-create-delivery-tag-value-pattern logs-create-delivery-record-field-length-bounds logs-delivery-source-configuration-value-length-bounds logs-delivery-source-s3-configuration-refused-key-over-length
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|GetDelivery|2xx->2xx|$.delivery.recordFields[*]
Issue-Key: logs|CreateDelivery|2xx->2xx|$.delivery.s3DeliveryConfiguration.suffixPath
Issue-Key: logs|GetDelivery|2xx->2xx|$.delivery.s3DeliveryConfiguration.suffixPath
Issue-Key: logs|PutDeliverySource|2xx->2xx|$.deliverySource.deliverySourceConfiguration
Issue-Key: logs|GetDeliverySource|2xx->2xx|$.deliverySource.deliverySourceConfiguration
Issue-Key: logs|GetDeliverySource|2xx->2xx|$.deliverySource.status
Issue-Related: 20261128T021229Z-logs-skips-ten-request-validation-refusals-answering-200.md
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs delivery records are built from an emulator-authored default recordFields catalog and drop deliverySourceConfiguration

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDelivery

## Description
This issue bundles 7 causes that share ONE fault: the vended-log delivery plane builds its records from emulator-authored defaults instead of the service's configuration template, so the delivery a caller gets back — and reads back — carries the wrong record-field vocabulary and the wrong S3 suffix path, and the delivery source omits a member AWS always emits.

Cause 1 of 7: CreateDelivery's default recordFields for an S3 delivery. AWS: when recordFields is not supplied the delivery is created with the configuration template's `defaultDeliveryConfigValues.recordFields` for the source's service/logType — the snake_case vended-log vocabulary (`schema_version_id`, `bucket_arn`, `bucket_name`, `request_time`, `bucket_owner_id`, `remote_ip`, `requester`, `request_id`, `operation`, `key_name`, `request_uri`, `http_status`, `error_code`, `bytes_sent_size`, `object_size`, `total_duration`, `turn_around_duration`, `referer`, `user_agent`, `version_id`, `host_id`, `signature_version`, `cipher_suite`, ...). Emulator: substitutes the legacy S3 server-access-log column names (`bucketowner`, `bucket`, `requestdatetime`, `remoteip`, `requester`, `requestid`, `operation`, `key`, `request_uri`, `httpstatus`, `errorcode`, `bytessent`, `objectsize`, `totaltime`, `turnaroundtime`, `referrer`, `useragent`, `versionid`, `hostid`, `sigv`, `ciphersuite`, `authtype`, `endpoint`, ...) — a different, position-shifted vocabulary. Why: a consumer that reads delivered objects by field name, or that round-trips the delivery into UpdateDeliveryConfiguration, references fields that do not exist. Fix: CreateDelivery must take its default recordFields from the configuration-template catalogue rather than a hand-written list. Traces: logs-create-delivery-suffix-path-length-bounds#task#6 (also logs-create-delivery-tags-count-bounds#task#6, logs-delivery-tag-value-length-bound#task#6, logs-describe-deliveries-source-names-count-bounds#setup#9).

Cause 2 of 7: GetDelivery replays the same wrong default set. AWS: the stored delivery's recordFields are the template's. Emulator: the legacy column names. Why/Fix: follows from cause 1, in the GetDelivery projection's source of truth. Traces: logs-create-delivery-suffix-path-length-bounds#task#7.

Cause 3 of 7: the same wrong default set on a second CreateDelivery path. AWS/Emulator: as cause 1, and here the response's `s3DeliveryConfiguration.suffixPath` also diverges. Why/Fix: as cause 1. Traces: logs-create-delivery-tag-value-pattern#task#6.

Cause 4 of 7: GetDelivery on that delivery. Same divergence, including suffixPath. Why/Fix: as cause 2. Traces: logs-create-delivery-tag-value-pattern#task#7.

Cause 5 of 7: GetDelivery's default s3DeliveryConfiguration.suffixPath. AWS: the template's default `{region}/{yyyy}/{MM}/{dd}/{HH}/` for this source type. Emulator: `{region}/{resource_id}/{yyyy}/{MM}/{dd}/{HH}/` — it injects a `{resource_id}` segment the template does not have. Why: delivered objects land at a different prefix than a client computing the path from the delivery record expects. Fix: the default suffixPath must come from the configuration template's defaultDeliveryConfigValues.s3DeliveryConfiguration. Traces: logs-create-delivery-record-field-length-bounds#task#9.

Cause 6 of 7: PutDeliverySource omits deliverySourceConfiguration. AWS: the response's `deliverySource.deliverySourceConfiguration` is always present, as `{}` when the source has none. Emulator: omits the member. Why: a client that reads the map unconditionally gets a null where AWS guarantees an object. Fix: the PutDeliverySource projection must emit the member, empty when unset. Traces: logs-delivery-source-configuration-value-length-bounds#task#6 (also logs-delivery-source-s3-configuration-refused-key-over-length#task#4).

Cause 7 of 7: GetDeliverySource omits deliverySourceConfiguration and status. AWS: emits `deliverySourceConfiguration: {}` and `status: ACTIVE` for a healthy source. Emulator: omits both. Why: a client cannot tell whether the source is active, so readiness polling never completes. Fix: the GetDeliverySource projection must emit both members. Traces: logs-delivery-source-configuration-value-length-bounds#task#7 (also logs-delivery-source-s3-configuration-refused-key-over-length#task#5).

## Evidence
logs-create-delivery-suffix-path-length-bounds#task#6 (200/200): expected delivery.recordFields[0..] = ['schema_version_id','bucket_arn','bucket_name','request_time','bucket_owner_id','remote_ip','requester','request_id','operation','key_name','request_uri','http_status','error_code','bytes_sent_size','object_size','total_duration','turn_around_duration','referer','user_agent','version_id','host_id','signature_version','cipher_suite', ...]; actual ['bucketowner','bucket','requestdatetime','remoteip','requester','requestid','operation','key','request_uri','httpstatus','errorcode','bytessent','objectsize','totaltime','turnaroundtime','referrer','useragent','versionid','hostid','sigv','ciphersuite','authtype','endpoint', ...]. Same on #task#7 (GetDelivery) and on logs-create-delivery-tag-value-pattern#task#6/#task#7.
logs-create-delivery-record-field-length-bounds#task#9: expected delivery.s3DeliveryConfiguration.suffixPath '{region}/{yyyy}/{MM}/{dd}/{HH}/', actual '{region}/{resource_id}/{yyyy}/{MM}/{dd}/{HH}/'.
logs-delivery-source-configuration-value-length-bounds#task#6: expected deliverySource.deliverySourceConfiguration {}, actual missing; #task#7: expected deliverySourceConfiguration {} and status 'ACTIVE', actual both missing.

## Steps to Reproduce
1. Create a delivery source and destination and call CreateDelivery without recordFields (fixture logs-create-delivery-suffix-path-length-bounds#task#6), then GetDelivery (#task#7): the recordFields vocabulary is the legacy S3 access-log column set, not the configuration template's.
2. Repeat on logs-create-delivery-tag-value-pattern#task#6 and #task#7 — same divergence, plus suffixPath.
3. Call GetDelivery on logs-create-delivery-record-field-length-bounds#task#9: the default suffixPath carries an extra {resource_id} segment.
4. Call PutDeliverySource then GetDeliverySource (logs-delivery-source-configuration-value-length-bounds#task#6, #task#7): deliverySourceConfiguration and status are omitted.
