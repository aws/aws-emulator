Issue-Source: trace-sweep-divergence
Issue-Trace: chime-sdk-voice-voice-connector-update-delete
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: chime-sdk-voice-voice-connector-update-delete chime-sdk-voice-voice-connector-settings-lifecycle chime-sdk-voice-voice-tone-analysis-request-validation
Issue-Shape-Id: com.amazonaws.chimesdkvoice#ChimeSDKTelephonyService
Issue-Key: chime-sdk-voice|CreateVoiceConnector|2xx->2xx|$.VoiceConnector.NetworkType
Issue-Key: chime-sdk-voice|CreateVoiceConnector|2xx->2xx|$.VoiceConnector.OutboundHostName
Issue-Key: chime-sdk-voice|GetVoiceConnector|2xx->2xx|$.VoiceConnector.NetworkType
Issue-Key: chime-sdk-voice|UpdateVoiceConnector|2xx->2xx|$.VoiceConnector.NetworkType
Issue-Commit: d175d2ebd7b20f74cd38c4d9513abc55db3772cc

# VoiceConnector response projection omits NetworkType and announces a non-emulator OutboundHostName

> _Note: this issue was identified against the code base at commit `d175d2ebd7b20f74cd38c4d9513abc55db3772cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVoiceConnector

## Description
Four causes in this bundle, all in the one place every Voice Connector response is built: the stored record and its projection onto the model's `VoiceConnector` shape. Every operation that returns a connector goes through `VoiceConnectorRecord::to_shape`, so a member the record never holds is absent from the create, the get, the list and the update alike.

Cause 1 of 4: CreateVoiceConnector omits `VoiceConnector.NetworkType`.
AWS: Smithy: `CreateVoiceConnector` -> output `VoiceConnector`, whose member `NetworkType` targets the enum `NetworkType` with values `IPV4_ONLY | DUAL_STACK` ("The type of network for the Voice Connector."). The input member `NetworkType` is OPTIONAL, and the model states no default for it; the recording shows that AWS nevertheless populates the output member on a create that names no network type, with the value `IPV4_ONLY` (recorded behavior only for the DEFAULT; the member and its value set are the model's). HTTP status is 201, as the operation's `http` binding declares.
Emulator: answers 201 with `$.VoiceConnector.NetworkType` ABSENT (expected 'IPV4_ONLY', got None). The record is built with `network_type: None` and then set straight from the request (`record.network_type = input.network_type`), so an omitted input member leaves the output member unset.
Why: a client that reads `NetworkType` off a create response to decide addressing (IPv4-only vs dual-stack) gets no value where AWS always gives one, so it either branches on a null it never sees against AWS or re-reads the connector and still finds nothing. An SDK that models the member as an enum sees `None` instead of a known variant.
Fix: the create handler's record construction plus the shared record default — the connector record must default its network type to the enum's `IPV4_ONLY` when the request names none, so the projection has a value to emit.
Traces: chime-sdk-voice-voice-connector-update-delete#setup#1.

Cause 2 of 4: CreateVoiceConnector returns an OutboundHostName that is not a host of this emulator.
AWS: Smithy: `VoiceConnector$OutboundHostName` (target `String`) — "The outbound host name for the Voice Connector." The fixture does not assert the literal host (expected '<shaped-not-asserted>'); what it requires is that a host the emulator hands a client back is an address the emulator is actually reached on, since this is the host a caller then directs SIP traffic at (recorded behavior only).
Emulator: returns a host in the real AWS Voice Connector domain — `<VoiceConnectorId>.voiceconnector.chime.aws` — which the replay classifies `host_not_own`: the emulator announced an address other than the one it is reached on. The suffix is a hard-coded constant in this service's state module.
Why: a client that follows the announced host leaves the emulator entirely and dials a name in AWS's own domain, so the emulated connector cannot be exercised end to end and, in a sealed environment, the lookup simply fails. The generated id is not the requirement here; the DOMAIN is.
Fix: the connector record's outbound-host derivation (the host suffix it composes) must be derived from the endpoint this emulator serves rather than from the literal AWS Voice Connector domain.
Traces: chime-sdk-voice-voice-connector-update-delete#setup#1.

Cause 3 of 4: GetVoiceConnector omits `VoiceConnector.NetworkType`.
AWS: Smithy: `GetVoiceConnector` (GET /voice-connectors/{VoiceConnectorId}, 200) -> output `VoiceConnector` carrying `NetworkType` (enum `IPV4_ONLY | DUAL_STACK`). A read reports the same connector a create reported, so the member is present with the connector's network type (`IPV4_ONLY` for a connector created without one, per the recording).
Emulator: answers 200 with `$.VoiceConnector.NetworkType` missing (expected 'IPV4_ONLY', got None), because the stored record carries no network type to project.
Why: a client that reads back a connector to learn its network type cannot, so a settings-lifecycle flow that creates then inspects a connector sees an incomplete resource.
Fix: follows from Cause 1 — the stored record's default; the get handler's projection needs no separate change once the record holds a value.
Traces: chime-sdk-voice-voice-connector-settings-lifecycle#task#2 (cluster also covers chime-sdk-voice-voice-connector-update-delete#task#2, chime-sdk-voice-voice-connector-update-delete#task#5 and chime-sdk-voice-voice-tone-analysis-request-validation#task#2).

Cause 4 of 4: UpdateVoiceConnector omits `VoiceConnector.NetworkType`.
AWS: Smithy: `UpdateVoiceConnector` (PUT /voice-connectors/{VoiceConnectorId}, 200) -> output `VoiceConnector` carrying `NetworkType`. The update's input declares only `Name` and `RequireEncryption`, so the connector's network type is unchanged and reported as it stands (`IPV4_ONLY` in the recording).
Emulator: answers 200 with `$.VoiceConnector.NetworkType` missing (expected 'IPV4_ONLY', got None).
Why: an update response is what a client stores as the connector's new state, so the member silently disappears from the client's view of a resource it just modified.
Fix: follows from Cause 1 — the record default; the update handler must also preserve the stored network type when it rewrites the record.
Traces: chime-sdk-voice-voice-connector-update-delete#task#4.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture chime-sdk-voice-voice-connector-update-delete, step setup#1, CreateVoiceConnector: expected status 201, actual 201; `$.VoiceConnector.NetworkType`: missing (expected 'IPV4_ONLY', got None).
Cause 2 — same step: `$.VoiceConnector.OutboundHostName`: host_not_own (expected '<shaped-not-asserted>', got 'b71c2f7f381e345467615.voiceconnector.chime.aws' — a host in AWS's own domain, not one this emulator answers on; the id half is a per-run value and not part of the requirement).
Cause 3 — fixture chime-sdk-voice-voice-connector-settings-lifecycle, step task#2, GetVoiceConnector: expected 200, actual 200; `$.VoiceConnector.NetworkType`: missing (expected 'IPV4_ONLY', got None). 4 further steps in the same cluster.
Cause 4 — fixture chime-sdk-voice-voice-connector-update-delete, step task#4, UpdateVoiceConnector: expected 200, actual 200; `$.VoiceConnector.NetworkType`: missing (expected 'IPV4_ONLY', got None).

Model evidence: smithy_query(query="shape", shape="VoiceConnector,NetworkType") — `VoiceConnector$NetworkType` targets `com.amazonaws.chimesdkvoice#NetworkType`, enum values ['IPV4_ONLY','DUAL_STACK']; `VoiceConnector$OutboundHostName` targets `String`.
Code evidence: services/service-com-amazonaws-chimesdkvoice-chimesdktelephonyservice/src/state.rs:42 (`OUTBOUND_HOST_SUFFIX = "voiceconnector.chime.aws"`), :286 (host composed from that suffix), :300 (`network_type: None`), :320 (projection emits `self.network_type` as-is); src/ops/create_voice_connector.rs:223 (`record.network_type = input.network_type`, no default).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Cause 1 and 2: replay fixture chime-sdk-voice-voice-connector-update-delete step setup#1 — POST /voice-connectors with a Name and RequireEncryption and NO NetworkType. The 201 body's `VoiceConnector` has no `NetworkType` member, and its `OutboundHostName` ends in `.voiceconnector.chime.aws`.
3. Cause 3: replay chime-sdk-voice-voice-connector-settings-lifecycle step task#2 — GET /voice-connectors/{VoiceConnectorId} for that connector; the 200 body's `VoiceConnector` still has no `NetworkType`.
4. Cause 4: replay chime-sdk-voice-voice-connector-update-delete step task#4 — PUT /voice-connectors/{VoiceConnectorId} with Name and RequireEncryption; the 200 body's `VoiceConnector` has no `NetworkType`.
