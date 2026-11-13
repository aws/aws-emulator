Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields ec2__s3.vpc-flow-log-hive-prefix-grant-covers-the-partitioned-path ec2__s3.vpc-flow-log-to-s3-appends-delivery-grants-to-an-existing-bucket-policy
Issue-Key: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields|run#11|AssertionError|assert s3_only == [], (
Issue-Key: ec2__s3|GetBucketPolicy|NoSuchBucketPolicy
Issue-Key: ec2__s3.vpc-flow-log-to-s3-appends-delivery-grants-to-an-existing-bucket-policy|_poll#8|Timeout|time.sleep(interval_s)
Issue-Commit: 60bb522c193ef1bb97524d7767b3094d62e07a4e

# EC2 flow logs: a cloud-watch-logs flow log reports a synthesized LogDestination ARN, and an S3 flow log never merges the log-delivery grants into the destination bucket policy (2 causes)

> _Note: this issue was identified against the code base at commit `60bb522c193ef1bb97524d7767b3094d62e07a4e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFlowLogs

## Description
Two net-new root causes in the EC2 VPC-Flow-Logs surface. Both are reached through CreateFlowLogs and read back through DescribeFlowLogs / the S3 destination bucket. Neither is covered by the open stub issues 20261022T235108Z-ec2-createflowlogs-stub.md, 20261023T102725Z-ec2-describeflowlogs-stub.md or 20261023T102725Z-ec2-deleteflowlogs-stub.md: in this replay both operations answered 200 and returned a real flow-log record, so these are fidelity defects in the implemented path, not the stub refusals those issues name.

Cause 1 of 2: a CloudWatch Logs flow log reports an S3-shaped LogDestination ARN that AWS does not return.
AWS: Smithy (com-amazonaws-ec2-amazonec2) models FlowLog.LogDestination as "The Amazon Resource Name (ARN) of the destination for the flow log data" and FlowLog.LogGroupName as "The name of the flow log group"; CreateFlowLogs documents LogDestination as the ALTERNATIVE to LogGroupName ("Alternatively, use the LogGroupName parameter"). Recorded behavior (measured on real AWS, stated in the check): when a flow log is created with LogGroupName + DeliverLogsPermissionArn and no LogDestination, DescribeFlowLogs returns LogDestinationType=cloud-watch-logs, LogGroupName, DeliverLogsPermissionArn, MaxAggregationInterval=600 and the default LogFormat, and carries NEITHER LogDestination NOR DestinationOptions — those two members are the S3 shape and are absent from a cloud-watch-logs record.
Emulator: DescribeFlowLogs returns a LogDestination the request never supplied. services/service-com-amazonaws-ec2-amazonec2/src/ops/create_flow_logs.rs defaults it with `state::flow_log_group_arn(&region, &name)` when `input.log_destination` is None, so the stored FlowLogSpec always carries a destination ARN of the form arn:aws:logs:<region>:<account>:log-group:<log group name> (the region, account and group name all come from the request/lease; only the shape is the requirement). The replay observed the key present with exactly that shape, failing `s3_only == []`.
Why: a client that reads LogDestinationType plus the destination fields to decide where a flow log actually delivers sees one flow log that looks like it has both a log-group destination and an ARN destination. Tooling that treats a present LogDestination as "this is an S3/Firehose flow log", or that round-trips DescribeFlowLogs output back into CreateFlowLogs, mis-classifies or is rejected for an invalid parameter combination.
Fix: CreateFlowLogs' handler/state projection for the cloud-watch-logs destination type — do not synthesize a LogDestination when the request named the group by LogGroupName; store None so DescribeFlowLogs omits logDestination (and keep destinationOptions absent for cloud-watch-logs).
Traces: ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields.

Cause 2 of 2: CreateFlowLogs with LogDestinationType=s3 performs no bucket-policy side effect, so the log-delivery grants are never written or merged.
AWS: recorded behavior only (the Smithy model describes the parameters, not the side effect; CreateFlowLogs documents that S3 flow-log records are "published to a single log file object that is stored in the specified bucket"). The recordings show that creating an S3-destination flow log appends TWO statements to the destination bucket's policy, both with Principal {"Service": "delivery.logs.amazonaws.com"}: an Allow of s3:PutObject on arn:aws:s3:::<bucket>/<optional key prefix>AWSLogs/<account>/* (with StringEquals {"s3:x-amz-acl": "bucket-owner-full-control", "aws:SourceAccount": <account>}), and an Allow of s3:GetBucketAcl on arn:aws:s3:::<bucket>; both scope ArnLike aws:SourceArn to arn:aws:logs:<region>:<account>:*. The bucket, key prefix, account and region are values taken from the request and the lease; the statement shape, principal, actions and conditions are the requirement. Any pre-existing statement (e.g. a DenyInsecureTransport deny) survives verbatim — the grants are APPENDED, not substituted. With DestinationOptions.HiveCompatiblePartitions=true the account segment of the granted resource becomes aws-account-id=<account>, i.e. arn:aws:s3:::<bucket>/<prefix>/AWSLogs/aws-account-id=<account>/*.
Emulator: no policy is written at all. create_flow_logs.rs builds a FlowLogSpec and returns FlowLogIds with Unsuccessful=[] but touches no S3 state (no bucket/policy reference anywhere in the handler). Consequently GetBucketPolicy on a bucket that had no prior policy answers 400 NoSuchBucketPolicy "The bucket policy does not exist" forever, and on a bucket that already had a policy the statement count never grows past the client's own statement, so the poll for the appended grants never converges (the second check was killed by the 60 s harness cap while polling). The flow log nevertheless reports FlowLogStatus=ACTIVE / DeliverLogsStatus=SUCCESS.
Why: a client that validates its logging setup by reading the destination bucket policy concludes delivery is unauthorized (or, worse, trusts DeliverLogsStatus=SUCCESS and ships a configuration that could never deliver). The merge semantics matter independently: a fix that overwrites the policy instead of appending would silently drop a customer's enforce-SSL deny.
Fix: CreateFlowLogs' handler (the s3 LogDestinationType branch) plus the cross-service state write into the S3 emulator's bucket-policy state — derive the granted prefix from the key prefix inside the LogDestination ARN, the account, and DestinationOptions.HiveCompatiblePartitions, and merge the two statements into any existing policy document rather than replacing it.
Traces: ec2__s3.vpc-flow-log-hive-prefix-grant-covers-the-partitioned-path, ec2__s3.vpc-flow-log-to-s3-appends-delivery-grants-to-an-existing-bucket-policy.

## Evidence
Cause 1 — check ec2__iam__logs.vpc-flow-log-to-cloudwatch-logs-names-the-group-and-omits-s3-fields [mismatch]:
  AssertionError: a CloudWatch Logs flow log reports S3-shaped keys {'LogDestination': 'arn:aws:logs:us-west-2:000000000000:log-group:<log group name>'}
  Expected: DescribeFlowLogs record for a LogGroupName-created flow log contains neither 'LogDestination' nor 'DestinationOptions'.
  Actual: 'LogDestination' present, synthesized by create_flow_logs.rs via state::flow_log_group_arn(&region, &name).

Cause 2 — check ec2__s3.vpc-flow-log-hive-prefix-grant-covers-the-partitioned-path [error]:
  botocore.exceptions.ClientError: An error occurred (NoSuchBucketPolicy) when calling the GetBucketPolicy operation: The bucket policy does not exist
  Expected: after CreateFlowLogs(LogDestinationType=s3, LogDestination=arn:aws:s3:::<bucket>), the bucket policy exists and its s3:PutObject Resource list is ['arn:aws:s3:::<bucket>/AWSLogs/<account>/*']; after a second, prefixed Hive-partitioned flow log it is that plus 'arn:aws:s3:::<bucket>/<prefix>/AWSLogs/aws-account-id=<account>/*'.
  Actual: no bucket policy is ever created.

Cause 2 — check ec2__s3.vpc-flow-log-to-s3-appends-delivery-grants-to-an-existing-bucket-policy [mismatch]:
  Error: Check timed out after 60.0s, while polling for the bucket policy to reach >= 3 statements (the client's own DenyInsecureTransport plus the two appended delivery statements).
  Expected: 3 statements — DenyInsecureTransport preserved verbatim, plus Allow s3:PutObject on arn:aws:s3:::<bucket>/AWSLogs/<account>/* with StringEquals {s3:x-amz-acl: bucket-owner-full-control, aws:SourceAccount: <account>}, plus Allow s3:GetBucketAcl on arn:aws:s3:::<bucket>, both Principal {Service: delivery.logs.amazonaws.com} and ArnLike aws:SourceArn arn:aws:logs:<region>:<account>:*.
  Actual: the policy keeps only the client's single statement; CreateFlowLogs still returned FlowLogIds with Unsuccessful=[] and the flow log reads ACTIVE/SUCCESS.

## Steps to Reproduce
Cause 1:
1. ec2 CreateVpc(CidrBlock=10.2.0.0/16).
2. iam CreateRole for the vpc-flow-logs.amazonaws.com service principal; logs CreateLogGroup(<group>).
3. ec2 CreateFlowLogs(ResourceIds=[<vpc>], ResourceType=VPC, TrafficType=ALL, LogGroupName=<group>, DeliverLogsPermissionArn=<role arn>) — note: NO LogDestination.
4. ec2 DescribeFlowLogs(FlowLogIds=[<id>]) and inspect the record: 'LogDestination' and 'DestinationOptions' must both be absent. The emulator returns LogDestination=arn:aws:logs:<region>:<account>:log-group:<group>.

Cause 2:
1. ec2 CreateVpc(CidrBlock=10.2.0.0/16); s3 CreateBucket(<bucket>).
2. (append variant) s3 PutBucketPolicy(<bucket>) with a single DenyInsecureTransport statement.
3. ec2 CreateFlowLogs(ResourceIds=[<vpc>], ResourceType=VPC, TrafficType=ALL, LogDestinationType=s3, LogDestination=arn:aws:s3:::<bucket>).
4. s3 GetBucketPolicy(<bucket>): expect the two delivery.logs.amazonaws.com statements appended (and the pre-existing deny untouched). The emulator answers NoSuchBucketPolicy when there was no prior policy, and leaves the prior policy unchanged when there was one.
5. (hive variant) ec2 CreateFlowLogs(..., LogDestination=arn:aws:s3:::<bucket>/vpc/, TrafficType=REJECT, DestinationOptions={FileFormat: plain-text, HiveCompatiblePartitions: true, PerHourPartition: false}) and re-read the policy: the s3:PutObject grants must be exactly the unprefixed arn:aws:s3:::<bucket>/AWSLogs/<account>/* plus arn:aws:s3:::<bucket>/vpc/AWSLogs/aws-account-id=<account>/*.
