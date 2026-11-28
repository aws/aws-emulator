Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back
Issue-Key: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back|run#13|AssertionError|assert ok == [("CREATE_COMPLETE", "ok"), ("DELETE_COMPLETE", "ok")], (
Issue-Commit: b880698c4f2cb4e70545429d30696ac71b7bccc0

# cloudformation__lambda__stepfunctions: a rolled-back stack's event stream omits the CREATE_COMPLETE event for the custom resource that succeeded before its dependent sibling failed

> _Note: this issue was identified against the code base at commit `b880698c4f2cb4e70545429d30696ac71b7bccc0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
Cause 1 of 1: when a stack create fails at one resource and rolls back, DescribeStackEvents does not report a CREATE_COMPLETE event for a resource that had already been created successfully — only the rollback's DELETE_COMPLETE event for it appears. Failing check: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back.

AWS: Smithy (com.amazonaws.cloudformation): DescribeStackEvents returns StackEvent records carrying LogicalResourceId, PhysicalResourceId, ResourceType, ResourceStatus (enum ResourceStatus, which includes CREATE_IN_PROGRESS, CREATE_COMPLETE, DELETE_IN_PROGRESS, DELETE_COMPLETE) and ResourceStatusReason — 'Current status of the resource' / 'Time the status was updated', i.e. one record per status transition of each resource of the stack operation. AWS therefore emits, for every resource it provisions, the CREATE_IN_PROGRESS/CREATE_COMPLETE pair at the moment that resource finishes, and keeps those events after the stack later fails: the rollback adds DELETE_IN_PROGRESS/DELETE_COMPLETE for the same LogicalResourceId, it does not replace or drop the create events. The recorded run shows exactly that for the deployed app: the custom resource whose waiter completes (logical id 'Ok', physical id 'ok') is first CREATE_COMPLETE, then DELETE_COMPLETE once the dependent sibling ('Bad', whose isComplete handler raises) fails the stack and CloudFormation rolls back to ROLLBACK_COMPLETE. The template is an aws-cdk custom_resources.Provider with an isComplete handler plus two CustomResources where the second depends on the first.

Emulator: for that same stack the only event with a terminal _COMPLETE status for logical resource 'Ok' is ('DELETE_COMPLETE', 'ok'); the ('CREATE_COMPLETE', 'ok') event AWS emits when the resource finished being created is absent from the whole paginated DescribeStackEvents stream (the check reads every page, oldest first). The stack itself does settle in ROLLBACK_COMPLETE, so the deletion half of the history is recorded while the creation half is not.

Why: a client that reconstructs what a failed deployment actually built reads the event stream — CDK/CFN CLI progress output, drift and audit tooling, and any rollback post-mortem. With the create events missing, a resource that was really provisioned (and then really deleted) appears to have been deleted without ever existing, so a caller cannot tell which resources the failed operation created, cannot pair DELETE_COMPLETE with its CREATE_COMPLETE, and cannot report per-resource progress for the successful part of a rolled-back stack.

Fix: the CloudFormation stack-operation event recorder — the provisioning path that appends per-resource StackEvents, and whichever rollback/cleanup step currently leaves the already-recorded CREATE_* events for a resource out of (or removes them from) the stack's event history that DescribeStackEvents serves.

Related, not a duplicate: open issue 20261127T221043Z-cloudformation-lambda-stepfunctions-2-causes-the-cdk-provider.md covers the provider-framework waiter never signalling and the waiter state machine blocking rollback (ROLLBACK_FAILED) for this same check; the suite does not mark this failure as owned by it — the check now gets past those assertions (the stack reaches ROLLBACK_COMPLETE) and fails on the missing create event instead.

## Evidence
Failing check: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back [mismatch].
Assertion (check line 48): assert ok == [("CREATE_COMPLETE", "ok"), ("DELETE_COMPLETE", "ok")], where ok = [(e["ResourceStatus"], e.get("PhysicalResourceId")) for e in every DescribeStackEvents page of the stack, oldest first, if e["LogicalResourceId"] == "Ok" and e["ResourceStatus"].endswith("_COMPLETE")].
Expected (AWS): [('CREATE_COMPLETE', 'ok'), ('DELETE_COMPLETE', 'ok')] — the sibling is created, then deleted by the rollback.
Actual (emulator): AssertionError: the sibling must be created and then deleted by the rollback, its events were [('DELETE_COMPLETE', 'ok')].
The stack status itself is the expected ROLLBACK_COMPLETE, so the divergence is confined to the event history: the CREATE_COMPLETE record for that logical resource is never served. (Logical ids 'Ok'/'Bad' and physical id 'ok' come from the deployed template; the stack name, request ids and timestamps are per-run values and are not part of the requirement.)

## Steps to Reproduce
1. Deploy a CloudFormation stack with two resources where the second declares a DependsOn the first, and the second's create fails (here: `cdk deploy` of the suite's cdk_provider_iscomplete app with context mode=fail, an aws-cdk custom_resources.Provider whose isComplete handler raises for the 'Bad' resource while 'Ok' completes first).
2. Wait for the stack to settle; it reaches ROLLBACK_COMPLETE.
3. Call DescribeStackEvents for the stack id and read every page.
4. Filter the events for LogicalResourceId 'Ok': AWS shows CREATE_IN_PROGRESS/CREATE_COMPLETE followed by DELETE_IN_PROGRESS/DELETE_COMPLETE; the emulator shows only the delete events, with no CREATE_COMPLETE for that resource.
5. Or replay the check id in trace_ids.
