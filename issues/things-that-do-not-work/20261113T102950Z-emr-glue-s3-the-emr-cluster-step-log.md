Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive
Issue-Key: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: a92bf394ff6ba595d0d96d1f253c81da2b113daf

# emr__glue__s3: the EMR cluster/step/log lifecycle never converges — a LogUri cluster whose only step is submitted via the CLI never reaches a settled step + WAITING cluster + S3 log objects, so the log-persistence check exhausts its budget with no verdict

> _Note: this issue was identified against the code base at commit `a92bf394ff6ba595d0d96d1f253c81da2b113daf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddJobFlowSteps

## Description
One net-new root cause, grounded in a failing behavioral check of the `emr__glue__s3` suite.

=== Cause 1 of 1: the EMR cluster lifecycle a LogUri log-persistence client depends on never settles, so nothing downstream of it is reachable ===

AWS: a keep-alive EMR cluster (`KeepJobFlowAliveWhenNoSteps=true`) created with a `LogUri` pointing at S3 runs its submitted step to a terminal state, then settles in `WAITING`; `TerminateJobFlows` drives it to `TERMINATED`; and the cluster's logs are flushed to S3 under `<LogUri>/<cluster-id>/` and OUTLIVE the terminated cluster. Smithy (com.amazonaws.elasticmapreduce#ElasticMapReduce): `RunJobFlow` takes `LogUri` "The location in Amazon S3 to write the log files of the job flow" and `Instances.KeepJobFlowAliveWhenNoSteps`; `ListSteps` reports `StepSummary$Status.State` over `StepState` (PENDING, CANCEL_PENDING, RUNNING, COMPLETED, CANCELLED, FAILED, INTERRUPTED); `DescribeCluster` reports `Cluster$Status.State` over `ClusterState` (STARTING, BOOTSTRAPPING, RUNNING, WAITING, TERMINATING, TERMINATED, TERMINATED_WITH_ERRORS). AWS states no promptness SLA for the log flush, which is why the check polls a bounded window rather than asserting immediately.

Emulator: the check produces no comparison at all — `Error: Check timed out after 60.0s`. Every wait in it before the final assertion is a bounded poll on an OBSERVABLE emulator response (`ListSteps` until the single step is COMPLETED/FAILED/CANCELLED/INTERRUPTED; `DescribeCluster` until the state leaves STARTING/BOOTSTRAPPING/RUNNING and is `WAITING`; `DescribeCluster` after `terminate-clusters` until `TERMINATED`; `ListObjectsV2` on `logs/<cluster-id>/` until `KeyCount > 0`), and the one fixed-duration loop in the check (the 30 s persistence re-read) is explicitly capped by the harness budget via `ctx.time_left()`. A check whose only unbounded time is spent in polls that the emulator never satisfies is a check the emulator hangs: one of those conditions is never answered, so the run is cut off instead of reaching a verdict. Hypothesis (the recording shows only the timeout, not which poll): the step never reaches a terminal state and/or no object is ever written under the cluster's `LogUri` prefix — the same two gaps the sibling `emr__glue__s3` checks already show (EMR steps never reach a terminal/COMPLETED state; cluster logs never land under LogUri).

Why: a client relying on AWS's behavior uses exactly this sequence as its operational contract — submit a step, wait for it to finish, terminate the cluster, and then read the cluster's logs out of S3 for post-mortem. Against the emulator that client blocks forever on the very first wait, and the property it is actually checking (S3-persisted EMR logs survive the cluster that produced them) is unreachable: no amount of waiting produces a settled step, a `WAITING` cluster, or a single log object, and the caller gets no error to branch on either — just a hang.

Fix: the EMR cluster/step execution and log-delivery path — services/service-com-amazonaws-emr-elasticmapreduce/src/ops/add_job_flow_steps.rs and run_job_flow.rs together with the cluster/step state machine in that crate's src/state.rs: a submitted step must advance to a terminal `StepState` and a keep-alive cluster with no pending steps must settle in `WAITING`, and the cluster's log delivery must write objects under `<LogUri>/<cluster-id>/` that remain after `TerminateJobFlows` reaches `TERMINATED`.

Traces: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive.

## Evidence
Failing check: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive — verdict `mismatch`.

Replay report detail, verbatim and complete:
  Error: Check timed out after 60.0s

What the check was waiting on, in order (all bounded polls over emulator responses, from the check source in the report):
  1. `emr.list_steps(ClusterId=…)` until exactly one step is in COMPLETED/FAILED/CANCELLED/INTERRUPTED, then `assert steps[0]["Status"]["State"] == "COMPLETED"`.
  2. `emr.describe_cluster(...)["Cluster"]["Status"]["State"]` until it leaves STARTING/BOOTSTRAPPING/RUNNING, then `assert state == "WAITING"` ("a keep-alive cluster whose only step finished must be WAITING").
  3. `aws emr terminate-clusters`, then poll `describe_cluster` until TERMINATED.
  4. `s3.list_objects_v2(Bucket=…, Prefix=f"logs/{cluster_id}/")` until `KeyCount > 0`.
  5. Only then the 30 s persistence re-read, which is itself capped by `min(deadline, now + ctx.time_left())` — so it cannot be what overran the budget.

Expected: each of (1)-(4) is answered by the emulator, and the run ends on the persistence assertion.
Actual: no verdict inside the harness budget; the run never got past the polls. The 60 s figure is this environment's per-check budget, not a requirement.

The cluster id, bucket name and role names of the run are per-run values and are not part of the requirement.

Related open issues that do NOT carry this check's replay key: 20261108T104909Z-emr-glue-s3-2-net-new-causes-cluster.md and 20261105T074335Z-emr-glue-s3-emr-cluster-steps-never-reach.md (steps never reach a terminal state), 20261029T184212Z-emr-clusters-accept-steps-but-never-execute-them.md, and 20261109T065007Z-emr-glue-s3-a-cli-created-auto-terminating.md. Closing the step/log gaps they describe should also close this check; it is filed so this check id is re-replayed.

## Steps to Reproduce
1. Create an S3 bucket, the EMR service role and EC2 instance profile, and a VPC/subnet in an AZ that offers m5.xlarge.
2. `aws emr create-cluster` with `--log-uri s3://<bucket>/logs/`, `--use-default-roles`-equivalent explicit roles, `KeepJobFlowAliveWhenNoSteps=true`, and exactly one step.
3. Poll `emr list-steps` until the single step reaches a terminal state. Expect COMPLETED; observe the poll never satisfied.
4. Poll `emr describe-cluster` until the state leaves STARTING/BOOTSTRAPPING/RUNNING. Expect `WAITING`.
5. `aws emr terminate-clusters --cluster-ids <id>` and poll to `TERMINATED`.
6. `s3 list-objects-v2 --bucket <bucket> --prefix logs/<cluster-id>/`. Expect `KeyCount > 0`, and the same non-zero count on every re-read for 30 s after termination.
   Observe: the sequence never advances past step 3/4, so the whole log-persistence property is never reached and the client simply hangs.
