Issue-Source: trace-sweep-divergence
Issue-Trace: iotsitewise-windfarm-trajwire-teardown-order-mistakes
Issue-Step: task#19
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotsitewise-windfarm-trajwire-teardown-order-mistakes
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelArn
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelCompositeModelSummaries
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelCompositeModels
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelCreationDate
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelHierarchies
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelId
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelLastUpdateDate
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelName
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelProperties
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelStatus
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelStatus.state
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelType
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$.assetModelVersion
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|$headers.etag
Issue-Key: iotsitewise|DescribeAssetModel|2xx->4xx|status
Issue-Key: iotsitewise|DeleteAssetModel|2xx->4xx|$.assetModelId
Issue-Key: iotsitewise|DeleteAssetModel|2xx->4xx|$.assetModelStatus
Issue-Key: iotsitewise|DeleteAssetModel|2xx->4xx|status
Issue-Commit: 87e7668771aa8c29b5e1dffb34be8842e4b92b4f

# DeleteAssetModel of a hierarchy-referenced child erases the model instead of settling it FAILED

> _Note: this issue was identified against the code base at commit `87e7668771aa8c29b5e1dffb34be8842e4b92b4f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAssetModel

## Description
IoT SiteWise validates an asset-model delete on an ASYNCHRONOUS plane: deleting a child model that a parent asset model's hierarchy still references is ACCEPTED (202, assetModelStatus.state=DELETING), the model then settles state=FAILED with assetModelStatus.error, and the model SURVIVES — it stays describable and listable under its original name, properties, hierarchies and version, and a later delete (once the parent's hierarchy reference is removed) succeeds. The emulator instead destroys the model on that first delete, so every subsequent call against it answers 404 ResourceNotFoundException and the FAILED-then-repair teardown flow is unreachable. This bundle has 2 causes; both must be closed.

Cause 1 of 2: after a refused (hierarchy-referenced) delete, DescribeAssetModel answers 404 instead of 200 with the FAILED status — fixture iotsitewise-windfarm-trajwire-teardown-order-mistakes, step task#19.
  AWS: Smithy: DescribeAssetModel GET /asset-models/{assetModelId}, http code 200, output assetModelId/assetModelArn/assetModelName/assetModelProperties/assetModelHierarchies/assetModelCreationDate/assetModelLastUpdateDate/assetModelStatus all @required, plus assetModelVersion, assetModelCompositeModels, assetModelCompositeModelSummaries and the ETag header; assetModelStatus -> AssetModelStatus{state @required, error: ErrorDetails{code @required (ErrorCode [VALIDATION_ERROR, INTERNAL_FAILURE]), message @required}}. The model still exists, so AWS answers 200 with the full projection and assetModelStatus = {state: 'FAILED', error: {code: 'VALIDATION_ERROR', message: 'Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model.'}}. That message is entirely FIXED text with no variable parts; the variable parts of the response are the model's own id, arn, name, properties, hierarchies, timestamps and version, read from state.
  Emulator: answers 404 with {"__type": "ResourceNotFoundException", "message": "Asset Model with ID <assetModelId> not found"} — the model is gone, so every @required output member is absent and the poll for state=FAILED never terminates (it capped at 180s).
  Why: a client that deletes bottom-up in the wrong order must poll the model to learn the delete failed and why. Against the emulator the resource has vanished, so the diagnosis AWS puts in assetModelStatus.error can never be read, and worse, a client believes a model AWS still holds was destroyed — the parent's hierarchy now points at an id the emulator no longer knows.
  Fix: the DeleteAssetModel handler and the asset-model state it writes — a delete blocked by a parent hierarchy reference must leave the model in state and settle it FAILED with ErrorDetails{VALIDATION_ERROR, <the fixed sentence above>} rather than removing it; DescribeAssetModel then serves it normally.
  Traces: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#19.

Cause 2 of 2: the retried DeleteAssetModel, issued after the parent's hierarchy reference is removed, answers 404 instead of 202 DELETING — fixture iotsitewise-windfarm-trajwire-teardown-order-mistakes, step task#22.
  AWS: Smithy: DeleteAssetModel DELETE /asset-models/{assetModelId}, http code 202, output {assetModelId, assetModelStatus @required}; errors InvalidRequestException(400), ConflictingOperationException(409), PreconditionFailedException(412), ResourceNotFoundException(404). Because the earlier delete left the model in place at FAILED, the model is still deletable: once the blocking hierarchy reference is gone AWS answers 202 with {assetModelId: <the same id>, assetModelStatus: {state: 'DELETING'}}.
  Emulator: answers 404 ResourceNotFoundException, so the output members assetModelId and the @required assetModelStatus are both absent. This is a direct consequence of cause 1 — the emulator already deleted the model — but it is the step a teardown script actually depends on, and it must answer 202 DELETING.
  Why: the repair path for a wrong-order teardown (remove the parent hierarchy, then re-issue the delete) is the normal client recovery; against the emulator it reports the resource as missing and a script that treats 404 as a hard failure aborts mid-teardown, while one that treats it as success never observes the DELETING transition.
  Fix: same handler/state as cause 1 — with the model preserved at FAILED, the re-issued delete must take the normal accepted path and return 202 with assetModelStatus.state=DELETING.
  Traces: iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#22.

Related open issues (neither carries a replay key for these two steps): 20261102T084333Z-iotsitewise-illegal-asset-model-definitions-settle-active-instead-of.md and 20261103T025010Z-iotsitewise-invalid-asset-model-definitions-settle-active-instead-of.md cover the missing asynchronous FAILED settle generally; 20261109T115126Z-iotsitewise-deleteassetmodel-accepts-a-delete-on-a-model-still.md covers the CREATING-state gate. The defect filed here is specifically that the blocked delete ERASES the model, so the resource is 404 afterwards.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 83.0%, 23 mismatched steps).

Cause 1 — iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#19, DescribeAssetModel: expected status 200, got 404. Expected body carried assetModelId/assetModelArn/assetModelName/assetModelProperties/assetModelHierarchies/assetModelCompositeModels/assetModelCompositeModelSummaries/assetModelCreationDate/assetModelLastUpdateDate/assetModelVersion and assetModelStatus = {'state': 'FAILED', 'error': {'code': 'VALIDATION_ERROR', 'message': 'Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model.'}}; got {'__type': 'ResourceNotFoundException', 'message': 'Asset Model with ID <assetModelId> not found'}. The fixture's poll on $.assetModelStatus.state capped at 180s because the resource never came back.

Cause 2 — iotsitewise-windfarm-trajwire-teardown-order-mistakes#task#22, DeleteAssetModel: expected status 202 with $.assetModelId = the same model id and $.assetModelStatus = {'state': 'DELETING'}; got 404, both members missing.

The recorded ids, arns, names and timestamps are this run's values filling the response template; the requirements are the statuses (200 then 202), the FAILED/VALIDATION_ERROR status with AWS's fixed sentence, and the model's continued existence.

Smithy grounding (services/service-com-amazonaws-iotsitewise-awsiotsitewise/smithy-model/com-amazonaws-iotsitewise-awsiotsitewise.json): DescribeAssetModel GET /asset-models/{assetModelId} code 200, assetModelStatus @required (AssetModelStatus{state @required, error ErrorDetails{code @required ErrorCode, message @required}}), ResourceNotFoundException httpError 404; DeleteAssetModel DELETE /asset-models/{assetModelId} code 202, output {assetModelId, assetModelStatus @required}, errors InvalidRequestException 400 / ConflictingOperationException 409 / PreconditionFailedException 412 / ResourceNotFoundException 404.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus; fixture iotsitewise-windfarm-trajwire-teardown-order-mistakes.
2. Create a child asset model and a parent asset model declaring a hierarchy whose childAssetModelId is the child.
3. DELETE /asset-models/{childId} while the parent hierarchy still references it.
4. Cause 1: GET /asset-models/{childId} (step task#19) — observe 404 ResourceNotFoundException instead of 200 with the full projection and assetModelStatus {state: FAILED, error: {code: VALIDATION_ERROR, message: 'Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model.'}}.
5. Remove the hierarchy reference from the parent (UpdateAssetModel), then re-issue DELETE /asset-models/{childId} (step task#22) — observe 404 instead of 202 with {assetModelId, assetModelStatus: {state: 'DELETING'}}.
