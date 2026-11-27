Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__transfer.workflow-naming-an-absent-log-group-as-structured-log-destination-rejected
Issue-Key: logs__transfer.workflow-naming-an-absent-log-group-as-structured-log-destination-rejected|_refusal#7|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {got!r}")
Issue-Commit: 7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08

# CreateWorkflow accepts a StructuredLogDestinations log group that does not exist (200 + WorkflowId) instead of refusing with InvalidRequestException

> _Note: this issue was identified against the code base at commit `7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
Cause 1 of 1: Transfer CreateWorkflow does not pre-validate the CloudWatch Logs log group named in StructuredLogDestinations, so a workflow that logs to a nonexistent log group is created.

AWS: CreateWorkflow resolves every ARN in StructuredLogDestinations against CloudWatch Logs before creating anything. Smithy: CreateWorkflowRequest.StructuredLogDestinations is a list (@length 0..1) of log-group ARNs — its documentation requires "the ARN for an existing log group" in the form arn:partition:logs:region-name:amazon-account-id:log-group:log-group-name:*; CreateWorkflow models InvalidRequestException (client, HTTP 400, required Message), which is "thrown when the client submits a malformed request". Recorded behavior: the request naming an absent log group is refused with HTTP 400, Error.Code 'InvalidRequestException' and Error.Message 'Unable to create logging. Log destination resource <log-group-arn> was not found', and no workflow is created (ListWorkflows shows nothing carrying that Description). The fixed text of that message is 'Unable to create logging. Log destination resource ' + <value> + ' was not found'; the only variable part is the log-group resource taken verbatim from the request's StructuredLogDestinations entry — never a literal from one run. Once the log group is created, the byte-identical request succeeds, returns a WorkflowId matching ^w-([a-z0-9]{17})$, and DescribeWorkflow reads StructuredLogDestinations back unchanged.

Emulator: the refusal never happens. The first CreateWorkflow — with StructuredLogDestinations pointing at a log group that was never created — answers HTTP 200 with a WorkflowId ('w-...'), so the check fails at its first assertion ('expected a refusal, the call succeeded'). The destination is stored unvalidated; no reference check against the logs service's log-group state is performed.

Why: a client relying on AWS's behavior treats CreateWorkflow success as proof that workflow logging is wired up. Against the emulator a typo'd or not-yet-created log group is accepted, so the negative path (catch InvalidRequestException, create the log group, retry) is never exercised and a provisioning bug that AWS rejects at create time silently ships. It also leaves a workflow behind where AWS leaves none, so cleanup/idempotency logic diverges.

Fix: CreateWorkflow's handler/validation in the Transfer service — add a pre-flight existence check for each StructuredLogDestinations ARN against the CloudWatch Logs log-group state (parse the arn:...:log-group:<name>:* form, look the name up in the logs state), and fail the whole request with InvalidRequestException (HTTP 400) and the message above, creating no workflow, before any state mutation.

Traces: logs__transfer.workflow-naming-an-absent-log-group-as-structured-log-destination-rejected.

## Evidence
Check logs__transfer.workflow-naming-an-absent-log-group-as-structured-log-destination-rejected.

Request (log group <group> deliberately NOT created yet):
  CreateWorkflow Description='sim2real <group>', Steps=[{Type: TAG, TagStepDetails{Name: mark-upload, SourceFileLocation: ${original.file}, Tags:[{stage: received}]}}], StructuredLogDestinations=['arn:aws:logs:<region>:<account>:log-group:<group>:*']

Expected (AWS, recorded): HTTP 400
  Error.Code    = 'InvalidRequestException'
  Error.Message = 'Unable to create logging. Log destination resource <log group> was not found'
                  (fixed text 'Unable to create logging. Log destination resource ... was not found';
                   the variable part is the log-group resource from the request)
  and no workflow carrying the request's Description exists afterwards.

Actual (emulator): HTTP 200
  {'WorkflowId': 'w-00000000000000001', 'ResponseMetadata': {'HTTPStatusCode': 200,
   'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '36', ...}}}
  -> AssertionError: expected a refusal, the call succeeded
(the WorkflowId value shown is this run's generated id, not a requirement)

## Steps to Reproduce
1. Pick a log-group name that does not exist in CloudWatch Logs (do not create it).
2. Call transfer.CreateWorkflow with Steps=[{Type:'TAG', TagStepDetails:{Name:'mark-upload', SourceFileLocation:'${original.file}', Tags:[{Key:'stage',Value:'received'}]}}] and StructuredLogDestinations=['arn:aws:logs:<region>:<account>:log-group:<that-name>:*'].
3. Observe HTTP 200 with a WorkflowId; AWS answers HTTP 400 InvalidRequestException 'Unable to create logging. Log destination resource <arn> was not found'.
4. Call ListWorkflows: the emulator has created a workflow with that Description; AWS has none.
5. Create the log group with logs.CreateLogGroup and repeat the identical CreateWorkflow: it must now succeed and DescribeWorkflow must read StructuredLogDestinations back as sent.
