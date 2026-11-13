Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__s3__sqs.sqs-esm-concurrent-batch-window-positive
Issue-Key: lambda__s3__sqs.sqs-esm-concurrent-batch-window-positive|run#22|AssertionError|assert control_max == 1, (
Issue-Commit: b3586cbe985cc2412e94e4f6c5b37df3b09bc671

# lambda__s3__sqs: an SQS event source mapping with MaximumBatchingWindowInSeconds=0 still merges arrivals seconds apart into one invocation

> _Note: this issue was identified against the code base at commit `b3586cbe985cc2412e94e4f6c5b37df3b09bc671`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: with no batching window, the SQS event-source poller still accumulates messages that arrived seconds apart and delivers them in one multi-record invocation. Failing check: lambda__s3__sqs.sqs-esm-concurrent-batch-window-positive.
AWS: Smithy (com.amazonaws.lambda#CreateEventSourceMapping) models MaximumBatchingWindowInSeconds and documents it as "The maximum amount of time, in seconds, that Lambda spends gathering records before invoking the function... For Kinesis, DynamoDB, and Amazon SQS event sources, the default batching window is 0 seconds." So with MaximumBatchingWindowInSeconds=0 (and BatchSize=10) the poller spends no time gathering: each message that is alone on the queue when the poller reads it becomes its own invocation of ONE record. Messages produced one every 2 s therefore yield one record per invocation (max batch size 1) — the record count per invocation is bounded by the arrival spacing, not by BatchSize. A twin mapping on another queue with MaximumBatchingWindowInSeconds=10 fed the same spacing accumulates and invokes with more than one record; the contrast between the two mappings is the whole point of the window.
Emulator: the zero-window (control) mapping merges records anyway. Fed six messages 2 s apart it produced 5 invocations with a maximum of 2 records in one of them, so the emulator's poller gathered across a ≥2 s interval although no batching window was configured.
Why: batch composition is observable application behavior — a handler's per-record work, partial-batch-failure reporting (ReportBatchItemFailures), per-invocation idempotency keys and concurrency accounting all depend on how many records an invocation carries. A client that deliberately leaves the window at 0 to get one-record invocations gets multi-record ones instead, and a client that sets a window can no longer attribute any batching to it, because the no-window mapping behaves the same way.
Fix: the SQS event-source poller's gather loop in the lambda service crate — the dispatch decision must respect MaximumBatchingWindowInSeconds (0 = invoke with whatever the current receive returned, do not hold records for the next poll tick), rather than batching everything accumulated between fixed poll ticks.
Traces: the check above.

## Evidence
Expected (AWS): on a mapping created with BatchSize=10 and MaximumBatchingWindowInSeconds=0, six messages sent 2 s apart are delivered as six invocations of one record each (max records per invocation == 1); the twin mapping with MaximumBatchingWindowInSeconds=10 shows max records per invocation > 1.
Actual (emulator):
  [mismatch] lambda__s3__sqs.sqs-esm-concurrent-batch-window-positive
  AssertionError: control ESM (no batching window) fed the same spaced-out arrivals still merged records into one invocation (saw 5 invocations for 6 messages, max batch=2) — spacing alone should force one record per invocation, so this run cannot attribute batching to the window
The counts quoted (5 invocations, max batch 2) are one run's numbers; the requirement is that a zero-window mapping never merges arrivals separated by more than the window into one invocation. Generated function/queue/bucket names and marker strings are per-run values.

## Steps to Reproduce
1. Create a function that writes one report object per invocation recording its record count and each record body.
2. Create two SQS queues; lambda.create_event_source_mapping(FunctionName=F, EventSourceArn=<control queue arn>, BatchSize=10, MaximumBatchingWindowInSeconds=0) and a twin with MaximumBatchingWindowInSeconds=10 on the other queue.
3. Send six messages to each queue, alternating and sleeping 2 s between sends.
4. Read the reports: the control queue's invocations must each carry exactly one record (max batch == 1); the windowed queue's must show at least one invocation with more than one record. Observe the control mapping reporting an invocation with 2 records.
Replay the check id in trace_ids to confirm a fix.
