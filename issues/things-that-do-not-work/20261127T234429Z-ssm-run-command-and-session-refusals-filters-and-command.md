Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-cancel-command-instance-id-count-bound
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-cancel-command-instance-id-count-bound ssm-cancel-command-repeated-instance-id-refused ssm-command-status-filter-values-differ-per-listing ssm-list-command-invocations-filter-count-bounds ssm-list-command-invocations-filter-value-length-bounds ssm-send-command-document-hash-length-bound ssm-send-command-notification-events-enum ssm-get-command-invocation-plugin-name-length-bound ssm-start-session-parameter-name-length
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|ListCommands|2xx->2xx|$.Commands[*].Status
Issue-Key: ssm|ListCommands|2xx->2xx|$.Commands[*].StatusDetails
Issue-Key: ssm|CancelCommand|4xx->4xx|$.Message
Issue-Key: ssm|ListCommands|4xx->4xx|$.message
Issue-Key: ssm|ListCommandInvocations|4xx->4xx|$.message
Issue-Key: ssm|SendCommand|4xx->4xx|$.Message
Issue-Key: ssm|SendCommand|4xx->4xx|$.message
Issue-Key: ssm|SendCommand|4xx->4xx|$.__type
Issue-Key: ssm|GetCommandInvocation|4xx->4xx|$.__type
Issue-Key: ssm|GetCommandInvocation|4xx->4xx|$.message
Issue-Key: ssm|StartSession|4xx->4xx|$.__type
Issue-Key: ssm|StartSession|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# Run Command and Session refusals, filters and command status diverge from AWS

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCommands

## Description
Ten net-new causes on the Run Command / Session plane: a command that never leaves Pending, refusals with the wrong code or message, and validation that runs in the wrong order.

Cause 1 of 10: a command targeting no live node never reaches a terminal status.
AWS: a SendCommand whose targets match no managed node settles: ListCommands reports Status 'Success' with StatusDetails 'NoInstancesInTag'.
Emulator: Status and StatusDetails stay 'Pending' indefinitely (the replay polled 20 times).
Why: every client that waits for a command to finish hangs; nothing downstream of a command can be asserted.
Fix: the Run Command state machine (terminal status for a command with no matching instances).
Traces: ssm-cancel-command-instance-id-count-bound#task#4.

Cause 2 of 10: CancelCommand's duplicate-instance refusal carries no message.
AWS: 400 with Message "Duplicate instanceIds <id> not allowed." — fixed text with the repeated id taken from the request.
Emulator: refuses with no Message at all.
Why: the caller cannot tell which id was duplicated, or why the call failed.
Fix: the CancelCommand validation message.
Traces: ssm-cancel-command-repeated-instance-id-refused#task#2.

Cause 3 of 10: ListCommands re-words the Status filter refusal.
AWS: 400 with message "Invalid filter value <value> for filter key Status".
Emulator: emits the generic enum template "Value '<value>' at 'filters.1.member.value' failed to satisfy constraint: Member must satisfy enum value set: [...]".
Why: clients matching on AWS's sentence do not recognise the refusal.
Fix: the ListCommands filter validation message.
Traces: ssm-command-status-filter-values-differ-per-listing#task#4.

Cause 4 of 10: ListCommandInvocations rejects the valid filter key InstanceId.
AWS: 400 naming only the single unsupported key: "1 validation error detected: Value 'PluginName' at 'filters.2.member.key' failed to satisfy constraint: Member must satisfy enum value set: [CommandId, DocumentName, ExecutionStage, InvokedBefore, InstanceId, InvokedAfter, Status]".
Emulator: reports two errors, adding one against 'InstanceId' — a key its own quoted enum set contains.
Why: a request whose only fault is one key is reported as two faults, and a legal key is named as illegal.
Fix: the ListCommandInvocations filter-key validation.
Traces: ssm-list-command-invocations-filter-count-bounds#task#6.

Cause 5 of 10: SendCommand's hash-mismatch refusal is emulator-authored.
AWS: 400 with Message 'Document hash does not match'.
Emulator: "Document <name> doesn't have Sha256 hash '<hash>'".
Why: the message is not AWS's, and it leaks the document name and hash AWS deliberately omits.
Fix: the SendCommand document-hash check message.
Traces: ssm-send-command-document-hash-length-bound#task#3.

Cause 6 of 10: SendCommand truncates the value it echoes in a length refusal.
AWS: "1 validation error detected: Value '<full 257-char value>' at 'documentHash' failed to satisfy constraint: Member must have length less than or equal to 256" — the whole submitted value is quoted.
Emulator: quotes a prefix followed by "... [257 bytes]".
Why: the refusal body differs from AWS for every over-long member, and the inserted byte count is not AWS text.
Fix: the validation message renderer (no truncation marker).
Traces: ssm-send-command-document-hash-length-bound#task#2.

Cause 7 of 10: SendCommand checks the instance before the notification config.
AWS: 400, __type InvalidNotificationConfig, Message 'NotificationArn is a required parameter to enable notification.'
Emulator: 400, __type InvalidInstanceId, Message 'Instances not in a valid state for account'.
Why: the caller is told to fix the targets when the real fault is the notification block, so the request can never be corrected from the error.
Fix: the SendCommand validation order (notification config before instance resolution).
Traces: ssm-send-command-notification-events-enum#task#1.

Cause 8 of 10: GetCommandInvocation answers a shape violation with a lookup error.
AWS: 400, __type ValidationException, message "2 validation errors detected: Value '<value>' at 'pluginName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.{4,128}$; Value '<value>' at 'pluginName' failed to satisfy constraint: Member must have length less than or equal to 128".
Emulator: 400, __type InvocationDoesNotExist, Message 'Invocation not found for <commandId>, <instanceId>'.
Why: an input-validation fault is reported as a missing invocation, so the caller retries forever against an existing command.
Fix: GetCommandInvocation input validation before invocation lookup.
Traces: ssm-get-command-invocation-plugin-name-length-bound#task#5.

Cause 9 of 10: GetCommandInvocation reports only one of two validation errors.
AWS: "2 validation errors detected: Value 'run' at 'pluginName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^.{4,128}$; Value 'run' at 'pluginName' failed to satisfy constraint: Member must have length greater than or equal to 4".
Emulator: emits only the length clause ('1 validation error detected').
Fix: the validation walk must evaluate pattern and length and report both, pattern first.
Traces: ssm-get-command-invocation-plugin-name-length-bound#task#2.

Cause 10 of 10: StartSession checks target connectivity before validating parameters.
AWS: 400, __type ValidationException, message "1 validation error detected: Value at 'parameters' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 255, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: ^.{1,255}$]".
Emulator: 400, __type TargetNotConnected, Message '<instanceId> is not connected.'
Why: the caller cannot discover the malformed parameter key, and a shape fault is reported as an environment fault.
Fix: StartSession input validation order.
Traces: ssm-start-session-parameter-name-length#task#3.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-cancel-command-instance-id-count-bound#task#4: $.Commands[0].Status expected 'Success' got 'Pending' (poll: not in ['Success'] after 20 attempts); StatusDetails expected 'NoInstancesInTag' got 'Pending'.
Cause 2 ssm-cancel-command-repeated-instance-id-refused#task#2: $.Message expected 'Duplicate instanceIds i-0123456789abcdef0 not allowed.', got None.
Cause 3 ssm-command-status-filter-values-differ-per-listing#task#4: $.message expected 'Invalid filter value Delayed for filter key Status'.
Cause 4 ssm-list-command-invocations-filter-count-bounds#task#6: expected 1 validation error against filters.2.member.key, got 2 errors (extra one against 'InstanceId').
Cause 5 ssm-send-command-document-hash-length-bound#task#3: $.Message expected 'Document hash does not match'.
Cause 6 ssm-send-command-document-hash-length-bound#task#2: $.message echo truncated with '... [257 bytes]'.
Cause 7 ssm-send-command-notification-events-enum#task#1: $.__type expected 'InvalidNotificationConfig' got 'InvalidInstanceId'.
Cause 8 ssm-get-command-invocation-plugin-name-length-bound#task#5: $.__type expected 'ValidationException' got 'InvocationDoesNotExist'.
Cause 9 ssm-get-command-invocation-plugin-name-length-bound#task#2: expected 2 validation errors, got 1.
Cause 10 ssm-start-session-parameter-name-length#task#3: $.__type expected 'ValidationException' got 'TargetNotConnected'.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Cause 1: fixture ssm-cancel-command-instance-id-count-bound step task#4 (ListCommands poll after SendCommand to a tag target).
3. Cause 2: fixture ssm-cancel-command-repeated-instance-id-refused step task#2.
4. Causes 3, 4: fixtures ssm-command-status-filter-values-differ-per-listing step task#4 and ssm-list-command-invocations-filter-count-bounds step task#6.
5. Causes 5, 6: fixture ssm-send-command-document-hash-length-bound steps task#2, task#3.
6. Cause 7: fixture ssm-send-command-notification-events-enum step task#1.
7. Causes 8, 9: fixture ssm-get-command-invocation-plugin-name-length-bound steps task#2, task#5.
8. Cause 10: fixture ssm-start-session-parameter-name-length step task#3.
