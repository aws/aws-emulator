Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: scheduler.create-schedule-ecs-awsvpc-subnet-length-bounds-rejected scheduler.create-schedule-ecs-placement-strategy-type-outside-enum-rejected scheduler.create-schedule-ecs-reference-id-over-1024-rejected scheduler.update-schedule-ecs-task-count-outside-range-rejected scheduler.create-schedule-target-role-arn-length-bounds-rejected scheduler.create-schedule-ecs-tag-key-over-128-chars-rejected scheduler.ecs-task-tag-value-of-257-characters-stored scheduler.untag-resource-tag-keys-count-bounds-rejected
Issue-Shape-Id: com.amazonaws.scheduler#AWSChronosService
Issue-Key: scheduler.create-schedule-ecs-awsvpc-subnet-length-bounds-rejected|_refusal#27|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: scheduler.create-schedule-ecs-placement-strategy-type-outside-enum-rejected|_refusal#26|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: scheduler.create-schedule-ecs-reference-id-over-1024-rejected|_refusal#26|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: scheduler.update-schedule-ecs-task-count-outside-range-rejected|_refusal#11|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: scheduler.create-schedule-target-role-arn-length-bounds-rejected|_refusal#27|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: scheduler.create-schedule-ecs-tag-key-over-128-chars-rejected|run#16|AssertionError|raise AssertionError(f"a 129-character ECS tag key was accepted: {made!r}")
Issue-Key: scheduler|CreateSchedule|ValidationException
Issue-Key: scheduler.untag-resource-tag-keys-count-bounds-rejected|run#16|AssertionError|assert empty == (
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# scheduler: five missing or over-strict target validations — EcsParameters on a non-ECS target, an unassumable Target.RoleArn, the ECS tag-key cap, an invented ECS tag-value cap, and UntagResource with an empty TagKeys list

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
Five net-new causes in EventBridge Scheduler's CreateSchedule/UpdateSchedule/UntagResource validation: four calls AWS refuses are accepted, one call AWS accepts is refused, and one is not routed at all.

Cause 1 of 5: EcsParameters supplied on a target that is not an ECS cluster is accepted instead of refused.
AWS: the target-kind check runs after field validation and refuses any Target carrying a parameter block that does not match the target ARN's kind, with 400 ValidationException and the message "Parameters EcsParameters not supported for the target." — fixed text, with the parameter-block member name (EcsParameters) as its only variable part. No schedule is created, so GetSchedule answers ResourceNotFoundException "Schedule <name> does not exist." and ListSchedules does not list it.
Emulator: returns 200 with a ScheduleArn and stores the schedule; the same acceptance happens on UpdateSchedule.
Why: a client wiring EcsParameters to an SQS/SNS/Lambda target gets a schedule that can never deliver as configured, and the four checks that use this refusal as their legal-value control cannot distinguish a field violation from a target-kind violation.
Fix: the CreateSchedule/UpdateSchedule target validation in the scheduler crate — the target-kind vs parameter-block compatibility check.
Traces: scheduler.create-schedule-ecs-awsvpc-subnet-length-bounds-rejected, scheduler.create-schedule-ecs-placement-strategy-type-outside-enum-rejected, scheduler.create-schedule-ecs-reference-id-over-1024-rejected, scheduler.update-schedule-ecs-task-count-outside-range-rejected

Cause 2 of 5: a Target.RoleArn that is syntactically valid but names no existing role is accepted.
AWS: Scheduler verifies at CreateSchedule time that it can assume the execution role, and refuses with 400 ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role." — entirely fixed text. Smithy: scheduler#RoleArn @length min 1 max 1600, @pattern ^arn:aws(-[a-z]+)?:iam::\d{12}:role\/[\w+=,.@\/-]+$, so a 1600-character role ARN passes field validation and reaches the assume check.
Emulator: returns 200 and stores the schedule for a role ARN that passes the pattern but names nothing in IAM.
Why: a typo in RoleArn is reported only much later (or never) instead of at create time, and a schedule is persisted that can never deliver.
Fix: the CreateSchedule/UpdateSchedule execution-role resolution in the scheduler crate — resolve the role and run the assume check before storing.
Traces: scheduler.create-schedule-target-role-arn-length-bounds-rejected

Cause 3 of 5: an ECS task tag KEY longer than 128 characters is accepted.
AWS: refuses with 400 ValidationException and the service-authored sentence "ECS tag key must not exceed 128 characters." — entirely fixed text, not a Smithy constraint message (the model places no length bound on the ECS Tags map's keys: scheduler#Tags member target TagMap). A 128-character key is accepted and read back intact by GetSchedule.
Emulator: returns 200 and stores the 129-character key.
Why: a schedule is created that ECS RunTask will reject at delivery time rather than at configure time, and the documented key cap is unenforced.
Fix: the EcsParameters.Tags validation in the CreateSchedule handler.
Traces: scheduler.create-schedule-ecs-tag-key-over-128-chars-rejected

Cause 4 of 5: an ECS task tag VALUE longer than 256 characters is refused, although AWS stores it.
AWS: recorded behavior only — CreateSchedule accepts an EcsParameters.Tags value of 256 and of 257 characters and GetSchedule returns each unchanged. The Smithy model states no length bound on the ECS Tags map's values.
Emulator: refuses the 257-character value with 400 ValidationException "1 validation error detected: Value '<value>' at 'target.ecsParameters.tags.1.member.<key>' failed to satisfy constraint: Member must have length less than or equal to 256" — a bound AWS does not apply, reported at a member path AWS does not emit.
Why: a legal schedule cannot be created at all; the emulator is strictly stricter than AWS here.
Fix: remove the invented 256-character bound from the EcsParameters.Tags value validation.
Traces: scheduler.ecs-task-tag-value-of-257-characters-stored

Cause 5 of 5: UntagResource with an empty TagKeys list is not routed to the operation and answers a not-implemented envelope.
AWS: refuses with 400 ValidationException "1 validation error detected: Value null at 'tagKeys' failed to satisfy constraint: Member must not be null" — entirely fixed text. (Smithy: scheduler#TagKeyList @length min 0 max 200, so the bound is not what refuses it; the empty query serialization makes the member absent.) A 200-key list is accepted and removes exactly the keys named.
Emulator: answers "NotImplemented" with the message "operation DELETE /tags/<url-encoded resource ARN> is not implemented in this emulator" — i.e. with no tagKeys query parameter present the request never reaches the UntagResource handler.
Why: the wrong error class and a 501-style envelope for a request AWS answers 400; worse, the router cannot resolve UntagResource without that query parameter, so the shape of the refusal is accidental.
Fix: the scheduler crate's REST routing for DELETE /tags/{ResourceArn} (bind the operation independently of the tagKeys query parameter) plus the missing-required-member check in the UntagResource handler.
Traces: scheduler.untag-resource-tag-keys-count-bounds-rejected

## Evidence
Cause 1 — scheduler.create-schedule-ecs-awsvpc-subnet-length-bounds-rejected:
  expected ValidationException "Parameters EcsParameters not supported for the target." for a Target{Arn=<SQS queue ARN>, EcsParameters{TaskDefinitionArn, NetworkConfiguration.awsvpcConfiguration.Subnets=["s"*1000]}}
  got      HTTP 200 {"ScheduleArn": "arn:aws:scheduler:<region>:<account>:schedule/<group>/at-limit"}
  same acceptance for EcsParameters.PlacementStrategy=[{type: binpack|random|spread}], for EcsParameters.ReferenceId="r"*1024, and for UpdateSchedule with EcsParameters.TaskCount in 1..10.

Cause 2 — scheduler.create-schedule-target-role-arn-length-bounds-rejected:
  expected ValidationException "The execution role you provide must allow AWS EventBridge Scheduler to assume the role." for Target.RoleArn = "arn:aws:iam::<account>:role/" + "p"*N padded to 1600 characters (no such role exists)
  got      HTTP 200 {"ScheduleArn": "…/schedule/<group>/at-limit"}

Cause 3 — scheduler.create-schedule-ecs-tag-key-over-128-chars-rejected:
  expected ValidationException "ECS tag key must not exceed 128 characters." for EcsParameters.Tags=[{"k"*129: "team-a"}]
  got      HTTP 200 {"ScheduleArn": "…/schedule/default/<name>"}

Cause 4 — scheduler.ecs-task-tag-value-of-257-characters-stored:
  expected HTTP 200, and GetSchedule returning Target.EcsParameters.Tags == [{<key>: <257-character value>}]
  got      ValidationException "1 validation error detected: Value '<257-character value>' at 'target.ecsParameters.tags.1.member.costcentre' failed to satisfy constraint: Member must have length less than or equal to 256"

Cause 5 — scheduler.untag-resource-tag-keys-count-bounds-rejected:
  expected ValidationException "1 validation error detected: Value null at 'tagKeys' failed to satisfy constraint: Member must not be null" for UntagResource(ResourceArn=<schedule-group ARN>, TagKeys=[])
  got      NotImplemented "operation DELETE /tags/arn%3Aaws%3Ascheduler%3A<region>%3A<account>%3Aschedule-group%2F<group> is not implemented in this emulator"

## Steps to Reproduce
1. Boot the emulator; create an SQS queue, an assumable scheduler.amazonaws.com role granting sqs:SendMessage on it, and a schedule group.
2. Cause 1: CreateSchedule with Target{Arn=<queue ARN>, RoleArn=<role>, EcsParameters{TaskDefinitionArn=<any>, ReferenceId="r"*1024}} — expect ValidationException "Parameters EcsParameters not supported for the target.", observe 200. Repeat with awsvpcConfiguration.Subnets=["s"*1000], with PlacementStrategy=[{"type":"binpack"}], and with UpdateSchedule carrying EcsParameters.TaskCount=1.
3. Cause 2: CreateSchedule with Target.RoleArn = "arn:aws:iam::<account>:role/" padded with 'p' to exactly 1600 characters — expect the assume-role ValidationException, observe 200.
4. Cause 3: CreateSchedule against an ECS cluster ARN with EcsParameters.Tags=[{"k"*129: "team-a"}] — expect "ECS tag key must not exceed 128 characters.", observe 200.
5. Cause 4: CreateSchedule against an ECS cluster ARN with EcsParameters.Tags=[{"costcentre": "v"*257}] — expect 200 and GetSchedule echoing it, observe ValidationException with a 256 bound.
6. Cause 5: CreateScheduleGroup with tags, then UntagResource(ResourceArn=<group ARN>, TagKeys=[]) — expect ValidationException "Value null at 'tagKeys' …", observe NotImplemented.
