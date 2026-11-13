Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-reservation-fleet-cancel-lifecycle
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-reservation-fleet-cancel-lifecycle ec2-capacity-reservation-fleet-lifecycle ec2-describe-capacity-reservation-fleets-contract ec2-verified-access-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CancelCapacityReservationFleets|2xx->2xx|$.CancelCapacityReservationFleetsResponse.successfulFleetCancellationSet.item.previousFleetState
Issue-Key: ec2|DetachVerifiedAccessTrustProvider|2xx->2xx|$.DetachVerifiedAccessTrustProviderResponse.verifiedAccessInstance.verifiedAccessInstanceTransitGatewayAttachmentSet
Issue-Commit: 3778ba40189b37afffee3a954bf30487a96c0bc7

# EC2 cancel-fleet and detach-trust-provider responses report a wrong previous state and omit an always-present set

> _Note: this issue was identified against the code base at commit `3778ba40189b37afffee3a954bf30487a96c0bc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelCapacityReservationFleets

## Description
Two net-new causes where an EC2 lifecycle write answers 200 but the returned resource representation does not match AWS: one reports a state the resource never held, the other drops a collection AWS always emits.

Cause 1 of 2: CancelCapacityReservationFleets reports previousFleetState = 'active' for a fleet that was still 'submitted' (fixture ec2-capacity-reservation-fleet-cancel-lifecycle, step task#3).
AWS: Smithy: CancelCapacityReservationFleets returns SuccessfulFleetCancellations (xmlName successfulFleetCancellationSet) of CapacityReservationFleetCancellationState, whose members are currentFleetState and previousFleetState — both the enum CapacityReservationFleetState with values submitted, modifying, active, partially_fulfilled, expiring, expired, cancelling, cancelled, failed — plus capacityReservationFleetId. previousFleetState is documented as "The previous state of the Capacity Reservation Fleet", i.e. the state the fleet actually held when the cancel request arrived. The recording shows a fleet cancelled shortly after creation reporting previousFleetState 'submitted': AWS leaves a newly created fleet in submitted until its capacity is fulfilled.
Emulator: reports previousFleetState 'active' for the same cancel.
Why: a client that reads the pre-cancel state, or that waits for the submitted -> active transition before acting, is told the fleet passed through a state it never occupied; the same wrong state is observable on the read path, since the cluster also covers a DescribeCapacityReservationFleets contract step and a second fleet lifecycle fixture.
Hypothesis: the fleet record is created directly in 'active' (or previousFleetState is filled from the fleet's current/default state at cancel time) instead of being created 'submitted' and carrying the state held at the moment the cancel arrived.
Fix: the capacity-reservation-fleet record in the emulator's state plus the CancelCapacityReservationFleets handler — create the fleet in submitted, keep its real state, and copy the state held at cancel time into previousFleetState (with currentFleetState set to the post-cancel state).
Traces: ec2-capacity-reservation-fleet-cancel-lifecycle#task#3; cluster members ec2-capacity-reservation-fleet-lifecycle#task#6 and ec2-describe-capacity-reservation-fleets-contract#task#5.

Cause 2 of 2: DetachVerifiedAccessTrustProvider omits the returned instance's transit-gateway attachment set (fixture ec2-verified-access-delete-lifecycle, step task#11).
AWS: Smithy: DetachVerifiedAccessTrustProvider returns VerifiedAccessTrustProvider and VerifiedAccessInstance (xmlName verifiedAccessInstance). The on-disk VerifiedAccessInstance shape declares verifiedAccessInstanceId, description, verifiedAccessTrustProviderSet, creationTime, lastUpdatedTime, tagSet, fipsEnabled and cidrEndpointsCustomSubDomain only — it does not declare the transit-gateway attachment set, so this member is recorded behavior only: AWS's wire response carries verifiedAccessInstanceTransitGatewayAttachmentSet on the returned instance, present and empty when the instance has no transit-gateway attachments.
Emulator: omits verifiedAccessInstanceTransitGatewayAttachmentSet entirely from verifiedAccessInstance.
Why: an element AWS always emits goes missing, so a client that walks the attachment set to decide whether an instance still has transit-gateway attachments (a normal pre-delete check) reads a missing node rather than an empty collection and has to special-case the emulator.
Fix: the DetachVerifiedAccessTrustProvider response serializer / the shared VerifiedAccessInstance projection — always emit the transit-gateway attachment set element, empty when the instance has no attachments.
Traces: ec2-verified-access-delete-lifecycle#task#11.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 — fixture ec2-capacity-reservation-fleet-cancel-lifecycle, step task#3, operation CancelCapacityReservationFleets (status 200 both sides):
  $.CancelCapacityReservationFleetsResponse.successfulFleetCancellationSet.item.previousFleetState: value_mismatch (expected 'submitted', got 'active')
  cluster also affects ec2-capacity-reservation-fleet-lifecycle#task#6 and ec2-describe-capacity-reservation-fleets-contract#task#5 (affected_count 3).

Cause 2 — fixture ec2-verified-access-delete-lifecycle, step task#11, operation DetachVerifiedAccessTrustProvider (status 200 both sides):
  $.DetachVerifiedAccessTrustProviderResponse.verifiedAccessInstance.verifiedAccessInstanceTransitGatewayAttachmentSet: missing (expected '' (present and empty), got None)

Model evidence: smithy_query operation=CancelCapacityReservationFleets,DetachVerifiedAccessTrustProvider and shape=CapacityReservationFleetCancellationState,CapacityReservationFleetState,VerifiedAccessInstance on services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json.

Neither cluster is owned by an open issue and neither is blocked by a stub (owned_by = [], stub_path = "" in the replay report).

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it.
2. Replay ec2-capacity-reservation-fleet-cancel-lifecycle. At step task#3 CancelCapacityReservationFleets is called on a fleet created earlier in the same fixture: expect successfulFleetCancellationSet.item.previousFleetState 'submitted'; observe 'active'. The same wrong state shows up in ec2-capacity-reservation-fleet-lifecycle#task#6 and ec2-describe-capacity-reservation-fleets-contract#task#5.
3. Replay ec2-verified-access-delete-lifecycle. At step task#11 DetachVerifiedAccessTrustProvider returns the Verified Access instance: expect verifiedAccessInstance to carry an empty verifiedAccessInstanceTransitGatewayAttachmentSet element; observe the element absent.
