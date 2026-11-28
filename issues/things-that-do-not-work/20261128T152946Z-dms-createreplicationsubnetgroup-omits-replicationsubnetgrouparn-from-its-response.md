Issue-Source: trace-sweep-divergence
Issue-Trace: dms-subnet-group-tags-and-repeated-delete
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dms-subnet-group-tags-and-repeated-delete instance-profile-and-subnet-group-lifecycle
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms|CreateReplicationSubnetGroup|2xx->2xx|$.ReplicationSubnetGroup.ReplicationSubnetGroupArn
Issue-Commit: f25215464d32a99c9e2775589ec5cea2397feddd

# dms CreateReplicationSubnetGroup omits ReplicationSubnetGroupArn from its response

> _Note: this issue was identified against the code base at commit `f25215464d32a99c9e2775589ec5cea2397feddd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationSubnetGroup

## Description
Cause 1 of 1: CreateReplicationSubnetGroup returns a 200 whose ReplicationSubnetGroup structure has no ReplicationSubnetGroupArn member, so the newly created subnet group is returned without its ARN.

AWS: recorded behavior only. The on-disk Smithy model (services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101) defines CreateReplicationSubnetGroup -> CreateReplicationSubnetGroupResponse { ReplicationSubnetGroup: ReplicationSubnetGroup }, and the ReplicationSubnetGroup shape models only ReplicationSubnetGroupIdentifier, ReplicationSubnetGroupDescription, VpcId, SubnetGroupStatus, Subnets, SupportedNetworkTypes and IsReadOnly -- it does not model the ARN. The recording nonetheless shows AWS populating $.ReplicationSubnetGroup.ReplicationSubnetGroupArn on the successful create, with the DMS subnet-group ARN form arn:aws:dms:<region>:<account-id>:subgrp:<subnet-group-resource-id>. Only the fixed parts of that value are a requirement: the arn:aws:dms prefix, the subgrp resource type, and the region/account/resource-id segments taken from the request context and the emulator's state. The recorded region (us-west-2), account (000000000000) and generated resource id (sim2real-probe-dms-tagsng-e54e6a64c8) are per-run values, not part of the requirement.

Emulator: replay reports, for the same 200 status, $.ReplicationSubnetGroup.ReplicationSubnetGroupArn: missing (expected 'arn:aws:dms:us-west-2:000000000000:subgrp:sim2real-probe-dms-tagsng-e54e6a64c8', got None). Every other asserted member of the structure matched, so the resource is created and described but is never assigned/emitted an ARN.

Why: a client that creates a subnet group and then uses its ARN -- AddTagsToResource / ListTagsForResource / RemoveTagsFromResource, or any later reference to the subnet group by ARN, or storing the ARN in IaC state -- reads None/absent from the create response and cannot proceed. The AWS SDKs surface this as an unset field rather than an error, so the failure shows up later as a null-ARN API call or a missing output in the caller's own state. The same cluster shows the gap on a second, independent fixture flow (instance-profile-and-subnet-group-lifecycle), so it is the operation's response and not one scenario's setup.

Fix: belongs in the dms CreateReplicationSubnetGroup handler and the state record it builds -- the subnet group's ARN must be composed when the resource is created (from the request region/account and the subnet group's resource id) and stored on the state object, then emitted by the ReplicationSubnetGroup serializer so DescribeReplicationSubnetGroups returns the same value. Note the existing open issue 20261127T114753Z-dms-error-responses-diverge-from-aws-wrong-error.md covers dms error codes/messages only and does not cover this success-path field.

Traces: dms-subnet-group-tags-and-repeated-delete#task#7 (representative) and instance-profile-and-subnet-group-lifecycle#task#7, both clustered under service=dms|CreateReplicationSubnetGroup|status=200->200|keys=$.ReplicationSubnetGroup.ReplicationSubnetGroupArn.

## Evidence
Replay cluster service=dms|CreateReplicationSubnetGroup|status=200->200, verdict=mismatch, expected_status=200, actual_status=200, affected_count=2.

Detail as returned by replay_wire_corpus:
  $.ReplicationSubnetGroup.ReplicationSubnetGroupArn: missing (expected 'arn:aws:dms:us-west-2:000000000000:subgrp:sim2real-probe-dms-tagsng-e54e6a64c8', got None)

Expected (AWS, recorded): the 200 CreateReplicationSubnetGroup body carries ReplicationSubnetGroup.ReplicationSubnetGroupArn set to arn:aws:dms:<region>:<account>:subgrp:<resource-id>.
Actual (emulator): the member is absent from the 200 body (None).

Affected steps: dms-subnet-group-tags-and-repeated-delete#task#7, instance-profile-and-subnet-group-lifecycle#task#7.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus (replay_wire_corpus).
2. Replay fixture dms-subnet-group-tags-and-repeated-delete, step task#7 (equivalently instance-profile-and-subnet-group-lifecycle, step task#7): a dms CreateReplicationSubnetGroup call with ReplicationSubnetGroupIdentifier, ReplicationSubnetGroupDescription and SubnetIds.
3. Inspect the 200 response body at $.ReplicationSubnetGroup.ReplicationSubnetGroupArn.
4. Observe the member is missing, where AWS returns the subnet group's arn:aws:dms:<region>:<account>:subgrp:<resource-id> ARN.
