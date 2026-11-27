Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.automated-backups-replication-nonexistent-source-instance-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.automated-backups-replication-nonexistent-source-instance-rejected|run#11|AssertionError|assert err_stop.get("Code") == _ERR_STOP_NOT_REPLICATING, (
Issue-Commit: 4f741250707f5bfbfc0e3af8f0ba3eed659dbba6

# rds StopDBInstanceAutomatedBackupsReplication again resolves the source instance first: a non-replicating ARN is refused DBInstanceNotFound (404) instead of InvalidDBInstanceState (400)

> _Note: this issue was identified against the code base at commit `4f741250707f5bfbfc0e3af8f0ba3eed659dbba6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopDBInstanceAutomatedBackupsReplication

## Description
REOPENS 20261122T085622Z-rds-2-net-new-causes-stopdbinstanceautomatedbackupsreplication-resolves-the.md: its fix left the call refused. That resolved report's cause 1 carries this check's exact replay key (`rds.automated-backups-replication-nonexistent-source-instance-rejected|run#11|AssertionError|assert err_stop.get("Code") == _ERR_STOP_NOT_REPLICATING`) and its RESOLUTION note claims the handler now resolves the replication from the SourceDBInstanceArn alone and looks no source DB instance up; the replayed check still fails on the same assertion, so the refusal ordering is back (or never landed) on the current build. The still-open 20261120T222703Z-rds-2-net-new-causes-startdbinstanceautomatedbackupsreplication-s-dbinstancenotfound.md is keyed on the START leg's message (which now passes) and only mentions this Stop leg in prose, so it does not carry it either.

Cause 1 of 1: the Stop leg checks the source instance's existence before the replication record of the calling Region.
AWS: Smithy (com.amazonaws.rds#AmazonRDSv19) models StopDBInstanceAutomatedBackupsReplication with input SourceDBInstanceArn (required) and exactly two errors — DBInstanceNotFoundFault (errorKind client, httpError 404, awsQueryError code `DBInstanceNotFound`) and InvalidDBInstanceStateFault (errorKind client, httpError 400, awsQueryError code `InvalidDBInstanceState`, doc "The DB instance isn't in a valid state."). The question this operation asks is about the DESTINATION Region it was called in: is the ARN's instance replicating its automated backups INTO this Region? Recorded behavior: for an ARN naming an instance that does not exist at all, the Stop is refused HTTP 400 `InvalidDBInstanceState`, message `DB Instance <SourceDBInstanceArn> is not replicating to the current region.` Fixed text: `DB Instance ` … ` is not replicating to the current region.` (trailing period included); the single variable part is the request's SourceDBInstanceArn reproduced WHOLE (the ARN, not the bare identifier). The Start leg on the identical ARN is refused 404 `DBInstanceNotFound` with `Requested DB Instance <identifier> not found in source region <region>` (that leg passes today) — the two legs resolve different things, which is why their codes differ.
Emulator: the Stop is refused HTTP 404 with Error.Code `DBInstanceNotFound` and the Start leg's sentence `Requested DB Instance <identifier> not found in source region <region>` — i.e. the handler performs the source-instance lookup first and never reaches the replicated-backup test that would produce InvalidDBInstanceStateFault.
Why: a client tearing down cross-Region automated-backup replication branches on exactly this distinction. `InvalidDBInstanceState` means "there is nothing to stop here" and is swallowed as idempotent success; `DBInstanceNotFound` (404) means "your ARN is wrong" and is escalated. Today a stop of a replication that is not in force — a repeat stop, a stop issued from the wrong destination Region, or a stale ARN — is reported as a bad ARN, so idempotent cleanup paths abort and reconciliation loops fail closed.
Fix: in the StopDBInstanceAutomatedBackupsReplication handler, order the checks so the calling Region's replicated-backup record is tested BEFORE any source-instance lookup, and refuse InvalidDBInstanceStateFault whenever no replication into the calling Region exists (missing source instance included); that refusal's message must be formatted from the request's SourceDBInstanceArn by this operation's own message constructor, not from the bare identifier or from the Start leg's source-region sentence. DBInstanceNotFoundFault stays modeled but is not the answer for this input.
Traces: failing check rds.automated-backups-replication-nonexistent-source-instance-rejected (AssertionError on the Stop leg's error code, check line 58). Handler: services/service-com-amazonaws-rds-amazonrdsv19/src/ops/stop_db_instance_automated_backups_replication.rs; message constructors in services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs (`backup_replication_not_to_this_region_message`, `source_db_instance_not_found_message`).

## Evidence
Check rds.automated-backups-replication-nonexistent-source-instance-rejected (suite heading `### rds`):
  setup: DescribeDBInstances(MaxRecords=20) succeeds (positive control). An ARN is built for a DB instance identifier that was never created: arn:aws:rds:<region>:<account>:db:<ghost-identifier>.
  step 1 (PASSES today): StartDBInstanceAutomatedBackupsReplication(SourceDBInstanceArn=<ghost-arn>) -> HTTP 404, Error.Code "DBInstanceNotFound", Error.Message "Requested DB Instance <ghost-identifier> not found in source region <region>".
  step 2 (the divergence): StopDBInstanceAutomatedBackupsReplication(SourceDBInstanceArn=<the same ghost-arn>)
    expected: HTTP 400, Error.Code "InvalidDBInstanceState", Error.Message "DB Instance <ghost-arn> is not replicating to the current region." — fixed text "DB Instance " / " is not replicating to the current region.", single variable part = the request's SourceDBInstanceArn in full.
    actual:   HTTP 404, Error.Code "DBInstanceNotFound", Error.Message "Requested DB Instance <ghost-identifier> not found in source region <region>".
  replay output: AssertionError: Stop on the same ARN: expected InvalidDBInstanceState, got {'Type': 'Sender', 'Code': 'DBInstanceNotFound', 'Message': 'Requested DB Instance sim2real-dbi-0e060af4a898 not found in source region us-west-2'}
  (the identifier and region in that line are this run's generated values and fill the template; only the status, code and the fixed text with its ARN variable are requirements.)
Model citation: StopDBInstanceAutomatedBackupsReplication input {SourceDBInstanceArn required}; errors [DBInstanceNotFoundFault httpError 404 / awsQueryError DBInstanceNotFound, InvalidDBInstanceStateFault httpError 400 / awsQueryError InvalidDBInstanceState].

## Steps to Reproduce
1. Boot the emulator and point an rds client at it; confirm DescribeDBInstances answers (positive control).
2. Build an ARN for a DB instance identifier that does not exist: arn:aws:rds:<region>:<account>:db:<ghost-identifier>, with <region> the caller's own Region.
3. Call StartDBInstanceAutomatedBackupsReplication(SourceDBInstanceArn=<that ARN>) and observe the correct 404 / DBInstanceNotFound / "Requested DB Instance <identifier> not found in source region <region>".
4. Call StopDBInstanceAutomatedBackupsReplication(SourceDBInstanceArn=<the same ARN>). Observe HTTP 404 / Error.Code DBInstanceNotFound instead of HTTP 400 / Error.Code InvalidDBInstanceState with message "DB Instance <that ARN> is not replicating to the current region."
5. Also exercise the existing-instance path: create a DB instance, never start backup replication, then Stop with its ARN — the code must be InvalidDBInstanceState and the message must quote the request ARN whole, not the bare identifier.
6. Re-replay behavioral check rds.automated-backups-replication-nonexistent-source-instance-rejected.
