Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-windfarm-trajwire-asset-association-mistakes
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-windfarm-trajwire-asset-association-mistakes iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes iotsitewise-windfarm-trajwire-teardown-order-mistakes
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|AssociateAssets|4xx->4xx|$.message
Issue-Key: iotsitewise|AssociateAssets|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iotsitewise|AssociateAssets|4xx->4xx|status
Issue-Key: iotsitewise|CreateAssetModel|4xx->4xx|$.message
Issue-Key: iotsitewise|CreateAssetModel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iotsitewise|CreateAssetModel|4xx->4xx|status
Issue-Key: iotsitewise|DescribeAssetModel|4xx->4xx|$.message
Issue-Key: iotsitewise|DescribeAssetModel|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iotsitewise|DescribeAssetModel|4xx->4xx|status
Issue-Key: iotsitewise|DeleteAsset|4xx->4xx|$.message
Issue-Key: iotsitewise|DisassociateAssets|4xx->4xx|$.message
Issue-Key: iotsitewise|DeleteAssetModel|2xx->4xx|status
Issue-Key: iotsitewise|DeleteAssetModel|2xx->4xx|$.assetModelId
Issue-Key: iotsitewise|DeleteAssetModel|2xx->4xx|$.assetModelStatus
Issue-Commit: bd9fe9d226ef24de2d25d46b8c18046d76d56a4d

# Asset hierarchy and delete refusals answer the wrong error class and emulator-authored messages

> _Note: this issue was identified against the code base at commit `bd9fe9d226ef24de2d25d46b8c18046d76d56a4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAssets

## Description
Eight distinct refusal paths across the asset/asset-model hierarchy surface answer a different error class, a different HTTP status, or a message the emulator wrote itself, instead of the refusal AWS produces. All eight are the same defect: the handler picks its error from its own internal state check rather than from the rule AWS applies at that point, and authors its own sentence. The open issue 20261029T194801Z-iotsitewise-asset-hierarchy-refusals-answer-the-wrong-error-class.md describes a neighbouring family but does not carry any of these replay keys, so none of these are covered by it.

Cause 1 of 8: AssociateAssets with a well-formed hierarchyId that the parent asset's model does not declare.
  AWS: Smithy: AssociateAssets.hierarchyId -> CustomID (@length 13..139, @pattern UUID|externalId:...); modelled errors include InvalidRequestException (httpError 400) and ResourceNotFoundException (httpError 404). Recorded behavior: AWS answers 400 with message 'Given HierarchyId <hierarchyId> not present in the Asset <assetId>'. Fixed text is 'Given HierarchyId ' / ' not present in the Asset '; the two variable parts are the hierarchyId from the request body and the assetId from the request URI label.
  Emulator: answers 404 with x-amzn-errortype ResourceNotFoundException and message "A hierarchy with ID <hierarchyId> doesn't exist".
  Why: a client treats 404/ResourceNotFoundException as 'the asset or hierarchy is gone' and retries or recreates, when AWS is telling it the request itself is wrong (that hierarchy is not on this asset's model). Retry/backoff and error-branching logic keyed on the error class takes the wrong branch.
  Fix: the AssociateAssets handler's hierarchy-resolution branch must raise InvalidRequestException with AWS's sentence instead of ResourceNotFoundException.
  Traces: iotsitewise-windfarm-trajwire-asset-association-mistakes#task#10 (expected 400, got 404).

Cause 2 of 8: AssociateAssets where the child asset is already associated under some parent.
  AWS: Smithy: AssociateAssets errors include InvalidRequestException (400), ConflictingOperationException (409), ResourceAlreadyExistsException (409). Recorded behavior: AWS answers 400 with the fixed message 'Child asset already has a parent' — the whole message is fixed text, with no variable part.
  Emulator: answers 409 with x-amzn-errortype ConflictingOperationException and message 'An operation associating the asset <childAssetId> with the asset <assetId> through the hierarchy <hierarchyId> is already in progress.'
  Why: ConflictingOperationException means 'a concurrent operation is in flight, retry later', so a client retries a request that can never succeed until it disassociates the child. The 400 AWS returns is terminal and tells the client its topology is wrong.
  Fix: the AssociateAssets handler's parent-occupancy check must raise InvalidRequestException with the fixed sentence, not the in-progress conflict it currently reuses.
  Traces: iotsitewise-windfarm-trajwire-asset-association-mistakes#task#12 (expected 400, got 409).

Cause 3 of 8: CreateAssetModel whose assetModelHierarchies[].childAssetModelId is not an id at all (a model NAME was passed).
  AWS: Smithy: AssetModelHierarchyDefinition.childAssetModelId -> CustomID, required, @length 13..139, @pattern '^(?!00000000-0000-0000-0000-000000000000)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$|^externalId:[a-zA-Z0-9_][a-zA-Z_\-0-9.:]*[a-zA-Z0-9_]+'. A value failing that pattern is refused by request validation BEFORE any lookup: 400 InvalidRequestException with the standard constraint report "1 validation error detected: Value at 'assetModelHierarchies.<n>.member.childAssetModelId' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>".
  Emulator: skips the pattern check, goes straight to a state lookup, and answers 404 ResourceNotFoundException 'Asset Model with ID <value> not found'.
  Why: the client is told the referenced model does not exist, so it creates or waits for a model, when the real fault is that it passed a name where an id/externalId reference belongs. Syntactic validation must precede existence lookup, as it does on every other member of this input.
  Fix: CreateAssetModel input validation — apply the childAssetModelId @pattern/@length constraints before the hierarchy target is resolved against state.
  Traces: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#task#3 (expected 400, got 404).

Cause 4 of 8: DescribeAssetModel with an assetModelId that is not a valid id reference.
  AWS: Smithy: DescribeAssetModel.assetModelId -> CustomID, required @httpLabel, @length 13..139, @pattern UUID|externalId:... . A non-conforming label is refused at validation: 400 InvalidRequestException "1 validation error detected: Value at 'assetModelId' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>".
  Emulator: answers 404 ResourceNotFoundException 'Asset Model with ID <value> not found'.
  Why: same inversion as cause 3 on the read path — a malformed reference is reported as a missing resource, so a client's 'create if absent' logic will try to create a model it already has under a different reference form.
  Fix: DescribeAssetModel input validation — enforce the assetModelId label constraints before the state lookup.
  Traces: iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#5 (expected 400, got 404).

Cause 5 of 8: DeleteAsset on an asset that is still a child in a hierarchy.
  AWS: Smithy: DeleteAsset errors include InvalidRequestException (400). Recorded behavior: AWS answers 400 with the fixed message 'You must disassociate the asset from its parent asset before you can delete it.' — entirely fixed text, no ids interpolated.
  Emulator: right status (400) but its own sentence: 'The asset <childId> is associated with the asset <parentId> through the hierarchy <hierarchyId>. Disassociate it before deleting it.'
  Why: the message is part of the contract. Clients and test suites that match on AWS's wording do not recognise the emulator's sentence, and the emulator's variant leaks ids AWS does not put in this message.
  Fix: DeleteAsset handler's parent-association guard — emit AWS's fixed sentence.
  Traces: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#10 (and task#11, same cluster).

Cause 6 of 8: DisassociateAssets naming a hierarchy that is not on the asset.
  AWS: Smithy: DisassociateAssets errors include ResourceNotFoundException (404). Recorded behavior: AWS answers 404 with message "A hierarchy with ID <hierarchyId> doesn't exist" — fixed text 'A hierarchy with ID ' / " doesn't exist", variable part the hierarchyId from the request.
  Emulator: right status (404) but reports the association rather than the hierarchy: 'Asset with ID <childAssetId> is not associated with the hierarchy with ID <hierarchyId> of asset with ID <assetId>'.
  Why: the emulator asserts the hierarchy exists and merely lacks this child, which is the opposite of what AWS reports; a client that branches on the message (or logs it for an operator) draws the wrong conclusion about which object is missing.
  Fix: DisassociateAssets handler — distinguish 'hierarchy absent' from 'child not in hierarchy' and emit AWS's hierarchy-absent sentence for the former.
  Traces: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#13.

Cause 7 of 8: DeleteAssetModel on a model AWS accepts for deletion is refused 400.
  AWS: Smithy: DeleteAssetModel http code 202, output assetModelId + assetModelStatus (required), errors InvalidRequestException (400) / ResourceNotFoundException (404) / ConflictingOperationException (409). Recorded behavior: AWS accepts the delete with 202 and an assetModelStatus of {'state': 'DELETING'}.
  Emulator: refuses the same request with 400 and emits no assetModelId/assetModelStatus at all.
  Why: teardown cannot complete. Every client that deletes a model it just created, or a model whose references it has already removed, is blocked, and an orchestrator sees a terminal 400 on a legal delete.
  Fix: DeleteAssetModel handler's precondition gating — the state it refuses on does not match AWS's; see also the open issue 20261102T084333Z-iotsitewise-illegal-asset-model-definitions-settle-active-instead-of.md, which reports the gating as inverted but carries neither of these replay keys.
  Traces: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#teardown#13 (expected 202, got 400).

Cause 8 of 8: the same DeleteAssetModel refusal on the teardown-order fixture, where AWS returns 202 with assetModelStatus {'state': 'DELETING'} and the emulator returns 400 with no body members.
  AWS: as cause 7 — DeleteAssetModel is 202-accepted and reports DELETING in assetModelStatus.
  Emulator: 400, assetModelId and assetModelStatus both absent.
  Why: as cause 7; recorded separately because it is a different fixture and a different model state reaching the same broken gate, so a fix validated on only one of the two can still leave this one refusing.
  Fix: same component as cause 7 (DeleteAssetModel precondition gating).
  Traces: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#18 (expected 202, got 400).

## Evidence
Cause 1: iotsitewise-windfarm-trajwire-asset-association-mistakes#task#10 — expected 400 / message 'Given HierarchyId <id> not present in the Asset <assetId>'; actual 404, x-amzn-errortype ResourceNotFoundException, message "A hierarchy with ID <id> doesn't exist".
Cause 2: ...#task#12 — expected 400 / 'Child asset already has a parent'; actual 409 ConflictingOperationException / 'An operation associating the asset ... is already in progress.'
Cause 3: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#task#3 — expected 400 / "1 validation error detected: Value at 'assetModelHierarchies.1.member.childAssetModelId' failed to satisfy constraint: Member must satisfy regular expression pattern: ..."; actual 404 ResourceNotFoundException / 'Asset Model with ID WindTurbineModel not found'.
Cause 4: iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes#task#5 — expected 400 / "1 validation error detected: Value at 'assetModelId' failed to satisfy constraint: Member must satisfy regular expression pattern: ..."; actual 404 ResourceNotFoundException / 'Asset Model with ID WindTurbineModel not found'.
Cause 5: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#10 and #task#11 — expected 400 / 'You must disassociate the asset from its parent asset before you can delete it.'; actual 400 / 'The asset <id> is associated with the asset <id> through the hierarchy <id>. Disassociate it before deleting it.'
Cause 6: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#13 — expected 404 / "A hierarchy with ID <id> doesn't exist"; actual 404 / 'Asset with ID <id> is not associated with the hierarchy with ID <id> of asset with ID <id>'.
Cause 7: iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes#teardown#13 — expected status 202, got 400.
Cause 8: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#18 — expected 202 with assetModelId + assetModelStatus {'state':'DELETING'}, got 400 with both members missing.
Smithy grounding (services/service-com-amazonaws-iotsitewise-awsiotsitewise/smithy-model/com-amazonaws-iotsitewise-awsiotsitewise.json): AssociateAssets/DisassociateAssets/DeleteAsset/DeleteAssetModel error sets and httpError codes; CustomID @length 13..139 and UUID|externalId @pattern on assetId/hierarchyId/childAssetId/assetModelId; AssetModelHierarchyDefinition.childAssetModelId required CustomID; DeleteAssetModel http code 202 with required assetModelStatus.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Cause 1/2: replay iotsitewise-windfarm-trajwire-asset-association-mistakes and inspect task#10 (AssociateAssets with a hierarchyId absent from the parent's model) and task#12 (AssociateAssets for a child that already has a parent); compare status, x-amzn-errortype and $.message.
3. Cause 3: replay iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes task#3 (CreateAssetModel with a hierarchy childAssetModelId given as a model name).
4. Cause 4: replay iotsitewise-windfarm-trajwire-rerun-and-lookup-mistakes task#5 (DescribeAssetModel with a non-id assetModelId label).
5. Cause 5/6: replay iotsitewise-windfarm-trajwire-teardown-order-mistakes task#10, task#11 (DeleteAsset on a child still associated) and task#13 (DisassociateAssets naming a hierarchy not on the asset).
6. Cause 7/8: replay iotsitewise-windfarm-trajwire-hierarchy-reference-mistakes teardown#13 and iotsitewise-windfarm-trajwire-teardown-order-mistakes task#18 (DeleteAssetModel) and confirm the 202 + assetModelStatus DELETING response.
