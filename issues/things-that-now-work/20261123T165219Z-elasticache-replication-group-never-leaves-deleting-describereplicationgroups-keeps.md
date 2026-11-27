Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache.elasticache-cluster-mode-slot-routing-moved-and-crossslot
Issue-Shape-Id: com.amazonaws.elasticache#AmazonElastiCacheV9
Issue-Key: elasticache.elasticache-cluster-mode-slot-routing-moved-and-crossslot|_error_code_and_message#12|AssertionError|raise AssertionError(
Issue-Commit: ab053b44c0c4551c75ed9277db4b5c52abcaed14

# ElastiCache replication group never leaves 'deleting': DescribeReplicationGroups keeps serving a deleted group instead of refusing 404 ReplicationGroupNotFoundFault

> _Note: this issue was identified against the code base at commit `ab053b44c0c4551c75ed9277db4b5c52abcaed14`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteReplicationGroup

## Description
One net-new root cause, grounded in the single failing elasticache check the replay report binds to no open issue and does not mark ALREADY FILED.

Cause 1 of 1: a replication group that DeleteReplicationGroup accepted stays in Status 'deleting' forever — it is never removed, so the by-id DescribeReplicationGroups keeps answering 200 with the full projection instead of the not-found refusal, and its member cache clusters keep being reported.

AWS: Smithy (services/service-com-amazonaws-elasticache-amazonelasticachev9/smithy-model/com-amazonaws-elasticache-amazonelasticachev9.json): DeleteReplicationGroup{ReplicationGroupId (required), RetainPrimaryCluster, FinalSnapshotIdentifier} -> DeleteReplicationGroupResult{ReplicationGroup}, errors include InvalidReplicationGroupStateFault (client, httpError 400, awsQueryError InvalidReplicationGroupState) and ReplicationGroupNotFoundFault (client, httpError 404, awsQueryError code ReplicationGroupNotFoundFault); DescribeReplicationGroups{ReplicationGroupId, MaxRecords, Marker} -> ReplicationGroupMessage declares the same ReplicationGroupNotFoundFault at 404. Recorded behavior: DeleteReplicationGroup(RetainPrimaryCluster=false) answers 200 with ReplicationGroup.Status 'deleting', the group's member clusters are torn down, and the group is then REMOVED — after which DescribeReplicationGroups(ReplicationGroupId=<id>) is refused with HTTP 404, Error.Code ReplicationGroupNotFoundFault (the check compares error codes on their base, i.e. 'ReplicationGroupNotFound') and the message "ReplicationGroup <ReplicationGroupId> not found." The fixed text of that message is "ReplicationGroup " + " not found."; the only variable part is the ReplicationGroupId taken verbatim from the request. Deletion is terminal: 'deleting' is a transient status the group leaves by ceasing to exist, never a resting state.

Emulator: DeleteReplicationGroup answers 200 and reports Status 'deleting' (that part matches), but the group is never actually deleted. Polling the by-id DescribeReplicationGroups every 2 s for the check's whole bounded wait never yields the refusal: the call keeps succeeding with HTTP 200 and a complete ReplicationGroup projection whose Status is 'deleting', whose three NodeGroups each carry Status 'deleting', and which still lists all three member clusters (<rg-id>-0001-001, -0002-001, -0003-001) plus a ConfigurationEndpoint. The check then fails the assertion that the call must be refused: "the call was expected to be rejected but SUCCEEDED, returning {'ReplicationGroups': [{... 'Status': 'deleting' ...}]}". So instead of 404 ReplicationGroupNotFoundFault / "ReplicationGroup <id> not found.", a client gets 200 and a live-looking group.

Why: a client that deletes a replication group has no way to learn the delete finished. Every SDK waiter and every teardown loop written against AWS polls DescribeReplicationGroups until it raises ReplicationGroupNotFound; against this emulator that poll never terminates, so cleanup code hangs or times out, and the group and its member cache clusters stay visible (and, on AWS, billable) indefinitely. A client also cannot distinguish 'still deleting' from 'stuck', and a subsequent CreateReplicationGroup reusing the same id will be refused as a duplicate forever.

Fix: the DeleteReplicationGroup handler and the replication-group lifecycle in the elasticache crate's state — the 'deleting' transition must complete, removing the group and its member cache clusters from state, after which both the by-id DescribeReplicationGroups and the list form must stop resolving it and the by-id form must answer 404 ReplicationGroupNotFoundFault with AWS's message text.

Related open issues, none of which carries this check id: 20261114T131431Z-elasticache-transient-lifecycle-windows-are-not-observable-creating.md (reports the OPPOSITE direction — creating/modifying/deleting settling too fast to observe), 20261111T005655Z-elasticache-data-plane-still-unreachable-and-multi-wait.md Cause 3 (lazy status settling costing a poll interval), 20261121T103935Z-ec2-elasticache-elasticache-deletereplicationgroup-is-accepted-while-the-group-is.md (DeleteReplicationGroup accepted in the creating state) and 20261108T093128Z-elasticache-deletereplicationgroup-stub.md (the operation now answers, so that stub report no longer describes this path).

Traces: elasticache.elasticache-cluster-mode-slot-routing-moved-and-crossslot.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout: 151/194 passed; elasticache 132/161 (29 failed), of which this is the one failure bound to no open issue.

[mismatch] elasticache.elasticache-cluster-mode-slot-routing-moved-and-crossslot, in the check's inline teardown (check_elasticache_elasticache_cluster_mode_slot_routing_moved_and_crossslot.py:1450 -> _error_code_and_message at :507):

  AssertionError: the call was expected to be rejected but SUCCEEDED, returning {'ReplicationGroups': [{'ReplicationGroupId': '<rg-id>', 'Description': 'sim2real cluster-mode slot routing check', 'Status': 'deleting', 'MemberClusters': ['<rg-id>-0001-001', '<rg-id>-0002-001', '<rg-id>-0003-001'], 'NodeGroups': [{'NodeGroupId': '0001', 'Status': 'deleting', 'Slots': '0-5461', ...}, {'NodeGroupId': '0002', 'Status': 'deleting', 'Slots': '5462-10922', ...}, {'NodeGroupId': '0003', 'Status': 'deleting', 'Slots': '10923-16383', ...}], 'AutomaticFailover': 'enabled', 'MultiAZ': 'disabled', 'ConfigurationEndpoint': {...}, 'ClusterEnabled': True, 'CacheNodeType': 'cache.t3.micro', 'ARN': 'arn:aws:elasticache:<region>:<account>:replicationgroup:<rg-id>', ...}]}

The check's own expectation, verbatim from its source:
  status = _teardown_rg_now(ec, rg_id)                       # asserts Status == 'deleting'  -> PASSED
  gone = _poll(lambda: _describe_rg(ec, rg_id), lambda g: g is None, timeout_s=..., interval_s=2)
  got = _error_code_and_message(lambda: ec.describe_replication_groups(ReplicationGroupId=rg_id))
  assert got == ("ReplicationGroupNotFound", f"ReplicationGroup {rg_id} not found."), (
      "a deleted replication group must be reported as ReplicationGroupNotFound with the exact "
      "message 'ReplicationGroup <id> not found.'")

Expected: HTTP 404, Error.Code ReplicationGroupNotFoundFault (base ReplicationGroupNotFound), Error.Message "ReplicationGroup <rg-id> not found."
Actual: HTTP 200 with the ReplicationGroup projection above, Status 'deleting'.

Note on the graded signal: the check's _poll helper returns None on timeout, so the preceding `assert gone is None` passed vacuously — the graded observation is the one above, that the by-id DescribeReplicationGroups answered 200 with Status 'deleting' after DeleteReplicationGroup had been accepted and the bounded wait had elapsed. The group id, ARN account/region, endpoint address/port and timestamps in the quoted response are per-run values and are not part of the requirement; the requirement is the 404/ReplicationGroupNotFoundFault refusal and its message template.

Model evidence: DescribeReplicationGroups errors = [InvalidParameterCombinationException 400, InvalidParameterValueException 400, ReplicationGroupNotFoundFault 404 (awsQueryError ReplicationGroupNotFoundFault)]; DeleteReplicationGroup errors include ReplicationGroupNotFoundFault 404 and InvalidReplicationGroupStateFault 400.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateCacheSubnetGroup over subnets of a VPC, then CreateReplicationGroup(ReplicationGroupId=<id>, Engine='redis', CacheNodeType='cache.t3.micro', NumNodeGroups=3, ReplicasPerNodeGroup=0, AutomaticFailoverEnabled=true) and poll DescribeReplicationGroups until Status='available'.
3. Call DeleteReplicationGroup(ReplicationGroupId=<id>, RetainPrimaryCluster=false). Observe 200 with ReplicationGroup.Status='deleting' (correct).
4. Poll DescribeReplicationGroups(ReplicationGroupId=<id>) every 2 s. Expected: within the deletion window the call starts failing with HTTP 404, Error.Code ReplicationGroupNotFoundFault and Error.Message "ReplicationGroup <id> not found.", and the member clusters <id>-0001-001/-0002-001/-0003-001 stop resolving in DescribeCacheClusters. Observed: the call keeps answering 200 with Status='deleting' (and every NodeGroup Status='deleting') for the whole bounded wait; the group is never removed.
5. Re-replay elasticache.elasticache-cluster-mode-slot-routing-moved-and-crossslot; its teardown must end with the ReplicationGroupNotFound refusal and that exact message.
