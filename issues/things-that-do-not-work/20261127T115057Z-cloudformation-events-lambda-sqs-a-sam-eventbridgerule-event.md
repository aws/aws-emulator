Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__lambda__sqs.sam-eventbridgerule-event-target-carries-input-retry-and-dlq
Issue-Key: cloudformation__events__lambda__sqs.sam-eventbridgerule-event-target-carries-input-retry-and-dlq|run#14|AssertionError|assert got == want, f"rule target {t!r}, expected (Id, Input, RetryPolicy, DeadLetterConfig) {want!r}"
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudformation__events__lambda__sqs: a SAM EventBridgeRule event's target drops Input, RetryPolicy and DeadLetterConfig and is given a generated Id instead of the declared one

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the AWS::Serverless::Function EventBridgeRule event is transformed into a rule target that keeps only Arn and a synthesized Id; the per-target delivery properties declared on the event are discarded.

AWS: the SAM transform carries the event's Target.Id, Input, RetryPolicy (MaximumRetryAttempts, MaximumEventAgeInSeconds) and DeadLetterConfig (the DLQ ARN) onto the generated AWS::Events::Rule target, so events:ListTargetsByRule returns Id/Input/RetryPolicy/DeadLetterConfig as declared. Smithy (eventbridge): Target members Id (required, @length 1..64, pattern ^[\.\-_A-Za-z0-9]+$), Input (@length 0..8192), RetryPolicy and DeadLetterConfig are all modeled per-target.
Emulator: the listed target is `{'Id': 'FnBusLambdaTarget', 'Arn': 'arn:aws:lambda:...:function:<fn>'}` where the template declared Id 'MyTarget', Input '{"Key": "Value"}', RetryPolicy {MaximumRetryAttempts: 5, MaximumEventAgeInSeconds: 900} and DeadLetterConfig {Arn: <queue arn>} — all four are absent/overwritten.
Why: retry limits, event age and dead-lettering declared in IaC are silently not in force, so failed deliveries are lost instead of dead-lettered, the target receives the raw event instead of the declared Input, and a client that addresses the target by its declared Id (RemoveTargets/updates) cannot find it.
Fix: the CloudFormation SAM transform for AWS::Serverless::Function EventBridgeRule events, and the AWS::Events::Rule provider's target mapping (it must pass through Id, Input, RetryPolicy, DeadLetterConfig).
Traces: cloudformation__events__lambda__sqs.sam-eventbridgerule-event-target-carries-input-retry-and-dlq.

## Evidence
Expected (Id, Input, RetryPolicy, DeadLetterConfig): ('MyTarget', '{"Key": "Value"}', {'MaximumRetryAttempts': 5, 'MaximumEventAgeInSeconds': 900}, {'Arn': 'arn:aws:sqs:...:<stack>-Dlq-...'})
Actual rule target: {'Id': 'FnBusLambdaTarget', 'Arn': 'arn:aws:lambda:...:function:<fn>'}

## Steps to Reproduce
1. CreateStack (CAPABILITY_AUTO_EXPAND) for a SAM template whose function declares an EventBridgeRule event with Target.Id, Input, RetryPolicy and DeadLetterConfig (DLQ queue in the same template).
2. After CREATE_COMPLETE call events ListTargetsByRule on the generated rule.
3. Observe only Id (generated) and Arn are present.
