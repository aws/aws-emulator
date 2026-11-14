Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table
Issue-Key: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table|_wait_for_table#13|Timeout|time.sleep(5)
Issue-Commit: 3ddd7d1a9606c20c808c737ad5d54c6c77061be0

# glue__iam__kinesis__s3__s3tables: a Kinesis channel with an S3 Tables destination goes ACTIVE but never creates its destination Iceberg table, so the check exhausts its budget polling S3 Tables

> _Note: this issue was identified against the code base at commit `3ddd7d1a9606c20c808c737ad5d54c6c77061be0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
One (1) net-new root cause. Cause 1 of 1 must be closed before this issue is submitted. The failing check was NOT marked ALREADY FILED or OWNED by the replay report, so no open issue carries its current replay key — on this commit it no longer fails on the `_wait_for_table` AssertionError but as a check-level TIMEOUT, because the table it polls for never appears at all.

=== Cause 1 of 1: CreateChannel with an S3TablesDestinationConfiguration reaches ChannelStatus ACTIVE and echoes its configuration, but the destination table is never created in S3 Tables ===
AWS: Smithy (kinesis): CreateChannel input S3TablesDestinationConfiguration -> S3TablesConfigurationList of S3TablesConfiguration{TableBucketARN, Namespace, TableName, CompressionType, PartitionSpec}, where TableName is documented "The name of the destination table. Amazon Kinesis Data Streams creates this table in the specified table bucket." The provisioning side effect itself is recorded behavior: once the channel is ACTIVE and records flow, S3 Tables holds a table whose name is the configured TableName, whose namespace is the single-element list [<configured Namespace>] and whose managedByService is "kinesis.amazonaws.com" (Smithy s3tables GetTable output members name, namespace, managedByService), and ListTables(tableBucketARN=<bucket arn>, namespace=<ns>) reports exactly that one table.
Emulator: the channel is created, reaches ACTIVE, correctly echoes S3TablesDestinationConfiguration.S3TablesConfigurationList[0].TableBucketARN/TableName and reports ChannelDestinationType S3_TABLES in its summary — but no table is ever created. The check puts records on the stream and polls s3tables GetTable/ListTables up to its 720 s budget; the table never appears and the check ends as "Check timed out after 600.0s".
Why: the whole streaming-table delivery feature is unobservable — a client that creates a channel and then reads or queries the Iceberg table it was promised finds nothing, ListTables on the target bucket/namespace stays empty, and because managedByService is never written no caller can tell a Kinesis-managed table from a user table.
Related but NOT covering: open issue 20261109T141301Z-glue-iam-kinesis-s3-s3tables-kinesis-createchannel-with-an-s3-tables-destination-never.md describes this same fault as its Cause 1 and even lists this check, but the report left the check unowned, so no open issue carries its current replay key (the failure is now a timeout rather than the `_wait_for_table` assertion that issue keys on).
Fix: the CreateChannel handler's S3 Tables destination provisioning — it must create the configured table in the S3 Tables state under the configured TableBucketARN/Namespace with the configured TableName, marked managedByService "kinesis.amazonaws.com" — and the S3 Tables table state it writes into, so GetTable and ListTables both resolve it.
Traces: glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator up, bootstrap_signal false; 338/645 passed; glue__iam__kinesis__s3__s3tables 0/2). The check is listed by the report under the service heading ### glue__iam__kinesis__s3__s3tables and is marked neither ALREADY FILED nor OWNED.

[mismatch] glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table — "Error: Check timed out after 600.0s". The check creates a Kinesis stream, an S3 Tables table bucket + namespace, a DLQ bucket and the service-execution role, calls CreateChannel with an S3TablesDestinationConfiguration, and asserts in order: `final["ChannelStatus"] == "ACTIVE"` (passes), S3TablesConfigurationList[0].TableBucketARN == the request's bucket ARN (passes), .TableName == the request's table name (passes), the summary's ChannelDestinationType == "S3_TABLES" (passes), then `table = _wait_for_table(ctx, infra, channel_arn)` with _TABLE_APPEARANCE_DEADLINE_S = 720.0 and records put every 30 s. The table never appears, so the check runs out of budget before asserting table["name"] == <table>, table["namespace"] == [<namespace>], table["managedByService"] == "kinesis.amazonaws.com", and that s3tables.list_tables(tableBucketARN=<bucket arn>, namespace=<ns>)["tables"] contains exactly that table. The channel ARN, table-bucket ARN, namespace and table name are per-run values, not part of the requirement.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay simparity-behavioral-aws-v1 and re-run glue__iam__kinesis__s3__s3tables.create-channel-s3-tables-destination-creates-iceberg-table; it must pass before submitting.
3. Create a Kinesis stream, an S3 Tables table bucket and namespace, an S3 bucket for the dead-letter queue, and an IAM service-execution role trusted by kinesis.amazonaws.com.
4. Call kinesis.create_channel(ChannelName=<name>, ServiceExecutionRoleARN=<role>, StreamConfigurationList=[...], S3TablesDestinationConfiguration={"DeadLetterQueueS3Configuration": {...}, "S3TablesConfigurationList": [{"TableBucketARN": <bucket arn>, "Namespace": <ns>, "TableName": <table>, "CompressionType": "ZSTD", "PartitionSpec": {"PartitionFields": [{"Transform": "TIME_HOUR", "SourceName": <field>}]}}]).
5. Poll DescribeChannel until ChannelStatus == 'ACTIVE' and confirm the echoed TableBucketARN/TableName and the summary's ChannelDestinationType == 'S3_TABLES'.
6. Put records on the stream, then poll s3tables.get_table / s3tables.list_tables(tableBucketARN=<bucket arn>, namespace=<ns>): the configured table must appear with name == <table>, namespace == [<ns>] and managedByService == 'kinesis.amazonaws.com', and ListTables must report exactly that table. The emulator never creates it.
