Issue-Source: trace-sweep-divergence
Issue-Trace: logs-delivery-class-group-refuses-customer-streams-and-writes
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-delivery-class-group-refuses-customer-streams-and-writes logs-export-task-destination-length-bound logs-get-log-fields-data-source-parameter-refusals logs-get-log-group-fields-identifier-arn-forms
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateLogStream|4xx->2xx|$
Issue-Key: logs|CreateLogStream|4xx->2xx|status
Issue-Key: logs|PutLogEvents|4xx->2xx|$.__type
Issue-Key: logs|PutLogEvents|4xx->2xx|$.message
Issue-Key: logs|PutLogEvents|4xx->2xx|status
Issue-Key: logs|DescribeLogStreams|2xx->2xx|$.logStreams[*]
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].retentionInDays
Issue-Key: logs|CreateExportTask|4xx->2xx|$.__type
Issue-Key: logs|CreateExportTask|4xx->2xx|$.message
Issue-Key: logs|CreateExportTask|4xx->2xx|status
Issue-Key: logs|GetLogFields|4xx->2xx|$.__type
Issue-Key: logs|GetLogFields|4xx->2xx|$.message
Issue-Key: logs|GetLogFields|4xx->2xx|status
Issue-Key: logs|GetLogGroupFields|4xx->2xx|$.__type
Issue-Key: logs|GetLogGroupFields|4xx->2xx|$.message
Issue-Key: logs|GetLogGroupFields|4xx->2xx|status
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs DELIVERY-class log groups accept customer stream writes and three further operation gates are unenforced

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLogStream

## Description
This issue bundles 7 causes that share ONE fault: operation-level preconditions that depend on the state of the referenced resource (the log group's class, the target S3 bucket, the data source, the ARN's region) are never evaluated, so the emulator serves success where AWS refuses — and the DELIVERY-class log group it creates is also projected without its retention.

Cause 1 of 7: CreateLogStream against a DELIVERY-class log group. AWS: 400 InvalidParameterException, message `This operation is only supported for AWS Services` (fixed text, no variable part) — a log group whose logGroupClass is DELIVERY accepts only service-vended delivery, not customer-created streams. Emulator: 200 with an empty body, creating the stream. Why: a client can write into a DELIVERY group here and cannot on AWS, so code validated against the emulator fails in production. Fix: CreateLogStream's handler must gate on the resolved log group's logGroupClass. Traces: logs-delivery-class-group-refuses-customer-streams-and-writes#task#4.

Cause 2 of 7: PutLogEvents against a DELIVERY-class log group. AWS: 400 InvalidParameterException, same fixed message `This operation is only supported for AWS Services`. Emulator: 200, ingesting the events. Why: as cause 1, and the ingested events are then visible to queries. Fix: the same logGroupClass gate in PutLogEvents. Traces: logs-delivery-class-group-refuses-customer-streams-and-writes#task#5.

Cause 3 of 7: the stream created by cause 1 is then listed. AWS: DescribeLogStreams returns no stream for the DELIVERY group. Emulator: returns a full logStreams[0] with creationTime, first/lastEventTimestamp, lastIngestionTime, uploadSequenceToken, arn and storedBytes. Why: state diverges, so an assertion that the group holds no customer stream passes on AWS and fails here. Fix: follows from causes 1 and 2. Traces: logs-delivery-class-group-refuses-customer-streams-and-writes#task#6.

Cause 4 of 7: DescribeLogGroups omits retentionInDays for the DELIVERY-class group. AWS: the group is projected with `retentionInDays: 2` — the retention the group was created with is always echoed. Emulator: omits the member. Why: a client reading retention back gets nothing and cannot tell "never set" from "set to 2". Fix: the DescribeLogGroups projection must emit retentionInDays whenever the stored group has one. Traces: logs-delivery-class-group-refuses-customer-streams-and-writes#task#3.

Cause 5 of 7: CreateExportTask to a bucket that does not exist. AWS: 400 InvalidParameterException, message `The given bucket does not exist. Please make sure the bucket is valid.` (fixed text). Emulator: 200, creating the export task. Why: an export that can never deliver is reported as started, so a client waits on a task AWS would have refused outright. Fix: CreateExportTask must resolve `destination` against S3 before accepting. Traces: logs-export-task-destination-length-bound#task#4.

Cause 6 of 7: GetLogFields with data-source parameters that do not resolve. AWS: 400 InvalidParameterException, message `Invalid data source parameters` (fixed text). Emulator: 200. Why: a malformed data-source selection is answered as a successful field listing. Fix: GetLogFields' data-source parameter validation. Traces: logs-get-log-fields-data-source-parameter-refusals#task#3 (also task#5).

Cause 7 of 7: GetLogGroupFields with a log-group ARN from another region. AWS: 400 ValidationException, message `Invalid LogGroup ARN. Cross-region requests are not supported. Region must be: <the endpoint's region>` — fixed text with the serving region as its only variable part. Emulator: 200. Why: a cross-region ARN is silently served as if it were local, hiding a real client bug. Fix: GetLogGroupFields' logGroupIdentifier ARN parsing must compare the ARN's region with the request's region. Traces: logs-get-log-group-fields-identifier-arn-forms#task#7.

## Evidence
logs-delivery-class-group-refuses-customer-streams-and-writes#task#4: expected 400 InvalidParameterException "This operation is only supported for AWS Services", actual 200 {}.
#task#5 (PutLogEvents): expected the same 400 refusal, actual 200.
#task#6 (DescribeLogStreams): expected no logStreams entry, actual {'logStreamName': 'app', 'creationTime': ..., 'uploadSequenceToken': ..., 'arn': 'arn:aws:logs:...:log-group:<group>:log-stream:app', 'storedBytes': 0}.
#task#3 (DescribeLogGroups): expected logGroups[0].retentionInDays=2, actual member missing.
logs-export-task-destination-length-bound#task#4: expected 400 InvalidParameterException "The given bucket does not exist. Please make sure the bucket is valid.", actual 200.
logs-get-log-fields-data-source-parameter-refusals#task#3: expected 400 InvalidParameterException "Invalid data source parameters", actual 200.
logs-get-log-group-fields-identifier-arn-forms#task#7: expected 400 ValidationException "Invalid LogGroup ARN. Cross-region requests are not supported. Region must be: us-west-2", actual 200.

## Steps to Reproduce
1. Create a log group with logGroupClass=DELIVERY, then call CreateLogStream and PutLogEvents against it (fixture logs-delivery-class-group-refuses-customer-streams-and-writes, steps task#4 and task#5): both are accepted 200 instead of refused with "This operation is only supported for AWS Services".
2. Call DescribeLogStreams on that group (#task#6): the emulator lists the stream AWS never created; DescribeLogGroups (#task#3) omits retentionInDays.
3. Call CreateExportTask with a destination bucket that does not exist (logs-export-task-destination-length-bound#task#4): accepted instead of refused.
4. Call GetLogFields with unresolvable data-source parameters (logs-get-log-fields-data-source-parameter-refusals#task#3) and GetLogGroupFields with an out-of-region log-group ARN (logs-get-log-group-fields-identifier-arn-forms#task#7): both accepted instead of refused.
