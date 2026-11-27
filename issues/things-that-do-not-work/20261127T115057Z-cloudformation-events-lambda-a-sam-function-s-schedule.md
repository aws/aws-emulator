Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events__lambda.sam-schedule-event-input-reaches-the-rule-target
Issue-Key: cloudformation__events__lambda.sam-schedule-event-input-reaches-the-rule-target|run#12|AssertionError|assert [t.get("Input") for t in targets] == ['{"job":"nightly"}'], f"processed rule targets {targets!r}"
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudformation__events__lambda: a SAM function's Schedule event produces a rule target whose Arn is the raw Fn::GetAtt object and which carries none of the event's input

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the AWS::Serverless::Function Schedule event is not fully transformed — the generated AWS::Events::Rule's target keeps the template's unresolved intrinsic as its Arn and the event's Input is dropped.

AWS: the SAM transform expands a Schedule event into an AWS::Events::Rule whose Targets[0].Arn is the RESOLVED function ARN (arn:aws:lambda:<region>:<account>:function:<name>), whose Id is the transform's generated target id, and whose Input is the event's declared Input string; events:ListTargetsByRule then shows that resolved target and the scheduled invocation carries the Input as the Lambda payload. Smithy (eventbridge): Target.Arn is required, @length 1..1600, a plain string — an object is not a legal value; Target.Input is a string @length 0..8192.
Emulator: the processed rule's targets read `[{'Arn': {'Fn::GetAtt': ['Fn','Arn']}, 'Id': 'FnTickLambdaTarget'}]` — the Fn::GetAtt is stored verbatim as the target Arn and no Input member is present, so nothing can be invoked and the declared input never reaches the target.
Why: every SAM schedule-driven function is dead in the emulator: the target ARN is not an ARN, so no invocation can be dispatched and a handler written to read the declared Input sees nothing.
Fix: the CloudFormation SAM transform for AWS::Serverless::Function Schedule events (intrinsic resolution of the target Arn and propagation of Input) and the AWS::Events::Rule provider's target serialization, which should never accept a non-string Arn.
Traces: cloudformation__events__lambda.sam-schedule-event-input-reaches-the-rule-target.

## Evidence
Expected: a rule target with a resolved function ARN and the event's Input string.
Actual: `AssertionError: processed rule targets [OrderedDict({'Arn': OrderedDict({'Fn::GetAtt': ['Fn', 'Arn']}), 'Id': 'FnTickLambdaTarget'})]` — Arn is the intrinsic object, Input absent.

## Steps to Reproduce
1. CreateStack (with CAPABILITY_AUTO_EXPAND) for a SAM template whose AWS::Serverless::Function has Events: {Tick: {Type: Schedule, Properties: {Schedule: rate(1 minute), Input: '{"k":"v"}'}}}.
2. After CREATE_COMPLETE, call events ListTargetsByRule on the generated rule.
3. Observe the target's Arn is the Fn::GetAtt object and Input is missing.
