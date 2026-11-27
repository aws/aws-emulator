Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__scheduler.deploy-schedule-group-creation-date-set-by-service-not-template cloudformation__scheduler.deploy-schedule-group-ignores-a-templated-last-modification-date-and-getatt-resolves-it cloudformation__scheduler.deploy-schedule-tags-property-rejected cloudformation__scheduler.deploy-stack-tags-reach-group-not-schedule
Issue-Key: cloudformation__scheduler.deploy-schedule-group-creation-date-set-by-service-not-template|run#11|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__scheduler.deploy-schedule-group-ignores-a-templated-last-modification-date-and-getatt-resolves-it|run#12|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in desc["Outputs"]}
Issue-Key: cloudformation__scheduler.deploy-schedule-tags-property-rejected|run#14|AssertionError|assert tagged_end == "ROLLBACK_COMPLETE", f"stack with Schedule Tags ended {tagged_end!r}"
Issue-Key: cloudformation__scheduler.deploy-stack-tags-reach-group-not-schedule|run#14|AssertionError|assert seen == expected, f"group tags {seen!r}, expected {expected!r}"
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# cloudformation__scheduler: Fn::GetAtt on AWS::Scheduler::ScheduleGroup resolves to no OutputValue, the Schedule resource's Tags property is not rejected, and the CloudFormation-injected tags are missing from the group

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new causes in CloudFormation's handling of the AWS::Scheduler::* resource types.

Cause 1 of 3: an Output whose Value is an Fn::GetAtt on an AWS::Scheduler::ScheduleGroup resolves to nothing, so DescribeStacks returns an Outputs entry with no OutputValue member.
AWS: AWS::Scheduler::ScheduleGroup exposes the attributes Arn, CreationDate, LastModificationDate and State, and a CREATE_COMPLETE stack's DescribeStacks Outputs carry each one as {OutputKey, OutputValue}. The values are the service's own and agree with GetScheduleGroup: Arn ends ":schedule-group/<name>"; State is "ACTIVE"; CreationDate and LastModificationDate are the group's own timestamps rendered in the millisecond UTC form "%Y-%m-%dT%H:%M:%S.mmmZ", and a template value supplied for either date is ignored (the service's timestamp wins, and a fresh group's LastModificationDate equals its CreationDate). Reference: AWS::Scheduler::ScheduleGroup template reference.
Emulator: the stack reaches CREATE_COMPLETE but every Outputs entry lacks OutputValue, so reading o["OutputValue"] raises KeyError. Both the plain CreateStack path and the change-set (CREATE change set then ExecuteChangeSet) path behave the same way.
Why: any template that publishes a schedule group's ARN, state or timestamps as a stack output, or that cross-references them with Fn::GetAtt, gets an unusable output; dependent resources and nested stacks read nothing.
Fix: the AWS::Scheduler::ScheduleGroup resource provider's attribute resolution in the cloudformation crate (the GetAtt attribute map), and the Outputs serializer that must always carry OutputValue.
Traces: cloudformation__scheduler.deploy-schedule-group-creation-date-set-by-service-not-template, cloudformation__scheduler.deploy-schedule-group-ignores-a-templated-last-modification-date-and-getatt-resolves-it

Cause 2 of 3: a template that gives AWS::Scheduler::Schedule a Tags property is deployed instead of being refused.
AWS: AWS::Scheduler::Schedule has no Tags property, so the template fails property validation: the stack ends ROLLBACK_COMPLETE, DescribeStackResources on it returns no resources, the schedule does not exist (GetSchedule answers ResourceNotFoundException "Schedule <name> does not exist."), and the stack's operation events include one VALIDATION_ERROR event for the offending resource with ValidationName "PROPERTY_VALIDATION", ValidationStatus "FAILED", ValidationFailureMode "FAIL" and ValidationPath "/Resources/<LogicalId>/Properties/Tags". The identical template without Tags deploys cleanly with no VALIDATION_ERROR events. Reference: AWS::Scheduler::Schedule template reference.
Emulator: the stack carrying the Tags property ends CREATE_COMPLETE.
Why: a template that is invalid against the resource schema deploys, so the typo is never reported and the stack's contents differ from what the author declared.
Fix: the AWS::Scheduler::Schedule resource provider's property schema / unknown-property validation in the cloudformation crate, and the VALIDATION_ERROR event it must emit.
Traces: cloudformation__scheduler.deploy-schedule-tags-property-rejected

Cause 3 of 3: the three CloudFormation-injected tags are not applied to a stack-created AWS::Scheduler::ScheduleGroup.
AWS: every resource CloudFormation creates for a stack carries, in addition to the stack-level Tags and the resource's own Tags, the three reserved tags aws:cloudformation:stack-name (the stack's name), aws:cloudformation:stack-id (the stack's ARN) and aws:cloudformation:logical-id (the resource's logical id in the template). So the group's tag set read through Scheduler is the stack Tags plus the resource's Tags plus those three.
Emulator: the group carries only the stack-level/resource tags — the three aws:cloudformation:* tags are absent.
Why: tag-driven cost allocation, stack-scoped automation and drift tooling all key on those three tags; without them a stack-created schedule group cannot be attributed to its stack or to its logical id.
Fix: the cloudformation crate's resource-creation path — inject the three reserved tags into every created resource's tag set.
Traces: cloudformation__scheduler.deploy-stack-tags-reach-group-not-schedule

## Evidence
Cause 1 — cloudformation__scheduler.deploy-schedule-group-creation-date-set-by-service-not-template:
  template: one AWS::Scheduler::ScheduleGroup with Outputs {Arn, CreationDate, LastModificationDate, State} each {"Value": {"Fn::GetAtt": ["Folder", <attr>]}}
  stack status CREATE_COMPLETE, then `{o["OutputKey"]: o["OutputValue"] for o in stack["Outputs"]}` -> KeyError: 'OutputValue'
  cloudformation__scheduler.deploy-schedule-group-ignores-a-templated-last-modification-date-and-getatt-resolves-it: same KeyError: 'OutputValue' on the change-set path, for Outputs Created/Modified/State

Cause 2 — cloudformation__scheduler.deploy-schedule-tags-property-rejected:
  expected the stack with Properties.Tags on the AWS::Scheduler::Schedule to end "ROLLBACK_COMPLETE", with one VALIDATION_ERROR event ("Timer", "AWS::Scheduler::Schedule", "PROPERTY_VALIDATION", "FAILED", "FAIL", "/Resources/Timer/Properties/Tags") and no stack resources
  got      AssertionError: stack with Schedule Tags ended 'CREATE_COMPLETE'

Cause 3 — cloudformation__scheduler.deploy-stack-tags-reach-group-not-schedule:
  expected the group's tags to be the stack/resource tags PLUS aws:cloudformation:stack-name = <stack name>, aws:cloudformation:stack-id = arn:aws:cloudformation:<region>:<account>:stack/<stack name>/<uuid>, aws:cloudformation:logical-id = <logical id>
  got      group tags {'sim2real': '<stack name>', 'cost-center': '<value>'} — the three aws:cloudformation:* tags absent

## Steps to Reproduce
1. Boot the emulator.
2. Cause 1: CreateStack with a template holding one AWS::Scheduler::ScheduleGroup (Properties.Name set, and also a CreationDate or LastModificationDate value) and Outputs mapping Arn/CreationDate/LastModificationDate/State to Fn::GetAtt on it. Wait for CREATE_COMPLETE and read DescribeStacks Stacks[0].Outputs — expect {OutputKey, OutputValue} pairs agreeing with GetScheduleGroup, observe entries with no OutputValue. Repeat via CreateChangeSet(ChangeSetType=CREATE) + ExecuteChangeSet.
3. Cause 2: CreateStack with a template whose AWS::Scheduler::Schedule resource has a Tags property — expect ROLLBACK_COMPLETE plus a PROPERTY_VALIDATION VALIDATION_ERROR event at /Resources/<LogicalId>/Properties/Tags, observe CREATE_COMPLETE. The identical template without Tags is the control and deploys cleanly.
4. Cause 3: CreateStack with stack-level Tags and an AWS::Scheduler::ScheduleGroup resource, then scheduler ListTagsForResource on the group ARN — expect the stack tags plus aws:cloudformation:stack-name, aws:cloudformation:stack-id and aws:cloudformation:logical-id; observe only the stack tags.
