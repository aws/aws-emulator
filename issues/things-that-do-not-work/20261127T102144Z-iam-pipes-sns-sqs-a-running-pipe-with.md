Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__pipes__sns__sqs.pipe-from-a-queue-publishes-only-filtered-transformed-records-to-an-sns-topic iam__pipes__sns__sqs.pipe-rendered-body-not-the-source-record-is-what-a-body-filter-policy-matches
Issue-Key: iam__pipes__sns__sqs.pipe-from-a-queue-publishes-only-filtered-transformed-records-to-an-sns-topic|run#23|Timeout|resp = sqs.receive_message(QueueUrl=urls["out"], MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Key: iam__pipes__sns__sqs.pipe-rendered-body-not-the-source-record-is-what-a-body-filter-policy-matches|_bodies#26|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=1)
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# iam__pipes__sns__sqs: a RUNNING pipe with an SNS-topic target publishes nothing, so neither InputTemplate rendering nor filter-criteria/filter-policy behaviour on the published payload ever happens (1 cause)

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: the pipes data plane never publishes to an SNS-topic target.
AWS: recorded behavior only (the Pipes data plane is not modelled; see https://docs.aws.amazon.com/eventbridge/latest/userguide/eb-pipes-input-transformation.html and .../eb-pipes-event-filtering.html). The recording shows that a pipe whose Source is an SQS queue and whose Target is a standard SNS topic ARN, once CurrentState is RUNNING, calls sns:Publish under its RoleArn for every source record its FilterCriteria matches, publishing the rendering of TargetParameters.InputTemplate (not the raw SQS record) as the message; records the filter does not match are consumed and never published; the topic then fans each published message out to its subscriptions, so a raw-message-delivery SQS subscriber receives exactly the rendered JSON, and a subscription whose FilterPolicyScope is MessageBody matches against that rendered JSON.
Emulator: nothing is ever published. Both checks create the role, queues, topic, subscription and pipe and then poll the subscribed queue(s) for the rendered records; no message ever arrives and the checks reach the 60 s harness budget with no verdict ('Check timed out after 60.0s'). Sibling pipes checks whose target is an SQS queue (iam__pipes__sqs, dynamodb__pipes__sqs) complete inside the same budget, so the stall is specific to the SNS-topic target rather than to pipe setup or to the budget itself.
Why: every Pipes→SNS fan-out silently delivers nothing. A client sees a pipe reporting RUNNING while its topic and all of that topic's subscribers stay empty, and no downstream behaviour that depends on the published payload — the InputTemplate rendering, FilterCriteria suppression, or an SNS MessageBody filter policy evaluated against the rendered body — can be exercised at all.
Fix: the pipes data-plane delivery path for the SNS-topic target type — the SQS-source poller must, for each matched record, render TargetParameters.InputTemplate and issue sns:Publish to the target topic ARN under the pipe's RoleArn (and delete the consumed source message), instead of dropping the record.
Traces: iam__pipes__sns__sqs.pipe-from-a-queue-publishes-only-filtered-transformed-records-to-an-sns-topic, iam__pipes__sns__sqs.pipe-rendered-body-not-the-source-record-is-what-a-body-filter-policy-matches

## Evidence
Both checks: expected the SNS topic's raw-message-delivery SQS subscriber to receive, per matched source message, the InputTemplate rendering of that record (and for the filter check, to receive only the matching record while the filtered-out one is consumed without publication). Actual: no message ever reaches the subscribed queue; the check never gets past its receive loop and the suite reports [mismatch] with 'Error: Check timed out after 60.0s'. Contrast: the same pipe shape with an SQS-queue target passes (iam__pipes__sqs 5/5, dynamodb__pipes__sqs 4/4), and CreatePipe with an SNS target is accepted with HTTP 200 (see the pipes__sns__sqs FIFO evidence), so creation succeeds and only publication is missing.

## Steps to Reproduce
1. CreateRole trusting pipes.amazonaws.com; create a source SQS queue and an output SQS queue; CreateTopic (standard).
2. Give the output queue a policy allowing sns.amazonaws.com sqs:SendMessage from the topic, then Subscribe(TopicArn=<topic>, Protocol='sqs', Endpoint=<output queue ARN>, Attributes={'RawMessageDelivery':'true'}).
3. PutRolePolicy granting sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on the source queue and sns:Publish on the topic.
4. CreatePipe(Name=..., RoleArn=..., Source=<source queue ARN>, SourceParameters={'SqsQueueParameters':{'BatchSize':1}}, Target=<topic ARN>, TargetParameters={'InputTemplate': '{"tier": <$.body.level>, "id": <$.body.id>}'}, DesiredState='RUNNING'); poll DescribePipe until CurrentState == 'RUNNING'.
5. SendMessage JSON bodies to the source queue and ReceiveMessage from the output queue. Observe the output queue stays empty indefinitely.
