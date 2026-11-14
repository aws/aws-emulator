Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-acceptAddressTransfer ec2.op-contract-acceptAddressTransfer-b ec2.op-contract-acceptCapacityReservationBillingOwnership ec2.op-contract-acceptCapacityReservationBillingOwnership-b ec2.op-contract-associate-capacity-reservation-billing-owner ec2.op-contract-attachVerifiedAccessTrustProvider ec2.op-contract-delete-ipam-scope ec2.op-contract-delete-route-server-endpoint ec2.op-contract-import-snapshot ec2.op-contract-modify-application-status-check ec2.op-contract-modify-capacity-reservation-state-conflict ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected ec2.op-contract-disassociate-capacity-reservation-billing-owner ec2.op-contract-get-capacity-manager-metric-data-dup ec2.op-contract-delete-local-gateway-virtual-interface-group ec2.associate-trunk-interface-non-trunk-eni-rejected ec2.delete-transit-gateway-dry-run-does-not-delete-rejected ec2.op-contract-acceptVpcEndpointConnections ec2.op-contract-acceptVpcEndpointConnections-b ec2.op-contract-delete-fpga-image-rejected ec2.op-contract-delete-image-usage-report ec2.op-contract-describeCapacityBlockExtensionOfferings
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-acceptAddressTransfer|run#7|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptAddressTransfer-b|run#7|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptCapacityReservationBillingOwnership|run#7|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-acceptCapacityReservationBillingOwnership-b|run#9|AssertionError|assert info["code"] == "DryRunOperation", f"expected DryRunOperation, got {info['code']!r}"
Issue-Key: ec2.op-contract-associate-capacity-reservation-billing-owner|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"unexpected dry-run code: {dry_run_code}"
Issue-Key: ec2.op-contract-attachVerifiedAccessTrustProvider|run#9|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.op-contract-delete-ipam-scope|run#13|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.op-contract-delete-route-server-endpoint|run#16|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.op-contract-import-snapshot|run#10|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.op-contract-modify-application-status-check|run#10|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.op-contract-modify-capacity-reservation-state-conflict|run#11|AssertionError|assert dry_code == "DryRunOperation", f"expected DryRunOperation, got {dry_code!r}"
Issue-Key: ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected|run#8|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code}"
Issue-Key: ec2.op-contract-disassociate-capacity-reservation-billing-owner|run#7|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.op-contract-get-capacity-manager-metric-data-dup|run#7|AssertionError|assert code == "DryRunOperation", f"expected DryRunOperation, got {code!r}"
Issue-Key: ec2.op-contract-delete-local-gateway-virtual-interface-group|run#6|AssertionError|assert dryrun_code == "DryRunOperation", (
Issue-Key: ec2.associate-trunk-interface-non-trunk-eni-rejected|run#14|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.delete-transit-gateway-dry-run-does-not-delete-rejected|run#8|AssertionError|assert code == "DryRunOperation", f"unexpected error code for DryRun delete: {code}"
Issue-Key: ec2.op-contract-acceptVpcEndpointConnections|run#6|AssertionError|assert info["code"] == "InvalidVpcEndpointServiceId.NotFound", (
Issue-Key: ec2.op-contract-acceptVpcEndpointConnections-b|run#5|AssertionError|assert info["code"] == "InvalidVpcEndpointServiceId.NotFound", (
Issue-Key: ec2.op-contract-delete-fpga-image-rejected|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-delete-image-usage-report|run#4|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Key: ec2.op-contract-describeCapacityBlockExtensionOfferings|run#5|AssertionError|assert dry_run_code == "DryRunOperation", f"expected DryRunOperation, got {dry_run_code!r}"
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# EC2 DryRun contract: authorized DryRun requests answer NotImplemented instead of 412 DryRunOperation, and the DryRun check is sequenced wrongly against id/existence validation (3 causes)

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AcceptAddressTransfer

## Description
Three distinct root causes in how the EC2 crate implements the modelled `DryRun` member. All three are in the DryRun pre-dispatch gate, not in any one operation's business logic.

Cause 1 of 3: an authorized DryRun=true request on an operation whose handler is a scaffold answers the scaffold's NotImplemented instead of the DryRun refusal.
AWS: Smithy (services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json) models `DryRun: Boolean` on the input of each of these operations (confirmed for AcceptAddressTransfer.DryRun, AcceptVpcEndpointConnections.DryRun, DescribeManagedPrefixLists.DryRun, EnableVolumeIO.DryRun, DescribeVpcBlockPublicAccessExclusions.DryRun). EC2 evaluates DryRun ahead of dispatching the request: when the caller IS authorized it answers HTTP 412 with ec2Query error code `DryRunOperation` and the fixed message `Request would have succeeded, but DryRun flag is set.` (no variable parts), and when the caller is NOT authorized it answers `UnauthorizedOperation`. The request is never executed either way.
Emulator: the DryRun gate is not reached at all; the request falls through to the operation handler, which answers code `NotImplemented` with message `operation <OperationName> is scaffolded but not implemented in this emulator`.
Why: DryRun is the only way a client can probe permissions and request well-formedness without side effects. A client that uses it (CDK/CloudFormation preflight, CLI --dry-run, permission probes) cannot distinguish "you may do this" from "this emulator cannot do this", so every preflight path reports a hard failure and the caller aborts.
Fix: the EC2 request pipeline's DryRun pre-dispatch gate (it must run before handler dispatch, including for scaffolded handlers), not the individual op handlers.
Traces: ec2.op-contract-acceptAddressTransfer, ec2.op-contract-acceptAddressTransfer-b, ec2.op-contract-acceptCapacityReservationBillingOwnership, ec2.op-contract-acceptCapacityReservationBillingOwnership-b, ec2.op-contract-associate-capacity-reservation-billing-owner, ec2.op-contract-attachVerifiedAccessTrustProvider, ec2.op-contract-delete-ipam-scope, ec2.op-contract-delete-route-server-endpoint, ec2.op-contract-import-snapshot, ec2.op-contract-modify-application-status-check, ec2.op-contract-modify-capacity-reservation-state-conflict, ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected, ec2.op-contract-disassociate-capacity-reservation-billing-owner, ec2.op-contract-get-capacity-manager-metric-data-dup, ec2.op-contract-delete-local-gateway-virtual-interface-group, ec2.associate-trunk-interface-non-trunk-eni-rejected, ec2.delete-transit-gateway-dry-run-does-not-delete-rejected.

Cause 2 of 3: for operations where AWS validates the referenced resource id BEFORE the DryRun permission check, the emulator answers DryRunOperation and never validates.
AWS: recorded behavior only. AcceptVpcEndpointConnections with a DryRun=true request naming a service id that does not exist is refused with `InvalidVpcEndpointServiceId.NotFound` — the id resolution precedes the DryRun short-circuit.
Emulator: answers `DryRunOperation` / `Request would have succeeded, but DryRun flag is set.`, so a request that cannot succeed is reported as one that would have.
Why: a client using DryRun as a validity probe is told a request naming a nonexistent resource is valid, and only discovers the failure on the real call.
Fix: the per-operation ordering of the id-resolution step relative to the shared DryRun gate, in those operations' handlers/validation.
Traces: ec2.op-contract-acceptVpcEndpointConnections, ec2.op-contract-acceptVpcEndpointConnections-b.

Cause 3 of 3: the mirror-image ordering defect — for operations where AWS short-circuits on DryRun before id-shape validation, the emulator validates the id first and answers a Malformed/NotFound code.
AWS: recorded behavior only. DeleteFpgaImage, DeleteImageUsageReport and DescribeCapacityBlockExtensionOfferings with DryRun=true answer `DryRunOperation` with the fixed message `Request would have succeeded, but DryRun flag is set.` even when the id in the request is malformed or unknown.
Emulator: answers `InvalidFpgaImageID.Malformed`, `InvalidImageUsageReportId.Malformed` and `InvalidCapacityReservationId.NotFound` respectively — the id check runs first.
Why: callers that probe permission with a placeholder id (the documented idiom for a pure permission probe) get a parameter error instead of the permission verdict they asked for.
Fix: same DryRun gate, ordering table per operation.
Traces: ec2.op-contract-delete-fpga-image-rejected, ec2.op-contract-delete-image-usage-report, ec2.op-contract-describeCapacityBlockExtensionOfferings.

## Evidence
Cause 1 — expected: HTTP 412, code `DryRunOperation`, message `Request would have succeeded, but DryRun flag is set.` (entirely fixed text). Actual: code `NotImplemented`, message `operation AcceptAddressTransfer is scaffolded but not implemented in this emulator` (the operation name is the only variable part of the emulator's own wording). Replay lines: `AssertionError: expected DryRunOperation, got 'NotImplemented'` (ec2.op-contract-acceptAddressTransfer, -b, -acceptCapacityReservationBillingOwnership, -b, -disassociate-capacity-reservation-billing-owner, -get-capacity-manager-metric-data-dup, -import-snapshot, -delete-route-server-endpoint); `AssertionError: unexpected dry-run code: NotImplemented` (ec2.op-contract-associate-capacity-reservation-billing-owner); `AssertionError: expected DryRunOperation, got NotImplemented` (ec2.op-contract-attachVerifiedAccessTrustProvider, -delete-ipam-scope, -modify-application-status-check, -modify-capacity-reservation-state-conflict, -reject-capacity-reservation-billing-ownership-rejected); `AssertionError: expected DryRunOperation (permission-check must precede existence-check), got NotImplemented` (ec2.op-contract-delete-local-gateway-virtual-interface-group); `AssertionError: unexpected error code for DryRun delete: NotImplemented` (ec2.delete-transit-gateway-dry-run-does-not-delete-rejected).

Cause 2 — expected code `InvalidVpcEndpointServiceId.NotFound`; actual `DryRunOperation`. Replay line: `AssertionError: expected InvalidVpcEndpointServiceId.NotFound, got 'DryRunOperation'` (both ec2.op-contract-acceptVpcEndpointConnections and -b).

Cause 3 — expected code `DryRunOperation`; actual `InvalidFpgaImageID.Malformed` / `InvalidImageUsageReportId.Malformed` / `InvalidCapacityReservationId.NotFound`. Replay lines: `AssertionError: expected DryRunOperation, got 'InvalidFpgaImageID.Malformed'`, `... got 'InvalidImageUsageReportId.Malformed'`, `... got 'InvalidCapacityReservationId.NotFound'`.

## Steps to Reproduce
1. Boot the emulator and point an EC2 client at it with credentials the emulator accepts.
2. Cause 1: call AcceptAddressTransfer (or any op listed in Traces) with DryRun=true and otherwise well-formed input. Observe code NotImplemented instead of HTTP 412 / DryRunOperation / 'Request would have succeeded, but DryRun flag is set.'
3. Cause 2: call AcceptVpcEndpointConnections with DryRun=true, a ServiceId that was never created and any VpcEndpointIds. Observe DryRunOperation instead of InvalidVpcEndpointServiceId.NotFound.
4. Cause 3: call DeleteFpgaImage with DryRun=true and a malformed FpgaImageId (e.g. 'afi-bogus'). Observe InvalidFpgaImageID.Malformed instead of DryRunOperation.
5. Re-replay the check ids in trace_ids to confirm.
