Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__events__sqs.apigw-aws-integration-eventbridge-putevents-delivers-event
Issue-Key: apigateway__events__sqs.apigw-aws-integration-eventbridge-putevents-delivers-event|_poll#27|Timeout|time.sleep(interval_s)
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# apigateway__events__sqs: a REST API AWS-integration to events:PutEvents never completes — the check exhausts its 60s budget with no event delivered

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: an API Gateway REST API whose method has an AWS integration to the EventBridge PutEvents action does not carry the request through to the bus, so the rule's SQS target never receives the event and the check never reaches a verdict.

AWS: a REST API with integration type AWS, service events and action PutEvents forwards the signed PutEvents body to EventBridge, answers 200 with the PutEvents response body ({"Entries":[{"EventId":...}],"FailedEntryCount":0}), and the event is then matched by any rule on the named bus and delivered to that rule's target. Smithy (eventbridge): PutEvents input Entries @length min 1 max 10, output {FailedEntryCount (default 0), Entries}; the only modeled error is InternalException (500). The integration path itself is not modeled — recorded behavior only: the recording shows the invoke returning the PutEvents result and the queue receiving the event.
Emulator: the check runs out of its 60.0s budget; nothing is asserted after the invoke, i.e. the integrated PutEvents call never produces a delivered event within the budget (no refusal, no error — the flow simply does not complete).
Why: a client that fronts EventBridge with API Gateway (the standard "no-Lambda proxy" pattern) sees its events silently disappear; every downstream rule/target is starved and the caller cannot tell ingestion failed.
Fix: the API Gateway integration dispatcher for AWS-service integrations (the path that maps an AWS integration's service/action onto the emulated target service) and/or the events PutEvents handler it calls.
Traces: apigateway__events__sqs.apigw-aws-integration-eventbridge-putevents-delivers-event.

## Evidence
Expected: the invoke answers the PutEvents response and the rule's queue receives the published event within the check's budget.
Actual: `Error: Check timed out after 60.0s` — the check never observed the delivered event (verdict: mismatch).

## Steps to Reproduce
1. CreateRestApi; add a resource+method with an AWS integration (type=AWS, integration URI naming the events service and the PutEvents action) and a deployment/stage.
2. CreateEventBus, PutRule on a source pattern, PutTargets naming an SQS queue (with a queue policy allowing events.amazonaws.com).
3. POST the PutEvents body to the deployed stage URL.
4. Observe: no message ever arrives on the target queue.
