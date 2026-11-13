Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-reservation-basic-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-reservation-basic-lifecycle ec2-capacity-reservation-cancel-lifecycle ec2-capacity-reservation-lifecycle ec2-capacity-reservation-usage-and-groups ec2-capacity-reservation-cancellation-quote ec2-capacity-reservation-cancellation-quote-error ec2-capacity-reservation-cancellation-quote-errors ec2-capacity-reservation-split-and-cancellation-quote ec2-capacity-reservation-split-workflow
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeCapacityReservations|2xx->2xx|$.DescribeCapacityReservationsResponse.capacityReservationSet.item.unusedReservationBillingOwnerId
Issue-Key: ec2|DescribeCapacityReservations|2xx->2xx|$.DescribeCapacityReservationsResponse.capacityReservationSet.item.state
Issue-Key: ec2|CreateCapacityReservationCancellationQuote|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|CreateCapacityReservationCancellationQuote|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeCapacityReservations|2xx->2xx|$.DescribeCapacityReservationsResponse.capacityReservationSet.item.availableInstanceCount
Issue-Key: ec2|DescribeCapacityReservations|2xx->2xx|$.DescribeCapacityReservationsResponse.capacityReservationSet.item.totalInstanceCount
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 capacity reservations never reach cancelled, lose their instance counts and omit unusedReservationBillingOwnerId

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCapacityReservations

## Description
Six net-new causes in the capacity-reservation state and projection, all on HTTP 200/400 paths that answer with the wrong body.

Cause 1 of 6 - DescribeCapacityReservations omits UnusedReservationBillingOwnerId (fixture ec2-capacity-reservation-basic-lifecycle, step task#2). AWS: every described reservation carries the account to which unused capacity is billed. Smithy: CapacityReservation member UnusedReservationBillingOwnerId -> AccountID, xmlName unusedReservationBillingOwnerId, @length min 12 max 12, pattern ^[0-9]{12}$ - so AWS emits the owning 12-digit account id. Emulator: the element is absent from capacityReservationSet.item. Why: a client reading billing ownership gets null for a modeled, always-present member. Fix: the DescribeCapacityReservations projection/serializer and the reservation record in state.

Cause 2 of 6 - CancelCapacityReservation does not move the reservation to cancelled (ec2-capacity-reservation-cancel-lifecycle#task#4). AWS: Smithy CapacityReservation.State is the CapacityReservationState enum and documents 'cancelled - The Capacity Reservation was canceled. The reserved capacity is no longer available for your use.'; after a successful cancel a describe reports state cancelled. Emulator: the subsequent describe still reports state active. Why: every cancel-then-poll workflow never converges. Fix: the CancelCapacityReservation handler's state transition.

Cause 3 of 6 - the same non-transition observed after the cancellation-quote flow (ec2-capacity-reservation-cancellation-quote#task#5): describe reports active where AWS reports cancelled.

Cause 4 of 6 - CreateCapacityReservationCancellationQuote refuses with the wrong class for a reservation that has no commitment (ec2-capacity-reservation-basic-lifecycle#task#3). AWS: `InvalidParameterValue`, 'The Capacity Reservation contains no commitment terms. Quote is not needed for cancellation.' - fixed text, no ids interpolated (recorded behavior only; ec2 models no error shapes). Emulator: `IncorrectState` with its own prose naming the reservation id and state. Fix: the quote handler's precondition classification and message.

Cause 5 of 6 - instance counts are wrong after a reservation split (ec2-capacity-reservation-lifecycle#task#5). AWS: the source reservation keeps its totalInstanceCount/availableInstanceCount (Smithy: CapacityReservation.TotalInstanceCount and AvailableInstanceCount, both Integer) consistent with the split that was performed. Emulator: reports half the expected counts (e.g. 2 where the recording has 4) and also omits unusedReservationBillingOwnerId. Why: capacity accounting diverges, so subsequent allocations are refused or admitted wrongly. Fix: the split/move handler's count arithmetic in state.

Cause 6 of 6 - the same wrong counts plus the missing cancelled state at the end of the lifecycle (ec2-capacity-reservation-lifecycle#task#7): expected state cancelled with totalInstanceCount/availableInstanceCount 4, emulator reports active with 2 and no unusedReservationBillingOwnerId. This cluster also covers ec2-capacity-reservation-split-and-cancellation-quote#teardown#6 and ec2-capacity-reservation-split-workflow#task#9.

Fix summary: the capacity-reservation state machine (cancel, split) and the DescribeCapacityReservations projection under services/service-com-amazonaws-ec2-amazonec2/src/ops/, plus the reservation record in the crate's state module.

Traces: representative step plus the five fixture#step bindings in trace_ids.

## Evidence
Replay detail, verbatim:
1. DescribeCapacityReservations (200/200): $...capacityReservationSet.item.unusedReservationBillingOwnerId missing (expected '000000000000', got None); cluster also covers ec2-capacity-reservation-cancel-lifecycle#task#2, ec2-capacity-reservation-lifecycle#task#3, ec2-capacity-reservation-usage-and-groups#task#2.
2. DescribeCapacityReservations (200/200): item.state value_mismatch (expected 'cancelled', got 'active') plus the missing unusedReservationBillingOwnerId.
3. DescribeCapacityReservations (200/200): item.state value_mismatch (expected 'cancelled', got 'active').
4. CreateCapacityReservationCancellationQuote (400/400): expected code 'InvalidParameterValue' / 'The Capacity Reservation contains no commitment terms. Quote is not needed for cancellation.'; actual 'IncorrectState' / "The Capacity Reservation 'cr-00000000000000022' is in state 'active' with no commitment duration, so cancelling it needs no cancellation quote" - cluster spans 7 steps across ec2-capacity-reservation-cancellation-quote*, ec2-capacity-reservation-split-and-cancellation-quote.
5. DescribeCapacityReservations (200/200): availableInstanceCount expected '4' got '2'; totalInstanceCount expected '4' got '2'; unusedReservationBillingOwnerId missing.
6. DescribeCapacityReservations (200/200): state expected 'cancelled' got 'active'; availableInstanceCount/totalInstanceCount expected '4' got '2'; unusedReservationBillingOwnerId missing.
All six clusters: verdict=mismatch, stub_path empty, owned_by empty.

## Steps to Reproduce
1. Clone, boot the emulator built from the checkout.
2. replay_wire_corpus on simparity-wire-aws-v1.
3. Replay fixture ec2-capacity-reservation-basic-lifecycle step task#2 and each fixture#step in trace_ids: create a reservation, cancel or split it, then DescribeCapacityReservations and compare state, totalInstanceCount, availableInstanceCount and unusedReservationBillingOwnerId with the recording.
