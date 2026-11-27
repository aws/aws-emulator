Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: batch__scheduler.schedule-name-over-64-chars-rejected-while-64-char-name-tags-batch-policy
Issue-Key: batch__scheduler.schedule-name-over-64-chars-rejected-while-64-char-name-tags-batch-policy|run#26|AssertionError|raise AssertionError(f"the policy carries tags {seen!r}; both schedules never fired")
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# batch__scheduler: a universal aws-sdk:batch:tagResource target never calls Batch, so the scheduling policy is never tagged

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
One net-new cause.

Cause 1 of 1: an EventBridge Scheduler occurrence whose Target.Arn is the universal target "arn:aws:scheduler:::aws-sdk:batch:tagResource" never makes the Batch call.
AWS: at each occurrence Scheduler assumes the execution role and makes the named AWS-SDK call with Target.Input as the request body, after substituting the context placeholders the Input uses (here "<aws.scheduler.schedule-arn>" is replaced by the firing schedule's own ARN). So the Batch scheduling policy named in the Input ends up carrying one tag per firing schedule, each value being that schedule's ARN "arn:aws:scheduler:<region>:<account>:schedule/<group>/<name>", and ListTagsForResource on the policy reports them. Reference: AWS Batch user guide, scheduling Batch jobs with EventBridge Scheduler.
Emulator: the policy's tags never change — after 240 s ListTagsForResource still returned only the tag the check set on CreateSchedulingPolicy, and neither of the two ENABLED schedules had tagged it. The create-time behaviour the same check exercises (a 64-character schedule name accepted, a 65-character one refused with the name length constraint, a duplicate name in one group refused ConflictException, and the same name in two groups being two distinct schedules) is correct; only the firing is missing.
Why: a universal-target schedule is a silent no-op, and the "<aws.scheduler.schedule-arn>" context substitution that identifies which schedule fired cannot be observed at all; a client cannot tell delivery from a dropped occurrence.
Fix: the occurrence-delivery path in the scheduler crate — the universal (aws-sdk) target invoker, including context-placeholder substitution in Target.Input.
Traces: batch__scheduler.schedule-name-over-64-chars-rejected-while-64-char-name-tags-batch-policy

## Evidence
batch__scheduler.schedule-name-over-64-chars-rejected-while-64-char-name-tags-batch-policy:
  two ENABLED at() schedules, in two different groups and sharing one 64-character name, each with Target{Arn="arn:aws:scheduler:::aws-sdk:batch:tagResource", RoleArn=<role allowed batch:TagResource on the one policy>, Input={"ResourceArn": "<policy ARN>", "Tags": {"fired-a"|"fired-b": "<aws.scheduler.schedule-arn>"}}}
  expected batch:ListTagsForResource on the policy to report fired-a = arn:aws:scheduler:<region>:<account>:schedule/<group-0>/<name> and fired-b = arn:aws:scheduler:<region>:<account>:schedule/<group-1>/<name>
  got      AssertionError: the policy carries tags {'sim2real': 'schedule-name-rule'}; both schedules never fired

## Steps to Reproduce
1. Boot the emulator. Create a Batch scheduling policy and note its ARN. Create an IAM role trusted by scheduler.amazonaws.com whose only grant is batch:TagResource on that policy ARN.
2. Create two schedule groups. In each, CreateSchedule with the same 64-character name, State ENABLED, ScheduleExpression at(<now+40s>), FlexibleTimeWindow {Mode: OFF} and Target{Arn="arn:aws:scheduler:::aws-sdk:batch:tagResource", RoleArn=<role>, Input={"ResourceArn": "<policy ARN>", "Tags": {"fired-a": "<aws.scheduler.schedule-arn>"}}} (use "fired-b" in the second group).
3. Poll batch ListTagsForResource(resourceArn=<policy ARN>) for up to 240 s — expect both keys present with each schedule's own ARN as the value; observe only the policy's original tag.
