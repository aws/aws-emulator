Issue-Source: trace-sweep-divergence
Issue-Trace: sagemaker-automl-job-unknown-name-refusals
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sagemaker-automl-job-unknown-name-refusals sagemaker-training-session-stop-unknown-name-refused sagemaker-transform-job-unknown-name-errors sagemaker-pipeline-callback-token-unknown-and-overlong-refused sagemaker-pipeline-step-failure-unknown-callback-token-refused
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker|DescribeAutoMLJobV2|4xx->4xx|$.Message
Issue-Key: sagemaker|StopTrainingSession|4xx->4xx|$.Message
Issue-Key: sagemaker|DescribeTransformJob|4xx->4xx|$.__type
Issue-Key: sagemaker|DescribeTransformJob|4xx->4xx|$.message
Issue-Key: sagemaker|SendPipelineExecutionStepSuccess|4xx->4xx|$.Message
Issue-Key: sagemaker|SendPipelineExecutionStepSuccess|4xx->4xx|$.__type
Issue-Key: sagemaker|SendPipelineExecutionStepFailure|4xx->4xx|$.Message
Issue-Key: sagemaker|SendPipelineExecutionStepFailure|4xx->4xx|$.__type
Issue-Commit: 0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad

# sagemaker not-found and callback-token refusals are emulator-worded and two use the wrong error code

> _Note: this issue was identified against the code base at commit `0ee9b03ca15a3996ac8c0c71f0e70a3fe13706ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAutoMLJobV2, StopTrainingSession, DescribeTransformJob, SendPipelineExecutionStepSuccess, SendPipelineExecutionStepFailure

## Description
Five independent causes in the emulated SageMaker control plane's resolution-failure path: every one of these refusals carries an emulator-authored sentence instead of the message AWS words for that error class, and two of them also answer with the wrong error code. All five are grounded in wire-fixture steps replayed against the emulator; each must be closed before this issue is submitted.

=== Cause 1 of 5: DescribeAutoMLJobV2 for an unknown AutoML job name is refused with an emulator-authored message ===
AWS: Smithy — DescribeAutoMLJobV2 input AutoMLJobName (@required, @length 1..32, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,31}$); the only modeled error is ResourceNotFound (client, HTTP 400, member Message). Recorded behavior: HTTP 400 with Error.Code ResourceNotFound and Message "Amazon SageMaker can't find an AutoML job called <AutoMLJobName>." The fixed text is "Amazon SageMaker can't find an AutoML job called " + the trailing "."; the only variable part is the AutoMLJobName taken verbatim from the request.
Emulator: status 400 and code are right, but $.Message is 'Could not find AutoML job "<name>".' — different wording and the name is double-quoted, which AWS's sentence does not do.
Why: SageMaker reuses ResourceNotFound for many subjects and clients discriminate on the message; a client (or a console/CLI surface, or a log scraper) that matches AWS's sentence matches nothing and falls through to generic handling.
Fix: the not-found message construction in the DescribeAutoMLJobV2 handler (or the shared AutoML-job lookup helper that raises ResourceNotFound).
Traces: sagemaker-automl-job-unknown-name-refusals#task#2.

=== Cause 2 of 5: StopTrainingSession for an unknown training-session name is refused with an emulator-authored message ===
AWS: Smithy — StopTrainingSession input TrainingSessionName (@required, @length 1..63, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$); only modeled error ResourceNotFound (client, HTTP 400, member Message). Recorded behavior: HTTP 400, Error.Code ResourceNotFound, Message "Training session not found: <TrainingSessionName>" — fixed text "Training session not found: " followed by the requested name, no terminating period and no quoting.
Emulator: 'Could not find training session "<name>".'
Why: same discrimination-by-message problem as cause 1; a caller cannot tell this not-found apart from the emulator's other not-founds, which all share one invented phrasing template.
Fix: the not-found message construction in the StopTrainingSession handler / the training-session lookup that raises ResourceNotFound.
Traces: sagemaker-training-session-stop-unknown-name-refused#task#1.

=== Cause 3 of 5: DescribeTransformJob for an unknown transform job answers ResourceNotFound and names the ARN, where AWS answers ValidationException and names the job name ===
AWS: Smithy — DescribeTransformJob input TransformJobName (@required, @length 1..63, @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,62}$); the model lists only ResourceNotFound, so the code AWS actually returns here is recorded behavior only. Recorded behavior: HTTP 400 with Error.Code (__type) ValidationException and message "Could not find requested job with name: <TransformJobName>" — fixed text "Could not find requested job with name: " plus the name exactly as the request sent it.
Emulator: __type 'ResourceNotFound' and message 'Could not find transform job "arn:aws:sagemaker:<region>:<account>:transform-job/<name>".' — the wrong error code, and the subject rendered as a constructed ARN rather than the requested name.
Why: the error code is the first thing a client branches on; botocore/SDK users catching ValidationException (or a retry policy keyed on it) see an unexpected ResourceNotFound and take a different path than against AWS. Echoing an ARN the caller never supplied also leaks the emulator's own ARN synthesis into error text.
Fix: the DescribeTransformJob handler's lookup-failure branch — it must raise the ValidationException shape with AWS's sentence built from the request's TransformJobName, not the ARN-formatting not-found helper.
Traces: sagemaker-transform-job-unknown-name-errors#task#2.

=== Cause 4 of 5: SendPipelineExecutionStepSuccess with a callback token that names no waiting step answers ResourceNotFound instead of AWS's AccessDeniedException ===
AWS: Smithy — SendPipelineExecutionStepSuccess input CallbackToken (@required, @length 10..10, @pattern ^[a-zA-Z0-9]+$); modeled errors ConflictException, ResourceLimitExceeded, ResourceNotFound. The code AWS returns for an unknown token is recorded behavior only: HTTP 400 with __type AccessDeniedException and message "The provided callback token does not exist, or you are not authorized to call the SendPipelineExecutionStepSuccess operation on the pipeline execution associated with the token." The whole message is fixed text for this error class (it deliberately conflates not-found with not-authorized, so it embeds no request value) and it names the operation that was called.
Emulator: __type 'ResourceNotFound' and message 'The callback token does not name a pipeline execution step waiting for it.'
Why: AWS intentionally refuses an unknown callback token as an authorization failure so a caller cannot probe token existence; the emulator both discloses that the token is unknown and gives a code no AWS client's error handling expects, so token-driven callback workers branch wrongly (and an access-denied retry/credential-refresh path is never exercised).
Fix: the SendPipelineExecutionStepSuccess handler's token-resolution branch — the refusal must be the AccessDeniedException shape with AWS's fixed sentence naming this operation.
Traces: sagemaker-pipeline-callback-token-unknown-and-overlong-refused#task#1.

=== Cause 5 of 5: SendPipelineExecutionStepFailure with a callback token that names no waiting step answers ResourceNotFound instead of AWS's AccessDeniedException ===
AWS: Smithy — SendPipelineExecutionStepFailure input CallbackToken (@required, @length 10..10, @pattern ^[a-zA-Z0-9]+$), FailureReason (@length 0..256); modeled errors ConflictException, ResourceLimitExceeded, ResourceNotFound. Recorded behavior: HTTP 400, __type AccessDeniedException, message "The provided callback token does not exist, or you are not authorized to call the SendPipelineExecutionStepFailure operation on the pipeline execution associated with the token." — same fixed text as cause 4 with this operation's name substituted.
Emulator: __type 'ResourceNotFound' and message 'The callback token does not name a pipeline execution step waiting for it.'
Why: as cause 4; the failure-reporting half of the Callback step is the path a worker uses to abort a pipeline, so a wrongly-coded refusal there strands the execution.
Fix: the SendPipelineExecutionStepFailure handler's token-resolution branch (the same shared callback-token resolver as cause 4; both call sites must name their own operation in the message).
Traces: sagemaker-pipeline-callback-token-unknown-and-overlong-refused#task#2, sagemaker-pipeline-step-failure-unknown-callback-token-refused#task#1.

Related open issues that do NOT cover these: 20261127T123223Z-sagemaker-6-net-new-causes-runtime-free-condition.md covers the *spent* callback-token checks blocking on the SQS body, not the unknown-token refusal code/message; 20261117T022256Z-sagemakertrainingsessionruntime-resource-resolution-path-diverges-unknown-session-accepted.md is a different service crate (sagemakertrainingsessionruntime).

## Evidence
Replay of the wire corpus (trace set simparity-wire-aws-v1) against the booted emulator, verbatim from the divergence clusters:

Cause 1 — fixture sagemaker-automl-job-unknown-name-refusals, step task#2, DescribeAutoMLJobV2, expected 400 / actual 400:
  $.Message: value_mismatch (expected "Amazon SageMaker can't find an AutoML job called sim2real-probe-automl-ad7eedef.", got 'Could not find AutoML job "sim2real-probe-automl-ad7eedef".')
  (the job name is the per-run probe name supplied in the request; the requirement is the sentence template around it.)

Cause 2 — fixture sagemaker-training-session-stop-unknown-name-refused, step task#1, StopTrainingSession, expected 400 / actual 400:
  $.Message: value_mismatch (expected 'Training session not found: sim2real-probe-smts-f186731b', got 'Could not find training session "sim2real-probe-smts-f186731b".')

Cause 3 — fixture sagemaker-transform-job-unknown-name-errors, step task#2, DescribeTransformJob, expected 400 / actual 400:
  $.__type: value_mismatch (expected 'ValidationException', got 'ResourceNotFound')
  $.message: value_mismatch (expected 'Could not find requested job with name: sim2real-probe-smtj-9b1e3d98a0', got 'Could not find transform job "arn:aws:sagemaker:us-west-2:000000000000:transform-job/sim2real-probe-smtj-9b1e3d98a0".')

Cause 4 — fixture sagemaker-pipeline-callback-token-unknown-and-overlong-refused, step task#1, SendPipelineExecutionStepSuccess, expected 400 / actual 400:
  $.__type: value_mismatch (expected 'AccessDeniedException', got 'ResourceNotFound')
  $.Message: value_mismatch (expected 'The provided callback token does not exist, or you are not authorized to call the SendPipelineExecutionStepSuccess operation on the pipeline execution associated with the token.', got 'The callback token does not name a pipeline execution step waiting for it.')

Cause 5 — fixture sagemaker-pipeline-callback-token-unknown-and-overlong-refused, step task#2, SendPipelineExecutionStepFailure (cluster also covers sagemaker-pipeline-step-failure-unknown-callback-token-refused#task#1), expected 400 / actual 400:
  $.__type: value_mismatch (expected 'AccessDeniedException', got 'ResourceNotFound')
  $.Message: value_mismatch (expected 'The provided callback token does not exist, or you are not authorized to call the SendPipelineExecutionStepFailure operation on the pipeline execution associated with the token.', got 'The callback token does not name a pipeline execution step waiting for it.')

## Steps to Reproduce
Against the booted emulator with boto3/botocore pointed at it (region us-west-2):

Cause 1: sm.describe_auto_ml_job_v2(AutoMLJobName=<a name no AutoML job has>). Observe 400 ResourceNotFound with 'Could not find AutoML job "<name>".' instead of "Amazon SageMaker can't find an AutoML job called <name>." (replay fixture sagemaker-automl-job-unknown-name-refusals step task#2).

Cause 2: sm.stop_training_session(TrainingSessionName=<unknown name>). Observe 'Could not find training session "<name>".' instead of 'Training session not found: <name>' (fixture sagemaker-training-session-stop-unknown-name-refused step task#1).

Cause 3: sm.describe_transform_job(TransformJobName=<unknown name>). Observe __type ResourceNotFound and an ARN-bearing message instead of ValidationException / 'Could not find requested job with name: <name>' (fixture sagemaker-transform-job-unknown-name-errors step task#2).

Causes 4 and 5: sm.send_pipeline_execution_step_success(CallbackToken=<any 10 alphanumeric chars no waiting step holds>) and sm.send_pipeline_execution_step_failure(CallbackToken=<same>, FailureReason='x'). Observe __type ResourceNotFound and the emulator sentence instead of AccessDeniedException with AWS's fixed message naming the called operation (fixture sagemaker-pipeline-callback-token-unknown-and-overlong-refused steps task#1 and task#2).
