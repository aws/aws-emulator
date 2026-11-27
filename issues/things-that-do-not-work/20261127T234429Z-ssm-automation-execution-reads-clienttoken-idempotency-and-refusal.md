Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-automation-client-token-fixed-length
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-automation-client-token-fixed-length ssm-automation-execution-mode-enum ssm-automation-target-parameter-name-length-bound ssm-automation-executions-filter-key-enum ssm-automation-executions-filter-value-count-bounds ssm-describe-automation-executions-filter-value-length-bounds ssm-automation-stop-type-enum ssm-automation-step-filter-value-length-bound ssm-execution-preview-target-map-value-length
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|StartAutomationExecution|2xx->2xx|$.AutomationExecutionId
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.AutomationExecutionId
Issue-Key: ssm|DescribeAutomationExecutions|2xx->2xx|$.AutomationExecutionMetadataList[*].AutomationExecutionId
Issue-Key: ssm|DescribeAutomationExecutions|2xx->2xx|$.AutomationExecutionMetadataList[*].LogFile
Issue-Key: ssm|DescribeAutomationExecutions|2xx->2xx|$.AutomationExecutionMetadataList[*].Outputs
Issue-Key: ssm|DescribeAutomationExecutions|2xx->2xx|$.AutomationExecutionMetadataList[*].ResolvedTargets
Issue-Key: ssm|DescribeAutomationExecutions|2xx->2xx|$.AutomationExecutionMetadataList[*].Targets
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.MaxErrors
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.StepExecutions
Issue-Key: ssm|GetAutomationExecution|2xx->2xx|$.AutomationExecution.TargetsSummary
Issue-Key: ssm|DescribeAutomationExecutions|4xx->4xx|$.message
Issue-Key: ssm|StopAutomationExecution|4xx->4xx|$.message
Issue-Key: ssm|DescribeAutomationStepExecutions|4xx->4xx|$.message
Issue-Key: ssm|StartExecutionPreview|4xx->4xx|$.message
Issue-Key: ssm|StartAutomationExecution|4xx->4xx|$.message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# SSM Automation execution reads, clientToken idempotency and refusal messages diverge

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartAutomationExecution

## Description
Ten net-new causes on the Automation plane: the execution identity minted for a ClientToken, the members of the execution projections, and the refusal messages of the automation entry points.

Cause 1 of 10: StartAutomationExecution does not derive the execution id from ClientToken.
AWS: a StartAutomationExecution carrying a ClientToken returns an AutomationExecutionId derived from (and stable for) that token, so the same token yields the same execution — the recorded id matches the submitted token's own UUID body. Recorded behavior; the modeled ClientToken is the idempotency key.
Emulator: mints an unrelated random UUID.
Why: a client that retries with the same ClientToken starts a second execution, and a client that computed the execution id from its token cannot address its run.
Fix: the StartAutomationExecution handler's id allocation / idempotency.
Traces: ssm-automation-client-token-fixed-length#task#4.

Cause 2 of 10: GetAutomationExecution echoes the wrong id.
AWS: AutomationExecution.AutomationExecutionId is the token-derived id.
Emulator: the random id.
Fix: follows cause 1; verify on the read.
Traces: ssm-automation-client-token-fixed-length#task#5.

Cause 3 of 10: DescribeAutomationExecutions reports the wrong id and omits modeled members.
AWS: each AutomationExecutionMetadata carries LogFile (empty string when none), Outputs ({} when none), ResolvedTargets ({'ParameterValues': [], 'Truncated': false}) and Targets ([]).
Emulator: all four absent, plus the wrong id.
Why: a client reading outputs or resolved targets from the list gets null instead of the documented empty value.
Fix: the DescribeAutomationExecutions metadata projection.
Traces: ssm-automation-client-token-fixed-length#task#7.

Cause 4 of 10: the same four members are omitted for an execution started without a token.
AWS: LogFile '', Outputs {}, ResolvedTargets {'ParameterValues': [], 'Truncated': false}, Targets [].
Emulator: absent.
Fix: as cause 3 (unconditional projection gap).
Traces: ssm-automation-execution-mode-enum#task#5.

Cause 5 of 10: GetAutomationExecution omits StepExecutions/TargetsSummary and defaults MaxErrors wrongly.
AWS: a rate-controlled automation reports MaxErrors '100%' (the documented default), a StepExecutions list with one entry per executed step (Action, StepName, StepStatus, Inputs, Outputs, IsStepBlocked, start/end times) and TargetsSummary counts.
Emulator: MaxErrors '1', StepExecutions and TargetsSummary absent.
Why: a client cannot see which steps ran or how targets fared, and reads a MaxErrors the service never applied.
Fix: the automation execution state record and the GetAutomationExecution projection.
Traces: ssm-automation-target-parameter-name-length-bound#task#5.

Cause 6 of 10: the DescribeAutomationExecutions filter-key enum set is short two members.
AWS: refuses an unknown key with "1 validation error detected: Value at 'filters.1.member.key' failed to satisfy constraint: Member must satisfy enum value set: [OpsItemId, TargetResourceGroup, ScheduledTimeAfter, AutomationSubtype, TagKey, ParentExecutionId, ExecutionStatus, StartTimeAfter, StartTimeBefore, CurrentAction, AutomationType, ExecutionId, DocumentNamePrefix, ScheduledTimeBefore]" and does not echo the value.
Emulator: echoes the value and lists a set missing ScheduledTimeAfter and ScheduledTimeBefore (on-disk Smithy AutomationExecutionFilterKey also lacks them; the recording is the wire requirement, so the enum and the message both need widening).
Why: ScheduledTimeAfter/Before filters are refused as unknown, and the message misstates the accepted set.
Fix: the AutomationExecutionFilterKey enum used by validation plus the message renderer.
Traces: ssm-automation-executions-filter-key-enum#task#3.

Cause 7 of 10: StopAutomationExecution's enum refusal echoes the value.
AWS: "1 validation error detected: Value at 'type' failed to satisfy constraint: Member must satisfy enum value set: [Complete, Cancel]" — no value is quoted for this member.
Emulator: inserts "Value 'Pause' at 'type'".
Fix: the validation message renderer for this member.
Traces: ssm-automation-stop-type-enum#task#4.

Cause 8 of 10: DescribeAutomationStepExecutions renders a list-member constraint at the wrong level.
AWS: reports the failure against the list member with the aggregated constraint set: "1 validation error detected: Value at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 150, Member must have length greater than or equal to 1]".
Emulator: reports "Value at 'filters.1.member.values.1.member' ... less than or equal to 150" — a deeper path and only one of the two constraints.
Why: a client parsing the failing member path points at a path AWS never emits.
Fix: the validation error renderer for list members.
Traces: ssm-automation-step-filter-value-length-bound#task#5.

Cause 9 of 10: StartExecutionPreview's TargetMap refusal is re-worded.
AWS: 400 with message "Member of TargetMap value must have length less than or equal to 50 and greater than 0".
Emulator: emits the generic "1 validation error detected: Value at 'executionInputs.automation.targetMaps.1.member.<Key>.member.1.member' failed to satisfy constraint: Member must have length less than or equal to 50".
Fix: StartExecutionPreview TargetMap validation message.
Traces: ssm-execution-preview-target-map-value-length#task#4.

Cause 10 of 10: StartAutomationExecution's ClientToken refusal echoes the value.
AWS: "2 validation errors detected: Value at 'clientToken' failed to satisfy constraint: Member must have length greater than or equal to 36; Value at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}" — no value quoted.
Emulator: quotes the submitted token in both clauses.
Fix: the validation message renderer.
Traces: ssm-automation-client-token-fixed-length#task#3.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-automation-client-token-fixed-length#task#4: $.AutomationExecutionId expected the token-derived id, got an unrelated UUID.
Cause 2 #task#5: $.AutomationExecution.AutomationExecutionId same divergence.
Cause 3 #task#7: $.AutomationExecutionMetadataList[0].LogFile/Outputs/ResolvedTargets/Targets missing (expected '', {}, {'ParameterValues': [], 'Truncated': False}, []) plus the wrong id.
Cause 4 ssm-automation-execution-mode-enum#task#5: the same four members missing.
Cause 5 ssm-automation-target-parameter-name-length-bound#task#5: $.AutomationExecution.MaxErrors expected '100%' got '1'; $.AutomationExecution.StepExecutions and TargetsSummary missing.
Cause 6 ssm-automation-executions-filter-key-enum#task#3: $.message enum set lacks ScheduledTimeAfter/ScheduledTimeBefore and echoes "Value 'DocumentName'".
Cause 7 ssm-automation-stop-type-enum#task#4: $.message echoes "Value 'Pause'".
Cause 8 ssm-automation-step-filter-value-length-bound#task#5: $.message path and constraint set diverge as quoted.
Cause 9 ssm-execution-preview-target-map-value-length#task#4: $.message expected 'Member of TargetMap value must have length less than or equal to 50 and greater than 0'.
Cause 10 ssm-automation-client-token-fixed-length#task#3: $.message quotes the token AWS omits.

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Causes 1-3, 10: fixture ssm-automation-client-token-fixed-length, steps task#3..task#7.
3. Cause 4: fixture ssm-automation-execution-mode-enum step task#5.
4. Cause 5: fixture ssm-automation-target-parameter-name-length-bound step task#5.
5. Cause 6: fixture ssm-automation-executions-filter-key-enum step task#3.
6. Cause 7: fixture ssm-automation-stop-type-enum step task#4.
7. Cause 8: fixture ssm-automation-step-filter-value-length-bound step task#5.
8. Cause 9: fixture ssm-execution-preview-target-map-value-length step task#4.
