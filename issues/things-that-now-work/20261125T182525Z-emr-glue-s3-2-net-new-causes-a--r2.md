Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive emr__glue__s3.eventbridge-rule-missing-target-permission-negative
Issue-Key: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive|run#23|Timeout|time.sleep(2)
Issue-Key: emr__glue__s3.eventbridge-rule-missing-target-permission-negative|run#38|Timeout|broken_received.extend(_receive_all(sqs, broken_queue_url))
Issue-Commit: a261d7034414071b17452f9a3bcb123672d78c4c

# emr__glue__s3: 2 net-new causes — a CLI-created EMR cluster never finishes its step, never settles in WAITING/TERMINATED and never persists logs under LogUri; and the Glue job run driving the EventBridge negative-delivery differential never settles in a terminal JobRunState

> _Note: this issue was identified against the code base at commit `a261d7034414071b17452f9a3bcb123672d78c4c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunJobFlow

## Description
Two net-new root causes in the emr__glue__s3 group. Both surviving failures of this replay report "Error: Check timed out after 60.0s": the replay harness kills the check while it is still polling an emulator state machine that never advances, so no assertion in either check body is ever reached. Neither check was marked ALREADY FILED, and neither was marked OWNED by an open issue in this report, so neither is covered by an existing work item.

Cause 1 of 2: a cluster created through the EMR CLI (create-cluster with --steps, --log-uri and a keep-alive instance group) never drives its step to a terminal StepState, never settles the cluster in WAITING, never reaches TERMINATED after TerminateClusters, and never persists the cluster's log bundle under its LogUri prefix in S3. Failing check: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive.
AWS: Smithy com.amazonaws.emr#StepState = PENDING | CANCEL_PENDING | RUNNING | COMPLETED | CANCELLED | FAILED | INTERRUPTED and com.amazonaws.emr#ClusterState = STARTING | BOOTSTRAPPING | RUNNING | WAITING | TERMINATING | TERMINATED | TERMINATED_WITH_ERRORS. A CUSTOM_JAR step on command-runner.jar running `bash -c echo <text>` advances PENDING -> RUNNING -> COMPLETED, ListSteps/DescribeStep report that terminal state with its Timeline, and a cluster whose instance group keeps the job flow alive with no steps left settles in WAITING (not a terminal state). TerminateClusters/TerminateJobFlows then moves it TERMINATING -> TERMINATED, and the cluster's logs — recorded behavior only; the model does not describe log delivery — are flushed to S3 under <LogUri>/<clusterId>/ and remain listable by ListObjectsV2 after the cluster is TERMINATED (the objects are ordinary S3 objects, not cluster-scoped state, so they outlive the cluster across repeated reads).
Emulator: the check is cut off at the 60 s harness budget inside its poll loops, in order: ListSteps never shows the single step in COMPLETED/FAILED/CANCELLED/INTERRUPTED, DescribeCluster never leaves STARTING/BOOTSTRAPPING/RUNNING for WAITING, and ListObjectsV2 on logs/<clusterId>/ never returns a non-zero KeyCount. No status/error-code divergence is produced — the operations are accepted and the state simply never advances. Cluster ids, bucket names and the log prefix are per-run values; the requirement is that each of those convergences happens at all.
Why: a client cannot observe the EMR lifecycle at all through the CLI path — it cannot tell a step that completed from one that will never run, cannot wait for a keep-alive cluster to become WAITING before submitting more work, and cannot read the logs EMR is required to persist to S3. Every troubleshooting flow that relies on logs surviving the cluster that produced them is unavailable, and any waiter hangs instead of returning an outcome.
Fix: the EMR step execution/lifecycle in the emr service state (the step the cluster accepts must be advanced to RUNNING and then to a terminal StepState, and a keep-alive cluster with no runnable steps must settle in WAITING), the TerminateClusters/TerminateJobFlows path to TERMINATED, and the cluster log-upload side effect that writes the log bundle under LogUri into S3 on step completion and on termination.
Traces: emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive ("Check timed out after 60.0s"). Related open issues that do NOT carry this check's replay key, so they do not cover it: 20261105T074335Z-emr-glue-s3-emr-cluster-steps-never-reach.md, 20261108T104909Z-emr-glue-s3-2-net-new-causes-cluster.md and 20261110T214521Z-emr-glue-s3-the-s3-log-permission-boundary.md describe the same missing side effects for other check ids; this cause should be closed together with them.

Cause 2 of 2: the Glue pythonshell job run that drives the EventBridge "Glue Job State Change" negative-delivery differential never becomes observable in a terminal JobRunState, so neither the permissioned SQS target's delivery nor the unpermissioned target's silence is ever graded. Failing check: emr__glue__s3.eventbridge-rule-missing-target-permission-negative.
AWS: Smithy com.amazonaws.glue#JobRunState = STARTING | RUNNING | STOPPING | STOPPED | SUCCEEDED | FAILED | TIMEOUT | ERROR | WAITING | EXPIRED; StartJobRun returns a JobRunId and GetJobRun returns JobRun{JobRunState, ErrorMessage, StartedOn, CompletedOn, ...}. A pythonshell job (PythonVersion 3.9, MaxCapacity 0.0625) whose script exits 0 progresses STARTING -> RUNNING -> SUCCEEDED with no ErrorMessage, and on that terminal transition AWS publishes an event with source `aws.glue`, detail-type `Glue Job State Change` and a detail carrying the job name and state SUCCEEDED. An ENABLED rule matching that pattern delivers the event to an SQS target whose queue policy allows sqs:SendMessage to Service events.amazonaws.com (conditioned on aws:SourceArn = the rule ARN) and delivers nothing to a sibling target queue with no resource policy. Job names, run ids, queue names and timestamps are per-run values; the requirement is the pair of observations (at least one message on the permissioned queue with detail.jobName = the requested job name and detail.state = "SUCCEEDED", and zero messages on the unpermissioned queue for the whole absence window).
Emulator: the 60 s budget is consumed inside the check's start-and-wait cycle over StartJobRun/GetJobRun, which polls until JobRunState is terminal (and restarts the run while a terminal run carries an "assume role permissions" ErrorMessage). No delivery assertion is reached, so the emulator never presents the run in a terminal JobRunState the client accepts. Hypothesis: the Glue job-run executor does not drive the run to a terminal JobRunState, so the terminal-transition event publication, the rule evaluation and both SQS reads are never exercised.
Why: a client polling GetJobRun cannot distinguish a run still executing from one that will never finish, and the EventBridge-to-SQS permission boundary becomes unobservable — a correctly-policed target and a target missing its queue policy both yield nothing, so silently broken event delivery is indistinguishable from working delivery.
Fix: the Glue job-run executor and the StartJobRun/GetJobRun run-state machine in the glue service (settle the run in a terminal JobRunState with AWS's ErrorMessage semantics), plus the terminal-transition `Glue Job State Change` event publication that feeds EventBridge rule evaluation and SQS target delivery.
Traces: emr__glue__s3.eventbridge-rule-missing-target-permission-negative ("Check timed out after 60.0s"). Open issue 20261124T203912Z-emr-glue-s3-the-eventbridge-glue-job-state-58c9b9.md records this same check id and 20261123T183657Z-emr-glue-s3-the-eventbridge-glue-job-state.md, 20261118T205847Z-emr-glue-s3-a-glue-pythonshell-job-run.md record non-converging Glue job runs for other ids, but the replay report marked this check neither ALREADY FILED nor OWNED (their replay keys bind different steps — the recorded key for this check was in _receive_all, the current cut-off is earlier, in the job-run wait), so it is filed here as still-present and should be closed together with them.

## Evidence
Suite report simparity-behavioral-aws-v1, service heading '### emr__glue__s3' (36/47 passed, 11 failed). The two failures below are the only ones in the report marked neither ALREADY FILED nor OWNED.

Cause 1 — [mismatch] emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive
  Replay output: "Error: Check timed out after 60.0s" (no assertion in the body reached).
  Scenario: `aws emr create-cluster --release-label emr-6.15.0 --applications Name=Hadoop --instance-type m5.xlarge --instance-count 1 --service-role <role> --ec2-attributes InstanceProfile=<profile>,SubnetId=<subnet> --log-uri s3://<bucket>/logs/ --steps '[{Name, Type: CUSTOM_JAR, ActionOnFailure: CONTINUE, Jar: command-runner.jar, Args: [bash, -c, echo sim2real-log-step]}]'`, then `aws emr terminate-clusters --cluster-ids <id>`.
  Expected (AWS): ListSteps shows the one step in State COMPLETED; DescribeCluster then reports State WAITING (keep-alive, no steps left); after terminate-clusters DescribeCluster reaches TERMINATED; ListObjectsV2(Bucket=<bucket>, Prefix="logs/<clusterId>/") returns KeyCount > 0 and keeps returning KeyCount > 0 on every re-read over the following 30 s.
  Actual (emulator): none of those convergences occurs within the budget — the step stays non-terminal, the cluster never reports WAITING, and no object ever appears under logs/<clusterId>/. 36 checks of this same service and budget complete, so the budget is not the constraint.

Cause 2 — [mismatch] emr__glue__s3.eventbridge-rule-missing-target-permission-negative
  Replay output: "Error: Check timed out after 60.0s" (no assertion in the body reached).
  Scenario: bucket holding scripts/job.py (prints and exits 0); IAM role trusting glue.amazonaws.com with AWSGlueServiceRole plus an inline Allow on s3:GetObject/s3:ListBucket; Glue job (Command Name=pythonshell, PythonVersion=3.9, MaxCapacity=0.0625, Timeout=5); ENABLED EventBridge rule with pattern {source:["aws.glue"], detail-type:["Glue Job State Change"], detail:{jobName:[<job>], state:["SUCCEEDED"]}}; two SQS target queues — "ok" with a policy allowing sqs:SendMessage to Service events.amazonaws.com conditioned on aws:SourceArn=<rule arn>, "broken" with no policy.
  Expected (AWS): GetJobRun converges on JobRunState "SUCCEEDED" with no ErrorMessage; ReceiveMessage on "ok" yields >= 1 message whose body has detail.jobName == <job> and detail.state == "SUCCEEDED"; ReceiveMessage on "broken" yields zero messages for the whole absence window.
  Actual (emulator): the start-and-wait cycle over StartJobRun/GetJobRun never yields an accepted terminal JobRunState, the budget expires inside it, and neither the positive delivery nor the negative absence is observed.

## Steps to Reproduce
Boot the emulator and replay the two checks in trace_ids from suite simparity-behavioral-aws-v1, or by hand:

Cause 1 (emr__glue__s3.cli-emr-cluster-terminate-s3-logs-persist-positive):
1. s3.create_bucket. Create an EMR service role (trusts elasticmapreduce.amazonaws.com, AmazonElasticMapReduceRole attached) and an EC2 instance profile/role (trusts ec2.amazonaws.com, AmazonElasticMapReduceforEC2Role attached). Create a VPC with a public subnet in an AZ offering m5.xlarge.
2. `aws emr create-cluster --release-label emr-6.15.0 --applications Name=Hadoop --instance-type m5.xlarge --instance-count 1 --service-role <role> --ec2-attributes InstanceProfile=<profile>,SubnetId=<subnet> --log-uri s3://<bucket>/logs/ --steps '[{"Name":"s","Type":"CUSTOM_JAR","ActionOnFailure":"CONTINUE","Jar":"command-runner.jar","Args":["bash","-c","echo sim2real-log-step"]}]'`.
3. Poll emr.list_steps(ClusterId): AWS reaches State COMPLETED; the emulator never leaves a non-terminal state.
4. Poll emr.describe_cluster(ClusterId).Cluster.Status.State: AWS reaches WAITING; the emulator never does.
5. `aws emr terminate-clusters --cluster-ids <id>`, then poll describe_cluster for TERMINATED: not reached.
6. s3.list_objects_v2(Bucket=<bucket>, Prefix="logs/<clusterId>/"): AWS returns KeyCount > 0 and keeps doing so on re-reads after termination; the emulator returns KeyCount 0 forever.

Cause 2 (emr__glue__s3.eventbridge-rule-missing-target-permission-negative):
1. s3.create_bucket; put_object scripts/job.py with a trivially succeeding body.
2. iam.create_role trusting glue.amazonaws.com; attach AWSGlueServiceRole; put_role_policy allowing s3:GetObject/s3:ListBucket on the bucket and its keys.
3. glue.create_job(Name=<job>, Role=<arn>, Command={Name:'pythonshell', ScriptLocation:'s3://<bucket>/scripts/job.py', PythonVersion:'3.9'}, MaxCapacity=0.0625, Timeout=5).
4. events.put_rule(Name=<rule>, State='ENABLED', EventPattern={source:['aws.glue'], detail-type:['Glue Job State Change'], detail:{jobName:[<job>], state:['SUCCEEDED']}}).
5. sqs.create_queue twice; set the events.amazonaws.com SendMessage policy (Condition ArnEquals aws:SourceArn=<rule arn>) on the 'ok' queue only; events.put_targets with both queues.
6. glue.start_job_run(JobName=<job>) and poll glue.get_job_run for a terminal JobRunState. AWS: SUCCEEDED within the window. Emulator: no accepted terminal state is ever observed and the check's budget expires here.
7. sqs.receive_message on both queues. AWS: one matching event on 'ok', nothing on 'broken'. Emulator: neither observation is reached.

## Resolution

Both causes are DECLINED: every call either check waits on is served, and every state it polls for
is reported. Nothing in either path is refused — the step's command and the run's script are
executed, the log objects are written and the publication is delivered — so there is no refusal to
re-word and no missing side effect to build.

Measured against this emulator, one observable changed at a time:

| what was changed for the measurement | where the check then stopped |
|---|---|
| every clock span of both services collapsed to 200 ms (`CLUSTER_PHASE_SPAN`, `STEP_RUN_SPAN`, `TRANSITION_SPAN`, `JOB_RUN_SETTLES_AFTER`) | unchanged for BOTH checks — same timeout, same statements (`run#23`, `time.sleep(2)`; `_receive_all#38`, `receive_message(…, WaitTimeSeconds=2)`) |
| the `Glue Job State Change` publication suppressed, so no event can reach either target | cause 2's cut moves EARLIER, `_receive_all#38` -> `_receive_all#32`: with the publication in place the check gets six calls further, because the admitted queue's receives answer a message at once instead of waiting out the long poll — so the delivery it is in that window for does happen |

So neither check is blocked on this emulator's clock, and cause 2's positive delivery is observed.
Each check is killed at the harness budget inside its own fixed-duration TAIL window, past the last
condition it polls an emulator response for: cause 1 inside the 30 s log-persistence re-read
(`run#23`, `time.sleep(2)`), cause 2 inside the absence window it holds on the unpermissioned queue
(`broken_received.extend(_receive_all(sqs, broken_queue_url))`). The same instrumentation for
cause 1, recorded in `issues/things-that-now-work/20261116T201047Z-emr-glue-s3-2-net-new-causes-an.md`,
walked its cut through all four of its polled conditions and measured ~20 s spent by its client
before its first poll.

What the two sequences cost this emulator, end to end over the whole assembled router with request
authentication on: 8.8 s from `RunJobFlow` to `TERMINATED` with the log objects listable
(`ReadyDateTime` 6 s after creation, the step's span 2 s, the shutdown 0.6 s), and 6.0 s from
`StartJobRun` to `SUCCEEDED` with the event already on the admitted queue.

Both halves are pinned, in the order a client observes them and with both lifecycles in flight at
once, by `core/router/tests/all/emr_cluster_and_glue_run_converge_on_one_emulator.rs`, beside the
suites that already cover each on its own
(`core/router/tests/all/emr_keep_alive_cluster_log_post_mortem.rs`,
`core/router/tests/all/glue_job_state_change_delivery_per_queue_policy.rs`,
`services/service-com-amazonaws-emr-elasticmapreduce/tests/all/cluster_log_persistence_after_termination.rs`,
`services/service-com-amazonaws-glue-awsglue/tests/all/job_run_state_change_events.rs`).
