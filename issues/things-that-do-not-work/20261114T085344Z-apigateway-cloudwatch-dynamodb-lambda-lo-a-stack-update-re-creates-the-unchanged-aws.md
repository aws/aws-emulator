Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree|_wait_stack#11|AssertionError|raise AssertionError(
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config|_wait_stack#11|AssertionError|raise AssertionError(
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# A stack update re-creates the unchanged AWS::Lambda::EventSourceMapping, so Lambda's 409 ResourceConflict rolls the whole update back

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: an UpdateStack whose template leaves AWS::Lambda::EventSourceMapping unchanged issues a fresh CreateEventSourceMapping for the same (EventSourceArn, FunctionName) pair, which Lambda correctly refuses with 409, and the stack rolls back.

AWS: an UpdateStack compares the resource's declared properties against the deployed ones; an AWS::Lambda::EventSourceMapping whose properties are unchanged by the update is a no-op (no create, no replacement) and the stack reaches UPDATE_COMPLETE. Lambda itself only ever answers 409 when a SECOND mapping is requested for a pair that already has one. Smithy: CreateEventSourceMapping errors include ResourceConflictException (errorKind client, httpError 409); its message for this class is the fixed text "An event source mapping with SQS arn (" <queue arn> ") and function (" <function name> ") already exists. Please update or delete the existing mapping with UUID " <uuid>, where the queue ARN, function name and UUID are values from the request and from the already-stored mapping. The refusal itself is faithful — the defect is that CloudFormation asks for it at all.

Emulator: during the update phase of the CDK-synthesized, asset-free order-service stack the event-source-mapping resource (logical id WorkerFunctionSqsEventSourceOrderServiceStackQueueA54EB36191D337BC) is handed to a create, Lambda answers 409 ResourceConflict, the resource reports CREATE/UPDATE_FAILED with HandlerErrorCode ResourceConflict and the stack ends UPDATE_ROLLBACK_COMPLETE instead of UPDATE_COMPLETE.

Why: a client that updates any part of a CDK/SAM app containing an SQS event source can never land a second deployment — every update of the stack rolls back, so the whole iterate-and-redeploy workflow that real AWS supports is unavailable, and the rollback masks the update that was actually requested.

Fix: the CloudFormation engine's update planner / resource-diff for AWS::Lambda::EventSourceMapping — an unchanged mapping must take the no-op path (and a changed one the update path) rather than being re-created; belongs with the CFN resource handler for that type, not with Lambda's CreateEventSourceMapping validation, which behaves correctly.

Traces: both checks below fail identically, at the _update_cfn/_wait_stack step, with the same 409 ResourceConflict on the same logical id. Note the existing open issue 20261113T185449Z-apigateway-cloudwatch-dynamodb-lambda-lo-...-the-aws.md covers a DIFFERENT cause in the same two checks (the AWS::CloudWatch::Alarm never being created); it does not cover this rollback, which happens earlier in the run.

## Evidence
Expected: UpdateStack on the already-deployed order-service stack reaches UPDATE_COMPLETE; the unchanged event-source-mapping resource is not re-created.
Actual (replay): AssertionError: stack <stack> ended in UPDATE_ROLLBACK_COMPLETE: WorkerFunctionSqsEventSourceOrderServiceStackQueueA54EB36191D337BC: Resource handler returned message: "An event source mapping with SQS arn (" <queue arn> ") and function (" <worker function> ") already exists. Please update or delete the existing mapping with UUID <uuid> (Service: Lambda, Status Code: 409, Request ID: <id>) (SDK Attempt Count: 1)" (RequestToken: <token>, HandlerErrorCode: ResourceConflict). The ARNs, names, UUID, request id and token are per-run values; the invariant is the 409 conflict being provoked by the update at all.

## Steps to Reproduce
1. Deploy the CDK-synthesized asset-free order-service stack (API Gateway + DynamoDB + Lambda worker + SQS queue + CloudWatch alarm) with CreateStack and wait for CREATE_COMPLETE.
2. Call UpdateStack with the same template body (the event-source-mapping resource unchanged).
3. Poll DescribeStacks: the stack reaches UPDATE_ROLLBACK_COMPLETE, and DescribeStackEvents shows the event-source-mapping resource failed with Lambda's 409 ResourceConflictException. On AWS the update is a no-op for that resource and the stack reaches UPDATE_COMPLETE.
