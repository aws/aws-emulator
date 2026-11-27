Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune.cluster-port-survives-modify-then-blocks-duplicate-and-busy-delete neptune.create-cluster-port-outside-valid-range-rejected neptune.instance-endpoint-port-inherits-cluster-custom-port neptune.modify-cluster-port-outside-valid-range-rejected
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune.cluster-port-survives-modify-then-blocks-duplicate-and-busy-delete|run#10|AssertionError|assert created["Port"] == _PORT, f"cluster Port was {created['Port']!r}, expected {_PORT}"
Issue-Key: neptune.create-cluster-port-outside-valid-range-rejected|run#15|AssertionError|assert created["Port"] == _MIN_PORT, created
Issue-Key: neptune.instance-endpoint-port-inherits-cluster-custom-port|run#10|AssertionError|assert created_cluster["Port"] == _CUSTOM_PORT, created_cluster
Issue-Key: neptune.modify-cluster-port-outside-valid-range-rejected|run#9|AssertionError|assert created["Port"] == _STARTING_PORT, created
Issue-Commit: 2db67cdf583f83462fa29a2772c7057ac4b27efa

# neptune: CreateDBCluster discards the request's Port and publishes the emulator's ephemeral listener port, so no cluster-port scenario can proceed

> _Note: this issue was identified against the code base at commit `2db67cdf583f83462fa29a2772c7057ac4b27efa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
THIS ISSUE HAS 1 CAUSE — it must be closed before submitting.

Cause 1 of 1: CreateDBCluster does not report the Port the request asked for. The DBCluster it projects (and every later read of that cluster) carries a port that is neither the requested value nor the engine default, but a value that changes from cluster to cluster and from boot to boot — the emulator's own ephemeral data-plane listener port.

AWS: Smithy (services/service-com-amazonaws-neptune-amazonrdsv19/smithy-model/com-amazonaws-neptune-amazonrdsv19.json) CreateDBCluster -> CreateDBClusterResult.DBCluster (com.amazonaws.neptune#DBCluster). CreateDBClusterMessage.Port (IntegerOptional) doc: "The port number on which the instances in the DB cluster accept connections. Default: 8182"; DBCluster.Port doc: "Specifies the port that the database engine is listening on". So Port is a value the CALLER chooses at create time within 1150-65535: a CreateDBCluster(Engine=neptune, Port=P) returns DBCluster.Port == P, DescribeDBClusters reports the same P for the life of the cluster, and 8182 appears only when the request named no Port. The value is a stored property of the cluster, never a property of the process serving it, so it is identical on every read and across restarts.

Emulator: for a request that explicitly names Port, the created cluster reports an unrelated high port: Port=8191 -> reported 34723; Port=1150 -> reported 44377; Port=8183 -> reported 41587; Port=8182 -> reported 36751. Each value differs per cluster and per boot, which identifies it as the emulator's own served-endpoint port rather than the stored one. The cluster record DOES keep the requested port (services/service-com-amazonaws-rds-amazonrdsv19/src/state/clusters.rs, DbClusterRecord::port, "`Port`: the port the instances in the cluster accept connections on"), but the projection overrides it with the address the data plane is answered at (services/service-com-amazonaws-rds-amazonrdsv19/src/state/api_plane_address.rs::api_plane_endpoint -> DbClusterRecord::served_endpoint -> DbClusterRecord::published_port), because engine_data_plane_is_an_api() treats every neptune-engine cluster that way. The stored Port is therefore write-only: nothing a client can read ever shows it.

Why: DBCluster.Port is the documented way a client learns where the graph engine accepts connections. A client that moves a Neptune cluster off 8182 to satisfy a security-group, proxy or firewall rule is told a port it never asked for, so the security-group rule, the connection string and the DNS/alias record it builds all name the wrong port; the value it reads back disagrees with the value it just sent, so no reconcile loop ever converges, and because the value also changes between boots, a cached connection string is wrong after every restart. Downstream, the whole port surface is unreachable behind this: the instance-side inheritance assertion, the ModifyDBCluster out-of-range refusal wording, the duplicate-create and cluster-not-empty refusals and the delete/describe-gone refusals of these checks are all never reached, so they cannot be confirmed fixed either.

Note on related issues: this is a REGRESSION of the fixed issue 20261117T150412Z-neptune-3-net-new-causes-createdbcluster-ignores-the.md, whose cause 1 was exactly "CreateDBCluster ignores the request's Port"; the open issue 20261119T170405Z-neptune-2-net-new-port-causes-a-db.md explicitly records that the cluster-side Port had started behaving ("CreateDBCluster/DescribeDBClusters report exactly the requested Port (8191, 8183, 8182 all asserted and passed)") and is now blocked at a strictly earlier assertion. The open issue 20261116T010226Z-neptune-dbcluster-projection-publishes-the-emulator-s-listener.md covers the WIRE-corpus case where a create that names NO Port must report the engine default 8182, and its closing note states the listener port is published deliberately because no listener API lets a service bind a model-fixed port; neither issue carries the replay keys of the four behavioral checks below (the suite marked none of them OWNED), and the explicit-Port echo asserted here is a distinct requirement from that default-port case. Whichever way the addressing conflict is resolved, a client-supplied Port must survive to the DBCluster projection.

Fix: the DBCluster projection shared by the CreateDBCluster/DescribeDBClusters/ModifyDBCluster handlers and the served-endpoint port it reads — DBCluster.Port must come from the cluster record's stored port (the value the create request carried, defaulting to the Neptune engine default when it carried none), not from the port the serving process happens to be bound to. If the data plane must still be dialable, publish the emulator's address through the endpoint host only and leave Port the cluster's own.

Traces: neptune.cluster-port-survives-modify-then-blocks-duplicate-and-busy-delete; neptune.create-cluster-port-outside-valid-range-rejected; neptune.instance-endpoint-port-inherits-cluster-custom-port; neptune.modify-cluster-port-outside-valid-range-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 8/19 checks passed (42.1%); per-service neptune 3/7 (4 failed). All other failures in the report are marked OWNED by an open issue and were skipped; the four neptune failures below are marked neither ALREADY FILED nor OWNED, and all four stop at the SAME assertion — the created cluster's Port.

1. neptune.cluster-port-survives-modify-then-blocks-duplicate-and-busy-delete [mismatch]: CreateDBCluster(DBClusterIdentifier=<id>, Engine=neptune, Port=8191, DBSubnetGroupName=<sng>, DeletionProtection=false), then wait for Status 'available'. At check line 149: `assert created["Port"] == _PORT` -> AssertionError: "cluster Port was 34723, expected 8191".

2. neptune.create-cluster-port-outside-valid-range-rejected [mismatch]: the out-of-range half PASSES first — CreateDBCluster(Port=1149) is refused with Error.Code 'InvalidParameterValue' and message 'Invalid endpoint port 1149. Valid range is: 1150-65535 ', and DescribeDBClusters on the refused identifier answers DBClusterNotFoundFault 'DBCluster <id> not found.' Then the legal neighbour CreateDBCluster(Port=1150) succeeds but at check line 101 `assert created["Port"] == _MIN_PORT` -> AssertionError; the returned DBCluster carries 'Port': 44377 (full record in the report, DBClusterIdentifier 'sim2real-neptune-portmin-...', Status 'creating', Engine 'neptune', EngineVersion '1.4.8.0').

3. neptune.instance-endpoint-port-inherits-cluster-custom-port [mismatch]: CreateDBCluster(Engine=neptune, Port=8183, DBSubnetGroupName=<sng>) -> at check line 51 `assert created_cluster["Port"] == _CUSTOM_PORT` -> AssertionError; returned DBCluster carries 'Port': 41587.

4. neptune.modify-cluster-port-outside-valid-range-rejected [mismatch]: CreateDBCluster(Engine=neptune, Port=8182, DBSubnetGroupName=<sng>) -> at check line 83 `assert created["Port"] == _STARTING_PORT` -> AssertionError; returned DBCluster carries 'Port': 36751.

The four reported ports (34723, 44377, 41587, 36751) are all distinct, all unrelated to the four requested ports (8191, 1150, 8183, 8182) and all in the ephemeral range, which is what identifies the value as the serving process's bound port rather than the stored one. Those literal numbers are one run's values, not requirements: the requirement is DBCluster.Port == the Port the create request carried (and 8182 when it carried none), stable across reads and restarts.

Model evidence (smithy_query on services/service-com-amazonaws-neptune-amazonrdsv19/smithy-model/com-amazonaws-neptune-amazonrdsv19.json): CreateDBCluster input member Port -> IntegerOptional, doc 'The port number on which the instances in the DB cluster accept connections. Default: 8182'; output CreateDBClusterResult.DBCluster -> com.amazonaws.neptune#DBCluster; DBCluster.Port doc 'Specifies the port that the database engine is listening on'. Code evidence: services/service-com-amazonaws-rds-amazonrdsv19/src/state/clusters.rs line 113-114 (DbClusterRecord.port, the stored requested port) and lines 65-81 (served_endpoint, whose published_port is what the projection reports); services/service-com-amazonaws-rds-amazonrdsv19/src/state/api_plane_address.rs lines 20-27 and 43-45 (every neptune-engine cluster publishes the emulator's own host AND port).

Not claimed as requirements: the generated cluster/subnet-group identifiers, DbClusterResourceId, ClusterCreateTime, the VpcSecurityGroupId, the Endpoint/ReaderEndpoint hostnames, request ids, and the specific ephemeral port numbers observed. Only the stored-Port echo relationship is claimed here.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from this checkout, and replay the simparity-behavioral-aws-v1 behavioral suite.
2. Replay the four checks: neptune.cluster-port-survives-modify-then-blocks-duplicate-and-busy-delete, neptune.create-cluster-port-outside-valid-range-rejected, neptune.instance-endpoint-port-inherits-cluster-custom-port, neptune.modify-cluster-port-outside-valid-range-rejected.
3. Or directly: CreateDBSubnetGroup over the region's subnets; then CreateDBCluster(DBClusterIdentifier=<id>, Engine=neptune, Port=8183, DBSubnetGroupName=<sng>). The response's CreateDBClusterResult.DBCluster.Port must be 8183, not an ephemeral port of the serving process.
4. Wait for Status 'available' and call DescribeDBClusters(DBClusterIdentifier=<id>): DBClusters[0].Port must still be 8183.
5. Repeat with Port omitted: DBCluster.Port must be the Neptune engine default 8182.
6. Restart the emulator and DescribeDBClusters the same cluster again: Port must be unchanged (it is a stored property of the cluster, not of the process).
7. Confirm the surface this unblocks, which these checks assert next and currently never reach: a member instance created with no Port of its own reports Endpoint.Port == the cluster's Port; an unrelated ModifyDBCluster/ModifyDBInstance (PreferredMaintenanceWindow) leaves both unchanged; ModifyDBCluster(Port=1149) is refused 400 InvalidParameterValue 'Invalid endpoint port 1149. Valid range is: 1150-65535 ' (trailing space; 1149 from the request, 1150 the floor) and leaves the stored Port alone, while ModifyDBCluster(Port=1150) is applied.
8. Ignore the generated identifiers, resource ids, timestamps, endpoint hostnames and the literal ephemeral port numbers in the recorded records.
