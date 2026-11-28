Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam|_status#11|Timeout|return cfn.describe_stacks(StackName=stack_name)["Stacks"][0]["StackStatus"]
Issue-Commit: 6a8f3c89e8157434acbd53c2a8e69f6d46f01430

# apigateway__cloudwatch__dynamodb__lambda__logs__sqs: the SAM order-service deployment still never completes — the check is killed at its 600 s cap with no terminal StackStatus and no graded leg (1 cause)

> _Note: this issue was identified against the code base at commit `6a8f3c89e8157434acbd53c2a8e69f6d46f01430`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the order-intake reference application deployed from an AWS::Serverless-2016-10-31 (SAM) template never converges and never answers, so the check produces no verdict at all — its entire output is one timeout line and not one of its eight legs (deploy, wiring, worker, logs, metrics, alarm, drift, teardown) is graded.

Related open issues exist for this same check but the suite reports it as not owned by any of them, so none carries this failure's replay key: 20261024T043741Z-apigateway-cloudwatch-dynamodb-lambda-lo-...-the-sam.md (key '...xflavor-order-service-sam|_post_order#5|Timeout|time.sleep(5.0)' at a 60 s cap) and 20261121T044528Z-apigateway-cloudwatch-dynamodb-lambda-lo-...-both-order.md cause 2 (key '...xflavor-order-service-sam|_poll_until#11|Timeout|time.sleep(interval_s)'). The hang persists at the 600 s cap and at a different frame, so it is filed here and should be closed together with that cause.

AWS: Smithy (cloudformation) DescribeStacks -> Stack.StackStatus targets com.amazonaws.cloudformation#StackStatus, whose terminal members are CREATE_COMPLETE, CREATE_FAILED, ROLLBACK_COMPLETE, ROLLBACK_FAILED, UPDATE_COMPLETE, UPDATE_FAILED, UPDATE_ROLLBACK_COMPLETE, UPDATE_ROLLBACK_FAILED, DELETE_COMPLETE, DELETE_FAILED. A CreateStack with CAPABILITY_AUTO_EXPAND of this template (REST API + intake and worker Lambda functions + DynamoDB table + SQS queue + event-source mapping + log group + CloudWatch alarm) always leaves *_IN_PROGRESS for one of those terminal statuses in bounded time, the method/AWS_PROXY integration and lambda:InvokeFunction permission synthesized from the function's Events block serve POST /orders at the stage invoke URL, and every call on that path (CreateStack, DescribeStacks, DescribeStackEvents, the stage request) returns a response. Recorded behavior only for the individual leg assertions.

Emulator: the check's whole output is 'Error: Check timed out after 600.0s'. No terminal StackStatus is ever observed and no HTTP status line from the deployed stage is ever observed, i.e. the stack stays in an *_IN_PROGRESS status (or a call on the deploy path never returns) for longer than the entire check budget. The check's own per-leg ceilings are all smaller than that budget (deploy 600 s, route 180 s, worker 180 s, logs 240 s, metrics 300 s), so this is the absence of any response rather than a leg assertion firing. Hypothesis: the AWS::Serverless::* resources are expanded late or not at all, so the synthesized API/integration/permission/event-source-mapping resources never exist and the stack never settles.

Why: a client deploying from SAM gets neither a converged stack nor an error it can branch on — every DescribeStacks waiter blocks indefinitely, no rollback reason is available to diagnose, and the whole downstream behavior of the application (queue delivery, structured logs, metric datapoint, alarm transition, in-place drift update, teardown) is unobservable.

Fix: the AWS::Serverless-2016-10-31 transform expansion in the cloudformation crate (it must materialize the API, method/integration, lambda permission and event-source-mapping resources the template only declares under Events) and the stack-progression state machine, which must always reach a modeled terminal StackStatus — including landing a failing nested resource handler as *_FAILED/ROLLBACK_COMPLETE with a resource-status reason — plus the execute-api stage request path, which must always produce a response rather than hang.

Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam.

## Evidence
Expected: CreateStack of order_service_sam.yaml with CAPABILITY_IAM/CAPABILITY_NAMED_IAM/CAPABILITY_AUTO_EXPAND reaches CREATE_COMPLETE, POST /orders at the stage invoke URL answers 2xx, and the eight legs (deploy, wiring, worker, logs, metrics, alarm, drift, teardown) are each graded.

Actual: the check's only output is 'Error: Check timed out after 600.0s' — no terminal StackStatus from DescribeStacks, no HTTP status line, no body, no leg verdict printed.

## Steps to Reproduce
1. CreateStack the order-intake AWS::Serverless-2016-10-31 template (REST API POST /orders -> intake function -> SQS queue -> worker function -> DynamoDB table, plus log group and CloudWatch alarm) with Capabilities CAPABILITY_IAM, CAPABILITY_NAMED_IAM, CAPABILITY_AUTO_EXPAND.
2. Poll DescribeStacks for a terminal StackStatus: none arrives.
3. POST an order at the stage invoke URL: no response.
4. Re-replay apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-sam; it must reach a terminal StackStatus and then run its wiring/worker/logs/metrics/alarm/drift/teardown legs.
