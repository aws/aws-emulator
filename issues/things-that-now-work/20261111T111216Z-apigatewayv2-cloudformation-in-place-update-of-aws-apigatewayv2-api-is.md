Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudformation.cfn-apigatewayv2-api-in-place-description-retained-protocol-replacement-then-not-found
Issue-Key: apigatewayv2__cloudformation.cfn-apigatewayv2-api-in-place-description-retained-protocol-replacement-then-not-found|_expect#16|AssertionError|raise AssertionError(
Issue-Commit: 9e6dcc54d456fb2455bf69889afaa61e9a9cc966

# In-place update of AWS::ApiGatewayV2::Api is refused with an emulator-authored NotImplemented reason

> _Note: this issue was identified against the code base at commit `9e6dcc54d456fb2455bf69889afaa61e9a9cc966`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: changing a mutable property of an HTTP/WebSocket API through CloudFormation is refused instead of applied.

AWS: recorded behavior only. An UpdateStack that changes AWS::ApiGatewayV2::Api.Description (a mutable property) is an in-place update — ApiGatewayV2 UpdateApi applies it, the ApiId is unchanged, the resource event goes UPDATE_IN_PROGRESS -> UPDATE_COMPLETE and the stack reaches UPDATE_COMPLETE. Only an immutable property (ProtocolType) forces replacement.

Emulator: the update fails the resource. Stack status 'UPDATE_ROLLBACK_COMPLETE', StatusReason 'The following resource(s) failed to update: [Api]. ', and the resource event is UPDATE_FAILED with reason 'NotImplemented: this emulator does not change the properties of an existing AWS::ApiGatewayV2::Api in place; the resource was left as it is.' The fixed text is 'NotImplemented: this emulator does not change the properties of an existing ' ... ' in place; the resource was left as it is.'; the variable part is the resource type. AWS has no such error.

Why: a client cannot edit an HTTP API's description (or any other mutable property) through CloudFormation/CDK/SAM; every such deploy rolls back, so the ProtocolType-forces-replacement behaviour that follows it can never be exercised either.

Fix: the AWS::ApiGatewayV2::Api resource handler's update path in the CloudFormation provisioner — classify mutable-property changes as in-place and call UpdateApi.

Traces: the failing check in trace_ids.

## Evidence
Expected: UPDATE_COMPLETE with the ApiId retained.
Actual: AssertionError: stack ended 'UPDATE_ROLLBACK_COMPLETE' ('The following resource(s) failed to update: [Api]. '), expected 'UPDATE_COMPLETE'; failed events: [('Api', 'UPDATE_FAILED', 'NotImplemented: this emulator does not change the properties of an existing AWS::ApiGatewayV2::Api in place; the resource was left as it is.')]

## Steps to Reproduce
1. CreateStack with an AWS::ApiGatewayV2::Api (ProtocolType HTTP); wait for CREATE_COMPLETE.
2. UpdateStack changing only Description.
3. DescribeStackEvents: Api is UPDATE_FAILED with the NotImplemented reason; the stack is UPDATE_ROLLBACK_COMPLETE.
