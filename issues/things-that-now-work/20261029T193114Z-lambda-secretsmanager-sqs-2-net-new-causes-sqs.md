Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret lambda__secretsmanager__sqs.pipeline-sqs-secret-sqs-chain lambda__secretsmanager__sqs.sqs-batch-message-format lambda__secretsmanager__sqs.sqs-lambda-basic-trigger lambda__secretsmanager__sqs.sqs-esm-missing-permission-fails
Issue-Key: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: lambda__secretsmanager__sqs.pipeline-sqs-secret-sqs-chain|_wait_for_messages#12|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__secretsmanager__sqs.sqs-batch-message-format|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__secretsmanager__sqs.sqs-lambda-basic-trigger|_wait_for_messages#8|Timeout|resp = sqs.receive_message(
Issue-Key: lambda__secretsmanager__sqs.sqs-esm-missing-permission-fails|run#7|AssertionError|raise AssertionError(
Issue-Commit: 6712e9b9ed86b1379f14acf29b1637f3b8f9994b

# lambda__secretsmanager__sqs: 2 net-new causes — SQS event source mappings never invoke the function, and CreateEventSourceMapping accepts a role with no SQS read permission

> _Note: this issue was identified against the code base at commit `6712e9b9ed86b1379f14acf29b1637f3b8f9994b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Two net-new root causes. The open issue 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md owns the direct-Invoke checks in this group but not the replay keys below.

Cause 1 of 2: an SQS event source mapping never produces an invocation, so every queue->function chain in this group stalls.
AWS: recorded behavior only for the poller; the model fixes the surface (lambda CreateEventSourceMapping takes EventSourceArn/FunctionName/BatchSize and reports State, which AWS drives to Enabled). AWS then polls the queue and invokes the function with an SQS Records envelope (one record per message, up to BatchSize), deleting the messages the handler processed.
Emulator: the mapping is created and reported, but no invocation ever happens and the messages are never consumed, so the four delivery checks are killed at the 60s harness budget.
Why: the queue -> function -> secret -> queue chain these checks exercise never starts, so neither the batch record format nor any downstream effect can be observed; the control plane's "healthy mapping" is misleading.
Fix: the lambda event-source-mapping poller/runtime — dequeue from the source, build the Records envelope, invoke the function, then delete or re-expose messages per the handler result.
Traces: lambda__secretsmanager__sqs.deployment-cfn-sqs-lambda-secret, lambda__secretsmanager__sqs.pipeline-sqs-secret-sqs-chain, lambda__secretsmanager__sqs.sqs-batch-message-format, lambda__secretsmanager__sqs.sqs-lambda-basic-trigger

Cause 2 of 2: CreateEventSourceMapping accepts a function whose execution role lacks the SQS read permissions.
AWS: refuses with HTTP 400 and Error.Code `InvalidParameterValueException` (modelled on lambda CreateEventSourceMapping), the message naming the role ARN from the request and the permission it is missing for the source queue.
Emulator: the call succeeds and returns a UUID.
Why: a mapping that can never poll is created and reported healthy, so the create-time check that normally catches a role missing sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes no longer fails the deployment.
Fix: the lambda CreateEventSourceMapping handler's validation — resolve the function's execution role and verify the source-queue read actions before accepting.
Traces: lambda__secretsmanager__sqs.sqs-esm-missing-permission-fails

## Evidence
Cause 1 — `Error: Check timed out after 60.0s` on all four delivery checks; no invocation and no message consumption observed.
Cause 2 — `AssertionError: CreateEventSourceMapping should reject a role lacking SQS read permissions, but it succeeded and returned UUID 'd78ccaf5-7c99-44c1-9658-883ee4fed469'`.

## Steps to Reproduce
Cause 1: CreateQueue, CreateFunction, CreateEventSourceMapping(EventSourceArn=<queue arn>, FunctionName=<fn>), SendMessage, then wait for the handler's side effect — nothing happens.
Cause 2: create a role with no sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes, create a function with it, then CreateEventSourceMapping against an SQS source — the call succeeds.
