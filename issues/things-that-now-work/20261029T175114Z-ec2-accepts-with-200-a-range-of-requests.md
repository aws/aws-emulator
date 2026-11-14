Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.associate-route-server-already-associated-rejected ec2.asvpce-private-dns-vpce-without-dns-hostnames-rejected ec2.create-route-server-endpoint-subnet-outside-vpc-rejected ec2.op-contract-create-transit-gateway-prefix-list-reference ec2.op-contract-associate-enclave-certificate-iam-role ec2.op-contract-create-replace-root-volume-task ec2.op-contract-delete-instance-connect-endpoint ec2.op-contract-delete-secondary-network ec2.op-contract-delete-vpn-gateway ec2.op-contract-request-spot-fleet ec2.op-contract-reset-address-attribute ec2.op-contract-disable-address-transfer ec2.op-contract-describeCapacityBlockOfferings ec2.op-contract-describeHostReservationOfferings ec2.op-contract-disable-capacity-manager ec2.op-contract-get-capacity-manager-attributes ec2.vpc-endpoint-gateway-create-reject-bad-service ec2.create-default-vpc-contract ec2.op-contract-cancel-capacity-reservation-fleets
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.associate-route-server-already-associated-rejected|_error_code#14|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ec2.asvpce-private-dns-vpce-without-dns-hostnames-rejected|_error#6|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: ec2.create-route-server-endpoint-subnet-outside-vpc-rejected|run#16|AssertionError|assert code == "InvalidParameterValue", f"expected InvalidParameterValue, got {code!r}"
Issue-Key: ec2.op-contract-create-transit-gateway-prefix-list-reference|run#7|AssertionError|assert code == "InvalidPrefixListID.NotFound", f"unexpected code: {code}"
Issue-Key: ec2.op-contract-associate-enclave-certificate-iam-role|_error_code#23|AssertionError|raise AssertionError(f"expected rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-create-replace-root-volume-task|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded: {result!r}")
Issue-Key: ec2.op-contract-delete-instance-connect-endpoint|_error_code#24|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-delete-secondary-network|_client_error#19|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-delete-vpn-gateway|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: ec2.op-contract-request-spot-fleet|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: ec2.op-contract-reset-address-attribute|_error_code_and_message#5|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded with {result!r}")
Issue-Key: ec2.op-contract-disable-address-transfer|run#13|AssertionError|assert described == [], f"expected no transfer after disable, got {described}"
Issue-Key: ec2.op-contract-describeCapacityBlockOfferings|run#7|AssertionError|raise AssertionError(
Issue-Key: ec2.op-contract-describeHostReservationOfferings|_error#11|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-disable-capacity-manager|_error#2|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Key: ec2.op-contract-get-capacity-manager-attributes|run#9|AssertionError|assert attrs["CapacityManagerStatus"] == "enabled", attrs
Issue-Key: ec2.vpc-endpoint-gateway-create-reject-bad-service|_error#7|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: ec2.create-default-vpc-contract|run#3|AssertionError|assert len(before) == 1, (
Issue-Key: ec2.op-contract-cancel-capacity-reservation-fleets|run#17|AssertionError|raise AssertionError(
Issue-Commit: abd75e982d55cd9e91541cb3d125feb2d8edb60f

# EC2 accepts with 200 a range of requests AWS refuses: no admission validation on ten create/delete/modify paths (10 causes)

> _Note: this issue was identified against the code base at commit `abd75e982d55cd9e91541cb3d125feb2d8edb60f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateRouteServer

## Description
Ten root causes where the emulator answers HTTP 200 (or 200 with an unchanged state) for a request AWS refuses. These are the most damaging class: the client believes a resource exists or a change landed.

Cause 1 of 10: AssociateRouteServer on a VPC already associated with a route server succeeds.
AWS: recorded behavior only — the second association is refused (a conflict code; the recording shows the first association leaves the VPC associated and a second attempt cannot succeed). Emulator: returns 200 with RouteServerAssociation State='associating' for the duplicate. Why: the caller ends up believing two associations exist on one VPC. Fix: AssociateRouteServer's handler must test the VPC's existing association before mutating state. Traces: ec2.associate-route-server-already-associated-rejected.

Cause 2 of 10: CreateVpcEndpoint with PrivateDnsEnabled=true succeeds in a VPC that has DNS hostnames disabled. AWS refuses it. Emulator: 200. Why: private DNS silently does not work and the caller has no signal. Fix: CreateVpcEndpoint's validation must read the VPC's enableDnsHostnames/enableDnsSupport attributes. Traces: ec2.asvpce-private-dns-vpce-without-dns-hostnames-rejected.

Cause 3 of 10: CreateRouteServerEndpoint accepts a SubnetId that is not in the route server's VPC. AWS refuses with `InvalidParameterValue`. Emulator: no error at all. Fix: CreateRouteServerEndpoint's cross-reference validation. Traces: ec2.create-route-server-endpoint-subnet-outside-vpc-rejected.

Cause 4 of 10: CreateTransitGatewayPrefixListReference succeeds where AWS refuses (the check observed no error code at all). Fix: that handler's referential validation. Traces: ec2.op-contract-create-transit-gateway-prefix-list-reference.

Cause 5 of 10: AssociateEnclaveCertificateIamRole succeeds for a request AWS refuses, and the success body is malformed: CertificateS3ObjectKey is returned as a concatenation of the role ARN and the certificate ARN separated by '/'. AWS returns an S3 object key, not two ARNs joined. Fix: that handler's admission check and its response construction. Traces: ec2.op-contract-associate-enclave-certificate-iam-role.

Cause 6 of 10: lifecycle guards missing on delete/modify. CreateReplaceRootVolumeTask succeeds where AWS refuses; DeleteInstanceConnectEndpoint succeeds against an endpoint already in a delete state (and returns State='delete-complete' for the repeat); DeleteSecondaryNetwork likewise returns 200 with State='delete-complete' on a resource AWS refuses to delete again; DeleteVpnGateway returns a bare 200 where AWS refuses. Why: idempotent-looking 200s hide a resource that is not in a deletable state, so dependency ordering in teardown scripts silently breaks. Fix: the delete handlers' state guards. Traces: ec2.op-contract-create-replace-root-volume-task, ec2.op-contract-delete-instance-connect-endpoint, ec2.op-contract-delete-secondary-network, ec2.op-contract-delete-vpn-gateway.

Cause 7 of 10: RequestSpotFleet accepts a request AWS refuses and mints a SpotFleetRequestId. Fix: RequestSpotFleet's launch-specification validation. Traces: ec2.op-contract-request-spot-fleet.

Cause 8 of 10: the address-transfer plane neither refuses nor applies. ResetAddressAttribute succeeds where AWS refuses (returning PtrRecordUpdate Status='PENDING'); DisableAddressTransfer returns 200 but DescribeAddressTransfers still lists the transfer afterwards with AddressTransferStatus='disabled' instead of dropping it. Why: a caller that disables a transfer and then re-reads still sees an offer, including TransferAccountId and an expiry. Fix: the address-transfer handlers' validation and their state mutation on disable. Traces: ec2.op-contract-reset-address-attribute, ec2.op-contract-disable-address-transfer.

Cause 9 of 10: selector-required and account-state-gated reads answer 200 instead of refusing. DescribeCapacityBlockOfferings with CapacityDurationHours and NO instance/ultraserver selector succeeds (AWS requires a selector); DescribeHostReservationOfferings answers 200 with None for a request AWS refuses; DisableCapacityManager and GetCapacityManagerAttributes answer 200 with CapacityManagerStatus='disabled' for an account where Capacity Manager was never enabled, instead of AWS's `CapacityManager.Disabled` refusal. Fix: those handlers' required-parameter and account-state gates. Traces: ec2.op-contract-describeCapacityBlockOfferings, ec2.op-contract-describeHostReservationOfferings, ec2.op-contract-disable-capacity-manager, ec2.op-contract-get-capacity-manager-attributes.

Cause 10 of 10: service-name and account-baseline invariants are not enforced. CreateVpcEndpoint accepts ServiceName='com.amazonaws.<region>.not-a-real-service' and returns a Gateway endpoint in 'pending'; and the account baseline holds TWO default VPCs in one region (DescribeVpcs returned both vpc-...001 and vpc-...022 as default), where AWS holds at most one default VPC per region. CancelCapacityReservationFleets with a well-formed but nonexistent fleet id does not raise at all. Fix: CreateVpcEndpoint's service-catalogue lookup, the default-VPC seeding in the EC2 state bootstrap, and CancelCapacityReservationFleets' id resolution. Traces: ec2.vpc-endpoint-gateway-create-reject-bad-service, ec2.create-default-vpc-contract, ec2.op-contract-cancel-capacity-reservation-fleets.

## Evidence
Verbatim replay diagnosis lines. Cause 1: `AssertionError: expected rejection, but the call SUCCEEDED and returned {'RouteServerAssociation': {'RouteServerId': 'rs-...', 'VpcId': 'vpc-...', 'State': 'associating'}, ... 'HTTPStatusCode': 200 ...}`. Cause 2: `AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error`. Cause 3: `AssertionError: expected InvalidParameterValue, got None`. Cause 4: `AssertionError: unexpected code: None`. Cause 5: `AssertionError: expected rejection, but the call SUCCEEDED and returned {'CertificateS3BucketName': 'aws-ec2-enclave-certificate-us-west-2-000000000000', 'CertificateS3ObjectKey': 'arn:aws:iam::000000000000:role/.../arn:aws:acm:us-west-2:...:certificate/...', ...}`. Cause 6: `expected a ClientError, but the call succeeded: {'ReplaceRootVolumeTask': {... 'TaskState': 'pending' ...}}`; `expected the call to be rejected, but it SUCCEEDED and returned {'InstanceConnectEndpoint': {... 'State': 'delete-complete' ...}}`; `expected a ClientError, but the call SUCCEEDED and returned {'SecondaryNetwork': {... 'State': 'delete-complete' ...}}`; `expected the call to be rejected, but it succeeded: {'ResponseMetadata': {... 'HTTPStatusCode': 200 ...}}`. Cause 7: `expected a ClientError, but the call succeeded with {'SpotFleetRequestId': 'sfr-...'}`. Cause 8: `expected a ClientError, but the call succeeded with {'Address': {'PublicIp': '54.0.0.2', 'AllocationId': 'eipalloc-...', 'PtrRecordUpdate': {'Status': 'PENDING'}}}`; `expected no transfer after disable, got [{'PublicIp': '54.0.0.2', 'AllocationId': 'eipalloc-...', 'TransferAccountId': '111111111111', 'TransferOfferExpirationTimestamp': ..., 'AddressTransferStatus': 'disabled'}]`. Cause 9: `describe_capacity_block_offerings(CapacityDurationHours=24) with no instance/ultraserver selector unexpectedly succeeded`; `expected a rejection, but the call SUCCEEDED and returned None`; `expected a rejection, but the call SUCCEEDED and returned {'CapacityManagerStatus': 'disabled', 'OrganizationsAccess': False, ...}`; `AssertionError: {'CapacityManagerStatus': 'disabled', 'OrganizationsAccess': False, 'DataExportCount': 0, ...}`; the sibling check records the required code as `CapacityManager.Disabled`. Cause 10: `expected this call to be rejected, but it SUCCEEDED: {'VpcEndpoint': {... 'ServiceName': 'com.amazonaws.us-west-2.not-a-real-service', 'State': 'pending' ...}}`; `the account baseline holds exactly one default VPC per region, but describe_vpcs found ['vpc-00000000000000001', 'vpc-00000000000000022']`; `expected cancel_capacity_reservation_fleets('crf-...') to raise InvalidCapacityReservationFleetId.Malformed or .NotFound, but it did not raise`. The ids, IPs and account numbers above are per-run values; the requirement is the refusal itself and its code.

## Steps to Reproduce
1. Boot the emulator; point an EC2 client at it.
2. Cause 1: CreateRouteServer, AssociateRouteServer(vpc), then AssociateRouteServer(same vpc) — second call returns 200.
3. Cause 2: CreateVpc with DNS hostnames disabled, then CreateVpcEndpoint(Interface, PrivateDnsEnabled=true) — returns 200.
4. Cause 3: CreateRouteServerEndpoint naming a subnet in a different VPC — returns 200.
5. Cause 6: DeleteInstanceConnectEndpoint twice; DeleteSecondaryNetwork twice; DeleteVpnGateway on a gateway AWS refuses to delete.
6. Cause 9: DescribeCapacityBlockOfferings(CapacityDurationHours=24) with no instance type; DisableCapacityManager / GetCapacityManagerAttributes on a fresh account.
7. Cause 10: CreateVpcEndpoint with ServiceName='com.amazonaws.us-west-2.not-a-real-service'; DescribeVpcs(Filters=[{isDefault:true}]) and count the default VPCs.
8. Re-replay the check ids in trace_ids.
