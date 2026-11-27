Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-waiter-holds-create-until-complete cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back
Issue-Key: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-waiter-holds-create-until-complete|_deployed#9|AssertionError|raise AssertionError(
Issue-Key: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back|_deployed#10|AssertionError|raise AssertionError(
Issue-Commit: 3c63a583c399d299d4ea298f38dfae7b708152ae

# cloudformation__lambda__stepfunctions: 2 causes — the CDK provider-framework isComplete waiter never completes its custom resource, and the failed stack cannot delete the waiter state machine (ROLLBACK_FAILED)

> _Note: this issue was identified against the code base at commit `3c63a583c399d299d4ea298f38dfae7b708152ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Cause 1 of 2: the asynchronous custom-resource waiter the CDK provider framework builds (a Step Functions state machine that polls the user's isComplete handler and then signals CloudFormation) never signals, so the custom resource is failed with "Operation timed out".
AWS: the provider framework's waiter state machine invokes the isComplete Lambda on a Wait/Choice loop and, on the first truthy IsComplete, calls back to the CloudFormation custom-resource response URL with status SUCCESS, which completes the resource and the stack (CREATE_COMPLETE). When isComplete raises instead, the waiter sends FAILED carrying that handler's error, and the resource's CREATE_FAILED ResourceStatusReason is "Received response status [FAILED] from custom resource. Message returned: <the handler's message>" — the fixed text is AWS's, the message is the handler's. Recorded behavior only for the exact CDK construct wiring.
Emulator: in both the positive and the negative case the resource is failed with the framework's own timeout instead: CREATE_FAILED "Received response status [FAILED] from custom resource. Message returned: Operation timed out (RequestId: <id>)". The success case therefore never reaches CREATE_COMPLETE, and the error case never carries the isComplete handler's message.
Why: every CDK construct built on the provider framework's asynchronous completion (custom resources that wait on a long-running operation) can never succeed, and a real handler failure is indistinguishable from a timeout.
Fix: the Step Functions execution path used by the waiter state machine — the Wait/Choice loop driving repeated Lambda invocations and the terminal callback to the custom-resource response URL.
Traces: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-waiter-holds-create-until-complete, cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back.

Cause 2 of 2: rollback of the failed stack cannot delete the provider-framework waiter state machine, so the stack ends ROLLBACK_FAILED instead of ROLLBACK_COMPLETE.
AWS: after a CREATE_FAILED resource, CloudFormation deletes the resources it created and the stack reaches ROLLBACK_COMPLETE; an AWS::StepFunctions::StateMachine created by the stack deletes cleanly (DeleteStateMachine is accepted and the machine goes away), so it does not block rollback. Recorded behavior only.
Emulator: the stack settles at ROLLBACK_FAILED with the event "The following resource(s) failed to delete: [Providerwaiterstatemachine5D4A9DF0]. ", leaving the stack in a state a client must repair by hand.
Why: a failed deployment cannot be rolled back or re-deployed — the stack is stuck in ROLLBACK_FAILED, which AWS only produces when a delete genuinely fails.
Fix: the AWS::StepFunctions::StateMachine delete handler in the CloudFormation resource provider (and the rollback path that calls it).
Traces: cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-error-fails-the-resource-and-rolls-back, cloudformation__lambda__stepfunctions.cdk-provider-framework-is-complete-waiter-holds-create-until-complete.

## Evidence
Cause 1 — expected: stack 'CREATE_COMPLETE' (waiter holds create until isComplete returns true) / the resource's CREATE_FAILED reason carrying the isComplete handler's error.
Actual (both checks): CREATE_FAILED event ('Ok', 'CREATE_FAILED', 'Received response status [FAILED] from custom resource. Message returned: Operation timed out (RequestId: ...)').
Cause 2 — expected: 'ROLLBACK_COMPLETE'. Actual: "stack '<name>' is 'ROLLBACK_FAILED'" with event ('<stack>', 'ROLLBACK_FAILED', 'The following resource(s) failed to delete: [Providerwaiterstatemachine5D4A9DF0]. ').

## Steps to Reproduce
1. `cdk deploy` a stack using the CDK provider framework with an isComplete handler (aws-cdk custom-resources Provider with isCompleteHandler).
2. Watch the custom resource: it is failed with 'Operation timed out' instead of completing on the first truthy IsComplete (cause 1).
3. Let the stack roll back: it settles ROLLBACK_FAILED because the provider waiter state machine cannot be deleted (cause 2).
4. Or replay the check ids in trace_ids.
