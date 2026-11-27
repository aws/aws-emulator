Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative
Issue-Key: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: 5c66e8f363b2794f9d87fe11d0e172b4338f59d2

# firehose__kinesis__s3: the CFN Kinesis→Firehose→S3 missing-permission stack is still rolled back during create — CreateStack ends ROLLBACK_COMPLETE instead of CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `5c66e8f363b2794f9d87fe11d0e172b4338f59d2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
REOPENS 20261122T172411Z-firehose-kinesis-s3-the-cfn-kinesis-firehose-s3.md: its fix left the call refused.

Cause 1 of 1: a CloudFormation stack declaring a Kinesis→Firehose→S3 pipeline is rolled back during create, so the stack never reaches CREATE_COMPLETE and none of the resources it declares (bucket, source stream, log group and log streams, the two roles, the two delivery streams) is available to the caller. The template is plain, fully-modelled CloudFormation: one AWS::Kinesis::Stream (ShardCount 1), one AWS::S3::Bucket, one AWS::Logs::LogGroup (RetentionInDays 1) with two AWS::Logs::LogStream children, two AWS::IAM::Role resources trusting firehose.amazonaws.com (identical except that one omits kinesis:GetRecords), and two AWS::KinesisFirehose::DeliveryStream resources with DeliveryStreamType KinesisStreamAsSource, a KinesisStreamSourceConfiguration (KinesisStreamARN + RoleARN via Fn::GetAtt) and a legacy S3DestinationConfiguration carrying BucketARN, RoleARN, Prefix, BufferingHints {IntervalInSeconds 0, SizeInMBs 1} and CloudWatchLoggingOptions. Created with Capabilities CAPABILITY_IAM/CAPABILITY_NAMED_IAM.

AWS: CreateStack for this template is accepted and the stack settles CREATE_COMPLETE, exposing its five Outputs (BucketName, StreamName, LogGroupName, BadLogStreamName, BadRoleName). A stack only enters ROLLBACK_COMPLETE when a declared resource fails to create or stabilize. Every property used is modelled and in range — Smithy (com.amazonaws.firehose): CreateDeliveryStream accepts DeliveryStreamType, KinesisStreamSourceConfiguration (KinesisStreamARN and RoleARN both required and ARN-patterned) and S3DestinationConfiguration (@deprecated but still accepted; RoleARN and BucketARN required, Prefix @length 0–1024, BufferingHints, CloudWatchLoggingOptions); BufferingHints.IntervalInSeconds @range 0–900 and SizeInMBs @range 1–128, so {0, 1} is valid. CreateDeliveryStream's modelled refusals are InvalidArgumentException / InvalidKMSResourceException / LimitExceededException / ResourceInUseException (all HTTP 400) and none applies. Recorded behavior only for the stack trajectory itself: the recording gets past the deploy and goes on to exercise delivery, which is the evidence that CreateStack for this template reaches CREATE_COMPLETE.
Emulator: the create does not complete. The check's deploy helper polls DescribeStacks until a terminal create state and observes ROLLBACK_COMPLETE, failing with `AssertionError: Stack <name> ended in ROLLBACK_COMPLETE`. Because the deploy is the check's first step, no Outputs are returned and nothing downstream is exercised: the permissioned sibling's delivery under good/, the Kinesis.AccessDenied delivery-error log record on the denied stream, the absence of the denied marker under bad/, and the restore-after-grant leg are all unreached. The replay output carries only the terminal stack status, not the failing resource's ResourceStatusReason, so the specific resource the emulator objected to is not attributable from the replay — which is itself part of the defect.
Why: a client deploying a Kinesis-sourced Firehose pipeline by CloudFormation gets no pipeline at all — CreateStack is accepted, then the stack is rolled back and every declared resource deleted, so the bucket, stream, roles and delivery streams the application depends on never exist. The template is undeployable, and because the rollback carries no service exception in ResourceStatusReason there is nothing for the client to correct.
Fix: the CloudFormation resource-provisioning path for this resource set — the create/stabilize handler of whichever resource fails (the AWS::KinesisFirehose::DeliveryStream provider for a KinesisStreamAsSource stream with a legacy S3DestinationConfiguration is the prime suspect, with AWS::Logs::LogStream and the Fn::GetAtt / Fn::Sub resolution of the role and bucket ARNs the other candidates) must create and stabilize so the stack reaches CREATE_COMPLETE; and, whatever the cause, the failing resource's ResourceStatusReason must carry the service exception that caused the rollback so the refusal is diagnosable.
Related: open issue 20261127T120636Z-firehose-kinesis-s3-2-net-new-causes-the.md names this same check under its Cause 2, but records it as a 60 s no-verdict TIMEOUT, not a rolled-back stack; the suite does not report this failure as owned by it, and the stopping point is now the deploy itself. The earlier no-verdict issue 20261120T082455Z-firehose-kinesis-s3-the-cfn-deployed-kinesis-firehose.md is also prior art.
Traces: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout (service heading `### firehose__kinesis__s3`, 22/24 passed).

[mismatch] firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative
  File "check_firehose__kinesis__s3_cfn_deploy_firehose_missing_kinesis_permission_negative.py", line 24, in run
    stack = _deploy_cfn(ctx, template)
  File "check_firehose__kinesis__s3_cfn_deploy_firehose_missing_kinesis_permission_negative.py", line 226, in _deploy_cfn
    raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
  AssertionError: Stack sim2real-xsvc-stk-f2572d46f976 ended in ROLLBACK_COMPLETE

Expected (AWS): CreateStack is accepted and DescribeStacks reports StackStatus CREATE_COMPLETE with the template's five Outputs (BucketName, StreamName, LogGroupName, BadLogStreamName, BadRoleName) resolved.
Actual (emulator): DescribeStacks reports StackStatus ROLLBACK_COMPLETE, no Outputs are available, and the check aborts on its first step.

This is the same replay key the fixed issue 20261122T172411Z-firehose-kinesis-s3-the-cfn-kinesis-firehose-s3.md carried (`...|_deploy_cfn#2|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")`): the call is still refused after that fix.

The stack name (sim2real-xsvc-stk-f2572d46f976) and all generated bucket/stream/role/log-stream names are per-run values and are not part of the requirement; the requirement is that a stack declaring these resource types and properties reaches CREATE_COMPLETE, and that any resource failure is reported with the service exception in its ResourceStatusReason.

Supporting model citations: Smithy com.amazonaws.firehose — CreateDeliveryStream input members DeliveryStreamType, KinesisStreamSourceConfiguration, S3DestinationConfiguration (@deprecated, still accepted); BufferingHints.IntervalInSeconds @range 0–900, SizeInMBs @range 1–128; errors InvalidArgumentException/InvalidKMSResourceException/LimitExceededException/ResourceInUseException, all HTTP 400 — none of which this request violates.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CloudFormation CreateStack with Capabilities ["CAPABILITY_IAM","CAPABILITY_NAMED_IAM"] and a template whose Resources are:
   - SourceStream: AWS::Kinesis::Stream, Properties {ShardCount: 1}
   - DestBucket: AWS::S3::Bucket
   - LogGroup: AWS::Logs::LogGroup, Properties {RetentionInDays: 1}
   - GoodLogStream / BadLogStream: AWS::Logs::LogStream, Properties {LogGroupName: {"Ref": "LogGroup"}}
   - GoodRole / BadRole: AWS::IAM::Role trusting firehose.amazonaws.com via sts:AssumeRole, each with one inline policy granting the S3 write actions on {"Fn::GetAtt": ["DestBucket","Arn"]} and {"Fn::Sub": "${DestBucket.Arn}/*"}, logs:PutLogEvents on "*", and Kinesis actions on {"Fn::GetAtt": ["SourceStream","Arn"]} — GoodRole with kinesis:DescribeStream/DescribeStreamSummary/GetShardIterator/ListShards/GetRecords, BadRole with the same list minus kinesis:GetRecords.
   - GoodDeliveryStream / BadDeliveryStream: AWS::KinesisFirehose::DeliveryStream, Properties {DeliveryStreamType: "KinesisStreamAsSource", KinesisStreamSourceConfiguration: {KinesisStreamARN: GetAtt SourceStream.Arn, RoleARN: GetAtt <role>.Arn}, S3DestinationConfiguration: {BucketARN: GetAtt DestBucket.Arn, RoleARN: GetAtt <role>.Arn, Prefix: "good/" | "bad/", BufferingHints: {IntervalInSeconds: 0, SizeInMBs: 1}, CloudWatchLoggingOptions: {Enabled: true, LogGroupName: Ref LogGroup, LogStreamName: Ref <log stream>}}}
   with Outputs BucketName/StreamName/LogGroupName/BadLogStreamName/BadRoleName.
3. Poll DescribeStacks on the stack. On AWS it settles CREATE_COMPLETE; observe that the emulator settles ROLLBACK_COMPLETE and returns no Outputs.
4. Call DescribeStackEvents on the stack to see which resource failed and whether its ResourceStatusReason names a service exception (it must).
5. After the stack deploys, the rest of the scenario must be exercisable: PutRecord two markers into the Kinesis stream, the permissioned sibling delivers under good/, the denied stream delivers nothing and logs errorCode 'Kinesis.AccessDenied', and an in-place grant of kinesis:GetRecords restores delivery under bad/.
6. Re-replay the check in trace_ids to confirm the fix.
