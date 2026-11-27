Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__sagemaker__scheduler__sts.minimum-privilege-caller-trigger-starts-pipeline-with-parameter
Issue-Key: iam__sagemaker__scheduler__sts|ListPipelineExecutions|NotImplemented
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# Implement sagemaker ListPipelineExecutions: the operation is an unimplemented scaffold stub answering 501 NotImplemented, so a scheduler-triggered pipeline run is unobservable

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPipelineExecutions

## Description
Cause 1 of 1: ListPipelineExecutions is a scaffold stub; every call is refused 501 NotImplemented, so no pipeline execution can ever be found.

AWS: Smithy: sagemaker#ListPipelineExecutions is a bound, paginated read returning PipelineExecutionSummaries; it answers HTTP 200 and lists the executions of the named pipeline (empty list when there are none). It is the only way to discover an execution a trigger started, since the trigger itself returns no execution id to the caller.
Emulator: HTTP 501 with code `NotImplemented` and message "operation ListPipelineExecutions is scaffolded but not implemented in this emulator" — the handler at services/service-com-amazonaws-sagemaker-sagemaker/src/ops/list_pipeline_executions.rs returns `Scaffolded::not_implemented("ListPipelineExecutions")` from its `handle`.
Why: a client cannot discover, poll or verify ANY pipeline execution. A minimum-privilege EventBridge Scheduler trigger that starts a pipeline with a parameter produces nothing observable: the caller gets a 501 rather than an empty list or the run, so it cannot tell "the trigger never fired" from "the listing is unavailable", and no retry or waiter can ever converge. 501 is also the wrong class — the request is well-formed and the pipeline exists.
Fix: implement the operation in its handler file services/service-com-amazonaws-sagemaker-sagemaker/src/ops/list_pipeline_executions.rs — read the executions of the requested PipelineName from state, apply the modeled filters/sort and pagination, and return PipelineExecutionSummaries (an empty list when the pipeline has no executions).
Traces: iam__sagemaker__scheduler__sts.minimum-privilege-caller-trigger-starts-pipeline-with-parameter.

## Evidence
Expected (AWS): HTTP 200 with PipelineExecutionSummaries for the named pipeline (empty list before the trigger fires, then the triggered execution).

Actual (emulator), verbatim from the replay: 'botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListPipelineExecutions operation: operation ListPipelineExecutions is scaffolded but not implemented in this emulator'.

Handler source confirming the stub: services/service-com-amazonaws-sagemaker-sagemaker/src/ops/list_pipeline_executions.rs line 169 — `Err(crate::scaffolded::Scaffolded::not_implemented("ListPipelineExecutions"))`.

## Steps to Reproduce
1. Create a SageMaker pipeline and an EventBridge Scheduler schedule whose target starts it with a parameter, using the minimum caller policy from the SageMaker developer guide.
2. Call sagemaker ListPipelineExecutions(PipelineName=<pipeline>) to find the execution the schedule started.
3. Expect HTTP 200 with PipelineExecutionSummaries; observe HTTP 501 NotImplemented 'operation ListPipelineExecutions is scaffolded but not implemented in this emulator'.
