Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__resource-groups.deploy-resourcegroups-group-duplicate-name-rejected-leaving-the-original
Issue-Key: cloudformation__resource-groups.deploy-resourcegroups-group-duplicate-name-rejected-leaving-the-original|failed_reason#24|AssertionError|assert len(rows) == 1, f"expected one {status} row for {logical_id}, got {rows!r}"
Issue-Commit: a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c

# A stack whose create is rejected by CloudFormation's pre-create validation records no stack-level CREATE_FAILED event, so the rollback carries no reason

> _Note: this issue was identified against the code base at commit `a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
One net-new cause behind the surviving cloudformation__resource-groups failure. The verdict itself is right — a second stack whose AWS::ResourceGroups::Group reuses a name resource-groups already holds settles ROLLBACK_COMPLETE — but the event record that explains the rejection is absent.

Cause 1 of 1: DescribeStackEvents for a stack rejected by CloudFormation's own pre-create validation contains no CREATE_FAILED row for the stack itself, so the failure reason is unreportable.
AWS: when a create is refused by CloudFormation's pre-create (name-conflict) validation, DescribeStackEvents carries exactly one row whose LogicalResourceId is the stack name, ResourceStatus is CREATE_FAILED and ResourceStatusReason is the validation summary — fixed text "Validation failed with N error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation.", where N is the number of validation errors found for that request (1 for a single duplicate name) and everything else is invariant. Smithy (com-amazonaws-cloudformation): StackEvent.ResourceStatusReason is "Success/failure message associated with the resource", and StackEvent.DetailedStatus documents VALIDATION_FAILED as "template validation failed because of invalid properties in the template. The ResourceStatusReason field shows what properties are defined incorrectly." The per-error detail is served by DescribeEvents as an OperationEvents row with EventType VALIDATION_ERROR, ValidationName NAME_CONFLICT_VALIDATION, ValidationStatus FAILED, ValidationFailureMode FAIL, ValidationPath "/Resources/<logical-id>" and ValidationStatusReason "Resource of type '<resource type>' with identifier '<identifier>' already exists." — fixed text with the resource type and the conflicting identifier taken from the request's template (here AWS::ResourceGroups::Group and the Name property).
Emulator: the rolled-back stack's DescribeStackEvents yields no CREATE_FAILED row at all for the stack's own LogicalResourceId (the filtered list is empty), so there is no ResourceStatusReason to read anywhere. Because the check cannot get past that, the DescribeEvents VALIDATION_ERROR rows and the "no resource was provisioned" assertion for the rejected stack were never reached and are not asserted here.
Why: every client and tool that reports why a deploy failed (the CLI's `cloudformation deploy`/`describe-stack-events`, CDK, Terraform's CFN resource, CI gates) reads the stack's CREATE_FAILED row to surface the reason. With the row missing, a rejected stack is indistinguishable from one that failed for an unknown cause: the operator sees ROLLBACK_COMPLETE and no diagnosis, and automation that matches on the validation summary or on NAME_CONFLICT_VALIDATION never fires.
Fix: the CloudFormation stack-create pre-validation path and the stack event log it writes — a validation refusal must append a stack-level CREATE_FAILED event (LogicalResourceId = stack name, PhysicalResourceId = stack id, ResourceType AWS::CloudFormation::Stack) carrying the validation summary as ResourceStatusReason before the rollback transition, and record the matching per-error VALIDATION_ERROR operation event DescribeEvents serves.
Traces: cloudformation__resource-groups.deploy-resourcegroups-group-duplicate-name-rejected-leaving-the-original

## Evidence
Check cloudformation__resource-groups.deploy-resourcegroups-group-duplicate-name-rejected-leaving-the-original

Stack A creates an AWS::ResourceGroups::Group named <group-name> and reaches CREATE_COMPLETE; GetGroup/GetGroupQuery read it back correctly (positive control passes). Stack B declares the same group Name and correctly settles ROLLBACK_COMPLETE.

Expected (AWS): DescribeStackEvents(StackName=<stack B id>) contains exactly one row with LogicalResourceId == <stack B name>, ResourceStatus == CREATE_FAILED and ResourceStatusReason == "Validation failed with 1 error(s). Call DescribeEvents to retrieve the full list of issues with resource and property details, resolve each error, then retry the operation."
Actual (emulator): no such row exists — the filtered rows list is empty.

AssertionError: expected one CREATE_FAILED row for <stack B name>, got []
(raised from failed_reason(stack_events(cf, stack_b_id), stack_b); recorded run named the stack 'sim2real-behav-cfn-dupg-b-34b7cccd0fef' — a per-run generated name, not part of the requirement)

Not asserted because the check stopped here: the DescribeEvents VALIDATION_ERROR row (NAME_CONFLICT_VALIDATION / FAILED / FAIL / "Resource of type 'AWS::ResourceGroups::Group' with identifier '<group-name>' already exists." / "/Resources/Group") and ListStackResources == [] for the rejected stack.

## Steps to Reproduce
1. CreateStack A with a template declaring one AWS::ResourceGroups::Group with Name=<group-name>, a Description and a TAG_FILTERS_1_0 ResourceQuery; wait for CREATE_COMPLETE.
2. CreateStack B with the same group Name (different Description); wait for a terminal status — ROLLBACK_COMPLETE is reached, which is correct.
3. Call DescribeStackEvents(StackName=<stack B id>) and look for the row whose LogicalResourceId equals the stack name and whose ResourceStatus is CREATE_FAILED: no such row is returned, so no ResourceStatusReason explains the rejection.
4. Call DescribeEvents(StackName=<stack B id>) and look for an OperationEvents row with EventType VALIDATION_ERROR / ValidationName NAME_CONFLICT_VALIDATION.
5. Re-replay the check in trace_ids.
