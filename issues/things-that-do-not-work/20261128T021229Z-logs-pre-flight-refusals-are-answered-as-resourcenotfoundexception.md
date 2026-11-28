Issue-Source: trace-sweep-divergence
Issue-Trace: delete-log-anomaly-detector-invalid-arn-refused
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: delete-log-anomaly-detector-invalid-arn-refused logs-delete-log-anomaly-detector-arn-pattern describe-import-task-batches-status-filter-values logs-delete-index-policy-identifier-length-bounds logs-delete-resource-policy-expected-revision-length-floor logs-describe-index-policies-identifier-count-bounds logs-list-log-anomaly-detectors-filter-arn-pattern-invalid logs-associate-kms-key-resource-identifier-length-floor logs-tags-for-resource-arn-length-bounds logs-describe-log-streams-prefix-length-bounds
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DeleteLogAnomalyDetector|4xx->4xx|$.message
Issue-Key: logs|DeleteLogAnomalyDetector|4xx->4xx|$.__type
Issue-Key: logs|DescribeImportTaskBatches|4xx->4xx|$.__type
Issue-Key: logs|DeleteIndexPolicy|4xx->4xx|$.__type
Issue-Key: logs|DeleteIndexPolicy|4xx->4xx|$.message
Issue-Key: logs|DeleteResourcePolicy|4xx->4xx|$.message
Issue-Key: logs|DescribeIndexPolicies|4xx->4xx|$.__type
Issue-Key: logs|DescribeIndexPolicies|4xx->4xx|$.message
Issue-Key: logs|ListLogAnomalyDetectors|4xx->4xx|$.__type
Issue-Key: logs|AssociateKmsKey|4xx->4xx|$.__type
Issue-Key: logs|ListTagsForResource|4xx->4xx|$.__type
Issue-Key: logs|ListTagsForResource|4xx->4xx|$.message
Issue-Key: logs|DescribeLogStreams|4xx->4xx|$.message
Issue-Related: 20261128T021229Z-logs-collection-and-null-constraint-failures-are-rendered.md
Issue-Related: 20261128T021229Z-logs-pattern-refusals-quote-smithy-anchored-regexes-and.md
Issue-Related: 20261128T021229Z-logs-validation-messages-list-enum-value-sets-in.md
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs pre-flight refusals are answered as ResourceNotFoundException with emulator-authored text

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteLogAnomalyDetector

## Description
This issue bundles 10 causes that share ONE fault: for a request that is invalid on its own face, the emulator skips the operation's own pre-flight refusal, resolves the identifier against state, and then reports whatever the lookup says — so the error class and the sentence both diverge. AWS refuses these before resolution, with a short operation-specific message. The status is 400 on both sides in every cause, so only the `__type` and `message` diverge.

Cause 1 of 10: DeleteLogAnomalyDetector with an empty anomalyDetectorArn. AWS: 400 InvalidParameterException, message `Invalid ARN` (fixed text, no variable part). Emulator: the Smithy @length floor message for AnomalyDetectorArn (`... 'anomalyDetectorArn' ... length greater than or equal to 1`). Why: the client gets a constraint sentence AWS never emits for this operation. Fix: DeleteLogAnomalyDetector's ARN pre-flight must raise InvalidParameterException "Invalid ARN" instead of deferring to the generic length check. Traces: delete-log-anomaly-detector-invalid-arn-refused#task#3 (also logs-delete-log-anomaly-detector-arn-pattern#task#3).

Cause 2 of 10: DeleteLogAnomalyDetector with a non-ARN string. AWS: 400 InvalidParameterException `Invalid ARN`. Emulator: 400 ResourceNotFoundException `The specified ARN does not exist or already deleted : <value>`. Why: a malformed ARN is reported as a missing resource, so a client retries or re-creates instead of fixing its input. Fix: as cause 1, applied before resolution. Traces: delete-log-anomaly-detector-invalid-arn-refused#task#4 (also logs-delete-log-anomaly-detector-arn-pattern#task#4).

Cause 3 of 10: DescribeImportTaskBatches with an unsupported BatchStatus filter. AWS: 400 InvalidParameterException, message `BatchStatus must be one of: IN_PROGRESS, COMPLETED, FAILED` (fixed text, with the allowed list as part of the sentence). Emulator: 400 ResourceNotFoundException `No matching import found.` — it resolves the import first. Why: the caller is told the import is missing when the filter is what AWS rejects. Fix: validate the status filter in DescribeImportTaskBatches before resolving importId. Traces: describe-import-task-batches-status-filter-values#task#2.

Cause 4 of 10: DeleteIndexPolicy with an over-long logGroupIdentifier. AWS: 400 InvalidParameterException, message `Invalid LogGroup or LogStream, both must have length less than or equal to 512.` (fixed text). Emulator: 400 ResourceNotFoundException `Log group cannot be found.`. Why/Fix: as cause 2, in DeleteIndexPolicy's identifier pre-flight. Traces: logs-delete-index-policy-identifier-length-bounds#task#6 (also task#7, task#8).

Cause 5 of 10: DeleteResourcePolicy for a resource with no policy. AWS: message `No policy found for ResourceArn` (fixed text). Emulator: `The specified resource policy does not exist.` — an emulator-authored sentence. Why: message-matching clients and docs-driven tests diverge. Fix: DeleteResourcePolicy's not-found sentence. Traces: logs-delete-resource-policy-expected-revision-length-floor#task#6.

Cause 6 of 10: DescribeIndexPolicies with an empty logGroupIdentifiers list. AWS: 400 ValidationException, message `LogGroupIdentifier(s) must be provided.` (fixed text). Emulator: 400 InvalidParameterException with the generic @length floor sentence. Why: both the error class and the sentence are wrong, so a client cannot branch on the class. Fix: DescribeIndexPolicies' required-input pre-flight. Traces: logs-describe-index-policies-identifier-count-bounds#task#7.

Cause 7 of 10: ListLogAnomalyDetectors with a malformed filterLogGroupArn. AWS: 400 InvalidParameterException `Invalid ARN`. Emulator: 400 ResourceNotFoundException `The specified log group does not exist.`. Why/Fix: as cause 2, in ListLogAnomalyDetectors. Traces: logs-list-log-anomaly-detectors-filter-arn-pattern-invalid#task#5.

Cause 8 of 10: AssociateKmsKey with a resourceIdentifier that is not an ARN. AWS: 400 ValidationException, message `Failed to parse <value> to ARN` — fixed text around the value taken from the request. Emulator: 400 InvalidParameterException `resourceIdentifier must be a log group ARN or a query-result ARN, got '<value>'`, an emulator-authored sentence in the wrong error class. Why: the class a client branches on and the text it logs are both wrong. Fix: AssociateKmsKey's ARN parse failure must raise ValidationException with AWS's sentence. Traces: logs-associate-kms-key-resource-identifier-length-floor#task#4.

Cause 9 of 10: ListTagsForResource with an unparseable resourceArn. AWS: 400 ValidationException, message `Invalid resourceArn` (fixed text, value NOT echoed). Emulator: 400 InvalidParameterException `1 validation error detected: Unable to parse resourceArn <value>` — wrong class, authored text, and it echoes the whole rejected value. Why/Fix: as cause 8, in ListTagsForResource. Traces: logs-tags-for-resource-arn-length-bounds#task#3 (also task#4).

Cause 10 of 10: DescribeLogStreams with an over-long logStreamNamePrefix. AWS: 400 with the member-specific constraint sentence `1 validation error detected: Value at 'logStreamNamePrefix' failed to satisfy constraint: Member must have length less than or equal to 512`. Emulator: the generic `Invalid LogGroup or LogStream, both must have length less than or equal to 512.`. Why: the caller cannot tell WHICH member was too long. Fix: DescribeLogStreams must apply the per-member @length check rather than the shared group/stream sentence. Traces: logs-describe-log-streams-prefix-length-bounds#task#6.

## Evidence
delete-log-anomaly-detector-invalid-arn-refused#task#3: expected $.message 'Invalid ARN', actual "1 validation error detected: Value at 'anomalyDetectorArn' failed to satisfy constraint: Member must have length greater than or equal to 1"; #task#4: expected InvalidParameterException/'Invalid ARN', actual ResourceNotFoundException/'The specified ARN does not exist or already deleted : x'.
describe-import-task-batches-status-filter-values#task#2: expected InvalidParameterException/'BatchStatus must be one of: IN_PROGRESS, COMPLETED, FAILED', actual ResourceNotFoundException/'No matching import found.'.
logs-delete-index-policy-identifier-length-bounds#task#6: expected InvalidParameterException/'Invalid LogGroup or LogStream, both must have length less than or equal to 512.', actual ResourceNotFoundException/'Log group cannot be found.'.
logs-delete-resource-policy-expected-revision-length-floor#task#6: expected 'No policy found for ResourceArn', actual 'The specified resource policy does not exist.'.
logs-describe-index-policies-identifier-count-bounds#task#7: expected ValidationException/'LogGroupIdentifier(s) must be provided.', actual InvalidParameterException/length-floor sentence.
logs-list-log-anomaly-detectors-filter-arn-pattern-invalid#task#5: expected InvalidParameterException/'Invalid ARN', actual ResourceNotFoundException/'The specified log group does not exist.'.
logs-associate-kms-key-resource-identifier-length-floor#task#4: expected ValidationException/'Failed to parse a to ARN', actual InvalidParameterException/"resourceIdentifier must be a log group ARN or a query-result ARN, got 'a'".
logs-tags-for-resource-arn-length-bounds#task#3: expected ValidationException/'Invalid resourceArn', actual InvalidParameterException/'1 validation error detected: Unable to parse resourceArn rrrr...'.
logs-describe-log-streams-prefix-length-bounds#task#6: expected "... 'logStreamNamePrefix' ... length less than or equal to 512", actual 'Invalid LogGroup or LogStream, both must have length less than or equal to 512.'.

## Steps to Reproduce
1. Call DeleteLogAnomalyDetector with anomalyDetectorArn='' and with 'x' (fixture delete-log-anomaly-detector-invalid-arn-refused, steps task#3 and task#4): AWS answers InvalidParameterException 'Invalid ARN' both times; the emulator answers a length-constraint sentence and a ResourceNotFoundException.
2. Replay describe-import-task-batches-status-filter-values#task#2, logs-delete-index-policy-identifier-length-bounds#task#6 and logs-list-log-anomaly-detectors-filter-arn-pattern-invalid#task#5: each is answered ResourceNotFoundException where AWS refuses the input pre-flight.
3. Replay logs-delete-resource-policy-expected-revision-length-floor#task#6, logs-describe-index-policies-identifier-count-bounds#task#7, logs-associate-kms-key-resource-identifier-length-floor#task#4, logs-tags-for-resource-arn-length-bounds#task#3 and logs-describe-log-streams-prefix-length-bounds#task#6 and compare $.__type and $.message with the recordings.
