Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: synthetics.canary-run-completes-publishes-artifacts-and-metrics
Issue-Shape-Id: com.amazonaws.synthetics#Synthetics
Issue-Key: synthetics.canary-run-completes-publishes-artifacts-and-metrics|run#21|AssertionError|raise AssertionError(
Issue-Commit: 729e703566ca4cafcbe0ca89913d95c7327bdd98

# synthetics: a PASSED canary run publishes no CloudWatchSynthetics metrics — SuccessPercent never appears for dimension CanaryName

> _Note: this issue was identified against the code base at commit `729e703566ca4cafcbe0ca89913d95c7327bdd98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCanary

## Description
One net-new root cause in the emulated CloudWatch Synthetics run lifecycle, grounded in the single failing synthetics check of this replay that was neither marked ALREADY FILED nor marked OWNED by an open issue.

=== Cause 1 of 1: a completed canary run emits no CloudWatch metric series in the CloudWatchSynthetics namespace ===

AWS: a canary run is observable in two places — the run record and CloudWatch. Smithy (services/service-com-amazonaws-synthetics-synthetics/smithy-model/com-amazonaws-synthetics-synthetics.json): GetCanaryRuns POST /canary/{Name}/runs -> 200 {CanaryRuns, NextToken}, errors InternalServerException 500 / ResourceNotFoundException 404 / ValidationException 400; CanaryRunState = RUNNING | PASSED | FAILED on CanaryRunStatus{State, StateReason, StateReasonCode, TestResult}. The metric side is not in the Smithy model — it is documented behavior (CloudWatch Synthetics canary metrics, https://docs.aws.amazon.com/AmazonCloudWatch/latest/monitoring/CloudWatch_Synthetics_Canaries_metrics.html): for every canary run, CloudWatch Synthetics publishes datapoints into the namespace `CloudWatchSynthetics` dimensioned by `CanaryName` (and, where applicable, StepName), including `SuccessPercent` — 100 for a run that passed, 0 for a run that failed — plus Duration and Failed. Those datapoints are therefore readable with cloudwatch:GetMetricStatistics(Namespace="CloudWatchSynthetics", MetricName="SuccessPercent", Dimensions=[{Name:"CanaryName", Value:<canary name>}], Period=60, Statistics=["Average"]) over a window covering the run, and the Average for a passing run is 100.0. The namespace string, the metric name, the dimension name and the 0/100 value domain are the fixed parts of the requirement; the canary name, the timestamps and the account/region come from the request and the emulator's state.

Emulator: the run itself now completes correctly — GetCanaryRuns reports CanaryRuns[0].Status.State = "PASSED" and an artifact object does land under the configured ArtifactS3Location — but nothing is ever published to CloudWatch. GetMetricStatistics for Namespace=CloudWatchSynthetics / MetricName=SuccessPercent / Dimensions=[{CanaryName:<name>}] keeps answering 200 with an EMPTY `Datapoints` list; the check polled it every 15 s for 240 s over a window of (now-15min, now+5min) and never saw a single datapoint. No status code or error code is involved — the call succeeds, the series simply does not exist, so neither SuccessPercent=100 nor any other CloudWatchSynthetics series can be read for a canary that ran.

NOTE: the open issue 20261109T183444Z-synthetics-2-net-new-root-causes-a-successful.md is grounded in this same check and carries its old replay key (the PASSED assertion). Its Cause 1 (a successful run recorded FAILED) is no longer reproducible — the run now reaches PASSED — and that issue explicitly records the metric assertion as "not reached because of this failure (so not asserted here)". The check has therefore advanced to a divergence that no open issue describes, and this replay no longer marks it OWNED. This is the next failure of the same check, not a restatement of that one.

Why: a client cannot observe canary health through CloudWatch at all. Dashboards, metric math and metric alarms built on CloudWatchSynthetics SuccessPercent (the canonical AWS pattern for alerting on a failing canary) never leave INSUFFICIENT_DATA, metric-based waiters block until their own timeout, and code that distinguishes "canary unhealthy" (SuccessPercent < 100) from "canary never reported" (no datapoints) misreads a healthy canary as one that has never run.

Fix: the metric-emission step of the synthetics run lifecycle — the run settle path in services/service-com-amazonaws-synthetics-synthetics/src/state.rs (beside the existing run-outcome/settle handling, where the run's State and Timeline are finalized) together with the StartCanary handler in services/service-com-amazonaws-synthetics-synthetics/src/ops/start_canary.rs: completing a run must publish the per-run CloudWatchSynthetics datapoints (SuccessPercent 100 on PASSED / 0 on FAILED, Duration, Failed) dimensioned CanaryName=<canary name> into the shared CloudWatch metric state, so that a GetMetricStatistics / ListMetrics read of that namespace returns them.

Traces: synthetics.canary-run-completes-publishes-artifacts-and-metrics.

## Evidence
Suite simparity-behavioral-aws-v1: 21/27 passed (77.8%). Per-service: synthetics 21/24 (88%), 3 failed — 1 is marked ALREADY FILED (UpdateCanary stub), 1 is marked OWNED by 20261030T021959Z-synthetics-a-dry-run-has-no-lifecycle-of.md, and the one below is the remainder. (cloudformation__synthetics 0/3 — all three OWNED by 20261109T183444Z-cloudformation-synthetics-3-net-new-root-causes-describestacks.md.)

[mismatch] synthetics.canary-run-completes-publishes-artifacts-and-metrics:
  check_synthetics_canary_run_completes_publishes_artifacts_and_metrics.py line 94:
  `AssertionError: run PASSED and an artifact was confirmed in S3, but the SuccessPercent CloudWatch metric had not landed within 240s (CloudWatch metric ingestion latency)`

What the check established BEFORE the failure (so these are no longer in dispute): GetCanaryRuns(Name=<name>) -> CanaryRuns[0].Status.State == "PASSED"; s3.ListObjectsV2(Bucket=<artifact bucket>, Prefix="canary/<name>/") (falling back to the whole bucket) returned at least one object.

Expected: cloudwatch.GetMetricStatistics(Namespace="CloudWatchSynthetics", MetricName="SuccessPercent", Dimensions=[{"Name":"CanaryName","Value":"<canary name>"}], StartTime=now-15min, EndTime=now+5min, Period=60, Statistics=["Average"]) returns a non-empty Datapoints list containing a datapoint with Average == 100.0 for the passing run.
Actual: 200 with Datapoints == [] on every poll for 240 s (polled every 15 s). No CloudWatchSynthetics series exists for the canary.

Model citations: GetCanaryRuns POST /canary/{Name}/runs -> 200 {CanaryRuns, NextToken}, errors InternalServerException 500 / ResourceNotFoundException 404 / ValidationException 400; CanaryRunState = RUNNING|PASSED|FAILED; CanaryRunStatus{State, StateReason, StateReasonCode, TestResult} ("This structure contains the status information about a canary run."). The metric publication itself is recorded/documented behavior only — the Synthetics model does not describe it.

## Steps to Reproduce
Setup: create an S3 artifact bucket; create an IAM execution role granting logs:CreateLogGroup/CreateLogStream/PutLogEvents, s3:PutObject + s3:GetBucketLocation on that bucket, s3:ListAllMyBuckets, and cloudwatch:PutMetricData limited to namespace CloudWatchSynthetics; pick the latest non-deprecated syn-nodejs-puppeteer runtime via synthetics.DescribeRuntimeVersions.

1. synthetics.CreateCanary(Name=<name>, Code={ZipFile:<zip containing a working heartbeat handler>, Handler:"pageLoadBlueprint.handler"}, ArtifactS3Location="s3://<bucket>", ExecutionRoleArn=<role arn>, RuntimeVersion=<latest>, Schedule={"Expression":"rate(0 minute)"}, Tags={"sim2real":"1"}).
2. Poll GetCanary(Name=<name>) until Canary.Status.State == "READY".
3. StartCanary(Name=<name>) -> 200.
4. Poll GetCanaryRuns(Name=<name>) until CanaryRuns[0].Status.State is PASSED or FAILED — it reaches "PASSED".
5. s3.ListObjectsV2(Bucket=<bucket>, Prefix="canary/<name>/") -> at least one artifact object is present.
6. cloudwatch.GetMetricStatistics(Namespace="CloudWatchSynthetics", MetricName="SuccessPercent", Dimensions=[{"Name":"CanaryName","Value":"<name>"}], StartTime=now-15min, EndTime=now+5min, Period=60, Statistics=["Average"]).
   Observed: 200 with Datapoints == [], repeatedly, for 240 s after the run completed.
   Expected: at least one datapoint, with Average == 100.0 for the passing run (0 for a failing one).
7. Also check cloudwatch.ListMetrics(Namespace="CloudWatchSynthetics") — no metric is listed for the canary at all, confirming nothing was published rather than a latency effect.
