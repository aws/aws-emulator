Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: neptunedata.list-ml-model-transform-jobs-max-items-above-limit-rejected neptunedata.gremlin-status-unknown-well-formed-query-id-rejected neptunedata.ml-model-transform-cancel-unknown-id-not-found neptunedata.opencypher-explain-update-query-refused-graph-unchanged
Issue-Shape-Id: com.amazonaws.neptunedata#AmazonNeptuneDataplane
Issue-Key: neptunedata.list-ml-model-transform-jobs-max-items-above-limit-rejected|_graph_client#1|AssertionError|raise AssertionError(f"the writer instance never became available; last status {state!r}")
Issue-Key: neptunedata.gremlin-status-unknown-well-formed-query-id-rejected|_graph_client#1|AssertionError|assert state == "available", f"the cluster instance last reported status {state!r}"
Issue-Key: neptunedata.ml-model-transform-cancel-unknown-id-not-found|_graph_client#1|AssertionError|assert state == "available", f"the cluster instance last reported status {state!r}"
Issue-Key: neptunedata.opencypher-explain-update-query-refused-graph-unchanged|_graph_client#1|AssertionError|assert state == "available", f"the cluster instance last reported status {state!r}"
Issue-Commit: ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7

# A Neptune writer DB instance never reaches DBInstanceStatus 'available': DescribeDBInstances keeps reporting 'creating' for the whole provisioning wait, so neptunedata checks never get a usable cluster

> _Note: this issue was identified against the code base at commit `ce19d6936b9d7a90d6cad029e8041d4a9ae7f5d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
THIS ISSUE HAS 1 CAUSE — it must be closed before submitting.

Cause 1 of 1: a Neptune DB instance created with CreateDBInstance into an existing DB cluster is left in the 'creating' state indefinitely — DescribeDBInstances never promotes DBInstanceStatus to 'available'. Four behavioral checks, all listed by the suite under service neptunedata, fail in their setup on exactly this wait, before any neptunedata operation is attempted. Failing check ids for this cause: neptunedata.list-ml-model-transform-jobs-max-items-above-limit-rejected (polled for the full bounded wait and the last observed status was 'creating'), neptunedata.gremlin-status-unknown-well-formed-query-id-rejected, neptunedata.ml-model-transform-cancel-unknown-id-not-found, neptunedata.opencypher-explain-update-query-refused-graph-unchanged (the same wait, bounded by the check's remaining time, expired with no status ever observed — the assertion reports status None because the poll loop's deadline had already passed when it was entered).

AWS: Smithy (services/service-com-amazonaws-neptune-amazonrdsv19/smithy-model/com-amazonaws-neptune-amazonrdsv19.json) com.amazonaws.neptune#DBInstance member DBInstanceStatus — "Specifies the current state of this database." AWS documents the Neptune/RDS instance lifecycle as 'creating' while the instance is being provisioned, then 'available' once provisioning completes; 'available' is the terminal success state and the only one in which the cluster's connection endpoint is usable. The model names no other requirement here, so for the transition itself this is recorded behavior only: every one of these checks, and the already-filed issue 20261119T200025Z-neptunedata-is-still-unreachable-nothing-serves-the-cluster.md, records the documented client pattern — CreateDBInstance, then poll DescribeDBInstances until DBInstanceStatus == 'available' — and that issue's evidence states the writer DID reach 'available' in an earlier replay of this emulator, so the promotion exists and this is a divergence from the emulator's own recorded behavior as well as from AWS.

Emulator: after CreateDBInstance(DBInstanceIdentifier=<name>-w, DBInstanceClass=db.t4g.medium, Engine=neptune, DBClusterIdentifier=<name>, PubliclyAccessible=false) is accepted, DescribeDBInstances(DBInstanceIdentifier=<name>-w) keeps answering DBInstances[0].DBInstanceStatus == 'creating' for the entire bounded wait (up to 1500 s, polled every 2 s) and never answers 'available'; the status never reaches any of the terminal failure states either ('failed', 'incompatible-network', 'incompatible-parameters', 'storage-full'), so a client polling for 'available' is given no outcome at all. Three sibling checks that create the same shape of cluster never observed a status: their wait window — the check's remaining time less a reserve — was already exhausted when the loop was entered, so DescribeDBInstances was not polled even once and the precondition was likewise never met. Hypothesis: the instance lifecycle advance is driven by something that did not run for these clusters (a time- or request-driven transition), rather than by a state machine that always eventually promotes an accepted CreateDBInstance. The generated cluster/instance identifiers and the particular wall-clock budgets are per-run values and are not part of the requirement; the requirement is that an accepted CreateDBInstance converge to 'available' (or to a documented failure status).

Why: 'available' is the documented gate every client waits on before using a cluster — the Neptune console flow, the AWS CLI's `neptune wait db-instance-available`, CloudFormation's Neptune instance resource and every SDK waiter block on it. A client gets a cluster that never becomes usable: it either hangs until its own timeout or gives up, and it never reaches the cluster's data-plane endpoint at all. It also masks the entire neptunedata surface downstream, since every data-plane check must first obtain an available writer.

Fix: the Neptune control plane's DB-instance lifecycle — the state CreateDBInstance writes and the status projection DescribeDBInstances reads — must advance an accepted instance from 'creating' to 'available' (or to a documented failure status) without depending on a caller-side trigger, and DescribeDBClusters must publish the cluster's endpoint only once that writer is available.

Traces: the four failing check ids listed above; no open issue carries them (20261119T200025Z owns nineteen other neptunedata checks and asserts the opposite observation, that the writer reached 'available').

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: Total 0/33 passed (0.0%); neptunedata 0/33 (0%) — Mismatched 4, Errored 29. The four mismatches are these; none is marked ALREADY FILED or OWNED.

[mismatch] neptunedata.list-ml-model-transform-jobs-max-items-above-limit-rejected
  File "check_neptunedata_list_ml_model_transform_jobs_max_items_above_limit_rejected.py", line 24, in run
    graph = _graph_client(ctx, "db.t4g.medium")
  File "...", line 105, in _graph_client
    raise AssertionError(f"the writer instance never became available; last status {state!r}")
  AssertionError: the writer instance never became available; last status 'creating'

The loop that produced it polls DescribeDBInstances every 2 s until deadline = now + min(1500, ctx.time_left() - 120), breaking on 'available' and raising on the terminal failure statuses:
  state = neptune.describe_db_instances(DBInstanceIdentifier=f"{name}-w")["DBInstances"][0]["DBInstanceStatus"]
so 'creating' was the answer to every poll in that window.

[mismatch] neptunedata.gremlin-status-unknown-well-formed-query-id-rejected
  File "...", line 85, in _graph_client
    assert state == "available", f"the cluster instance last reported status {state!r}"
  AssertionError: the cluster instance last reported status None
[mismatch] neptunedata.ml-model-transform-cancel-unknown-id-not-found — same assertion, line 91: status None
[mismatch] neptunedata.opencypher-explain-update-query-refused-graph-unchanged — same assertion, line 107: status None

In those three the wait is `limit = time.monotonic() + min(1500, ctx.time_left() - 400)` with `state = None` initialised before the loop, so None means the window was already past when the loop was entered and DescribeDBInstances was never polled; the 'available' precondition was never met in any of the four.

All four reach that point with the preceding control-plane and EC2 calls accepted: CreateSecurityGroup, AuthorizeSecurityGroupIngress(port 8182), CreateDBSubnetGroup(private subnets), CreateDBCluster(Engine=neptune, StorageEncrypted=true, EnableIAMDatabaseAuthentication=true, DeletionProtection=false), CreateDBInstance(DBInstanceClass=db.t4g.medium, Engine=neptune, PubliclyAccessible=false).

Expected: DescribeDBInstances on an accepted CreateDBInstance reports DBInstanceStatus 'creating' and then 'available' within the provisioning window, after which DescribeDBClusters publishes a usable Endpoint/Port and the checks proceed to their real assertions — e.g. ListMLModelTransformJobs accepts maxItems=1024 and answers ids == [], and refuses maxItems=1025 with InvalidParameterException and message "Max allowed value of maxItems is 1024" (fixed text "Max allowed value of maxItems is " plus the modelled ceiling); GetGremlinQueryStatus for a well-formed UUID naming no query answers HTTP 400 InvalidParameterException with "Supplied queryId <queryId> is invalid" (the queryId taken from the request); ExecuteOpenCypherExplainQuery on an update query answers HTTP 400 UnsupportedOperationException with "EXPLAIN is not supported for update queries" (fixed text, no variable parts).
Actual: DBInstanceStatus stays 'creating' for the whole wait (and in three sibling checks no status is ever obtained), so no cluster endpoint is ever used and none of those assertions is reached.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator from this checkout.
2. Create the private network prerequisites: CreateSecurityGroup in the VPC, AuthorizeSecurityGroupIngress for TCP 8182 from the private CIDR, CreateDBSubnetGroup over the private subnet ids.
3. CreateDBCluster with DBClusterIdentifier=<name>, Engine=neptune, StorageEncrypted=true, EnableIAMDatabaseAuthentication=true, DeletionProtection=false, the subnet group and the security group.
4. CreateDBInstance with DBInstanceIdentifier=<name>-w, DBClusterIdentifier=<name>, DBInstanceClass=db.t4g.medium, Engine=neptune, PubliclyAccessible=false.
5. Poll DescribeDBInstances(DBInstanceIdentifier=<name>-w) every 2 s and read DBInstances[0].DBInstanceStatus. Observe it stays 'creating' and never becomes 'available' (and never reaches 'failed', 'incompatible-network', 'incompatible-parameters' or 'storage-full'). Equivalently, `aws neptune wait db-instance-available --db-instance-identifier <name>-w` never returns.
6. Expected: the status reaches 'available' within the provisioning window, after which DescribeDBClusters publishes the cluster Endpoint and Port for data-plane use.
7. Re-replay the four check ids in trace_ids to confirm the fix — each must get past the 'available' wait and then be graded on its actual neptunedata assertions.
