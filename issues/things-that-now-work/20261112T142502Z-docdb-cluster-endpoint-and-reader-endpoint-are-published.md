Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-cluster-and-instance-endpoint-discovery
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-cluster-and-instance-endpoint-discovery docdb-cluster-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.Endpoint
Issue-Key: docdb|CreateDBCluster|2xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.ReaderEndpoint
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.DBClusterMembers.DBClusterMember.PromotionTier
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.Endpoint
Issue-Key: docdb|DescribeDBClusters|2xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.ReaderEndpoint
Issue-Commit: 4f8baca82116a343bf4a764bcd60c9c8f261ed5e

# DocDB cluster endpoint and reader endpoint are published as docdb.amazonaws.com hosts the emulator does not serve

> _Note: this issue was identified against the code base at commit `4f8baca82116a343bf4a764bcd60c9c8f261ed5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Three net-new causes, all the same fault: every DocDB response that projects a cluster's connection endpoints fills them with a templated name in AWS's public DNS namespace instead of an address the emulator itself answers on, so the replay harness refuses the published value (verdict host_not_own) wherever it appears. One cause additionally drops a modeled DBClusterMember member.

Cause 1 of 3: CreateDBCluster response — DBCluster.Endpoint and DBCluster.ReaderEndpoint (fixture docdb-cluster-and-instance-endpoint-discovery, step task#7; same cluster also covers docdb-cluster-lifecycle#task#7).
AWS: Smithy com.amazonaws.docdb#AmazonRDSv19 models DBCluster.Endpoint as the connection endpoint for the primary instance of the cluster and DBCluster.ReaderEndpoint as the reader endpoint, and the Endpoint structure (Address/Port/HostedZoneId) carries the service-level documentation "Network information for accessing a cluster or instance. Client programs must specify a valid endpoint to access these Amazon DocumentDB resources." The requirement is therefore not the SHAPE of the host string — the real-AWS form <cluster-id>.cluster-<resource-id>.<region>.docdb.amazonaws.com and the resource/account ids inside it are per-environment values and are explicitly not asserted by the fixture ('<shaped-not-asserted>') — but that the host:port handed to the caller is the address that actually serves the cluster.
Emulator: it returns a host it does not serve, of the form <cluster-identifier>.cluster-<account-id>.<region>.docdb.amazonaws.com (reader: ...cluster-ro-<account-id>...), which the harness rejects as host_not_own: the address announced is not one of the emulator's own hosts.
Why: a client that does what the model documents — read DBCluster.Endpoint/ReaderEndpoint and open a MongoDB connection — gets a name that either does not resolve or resolves to real AWS, never to the emulated cluster. No DocumentDB data-plane work is possible and the failure is indistinguishable from a client-side network fault.
Fix: the docdb CreateDBCluster handler / cluster state record that synthesizes the endpoint addresses, and the DBCluster serializer that projects Endpoint and ReaderEndpoint, must publish the emulator's own serving host and the port its DocumentDB listener is bound to.
Traces: docdb-cluster-and-instance-endpoint-discovery#task#7, docdb-cluster-lifecycle#task#7.

Cause 2 of 3: DescribeDBClusters response — same two endpoint members, plus DBClusterMembers.DBClusterMember.PromotionTier missing (fixture docdb-cluster-and-instance-endpoint-discovery, step task#10).
AWS: as in cause 1 for the endpoints. For the member, Smithy models DBClusterMember.PromotionTier (IntegerOptional) as "A value that specifies the order in which an Amazon DocumentDB replica is promoted to the primary instance after a failure of the existing primary instance"; the recording shows it present on the cluster member of a created instance (the value is whatever promotion tier the instance was created with, defaulting to 1).
Emulator: publishes the unserved *.docdb.amazonaws.com cluster and reader hosts, and omits PromotionTier from the DBClusterMember element entirely (got None).
Why: besides the unreachable endpoint, a client reading the cluster's membership cannot determine failover ordering — the member element silently lacks a modeled field, so failover-priority logic reads it as absent rather than as the instance's tier.
Fix: the DBCluster serializer's endpoint projection (as in cause 1) and the DBClusterMember projection in the docdb cluster state/serializer, which must carry the member instance's promotion tier.
Traces: docdb-cluster-and-instance-endpoint-discovery#task#10.

Cause 3 of 3: DescribeDBClusters response in the cluster lifecycle flow — DBCluster.Endpoint and DBCluster.ReaderEndpoint (fixture docdb-cluster-lifecycle, step task#8).
AWS: as in cause 1.
Emulator: same unserved templated hosts, confirming the defect is in the shared endpoint construction rather than in one operation's handler.
Why: as in cause 1 — the endpoint read back after the cluster becomes available is still not an address the emulator answers on.
Fix: same component as cause 1 (shared cluster endpoint construction / DBCluster serializer).
Traces: docdb-cluster-lifecycle#task#8.

Related open issue, which does NOT cover these steps: '20261109T120625Z-docdb-cluster-instance-endpoints-are-published-as-real' (DocDB cluster/instance endpoints are published as real *.docdb.amazonaws.com DNS names the emulator does not serve) describes the same root cause from the BEHAVIORAL corpus and carries only behavioral check keys; these wire fixture steps are not in its replay pointer, so they are filed here.

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 89.2%, 13 mismatched steps of 120).

Cause 1 — docdb-cluster-and-instance-endpoint-discovery#task#7, CreateDBCluster, expected 200 / actual 200:
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.Endpoint: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-docdbep-8676742b.cluster-000000000001.us-west-2.docdb.amazonaws.com')
  $.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.ReaderEndpoint: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-docdbep-8676742b.cluster-ro-000000000001.us-west-2.docdb.amazonaws.com')
  (affected: docdb-cluster-and-instance-endpoint-discovery#task#7, docdb-cluster-lifecycle#task#7)

Cause 2 — docdb-cluster-and-instance-endpoint-discovery#task#10, DescribeDBClusters, expected 200 / actual 200:
  $...DBCluster.DBClusterMembers.DBClusterMember.PromotionTier: missing (expected '1', got None)
  $...DBCluster.Endpoint: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-docdbep-8676742b.cluster-000000000001.us-west-2.docdb.amazonaws.com')
  $...DBCluster.ReaderEndpoint: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-docdbep-8676742b.cluster-ro-000000000001.us-west-2.docdb.amazonaws.com')

Cause 3 — docdb-cluster-lifecycle#task#8, DescribeDBClusters, expected 200 / actual 200:
  $...DBCluster.Endpoint: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-docdb-cluster-9c38e6b1.cluster-000000000001.us-west-2.docdb.amazonaws.com')
  $...DBCluster.ReaderEndpoint: host_not_own (expected '<shaped-not-asserted>', got 'sim2real-probe-docdb-cluster-9c38e6b1.cluster-ro-000000000001.us-west-2.docdb.amazonaws.com')

Expected (AWS): Endpoint/ReaderEndpoint are addresses at which the cluster's primary and reader actually answer; the fixture asserts only that the published host is one the server itself serves, not its literal text.
Actual (emulator): both members carry a templated public-AWS DNS name the emulator does not serve. The cluster identifier prefix, the zero-padded account id and the region in the captured strings are per-run/environment values and are not part of the requirement; the requirement is that the announced host is the emulator's own serving host. The missing PromotionTier value '1' likewise stands for "the tier the member was created with", not for the literal 1.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. ec2 CreateVpc + CreateSubnet in two AZs; docdb CreateDBSubnetGroup over those subnets; ec2 CreateSecurityGroup.
3. docdb CreateDBCluster(DBClusterIdentifier=<unique>, Engine='docdb', MasterUsername=<user>, MasterUserPassword=<pw>, DBSubnetGroupName=<group>, VpcSecurityGroupIds=[<sg>]) — fixture docdb-cluster-and-instance-endpoint-discovery step task#7. Read DBCluster.Endpoint and DBCluster.ReaderEndpoint: observed hosts in the *.docdb.amazonaws.com namespace that the emulator does not serve; expected the emulator's own serving host.
4. docdb CreateDBInstance into that cluster, then docdb DescribeDBClusters(DBClusterIdentifier=<cluster>) — step task#10. Observed: the same two unserved endpoint hosts, and DBClusterMembers.DBClusterMember has no PromotionTier element; expected PromotionTier present with the member's promotion tier.
5. Run the cluster lifecycle flow (fixture docdb-cluster-lifecycle) to step task#8 and call DescribeDBClusters again: observed the same unserved Endpoint/ReaderEndpoint hosts.
6. Resolve/connect to either published host from a client: nothing on the emulator is contacted.
