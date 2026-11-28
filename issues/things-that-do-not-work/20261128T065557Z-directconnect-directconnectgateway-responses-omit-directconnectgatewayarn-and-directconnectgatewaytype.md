Issue-Source: trace-sweep-divergence
Issue-Trace: direct-connect-gateway-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: direct-connect-gateway-lifecycle
Issue-Shape-Id: com.amazonaws.directconnect#OvertureService
Issue-Key: directconnect|CreateDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayArn
Issue-Key: directconnect|CreateDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayType
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->2xx|$.directConnectGateways[*].directConnectGatewayArn
Issue-Key: directconnect|DescribeDirectConnectGateways|2xx->2xx|$.directConnectGateways[*].directConnectGatewayType
Issue-Key: directconnect|UpdateDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayArn
Issue-Key: directconnect|UpdateDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayType
Issue-Key: directconnect|DeleteDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayArn
Issue-Key: directconnect|DeleteDirectConnectGateway|2xx->2xx|$.directConnectGateway.directConnectGatewayType
Issue-Commit: ddcc4b972bc61c5877b60e681757a414edac67ab

# directconnect: DirectConnectGateway responses omit directConnectGatewayArn and directConnectGatewayType

> _Note: this issue was identified against the code base at commit `ddcc4b972bc61c5877b60e681757a414edac67ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDirectConnectGateway

## Description
Every operation in the Direct Connect gateway lifecycle serializes a `DirectConnectGateway` structure that is missing two members AWS always populates: `directConnectGatewayArn` and `directConnectGatewayType`. The emulator returns HTTP 200 with an otherwise well-shaped body, so the omission is silent. This is ONE shared serialization fault in the `DirectConnectGateway` shape writer, reached through four separate operations; a fixer must close all FOUR causes below.

Note on the model: the checked-in Smithy model (services/service-com-amazonaws-directconnect-overtureservice) defines `DirectConnectGateway` with only directConnectGatewayId, directConnectGatewayName, amazonSideAsn, ownerAccount, directConnectGatewayState, stateChangeError, totalPrefixPoolAllocations and tags — it has NO directConnectGatewayArn or directConnectGatewayType member, and there is no `DirectConnectGatewayType` shape at all. So for these two members the requirement is RECORDED BEHAVIOR ONLY: the fixtures show live AWS emitting both on every DirectConnectGateway it returns, which means the bundled model lags the wire and the emulator's serializer must be widened past it rather than treated as complete.

Shape of the two members, as the recordings show them (general form, not one run's literals):
- `directConnectGatewayArn`: a string ARN `arn:aws:directconnect::<ownerAccount>:dx-gateway/<directConnectGatewayId>`. Fixed text: `arn:aws:directconnect::`, `:dx-gateway/`. Variable parts taken from state: the partition, the EMPTY region field (a Direct Connect gateway is a global resource, so the region segment carries no value), the owning account id, and the gateway's own id. The concrete `arn:aws:directconnect::000000000000:dx-gateway/a8c7f732-281a-4812-a381-000000000001` in the fixture is one run filling that template — the account id and the gateway UUID are state, never literals to hardcode.
- `directConnectGatewayType`: an enum-valued string; the recordings show `Default` for a gateway created by CreateDirectConnectGateway without a type override. `Default` is the fixed value for this class of gateway, not a per-run value.

Both members must be emitted consistently by whatever renders a DirectConnectGateway, so that a gateway's ARN and type are stable across its create/describe/update/delete views and the ARN keeps matching the id in the same payload.

Cause 1 of 4: CreateDirectConnectGateway (fixture `direct-connect-gateway-lifecycle`, step `task#1`).
AWS: recorded behavior only — the recorded 200 response carries `$.directConnectGateway.directConnectGatewayArn` (shaped ARN as above) and `$.directConnectGateway.directConnectGatewayType` = `Default`. The Smithy model declares neither member.
Emulator: returns 200 but both keys are absent from `$.directConnectGateway` (got None for each).
Why: a client that reads the ARN off the create response to tag the gateway, attach a policy, or hand it to another service (RAM, CloudFormation, Resource Groups) gets a null/KeyError at the moment of creation and has no ARN to carry forward; a client that branches on gateway type cannot tell a Default gateway from any other.
Fix: the CreateDirectConnectGateway response serializer, via the shared DirectConnectGateway shape writer, plus the state record so the ARN and type are stored at create time rather than synthesized per call.
Traces: direct-connect-gateway-lifecycle#task#1.

Cause 2 of 4: DescribeDirectConnectGateways (fixture `direct-connect-gateway-lifecycle`, step `task#2`; same cluster also covers steps task#8 and task#10).
AWS: recorded behavior only — each element of the 200 response's `$.directConnectGateways` list carries `directConnectGatewayArn` and `directConnectGatewayType` = `Default`.
Emulator: returns 200 with the list populated, but `$.directConnectGateways[0].directConnectGatewayArn` and `$.directConnectGateways[0].directConnectGatewayType` are absent (got None). The gap persists across the whole lifecycle — the same two keys are still missing on the later describes after update and after delete.
Why: Describe is the discovery path, so a client enumerating gateways to build an inventory or to resolve an id to an ARN gets neither; it cannot cross-reference gateways with resources identified by ARN, and the omission is invisible because the call succeeds.
Fix: the DescribeDirectConnectGateways list-element serializer, through the same shared DirectConnectGateway shape writer.
Traces: direct-connect-gateway-lifecycle#task#2, direct-connect-gateway-lifecycle#task#8, direct-connect-gateway-lifecycle#task#10.

Cause 3 of 4: UpdateDirectConnectGateway (fixture `direct-connect-gateway-lifecycle`, step `task#7`).
AWS: recorded behavior only — the recorded 200 response echoes the updated gateway in `$.directConnectGateway` WITH `directConnectGatewayArn` and `directConnectGatewayType` = `Default`; renaming a gateway does not change either value, since the ARN is keyed on the gateway id and not its name.
Emulator: returns 200 with the renamed gateway but both keys absent (got None).
Why: a client that uses the update response as the authoritative post-change view of the gateway loses the ARN and type, and cannot confirm that a rename left the gateway's identity untouched.
Fix: the UpdateDirectConnectGateway response serializer, through the shared DirectConnectGateway shape writer; the ARN must be re-emitted from the stored gateway id, not rebuilt from the new name.
Traces: direct-connect-gateway-lifecycle#task#7.

Cause 4 of 4: DeleteDirectConnectGateway (fixture `direct-connect-gateway-lifecycle`, step `task#9`).
AWS: recorded behavior only — the recorded 200 response returns the gateway in its `deleting` state in `$.directConnectGateway`, still carrying `directConnectGatewayArn` and `directConnectGatewayType` = `Default`; a gateway entering deletion keeps its ARN and type.
Emulator: returns 200 with the deleting gateway but both keys absent (got None).
Why: a client that records the ARN of what it just deleted — for audit, for cleanup bookkeeping, or to poll the resource until it disappears — has nothing to record.
Fix: the DeleteDirectConnectGateway response serializer, through the shared DirectConnectGateway shape writer.
Traces: direct-connect-gateway-lifecycle#task#9.

Related but NOT covering this: the open issue `20261127T015317Z-directconnect-describelocations-publishes-a-wrong-us-west-2.md` owns a separate DescribeLocations catalog divergence in this service and says nothing about the DirectConnectGateway shape.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture `direct-connect-gateway-lifecycle` (4 divergence clusters, 6 affected steps, all verdict=mismatch, expected_status 200 -> actual_status 200):

1) CreateDirectConnectGateway, step task#1:
   $.directConnectGateway.directConnectGatewayArn: missing (expected '<shaped-not-asserted>', got None)
   $.directConnectGateway.directConnectGatewayType: missing (expected 'Default', got None)

2) DescribeDirectConnectGateways, step task#2 (also task#8, task#10):
   $.directConnectGateways[0].directConnectGatewayArn: missing (expected 'arn:aws:directconnect::<account>:dx-gateway/<gatewayId>' form, got None)
   $.directConnectGateways[0].directConnectGatewayType: missing (expected 'Default', got None)

3) UpdateDirectConnectGateway, step task#7:
   $.directConnectGateway.directConnectGatewayArn: missing (expected ARN form above, got None)
   $.directConnectGateway.directConnectGatewayType: missing (expected 'Default', got None)

4) DeleteDirectConnectGateway, step task#9:
   $.directConnectGateway.directConnectGatewayArn: missing (expected ARN form above, got None)
   $.directConnectGateway.directConnectGatewayType: missing (expected 'Default', got None)

Expected (AWS, from the recordings): every DirectConnectGateway rendering includes directConnectGatewayArn = arn:aws:directconnect::<ownerAccount>:dx-gateway/<directConnectGatewayId> (empty region segment) and directConnectGatewayType = Default.
Actual (emulator): both members omitted entirely from every DirectConnectGateway rendering, on all four operations, with a 200 status and no error.

Model evidence: smithy_query shape=DirectConnectGateway on services/service-com-amazonaws-directconnect-overtureservice/smithy-model/com-amazonaws-directconnect-overtureservice.json lists members directConnectGatewayId, directConnectGatewayName, amazonSideAsn, ownerAccount, directConnectGatewayState, stateChangeError, totalPrefixPoolAllocations, tags — no directConnectGatewayArn, no directConnectGatewayType; shape 'DirectConnectGatewayType' does not exist in the model. The requirement therefore rests on the recordings, and the bundled model is behind the wire.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from that checkout.
2. Replay the wire fixture `direct-connect-gateway-lifecycle` from trace set simparity-wire-aws-v1.
3. Cause 1 — step task#1: call CreateDirectConnectGateway with a gateway name and amazonSideAsn. Response is 200; inspect $.directConnectGateway and observe directConnectGatewayArn and directConnectGatewayType are both absent.
4. Cause 2 — step task#2 (repeat at task#8 and task#10): call DescribeDirectConnectGateways. Response is 200; inspect $.directConnectGateways[0] and observe both members absent.
5. Cause 3 — step task#7: call UpdateDirectConnectGateway with the gateway id and a new name. Response is 200; inspect $.directConnectGateway and observe both members absent.
6. Cause 4 — step task#9: call DeleteDirectConnectGateway with the gateway id. Response is 200 with state 'deleting'; inspect $.directConnectGateway and observe both members absent.
7. In each case AWS returns directConnectGatewayArn = arn:aws:directconnect::<ownerAccount>:dx-gateway/<directConnectGatewayId> and directConnectGatewayType = 'Default'. Re-replay all four steps (task#1, task#2, task#7, task#9, plus task#8 and task#10) to confirm a fix.
