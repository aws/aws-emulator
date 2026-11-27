Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-create-volume-gp3-throughput-per-iops-ratio
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-create-volume-gp3-throughput-per-iops-ratio ec2-create-volume-io1-iops-per-gib-ratio ec2-copy-snapshot-error-handling ec2-volume-snapshot-copy-lifecycle ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block ec2-describe-instance-attribute-ena-support ec2-vfd-firehose-destination-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateVolume|4xx->2xx|$.Response
Issue-Key: ec2|CreateVolume|4xx->2xx|status
Issue-Key: ec2|ModifyVolume|4xx->2xx|$.Response
Issue-Key: ec2|ModifyVolume|4xx->2xx|status
Issue-Key: ec2|CopySnapshot|4xx->2xx|$.Response
Issue-Key: ec2|CopySnapshot|4xx->2xx|status
Issue-Key: ec2|CopySnapshot|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CopySnapshot|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CopySnapshot|4xx->4xx|status
Issue-Key: ec2|AssociateVpcCidrBlock|4xx->2xx|$.Response
Issue-Key: ec2|AssociateVpcCidrBlock|4xx->2xx|status
Issue-Key: ec2|DescribeInstanceAttribute|4xx->2xx|$.Response
Issue-Key: ec2|DescribeInstanceAttribute|4xx->2xx|status
Issue-Key: ec2|CreateFlowLogs|4xx->2xx|$.Response
Issue-Key: ec2|CreateFlowLogs|4xx->2xx|status
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2 accepts invalid volume, snapshot, CIDR, attribute and flow-log requests AWS refuses

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVolume

## Description
Seven request-validation rules are missing from the ec2 crate: the emulator answers 200 (or, under DryRun, 412 DryRunOperation) for requests AWS refuses with 400. The shared fault is that these handlers perform no semantic validation of the request before mutating/answering, and where DryRun is involved the DryRun short-circuit runs BEFORE validation instead of after it. This is a BUNDLE OF 7 CAUSES; all seven must be closed.

Cause 1 of 7: CreateVolume with a Throughput:Iops ratio above the gp3 maximum.
AWS: Smithy: CreateVolume.Throughput and .Iops are plain integers with no @range, and CreateVolume declares no modeled errors, so the rule is recorded behavior only. AWS refuses 400 Code 'InvalidParameterValue', Message 'Throughput (MiBps) to iops ratio of <requested-ratio> is too high; maximum is <max-ratio> MiBps per iops'. Fixed text: 'Throughput (MiBps) to iops ratio of ', ' is too high; maximum is ', ' MiBps per iops'; variable parts: the ratio computed from the request's Throughput/Iops and the service maximum, both formatted to six decimal places.
Emulator: 200 — the volume is created with the out-of-range combination and no Response error is emitted.
Why: a client gets a volume whose throughput/IOPS combination AWS would never provision, so capacity-planning and the error path it is testing are both invisible.
Fix: CreateVolume validation (throughput-per-IOPS ratio check for gp3) in the ec2 volume ops.
Traces: ec2-create-volume-gp3-throughput-per-iops-ratio#task#4 (cluster also covers ec2-create-volume-io1-iops-per-gib-ratio#task#4).

Cause 2 of 7: ModifyVolume with the same out-of-range Throughput:Iops ratio.
AWS: recorded behavior only; same refusal as cause 1 — 400 'InvalidParameterValue' with the 'Throughput (MiBps) to iops ratio of ... is too high; maximum is ... MiBps per iops' template.
Emulator: 200, the modification is applied.
Why: the same unprovisionable configuration is reachable by modification even if creation is fixed — this is a separate code path from cause 1.
Fix: ModifyVolume validation (same ratio rule, modify path).
Traces: ec2-create-volume-gp3-throughput-per-iops-ratio#task#3 (cluster also covers ec2-create-volume-io1-iops-per-gib-ratio#task#3).

Cause 3 of 7: CopySnapshot with an invalid SourceSnapshotId.
AWS: recorded behavior only. 400 Code 'InvalidParameterValue', Message 'The specified snapshotId is invalid' — fixed text with no variable part.
Emulator: 200, the copy is accepted.
Why: the client receives a snapshot id for a copy that could not exist, and subsequent describes of it diverge too.
Fix: CopySnapshot source-id validation.
Traces: ec2-copy-snapshot-error-handling#task#1 (cluster also covers ec2-volume-snapshot-copy-lifecycle#task#2).

Cause 4 of 7: CopySnapshot with DryRun=true AND an invalid SourceSnapshotId — DryRun is evaluated before validation.
AWS: recorded behavior only. AWS validates parameters first, so the invalid snapshotId wins: 400 'InvalidParameterValue' / 'The specified snapshotId is invalid'. The DryRunOperation response (412, 'Request would have succeeded, but DryRun flag is set.') is only emitted when the request WOULD have succeeded.
Emulator: 412 Code 'DryRunOperation', Message 'Request would have succeeded, but DryRun flag is set.' — it reports that an invalid request would have succeeded.
Why: DryRun is used precisely as a pre-flight validator; answering DryRunOperation for a request AWS rejects makes the pre-flight useless and actively misleading.
Fix: the DryRun short-circuit ordering in CopySnapshot (and the shared DryRun helper): validate parameters, then honour DryRun.
Traces: ec2-copy-snapshot-error-handling#task#2.

Cause 5 of 7: AssociateVpcCidrBlock with a CIDR from a different RFC1918 range than the VPC's primary CIDR.
AWS: recorded behavior only. 400 Code 'InvalidVpc.Range', Message "The CIDR '<cidr>' is restricted. Use a CIDR from the same private address range as the current VPC CIDR, or use a publicly-routable CIDR. For additional restrictions, see https://docs.aws.amazon.com/vpc/latest/userguide/vpc-cidr-blocks.html". Fixed text: everything but the quoted CIDR; variable part: the requested CidrBlock.
Emulator: 200 — the association is created, and the extra association then shows up in DescribeVpcs (see the separate state-projection issue).
Why: a client building a multi-CIDR VPC never learns the range restriction, and the VPC's association set diverges from AWS for the rest of the trajectory.
Fix: AssociateVpcCidrBlock validation — compare the requested CIDR's private-range class with the VPC's existing CIDRs and refuse with InvalidVpc.Range.
Traces: ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block#task#2 (cluster also covers #task#3).

Cause 6 of 7: DescribeInstanceAttribute with Attribute=enaSupport.
AWS: Smithy models enaSupport as a value of InstanceAttributeName and EnaSupport as an output member, but the recording shows AWS's DescribeInstanceAttribute does NOT accept it: 400 Code 'InvalidParameterValue', Message 'Value (enaSupport) for parameter attribute is invalid. Unknown attribute.' Fixed text: 'Value (', ') for parameter attribute is invalid. Unknown attribute.'; variable part: the rejected attribute name as spelled in the request.
Emulator: 200 with an attribute payload.
Why: a client probing instance capabilities cannot tell a supported attribute from an unsupported one, and code written against the emulator breaks against AWS.
Fix: DescribeInstanceAttribute's accepted-attribute list (reject the attribute names AWS refuses even though they appear in the model's enum).
Traces: ec2-describe-instance-attribute-ena-support#task#3.

Cause 7 of 7: CreateFlowLogs naming a Kinesis Data Firehose delivery stream in another Region.
AWS: recorded behavior only. 400 Code 'InvalidParameter', Message 'Invalid Kinesis Data Firehose LogDestination <arn>. Cross-region delivery not supported.' Fixed text: 'Invalid Kinesis Data Firehose LogDestination ', '. Cross-region delivery not supported.'; variable part: the LogDestination ARN from the request.
Emulator: 200 — the flow log is created against a destination in a foreign Region.
Why: the flow log is created but could never deliver; the client's validation test passes against the emulator and fails in production.
Fix: CreateFlowLogs destination validation — parse the LogDestination ARN's Region and refuse a cross-Region Firehose destination with InvalidParameter.
Traces: ec2-vfd-firehose-destination-mistakes#task#3.

## Evidence
Cause 1 — CreateVolume: expected 400, got 200; expected body {'Errors': {'Error': {'Code': 'InvalidParameterValue', 'Message': 'Throughput (MiBps) to iops ratio of 0.333333 is too high; maximum is 0.250000 MiBps per iops'}}}, got no Response element (2 steps).
Cause 2 — ModifyVolume: expected 400, got 200; same expected Code/Message (2 steps).
Cause 3 — CopySnapshot: expected 400, got 200; expected {'Code': 'InvalidParameterValue', 'Message': 'The specified snapshotId is invalid'}, got no Response element (2 steps).
Cause 4 — CopySnapshot with DryRun: expected 400, got 412; Code expected 'InvalidParameterValue', got 'DryRunOperation'; Message expected 'The specified snapshotId is invalid', got 'Request would have succeeded, but DryRun flag is set.'
Cause 5 — AssociateVpcCidrBlock: expected 400, got 200; expected {'Code': 'InvalidVpc.Range', 'Message': "The CIDR '192.168.0.0/16' is restricted. Use a CIDR from the same private address range as the current VPC CIDR, or use a publicly-routable CIDR. For additional restrictions, see https://docs.aws.amazon.com/vpc/latest/userguide/vpc-cidr-blocks.html"}, got no Response element (2 steps).
Cause 6 — DescribeInstanceAttribute: expected 400, got 200; expected {'Code': 'InvalidParameterValue', 'Message': 'Value (enaSupport) for parameter attribute is invalid. Unknown attribute.'}, got no Response element.
Cause 7 — CreateFlowLogs: expected 400, got 200; expected {'Code': 'InvalidParameter', 'Message': 'Invalid Kinesis Data Firehose LogDestination arn:aws:firehose:us-east-1:<account>:deliverystream/<stream>. Cross-region delivery not supported.'}, got no Response element.
The quoted ratios, CIDRs and ARNs are the recordings' request values filling the message templates; the requirement is the status, code and the template's fixed text.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Cause 1: ec2-create-volume-gp3-throughput-per-iops-ratio step task#4 — CreateVolume gp3 with Throughput/Iops above the 0.25 MiBps-per-IOPS ceiling.
3. Cause 2: same fixture step task#3 — ModifyVolume with the same ratio.
4. Cause 3: ec2-copy-snapshot-error-handling step task#1 — CopySnapshot with an invalid SourceSnapshotId.
5. Cause 4: ec2-copy-snapshot-error-handling step task#2 — the same CopySnapshot with DryRun=true.
6. Cause 5: ec2-associate-vpc-cidr-block-restricted-range-on-rfc1918-block step task#2 — AssociateVpcCidrBlock 192.168.0.0/16 on a 10.x VPC.
7. Cause 6: ec2-describe-instance-attribute-ena-support step task#3 — DescribeInstanceAttribute Attribute=enaSupport.
8. Cause 7: ec2-vfd-firehose-destination-mistakes step task#3 — CreateFlowLogs with a Firehose LogDestination ARN in another Region.
9. In each case observe a 2xx/412 where the fixture expects 400 with the quoted Code and Message.
