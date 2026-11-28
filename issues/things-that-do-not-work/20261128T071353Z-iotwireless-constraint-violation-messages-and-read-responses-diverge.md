Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-device-profile-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-device-profile-lifecycle iot-wireless-wireless-device-import-task-start-validation iot-wireless-fuota-task-lifecycle iot-wireless-fuota-start-unknown-and-deleted-task-not-found iot-wireless-fuota-task-start-past-start-time iot-wireless-multicast-send-data-validation iotwireless-multicast-group-session-schedule-window
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|CreateDeviceProfile|4xx->4xx|$.Message
Issue-Key: iotwireless|StartWirelessDeviceImportTask|4xx->4xx|$.Message
Issue-Key: iotwireless|GetFuotaTask|2xx->2xx|$.ProtocolType
Issue-Key: iotwireless|GetFuotaTask|2xx->2xx|$.CreatedAt
Issue-Related: 20261128T071353Z-iotwireless-session-starts-refuse-date-time-body-timestamps.md
Issue-Commit: 96e759e54960d5d1fd2a95fa47259d85e3163a48

# iotwireless constraint-violation messages and read responses diverge from AWS

> _Note: this issue was identified against the code base at commit `96e759e54960d5d1fd2a95fa47259d85e3163a48`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeviceProfile

## Description
Four fidelity defects in iotwireless operations that ARE implemented: the boundary validator renders constraint-violation messages from the raw on-disk model (wrong numeric limit, regex anchors left in), and the read responses drop a member AWS returns and serialize CreatedAt in a shape the corpus never carries. 4 causes; all four must be closed.

Cause 1 of 4: CreateDeviceProfile's maxEirp refusal quotes the wrong limit.
AWS: recorded behavior (the model is wider than the service) — Smithy LoRaWANDeviceProfile$MaxEirp targets MaxEirp with @range min 0 max 36, but the recorded AWS refusal is 400 ValidationException with Message "1 validation error detected: Value at 'loRaWAN.maxEirp' failed to satisfy constraint: Member must have value less than or equal to 15". Fixed text: "1 validation error detected: Value at '<memberPath>' failed to satisfy constraint: Member must have value less than or equal to <limit>"; the variable parts are the lowerCamel member path of the offending member in the request ('loRaWAN.maxEirp') and the enforced limit, which for MaxEirp is 15. So AWS enforces maxEirp <= 15 and says 15.
Emulator: same status (400) and same error code, but the message ends "... less than or equal to 36" — it renders the model's @range max verbatim, and correspondingly accepts maxEirp values 16..36 that AWS refuses.
Why: a client that reads the refusal to learn the permitted ceiling is told 36, and a device profile created with maxEirp in 16..36 succeeds on the emulator and fails on AWS — the exact class of bug an emulator exists to catch before deployment.
Fix: CreateDeviceProfile's validation (the range check behind CREATE_DEVICE_PROFILE_INPUT_SCHEMA for LoRaWAN.MaxEirp in services/service-com-amazonaws-iotwireless-iotwireless/src/ops/create_device_profile.rs and src/ops/types.rs) must enforce and report the service's limit of 15 rather than the model's 36.
Traces: iot-wireless-device-profile-lifecycle#task#4.

Cause 2 of 4: pattern-constraint messages leak the anchors of the model regex.
AWS: Smithy — StartWirelessDeviceImportTask (POST /wireless_device_import_task) has required DestinationName of shape DestinationName, length 0..128, @pattern "^[a-zA-Z0-9-_]+$". The recorded AWS refusal is 400 ValidationException with Message "1 validation error detected: Value at 'destinationName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-_]+": fixed text "1 validation error detected: Value at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>", where <pattern> is the member's pattern with the ^ and $ anchors stripped and <memberPath> is the request member's lowerCamel path.
Emulator: same status and code, but the message ends "... pattern: ^[a-zA-Z0-9-_]+$" — the model's trait value printed with its anchors.
Why: callers and test suites match the refusal text, and anything that echoes the quoted pattern back to a user or compiles it sees a different regex than AWS reports. It is service-wide: every @pattern member in iotwireless renders the same way.
Fix: the shared constraint-message renderer in the boundary validator must strip leading ^ / trailing $ when quoting a @pattern value, for every operation, not just this one.
Traces: iot-wireless-wireless-device-import-task-start-validation#task#1 (and its cluster member task#3).

Cause 3 of 4: GetFuotaTask omits the top-level ProtocolType member.
AWS: recorded behavior only — the on-disk model's GetFuotaTaskResponse declares no ProtocolType member (the model is stale), but every recorded 200 response from GetFuotaTask (GET /fuota-tasks/{Id}) carries top-level "ProtocolType": "LoRaWAN" alongside Arn/Id/Status/Name/LoRaWAN/CreatedAt. The value names the protocol family the task was created for, so a LoRaWAN FUOTA task reports "LoRaWAN".
Emulator: the member is absent from the response body entirely.
Why: a client that branches on the task's protocol family (or an SDK whose generated response model requires it) reads null and cannot tell what kind of task it got back; round-tripping the response loses the field.
Fix: the GetFuotaTask output shape and serializer must carry ProtocolType, populated from the task record in src/state.rs and emitted by src/ops/get_fuota_task.rs.
Traces: iot-wireless-fuota-task-lifecycle#task#3 (cluster member task#6), iot-wireless-fuota-start-unknown-and-deleted-task-not-found#task#7 (cluster member iot-wireless-fuota-task-start-past-start-time#task#7).

Cause 4 of 4: CreatedAt is serialized as a sub-second fractional epoch.
AWS: recorded behavior only — CreatedAt targets com.amazonaws.iotwireless#CreatedAt ("Created at timestamp for the resource"), a timestamp with no @timestampFormat override, and the corpus asserts its SHAPE rather than an instant. Every recorded CreatedAt in the corpus is a whole-second timestamp value; no recorded response carries a fractional-second one. Hypothesis: iotwireless stamps and serializes resource creation times at second precision.
Emulator: emits a fractional epoch number with sub-second digits, e.g. CreatedAt 1795849713.734445 from GetMulticastGroup and 1795849740.052655 from GetFuotaTask, which does not satisfy the recorded shape. The divergence is the serialized shape/precision, not the instant: the clock value itself is per-run and is not a requirement.
Why: clients and SDK date parsers that accept the shape AWS emits (and any consumer comparing or formatting the value) see a timestamp in a form AWS never returns, so strict parsers reject the response and equality/precision-sensitive assertions fail.
Fix: the timestamp serialization for this service's responses (the DateTime construction in src/ops/get_multicast_group.rs:193 and the equivalent in src/ops/get_fuota_task.rs, or the creation stamp written in src/state.rs) must produce the recorded whole-second form.
Traces: iot-wireless-multicast-send-data-validation#task#5 (cluster member iotwireless-multicast-group-session-schedule-window#task#13), iot-wireless-fuota-start-unknown-and-deleted-task-not-found#task#7; the same key also diverges on iotwireless-multicast-group-session-schedule-window#task#10, whose other keys are filed in the iotwireless session-start bundle.

## Evidence
Cause 1 — CreateDeviceProfile(LoRaWAN={MaxEirp: <above the limit>})
  expected (AWS): 400 ValidationException, $.Message = "1 validation error detected: Value at 'loRaWAN.maxEirp' failed to satisfy constraint: Member must have value less than or equal to 15"
  actual (emulator): 400 ValidationException, $.Message = "... Member must have value less than or equal to 36"
  fixture iot-wireless-device-profile-lifecycle step task#4

Cause 2 — StartWirelessDeviceImportTask(DestinationName=<violates the pattern>)
  expected (AWS): 400 ValidationException, $.Message = "1 validation error detected: Value at 'destinationName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9-_]+"
  actual (emulator): 400 ValidationException, $.Message = "... pattern: ^[a-zA-Z0-9-_]+$"
  fixture iot-wireless-wireless-device-import-task-start-validation steps task#1, task#3

Cause 3 — GetFuotaTask(Id=<id>)
  expected (AWS): 200 with $.ProtocolType = "LoRaWAN"
  actual (emulator): 200 with $.ProtocolType missing (None)
  fixtures iot-wireless-fuota-task-lifecycle steps task#3, task#6; iot-wireless-fuota-start-unknown-and-deleted-task-not-found step task#7; iot-wireless-fuota-task-start-past-start-time step task#7

Cause 4 — GetMulticastGroup / GetFuotaTask 200 bodies
  expected (AWS): $.CreatedAt in the recorded whole-second timestamp shape
  actual (emulator): $.CreatedAt = 1795849713.734445 (GetMulticastGroup), 1795849740.052655 (GetFuotaTask) — a sub-second fractional epoch
  fixtures iot-wireless-multicast-send-data-validation step task#5; iotwireless-multicast-group-session-schedule-window steps task#13, task#10; iot-wireless-fuota-start-unknown-and-deleted-task-not-found step task#7

Model citations: LoRaWANDeviceProfile$MaxEirp -> MaxEirp @range 0..36 (service enforces 15 per the recording); DestinationName @pattern "^[a-zA-Z0-9-_]+$"; GetFuotaTaskResponse members (no ProtocolType in the on-disk model); CreatedAt timestamp with no @timestampFormat.

## Steps to Reproduce
1. CreateDeviceProfile(Name="p", LoRaWAN={MaxEirp: 40}) -> emulator's 400 message quotes the limit 36 instead of 15; CreateDeviceProfile(..., MaxEirp: 20) succeeds on the emulator where AWS refuses it (Cause 1).
2. StartWirelessDeviceImportTask(DestinationName="bad name!", Sidewalk={...}) -> emulator's 400 message quotes the pattern as ^[a-zA-Z0-9-_]+$ instead of [a-zA-Z0-9-_]+ (Cause 2).
3. CreateFuotaTask(Name=..., LoRaWAN={RfRegion: US915}, FirmwareUpdateImage=..., FirmwareUpdateRole=...) then GetFuotaTask(Id=<id>) -> the 200 body has no ProtocolType member (AWS returns "ProtocolType": "LoRaWAN") (Cause 3).
4. CreateMulticastGroup(...) then GetMulticastGroup(Id=<id>) (likewise GetFuotaTask) -> CreatedAt comes back as a fractional epoch with sub-second digits instead of the whole-second form the recorded responses carry (Cause 4).
