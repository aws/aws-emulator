Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-shadow-null-removal-delta-and-client-token
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-shadow-null-removal-delta-and-client-token
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|UpdateThingShadow|2xx->4xx|$.metadata
Issue-Key: iot-data|UpdateThingShadow|2xx->4xx|$.state
Issue-Key: iot-data|UpdateThingShadow|2xx->4xx|$.timestamp
Issue-Key: iot-data|UpdateThingShadow|2xx->4xx|$.version
Issue-Key: iot-data|UpdateThingShadow|2xx->4xx|status
Issue-Key: iot-data|UpdateThingShadow|2xx->2xx|$.metadata.desired.mode
Issue-Key: iot-data|UpdateThingShadow|4xx->2xx|$.message
Issue-Key: iot-data|UpdateThingShadow|4xx->2xx|$.traceId
Issue-Key: iot-data|UpdateThingShadow|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot-data|UpdateThingShadow|4xx->2xx|status
Issue-Key: iot-data|GetThingShadow|2xx->2xx|$.state.desired
Issue-Key: iot-data|GetThingShadow|2xx->2xx|$.state.reported.color
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot-data UpdateThingShadow mishandles null-attribute removal, metadata timestamps and clientToken validation

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateThingShadow

## Description
The shadow document engine behind UpdateThingShadow/GetThingShadow diverges from AWS's Device Shadow semantics in four ways: the documented null-removal update is refused outright, the resulting document is therefore wrong on the following read, per-attribute metadata timestamps are not recorded for every updated attribute, and an illegal clientToken is accepted. 4 causes, all in UpdateThingShadow's payload handling and the shadow state it writes.

Cause 1 of 4: setting a desired attribute to null is refused 400 instead of removing it.
AWS: Smithy: UpdateThingShadow is POST /things/{thingName}/shadow with a required JsonDocument @httpPayload blob and HTTP code 200; the modeled client refusals are InvalidRequestException 400, ConflictException 409, RequestEntityTooLargeException 413 and UnsupportedDocumentEncodingException 415. The payload's semantics are not modeled (opaque blob), so: recorded behavior / AWS Device Shadow rule — writing null to a shadow attribute DELETES that attribute, and writing null to a whole section deletes the section. The recording shows the update answering 200 with state.desired reduced to null, a metadata.desired section carrying a timestamp, the shadow's incremented version number and a top-level timestamp.
Emulator: answers 400 and returns no state, metadata, version or timestamp at all — the null-removal update is rejected as a malformed document rather than applied.
Why: null-removal is the only way a client can retract a desired value. Refusing it means a desired attribute can never be cleared, so a device keeps acting on a stale desired state forever, and the client sees a 400 that implies its own document is malformed.
Fix: UpdateThingShadow's payload merge in the shadow state — treat a null value as a delete of that key/section rather than a validation failure, and return the merged document with the bumped version.
Traces: iotdata-shadow-null-removal-delta-and-client-token#task#8.

Cause 2 of 4: the following GetThingShadow serves a document that does not reflect the updates.
AWS: Smithy: GetThingShadow is GET /things/{thingName}/shadow returning the JsonDocument payload, HTTP code 200. recorded behavior: after the sequence of updates in this fixture the document's state.reported carries the last reported value and state.desired is absent, having been removed by the null-removal update.
Emulator: answers 200 but with the wrong document — state.reported holds the earlier value instead of the latest reported one, and state.desired is still present carrying the value that should have been removed.
Why: the shadow is the contract between device and application. A read that returns a superseded reported value and a resurrected desired value makes the device re-apply a retracted instruction and makes the application believe a reported change never landed. This is downstream of Cause 1 but is its own cluster and must be re-replayed separately.
Fix: UpdateThingShadow's state merge and the document GetThingShadow serializes from it — the stored document must be the accumulated merge result, including deletions.
Traces: iotdata-shadow-null-removal-delta-and-client-token#task#9.

Cause 3 of 4: metadata is not recorded for every attribute the update wrote.
AWS: payload semantics are unmodeled (JsonDocument blob), so recorded behavior only — AWS's shadow document mirrors each state section in a metadata section, with a per-attribute object carrying a timestamp for every attribute present in state. In the recording the update's response carries metadata.desired.<attribute> = {"timestamp": <epoch seconds>}; the timestamp value itself is per-run and is not a requirement, but its presence for each updated attribute is.
Emulator: answers 200 but metadata.desired.<attribute> is absent — the metadata section is not populated per attribute.
Why: clients and devices use metadata timestamps to decide whether their local copy is newer than the shadow and to resolve concurrent writes; with the sub-object missing, every such comparison fails or is skipped.
Fix: UpdateThingShadow's response/state serializer — build the metadata tree to mirror the merged state, one timestamped entry per attribute.
Traces: iotdata-shadow-null-removal-delta-and-client-token#task#3.

Cause 4 of 4: an illegal clientToken is accepted with 200 instead of being refused.
AWS: payload semantics are unmodeled, so recorded behavior only — the shadow document's clientToken is a bounded string, and a document whose clientToken violates that bound is refused 400 with the fixed message "invalid client token" (no variable part) and a traceId plus an x-amzn-errortype header; the modeled carrier is InvalidRequestException (httpError 400).
Emulator: answers 200 and applies the update, with no message, no traceId and no x-amzn-errortype header.
Why: clientToken is how a client correlates its own request with the accepted/rejected/delta messages the shadow service emits. Accepting an illegal token means the emulator will echo a token AWS would have refused, so a client that validates the echoed token against what it sent diverges; worse, a malformed-request bug in the client is silently committed to the shadow instead of being surfaced.
Fix: UpdateThingShadow's payload validation — validate clientToken before merging, and refuse with the 400 InvalidRequestException body.
Traces: iotdata-shadow-null-removal-delta-and-client-token#task#7.

## Evidence
Cause 1 — iotdata-shadow-null-removal-delta-and-client-token#task#8: expected status 200 with $.state {'desired': None}, $.metadata {'desired': {'timestamp': <shaped>}}, $.version 4 and $.timestamp present; actual status 400 with all four members absent.
Cause 2 — iotdata-shadow-null-removal-delta-and-client-token#task#9 (GetThingShadow, status 200 both): $.state.reported.color expected 'green', got 'blue'; $.state.desired expected absent, got {'color': 'green'}.
Cause 3 — iotdata-shadow-null-removal-delta-and-client-token#task#3 (status 200 both): $.metadata.desired.mode expected {'timestamp': <shaped>}, got None.
Cause 4 — iotdata-shadow-null-removal-delta-and-client-token#task#7: expected status 400 with $.message 'invalid client token' plus $.traceId and $headers.x-amzn-errortype present; actual status 200 with all three absent.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), or by hand against the data listener for a thing <t>:
1. POST /things/<t>/shadow with {"state":{"desired":{"mode":"auto"}}} -> response must carry metadata.desired.mode.timestamp; emulator omits it (Cause 3).
2. POST /things/<t>/shadow with a document whose clientToken exceeds the allowed bound -> expect 400 'invalid client token' with traceId and x-amzn-errortype; emulator answers 200 and applies the update (Cause 4).
3. POST /things/<t>/shadow with {"state":{"desired":null}} -> expect 200 with state.desired null, a metadata.desired timestamp, the bumped version and a timestamp; emulator answers 400 (Cause 1).
4. GET /things/<t>/shadow -> expect state.reported to hold the latest reported value and state.desired to be gone; emulator serves the superseded reported value and the removed desired section (Cause 2).
