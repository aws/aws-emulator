Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch.deploy-cloudformation-composite-alarm-derives-state cloudwatch.deploy-cloudwatch-1-alarm-rename-replaces-and-dangling-composite-rejected cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: cloudwatch.deploy-cloudformation-composite-alarm-derives-state|run#12|AssertionError|assert len(described["CompositeAlarms"]) == 1, f"expected 1 composite alarm, got {described['CompositeAlarms']!r}"
Issue-Key: cloudwatch.deploy-cloudwatch-1-alarm-rename-replaces-and-dangling-composite-rejected|run#68|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle|_drain#6|Timeout|time.sleep(2)
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudwatch: 3 net-new causes — a stack-created composite alarm is invisible to DescribeAlarms, a dangling AlarmRule is accepted instead of rolling the stack back, and the metric-stream lifecycle never settles

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Three distinct root causes.

Cause 1 of 3: a composite alarm deployed together with its child metric alarm in one stack is not registered in CloudWatch.
AWS: provisioning AWS::CloudWatch::CompositeAlarm performs PutCompositeAlarm (Smithy: PutCompositeAlarm — AlarmName and AlarmRule required, no output shape), after which DescribeAlarms returns the alarm in its CompositeAlarms list (DescribeAlarms output has distinct CompositeAlarms/MetricAlarms/LogAlarms lists) and its StateValue is derived server-side from the AlarmRule as the child's state changes.
Emulator: once the stack reaches CREATE_COMPLETE, DescribeAlarms for the composite's name returns CompositeAlarms == [], so the composite does not exist and no AlarmRule evaluation can be observed.
Why: a client that deploys a composite rollup gets no alarm at all — no state, no actions, no evaluation of the AlarmRule — while CloudFormation reports success.
Fix: the CloudFormation resource handler for AWS::CloudWatch::CompositeAlarm (register the alarm through CloudWatch's composite-alarm state) and/or DescribeAlarms' CompositeAlarms serialization.
Traces: cloudwatch.deploy-cloudformation-composite-alarm-derives-state.

Cause 2 of 3: a composite alarm whose AlarmRule names an alarm that does not exist is created successfully instead of being refused.
AWS: PutCompositeAlarm validates the AlarmRule's references; an AlarmRule referencing a non-existent alarm name is rejected, so the CloudFormation resource create fails and the stack ends ROLLBACK_COMPLETE (the engine holds the resource in CREATE_IN_PROGRESS for the dependency's consistency window first). No composite alarm is left behind.
Emulator: the stack carrying a CompositeAlarm with a dangling AlarmRule reaches CREATE_COMPLETE — the rule's reference is never resolved, so an unsatisfiable composite alarm is accepted.
Why: a typo'd or deleted child alarm produces a composite alarm that can never evaluate, and the deploy reports success — the client loses the only signal AWS gives that its rollup is broken. The open issue 20261108T001650Z-cloudwatch-3-net-new-causes-a-composite-alarmrule.md covers AlarmRule EVALUATION (the AND combinator) and does not cover this missing reference validation.
Fix: PutCompositeAlarm's request validation (resolve every alarm name referenced by AlarmRule against stored alarms and refuse otherwise), and the AWS::CloudWatch::CompositeAlarm resource handler so the refusal propagates into a stack rollback.
Traces: cloudwatch.deploy-cloudwatch-1-alarm-rename-replaces-and-dangling-composite-rejected.

Cause 3 of 3: the deploy-fronted metric-stream lifecycle never reaches a verdict — the check is cut off at the harness's 60 s cap.
AWS: the monitoring stack's two AWS::CloudWatch::MetricStream resources (one with an ExcludeFilters list over CPUUtilization/NetworkIn/NetworkOut/DiskReadBytes/DiskWriteBytes) deploy, and ListMetricStreams/GetMetricStream answer immediately for them; every call in the lifecycle is a fast control-plane call.
Emulator: recorded behavior only — the check makes no progress and is killed at 60.0 s, so some call on the deploy/solve path (stack create of the metric-stream + Firehose + S3 + IAM template, or a metric-stream read) never returns a terminal answer.
Why: a client driving metric streams through CloudFormation hangs instead of getting an answer; a hung control-plane call is indistinguishable from an outage and no timeout/error is ever surfaced.
Fix: whichever call on this path does not settle — the CloudFormation create path for AWS::CloudWatch::MetricStream and the metric-stream read operations (ListMetricStreams/GetMetricStream) must each return promptly.
Traces: cloudwatch.mstrx-e2e-find-metric-streams-excluding-namespace-lifecycle.

## Evidence
Cause 1 — expected 1 composite alarm from DescribeAlarms after CREATE_COMPLETE. Actual: AssertionError: expected 1 composite alarm, got [].
Cause 2 — expected ROLLBACK_COMPLETE for the stack whose CompositeAlarm AlarmRule references a never-created alarm. Actual: AssertionError: stack <name> with a dangling AlarmRule ended in 'CREATE_COMPLETE', expected ROLLBACK_COMPLETE.
Cause 3 — expected the lifecycle to complete (its own stack budget is 900 s of waiting on fast control-plane calls). Actual: "Check timed out after 60.0s" with no step completed.

## Steps to Reproduce
Cause 1: CreateStack with an AWS::CloudWatch::Alarm plus an AWS::CloudWatch::CompositeAlarm whose AlarmRule references it; after CREATE_COMPLETE call DescribeAlarms(AlarmNames=[<composite>]) — CompositeAlarms is empty.
Cause 2: CreateStack with a valid metric alarm, a CompositeAlarm whose AlarmRule is ALARM("<name-never-created>"), and nothing creating that name; poll DescribeStacks — it reaches CREATE_COMPLETE instead of ROLLBACK_COMPLETE. Equivalently call PutCompositeAlarm directly with an AlarmRule naming a non-existent alarm: it is accepted.
Cause 3: deploy the mstrx monitoring template (two AWS::CloudWatch::MetricStream resources, one with ExcludeFilters) and then list/read the streams — the sequence never completes.
