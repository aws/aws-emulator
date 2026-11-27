Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__events__sqs.account-level-events-metrics-are-published-without-dimensions cloudwatch__events__sqs.default-bus-rule-publishes-triggered-rules-and-matched-events-by-rule-name cloudwatch__events__sqs.delivered-event-publishes-ingestion-to-invocation-latency cloudwatch__events__sqs.matched-event-counts-invocations-created
Issue-Key: cloudwatch__events__sqs.account-level-events-metrics-are-published-without-dimensions|_poll#17|AssertionError|raise AssertionError(f"{description} not observed within {budget_s:.0f}s; last {last!r}")
Issue-Key: cloudwatch__events__sqs.default-bus-rule-publishes-triggered-rules-and-matched-events-by-rule-name|_poll#17|AssertionError|raise AssertionError(f"{description} not observed within {budget_s:.0f}s; last {last!r}")
Issue-Key: cloudwatch__events__sqs.delivered-event-publishes-ingestion-to-invocation-latency|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__events__sqs.matched-event-counts-invocations-created|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudwatch__events__sqs: EventBridge publishes no AWS/Events rule metrics — Invocations, TriggeredRules, MatchedEvents and the ingestion-to-invocation latency series never appear

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricData

## Description
Cause 1 of 1: matched and delivered events are not metered into CloudWatch, at either the account level (no dimensions) or per rule (RuleName dimension).

AWS: a rule that matches an event publishes AWS/Events MatchedEvents and TriggeredRules, and each successful target delivery publishes Invocations, both dimensionless (account level) and dimensioned by RuleName; a delivered event also publishes IngestionToInvocationStartLatency. Datapoints are readable with GetMetricData/GetMetricStatistics within the normal metric delay. Recorded behavior only (these metric names are documented in the EventBridge monitoring guide, not modeled in Smithy).
Emulator: every series is absent — `the dimensionless AWS/Events Invocations series not observed within 240s; last None`, `AWS/Events TriggeredRules under {RuleName=<rule>} not observed within 240s; last None`, and the two delivery-metric checks exhaust their 60s budget without ever seeing a datapoint. The events are delivered to the queue, but nothing is metered.
Why: all EventBridge observability is blind: operators cannot see rule match/invocation volume or delivery latency, and every alarm, dashboard or autoscaling policy keyed on AWS/Events stays in INSUFFICIENT_DATA.
Fix: the metric emission on the events rule-matching and target-invocation paths (the events→cloudwatch metric bridge must publish MatchedEvents, TriggeredRules, Invocations and IngestionToInvocationStartLatency with and without the RuleName dimension).
Traces: cloudwatch__events__sqs.account-level-events-metrics-are-published-without-dimensions, cloudwatch__events__sqs.default-bus-rule-publishes-triggered-rules-and-matched-events-by-rule-name, cloudwatch__events__sqs.delivered-event-publishes-ingestion-to-invocation-latency, cloudwatch__events__sqs.matched-event-counts-invocations-created.

## Evidence
`AssertionError: the dimensionless AWS/Events Invocations series not observed within 240s; last None`
`AssertionError: AWS/Events TriggeredRules under {RuleName=sim2real-behav-rule-<id>} not observed within 240s; last None`
The latency and invocation-count checks end with `Error: Check timed out after 60.0s` (no datapoint ever observed).

## Steps to Reproduce
1. events PutRule on the default bus with a source pattern, PutTargets naming an SQS queue.
2. PutEvents matching the pattern; confirm the message arrives on the queue.
3. cloudwatch GetMetricData for AWS/Events MatchedEvents / TriggeredRules / Invocations, once with no dimensions and once with Dimensions=[{Name:RuleName,Value:<rule>}], and for IngestionToInvocationStartLatency.
4. Observe no datapoints.
