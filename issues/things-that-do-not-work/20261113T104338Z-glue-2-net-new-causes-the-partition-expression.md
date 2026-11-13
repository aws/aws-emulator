Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.get-partitions-expression-on-a-real-non-partition-column-refused glue.get-partitions-malformed-expression-fails glue.get-partitions-unknown-column-expression-fails glue.get-unfiltered-partitions-metadata-malformed-expression-blocked glue.glue-job-reads-its-source-through-the-catalog-and-writes-partitions-into-a-registered-table glue.glue-job-rejects-a-type-violating-source-row-and-writes-no-output-or-partition glue.glue-job-runs-add-catalog-partitions-incrementally-and-re-registering-one-creates-no-duplicate
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.get-partitions-expression-on-a-real-non-partition-column-refused|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.get-partitions-malformed-expression-fails|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.get-partitions-unknown-column-expression-fails|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.get-unfiltered-partitions-metadata-malformed-expression-blocked|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: glue.glue-job-reads-its-source-through-the-catalog-and-writes-partitions-into-a-registered-table|run#31|AssertionError|assert record.get("JobRunState") == "SUCCEEDED", (
Issue-Key: glue.glue-job-rejects-a-type-violating-source-row-and-writes-no-output-or-partition|run#24|AssertionError|assert control.get("JobRunState") == "SUCCEEDED", (
Issue-Key: glue.glue-job-runs-add-catalog-partitions-incrementally-and-re-registering-one-creates-no-duplicate|_go#27|AssertionError|assert record.get("JobRunState") == "SUCCEEDED", (
Issue-Commit: e18aca9896c14b293d2c6f422ce254ac7004d2d3

# glue: 2 net-new causes — the partition-Expression refusal Message is emulator-authored instead of AWS's fixed 'Invalid partition expression!', and every Glue ETL job run FAILS because the runtime has no awsglue library

> _Note: this issue was identified against the code base at commit `e18aca9896c14b293d2c6f422ce254ac7004d2d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPartitions, GetUnfilteredPartitionsMetadata, StartJobRun

## Description
Two net-new root causes in the glue service. Neither is marked ALREADY FILED nor OWNED by an open issue in the replay report.

Cause 1 of 2: the InvalidInputException Message for an unusable GetPartitions / GetUnfilteredPartitionsMetadata Expression is emulator-authored and request-derived instead of AWS's single fixed string.
AWS: Smithy (glue) GetPartitions.Expression -> PredicateString (@length min 0 max 2048); modeled error InvalidInputException (errorKind client, httpError 400). GetUnfilteredPartitionsMetadata carries the same Expression member and the same InvalidInputException. Real Glue refuses EVERY Expression it cannot evaluate as a partition filter — a syntactically malformed predicate, a predicate on a column the table does not have, and a predicate on a real column that is not a partition key — with HTTP 400, error code InvalidInputException and the Message 'Invalid partition expression!'. That message is ENTIRELY fixed text: it contains no request- or state-derived part, so it is identical for all three refusal classes and never echoes the expression, the column name or the operator vocabulary.
Emulator: the status (400) and the code (InvalidInputException) are right, but three different emulator-authored messages are returned, each embedding a value taken from the request: (a) for a predicate on a real non-partition column, 'Expression <column>, which is not a partition key of the table' — observed as 'Expression references payload, which is not a partition key of the table'; (b) for a predicate on an unknown column, the same template — observed as 'Expression references not_a_partition_key, which is not a partition key of the table'; (c) for a malformed predicate, "Expression <expression> is not a filter on the table's partitions; an expression uses SQL syntax similar to the SQL WHERE filter clause, built from the operators =, <>, >, <, >=, <=, AND, OR, IN, BETWEEN, LIKE, NOT and IS NULL" — observed with <expression> = "dt >>>> not a valid predicate" and with <expression> = 'year = AND ((('. The column names and expressions quoted here are per-run request values filling those templates; the templates themselves are the defect, because AWS emits none of them.
Why: a client that distinguishes Glue's refusal classes by Message text (error-handling code, retries that special-case a bad filter, log/alarm matchers) sees a string AWS never produces, and tooling that asserts on 'Invalid partition expression!' treats a correctly refused call as an unknown failure. The emulator also leaks a classification AWS deliberately does not disclose (which column was at fault).
Fix: the shared partition-Expression validator behind the get_partitions and get_unfiltered_partitions_metadata handlers in the glue service — it must raise InvalidInputException with the one fixed message for every unusable expression instead of composing a per-reason message.
Traces: glue.get-partitions-expression-on-a-real-non-partition-column-refused, glue.get-partitions-malformed-expression-fails, glue.get-partitions-unknown-column-expression-fails, glue.get-unfiltered-partitions-metadata-malformed-expression-blocked.

Cause 2 of 2: a Glue Spark job run now executes but its Python runtime does not provide the AWS Glue ETL library, so every catalog-aware ETL script ends FAILED on an import.
AWS: Smithy (glue) StartJobRun -> JobRunId; GetJobRun/GetJobRuns -> JobRun{JobRunState (modeled enum STARTING/RUNNING/STOPPING/STOPPED/SUCCEEDED/FAILED/TIMEOUT/...), ErrorMessage}. Recorded behavior: every AWS Glue Spark job image ships the AWS Glue ETL library `awsglue` — awsglue.context.GlueContext, awsglue.job.Job, awsglue.dynamicframe.DynamicFrame and awsglue.utils.getResolvedOptions are the documented entry points of a Glue PySpark script — so a Glue 4.0 G.1X script that imports them, reads through create_dynamic_frame.from_catalog, writes with getSink(enableUpdateCatalog=True) and calls job.commit() reaches JobRunState SUCCEEDED with no ErrorMessage, leaves its Parquet output under the sink path and registers the written partitions in the catalog table.
Emulator: StartJobRun is accepted and the run is executed, but it settles JobRunState='FAILED' with ErrorMessage "ModuleNotFoundError: No module named 'awsglue.context'" for every script whose first statement is `from awsglue.context import GlueContext`. Because the script dies at import, nothing downstream happens: no catalog read, no S3 output object, no partition registered on the target table, no marker object.
Why: the whole Glue ETL surface is unusable against the emulator. A client cannot test a real Glue job script at all — the documented library every Glue script imports is absent — and the failure is reported as a job failure, so it looks like the customer's script is broken rather than the environment. Catalog side effects a job is expected to produce (partitions registered via enableUpdateCatalog) never appear.
Fix: the job-run executor's PySpark runtime environment in the glue service — the interpreter a job run executes with must provide the awsglue package (context, job, dynamicframe, transforms, utils) wired to the emulator's catalog and S3, not just a bare pyspark. The job-run state machine itself is fine; the failure is the missing runtime library. (The fixed issue 20261106T214711Z-glue-s3-stepfunctions-glue-startjobrun-never-executes-the.md made runs execute at all; this is the next wall behind it, and it is not a refusal of StartJobRun — the API call still answers 200.)
Traces: glue.glue-job-reads-its-source-through-the-catalog-and-writes-partitions-into-a-registered-table, glue.glue-job-rejects-a-type-violating-source-row-and-writes-no-output-or-partition, glue.glue-job-runs-add-catalog-partitions-incrementally-and-re-registering-one-creates-no-duplicate.

## Evidence
Replay output (behavioral suite, section ### glue):

Cause 1 — expected vs actual Message, all four with code InvalidInputException / HTTP 400:
  [mismatch] glue.get-partitions-expression-on-a-real-non-partition-column-refused
    expected message 'Invalid partition expression!', got 'Expression references payload, which is not a partition key of the table'
  [mismatch] glue.get-partitions-unknown-column-expression-fails
    expected message 'Invalid partition expression!', got 'Expression references not_a_partition_key, which is not a partition key of the table'
  [mismatch] glue.get-partitions-malformed-expression-fails
    expected message 'Invalid partition expression!', got "Expression dt >>>> not a valid predicate is not a filter on the table's partitions; an expression uses SQL syntax similar to the SQL WHERE filter clause, built from the operators =, <>, >, <, >=, <=, AND, OR, IN, BETWEEN, LIKE, NOT and IS NULL"
  [mismatch] glue.get-unfiltered-partitions-metadata-malformed-expression-blocked
    expected message 'Invalid partition expression!', got "Expression year = AND ((( is not a filter on the table's partitions; an expression uses SQL syntax similar to the SQL WHERE filter clause, built from the operators =, <>, >, <, >=, <=, AND, OR, IN, BETWEEN, LIKE, NOT and IS NULL"
The column names ('payload', 'not_a_partition_key') and the quoted expressions are the per-run request values; 'Invalid partition expression!' is fixed text in full. In every case the positive control (a well-formed predicate on the real partition key 'dt') succeeded first, so the table, the partition and get_partitions itself were healthy.

Cause 2 — job runs fail at import:
  [mismatch] glue.glue-job-reads-its-source-through-the-catalog-and-writes-partitions-into-a-registered-table
    AssertionError: job run ended 'FAILED': "ModuleNotFoundError: No module named 'awsglue.context'"
  [mismatch] glue.glue-job-rejects-a-type-violating-source-row-and-writes-no-output-or-partition
    AssertionError: the control run over clean bytes ended 'FAILED': "ModuleNotFoundError: No module named 'awsglue.context'"; nothing the negative arm shows would be attributable to the changed cell
  [mismatch] glue.glue-job-runs-add-catalog-partitions-incrementally-and-re-registering-one-creates-no-duplicate
    AssertionError: the run over 'src_first_csv' ended 'FAILED': "ModuleNotFoundError: No module named 'awsglue.context'"
Expected (AWS): JobRunState 'SUCCEEDED', no ErrorMessage, the Parquet output written under the sink path and the written partition values present on the target table through GetPartitions.
Actual (emulator): JobRunState 'FAILED' with the ModuleNotFoundError above, and none of the side effects.
The failing scripts begin with: from awsglue.context import GlueContext / from awsglue.dynamicframe import DynamicFrame / from awsglue.job import Job / from awsglue.utils import getResolvedOptions.

## Steps to Reproduce
Cause 1:
1. Build and boot the emulator from this checkout (glue serves).
2. glue CreateDatabase, then CreateTable with StorageDescriptor.Columns [{Name: col1, Type: string}] and PartitionKeys [{Name: dt, Type: string}]; CreatePartition with Values ['2024-01-01'].
3. glue GetPartitions with Expression "dt = '2024-01-01'" — succeeds and returns the one partition (control).
4. glue GetPartitions with Expression "dt >>>> not a valid predicate": expect HTTP 400, code InvalidInputException, Message exactly 'Invalid partition expression!'; observe the emulator's "Expression ... is not a filter on the table's partitions; ... operators ..." text.
5. Repeat with Expression "not_a_partition_key = 'x'" (unknown column) and, on a table whose StorageDescriptor.Columns contains 'payload' while PartitionKeys does not, with Expression "payload = 'hello'": expect the SAME fixed Message 'Invalid partition expression!' for both; observe the two 'Expression references <column>, which is not a partition key of the table' variants.
6. Repeat step 4 against GetUnfilteredPartitionsMetadata (CatalogId, SupportedPermissionTypes set): same expected Message.
7. Or re-replay: glue.get-partitions-expression-on-a-real-non-partition-column-refused, glue.get-partitions-malformed-expression-fails, glue.get-partitions-unknown-column-expression-fails, glue.get-unfiltered-partitions-metadata-malformed-expression-blocked.

Cause 2:
1. Create an S3 bucket, put a header-bearing CSV, and register a Glue database plus a source table over it.
2. Upload a PySpark script that starts with `from awsglue.context import GlueContext`, `from awsglue.job import Job`, `from awsglue.utils import getResolvedOptions`, builds a GlueContext, reads with create_dynamic_frame.from_catalog, writes with getSink(..., enableUpdateCatalog=True, partitionKeys=['dt']) and calls job.commit().
3. glue CreateJob (GlueVersion 4.0, WorkerType G.1X, NumberOfWorkers 2, Command glueetl pointing at that script) and StartJobRun.
4. Poll GetJobRun: expect JobRunState 'SUCCEEDED' and the output/partitions written; observe 'FAILED' with ErrorMessage "ModuleNotFoundError: No module named 'awsglue.context'".
5. Or re-replay: glue.glue-job-reads-its-source-through-the-catalog-and-writes-partitions-into-a-registered-table, glue.glue-job-rejects-a-type-violating-source-row-and-writes-no-output-or-partition, glue.glue-job-runs-add-catalog-partitions-incrementally-and-re-registering-one-creates-no-duplicate.
