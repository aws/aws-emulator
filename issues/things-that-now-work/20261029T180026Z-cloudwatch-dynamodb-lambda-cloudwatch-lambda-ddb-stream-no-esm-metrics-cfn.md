Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.ddb-stream-lambda-batch-size-limit-metric cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception cloudwatch__dynamodb__lambda.ddb-stream-lambda-invocation-count-metric cloudwatch__dynamodb__lambda.ddb-stream-lambda-iteratorage-metric-cloudwatch cloudwatch__dynamodb__lambda.ddb-stream-lambda-missing-cloudwatch-permission-negative cloudwatch__dynamodb__lambda.ddb-stream-lambda-throttled-esm-metric cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative cloudwatch__dynamodb__lambda.deploy-ddb-lambda-cloudwatch-alarm-stack cloudwatch__dynamodb__lambda.deploy-lambda-log-group-retention-configured cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-batch-size-limit-metric|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-error-metric-on-handler-exception|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-invocation-count-metric|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-iteratorage-metric-cloudwatch|_poll_until#10|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-missing-cloudwatch-permission-negative|_poll_until#17|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.ddb-stream-lambda-throttled-esm-metric|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative|run#12|IndexError|state = cw.describe_alarms(AlarmNames=[alarm_name])["MetricAlarms"][0]["StateValue"]
Issue-Key: cloudwatch__dynamodb__lambda|GetFunction|ResourceNotFoundException
Issue-Key: cloudwatch__dynamodb__lambda.deploy-lambda-log-group-retention-configured|run#9|AssertionError|assert len(matches) == 1, f"expected exactly one log group {log_group_name}, got {matches}"
Issue-Key: cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected|_error#9|AssertionError|raise AssertionError(f"call was ACCEPTED but real AWS rejects it; response: {resp!r}")
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# CloudWatch/Lambda/DDB-stream: no ESM metrics, CFN-created functions and log groups invisible, MetricsConfig not validated (5 causes)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
Five net-new root causes in the cloudwatch__dynamodb__lambda group. Each needs its own fix.

Cause 1 of 5: DynamoDB-stream event source mappings never run, so no Lambda/ESM metric is ever emitted.
AWS: an enabled ESM polls the stream, invokes the function, and the resulting `AWS/Lambda` metrics (Invocations, Errors, IteratorAge, Throttles) appear in CloudWatch with the function dimension.
Emulator: no delivery and no metric data; the six checks poll until `Check timed out after 60.0s`.
Why: stream-driven workloads are silently inert and nothing can alarm on them.
Fix: the lambda event-source-mapping poller for DynamoDB streams plus its metric emission into cloudwatch.
Traces: cloudwatch__dynamodb__lambda.ddb-stream-lambda-batch-size-limit-metric, .ddb-stream-lambda-error-metric-on-handler-exception, .ddb-stream-lambda-invocation-count-metric, .ddb-stream-lambda-iteratorage-metric-cloudwatch, .ddb-stream-lambda-missing-cloudwatch-permission-negative, .ddb-stream-lambda-throttled-esm-metric

Cause 2 of 5: a composite-alarm stack yields no inspectable resources.
AWS: a stack declaring a composite alarm over a missing metric alarm fails the resource and the stack's resource/alarm listings are populated enough to read the failure.
Emulator: the check crashes with `IndexError: list index out of range` — the listing it indexes (stack resources / alarms) is empty.
Why: a caller cannot discover what the stack created or why the composite alarm was refused.
Fix: the cloudformation stack-resource listing for AWS::CloudWatch::CompositeAlarm (and the alarm state it should create).
Traces: cloudwatch__dynamodb__lambda.deploy-cloudwatch-composite-alarm-missing-metric-negative

Cause 3 of 5: a function created by a CloudFormation stack is not visible to Lambda.
AWS: once the stack reaches CREATE_COMPLETE, `GetFunction` on the generated function name/ARN answers 200.
Emulator: `ResourceNotFoundException` `Function not found: arn:aws:lambda:<region>:<account>:function:<generated name>` — the stack reported success but the function does not exist in the lambda service.
Why: a "successful" deploy leaves no function; every post-deploy step against it fails. (The open issue 20261025T020208Z-cloudwatch-dynamodb-lambda-lambda-listeventsourcemappings-answers-resourcenotfoundexception-function-not-found-for.md carries the ListEventSourceMappings siblings; it does not carry this GetFunction replay key.)
Fix: the cloudformation AWS::Lambda::Function provisioner (register the function in lambda state).
Traces: cloudwatch__dynamodb__lambda.deploy-ddb-lambda-cloudwatch-alarm-stack

Cause 4 of 5: the Lambda log group a template configures is never created.
AWS: the function's `/aws/lambda/<function name>` log group exists (and carries the configured retention) after the stack deploys.
Emulator: `DescribeLogGroups` returns `[]` for that name.
Why: retention configuration cannot be verified and nothing can be read from the function's logs.
Fix: the cloudformation/lambda log-group creation on function provisioning (cloudwatchlogs state).
Traces: cloudwatch__dynamodb__lambda.deploy-lambda-log-group-retention-configured

Cause 5 of 5: CreateEventSourceMapping accepts a Kafka-only MetricsConfig on a DynamoDB-stream source.
AWS: `MetricsConfig` is only valid for the event-source types that support it; supplying it for a DynamoDB-stream mapping is refused with a client error (HTTP 400).
Emulator: the call is ACCEPTED (HTTP 202) and the mapping is returned with `MetricsConfig: {'Metrics': ['ErrorCount']}`.
Why: an invalid mapping is created, so the caller believes it configured metrics that will never exist, and the negative path is untested.
Fix: CreateEventSourceMapping / UpdateEventSourceMapping input validation (MetricsConfig vs event-source type).
Traces: cloudwatch__dynamodb__lambda.xsvc-lambda-1-esm-metrics-config-kafka-only-metric-rejected

## Evidence
- Cause 1 (six checks): Error: Check timed out after 60.0s.
- Cause 2: IndexError: list index out of range.
- Cause 3: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetFunction operation: Function not found: arn:aws:lambda:us-west-2:000000000000:function:<stack-generated name>.
- Cause 4: AssertionError: expected exactly one log group /aws/lambda/<function name>, got [].
- Cause 5: AssertionError: call was ACCEPTED but real AWS rejects it; response: {'ResponseMetadata': {'HTTPStatusCode': 202, ...}, 'UUID': ..., 'EventSourceArn': 'arn:aws:dynamodb:...:table/<table>/stream/<ts>', 'MetricsConfig': {'Metrics': ['ErrorCount']}, 'State': 'Updating', ...}.

## Steps to Reproduce
Boot the emulator and replay the ten checks in trace_ids.
