Issue-Source: trace-sweep-divergence
Issue-Trace: compliance-items-filter-type-enum
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compliance-items-filter-type-enum ssm-association-executions-filter-key-outside-enum ssm-command-status-filter-values-differ-per-listing ssm-list-command-invocations-filter-count-bounds ssm-instance-information-legacy-filter-value-length-bound ssm-instance-patch-states-max-results-range ssm-instance-property-filter-value-length-bounds ssm-inventory-aggregator-group-filter-type-enum ssm-cancel-command-instance-id-count-bound ssm-cancel-command-repeated-instance-id-refused
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|ListComplianceItems|2xx->4xx|$.ComplianceItems
Issue-Key: ssm|ListComplianceItems|2xx->4xx|status
Issue-Key: ssm|CreateAssociation|2xx->4xx|status
Issue-Key: ssm|ListCommandInvocations|2xx->4xx|$.CommandInvocations
Issue-Key: ssm|ListCommandInvocations|2xx->4xx|status
Issue-Key: ssm|DescribeInstanceInformation|2xx->4xx|$.InstanceInformationList
Issue-Key: ssm|DescribeInstanceInformation|2xx->4xx|status
Issue-Key: ssm|DescribeInstancePatchStates|2xx->4xx|$.InstancePatchStates
Issue-Key: ssm|DescribeInstancePatchStates|2xx->4xx|status
Issue-Key: ssm|DescribeInstancePatchStates|4xx->4xx|$.message
Issue-Key: ssm|DescribeInstanceProperties|2xx->4xx|$.InstanceProperties
Issue-Key: ssm|DescribeInstanceProperties|2xx->4xx|status
Issue-Key: ssm|GetInventory|2xx->4xx|$.Entities
Issue-Key: ssm|GetInventory|2xx->4xx|status
Issue-Key: ssm|CancelCommand|2xx->4xx|$
Issue-Key: ssm|CancelCommand|2xx->4xx|status
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# SSM refuses nine requests AWS accepts, answering 400 where AWS returns 200

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListComplianceItems

## Description
Nine net-new causes sharing one fault: the emulator's input validation is stricter than AWS's, so a request AWS serves 200 is refused 400. Bundled by that shared fault.

Cause 1 of 9: ListComplianceItems refuses a legal filter combination.
AWS: 200 with ComplianceItems: [] for the probed filter set (recorded behavior; the filter Type value probed is accepted by AWS).
Emulator: 400.
Why: a legal compliance query fails outright, so the caller cannot list compliance items at all.
Fix: ListComplianceItems filter validation.
Traces: compliance-items-filter-type-enum#task#6.

Cause 2 of 9: CreateAssociation refuses a legal association create used as fixture setup.
AWS: 200 with the AssociationDescription.
Emulator: 400.
Why: the association never exists, so every dependent read in the flow is unreachable.
Fix: CreateAssociation input validation.
Traces: ssm-association-executions-filter-key-outside-enum#setup#2.

Cause 3 of 9: ListCommandInvocations refuses a legal filter set.
AWS: 200 with CommandInvocations: [].
Emulator: 400.
Fix: ListCommandInvocations filter validation.
Traces: ssm-command-status-filter-values-differ-per-listing#task#3.

Cause 4 of 9: DescribeInstanceInformation refuses a legal legacy filter value.
AWS: 200 with InstanceInformationList: [].
Emulator: 400.
Fix: DescribeInstanceInformation legacy-filter validation.
Traces: ssm-instance-information-legacy-filter-value-length-bound#task#2.

Cause 5 of 9: DescribeInstancePatchStates refuses a MaxResults AWS accepts.
AWS: 200 with InstancePatchStates: [] for a MaxResults below 10 (the service's floor is 1, see cause 6).
Emulator: 400.
Fix: DescribeInstancePatchStates input validation.
Traces: ssm-instance-patch-states-max-results-range#task#1.

Cause 6 of 9: DescribeInstancePatchStates states the wrong MaxResults floor in its refusal.
AWS: for maxResults 0, 400 ValidationException "1 validation error detected: Value '<value>' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1". The limit 1 is the requirement (the on-disk Smithy range min is 10 and is not what the service enforces).
Emulator: same shape but states "greater than or equal to 10".
Why: the message tells the caller a bound AWS does not apply, and the same wrong bound drives cause 5's over-strict refusal.
Fix: DescribeInstancePatchStates input validation (floor 1, ceiling 50 — see the companion ceiling cause filed under the 'accepts requests AWS refuses' issue).
Traces: ssm-instance-patch-states-max-results-range#task#5.

Cause 7 of 9: DescribeInstanceProperties refuses a legal filter value.
AWS: 200 with InstanceProperties: [].
Emulator: 400.
Fix: DescribeInstanceProperties filter validation.
Traces: ssm-instance-property-filter-value-length-bounds#task#1.

Cause 8 of 9: GetInventory refuses a legal aggregator group filter.
AWS: 200 with Entities carrying the group's matchingCount/notMatchingCount content.
Emulator: 400.
Why: inventory group aggregation is unusable for a request AWS answers.
Fix: GetInventory aggregator/group filter validation.
Traces: ssm-inventory-aggregator-group-filter-type-enum#task#1.

Cause 9 of 9: CancelCommand refuses instance ids AWS accepts.
AWS: 200 with an empty body {} — CancelCommand does not require the instance to be a live managed node.
Emulator: 400, __type InvalidInstanceId, Message 'Instances not in a valid state for account'.
Why: a cancel of a command targeting ids that are not online fails instead of succeeding, so cancel flows cannot complete.
Fix: the CancelCommand handler's instance precondition.
Traces: ssm-cancel-command-instance-id-count-bound#task#3.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 compliance-items-filter-type-enum#task#6: expected 200 with $.ComplianceItems [], got 400.
Cause 2 ssm-association-executions-filter-key-outside-enum#setup#2: expected 200, got 400.
Cause 3 ssm-command-status-filter-values-differ-per-listing#task#3: expected 200 with $.CommandInvocations [], got 400.
Cause 4 ssm-instance-information-legacy-filter-value-length-bound#task#2: expected 200 with $.InstanceInformationList [], got 400.
Cause 5 ssm-instance-patch-states-max-results-range#task#1: expected 200 with $.InstancePatchStates [], got 400.
Cause 6 ssm-instance-patch-states-max-results-range#task#5: $.message expected "...Member must have value greater than or equal to 1", got "...greater than or equal to 10".
Cause 7 ssm-instance-property-filter-value-length-bounds#task#1: expected 200 with $.InstanceProperties [], got 400.
Cause 8 ssm-inventory-aggregator-group-filter-type-enum#task#1: expected 200 with $.Entities [{'Data': {'<group>': {'Content': [{'notMatchingCount': '0'}, {'matchingCount': '0'}]}}}], got 400.
Cause 9 ssm-cancel-command-instance-id-count-bound#task#3: expected 200 with {}, got 400 {'__type': 'InvalidInstanceId', 'Message': 'Instances not in a valid state for account'}.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus.
2. Replay each listed fixture and inspect the named step: the step sends a request AWS answers 200 and the emulator answers 400.
3. Representative: fixture compliance-items-filter-type-enum step task#6 (ListComplianceItems).
