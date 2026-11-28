Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.send-data-to-unknown-multicast-group-rejected
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless.send-data-to-unknown-multicast-group-rejected|run#8|AssertionError|assert (neighbour["code"], neighbour.get("Message")) == (
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# iotwireless SendDataToMulticastGroup is an unimplemented scaffold stub: every multicast downlink answers 501 NotImplemented instead of a MessageId or the modeled refusal

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendDataToMulticastGroup

## Description
1 cause.

Cause 1 of 1: SendDataToMulticastGroup never serves the operation.
AWS: Smithy — SendDataToMulticastGroup, POST /multicast-groups/{Id}/data, success status 201, input Id (httpLabel, required), PayloadData (required, base64 @pattern, len 0..2048) and WirelessMetadata (required, MulticastWirelessMetadata{LoRaWAN}); output MessageId (MulticastGroupMessageId). Modeled errors include ValidationException (400), ResourceNotFoundException (404, members Message/ResourceId/ResourceType), ConflictException (409), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500). Recorded behavior: a downlink to a LIVE group that has no running session is refused 400 ValidationException whose fixed text is "operation is not allowed in current sate. ResourceType: MulticastGroup, ResourceId: <groupId>, state: <status>" (note AWS's own spelling "sate"; ResourceId is the requested group id and state is the group's current Status, e.g. Pending); a downlink to a group id that does not exist, or to one that was deleted, is refused 404 ResourceNotFoundException with fixed text "Cannot complete the step because resource is not found, with resource type MulticastGroup, resource Id <groupId>" and the error body also carrying ResourceId=<groupId> and ResourceType="MulticastGroup"; a downlink to a group whose session is In_Session returns 201 with a non-empty MessageId.
Emulator: the handler is the generated scaffold — services/service-com-amazonaws-iotwireless-iotwireless/src/ops/send_data_to_multicast_group.rs returns Scaffolded::not_implemented("SendDataToMulticastGroup"), so every call answers HTTP 501 with code "NotImplemented" and message "operation SendDataToMulticastGroup is scaffolded but not implemented in this emulator" — no MessageId, no state refusal, no not-found refusal, and none of the error body's ResourceId/ResourceType members. Note: the replay report marks the sibling SendDataToWirelessDevice stub as already filed; that issue is about the per-device downlink operation and does NOT cover this multicast operation, which is a separate handler file and was not flagged.
Why: the multicast data plane does not exist for any client. A caller cannot send a multicast downlink at all, and a caller that probes a group's readiness cannot distinguish "group does not exist" (404 ResourceNotFoundException) from "group exists but has no running session" (400 ValidationException) — both collapse to 501, which SDK retry/error handling treats as a server fault rather than a client error.
Fix: implement the operation in its handler file, resolving Id against the multicast-group store (404 ResourceNotFoundException with ResourceId/ResourceType populated when absent or deleted), gating on the group's session state (400 ValidationException with the current-state sentence when the group is not In_Session), and returning 201 with a minted MessageId otherwise.
Traces: iotwireless.send-data-to-unknown-multicast-group-rejected — the live-group probe answered {'code': 'NotImplemented'} where the check expects ('ValidationException', 'operation is not allowed in current sate. ResourceType: MulticastGroup, ResourceId: <groupId>, state: Pending'). Handler file: services/service-com-amazonaws-iotwireless-iotwireless/src/ops/send_data_to_multicast_group.rs lines 151-169 (scaffold marker at lines 4-12).

## Evidence
SendDataToMulticastGroup(Id=<existing group, Status Pending>, PayloadData="3q2+7w==", WirelessMetadata={LoRaWAN:{FPort:42}})
  expected (AWS): 400 ValidationException, Message "operation is not allowed in current sate. ResourceType: MulticastGroup, ResourceId: <groupId>, state: Pending"
  actual (emulator): 501, Error.Code "NotImplemented", Message "operation SendDataToMulticastGroup is scaffolded but not implemented in this emulator"

SendDataToMulticastGroup(Id=<unknown or deleted uuid>, ...)
  expected (AWS): 404 ResourceNotFoundException, Message "Cannot complete the step because resource is not found, with resource type MulticastGroup, resource Id <groupId>", plus ResourceId=<groupId>, ResourceType="MulticastGroup"
  actual (emulator): 501 NotImplemented

SendDataToMulticastGroup(Id=<group In_Session>, ...)
  expected (AWS): 201 with a non-empty MessageId
  actual (emulator): 501 NotImplemented

## Steps to Reproduce
1. CreateMulticastGroup(Name=..., LoRaWAN={RfRegion: US915, DlClass: ClassC}) -> Id; GetMulticastGroup(Id) reports Status "Pending".
2. SendDataToMulticastGroup(Id=<id>, PayloadData="3q2+7w==", WirelessMetadata={"LoRaWAN": {"FPort": 42}}) -> emulator answers 501 NotImplemented instead of 400 ValidationException naming the group's current state.
3. SendDataToMulticastGroup(Id="00000000-0000-4000-8000-0000000000bb", same payload) -> 501 NotImplemented instead of 404 ResourceNotFoundException carrying Message/ResourceId/ResourceType.
4. DeleteMulticastGroup(Id=<id>), wait until GetMulticastGroup 404s, then repeat the downlink -> still 501 instead of the same not-found refusal.
