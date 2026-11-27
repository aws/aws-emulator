Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__ssm.deployment-strategy-replicated-to-ssm-document-mirrors-it-and-is-deleted-with-it appconfig__ssm.replicate-to-ssm-document-rejected-when-the-document-name-is-taken
Issue-Key: appconfig__ssm.deployment-strategy-replicated-to-ssm-document-mirrors-it-and-is-deleted-with-it|_wait_for_document#13|AssertionError|raise AssertionError(f"no SSM document named {name!r} appeared for the strategy")
Issue-Key: appconfig__ssm.replicate-to-ssm-document-rejected-when-the-document-name-is-taken|_refusal#12|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# appconfig__ssm: 2 causes — a deployment strategy with ReplicateTo SSM_DOCUMENT creates no SSM document and does not refuse a name already taken

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeploymentStrategy

## Description
Cause 1 of 2: ReplicateTo SSM_DOCUMENT does not create the mirroring SSM document.
AWS: CreateDeploymentStrategy with ReplicateTo SSM_DOCUMENT writes an SSM document named after the strategy that mirrors it, and DeleteDeploymentStrategy deletes that document with it.
Emulator: the strategy is created (HTTP 201 with ReplicateTo 'SSM_DOCUMENT') but no SSM document named for the strategy ever appears, so the mirror and its deletion cannot be observed.
Why: a client that reads the replicated document (the documented way to share a strategy) finds nothing, while AppConfig reports the replication was configured.
Fix: the ReplicateTo handling in the AppConfig CreateDeploymentStrategy/DeleteDeploymentStrategy handlers, writing/removing the SSM document.
Traces: appconfig__ssm.deployment-strategy-replicated-to-ssm-document-mirrors-it-and-is-deleted-with-it.

Cause 2 of 2: the same call is not refused when the document name is already taken.
AWS: if an SSM document with the target name already exists, CreateDeploymentStrategy with ReplicateTo SSM_DOCUMENT is refused (HTTP 4xx) and no strategy is created.
Emulator: answers 201 with the full strategy ({'Id': 'algcbw8', 'Name': '<strategy>', 'ReplicateTo': 'SSM_DOCUMENT', …}) despite the name conflict.
Why: two owners end up believing they own the same document name; the conflict surfaces later, or never.
Fix: check the SSM document namespace before accepting the strategy in the same handler.
Traces: appconfig__ssm.replicate-to-ssm-document-rejected-when-the-document-name-is-taken.

## Evidence
Cause 1: "no SSM document named 'sim2real-xsvc-ds-b80f712c19d1' appeared for the strategy".
Cause 2: "expected a refusal, but the call succeeded: {'HTTPStatusCode': 201, 'Id': 'algcbw8', 'Name': 'sim2real-xsvc-ds-71968f34b394', 'DeploymentDurationInMinutes': 0, 'GrowthType': 'LINEAR', 'GrowthFactor': 100.0, 'FinalBakeTimeInMinutes': 0, 'ReplicateTo': 'SSM_DOCUMENT'}".

## Steps to Reproduce
1. CreateDeploymentStrategy(ReplicateTo='SSM_DOCUMENT'); then SSM DescribeDocument/ListDocuments for a document named after the strategy; DeleteDeploymentStrategy and check the document is gone.
2. CreateDocument an SSM document with the name the strategy would replicate to, then CreateDeploymentStrategy(ReplicateTo='SSM_DOCUMENT') with that name.
