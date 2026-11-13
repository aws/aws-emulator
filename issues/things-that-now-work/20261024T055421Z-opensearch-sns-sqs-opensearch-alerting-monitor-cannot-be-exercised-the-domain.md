Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false
Issue-Key: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false|run#8|AssertionError|assert status == 200, f"creating index {index!r} returned HTTP {status!r}: {created!r}"
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# OpenSearch alerting monitor cannot be exercised: the domain data plane returns 404 UnknownService 'es'

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One root cause.

Cause 1 of 1: signed requests to the domain endpoint the control plane published are not served, so an alerting monitor and its SNS destination cannot be set up or triggered.
AWS: a created domain publishes DomainStatus.Endpoint and answers SigV4-signed REST requests against it — PUT /<index> creates an index (HTTP 200), and the alerting plugin paths (/_plugins/_alerting/monitors, /_plugins/_alerting/destinations) create a monitor whose trigger publishes its rendered message to the configured SNS destination, withholding it when the trigger condition is false. Recorded behavior only for the rendered message body; the requirement is that the published endpoint is served as an OpenSearch engine and that the monitor's trigger fires only when its condition holds.
Emulator: the first index creation against the endpoint returns HTTP 404 with {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"} — no service is bound to the 'es' endpoint the control plane advertised, so the monitor, its trigger and the SNS->SQS delivery are all unreachable.
Why: alerting is the main reason to put an OpenSearch domain behind SNS/SQS. A client that creates a monitor gets an error that is not an OpenSearch error, and neither the positive (notification published with the rendered count) nor the negative (notification withheld when the trigger is false) behavior exists.
Fix: register and serve the OpenSearch domain data plane (the 'es' endpoint prefix / published domain endpoint host) in the emulator's request router, including the alerting plugin paths and their SNS destination dispatch. Same component as the opensearch data-plane cause filed in this sweep; filed separately because this check belongs to a different service grouping.
Traces: opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false.

## Evidence
Expected: signed PUT https://<DomainStatus.Endpoint>/sim2real-behav-alerting -> HTTP 200, then the alerting monitor's trigger publishes one notification to the SNS topic (delivered to the subscribed SQS queue) and withholds it when the condition is false.
Actual: AssertionError: creating index 'sim2real-behav-alerting' returned HTTP 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}

## Steps to Reproduce
1. opensearch.create_domain(...) and wait for it to become available; read DomainStatus['Endpoint'].
2. Issue a SigV4-signed PUT to https://<endpoint>/sim2real-behav-alerting.
3. Observe HTTP 404 with __type UnknownService for 'es' instead of 200.
4. Replay opensearch__sns__sqs.alerting-monitor-trigger-publishes-the-rendered-count-to-sns-and-withholds-it-when-false.
