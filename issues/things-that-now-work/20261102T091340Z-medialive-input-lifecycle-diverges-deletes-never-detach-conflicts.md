Issue-Source: trace-sweep-divergence
Issue-Trace: medialive-cmc-traj-attached-input-conflicts-and-delete-order
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: medialive-cmc-traj-attached-input-conflicts-and-delete-order medialive-cmc-traj-role-mistakes-and-idempotent-retry
Issue-Shape-Id: com.amazonaws.medialive#MediaLive
Issue-Key: medialive|DeleteInputSecurityGroup|4xx->2xx|$.message
Issue-Key: medialive|DeleteInputSecurityGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: medialive|DeleteInputSecurityGroup|4xx->2xx|status
Issue-Key: medialive|DeleteInput|2xx->4xx|$.arn
Issue-Key: medialive|DeleteInput|2xx->4xx|$.attachedChannels
Issue-Key: medialive|DeleteInput|2xx->4xx|$.destinations
Issue-Key: medialive|DeleteInput|2xx->4xx|$.id
Issue-Key: medialive|DeleteInput|2xx->4xx|$.inputClass
Issue-Key: medialive|DeleteInput|2xx->4xx|$.inputDevices
Issue-Key: medialive|DeleteInput|2xx->4xx|$.inputNetworkLocation
Issue-Key: medialive|DeleteInput|2xx->4xx|$.inputPartnerIds
Issue-Key: medialive|DeleteInput|2xx->4xx|$.inputSourceType
Issue-Key: medialive|DeleteInput|2xx->4xx|$.mediaConnectFlows
Issue-Key: medialive|DeleteInput|2xx->4xx|$.name
Issue-Key: medialive|DeleteInput|2xx->4xx|$.securityGroups
Issue-Key: medialive|DeleteInput|2xx->4xx|$.smpte2110ReceiverGroupSettings
Issue-Key: medialive|DeleteInput|2xx->4xx|$.sources
Issue-Key: medialive|DeleteInput|2xx->4xx|$.state
Issue-Key: medialive|DeleteInput|2xx->4xx|$.tags
Issue-Key: medialive|DeleteInput|2xx->4xx|$.type
Issue-Key: medialive|DeleteInput|2xx->4xx|status
Issue-Key: medialive|DescribeInput|2xx->2xx|$
Issue-Key: medialive|DescribeInput|2xx->2xx|$.attachedChannels[*]
Issue-Key: medialive|DescribeInput|2xx->2xx|$.destinations[*]
Issue-Key: medialive|DescribeInput|2xx->2xx|$.state
Issue-Related: 20261102T091340Z-medialive-request-validation-missing-or-misworded-across-channel.md
Issue-Commit: 54f388b9499ec3dd7b64bc533fb6b62161b4e3d7

# medialive input lifecycle diverges: deletes never detach, conflicts not raised

> _Note: this issue was identified against the code base at commit `54f388b9499ec3dd7b64bc533fb6b62161b4e3d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteInput

## Description
MediaLive's channel/input/security-group attachment state is maintained incorrectly: the conflicts AWS raises while a resource is in use are not raised, deleting a channel never detaches the inputs it held, and so an input can never reach DELETED and its security group never returns to IDLE. This bundle has 6 causes; all six must be closed.

Cause 1 of 6 - CreateChannel accepts an input that is already attached to another channel (fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step task#8).
AWS: an input of inputClass STANDARD may be attached to one channel at a time. CreateChannel (Smithy: POST /prod/channels, errors include ConflictException httpError 409) refuses a request whose inputAttachments name an already-attached input with 409 ConflictException and the message "inputId=<input id> is already attached" - fixed text "inputId=" and " is already attached" around the id taken from the request.
Emulator: 201, a second channel is created and attached to the same input; no message, no x-amzn-errortype.
Why: a client relying on the 409 to detect that it is double-booking an input creates a configuration real MediaLive forbids, and the extra attachment then blocks every later teardown (causes 3-6).
Fix: CreateChannel's attachment validation, against the input's attachedChannels in state.
Traces: medialive-cmc-traj-attached-input-conflicts-and-delete-order#task#8.

Cause 2 of 6 - DeleteInputSecurityGroup accepts a group that inputs still reference (fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step task#10).
AWS: deleting an input security group that inputs still use is refused 409 with the message "Input Security Group <group id> cannot be deleted because it is still being used by inputs." - fixed text around the group id from the request path (Smithy: DELETE /prod/inputSecurityGroups/{InputSecurityGroupId}).
Emulator: 200, the delete is accepted; no message, no x-amzn-errortype.
Why: a client that deletes in the wrong order gets no warning and loses the group out from under live inputs, where AWS would have refused and told it to delete the inputs first.
Fix: DeleteInputSecurityGroup's precondition check against the group's inputs list in state.
Traces: medialive-cmc-traj-attached-input-conflicts-and-delete-order#task#10.

Cause 3 of 6 - DeleteInput refuses with 409 an input whose only channel has already been deleted (fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step task#13).
AWS: once the channel holding an input is deleted, the input is detached, and DeleteInput (Smithy: DELETE /prod/inputs/{InputId}, code 200) succeeds with 200 and returns the input projection with state "DELETING" (Smithy: InputState enum CREATING|DETACHED|ATTACHED|DELETING|DELETED) and an empty attachedChannels list.
Emulator: 409 with an empty body - every modeled output member (arn, id, name, type, sources, destinations, state, attachedChannels, securityGroups, inputClass, inputPartnerIds, inputNetworkLocation, smpte2110ReceiverGroupSettings, mediaConnectFlows, inputDevices, inputSourceType, tags) is missing - because the deleted channel is still recorded as attached.
Why: the ordinary teardown order (delete channel, then input, then security group) is impossible against the emulator: the input delete is refused forever.
Fix: the DeleteChannel handler/state - deleting a channel must remove it from every input's attachedChannels and return the input to DETACHED - and DeleteInput's conflict check, which must only fire for live attachments.
Traces: medialive-cmc-traj-attached-input-conflicts-and-delete-order#task#13 (cluster also covers medialive-cmc-traj-role-mistakes-and-idempotent-retry#teardown#15).

Cause 4 of 6 - a deleted input never leaves ATTACHED, so DescribeInput never reports DELETED (fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order, step task#15).
AWS: after an accepted DeleteInput, DescribeInput (GET /prod/inputs/{InputId}) reports state "DELETED" with destinations and attachedChannels emptied; a client polls DescribeInput until state is DELETED.
Emulator: DescribeInput keeps answering 200 with state "ATTACHED", the original destinations still present and the deleted channel still listed in attachedChannels, so the poll exhausts (replay: "$.state: poll: not in ['DELETED'] after 13 attempt(s) (expected 'DELETED', got 'ATTACHED')").
Why: any client that waits for DELETED hangs until its own timeout and can never confirm teardown; the resource is also never released, so the quota/state view stays wrong.
Fix: the input state machine in the medialive state module - DeleteInput must transition DELETING -> DELETED and clear destinations/attachedChannels, and channel deletion must detach (cause 3).
Traces: medialive-cmc-traj-attached-input-conflicts-and-delete-order#task#15.

Cause 5 of 6 - CreateChannel retried with the same requestId mints additional channels, so an input accumulates attachments (fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry, step task#12).
AWS: requestId is CreateChannel's idempotency token (Smithy: CreateChannel input member RequestId jsonName "requestId", idempotencyToken: true), so a repeated create with the same token does not create another channel; the input stays attached to exactly one channel and DescribeInput reports a single-element attachedChannels list.
Emulator: DescribeInput returns extra attachedChannels entries (replay: "$.attachedChannels[1]: extra (expected None, got '1000001')", "$.attachedChannels[2]: extra (expected None, got '1000002')") - the retried create and the creates AWS refused each attached another channel to the same input.
Why: a client that retries a create after a timeout silently ends up with duplicate channels billed and attached, and its input's attachment list no longer matches AWS.
Fix: CreateChannel's handler/state - honour requestId as an idempotency key (return the existing channel for a repeated token) and keep the input's attachedChannels authoritative.
Traces: medialive-cmc-traj-role-mistakes-and-idempotent-retry#task#12.

Cause 6 of 6 - the same never-detached input blocks teardown in the second fixture (fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry, step teardown#16).
AWS: after its channels are deleted and DeleteInput is accepted, DescribeInput reports state "DELETED" with destinations and attachedChannels empty.
Emulator: state stays "ATTACHED" with both destinations still present and two channel ids still in attachedChannels; the poll fails ("$.state: poll: not in ['DELETED'] after 10 attempt(s) (expected 'DELETED', got 'ATTACHED')").
Why: as cause 4 - teardown cannot complete, and because the input is still in use the security group also stays IN_USE instead of IDLE.
Fix: as causes 3 and 4 - channel deletion must detach and input deletion must reach DELETED.
Traces: medialive-cmc-traj-role-mistakes-and-idempotent-retry#teardown#16.

Related open issues (neither covers these causes): 20261029T201850Z-medialive-createpartnerinput-accepts-a-non-cdi-input-and-the.md owns the partner-input variant of the stuck teardown, and 20261029T201850Z-medialive-not-found-refusals-carry-emulator-authored-resource.md owns the 404 message wording.

## Evidence
Expected vs actual, from the replay of simparity-wire-aws-v1 (all clusters verdict=mismatch, service=medialive):

1. CreateChannel, medialive-cmc-traj-attached-input-conflicts-and-delete-order#task#8: status expected 409, got 201; $.message missing (expected 'inputId=<input id> is already attached'); $headers.x-amzn-errortype missing.
2. DeleteInputSecurityGroup, ...#task#10: status expected 409, got 200; $.message missing (expected 'Input Security Group <group id> cannot be deleted because it is still being used by inputs.'); x-amzn-errortype missing.
3. DeleteInput, ...#task#13: status expected 200, got 409; the whole modeled body is missing - $.arn, $.id, $.name, $.type, $.sources, $.smpte2110ReceiverGroupSettings, $.inputSourceType, $.destinations, $.mediaConnectFlows, $.inputDevices, $.state (expected 'DELETING'), $.attachedChannels (expected []), $.securityGroups, $.tags, $.inputClass, $.inputPartnerIds, $.inputNetworkLocation.
4. DescribeInput, ...#task#15: 200 vs 200 but $.state value_mismatch (expected 'DELETED', got 'ATTACHED'); $.destinations[0] and [1] extra (expected None); $.attachedChannels[0] extra (expected None); '$.state: poll: not in [DELETED] after 13 attempt(s)'.
5. DescribeInput, medialive-cmc-traj-role-mistakes-and-idempotent-retry#task#12: $.attachedChannels[1] extra (expected None, got a second channel id); $.attachedChannels[2] extra (expected None, got a third).
6. DescribeInput, ...#teardown#16: $.state value_mismatch (expected 'DELETED', got 'ATTACHED'); $.destinations[0], [1], $.attachedChannels[0], [1] all extra; '$.state: poll: not in [DELETED] after 10 attempt(s)'.

Model references: CreateChannel POST /prod/channels errors include ConflictException httpError 409 and input member RequestId (idempotencyToken: true); DeleteInput DELETE /prod/inputs/{InputId} code 200 with ConflictException 409 modeled; DeleteInputSecurityGroup DELETE /prod/inputSecurityGroups/{InputSecurityGroupId} code 200; DescribeInput output State -> InputState enum [CREATING, DETACHED, ATTACHED, DELETING, DELETED]; ConflictException{message} httpError 409.

The recorded ids (input 1000000, channels 1000001/1000002, security group 3000000) and the 198.18.0.x destination URLs fill the variable slots of these messages and of the state snapshots; they are per-run values, not requirements.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-wire-aws-v1 (replay_wire_corpus).
2. Fixture medialive-cmc-traj-attached-input-conflicts-and-delete-order: create an input security group and an RTMP_PUSH input in it, create a channel attaching that input, then POST /prod/channels again attaching the SAME input -> expect 409 'inputId=<id> is already attached', observe 201 (step task#8).
3. DELETE /prod/inputSecurityGroups/{id} while the input still references the group -> expect 409 'Input Security Group <id> cannot be deleted because it is still being used by inputs.', observe 200 (step task#10).
4. DELETE the channel, then DELETE /prod/inputs/{id} -> expect 200 with state 'DELETING' and attachedChannels [], observe 409 with an empty body (step task#13).
5. Poll GET /prod/inputs/{id} -> expect state to reach 'DELETED', observe 'ATTACHED' with the deleted channel still in attachedChannels until the 60s poll cap (step task#15).
6. Fixture medialive-cmc-traj-role-mistakes-and-idempotent-retry: POST /prod/channels twice with the same requestId, then GET /prod/inputs/{id} -> expect one entry in attachedChannels, observe three (step task#12).
7. Same fixture teardown: delete the channels, delete the input, then poll GET /prod/inputs/{id} -> expect 'DELETED', observe 'ATTACHED' with destinations and attachedChannels still populated (step teardown#16).
