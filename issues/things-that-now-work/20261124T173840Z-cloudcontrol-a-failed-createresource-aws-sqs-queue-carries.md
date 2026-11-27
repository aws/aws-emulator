Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource
Issue-Shape-Id: com.amazonaws.cloudcontrol#CloudApiService
Issue-Key: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource|run#22|AssertionError|assert ".fifo" in message and "FIFO" in message, (
Issue-Commit: f8a8db808fb2efa4ac1d60fcd59e02b605ad9cca

# cloudcontrol: a FAILED CreateResource(AWS::SQS::Queue) carries the generic placeholder StatusMessage "Error occurred during operation 'CreateQueue'." instead of the downstream handler's FIFO-naming diagnosis

> _Note: this issue was identified against the code base at commit `f8a8db808fb2efa4ac1d60fcd59e02b605ad9cca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceRequestStatus

## Description
Cause 1 of 1: when a Cloud Control CreateResource is schema-valid at the front door but refused by the downstream resource handler's actual service call, the terminal ProgressEvent's StatusMessage is a generic, content-free placeholder naming only the downstream API call, instead of the diagnosis the handler surfaces for that refusal. Failing check: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource.

AWS: Smithy (services/service-com-amazonaws-cloudcontrol-cloudapiservice) models ProgressEvent.StatusMessage as StatusMessage, a string with @length(min=0,max=2048) and pattern ^[\s\S]*$, documented "Any message explaining the current status"; ProgressEvent.ErrorCode is HandlerErrorCode (enum including GeneralServiceException), documented "For requests with a status of FAILED, the associated error code." The model does not fix the message text, so the requirement here is recorded behavior only. Recorded behavior: a CreateResource of TypeName AWS::SQS::Queue with DesiredState {"QueueName": <name without a .fifo suffix>, "FifoQueue": true} passes the registry schema (the .fifo cross-field rule is not expressible in the AWS::SQS::Queue JSON schema), is handed to the SQS resource handler, and is refused only when the handler calls SQS CreateQueue. The request reaches OperationStatus FAILED with ErrorCode GeneralServiceException (SQS's FIFO-naming refusal is a plain HTTP 400 with no typed exception, so the handler buckets it under the catch-all), and the StatusMessage relays the downstream FIFO-naming diagnosis: it states the .fifo-suffix requirement for a FIFO queue name — its fixed text is SQS's FIFO queue-name rule wording (a FIFO queue's name must end with the .fifo suffix, may contain only alphanumeric characters, hyphens or underscores, and be 1 to 80 characters long), and its only variable parts are values from the request and the downstream call: the offending queue name and the downstream service/status/Request ID suffix that Cloud Control appends (the check deliberately does not pin those, asserting only that both ".fifo" and "FIFO" appear in the message). Smithy: SQS CreateQueue documents "To successfully create a new queue, you must provide a queue name that adheres to the limits related to queues" and that FifoQueue selects a FIFO queue; the queue-name rule itself is AWS-documented rather than modeled.

Emulator: the create does reach OperationStatus FAILED and does carry ErrorCode "GeneralServiceException" (both of those assertions pass), but GetResourceRequestStatus returns StatusMessage "Error occurred during operation 'CreateQueue'." — a fixed template whose only variable part is the downstream operation name, carrying no diagnosis at all. Neither ".fifo" nor "FIFO" appears, so nothing in the response says why the create failed. The emulator is discarding the downstream refusal's message when it records the terminal progress event and substituting a generic "Error occurred during operation '<Op>'." placeholder.

Why: ProgressEvent.StatusMessage is the only place Cloud Control reports WHY an asynchronous request failed — ErrorCode GeneralServiceException is the catch-all and carries no information. A client that polls GetResourceRequestStatus and reads StatusMessage to classify, log or display the failure (CloudFormation-CLI contract tests, IaC tooling that surfaces the handler message to an operator, retry logic that distinguishes a malformed request from a transient service error) gets a message with no diagnostic content and cannot tell a FIFO-naming violation from any other CreateQueue failure. Operators debugging against the emulator are told only that "an error occurred", which real AWS never does here.

Fix: the AWS::SQS::Queue resource handler's create path in the cloudcontrol service — specifically the step that converts a downstream service refusal into the terminal ProgressEvent. It must carry the downstream refusal's own message through onto StatusMessage (the handler's diagnosis for the refusal), rather than emitting a generic "Error occurred during operation '<Op>'." placeholder. The FAILED / GeneralServiceException classification already produced is correct and should not change, and no partial resource must be left behind (the later GetResource ResourceNotFoundException and native SQS AWS.SimpleQueueService.NonExistentQueue steps of the check were never reached and must also pass).

Not covered by any open issue: 20261116T000321Z-cloudcontrol-a-downstream-handler-rejected-createresource-reports-the.md is the opposite defect on a different resource type — there the emulator relays DynamoDB's raw ValidationException text verbatim where AWS wants the handler's own sentence; here the emulator relays nothing at all and substitutes a placeholder. 20261121T195413Z-cloudcontrol-3-net-new-causes-downstream-handler-input.md concerns the ErrorCode CLASSIFICATION of downstream input refusals (GeneralServiceException where AWS answers InvalidRequest); in this check GeneralServiceException is the CORRECT code and passes, and the divergence is purely the StatusMessage content. 20261123T182309Z (RoleArn/permission classification) and 20261121T121249Z / 20261123T055036Z (SQS QueueUrl keying and UpdateResource patches) concern different operations and fields.

Traces: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource (asserts at the StatusMessage content check; the positive-control FIFO create, the OperationStatus==FAILED and the ErrorCode==GeneralServiceException assertions all pass before it).

## Evidence
Check: cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource
Verdict: mismatch

Request: CreateResource(TypeName="AWS::SQS::Queue", DesiredState={"QueueName": "<name with NO .fifo suffix>", "FifoQueue": true})
Then: GetResourceRequestStatus(RequestToken=<token from CreateResource.ProgressEvent.RequestToken>) polled every 2s to a terminal OperationStatus.

Agreeing fields (emulator correct):
  ProgressEvent.OperationStatus == "FAILED"
  ProgressEvent.ErrorCode == "GeneralServiceException"
  (and the positive control — the same create with a ".fifo" name — reaches SUCCESS and resolves via native SQS GetQueueUrl)

Diverging field:
  Expected ProgressEvent.StatusMessage: the downstream SQS FIFO-naming diagnosis, i.e. SQS's fixed FIFO queue-name rule wording stating that a FIFO queue's name must end with the ".fifo" suffix (and may contain only alphanumeric characters, hyphens or underscores, 1 to 80 characters), with the only variable parts being the request's queue name and the "(Service: ..., Status Code: 400, Request ID: ...)" suffix Cloud Control appends. The check pins only the invariant content: the message must contain both ".fifo" and "FIFO".
  Actual ProgressEvent.StatusMessage: "Error occurred during operation 'CreateQueue'."

Fixed vs variable in the actual message: "Error occurred during operation '...'." is the emulator's own fixed placeholder template and "CreateQueue" is the downstream operation name it interpolates. It contains no part of the required diagnosis — no rule text, no queue name, no .fifo mention. Nothing in it is a per-run value, so this is not a request-id/timestamp difference: the whole sentence is the wrong text.

Assertion raised by the runner:
  File "check_cloudcontrol_server_side_rollback_on_create_failure_no_partial_resource.py", line 67, in run
    assert ".fifo" in message and "FIFO" in message, (
  AssertionError: the StatusMessage does not carry the downstream handler's own FIFO-suffix diagnosis: "Error occurred during operation 'CreateQueue'."

Model citation: Smithy com.amazonaws.cloudcontrol#ProgressEvent.StatusMessage -> StatusMessage, @length(min=0,max=2048), pattern ^[\s\S]*$, doc "Any message explaining the current status"; ProgressEvent.ErrorCode -> HandlerErrorCode, values include GeneralServiceException. The message text is not modeled — recorded behavior only.

## Steps to Reproduce
1. Boot the emulator and create cloudcontrol, sqs and sts clients.
2. Positive control (must pass first): CreateResource(TypeName="AWS::SQS::Queue", DesiredState={"QueueName": "<unique-name>.fifo", "FifoQueue": true}); poll GetResourceRequestStatus to SUCCESS and confirm sqs.get_queue_url(QueueName="<unique-name>.fifo") resolves.
3. Call CreateResource(TypeName="AWS::SQS::Queue", DesiredState={"QueueName": "<unique-name-with-no-.fifo-suffix>", "FifoQueue": true}). This DesiredState is valid against the AWS::SQS::Queue registry schema, so Cloud Control must accept the request synchronously and fail it asynchronously in the handler.
4. Take ProgressEvent.RequestToken from the response and poll GetResourceRequestStatus(RequestToken=...) until OperationStatus is terminal.
5. Observe OperationStatus == "FAILED" and ErrorCode == "GeneralServiceException" (both correct).
6. Read ProgressEvent.StatusMessage. Expected the downstream SQS FIFO-naming diagnosis — a message stating that a FIFO queue name must end with the ".fifo" suffix (so containing both ".fifo" and "FIFO"); observed the placeholder "Error occurred during operation 'CreateQueue'."
7. Re-replay cloudcontrol.server-side-rollback-on-create-failure-no-partial-resource to confirm the fix; the check continues past the StatusMessage assertion into GetResource(Identifier=<would-be queue url>) expecting ResourceNotFoundException and a native sqs.get_queue_url expecting AWS.SimpleQueueService.NonExistentQueue with message "The specified queue does not exist.", so those later no-partial-resource steps must also pass.
