Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iotwireless.multicast-session-start-outside-window-rejected iotwireless.multicast-session-cancel-ends-running-session iotwireless.multicast-session-cancel-rejects-non-uuid-group-id iotwireless.fuota-task-restart-after-start-time-set-rejected iotwireless.fuota-start-rejected-until-role-can-read-image
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless.multicast-session-start-outside-window-rejected|run#6|AssertionError|assert got == ("ValidationException", WINDOW_MESSAGE), (
Issue-Key: iot-wireless|StartMulticastGroupSession|ValidationException
Issue-Key: iot-wireless|StartFuotaTask|ValidationException
Issue-Key: iotwireless.fuota-start-rejected-until-role-can-read-image|run#21|AssertionError|assert got == ("ValidationException", refused), (
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# iotwireless refuses @timestampFormat date-time body timestamps: StartMulticastGroupSession and StartFuotaTask answer 400 ValidationException "invalid timestamp: failed to parse int"

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMulticastGroupSession

## Description
The emulator cannot deserialize an ISO-8601 timestamp sent in a restJson1 JSON body for iotwireless, so every request that carries a session/task start time is refused at the deserialization step, before the operation's handler is ever reached. 2 causes, one per affected operation; both must be closed.

Cause 1 of 2: StartMulticastGroupSession rejects LoRaWAN.SessionStartTime.
AWS: Smithy — StartMulticastGroupSession (PUT /multicast-groups/{Id}/session, success 204) takes required LoRaWAN of shape LoRaWANMulticastSession, whose member SessionStartTime targets SessionStartTimeTimestamp and carries @timestampFormat "date-time". A date-time timestamp is on the wire as an RFC-3339/ISO-8601 string (e.g. "2026-11-28T01:23:45Z"), which is exactly what every AWS SDK serializes for that member, NOT an epoch number. AWS accepts such a body: it either schedules the session (204) or refuses it on a MODELED semantic ground — ValidationException "start time should be between 3 minutes and 48 hours from now" when the start lies outside the documented 3-minutes-to-48-hours window, ResourceNotFoundException for an unknown group id.
Emulator: the deserializer parses the member as an epoch integer and answers HTTP 400 ValidationException with message "invalid timestamp: failed to parse int" for a well-formed date-time string. Fixed text "invalid timestamp: failed to parse int" is emulator-authored; AWS never emits it, and for the out-of-window case AWS's message is the fixed sentence "start time should be between 3 minutes and 48 hours from now".
Why: no client can schedule a multicast session at all. Every SDK sends the date-time form the model mandates, so StartMulticastGroupSession is unreachable, group Status never moves to Session_Waiting/In_Session, GetMulticastGroupSession never reports a start time, and the window-validation refusal AWS gives is replaced by a parse error — a caller sees a client-side "bad input" for input that is correct.
Fix: the request-deserialization layer for this service's restJson1 JSON bodies (the generated input schema / shared timestamp decoder behind StartMulticastGroupSession's INPUT_SCHEMA) must honour @timestampFormat date-time for structure members in the body instead of defaulting to epoch-seconds; the semantic window and not-found refusals then belong in the operation's handler/validation.
Traces: iotwireless.multicast-session-start-outside-window-rejected (mismatch: a start one minute ahead answered ('ValidationException', 'invalid timestamp: failed to parse int') instead of ('ValidationException', 'start time should be between 3 minutes and 48 hours from now')), iotwireless.multicast-session-cancel-ends-running-session, iotwireless.multicast-session-cancel-rejects-non-uuid-group-id (both error out on the StartMulticastGroupSession call with ValidationException "invalid timestamp: failed to parse int"). Handler file: services/service-com-amazonaws-iotwireless-iotwireless/src/ops/start_multicast_group_session.rs (note: its handler is additionally still a scaffold stub, so the 400 masks a 501 — the fix must carry the request through to real behaviour).

Cause 2 of 2: StartFuotaTask rejects LoRaWAN.StartTime.
AWS: Smithy — StartFuotaTask (PUT /fuota-tasks/{Id}, success 204) takes LoRaWAN of shape LoRaWANStartFuotaTask, whose only member StartTime targets StartTime ("Start time of a FUOTA task") and carries @timestampFormat "date-time"; so the member is an ISO-8601 string on the wire. AWS accepts the body and then answers either 204, or a modeled refusal about the task's own state: ValidationException whose message names the unreadable image, fixed text "Unable to find S3 bucket and key, <bucket> and key <key> using IAM role <roleArn>. Please verify the S3 bucket permissions and the trust relationships configured for the role assigned to the FUOTA task." (bucket, key and role ARN are values from the request/state), or ConflictException "Can not complete step because startTime is already set for resource type FuotaTask, resource Id <taskId>" on a second start (taskId from the request).
Emulator: HTTP 400 ValidationException "invalid timestamp: failed to parse int" — the same epoch-int parse applied to a date-time member.
Why: a FUOTA task can never be started, so its Status stays Pending forever, LoRaWAN.StartTime is never set, and neither the image-access ValidationException nor the already-scheduled ConflictException can ever be observed. Clients relying on a start-time schedule see a bogus input error.
Fix: same deserialization layer as Cause 1, applied to StartFuotaTask's input schema (LoRaWANStartFuotaTask.StartTime); the state/conflict refusals then belong in the operation's handler.
Traces: iotwireless.fuota-task-restart-after-start-time-set-rejected (error: ValidationException "invalid timestamp: failed to parse int" from StartFuotaTask), iotwireless.fuota-start-rejected-until-role-can-read-image (mismatch: got ('ValidationException', 'invalid timestamp: failed to parse int')). Handler file: services/service-com-amazonaws-iotwireless-iotwireless/src/ops/start_fuota_task.rs.

## Evidence
StartMulticastGroupSession, LoRaWAN={"SessionStartTime": <datetime>, "SessionTimeout": 60, "DlDr": 8, "DlFreq": 923300000}
  expected (AWS): 204 for a start 10 minutes ahead; 400 ValidationException "start time should be between 3 minutes and 48 hours from now" for a start 1 minute or 49 hours ahead
  actual (emulator): 400 ValidationException "invalid timestamp: failed to parse int" for every start time

StartFuotaTask, LoRaWAN={"StartTime": <datetime>}
  expected (AWS): 204, or 400 ValidationException "Unable to find S3 bucket and key, <bucket> and key <key> using IAM role <roleArn>. Please verify the S3 bucket permissions and the trust relationships configured for the role assigned to the FUOTA task.", or 409 ConflictException "Can not complete step because startTime is already set for resource type FuotaTask, resource Id <taskId>"
  actual (emulator): 400 ValidationException "invalid timestamp: failed to parse int"

Model: LoRaWANMulticastSession.SessionStartTime -> SessionStartTimeTimestamp, timestampFormat date-time; LoRaWANStartFuotaTask.StartTime -> StartTime, timestampFormat date-time. Both are therefore ISO-8601 strings in the JSON body, not epoch numbers.

## Steps to Reproduce
1. CreateMulticastGroup(Name=..., LoRaWAN={RfRegion: US915, DlClass: ClassC}) -> note Id.
2. StartMulticastGroupSession(Id=<id>, LoRaWAN={SessionStartTime: now+10min (UTC, whole seconds), SessionTimeout: 60, DlDr: 8, DlFreq: 923300000}) with any AWS SDK -> emulator answers 400 ValidationException "invalid timestamp: failed to parse int" instead of 204; repeating with now+1min shows the window refusal is never reached.
3. CreateFuotaTask(Name=..., FirmwareUpdateImage=s3://<bucket>/<key>, FirmwareUpdateRole=<roleArn>, LoRaWAN={RfRegion: US915}) -> note Id.
4. StartFuotaTask(Id=<id>, LoRaWAN={StartTime: now+45min}) -> emulator answers 400 ValidationException "invalid timestamp: failed to parse int" instead of 204 / the image-access ValidationException / the already-started ConflictException.
