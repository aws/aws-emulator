Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file|run#9|AssertionError|assert message == (
Issue-Commit: 435579c3f4f417d2ee68a6d86543c2b3ca676fd8

# devops-agent CreateAssetFile one-file refusal carries an emulator-authored ValidationException message instead of AWS's "Assets of type '<assetType>' support only one file." wording

> _Note: this issue was identified against the code base at commit `435579c3f4f417d2ee68a6d86543c2b3ca676fd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAssetFile

## Description
Cause 1 of 1: CreateAssetFile against an artifact asset that already holds its single file is refused with the right code (ValidationException, HTTP 400) but with a message the emulator authors itself, not the message AWS returns for that rejection class.

AWS: Smithy (services/service-com-amazonaws-devopsagent-devopsagent/smithy-model/com-amazonaws-devopsagent-devopsagent.json) models CreateAssetFile with input members agentSpaceId/assetId/path/content and the error ValidationException (errorKind client, HTTP 400, "The input fails to satisfy the constraints specified by the service."); the model says nothing about the per-assetType file-count rule, so the message itself is recorded behavior only. The recording shows AWS refusing the second file of a single-file asset type with ValidationException and the message "Assets of type 'artifact' support only one file. Use UpdateAssetFile to replace the existing file." In that message everything is FIXED text except the quoted asset type, which is a value from state/request (the asset's assetType, here 'artifact'); it names no assetId and no existing path.

Emulator: answers ValidationException with "Asset <assetId> contains a single file at <path>, so another file cannot be created in it" — a different sentence that additionally leaks the asset id and the existing file path, and does not mention the asset type or point the caller at UpdateAssetFile. Replayed message observed: 'Asset asset-0000000000000002 contains a single file at a.txt, so another file cannot be created in it' (the id and path there are per-run values filling that emulator template, not part of the requirement).

Why: a client that matches on, logs, or surfaces the refusal text — and the many clients that key remediation off AWS's explicit "Use UpdateAssetFile to replace the existing file." instruction — sees text that never occurs against AWS. Message-based assertions and user-facing error surfaces diverge even though the status and error code agree.

Fix: the message belongs in the CreateAssetFile refusal arm and its shared message builder — services/service-com-amazonaws-devopsagent-devopsagent/src/ops/create_asset_file.rs (the asset.content_is_one_file arm calling crate::state::asset_holds_one_file_message) and crate::state::asset_holds_one_file_message in src/state.rs. It must emit AWS's sentence, parameterised on the asset's assetType rather than on the asset id and existing path. The surrounding behavior is already correct and must be preserved: code ValidationException, HTTP 400, the existing file left untouched (ListAssetFiles still ['a.txt'], GetAssetFile still returns the original content) — only the message diverges.

Traces: devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file.

## Evidence
Check devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file (mismatch).

Request sequence: CreateAgentSpace -> CreateAsset(assetType="artifact", content={file:{path:"a.txt", body:{text:"original"}}}) -> CreateAssetFile(agentSpaceId=<space>, assetId=<asset>, path="b.txt", content={text:"second"}).

Expected (AWS): Error.Code == "ValidationException", HTTP 400, Error.Message == "Assets of type 'artifact' support only one file. Use UpdateAssetFile to replace the existing file." (fixed text; the quoted asset type is the request/state value).

Actual (emulator): Error.Code == "ValidationException", HTTP 400, Error.Message == "Asset asset-0000000000000002 contains a single file at a.txt, so another file cannot be created in it".

Assertion raised:
  File "check_devops_agent_create_asset_file_rejected_when_artifact_already_has_its_one_file.py", line 57, in run
    assert message == (...)
  AssertionError: unexpected message 'Asset asset-0000000000000002 contains a single file at a.txt, so another file cannot be created in it'

The post-refusal assertions in the same check (ListAssetFiles == ['a.txt'] and GetAssetFile content == 'original') were never reached, so only the message is known to diverge.

## Steps to Reproduce
1. Boot the emulator and point a botocore devops-agent client at it.
2. space_id = create_agent_space(name=<unique>)['agentSpace']['agentSpaceId'].
3. asset_id = create_asset(agentSpaceId=space_id, assetType='artifact', metadata={'title':'probe artifact','description':'holds exactly one file'}, content={'file':{'path':'a.txt','body':{'text':'original'}}})['asset']['assetId'].
4. Call create_asset_file(agentSpaceId=space_id, assetId=asset_id, path='b.txt', content={'text':'second'}).
5. Inspect the ClientError: Code is ValidationException (correct) but Message is 'Asset <assetId> contains a single file at a.txt, so another file cannot be created in it' instead of "Assets of type 'artifact' support only one file. Use UpdateAssetFile to replace the existing file."
6. Replay: behavioral check devops-agent.create-asset-file-rejected-when-artifact-already-has-its-one-file.
