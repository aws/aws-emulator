Issue-Source: trace-sweep-divergence
Issue-Trace: iot-wireless-fuota-task-lifecycle
Issue-Step: task#7
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-wireless-fuota-task-lifecycle
Issue-Shape-Id: com.amazonaws.iotwireless#iotwireless
Issue-Key: iotwireless|CreateFuotaTask|4xx->4xx|$.message
Issue-Commit: d0b60cf4e620490598360e9e180ae29d3f253b9d

# CreateFuotaTask validation rejection returns 400 with no message member

> _Note: this issue was identified against the code base at commit `d0b60cf4e620490598360e9e180ae29d3f253b9d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFuotaTask

## Description
Cause 1 of 1: CreateFuotaTask rejects a constraint-violating request with the right HTTP status but an empty error body — the framework validation message is never serialized.

AWS: Smithy (services/service-com-amazonaws-iotwireless-iotwireless) models CreateFuotaTask input member FirmwareUpdateRole as @required with @length min 1, max 2048 (likewise FirmwareUpdateImage, @required @length 1..4096), and binds ValidationException (errorKind client, httpError 400) to the operation. An empty-string FirmwareUpdateRole therefore violates the modeled length constraint and AWS answers HTTP 400 with the AWS framework constraint-violation payload, whose message member is serialized in the lower-case `message` form used by the standard validation envelope (the modeled ValidationException member is `Message`; the recorded wire body uses `message`). The message follows AWS's fixed constraint-violation template: the fixed text is "<N> validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must have length greater than or equal to <min>". Its variable parts all come from the request and the model: <N> is the number of violations found in this request, <value> is the rejected value as sent (the empty string here), <memberName> is the lowerCamel input member name that failed (`firmwareUpdateRole`), and <min> is the member's modeled @length minimum (1). The recorded response for this step is status 400 with message "1 validation error detected: Value '' at 'firmwareUpdateRole' failed to satisfy constraint: Member must have length greater than or equal to 1" — the numbers and member name in that recording are template fills, not literals to hard-code.

Emulator: replay of iot-wireless-fuota-task-lifecycle step task#7 shows the emulator does return status 400 (so it does detect that the request is bad), but the response body carries no `message` key at all: the replay reports `$.message: missing (expected "1 validation error detected: Value '' at 'firmwareUpdateRole' failed to satisfy constraint: Member must have length greater than or equal to 1", got None)`. The client receives a 400 whose body does not say what was wrong.

Why: a client relying on AWS behavior reads the validation message to learn which member it got wrong and how. With no `message` member, SDK error objects surface an empty/absent error message, so error-handling code that logs or branches on the text, or that shows the operator which field to correct, gets nothing; retry/repair logic that parses the member name out of the message cannot identify `firmwareUpdateRole`. Only the bare status survives, which makes an input bug indistinguishable from any other 400.

Fix: the CreateFuotaTask input validation path and its error serializer — the code that detects the @required/@length violation must build the ValidationException payload from a template (violation count, offending value, lowerCamel member name, modeled minimum) and the serializer must emit it under the `message` key the iotwireless wire error envelope uses, rather than returning a 400 with an empty body. The same template should be shared by the other @required/@length-constrained members of this input (e.g. FirmwareUpdateImage) so any constraint violation reports itself.

Traces: iot-wireless-fuota-task-lifecycle#task#7 (CreateFuotaTask, expected 400 with message, got 400 with no message).

Note: the other iotwireless divergences in this replay (GetNetworkAnalyzerConfiguration, GetWirelessGateway, CreateWirelessGatewayTaskDefinition/GetWirelessGatewayTaskDefinition) are already owned by open issues 20261106T192513Z-iotwireless-networkanalyzerconfiguration-read-path-drops-tracecontent-description-and.md, 20261103T042841Z-iotwireless-getwirelessgateway-drops-beaconing-ignores-renames-and-deletion.md and 20261104T232607Z-iotwireless-wirelessgatewaytaskdefinition-lifecycle-unenforced-missing-update-accepted-and-deleted.md and are not re-filed here. Open issue 20261111T155554Z-iotwireless-createwirelessdevice-duplicate-deveui-conflictexception-carries-non-aws-message.md concerns a different operation's ConflictException wording and does not cover this ValidationException gap.

## Evidence
Fixture iot-wireless-fuota-task-lifecycle, step task#7, operation CreateFuotaTask (POST /fuota-tasks).

Expected (recorded AWS response): HTTP 400, body member `message` = "1 validation error detected: Value '' at 'firmwareUpdateRole' failed to satisfy constraint: Member must have length greater than or equal to 1". Fixed text: "<N> validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must have length greater than or equal to <min>". Variable fills from request/model: N=1 (violations in this request), value='' (the FirmwareUpdateRole sent), member=firmwareUpdateRole (failing input member, lowerCamel), min=1 (modeled @length min).

Actual (emulator): HTTP 400, no `message` member in the body. Replay detail: `$.message: missing (expected "1 validation error detected: Value '' at 'firmwareUpdateRole' failed to satisfy constraint: Member must have length greater than or equal to 1", got None)`.

Status matches (400 -> 400); the divergence is entirely the absent error message. Model evidence: CreateFuotaTask.FirmwareUpdateRole @required @length{min:1,max:2048}; operation errors include ValidationException (client, httpError 400); ValidationException member Message @length{0,2048}, wire form `message`.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay wire fixture iot-wireless-fuota-task-lifecycle against it (replay_wire_corpus).
3. At step task#7, CreateFuotaTask is called with FirmwareUpdateRole set to the empty string (violating its modeled @length min 1).
4. Observe the emulator answers HTTP 400 but the JSON body contains no `message` member, whereas the recorded AWS response carries the "1 validation error detected: Value '' at 'firmwareUpdateRole' failed to satisfy constraint: Member must have length greater than or equal to 1" validation message.
