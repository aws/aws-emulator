Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__cognito-idp.sign-in-successes-carry-the-userpool-and-client-dimensions
Issue-Key: cloudwatch__cognito-idp.sign-in-successes-carry-the-userpool-and-client-dimensions|_poll_until_stable#17|Timeout|time.sleep(interval_s)
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cloudwatch__cognito-idp: user-pool sign-in metrics are never published, so SignInSuccesses with UserPool/UserPoolClient dimensions never appears

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1 — no AWS/Cognito metrics are emitted for user-pool traffic.
AWS: a successful sign-in publishes the AWS/Cognito metric SignInSuccesses carrying the UserPool and UserPoolClient dimensions, visible through GetMetricStatistics/ListMetrics within the usual aggregation delay.
Emulator: the check polls for that datapoint until its budget ends ('Check timed out after 60.0s') — the metric is never published, so neither the metric name nor its dimension set can be observed.
Why: dashboards and alarms built on AWS/Cognito counters stay empty, and a client cannot tell 'no traffic' from 'no instrumentation'.
Fix: metric emission in the cognito-idp sign-in path into the cloudwatch metric store.
Traces: cloudwatch__cognito-idp.sign-in-successes-carry-the-userpool-and-client-dimensions. The open issue 20261029T064332Z-cloudwatch-cognito-idp-cognito-user-pool-traffic-still.md covers the sibling check obs-cognito-idp-2 but does not carry this replay key.

## Evidence
cloudwatch__cognito-idp.sign-in-successes-carry-the-userpool-and-client-dimensions -> 'Error: Check timed out after 60.0s' while waiting for the AWS/Cognito SignInSuccesses datapoint after a successful InitiateAuth. Expected: a datapoint under namespace AWS/Cognito, MetricName SignInSuccesses, Dimensions [{Name:'UserPool',Value:<poolId>},{Name:'UserPoolClient',Value:<clientId>}]. Actual: no datapoint at all.

## Steps to Reproduce
1. CreateUserPool + CreateUserPoolClient, create a confirmed user and sign in successfully.
2. GetMetricStatistics(Namespace='AWS/Cognito', MetricName='SignInSuccesses', Dimensions=[UserPool,<poolId>],[UserPoolClient,<clientId>], Statistics=['Sum']) -> expect a datapoint.
