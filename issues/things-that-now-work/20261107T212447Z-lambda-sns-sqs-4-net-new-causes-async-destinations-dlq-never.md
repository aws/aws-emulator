Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative lambda__sns__sqs.lambda-sns-sdk-publish lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits
Issue-Key: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure|_poll_until#14|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes|_await_messages#11|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message|_await_messages#10|AssertionError|raise AssertionError(f"{description}: {len(got)} of {count} message(s) arrived within "
Issue-Key: lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs|_poll_for_message#14|AssertionError|raise AssertionError(f"no failure-destination event delivered within {timeout_s}s")
Issue-Key: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative|_invoke_json_until#14|AssertionError|raise AssertionError(f"predicate never satisfied, last result: {last_result}")
Issue-Key: lambda__sns__sqs.lambda-sns-sdk-publish|_wait_for_messages#12|AssertionError|raise AssertionError(
Issue-Key: lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits|run#20|AssertionError|assert observed_max >= 2, (
Issue-Commit: a5c62b28646ac18350a618623d28fc3117c37127

# 4 net-new causes: async destinations/DLQ never deliver, in-function SQS authorization not enforced, no 'sns' client, SQS ESM never scales

> _Note: this issue was identified against the code base at commit `a5c62b28646ac18350a618623d28fc3117c37127`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Four net-new root causes. Open issues 20261107T014900Z-lambda-sns-sqs-async-destinations-and-dead-letter.md and 20261106T133436Z-lambda-sns-sqs-in-function-aws-sdk-calls.md cover other checks in this service; none of the checks below carry their replay keys.

Cause 1 of 4 — asynchronous invocation destinations and the dead-letter topic never deliver a record.
AWS: recorded behavior only. For an async (Event) invoke, Lambda writes exactly one invocation record to the EventInvokeConfig's OnSuccess destination when the handler succeeds and to the OnFailure destination (and/or DeadLetterConfig target) when it exhausts its retries; when the destination is an SNS topic the record is published as the message and relayed to the topic's subscribers, with the Lambda error attributes attached for a dead-lettered event.
Emulator: nothing is ever delivered on any of these paths. Replay details: `AssertionError: failure-destination invocation record on SNS-fed queue not met within 90s. Last result: None`; `AssertionError: the dead-lettered event relayed by SNS: 0 of 1 message(s) arrived within 150s`; `AssertionError: the SNS notification of the success record: 0 of 1 message(s) arrived within 150s`; `AssertionError: no failure-destination event delivered within 90.0s`.
Why: every failed async event is lost with no record anywhere, and a success record a client uses to chain work never arrives — so a client cannot build reliable fan-out or failure handling, and cannot even detect that events are being dropped.
Fix: the Lambda asynchronous invocation pipeline's destination dispatcher — OnSuccess/OnFailure delivery to SNS and SQS targets and DeadLetterConfig delivery after retry exhaustion, including the error message attributes on the dead-lettered event.
Traces: lambda__sns__sqs.cfn-deploy-lambda-destination-sns-on-failure, lambda__sns__sqs.dead-letter-topic-relays-the-original-event-with-lambda-error-attributes, lambda__sns__sqs.on-success-destination-topic-publishes-the-invocation-record-as-the-message, lambda__sns__sqs.sns-lambda-destination-onfailure-to-sqs.

Cause 2 of 4 — sqs:ReceiveMessage from inside a Lambda is not authorized.
AWS: recorded behavior only. ReceiveMessage from a function is evaluated against the execution role's identity policy; a role without sqs:ReceiveMessage is refused with AccessDeniedException and receives no messages.
Emulator: the call succeeds and returns the queue's messages. Replay detail: `AssertionError: predicate never satisfied, last result: {'ok': True, 'count': 1, 'bodies': ['payload-a9c5a54a1d19']}`.
Why: any function can drain any queue in the account regardless of its role, so queue isolation between components is not enforced and the client's negative test passes vacuously.
Fix: authorization on the SQS ReceiveMessage handler — evaluate the caller's (function execution role's) identity policy and queue policy and refuse with AccessDeniedException.
Traces: lambda__sns__sqs.lambda-sdk-receive-message-missing-permission-negative.

Cause 3 of 4 — the Lambda Python runtime's bundled botocore knows no 'sns' client, so a handler's publish produces nothing.
AWS: recorded behavior only. The managed python3.x runtimes ship a complete boto3/botocore, so `boto3.client("sns").publish(...)` resolves and the message reaches the topic's subscribers.
Emulator: no message is produced. Replay detail: `AssertionError: Expected 1 messages, got 0 within 60s` — and the sibling checks in this service show the underlying refusal, `errorType: 'UnknownServiceError'`, `errorMessage: "Unknown service: 'sns'. Valid service names are: sqs, events, eventbridge, dynamodb, logs, cloudwatchlogs, apigateway, secretsmanager"`.
Why: no handler can publish to SNS; the failure is an SDK-construction error rather than a service response, so it is indistinguishable from a missing sns:Publish grant.
Fix: the in-process Lambda Python runtime's bundled botocore service catalogue — the `sns` service model must be resolvable so publishes reach the emulator's SNS handlers.
Traces: lambda__sns__sqs.lambda-sns-sdk-publish.

Cause 4 of 4 — an SQS event source mapping never scales beyond one concurrent batch.
AWS: recorded behavior only. An SQS event source mapping with a configured ScalingConfig.MaximumConcurrency (>= 2) draining a backlog runs batches concurrently up to that limit, so overlapping invocations are observable.
Emulator: batches are processed strictly serially. Replay detail: `AssertionError: expected the ESM to actually scale up to the configured concurrency (>=2) while draining a backlog of 16 messages, but max observed overlap was 1`.
Why: a client sizing MaximumConcurrency to meet a drain-rate target gets none of it — throughput is capped at one batch at a time, queues back up, and the configuration setting has no observable effect.
Fix: the SQS event-source-mapping poller — concurrent batch dispatch bounded by ScalingConfig.MaximumConcurrency and the function's reserved concurrency.
Traces: lambda__sns__sqs.sqs-lambda-esm-maximum-concurrency-limits.

## Evidence
Cause 1: expected one invocation record on the OnSuccess/OnFailure destination and the DLQ target per async invoke; actual `Last result: None` / `0 of 1 message(s) arrived within 150s` / `no failure-destination event delivered within 90.0s`.
Cause 2: expected AccessDeniedException and no messages for a role without sqs:ReceiveMessage; actual {'ok': True, 'count': 1, 'bodies': ['payload-...']}.
Cause 3: expected 1 message at the topic's subscriber after a handler publish; actual 0 within 60 s (underlying errorType UnknownServiceError for 'sns').
Cause 4: expected concurrent batches up to the configured MaximumConcurrency (>=2); actual max observed overlap 1.

## Steps to Reproduce
Cause 1: PutFunctionEventInvokeConfig with OnSuccess=SNS topic and OnFailure=SQS queue (and DeadLetterConfig=SNS topic); async-invoke with a succeeding and then a failing handler; poll the topic's subscriber queue and the OnFailure queue — nothing arrives.
Cause 2: CreateFunction whose handler calls ReceiveMessage on a seeded queue with no sqs:ReceiveMessage in its role; Invoke — the messages are returned.
Cause 3: CreateFunction whose handler calls boto3.client("sns").publish and Invoke; poll a queue subscribed to the topic — no message.
Cause 4: CreateEventSourceMapping on an SQS queue with ScalingConfig.MaximumConcurrency=4, send 16 messages, and record overlapping invocations — never more than 1.
