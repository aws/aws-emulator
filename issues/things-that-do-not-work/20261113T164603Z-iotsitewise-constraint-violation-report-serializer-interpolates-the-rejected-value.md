Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-windfarm-trajwire-property-definition-mistakes
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-windfarm-trajwire-property-definition-mistakes iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes iotsitewise-windfarm-trajwire-asset-association-mistakes
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|CreateAssetModel|4xx->4xx|$.message
Issue-Key: iotsitewise|AssociateAssets|4xx->4xx|$.message
Issue-Key: iotsitewise|DescribeAssetModel|4xx->4xx|$.message
Issue-Commit: 87e7668771aa8c29b5e1dffb34be8842e4b92b4f

# Constraint-violation report serializer interpolates the rejected value and stops at the first violation

> _Note: this issue was identified against the code base at commit `87e7668771aa8c29b5e1dffb34be8842e4b92b4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetModel

## Description
The emulator answers the right status (400) and the right error class for every input-constraint refusal below, but it composes its own text for AWS's standard constraint-violation report. Two deviations recur in the shared serializer: (a) it interpolates the rejected value ("Value '<rejected>' at '<member>'") where AWS's sentence for these members is value-free ("Value at '<member>'"), and (b) it short-circuits after the first violation a member incurs where AWS accumulates every violation and declares the true count. It also renders an enum value set in the model's declaration order rather than AWS's.

The AWS template for this error class is: "<N> validation error(s) detected: " followed by one clause per violation joined with "; ", each clause reading "Value at '<memberPath>' failed to satisfy constraint: <constraint phrase>". FIXED text: "validation error detected: " / "validation errors detected: ", "Value at '", "' failed to satisfy constraint: ", and the constraint phrases "Member must satisfy enum value set: ", "Member must satisfy regular expression pattern: ", "Member must have length greater than or equal to ". VARIABLE parts: N (the number of violations actually detected), the member path (including list index and ".member" for list elements), and the limit / pattern / enum-set text taken from the Smithy model. The rejected value is NOT part of the template for these members.

This bundle has 3 causes; all three must be closed. NOTE: open issue 20261103T025010Z-iotsitewise-validation-messages-quote-the-rejected-value-aws-omits.md describes the same serializer family, but its replay key binds only the ListAssetModels maxResults cluster (iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#8), so the three clusters below are not covered by any filed replay pointer and are filed here.

Cause 1 of 3: CreateAssetModel with an invalid assetModelProperties[].dataType — fixture iotsitewise-windfarm-trajwire-property-definition-mistakes, step task#1 (cluster also covers task#2..task#6 of that fixture and iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#4).
  AWS: Smithy: CreateAssetModel.assetModelProperties -> AssetModelPropertyDefinitions, element dataType -> PropertyDataType (required enum). AWS answers 400 with "1 validation error detected: Value at 'assetModelProperties.<n>.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [JSON, BOOLEAN, ANNOTATION, DOUBLE, STRING, STRUCT, VIDEO, INTEGER]" — no rejected value, and the set rendered in that order.
  Emulator: "1 validation error detected: Value 'double' at 'assetModelProperties.1.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [STRING, INTEGER, DOUBLE, BOOLEAN, STRUCT, VIDEO, ANNOTATION, JSON]" — the value clause is inserted and the set is in the on-disk PropertyDataType declaration order.
  Why: clients and conformance tooling match or parse this message; a differently shaped sentence is a different error to them, and echoing the caller's value into a body AWS keeps value-free leaks request content into the error.
  Fix: the shared constraint-report serializer used by CreateAssetModel input validation — drop the value clause and render the enum value set in AWS's order.
  Traces: iotsitewise-windfarm-trajwire-property-definition-mistakes#task#1.

Cause 2 of 3: AssociateAssets with a hierarchyId that violates BOTH the @pattern and the @length constraint — fixture iotsitewise-windfarm-trajwire-asset-association-mistakes, step task#8 (cluster also covers task#9).
  AWS: Smithy: AssociateAssets.hierarchyId -> CustomID, required, @length min 13 max 139, @pattern '^(?!00000000-0000-0000-0000-000000000000)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$|^externalId:[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+$'. A short non-UUID name incurs two violations and AWS reports both: "2 validation errors detected: Value at 'hierarchyId' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>; Value at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13" — pattern clause first, then length.
  Emulator: "1 validation error detected: Value 'Turbines' at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13" — one clause only, the pattern violation never reported, and the value interpolated. Both responses are 400.
  Why: a client that fixes only the violation it was told about re-submits and is refused again on a violation it was never shown; the declared count is itself part of the message contract.
  Fix: the same constraint-report serializer must accumulate all violations a member incurs instead of returning on the first, emit them pattern-then-length, and set N to the number emitted.
  Traces: iotsitewise-windfarm-trajwire-asset-association-mistakes#task#8.

Cause 3 of 3: DescribeAssetModel with an assetModelVersion outside the modelled pattern — fixture iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes, step task#7.
  AWS: Smithy: DescribeAssetModel.assetModelVersion -> AssetModelVersionFilter, @httpQuery assetModelVersion, @pattern '^(LATEST|ACTIVE)$'. AWS answers 400 with "1 validation error detected: Value at 'assetModelVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(LATEST|ACTIVE)$".
  Emulator: the same sentence with the value interpolated — "1 validation error detected: Value 'active' at 'assetModelVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(LATEST|ACTIVE)$". The pattern text itself is correct here, which confirms the defect is in the shared report builder and not in one operation's validator.
  Why: as cause 1 — the refusal is correct but its message is not AWS's.
  Fix: same serializer, query-bound member path.
  Traces: iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#7.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 83.0%, 23 mismatched steps).

Cause 1 — iotsitewise-windfarm-trajwire-property-definition-mistakes#task#1, CreateAssetModel, expected 400 / actual 400, $.message value_mismatch: expected "1 validation error detected: Value at 'assetModelProperties.1.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [JSON, BOOLEAN, ANNOTATION, DOUBLE, STRING, STRUCT, VIDEO, INTEGER]"; got "1 validation error detected: Value 'double' at 'assetModelProperties.1.member.dataType' failed to satisfy constraint: Member must satisfy enum value set: [STRING, INTEGER, DOUBLE, BOOLEAN, STRUCT, VIDEO, ANNOTATION, JSON]". Cluster of 7 steps.

Cause 2 — iotsitewise-windfarm-trajwire-asset-association-mistakes#task#8, AssociateAssets, expected 400 / actual 400, $.message value_mismatch: expected "2 validation errors detected: Value at 'hierarchyId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!00000000-0000-0000-0000-000000000000)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$|^externalId:[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+; Value at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13"; got "1 validation error detected: Value 'Turbines' at 'hierarchyId' failed to satisfy constraint: Member must have length greater than or equal to 13". Cluster of 2 steps (task#8, task#9).

Cause 3 — iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#7, DescribeAssetModel, expected 400 / actual 400, $.message value_mismatch: expected "1 validation error detected: Value at 'assetModelVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(LATEST|ACTIVE)$"; got "1 validation error detected: Value 'active' at 'assetModelVersion' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(LATEST|ACTIVE)$".

The literal values 'double', 'Turbines', 'active' and the index '1' are these recordings filling the template; the requirement is the sentence structure, the clause set and the declared count, with member path / limit / pattern / enum set taken from the request and the model.

Smithy grounding (services/service-com-amazonaws-iotsitewise-awsiotsitewise/smithy-model/com-amazonaws-iotsitewise-awsiotsitewise.json): AssociateAssets.hierarchyId -> CustomID, required, @length 13..139, @pattern UUID|externalId; DescribeAssetModel.assetModelVersion -> AssetModelVersionFilter @httpQuery, @pattern ^(LATEST|ACTIVE)$; CreateAssetModel.assetModelProperties element dataType -> PropertyDataType enum; all three operations model InvalidRequestException at httpError 400.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: POST /asset-models with assetModelProperties[1].dataType = 'double' (wrong case) — fixture iotsitewise-windfarm-trajwire-property-definition-mistakes, step task#1. Diff $.message: confirm no "Value '<rejected>'" clause and the enum set rendered in AWS's order.
3. Cause 2: POST /assets/{assetId}/associate with hierarchyId = a short non-UUID name ('Turbines') — fixture iotsitewise-windfarm-trajwire-asset-association-mistakes, step task#8. Confirm the message declares "2 validation errors detected" and carries the pattern clause followed by the length clause.
4. Cause 3: GET /asset-models/{assetModelId}?assetModelVersion=active — fixture iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes, step task#7. Confirm the value clause is absent.
5. All three stay 400 with the same error class; only $.message must change.
