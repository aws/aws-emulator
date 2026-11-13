Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.ec2mr-cfnres-events-explain-every-listed-resource cloudformation.stack-events-include-resource-status cloudformation.ec2mr-cfnres-update-drops-removed-resource-and-lists-added cloudformation.ec2mr-cfnres-failed-resource-carries-status-reason-rejected-sibling-omits-it cloudformation.ec2mr-cfnres-single-resource-detail-adds-metadata cloudformation.ec2mr-cfnres-physical-id-lookup-returns-whole-owning-stack cloudformation.ec2mr-cfnres-nested-stack-is-one-resource-not-its-children cloudformation.nested-stack-parentage-and-cascade-delete cloudformation__cloudwatch__s3__sns__sqs.cfn-changeset-include-nested-stacks-lists-child-resource-changes cloudformation.deep-cloudformation-2-snapshot-volume cloudformation__lambda__sqs.cfn-sqs-esm-create-list-delete-lifecycle cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services
Issue-Key: cloudformation.ec2mr-cfnres-events-explain-every-listed-resource|run#11|AssertionError|assert len(events) >= floor, (
Issue-Key: cloudformation.stack-events-include-resource-status|run#5|AssertionError|assert any(
Issue-Key: cloudformation.ec2mr-cfnres-update-drops-removed-resource-and-lists-added|run#32|AssertionError|assert "DELETE_COMPLETE" in statuses, (
Issue-Key: cloudformation.ec2mr-cfnres-failed-resource-carries-status-reason-rejected-sibling-omits-it|run#9|AssertionError|assert set(rows) == {"GoodParam", "RejectedParam"}, (
Issue-Key: cloudformation.ec2mr-cfnres-single-resource-detail-adds-metadata|run#11|AssertionError|assert set(bare) == _DETAIL_KEYS, (
Issue-Key: cloudformation.ec2mr-cfnres-physical-id-lookup-returns-whole-owning-stack|run#14|AssertionError|assert message == "Either StackName or PhysicalResourceId must be specified.", (
Issue-Key: cloudformation.ec2mr-cfnres-nested-stack-is-one-resource-not-its-children|run#25|AssertionError|assert child_arn.startswith(f"arn:aws:cloudformation:{ctx.region}:"), (
Issue-Key: cloudformation.nested-stack-parentage-and-cascade-delete|run#23|AssertionError|assert ":stack/" in child_id, (
Issue-Key: cloudformation|DescribeStackResource|ValidationError
Issue-Key: cloudformation.deep-cloudformation-2-snapshot-volume|run#14|AssertionError|assert volume_id.startswith("vol-"), f"physical id is not an EBS volume id: {volume_id!r}"
Issue-Key: lambda|GetEventSourceMapping|ValidationException
Issue-Key: s3tables|GetTableBucket|ValidationException
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation stack events and resource views are incomplete and physical resource ids are synthesized names: no per-resource events, no resources on a failed stack, StackResourceDetail omits Metadata, nested-stack and provider ids are not real ids (7 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackEvents

## Description
This issue bundles 7 net-new root causes in CloudFormation's read surface for events, resources and physical ids. Related open issues cover physical ids for specific services (20261024T043150Z-cloudformation-step-functions-lambda-resources-physical-resource-id.md, 20261022T235108Z-cloudformation-stack-outputs-carry-no-outputvalue-failing-resources.md); none carries the replay keys below.

Cause 1 of 7: DescribeStackEvents emits only stack-level events, never per-resource events.
AWS: Smithy: StackEvent carries LogicalResourceId, PhysicalResourceId, ResourceType, ResourceStatus and ResourceStatusReason; every resource transitions at least CREATE_IN_PROGRESS -> CREATE_COMPLETE, so a two-resource stack owes at least six events.
Emulator: only the two stack-level events (CREATE_IN_PROGRESS / CREATE_COMPLETE for the AWS::CloudFormation::Stack itself) are recorded; no event names any resource logical id.
Why: the event log is the only audit trail for a deploy; without per-resource events no client can tell which resource failed or when it was created.
Fix: emit per-resource events from the provisioning engine and serve them from the DescribeStackEvents handler.
Traces: cloudformation.ec2mr-cfnres-events-explain-every-listed-resource, cloudformation.stack-events-include-resource-status.

Cause 2 of 7: a resource removed by an update gets no DELETE_COMPLETE event.
AWS: dropping a resource from the template records its DELETE_IN_PROGRESS/DELETE_COMPLETE events.
Emulator: the event list for the removed logical id is empty.
Fix: same event path, update/delete phase.
Traces: cloudformation.ec2mr-cfnres-update-drops-removed-resource-and-lists-added.

Cause 3 of 7: a failed stack lists no resources at all.
AWS: DescribeStackResources on a failed stack still lists the declared resources, the failing one carrying ResourceStatusReason and its rejected sibling carrying none.
Emulator: the failed stack lists [].
Why: the reason a deploy failed is unreachable.
Fix: keep resource records for failed stacks in the stack state and serve them from DescribeStackResources.
Traces: cloudformation.ec2mr-cfnres-failed-resource-carries-status-reason-rejected-sibling-omits-it.

Cause 4 of 7: DescribeStackResource's StackResourceDetail omits Metadata.
AWS: Smithy: StackResourceDetail has a Metadata member carrying the resource's declared Metadata attribute; the recorded AWS response for a resource with no Metadata still sends the key set including 'Metadata'.
Emulator: the response carries every other member but no Metadata.
Fix: DescribeStackResource serializer + store the resource's Metadata attribute.
Traces: cloudformation.ec2mr-cfnres-single-resource-detail-adds-metadata.

Cause 5 of 7: DescribeStackResources rejects StackName together with PhysicalResourceId.
AWS: Smithy: DescribeStackResources input = {StackName, LogicalResourceId, PhysicalResourceId}; passing PhysicalResourceId resolves the owning stack, and AWS answers with that stack's resources rather than refusing the combination.
Emulator: answers ValidationError 'Cannot specify both StackName and PhysicalResourceId in the same request'.
Why: the physical-id lookup path (used to find which stack owns a resource) is unusable.
Fix: DescribeStackResources input handling.
Traces: cloudformation.ec2mr-cfnres-physical-id-lookup-returns-whole-owning-stack.

Cause 6 of 7: a nested stack's PhysicalResourceId is a synthesized name instead of the child stack ARN.
AWS: an AWS::CloudFormation::Stack resource's PhysicalResourceId is the child stack's ARN, which is what makes the second hop (DescribeStackResources on the child) possible; the parent lists the nested stack as ONE resource, not its children.
Emulator: the id is '<parent>-Child-<hex>', and DescribeStackResource on it answers ValidationError 'Stack with id ...-Child-... does not exist'.
Why: nested-stack traversal, cascade delete and change sets with IncludeNestedStacks are all broken.
Fix: set the nested-stack resource's physical id to the created child stack ARN in the provisioning engine.
Traces: cloudformation.ec2mr-cfnres-nested-stack-is-one-resource-not-its-children, cloudformation.nested-stack-parentage-and-cascade-delete, cloudformation__cloudwatch__s3__sns__sqs.cfn-changeset-include-nested-stacks-lists-child-resource-changes.

Cause 7 of 7: provider resources get synthesized names as physical ids, so the owning service cannot address them.
AWS: the physical id is the id the owning service issues — an EBS volume is 'vol-...', a Lambda event source mapping is a 36-character UUID, an S3 Tables table bucket is its ARN matching ^arn:aws[-a-z0-9]*:[a-z0-9]+:[-a-z0-9]*:[0-9]{12}:bucket/[a-z0-9_-]{3,63}$.
Emulator: '<stack>-Vol-<hex>', '<stack>-Esm-<hex>', '<stack>-t', so EC2 DescribeVolumes answers InvalidParameterValue, Lambda GetEventSourceMapping answers ValidationException on the UUID length, and S3 Tables GetTableBucket answers ValidationException on the ARN pattern.
Why: nothing a stack creates can be used through the owning service's API.
Fix: each resource type's provisioner must create the resource in the owning service and record the returned id as PhysicalResourceId.
Traces: cloudformation.deep-cloudformation-2-snapshot-volume, cloudformation__lambda__sqs.cfn-sqs-esm-create-list-delete-lifecycle, cloudformation__s3__s3tables__s3vectors.cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services.

## Evidence
Replayed suite:
- ec2mr-cfnres-events-explain-every-listed-resource: "the stack produced 2 events for 2 resources; every resource and the stack itself transition twice, so at least 6 are owed. Got [(stack,'CREATE_COMPLETE'),(stack,'CREATE_IN_PROGRESS')]".
- stack-events-include-resource-status: "expected a CREATE_COMPLETE event for resource 'Handle', got [ ... two AWS::CloudFormation::Stack events only ]".
- ec2mr-cfnres-update-drops-removed-resource-and-lists-added: "the events for the removed resource report []".
- ec2mr-cfnres-failed-resource-carries-status-reason...: "the failed stack lists [], expected both resources to appear".
- ec2mr-cfnres-single-resource-detail-adds-metadata: keys ['DriftInformation','LastUpdatedTimestamp','LogicalResourceId','PhysicalResourceId','ResourceStatus','ResourceType','StackId','StackName'] vs AWS's set that also contains 'Metadata'.
- ec2mr-cfnres-physical-id-lookup-returns-whole-owning-stack: "passing both StackName and PhysicalResourceId answered 'Cannot specify both StackName and PhysicalResourceId in the same request'".
- ec2mr-cfnres-nested-stack-is-one-resource-not-its-children / nested-stack-parentage-and-cascade-delete: PhysicalResourceId '<stack>-Child-<hex>' instead of the child stack ARN; cfn-changeset-include-nested-stacks-lists-child-resource-changes fails with ValidationError 'Stack with id ...-Child-EE50F16290B9 does not exist'.
- deep-cloudformation-2-snapshot-volume: "physical id is not an EBS volume id: '...-Vol-D108746FFFE6'" and DescribeVolumes answers InvalidParameterValue "Expected: 'vol-...'".
- cfn-sqs-esm-create-list-delete-lifecycle: GetEventSourceMapping ValidationException "Value '...-Esm-4A75341838D6' at 'uUID' failed to satisfy constraint: Member must have length less than or equal to 36".
- cfn-s3-bucket-variant-physical-ids-resolve-in-owning-services: GetTableBucket ValidationException on the tableBucketARN pattern.

## Steps to Reproduce
1. Boot the emulator, replay the suite, re-run the trace_ids.
2. Cause 1 repro: CreateStack with two resources, wait for CREATE_COMPLETE, then DescribeStackEvents — only AWS::CloudFormation::Stack events are returned.
3. Cause 6 repro: CreateStack with a nested AWS::CloudFormation::Stack child, then DescribeStackResources(parent) and feed the child's PhysicalResourceId to DescribeStackResources — AWS resolves the child stack, the emulator answers 'Stack with id ... does not exist'.
4. Cause 7 repro: CreateStack with an AWS::EC2::Volume, read its PhysicalResourceId, call ec2:DescribeVolumes with it.
