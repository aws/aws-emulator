Issue-Source: trace-sweep-divergence
Issue-Trace: iot-cancel-audit-mitigation-actions-task-id-pattern-and-length
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-cancel-audit-mitigation-actions-task-id-pattern-and-length iot-get-command-execution-target-arn-validation iot-delete-v2-logging-level-invalid-target-type
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CancelAuditMitigationActionsTask|4xx->4xx|$.message
Issue-Key: iot|DeleteV2LoggingLevel|4xx->4xx|$.message
Issue-Related: 20261128T003242Z-iot-commands-plane-listcommandexecutions-getcommandexecution-deletecommandexecution-skip-pre.md
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot validation messages drop the rejected value and list a stale targetType enum set

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelAuditMitigationActionsTask

## Description
Three independent net-new root causes in service iot, all in the text of a constraint-violation refusal. Status and error class are correct in all three; only the message diverges. Each cause must be closed before submitting. Related but NOT covering these: open issue 20261127T130644Z-iot-7-net-new-causes-listcommandexecutions-three-validationexception.md (cause 4) records the same value-dropping formatter for ListThings.maxResults and notes the echo decision must be made per member, and the resolved issue it cites records one member (CreateRoleAlias.credentialDurationSeconds) that must NOT echo its value — so the formatter cannot be flipped globally and these three members are unowned.

Cause 1 of 3: CancelAuditMitigationActionsTask's @pattern violation message omits the rejected value.
AWS: Smithy CancelAuditMitigationActionsTask (PUT /audit/mitigationactions/tasks/{taskId}/cancel) binds taskId as @httpLabel @required to MitigationActionsTaskId with @length(min 1, max 128) and @pattern ^[a-zA-Z0-9_-]+$, and models InvalidRequestException (httpError 400). Recorded behavior: a taskId that violates the pattern is refused 400 with "1 validation error detected: Value '<taskId>' at 'taskId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+". Fixed text: "1 validation error detected: Value '", "' at 'taskId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+"; the only variable part is the rejected taskId copied from the request path (the pattern body is the modeled one).
Emulator: same status and class, but the message is "1 validation error detected: Value at 'taskId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+" — the "'<value>' " segment is missing, so the sentence reads "Value at 'taskId'".
Why: the operator loses the one fact the message exists to carry (WHICH value was rejected), and every exact-match client or assertion against AWS's sentence diverges.
Fix: the service's constraint-violation message builder in its @pattern branch plus CancelAuditMitigationActionsTask's input validation path that feeds it (services/service-com-amazonaws-iot-awsiotservice/src/ops/cancel_audit_mitigation_actions_task.rs and the shared validation-message builder).
Traces: iot-cancel-audit-mitigation-actions-task-id-pattern-and-length#task#2 (cluster of 2: also #task#3, the @length leg of the same member).

Cause 2 of 3: GetCommandExecution's executionId @pattern violation message omits the rejected value in the same way.
AWS: Smithy GetCommandExecution binds executionId as @httpLabel @required to CommandExecutionId with @length(min 1, max 64) and @pattern ^[a-zA-Z0-9_-]+$, and models ValidationException (400). Recorded behavior: the refusal message is "1 validation error detected: Value '<executionId>' at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+", with the rejected executionId from the request path as its only variable part.
Emulator: "1 validation error detected: Value at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+".
Why: same as Cause 1. Filed as its own cause because it is a different operation and member, and the value-echo decision is per member.
Fix: GetCommandExecution's input validation path and the shared @pattern report (services/service-com-amazonaws-iot-awsiotservice/src/ops/get_command_execution.rs).
Traces: iot-get-command-execution-target-arn-validation#task#4.

Cause 3 of 3: DeleteV2LoggingLevel's enum refusal lists an enum value set that is missing DEVICE_DEFENDER.
AWS: Smithy DeleteV2LoggingLevel (DELETE /v2LoggingLevel) binds targetType as @httpQuery @required to LogTargetType and models InvalidRequestException (400). The on-disk model's LogTargetType carries [DEFAULT, THING_GROUP, CLIENT_ID, SOURCE_IP, PRINCIPAL_ID], but the recording shows the live service's set, so recorded behavior governs: an unknown targetType is refused 400 with "1 validation error detected: Value '<targetType>' at 'targetType' failed to satisfy constraint: Member must satisfy enum value set: [DEFAULT, SOURCE_IP, DEVICE_DEFENDER, PRINCIPAL_ID, THING_GROUP, CLIENT_ID]". Fixed text: the sentence plus that bracketed set in that order; the only variable part is the rejected targetType from the query string. DEVICE_DEFENDER being in the advertised set also means it is an accepted targetType value, not only a word in the message.
Emulator: it does echo the rejected value here, but the advertised set is "[DEFAULT, SOURCE_IP, PRINCIPAL_ID, THING_GROUP, CLIENT_ID]" — DEVICE_DEFENDER is absent.
Why: the message is how a client discovers the legal values; a caller is told DEVICE_DEFENDER is not a valid log target type, so Device Defender log-level management looks unsupported, and exact-match comparisons against AWS's set fail.
Fix: the LogTargetType value set the iot service validates against and the enum-set rendering in DeleteV2LoggingLevel's validation (the shared LogTargetType enum in the iot service crate plus services/service-com-amazonaws-iot-awsiotservice/src/ops/delete_v2_logging_level.rs); the sibling SetV2LoggingLevel/GetV2LoggingOptions paths read the same set.
Traces: iot-delete-v2-logging-level-invalid-target-type#task#1.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. In each case the HTTP status and the error code matched; only $.message diverged. The quoted probe names/ids below are one run's values filling the templates.

Cause 1 — iot-cancel-audit-mitigation-actions-task-id-pattern-and-length#task#2, CancelAuditMitigationActionsTask, status 400 both sides:
  expected $.message "1 validation error detected: Value 'sim2real-probe-mtask-8912cbb4.v2' at 'taskId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+"
  actual   $.message "1 validation error detected: Value at 'taskId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+"
  (same cluster: #task#3)

Cause 2 — iot-get-command-execution-target-arn-validation#task#4, GetCommandExecution, status 400 both sides:
  expected $.message "1 validation error detected: Value 'sim2real-probe-cmdtarget-b6a7d640.v2' at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+"
  actual   $.message "1 validation error detected: Value at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9_-]+"

Cause 3 — iot-delete-v2-logging-level-invalid-target-type#task#1, DeleteV2LoggingLevel, status 400 both sides:
  expected $.message "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'targetType' failed to satisfy constraint: Member must satisfy enum value set: [DEFAULT, SOURCE_IP, DEVICE_DEFENDER, PRINCIPAL_ID, THING_GROUP, CLIENT_ID]"
  actual   $.message "1 validation error detected: Value 'NOT_A_REAL_TYPE' at 'targetType' failed to satisfy constraint: Member must satisfy enum value set: [DEFAULT, SOURCE_IP, PRINCIPAL_ID, THING_GROUP, CLIENT_ID]"

Fixed vs variable text: in Causes 1 and 2 the only variable part is the rejected member value from the request (the probe names above are one run's values); the pattern body is the modeled @pattern. In Cause 3 the variable part is the rejected targetType and the bracketed enum set is fixed text that must include DEVICE_DEFENDER.

Model citations: MitigationActionsTaskId @length(1,128) @pattern ^[a-zA-Z0-9_-]+$, CancelAuditMitigationActionsTask errors InvalidRequestException(400)/ResourceNotFoundException(404)/ThrottlingException(400)/InternalFailureException(500). CommandExecutionId @length(1,64) @pattern ^[a-zA-Z0-9_-]+$, GetCommandExecution errors ValidationException(400)/ResourceNotFoundException(404). DeleteV2LoggingLevel targetType -> LogTargetType (on-disk values [DEFAULT, THING_GROUP, CLIENT_ID, SOURCE_IP, PRINCIPAL_ID]; the recording adds DEVICE_DEFENDER), errors InvalidRequestException(400)/InternalException(500)/ServiceUnavailableException(503).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from this checkout.
2. PUT /audit/mitigationactions/tasks/sim2real-probe-mtask.v2/cancel — the InvalidRequestException message must quote the rejected taskId: "... Value 'sim2real-probe-mtask.v2' at 'taskId' ..."; the emulator omits the quoted value.
3. GET /command-executions/sim2real-probe.v2?targetArn=<any> — the ValidationException message must quote the rejected executionId.
4. DELETE /v2LoggingLevel?targetType=NOT_A_REAL_TYPE&targetName=x — the enum value set in the message must read [DEFAULT, SOURCE_IP, DEVICE_DEFENDER, PRINCIPAL_ID, THING_GROUP, CLIENT_ID]; the emulator omits DEVICE_DEFENDER. Also confirm targetType=DEVICE_DEFENDER is accepted rather than refused.
5. Re-replay iot-cancel-audit-mitigation-actions-task-id-pattern-and-length#task#2, iot-get-command-execution-target-arn-validation#task#4 and iot-delete-v2-logging-level-invalid-target-type#task#1 to confirm the fix.
