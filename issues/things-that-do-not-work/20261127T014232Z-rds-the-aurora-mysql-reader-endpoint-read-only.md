Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.aurora-mysql-reader-endpoint-write-refused-read-only
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.aurora-mysql-reader-endpoint-write-refused-read-only|_connect_reader_replica#26|Timeout|candidate = _my_connect_with_retry(reader_host, reader_port, password)
Issue-Commit: 4f741250707f5bfbfc0e3af8f0ba3eed659dbba6

# rds: the aurora-mysql reader-endpoint read-only check never completes — the cluster's ReaderEndpoint cannot be reached as a distinct read-only replica

> _Note: this issue was identified against the code base at commit `4f741250707f5bfbfc0e3af8f0ba3eed659dbba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusters

## Description
Cause 1 of 1: an Aurora MySQL cluster's ReaderEndpoint never resolves to a distinct, read-only replica member, so the whole reader-endpoint refusal flow is unreachable and the check exhausts its budget.
AWS: Smithy (com.amazonaws.rds#AmazonRDSv19) documents DBCluster$ReaderEndpoint as "The reader endpoint for the DB cluster. The reader endpoint for a DB cluster load-balances connections across the Aurora Replicas that are available in a DB cluster" — i.e. it is a hostname DISTINCT from the writer DBCluster$Endpoint and every connection opened to it lands on an Aurora Replica, never on the writer. Recorded behavior for the data plane: a session opened on the ReaderEndpoint of a two-member aurora-mysql cluster reports @@aurora_server_id equal to the reader member's DB instance identifier and @@innodb_read_only = 1, sees the row the writer committed (one shared volume), and refuses an INSERT with MySQL errno 1836 and message "Running in read-only mode" (the storage engine's own read-only mode, NOT the 1290 ER_OPTION_PREVENTS_STATEMENT a plain RDS MySQL read replica uses); the refused row is then absent on both the reader and the writer. Fixed text of that refusal: "Running in read-only mode", errno 1836; nothing in it comes from the request.
Emulator: the check never reaches any of those assertions — the replay reports "Check timed out after 60.0s" for the whole flow (CreateDBCluster(aurora-mysql) + a writer member + a PromotionTier=15 reader member, poll to available, writer session, then connect the ReaderEndpoint until @@aurora_server_id is the reader). Hypothesis (the replay reports only the timeout, so this is not a finding): the run is spent in `_connect_reader_replica`, which reopens a connection to the ReaderEndpoint until it reports the reader member's @@aurora_server_id — the same shape as the published-endpoint family below, where an Aurora cluster's Endpoint and ReaderEndpoint are the one address the core binds, so every reader connection lands on the writer and the loop can never satisfy its condition.
Why: a client cannot split reads from writes on an Aurora cluster: reads issued to the ReaderEndpoint are served by the writer (no read scaling), and — worse — a write mistakenly issued to the "reader" is ACCEPTED instead of being refused errno 1836, so an application that relies on the reader endpoint being read-only loses that guarantee and can mutate data it believed unreachable for writes.
Fix: the cluster-endpoint representation of a served Aurora engine in the RDS control plane (the same place DescribeDBClusters fills Endpoint/ReaderEndpoint) must publish a reader endpoint that is its own dialable address and routes each connection to an Aurora Replica member, and the served reader session must report @@innodb_read_only = 1 and refuse writes with errno 1836 / "Running in read-only mode". Open issue 20261118T181500Z-rds-the-published-endpoint-of-a-served-engine-is-the-core-listeners.md cause 1 describes this family and names the core listener change a fixer will hit (vendor/server/src/listeners.rs binds every resource listener on one operator-chosen interface), but neither its trace list nor its Issue-Keys include this check, so this check's replay key is recorded nowhere and must be closed with it.
Traces: failing check rds.aurora-mysql-reader-endpoint-write-refused-read-only (timeout, no assertion reached).

## Evidence
Check rds.aurora-mysql-reader-endpoint-write-refused-read-only (suite heading `### rds`):
  Error: Check timed out after 60.0s — no assertion in the check body was reached, so the replay carries no expected/actual pair beyond the timeout.
  Flow the check performs: CreateDBCluster(Engine=aurora-mysql, EngineVersion=<DescribeDBEngineVersions DefaultOnly>, MasterUsername/MasterUserPassword, DatabaseName, BackupRetentionPeriod=1, a VPC security group admitting 3306 and a DB subnet group over the run's private subnets); CreateDBInstance writer member; CreateDBInstance reader member with PromotionTier=15; poll both to available and the cluster to available with a non-empty ReaderEndpoint; open a session on DBCluster.Endpoint and assert @@innodb_read_only == 0, create a table and commit one row.
  What must then hold on the ReaderEndpoint (never observed here):
    - connections to DBCluster.ReaderEndpoint report @@aurora_server_id == <the reader member's DBInstanceIdentifier> (so ReaderEndpoint is a different hostname from Endpoint and routes to the replica),
    - @@innodb_read_only == 1 on that session,
    - the writer's committed row becomes visible there,
    - INSERT is refused with MySQL errno 1836 and message exactly "Running in read-only mode" (not errno 1290),
    - the refused row is absent on both the reader and the writer (the cluster still holds exactly 1 row).
  Related, not covering: 20261118T181500Z-rds-the-published-endpoint-of-a-served-engine-is-the-core-listeners.md cause 1 (an Aurora cluster's writer and reader endpoints are both the core's bind interface, e.g. 127.0.0.1) — same family, but it carries the rds.aurora-writer-reader-endpoint-query-routing key, not this check's.
Model citation: DBCluster$ReaderEndpoint doc (load-balances connections across the Aurora Replicas available in the cluster); DBCluster$Endpoint is the writer endpoint.

## Steps to Reproduce
1. CreateDBCluster(Engine=aurora-mysql, EngineVersion=<default>, MasterUsername=sim2real, MasterUserPassword=<legal>, DatabaseName=sim2real, BackupRetentionPeriod=1, VpcSecurityGroupIds=[<sg admitting 3306>], DBSubnetGroupName=<group over private subnets>).
2. CreateDBInstance a writer member of that cluster (db.t4g.medium or db.t3.medium) and a second member with PromotionTier=15; poll both to available and the cluster to available.
3. DescribeDBClusters and compare Endpoint with ReaderEndpoint — they must be different hostnames, each dialable on the cluster Port.
4. Open a native MySQL session on Endpoint: @@innodb_read_only must be 0; create a table and insert one row.
5. Open sessions on ReaderEndpoint until one reports @@aurora_server_id == <the reader member's identifier> (today no connection ever does); on that session @@innodb_read_only must be 1, the committed row must be visible, and an INSERT must be refused with errno 1836 / "Running in read-only mode".
6. Confirm the refused row is absent on both the reader and the writer (the table holds exactly 1 row).
7. Re-replay behavioral check rds.aurora-mysql-reader-endpoint-write-refused-read-only.
