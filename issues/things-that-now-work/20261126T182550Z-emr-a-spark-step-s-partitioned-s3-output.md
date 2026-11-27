Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr.spark-step-partitioned-write-lands-hive-layout-in-s3
Issue-Shape-Id: com.amazonaws.emr#ElasticMapReduce
Issue-Key: emr.spark-step-partitioned-write-lands-hive-layout-in-s3|run#23|AssertionError|assert bases[0].startswith("part-") and bases[0].endswith(".json"), (
Issue-Commit: 38a0a8578508fa045104c1fd6763f635f665f954

# emr: a Spark step's partitioned S3 output is written as bare `part-00000` files, not Spark part-file names (`part-NNNNN-<uuid>-cNNN.json`)

> _Note: this issue was identified against the code base at commit `38a0a8578508fa045104c1fd6763f635f665f954`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
1 net-new root cause, grounded in 1 failing behavioral check whose replay key is carried by no open issue.

Cause 1 of 1: the objects an EMR Spark step's DataFrameWriter lands in S3 are named with a bare `part-<index>` basename carrying no write-job UUID and no format suffix, so the output of `write.partitionBy(...).json(<s3 uri>)` is not a Spark part file. Failing check id: emr.spark-step-partitioned-write-lands-hive-layout-in-s3.

AWS: recorded behavior only — the EMR Smithy model (RunJobFlow / StepConfig.HadoopJarStep) models the step submission and the step's terminal state, and says nothing about the S3 objects the step's application writes, so the requirement comes from what a real `spark-submit` of this job produces. On AWS the step runs `command-runner.jar spark-submit ... s3://<bucket>/job/partitioned_write.py s3://<bucket>/input/ s3://<bucket>/out/agg/` and the PySpark job calls `agg.repartition(1).write.mode("overwrite").partitionBy("dept").json(out)`. Spark's FileOutputCommitter then lands, under each Hive-style partition directory `dept=<value>/`, one part file whose basename follows Spark's fixed template: the fixed text is the `part-` prefix, the `-c` file-counter separator and the format suffix matching the chosen writer (`.json` for the JSON writer); its variable parts are the zero-padded task/partition index, the write-job UUID Spark mints per write, and the counter — i.e. `part-NNNNN-<write-job-uuid>-cNNN.json`. The `_SUCCESS` marker (requested here by `spark.hadoop.mapreduce.fileoutputcommitter.marksuccessfuljobs=true`) sits directly under the output prefix. One recording's index, UUID and counter digits are per-run values and are not part of the requirement; the `part-` prefix and the `.json` suffix are.
Emulator: the step reaches COMPLETED, the `_SUCCESS` marker is served under `out/agg/`, and exactly the expected Hive-style directories `dept=eng/` and `dept=ops/` each hold exactly one object — but that object is named `part-00000`: no write-job UUID, no `-cNNN` counter and no `.json` format suffix. Replay output: `AssertionError: partition 'dept=eng' holds 'part-00000', not a Spark JSON part file`. The assertions on the record body that follow (the aggregated `total`, the run nonce, and the partition column pruned out of the body) are never reached, so only the naming divergence is claimed here.
Why: a client that locates Spark output by its name — a `*.json` glob or suffix filter, an S3 Inventory/lifecycle rule or EventBridge S3 notification pattern keyed on the suffix, a downstream reader that selects part files by extension, or any tooling that parses the write-job UUID out of the basename to attribute files to one write — finds nothing under the partition directories even though the step succeeded. It also means the object was not produced by a format-aware Spark writer at all, so a client cannot rely on the format of the bytes inside matching the writer it asked for.
Fix: the EMR step execution component of the emr service crate — specifically the Spark/`spark-submit` step runner and the S3 writer it drives when materializing a DataFrameWriter's output. The part-file naming must come from the writer's format and its per-write job UUID (prefix `part-`, zero-padded task index, write-job UUID, `-cNNN` counter, format suffix), not from a bare index. The partition-directory layout and the `_SUCCESS` marker it already produces are correct and must be preserved.
Traces: emr.spark-step-partitioned-write-lands-hive-layout-in-s3.

## Evidence
Check: emr.spark-step-partitioned-write-lands-hive-layout-in-s3 (verdict: mismatch)
Reported error: AssertionError: partition 'dept=eng' holds 'part-00000', not a Spark JSON part file
(assertion at check_emr_spark_step_partitioned_write_lands_hive_layout_in_s3.py:612 — `assert bases[0].startswith("part-") and bases[0].endswith(".json")`)

Scenario: one S3 bucket holding `input/data.csv` (header `dept,amount,run`, rows (eng,7),(ops,11),(eng,5),(ops,4),(eng,2), each carrying this run's nonce) and `job/partitioned_write.py` (a PySpark job that reads the CSV, groups by `dept`, sums `amount`, then `repartition(1).write.mode("overwrite").partitionBy("dept").json(out)`). A 2-node m5.xlarge emr-6.15.0 cluster with Applications=[Spark] runs one step: `command-runner.jar` with Args `spark-submit --deploy-mode client --conf spark.hadoop.mapreduce.fileoutputcommitter.marksuccessfuljobs=true s3://<bucket>/job/partitioned_write.py s3://<bucket>/input/ s3://<bucket>/out/agg/`.

What already matches AWS and is NOT the complaint:
- DescribeStep reports Status.State COMPLETED with an ordered Status.Timeline (CreationDateTime <= StartDateTime <= EndDateTime).
- `out/agg/_SUCCESS` is present directly under the output prefix.
- The partition directories are exactly `dept=eng/` and `dept=ops/`, one visible object in each (repartition(1)).

The divergence:
  expected: each partition directory holds one Spark part file whose basename starts with `part-` and ends with the writer's format suffix `.json` — AWS's template `part-NNNNN-<write-job-uuid>-cNNN.json` (fixed: the `part-` prefix, the `-c` counter separator, the `.json` suffix; variable: the task index, the write-job UUID, the counter digits).
  actual:   `dept=eng/part-00000` — basename `part-00000`, with no write-job UUID, no `-cNNN` counter and no `.json` suffix. `dept=ops/` is named the same way.

## Steps to Reproduce
1. s3:CreateBucket B. s3:PutObject B/input/data.csv with body "dept,amount,run\neng,7,<nonce>\nops,11,<nonce>\neng,5,<nonce>\nops,4,<nonce>\neng,2,<nonce>\n".
2. s3:PutObject B/job/partitioned_write.py with a PySpark script that reads argv[1] as a header CSV, casts `amount` to long, groups by `dept` aggregating sum(amount) as `total` and first(run) as `run`, then `agg.repartition(1).write.mode("overwrite").partitionBy("dept").json(argv[2])`.
3. Create the EMR service role and EC2 instance profile, a VPC and a subnet.
4. emr:RunJobFlow with ReleaseLabel emr-6.15.0, Applications=[{Name: Spark}], Instances={InstanceCount: 2, MasterInstanceType: m5.xlarge, SlaveInstanceType: m5.xlarge, Ec2SubnetId: <subnet>, KeepJobFlowAliveWhenNoSteps: true}, LogUri s3://B/logs/, StepConcurrencyLevel 1, and one step: HadoopJarStep={Jar: "command-runner.jar", Args: ["spark-submit", "--deploy-mode", "client", "--conf", "spark.hadoop.mapreduce.fileoutputcommitter.marksuccessfuljobs=true", "s3://B/job/partitioned_write.py", "s3://B/input/", "s3://B/out/agg/"]}.
5. Poll emr:ListSteps until the step is terminal, then emr:DescribeStep — it is COMPLETED (this matches AWS).
6. Poll s3:ListObjectsV2(Bucket=B, Prefix="out/agg/") until `out/agg/_SUCCESS` appears (this matches AWS).
7. Inspect the keys under `out/agg/`. Expected per AWS: `out/agg/dept=eng/part-00000-<uuid>-c000.json` and `out/agg/dept=ops/part-00000-<uuid>-c000.json` — a `part-` prefix and a `.json` suffix in each partition directory. Today: `out/agg/dept=eng/part-00000` and `out/agg/dept=ops/part-00000`, with no UUID, no counter and no format suffix.
8. s3:GetObject on the part file and confirm it holds exactly one JSON record per partition with fields {total, run} and no `dept` field — not reached today, because step 7 fails first.
