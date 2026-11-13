Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.read-replica-replication-query-and-promote rds.replica-uncommitted-primary-write-absent-until-commit rds.read-replica-promote-and-switchover rds.replica-session-read-only-override-rejected rds.e2e-read-replica-promotion-independent-writes rds.read-replica-promote-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.read-replica-replication-query-and-promote|run#24|AssertionError|assert rep_addr != src_addr, (
Issue-Key: rds.replica-uncommitted-primary-write-absent-until-commit|run#14|AssertionError|assert rep_addr != src_addr, (
Issue-Key: rds.read-replica-promote-and-switchover|run#13|KeyError|repl_status = next(s for s in replica["StatusInfos"] if s["StatusType"] == "read replication")
Issue-Key: rds.replica-session-read-only-override-rejected|run#14|AssertionError|assert in_recovery is True, (
Issue-Key: rds.e2e-read-replica-promotion-independent-writes|_pg_exec_retry#24|Timeout|time.sleep(2)
Issue-Key: rds.read-replica-promote-lifecycle|_wait_until#12|Timeout|last = probe()
Issue-Commit: 14b50dd5b1a37831270f0fabc7b7cd82f4b7ecdf

# rds: 4 net-new causes in read-replica fidelity — a replica advertises the source's endpoint, carries no StatusInfos, is not a standby, and two replica flows never complete

> _Note: this issue was identified against the code base at commit `14b50dd5b1a37831270f0fabc7b7cd82f4b7ecdf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstanceReadReplica

## Description
FOUR net-new root causes, all in the RDS read-replica path. Each must be closed before this issue is submitted.

Context: the open issue 20261104T053605Z-rds-createdbinstancereadreplica-stub.md names CreateDBInstanceReadReplica as a 501 scaffold stub and lists some of the check ids below in its trace list; it does NOT cover these causes. The operation now answers 2xx and creates a replica, so these are defects in what it creates and reports, not the stub. Likewise 20261104T053605Z-rds-the-published-db-instance-cluster-endpoint-serves.md (endpoint serves no engine protocol) lists some of these ids, but a session now opens against the endpoint (Cause 3 observes a SQL answer), so these failures are past that cause and are not covered by it.

=== Cause 1 of 4: a read replica advertises the SAME endpoint address as its source instance ===
AWS: CreateDBInstanceReadReplica returns a DBInstance whose Endpoint is the replica's OWN connection endpoint. Smithy: CreateDBInstanceReadReplica output DBInstance$Endpoint -> Endpoint{Address,Port,HostedZoneId}; Endpoint$Address is documented as "the DNS address of the DB instance" and Endpoint is "the information you need to connect to an Amazon RDS DB instance". Because the replica is a distinct DB instance with a distinct DBInstanceIdentifier, its Endpoint.Address is necessarily distinct from the source instance's Endpoint.Address — that is what lets a client point readers at the replica and writers at the primary.
Emulator: DescribeDBInstances/CreateDBInstanceReadReplica report, for the replica, an Endpoint.Address byte-identical to the source instance's. The checks assert rep_addr != src_addr and fail: "the replica must serve its own endpoint so readers can be pointed at it" and "the replica must serve its own endpoint so a reader can be pointed at it; it advertises '127.0.0.1', the same host as the source" (the literal host value is this environment's and is not the requirement — the requirement is that the two addresses differ and each resolves to its own instance).
Why: a client cannot separate reads from writes. Every reader connection opened from the replica's published endpoint lands on the primary, so replication lag, read-only enforcement and promotion are all unobservable, and a reader workload silently loads the writer.
Fix: the CreateDBInstanceReadReplica handler and the DB-instance endpoint derivation in the rds service crate's state — mint and store a per-instance endpoint for the replica instead of copying/sharing the source instance's.
Traces: rds.read-replica-replication-query-and-promote, rds.replica-uncommitted-primary-write-absent-until-commit.

=== Cause 2 of 4: DescribeDBInstances omits StatusInfos on a read replica ===
AWS: a DB instance that is a read replica reports DBInstance$StatusInfos, a list of DBInstanceStatusInfo ("Provides a list of status information for a DB instance"), containing an entry whose StatusType is "read replication" with Normal/Status describing replication health. Smithy: DBInstance$StatusInfos -> DBInstanceStatusInfoList of DBInstanceStatusInfo{StatusType, Normal, Status, Message}.
Emulator: the DescribeDBInstances response for the replica has no StatusInfos member at all — the check raises KeyError: 'StatusInfos' when selecting the "read replication" entry.
Why: a client cannot tell whether replication is healthy, degraded or stopped, and any code that reads StatusInfos (the documented way to monitor a replica) crashes on a missing key rather than seeing a status.
Fix: the DBInstance serializer/state for read replicas in the rds service crate — populate StatusInfos with a "read replication" DBInstanceStatusInfo for every instance that has a ReadReplicaSourceDBInstanceIdentifier.
Traces: rds.read-replica-promote-and-switchover.

=== Cause 3 of 4: the replica's engine does not run as a replicating standby ===
AWS: a PostgreSQL read replica is a hot standby. A session opened on the replica answers SELECT pg_is_in_recovery() with true, and write statements are refused with SQLSTATE 25006 (read_only_sql_transaction), which is why a session-level override (SET transaction_read_only = off / SET default_transaction_read_only = off) cannot make the replica writable. Recorded behavior only for the individual statements; the recording shows pg_is_in_recovery() = t on the replica.
Emulator: the replica's engine answers pg_is_in_recovery() with false — "a read replica's engine must report pg_is_in_recovery() = true; it reports False, so this is not a replicating standby". The instance therefore is not in recovery and the read-only refusal path it implies cannot be exercised.
Why: a client's read-only safety assumption is void — writes a real replica refuses with 25006 are accepted here, so an application that accidentally points writes at the replica diverges from AWS silently and the two copies disagree.
Fix: the read-replica data plane in the rds service crate — run the replica's engine in standby/recovery mode (pg_is_in_recovery() true, writes refused with SQLSTATE 25006) and apply the primary's committed changes to it.
Traces: rds.replica-session-read-only-override-rejected.

=== Cause 4 of 4: a create-primary-then-read-replica flow never completes ===
AWS: CreateDBInstance followed by CreateDBInstanceReadReplica and DescribeDBInstances polling converges: each instance reaches DBInstanceStatus 'available' and every control-plane call answers promptly.
Emulator: both checks that create a primary and a read replica and then poll produce no result at all — "Error: Check timed out after 60.0s". No response ever diverges because no response arrives within the budget. Hypothesis: the replica (or its source) never reaches a terminal status the poll accepts, or a call in that flow does not return, so the flow stalls.
Why: a client that waits for a replica to become available waits forever; nothing downstream of replica creation (promotion, switchover, independent writes) can run.
Fix: the CreateDBInstanceReadReplica handler and the DB-instance status progression in the rds service crate's state — drive the replica to 'available' (and answer every call in that flow) within a bounded time.
Traces: rds.e2e-read-replica-promotion-independent-writes, rds.read-replica-promote-lifecycle.

## Evidence
Cause 1 — expected: the replica's DBInstance$Endpoint.Address differs from the source instance's Endpoint.Address. Actual: identical addresses; `AssertionError: the replica must serve its own endpoint so readers can be pointed at it` (rds.read-replica-replication-query-and-promote, check line 335) and `AssertionError: the replica must serve its own endpoint so a reader can be pointed at it; it advertises '127.0.0.1', the same host as the source` (rds.replica-uncommitted-primary-write-absent-until-commit, check line 263).

Cause 2 — expected: DescribeDBInstances on the replica returns StatusInfos containing an entry with StatusType == 'read replication'. Actual: `KeyError: 'StatusInfos'` at `next(s for s in replica["StatusInfos"] if s["StatusType"] == "read replication")` (rds.read-replica-promote-and-switchover, check line 67) — the member is absent from the response.

Cause 3 — expected: `SELECT pg_is_in_recovery()` on the replica returns true. Actual: `AssertionError: a read replica's engine must report pg_is_in_recovery() = true; it reports False, so this is not a replicating standby` (rds.replica-session-read-only-override-rejected, check line 259).

Cause 4 — expected: the primary and its read replica both reach DBInstanceStatus 'available' and the flow proceeds. Actual: `Error: Check timed out after 60.0s` with no response recorded (rds.e2e-read-replica-promotion-independent-writes, rds.read-replica-promote-lifecycle).

## Steps to Reproduce
1. Boot the rds emulator.
2. rds.create_db_instance(DBInstanceIdentifier=<src>, Engine='postgres', DBInstanceClass='db.t3.micro', MasterUsername=<u>, MasterUserPassword=<p>, AllocatedStorage=20, DBName='sim2real', DBSubnetGroupName=<sng>) and wait for DBInstanceStatus='available'.
3. rds.create_db_instance_read_replica(DBInstanceIdentifier=<rep>, SourceDBInstanceIdentifier=<src>, DBInstanceClass='db.t3.micro').
4. rds.describe_db_instances(DBInstanceIdentifier=<rep>) — compare Endpoint.Address with the source's (Cause 1: they are equal) and read the 'StatusInfos' key (Cause 2: absent).
5. Open a PostgreSQL session on the replica's endpoint and run `SELECT pg_is_in_recovery()` (Cause 3: returns false) and then an INSERT (AWS refuses with SQLSTATE 25006).
6. Replay rds.e2e-read-replica-promotion-independent-writes or rds.read-replica-promote-lifecycle end to end (Cause 4: the flow does not finish within 60 s).
