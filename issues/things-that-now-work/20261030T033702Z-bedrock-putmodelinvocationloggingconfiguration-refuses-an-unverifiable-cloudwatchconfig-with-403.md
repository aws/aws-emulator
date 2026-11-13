Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-model-invocation-logging-config
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-model-invocation-logging-config
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|4xx->4xx|$.message
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: bedrock|PutModelInvocationLoggingConfiguration|4xx->4xx|status
Issue-Commit: e8c5c92d9bc397e4014b473476bd2d32121bdaea

# bedrock PutModelInvocationLoggingConfiguration refuses an unverifiable cloudWatchConfig with 403 AccessDeniedException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `e8c5c92d9bc397e4014b473476bd2d32121bdaea`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutModelInvocationLoggingConfiguration

## Description
Cause 1 of 1: the cloudWatchConfig branch of PutModelInvocationLoggingConfiguration answers one hard-wired refusal (403 AccessDeniedException / "Cross-account pass role is not allowed.") for every destination it cannot verify, instead of the 400 ValidationException AWS returns when the named log group / role cannot be validated.

AWS: Smithy (com.amazonaws.bedrock) models PutModelInvocationLoggingConfiguration as PUT /logging/modelinvocations, success code 200 with an empty output shape, one required input member `loggingConfig` (LoggingConfig), @idempotent, and errors AccessDeniedException (httpError 403), ValidationException (httpError 400), ThrottlingException (429), InternalServerException (500). LoggingConfig.cloudWatchConfig targets CloudWatchConfig, whose required `logGroupName` and `roleArn` are the two values the service must validate before enabling the destination. The model does not state WHICH of the two modeled client errors a failed validation takes, so for that outcome: recorded behavior only. The recorded response for a cloudWatchConfig naming a log group that does not exist together with a role that does not exist is HTTP 400 with the ValidationException message "Failed to validate permissions for log group: /aws/bedrock/nonexistent-probe-log-group, with role: arn:aws:iam::000000000000:role/nonexistent-role. Verify the IAM role permissions are correct." Message anatomy for this error class: the FIXED text is "Failed to validate permissions for log group: ", ", with role: " and ". Verify the IAM role permissions are correct."; the two VARIABLE parts are taken straight from the request — the submitted cloudWatchConfig.logGroupName and the submitted cloudWatchConfig.roleArn. The log-group name and role ARN quoted above are this recording's values filling that template, not part of the requirement. So: a destination whose log group or role cannot be validated is a ValidationException (400) that echoes the submitted logGroupName and roleArn; AccessDeniedException (403) with the fixed, variable-free text "Cross-account pass role is not allowed." is reserved for the PassRole decision over a role that DOES resolve but is not passable (see the related issue below).

Emulator: the request is refused, but with the wrong status, the wrong error code and a message belonging to a different error class. Replay reports status: expected 400, got 403; $headers.x-amzn-errortype: got 'AccessDeniedException' where the recording carries the ValidationException shape; $.message: expected 'Failed to validate permissions for log group: /aws/bedrock/nonexistent-probe-log-group, with role: arn:aws:iam::000000000000:role/nonexistent-role. Verify the IAM role permissions are correct.', got 'Cross-account pass role is not allowed.' The emulator therefore performs no validation of logGroupName/roleArn existence at all: it takes the cross-account-pass-role exit for a destination whose role does not even resolve, and it never composes the "Failed to validate permissions" message, so neither of the two variable parts (the submitted log group name and role ARN) ever appears in a refusal.

Related open issues, neither of which covers this step (this divergence is reported with no owning issue, so its replay key is carried here): 20261029T221455Z-bedrock-logs-putmodelinvocationloggingconfiguration-performs-no-passrole-decision-on.md reports the OPPOSITE symptom on the valid-role path (a same-account, correctly-trusted role ACCEPTED 200 where the recording refuses 403 AccessDeniedException), and 20261029T211357Z-bedrock-answers-resourcenotfoundexception-or-accessdeniedexception-where-aws-answers.md covers other operations' error-class substitutions. Taken together they mean the branch must DECIDE per destination over three outcomes — 400 ValidationException when the log group or role cannot be validated, 403 AccessDeniedException when a resolvable role is not passable, 200 otherwise — rather than return one constant answer.

Why: a client relying on AWS's behavior branches on the error class. Bootstrap and IaC code (console-equivalent setup, Terraform/CloudFormation convergence, a CI step that Puts then Gets) treats ValidationException as "my logGroupName/roleArn are wrong, re-read them from the request and retry/fix the inputs" and AccessDeniedException as "the credential or the role trust boundary refuses this, escalate to an administrator". Getting 403 AccessDeniedException for a simple typo in a log group name sends the caller down the permissions-escalation path for an input error it could have fixed itself, and the operator never sees which log group or role was rejected because the emulator's message echoes neither. SDK retry/classification layers also treat 403 differently from 400, and code that catches ValidationException to surface a field-level remediation message never runs.

Fix: the cloudWatchConfig validation of PutModelInvocationLoggingConfiguration's handler. Before the PassRole decision it must validate that cloudWatchConfig.logGroupName names an existing CloudWatch Logs log group and that cloudWatchConfig.roleArn names a role that resolves and grants the log-delivery permissions, and on failure refuse with the modeled ValidationException (HTTP 400, x-amzn-errortype ValidationException) whose message is composed from the fixed text above with the submitted logGroupName and roleArn interpolated, committing nothing to the account/region logging singleton. The AccessDeniedException (403) exit with the fixed "Cross-account pass role is not allowed." text must be reachable only from the PassRole decision on a role that does resolve.

Traces: bedrock-model-invocation-logging-config#task#4 (PutModelInvocationLoggingConfiguration). Handler: services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/src/ops/put_model_invocation_logging_configuration.rs.

## Evidence
Wire corpus simparity-wire-aws-v1, fixture bedrock-model-invocation-logging-config, step task#4, operation PutModelInvocationLoggingConfiguration, verdict mismatch, divergence_key service=bedrock|PutModelInvocationLoggingConfiguration|status=400->403|keys=$.message,$headers.x-amzn-errortype,status (affected_count 1, no owning issue, no stub).

Replay detail, verbatim:
  status: expected 400, got 403
  $.message: value_mismatch (expected 'Failed to validate permissions for log group: /aws/bedrock/nonexistent-probe-log-group, with role: arn:aws:iam::000000000000:role/nonexistent-role. Verify the IAM role permissions are correct.', got 'Cross-account pass role is not allowed.')
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'AccessDeniedException')

Expected (recorded AWS): HTTP 400, error class ValidationException, message = fixed text "Failed to validate permissions for log group: " + <submitted cloudWatchConfig.logGroupName> + ", with role: " + <submitted cloudWatchConfig.roleArn> + ". Verify the IAM role permissions are correct."
Actual (emulator): HTTP 403, x-amzn-errortype AccessDeniedException, message "Cross-account pass role is not allowed." (fixed text with no variable part — it names neither the log group nor the role).

Model facts used (services/service-com-amazonaws-bedrock-amazonbedrockcontrolplaneservice/smithy-model/com-amazonaws-bedrock-amazonbedrockcontrolplaneservice.json): PutModelInvocationLoggingConfiguration — http PUT /logging/modelinvocations, code 200, @idempotent; input PutModelInvocationLoggingConfigurationRequest with required member loggingConfig -> LoggingConfig; output shape empty; errors AccessDeniedException httpError 403 (client), ValidationException httpError 400 (client), ThrottlingException 429, InternalServerException 500. Both modeled client errors are available to this operation, so answering 403 here is a choice of the wrong modeled error, not a missing one.

Other steps of the same fixture: task#3 (GetModelInvocationLoggingConfiguration returning an empty body) diverges too but is already owned by 20261029T211357Z-bedrock-read-operations-return-an-empty-json-body.md and is NOT re-filed here.

## Steps to Reproduce
Boot the emulator from this checkout and replay wire fixture bedrock-model-invocation-logging-config, step task#4 (trace set simparity-wire-aws-v1).

1. Call bedrock PutModelInvocationLoggingConfiguration (PUT /logging/modelinvocations) with loggingConfig.cloudWatchConfig naming a log group that does not exist and a roleArn for a role that does not exist in the account, e.g. {"loggingConfig": {"cloudWatchConfig": {"logGroupName": "/aws/bedrock/nonexistent-probe-log-group", "roleArn": "arn:aws:iam::000000000000:role/nonexistent-role"}}} (the particular names are this recording's values; any unresolvable pair reproduces it).
2. Expect (recorded AWS): HTTP 400 with error class ValidationException and message "Failed to validate permissions for log group: <the logGroupName you submitted>, with role: <the roleArn you submitted>. Verify the IAM role permissions are correct."
3. Observe: HTTP 403 with x-amzn-errortype AccessDeniedException and message "Cross-account pass role is not allowed." — the status class, the error code and the whole message differ, and the submitted log group name and role ARN appear nowhere in the response.
4. Then call GetModelInvocationLoggingConfiguration and confirm the refused Put committed nothing to the account/region logging singleton (this part is not what diverges; it is the invariant a correct ValidationException refusal must keep).
