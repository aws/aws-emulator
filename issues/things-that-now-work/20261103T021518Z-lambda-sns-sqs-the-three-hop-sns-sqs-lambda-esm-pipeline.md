Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.pipeline-sns-sqs-lambda
Issue-Key: lambda__sns__sqs.pipeline-sns-sqs-lambda|probe#18|Timeout|return ddb.get_item(
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# The three-hop SNS -> SQS -> Lambda ESM pipeline never delivers: the SQS event source mapping does not invoke the function for messages a topic subscription enqueued

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Cause 1 of 1: a message published to an SNS topic, fanned out to an SQS queue by an `sqs`-protocol subscription, is never handed to the Lambda function bound to that queue by an event source mapping. The end-to-end effect the function would produce never appears and the check is killed at the harness cap.

AWS: with (a) a queue policy allowing `sqs:SendMessage` to principal `sns.amazonaws.com` conditioned on `aws:SourceArn` = the topic ARN, (b) an `sqs`-protocol SNS subscription on that queue, and (c) an enabled Lambda event source mapping whose EventSourceArn is the queue ARN and whose execution role allows `sqs:ReceiveMessage`/`sqs:DeleteMessage`/`sqs:GetQueueAttributes`, a single Publish propagates all three hops: SNS enqueues the notification envelope, the ESM poller receives it and invokes the function with an `event["Records"]` list whose `body` is the SNS envelope, and on a successful invocation the ESM deletes the message so the queue drains. The caller observes the function's side effect (here a DynamoDB PutItem) within seconds. Recorded behavior only for the propagation latency; the model states only that CreateEventSourceMapping returns the mapping's UUID and State.

Emulator: the side effect never materialises. After Publish, the DynamoDB item keyed on the published `order_id` is polled with ConsistentRead for up to 90s and `get_item` returns no `Item`, so `_wait_for_effect` never satisfies its predicate and the check exceeds the 60s harness cap with `Check timed out after 60.0s` — it never even reaches its follow-on assertion that the queue has drained. The ESM is created and returns a UUID, so the mapping appears healthy while polling nothing.

Why: the single most common serverless fan-out topology (topic -> queue -> function) is inert. A client sees Publish succeed, Subscribe succeed and CreateEventSourceMapping return an enabled mapping, then waits forever for work that is never performed, with no error anywhere to attribute the loss to. Messages also accumulate undeleted in the queue because nothing consumes them, so a caller asserting on queue depth draws the opposite conclusion from the truth.

Fix: the Lambda event source mapping poller — an enabled mapping on an SQS queue must receive from the queue, invoke the target function with the `Records` envelope, and delete the batch on success. Whether the SNS `sqs`-protocol subscription enqueued the message at all should be confirmed first (that hop is a prerequisite); the mapping's own poll/invoke/delete loop is the component that owes the invocation.

Related (does NOT cover this check): 20261029T183635Z-lambda-sns-sqs-sqs-event-source-mappings-never.md and 20261025T022656Z-lambda-sns-sqs-sns-sqs-to-lambda-delivery.md own many sibling checks in this service, but this check was not marked as owned by either, so its replay key is unclaimed and it is filed here.

Traces: lambda__sns__sqs.pipeline-sns-sqs-lambda.

## Evidence
Replay of simparity-behavioral-aws-v1, check `pipeline-sns-sqs-lambda` (GROUP `event-propagation`, verdict: mismatch).

Error: `Check timed out after 60.0s`.

The check, per its replayed source, wires the full topology and then waits on the effect:
  - creates a topic, a queue and a DynamoDB table;
  - SetQueueAttributes with a Policy allowing `sqs:SendMessage` to `{"Service": "sns.amazonaws.com"}` on the queue ARN, conditioned `ArnEquals aws:SourceArn` = the topic ARN;
  - Subscribe(TopicArn=<topic>, Protocol="sqs", Endpoint=<queue arn>);
  - creates a function whose handler parses `record["body"]`, then `body["Message"]`, and calls `ddb.put_item(Item={"pk": {"S": msg["order_id"]}})`, with role permissions for `sqs:ReceiveMessage`/`sqs:DeleteMessage`/`sqs:GetQueueAttributes` on the queue and `dynamodb:PutItem` on the table;
  - `_wire_event_source(fn, queue_arn)` -> a mapping UUID is returned;
  - `sns.publish(TopicArn=<topic>, Message=json.dumps({"order_id": <order_id>}))`;
  - polls `ddb.get_item(Key={"pk": {"S": <order_id>}}, ConsistentRead=True)` for up to 90s, guarding `assert item is not None, "End-to-end SNS -> SQS -> Lambda pipeline did not deliver"`.

Expected: the item appears, then the queue drains. Actual: `get_item` returns no `Item` for the whole window, the check overruns the 60s cap and is terminated before the assertion can be reported, and the subsequent queue-drained probe is never reached.

The topic/queue/table names and the generated `order_id` are per-run values and not part of the requirement; the requirement is that one Publish traverses subscription and event source mapping to produce exactly one invocation and the resulting side effect.

## Steps to Reproduce
1. Create an SNS topic, an SQS queue and a DynamoDB table.
2. SetQueueAttributes on the queue with a Policy allowing `sqs:SendMessage` to principal `{"Service":"sns.amazonaws.com"}` on the queue ARN, conditioned on `ArnEquals aws:SourceArn` = the topic ARN.
3. Subscribe the queue ARN to the topic with Protocol `sqs`.
4. Create a Lambda function that, for each `event["Records"]` entry, parses `record["body"]` then its `Message` field and PutItems `{"pk": {"S": msg["order_id"]}}` into the table; grant its role `sqs:ReceiveMessage`, `sqs:DeleteMessage`, `sqs:GetQueueAttributes` on the queue and `dynamodb:PutItem` on the table.
5. CreateEventSourceMapping with EventSourceArn = the queue ARN, FunctionName = the function, Enabled = true. Note the returned UUID.
6. Publish `{"order_id": "<unique>"}` to the topic.
7. Poll GetItem on the table for `pk = <unique>` with ConsistentRead for 90s. Observe no item ever appears, and that messages remain in the queue (ApproximateNumberOfMessages does not fall to 0). On AWS the item appears within seconds and the queue drains.
8. Or replay the behavioral check `pipeline-sns-sqs-lambda`, which times out at the 60s cap.
