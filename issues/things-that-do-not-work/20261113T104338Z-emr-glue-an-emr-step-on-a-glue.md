Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog
Issue-Key: emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog|run#38|AssertionError|assert ddl_step["Status"]["State"] == "COMPLETED", (
Issue-Commit: e18aca9896c14b293d2c6f422ce254ac7004d2d3

# emr__glue: an EMR step on a Glue-metastore cluster settles FAILED with an emulator-authored reason because no Hadoop jar is executed, so a Hive ADD PARTITION step never lands the partition in the Glue catalog

> _Note: this issue was identified against the code base at commit `e18aca9896c14b293d2c6f422ce254ac7004d2d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
One net-new root cause. The failing check is marked neither ALREADY FILED nor OWNED by any open issue.

Cause 1 of 1: a submitted EMR step runs the step lifecycle but executes no application, so every step ends FAILED and its side effects on the Glue Data Catalog never happen.
AWS: Smithy (elasticmapreduce) AddJobFlowSteps -> StepIds; DescribeStep/ListSteps -> Step{Status: StepStatus{State (modeled StepState enum PENDING/RUNNING/COMPLETED/CANCELLED/FAILED/INTERRUPTED), StateChangeReason, FailureDetails}}. Recorded behavior: on a cluster whose hive-site classification sets hive.metastore.client.factory.class to com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory, a command-runner Hive step that runs ALTER TABLE ... ADD PARTITION reaches Status.State 'COMPLETED' with no FailureDetails, and the partition it added is afterwards readable through glue GetPartition/GetPartitions, while a partition value the step never added stays absent ('Cannot find partition.').
Emulator: the step is accepted and walks the lifecycle, but settles Status.State 'FAILED' with StateChangeReason/FailureDetails Message 'This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application.' — a string AWS never emits. Because nothing executed, the Glue catalog is untouched: the ADD PARTITION never reaches the metastore.
Why: a client cannot exercise any on-cluster workload. Every step fails, so EMR-to-Glue integration (the Glue catalog used as the Hive metastore) is unobservable, and the failure is attributed to the step rather than to the environment — a caller debugging it is pushed towards the instance profile's Glue permissions, which the check's own error text calls out as the usual cause, when nothing ran at all.
Fix: the EMR step executor in the emr service — a step must actually execute its command/jar (at minimum the Hive/command-runner paths the catalog integration needs) and settle COMPLETED with the catalog side effects applied, instead of a lifecycle-only state machine that always ends FAILED with an emulator-authored reason. Related already-filed issues cover other EMR step checks (20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md, 20261108T151103Z, 20261108T104909Z) and the fixed 20261105T093917Z-emr-glue-spark-steps-on-a-glue-metastore.md made steps reach a terminal state; none of them names this check in its replay set, so this recorded unit is not re-replayed by any of them.
Traces: emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog.

## Evidence
Replay output (behavioral suite, section ### emr__glue):
  [mismatch] emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog
  AssertionError: the ADD PARTITION step must COMPLETE; got 'FAILED' (reason {'Message': 'This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application.'}). A failure here usually means the instance profile lacks the Glue catalog write actions

Expected (AWS): DescribeStep Status.State 'COMPLETED', no FailureDetails, and glue GetPartition for the added partition value answering 200 while the never-added value stays refused.
Actual (emulator): Status.State 'FAILED' with the emulator-authored Message quoted above, and no partition in the Glue catalog.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (emr and glue both serve).
2. Create the EMR service role and instance profile, then RunJobFlow with ReleaseLabel emr-6.15.0, applications Hadoop/Hive, KeepJobFlowAliveWhenNoSteps true, and a Configurations entry for classification 'hive-site' setting hive.metastore.client.factory.class = com.amazonaws.glue.catalog.metastore.AWSGlueDataCatalogHiveClientFactory.
3. glue CreateDatabase and CreateTable with a 'dt' partition key over an S3 location holding the rows.
4. AddJobFlowSteps with a command-runner.jar step running hive -e "ALTER TABLE <db>.<tbl> ADD PARTITION (dt='2026-01-02') LOCATION '...'".
5. Poll DescribeStep: expect Status.State 'COMPLETED'; observe 'FAILED' with the emulator-authored Message.
6. glue GetPartition for dt='2026-01-02': expect it to exist; observe it absent.
7. Or re-replay: emr__glue.on-cluster-add-partition-lands-in-the-glue-catalog.
