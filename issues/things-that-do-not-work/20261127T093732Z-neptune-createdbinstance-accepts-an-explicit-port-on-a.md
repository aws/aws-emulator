Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptune.modify-cluster-port-propagates-to-member-and-new-member-port-is-cluster-scoped
Issue-Shape-Id: com.amazonaws.neptune#AmazonRDSv19
Issue-Key: neptune.modify-cluster-port-propagates-to-member-and-new-member-port-is-cluster-scoped|run#29|AssertionError|raise AssertionError(
Issue-Commit: ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7

# Neptune CreateDBInstance accepts an explicit Port on a DB cluster member instead of refusing it with InvalidParameterCombination

> _Note: this issue was identified against the code base at commit `ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Cause 1 of 1: Neptune CreateDBInstance does not enforce that the port of a cluster member is owned by the DB cluster, so a request that supplies both DBClusterIdentifier and its own Port is created instead of refused.

AWS: In Neptune (as in Aurora/RDS cluster engines) the endpoint port is a property of the DB cluster, not of its instances. A CreateDBInstance request that names a DBClusterIdentifier and also supplies Port is refused with HTTP 400 and the awsQuery error code `InvalidParameterCombination`, message `The requested DB Instance will be a member of a DB Cluster. Set database endpoint port number for the DB Cluster.` Smithy (service-com-amazonaws-neptune-amazonrdsv19): CreateDBInstance input carries both `DBClusterIdentifier` (required) and `Port` (IntegerOptional); the model does not bind `InvalidParameterCombination` to this operation, so this is the service-common awsQuery client error at HTTP 400 — recorded behavior only for the pairing of code and message. The message above is entirely fixed text: it has no variable parts taken from the request or from state (no identifier, no port value is interpolated). The refusal is also atomic — no instance is left behind, so a subsequent DescribeDBInstances on that identifier answers `DBInstanceNotFound`.

Emulator: the call succeeds with HTTP 200 and returns a DBInstance for the member (DBInstanceStatus `creating`, DBInstanceClass db.t3.medium, Engine neptune), silently ignoring the supplied Port and projecting the cluster's current port into `Endpoint.Port` instead. No error is raised at all, so the check's refusal branch is never reached and the member is created. Note the earlier legs of the same check pass: CreateDBCluster(Port) is honoured, ModifyDBCluster(Port, ApplyImmediately=true) is reflected in DescribeDBClusters and propagates to the existing member's Endpoint.Port — only the negative class is missing.

Why: a client that relies on AWS rejecting instance-level Port on a cluster member gets the opposite outcome. Code that probes this combination to decide where to configure the port sees it accepted and concludes instance-level ports are supported; worse, an operator who mistakenly passes Port=8190 to a member gets a silently different port (the cluster's) with no diagnostic, and any provisioning flow that counts on the create failing atomically now has an extra cluster member to clean up. Negative-path test suites and CloudFormation/CDK validation layers that mirror AWS's parameter-combination rules diverge from the emulator.

Fix: the Neptune CreateDBInstance handler's input validation — reject the request before any state is mutated when `DBClusterIdentifier` is present and `Port` is set, emitting the awsQuery error code `InvalidParameterCombination` at HTTP 400 with the fixed message above, and leaving no DBInstance in state.

Traces: neptune.modify-cluster-port-propagates-to-member-and-new-member-port-is-cluster-scoped (the assertion `expected CreateDBInstance with its own Port on a cluster member to be refused, but it succeeded`).

## Evidence
Check: neptune.modify-cluster-port-propagates-to-member-and-new-member-port-is-cluster-scoped [mismatch]

Request (after CreateDBCluster(Port=8183), one portless member, ModifyDBCluster(Port=8184, ApplyImmediately=True) all of which behaved correctly):
  CreateDBInstance(
    DBInstanceIdentifier=<member-two-id>,
    DBClusterIdentifier=<cluster-id>,
    DBInstanceClass='db.t3.medium',
    Engine='neptune',
    PubliclyAccessible=False,
    Port=8190,
    Tags=[...])

Expected (AWS): HTTP 400, Error.Code = 'InvalidParameterCombination', Error.Message = 'The requested DB Instance will be a member of a DB Cluster. Set database endpoint port number for the DB Cluster.' (message is fixed text, no interpolated values); the instance is NOT created, so DescribeDBInstances(DBInstanceIdentifier=<member-two-id>) then answers Error.Code = 'DBInstanceNotFound'.

Actual (emulator): HTTP 200 with a DBInstance body, e.g. {'DBInstance': {'DBInstanceIdentifier': '<member-two-id>', 'DBInstanceClass': 'db.t3.medium', 'Engine': 'neptune', 'DBInstanceStatus': 'creating', 'Endpoint': {'Address': ..., 'Port': 8184, 'HostedZoneId': ...}, 'DBParameterGroups': [{'DBParameterGroupName': 'default.neptune1.4', ...}], 'DBSubnetGroup': {...}, ...}} — no error, and the requested Port=8190 is dropped in favour of the cluster's port (8184 here) rather than being rejected. (Identifiers, addresses, timestamps and the concrete port numbers above are this run's values; the requirement is the refusal, its code and its fixed message.)

Assertion raised: AssertionError: expected CreateDBInstance with its own Port on a cluster member to be refused, but it succeeded: {...}

## Steps to Reproduce
1. Boot the emulator and point a botocore `neptune` client at it.
2. CreateDBSubnetGroup with subnets in the default VPC.
3. CreateDBCluster(DBClusterIdentifier=<cluster>, Engine='neptune', Port=8183, DBSubnetGroupName=<sng>); wait for status 'available'.
4. CreateDBInstance(DBInstanceIdentifier=<member1>, DBClusterIdentifier=<cluster>, DBInstanceClass='db.t3.medium', Engine='neptune', PubliclyAccessible=False) — no Port; wait for 'available'.
5. ModifyDBCluster(DBClusterIdentifier=<cluster>, Port=8184, ApplyImmediately=True); wait for 'available' (this leg behaves correctly today).
6. CreateDBInstance(DBInstanceIdentifier=<member2>, DBClusterIdentifier=<cluster>, DBInstanceClass='db.t3.medium', Engine='neptune', PubliclyAccessible=False, Port=8190).
   Expected: ClientError with Code 'InvalidParameterCombination' and the message 'The requested DB Instance will be a member of a DB Cluster. Set database endpoint port number for the DB Cluster.'
   Observed: HTTP 200 and a created DBInstance whose Endpoint.Port is the cluster's port.
7. DescribeDBInstances(DBInstanceIdentifier=<member2>). Expected: ClientError 'DBInstanceNotFound'. Observed: the instance exists.
