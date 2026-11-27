Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker__sqs.callback-token-answered-once-is-rejected-for-a-second-answer sagemaker__sqs.stop-with-running-callback-step-queues-a-stopping-message-and-waits-for-the-token sagemaker__sqs.stopped-execution-answered-with-step-failure-ends-failed-with-the-consumer-reason
Issue-Key: sagemaker__sqs.callback-token-answered-once-is-rejected-for-a-second-answer|run#14|AssertionError|assert first.get("arguments") == {"ticket": ticket}, (
Issue-Key: sagemaker__sqs.stop-with-running-callback-step-queues-a-stopping-message-and-waits-for-the-token|run#14|AssertionError|assert first.get("pipelineExecutionArn") == execution_arn, (
Issue-Key: sagemaker__sqs.stopped-execution-answered-with-step-failure-ends-failed-with-the-consumer-reason|run#17|AssertionError|assert (stopping.get("status"), stopping.get("token")) == ("Stopping", token), (
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# sagemaker__sqs: the Callback step's SQS message body is malformed — Arguments are enqueued unresolved as {"Get": ...}, pipelineExecutionArn is absent, and the stop notification uses Status instead of status (3 causes)

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartPipelineExecution, StopPipelineExecution

## Description
Three independent root causes in the JSON body the emulated SageMaker Pipelines Callback step posts to its `SqsQueueUrl`, all grounded in failing behavioral checks the suite report lists under `### sagemaker__sqs`. They are numbered; each must be closed before this issue is submitted.

=== Cause 1 of 3: the callback message carries the step's Arguments unresolved, as the literal {"Get": "Parameters.<name>"} expression ===
AWS: recorded behavior only for the message body (Smithy models the step metadata, `CallbackStepMetadata` {CallbackToken length 10 pattern ^[a-zA-Z0-9]+$, SqsQueueUrl, OutputParameters}, not the SQS payload). A pipeline definition's `{"Get": "Parameters.<name>"}` is a pipeline-variable reference that the service resolves when the execution runs, from the execution's `PipelineParameters` (falling back to the parameter's `DefaultValue`). Recorded behavior: for a definition declaring Parameters [{Name:"Ticket", Type:"String", DefaultValue:"none"}] and a Callback step with Arguments {"ticket": {"Get": "Parameters.Ticket"}}, an execution started with PipelineParameters [{Name:"Ticket", Value:<v>}] queues a body whose `arguments` is {"ticket": <v>} — the resolved scalar, with no `Get` wrapper anywhere.
Emulator: `arguments` is the definition text verbatim: {"ticket": {"Get": "Parameters.Ticket"}}. The same unresolved expression appears in the stop-path message ({"export": {"Get": "Parameters.Export"}}).
Why: the consumer of a Callback step is an external worker that acts on `arguments` — an S3 URI, a ticket id, a destination. It receives a JSON object describing a pipeline expression instead of the value, so it either does the wrong work or fails to parse, and parameterised pipelines cannot be driven at all.
Fix: the Callback-step dispatcher in the pipeline-execution runner — the step's `Arguments` must be passed through the execution's pipeline-variable resolution (Parameters, and the same `Get` resolution used elsewhere) before the body is serialized and enqueued.
Traces: sagemaker__sqs.callback-token-answered-once-is-rejected-for-a-second-answer, sagemaker__sqs.stopped-execution-answered-with-step-failure-ends-failed-with-the-consumer-reason.

=== Cause 2 of 3: the callback message has no pipelineExecutionArn member ===
AWS: recorded behavior only. The body SageMaker posts for a Callback step carries lower-camel-case keys including `token` (the 10-character callback token) and `pipelineExecutionArn` — the ARN of the execution the waiting step belongs to, matching the `PipelineExecutionArn` that StartPipelineExecution returned and that SendPipelineExecutionStepSuccess/Failure echo back.
Emulator: the member is absent; the observed body is {'token': <10 chars>, 'arguments': {...}} (plus a status member on a stop), so reading `pipelineExecutionArn` yields None.
Why: one queue serves many executions, so a worker correlates by this ARN: without it, it cannot look up the execution it is answering for, cannot ignore messages belonging to other executions, and cannot report progress against the right run. Every consumer that filters its queue on the execution ARN sees no messages at all.
Fix: the same callback-message serializer in the pipeline-execution runner — include `pipelineExecutionArn` for the execution that owns the step. (The identical defect is also the cause of two timed-out checks filed under the `sagemaker` heading.)
Traces: sagemaker__sqs.stop-with-running-callback-step-queues-a-stopping-message-and-waits-for-the-token.

=== Cause 3 of 3: the stop notification names its status with the key `Status` instead of `status` ===
AWS: recorded behavior only. When an execution with a Callback step still waiting is stopped, SageMaker posts a second message for the SAME `token`, telling the consumer to wind down; its status member uses the body's lower-camel-case convention — `"status": "Stopping"` — consistent with `token`, `arguments` and `pipelineExecutionArn` in the same body. The execution stays in `Stopping` until that token is answered.
Emulator: the queued stop body is {'token': '<10 chars>', 'arguments': {...}, 'Status': 'Stopping'} — the value is right, the key is capitalized, so `body["status"]` is missing. (The same body also shows Causes 1 and 2.)
Why: JSON keys are case-sensitive, so a consumer written against AWS's body never sees the stop signal: it keeps waiting on a token whose execution is already stopping, and the stop never completes. A capitalized key in an otherwise lower-camel-case body is indistinguishable to a client from no signal at all.
Fix: the key naming in the callback stop-notification serializer (the stop path of the Callback-step dispatcher), which must emit `status` in the body's lower-camel-case convention.
Traces: sagemaker__sqs.stopped-execution-answered-with-step-failure-ends-failed-with-the-consumer-reason.

## Evidence
Replay output, verbatim from the behavioral suite report (all under `### sagemaker__sqs`):

Cause 1 — [mismatch] sagemaker__sqs.callback-token-answered-once-is-rejected-for-a-second-answer:
  assert first.get("arguments") == {"ticket": ticket}
  AssertionError: the callback message did not carry the resolved step arguments: {'ticket': {'Get': 'Parameters.Ticket'}}
The definition was {"Version":"2020-12-01","Parameters":[{"Name":"Ticket","Type":"String","DefaultValue":"none"}],"Steps":[{"Name":"AwaitApproval","Type":"Callback","SqsQueueUrl":<url>,"Arguments":{"ticket":{"Get":"Parameters.Ticket"}},"OutputParameters":[{"OutputName":"approver","OutputType":"String"}]}]} and the execution was started with PipelineParameters=[{"Name":"Ticket","Value":<generated ticket>}]. Expected arguments: {"ticket": <that parameter's value>} (the value itself is per-run; the requirement is that no `Get` wrapper survives).

Cause 2 — [mismatch] sagemaker__sqs.stop-with-running-callback-step-queues-a-stopping-message-and-waits-for-the-token:
  assert first.get("pipelineExecutionArn") == execution_arn
  AssertionError: the callback message names another execution: None
Expected: the body's `pipelineExecutionArn` equals the PipelineExecutionArn StartPipelineExecution returned.

Cause 3 — [mismatch] sagemaker__sqs.stopped-execution-answered-with-step-failure-ends-failed-with-the-consumer-reason:
  assert (stopping.get("status"), stopping.get("token")) == ("Stopping", token)
  AssertionError: the stop did not queue a Stopping message for the same token: {'token': '70dada7617', 'arguments': {'export': {'Get': 'Parameters.Export'}}, 'Status': 'Stopping'}
The token value shown is per-run. Expected body: lower-camel-case `status` set to "Stopping", `token` equal to the token of the still-waiting step, `arguments` resolved (Cause 1) and `pipelineExecutionArn` present (Cause 2).

## Steps to Reproduce
Against the booted emulator with boto3/botocore pointed at it:

1. sqs.create_queue, read its QueueArn, and iam.create_role with a trust policy for sagemaker.amazonaws.com plus an inline allow of sqs:SendMessage on that queue ARN.
2. sagemaker.create_pipeline with RoleArn=<that role> and definition {"Version":"2020-12-01","Parameters":[{"Name":"Ticket","Type":"String","DefaultValue":"none"}],"Steps":[{"Name":"AwaitApproval","Type":"Callback","SqsQueueUrl":<queue url>,"Arguments":{"ticket":{"Get":"Parameters.Ticket"}},"OutputParameters":[{"OutputName":"approver","OutputType":"String"}]}]}.
3. sagemaker.start_pipeline_execution(PipelineName=..., PipelineParameters=[{"Name":"Ticket","Value":"T-1"}]); keep the returned PipelineExecutionArn.
4. sqs.receive_message and json.loads the Body. Observe `arguments` == {"ticket": {"Get": "Parameters.Ticket"}} instead of {"ticket": "T-1"} (Cause 1) and no `pipelineExecutionArn` member (Cause 2).
5. With the step still waiting, sagemaker.stop_pipeline_execution(PipelineExecutionArn=<that ARN>) and receive again. Observe the second body for the same token uses the key `Status` ("Stopping") instead of `status` (Cause 3).
