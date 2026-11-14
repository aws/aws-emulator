Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork-b ec2.op-contract-modify-client-vpn-endpoint ec2.op-contract-associate-capacity-reservation-billing-owner ec2.op-contract-disassociate-capacity-reservation-billing-owner ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected ec2.op-contract-acceptCapacityReservationBillingOwnership ec2.op-contract-purchase-reserved-instances-offering-rejected ec2.op-contract-get-capacity-manager-metric-data-dup ec2.op-contract-get-capacity-manager-attributes ec2.op-contract-request-spot-fleet
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork|run#12|AssertionError|assert info2["code"] == "InvalidClientVpnEndpointId.NotFound", (
Issue-Key: ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork-b|run#15|AssertionError|assert info2["code"] == "InvalidClientVpnEndpointId.NotFound", (
Issue-Key: ec2.op-contract-modify-client-vpn-endpoint|run#19|AssertionError|assert notfound_code == "InvalidClientVpnEndpointId.NotFound", (
Issue-Key: ec2.op-contract-associate-capacity-reservation-billing-owner|run#8|AssertionError|assert code == "InvalidCapacityReservationId.Malformed", f"unexpected error code: {code}"
Issue-Key: ec2.op-contract-disassociate-capacity-reservation-billing-owner|run#11|AssertionError|assert code2 == "InvalidCapacityReservationId.NotFound", f"unexpected code: {code2!r}"
Issue-Key: ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected|run#12|AssertionError|assert code == "InvalidCapacityReservationId.Malformed", f"unexpected code: {code}"
Issue-Key: ec2.op-contract-acceptCapacityReservationBillingOwnership|run#11|AssertionError|assert info2["code"] == "InvalidState", f"expected InvalidState, got {info2['code']!r}"
Issue-Key: ec2.op-contract-purchase-reserved-instances-offering-rejected|run#8|AssertionError|assert err.get("Code") == "InvalidParameterValue", (
Issue-Key: ec2.op-contract-get-capacity-manager-metric-data-dup|run#10|AssertionError|assert code == "CapacityManager.DataNotAvailable", (
Issue-Key: ec2.op-contract-get-capacity-manager-attributes|run#9|AssertionError|assert attrs["CapacityManagerStatus"] == "enabled", attrs
Issue-Key: ec2.op-contract-request-spot-fleet|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Commit: 965b3053fd6a88603311dd441a6858d4fb538678

# EC2: 10 net-new causes — Client VPN, Capacity Reservation billing-ownership, Reserved Instances, Capacity Manager and Spot Fleet refusal paths answer NotImplemented, the wrong code, or accept what AWS refuses

> _Note: this issue was identified against the code base at commit `965b3053fd6a88603311dd441a6858d4fb538678`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplySecurityGroupsToClientVpnTargetNetwork

## Description
Ten net-new root causes, all in the ec2 control plane. Each is a distinct operation's refusal/acceptance contract and must be closed separately. Note on the model: the EC2 Smithy model declares NO modeled errors for any of these operations (ec2Query carries error codes at the protocol level), so each requirement below is AWS's documented/recorded refusal for that call.

Cause 1 of 10: ApplySecurityGroupsToClientVpnTargetNetwork answers 501 NotImplemented for an unknown endpoint id.
AWS: an unknown-but-well-formed ClientVpnEndpointId is refused with HTTP 400 and error code InvalidClientVpnEndpointId.NotFound, whose message names the requested endpoint id from the request. Smithy: ApplySecurityGroupsToClientVpnTargetNetwork requires ClientVpnEndpointId, VpcId and SecurityGroupIds; errors: [] (protocol-level).
Emulator: HTTP 501 with code NotImplemented.
Why: a client that handles InvalidClientVpnEndpointId.NotFound as a terminal client fault instead retries a 501 as a server fault, or aborts.
Fix: the ApplySecurityGroupsToClientVpnTargetNetwork handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/apply_security_groups_to_client_vpn_target_network.rs) — its existence check and refusal. An issue already filed for this file covers two OTHER checks it blocks; these two checks were not attributed to it.
Traces: ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork, ec2.op-contract-applySecurityGroupsToClientVpnTargetNetwork-b.

Cause 2 of 10: ModifyClientVpnEndpoint answers NotImplemented for an unknown endpoint id.
AWS: HTTP 400, InvalidClientVpnEndpointId.NotFound, message naming the requested endpoint id.
Emulator: code NotImplemented.
Why: same client consequence as cause 1 — a server-fault code for a client fault.
Fix: the ModifyClientVpnEndpoint handler's lookup and refusal.
Traces: ec2.op-contract-modify-client-vpn-endpoint.

Cause 3 of 10: AssociateCapacityReservationBillingOwner answers NotImplemented for a malformed reservation id.
AWS: HTTP 400, InvalidCapacityReservationId.Malformed, message quoting the malformed id from the request.
Emulator: code NotImplemented.
Why: id-shape validation is invisible to clients; a malformed id reads as an emulator outage.
Fix: the AssociateCapacityReservationBillingOwner handler's id validation.
Traces: ec2.op-contract-associate-capacity-reservation-billing-owner.

Cause 4 of 10: DisassociateCapacityReservationBillingOwner answers NotImplemented for an unknown reservation id.
AWS: HTTP 400, InvalidCapacityReservationId.NotFound naming the requested id; with DryRun on an owned reservation it answers DryRunOperation with "Request would have succeeded, but DryRun flag is set."
Emulator: code NotImplemented.
Fix: the DisassociateCapacityReservationBillingOwner handler.
Why: as cause 3.
Traces: ec2.op-contract-disassociate-capacity-reservation-billing-owner.

Cause 5 of 10: RejectCapacityReservationBillingOwnership answers NotImplemented for a malformed reservation id.
AWS: HTTP 400, InvalidCapacityReservationId.Malformed naming the malformed id.
Emulator: code NotImplemented.
Why: as cause 3.
Fix: the RejectCapacityReservationBillingOwnership handler's id validation.
Traces: ec2.op-contract-reject-capacity-reservation-billing-ownership-rejected.

Cause 6 of 10: AcceptCapacityReservationBillingOwnership treats a cancelled reservation as not found.
AWS: for a reservation that EXISTS but is in the cancelled state, the call is refused with HTTP 400, error code InvalidState and the message "You can't assign billing of a Capacity Reservation that is in the <state> state. Wait for the Capacity Reservation to reach the active state and then try again." — fixed text with the reservation's current state substituted from stored state. The DryRun leg (DryRunOperation, "Request would have succeeded, but DryRun flag is set.") already matches.
Emulator: answers InvalidCapacityReservationId.NotFound, i.e. a cancelled reservation is indistinguishable from one that never existed.
Why: a client cannot tell "wait/retry when active" from "this id does not exist", so it stops instead of waiting; state is also lost on cancel.
Fix: the AcceptCapacityReservationBillingOwnership handler's state check and the capacity-reservation state retained after CancelCapacityReservation.
Traces: ec2.op-contract-acceptCapacityReservationBillingOwnership.

Cause 7 of 10: PurchaseReservedInstancesOffering answers NotImplemented instead of a client refusal.
AWS: HTTP 400, InvalidParameterValue with the message naming the offending parameter value for an invalid purchase request.
Emulator: code NotImplemented.
Why: as cause 3.
Fix: the PurchaseReservedInstancesOffering handler's parameter validation.
Traces: ec2.op-contract-purchase-reserved-instances-offering-rejected.

Cause 8 of 10: GetCapacityManagerMetricData answers NotImplemented instead of CapacityManager.DataNotAvailable.
AWS: when Capacity Manager is enabled but has not ingested data for the requested window, the call is refused with HTTP 400, error code CapacityManager.DataNotAvailable and the message "Capacity Manager data isn't available yet. Try again later." (entirely fixed text), and no datapoint is reported.
Emulator: code NotImplemented.
Why: a client that polls until data is available treats the 501 as a broken endpoint.
Fix: the GetCapacityManagerMetricData handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/get_capacity_manager_metric_data.rs). An issue is already filed for that file for two OTHER checks it blocks; this check was not attributed to it.
Traces: ec2.op-contract-get-capacity-manager-metric-data-dup.

Cause 9 of 10: EnableCapacityManager does not change what GetCapacityManagerAttributes reports.
AWS: while Capacity Manager is off, GetCapacityManagerAttributes is refused with CapacityManager.Disabled and "Capacity Manager isn't enabled for this account."; after EnableCapacityManager(OrganizationsAccess=false) it succeeds with CapacityManagerStatus="enabled", OrganizationsAccess=false, an integer DataExportCount and an IngestionStatus of initial-ingestion-in-progress / ingestion-complete / ingestion-failed.
Emulator: EnableCapacityManager succeeds, but GetCapacityManagerAttributes keeps answering 200 with CapacityManagerStatus="disabled", OrganizationsAccess=false, DataExportCount=0 and NO IngestionStatus member — the enablement is never stored and the disabled branch is never a refusal.
Why: a client enabling the feature and polling for "enabled" never proceeds; one that keys off the Disabled refusal never sees it either.
Fix: the account-level Capacity Manager state written by EnableCapacityManager/DisableCapacityManager and read by GetCapacityManagerAttributes, plus that read's refusal when disabled.
Traces: ec2.op-contract-get-capacity-manager-attributes.

Cause 10 of 10: RequestSpotFleet accepts a config naming a nonexistent IAM fleet role.
AWS: a SpotFleetRequestConfig whose IamFleetRole ARN does not exist is refused with HTTP 400, error code InvalidSpotFleetRequestConfig and the message "Parameter: SpotFleetRequestConfig.IamFleetRole is invalid." (fixed text), and no fleet request is created.
Emulator: returns HTTP 200 with a new SpotFleetRequestId, so an unusable fleet request is created and leaks into DescribeSpotFleetRequests.
Why: a client's negative path never fires; the fleet is reported as requested although no role can ever assume it.
Fix: the RequestSpotFleet handler's validation of SpotFleetRequestConfig.IamFleetRole against IAM, before any request id is minted.
Traces: ec2.op-contract-request-spot-fleet.

## Evidence
Cause 1 — AssertionError: expected InvalidClientVpnEndpointId.NotFound, got 'NotImplemented' (both checks).
Cause 2 — AssertionError: unexpected error code: 'NotImplemented' (expected InvalidClientVpnEndpointId.NotFound).
Cause 3 — AssertionError: unexpected error code: NotImplemented (expected InvalidCapacityReservationId.Malformed).
Cause 4 — AssertionError: unexpected code: 'NotImplemented' (expected InvalidCapacityReservationId.NotFound).
Cause 5 — AssertionError: unexpected code: NotImplemented (expected InvalidCapacityReservationId.Malformed).
Cause 6 — AssertionError: expected InvalidState, got 'InvalidCapacityReservationId.NotFound' after CancelCapacityReservation on the reservation just created; the expected message is "You can't assign billing of a Capacity Reservation that is in the cancelled state. Wait for the Capacity Reservation to reach the active state and then try again."
Cause 7 — AssertionError: expected InvalidParameterValue, got 'NotImplemented'.
Cause 8 — AssertionError: expected CapacityManager.DataNotAvailable with no datapoint reported, got 'NotImplemented'.
Cause 9 — AssertionError: {'CapacityManagerStatus': 'disabled', 'OrganizationsAccess': False, 'DataExportCount': 0, ...} with HTTPStatusCode 200, after enable_capacity_manager(OrganizationsAccess=False) and a 120 s poll for status 'enabled'; no IngestionStatus member present.
Cause 10 — AssertionError: expected a ClientError, but the call succeeded with {'SpotFleetRequestId': 'sfr-...', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for SpotFleetRequestConfig.IamFleetRole = a nonexistent role ARN.

## Steps to Reproduce
1. ApplySecurityGroupsToClientVpnTargetNetwork / ModifyClientVpnEndpoint with a well-formed but unknown cvpn-endpoint id.
2. AssociateCapacityReservationBillingOwner and RejectCapacityReservationBillingOwnership with a malformed CapacityReservationId; DisassociateCapacityReservationBillingOwner with an unknown one.
3. CreateCapacityReservation(t3.micro, Linux/UNIX, InstanceCount=1, EndDateType=unlimited), CancelCapacityReservation on it, then AcceptCapacityReservationBillingOwnership on the same id.
4. PurchaseReservedInstancesOffering with an invalid offering/parameter.
5. EnableCapacityManager(OrganizationsAccess=False), then GetCapacityManagerAttributes (poll for CapacityManagerStatus=='enabled') and GetCapacityManagerMetricData for a window with no data.
6. RequestSpotFleet with a valid config, then the same config with IamFleetRole set to a nonexistent role ARN; scan DescribeSpotFleetRequests for that role.
Replay the trace_ids listed.
