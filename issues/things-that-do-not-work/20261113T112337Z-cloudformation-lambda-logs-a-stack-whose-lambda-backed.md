Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources
Issue-Key: cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources|_poll_for_log_line#16|Timeout|time.sleep(interval_s)
Issue-Commit: 0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225

# cloudformation__lambda__logs: a stack whose Lambda-backed custom resource signals FAILED never settles — the verbatim failure Reason and the Delete-on-rollback/Delete-on-delete behaviour reach no verdict

> _Note: this issue was identified against the code base at commit `0e1ab0d3410bad8cabf8edfe5a86b78af8e5e225`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the scenario that grades a custom resource's FAILED Reason and the Delete invocations that follow never completes — the check exhausts the harness budget with no stack outcome. Failing check: cloudformation__lambda__logs.cfn-custom-resource-failure-reason-exact-and-delete-removes-resources.

AWS: recorded behavior only for the end-to-end flow; the modelled surface is cloudformation CreateStack/DescribeStackEvents/DeleteStack. On AWS, a Lambda-backed AWS::CloudFormation::CustomResource whose provider answers the Create with Status FAILED and a Reason string causes the stack to go CREATE_FAILED and then roll back, and the ResourceStatusReason of that resource's event is the provider's Reason verbatim — not an emulator-authored paraphrase. Both the automatic rollback and a later explicit DeleteStack invoke the provider again with RequestType 'Delete', and when the stack is gone neither the function nor its /aws/lambda/<function> log group is left behind. All of this settles in bounded time: the create reaches a terminal status, the events are readable, and the delete converges to DELETE_COMPLETE.

Emulator: 'Check timed out after 60.0s'. The stack never reaches a terminal status the check can read, so neither the verbatim Reason nor the Delete-request behaviour nor the leftover-resource assertions are ever evaluated.

Why: a caller deploying a template with a custom resource that fails gets a stack that never settles instead of CREATE_FAILED with the provider's own Reason, so deploy tooling blocks forever, the diagnostic text a provider sends is lost, and the Delete lifecycle (and the cleanup it performs) cannot be relied on.

Fix: the CloudFormation custom-resource provisioner — it must consume the provider's response signal (Status/Reason), drive the resource to CREATE_FAILED with that Reason recorded verbatim on the stack event, roll the stack back, and invoke the provider with RequestType 'Delete' on both rollback and DeleteStack, converging to a terminal stack status in bounded time.

Related open issue that does NOT cover this check: 20261109T143515Z-cloudformation-lambda-logs-4-net-new-causes-change.md.

Traces: the failing check id above.

## Evidence
cfn-custom-resource-failure-reason-exact-and-delete-removes-resources — Error: Check timed out after 60.0s.

Expected (AWS): CreateStack with a custom resource whose provider answers {"Status":"FAILED","Reason":"<reason>"} ends CREATE_FAILED/ROLLBACK_COMPLETE with that <reason> appearing verbatim as the resource event's ResourceStatusReason; the provider is invoked with RequestType 'Delete' on the rollback and again on DeleteStack; after DELETE_COMPLETE neither the function nor /aws/lambda/<function> remains. Actual: the stack never reaches a terminal status inside the budget, so none of it is observable.

## Steps to Reproduce
1. Create a Lambda function that answers a CloudFormation custom-resource Create with Status FAILED and a known Reason string, and a template declaring AWS::CloudFormation::CustomResource with that function as ServiceToken.
2. Call CreateStack and poll DescribeStacks/DescribeStackEvents for a terminal status and the resource's ResourceStatusReason.
3. Observe that no terminal status arrives; the check hits its 60 s cap before it can compare the Reason or check the Delete invocations and leftover resources.
