Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.automated-backups-replication-nonexistent-source-instance-rejected rds.integration-lifecycle
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.automated-backups-replication-nonexistent-source-instance-rejected|run#9|AssertionError|assert err_start.get("Message") == expected_start_msg, err_start.get("Message")
Issue-Key: rds|ModifyDBClusterParameterGroup|InvalidParameterValue
Issue-Commit: 955f6c0c0ddc3c6d3572e0c42ed8336820cafac5

# rds: 2 net-new causes — StartDBInstanceAutomatedBackupsReplication's DBInstanceNotFound message is the generic one, and the aurora-mysql8.0 cluster-parameter catalog has no binlog_backup

> _Note: this issue was identified against the code base at commit `955f6c0c0ddc3c6d3572e0c42ed8336820cafac5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDBInstanceAutomatedBackupsReplication

## Description
Two independent, net-new RDS control-plane divergences, both grounded in failing behavioral checks under the `rds` heading. Neither is covered by an open issue: the only existing issue mentioning binlog parameters (20261117T174147Z-cloudformation-rds-3-net-new-causes-an-invalid.md) is about the CASING of an accepted `binlog_row_image` VALUE being rolled back, not about a parameter NAME the family catalog does not publish at all.

=== Cause 1 of 2: StartDBInstanceAutomatedBackupsReplication answers DBInstanceNotFound with the wrong message text ===
AWS: Smithy (com-amazonaws-rds-amazonrdsv19) models StartDBInstanceAutomatedBackupsReplication with error DBInstanceNotFoundFault, awsQueryError code `DBInstanceNotFound`, HTTP 404 — the code the emulator already returns is right. For this operation AWS does NOT word the fault as the plain instance lookup sentence: cross-region automated-backup replication resolves the source instance in the SOURCE region named by the `SourceDBInstanceArn`, and the message says so. Recorded behavior: the message is `Requested DB Instance <db-instance-identifier> not found in source region <region>`. Fixed text: `Requested DB Instance ` … ` not found in source region `. Variable parts: the DB instance identifier parsed out of the request's `SourceDBInstanceArn`, and the region parsed out of that same ARN (the check's source region). No trailing period.
Emulator: returns code DBInstanceNotFound (correct) but the message `DBInstance <db-instance-identifier> not found.` — the generic DescribeDBInstances-style sentence, with different fixed text (`DBInstance ` / ` not found.`), a trailing period, and no mention of the source region at all.
Why: a client distinguishing "the ARN names an instance that is not in the source region" from a plain missing-instance lookup parses or matches on this sentence; cross-region backup-replication tooling that reports or branches on the source-region wording gets the wrong diagnosis, and any assertion/log scrape against AWS's text fails. The StopDBInstanceAutomatedBackupsReplication leg of the same flow (expected `InvalidDBInstanceState` with `DB Instance <source-arn> is not replicating to the current region.`) is never reached in this run because the Start leg's assertion fires first, so it must be re-verified with the same check after the fix.
Fix: the not-found refusal in the StartDBInstanceAutomatedBackupsReplication handler must use this operation's own source-region message rather than the shared instance-not-found constant; the Stop counterpart's message must likewise be its own "is not replicating to the current region" sentence (see Traces for the files).
Traces: failing check `rds.automated-backups-replication-nonexistent-source-instance-rejected` (AssertionError on the Start message). Handlers: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/start_db_instance_automated_backups_replication.rs and .../stop_db_instance_automated_backups_replication.rs.

=== Cause 2 of 2: the aurora-mysql8.0 cluster-parameter catalog does not publish binlog_backup, so a valid ModifyDBClusterParameterGroup is refused ===
AWS: Smithy models ModifyDBClusterParameterGroup with only DBParameterGroupNotFoundFault (404) and InvalidDBParameterGroupStateFault (400); `InvalidParameterValue` is the AWS Query common error used when a named parameter is not one the group's family publishes. Recorded behavior only for the catalog contents: real Aurora MySQL 8.0 cluster parameter groups (family `aurora-mysql8.0`) publish the enhanced-binlog parameter set — `binlog_backup`, `binlog_replication_globaldb` and `aurora_enhanced_binlog` alongside `binlog_format` and `binlog_row_image`/`binlog_row_metadata` — and setting `binlog_backup=0` with ApplyMethod `pending-reboot` on a customer-created group of that family SUCCEEDS (HTTP 200, returning the group name). These are exactly the prerequisites an Aurora→Redshift-Serverless zero-ETL integration requires, so the check sets them before CreateIntegration.
Emulator: the single ModifyDBClusterParameterGroup call that sets binlog_format/binlog_row_image/binlog_row_metadata/binlog_backup/binlog_replication_globaldb/aurora_enhanced_binlog is refused with HTTP 400 `InvalidParameterValue`, message `Could not find parameter with name: binlog_backup`. Because the handler validates every entry before writing any, the whole request is rejected and the group keeps none of the settings. The refusal comes from the family catalog lookup in state (`validate_cluster_parameter_setting`), which has no entry for `binlog_backup` under family `aurora-mysql8.0`. The call fails at the first unpublished name, so `binlog_replication_globaldb` and `aurora_enhanced_binlog` are never reached and may be missing from the same catalog too — all three must be published for the flow to proceed.
Why: a request AWS accepts is refused, so the whole zero-ETL integration lifecycle is unreachable on this emulator — the cluster is never created, CreateIntegration/DescribeIntegrations/ModifyIntegration are never exercised, and the documented downstream refusals (InvalidParameterCombination naming binlog_row_metadata when it is unset; IntegrationNotFoundFault from ModifyIntegration after delete) cannot be observed. Any client that configures enhanced binlog as an Aurora MySQL prerequisite breaks at the first modify.
Fix: the published cluster-parameter catalog for the Aurora MySQL 8.0 family in RDS state — the same catalog DescribeDBClusterParameters reports and ModifyDBClusterParameterGroup validates against — must carry the enhanced-binlog parameters (`binlog_backup`, `binlog_replication_globaldb`, `aurora_enhanced_binlog`) with their apply type, data type and allowed values, so a request naming them is accepted and echoed rather than refused as unknown.
Traces: failing check `rds.integration-lifecycle` (ClientError InvalidParameterValue on ModifyDBClusterParameterGroup). Validation site: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_cluster_parameter_group.rs:258 calling crate::state::validate_cluster_parameter_setting, whose family catalog lives in services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs.

## Evidence
Cause 1 — check rds.automated-backups-replication-nonexistent-source-instance-rejected:
  request: StartDBInstanceAutomatedBackupsReplication(SourceDBInstanceArn="arn:aws:rds:<region>:<account>:db:<ghost-id>") for an instance that does not exist.
  expected: HTTP 404, Error.Code = "DBInstanceNotFound", Error.Message = "Requested DB Instance <ghost-id> not found in source region <region>" (fixed text "Requested DB Instance "/" not found in source region "; variables = identifier and region from the request ARN).
  actual:   Error.Code = "DBInstanceNotFound" (correct), Error.Message = "DBInstance <ghost-id> not found."
  replay output: AssertionError: DBInstance sim2real-dbi-25d98bb99245 not found.  (assert err_start.get("Message") == expected_start_msg)
  not yet reached in this run, must also hold after the fix: StopDBInstanceAutomatedBackupsReplication on the same ARN -> Error.Code = "InvalidDBInstanceState", Error.Message = "DB Instance <source-arn> is not replicating to the current region."

Cause 2 — check rds.integration-lifecycle:
  request: ModifyDBClusterParameterGroup(DBClusterParameterGroupName=<customer-created group, DBParameterGroupFamily="aurora-mysql8.0">, Parameters=[binlog_format=ROW, binlog_row_image=full, binlog_row_metadata=FULL, binlog_backup=0, binlog_replication_globaldb=0, aurora_enhanced_binlog=1], all ApplyMethod=pending-reboot).
  expected: HTTP 200 returning DBClusterParameterGroupName; all six settings recorded as pending-reboot.
  actual:   HTTP 400 InvalidParameterValue — "Could not find parameter with name: binlog_backup"; nothing written to the group.
  replay output: botocore.exceptions.ClientError: An error occurred (InvalidParameterValue) when calling the ModifyDBClusterParameterGroup operation: Could not find parameter with name: binlog_backup
  consequence: the check aborts before CreateDBCluster, so the zero-ETL integration create/describe/modify/delete legs are never exercised.

## Steps to Reproduce
Cause 1:
1. Boot the emulator and point an rds client at it.
2. Call StartDBInstanceAutomatedBackupsReplication with SourceDBInstanceArn="arn:aws:rds:<region>:<account>:db:<identifier-that-does-not-exist>".
3. Observe Error.Code is DBInstanceNotFound but Error.Message is "DBInstance <identifier> not found." instead of "Requested DB Instance <identifier> not found in source region <region>".
4. Call StopDBInstanceAutomatedBackupsReplication with the same ARN and check Code=InvalidDBInstanceState with message "DB Instance <arn> is not replicating to the current region."
5. Re-replay behavioral check rds.automated-backups-replication-nonexistent-source-instance-rejected.

Cause 2:
1. CreateDBClusterParameterGroup(DBClusterParameterGroupName=<new name>, DBParameterGroupFamily="aurora-mysql8.0", Description=...).
2. ModifyDBClusterParameterGroup on that group with Parameters=[{binlog_format,ROW,pending-reboot},{binlog_row_image,full,pending-reboot},{binlog_row_metadata,FULL,pending-reboot},{binlog_backup,0,pending-reboot},{binlog_replication_globaldb,0,pending-reboot},{aurora_enhanced_binlog,1,pending-reboot}].
3. Observe HTTP 400 InvalidParameterValue "Could not find parameter with name: binlog_backup" instead of a 200; DescribeDBClusterParameters shows none of the six applied.
4. After publishing the parameters, repeat and confirm binlog_replication_globaldb and aurora_enhanced_binlog are also accepted (they are shadowed by the first failure today).
5. Re-replay behavioral check rds.integration-lifecycle.
