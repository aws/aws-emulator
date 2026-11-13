Issue-Source: trace-sweep-divergence
Issue-Trace: athena-cats-traj-table-location-mistakes
Issue-Step: task#14
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: athena-cats-traj-table-location-mistakes
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].LastAccessTime
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].Parameters.transient_lastDdlTime
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].PartitionKeys
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].StorageDescriptor.BucketColumns
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].StorageDescriptor.Location
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].StorageDescriptor.Parameters
Issue-Key: glue|GetTables|2xx->2xx|$.TableList[*].StorageDescriptor.SkewedInfo
Issue-Commit: 8c5329059e752b9fa65691305eadc8c02e100ead

# Glue GetTables omits Hive-compatibility defaults on tables created through Athena DDL (1 cause)

> _Note: this issue was identified against the code base at commit `8c5329059e752b9fa65691305eadc8c02e100ead`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTables

## Description
ONE net-new cause in the Glue table that Athena DDL registers: GetTables returns the table without the members the Data Catalog always materializes for a Hive-compatible external table, so its storage descriptor is a partial object.

Cause 1 of 1 — GetTables (fixture athena-cats-traj-table-location-mistakes step task#14).
AWS: Smithy com.amazonaws.glue#Table models LastAccessTime (timestamp), PartitionKeys (ColumnList — its documentation states explicitly that 'When you create a table used by Amazon Athena, and you do not specify any partitionKeys, you must at least set the value of partitionKeys to an empty list', i.e. the catalog stores and returns []), Parameters (ParametersMap) and StorageDescriptor; com.amazonaws.glue#StorageDescriptor models BucketColumns (NameStringList), Parameters (ParametersMap), SkewedInfo and Location; com.amazonaws.glue#SkewedInfo models SkewedColumnNames, SkewedColumnValues and SkewedColumnValueLocationMaps. For a table registered by a Hive CREATE EXTERNAL TABLE, the Data Catalog returns these present-and-empty rather than absent: PartitionKeys [], StorageDescriptor.BucketColumns [], StorageDescriptor.Parameters {}, StorageDescriptor.SkewedInfo {SkewedColumnNames: [], SkewedColumnValues: [], SkewedColumnValueLocationMaps: {}}, LastAccessTime 0, and Table.Parameters carrying the Hive key transient_lastDdlTime (its value is the DDL epoch second — a per-run value, not part of the requirement). StorageDescriptor.Location is the S3 prefix holding the table's data, per the member's own documentation ('The physical location of the table'), never an individual object key.
Emulator: GetTables returns TableList[0] with LastAccessTime absent, Parameters.transient_lastDdlTime absent, PartitionKeys absent, StorageDescriptor.BucketColumns absent, StorageDescriptor.Parameters absent and StorageDescriptor.SkewedInfo absent; StorageDescriptor.Location is the object key the rejected DDL named (…/products/category_a/items.csv) instead of the prefix (…/products).
Why: a client that round-trips a Glue table (GetTable/GetTables then CreateTable or UpdateTable, which every IaC tool, crawler-replacement script and catalog migrator does) reads null where AWS gives an empty list or map, and either faults on the null or writes the table back without its Hive-compatibility members; code that iterates PartitionKeys to decide whether a table is partitioned cannot distinguish 'unpartitioned' from 'unknown'. A consumer that derives an S3 prefix from StorageDescriptor.Location to list or write data objects gets a single-object path and lists nothing.
Fix: the Glue table state/serializer for tables created through the Athena DDL path — materialize the Hive-compatibility defaults (PartitionKeys, BucketColumns, StorageDescriptor.Parameters, SkewedInfo, LastAccessTime, transient_lastDdlTime) and store Location as the data prefix. The Location half of this cause is downstream of the Athena cause filed as 'Athena query failures are misclassified…' (fixture athena-cats-traj-table-location-mistakes step task#9): AWS never creates this table, because it rejects a file LOCATION. The open issue 20261104T063259Z-glue-catalog-objects-created-through-athena-ddl-are.md covers Athena-DDL catalog objects being absent altogether and does not carry this cluster's replay key.
Traces: athena-cats-traj-table-location-mistakes#task#14.

## Evidence
Replay of trace set simparity-wire-aws-v1 (36 fixtures, 384 steps, score 35.9%).

athena-cats-traj-table-location-mistakes#task#14, glue GetTables, status 200 vs 200:
  $.TableList[0].LastAccessTime: missing (expected 0.0, got None)
  $.TableList[0].Parameters.transient_lastDdlTime: missing (expected '<shaped-not-asserted>', got None)
  $.TableList[0].PartitionKeys: missing (expected [], got None)
  $.TableList[0].StorageDescriptor.BucketColumns: missing (expected [], got None)
  $.TableList[0].StorageDescriptor.Parameters: missing (expected {}, got None)
  $.TableList[0].StorageDescriptor.SkewedInfo: missing (expected {'SkewedColumnNames': [], 'SkewedColumnValueLocationMaps': {}, 'SkewedColumnValues': []}, got None)
  $.TableList[0].StorageDescriptor.Location: value_mismatch (expected the table's data prefix 's3://<bucket>/products', got the object key 's3://<bucket>/products/category_a/items.csv')

Smithy cited: com.amazonaws.glue#Table members LastAccessTime (timestamp), PartitionKeys (ColumnList; doc requires an empty list for an Athena-used table with no partition keys), Parameters (ParametersMap), StorageDescriptor; com.amazonaws.glue#StorageDescriptor members Location ('The physical location of the table'), BucketColumns, Parameters, SkewedInfo; com.amazonaws.glue#SkewedInfo members SkewedColumnNames, SkewedColumnValues, SkewedColumnValueLocationMaps. The transient_lastDdlTime key is Hive-compatibility recorded behavior; its value is per-run and not asserted.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Fixture athena-cats-traj-table-location-mistakes, step task#14 — register an external table through Athena DDL (CREATE EXTERNAL TABLE … LOCATION 's3://<bucket>/products/…'), then call Glue GetTables for the owning database.
4. Inspect TableList[0]: PartitionKeys, LastAccessTime, Parameters.transient_lastDdlTime, StorageDescriptor.BucketColumns, StorageDescriptor.Parameters, StorageDescriptor.SkewedInfo are all absent, and StorageDescriptor.Location is an object key rather than the data prefix.
