Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-voice.voice-connector-in-group-delete-rejected
Issue-Shape-Id: com.amazonaws.chimesdkvoice#ChimeSDKTelephonyService
Issue-Key: chime-sdk-voice.voice-connector-in-group-delete-rejected|_refusal#13|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Commit: 736b28b40b20576fe554bad4abfa7fe4ba2cddb1

# DeleteVoiceConnector deletes a connector that a Voice Connector group still lists, answering 204 instead of refusing with BadRequestException

> _Note: this issue was identified against the code base at commit `736b28b40b20576fe554bad4abfa7fe4ba2cddb1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVoiceConnector

## Description
Cause 1 of 1: DeleteVoiceConnector performs no referential check against the Voice Connector groups that list the connector, so a grouped connector is destroyed on request. Failing check: chime-sdk-voice.voice-connector-in-group-delete-rejected.

AWS: a Voice Connector that is still a member of a Voice Connector group cannot be deleted. AWS refuses the call with HTTP 400 and error code BadRequestException; the recorded message is "Could not delete VoiceConnector '<VoiceConnectorId>' with associated VoiceConnectorGroups" — the fixed text is "Could not delete VoiceConnector '" / "' with associated VoiceConnectorGroups", and the only variable part is the VoiceConnectorId taken from the request path. The connector is left untouched by the refusal (a following GetVoiceConnector still returns it with its original Name). Once the owning group is updated to an empty VoiceConnectorItems list, the same delete succeeds with 204 and GetVoiceConnector then answers NotFoundException with "Could not find VoiceConnector with Id '<VoiceConnectorId>'". Smithy: DeleteVoiceConnector models BadRequestException (HTTP 400) — "The input parameters don't match the service's restrictions" — alongside ConflictException/NotFoundException, and its success code is 204; the group-membership precondition itself is recorded behavior only (the model's doc text names only the phone-number precondition).

Emulator: the handler resolves the connector, removes it from the store unconditionally and returns 204 with an empty body. The replayed check saw `{'ResponseMetadata': {'HTTPStatusCode': 204, ...}}` where a BadRequestException was required, i.e. the grouped connector was actually deleted while the group still listed it.

Why: a client relying on AWS's behavior treats the 400 as the signal that the connector is still wired into a group and that it must empty or delete the group first. Against the emulator the delete silently succeeds, leaving a Voice Connector group that references a connector id that no longer exists — dangling group membership that AWS makes impossible. Teardown and migration code that depends on the refusal to order its steps (empty the group, then delete the connector) is never exercised, and code that treats a successful delete as proof the resource had no dependents draws the wrong conclusion.

Fix: the DeleteVoiceConnector handler's precondition check — before removing the record from state, scan the service state's voice-connector groups for any group whose VoiceConnectorItems contain the requested VoiceConnectorId and, if one exists, return the modeled BadRequestException (400) with the message above, leaving the connector and the group unmodified. The symmetric group-side rule (a non-empty group refusing its own delete) is exercised by a separate check that is currently blocked by the unimplemented DeleteVoiceConnectorGroup stub and is not part of this issue.

Traces: services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/ops/delete_voice_connector.rs lines 192-202 remove the connector with no dependency check; the modeled BadRequestException variant is already available at line 112.

## Evidence
Check chime-sdk-voice.voice-connector-in-group-delete-rejected:
  create_voice_connector -> VoiceConnectorId V
  create_voice_connector_group(VoiceConnectorItems=[{VoiceConnectorId: V, Priority: 1}]) -> group G, and GetVoiceConnectorGroup confirms G lists exactly [V]
  delete_voice_connector(VoiceConnectorId=V)

Expected (AWS): ClientError, code 'BadRequestException', HTTP 400, message "Could not delete VoiceConnector '<V>' with associated VoiceConnectorGroups"; the connector survives unchanged.

Actual (emulator): the call SUCCEEDED — {'ResponseMetadata': {'HTTPStatusCode': 204, 'HTTPHeaders': {'content-type': 'application/json', ...}, 'RetryAttempts': 0}} — and the connector was removed from state.

Harness assertion: AssertionError: expected a refusal, but the call succeeded: {'ResponseMetadata': {'HTTPStatusCode': 204, ...}} (check_chime_sdk_voice_voice_connector_in_group_delete_rejected.py line 117 via _refusal at line 22).

## Steps to Reproduce
1. voice = client('chime-sdk-voice')
2. vc_id = voice.create_voice_connector(Name=<unique>, AwsRegion=<region>, RequireEncryption=True)['VoiceConnector']['VoiceConnectorId']; poll get_voice_connector(VoiceConnectorId=vc_id) until it stops answering NotFoundException.
3. group_id = voice.create_voice_connector_group(Name=<unique>, VoiceConnectorItems=[{'VoiceConnectorId': vc_id, 'Priority': 1}])['VoiceConnectorGroup']['VoiceConnectorGroupId']; confirm get_voice_connector_group(VoiceConnectorGroupId=group_id) lists exactly [vc_id].
4. Call voice.delete_voice_connector(VoiceConnectorId=vc_id).
   Observed: 204 success, connector deleted while the group still lists it.
   Expected: ClientError BadRequestException / HTTP 400, message "Could not delete VoiceConnector '<vc_id>' with associated VoiceConnectorGroups", connector still readable with its original Name.
5. Then update_voice_connector_group(VoiceConnectorGroupId=group_id, Name=<same name>, VoiceConnectorItems=[]) and retry the delete: it must now return 204 and get_voice_connector must answer NotFoundException with "Could not find VoiceConnector with Id '<vc_id>'".
