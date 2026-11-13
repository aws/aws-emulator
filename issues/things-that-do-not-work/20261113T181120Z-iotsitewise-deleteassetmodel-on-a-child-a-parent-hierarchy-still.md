Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotsitewise.sitewise-delete-asset-model-settles-failed-and-the-model-survives-under-a-parent
Issue-Shape-Id: com.amazonaws.iotsitewise#AWSIoTSiteWise
Issue-Key: iotsitewise|DescribeAssetModel|ResourceNotFoundException
Issue-Commit: 3ec8579e755ba478acd05f82a020be272a76f6a1

# DeleteAssetModel on a child a parent hierarchy still references erases the model instead of accepting the delete and settling it FAILED (VALIDATION_ERROR)

> _Note: this issue was identified against the code base at commit `3ec8579e755ba478acd05f82a020be272a76f6a1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAssetModel

## Description
This bundle has 1 cause; it must be closed.

Cause 1 of 1: a DeleteAssetModel blocked by a parent hierarchy reference destroys the asset model, so DescribeAssetModel/ListAssetModels can no longer observe the asynchronous FAILED settle and the repair path is unreachable. Failing check: iotsitewise.sitewise-delete-asset-model-settles-failed-and-the-model-survives-under-a-parent.

  AWS: IoT SiteWise validates an asset-model delete on the ASYNCHRONOUS plane. Smithy (services/service-com-amazonaws-iotsitewise-awsiotsitewise/smithy-model/com-amazonaws-iotsitewise-awsiotsitewise.json): DeleteAssetModel DELETE /asset-models/{assetModelId}, http code 202, output {assetModelId, assetModelStatus @required}; errors InvalidRequestException 400, ConflictingOperationException 409, PreconditionFailedException 412, ResourceNotFoundException 404, ThrottlingException 429, InternalFailureException 500. DescribeAssetModel GET /asset-models/{assetModelId}, code 200, with assetModelId/assetModelArn/assetModelName/assetModelDescription/assetModelProperties/assetModelHierarchies/assetModelCreationDate/assetModelLastUpdateDate/assetModelStatus all @required; assetModelStatus -> AssetModelStatus{state @required, error ErrorDetails{code @required (ErrorCode: VALIDATION_ERROR|INTERNAL_FAILURE), message @required}}. ListAssetModels GET /asset-models, code 200, assetModelSummaries @required. Deleting a child model while a parent asset model's hierarchy still declares childAssetModelId = that child is therefore NOT refused synchronously: AWS answers 202 with assetModelStatus.state = DELETING, the model then settles state = FAILED carrying error.code = VALIDATION_ERROR and error.message = "Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model." — that sentence is entirely FIXED text with no variable parts. The model SURVIVES: it stays listed in ListAssetModels under its original name and stays describable with its full projection (its id, arn, name, properties, hierarchies, timestamps and version are the per-run variable parts read from state). Once the parent's hierarchy reference is removed via UpdateAssetModel, the same DeleteAssetModel is accepted again (202, DELETING) and converges, after which DescribeAssetModel answers 404 ResourceNotFoundException.
  Emulator: the delete is accepted, but the emulator REMOVES the asset model from state immediately. The check's subsequent poll of DescribeAssetModel for the terminal state raises 404 ResourceNotFoundException with message "Asset Model with ID <assetModelId> not found" (the id is the per-run value; the surrounding text is the emulator's fixed message), so every @required DescribeAssetModel output member is absent, state = FAILED is never observable, assetModelStatus.error (VALIDATION_ERROR + AWS's fixed sentence) is never served at all, and the model also disappears from ListAssetModels.
  Why: a client that tears an estate down in the wrong order relies on polling the model to learn that the delete failed and why — the diagnosis AWS places in assetModelStatus.error is the only channel, since the synchronous response (202 DELETING) is indistinguishable from a legal delete. Against the emulator the resource vanishes, so the client concludes a model AWS still holds was destroyed, the parent's hierarchy now points at an id the emulator no longer knows, and the documented recovery (drop the parent's hierarchy reference, re-issue the delete) cannot be exercised because the resource is already 404.
  Fix: the DeleteAssetModel handler and the asset-model state it writes. A delete blocked by a parent hierarchy reference must leave the model in state and settle it asynchronously at state = FAILED with ErrorDetails{code = VALIDATION_ERROR, message = the fixed sentence above}; DescribeAssetModel and ListAssetModels must keep serving it unchanged, and a re-issued delete after the reference is removed must take the normal accepted path (202, DELETING) and then converge to removal.
  Traces: iotsitewise.sitewise-delete-asset-model-settles-failed-and-the-model-survives-under-a-parent.

Related open issues (none of which carries this behavioral check's replay key, so none covers it): 20261113T164603Z-iotsitewise-deleteassetmodel-of-a-hierarchy-referenced-child-erases-the.md describes the same erase-instead-of-FAILED behaviour from the wire corpus (fixture iotsitewise-windfarm-trajwire-teardown-order-mistakes, steps task#19/task#22) — fixing that should fix this check, and this entry adds the behavioral replay key for it; 20261109T115126Z-iotsitewise-deleteassetmodel-accepts-a-delete-on-a-model-still.md covers the CREATING-state gate instead; 20261102T084333Z-iotsitewise-illegal-asset-model-definitions-settle-active-instead-of.md and 20261103T025010Z-iotsitewise-invalid-asset-model-definitions-settle-active-instead-of.md cover the missing asynchronous FAILED settle for illegal definitions generally.

## Evidence
Behavioral replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 71/91 checks passed; check iotsitewise.sitewise-delete-asset-model-settles-failed-and-the-model-survives-under-a-parent ERRORED.

Sequence exercised by the check: CreateAssetModel(child, one DOUBLE measurement) -> settles ACTIVE; CreateAssetModel(parent, one DOUBLE measurement + assetModelHierarchies=[{name: 'Turbines', childAssetModelId: <child>}]) -> settles ACTIVE; DeleteAssetModel(assetModelId=<child>) while the parent hierarchy still references it.

Expected (AWS): DeleteAssetModel -> 202 assetModelStatus.state = 'DELETING'; then DescribeAssetModel(<child>) -> 200 with assetModelStatus = {'state': 'FAILED', 'error': {'code': 'VALIDATION_ERROR', 'message': 'Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model.'}}, the child still present in ListAssetModels under its original name, and DescribeAssetModel still reporting that name.

Actual (emulator): the poll on DescribeAssetModel fails with
  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeAssetModel operation: Asset Model with ID 821cd49b-1894-460f-b0eb-bd40c6059142 not found
(HTTP 404). The id 821cd49b-1894-460f-b0eb-bd40c6059142 is this run's generated value filling the message template, not a requirement; the requirements are the 200 status, the FAILED/VALIDATION_ERROR status with AWS's fixed sentence, and the model's continued presence in DescribeAssetModel and ListAssetModels. The check's positive control (remove the parent hierarchy via UpdateAssetModel, re-issue DeleteAssetModel, expect convergence to 404) is never reached.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-behavioral-aws-v1 check iotsitewise.sitewise-delete-asset-model-settles-failed-and-the-model-survives-under-a-parent.
2. CreateAssetModel a child model with one measurement property; poll DescribeAssetModel until assetModelStatus.state = ACTIVE.
3. CreateAssetModel a parent model with assetModelHierarchies = [{name: 'Turbines', childAssetModelId: <childId>}]; poll until ACTIVE.
4. DeleteAssetModel(assetModelId=<childId>) while the parent hierarchy still references it — the response reports DELETING (as AWS does).
5. Poll DescribeAssetModel(assetModelId=<childId>): observe 404 ResourceNotFoundException ('Asset Model with ID <childId> not found') instead of 200 with assetModelStatus {state: 'FAILED', error: {code: 'VALIDATION_ERROR', message: 'Attempted to delete an asset model that is referenced by a hierarchy in a parent asset model. You must remove hierarchy references before you can delete the asset model.'}}.
6. ListAssetModels (paginated): observe the child's name is absent, where AWS still lists it.
7. Control AWS converges on but the emulator cannot reach: UpdateAssetModel the parent with assetModelHierarchies = [], then DeleteAssetModel(<childId>) again — AWS answers 202 DELETING and the model then disappears.
