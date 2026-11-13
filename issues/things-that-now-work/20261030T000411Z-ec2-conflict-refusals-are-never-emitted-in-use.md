Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-elastic-ip-association
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-elastic-ip-association ec2-conflict-flow-log-duplicate ec2-conflict-volume-attachment ec2-t12-eip-disassociate-and-domain-attribute
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|ReleaseAddress|4xx->2xx|$.Response
Issue-Key: ec2|ReleaseAddress|4xx->2xx|status
Issue-Key: ec2|CreateFlowLogs|4xx->2xx|$.Response
Issue-Key: ec2|CreateFlowLogs|4xx->2xx|status
Issue-Key: ec2|DeleteVolume|4xx->2xx|$.Response
Issue-Key: ec2|DeleteVolume|4xx->2xx|status
Issue-Key: ec2|ResetAddressAttribute|4xx->2xx|$.Response
Issue-Key: ec2|ResetAddressAttribute|4xx->2xx|status
Issue-Commit: 7c2160283d3342f6e10783a5c4b2d926b7b7bab3

# EC2 conflict refusals are never emitted: in-use and duplicate states answer 200

> _Note: this issue was identified against the code base at commit `7c2160283d3342f6e10783a5c4b2d926b7b7bab3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReleaseAddress

## Description
The emulator does not check the conflicting-state preconditions that AWS enforces before mutating: when a resource is in use, already exists, or already has a pending request, AWS refuses the call with HTTP 400 and a specific error code, while the emulator performs the mutation and answers 200. This bundle carries 4 causes.

Cause 1 of 4: ReleaseAddress on an Elastic IP that is currently associated.
AWS: refuses with HTTP 400 and error code InvalidIPAddress.InUse. Message template: fixed text "Address " + the public IP taken from the request/state + fixed text " is in use." (recorded: "Address 54.0.0.2 is in use.").
Emulator: answers HTTP 200 with no Response/Errors body and releases the address.
Why: a client relying on the refusal to detect that it must disassociate first silently loses an address that is still serving traffic, and never sees the conflict.
Fix: the association precondition check in the ReleaseAddress handler's validation (services/service-com-amazonaws-ec2-amazonec2/src/ops/release_address.rs) against the address-association state.
Traces: ec2-conflict-elastic-ip-association#task#9.

Cause 2 of 4: CreateFlowLogs for a duplicate flow log (same configuration and log destination).
AWS: refuses with HTTP 400 and error code FlowLogAlreadyExists; message is the fixed text "Error. There is an existing Flow Log with the same configuration and log destination." (no variable parts).
Emulator: answers HTTP 200 and creates a second flow log.
Why: duplicate-detection logic in clients is defeated and the account accumulates duplicate flow logs the real service would have rejected.
Fix: duplicate-configuration detection in the CreateFlowLogs handler before it inserts into flow-log state (Smithy: CreateFlowLogs declares no operation errors, so the code is service-common; recorded behavior supplies the code and message).
Traces: ec2-conflict-flow-log-duplicate#task#4.

Cause 3 of 4: DeleteVolume on a volume that is attached to an instance.
AWS: refuses with HTTP 400 and error code VolumeInUse. Message template: fixed text "Volume " + the vol-* id from the request + fixed text " is currently attached to " + the instance id from state (recorded: "Volume vol-0000000000000002b is currently attached to <instance-id>").
Emulator: answers HTTP 200 and deletes the attached volume.
Why: a client that expects VolumeInUse to gate teardown ordering destroys a volume still attached, and loses the signal that a detach is required.
Fix: the attachment precondition in the DeleteVolume handler's validation, read from volume-attachment state.
Traces: ec2-conflict-volume-attachment#task#11.

Cause 4 of 4: ResetAddressAttribute while a pending reverse-DNS request exists.
AWS: refuses with HTTP 400 and error code PendingRequestAlreadyExists; message is the fixed text "A pending request for this record already exists." (no variable parts).
Emulator: answers HTTP 200 and accepts the second request.
Why: a client cannot discover that its earlier ModifyAddressAttribute request is still pending and double-submits.
Fix: pending-request bookkeeping for address attributes, checked in the ResetAddressAttribute handler. Related but not covering: 20261029T200817Z-ec2-elastic-ip-and-application-status-check-state.md (that issue owns other elastic-IP state steps, not this refusal).
Traces: ec2-t12-eip-disassociate-and-domain-attribute#task#14.

## Evidence
Cause 1 - expected 400 {Code: InvalidIPAddress.InUse, Message: 'Address <public-ip> is in use.'}, actual 200 with $.Response missing.
Cause 2 - expected 400 {Code: FlowLogAlreadyExists, Message: 'Error. There is an existing Flow Log with the same configuration and log destination.'}, actual 200.
Cause 3 - expected 400 {Code: VolumeInUse, Message: 'Volume <volume-id> is currently attached to <instance-id>'}, actual 200.
Cause 4 - expected 400 {Code: PendingRequestAlreadyExists, Message: 'A pending request for this record already exists.'}, actual 200.

## Steps to Reproduce
Replay simparity-wire-aws-v1. ec2-conflict-elastic-ip-association#task#9 releases an associated EIP; ec2-conflict-flow-log-duplicate#task#4 re-creates an identical flow log; ec2-conflict-volume-attachment#task#11 deletes an attached volume; ec2-t12-eip-disassociate-and-domain-attribute#task#14 resets an address attribute with a pending request outstanding.
