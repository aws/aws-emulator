Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3 cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-unlisted-namespace-blocked-until-update
Issue-Key: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3|run#22|AssertionError|assert desc["DeliveryStreamStatus"] == "ACTIVE", (
Issue-Key: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-unlisted-namespace-blocked-until-update|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# CloudFormation-deployed metric-stream pipeline: the stack reports CREATE_COMPLETE while AWS::KinesisFirehose::DeliveryStream is still CREATING, and the deployed pipeline exports nothing to S3 (2 causes)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct causes in one CloudFormation-deployed Bucket -> Firehose(role) -> CloudWatch MetricStream(role) pipeline.

Cause 1 of 2: the AWS::KinesisFirehose::DeliveryStream resource does not stabilise before the stack completes. Failing check: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3 (the same defect also blocks the second check, which deploys the same template).
AWS: CloudFormation only reports CREATE_COMPLETE for a resource once its create has stabilised, so immediately after the stack reaches CREATE_COMPLETE a DescribeDeliveryStream on the template's DeliveryStreamName reports DeliveryStreamStatus = ACTIVE (recorded behavior: the check reads the status once, with no polling, straight after CREATE_COMPLETE). Nothing in the Firehose model makes CREATING observable after a completed create.
Emulator: the stack reaches CREATE_COMPLETE but DescribeDeliveryStream answers DeliveryStreamStatus = 'CREATING' (200, no error), so the resource was reported complete while still provisioning.
Why: a client that deploys a stack and then uses the stream — publishes to it, attaches a metric stream, or asserts the destination configuration — gets a stream that refuses data (puts to a non-ACTIVE stream are refused) even though CloudFormation told it the deployment finished. Every post-deploy step built on CREATE_COMPLETE becomes racy against the emulator and correct against AWS.
Fix: the CloudFormation resource provider for AWS::KinesisFirehose::DeliveryStream must not report the resource CREATE_COMPLETE until the created stream is observable as ACTIVE (or the firehose state must activate a stream created through that path as the direct CreateDeliveryStream path does — streams created through the Firehose API do reach ACTIVE in this same run).

Cause 2 of 2: the deployed metric stream exports nothing, so the filter's differential barrier is never reachable. Failing check: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-unlisted-namespace-blocked-until-update.
AWS: a running AWS::CloudWatch::MetricStream whose IncludeFilters names a namespace exports datapoints published to that namespace through the Firehose stream into the S3 destination, in at least two distinct delivered objects over successive buffer intervals, and exports nothing for a namespace the filter does not list; an UpdateStack that adds the second namespace then makes it arrive in the same bucket (recorded behavior only — no CloudWatch or CloudFormation response carries the exported payload).
Emulator: the check reaches no assertion at all and exhausts the harness budget — 'Check timed out after 60.0s' — while polling the S3 prefix for a delivered record for the LISTED namespace, i.e. the deployed pipeline produces no object. Hypothesis: the same inert export path as the non-CFN metric streams.
Why: the IncludeFilters property has no observable effect, so a client cannot tell a correctly filtered export from no export at all, and a stack update that widens the filter changes nothing.
Fix: the CloudWatch metric-stream exporter (the background export path that writes into the Firehose destination) and the Firehose delivery path it feeds; the exporter must honour the stream's IncludeFilters.
Related but not covering it: open issue 20261024T005347Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-export-nothing-no-records-reach.md owns the non-CFN metric-stream export checks and does not carry this check's replay key; fixed issue 20261022T194351Z-cloudwatch-firehose-s3-cloudwatch-metric-streams-a-cfn-declared-aws-cloudwatch.md made the CFN-declared MetricStream exist (GetMetricStream and the stack Outputs now answer) but left the deployed pipeline inert, and fixed issue 20261022T064119Z-firehose-cloudformation-deployed-firehose-delivery-streams-are-never-provisioned.md now provisions the stream but leaves it in CREATING.

Traces: the two failing check ids above (both deploy the identical template: S3 bucket, IAM roles, AWS::KinesisFirehose::DeliveryStream with ExtendedS3DestinationConfiguration BufferingHints {IntervalInSeconds 60, SizeInMBs 1}, AWS::CloudWatch::MetricStream OutputFormat json).

## Evidence
Cause 1 — expected (AWS): after cfn.create_stack(...) settles at CREATE_COMPLETE, firehose.describe_delivery_stream(DeliveryStreamName=<template's name>)['DeliveryStreamDescription']['DeliveryStreamStatus'] == 'ACTIVE'.
Actual (emulator): AssertionError: the stack completed but delivery stream 'sim2real-xsvc-msfh-8b098d49aca4' is 'CREATING' (check_cloudwatch__firehose__s3_deploy_cloudwatch_3_metric_stream_cfn_delivers_put_metric_to_s3.py line 189).

Cause 2 — expected (AWS): after PutMetricData into the IncludeFilters namespace, objects appear under s3://<stack bucket>/sim2real-metric-stream/ whose bodies carry the metric name and value, in >= 2 distinct objects; the unlisted namespace appears in none; after UpdateStack adds it, it appears.
Actual (emulator): no object is ever observed — the check reaches no assertion and the harness reports 'Check timed out after 60.0s' (its own ceilings are _STACK_TIMEOUT_S=600, _DELIVERY_TIMEOUT_S=480, so it was still polling for the first delivery).

## Steps to Reproduce
1. cfn.create_stack with the template used by these checks: AWS::S3::Bucket, an IAM role trusting firehose.amazonaws.com, AWS::KinesisFirehose::DeliveryStream (DirectPut, ExtendedS3DestinationConfiguration -> the bucket, Prefix 'sim2real-metric-stream/', BufferingHints {IntervalInSeconds: 60, SizeInMBs: 1}, CompressionFormat UNCOMPRESSED), an IAM role trusting streams.metrics.cloudwatch.amazonaws.com, and AWS::CloudWatch::MetricStream (OutputFormat json, IncludeFilters [one namespace]); Capabilities ['CAPABILITY_NAMED_IAM'].
2. Poll DescribeStacks until the status is terminal; it reaches CREATE_COMPLETE.
3. Immediately call firehose.describe_delivery_stream on the template's DeliveryStreamName -> emulator reports 'CREATING' (cause 1); AWS reports 'ACTIVE'.
4. cloudwatch.put_metric_data into the IncludeFilters namespace, then list s3://<bucket>/sim2real-metric-stream/ repeatedly -> emulator never produces an object (cause 2).
5. UpdateStack with IncludeFilters naming a second, previously unlisted namespace and publish to it; the new namespace must then appear in the same bucket.
6. Re-replay: cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-delivers-put-metric-to-s3, cloudwatch__firehose__s3.deploy-cloudwatch-3-metric-stream-cfn-unlisted-namespace-blocked-until-update.
