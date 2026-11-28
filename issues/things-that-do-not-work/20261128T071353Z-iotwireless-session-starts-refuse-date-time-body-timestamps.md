Issue-Source: trace-sweep-divergence
Issue-Trace: iotwireless-multicast-group-session-schedule-window
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotwireless-multicast-group-session-schedule-window iot-wireless-multicast-session-start-on-deleted-group iot-wireless-fuota-task-start-past-start-time
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|StartMulticastGroupSession|2xx->4xx|$
Issue-Key: iotwireless|StartMulticastGroupSession|2xx->4xx|status
Issue-Key: iotwireless|StartMulticastGroupSession|4xx->4xx|$.Message
Issue-Key: iotwireless|StartMulticastGroupSession|4xx->4xx|$.ResourceId
Issue-Key: iotwireless|StartMulticastGroupSession|4xx->4xx|$.ResourceType
Issue-Key: iotwireless|StartMulticastGroupSession|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iotwireless|StartMulticastGroupSession|4xx->4xx|status
Issue-Key: iotwireless|StartFuotaTask|4xx->4xx|$.Message
Issue-Key: iotwireless|GetMulticastGroup|2xx->2xx|$.CreatedAt
Issue-Key: iotwireless|GetMulticastGroup|2xx->2xx|$.LoRaWAN.DefaultSessionParameters.DlDr
Issue-Key: iotwireless|GetMulticastGroup|2xx->2xx|$.Status
Issue-Commit: 96e759e54960d5d1fd2a95fa47259d85e3163a48

# iotwireless session starts refuse date-time body timestamps and mis-serialize ValidationException

> _Note: this issue was identified against the code base at commit `96e759e54960d5d1fd2a95fa47259d85e3163a48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMulticastGroupSession

## Description
The multicast-session and FUOTA start paths of iotwireless never execute: the restJson1 input deserializer decodes a @timestampFormat date-time body member as an epoch integer and refuses the request at the boundary, and the refusal it emits is the synthetic smithy.framework ValidationException, whose JSON member is lowercase `message` rather than the modeled `Message`. Because the request dies before the handler, every state transition and every modeled refusal behind these two operations is unreachable, and the group read that follows reports stale state. 5 causes; all five must be closed.

An open issue, 20261128T053754Z-iotwireless-refuses-timestampformat-date-time-body-timestamps-startmulticastgroupsession.md, describes the same deserialization fault from the BEHAVIORAL corpus, but it carries no wire replay key, so the wire fixture steps below are not covered by it and are filed here.

Cause 1 of 5: StartMulticastGroupSession refuses a well-formed date-time SessionStartTime.
AWS: Smithy — StartMulticastGroupSession is PUT /multicast-groups/{Id}/session with success status 204 and an empty body; input LoRaWAN is required and targets LoRaWANMulticastSession, whose member SessionStartTime targets SessionStartTimeTimestamp and carries @timestampFormat "date-time" (so it is an RFC-3339/ISO-8601 string in the JSON body, which is what every AWS SDK serializes for that member), with DlDr @range 0..15, DlFreq @range 100000000..1000000000 and SessionTimeout @range 60..172800. For a start time inside the accepted window AWS answers 204 with an empty body.
Emulator: HTTP 400 with body {"__type": "ValidationException", "message": "invalid timestamp: failed to parse int"} for exactly that request. The fixed text "invalid timestamp: failed to parse int" is emulator-authored; AWS emits no such message for this error class.
Why: no client can schedule a multicast session at all — the SDK-mandated wire form of the member is rejected as bad input, so the caller is told to fix a request that is already correct, and the group never leaves its create-time state.
Fix: the request-deserialization layer for this service's restJson1 bodies (the timestamp decoder behind StartMulticastGroupSession's INPUT_SCHEMA, services/service-com-amazonaws-iotwireless-iotwireless/src/ops/types.rs / START_MULTICAST_GROUP_SESSION_INPUT_SCHEMA) must honour @timestampFormat date-time for body structure members instead of defaulting to epoch-int; the operation's handler (src/ops/start_multicast_group_session.rs, still a SCAFFOLDED 501 stub, so the 400 currently masks a 501) must then carry the request through to real behaviour.
Traces: iotwireless-multicast-group-session-schedule-window#task#8.

Cause 2 of 5: the boundary refusal carries no `Message` member, because the error is serialized from a synthetic shape with a lowercase `message`.
AWS: Smithy — com.amazonaws.iotwireless#ValidationException (httpError 400) has exactly one member, `Message` (target Message, length 0..2048); with no @jsonName the restJson1 wire key is `Message`, capital M. The recorded refusal of a StartMulticastGroupSession whose LoRaWAN omits DlDr is 400 ValidationException with Message "1 validation error detected: Value 'DlDr' at 'null' failed to satisfy constraint: Member must have non-null and nonempty value" — fixed text "1 validation error detected: Value '<member>' at '<path>' failed to satisfy constraint: Member must have non-null and nonempty value", with the member name taken from the request's missing member.
Emulator: HTTP 400 with NO `Message` member at all. The body is the synthetic smithy.framework#ValidationException declared in src/scaffolded.rs (VALIDATION_EXCEPTION_MESSAGE / VALIDATION_EXCEPTION_SCHEMA, lines 117-144), whose member is named `message`, so the JSON key is lowercase and the content is the Cause 1 parse error rather than a statement about the absent member.
Why: a client that reads err.Message (as every SDK's modeled ValidationException does) gets null for every refusal this path produces, so the reason the request was rejected is invisible, and error-message assertions and retry/branch logic keyed on it break.
Fix: refusals must be serialized from the service's MODELED ValidationException (member `Message`) rather than the synthetic framework shape — in the boundary validator / Scaffolded::validation rendering in src/scaffolded.rs — and the missing-required-member refusal itself belongs in StartMulticastGroupSession's validation.
Traces: iotwireless-multicast-group-session-schedule-window#task#3 (and its cluster: task#4, task#5, task#6).

Cause 3 of 5: starting a session on a deleted multicast group answers 400 ValidationException instead of 404 ResourceNotFoundException.
AWS: Smithy — StartMulticastGroupSession models ResourceNotFoundException (httpError 404) with members Message, ResourceId ("Id of the not found resource") and ResourceType. For a group id that no longer exists AWS answers HTTP 404, x-amzn-errortype ResourceNotFoundException, ResourceType "MulticastGroup", ResourceId the group id from the request URI, and Message with the fixed text "Cannot complete the step because resource is not found, with resource type <ResourceType>, resource Id <ResourceId>" — the resource type and id are the only variable parts, both taken from the request.
Emulator: HTTP 400, x-amzn-errortype ValidationException, no Message, no ResourceId, no ResourceType: the Cause 1 parse refusal fires before the handler ever looks the group up, so the not-found path is unreachable.
Why: a client cannot distinguish a deleted/unknown group from a malformed request. 404-vs-400 drives SDK error-type matching, idempotent-cleanup logic and retry classification, so deletion-tolerant callers break.
Fix: after Cause 1 is fixed, the existence check belongs in the StartMulticastGroupSession handler (src/ops/start_multicast_group_session.rs), returning the modeled ResourceNotFoundException (the service already has a constructor for it: crate::state::multicast_group_not_found, used by src/ops/get_multicast_group.rs:157-161).
Traces: iot-wireless-multicast-session-start-on-deleted-group#task#4.

Cause 4 of 5: StartFuotaTask does not refuse a start time in the past with AWS's message.
AWS: Smithy — StartFuotaTask is PUT /fuota-tasks/{Id} with success 204; input LoRaWAN targets LoRaWANStartFuotaTask, whose StartTime member carries @timestampFormat "date-time". For a StartTime that is already in the past AWS answers 400 ValidationException with Message "startTime should be in future" — that whole sentence is fixed text with no variable parts.
Emulator: HTTP 400 with no Message member (the Cause 1/Cause 2 parse refusal under a lowercase key), so the modeled semantic refusal is never produced.
Why: a caller scheduling a FUOTA task cannot tell a bad schedule from a bad request, and a client matching on the refusal text sees nothing at all.
Fix: honour date-time for LoRaWANStartFuotaTask.StartTime in the input schema, then implement the past-start-time check in StartFuotaTask's handler (src/ops/start_fuota_task.rs, currently a SCAFFOLDED 501 stub) returning the modeled ValidationException.
Traces: iot-wireless-fuota-task-start-past-start-time#task#6.

Cause 5 of 5: GetMulticastGroup reports create-time state after a session has been scheduled.
AWS: Smithy — GetMulticastGroup (GET /multicast-groups/{Id}, 200) returns Status (MulticastGroupStatus, a free string) and LoRaWAN of shape LoRaWANMulticastGet with DefaultSessionParameters (DefaultSessionParametersMulticast{DlDr @range 0..15, DlFreq}). Recorded behavior: once a session has been accepted for a start time inside the schedule window, the group's Status is "Session_Waiting" and LoRaWAN.DefaultSessionParameters.DlDr is the DlDr the accepted StartMulticastGroupSession request carried.
Emulator: Status "Pending" and the create-time DlDr, i.e. the session was never applied to the group record — the direct consequence of Cause 1 plus the stub handler.
Why: a client polling the group to learn whether its session is scheduled never observes the transition, so any wait-for-Session_Waiting loop hangs and the parameters it reads back are not the ones it submitted.
Fix: StartMulticastGroupSession's handler must write the session parameters and the resulting lifecycle status into the multicast-group record in src/state.rs; GetMulticastGroup already reports both from that record (src/ops/get_multicast_group.rs:175-193). This step's CreatedAt key diverges for the separate serialization reason filed in the iotwireless validation-message/read-response bundle.
Traces: iotwireless-multicast-group-session-schedule-window#task#10.

## Evidence
Cause 1 — StartMulticastGroupSession (PUT /multicast-groups/{Id}/session), LoRaWAN={SessionStartTime: <ISO-8601 date-time inside the window>, SessionTimeout, DlDr, DlFreq}
  expected (AWS): 204, empty body {}
  actual (emulator): 400 {"__type": "ValidationException", "message": "invalid timestamp: failed to parse int"}
  fixture iotwireless-multicast-group-session-schedule-window step task#8

Cause 2 — same operation, LoRaWAN without DlDr
  expected (AWS): 400, $.Message = "1 validation error detected: Value 'DlDr' at 'null' failed to satisfy constraint: Member must have non-null and nonempty value"
  actual (emulator): 400, $.Message missing (body uses lowercase "message")
  fixture iotwireless-multicast-group-session-schedule-window steps task#3, task#4, task#5, task#6

Cause 3 — same operation against a deleted group id
  expected (AWS): 404, $.ResourceType = "MulticastGroup", $.ResourceId = <group id from the request>, $.Message = "Cannot complete the step because resource is not found, with resource type MulticastGroup, resource Id <group id>", x-amzn-errortype ResourceNotFoundException
  actual (emulator): 400, x-amzn-errortype ValidationException, $.Message / $.ResourceId / $.ResourceType all missing
  fixture iot-wireless-multicast-session-start-on-deleted-group step task#4

Cause 4 — StartFuotaTask (PUT /fuota-tasks/{Id}), LoRaWAN={StartTime: <date-time in the past>}
  expected (AWS): 400, $.Message = "startTime should be in future"
  actual (emulator): 400, $.Message missing
  fixture iot-wireless-fuota-task-start-past-start-time step task#6

Cause 5 — GetMulticastGroup after an accepted session start
  expected (AWS): $.Status = "Session_Waiting", $.LoRaWAN.DefaultSessionParameters.DlDr = the DlDr submitted with the session (8 in this fixture)
  actual (emulator): $.Status = "Pending", $.LoRaWAN.DefaultSessionParameters.DlDr = 9 (the create-time value)
  fixture iotwireless-multicast-group-session-schedule-window step task#10

Model citations: LoRaWANMulticastSession$SessionStartTime -> SessionStartTimeTimestamp @timestampFormat date-time; LoRaWANStartFuotaTask$StartTime @timestampFormat date-time; com.amazonaws.iotwireless#ValidationException$Message; ResourceNotFoundException{Message, ResourceId, ResourceType} httpError 404; StartMulticastGroupSession/StartFuotaTask success status 204.

## Steps to Reproduce
1. CreateMulticastGroup(Name=..., LoRaWAN={RfRegion: US915, DlClass: ClassC, DefaultSessionParameters: {DlDr: 9, DlFreq: ...}}) -> note Id.
2. StartMulticastGroupSession(Id=<id>, LoRaWAN={SessionStartTime: now+10min, SessionTimeout: 60, DlDr: 8, DlFreq: 923300000}) with any AWS SDK -> emulator answers 400 ValidationException "invalid timestamp: failed to parse int" instead of 204 (Cause 1).
3. Repeat step 2 with DlDr omitted -> emulator answers 400 whose body has no "Message" member instead of the "1 validation error detected: Value 'DlDr' at 'null' ..." message (Cause 2).
4. DeleteMulticastGroup(Id=<id>), then StartMulticastGroupSession(Id=<id>, ...) -> emulator answers 400 ValidationException instead of 404 ResourceNotFoundException with ResourceType MulticastGroup and ResourceId <id> (Cause 3).
5. CreateFuotaTask(...), then StartFuotaTask(Id=<taskId>, LoRaWAN={StartTime: now-10min}) -> emulator answers 400 with no Message instead of 400 ValidationException "startTime should be in future" (Cause 4).
6. Recreate a group, StartMulticastGroupSession with a start time inside the window, then GetMulticastGroup(Id=<id>) -> Status stays "Pending" and DefaultSessionParameters.DlDr stays at the create-time value instead of "Session_Waiting" and the submitted DlDr (Cause 5).
