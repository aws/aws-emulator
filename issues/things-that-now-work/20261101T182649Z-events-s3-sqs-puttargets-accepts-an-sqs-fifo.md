Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.eventbridge-fifo-queue-target-arn
Issue-Key: events__s3__sqs.eventbridge-fifo-queue-target-arn|_error#20|AssertionError|raise AssertionError(
Issue-Commit: a11f6d463fab0784f8ee64a0ac214df743be0b52

# events/s3/sqs: PutTargets accepts an SQS FIFO queue target with no SqsParameters.MessageGroupId instead of refusing it with ValidationException

> _Note: this issue was identified against the code base at commit `a11f6d463fab0784f8ee64a0ac214df743be0b52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
One net-new root cause (Cause 1 of 1), grounded in failing check events__s3__sqs.eventbridge-fifo-queue-target-arn. The emulator's EventBridge PutTargets performs no target-type validation for SQS FIFO queue ARNs, so a target real AWS refuses is registered successfully.

Cause 1 of 1: PutTargets does not require SqsParameters.MessageGroupId when the target ARN is a FIFO queue (an SQS queue whose name ends in `.fifo`). Failing check id: events__s3__sqs.eventbridge-fifo-queue-target-arn.

AWS: EventBridge requires the message group to be supplied for a fair/FIFO SQS target. Smithy (services/service-com-amazonaws-eventbridge-awsevents/smithy-model/com-amazonaws-eventbridge-awsevents.json): Target.SqsParameters -> SqsParameters, documented as "Contains the message group ID to use when the target is an Amazon SQS fair or FIFO queue. If you specify a fair or FIFO queue as a target, the queue must have content-based deduplication enabled"; SqsParameters.MessageGroupId is `@length min 0 max 100`. PutTargets' modelled errors are ConcurrentModificationException, InternalException, LimitExceededException, ManagedRuleException and ResourceNotFoundException, so the refusal code is NOT modelled for this operation — recorded behavior only: AWS answers HTTP 400 with error code `ValidationException` and message `Parameter(s) SqsParameters must be specified for target: <TargetId>.` In that message the fixed text is `Parameter(s) SqsParameters must be specified for target: ` followed by the offending target's `Id` taken verbatim from the request and a terminating `.`; nothing else in it is variable. Because AWS rejects the whole call, no target is registered: a follow-up ListTargetsByRule returns an empty Targets list.

Emulator: the same PutTargets call (Targets=[{Id: <id>, Arn: <fifo queue arn>}], no SqsParameters) returns HTTP 200 with `{"FailedEntryCount": 0, "FailedEntries": []}` — no error at all. Replay output: `AssertionError: expected put_targets to be refused but it SUCCEEDED and returned {'FailedEntryCount': 0, 'FailedEntries': [], ... 'HTTPStatusCode': 200 ...}`. The same check's positive control passed first (the identical FIFO ARN with SqsParameters.MessageGroupId present registers and the PutEvents event is actually delivered to the FIFO queue), so bus/rule/queue-policy wiring is working and the only divergence is the missing validation.

Why: a client that mis-wires a FIFO target gets a success response and a registered target that can never deliver (AWS SQS rejects SendMessage to a FIFO queue without a message group), so the misconfiguration surfaces later as silently lost events instead of a synchronous 400 at configuration time. Infrastructure tooling that asserts on the ValidationException (CDK/CloudFormation rollback, Terraform plan/apply error handling, negative tests) sees the emulator accept a template real AWS refuses, and the error branch of that code is never exercised.

Fix: belongs in the PutTargets handler's per-target validation for the eventbridge service crate (services/service-com-amazonaws-eventbridge-awsevents, the PutTargets operation handler and the target-validation helper it calls, before any target is committed to rule state). It must detect an SQS target ARN naming a FIFO queue (resource name ending in `.fifo`), and when the entry carries no SqsParameters (or SqsParameters with no MessageGroupId) reject the entire request with HTTP 400 / ValidationException and the message template above, leaving rule target state untouched so ListTargetsByRule still reports no targets.

Traces: events__s3__sqs.eventbridge-fifo-queue-target-arn (the only failing check in this suite not already owned by an open issue). Related but distinct open issues for this service group, none of which covers this cause: 20261029T182228Z-events-s3-sqs-s3-post-object-answers-501.md and 20261029T182228Z-cloudwatch-events-s3-sqs-an-eventbridge-sqs-target-with-no-queue-policy.md (both are about delivery/dead-lettering of SQS targets, not PutTargets input validation).

## Evidence
Expected (AWS): PutTargets(Rule=<rule>, EventBusName=<custom bus>, Targets=[{"Id": "fifo-target", "Arn": "arn:aws:sqs:<region>:<account>:<name>.fifo"}]) -> HTTP 400, Error.Code = "ValidationException", Error.Message = "Parameter(s) SqsParameters must be specified for target: fifo-target." (fixed text + the request's target Id); afterwards ListTargetsByRule(Rule=<rule>, EventBusName=<bus>).Targets == [].

Actual (emulator): HTTP 200, body {"FailedEntryCount": 0, "FailedEntries": []}, no error raised; the target is registered.

Replay detail: check_events__s3__sqs_eventbridge_fifo_queue_target_arn.py line 120 in run -> line 152 in _error -> AssertionError: expected put_targets to be refused but it SUCCEEDED and returned {'FailedEntryCount': 0, 'FailedEntries': [], 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '41', ...}, 'RetryAttempts': 0}}.

Positive control in the same check succeeded: the identical ARN WITH SqsParameters={'MessageGroupId': 'orders-group'} registered and PutEvents was delivered to the FIFO queue, isolating the defect to the missing FIFO/SqsParameters validation.

## Steps to Reproduce
1. sqs:CreateQueue a queue whose name ends in `.fifo` with Attributes {FifoQueue: "true", ContentBasedDeduplication: "true"}; read its QueueArn via GetQueueAttributes.
2. events:CreateEventBus with a fresh name; events:PutRule on that bus with EventPattern {"source": ["app.orders"], "detail-type": ["OrderPlaced"]}; DescribeRule to get the rule ARN.
3. sqs:SetQueueAttributes to attach a policy allowing events.amazonaws.com sqs:SendMessage on the queue ARN, conditioned on aws:SourceArn = the rule ARN.
4. events:PutTargets with Targets=[{Id: "fifo-target", Arn: <fifo queue arn>, SqsParameters: {MessageGroupId: "orders-group"}}] and PutEvents a matching event; confirm it is delivered (positive control).
5. events:RemoveTargets(Ids=["fifo-target"]), then events:PutTargets again with Targets=[{Id: "fifo-target", Arn: <same fifo queue arn>}] and NO SqsParameters.
6. Observe: the emulator returns 200 with FailedEntryCount 0; AWS returns 400 ValidationException "Parameter(s) SqsParameters must be specified for target: fifo-target." and registers nothing (ListTargetsByRule returns []).
