Issue-Source: trace-sweep-divergence
Issue-Trace: rds-db-instance-snapshot-and-restore
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-db-instance-snapshot-and-restore
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBInstanceAutomatedBackups|2xx->2xx|$.DescribeDBInstanceAutomatedBackupsResponse.DescribeDBInstanceAutomatedBackupsResult.DBInstanceAutomatedBackups.DBInstanceAutomatedBackup.DBInstanceAutomatedBackupsArn
Issue-Commit: 601df947af41928830de48079c97009550b26bdc

# rds auto-backup ARN carries an ab- id of the wrong width and alphabet

> _Note: this issue was identified against the code base at commit `601df947af41928830de48079c97009550b26bdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstanceAutomatedBackups

## Description
One net-new wire cause in the rds service, grounded in the single replayed divergence cluster the report marks neither blocked-by-a-stub nor OWNED by an open issue (the other 60 clusters of this replay all carry a non-empty owned_by).

Related open issues a fixer should read first, none of which carries this replay key: 20261124T183738Z-rds-2-net-new-causes-a-cloudformation-deployed.md (its Cause 2 is the automated-backup LIFECYCLE never settling for a live instance, a behavioral check, not the shape of the published ARN), 20261119T043432Z-rds-5-net-new-behavioral-causes-two-automated.md (automated-backup not-found MESSAGES), 20261120T222703Z-rds-2-net-new-causes-startdbinstanceautomatedbackupsreplication-s-dbinstancenotfound.md and 20261122T085622Z-rds-2-net-new-causes-stopdbinstanceautomatedbackupsreplication-resolves-the.md (replication-start/stop refusals) and the open scaffold issue 20261104T053605Z-rds-describedbinstanceautomatedbackups-stub.md (the handler is in fact implemented at services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_instance_automated_backups.rs). None of them covers the cause below.

=== Cause 1 of 1: DBInstanceAutomatedBackupsArn publishes an `ab-` resource id of a width and character set the service never answers ===
AWS: Smithy DBInstanceAutomatedBackup$DBInstanceAutomatedBackupsArn is "The Amazon Resource Name (ARN) for the replicated automated backups", and DescribeDBInstanceAutomatedBackups$DBInstanceAutomatedBackupsArn documents the form of the value — "the Amazon Resource Name (ARN) of the replicated automated backups, for example, arn:aws:rds:us-east-1:123456789012:auto-backup:ab-L2IJCEXJP7XQ7HOJ4SIEXAMPLE": the resource part is the `auto-backup` resource type followed by an `ab-` prefixed token that is the automated backup's own id, one per source DB instance per Region (DbiResourceId: "the resource ID for the source DB instance, which can't be changed and which is unique to an Amazon Web Services Region"). Beyond the ARN grammar itself the model does not state the token's width, so for the token: recorded behavior only — the recorded 200 response for this step carries an `ab-` token of 64 characters drawn from the lowercase base32 alphabet (a-z and the digits 2-7 only; no 0, 1, 8 or 9). That width and that alphabet are the requirement; the particular 64 characters of one recording are a per-run minted value and are NOT.
Emulator: the same 200 answer publishes an `ab-` token of 58 characters whose tail is hexadecimal — a 26-character lowercased copy of the source instance's resource-id token followed by a 32-character hex digest, so the published token is 6 characters short and uses characters (0, 1, 8, 9) that never appear in the recorded form. The width is hard-coded: AUTOMATED_BACKUP_ID_TOKEN_LEN = 58 in services/service-com-amazonaws-rds-amazonrdsv19/src/state/instance_automated_backups.rs, and the filler is `super::minted_hex`, a hex digest, rather than a base32 token.
Why: the automated-backup ARN is an identity a client keeps and matches — it is the value DescribeDBInstanceAutomatedBackups and DeleteDBInstanceAutomatedBackup accept back as DBInstanceAutomatedBackupsArn, and the value that goes into IAM resource statements, tag-and-ARN inventories and backup-governance tooling. A client that validates an rds auto-backup ARN against the resource-name grammar AWS answers (token width, base32 alphabet) rejects every ARN this emulator publishes, and ARNs minted against real AWS and against the emulator are not interchangeable in the same configuration even though both describe the same resource type.
Fix: the automated-backup id minter in the rds service's state — `automated_backup_id` / `instance_automated_backups_arn` and the AUTOMATED_BACKUP_ID_TOKEN_LEN constant that sets its width, plus the inverse reader `dbi_resource_id_in_automated_backups_arn` and `names_an_automated_backup`/`check_automated_backups_arn` which must keep accepting the token form the minter emits (file citations under Traces). The token must be minted at the recorded width in the recorded alphabet, and must stay a function of the source instance's DbiResourceId and Region so one backup reports one ARN on every read and the source id still reads back out of it.
Traces: rds-db-instance-snapshot-and-restore#task#9 (DescribeDBInstanceAutomatedBackups, 200 vs 200, key $.DescribeDBInstanceAutomatedBackupsResponse.DescribeDBInstanceAutomatedBackupsResult.DBInstanceAutomatedBackups.DBInstanceAutomatedBackup.DBInstanceAutomatedBackupsArn). Code: services/service-com-amazonaws-rds-amazonrdsv19/src/state/instance_automated_backups.rs lines 374-420 (`instance_automated_backups_arn`, `automated_backup_id`) and line 558 (AUTOMATED_BACKUP_ID_TOKEN_LEN = 58), consumed at lines 205-206, 221, 301-302 and 332; the reading side at lines 447-462 and 573-583; handler services/service-com-amazonaws-rds-amazonrdsv19/src/ops/describe_db_instance_automated_backups.rs.

## Evidence
Replay of fixture rds-db-instance-snapshot-and-restore, step task#9 (service rds, operation DescribeDBInstanceAutomatedBackups): expected_status 200, actual_status 200, verdict mismatch on one key.

  $.DescribeDBInstanceAutomatedBackupsResponse.DescribeDBInstanceAutomatedBackupsResult.DBInstanceAutomatedBackups.DBInstanceAutomatedBackup.DBInstanceAutomatedBackupsArn: value_mismatch
    expected 'arn:aws:rds:us-west-2:000000000000:auto-backup:ab-nlyeoxsdrpbgx45kvq2jhfm3sgnlogi5zbe2tb5hntrsixowicck52j3c64qakwa'
    actual   'arn:aws:rds:us-west-2:000000000000:auto-backup:ab-azrq3tsihnj8ar97tdmcbejduv68ea3edb81b8ee16f7b0ba8ebc21f1cc'

What is a requirement here and what is not: the arn:aws:rds:<region>:<account>:auto-backup:ab-<token> grammar is a requirement (and both sides satisfy it). The TOKEN'S shape is the divergence: expected token length 64, characters only from [a-z2-7]; actual token length 58, containing 0, 1, 8 and 9 (a 26-character lowercase source token followed by a 32-character hex digest, '68ea3edb81b8ee16f7b0ba8ebc21f1cc'). The particular characters of either token, the region, the account id 000000000000 and the per-run instance identifier are environment/per-run values and are NOT requirements — only the token's width and alphabet, and that the token remains one stable function of the source instance's DbiResourceId and Region.

Source of the emulator value: services/service-com-amazonaws-rds-amazonrdsv19/src/state/instance_automated_backups.rs declares `const AUTOMATED_BACKUP_ID_TOKEN_LEN: usize = 58;` and builds the token in `automated_backup_id` as the source instance's lowercased resource-id token padded out with `super::minted_hex(...)`, i.e. a hexadecimal digest. Its own doc comment acknowledges the model's documented example (`ab-L2IJCEXJP7XQ7HOJ4SIEXAMPLE`) differs from what it emits and claims 58 is "the width the auto-backup ARN Amazon RDS answers is recorded carrying" — the recorded answer in this corpus carries 64 base32 characters, so that constant and that alphabet are both wrong.

## Steps to Reproduce
1. Boot the emulator and sign requests for us-west-2.
2. CreateDBInstance(DBInstanceIdentifier=<id>, Engine=mysql, DBInstanceClass=db.t3.micro, AllocatedStorage=20, StorageType=gp2, BackupRetentionPeriod=1) and poll DescribeDBInstances until DBInstanceStatus=available, as fixture rds-db-instance-snapshot-and-restore does before its task#9 step.
3. Call DescribeDBInstanceAutomatedBackups(DBInstanceIdentifier=<id>) and read DBInstanceAutomatedBackups[0].DBInstanceAutomatedBackupsArn.
4. Split the ARN on ':auto-backup:' and inspect the resource token: it must begin with 'ab-' and be followed by 64 characters drawn only from [a-z2-7]. The emulator answers 'ab-' plus 58 characters including 0/1/8/9 (hex tail).
5. Call DescribeDBInstanceAutomatedBackups again passing that ARN as DBInstanceAutomatedBackupsArn: the same single record must come back, i.e. whatever token form is minted must still round-trip through the service's own ARN reader and still resolve to the source instance's DbiResourceId.
