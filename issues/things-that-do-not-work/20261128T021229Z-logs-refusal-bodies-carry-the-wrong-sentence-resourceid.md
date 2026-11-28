Issue-Source: trace-sweep-divergence
Issue-Trace: logs-delete-delivery-id-length-bounds
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-delete-delivery-id-length-bounds logs-delete-delivery-destination-policy-name-length-bounds logs-delete-delivery-source-name-length-bounds delete-scheduled-query-identifier-length-bounds logs-query-id-length-bounds-for-log-group-listing logs-insights-start-query-and-describe-workflow logs-log-group-query-discovery logs-osdocstat-traj-insights-query-audit-group logs-osdocstat-traj-log-group-arn-forms
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DeleteDelivery|4xx->4xx|$.message
Issue-Key: logs|DeleteDeliveryDestinationPolicy|4xx->4xx|$.message
Issue-Key: logs|DeleteDeliverySource|4xx->4xx|$.message
Issue-Key: logs|DeleteScheduledQuery|4xx->4xx|$.message
Issue-Key: logs|ListLogGroupsForQuery|4xx->4xx|$.message
Issue-Key: logs|StartQuery|4xx->4xx|$.message
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs refusal bodies carry the wrong sentence: resourceId pre-flights, member names, empty and SDK-wrapped messages

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDelivery

## Description
This issue bundles 6 causes that share ONE fault: the refusal BODY the emulator emits is not the sentence CloudWatch Logs emits for that operation — it leaks the generic Smithy constraint text where AWS has a short operation-specific sentence, names the wrong member, emits an empty message, or wraps the message in the AWS SDK's client-side diagnostic envelope. Status is 400 on both sides throughout.

Cause 1 of 3 identifier pre-flights (cause 1 of 6): DeleteDelivery with an empty id. AWS: 400 with message `Must specify a valid resourceId. ` — fixed text, including its trailing space, and no variable part. Emulator: `2 validation errors detected: Value at 'id' failed to satisfy constraint: Member must have length greater than or equal to 1; Value at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9A-Za-z]+$`. Why: a client matching the documented refusal never matches, and the emulator also leaks the anchored model pattern. Fix: DeleteDelivery's identifier pre-flight must emit AWS's resourceId sentence instead of falling through to the generic constraint renderer. Traces: logs-delete-delivery-id-length-bounds#task#3.

Cause 2 of 6: DeleteDeliveryDestinationPolicy with an empty deliveryDestinationName. AWS: the same fixed `Must specify a valid resourceId. `. Emulator: the generic `... 'deliveryDestinationName' ... length greater than or equal to 1`. Why/Fix: as cause 1. Traces: logs-delete-delivery-destination-policy-name-length-bounds#task#5.

Cause 3 of 6: DeleteDeliverySource with an empty name. AWS: the same fixed `Must specify a valid resourceId. `. Emulator: the generic `... 'name' ... length greater than or equal to 1`. Why/Fix: as cause 1. Traces: logs-delete-delivery-source-name-length-bounds#task#3.

Cause 4 of 6: DeleteScheduledQuery names the wrong member. AWS: `1 validation error detected: Value at 'scheduledQueryName' failed to satisfy constraint: Member must have length less than or equal to 300` — the member path in the sentence is the operation's own input member name. Emulator: names `identifier`, its internal member. Why: a client cannot map the refusal back to the field it set. Fix: DeleteScheduledQuery's validation must report the wire member name. Traces: delete-scheduled-query-identifier-length-bounds#task#2 (also task#3).

Cause 5 of 6: ListLogGroupsForQuery emits an empty message member. AWS: the refusal body carries NO `message` member at all for this refusal. Emulator: emits `"message": ""`. Why: a client that tests for the presence of a message, or logs it, sees an empty string where AWS sends nothing. Fix: the error serializer must omit `message` rather than emit an empty one. Traces: logs-query-id-length-bounds-for-log-group-listing#task#4 (also task#5, task#6).

Cause 6 of 6: StartQuery's MalformedQueryException message carries the SDK's diagnostic envelope. AWS: the wire message is the query-parser sentence alone (recorded behavior: the recorded value is shape-redacted, but it is a bare parser message). Emulator: `unexpected symbol found abc at line 1 and position 24 (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: <uuid>; Proxy: null)` — a string the AWS SDK composes CLIENT-side from the response, never something a service puts on the wire. Why: a client that re-parses or displays the message sees nested, duplicated error metadata, and any client that logs it records a fabricated request id. The per-run request id and the parser's position values are variable; the defect is the fixed `(Service: ...; Status Code: ...; Error Code: ...; Request ID: ...; Proxy: null)` suffix, which must not be present at all. Fix: StartQuery's MalformedQueryException serializer must put only the parser sentence in `message`. Traces: logs-insights-start-query-and-describe-workflow#task#4 (also logs-log-group-query-discovery#task#6, logs-osdocstat-traj-insights-query-audit-group#task#7, logs-osdocstat-traj-log-group-arn-forms#task#7).

## Evidence
logs-delete-delivery-id-length-bounds#task#3 (400/400): expected $.message 'Must specify a valid resourceId. ', actual "2 validation errors detected: Value at 'id' ... greater than or equal to 1; Value at 'id' ... pattern: ^[0-9A-Za-z]+$".
logs-delete-delivery-destination-policy-name-length-bounds#task#5 and logs-delete-delivery-source-name-length-bounds#task#3: same expected sentence, actual generic length messages on 'deliveryDestinationName' / 'name'.
delete-scheduled-query-identifier-length-bounds#task#2: expected member 'scheduledQueryName', actual 'identifier'.
logs-query-id-length-bounds-for-log-group-listing#task#4: expected $.message absent, actual ''.
logs-insights-start-query-and-describe-workflow#task#4: actual $.message 'unexpected symbol found abc at line 1 and position 24 (Service: AWSLogs; Status Code: 400; Error Code: MalformedQueryException; Request ID: ...; Proxy: null)' where the recorded wire message carries no SDK envelope.

## Steps to Reproduce
1. Call DeleteDelivery with id='', DeleteDeliveryDestinationPolicy with deliveryDestinationName='' and DeleteDeliverySource with name='' (fixtures logs-delete-delivery-id-length-bounds#task#3, logs-delete-delivery-destination-policy-name-length-bounds#task#5, logs-delete-delivery-source-name-length-bounds#task#3): each answers the generic Smithy constraint sentence instead of 'Must specify a valid resourceId. '.
2. Call DeleteScheduledQuery with an over-long name (delete-scheduled-query-identifier-length-bounds#task#2): the message names 'identifier' instead of 'scheduledQueryName'.
3. Call ListLogGroupsForQuery with an out-of-bounds queryId (logs-query-id-length-bounds-for-log-group-listing#task#4): the body carries an empty message member.
4. Call StartQuery with a malformed query string (logs-insights-start-query-and-describe-workflow#task#4): the message is wrapped in the SDK's '(Service: ...; Request ID: ...; Proxy: null)' envelope.
