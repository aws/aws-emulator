Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: pipes__sns__sqs.pipe-sqs-source-publishes-the-rendered-template-to-an-sns-topic-subscriber pipes__sns__sqs.pipe-published-message-body-filter-policy-withholds-non-matching-records pipes__sns__sqs.pipes-sns-fifo-topic-target-rejected
Issue-Key: pipes__sns__sqs.pipe-sqs-source-publishes-the-rendered-template-to-an-sns-topic-subscriber|_drain#14|Timeout|msgs = sqs.receive_message(
Issue-Key: pipes__sns__sqs.pipe-published-message-body-filter-policy-withholds-non-matching-records|_drain#20|Timeout|msgs = sqs.receive_message(
Issue-Key: pipes__sns__sqs.pipes-sns-fifo-topic-target-rejected|_expect_refusal#21|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded with {answer!r}")
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# pipes__sns__sqs: 2 net-new causes — a RUNNING pipe with an SNS-topic target publishes nothing, and CreatePipe accepts an SNS FIFO topic as the target instead of refusing it

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Two distinct root causes in the EventBridge Pipes SNS-target path.

Cause 1 of 2: the pipes data plane never publishes to an SNS-topic target.
AWS: recorded behavior only (the Pipes data plane is not modelled; https://docs.aws.amazon.com/eventbridge/latest/userguide/eb-pipes-input-transformation.html, .../eb-pipes-event-filtering.html, https://docs.aws.amazon.com/sns/latest/dg/sns-message-filtering-scope.html). Once a pipe whose Source is an SQS queue and whose Target is a standard SNS topic reports CurrentState RUNNING, every source record publishes to the topic under the pipe's RoleArn — the rendering of TargetParameters.InputTemplate when one is configured, otherwise the pipe's SQS record JSON (messageId, body, eventSourceARN...) — the topic fans it to each subscription, a raw-message-delivery SQS subscriber receives exactly that payload, a subscription with FilterPolicyScope=MessageBody matches against the published JSON and withholds the non-matching record, and the delivered source message leaves the source queue (ApproximateNumberOfMessages and ApproximateNumberOfMessagesNotVisible both 0).
Emulator: nothing is ever published. Both delivery checks stand up role, queues, topic, subscription(s) and pipe and then poll the subscribed queue(s); no message ever arrives, and the checks consume the whole 60 s harness budget with no verdict ('Check timed out after 60.0s'). Sibling pipes checks with an SQS-queue target (pipes__sqs, iam__pipes__sqs, dynamodb__pipes__sqs) finish inside that same budget, so the stall belongs to the SNS-topic target and not to pipe setup or the budget.
Why: every Pipes→SNS fan-out silently delivers nothing while the pipe reports RUNNING; neither the InputTemplate rendering, nor the source-queue drain, nor any SNS subscription filter policy evaluated on the published body can be observed.
Fix: the pipes data-plane delivery path for the SNS-topic target type — for each matched source record, render the InputTemplate and issue sns:Publish to the target topic ARN under the pipe's RoleArn, then delete the consumed source message.
Traces: pipes__sns__sqs.pipe-sqs-source-publishes-the-rendered-template-to-an-sns-topic-subscriber, pipes__sns__sqs.pipe-published-message-body-filter-policy-withholds-non-matching-records

Cause 2 of 2: CreatePipe accepts an SNS FIFO topic as the Target.
AWS: Smithy (services/service-com-amazonaws-pipes-pipes) declares CreatePipe's ValidationException as a client error with httpError 400 and members `message` + `fieldList`. An SNS FIFO topic is not a supported pipe target: the call is refused 400 ValidationException whose `Message` is a compact JSON document of the fixed shape {"ValidationErrorFields":[{"Name":...,"Message":...}]} carrying two entries, both naming the member `Target` — the fixed reasons `Invalid pipe target ARN` and `SNS FIFO is not a supported service for a target.` No pipe is created, so DescribePipe on that name is refused NotFoundException with the message `Pipe <name> does not exist.` (fixed text `Pipe ` / ` does not exist.`, variable part the requested pipe name).
Emulator: the FIFO-topic target is accepted — HTTP 200 with a full CreatePipeResponse (Arn/Name/DesiredState=STOPPED/CurrentState=CREATING) — and the pipe is left in state.
Why: a client gets a pipe that reports as created but can never deliver to a FIFO topic; the unsupported-target condition surfaces (if at all) as silent non-delivery instead of a create-time ValidationException naming Target, and its negative-path handling never runs.
Fix: the CreatePipe handler's target-ARN/target-type validation in the pipes service — recognise SNS FIFO topic ARNs (the `.fifo` topic name) as an unsupported target and refuse before any state is written, emitting ValidationException with both the JSON `message` document and the modelled `fieldList`.
Traces: pipes__sns__sqs.pipes-sns-fifo-topic-target-rejected

## Evidence
Cause 1 — expected: the raw-message-delivery SQS subscriber of the pipe's target topic receives, for the sent source message, the published record (for the plain-template check, a record whose messageId/body/eventSourceARN equal the sent message and the source queue ARN; for the filter check, records ('s1','silver') and ('g1','gold') on the unfiltered subscriber and only ('g1','gold') on the FilterPolicyScope=MessageBody tier=gold subscriber), and the source queue drains to {'ApproximateNumberOfMessages': '0', 'ApproximateNumberOfMessagesNotVisible': '0'}. Actual: no message ever arrives on any subscribed queue; suite verdict [mismatch] 'Error: Check timed out after 60.0s' for both checks.

Cause 2 — CreatePipe(Target=<arn:aws:sns:...:<name>.fifo>) expected: HTTP 400 ValidationException with Message = {"ValidationErrorFields":[{"Name":"Target","Message":"Invalid pipe target ARN"},{"Name":"Target","Message":"SNS FIFO is not a supported service for a target."}]} and then DescribePipe → NotFoundException 'Pipe <name> does not exist.' Actual: HTTPStatusCode 200 with {'Arn': 'arn:aws:pipes:<region>:<account>:pipe/<name>', 'Name': '<name>', 'DesiredState': 'STOPPED', 'CurrentState': 'CREATING', ...} — AssertionError: expected a refusal, the call succeeded.

## Steps to Reproduce
1. Create three SQS queues (feed, idle, sink), a standard SNS topic and a FIFO SNS topic (CreateTopic with Name ending '.fifo' and Attributes={'FifoTopic':'true'}).
2. Give the sink queue a policy allowing sns.amazonaws.com sqs:SendMessage from the standard topic, then Subscribe(TopicArn=<standard topic>, Protocol='sqs', Endpoint=<sink ARN>, Attributes={'RawMessageDelivery':'true'}).
3. CreateRole trusting pipes.amazonaws.com with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source queues and sns:Publish on both topics.
4. Cause 2: CreatePipe(Name=..., RoleArn=..., Source=<idle queue ARN>, Target=<FIFO topic ARN>, DesiredState='STOPPED'). Observe HTTP 200 instead of 400 ValidationException, and that DescribePipe finds the pipe.
5. Cause 1: CreatePipe(Name=..., RoleArn=..., Source=<feed queue ARN>, SourceParameters={'SqsQueueParameters':{'BatchSize':1}}, Target=<standard topic ARN>) (and, for the filter variant, with TargetParameters={'InputTemplate': '{"tier": <$.body.level>, "id": <$.body.id>}'} plus a second subscription carrying FilterPolicyScope='MessageBody' and FilterPolicy {"tier":["gold"]}); wait for CurrentState RUNNING; SendMessage to the feed queue; ReceiveMessage from the sink queue(s). Observe the sink queues stay empty and the feed queue never drains.
