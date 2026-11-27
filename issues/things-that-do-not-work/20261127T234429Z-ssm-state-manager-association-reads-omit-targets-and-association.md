Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-association-batch-target-map-key-beyond-model-length-accepted
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-association-batch-target-map-key-beyond-model-length-accepted ssm-create-association-target-location-concurrency-length ssm-association-target-location-targets-max-concurrency-pattern ssm-describe-association-version-pattern ssm-list-associations-filter-key-enum ssm-association-executions-invalid-next-token
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm|CreateAssociationBatch|2xx->2xx|$.Successful[*].Targets
Issue-Key: ssm|DescribeAssociation|2xx->2xx|$.AssociationDescription.Targets
Issue-Key: ssm|CreateAssociation|2xx->2xx|$.AssociationDescription.Targets
Issue-Key: ssm|CreateAssociation|4xx->4xx|$.message
Issue-Key: ssm|DescribeAssociation|4xx->4xx|$.message
Issue-Key: ssm|DescribeAssociation|4xx->4xx|$.Message
Issue-Key: ssm|ListAssociations|4xx->4xx|$.message
Issue-Key: ssm|DescribeAssociationExecutions|4xx->4xx|$.Message
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# State Manager association reads omit Targets and association refusals diverge from AWS

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssociation

## Description
Eight net-new causes on the State Manager association plane: the Targets a client submitted are dropped from every association projection, and the association refusal bodies diverge.

Cause 1 of 8: CreateAssociationBatch drops Targets from the success entry.
AWS: Successful[].Targets echoes the submitted target list (key and values). Smithy: AssociationDescription$Targets (list, length 0..5).
Emulator: member absent.
Why: a client that reads back what it registered cannot confirm the targets, and tooling that re-submits the returned description registers an association with no targets.
Fix: the CreateAssociationBatch response projection / association state write.
Traces: ssm-association-batch-target-map-key-beyond-model-length-accepted#task#3.

Cause 2 of 8: DescribeAssociation drops Targets.
AWS: AssociationDescription.Targets echoes the registered targets.
Emulator: member absent.
Fix: the DescribeAssociation projection.
Traces: ssm-association-batch-target-map-key-beyond-model-length-accepted#task#4.

Cause 3 of 8: CreateAssociation drops Targets from its own response.
AWS: AssociationDescription.Targets echoes the submitted targets.
Emulator: member absent.
Fix: the CreateAssociation response projection.
Traces: ssm-create-association-target-location-concurrency-length#task#5.

Cause 4 of 8: CreateAssociation quotes the targetsMaxConcurrency pattern un-anchored.
AWS: "1 validation error detected: Value '101%' at 'targetLocations.1.member.targetsMaxConcurrency' failed to satisfy constraint: Member must satisfy regular expression pattern: ^([1-9][0-9]*|[1-9][0-9]%|[1-9]%|100%)$" — here AWS does quote the value, and the pattern carries its ^...$ anchors.
Emulator: same body with the anchors stripped.
Why: the quoted grammar admits values AWS refuses.
Fix: the validation message renderer (emit the pattern as modeled).
Traces: ssm-association-target-location-targets-max-concurrency-pattern#task#1.

Cause 5 of 8: DescribeAssociation re-brackets the associationVersion pattern.
AWS: "Member must satisfy regular expression pattern: ([$]LATEST)|([1-9][0-9]*)".
Emulator: emits "(([$]LATEST)|([1-9][0-9]*))" — an extra group AWS does not send.
Fix: the validation message renderer for associationVersion.
Traces: ssm-describe-association-version-pattern#task#4.

Cause 6 of 8: DescribeAssociation's unknown-version message is emulator-authored.
AWS: 400 with Message "Association version <n> does not exist for association <associationId>. Association version must be greater than 0 and less than or equal to the latest version. The latest version is <latest>." — fixed text with the requested version, the association id and the current latest version.
Emulator: "The version you specified isn't valid. Use ListAssociationVersions to view all versions of an association according to the association ID. Or, use the $LATEST parameter to view the latest version of the association."
Why: the caller loses the only machine-usable facts AWS supplies (which version was asked for and what the latest is).
Fix: the DescribeAssociation version lookup refusal message.
Traces: ssm-describe-association-version-pattern#task#7.

Cause 7 of 8: ListAssociations' filter-key enum set is missing PluginName.
AWS: the accepted set is [LastExecutedBefore, Name, AssociationName, InstanceId, CloudConnectorId, LastExecutedAfter, ResourceGroupName, AssociationId, PluginName, AssociationStatusName].
Emulator: omits PluginName (the on-disk AssociationFilterKey enum also omits it; the recording is the wire requirement), so a PluginName filter is refused and the message misstates the set.
Fix: the AssociationFilterKey enum used by validation plus the message.
Traces: ssm-list-associations-filter-key-enum#task#2.

Cause 8 of 8: DescribeAssociationExecutions adds a Message AWS does not send.
AWS: refuses an invalid NextToken with 400 and no Message member in the body.
Emulator: adds Message 'The NextToken value is not valid'.
Why: the body carries a field AWS omits, which clients that enumerate response members treat as a different error shape.
Fix: the DescribeAssociationExecutions InvalidNextToken error body.
Traces: ssm-association-executions-invalid-next-token#task#3.

## Evidence
Replay of simparity-wire-aws-v1.
Cause 1 ssm-association-batch-target-map-key-beyond-model-length-accepted#task#3: $.Successful[0].Targets missing (expected [{'Key': 'aws:NoOpAutomationTag', 'Values': ['AWS-NoOpAutomationTarget-Value']}]).
Cause 2 #task#4: $.AssociationDescription.Targets missing (same expected value).
Cause 3 ssm-create-association-target-location-concurrency-length#task#5: $.AssociationDescription.Targets missing.
Cause 4 ssm-association-target-location-targets-max-concurrency-pattern#task#1: pattern quoted without ^...$.
Cause 5 ssm-describe-association-version-pattern#task#4: expected '([$]LATEST)|([1-9][0-9]*)', got '(([$]LATEST)|([1-9][0-9]*))'.
Cause 6 #task#7: $.Message expected 'Association version 2 does not exist for association <id>. Association version must be greater than 0 and less than or equal to the latest version. The latest version is 1.'
Cause 7 ssm-list-associations-filter-key-enum#task#2: enum set in $.message lacks PluginName.
Cause 8 ssm-association-executions-invalid-next-token#task#3: $.Message unexpected_present (expected None, got 'The NextToken value is not valid').

## Steps to Reproduce
1. Boot the emulator and replay the corpus.
2. Causes 1-2: fixture ssm-association-batch-target-map-key-beyond-model-length-accepted steps task#3, task#4.
3. Cause 3: fixture ssm-create-association-target-location-concurrency-length step task#5.
4. Cause 4: fixture ssm-association-target-location-targets-max-concurrency-pattern step task#1.
5. Causes 5-6: fixture ssm-describe-association-version-pattern steps task#4, task#7.
6. Cause 7: fixture ssm-list-associations-filter-key-enum step task#2.
7. Cause 8: fixture ssm-association-executions-invalid-next-token step task#3.
