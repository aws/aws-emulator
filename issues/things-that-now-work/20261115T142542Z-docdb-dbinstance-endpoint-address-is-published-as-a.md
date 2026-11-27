Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb.cluster-mongo-insert-find-round-trip
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb.cluster-mongo-insert-find-round-trip|run#23|AssertionError|raise AssertionError(
Issue-Commit: 7b0cd7b14eb321355db06251cc80d3f9fa1c4d53

# DocDB DBInstance.Endpoint.Address is published as a *.docdb.amazonaws.com DNS name the emulator serves nothing at, so an in-VPC connect to the advertised instance endpoint fails name resolution and no MongoDB data plane exists

> _Note: this issue was identified against the code base at commit `7b0cd7b14eb321355db06251cc80d3f9fa1c4d53`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
Cause 1 of 1: DescribeDBInstances advertises the instance's Endpoint.Address as an AWS-public DNS name (<instance-id>.<resource-id>.<region>.docdb.amazonaws.com, port 27017) that is not a host the emulator serves or that resolves inside the emulated environment, and nothing listens for the MongoDB wire protocol behind it. A client that reads the advertised Endpoint and dials it from inside the cluster's own VPC — exactly what the model documents as the only way to reach DocumentDB — gets a resolver failure, so the entire DocumentDB data plane is absent. Failing check: docdb.cluster-mongo-insert-find-round-trip.

Relation to already-filed issues (neither carries this failure's replay key, so this failure is not covered by them): open issue 20261109T120625Z-docdb-cluster-instance-endpoints-are-published-as-real.md describes the same family of defect for the *cluster* endpoint and lists this check, but its recorded replay key for it is `_probe_call#21|PublishedUrlRejected` — the harness refusing the published URL before any connect. That is no longer where the check fails: the probe now runs in-VPC, dials the address itself, and fails at `run#23` on DATAPLANE_UNIMPLEMENTED with a gaierror. Open issue 20261112T142502Z-docdb-cluster-endpoint-and-reader-endpoint-are-published.md likewise covers DBCluster.Endpoint/ReaderEndpoint, not DBInstance.Endpoint.Address as dialed. Fixed issue 20261110T231901Z-docdb-data-plane-still-unreachable-the-in-vpc.md fixed the Lambda sandbox socket layer (its emulator-authored OSError 'a Lambda function in this emulator has no host network to reach' is gone, replaced by a genuine resolver error), so this is NOT a reopen of it: the remaining cause is the published address and the missing listener behind it. Open issue 20261109T235356Z-docdb-data-plane-check-cannot-run-the-emulator.md (missing _ssl in the python3.12 runtime) is also past: the probe imports and the Invoke returns no FunctionError.

AWS: Smithy (com.amazonaws.docdb#AmazonRDSv19) models DBInstance.Endpoint as the structure Endpoint, documented "Network information for accessing a cluster or instance. Client programs must specify a valid endpoint to access these Amazon DocumentDB resources.", with Endpoint.Address "Specifies the DNS address of the instance" and Endpoint.Port "Specifies the port that the database engine is listening on". The contract is therefore not that some string is emitted, but that the emitted host:port is the address the engine answers the MongoDB wire protocol on: a client resolves Address, opens TCP to Port, completes TLS and a SCRAM MongoDB handshake as the cluster master user, then runs insert/find. DocumentDB is VPC-only, so the conforming client is one inside the cluster's VPC — on AWS a VpcConfig'd Lambda dialing the advertised Address connects. The *shape* of the hostname on real AWS is an environment value, not a requirement; what is required is that the published address is reachable and served by the emulated DocumentDB endpoint the caller was handed.

Emulator: CreateDBInstance/DescribeDBInstances report DBInstanceStatus 'available' and an Endpoint whose Address is a templated amazonaws.com host and whose Port is 27017, but that name resolves to nothing the emulator serves. The in-VPC probe's connect fails with `gaierror: [Errno -2] Name or service not known: <advertised host>` and the check raises `DATAPLANE_UNIMPLEMENTED: the instance endpoint <host>:27017 advertised in DescribeDBInstances accepts no TCP connection from inside its own VPC (connect: gaierror: ...); an endpoint that cannot be connected to is not a data plane`. The fixed text here is the harness's; the diverging emulator behavior is that there is no AWS-shaped response at all — no status, no error code — just an unresolvable address in place of a connectable endpoint. The instance identifier, resource id, region and port in the captured line are per-run/environment values, not requirements.

Why: a client that does exactly what the model documents — read DBInstance.Endpoint.Address/Port and connect from inside the VPC — can never reach the cluster, so every DocumentDB application workflow (authenticate, insert, find, aggregate, watch a change stream, run a transaction, build an index) is unobservable on this emulator. Worse, the failure surfaces as a DNS error pointing at AWS's public namespace, so a user debugging it is sent to VPC DNS, route tables and security groups that are in fact correct, and cannot distinguish an unimplemented data plane from a network misconfiguration of their own.

Fix: the docdb instance/cluster endpoint state and the DBInstance serializer that projects Endpoint.Address/Endpoint.Port (paired with the DBCluster Endpoint/ReaderEndpoint projection) must publish the emulator's own serving host and the port its DocumentDB listener is bound to instead of a templated *.docdb.amazonaws.com name, and a MongoDB wire listener must be stood up behind that address so an in-VPC connect reaches it.

Traces: docdb.cluster-mongo-insert-find-round-trip (fails at the DATAPLANE_UNIMPLEMENTED assertion after the in-VPC probe's connect returns 'connect: gaierror: Name or service not known').

## Evidence
Expected (AWS): DescribeDBInstances returns Endpoint.Address/Endpoint.Port that the DocumentDB engine answers on; an in-VPC client resolves the address, connects on the advertised port, completes TLS and SCRAM-SHA-1 auth as the master user, inserts one document (ok=1, n=1) and finds exactly that document.

Actual (emulator): the instance reaches DBInstanceStatus 'available' and advertises an Endpoint in AWS's public DNS namespace that the emulator serves nothing at. The in-VPC Lambda probe's connect fails name resolution and the check raises:

  File "check_docdb_cluster_mongo_insert_find_round_trip.py", line 218, in run
    raise AssertionError(
  AssertionError: DATAPLANE_UNIMPLEMENTED: the instance endpoint sim2real-behav-docdb-in-fdd783695733.g94c0clz3dei.us-west-2.docdb.amazonaws.com:27017 advertised in DescribeDBInstances accepts no TCP connection from inside its own VPC (connect: gaierror: [Errno -2] Name or service not known: sim2real-behav-docdb-in-fdd783695733.g94c0clz3dei.us-west-2.docdb.amazonaws.com); an endpoint that cannot be connected to is not a data plane

The host name, resource id, region and the 27017 port in that line are per-run/environment values; the requirement is that the published Address is an address the emulator itself serves and that a MongoDB session can be established on it. Note this is a different failure point from the one recorded against open issue 20261109T120625Z (`_probe_call#21|PublishedUrlRejected`, the harness rejecting the published URL before any socket was opened) — the connect is now attempted and fails in the resolver.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. ec2 CreateVpc + two CreateSubnet calls in two AZs; CreateSecurityGroup for the probe and one for the cluster admitting TCP 27017 from the probe's group.
3. lambda CreateFunction(Runtime='python3.12', Handler='index.handler', Code={'ZipFile': <handler doing socket.create_connection((event['host'], event['port']), timeout=10) then a TLS MongoDB session>}, VpcConfig={SubnetIds: <the two subnets>, SecurityGroupIds: [<probe sg>]}); wait for State Active.
4. docdb CreateDBSubnetGroup over the subnets; CreateDBCluster(DBClusterIdentifier=<unique>, Engine='docdb', MasterUsername='dbadmin', MasterUserPassword=<pw>, DBSubnetGroupName=<group>, VpcSecurityGroupIds=[<cluster sg>]); CreateDBInstance(DBInstanceClass='db.t3.medium', Engine='docdb', DBClusterIdentifier=<cluster>).
5. Poll docdb DescribeDBInstances(DBInstanceIdentifier=<instance>) until DBInstanceStatus='available' and read Endpoint.Address/Endpoint.Port. Observed: a '<instance>.<resource-id>.<region>.docdb.amazonaws.com' host on port 27017 — a name in AWS's public DNS namespace that the emulator does not serve. Expected: the emulator's own serving host and the port its DocumentDB listener is bound to.
6. lambda Invoke the probe with that host/port. Observed: connect fails with gaierror [Errno -2] Name or service not known. Expected: the socket connects, TLS and SCRAM auth succeed, and insert/find round-trips the document.
7. Or replay docdb.cluster-mongo-insert-find-round-trip.
