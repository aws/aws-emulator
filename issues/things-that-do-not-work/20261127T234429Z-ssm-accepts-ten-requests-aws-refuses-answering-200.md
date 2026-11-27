Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-automation-loop-document-validation
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-automation-loop-document-validation ssm-document-requires-version-name-pattern ssm-automation-step-filters-count-limit ssm-command-status-filter-values-differ-per-listing ssm-list-command-invocations-filter-count-bounds ssm-describe-instance-properties-filters-with-operator-count ssm-describe-task-invocations-filter-value-length-bounds ssm-instance-information-filter-value-set-over-100-refused ssm-instance-patch-states-max-results-range ssm-maintenance-window-schedule-resource-type-values
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateDocument|4xx->2xx|$.Message
Issue-Key: ssm|CreateDocument|4xx->2xx|$.__type
Issue-Key: ssm|CreateDocument|4xx->2xx|status
Issue-Key: ssm|CreateDocument|4xx->2xx|$.message
Issue-Key: ssm|DescribeDocument|4xx->2xx|$.Message
Issue-Key: ssm|DescribeDocument|4xx->2xx|$.__type
Issue-Key: ssm|DescribeDocument|4xx->2xx|status
Issue-Key: ssm|DescribeAutomationStepExecutions|4xx->2xx|$.Message
Issue-Key: ssm|DescribeAutomationStepExecutions|4xx->2xx|$.__type
Issue-Key: ssm|DescribeAutomationStepExecutions|4xx->2xx|status
Issue-Key: ssm|ListCommandInvocations|4xx->2xx|$.__type
Issue-Key: ssm|ListCommandInvocations|4xx->2xx|$.message
Issue-Key: ssm|ListCommandInvocations|4xx->2xx|status
Issue-Key: ssm|DescribeInstanceProperties|4xx->2xx|$.__type
Issue-Key: ssm|DescribeInstanceProperties|4xx->2xx|$.message
Issue-Key: ssm|DescribeInstanceProperties|4xx->2xx|status
Issue-Key: ssm|DescribeMaintenanceWindowExecutionTaskInvocations|4xx->2xx|$.__type
Issue-Key: ssm|DescribeMaintenanceWindowExecutionTaskInvocations|4xx->2xx|$.message
Issue-Key: ssm|DescribeMaintenanceWindowExecutionTaskInvocations|4xx->2xx|status
Issue-Key: ssm|DescribeInstanceInformation|4xx->2xx|$.__type
Issue-Key: ssm|DescribeInstanceInformation|4xx->2xx|$.message
Issue-Key: ssm|DescribeInstanceInformation|4xx->2xx|status
Issue-Key: ssm|DescribeInstancePatchStates|4xx->2xx|$.__type
Issue-Key: ssm|DescribeInstancePatchStates|4xx->2xx|$.message
Issue-Key: ssm|DescribeInstancePatchStates|4xx->2xx|status
Issue-Key: ssm|DescribeMaintenanceWindowSchedule|4xx->2xx|$.__type
Issue-Key: ssm|DescribeMaintenanceWindowSchedule|4xx->2xx|$.message
Issue-Key: ssm|DescribeMaintenanceWindowSchedule|4xx->2xx|status
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# SSM accepts ten requests AWS refuses, answering 200 where AWS returns a 400 refusal

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDocument

## Description
Ten net-new causes that share one fault: a request AWS refuses with 400 is served 200 by the emulator, because the semantic (post-shape) validation AWS applies is missing. Bundled by that shared fault; each cause names its own operation, status, error code and message.

Cause 1 of 10: CreateDocument does not validate automation document content.
AWS: refuses an aws:loop step whose MaxIterations is out of range with HTTP 400, __type InvalidDocumentContent, Message "In step <stepName>, the 'MaxIterations' input must be at least 1 and not greater than 100. Provided value was <value>." Fixed text is the sentence; variables are the step name and the submitted value. Smithy: CreateDocument error InvalidDocumentContent.
Emulator: 200 with the document created.
Why: an invalid runbook is accepted and only fails (or loops) at execution time.
Fix: CreateDocument content validation.
Traces: ssm-automation-loop-document-validation#task#1.

Cause 2 of 10: CreateDocument accepts VersionName inside Requires for an ApplicationConfiguration document.
AWS: 400, __type ValidationException, message "VersionName under Requires is not supported for ApplicationConfiguration document."
Emulator: 200.
Why: a client gets a document whose Requires entry AWS would never store.
Fix: CreateDocument validation of the Requires list by document type.
Traces: ssm-document-requires-version-name-pattern#task#5.

Cause 3 of 10: DescribeDocument answers 200 for a document that was deleted.
AWS: 400, __type InvalidDocument, Message "Document with name <name> does not exist." (fixed sentence; variable is the requested name).
Emulator: 200 with a document body.
Why: a deleted document stays readable, so delete-then-verify flows and 'does this exist' probes both succeed wrongly.
Fix: DescribeDocument lookup against document state after delete (delete must remove the record).
Traces: ssm-automation-loop-document-validation#task#6.

Cause 4 of 10: DescribeAutomationStepExecutions accepts duplicate filter keys.
AWS: 400, __type InvalidFilterKey, Message "Cannot specify duplicate filter keys for <FilterKey>".
Emulator: 200.
Fix: DescribeAutomationStepExecutions filter validation.
Traces: ssm-automation-step-filters-count-limit#task#7.

Cause 5 of 10: ListCommandInvocations accepts an unsupported Status filter value.
AWS: 400, __type ValidationException, message "Invalid filter value <value> for filter key Status" — the Status values accepted by ListCommandInvocations are narrower than ListCommands'.
Emulator: 200 with a result list.
Fix: ListCommandInvocations Status filter value validation.
Traces: ssm-command-status-filter-values-differ-per-listing#task#6.

Cause 6 of 10: DescribeInstanceProperties ignores the tag-key count ceiling.
AWS: 400, __type ValidationException, message "The number of tag keys was <n>. Max: 5."
Emulator: 200.
Fix: DescribeInstanceProperties filter validation.
Traces: ssm-describe-instance-properties-filters-with-operator-count#task#3.

Cause 7 of 10: DescribeMaintenanceWindowExecutionTaskInvocations accepts an invalid invocation-status filter value.
AWS: 400, __type ValidationException, message "Invocation Status [<value>] are not valid".
Emulator: 200.
Fix: that operation's filter value validation.
Traces: ssm-describe-task-invocations-filter-value-length-bounds#task#2.

Cause 8 of 10: DescribeInstanceInformation accepts a multi-valued AgentVersion filter.
AWS: 400, __type InvalidInstanceInformationFilterValue, message "AgentVersion filter may contain only one value."
Emulator: 200.
Fix: DescribeInstanceInformation legacy-filter validation.
Traces: ssm-instance-information-filter-value-set-over-100-refused#task#3.

Cause 9 of 10: DescribeInstancePatchStates does not enforce the MaxResults ceiling.
AWS: 400, __type ValidationException, message "1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 50". Smithy on disk models PatchComplianceMaxResults range 10..100, but the recorded service enforces 1..50 and the recording is the wire requirement.
Emulator: 200.
Fix: DescribeInstancePatchStates input validation (range 1..50).
Traces: ssm-instance-patch-states-max-results-range#task#3.

Cause 10 of 10: DescribeMaintenanceWindowSchedule accepts an unknown target key.
AWS: 400, __type ValidationException, message "<key> is not an known target key. Keys can one of the following: InstanceIds, tag-key, or tag:<tag-key>" (AWS's wording, including its grammar, is the requirement; the variable is the submitted key).
Emulator: 200.
Fix: DescribeMaintenanceWindowSchedule target validation.
Traces: ssm-maintenance-window-schedule-resource-type-values#task#5.

## Evidence
Replay of simparity-wire-aws-v1. Each cause: expected 400 with the quoted __type/Message, actual 200 with those keys absent.
Cause 1 ssm-automation-loop-document-validation#task#1: expected $.__type 'InvalidDocumentContent', $.Message "In step cycle, the 'MaxIterations' input must be at least 1 and not greater than 100. Provided value was 0."; got 200.
Cause 2 ssm-document-requires-version-name-pattern#task#5: expected 'ValidationException' / 'VersionName under Requires is not supported for ApplicationConfiguration document.'; got 200.
Cause 3 ssm-automation-loop-document-validation#task#6: expected 'InvalidDocument' / 'Document with name <name> does not exist.'; got 200.
Cause 4 ssm-automation-step-filters-count-limit#task#7: expected 'InvalidFilterKey' / 'Cannot specify duplicate filter keys for StepName'; got 200.
Cause 5 ssm-command-status-filter-values-differ-per-listing#task#6: expected 'ValidationException' / 'Invalid filter value Incomplete for filter key Status'; got 200.
Cause 6 ssm-describe-instance-properties-filters-with-operator-count#task#3: expected 'ValidationException' / 'The number of tag keys was 6. Max: 5.'; got 200.
Cause 7 ssm-describe-task-invocations-filter-value-length-bounds#task#2: expected 'ValidationException' / 'Invocation Status [<256-char value>] are not valid'; got 200.
Cause 8 ssm-instance-information-filter-value-set-over-100-refused#task#3: expected 'InvalidInstanceInformationFilterValue' / 'AgentVersion filter may contain only one value.'; got 200.
Cause 9 ssm-instance-patch-states-max-results-range#task#3: expected 'ValidationException' / "...Value '51' at 'maxResults' ... less than or equal to 50"; got 200.
Cause 10 ssm-maintenance-window-schedule-resource-type-values#task#5: expected 'ValidationException' / 'resource-groups:Name is not an known target key...'; got 200.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus.
2. Replay each listed fixture and inspect the named step: the step sends a request AWS refuses and asserts the 400 body; the emulator answers 200.
3. Representative: fixture ssm-automation-loop-document-validation step task#1 — CreateDocument with an aws:loop step whose MaxIterations is 0.
