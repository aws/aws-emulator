Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__sqs.cfn-event-source-mapping-getatt-arn-matches-the-mapping cloudformation__lambda__sqs.sam-dead-letter-queue-grants-the-role-and-deploys cloudformation__lambda__sqs.sam-event-invoke-config-routes-a-failed-async-event-to-the-queue
Issue-Key: cloudformation__lambda__sqs.cfn-event-source-mapping-getatt-arn-matches-the-mapping|run#16|AssertionError|assert out.get("Arn") == want, f"GetAtt EventSourceMappingArn {out.get('Arn')!r}, expected {want!r}"
Issue-Key: cloudformation__lambda__sqs.sam-dead-letter-queue-grants-the-role-and-deploys|run#7|AssertionError|assert status == "CREATE_COMPLETE", f"create ended {status}: {_failures(cfn, stack_id)!r}"
Issue-Key: cloudformation__lambda__sqs.sam-event-invoke-config-routes-a-failed-async-event-to-the-queue|run#10|AssertionError|assert types.get("FnEventInvokeConfig") == "AWS::Lambda::EventInvokeConfig", (
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 3 causes in CloudFormation's Lambda/SQS surface: EventSourceMappingArn GetAtt, SAM DeadLetterQueue grant, SAM EventInvokeConfig transform

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three causes.

Cause 1 of 3: AWS::Lambda::EventSourceMapping's `EventSourceMappingArn` attribute is synthesized from the logical id instead of being the mapping's own ARN.
AWS: Lambda identifies an event source mapping by its UUID, and the mapping ARN is `arn:aws:lambda:<region>:<account>:event-source-mapping:<uuid>` (Smithy: com.amazonaws.lambda CreateEventSourceMapping output carries UUID and EventSourceMappingArn). `Fn::GetAtt [Mapping, EventSourceMappingArn]` returns exactly the value GetEventSourceMapping reports for that mapping.
Emulator: GetAtt returns `arn:aws:lambda:<region>:<account>:event-source-mapping/<stack>-<logical-id>-<suffix>` — the wrong separator (`/` instead of `:`) and a stack-derived name where the UUID belongs; it does not equal the mapping's own ARN.
Why: a template that passes the attribute into an IAM policy resource, a tag, or another service's configuration writes an ARN that names nothing, so grants and lookups built on it silently fail.
Fix: the AWS::Lambda::EventSourceMapping resource handler's read/attribute projection (it must return the mapping's UUID-based ARN from Lambda state).
Traces: cloudformation__lambda__sqs.cfn-event-source-mapping-getatt-arn-matches-the-mapping.

Cause 2 of 3: a SAM function with a DeadLetterQueue is refused at create because the generated execution-role grant is not seen by Lambda's DLQ permission check.
AWS: `AWS::Serverless::Function` with `DeadLetterQueue: {Type: SQS, TargetArn: <queue arn>}` transforms into an AWS::Lambda::Function with DeadLetterConfig plus an sqs:SendMessage grant on the queue added to the function's role policy, and the stack reaches CREATE_COMPLETE. Lambda's modelled refusal for a role that genuinely cannot send is `InvalidParameterValueException` (Smithy: com.amazonaws.lambda CreateFunction error InvalidParameterValueException, HTTP 400) with the fixed text `The provided execution role does not have permissions to call SendMessage on SQS`.
Emulator: the create ends ROLLBACK_COMPLETE with the function's CREATE_FAILED reason `Resource handler returned message: "The provided execution role does not have permissions to call SendMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (HandlerErrorCode: InvalidRequest)` even though SAM is supposed to have granted it.
Why: no SAM template that uses DeadLetterQueue can be deployed; the whole stack rolls back.
Fix: the SAM transform's DeadLetterQueue policy generation, and Lambda's CreateFunction DLQ permission check (it must evaluate the role policy the transform attached).
Traces: cloudformation__lambda__sqs.sam-dead-letter-queue-grants-the-role-and-deploys.

Cause 3 of 3: the SAM transform emits no AWS::Lambda::EventInvokeConfig for a function's EventInvokeConfig.
AWS: `AWS::Serverless::Function` with an `EventInvokeConfig` (MaximumRetryAttempts / DestinationConfig OnFailure) expands into an `AWS::Lambda::EventInvokeConfig` resource named `<Function>EventInvokeConfig`, so the processed template lists it and a failed asynchronous invocation is routed to the configured destination.
Emulator: the processed template contains only `{'Dlq': 'AWS::SQS::Queue', 'Fn': 'AWS::Lambda::Function', 'FnRole': 'AWS::IAM::Role'}` — no EventInvokeConfig resource at all, so the async failure destination is never configured.
Why: a client's asynchronous failure routing is dropped on deploy; failed events disappear instead of reaching the on-failure queue, with no error at any point.
Fix: the SAM transform's AWS::Serverless::Function EventInvokeConfig expansion.
Traces: cloudformation__lambda__sqs.sam-event-invoke-config-routes-a-failed-async-event-to-the-queue.

## Evidence
Cause 1 — expected `arn:aws:lambda:<region>:<account>:event-source-mapping:<uuid>`; actual `arn:aws:lambda:us-west-2:000000000000:event-source-mapping/sim2real-esmga-7cc1095dc568-Mapping-9C6A571487DB` (`AssertionError: GetAtt EventSourceMappingArn ... expected 'arn:aws:lambda:us-west-2:000000000000:event-source-mapping:15c347f7-90b0-42ed-b16f-6582184be7fa'`). Fixed parts of the ARN: the `event-source-mapping:` segment and separator; variable parts: region, account, UUID.
Cause 2 — expected CREATE_COMPLETE; actual `AssertionError: create ended ROLLBACK_COMPLETE: [('Fn', 'Resource handler returned message: "The provided execution role does not have permissions to call SendMessage on SQS (Service: Lambda, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" (HandlerErrorCode: InvalidRequest)')]`.
Cause 3 — expected an `AWS::Lambda::EventInvokeConfig` in the processed template; actual `AssertionError: no FnEventInvokeConfig in the processed template: {'Dlq': 'AWS::SQS::Queue', 'Fn': 'AWS::Lambda::Function', 'FnRole': 'AWS::IAM::Role'}`.

## Steps to Reproduce
Cause 1: deploy a stack with an AWS::Lambda::EventSourceMapping over an SQS queue and an Output of `Fn::GetAtt [Mapping, EventSourceMappingArn]`; compare with GetEventSourceMapping's EventSourceMappingArn.
Cause 2: deploy a SAM template with `AWS::Serverless::Function` + `DeadLetterQueue: {Type: SQS, TargetArn: <queue>}`.
Cause 3: deploy a SAM template with `AWS::Serverless::Function` + `EventInvokeConfig` naming an OnFailure SQS destination; list the processed template's resources.
Replay: the three check ids in trace_ids.
