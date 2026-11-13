Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dataexchange.delete-asset-after-finalize-rejected
Issue-Shape-Id: com.amazonaws.dataexchange#DataExchange
Issue-Key: dataexchange.delete-asset-after-finalize-rejected|run#7|AssertionError|assert code == "ConflictException", f"expected ConflictException deleting a finalized-revision asset, got {code}"
Issue-Commit: d731633d9e655898fab102cd9a4c0063529f6263

# DataExchange DeleteAsset is an unimplemented scaffold stub: answers 501 NotImplemented instead of 204 / ConflictException

> _Note: this issue was identified against the code base at commit `d731633d9e655898fab102cd9a4c0063529f6263`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAsset

## Description
One root cause.

Cause 1 of 1: `DeleteAsset` is still a scaffold stub and answers HTTP 501 `NotImplemented` for every call, so neither the success path nor the finalized-revision refusal exists. Failing check: dataexchange.delete-asset-after-finalize-rejected.

AWS: Smithy (services/service-com-amazonaws-dataexchange-dataexchange/smithy-model/com-amazonaws-dataexchange-dataexchange.json) models `DeleteAsset` as DELETE /v1/data-sets/{DataSetId}/revisions/{RevisionId}/assets/{AssetId} with required httpLabel members AssetId, DataSetId, RevisionId (each `Id`, pattern ^[a-zA-Z0-9]{30,40}$), an empty output and success status 204. Its modeled errors are AccessDeniedException (403), ConflictException (409), InternalServerException (500), ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400); ConflictException is documented as "The request couldn't be completed because it conflicted with the current state of the resource" and carries a required `Message` plus optional `ResourceId`/`ResourceType`. Recorded behavior: deleting an asset that belongs to a revision which has been finalized (UpdateRevision with Finalized=true) is refused with HTTP 409, error code `ConflictException` and the message `Cannot take action on a Finalized Revision.` — that whole sentence is fixed text for this error class, with no variable part; the asset remains listed by ListRevisionAssets. Deleting an asset in a revision that is NOT finalized succeeds with 204 and an empty body, after which GetAsset on that asset id is refused with HTTP 404, code `ResourceNotFoundException` and the fixed message `The requested resource was not found.`

Emulator: every DeleteAsset call returns HTTP 501 with error code `NotImplemented` and the message `operation DeleteAsset is scaffolded but not implemented in this emulator`. The replayed check caught this on the finalized-revision delete and reported `expected ConflictException deleting a finalized-revision asset, got NotImplemented`; because the stub refuses unconditionally, the 204 success path and the subsequent ResourceNotFoundException read-back are unreachable too.

Why: a client cannot delete assets at all, and more dangerously it cannot distinguish "this asset is protected because its revision is finalized" (a modeled, retry-never 409 that client code branches on) from "the service is broken" (501, which SDK retry policies treat as a server-side condition). Teardown and revision-editing flows that expect 204 then a 404 read-back fail, and immutability of finalized revisions is never enforced or reported.

Fix: implement the handler for this operation (its `handle` body in services/service-com-amazonaws-dataexchange-dataexchange/src/ops/delete_asset.rs, which is the only file for the operation) against the service state: resolve DataSetId/RevisionId/AssetId, return ResourceNotFoundException when any of them does not resolve, return ConflictException with the fixed finalized-revision message when the owning revision is finalized, otherwise remove the asset from state and answer 204 with an empty body so a following GetAsset is refused with ResourceNotFoundException. Remove the SCAFFOLDED marker line so the stub inventory stays accurate. Note this is NOT covered by the already-filed stub issues for ListDataGrants, GetAsset, DeleteRevision or UpdateAsset, nor by 20261029T172356Z-dataexchange-client-error-messages-are-emulator-prose-instead.md (that one is about wording of errors the emulator does produce); DeleteAsset produces no modeled error at all.

Traces: dataexchange.delete-asset-after-finalize-rejected.

## Evidence
Check dataexchange.delete-asset-after-finalize-rejected [mismatch]:
  AssertionError: expected ConflictException deleting a finalized-revision asset, got NotImplemented
  (check_dataexchange_delete_asset_after_finalize_rejected.py line 37)

Expected (AWS, recorded): DeleteAsset on an asset of a finalized revision ->
  HTTP 409, Code=ConflictException, Message="Cannot take action on a Finalized Revision."
  and the asset is still returned by ListRevisionAssets.
  DeleteAsset on an asset of a non-finalized revision -> HTTP 204, empty body; a following
  GetAsset -> HTTP 404, Code=ResourceNotFoundException, Message="The requested resource was not found."

Actual (emulator): HTTP 501, Code=NotImplemented,
  Message="operation DeleteAsset is scaffolded but not implemented in this emulator".

Source: services/service-com-amazonaws-dataexchange-dataexchange/src/ops/delete_asset.rs lines 3-6 carry the
`//! SCAFFOLDED, NOT IMPLEMENTED` marker and line 186 returns
`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteAsset"))`.

Smithy: DeleteAsset http {DELETE, /v1/data-sets/{DataSetId}/revisions/{RevisionId}/assets/{AssetId}, code 204};
errors include ConflictException (409, required Message) and ResourceNotFoundException (404, required Message).

## Steps to Reproduce
1. CreateDataSet (AssetType=S3_SNAPSHOT), then CreateRevision on it.
2. Import an asset into that revision via a CreateJob/StartJob IMPORT_ASSETS_FROM_SIGNED_URL job and wait for COMPLETED.
3. UpdateRevision(DataSetId, RevisionId, Finalized=true).
4. Call DeleteAsset(AssetId, DataSetId, RevisionId).
   Expected: HTTP 409 ConflictException "Cannot take action on a Finalized Revision.", and the asset still appears in ListRevisionAssets.
   Actual: HTTP 501 NotImplemented "operation DeleteAsset is scaffolded but not implemented in this emulator".
5. Repeat steps 2 on a second, NON-finalized revision and call DeleteAsset for its asset.
   Expected: HTTP 204 empty body; GetAsset on that id then returns HTTP 404 ResourceNotFoundException "The requested resource was not found."
   Actual: HTTP 501 NotImplemented.
