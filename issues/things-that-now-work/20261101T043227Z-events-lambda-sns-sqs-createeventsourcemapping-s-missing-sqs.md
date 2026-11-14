Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns__sqs.sqs-lambda-esm-missing-permission
Issue-Key: lambda|CreateEventSourceMapping|InvalidParameterValueException
Issue-Commit: 7dbe25757f78c9632b5e7ee1be76e39ac39f57ee

# events__lambda__sns__sqs: CreateEventSourceMapping's missing-sqs:ReceiveMessage refusal message says "The provided execution role" instead of AWS's "The function execution role" (1 cause)

> _Note: this issue was identified against the code base at commit `7dbe25757f78c9632b5e7ee1be76e39ac39f57ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
This issue carries ONE net-new root cause. It must be closed before submitting; a fixer should re-replay the single check id listed in trace_ids (events__lambda__sns__sqs.sqs-lambda-esm-missing-permission). The replay report left this check UNMARKED (no ALREADY-FILED marker and no OWNED marker), so no open issue carries its replay key.

=== Cause 1 of 1: the InvalidParameterValueException message emitted when an SQS event source mapping is created for a function whose execution role lacks sqs:ReceiveMessage is emulator-authored wording ===
Failing check id: events__lambda__sns__sqs.sqs-lambda-esm-missing-permission.

AWS: Smithy (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) models CreateEventSourceMapping as POST /2015-03-31/event-source-mappings with success code 202 and errors InvalidParameterValueException (400), ResourceConflictException (409), ResourceNotFoundException (404), ServiceException (500), TooManyRequestsException (429). The model carries no message text, so the wording is recorded behavior only: AWS pre-flight-validates the function's execution role against the SQS source at mapping-creation time and refuses with HTTP 400, error code `InvalidParameterValueException` and the message `The function execution role does not have permissions to call ReceiveMessage on SQS`. For this refusal class the sentence is WHOLLY FIXED TEXT — it names neither the role, the function, the queue nor the queue ARN; the only parts that vary across the family are the API name of the missing permission and the source service (here `ReceiveMessage` / `SQS`, derived from the source type being wired, not from any request value).

Emulator: the HTTP status (400), the error code (InvalidParameterValueException) and the discrimination on permissions are all correct — the emulator does refuse the under-privileged mapping — but the message is rewritten as `The provided execution role does not have permissions to call ReceiveMessage on SQS`. The divergence is confined to the opening clause of the fixed text: AWS says `The function execution role`, the emulator says `The provided execution role`; the remainder (`does not have permissions to call ReceiveMessage on SQS`) matches. Replay: `AssertionError: expected a permission-related rejection creating the ESM, got ('InvalidParameterValueException', 'The provided execution role does not have permissions to call ReceiveMessage on SQS')` at check_events__lambda__sns__sqs_sqs_lambda_esm_missing_permission.py line 96.

Why: the error message is part of the contract for this refusal. Because the error CODE (InvalidParameterValueException) is shared by every other creation-time rejection of this operation (bad BatchSize, unsupported source ARN, nonexistent queue, invalid FilterCriteria), the message is the only discriminator a client has. IaC tooling surfacing why an SQS trigger could not be wired, CI parity harnesses, and support automation that pattern-matches AWS's sentence to say "grant the role sqs:ReceiveMessage" never match, and so cannot tell a permissions failure apart from an unrelated parameter failure.

Fix: the message template in CreateEventSourceMapping's execution-role/source validation step (the handler validation that probes the function execution role's permissions on the SQS source before the mapping is persisted). Emit AWS's fixed sentence verbatim, with the permission name and source service as the only varying parts of the family.

Related but NOT covering this check: open issues 20261030T110426Z-events-lambda-sfn-sqs-createeventsourcemapping-s-under-privileged-role-refusal-says-the.md, 20261030T121631Z-events-lambda-sqs-createeventsourcemapping-s-under-privileged-execution.md, 20261030T105143Z-lambda-secretsmanager-sqs-createeventsourcemapping-s-permission-refusal-uses.md and 20261030T110325Z-lambda-s3-sqs-createeventsourcemapping-s-unauthorized-execution-role.md report the same wording divergence but each carries a DIFFERENT check's replay key; none carries events__lambda__sns__sqs.sqs-lambda-esm-missing-permission, which the report left unmarked. Open issue 20261029T182228Z-events-lambda-sns-sqs-eventbridge-and-sns-never.md concerns an ESM that CONSUMES despite a role without receive permission — the opposite behavior from the one graded here, since this refusal does happen and only its wording diverges.

Traces: events__lambda__sns__sqs.sqs-lambda-esm-missing-permission (assertion at check_events__lambda__sns__sqs_sqs_lambda_esm_missing_permission.py line 96).

## Evidence
Cause 1 — events__lambda__sns__sqs.sqs-lambda-esm-missing-permission, verdict mismatch.

Expected: CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<queue arn>) where the function's execution role holds NO sqs:ReceiveMessage -> HTTP 400, error code `InvalidParameterValueException`, message exactly `The function execution role does not have permissions to call ReceiveMessage on SQS` (wholly fixed text for this refusal class; only the permission name and source service vary across the family), and no mapping persisted.

Actual (replay traceback):
  File "check_events__lambda__sns__sqs_sqs_lambda_esm_missing_permission.py", line 96, in run
    assert refusal == (
  AssertionError: expected a permission-related rejection creating the ESM, got ('InvalidParameterValueException', 'The provided execution role does not have permissions to call ReceiveMessage on SQS')

The tuple compared in the check is (error code, error message): the code matches AWS, so status and classification agree; the mismatch is the fixed opening clause of the message — AWS `The function execution role`, emulator `The provided execution role`. The check's positive control (a twin ESM whose role DOES hold sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes draining its queue) passed before this assertion, so the ESM delivery path works on this target and the refusal is genuinely attributable to the missing permission.

Model: CreateEventSourceMapping POST /2015-03-31/event-source-mappings, success 202; errors InvalidParameterValueException(400), ResourceConflictException(409), ResourceNotFoundException(404), ServiceException(500), TooManyRequestsException(429). No message text is modeled, so the wording requirement is recorded behavior.

## Steps to Reproduce
1. sqs:CreateQueue -> queue URL; sqs:GetQueueAttributes(AttributeNames=[QueueArn]) -> source queue ARN.
2. Positive control: iam:CreateRole with the lambda.amazonaws.com trust policy AND an inline policy granting sqs:ReceiveMessage, sqs:DeleteMessage, sqs:GetQueueAttributes on a second queue's ARN; lambda:CreateFunction with that role; lambda:CreateEventSourceMapping against the second queue; sqs:SendMessage and poll ApproximateNumberOfMessages until 0 — confirms the ESM delivery path works on this target and measures its latency.
3. iam:CreateRole with the standard lambda.amazonaws.com trust policy and NO SQS permissions attached.
4. lambda:CreateFunction(FunctionName=<fn>, Role=<that role>, Handler=lambda_function.lambda_handler, Runtime=<python>, Code=<zip returning {"ok": true}>); wait until Active.
5. lambda:CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<queue arn from step 1>).
   Expected: HTTP 400, code InvalidParameterValueException, message exactly `The function execution role does not have permissions to call ReceiveMessage on SQS`.
   Observed: HTTP 400, code InvalidParameterValueException, message `The provided execution role does not have permissions to call ReceiveMessage on SQS`.
6. lambda:ListEventSourceMappings(FunctionName=<fn>, EventSourceArn=<queue arn>) -> must be empty (the rejected call persists nothing).
7. sqs:SendMessage to the queue from step 1, then sqs:ReceiveMessage -> the message must still be there, since no mapping exists to consume it.
Replay: events__lambda__sns__sqs.sqs-lambda-esm-missing-permission.
