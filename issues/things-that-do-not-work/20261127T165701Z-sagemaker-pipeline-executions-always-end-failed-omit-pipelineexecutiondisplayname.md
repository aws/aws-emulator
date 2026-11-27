Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-pipeline-execution-token-replay-and-retry-refusals
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-pipeline-execution-token-replay-and-retry-refusals sagemaker-pipeline-execution-unknown-target-errors
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribePipelineExecution|2xx->2xx|$.CreationTime
Issue-Key: sagemaker|DescribePipelineExecution|2xx->2xx|$.LastModifiedTime
Issue-Key: sagemaker|DescribePipelineExecution|2xx->2xx|$.PipelineExecutionStatus
Issue-Key: sagemaker|DescribePipelineExecution|2xx->2xx|$.PipelineExecutionDisplayName
Issue-Key: sagemaker|StartPipelineExecution|2xx->2xx|$.PipelineExecutionArn
Issue-Commit: 0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad

# sagemaker pipeline executions always end Failed, omit PipelineExecutionDisplayName, and ignore ClientRequestToken replay

> _Note: this issue was identified against the code base at commit `0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribePipelineExecution, StartPipelineExecution

## Description
Three independent causes on the emulated SageMaker Pipelines execution path, all reached by replaying wire fixtures. Each is numbered and each must be closed before this issue is submitted.

=== Cause 1 of 3: a started pipeline execution reaches terminal status Failed instead of Succeeded ===
AWS: Smithy — DescribePipelineExecution output PipelineExecutionStatus targets the PipelineExecutionStatus enum {Executing, Stopping, Stopped, Failed, Succeeded, CompletedWithFailures}; FailureReason (@length 0..1300) is present only when the execution failed. Recorded behavior: an execution of the fixtures' definitions polls from Executing to PipelineExecutionStatus 'Succeeded' and carries no FailureReason; the replay harness polls DescribePipelineExecution with 'Failed' in its fail_on set, so AWS never passes through Failed for these definitions.
Emulator: every such execution settles 'Failed' — the poll aborts with "'Failed' is in fail_on after 2 attempt(s)" on two different fixtures.
Why: no client can observe a successful pipeline run, so every downstream flow that needs one Succeeded execution (retry refusals, stop-after-finish refusals, selective execution, step listing, version pinning) is unreachable, and callers' error handling is handed a failure AWS would not report. Related open issues — 20261024T160943Z-sagemaker-runs-no-job-work-processing-pipeline-step.md and cause 1 of 20261127T123223Z-sagemaker-6-net-new-causes-runtime-free-condition.md — describe the same symptom from the behavioral corpus and name the no-runtime FailureReason; neither carries these wire fixture/step keys, so this replay evidence is unowned and must be closed here.
Fix: the pipeline-execution runner/state that decides an execution's terminal status (it must execute or evaluate the steps it can and only fail when a step genuinely needs an unimplemented job runtime).
Traces: sagemaker-pipeline-execution-token-replay-and-retry-refusals#task#6, sagemaker-pipeline-execution-unknown-target-errors#task#6.

=== Cause 2 of 3: DescribePipelineExecution omits PipelineExecutionDisplayName ===
AWS: Smithy — DescribePipelineExecution output member PipelineExecutionDisplayName targets PipelineExecutionName (@length 1..82, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,81}$). Recorded behavior: the member is always present on a describe of a started execution (the recording asserts its shape, not a literal value); when StartPipelineExecution was not given a PipelineExecutionDisplayName, SageMaker derives one for the execution and reports it here.
Emulator: the member is absent from the response entirely ($.PipelineExecutionDisplayName: missing).
Why: the display name is how consoles, listings and correlation logic label an execution; a client that reads it (or an SDK model that expects the member on a describe) gets None and cannot render or match the execution it just started.
Fix: the DescribePipelineExecution serializer / the execution state that stores the execution's display name at StartPipelineExecution time (deriving one when the caller supplied none).
Traces: sagemaker-pipeline-execution-unknown-target-errors#task#6.

=== Cause 3 of 3: StartPipelineExecution does not replay a ClientRequestToken — it mints a new execution with an id of the wrong form ===
AWS: Smithy — StartPipelineExecution.ClientRequestToken is @required and @idempotencyToken (@length 32..128); output PipelineExecutionArn matches ^arn:aws[a-z\\-]*:sagemaker:[a-z0-9\\-]*:[0-9]{12}:pipeline\\/.*\\/execution\\/.*$ . Recorded behavior: in a token-replay fixture the second StartPipelineExecution answers 200 with the SAME PipelineExecutionArn the first call returned — an idempotency token that has already been used does not create a second execution — and the execution-id segment of that ARN is a short lowercase alphanumeric id (12 characters in the recording), not a hex digest.
Emulator: the replayed call answers 200 with a different ARN whose execution segment is a 32-character hex string, i.e. a freshly minted execution. Hypothesis: the handler ignores ClientRequestToken on StartPipelineExecution and generates a new execution id per request (the id's form is a second, independent symptom of the same minting path).
Why: idempotent retry is the documented contract of an @idempotencyToken member — a client that retries after a timeout silently starts a duplicate pipeline run, doubling work and leaving two executions where AWS has one. The id form also breaks clients that parse or display the execution id segment.
Fix: the StartPipelineExecution handler's idempotency path — key accepted executions by (pipeline, ClientRequestToken) and return the existing execution's ARN on replay; and mint execution ids in AWS's short lowercase-alphanumeric form.
Traces: sagemaker-pipeline-execution-token-replay-and-retry-refusals#task#3.

## Evidence
Replay of the wire corpus (trace set simparity-wire-aws-v1) against the booted emulator, verbatim from the divergence clusters:

Cause 1 — fixture sagemaker-pipeline-execution-token-replay-and-retry-refusals, step task#6, DescribePipelineExecution, expected 200 / actual 200:
  $.PipelineExecutionStatus: value_mismatch (expected 'Succeeded', got 'Failed')
  $.PipelineExecutionStatus: poll: 'Failed' is in fail_on after 2 attempt(s) (expected 'Succeeded', got 'Failed')
and fixture sagemaker-pipeline-execution-unknown-target-errors, step task#6, same operation:
  $.PipelineExecutionStatus: value_mismatch (expected 'Succeeded', got 'Failed')
  $.PipelineExecutionStatus: poll: 'Failed' is in fail_on after 2 attempt(s)

Cause 2 — fixture sagemaker-pipeline-execution-unknown-target-errors, step task#6:
  $.PipelineExecutionDisplayName: missing (expected '<shaped-not-asserted>', got None)
  (the recording asserts only that the member is present and well-shaped, not a particular name.)

Cause 3 — fixture sagemaker-pipeline-execution-token-replay-and-retry-refusals, step task#3, StartPipelineExecution, expected 200 / actual 200:
  $.PipelineExecutionArn: value_mismatch (expected 'arn:aws:sagemaker:us-west-2:000000000000:pipeline/<pipeline>/execution/ahkj444ui1zs', got 'arn:aws:sagemaker:us-west-2:000000000000:pipeline/<pipeline>/execution/59ba2479847e1cf0ed20dc0c9d379757')
  (the recorded id 'ahkj444ui1zs' is a per-run generated value — the requirements are that this replayed call returns the ARN its first call already returned, and that the execution-id segment has AWS's short lowercase-alphanumeric form rather than a 32-char hex digest. The preceding StartPipelineExecution in the same fixture reported no ARN divergence.)

The timestamp-shape mismatches that also appear in these two clusters ($.CreationTime, $.LastModifiedTime) are a separate root cause, filed in the sagemaker read-back-shape issue of this sweep.

## Steps to Reproduce
Against the booted emulator with boto3/botocore pointed at it (region us-west-2):

Cause 1: sm.create_pipeline(...) then sm.start_pipeline_execution(PipelineName=..., ClientRequestToken=<32+ chars>) and poll sm.describe_pipeline_execution(PipelineExecutionArn=...). Observe PipelineExecutionStatus settles 'Failed' instead of 'Succeeded' (fixtures sagemaker-pipeline-execution-token-replay-and-retry-refusals step task#6 and sagemaker-pipeline-execution-unknown-target-errors step task#6).

Cause 2: in the same describe response, observe no PipelineExecutionDisplayName member at all, even though StartPipelineExecution was accepted (fixture sagemaker-pipeline-execution-unknown-target-errors step task#6).

Cause 3: call sm.start_pipeline_execution twice for the same pipeline with the identical ClientRequestToken. Observe two different PipelineExecutionArns (so a second execution was created), the second execution id being a 32-character hex string rather than AWS's short lowercase-alphanumeric id (fixture sagemaker-pipeline-execution-token-replay-and-retry-refusals step task#3).
