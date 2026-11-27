Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__scheduler__sqs.put-metric-data-refused-by-cloudwatch-dead-letters-while-twin-metric-lands
Issue-Key: cloudwatch__scheduler__sqs.put-metric-data-refused-by-cloudwatch-dead-letters-while-twin-metric-lands|run#19|AssertionError|raise AssertionError(
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# cloudwatch__scheduler__sqs: a universal aws-sdk:cloudwatch:putMetricData target neither writes the metric nor dead-letters the refusal

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
One net-new cause.

Cause 1 of 1: an EventBridge Scheduler occurrence whose Target.Arn is the universal target "arn:aws:scheduler:::aws-sdk:cloudwatch:putMetricData" makes no CloudWatch call — neither the succeeding one nor the failing one is observable.
AWS: at each occurrence Scheduler assumes the execution role and calls cloudwatch:PutMetricData with Target.Input as the request. A well-formed datum lands, so GetMetricStatistics over the namespace and metric the Input named reports the value that was put. A datum that CloudWatch refuses — here one carrying both Value and StatisticValues, which are mutually exclusive — is a failed invocation: with Target.DeadLetterConfig.Arn set and RetryPolicy.MaximumRetryAttempts 0 Scheduler writes one dead-letter event to that queue whose Body is the schedule's Input verbatim and whose MessageAttributes carry ERROR_CODE = "InvalidParameterCombination", ERROR_MESSAGE = "The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues are mutually exclusive and you have specified both." (the fixed text is the whole sentence; the member index comes from the position of the offending datum in the request), EXHAUSTED_RETRY_CONDITION = "MaximumRetryAttempts", RETRY_ATTEMPTS = "0", TARGET_ARN = the universal target ARN and SCHEDULE_ARN = the firing schedule's own ARN. The refused namespace must hold no datapoints and the succeeding schedule must not dead-letter.
Emulator: neither leg happened — after 300 s the twin namespace had no datapoint at all (sum None) and the refused leg's dead-letter queue held nothing, so the universal target was never invoked.
Why: a universal-target schedule is a silent no-op; a client cannot observe a successful put, cannot observe a refused one, and gets no dead-letter event to diagnose from.
Fix: the occurrence-delivery path in the scheduler crate — the universal (aws-sdk) target invoker, and its failure path that writes the dead-letter event with the target service's error code and message.
Traces: cloudwatch__scheduler__sqs.put-metric-data-refused-by-cloudwatch-dead-letters-while-twin-metric-lands

## Evidence
cloudwatch__scheduler__sqs.put-metric-data-refused-by-cloudwatch-dead-letters-while-twin-metric-lands:
  two ENABLED at() schedules, each Target{Arn="arn:aws:scheduler:::aws-sdk:cloudwatch:putMetricData", RoleArn=<role>, Input={"Namespace": "<ns>", "MetricData": [<datum>]}, RetryPolicy{MaximumRetryAttempts: 0, MaximumEventAgeInSeconds: 300}, DeadLetterConfig{Arn: <dlq>}}; the good datum is {MetricName, Value: 1.0, Unit: Count}, the bad one adds StatisticValues
  expected the good namespace to sum to 1.0, and the bad leg's DLQ to hold one event with ERROR_CODE "InvalidParameterCombination" and ERROR_MESSAGE "The parameters MetricData.member.1.Value and MetricData.member.1.StatisticValues are mutually exclusive and you have specified both."
  got      AssertionError: twin datapoint sum=None, refused leg dead letters=[], twin dead letters=[]

## Steps to Reproduce
1. Boot the emulator. Create two SQS queues as dead-letter queues and an IAM role trusted by scheduler.amazonaws.com granting cloudwatch:PutMetricData and sqs:SendMessage on both queues.
2. CreateSchedule twice with State ENABLED, ScheduleExpression at(<now+40s>), FlexibleTimeWindow {Mode: OFF}, RetryPolicy {MaximumRetryAttempts: 0, MaximumEventAgeInSeconds: 300}, DeadLetterConfig {Arn: <its own dlq>} and Target.Arn = "arn:aws:scheduler:::aws-sdk:cloudwatch:putMetricData"; Input = {"Namespace":"<ns>/twin","MetricData":[{"MetricName":"ScheduledPut","Value":1.0,"Unit":"Count"}]} for the first and the same datum plus "StatisticValues":{...} under "<ns>/refused" for the second.
3. Poll cloudwatch GetMetricStatistics over "<ns>/twin" and receive from the refused leg's DLQ for up to 300 s — expect a sum of 1.0 and one dead-letter event with the InvalidParameterCombination attributes; observe no datapoint and no dead-letter event.
