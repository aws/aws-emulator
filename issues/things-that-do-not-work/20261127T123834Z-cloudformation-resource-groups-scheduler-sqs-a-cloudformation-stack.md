Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__resource-groups__scheduler__sqs.deploy-stack-group-omits-schedule-group-lists-queue
Issue-Key: cloudformation__resource-groups__scheduler__sqs.deploy-stack-group-omits-schedule-group-lists-queue|run#22|AssertionError|assert members == {(queue_arn, "AWS::SQS::Queue")}, (
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# cloudformation__resource-groups__scheduler__sqs: a CLOUDFORMATION_STACK_1_0 resource group lists the stack's AWS::Scheduler::ScheduleGroup, which Resource Groups does not support

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListGroupResources

## Description
One net-new cause.

Cause 1 of 1: a stack-based resource group (ResourceQuery Type CLOUDFORMATION_STACK_1_0 with ResourceTypeFilters ["AWS::AllSupported"]) lists the stack's schedule group as a member.
AWS: AWS::AllSupported means every resource type Resource Groups SUPPORTS, and AWS::Scheduler::ScheduleGroup is not one of them. So for a stack holding exactly one AWS::SQS::Queue and one AWS::Scheduler::ScheduleGroup, ListGroupResources on the stack-based group returns exactly one Resources entry — Identifier {ResourceArn: the queue's ARN, ResourceType: "AWS::SQS::Queue"} — and never an entry for the schedule group, even though CloudFormation's own ListStackResources reports both resource types for that stack. Reference: Resource Groups user guide, supported resources.
Emulator: ListGroupResources returns two entries, ResourceType "AWS::SQS::Queue" and "AWS::Scheduler::ScheduleGroup", the latter carrying the schedule group's ARN.
Why: a client enumerating a stack through Resource Groups receives a resource type the API does not support, so group membership counts, tag-on-group operations and any per-type dispatch over the listing see a member AWS would never return.
Fix: the resource-groups crate's supported-resource-type table, applied when a CLOUDFORMATION_STACK_1_0 query expands AWS::AllSupported over a stack's resources.
Traces: cloudformation__resource-groups__scheduler__sqs.deploy-stack-group-omits-schedule-group-lists-queue

## Evidence
cloudformation__resource-groups__scheduler__sqs.deploy-stack-group-omits-schedule-group-lists-queue:
  stack (CREATE_COMPLETE) holds resource types {AWS::Scheduler::ScheduleGroup, AWS::SQS::Queue}; resource group created with ResourceQuery {Type: "CLOUDFORMATION_STACK_1_0", Query: {"ResourceTypeFilters": ["AWS::AllSupported"], "StackIdentifier": "<stack id>"}}
  expected ListGroupResources to return exactly {(<queue ARN>, "AWS::SQS::Queue")}
  got      "the stack-based group should list only the queue; it lists ['AWS::SQS::Queue', 'AWS::Scheduler::ScheduleGroup'], schedule group included: True"

## Steps to Reproduce
1. Boot the emulator. CreateStack with a template holding one AWS::Scheduler::ScheduleGroup and one AWS::SQS::Queue; wait for CREATE_COMPLETE.
2. resource-groups CreateGroup with ResourceQuery {Type: "CLOUDFORMATION_STACK_1_0", Query: json of {"ResourceTypeFilters": ["AWS::AllSupported"], "StackIdentifier": "<stack id>"}}.
3. Poll ListGroupResources until the queue appears, then inspect the full member set — expect only the queue; observe the schedule group listed as AWS::Scheduler::ScheduleGroup too.
