Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs
Issue-Key: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs|_collect#9|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Commit: 06adb3e2aae322591dd1176b14ac77cd8d29e903

# Lambda durable-execution status-change events are never emitted, so an EventBridge default-bus rule delivers nothing and StopDurableExecution's STOPPED transition is unobservable

> _Note: this issue was identified against the code base at commit `06adb3e2aae322591dd1176b14ac77cd8d29e903`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDurableExecution

## Description
One root cause.

Cause 1 of 1: a durable execution started on a Lambda function that carries a DurableConfig never produces any durable-execution status-change event, so an EventBridge rule on the default bus that targets SQS receives nothing for RUNNING, SUCCEEDED or STOPPED, and the check cannot get past its first drain.

AWS: Lambda's durable-execution APIs are modelled and addressable per execution — Smithy (lambda): StopDurableExecution, POST /2025-12-01/durable-executions/{DurableExecutionArn}/stop, input DurableExecutionArn @httpLabel @required @length(min 1, max 1024) @pattern `^arn:([a-zA-Z0-9-]+):lambda:([a-zA-Z0-9-]+):(\d{12}):function:([a-zA-Z0-9_-]+):(\$LATEST(?:\.PUBLISHED)?|[0-9]+)/durable-execution/([a-zA-Z0-9_-]+)/([a-zA-Z0-9_-]+)$` and optional Error (ErrorObject, @httpPayload); output StopTimestamp (timestamp, @required); modelled errors InvalidParameterValueException (400), ResourceNotFoundException (404), TooManyRequestsException (429), ServiceException (500), the KMS* family (502). The event emission itself is not in any Smithy model — recorded behavior only: on AWS each durable execution publishes a status-change event onto the default EventBridge bus whose `detail` carries at least `durableExecutionArn` and `status` (RUNNING, then SUCCEEDED for a completing execution, or STOPPED after StopDurableExecution), and the STOPPED event's `endTimestamp` equals the StopTimestamp the StopDurableExecution call returned. A rule on the default bus matching that event delivers the event body to its SQS target.

Emulator: after the role, the function and the default-bus rule with the SQS target are all created and accepted, nothing is ever delivered to the target queue. The check's warm-up (`_await_rule_live`, waiting for a SUCCEEDED status change for a one-second execution) and its first drain (waiting for a RUNNING event for the started execution, with its own 120 s bound) both find an empty queue, and the check is cut off: `Check timed out after 60.0s`. No durable-execution event of any status is observable, so StopDurableExecution's STOPPED transition and its endTimestamp/StopTimestamp agreement cannot be reached at all.

Why: a client that drives durable executions by reacting to their status-change events — an operator-initiated stop that must be confirmed, a completion handler, an alarm on failed executions — sees an execution that starts and then never reports anything. It cannot tell a running execution from a stopped or dropped one, and every EventBridge rule or pipe keyed on `aws.lambda` durable-execution status changes is silently dead while still reporting a healthy rule and target.

Fix: the durable-execution runtime in the lambda service and its event emission into the default EventBridge bus — the execution lifecycle (start -> RUNNING, completion -> SUCCEEDED/FAILED, StopDurableExecution -> STOPPED) must publish a status-change event naming the execution ARN and status, and the STOPPED event must carry the same timestamp StopDurableExecution returned as StopTimestamp. Related open issue 20261026T070343Z-events-lambda-sqs-lambda-invoke-on-a-durable-execution-function-returns.md covers Invoke not returning the DurableExecutionArn handle; it does not carry this replay key, and the failure now occurs further along — the execution is addressable but emits no events.

Traces: events__lambda__sqs.xsvc-lambda-1-durable-stop-emits-stopped-status-change-to-sqs.

## Evidence
Expected (recorded AWS behavior): with a default-bus EventBridge rule targeting an SQS queue, starting a durable execution on a DurableConfig function delivers a status-change event with detail.status='RUNNING' for that execution ARN; calling StopDurableExecution(DurableExecutionArn=<arn>, Error={ErrorType, ErrorMessage}) returns 200 with StopTimestamp, and a following event carries detail.status='STOPPED' with detail.endTimestamp equal to that StopTimestamp. The status strings, the detail member names and the StopTimestamp/endTimestamp agreement are the requirement; the account id, region, function name, execution id and the timestamp value in any one run are filled in from the request and the emulator's state.

Actual (emulator replay): the target queue stays empty throughout — the warm-up drain for a SUCCEEDED event and the drain for the RUNNING event of the started execution both return no messages, and the check is cut off with 'Check timed out after 60.0s'. No durable-execution status-change event of any status reaches the rule's SQS target, so the STOPPED assertion is never reached.

## Steps to Reproduce
1. Create an IAM role Lambda can assume, and a Python function whose handler is decorated with @durable_execution and calls context.wait(Duration.from_seconds(...)), configured with a DurableConfig and an execution timeout.
2. Create an SQS queue and an EventBridge rule on the DEFAULT bus matching Lambda durable-execution status-change events for that function ARN, with the queue as its target.
3. Start a durable execution with a short wait and poll the queue: AWS delivers RUNNING and then SUCCEEDED events for it; the emulator delivers nothing.
4. Start a second durable execution with a long wait (e.g. 300 s) and poll for its RUNNING event: the queue stays empty.
5. Call StopDurableExecution(DurableExecutionArn=<arn>, Error={'ErrorType': ..., 'ErrorMessage': ...}) and poll for a STOPPED event whose detail.endTimestamp equals the returned StopTimestamp: no event ever arrives.
6. Re-replay the check id listed in trace_ids.
