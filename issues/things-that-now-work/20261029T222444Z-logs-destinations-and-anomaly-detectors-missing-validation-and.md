Issue-Source: trace-sweep-divergence
Issue-Trace: logs-destination-lifecycle-and-policy
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-destination-lifecycle-and-policy logs-subscription-filter-to-destination-pipeline logspc-destination-undeliverable-target-rejected logs-log-anomaly-detection-workflow logs-log-anomaly-detector-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|PutDestination|4xx->2xx|$.__type
Issue-Key: logs|PutDestination|4xx->2xx|$.message
Issue-Key: logs|PutDestination|4xx->2xx|status
Issue-Key: logs|DeleteDestination|4xx->2xx|$
Issue-Key: logs|DeleteDestination|4xx->2xx|status
Issue-Key: logs|PutDestinationPolicy|4xx->4xx|$.message
Issue-Key: logs|PutSubscriptionFilter|4xx->4xx|$.message
Issue-Key: logs|DescribeDestinations|2xx->2xx|$.destinations[*]
Issue-Key: logs|PutDestinationPolicy|4xx->2xx|$
Issue-Key: logs|PutDestinationPolicy|4xx->2xx|status
Issue-Key: logs|CreateLogAnomalyDetector|4xx->4xx|$.__type
Issue-Key: logs|CreateLogAnomalyDetector|4xx->4xx|$.message
Issue-Key: logs|GetLogAnomalyDetector|4xx->4xx|$.message
Issue-Key: logs|GetLogAnomalyDetector|2xx->2xx|$.anomalyDetectorStatus
Issue-Related: 20261029T222444Z-logs-read-paths-omit-modeled-response-members-and.md
Issue-Commit: 02f6d4f25c27220a14c0924662b79788aefc3700

# logs destinations and anomaly detectors: missing validation and rewritten refusal text (10 causes)

> _Note: this issue was identified against the code base at commit `02f6d4f25c27220a14c0924662b79788aefc3700`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDestination

## Description
Ten root causes across the cross-account destination surface and the log-anomaly-detector surface. The shared fault is the same in both: calls AWS refuses are accepted (so state the client believes exists on AWS does not), and where a refusal does happen its class or text is the emulator's own rather than AWS's for the error class. Cause count: 10 — all ten must be closed. The open issue 20261028T000431Z-logs-cloudwatch-logs-destinations-putsubscriptionfilter-to-a-destination-with.md covers a different destination cause (subscription filters to a policy-less destination) and carries different replay keys; cause 7 of 20261029T182816Z-logs-ten-cloudwatch-logs-refusal-messages-are-emulator.md covers the anomaly-detector not-found wording from the behavioral corpus only.

=== Cause 1 of 10: PutDestination accepts a roleArn from another account ===
AWS: refuses with HTTP 400 InvalidParameterException and the fixed text 'Cross-account role passing is not allowed.' (no variable parts).
Emulator: answers HTTP 200 and creates the destination.
Why: a client believes a cross-account destination was created; on AWS it was not, so every subsequent PutSubscriptionFilter/DescribeDestinations step diverges. Three fixtures hit this one check.
Fix: the roleArn account check in the PutDestination handler (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/).
Traces: logs-destination-lifecycle-and-policy#task#2.

=== Cause 2 of 10: DeleteDestination on an absent destination answers 200 ===
AWS: HTTP 400 ResourceNotFoundException with the fixed text 'The specified destination does not exist.'
Emulator: HTTP 200 with an empty body.
Why: delete is silently idempotent in the emulator but not on AWS, so teardown logic that asserts the refusal never fires.
Fix: the existence check in the DeleteDestination handler.
Traces: logs-destination-lifecycle-and-policy#task#4 (also logspc-destination-undeliverable-target-rejected#task#5).

=== Cause 3 of 10: PutDestinationPolicy validates the destination before the access policy ===
AWS: when the accessPolicy is not parseable IAM the refusal is about the policy — 'Error occurred while parsing accessPolicy. Please check if the accessPolicy has been constructed correctly using IAM grammar.' — regardless of whether the named destination exists; AWS parses the document first.
Emulator: refuses with 'Destination with name <destinationName> does not exist' — the destination check runs first.
Why: a caller fixing its policy document is told the destination is missing instead, and vice versa; the two failure modes are indistinguishable.
Fix: the validation order in the PutDestinationPolicy handler — parse accessPolicy before resolving the destination.
Traces: logs-subscription-filter-to-destination-pipeline#task#5.

=== Cause 4 of 10: PutSubscriptionFilter's vended-logs destination refusal is emulator-authored ===
AWS: a destinationArn that denotes a vended-logs target supplied without a roleArn is refused with the fixed text 'destinationArn for vendor logs cannot be used without roleArn'.
Emulator: 'destinationArn must be the ARN of a kinesis, firehose, lambda, logs resource' — a shape complaint that misdescribes a well-formed ARN.
Why: the caller is told its ARN is the wrong kind rather than that it must also pass a roleArn, so it never supplies the missing parameter.
Fix: the destinationArn/roleArn pairing check in the PutSubscriptionFilter handler.
Traces: logs-subscription-filter-to-destination-pipeline#task#6.

=== Cause 5 of 10: DescribeDestinations lists a destination AWS never created ===
AWS: after PutDestination is refused for an undeliverable/cross-account target, DescribeDestinations returns NO destination entry.
Emulator: lists the destination (destinationName/targetArn/roleArn/arn/creationTime).
Why: the read path confirms a destination that does not exist on AWS — the consequence of cause 1 made visible, and the step a client uses to decide whether to retry.
Fix: the same validation branch as cause 1 (nothing must be persisted); the DescribeDestinations read path then agrees.
Traces: logspc-destination-undeliverable-target-rejected#task#3.

=== Cause 6 of 10: PutDestinationPolicy on an absent destination answers 200 ===
AWS: HTTP 400 InvalidParameterException with 'Destination with name <destinationName> does not exist' — fixed text plus one variable part, the name from the request.
Emulator: HTTP 200 with an empty body.
Why: a client believes it attached an access policy to a destination that does not exist, so the cross-account grant it reports is fictional.
Fix: the existence check in the PutDestinationPolicy handler (after the accessPolicy parse — see cause 3).
Traces: logspc-destination-undeliverable-target-rejected#task#4.

=== Cause 7 of 10: CreateLogAnomalyDetector with an empty logGroupArnList raises the wrong error class ===
AWS: HTTP 400 ValidationException with the fixed text 'LogGroupArn(s) must be provided.'
Emulator: HTTP 400 InvalidParameterException with 'logGroupArnList must contain exactly 1 log group ARN, got 0' — wrong error code AND an emulator-authored message that also invents a one-ARN-only rule.
Why: a client branching on the error code takes the wrong path, and the message advertises a cardinality restriction AWS does not impose.
Fix: the logGroupArnList validation branch of the CreateLogAnomalyDetector handler — raise ValidationException with AWS's text.
Traces: logs-log-anomaly-detection-workflow#task#8.

=== Cause 8 of 10: GetLogAnomalyDetector's not-found message drops the ARN ===
AWS: ResourceNotFoundException with 'The specified ARN does not exist or already deleted : <anomalyDetectorArn>' — fixed text (note the space before the colon) plus one variable part, the ARN from the request.
Emulator: 'The specified anomaly detector does not exist.'
Why: the ARN that failed is unrecoverable from the error, and AWS's deliberate 'does not exist or already deleted' ambiguity is lost.
Fix: the not-found branch of the GetLogAnomalyDetector handler.
Traces: logs-log-anomaly-detection-workflow#task#10 (also logs-log-anomaly-detector-lifecycle#task#12).

=== Cause 9 of 10: CreateLogAnomalyDetector's missing-log-group message drops the group and owner ===
AWS: ResourceNotFoundException with "Log group '<logGroupArn>' for owner '<accountId>' cannot be found." — fixed text plus two variable parts, the ARN from logGroupArnList and the caller's account id.
Emulator: 'The specified log group does not exist.'
Why: with several ARNs in the list the caller cannot tell which one was missing.
Fix: the log-group resolution branch of the CreateLogAnomalyDetector handler.
Traces: logs-log-anomaly-detector-lifecycle#task#3.

=== Cause 10 of 10: a newly created anomaly detector reports INITIALIZING instead of TRAINING ===
AWS: immediately after CreateLogAnomalyDetector the detector's anomalyDetectorStatus is 'TRAINING', and anomalyVisibilityTime is populated (defaulted to 7). Smithy: AnomalyDetectorStatus enum is INITIALIZING | TRAINING | ANALYZING | FAILED | DELETED | PAUSED, and anomalyVisibilityTime has @range 7..90.
Emulator: reports 'INITIALIZING' and omits anomalyVisibilityTime.
Why: a client that waits for TRAINING (the first state AWS exposes) before proceeding waits forever on a status AWS never returns here.
Fix: the detector's initial status and the anomalyVisibilityTime default in the CreateLogAnomalyDetector/GetLogAnomalyDetector state path.
Traces: logs-log-anomaly-detector-lifecycle#task#5.

## Evidence
Cause 1: "status: expected 400, got 200; $.__type: missing (expected 'InvalidParameterException', got None); $.message: missing (expected 'Cross-account role passing is not allowed.', got None)" — 3 affected steps.
Cause 2: "status: expected 400, got 200; $: empty_body (expected {'__type': 'ResourceNotFoundException', 'message': 'The specified destination does not exist.'}, got {})" — 2 affected steps.
Cause 3: "$.message: value_mismatch (expected 'Error occurred while parsing accessPolicy. Please check if the accessPolicy has been constructed correctly using IAM grammar.', got 'Destination with name <destinationName> does not exist')" at HTTP 400 -> 400.
Cause 4: "$.message: value_mismatch (expected 'destinationArn for vendor logs cannot be used without roleArn', got 'destinationArn must be the ARN of a kinesis, firehose, lambda, logs resource')".
Cause 5: "$.destinations[0]: extra (expected None, got {'destinationName': <name>, 'targetArn': <kinesis stream arn>, 'roleArn': <role arn>, 'arn': <destination arn>, 'creationTime': ...})" — the names/ARNs/time are per-run; the requirement is that no entry exists.
Cause 6: "status: expected 400, got 200; $: empty_body (expected {'__type': 'InvalidParameterException', 'message': 'Destination with name <destinationName> does not exist'}, got {})".
Cause 7: "$.__type: value_mismatch (expected 'ValidationException', got 'InvalidParameterException'); $.message: value_mismatch (expected 'LogGroupArn(s) must be provided.', got 'logGroupArnList must contain exactly 1 log group ARN, got 0')".
Cause 8: "$.message: value_mismatch (expected 'The specified ARN does not exist or already deleted : arn:aws:logs:us-west-2:000000000000:anomaly-detector:<id>', got 'The specified anomaly detector does not exist.')" — 2 affected steps; the detector id is per-run.
Cause 9: "$.message: value_mismatch (expected \"Log group 'arn:aws:logs:us-west-2:000000000000:log-group:<name>' for owner '000000000000' cannot be found.\", got 'The specified log group does not exist.')".
Cause 10: "$.anomalyDetectorStatus: value_mismatch (expected 'TRAINING', got 'INITIALIZING'); $.anomalyVisibilityTime: missing (expected 7, got None)".

## Steps to Reproduce
1/5: PutDestination(destinationName=..., targetArn=<a kinesis stream>, roleArn=<a role in another account>) — expect 400 InvalidParameterException 'Cross-account role passing is not allowed.'; then DescribeDestinations and expect NO entry (logs-destination-lifecycle-and-policy#task#2, logspc-destination-undeliverable-target-rejected#task#3).
2: DeleteDestination(destinationName=<never created>) — expect 400 ResourceNotFoundException 'The specified destination does not exist.' (logs-destination-lifecycle-and-policy#task#4).
3: PutDestinationPolicy(destinationName=<does not exist>, accessPolicy=<not valid IAM>) — expect the accessPolicy IAM-grammar refusal, not the destination not-found one (logs-subscription-filter-to-destination-pipeline#task#5).
4: PutSubscriptionFilter with a vended-logs destinationArn and no roleArn — expect 'destinationArn for vendor logs cannot be used without roleArn' (#task#6).
6: PutDestinationPolicy(destinationName=<does not exist>, accessPolicy=<valid IAM>) — expect 400 InvalidParameterException 'Destination with name <name> does not exist' (logspc-destination-undeliverable-target-rejected#task#4).
7: CreateLogAnomalyDetector(logGroupArnList=[]) — expect 400 ValidationException 'LogGroupArn(s) must be provided.' (logs-log-anomaly-detection-workflow#task#8).
8: GetLogAnomalyDetector(anomalyDetectorArn=<well-formed but unknown>) — expect 'The specified ARN does not exist or already deleted : <arn>' (#task#10).
9: CreateLogAnomalyDetector(logGroupArnList=[<a log-group ARN that does not exist>]) — expect "Log group '<arn>' for owner '<acct>' cannot be found." (logs-log-anomaly-detector-lifecycle#task#3).
10: CreateLogAnomalyDetector on an existing group, then GetLogAnomalyDetector — expect anomalyDetectorStatus 'TRAINING' and anomalyVisibilityTime 7 (#task#5).
