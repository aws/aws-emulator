Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler__sqs.fifo-queue-target-without-sqs-parameters-refused-while-grouped-twin-delivers scheduler__sqs.standard-queue-target-with-message-group-refused-while-plain-twin-delivers
Issue-Key: scheduler__sqs.fifo-queue-target-without-sqs-parameters-refused-while-grouped-twin-delivers|_refusal#25|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: scheduler__sqs.standard-queue-target-with-message-group-refused-while-plain-twin-delivers|_refused#23|AssertionError|raise AssertionError(f"expected a refusal, the call returned {out!r}")
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# scheduler__sqs: SqsParameters is not matched against the target queue kind — a FIFO queue target without SqsParameters and a standard queue target carrying MessageGroupId are both accepted

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Two net-new causes, both in CreateSchedule's target-parameter validation for SQS targets.

Cause 1 of 2: a schedule whose Target.Arn names a FIFO queue and that supplies no SqsParameters is accepted.
AWS: a FIFO queue target requires a MessageGroupId, so Scheduler refuses the create with 400 ValidationException and the message "Parameters not valid due to the following reason: Parameters SqsParameters must be specified." — entirely fixed text. Nothing is stored: GetSchedule answers ResourceNotFoundException "Schedule <name> does not exist." and ListSchedules on the group does not list it. The identical schedule carrying Target.SqsParameters.MessageGroupId is accepted, stores that value, and delivers its Input to the FIFO queue under that MessageGroupId. Reference: Scheduler API reference, SqsParameters.
Emulator: returns 200 with a ScheduleArn and stores the schedule.
Why: a schedule is persisted that can never deliver — SQS rejects a SendMessage to a FIFO queue with no MessageGroupId — and the client is told nothing at configure time.
Fix: the SQS-target parameter validation in the scheduler crate's CreateSchedule/UpdateSchedule handler: require SqsParameters when the target ARN names a .fifo queue.
Traces: scheduler__sqs.fifo-queue-target-without-sqs-parameters-refused-while-grouped-twin-delivers

Cause 2 of 2: a schedule whose Target.Arn names a STANDARD queue and that supplies SqsParameters.MessageGroupId is accepted.
AWS: MessageGroupId has no meaning for a standard queue, so Scheduler refuses the create with 400 ValidationException and the message "Parameters MessageGroupId not valid for the target." — entirely fixed text, the member name MessageGroupId being its only variable part. Nothing is stored (GetSchedule answers ResourceNotFoundException "Schedule <name> does not exist."; the group lists only the plain twin), and the plain twin delivers its Input to the standard queue with no MessageGroupId system attribute.
Emulator: returns 200 with a ScheduleArn and stores the schedule.
Why: the refusal that tells a client its target kind and its parameters disagree never happens, so the mistake surfaces only as a delivery failure or as a silently ignored parameter.
Fix: the same SQS-target parameter validation: reject MessageGroupId when the target ARN does not name a .fifo queue.
Traces: scheduler__sqs.standard-queue-target-with-message-group-refused-while-plain-twin-delivers

## Evidence
Cause 1 — scheduler__sqs.fifo-queue-target-without-sqs-parameters-refused-while-grouped-twin-delivers:
  Target{Arn=<arn of a queue named …​.fifo>, RoleArn=<role granting sqs:SendMessage>, Input=<json>} with no SqsParameters
  expected ValidationException "Parameters not valid due to the following reason: Parameters SqsParameters must be specified."
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/<group>/<name>"}

Cause 2 — scheduler__sqs.standard-queue-target-with-message-group-refused-while-plain-twin-delivers:
  Target{Arn=<standard queue ARN>, RoleArn=<role>, Input=<json>, SqsParameters={"MessageGroupId": "tenant-a"}}
  expected ValidationException "Parameters MessageGroupId not valid for the target."
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/<group>/<name>"}

## Steps to Reproduce
1. Boot the emulator. Create a FIFO queue (QueueName ending ".fifo", FifoQueue=true) and a standard queue; create an IAM role trusted by scheduler.amazonaws.com granting sqs:SendMessage on each; create a schedule group.
2. Cause 1: CreateSchedule with Target{Arn=<fifo queue ARN>, RoleArn=<role>, Input="{}"} and no SqsParameters — expect ValidationException "Parameters not valid due to the following reason: Parameters SqsParameters must be specified.", observe 200. Then confirm GetSchedule finds the schedule that should not exist.
3. Cause 2: CreateSchedule with Target{Arn=<standard queue ARN>, RoleArn=<role>, Input="{}", SqsParameters={"MessageGroupId":"tenant-a"}} — expect ValidationException "Parameters MessageGroupId not valid for the target.", observe 200.
