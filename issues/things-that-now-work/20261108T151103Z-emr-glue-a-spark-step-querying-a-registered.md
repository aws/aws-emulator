Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails
Issue-Key: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails|run#31|AssertionError|assert good_state == "COMPLETED", (
Issue-Commit: 4466ef5882b584b00b87b338b6098c1e65748c9e

# emr__glue: a Spark step querying a REGISTERED Glue Data Catalog table settles FAILED without running Spark, so the unregistered-table failure cannot be distinguished

> _Note: this issue was identified against the code base at commit `4466ef5882b584b00b87b338b6098c1e65748c9e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStep

## Description
Cause 1 of 1: on a cluster configured with the Glue Data Catalog as its Hive metastore, the EMR step that queries a table that IS registered in the catalog — the check's positive control — settles FAILED without running Spark, so the companion step's failure on an unregistered table proves nothing about the catalog. Related open issue that does not cover this check (the suite marks it as a plain mismatch, not OWNED): 20261105T093917Z-emr-glue-spark-steps-on-a-glue-metastore.md.

AWS: Smithy (EMR): DescribeStep(ClusterId: ClusterId required, StepId: StepId required) -> {Step}; Step.Status is StepStatus {State: StepState, StateChangeReason, FailureDetails, Timeline}, StepState = PENDING | CANCEL_PENDING | RUNNING | COMPLETED | CANCELLED | FAILED | INTERRUPTED. The model does not bind a command to an outcome, so recorded behavior only: real EMR runs the step's spark-submit on the cluster, and a Spark job that reads a table present in the Glue catalog (with the role permitted to glue:GetTable/GetPartitions and the EMRFS read) exits 0, so the step reaches COMPLETED; a Spark job reading a table that is NOT registered fails in the driver with Spark's own AnalysisException and the step reaches FAILED with FailureDetails describing that driver failure.

Emulator: BOTH steps settle FAILED a couple of seconds after submission with the identical emulator-authored text — Status.StateChangeReason.Message 'This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application.' and FailureDetails {Reason: that sentence, Message: "No application was started for this step: this emulator holds the cluster's step state and runs no Hadoop, Spark or MapReduce runtime, so the step's own side effects did not happen."}. Neither sentence is AWS text, and no Spark job is run against the Glue metastore.

Why: a client cannot tell a Glue-catalog resolution failure from the blanket emulator failure — both steps carry the same state and the same reason — so the Glue-metastore integration is unverifiable and any error handling that reads FailureDetails to decide 'the table is not registered' versus 'the job itself failed' takes the wrong branch. A pipeline that waits for the catalog-backed query to COMPLETE never proceeds.

Fix: the EMR crate's step execution/state machine and its Glue-metastore (hive-site/glue catalog) configuration path — the step's terminal state must follow the modelled outcome of its command (COMPLETED for the registered-table query) and FailureDetails must describe a real driver failure for the unregistered-table query rather than a blanket 'no application was started'.

Traces: emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator, service heading `### emr__glue`.

[mismatch] emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails:
  AssertionError: the step querying the REGISTERED Glue table is the positive control and must reach COMPLETED -- otherwise the other step's failure says nothing about the catalog -- got 'FAILED' ({'Reason': 'This emulator runs the step lifecycle but executes no Hadoop jar, so the step ended without running its application.', 'Message': "No application was started for this step: this emulator holds the cluster's step state and runs no Hadoop, Spark or MapReduce runtime, so the step's own side effects did not happen."})

Expected: Step.Status.State == 'COMPLETED' for the registered-table step, with no FailureDetails; FAILED with a Spark driver AnalysisException in FailureDetails only for the unregistered-table step.

Smithy grounding: DescribeStep input {ClusterId required, StepId required} -> {Step}, errors InvalidRequestException (client, 400) and InternalServerException (server, 500); StepState enum as above. The outcome of a given spark-submit is recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay check emr__glue.emr-spark-step-on-an-unregistered-glue-table-fails from suite simparity-behavioral-aws-v1 (service token `emr__glue`):
1. Create a Glue database and register one table over an S3 prefix; leave a second table name unregistered.
2. RunJobFlow a cluster whose Configurations set the Hive metastore to the Glue Data Catalog, with an instance role permitted on Glue and the S3 prefix.
3. AddJobFlowSteps a spark-submit step that reads the REGISTERED table, and a second one that reads the unregistered table.
4. Poll DescribeStep on both. Expect COMPLETED for the registered-table step and FAILED with a Spark driver AnalysisException for the unregistered one; observe both FAILED with the identical 'executes no Hadoop jar' / 'No application was started for this step' reason.
