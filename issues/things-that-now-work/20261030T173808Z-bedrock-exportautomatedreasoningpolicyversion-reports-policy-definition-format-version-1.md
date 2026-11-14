Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-automated-reasoning-policy-definition-round-trip
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-automated-reasoning-policy-definition-round-trip
Issue-Shape-Id: com.amazonaws.bedrock#AmazonBedrockControlPlaneService
Issue-Key: bedrock|ExportAutomatedReasoningPolicyVersion|2xx->2xx|$.version
Issue-Commit: f7507ccd8e733df5ba5b0e6ba5890532707d636f

# bedrock ExportAutomatedReasoningPolicyVersion reports policy-definition format version '1.0' instead of the modeled default '1'

> _Note: this issue was identified against the code base at commit `f7507ccd8e733df5ba5b0e6ba5890532707d636f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExportAutomatedReasoningPolicyVersion

## Description
Cause 1 of 1: the exported Automated Reasoning policy definition carries a wrong `version` (policy-definition FORMAT version) value.

AWS: ExportAutomatedReasoningPolicyVersion (GET /automated-reasoning-policies/{policyArn}/export, 200) returns the AutomatedReasoningPolicyDefinition structure as its httpPayload (Smithy: ExportAutomatedReasoningPolicyVersionResponse.policyDefinition, @httpPayload @required, target AutomatedReasoningPolicyDefinition). Smithy: AutomatedReasoningPolicyDefinition.version targets AutomatedReasoningPolicyFormatVersion (plain string, no enum/pattern/length constraint) and carries @default "1", documented as "The version of the policy definition format." So the exported document's `version` member is the format-version token "1" — a single-component literal, NOT a dotted release number, and NOT the policy's own resource version (the policy version is carried separately, e.g. in the `:N` suffix of the policy ARN and in CreateAutomatedReasoningPolicyVersion's `version` output).

Emulator: the same member is serialized as the string "1.0". Replay of bedrock-automated-reasoning-policy-definition-round-trip step task#2 reports status 200 == 200 but `$.version: value_mismatch (expected '1', got '1.0')`. No status, error code or error message is involved — this is a success-path payload value.

Why: a client that reads the format version to decide how to parse an exported definition (or that validates it against the one documented/defaulted value) sees an unrecognised format "1.0" and either rejects the export or takes an unknown-format branch. It also breaks the export/import round trip: the definition handed back by Export is meant to be feedable straight into CreateAutomatedReasoningPolicy / UpdateAutomatedReasoningPolicy, whose `policyDefinition.version` member has the same "1" default, so a definition stamped "1.0" is not the document AWS would have produced for the same policy. Because the mismatch is in a defaulted constant rather than in state, it affects every export in the emulator, not one policy.

Fix: bedrock's Automated Reasoning policy-definition serializer / the default used when the policy definition is materialized for ExportAutomatedReasoningPolicyVersion (the handler's output mapping for policyDefinition, and the stored definition's format-version field in the Automated Reasoning policy state so Get/Create/Update agree). The value must be the modeled default "1" whenever the request did not supply a format version, and must otherwise echo the format version that was stored with the policy definition.

Traces: bedrock-automated-reasoning-policy-definition-round-trip#task#2 (ExportAutomatedReasoningPolicyVersion, 200 vs 200, $.version). Note: this step is NOT covered by any open bedrock issue — the replay reports no owner for it, and the nearest filed items (20261029T211357Z-bedrock-resource-state-defects-definitionhash-policy-version-creation.md, which covers definitionHash and CreateAutomatedReasoningPolicyVersion, and the now-resolved 20261029T172312Z-bedrock-exportautomatedreasoningpolicyversion-stub.md) do not carry this replay key.

## Evidence
Fixture bedrock-automated-reasoning-policy-definition-round-trip, step task#2, operation ExportAutomatedReasoningPolicyVersion.
Expected status 200, actual status 200 (status agrees).
Body divergence: $.version: value_mismatch (expected '1', got '1.0').
Model evidence (Smithy, com.amazonaws.bedrock): AutomatedReasoningPolicyDefinition.version -> AutomatedReasoningPolicyFormatVersion (string), default "1", doc "The version of the policy definition format."; ExportAutomatedReasoningPolicyVersionResponse.policyDefinition is @httpPayload @required with target AutomatedReasoningPolicyDefinition; http binding GET /automated-reasoning-policies/{policyArn}/export, code 200.
Fixed text / variable parts: `version` is a fixed defaulted constant, not a per-request value — "1" is the whole requirement; no part of it is derived from the request, the account, the region or the clock.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay wire fixture bedrock-automated-reasoning-policy-definition-round-trip against it (trace set simparity-wire-aws-v1).
3. The fixture creates an Automated Reasoning policy with a policy definition, then at step task#2 calls ExportAutomatedReasoningPolicyVersion on that policy's ARN (GET /automated-reasoning-policies/{policyArn}/export).
4. Read the 200 response payload (the AutomatedReasoningPolicyDefinition document) and inspect its `version` member.
5. Observed: "1.0". Expected (recorded AWS response and the modeled @default): "1".
