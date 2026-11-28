Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker.retry-pipeline-execution-rejects-succeeded-execution sagemaker.stop-finished-pipeline-execution-rejected sagemaker.list-pipeline-executions-unissued-next-token-rejected sagemaker.pipeline-condition-step-outcome-selects-exactly-one-declared-branch
Issue-Shape-Id: com.amazonaws.sagemaker#SageMaker
Issue-Key: sagemaker.retry-pipeline-execution-rejects-succeeded-execution|run#16|AssertionError|assert code == "ValidationException", (
Issue-Key: sagemaker.stop-finished-pipeline-execution-rejected|run#17|AssertionError|assert re.fullmatch(quoted_shape, quoted), (
Issue-Key: sagemaker.list-pipeline-executions-unissued-next-token-rejected|run#17|AssertionError|assert (code, message) == ("ValidationException", "Invalid or expired next token"), (
Issue-Key: sagemaker.pipeline-condition-step-outcome-selects-exactly-one-declared-branch|run#24|KeyError|taken_if = true_steps[_IF_STEP]["Metadata"]["Fail"]["ErrorMessage"]
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# sagemaker: 4 net-new causes in the Pipelines execution plane — RetryPipelineExecution is an unimplemented 501 stub, the Stop/NextToken refusals are emulator-worded, and a Fail step's metadata omits ErrorMessage

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RetryPipelineExecution, StopPipelineExecution, ListPipelineExecutions, ListPipelineExecutionSteps

## Description
Four independent root causes in the emulated SageMaker Pipelines execution plane, all grounded in failing behavioral checks of the `sagemaker` service. Each must be closed before this issue is submitted.

=== Cause 1 of 4: RetryPipelineExecution is an unimplemented scaffold stub, so no execution can be retried and the succeeded-execution refusal never happens ===
AWS: Smithy — RetryPipelineExecution input PipelineExecutionArn (@required, PipelineExecutionArn, @length 0..2048, @pattern ^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:pipeline\/.*\/execution\/.*$) and ClientRequestToken (@required, @idempotencyToken, @length 32..128); output PipelineExecutionArn; modeled errors ConflictException, ResourceLimitExceeded, ResourceNotFound (all client, HTTP 400). The refusal class AWS returns for an execution that is not retryable is recorded behavior only: HTTP 400 with Error.Code `ValidationException` and a message whose fixed text is "Pipeline execution with ARN " + <the PipelineExecutionArn from the request> + " status '" + <that execution's current status, from state> + "'. Only pipelines with 'Failed', 'Stopped', or 'CompletedWithFailures' status can be retried." — i.e. everything outside the ARN and the status word is fixed text; the ARN and the status are taken from the request and the emulator's state, never hard-coded. On a retryable (Failed) execution the call must be ACCEPTED, answer that same execution's ARN, and re-run the failed step (a second step run starting after the first one ended).
Emulator: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/retry_pipeline_execution.rs is still the generated scaffold (its module doc carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and `handle` ends in `Err(crate::scaffolded::Scaffolded::not_implemented("RetryPipelineExecution"))`), so every call answers HTTP 501 with error code `NotImplemented`. The check sees code 'NotImplemented' where AWS answers 'ValidationException'.
Why: a client that retries a failed pipeline execution — the normal recovery path for Pipelines — gets a 501 that no SDK error handler models, so neither the retry nor its refusal is reachable; a caller cannot distinguish "not retryable" from "service broken", and the idempotent retry/resume flow cannot be exercised at all.
Fix: implement the operation in its handler services/service-com-amazonaws-sagemaker-sagemaker/src/ops/retry_pipeline_execution.rs (status gate + re-run of the failed step in the pipeline-execution state), and delete the SCAFFOLDED marker line.
Traces: sagemaker.retry-pipeline-execution-rejects-succeeded-execution.

=== Cause 2 of 4: StopPipelineExecution's refusal for an already-terminal execution is worded by the emulator instead of AWS's sentence ===
AWS: Smithy — StopPipelineExecution input PipelineExecutionArn (@required) and ClientRequestToken (@required, @idempotencyToken, @length 32..128); modeled errors ConflictException, ResourceNotFound. The refusal for a non-Executing execution is recorded behavior only: HTTP 400, Error.Code `ValidationException`, message whose fixed text is "Pipeline execution with ARN " + <the execution ARN> + " status '" + <the execution's current status> + "'. Only pipelines with 'Executing' status can be stopped." The ARN comes from the request and the status word from state; the rest is fixed text for this error class.
Emulator: the same call is refused with the emulator-authored sentence "Pipeline execution <arn> is <Status>, so it is not executing and cannot be stopped." — the ARN is not introduced by "with ARN ", the status is not quoted, and the clause naming the only stoppable status is absent, so a client matching AWS's sentence (or extracting the quoted status / the ARN after the fixed prefix) extracts nothing.
Why: Pipelines callers branch on this refusal's text to decide whether to stop waiting or keep polling; AWS's wording is the contract and is also what console/CLI surfaces and log scrapers key on. A reworded refusal makes a stop-then-settle loop misclassify a terminal execution.
Fix: the not-stoppable branch of services/service-com-amazonaws-sagemaker-sagemaker/src/ops/stop_pipeline_execution.rs (the status gate that raises the ValidationException), which must build AWS's sentence from the request's ARN and the stored status.
Traces: sagemaker.stop-finished-pipeline-execution-rejected.

=== Cause 3 of 4: ListPipelineExecutions rejects an unissued NextToken with an emulator-authored message ===
AWS: Smithy — ListPipelineExecutions is @paginated (inputToken/outputToken NextToken, items PipelineExecutionSummaries, pageSize MaxResults); NextToken is @length 0..8192, @pattern .*; the only modeled error is ResourceNotFound. The refusal for a token the service never issued is recorded behavior only: HTTP 400, Error.Code `ValidationException`, message exactly "Invalid or expired next token" — the whole message is fixed text for this error class (it embeds no request value, so nothing in it varies per call).
Emulator: the code is right but the message is 'NextToken does not name a page boundary of this result set; restart the listing without it.'
Why: every pagination-resume helper that distinguishes a stale/forged cursor from any other ValidationException matches on AWS's fixed sentence; against the emulator it matches nothing and the client treats a bad cursor as a generic validation failure (or retries it forever).
Fix: the NextToken decode/validation branch of services/service-com-amazonaws-sagemaker-sagemaker/src/ops/list_pipeline_executions.rs (or the shared page-cursor validator it calls, which must then word every SageMaker listing's refusal the same way AWS does for that listing).
Traces: sagemaker.list-pipeline-executions-unissued-next-token-rejected.

=== Cause 4 of 4: a Fail step's PipelineExecutionStep.Metadata.Fail carries no ErrorMessage, so the step's resolved message is never observable ===
AWS: Smithy — ListPipelineExecutionSteps returns PipelineExecutionSteps with Metadata (PipelineExecutionStepMetadata). For a Fail step that member is `Fail` (FailStepMetadata) whose single member is `ErrorMessage` (String3072, @length 0..3072), documented as "A message that you define and then is processed and rendered by the Fail step when the error occurs." So for a Fail step that ran, Metadata.Fail.ErrorMessage is present and holds the step's RESOLVED ErrorMessage — a declaration of {"Get": "Parameters.X"} must be rendered as the value that parameter resolved to for this execution, not left unresolved and not omitted.
Emulator: the step is listed with the right StepName/StepStatus/timestamps and a `Fail` metadata object, but that object has no `ErrorMessage` key at all (the check dies on KeyError: 'ErrorMessage' reading Metadata['Fail']['ErrorMessage']), so the rendered message of a Fail step — and with it the proof that the executor resolved the step's parameter references — is unavailable for every execution.
Why: a Fail step exists precisely to surface an author-defined message; a client (or operator UI) reading why an execution failed finds no message, and a pipeline whose two branches declare different Fail messages is indistinguishable, so parameter resolution inside the taken branch cannot be verified.
Fix: the Fail-step execution/metadata projection — the pipeline executor's Fail-step handling in the sagemaker crate's pipeline-execution state, serialized through ListPipelineExecutionSteps (services/service-com-amazonaws-sagemaker-sagemaker/src/ops/list_pipeline_execution_steps.rs): it must store and emit FailStepMetadata.ErrorMessage with the resolved text.
Traces: sagemaker.pipeline-condition-step-outcome-selects-exactly-one-declared-branch.

Related open issues that do NOT cover these four causes: 20261127T165701Z-sagemaker-not-found-and-callback-token-refusals-are.md (unknown-callback-token and unknown-job not-found refusals on other operations), 20261127T165701Z-sagemaker-pipeline-executions-always-end-failed-omit-pipelineexecutiondisplayname.md and 20261127T123223Z-sagemaker-6-net-new-causes-runtime-free-condition.md (executions ending Failed / Search / cluster refusals — not the Fail-step metadata member, the Stop or NextToken wording, or the RetryPipelineExecution stub), and 20261127T121249Z-iam-sagemaker-scheduler-sts-implement-sagemaker-listpipelineexecutions-...md (ListPipelineExecutions existing at all, which it now does).

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator (service heading `### sagemaker`), verbatim from the failing checks:

Cause 1 — check sagemaker.retry-pipeline-execution-rejects-succeeded-execution [mismatch]:
  AssertionError: retrying a succeeded execution must be refused with ValidationException; got 'NotImplemented'
  (the check calls sm.retry_pipeline_execution(PipelineExecutionArn=<a Succeeded execution>, ClientRequestToken=<32+ chars>) and then expects the refusal's fixed head "Pipeline execution with ARN " and tail " status 'Succeeded'. Only pipelines with 'Failed', 'Stopped', or 'CompletedWithFailures' status can be retried." around the execution ARN; it also expects the same call on the Failed execution of the same pipeline to be ACCEPTED and to re-run the failed step.)
  Handler evidence: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/retry_pipeline_execution.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented`) and line 164 (`Err(crate::scaffolded::Scaffolded::not_implemented("RetryPipelineExecution"))`).

Cause 2 — check sagemaker.stop-finished-pipeline-execution-rejected [mismatch]:
  AssertionError: the refusal must quote an execution ARN (^arn:aws:sagemaker:us\-west\-2:[0-9]{12}:pipeline/[A-Za-z0-9-]+/execution/[a-z0-9]+$); got message 'Pipeline execution arn:aws:sagemaker:us-west-2:000000000000:pipeline/sim2real-behav-smstop-52ea3e634e0d/execution/847e9f93b6d3ec5e7e8300ec12cb7dfd is Succeeded, so it is not executing and cannot be stopped.'
  Expected (code, message) = ('ValidationException', "Pipeline execution with ARN <that execution's ARN> status 'Succeeded'. Only pipelines with 'Executing' status can be stopped."). The pipeline name, account, region and execution id above are this run's values that fill the template; the requirement is the sentence around them. The same check's positive control (stopping an Executing execution) passes, so only the refusal's wording diverges.

Cause 3 — check sagemaker.list-pipeline-executions-unissued-next-token-rejected [mismatch]:
  AssertionError: a NextToken the service never issued gave ('ValidationException', 'NextToken does not name a page boundary of this result set; restart the listing without it.')
  Expected ('ValidationException', 'Invalid or expired next token'). The forged token used is a well-formed string the service never issued.

Cause 4 — check sagemaker.pipeline-condition-step-outcome-selects-exactly-one-declared-branch [error]:
  KeyError: 'ErrorMessage' at `taken_if = true_steps['TookIfBranch']['Metadata']['Fail']['ErrorMessage']`
  Everything before it passed: Metadata.Condition.Outcome is 'True'/'False' for the two executions, only the taken branch's step is listed, the condition step is Succeeded and the branch leaf Failed with ordered timestamps. Only FailStepMetadata.ErrorMessage is missing, so the resolved per-branch marker value (the check passes distinct values via Parameters.IfMarker / Parameters.ElseMarker) is unobservable.

## Steps to Reproduce
Against the booted emulator with boto3 pointed at it (region us-west-2), after creating a SageMaker execution role and a pipeline:

Cause 1: start an execution and let it reach Succeeded, then sm.retry_pipeline_execution(PipelineExecutionArn=<that ARN>, ClientRequestToken='a'*32). Observe HTTP 501 / Error.Code 'NotImplemented' instead of HTTP 400 ValidationException with "Pipeline execution with ARN <arn> status 'Succeeded'. Only pipelines with 'Failed', 'Stopped', or 'CompletedWithFailures' status can be retried." Also retry a Failed execution of the same pipeline: it must be accepted, answer that execution's ARN and re-run the failed step. (check sagemaker.retry-pipeline-execution-rejects-succeeded-execution)

Cause 2: let an execution reach Succeeded, then sm.stop_pipeline_execution(PipelineExecutionArn=<that ARN>). Observe ValidationException with 'Pipeline execution <arn> is Succeeded, so it is not executing and cannot be stopped.' instead of "Pipeline execution with ARN <arn> status 'Succeeded'. Only pipelines with 'Executing' status can be stopped." (check sagemaker.stop-finished-pipeline-execution-rejected)

Cause 3: sm.list_pipeline_executions(PipelineName=<a pipeline>, NextToken='c2ltMnJlYWwtZm9yZ2VkLXRva2Vu'). Observe the message 'NextToken does not name a page boundary of this result set; restart the listing without it.' instead of 'Invalid or expired next token'. (check sagemaker.list-pipeline-executions-unissued-next-token-rejected)

Cause 4: create a pipeline whose Condition step has one Fail step in IfSteps and another in ElseSteps, each with ErrorMessage {"Get": "Parameters.<Marker>"}; start it, wait for Failed, then sm.list_pipeline_execution_steps(PipelineExecutionArn=<arn>) and read the Fail step's ['Metadata']['Fail']. Observe no 'ErrorMessage' key instead of the resolved marker value. (check sagemaker.pipeline-condition-step-outcome-selects-exactly-one-declared-branch)
