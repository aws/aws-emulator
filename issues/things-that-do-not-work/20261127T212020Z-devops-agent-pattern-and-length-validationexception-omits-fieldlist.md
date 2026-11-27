Issue-Source: trace-sweep-divergence
Issue-Trace: devops-agent-delete-asset-file-path-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: devops-agent-delete-asset-file-path-validation devops-agent-delete-asset-identifier-validation devops-agent-get-asset-id-pattern-and-version-validation devops-agent-list-pending-messages-unknown-execution
Issue-Shape-Id: com.amazonaws.devopsagent#DevOpsAgent
Issue-Key: devops-agent|DeleteAssetFile|4xx->4xx|$.fieldList
Issue-Key: devops-agent|DeleteAssetFile|4xx->4xx|$.message
Issue-Key: devops-agent|DeleteAsset|4xx->4xx|$.fieldList
Issue-Key: devops-agent|DeleteAsset|4xx->4xx|$.message
Issue-Key: devops-agent|GetAsset|4xx->4xx|$.fieldList
Issue-Key: devops-agent|GetAsset|4xx->4xx|$.message
Issue-Key: devops-agent|ListPendingMessages|4xx->4xx|$.fieldList
Issue-Key: devops-agent|ListPendingMessages|4xx->4xx|$.message
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# devops-agent pattern and length ValidationException omits fieldList and echoes the submitted value

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteAssetFile, DeleteAsset, GetAsset, ListPendingMessages

## Description
Four net-new wire divergences in devops-agent that share ONE fault: when an input member violates a modeled @length or @pattern constraint the emulator answers 400 ValidationException with the right status and code but the wrong body — the modeled `fieldList` is absent and `message` is worded differently from AWS, including echoing the rejected value back to the caller. This is the @length/@pattern half of the validation-envelope defect; the @range half is filed separately as 'devops-agent range-constraint ValidationException omits fieldList ...'. A fixer must close all four causes below before submitting.

=== Shared requirement (applies to all four causes) ===
AWS: Smithy: ValidationException { message (required), fieldList: list of ValidationExceptionField { path (required) — "A JSONPointer expression to the structure member whose value failed to satisfy the modeled constraints", message (required) } }, httpError 400, modeled on all four operations below. Recorded behavior:
  message       = "<N> validation error detected. <the field message>"  (period after "detected")
  field.path    = "/<memberName>"  (JSON pointer, leading slash, also for @httpLabel-bound members)
  for a @pattern violation: field.message = "Value at '/<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <the modeled pattern>"
  for a @length violation: field.message = "Value with length <actual length> at '/<member>' failed to satisfy constraint: Member must have length between <min> and <max>, inclusive"
Fixed text: " validation error detected. ", "Value at '" / "Value with length <n> at '", "' failed to satisfy constraint: Member must satisfy regular expression pattern: " / "Member must have length between <min> and <max>, inclusive". Variable parts taken from request and model: N, the member pointer, the modeled pattern or length bounds, and — for @length only — the measured length of the submitted value. AWS never echoes the submitted value itself: for a pattern violation it says "Value at" with no value, and for a length violation it reports the LENGTH, not the string.
Emulator: `fieldList` is absent, and message reads "<N> validation error detected: Value '<the whole submitted value>' at '<memberName>' failed to satisfy constraint: Member must <...>" — colon instead of period, the submitted value inlined, the member named without the pointer slash, and for @length the one-sided "Member must have length less than or equal to <max>" instead of AWS's "Value with length <n> ... between <min> and <max>, inclusive".
Why: clients read fieldList to attribute the failure to an input field and get null; clients and operators matching AWS's message text do not recognize the emulator's; and inlining an oversized value makes the error body grow with the rejected input (the recorded 513-character path is reproduced verbatim inside the emulator's message) where AWS emits a constant-size message naming only the length.
Fix: the shared constraint-validation stage of the devops-agent request walk and the ValidationException serializer: emit one ValidationExceptionField per violation with a JSON-pointer path, render "Value at" for @pattern and "Value with length <n> at" for @length, use the inclusive both-bounds length phrase, compose the summary with "<N> validation error detected. ", and never inline the submitted value.

=== Cause 1 of 4: DeleteAssetFile.path @length 1..512 ===
AWS: Smithy: DeleteAssetFile input path -> AssetFilePath, @httpLabel, required, length {min: 1, max: 512}, pattern ^[a-zA-Z0-9_./ ()-]+$. A 513-character path is refused 400 ValidationException with field path "/path" and message "Value with length 513 at '/path' failed to satisfy constraint: Member must have length between 1 and 512, inclusive".
Emulator: fieldList null; message is "1 validation error detected: Value '<the full 513-character path>' at 'path' failed to satisfy constraint: Member must have length less than or equal to 512" — it inlines the whole rejected string instead of reporting its length, and gives only the upper bound.
Traces: devops-agent-delete-asset-file-path-validation#task#3 (cluster also covers task#4 of the same fixture).

=== Cause 2 of 4: DeleteAsset.assetId @pattern ^[a-zA-Z0-9_.-]+$ ===
AWS: Smithy: DeleteAsset input assetId -> ResourceId, @httpLabel, required, length {min: 1, max: 128}, pattern ^[a-zA-Z0-9_.-]+$. A value with disallowed characters is refused with field path "/assetId" and message "Value at '/assetId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$".
Emulator: fieldList null; message echoes the rejected id and drops the pointer slash.
Traces: devops-agent-delete-asset-identifier-validation#task#3 (cluster also covers task#4).

=== Cause 3 of 4: GetAsset.assetId @pattern ^[a-zA-Z0-9_.-]+$ ===
AWS: Smithy: GetAsset input assetId -> ResourceId, @httpLabel, required, pattern ^[a-zA-Z0-9_.-]+$; same envelope as cause 2 at path "/assetId".
Emulator: fieldList null; message echoes the rejected id and drops the pointer slash.
Traces: devops-agent-get-asset-id-pattern-and-version-validation#task#4 (cluster also covers task#7).

=== Cause 4 of 4: ListPendingMessages.executionId @pattern ^[a-zA-Z0-9_.-]+$ ===
AWS: Smithy: ListPendingMessages input executionId -> ResourceId, required, length {min: 1, max: 128}, pattern ^[a-zA-Z0-9_.-]+$ — a BODY member of POST /agents/agent-space/{agentSpaceId}/pendingMessages, and AWS still names it with a JSON pointer: path "/executionId", message "Value at '/executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$".
Emulator: fieldList null; message echoes the rejected executionId and names it 'executionId' without the slash.
Traces: devops-agent-list-pending-messages-unknown-execution#task#3.

## Evidence
Replay of simparity-wire-aws-v1. Status 400 and code ValidationException agree in all four causes; the body diverges.

Cause 1 — devops-agent-delete-asset-file-path-validation#task#3, DeleteAssetFile:
  expected $.fieldList = [{'path': '/path', 'message': "Value with length 513 at '/path' failed to satisfy constraint: Member must have length between 1 and 512, inclusive"}]; actual: None.
  expected $.message = "1 validation error detected. Value with length 513 at '/path' failed to satisfy constraint: Member must have length between 1 and 512, inclusive"; actual: "1 validation error detected: Value 'pppp…[513 bytes]' at 'path' failed to satisfy constraint: Member must have length less than or equal to 512" (the replay reports the full 513-byte value inlined in the message and truncates it for display).

Cause 2 — devops-agent-delete-asset-identifier-validation#task#3, DeleteAsset:
  expected $.fieldList = [{'path': '/assetId', 'message': "Value at '/assetId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"}]; actual: None.
  expected $.message = "1 validation error detected. Value at '/assetId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"; actual: "1 validation error detected: Value 'bad+asset$id' at 'assetId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$".

Cause 3 — devops-agent-get-asset-id-pattern-and-version-validation#task#4, GetAsset: same expected pair at /assetId; actual message "1 validation error detected: Value 'bad!id' at 'assetId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$", fieldList None.

Cause 4 — devops-agent-list-pending-messages-unknown-execution#task#3, ListPendingMessages:
  expected $.fieldList = [{'path': '/executionId', 'message': "Value at '/executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"}]; actual: None.
  expected $.message = "1 validation error detected. Value at '/executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$"; actual: "1 validation error detected: Value 'bad*execution' at 'executionId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9_.-]+$".

The specific rejected strings ('bad+asset$id', 'bad!id', 'bad*execution', the 513 'p' path) are this corpus's inputs; the requirement is the message template and the presence of fieldList.

## Steps to Reproduce
Boot the emulator from this checkout and replay the fixture steps named per cause. By hand:
1. DELETE /asset/agent-space/{agentSpaceId}/assets/{assetId}/files/<513-character path> — expect 400 with fieldList [{'path':'/path', ...}] and a message that reports the LENGTH (513), not the string.
2. DELETE /asset/agent-space/{agentSpaceId}/assets/bad+asset$id — expect fieldList [{'path':'/assetId', ...}] and a message with no value echoed.
3. GET /asset/agent-space/{agentSpaceId}/assets/bad!id — same.
4. POST /agents/agent-space/{agentSpaceId}/pendingMessages with body {"executionId": "bad*execution"} — expect fieldList [{'path':'/executionId', ...}].
In each case check that the response body contains `fieldList` and that `message` matches the expected text exactly.
