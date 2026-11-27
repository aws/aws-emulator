Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__sagemaker.sagemaker-batch-transform-reads-s3-input s3__sagemaker.sagemaker-batch-transform-writes-s3-output s3__sagemaker.sagemaker-batch-transform-missing-role-permission-negative s3__sagemaker.sagemaker-batch-transform-nonexistent-s3-input-negative
Issue-Key: s3__sagemaker.sagemaker-batch-transform-reads-s3-input|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Key: s3__sagemaker.sagemaker-batch-transform-writes-s3-output|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: s3__sagemaker.sagemaker-batch-transform-missing-role-permission-negative|_poll_until#22|Timeout|time.sleep(interval_s)
Issue-Key: s3__sagemaker.sagemaker-batch-transform-nonexistent-s3-input-negative|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Commit: def798b9c9a5a532a0598d5d900881d6a843c290

# s3__sagemaker: a batch transform job never settles — CreateTransformJob is accepted but DescribeTransformJob never reports a terminal TransformJobStatus, writes no <input>.out output and never refuses a missing S3 input

> _Note: this issue was identified against the code base at commit `def798b9c9a5a532a0598d5d900881d6a843c290`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateTransformJob

## Description
1 cause, grounded in 4 failing checks.

Cause 1 of 1: the batch-transform lifecycle never reaches a terminal state. `CreateTransformJob` is accepted, but the job neither runs (no output object under `TransformOutput.S3OutputPath`), nor fails for a role that cannot write, nor is the request refused for an S3 input prefix that holds nothing — the client polls `DescribeTransformJob` until the run budget expires.

AWS: Smithy (services/service-com-amazonaws-sagemaker-sagemaker/smithy-model/com-amazonaws-sagemaker-sagemaker.json) models `DescribeTransformJob` with required output members `TransformJobName`, `TransformJobArn`, `TransformJobStatus` (enum) and `TransformInput`/`TransformResources`, plus optional `FailureReason` (@length 0..1024), `TransformStartTime` and `TransformEndTime`; its only modeled error is `ResourceNotFound` (400). AWS drives `TransformJobStatus` from `InProgress` to one of the terminal values (`Completed`, `Failed`, `Stopped`) and sets `TransformEndTime`; a `Completed` job has written one output record per input record to `<input-object-name>.out` under `TransformOutput.S3OutputPath` and nothing under the input prefix; a job whose Model execution role lacks `s3:PutObject` on the output bucket settles `Failed` with a `FailureReason` and writes no output; and `CreateTransformJob` naming an `S3Prefix` input under which no object exists is refused at once with `ValidationException` and the missing-object message, creating no job (recorded behavior for the refusal class — the model declares no operation-specific error shape for it beyond the service-common set).

Emulator: every one of these four paths hangs. The job is created and `DescribeTransformJob` keeps answering a non-terminal status, so the polling loop is still running when the 60 s per-check cap fires: `Check timed out after 60.0s`. No output object appears, no `FailureReason` is ever set, and the nonexistent-input request is accepted instead of being refused with `ValidationException`.

Why: a client that submits a batch transform and waits for it — the only documented way to consume one — waits forever. Pipelines that gate on `Completed` never advance; pipelines that gate on `Failed`/`FailureReason` never see a failure, so an under-permissioned role looks indistinguishable from a healthy one; and a caller that passes an empty or mistyped input prefix is told nothing at submit time and later finds no output and no explanation. Nothing downstream of a transform job can be built against this emulator.

Fix: the sagemaker crate's transform-job execution and state — `CreateTransformJob` must validate `TransformInput.DataSource.S3DataSource.S3Uri` (refusing an `S3Prefix` with no objects as `ValidationException` before any job record is created) and the transform runner must advance the stored job to a terminal `TransformJobStatus`, writing `<input-object>.out` under `TransformOutput.S3OutputPath` on success and recording a `FailureReason` with status `Failed` when the Model execution role is denied `s3:PutObject` on the output bucket. Related open issues 20261029T211510Z-s3-sagemaker-sagemaker-jobs-never-execute-and-are-never-validated.md and 20261125T175728Z-s3-sagemaker-createtransformjob-against-an-empty-transforminput-s3-prefix-never.md describe the same surface; the replay suite does not report either as owning these four checks, which are filed here.

Traces: s3__sagemaker.sagemaker-batch-transform-reads-s3-input; s3__sagemaker.sagemaker-batch-transform-writes-s3-output; s3__sagemaker.sagemaker-batch-transform-missing-role-permission-negative; s3__sagemaker.sagemaker-batch-transform-nonexistent-s3-input-negative.

## Evidence
Expected (AWS):
  - a well-formed transform job reaches DescribeTransformJob.TransformJobStatus=Completed with TransformEndTime set, and one output row per input row at <input-filename>.out under TransformOutput.S3OutputPath (nothing under the input prefix or a sibling prefix);
  - a job whose Model execution role lacks s3:PutObject on the output bucket reaches TransformJobStatus=Failed with a FailureReason and writes no output;
  - CreateTransformJob with an S3Prefix input holding no object is refused immediately with ValidationException and the missing-object message, creating no job.

Actual (emulator) — all four checks are killed by the per-check cap while still polling DescribeTransformJob for a terminal status:
  [mismatch] s3__sagemaker.sagemaker-batch-transform-reads-s3-input        — Error: Check timed out after 60.0s
  [mismatch] s3__sagemaker.sagemaker-batch-transform-writes-s3-output      — Error: Check timed out after 60.0s
  [mismatch] s3__sagemaker.sagemaker-batch-transform-missing-role-permission-negative — Error: Check timed out after 60.0s
  [mismatch] s3__sagemaker.sagemaker-batch-transform-nonexistent-s3-input-negative    — Error: Check timed out after 60.0s

No terminal TransformJobStatus, no FailureReason, no <input>.out object and no ValidationException refusal is ever produced.

## Steps to Reproduce
1. CreateBucket; PutObject a small CSV under train/ and another under transform-input/.
2. Create a SageMaker execution role allowed s3:GetObject and s3:PutObject on the bucket, run a training job, and CreateModel from its artifact.
3. CreateTransformJob with TransformInput.DataSource.S3DataSource {S3DataType=S3Prefix, S3Uri=s3://<bucket>/transform-input/} and TransformOutput.S3OutputPath=s3://<bucket>/transform-output/.
4. Poll DescribeTransformJob: TransformJobStatus never becomes Completed/Failed/Stopped and no transform-output/<input>.out object appears.
5. Repeat with (a) a role lacking s3:PutObject on the output bucket — no Failed status and no FailureReason; (b) an S3Prefix input that holds no object — CreateTransformJob is accepted instead of refused with ValidationException.
6. Replay checks: s3__sagemaker.sagemaker-batch-transform-reads-s3-input, s3__sagemaker.sagemaker-batch-transform-writes-s3-output, s3__sagemaker.sagemaker-batch-transform-missing-role-permission-negative, s3__sagemaker.sagemaker-batch-transform-nonexistent-s3-input-negative
